//! The zone-channel auto-join. The client joins every [`ChannelState::zone_mask`] row composed for
//! the first zone of a session, once (`0x508090`, its registered flag reset only at world entry);
//! after that the server moves the zone channels itself, joining each new name, and the confirm
//! renames the slot by its ChannelID (`0x507880`).
//!
//! The join's `CHAT_MSG_CHANNEL_NOTICE` is also one of the events that trigger AceEvent-2.0's
//! `AceEvent_FullyInitialized`, which many Ace2 addons wait for.

use bevy::prelude::*;

use benilla_assets::LockRecover;
use benilla_formats::ChatChannelsCatalog;

use crate::area::AreaTableRes;
use crate::net::NetCommands;

use super::edit::{ChannelState, SlotState};

/// `AreaTable.dbc` flag `0x08`, the walk's city gate, set on the six capitals; vmangos names it
/// `AREA_FLAG_SLAVE_CAPITAL`, "Allow trade channel" (`DBCEnums.h:58`).
const AREA_FLAG_TRADE_CHANNEL: u32 = 0x08;

/// `AreaTable.dbc` flag `0x100`, the capital test the channel verbs' composition reads (`0x507b7d`,
/// `0x5094be`).
const AREA_FLAG_CAPITAL: u32 = 0x100;

/// `AreaTable.dbc` flag `0x200`: the one row (id 3459, "City"), cached at load, whose localized
/// name the city-named channels take.
const AREA_FLAG_CITY_NAME_ROW: u32 = 0x200;

/// The city word, as the client's load-time scan finds it; `None` skips the city-named rows.
pub(crate) fn city_word(areas: &benilla_formats::AreaTableCatalog) -> Option<&str> {
    areas
        .rows()
        .find(|r| r.flags & AREA_FLAG_CITY_NAME_ROW != 0)
        .map(|r| r.name.as_str())
        .filter(|n| !n.is_empty())
}

/// The zone update's own state; the joined list is not copied here, as the walk reads the records
/// themselves ([`ChannelState::joined`]).
#[derive(Resource, Default)]
pub(crate) struct ZoneChannelWalk {
    /// The chat's ready flag (`[0xbcefe8]`), set by the first zone update's walk (`0x508249`).
    ready: bool,
}

impl ZoneChannelWalk {
    /// The chat's ready flag (`[0xbcefe8]`): read by the zone update (`0x5080ba`), the
    /// guild-recruitment cascade (`0x507d43`) and the mode setter (`0x50826e`).
    pub(super) fn ready(&self) -> bool {
        self.ready
    }

    /// A session whose walk ran, the chat ready.
    #[cfg(test)]
    pub(crate) fn ready_chat() -> Self {
        ZoneChannelWalk { ready: true }
    }

    /// Forget the session: the next world entry walks from nothing.
    fn clear_session(&mut self) {
        self.ready = false;
    }
}

/// End the channel session: the server leaves every channel with the session, so the next one
/// starts in none, and the pane starts empty, as the session init clears it (`0x50f198`-`0x50f202`).
/// The verbs read the records live, so nothing reaches the VM. The mask stays, as it belongs to the
/// character's file.
fn end_channel_session(channels: &mut ChannelState, walk: &mut ZoneChannelWalk) {
    walk.clear_session();
    channels.joined.clear();
    channels.pane = Default::default();
}

/// The session-end edge: a confirmed `/logout` back to the glue screens (`OnExit(InWorld)`).
pub(super) fn end_session_channels(
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
) {
    end_channel_session(&mut channels, &mut walk);
}

/// The other session end, a dead socket: a recoverable drop never leaves `InWorld`, but the
/// reconnect gets a fresh server-side `Player` in no channels, so it clears the same state. A
/// logout or a final drop leaves `InWorld`, where [`end_session_channels`] clears after the UI
/// shutdown's PLAYER_LEAVING_WORLD (`0x529163` before `0x529172`).
pub(super) fn end_session_channels_on_disconnect(
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut disconnects: MessageReader<crate::net::DisconnectedMessage>,
) {
    let recoverable = |m: &crate::net::DisconnectedMessage| {
        m.end == benilla_protocol::SessionEnd::Lost && !m.session_over
    };
    if !disconnects.read().any(recoverable) {
        return;
    }
    end_channel_session(&mut channels, &mut walk);
}

/// What the channel verbs read and write at the call: the channel records, the `ZONECHANNELS`
/// mask (`0xbcf010`) and `ChatChannels.dbc` composed for the client's zone id (`0xbd080c`).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ChannelLive<'w> {
    channels: ResMut<'w, ChannelState>,
    zone: Option<Res<'w, crate::area::ZoneIds>>,
    areas: Option<Res<'w, AreaTableRes>>,
}

impl ChannelLive<'_> {
    /// The records in slot order, holes kept.
    pub(crate) fn slots(&self) -> Vec<Option<benilla_ui::script::ChannelSlotInfo>> {
        self.channels.ui_slots()
    }

    /// The channel pane's state.
    pub(crate) fn pane(&mut self) -> &mut benilla_ui::script::ChannelPane {
        &mut self.channels.pane
    }

    /// A record's flags byte, as a display list writes it (`0x503a69`).
    pub(crate) fn set_flags(&mut self, slot: usize, flags: u8) {
        if let Some(Some(s)) = self.channels.joined.get_mut(slot) {
            s.flags = flags;
        }
    }

    /// The catalog composed for the zone id at the call; zone-less while it is 0 or unknown, and
    /// empty without `ChatChannels.dbc`.
    pub(crate) fn catalog(&self) -> Vec<benilla_ui::script::ZoneChannelRow> {
        if self.channels.channels.is_empty() {
            return Vec::new();
        }
        let areas = self.areas.as_deref();
        let zone = self.zone.as_deref().map_or(0, |z| z.zone);
        // The zone's area record and text (`0x507ab3`, `0x507aad`).
        let (name, in_city, capital) = areas
            .and_then(|a| a.0.get(zone))
            .map(|r| {
                (
                    r.name.clone(),
                    r.flags & AREA_FLAG_TRADE_CHANNEL != 0,
                    r.flags & AREA_FLAG_CAPITAL != 0,
                )
            })
            .unwrap_or_default();
        zone_channel_catalog(
            &self.channels.channels,
            &name,
            in_city,
            capital,
            areas.and_then(|a| city_record_name(&a.0)),
        )
    }

    /// A channel id's `ZONECHANNELS` bit set, once the mask is seated.
    pub(crate) fn add_zone_bit(&mut self, id: u32) {
        if let Some(mask) = &mut self.channels.zone_mask {
            *mask |= super::edit::zone_bit(id);
        }
    }
}

/// Startup: read `ChatChannels.dbc`; without an install nothing auto-joins and arg7 stays 0.
pub(super) fn load_chat_channels(
    mut channels: ResMut<ChannelState>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_chat_channels_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("chat: {} ChatChannels rows", cat.rows().len());
            channels.channels = cat;
        }
        Err(e) => warn!("chat: ChatChannels.dbc failed to load — no zone channels: {e:#}"),
    }
}

/// The name a channel verb composes for `row` (`0x507b70`-`0x507bc0` for the join,
/// `0x5094b1`-`0x509505` for the leave): the city record's name when the zone's area is a capital
/// (`AreaTable.Flags & 0x100`), the row is city-named (`flags & 0x20`) and the record is cached,
/// else the zone text; `None`, the verb's bail, when that subject is empty, whatever the row.
fn compose(
    row: &benilla_formats::ChatChannelRow,
    zone_name: &str,
    capital: bool,
    city: Option<&str>,
) -> Option<String> {
    let subject = match city {
        Some(city) if capital && row.takes_city_name() => city,
        _ => zone_name,
    };
    if subject.is_empty() {
        return None;
    }
    Some(row.pattern.replacen("%s", subject, 1))
}

/// The name a channel verb composes for `row` in the zone at the call (`[0xbd080c]`, its area record
/// and text): [`compose`] over the live inputs.
pub(super) fn verb_name(
    row: &benilla_formats::ChatChannelRow,
    areas: Option<&AreaTableRes>,
    zone: u32,
) -> Option<String> {
    let (name, capital) = areas
        .and_then(|a| a.0.get(zone))
        .map(|r| (r.name.clone(), r.flags & AREA_FLAG_CAPITAL != 0))
        .unwrap_or_default();
    compose(
        row,
        &name,
        capital,
        areas.and_then(|a| city_record_name(&a.0)),
    )
}

/// The cached city record's name (`[0xb74464] + 0x2c`), empty or not: the `AreaTable.dbc` row with
/// flag `0x200`.
fn city_record_name(areas: &benilla_formats::AreaTableCatalog) -> Option<&str> {
    areas
        .rows()
        .find(|r| r.flags & AREA_FLAG_CITY_NAME_ROW != 0)
        .map(|r| r.name.as_str())
}

/// The zone walk's composition, as [`plan_walk`] reads its names (the walk's own shape is #1515).
fn walk_compose(
    row: &benilla_formats::ChatChannelRow,
    zone_name: &str,
    city_word: Option<&str>,
) -> Option<String> {
    if row.is_zone_dependent() && zone_name.is_empty() {
        return None;
    }
    if row.takes_city_name() && city_word.is_none() {
        return None;
    }
    Some(row.joinable_name(zone_name, city_word.unwrap_or_default()))
}

/// The names the walk registers on a character with no slots, in table order: the reference
/// registers a slot before its city gate, so a city-only row takes its number even outside a
/// capital. The `N.` of a channel line is that slot, never the ChannelID.
#[cfg(test)]
pub(crate) fn tracked_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter_map(|r| walk_compose(r, zone_name, city_word))
        .collect()
}

/// [`tracked_channels`] with the city gate applied: the names the walk joins. `in_city` gates the
/// city-only rows (`0x10`); `city_word` fills a city-named row's (`0x20`) `%s`.
#[cfg(test)]
pub(crate) fn wanted_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter(|r| in_city || !r.is_city_only())
        .filter_map(|r| walk_compose(r, zone_name, city_word))
        .collect()
}

/// One step of a walk, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WalkStep {
    /// `CMSG_LEAVE_CHANNEL` of the old name, from a confirmed slot only.
    Leave(String),
    /// `CMSG_JOIN_CHANNEL`.
    Join(String),
    /// Renamed in place at send time ([`ChannelState::rename_slot`]).
    Rename { old: String, new: String },
    /// A slot claimed for an untracked row, before the city gate, with the row's ChannelID.
    Register(String, u32),
    /// State 3, the row stopped applying; the record and its number survive.
    Suspend(String),
    /// The state-3 bypass with the name unchanged: the reference writes `(old == 3) ? 1 : 2`, so
    /// the slot confirms with a plain `YOU_JOINED`.
    Confirm(String),
}

/// The walk as a plan: the reference's two passes, for a player in `zone_name`. Pass 1 composes
/// each held slot, in slot order, for this zone whatever the mask says. A moved name leaves if
/// confirmed, then the city gate suspends it or renames and re-joins it; a suspended slot the gate
/// admits re-joins (the state-3 bypass). A row that stopped applying under an unchanged name, or no
/// longer composes, leaves and suspends here, where the reference skips any other unchanged name:
/// its city-named rows move to the zone name outside a capital ([`walk_compose`]). Pass 2 registers the
/// unslotted rows whose mask bit is set, in DBC order, before the gate.
pub(crate) fn plan_walk(
    channels: &ChannelState,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<WalkStep> {
    use WalkStep as W;
    let catalog = &channels.channels;
    let eligible = |row: &benilla_formats::ChatChannelRow| in_city || !row.is_city_only();
    let mut steps = Vec::new();
    let mut tracked_rows: Vec<u32> = Vec::new();

    // Pass 1.
    for slot in channels.joined.iter().flatten() {
        let Some(row) = catalog.row_for_name(&slot.name) else {
            continue; // a custom channel: not the walk's
        };
        tracked_rows.push(row.id);
        let old = slot.name.as_str();
        match walk_compose(row, zone_name, city_word) {
            Some(new) if new.eq_ignore_ascii_case(old) => {
                if slot.state == SlotState::Suspended {
                    if eligible(row) {
                        steps.push(W::Confirm(old.to_string()));
                        steps.push(W::Join(old.to_string()));
                    }
                } else if !eligible(row) {
                    if slot.state == SlotState::Joined {
                        steps.push(W::Leave(old.to_string()));
                    }
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            Some(new) => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if eligible(row) {
                    steps.push(W::Rename {
                        old: old.to_string(),
                        new: new.clone(),
                    });
                    steps.push(W::Join(new));
                } else {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            None => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if slot.state != SlotState::Suspended {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
        }
    }

    // Pass 2.
    for row in catalog.rows() {
        if !channels.zone_row_wanted(row.id) || tracked_rows.contains(&row.id) {
            continue;
        }
        let Some(name) = walk_compose(row, zone_name, city_word) else {
            continue;
        };
        steps.push(W::Register(name.clone(), row.id));
        if eligible(row) {
            steps.push(W::Join(name));
        } else {
            steps.push(W::Suspend(name));
        }
    }
    steps
}

/// Apply a plan in order. Nothing relists: neither the walk nor its join (`0x507a80`) calls the
/// list's update (`0x505d40`).
pub(crate) fn apply_walk(steps: &[WalkStep], channels: &mut ChannelState, commands: &NetCommands) {
    for step in steps {
        match step {
            WalkStep::Leave(name) => {
                debug!("chat: leaving zone channel {name:?}");
                let _ = commands.0.send(channels.leave_command(name.clone()));
            }
            WalkStep::Join(name) => {
                debug!("chat: joining zone channel {name:?}");
                // The walk's own join: 3.3.5a's passes auto 1, voice 0, and the row's ChannelID,
                // which the record it registered or renamed carries.
                let _ = commands.0.send(channels.join_command(
                    channels.slot_id(name),
                    name.clone(),
                    String::new(),
                    true,
                    false,
                ));
            }
            WalkStep::Rename { old, new } => {
                if let Some(n) = channels.rename_slot(old, new) {
                    debug!("chat: zone channel slot {n} renamed {old:?} → {new:?}");
                }
            }
            WalkStep::Register(name, id) => match channels.claim_walk_slot(name, *id) {
                Some(n) => debug!("chat: zone channel {name:?} registered as slot {n}"),
                None => warn!(
                    "chat: no free slot for zone channel {name:?} — all {} are taken",
                    super::edit::MAX_CHANNELS
                ),
            },
            WalkStep::Suspend(name) => {
                if let Some(n) = channels.suspend_slot(name) {
                    debug!("chat: zone channel slot {n} ({name:?}) suspended — not eligible here");
                }
            }
            WalkStep::Confirm(name) => channels.confirm_slot(name),
        }
    }
}

/// The zone update's walk leg (`0x5080ba`-`0x508249`) over `real_zone`, the stored real zone text
/// (`[0xbd0780]`), in the zone id (`[0xbd080c]`, `0x5080c0`): whether the update goes on to the
/// cascade. A ready chat goes straight on (`0x5080ca`), a zone with no area record stops there
/// (`0x5080dd`), else the walk runs and sets the ready flag (`0x508249`).
pub(super) fn zone_walk(
    In(real_zone): In<String>,
    commands: Res<NetCommands>,
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    ids: Option<Res<crate::area::ZoneIds>>,
    mut walk: ResMut<ZoneChannelWalk>,
) -> bool {
    let zone = ids.map_or(0, |z| z.zone);
    if walk.ready {
        return true;
    }
    let Some(areas) = areas else {
        return false;
    };
    let Some(row) = areas.0.get(zone) else {
        return false;
    };
    let in_city = row.flags & AREA_FLAG_TRADE_CHANNEL != 0;
    let steps = plan_walk(&channels, &real_zone, in_city, city_word(&areas.0));
    apply_walk(&steps, &mut channels, &commands);
    walk.ready = true;
    true
}

/// Every `ChatChannels.dbc` row as the VM needs it: the id, the Shortcut the verbs match a typed
/// name against, the name composed here (`None` is the verbs' nil leg), and whether
/// `EnumerateServerChannels` lists it (a city-only row, `flags & 0x10`, only in a city).
pub(crate) fn zone_channel_catalog(
    catalog: &ChatChannelsCatalog,
    zone_name: &str,
    in_city: bool,
    capital: bool,
    city: Option<&str>,
) -> Vec<benilla_ui::script::ZoneChannelRow> {
    catalog
        .rows()
        .iter()
        .map(|r| benilla_ui::script::ZoneChannelRow {
            id: r.id,
            shortcut: r.shortcut.clone(),
            resolved: compose(r, zone_name, capital, city),
            listed: !r.is_city_only() || in_city,
            auto_join: r.flags & 1 != 0,
            guild_recruitment: r.is_guild_recruitment(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::ChatChannelRow;
    use WalkStep as W;

    /// The shipped 3.3.5a table, hand-built so the tests run without an install.
    fn catalog() -> ChatChannelsCatalog {
        ChatChannelsCatalog::from_rows(
            [
                (1, 0x80003, "General - %s", "General"),
                (2, 0x0003B, "Trade - %s", "Trade"),
                (22, 0x10003, "LocalDefense - %s", "LocalDefense"),
                (23, 0x10004, "WorldDefense", "WorldDefense"),
                (25, 0x20032, "GuildRecruitment - %s", "GuildRecruitment"),
                (26, 0x40039, "LookingForGroup", "LookingForGroup"),
            ]
            .into_iter()
            .map(|(id, flags, pattern, shortcut)| ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            })
            .collect(),
        )
    }

    /// The city word, row 3459 of the shipped `AreaTable.dbc`.
    const CITY: Option<&str> = Some("City");

    /// General, Trade and LocalDefense.
    const SEED: u32 = 0x0020_0003;
    /// `GuildRecruitment`'s bit, `1 << (25 - 1)`, which the cascade's confirmed join sets.
    const GUILD_RECRUITMENT_BIT: u32 = 1 << 24;

    fn state(mask: u32) -> ChannelState {
        ChannelState {
            channels: catalog(),
            zone_mask: Some(mask),
            ..Default::default()
        }
    }

    /// A record in state 0, carrying the ChannelID its claim stored.
    fn joined(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot::zone(
            name,
            catalog().zone_channel_id(name),
        ))
    }

    fn suspended(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot {
            name: name.into(),
            id: catalog().zone_channel_id(name),
            state: SlotState::Suspended,
            leave_asked: false,
            flags: 0,
            instance: 0,
            voice: false,
            permanent: false,
        })
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// WorldDefense, LookingForGroup and GuildRecruitment have no bit in a fresh mask.
    #[test]
    fn an_ordinary_zone_joins_general_and_local_defense() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Elwynn Forest", false, CITY),
            vec!["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
    }

    #[test]
    fn a_capital_adds_the_shared_trade_channel() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", true, CITY),
            vec![
                "General - Stormwind City",
                "Trade - City",
                "LocalDefense - Stormwind City",
            ]
        );
    }

    /// "General - " would be a real channel on the server.
    #[test]
    fn an_unknown_zone_joins_nothing_zone_dependent() {
        assert!(wanted_channels(&catalog(), SEED, "", false, CITY).is_empty());
        assert!(wanted_channels(&catalog(), SEED, "", true, CITY).is_empty());
    }

    #[test]
    fn leaving_the_world_ends_the_channel_session() {
        let mut channels = state(SEED);
        channels.joined = vec![
            joined("General - Tanaris"),
            joined("LocalDefense - Tanaris"),
        ];
        let mut walk = ZoneChannelWalk::ready_chat();

        // No VM in a unit test: the mirror leg is the one line this cannot reach.
        end_channel_session(&mut channels, &mut walk);

        assert!(!walk.ready(), "the next entry re-walks from scratch");
        assert!(
            channels.joined.is_empty(),
            "the confirmed list is the server's, and the server just destroyed it — a survivor \
             here is what renumbers the next character's channels"
        );
        assert_eq!(
            channels.zone_mask,
            Some(SEED),
            "the mask is the character's file, not the session's"
        );
    }

    /// A locale whose sentinel row ships blank: the deviation in `compose`.
    #[test]
    fn a_missing_city_word_skips_the_trade_channel_rather_than_half_naming_it() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", true, None),
            vec!["General - Stormwind City", "LocalDefense - Stormwind City"]
        );
    }

    #[test]
    fn crossing_a_zone_border_renames_the_same_rows() {
        let cat = catalog();
        let felwood = wanted_channels(&cat, SEED, "Felwood", false, CITY);
        let winterspring = wanted_channels(&cat, SEED, "Winterspring", false, CITY);
        assert_ne!(felwood, winterspring, "the names differ");
        let ids = |v: &[String]| -> Vec<u32> { v.iter().map(|n| cat.zone_channel_id(n)).collect() };
        assert_eq!(
            ids(&felwood),
            ids(&winterspring),
            "…but they are the same DBC rows: [1, 22] either side of the border"
        );
        assert_eq!(ids(&felwood), vec![1, 22]);
    }

    /// The walk's join sends the ChannelID its record was registered with, and its leave the
    /// record's own (`0x507c7e`, `0x5095f1`).
    #[test]
    fn the_walks_packets_carry_the_records_channel_ids() {
        let mut c = state(SEED);
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = crate::net::NetCommands(tx);
        apply_walk(
            &plan_walk(&c, "Elwynn Forest", false, CITY),
            &mut c,
            &commands,
        );
        apply_walk(&plan_walk(&c, "Westfall", false, CITY), &mut c, &commands);
        let wire: Vec<(bool, u32, String)> = rx
            .try_iter()
            .filter_map(|cmd| match cmd {
                crate::net::ClientCommand::JoinChannel {
                    channel_id, name, ..
                } => Some((true, channel_id, name)),
                crate::net::ClientCommand::LeaveChannel { channel_id, name } => {
                    Some((false, channel_id, name))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            wire,
            vec![
                (true, 1, s("General - Elwynn Forest")),
                (true, 22, s("LocalDefense - Elwynn Forest")),
                (false, 1, s("General - Elwynn Forest")),
                (true, 1, s("General - Westfall")),
                (false, 22, s("LocalDefense - Elwynn Forest")),
                (true, 22, s("LocalDefense - Westfall")),
            ]
        );
    }

    #[test]
    fn a_first_walk_registers_every_mask_row_before_the_gate() {
        let mut c = state(SEED);
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Elwynn Forest"), 1),
                W::Join(s("General - Elwynn Forest")),
                W::Register(s("Trade - City"), 2),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest"), 22),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(1));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("LocalDefense - Elwynn Forest"), Some(3));

        let steps = plan_walk(&state(SEED), "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Stormwind City"), 1),
                W::Join(s("General - Stormwind City")),
                W::Register(s("Trade - City"), 2),
                W::Join(s("Trade - City")),
                W::Register(s("LocalDefense - Stormwind City"), 22),
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
    }

    /// Apply a plan's slot writes as the system does, without the wire.
    fn apply_steps(steps: &[WalkStep], c: &mut ChannelState) {
        for step in steps {
            match step {
                W::Leave(_) | W::Join(_) => {}
                W::Rename { old, new } => {
                    c.rename_slot(old, new);
                }
                W::Register(name, id) => {
                    c.claim_walk_slot(name, *id);
                }
                W::Suspend(name) => {
                    c.suspend_slot(name);
                }
                W::Confirm(name) => c.confirm_slot(name),
            }
        }
    }

    /// The suspended Trade slot between the two is walked too, and stays as it is.
    #[test]
    fn a_border_crossing_is_leave_rename_join_per_slot_in_slot_order() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Felwood"),
            suspended("Trade - City"),
            joined("LocalDefense - Felwood"),
        ];
        let steps = plan_walk(&c, "Winterspring", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Felwood")),
                W::Rename {
                    old: s("General - Felwood"),
                    new: s("General - Winterspring")
                },
                W::Join(s("General - Winterspring")),
                W::Leave(s("LocalDefense - Felwood")),
                W::Rename {
                    old: s("LocalDefense - Felwood"),
                    new: s("LocalDefense - Winterspring")
                },
                W::Join(s("LocalDefense - Winterspring")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(
            c.number_of("General - Winterspring"),
            Some(1),
            "renamed in place"
        );
        assert_eq!(
            c.slot_state("General - Winterspring"),
            Some(SlotState::Renamed)
        );
        assert_eq!(c.number_of("Trade - City"), Some(2), "untouched");
        assert_eq!(c.number_of("LocalDefense - Winterspring"), Some(3));
    }

    /// Trade suspends on the way out and re-joins through the state-3 bypass.
    #[test]
    fn a_city_round_trip_keeps_the_slot_and_its_number() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];

        // Out of the city.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Stormwind City")),
                W::Rename {
                    old: s("General - Stormwind City"),
                    new: s("General - Elwynn Forest")
                },
                W::Join(s("General - Elwynn Forest")),
                W::Leave(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Leave(s("LocalDefense - Stormwind City")),
                W::Rename {
                    old: s("LocalDefense - Stormwind City"),
                    new: s("LocalDefense - Elwynn Forest")
                },
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(
            c.number_of("LocalDefense - Elwynn Forest"),
            Some(3),
            "and nothing above it renumbered — the slot went quiet, it did not go away"
        );
        // The notices confirm the two renamed rows.
        c.confirm_slot("General - Elwynn Forest");
        c.confirm_slot("LocalDefense - Elwynn Forest");

        // Back in: the bypass joins and clears the suspension, with no LEAVE.
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Elwynn Forest")),
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Stormwind City")
                },
                W::Join(s("General - Stormwind City")),
                W::Confirm(s("Trade - City")),
                W::Join(s("Trade - City")),
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Stormwind City")
                },
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Joined));
        assert_eq!(c.number_of("Trade - City"), Some(2), "still slot 2");
    }

    /// The explicit leave clears the bit and `YOU_LEFT` frees the slot, so neither pass walks
    /// General again, this session or the next.
    #[test]
    fn a_left_channel_stays_left_this_session_and_the_next() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            joined("LocalDefense - Elwynn Forest"),
        ];

        // `/leave General`: the stock `SlashCmdList["LEAVE"]` hands `LeaveChannelByName` the bare
        // token and the VM composes it for the zone; `/leave 1` names the slot here.
        let target = "General - Elwynn Forest";
        assert_eq!(target, "General - Elwynn Forest", "the slot's wire name");
        c.note_zone_channel_left(target);
        assert_eq!(c.zone_mask, Some(SEED & !1), "bit 0 cleared");
        // The server's `YOU_LEFT` frees the slot in place.
        assert_eq!(c.free_slot(target), Some(1));

        // The next border: General is neither walked (no slot) nor registered (no bit).
        let steps = plan_walk(&c, "Westfall", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Westfall")
                },
                W::Join(s("LocalDefense - Westfall")),
            ]
        );

        // The next login: a fresh slot array under the file's mask.
        let relog = state(SEED & !1);
        let steps = plan_walk(&relog, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("Trade - City"), 2),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest"), 22),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ],
            "General is gone for good; Trade is now number 1"
        );

        // Until a `/join General` claims its slot at send and its confirm sets the bit back.
        let mut relog = relog;
        apply_steps(&steps, &mut relog);
        relog.claim_slot("General - Elwynn Forest");
        relog.note_zone_channel_joined("General - Elwynn Forest");
        assert_eq!(relog.zone_mask, Some(SEED));
        let steps = plan_walk(&relog, "Westfall", false, CITY);
        assert!(
            steps.contains(&W::Join(s("General - Westfall"))),
            "walked again from its slot: {steps:?}"
        );
    }

    /// Zero is a real state, every zone channel left; unseated means the file is not read yet.
    #[test]
    fn a_zero_or_unseated_mask_registers_nothing() {
        assert!(plan_walk(&state(0), "Elwynn Forest", false, CITY).is_empty());
        let unseated = ChannelState {
            channels: catalog(),
            zone_mask: None,
            ..Default::default()
        };
        assert!(plan_walk(&unseated, "Elwynn Forest", false, CITY).is_empty());
        assert!(!unseated.zone_row_wanted(1));
    }

    /// Once the cascade's confirmed join sets bit 24, `GuildRecruitment` takes the same city gate
    /// (`0x10`) and city name (`0x20`) as Trade, last in DBC order.
    #[test]
    fn the_guild_recruitment_bit_rides_the_walk_like_trade() {
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Elwynn Forest",
            false,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City"), 25),
                W::Suspend(s("GuildRecruitment - City")),
            ],
            "registered and suspended out here, number 4"
        );
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Ironforge",
            true,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City"), 25),
                W::Join(s("GuildRecruitment - City")),
            ]
        );
    }

    /// A `/join General` after a `/leave`, or the cascade's own join: pass 1 is the slot array.
    #[test]
    fn a_channel_joined_outside_the_walk_is_walked_from_its_slot() {
        let mut c = state(SEED | GUILD_RECRUITMENT_BIT);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];
        // The cascade's join: a slot, then the notice sets the bit.
        c.claim_slot("GuildRecruitment - City");
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert!(
            steps.is_empty(),
            "same zone, every name unchanged, every row eligible — nothing to do: {steps:?}"
        );
        // Walking out suspends it exactly like Trade.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert!(steps.contains(&W::Leave(s("GuildRecruitment - City"))));
        assert!(steps.contains(&W::Suspend(s("GuildRecruitment - City"))));
        assert!(!steps.iter().any(|st| matches!(st, W::Register(..))));
    }

    /// State 3 sends no LEAVE, and the rename's `(old == 3) ? 1 : 2` lands it on 1, a plain
    /// `YOU_JOINED`.
    #[test]
    fn a_suspended_slot_whose_name_moved_rejoins_without_a_leave() {
        let mut c = state(SEED);
        c.joined = vec![suspended("General - Elwynn Forest")];
        let steps = plan_walk(&c, "Westfall", false, CITY);
        assert_eq!(
            &steps[..2],
            &[
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Westfall")
                },
                W::Join(s("General - Westfall")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(
            c.slot_state("General - Westfall"),
            Some(SlotState::Pending),
            "3 → 1, not 2"
        );
    }

    #[test]
    fn the_leave_clears_the_mask_bit_of_a_slot_carrying_the_wire_name() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            None,
            joined("MyChan"),
        ];
        // The mask clear needs a slot carrying the wire name.
        c.note_zone_channel_left("General - Nowhere");
        assert_eq!(c.zone_mask, Some(SEED), "no slot carries it: no clear");
        c.note_zone_channel_left("General - Elwynn Forest");
        assert_eq!(c.zone_mask, Some(SEED & !1));
    }

    #[test]
    fn the_slot_set_and_the_join_set_differ_only_by_the_city_gate() {
        let cat = catalog();
        assert_eq!(
            tracked_channels(&cat, SEED, "Stormwind City", None),
            wanted_channels(&cat, SEED, "Stormwind City", true, None),
            "no city word: the city-NAMED rows are unresolvable, so neither list carries them"
        );
        assert!(
            tracked_channels(&cat, SEED, "", CITY).is_empty(),
            "no zone: nothing zone-dependent composes, so nothing takes a slot either"
        );
    }

    /// Zone-less is not empty: every row stays, and a zone-dependent one carries `resolved: None`,
    /// the reference's nil leg, not the custom-channel leg.
    #[test]
    fn a_zoneless_catalog_still_carries_every_row() {
        let rows = zone_channel_catalog(&catalog(), "", false, false, None);
        assert_eq!(rows.len(), 6, "every DBC row is present, zone or no zone");

        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(by("General").id, 1, "…and each carries its ChannelID");
        assert_eq!(
            by("General").resolved,
            None,
            "a zone-dependent row with no zone is UNRESOLVED — the nil leg, not the custom leg"
        );
        assert_eq!(by("Trade").resolved, None);
        assert_eq!(
            by("WorldDefense").resolved,
            None,
            "an empty zone text bails every row, one with no %s included (`0x507b9b`)"
        );

        // Once a zone is known, the same rows resolve.
        let rows = zone_channel_catalog(&catalog(), "Elwynn Forest", false, false, CITY);
        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(
            by("General").resolved,
            Some("General - Elwynn Forest".to_string())
        );
        assert!(
            !by("Trade").listed,
            "…and Trade is city-only, so it is not listed out here"
        );
    }

    /// The verbs' composition (`0x507b70`, `0x5094b1`): a city-named row takes the city record's
    /// name only in a capital's area, the zone text elsewhere or with no record; an empty subject
    /// bails (`0x507b9e`, `0x5094e3`), which `LeaveChannelByName` composes exactly as the join.
    #[test]
    fn the_verbs_compose_the_city_name_only_in_a_capital() {
        let resolved = |zone: &str, capital: bool, city: Option<&str>, shortcut: &str| {
            zone_channel_catalog(&catalog(), zone, capital, capital, city)
                .into_iter()
                .find(|r| r.shortcut == shortcut)
                .unwrap()
                .resolved
        };
        assert_eq!(
            resolved("Stormwind City", true, CITY, "Trade").as_deref(),
            Some("Trade - City")
        );
        assert_eq!(
            resolved("Stormwind City", true, CITY, "General").as_deref(),
            Some("General - Stormwind City"),
            "a row without the city bit takes the zone text in a capital"
        );
        assert_eq!(
            resolved("Elwynn Forest", false, CITY, "Trade").as_deref(),
            Some("Trade - Elwynn Forest"),
            "outside a capital a city-named row takes the zone text"
        );
        assert_eq!(
            resolved("Stormwind City", true, None, "Trade").as_deref(),
            Some("Trade - Stormwind City"),
            "no city record: the zone text"
        );
        assert_eq!(
            resolved("Stormwind City", true, Some(""), "Trade"),
            None,
            "an empty city name is an empty subject"
        );
    }

    /// arg7: a composed name maps back to its ChannelID by vmangos's substring match
    /// (`DBCStores.cpp:531`); a custom one to 0.
    #[test]
    fn composed_names_carry_their_channel_id_back() {
        let cat = catalog();
        assert_eq!(cat.zone_channel_id("General - Elwynn Forest"), 1);
        assert_eq!(cat.zone_channel_id("Trade - City"), 2);
        assert_eq!(cat.zone_channel_id("LocalDefense - Durotar"), 22);
        assert_eq!(cat.zone_channel_id("World"), 0);
    }

    /// `LookingForGroup`'s bit, `1 << (26 - 1)`.
    const LFG_BIT: u32 = 1 << 25;

    /// The session's one walk asks for LookingForGroup only when the mask holds its bit, and the
    /// walk's city gate keeps that ask to a capital.
    #[test]
    fn the_walk_asks_for_looking_for_group_only_from_its_mask_bit_in_a_capital() {
        let joins = |mask: u32, zone: &str, in_city: bool| -> Vec<String> {
            plan_walk(&state(mask), zone, in_city, CITY)
                .into_iter()
                .filter_map(|st| match st {
                    W::Join(n) => Some(n),
                    _ => None,
                })
                .collect()
        };
        assert!(!joins(SEED, "Orgrimmar", true).contains(&s("LookingForGroup")));
        assert!(joins(SEED | LFG_BIT, "Orgrimmar", true).contains(&s("LookingForGroup")));
        let durotar = plan_walk(&state(SEED | LFG_BIT), "Durotar", false, CITY);
        assert!(durotar.contains(&W::Suspend(s("LookingForGroup"))));
        assert!(!durotar.contains(&W::Join(s("LookingForGroup"))));
    }
}
