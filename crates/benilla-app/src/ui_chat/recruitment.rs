//! The `AUTO_JOIN_GUILD_CHANNEL` cascade, the one place the client joins or leaves
//! `GuildRecruitment - City` on its own. On AUTO (the chat's guild-recruitment mode, the default) a
//! guilded player leaves it and an unguilded one in a capital joins it (`0x49eb70`), with no
//! membership check; otherwise a retry waits for the next zone change. Once joined, the zone walk
//! owns it as it owns Trade. Triggers: the `guildRecruitmentChannel` CVar moving to 1 (`0x512750` →
//! the mode setter) and the `PLAYER_GUILDID` watcher, whose first sight is the player create; its
//! move to 0 leaves a joined channel. Deviation: no re-run when a remote player streams in, because
//! the reference's re-run only repeats the join, which vmangos swallows, or the leave, whose "Not
//! on channel" notice nothing prints.

#[cfg(test)]
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::channels::{city_word, ZoneChannelWalk};
use super::edit::ChannelState;

/// vmangos `AREA_FLAG_CAPITAL`, the cascade's capital test; the walk tests `0x8`, which marks the
/// same six rows in 1.12 data.
const AREA_FLAG_CAPITAL: u32 = 0x100;

/// The cascade's own state: whether a run is owed, and the guild id its watcher last saw.
#[derive(Resource, Default)]
pub(crate) struct GuildRecruitmentCascade {
    /// A run is owed: the one-shot and the tail-jump from the mode setter in one flag, kept while
    /// the cascade cannot act.
    pending: bool,
    /// The last `PLAYER_GUILDID` seen; `None` makes the first sight a change, the create trigger.
    last_guild_id: Option<u32>,
    /// A `guildRecruitmentChannel` move the cascade system applies, as it owns the VM.
    cvar: Option<u8>,
}

impl GuildRecruitmentCascade {
    /// A trigger fired: the CVar's move to 1, or the watcher.
    pub(super) fn request(&mut self) {
        self.pending = true;
    }

    /// The watcher: note the guild id, and request a run when it moved or is first seen.
    pub(super) fn observe_guild_id(&mut self, guild_id: u32) -> bool {
        if self.last_guild_id == Some(guild_id) {
            return false;
        }
        self.last_guild_id = Some(guild_id);
        self.pending = true;
        true
    }

    /// Session end: forget the owed run, the last guild id and a queued CVar move.
    pub(super) fn clear_session(&mut self) {
        self.pending = false;
        self.last_guild_id = None;
        self.cvar = None;
    }
}

/// `guildRecruitmentChannel`'s change callback (`0x512750`), queued for
/// [`guild_recruitment_cascade`]. Before the chat cache seats the latch it is dropped: the restore
/// overwrites the mode (`0x508320`).
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut state: ResMut<GuildRecruitmentCascade>,
    channels: Res<ChannelState>,
) {
    if ev.is("guildRecruitmentChannel") && channels.zone_mask.is_some() {
        state.cvar = Some(ev.num().clamp(0.0, 255.0) as u8);
    }
}

/// What the cascade does once the player and the zone row have resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cascade {
    /// Guilded: leave `GuildRecruitment - City`.
    Leave,
    /// Unguilded, in a capital: join `GuildRecruitment`.
    Join,
    /// Unguilded, not in a capital: arm the retry, no packet.
    Deferred,
}

/// The cascade's verdict: `None` when the latch is not AUTO, not even a retry. The guild test
/// comes first, so a guilded player leaves from anywhere.
pub(crate) fn cascade(auto: bool, guild_id: u32, in_capital: bool) -> Option<Cascade> {
    if !auto {
        return None;
    }
    if guild_id != 0 {
        return Some(Cascade::Leave);
    }
    Some(if in_capital {
        Cascade::Join
    } else {
        Cascade::Deferred
    })
}

/// The cascade, chained after the walk: the reference runs it at the walk's tail, on a fresh zone.
/// It owns no VM: the mode latch and the windows' list are read and written with the `World` lent,
/// and `UPDATE_CHAT_WINDOWS` fires in its turn.
pub(super) fn guild_recruitment_cascade(world: &mut World) {
    if world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .is_none()
    {
        return;
    }
    // The mode setter (`0x508260`), called with 0: unmoved is a no-op; 1 runs the cascade, 0
    // leaves a joined recruitment channel and refreshes the windows.
    let mode = world
        .get_resource_mut::<GuildRecruitmentCascade>()
        .and_then(|mut s| s.cvar.take());
    if let Some(mode) = mode {
        let moved = crate::ui_script::safe_world::lend(world, |s| {
            s.set_guild_recruitment_mode_from_cvar(mode)
        });
        if moved == Some(true) {
            if mode == 1 {
                world.resource_mut::<GuildRecruitmentCascade>().request();
            } else if let Ok(Some(relist)) = world.run_system_cached(cvar_leave) {
                // The leave's relist, then the setter's `UPDATE_CHAT_WINDOWS` with or without a
                // record (`0x50829f`, `0x5082ab`).
                if relist {
                    crate::ui_script::safe_world::lend(world, |s| s.channels_updated());
                }
                crate::ui_script::safe_world::raise(world, "UPDATE_CHAT_WINDOWS", vec![]);
            }
        }
    }
    let auto = crate::ui_script::safe_world::lend(world, |s| s.guild_recruitment_mode() == 1)
        .unwrap_or(false);
    let Ok(Some(ran)) = world.run_system_cached_with(cascade_run, auto) else {
        return;
    };
    // Both acting arms fire it; chat frames re-read channels on it.
    crate::ui_script::safe_world::lend(world, |s| {
        match ran {
            CascadeRan::Joined(Some((shortcut, id))) => {
                s.register_chat_window_channel(0, &shortcut, id);
            }
            // The leave's relist comes inside it, ahead of the event (`0x509680`).
            CascadeRan::Left { relist: true } => s.channels_updated(),
            _ => {}
        }
        s.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    });
}

/// What an acting cascade run did, for the events that follow it.
enum CascadeRan {
    /// The leave went out; `relist` when it freed a record still in state 1.
    Left { relist: bool },
    /// The join went out, or skipped its send; the window entry it registers, if any.
    Joined(Option<(String, u32)>),
}

/// The mode setter's leave at 0, with the row found: `None` without one, else whether the leave
/// freed a record and relists.
fn cvar_leave(
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    commands: Res<NetCommands>,
) -> Option<bool> {
    let Some(name) = areas.as_deref().and_then(|a| {
        let row = channels
            .channels
            .rows()
            .iter()
            .find(|r| r.is_guild_recruitment())?;
        Some(row.joinable_name("", city_word(&a.0)?))
    }) else {
        return None;
    };
    info!("chat: guildRecruitmentChannel 0 — leaving {name:?}");
    Some(cascade_leave(&mut channels, &commands, &name).unwrap_or(false))
}

/// One run of the cascade under the `auto` latch: `None` when it does nothing, else the window
/// entry a join registers ahead of `UPDATE_CHAT_WINDOWS`.
#[allow(clippy::option_option)]
fn cascade_run(
    In(auto): In<bool>,
    mut state: ResMut<GuildRecruitmentCascade>,
    mut channels: ResMut<ChannelState>,
    walk: Res<ZoneChannelWalk>,
    areas: Option<Res<AreaTableRes>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) -> Option<CascadeRan> {
    // Only while the avatar is streamed: a despawned body has not "left the guild".
    let guild_id = self_q.iter().next().map(|s| s.0.player_guild_id())?;
    state.observe_guild_id(guild_id);
    if !state.pending {
        return None;
    }
    // Wait for the chat-cache restore, which seats the latch and the mask.
    channels.zone_mask?;
    // The zone row: unresolvable ⇒ the retry stays armed.
    let areas = areas.as_deref()?;
    let row = walk.zone_id.and_then(|id| areas.0.get(id))?;
    let verdict = cascade(auto, guild_id, row.flags & AREA_FLAG_CAPITAL != 0);
    let Some(action) = verdict else {
        state.pending = false; // STANDARD: nothing, not even the retry
        return None;
    };
    if action == Cascade::Deferred {
        return None; // the retry stays armed
    }
    // The name is composed against the "City" row whatever the zone; with no row or city word the
    // run is consumed, not retried.
    let target = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())
        .cloned()
        .and_then(|row| {
            let name = row.joinable_name("", city_word(&areas.0)?);
            Some((row, name))
        });
    let Some((row, name)) = target else {
        state.pending = false;
        return None;
    };
    state.pending = false;
    // Guilded leaves a held record and unguilded joins with none; anything else does nothing, not
    // even the event (`0x507d98`-`0x507dc6`).
    let held = channels.leave_record(&name).is_some();
    match action {
        Cascade::Leave if !held => None,
        Cascade::Join if held => None,
        Cascade::Leave => {
            info!("chat: guild recruitment cascade — guilded, leaving {name:?}");
            let relist = cascade_leave(&mut channels, &commands, &name).unwrap_or(false);
            Some(CascadeRan::Left { relist })
        }
        Cascade::Join => {
            info!("chat: guild recruitment cascade — unguilded in a capital, joining {name:?}");
            // The quiet join (`0x507dea` -> `0x507a80`): a held record skips the send, a failed
            // claim also the window's list, with no line; no relist (`0x505d40`) either way.
            match channels.claim_for_join(&name, 0) {
                super::edit::JoinRecord::Claimed(n) => {
                    debug!("chat: {name:?} registered as slot {n}");
                    // The client's own join (`0x49eb70`).
                    let _ =
                        commands
                            .0
                            .send(channels.join_command(name, String::new(), true, false));
                }
                super::edit::JoinRecord::Held => {}
                super::edit::JoinRecord::Full | super::edit::JoinRecord::Refused => {
                    warn!("chat: no record for {name:?}; not joined");
                    return Some(CascadeRan::Joined(None));
                }
            }
            Some(CascadeRan::Joined(Some((row.shortcut.clone(), row.id))))
        }
        Cascade::Deferred => None,
    }
}

/// The forced leave (`0x5093f0` with its third argument 1): with no record nothing (`0x5095b2`);
/// otherwise the packet under the record's name in any state (`0x5095c0`), the mask bit
/// (`0x509639`), and a record still in state 1 freed (`0x50963f`-`0x50967a`). `Some(true)` when it
/// freed one, which relists (`0x509680`). No window loses its entry: the strip matches the full
/// name verbatim, which none holds.
fn cascade_leave(channels: &mut ChannelState, commands: &NetCommands, name: &str) -> Option<bool> {
    let slot = channels.slot(channels.leave_record(name)?)?;
    let (name, pending) = (
        slot.name.clone(),
        slot.state == super::edit::SlotState::Pending,
    );
    let _ = commands.0.send(channels.leave_command(name.clone()));
    channels.note_zone_channel_left(&name);
    channels.note_leave_asked(&name);
    if pending {
        channels.free_slot(&name);
    }
    Some(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four legs, and the order of its two tests.
    #[test]
    fn the_cascade_reads_the_latch_then_the_guild_then_the_capital() {
        assert_eq!(
            cascade(false, 0, true),
            None,
            "STANDARD: nothing, not even the retry"
        );
        assert_eq!(cascade(false, 7, true), None);
        assert_eq!(
            cascade(true, 7, false),
            Some(Cascade::Leave),
            "guilded leaves from anywhere — the guild test precedes the capital test"
        );
        assert_eq!(cascade(true, 7, true), Some(Cascade::Leave));
        assert_eq!(cascade(true, 0, true), Some(Cascade::Join));
        assert_eq!(cascade(true, 0, false), Some(Cascade::Deferred));
    }

    #[test]
    fn the_cascades_leave_keeps_the_windows_entry() {
        const GUILD_RECRUITMENT: u32 = 25;
        let mut script = UiScript::new().expect("a VM");
        assert!(script.register_chat_window_channel(0, "GuildRecruitment", GUILD_RECRUITMENT));
        let mut channels = ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: GUILD_RECRUITMENT,
                    flags: 0x32,
                    pattern: "GuildRecruitment - %s".into(),
                    shortcut: "GuildRecruitment".into(),
                },
            ]),
            zone_mask: Some(1 << (GUILD_RECRUITMENT - 1)),
            ..Default::default()
        };
        channels.claim_slot("GuildRecruitment - City");
        let (tx, rx) = crossbeam_channel::unbounded();

        assert_eq!(
            cascade_leave(&mut channels, &NetCommands(tx), "GuildRecruitment - City"),
            Some(false),
            "a confirmed record stays for its YOU_LEFT"
        );

        assert!(
            matches!(rx.try_recv(), Ok(crate::net::ClientCommand::LeaveChannel { name, .. }) if name == "GuildRecruitment - City"),
            "the CMSG_LEAVE_CHANNEL goes out"
        );
        assert_eq!(channels.zone_mask, Some(0), "the mask bit clears");
        assert_eq!(
            script.chat_window_looks()[0].channels,
            vec![("GuildRecruitment".to_string(), GUILD_RECRUITMENT)],
            "window 1 still carries the entry the join registered"
        );
    }

    /// The forced leave of a record whose join is unconfirmed sends and frees it (`0x50963f`-
    /// `0x50967a`), which relists; with no record it sends nothing (`0x5095b2`).
    #[test]
    fn the_cascades_leave_frees_a_pending_record_and_skips_a_missing_one() {
        const GUILD_RECRUITMENT: u32 = 25;
        let mut channels = ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: GUILD_RECRUITMENT,
                    flags: 0x2_0032,
                    pattern: "GuildRecruitment - %s".into(),
                    shortcut: "GuildRecruitment".into(),
                },
            ]),
            zone_mask: Some(0),
            ..Default::default()
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        assert_eq!(
            cascade_leave(&mut channels, &commands, "GuildRecruitment - City"),
            None
        );
        assert!(rx.try_recv().is_err(), "no record, no leave");
        channels.claim_for_join("GuildRecruitment - City", 0);
        assert_eq!(
            cascade_leave(&mut channels, &commands, "GuildRecruitment - City"),
            Some(true),
            "the pending record is freed and relists"
        );
        assert!(
            matches!(rx.try_recv(), Ok(crate::net::ClientCommand::LeaveChannel { name, .. }) if name == "GuildRecruitment - City")
        );
        assert_eq!(channels.number_of("GuildRecruitment - City"), None);
    }

    #[test]
    fn the_watcher_fires_on_first_sight_and_on_change() {
        let mut c = GuildRecruitmentCascade::default();
        assert!(!c.pending);
        assert!(c.observe_guild_id(0), "first sight, even of 'no guild'");
        assert!(c.pending);
        c.pending = false;
        assert!(!c.observe_guild_id(0), "unchanged");
        assert!(!c.pending);
        assert!(c.observe_guild_id(42), "joined a guild");
        assert!(c.pending);
        c.clear_session();
        assert!(!c.pending);
        assert!(c.observe_guild_id(42), "a new session sees it fresh");
    }

    /// The join arm writes the slot and window 1's list, then signals `UPDATE_CHAT_WINDOWS`
    /// (`0x81b530` runs its listeners in the caller): the listener reads the window's new entry.
    #[test]
    fn the_joins_listener_reads_the_windows_new_entry() {
        const GUILD_RECRUITMENT: u32 = 25;
        const CAPITAL: u32 = 1519;
        let row = |flags, name: &str| benilla_formats::AreaTableRow {
            map_id: 0,
            zone_id: 0,
            explore_flag: 0,
            flags,
            faction_group_mask: 0,
            exploration_level: 0,
            name: name.to_string(),
            ambient_multiplier: 0.0,
        };
        let mut walk = ZoneChannelWalk::default();
        walk.zone_id = Some(CAPITAL);
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .insert_resource(AreaTableRes(benilla_formats::AreaTableCatalog::from_rows(
                vec![
                    (CAPITAL, row(AREA_FLAG_CAPITAL, "Stormwind City")),
                    (1, row(0x200, "City")),
                ],
            )))
            .insert_resource(ChannelState {
                channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                    benilla_formats::ChatChannelRow {
                        id: GUILD_RECRUITMENT,
                        // `GUILD_REQ`, the row the cascade joins.
                        flags: 0x2_0032,
                        pattern: "GuildRecruitment - %s".into(),
                        shortcut: "GuildRecruitment".into(),
                    },
                ]),
                zone_mask: Some(0),
                ..Default::default()
            })
            .insert_resource(walk)
            .init_resource::<GuildRecruitmentCascade>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .add_systems(Update, guild_recruitment_cascade);
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
        ));
        app.world_mut()
            .resource_mut::<GuildRecruitmentCascade>()
            .request();
        let mut script = UiScript::new().expect("VM");
        script.set_guild_recruitment_mode(1);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_WINDOWS')
                 f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = tostring((GetChatWindowChannels(1))) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.update();
        let seen: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(
            seen, "GuildRecruitment",
            "the listener reads the joined entry"
        );
    }
}
