//! The guild session: the identity cache, the roster, the ranks and the outbound verbs.
//! [`GuildState`] mirrors the guild packets and [`feed`] turns it into the snapshot the guild
//! windows read. The roster carries no guild or rank names: those come only from
//! `SMSG_GUILD_QUERY_RESPONSE`, cached by guild id and filled lazily, as in the reference.

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::{
    guild_event, GuildCommandResult, GuildEventNotice, GuildInfo, GuildQueryResponse, GuildRoster,
    GuildRosterMember, GUILD_RANKS_MAX_COUNT,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{LastOnline, UnitGuild};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiDrain, UiFeed};

mod event_log;
mod feed;
mod lines;
mod sort;

pub(crate) use sort::RosterRow;
use sort::{SortField, SortStack};

/// `GuildRoster()`'s silence after a request: the reference's 10 000 ms against. The arg1 edge of
/// [`RosterUpdate`] clears it.
const ROSTER_REQUEST_THROTTLE_SECS: f64 = 10.0;

/// One guild's identity, from `SMSG_GUILD_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Identity {
    /// Empty means no such guild, as the reference reads it (cache insert or remove); cached as a
    /// negative so the query is not re-sent.
    name: String,
    /// The ten rank names, index 0 = guild master; empty past the guild's real rank count.
    rank_names: [String; GUILD_RANKS_MAX_COUNT],
    /// The tabard's five emblem indices for the body composite: the reference reads this record
    /// from its character compositor as well as from `GetGuildInfo`.
    emblem: GuildEmblem,
}

impl Identity {
    /// The name of rank `index`, or `""`. Deviation: bounded at the ten slots, because the
    /// reference's read runs past its array.
    fn rank_name(&self, index: u32) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.rank_names.get(i))
            .map(String::as_str)
            .unwrap_or_default()
    }

    fn exists(&self) -> bool {
        !self.name.is_empty()
    }
}

/// Which `GUILD_ROSTER_UPDATE` the feed owes. A truthy arg1 makes the stock pane re-request the
/// roster (`FriendsFrame.lua:646-653`). The reference's fires it bare for a parsed roster, a
/// show-offline change, a re-sort and a landed guild record, and with arg1 only for
/// `SMSG_GUILD_EVENT` cases `0x00`-`0x0b` and two `SMSG_GUILD_COMMAND_RESULT` arms, clearing the
/// `GuildRoster()` throttle first. arg1 on a parsed roster would re-request forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterUpdate {
    /// Repaint: a roster just applied, a local re-order, a late identity.
    Applied,
    /// The roster is stale: re-request it.
    Stale,
}

impl RosterUpdate {
    /// Whether this edge carries arg1.
    fn arg1(self) -> bool {
        self == RosterUpdate::Stale
    }
}

/// The `guildMemberNotify` CVar, Guild Member Alert on the Chat options page. Off by default: its
/// register site pushes `"0"`. Kept apart from [`GuildState`], which is cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct GuildMemberNotify(pub(crate) bool);

/// The guild session mirror: filled by the packet handlers, read by the feed, reset on disconnect.
#[derive(Resource, Default)]
pub(crate) struct GuildState {
    /// Our `PLAYER_GUILDID`, `0` when guildless; `IsInGuild` reads it, not the roster.
    guild_id: u32,
    /// Our `PLAYER_GUILDRANK`, 0-based, `0` = guild master.
    rank_index: u32,
    /// Apart from the roster: `GE_MOTD` sets it alone, and at login vmangos sends that event before
    /// any roster (`CharacterHandler.cpp:558`).
    motd: String,
    /// The guild information text (`GuildInfoFrame`), carried only by the roster.
    info_text: String,
    /// One rights word per rank id; its length is the guild's real rank count.
    rank_rights: Vec<u32>,
    /// Each rank's guild bank rights, by rank id, as long as [`Self::rank_rights`].
    rank_bank: Vec<benilla_protocol::messages::GuildRankBank>,
    /// The members, in wire order.
    members: Vec<GuildRosterMember>,
    sort: SortStack,
    /// `SetGuildRosterShowOffline`, off at start like the reference's; the FrameXML restores the
    /// saved value at `VARIABLES_LOADED`.
    show_offline: bool,
    /// The selected member as a guid, `0` for none, as `SetGuildRosterSelection` stores it: a
    /// re-sort keeps the same player selected.
    selection: u64,
    /// Member guids in the order last shown, mapping a Lua row index back to its player.
    display_order: Vec<u64>,
    /// The invitations not yet signalled, `(inviter, guild)`: every packet fires its own
    /// `GUILD_INVITE_REQUEST` and nothing is held after it (`0x6cc3e5`).
    pending_invites: Vec<(String, String)>,
    roster_event: Option<RosterUpdate>,
    /// Real-time seconds before which `GuildRoster()` is swallowed; `0.0` = allowed now.
    roster_allowed_at: f64,
    /// The pushed snapshot is stale; the feed rebuilds only when set.
    dirty: bool,
    /// `/ginfo` answers for [`feed`], which builds their lines where the VM is: their templates are
    /// not catalog rows, so they cannot ride [`crate::ui_action::UiError`].
    pending_info: Vec<GuildInfo>,
    /// The permissions query owes the server `MSG_GUILD_BANK_MONEY_WITHDRAWN`'s ask.
    bank_withdraw_ask: bool,
    /// A guild or rank change owes the permissions query (`0x5caa40`): `MSG_GUILD_PERMISSIONS`,
    /// then the withdrawal ask.
    permissions_ask: bool,
}

/// The guild cache (`0xc5d9c0`): each guild's record by id, asked once, negatives included; keyed by id because `PLAYER_GUILDID` is public, so any visible player's guild is nameable ([`unit_guild`]). Apart from [`GuildState`], as 12340 keeps it apart from the roster, so the verbs read it at the call.
#[derive(Resource, Default)]
pub(crate) struct GuildCache {
    identities: QueryCache<u32, Identity>,
    /// Bumped when an identity lands or is evicted, never by an ask; the unit feeds watch it.
    generation: u64,
    /// Each guild's last seen timestamp, the watch's table (`0xc9eb5c`).
    stamps: Vec<(u32, u32)>,
    /// The guilds asked by the timestamp watch, one entry per ask, whose answer runs its walk (`0x6e1b90`).
    stamp_asks: Vec<u32>,
}

impl crate::query_cache::AskOnce for GuildCache {
    fn clear_pending(&mut self) {
        self.identities.clear_pending();
    }
}

impl GuildCache {
    /// Send `CMSG_GUILD_QUERY` for a guild we hold nothing for, a negative counting as held.
    fn request(&self, guild_id: u32, commands: &NetCommands) {
        if guild_id != 0 {
            self.identities.get_or_ask(guild_id, || {
                let _ = commands.0.send(ClientCommand::GuildQuery { guild_id });
            });
        }
    }

    /// A guild's identity if we hold a real one; `None` while in flight and for no such guild.
    fn identity(&self, guild_id: u32) -> Option<&Identity> {
        self.identities.get(guild_id).filter(|i| i.exists())
    }

    /// The read that also asks: `None` on a miss, which sends the query, as the reference's
    /// `GetGuildRosterInfo` answers nil and queries inline.
    fn resolve(&self, guild_id: u32, commands: &NetCommands) -> Option<&Identity> {
        self.request(guild_id, commands);
        self.identity(guild_id)
    }

    /// A guild's designed crest read with no ask; `None` for guild 0, no record or an undesigned crest.
    pub(crate) fn cached_emblem(&self, guild_id: u32) -> Option<GuildEmblem> {
        let emblem = self.identity(guild_id)?.emblem;
        emblem.is_designed().then_some(emblem)
    }

    /// The landed-identity counter the unit feeds watch.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// A guild's record read with no ask, as `CanSaveTabardNow`, the seed and the save read it (`0x6de20c`; a miss with no callback asks nothing, `0x67d95d`): its five tabard fields, `-1`s when undesigned; `None` for guild 0 (`0x67d93c`) or no record.
    pub(crate) fn emblem_record(&self, guild_id: u32) -> Option<[i32; 5]> {
        if guild_id == 0 {
            return None;
        }
        let e = &self.identity(guild_id)?.emblem;
        Some([
            e.emblem_style,
            e.emblem_color,
            e.border_style,
            e.border_color,
            e.background_color,
        ])
    }

    /// A guild's record dropped with no ask (`0x67a5f0`); whether one was held. A pending one runs its callbacks unfound (`0x67a61f` -> `0x67a533`), which the timestamp walk skips (`0x6e1b93`), so its asks go.
    pub(crate) fn evict(&mut self, guild_id: u32) -> bool {
        self.stamp_asks.retain(|g| *g != guild_id);
        let evicted = self.identities.evict(guild_id);
        if evicted {
            self.generation = self.generation.wrapping_add(1);
        }
        evicted
    }

    /// The guild timestamp's watch (`0x6e1c60`): a stamp already held for the guild does nothing (`0x6e1ccc`); else it is stored, the record dropped and asked again with the walk's callback (`0x6e1d47`, `0x6e1d5f`).
    pub(crate) fn stamp_changed(&mut self, guild_id: u32, stamp: u32, commands: &NetCommands) {
        match self.stamps.iter_mut().find(|(g, _)| *g == guild_id) {
            Some((_, held)) if *held == stamp => return,
            Some((_, held)) => *held = stamp,
            None => self.stamps.push((guild_id, stamp)),
        }
        self.evict(guild_id);
        if guild_id != 0 {
            self.request(guild_id, commands);
            self.stamp_asks.push(guild_id);
        }
    }

    /// The timestamp watch's callbacks the answer for `guild_id` runs, taken in ask order.
    fn take_stamp_asks(&mut self, guild_id: u32) -> usize {
        let before = self.stamp_asks.len();
        self.stamp_asks.retain(|g| *g != guild_id);
        before - self.stamp_asks.len()
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`'s record, an empty name as a negative.
    fn insert(&mut self, response: &GuildQueryResponse) {
        self.identities.insert(
            response.guild_id,
            Some(Identity {
                name: response.name.clone(),
                rank_names: response.rank_names.clone(),
                emblem: GuildEmblem {
                    emblem_style: response.emblem_style,
                    emblem_color: response.emblem_color,
                    border_style: response.border_style,
                    border_color: response.border_color,
                    background_color: response.background_color,
                },
            }),
        );
        self.generation = self.generation.wrapping_add(1);
    }
}

impl GuildState {
    /// `IsInGuild`: our own `PLAYER_GUILDID` is set, known before any guild packet.
    pub(crate) fn in_guild(&self) -> bool {
        self.guild_id != 0
    }

    /// Mirror our descriptor's guild fields; `true` when either moved. Asks for nothing: the
    /// identity cache is lazy and the roster is the FrameXML's to request.
    fn mirror_self(&mut self, guild_id: u32, rank_index: u32) -> bool {
        if (self.guild_id, self.rank_index) == (guild_id, rank_index) {
            return false;
        }
        // A first 0 to N is learning our guild, not changing it: at login the MOTD event arrives
        // before our descriptor (`CharacterHandler.cpp:558`), and a wipe here would lose it. Only
        // a move out of a real guild clears the mirror.
        let known = self.guild_id != 0;
        let moved = self.guild_id != guild_id;
        self.guild_id = guild_id;
        self.rank_index = rank_index;
        if moved && known {
            // The old guild's roster goes; the identity cache is keyed by id and stays true.
            self.motd.clear();
            self.info_text.clear();
            self.rank_rights.clear();
            self.rank_bank.clear();
            self.members.clear();
            self.selection = 0;
        }
        if moved {
            self.note_roster_update(RosterUpdate::Applied);
        }
        // The guild id's and the rank's watchers (`0x6e1bb0`, `0x6d1ae0`) ask our rights again;
        // learning our guild at login stands for world entry's ask (`0x5cb900`).
        if guild_id != 0 {
            self.permissions_ask = true;
        }
        self.dirty = true;
        true
    }

    /// `MSG_GUILD_PERMISSIONS`' answer (`0x5cb9f0`): one rank's rights, then a bare
    /// `GUILD_ROSTER_UPDATE`.
    fn apply_permissions(&mut self, p: &benilla_protocol::messages::GuildPermissions) {
        let Ok(rank) = usize::try_from(p.rank) else {
            return;
        };
        if rank >= GUILD_RANKS_MAX_COUNT {
            return;
        }
        if self.rank_rights.len() <= rank {
            self.rank_rights.resize(rank + 1, 0);
        }
        if self.rank_bank.len() <= rank {
            self.rank_bank.resize(rank + 1, Default::default());
        }
        self.rank_rights[rank] = p.rights;
        self.rank_bank[rank] = p.bank;
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// Owe the VM a `GUILD_ROSTER_UPDATE`. [`RosterUpdate::Stale`] wins over `Applied` in one
    /// frame, since a re-request also repaints, and clears the request throttle.
    fn note_roster_update(&mut self, kind: RosterUpdate) {
        if kind.arg1() {
            self.roster_allowed_at = 0.0;
        }
        if kind.arg1() || self.roster_event.is_none() {
            self.roster_event = Some(kind);
        }
        self.dirty = true;
    }

    /// `SMSG_INVALIDATE_PLAYER` (`0x635400`): a guild guid drops that guild's record; a member's
    /// guid re-asks the roster at once and restarts the `GuildRoster()` throttle (`0x5cab49`).
    fn invalidate_player(
        &mut self,
        cache: &mut GuildCache,
        guid: u64,
        now: f64,
        commands: &NetCommands,
    ) {
        if benilla_protocol::guid::is_guild(guid) {
            if cache.evict(guid as u32) {
                self.dirty = true;
            }
        } else if self.members.iter().any(|m| m.guid == guid) {
            self.roster_allowed_at = now + ROSTER_REQUEST_THROTTLE_SECS;
            let _ = commands.0.send(ClientCommand::GuildRosterRequest);
        }
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`: fill the identity cache, an empty name as a negative.
    fn apply_query_response(&mut self, cache: &mut GuildCache, response: &GuildQueryResponse) {
        cache.insert(response);
        self.dirty = true;
        if response.guild_id == self.guild_id {
            // Our rank names landed: repaint, never re-request, as the reference's cache callback
            // does.
            self.note_roster_update(RosterUpdate::Applied);
        }
    }

    /// `SMSG_GUILD_ROSTER`: a complete snapshot, never a delta.
    fn apply_roster(&mut self, roster: GuildRoster) {
        self.motd = roster.motd;
        self.info_text = roster.info;
        // Deviation: clamped to ten, because the reference's unbounded loop overruns its ten-slot
        // array on a hostile `rankCount >= 12`.
        self.rank_rights = roster.rank_rights;
        self.rank_rights.truncate(GUILD_RANKS_MAX_COUNT);
        self.rank_bank = roster.rank_bank;
        self.rank_bank.truncate(GUILD_RANKS_MAX_COUNT);
        self.members = roster.members;
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// `SMSG_GUILD_EVENT`'s effect on the mirror; its line is [`lines`]'.
    fn apply_event(&mut self, cache: &mut GuildCache, notice: &GuildEventNotice) {
        match notice.event {
            guild_event::MOTD => {
                // Stored so `GetGuildRosterMOTD` agrees with the event before the next roster.
                self.motd = notice.params.first().cloned().unwrap_or_default();
                self.dirty = true;
            }
            guild_event::UPDATE_RANK_NAME => {
                // Written into the cached record with no event of its own; params are the rank id
                // as text, then the name (vmangos `Guild/Guild.h:133`).
                let rank = notice.params.first().and_then(|p| p.parse::<usize>().ok());
                let name = notice.params.get(1).cloned().unwrap_or_default();
                if let (Some(rank), Some(identity)) =
                    (rank, cache.identities.get_mut(self.guild_id))
                {
                    if let Some(slot) = identity.rank_names.get_mut(rank) {
                        *slot = name;
                        self.dirty = true;
                    }
                }
                // Our own rank changed: its rights again (`0x6d951b`, `0x6d9524`).
                if rank.is_some_and(|r| r as u64 == u64::from(self.rank_index)) {
                    self.permissions_ask = true;
                }
            }
            _ => {}
        }
        // Only cases `0x00`-`0x0b` carry arg1: not sign-on or sign-off, nor anything past them.
        if notice.event <= guild_event::UPDATE_ROSTER {
            self.note_roster_update(RosterUpdate::Stale);
        }
    }

    /// `SMSG_GUILD_COMMAND_RESULT`'s effect on the mirror: stale only for a successful command
    /// `0x13`/`0x14` or result `0x14` on command `0x05`; the server re-sends the roster for any
    /// real change.
    fn apply_command_result(&mut self, result: &GuildCommandResult) {
        use benilla_protocol::messages::{guild_command, guild_command_error};
        let stale = match result.result {
            guild_command_error::PLAYER_NO_MORE_IN_GUILD => {
                matches!(result.command, guild_command::UNK19 | guild_command::UNK20)
            }
            guild_command_error::UNK20 => result.command == 0x05,
            _ => false,
        };
        if stale {
            self.note_roster_update(RosterUpdate::Stale);
        }
    }

    /// `SMSG_GUILD_INVITE`: queue the invitation for the feed, which signals the popup's event.
    fn apply_invite(&mut self, inviter: String, guild: String) {
        self.pending_invites.push((inviter, guild));
        self.dirty = true;
    }

    /// `SortGuildRoster(field)`: re-order and repaint, never re-request ([`sort::SortStack`]).
    fn sort_by(&mut self, field: &str) {
        self.sort.select(SortField::parse(field));
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// `SetGuildRosterShowOffline(flag)`: re-sort and repaint; an unchanged value does nothing, as
    /// in the reference.
    fn set_show_offline(&mut self, on: bool) {
        if self.show_offline != on {
            self.show_offline = on;
            self.note_roster_update(RosterUpdate::Applied);
        }
    }

    /// `SetGuildRosterSelection(index)`: store the row's guid. No event: the stock click handler
    /// repaints itself (`FriendsFrame.lua:706`).
    fn select(&mut self, index: u32) {
        let guid = self.guid_at(index).unwrap_or(0);
        if self.selection != guid {
            self.selection = guid;
            self.dirty = true;
        }
    }

    /// The guid at a 1-based display row, over the whole roster, not the show-offline count.
    fn guid_at(&self, index: u32) -> Option<u64> {
        index
            .checked_sub(1)
            .and_then(|i| self.display_order.get(i as usize))
            .copied()
    }

    /// The member at a 1-based display row, mutable because the note verbs write before they send.
    fn member_at_mut(&mut self, index: u32) -> Option<&mut GuildRosterMember> {
        let guid = self.guid_at(index)?;
        self.members.iter_mut().find(|m| m.guid == guid)
    }

    /// `GetNumGuildMembers()`: the whole roster with show-offline on, else the online tally. A loop
    /// bound, not the addressable range: with show-offline off the offline rows sort last
    /// ([`sort`]).
    fn num_members(&self) -> usize {
        if self.show_offline {
            self.members.len()
        } else {
            self.members.iter().filter(|m| m.is_online()).count()
        }
    }

    /// Our `PLAYER_GUILDRANK`.
    pub(crate) fn rank_index(&self) -> u32 {
        self.rank_index
    }

    /// Our rank's rights for bank tab `tab`: its rights mask and its daily withdrawals, 0s before
    /// a roster (`0x5a41c0`, `0x5a41f0`).
    pub(crate) fn own_bank_tab(&self, tab: u8) -> (u32, u32) {
        self.rank_bank
            .get(self.rank_index as usize)
            .and_then(|r| r.tabs.get(usize::from(tab)))
            .copied()
            .unwrap_or((0, 0))
    }

    /// Give our rank `rights` and `withdrawals` on bank tab `tab`, as a permissions answer does.
    #[cfg(test)]
    pub(crate) fn set_own_bank_tab(&mut self, tab: usize, rights: u32, withdrawals: u32) {
        let rank = self.rank_index as usize;
        if self.rank_bank.len() <= rank {
            self.rank_bank.resize(rank + 1, Default::default());
        }
        self.rank_bank[rank].tabs[tab] = (rights, withdrawals);
    }

    /// Our rank's rights word, which every `Can*` predicate tests; `0` (none) before a roster.
    fn own_rights(&self) -> u32 {
        self.rank_rights
            .get(self.rank_index as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Days since logout, fully decomposed as `GetGuildRosterLastOnline` does, each step truncated
/// (`_ftol`) and the remainder after months narrowed to f32. The stock frame shows only the largest
/// non-zero unit, and all zeroes as under an hour (`FriendsFrame.lua:957-978`).
pub(crate) fn last_online(days: f32) -> LastOnline {
    if days.is_nan() || days <= 0.0 {
        // NaN, negative, and an online member's absent float: all zeroes.
        return LastOnline::default();
    }
    // f64 for the reference's x87 extended precision; it multiplies by f32 reciprocals (365).
    const PER_YEAR: f32 = 1.0 / 365.0;
    const PER_MONTH: f32 = 1.0 / 30.0;
    let days = f64::from(days);
    let years = (days * f64::from(PER_YEAR)) as u32;
    let rem = days - f64::from(years) * 365.0;
    let months = (rem * f64::from(PER_MONTH)) as u32;
    let rem = (rem - f64::from(months) * 30.0) as f32;
    let day = rem as u32;
    LastOnline {
        years,
        months,
        days: day,
        hours: ((rem - day as f32) * 24.0) as u32,
    }
}

/// `GetGuildInfo(unit)`: the unit's public guild fields joined against the identity cache, asking
/// on a miss. `None` for a guildless unit, a creature or an unanswered query, one nil path in the
/// reference.
pub(crate) fn unit_guild(
    fields: &ObjectFields,
    guild: &GuildCache,
    commands: &NetCommands,
) -> Option<UnitGuild> {
    let guild_id = fields.player_guild_id();
    let rank_index = fields.player_guild_rank();
    let identity = guild.resolve(guild_id, commands)?;
    Some(UnitGuild {
        name: identity.name.clone(),
        rank_name: identity.rank_name(rank_index).to_string(),
        rank_index,
    })
}

/// A unit's guild name alone: the overhead name stack's guild line (`"\n<%s>"`), shown under
/// `UnitNamePlayerGuild`'s mask bit `0x10` and read. [`unit_guild`] without its clones:
/// [`crate::nameplates::drive_nameplates`] reads it per frame.
pub(crate) fn unit_guild_name<'a>(
    fields: &ObjectFields,
    guild: &'a GuildCache,
    commands: &NetCommands,
) -> Option<&'a str> {
    guild
        .resolve(fields.player_guild_id(), commands)
        .map(|identity| identity.name.as_str())
}

/// A player's guild name off its `PLAYER_GUILDID`, asking on a miss: the reference's
/// `GetPlayerGuildNameText` (`0x6db6a0`), `None` for no guild or a query in flight.
pub(crate) fn player_guild_name<'a>(
    fields: &ObjectFields,
    guild: &'a GuildCache,
    commands: &NetCommands,
) -> Option<&'a str> {
    guild
        .resolve(fields.player_guild_id(), commands)
        .map(|identity| identity.name.as_str())
}

/// A unit's guild crest for the body composite, off its public `PLAYER_GUILDID`, asking on a miss
/// like [`unit_guild`]. `None` keeps a Guild Tabard's own `Tabard_A_05Default` art, as the
/// reference never enters its install for a guildless wearer, a creature, a query in flight
/// (re-dressed when [`GuildCache::generation`] moves) or an undesigned `-1` crest, which
/// would paint blank.
pub(crate) fn unit_guild_emblem(
    fields: &ObjectFields,
    guild: &GuildCache,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.player_guild_id(), guild, commands)
}

/// The same crest for a corpse, off its own `CORPSE_FIELD_GUILD` through the same lookup, with the
/// same `None` cases.
pub(crate) fn corpse_guild_emblem(
    fields: &ObjectFields,
    guild: &GuildCache,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.corpse_guild(), guild, commands)
}

fn guild_emblem(guild_id: u32, guild: &GuildCache, commands: &NetCommands) -> Option<GuildEmblem> {
    let emblem = guild.resolve(guild_id, commands)?.emblem;
    emblem.is_designed().then_some(emblem)
}

/// The guild packet handlers: they update the mirror and queue the message ids [`lines`] names,
/// whose catalog rows pick the surface and the sound; the guild events fire from the feed.
pub(crate) mod net {
    use super::*;
    use crate::ui_action::signals::SignalSink;
    use crate::ui_action::UiError;
    use crate::ui_script::live::UiEvents;
    use crate::ui_social::SocialState;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the guild handlers, one per kind, plus the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::GuildQueryResponse, on_query_response)
            .net_handler(K::GuildRoster, on_roster)
            .net_handler(K::GuildPermissions, on_permissions)
            .net_handler(K::GuildEvent, on_event)
            .net_handler(K::GuildCommandResult, on_command_result)
            .net_handler(K::GuildInvite, on_invite)
            .net_handler(K::GuildDecline, on_decline)
            .net_handler(K::GuildInfo, on_info)
            .net_handler(K::InvalidatePlayer, on_invalidate_player)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_invalidate_player(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut cache: ResMut<GuildCache>,
        commands: Res<NetCommands>,
        time: Res<Time<Real>>,
    ) {
        if let SessionEvent::InvalidatePlayer { guid } = ev {
            guild.invalidate_player(&mut cache, guid, time.elapsed_secs_f64(), &commands);
        }
    }

    fn on_query_response(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut cache: ResMut<GuildCache>,
        players: Query<&crate::net::ObjectStore, With<crate::net::NetEntity>>,
        ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
    ) {
        if let SessionEvent::GuildQueryResponse(response) = ev {
            let guild_id = response.guild_id;
            let found = !response.name.is_empty();
            let walks = cache.take_stamp_asks(guild_id);
            query_response(&mut guild, &mut cache, response);
            // The timestamp watch's callback, on a found record (`0x6e1b93`): each visible player in that guild runs its guild change (`0x6e1b7a` -> `0x6df7a0`).
            let Some(mut ui) = ui.filter(|_| found) else {
                return;
            };
            let members = players
                .iter()
                .filter(|s| {
                    s.0.object_type() == Some(benilla_protocol::messages::ObjectType::Player)
                        && s.0.player_guild_id() == guild_id
                })
                .count();
            for _ in 0..walks * members {
                ui.fire("TABARD_CANSAVE_CHANGED", vec![]);
            }
        }
    }

    fn on_roster(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildRoster(r) = ev {
            roster(&mut guild, r);
        }
    }

    fn on_permissions(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildPermissions(p) = ev {
            guild.apply_permissions(&p);
        }
    }

    /// Reads the friends list, the notify CVar and our guid for the sign-on condition ([`event`]).
    fn on_event(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut cache: ResMut<GuildCache>,
        mut ui: ResMut<UiEvents>,
        social: Res<SocialState>,
        notify: Res<GuildMemberNotify>,
        self_guid: Res<crate::net::SelfGuid>,
    ) {
        if let SessionEvent::GuildEvent(notice) = ev {
            event(
                &mut guild,
                &mut cache,
                &mut *ui,
                &social,
                &notify,
                self_guid.0,
                notice,
            );
        }
    }

    fn on_command_result(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut ui: ResMut<UiEvents>,
    ) {
        if let SessionEvent::GuildCommandResult(result) = ev {
            command_result(&mut guild, &mut *ui, result);
        }
    }

    fn on_invite(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut ui: ResMut<UiEvents>,
    ) {
        if let SessionEvent::GuildInvite { inviter, guild: g } = ev {
            invite(&mut guild, &mut *ui, inviter, g);
        }
    }

    fn on_decline(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        if let SessionEvent::GuildDecline { name } = ev {
            decline(&mut *ui, &name);
        }
    }

    fn on_info(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildInfo(i) = ev {
            info(&mut guild, i);
        }
    }

    /// Reset at session end: the next login may be another character. The identity cache goes
    /// too, as 12340 builds `guildcache` with persistence off and clears it at world entry
    /// (`0x52b7c4`).
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut cache: ResMut<GuildCache>,
    ) {
        *guild = GuildState::default();
        *cache = GuildCache::default();
    }

    /// Show the message ids [`lines`] named, each at the display (`0x5216f0`) in the handler's turn.
    fn push_lines(errors: &mut impl SignalSink, lines: impl IntoIterator<Item = UiError>) {
        for line in lines {
            errors.line(line);
        }
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`.
    pub(crate) fn query_response(
        guild: &mut GuildState,
        cache: &mut GuildCache,
        response: GuildQueryResponse,
    ) {
        guild.apply_query_response(cache, &response);
    }

    /// `SMSG_GUILD_ROSTER`.
    pub(crate) fn roster(guild: &mut GuildState, roster: GuildRoster) {
        guild.apply_roster(roster);
    }

    /// `SMSG_GUILD_EVENT`. The sign-on/sign-off pair's guid serves its line's condition, four
    /// conjuncts in the reference's `0x0c`/`0x0d` arms, each failing to the silent exit:
    ///
    /// 1. a local player exists (ours: we know our guid);
    /// 2. `guildMemberNotify` is on;
    /// 3. the subject is not us: the reference compares names, we compare guids, and vmangos sends
    ///    the sign-on to the signer too (`Guild.cpp:651-656`);
    /// 4. the subject is not a friend (the friend-slot lookup, not the ignore check), since
    ///    `SMSG_FRIEND_STATUS` prints the same line ungated.
    pub(crate) fn event(
        guild: &mut GuildState,
        cache: &mut GuildCache,
        errors: &mut impl SignalSink,
        social: &SocialState,
        notify: &GuildMemberNotify,
        self_guid: Option<u64>,
        notice: GuildEventNotice,
    ) {
        let announce = announce_signon(social, notify, self_guid, notice.guid);
        guild.apply_event(cache, &notice);
        push_lines(errors, lines::event_line(&notice, announce));
    }

    /// The sign-on/sign-off line's four conjuncts ([`event`]) as one predicate.
    pub(super) fn announce_signon(
        social: &SocialState,
        notify: &GuildMemberNotify,
        self_guid: Option<u64>,
        subject: Option<u64>,
    ) -> bool {
        if !notify.0 {
            return false; // conjunct 2
        }
        match (self_guid, subject) {
            // conjuncts 3 and 4.
            (Some(me), Some(subject)) => subject != me && !social.is_friend(subject),
            // No local player (conjunct 1) or no guid on the wire: the silent exit.
            _ => false,
        }
    }

    /// `SMSG_GUILD_COMMAND_RESULT`.
    pub(crate) fn command_result(
        guild: &mut GuildState,
        errors: &mut impl SignalSink,
        result: GuildCommandResult,
    ) {
        guild.apply_command_result(&result);
        push_lines(errors, lines::command_line(&result));
    }

    /// `SMSG_GUILD_INVITE`: the popup's show edge and the notice line printed beside it.
    pub(crate) fn invite(
        guild: &mut GuildState,
        errors: &mut impl SignalSink,
        inviter: String,
        guild_name: String,
    ) {
        push_lines(errors, [lines::invite_line(&inviter, &guild_name)]);
        guild.apply_invite(inviter, guild_name);
    }

    /// `SMSG_GUILD_DECLINE`: a line, no state.
    pub(crate) fn decline(errors: &mut impl SignalSink, name: &str) {
        push_lines(errors, [lines::decline_line(name)]);
    }

    /// `SMSG_GUILD_INFO`, the `/ginfo` answer: parked for the feed, which builds its two lines
    /// ([`GuildState::pending_info`]).
    pub(crate) fn info(guild: &mut GuildState, info: GuildInfo) {
        guild.pending_info.push(info);
    }
}

/// The guild feed's system set, so the chat drain can run after the guild events fire.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GuildFeed;

/// The guild's `.in_set(UiDrain)` drain, which the guild bank's drain reads after.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GuildDrain;

/// The guild windows' session: the wire mirror, the VM feed, and the outbound intents.
pub(crate) struct UiGuildPlugin;

/// `guildMemberNotify`'s change callback: conjunct 2 of the sign-on/sign-off line's condition.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut notify: ResMut<GuildMemberNotify>) {
    if ev.is("guildMemberNotify") {
        notify.0 = ev.flag();
    }
}

impl Plugin for UiGuildPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        event_log::register(app);
        app.add_observer(on_cvar);
        crate::query_cache::register::<GuildCache>(app);
        app.init_resource::<GuildState>()
            .init_resource::<GuildCache>()
            .init_resource::<GuildMemberNotify>()
            .add_systems(
                Update,
                (
                    feed::feed_guild.in_set(UiFeed).in_set(GuildFeed),
                    // After the roster's push, whose rank names `GetGuildEventInfo` reads.
                    event_log::feed_guild_event_log
                        .in_set(UiFeed)
                        .after(feed::feed_guild),
                    crate::ui_script::raises::then_fire(feed::drain_guild)
                        .in_set(UiDrain)
                        .in_set(GuildDrain),
                )
                    // Never against the boot VM: the feed's edge memo survives the entry load, so
                    // an edge spent on a VM with no frames, such as the login MOTD, is lost.
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_social::SocialState;
    use benilla_protocol::messages::{guild_command, guild_command_error, guild_presence};

    fn identity(name: &str, ranks: &[&str]) -> Identity {
        let mut rank_names: [String; GUILD_RANKS_MAX_COUNT] = Default::default();
        for (slot, name) in rank_names.iter_mut().zip(ranks) {
            *slot = (*name).to_string();
        }
        Identity {
            name: name.to_string(),
            rank_names,
            emblem: GuildEmblem::default(),
        }
    }

    fn member(guid: u64, name: &str, presence: u8) -> GuildRosterMember {
        GuildRosterMember {
            guid,
            presence,
            name: name.to_string(),
            level: 60,
            ..Default::default()
        }
    }

    fn event(event: u8, params: &[&str], guid: Option<u64>) -> GuildEventNotice {
        GuildEventNotice {
            event,
            params: params.iter().map(|p| (*p).to_string()).collect(),
            guid,
        }
    }

    #[test]
    fn the_signon_condition_is_all_four_conjuncts() {
        let mut social = SocialState::default();
        let contact = |guid, flags| benilla_protocol::messages::ContactEntry {
            guid,
            flags,
            ..Default::default()
        };
        crate::ui_social::net::contact_list(
            &mut social,
            benilla_protocol::messages::ContactList {
                mask: 7,
                contacts: vec![contact(7, 1), contact(9, 2)],
            },
        );
        let on = GuildMemberNotify(true);
        let off = GuildMemberNotify(false);
        let me = Some(1);

        // 2: the CVar, off by default.
        assert!(
            !net::announce_signon(&social, &off, me, Some(5)),
            "guildMemberNotify off silences the whole family"
        );
        assert!(net::announce_signon(&social, &on, me, Some(5)));

        // 3: not us; vmangos sends the sign-on to the signer too (`Guild.cpp:651-656`).
        assert!(
            !net::announce_signon(&social, &on, me, Some(1)),
            "your own sign-on is not announced to you"
        );

        // 4: not a friend, whose `SMSG_FRIEND_STATUS` prints the same line.
        assert!(
            !net::announce_signon(&social, &on, me, Some(7)),
            "a guildmate who is also a friend is announced by the friend path, not twice"
        );

        // An ignored guildmate is announced: the ignore list plays no part.
        assert!(
            social.is_ignored(9),
            "the fixture's ignore really is an ignore"
        );
        assert!(
            net::announce_signon(&social, &on, me, Some(9)),
            "the reference announces an ignored guildmate — the reference is not the ignore check"
        );

        // 1: a local player, and the pair's guid; either missing is the silent exit.
        assert!(!net::announce_signon(&social, &on, None, Some(5)));
        assert!(!net::announce_signon(&social, &on, me, None));
    }

    #[test]
    fn show_offline_changes_the_count_and_the_order_not_the_membership() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            members: vec![
                member(1, "Zed", guild_presence::ONLINE),
                member(2, "Gone", guild_presence::OFFLINE),
                member(3, "Away", guild_presence::ONLINE | guild_presence::AFK),
            ],
            ..Default::default()
        });

        let rows = feed::display_rows(&guild, None, None);
        assert_eq!(rows.len(), 3, "every member, always");
        assert_eq!(guild.num_members(), 2, "but the count is the online tally");
        assert!(
            rows[..2].iter().all(|r| r.info.online),
            "and the counted rows are the leading ones"
        );
        assert!(!rows[2].info.online);

        guild.set_show_offline(true);
        let rows = feed::display_rows(&guild, None, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(guild.num_members(), 3, "now the count is everybody");
    }

    #[test]
    fn last_online_decomposes_largest_unit_first() {
        // Under an hour: every unit zero.
        assert_eq!(last_online(0.02), LastOnline::default());
        assert_eq!(last_online(0.0), LastOnline::default());
        // Just over an hour.
        assert_eq!(
            last_online(1.0 / 24.0 + 0.001),
            LastOnline {
                hours: 1,
                ..Default::default()
            }
        );
        // Just under and just over a day.
        assert_eq!(last_online(0.99).days, 0);
        assert_eq!(last_online(0.99).hours, 23);
        assert_eq!(last_online(1.01).days, 1);
        // Just under and just over a month (30 days).
        assert_eq!(last_online(29.9).months, 0);
        assert_eq!(last_online(29.9).days, 29);
        assert_eq!(
            last_online(30.0),
            LastOnline {
                months: 1,
                ..Default::default()
            }
        );
        // Just under and just over a year (365 days).
        assert_eq!(last_online(364.9).years, 0);
        assert_eq!(last_online(364.9).months, 12, "12 months, not 1 year");
        assert_eq!(
            last_online(365.0),
            LastOnline {
                years: 1,
                ..Default::default()
            }
        );
        // The full decomposition: 400.5 days is 1 year, 1 month, 5 days, 12 hours.
        assert_eq!(
            last_online(400.5),
            LastOnline {
                years: 1,
                months: 1,
                days: 5,
                hours: 12,
            }
        );
    }

    #[test]
    fn an_empty_query_name_is_a_negative_not_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();

        assert!(cache.resolve(7, &commands).is_none());
        assert!(cache.resolve(7, &commands).is_none());
        assert_eq!(rx.try_iter().count(), 1, "asked exactly once");

        guild.apply_query_response(
            &mut cache,
            &GuildQueryResponse {
                guild_id: 7,
                name: String::new(),
                ..Default::default()
            },
        );
        assert!(cache.identities.answered(7), "cached as a negative");
        assert!(!cache.identities.is_pending(7), "and no longer in flight");
        assert!(
            cache.resolve(7, &commands).is_none(),
            "an empty name is not a guild"
        );
        assert_eq!(rx.try_iter().count(), 0, "and is never re-asked");
    }

    #[test]
    fn a_guild_event_asks_again_and_a_local_resort_does_not() {
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();

        guild.apply_event(&mut cache, &event(guild_event::JOINED, &["Furor"], None));
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Stale));
        assert!(RosterUpdate::Stale.arg1(), "the pane re-requests");

        guild.sort_by("level");
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Applied));
        assert!(
            !RosterUpdate::Applied.arg1(),
            "a column click must never ask the server for a roster"
        );

        guild.set_show_offline(true);
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Applied));

        guild.apply_roster(GuildRoster::default());
        assert_eq!(
            guild.roster_event.take(),
            Some(RosterUpdate::Applied),
            "the roster we just applied is the one we have — asking for it again is the loop"
        );

        // Both in one frame: the re-request subsumes the repaint.
        guild.apply_roster(GuildRoster::default());
        guild.apply_event(
            &mut cache,
            &event(guild_event::PROMOTION, &["A", "B", "Officer"], None),
        );
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Stale));
    }

    #[test]
    fn signing_on_and_off_does_not_re_request() {
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();
        for ev in [guild_event::SIGNED_ON, guild_event::SIGNED_OFF] {
            guild.roster_event = None;
            guild.apply_event(&mut cache, &event(ev, &["Tigole"], Some(9)));
            assert_eq!(guild.roster_event, None, "event {ev:#04x}");
        }
        // The case just below them does.
        guild.apply_event(&mut cache, &event(guild_event::UPDATE_ROSTER, &[], None));
        assert_eq!(guild.roster_event, Some(RosterUpdate::Stale));
    }

    #[test]
    fn the_stale_edge_clears_the_request_throttle() {
        let mut guild = GuildState {
            roster_allowed_at: 12_345.0,
            ..Default::default()
        };
        guild.note_roster_update(RosterUpdate::Applied);
        assert_eq!(guild.roster_allowed_at, 12_345.0, "a repaint does not");
        guild.note_roster_update(RosterUpdate::Stale);
        assert_eq!(guild.roster_allowed_at, 0.0);
    }

    #[test]
    fn most_command_results_do_not_move_the_roster() {
        let mut guild = GuildState::default();
        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::INVITE,
            name: "Kaplan".into(),
            result: guild_command_error::PERMISSIONS,
        });
        assert_eq!(guild.roster_event, None);

        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::UNK19,
            name: String::new(),
            result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
        });
        assert_eq!(guild.roster_event, Some(RosterUpdate::Stale));
    }

    #[test]
    fn an_invalidated_guild_guid_drops_its_record_and_a_member_re_asks_the_roster() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();
        cache
            .identities
            .insert(7, Some(identity("Legacy", &["GM"])));
        assert!(guild.mirror_self(7, 0));
        guild.apply_roster(GuildRoster {
            rank_rights: vec![0xffff],
            members: vec![member(0x2A, "Tigole", guild_presence::ONLINE)],
            ..Default::default()
        });

        guild.invalidate_player(&mut cache, 0x1FF6_0000_0000_0007, 100.0, &commands);
        assert!(!cache.identities.answered(7), "the guild record is gone");
        assert_eq!(rx.try_iter().count(), 0, "a guild guid asks nothing");

        guild.invalidate_player(&mut cache, 0x99, 100.0, &commands);
        assert_eq!(rx.try_iter().count(), 0, "a non-member asks nothing");

        guild.invalidate_player(&mut cache, 0x2A, 100.0, &commands);
        assert!(matches!(
            rx.try_iter().collect::<Vec<_>>().as_slice(),
            [ClientCommand::GuildRosterRequest]
        ));
        assert_eq!(
            guild.roster_allowed_at,
            100.0 + ROSTER_REQUEST_THROTTLE_SECS
        );
    }

    #[test]
    fn leaving_the_guild_drops_the_roster_but_not_the_identities() {
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();
        cache
            .identities
            .insert(7, Some(identity("Legacy", &["GM"])));
        assert!(guild.mirror_self(7, 0), "joined");
        guild.apply_roster(GuildRoster {
            motd: "Raid at eight".into(),
            rank_rights: vec![0xffff, 0x3],
            members: vec![member(1, "Tigole", guild_presence::ONLINE)],
            ..Default::default()
        });
        guild.selection = 1;
        assert!(guild.in_guild());

        assert!(guild.mirror_self(0, 0), "left");
        assert!(!guild.in_guild());
        assert!(guild.members.is_empty());
        assert!(guild.motd.is_empty());
        assert_eq!(guild.selection, 0);
        assert!(cache.identities.answered(7), "identities survive");

        assert!(!guild.mirror_self(0, 0), "no edge when nothing moved");
    }

    /// At login the MOTD event arrives before our descriptor names the guild.
    #[test]
    fn learning_our_own_guild_id_is_not_leaving_a_guild() {
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();
        guild.apply_event(
            &mut cache,
            &event(guild_event::MOTD, &["Raid Wednesday at eight."], None),
        );
        assert!(!guild.in_guild(), "the descriptor has not said yet");

        assert!(
            guild.mirror_self(1, 3),
            "the descriptor finally names the guild"
        );
        assert_eq!(
            guild.motd, "Raid Wednesday at eight.",
            "the MOTD is about the guild we have just been told we are in"
        );
        assert_eq!(
            guild.roster_event,
            Some(RosterUpdate::Stale),
            "the pane still learns the snapshot moved — the MOTD packet's own stale signal, \
             which outranks the `Applied` this edge notes"
        );
    }

    #[test]
    fn own_rights_index_through_the_descriptor_rank() {
        let mut guild = GuildState::default();
        guild.mirror_self(7, 1);
        assert_eq!(guild.own_rights(), 0, "no roster yet");
        guild.apply_roster(GuildRoster {
            rank_rights: vec![0xffff, 0x00ff, 0x000f],
            ..Default::default()
        });
        assert_eq!(guild.own_rights(), 0x00ff);
        assert_eq!(guild.rank_rights.len(), 3);
        guild.mirror_self(7, 9); // a rank past the array
        assert_eq!(guild.own_rights(), 0);
    }

    #[test]
    fn an_over_long_rank_array_is_clamped() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            rank_rights: vec![1; 14],
            ..Default::default()
        });
        assert_eq!(guild.rank_rights.len(), GUILD_RANKS_MAX_COUNT);
    }

    #[test]
    fn a_rank_rename_patches_the_identity_cache() {
        let mut guild = GuildState::default();
        let mut cache = GuildCache::default();
        guild.mirror_self(7, 0);
        cache
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));
        guild.apply_event(
            &mut cache,
            &event(guild_event::UPDATE_RANK_NAME, &["1", "Officer"], None),
        );
        assert_eq!(cache.identities.get(7).unwrap().rank_name(1), "Officer");
        assert!(guild.dirty);
    }

    #[test]
    fn selection_follows_the_player_not_the_row() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            members: vec![
                member(11, "Alice", guild_presence::ONLINE),
                member(22, "Bob", guild_presence::ONLINE),
            ],
            ..Default::default()
        });
        guild.display_order = vec![11, 22];
        guild.select(2);
        assert_eq!(guild.selection, 22);
        assert_eq!(
            guild.member_at_mut(2).map(|m| m.name.clone()),
            Some("Bob".to_string()),
            "a display row resolves to the member the wire wants by name"
        );

        // Re-ordered: the same player stays selected.
        guild.display_order = vec![22, 11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 1);

        guild.select(0);
        assert_eq!(guild.selection, 0, "0 = nothing selected");
        guild.select(9);
        assert_eq!(guild.selection, 0, "past the end selects nothing");

        // Bob leaves: the stored guid stops resolving.
        guild.selection = 22;
        guild.apply_roster(GuildRoster {
            members: vec![member(11, "Alice", guild_presence::ONLINE)],
            ..Default::default()
        });
        guild.display_order = vec![11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 0);
    }

    #[test]
    fn every_invitation_packet_queues_its_own_event() {
        let mut guild = GuildState {
            dirty: false,
            ..Default::default()
        };
        // The same inviter and guild twice: two events, as `0x6cc3e5` signals per packet.
        for _ in 0..2 {
            guild.apply_invite("Tigole".into(), "Legacy of Steel".into());
        }
        assert!(guild.dirty, "the feed runs to signal them");
        assert_eq!(
            guild.pending_invites,
            vec![("Tigole".to_string(), "Legacy of Steel".to_string()); 2]
        );
    }

    #[test]
    fn unit_guild_asks_on_a_miss_and_never_names_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut cache = GuildCache::default();
        cache
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));

        // A creature or a guildless player: no `PLAYER_GUILDID`.
        let guildless = ObjectFields::from_pairs(&[]);
        assert!(unit_guild(&guildless, &cache, &commands).is_none());
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        // Fields 191/192 = PLAYER_GUILDID/PLAYER_GUILDRANK.
        let unknown = ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, 9),
            (benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 1),
        ]);
        assert!(
            unit_guild(&unknown, &cache, &commands).is_none(),
            "a query in flight is not a blank-named guild"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let member = ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, 7),
            (benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 1),
        ]);
        let resolved = unit_guild(&member, &cache, &commands).expect("cached");
        assert_eq!(resolved.name, "Legacy");
        assert_eq!(resolved.rank_name, "Off");
        assert_eq!(resolved.rank_index, 1);

        // A rank past the ten slots names nothing.
        let odd_rank = ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, 7),
            (benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 99),
        ]);
        assert_eq!(
            unit_guild(&odd_rank, &cache, &commands).unwrap().rank_name,
            ""
        );
    }

    #[test]
    fn unit_guild_name_is_unit_guilds_name_on_every_leg() {
        let (commands, rx) = net_commands();
        let mut cache = GuildCache::default();
        cache
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));
        // The negative cache: an empty answer is no such guild.
        cache.identities.insert(8, Some(identity("", &[])));

        let guildless = ObjectFields::from_pairs(&[]);
        assert_eq!(unit_guild_name(&guildless, &cache, &commands), None);
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        let unknown =
            ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_PLAYER_GUILDID, 9)]);
        assert_eq!(
            unit_guild_name(&unknown, &cache, &commands),
            None,
            "a query in flight draws no line"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let blank = ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_PLAYER_GUILDID, 8)]);
        assert_eq!(unit_guild_name(&blank, &cache, &commands), None);
        assert_eq!(
            rx.try_iter().count(),
            0,
            "the negative cache re-asks nothing"
        );

        let member = ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_PLAYER_GUILDID, 7),
            (benilla_protocol::field::FIELD_PLAYER_GUILDRANK, 1),
        ]);
        assert_eq!(unit_guild_name(&member, &cache, &commands), Some("Legacy"));
        let via_line = unit_guild_name(&member, &cache, &commands).map(str::to_owned);
        let via_api = unit_guild(&member, &cache, &commands).map(|g| g.name);
        assert_eq!(via_line, via_api, "the two readers of one cache disagree");
    }

    /// A command channel whose receiver stays alive, so a send neither blocks nor is dropped.
    fn net_commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }
}
