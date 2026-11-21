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
pub(super) fn guild_recruitment_cascade(
    mut script: crate::ui_script::InTurn,
    mut state: ResMut<GuildRecruitmentCascade>,
    mut channels: ResMut<ChannelState>,
    walk: Res<ZoneChannelWalk>,
    areas: Option<Res<AreaTableRes>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    // The mode setter (`0x508260`), called with 0: unmoved is a no-op; 1 runs the cascade, 0
    // leaves a joined recruitment channel and refreshes the windows.
    if let Some(mode) = state.cvar.take() {
        if script.set_guild_recruitment_mode_from_cvar(mode) {
            if mode == 1 {
                state.request();
            } else if let Some(name) = areas.as_deref().and_then(|a| {
                let row = channels
                    .channels
                    .rows()
                    .iter()
                    .find(|r| r.is_guild_recruitment())?;
                Some(row.joinable_name("", city_word(&a.0)?))
            }) {
                if channels.number_of(&name).is_some() {
                    info!("chat: guildRecruitmentChannel 0 — leaving {name:?}");
                    cascade_leave(&mut channels, &commands, name);
                    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
                }
            }
        }
    }
    // Only while the avatar is streamed: a despawned body has not "left the guild".
    let Some(guild_id) = self_q.iter().next().map(|s| s.0.player_guild_id()) else {
        return;
    };
    state.observe_guild_id(guild_id);
    if !state.pending {
        return;
    }
    // Wait for the chat-cache restore, which seats the latch and the mask.
    if channels.zone_mask.is_none() {
        return;
    }
    // The zone row: unresolvable ⇒ the retry stays armed.
    let Some(areas) = areas.as_deref() else {
        return;
    };
    let Some(row) = walk.zone_id.and_then(|id| areas.0.get(id)) else {
        return;
    };
    let auto = script.guild_recruitment_mode() == 1;
    let verdict = cascade(auto, guild_id, row.flags & AREA_FLAG_CAPITAL != 0);
    let Some(action) = verdict else {
        state.pending = false; // STANDARD: nothing, not even the retry
        return;
    };
    if action == Cascade::Deferred {
        return; // the retry stays armed
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
        return;
    };
    match action {
        Cascade::Leave => {
            info!("chat: guild recruitment cascade — guilded, leaving {name:?}");
            cascade_leave(&mut channels, &commands, name);
        }
        Cascade::Join => {
            info!("chat: guild recruitment cascade — unguilded in a capital, joining {name:?}");
            // The slot (by name), window 1's list, then the send, always.
            match channels.claim_slot(&name) {
                Some(n) => {
                    debug!("chat: {name:?} registered as slot {n}");
                    // After an earlier arm's `UPDATE_CHAT_WINDOWS` listeners, as the write below.
                    let slots = channels.ui_slots();
                    script.after_raises(move |s| s.set_channel_slots(slots));
                }
                None => warn!(
                    "chat: no free slot for {name:?} — all {} are taken",
                    super::edit::MAX_CHANNELS
                ),
            }
            let (shortcut, id) = (row.shortcut.clone(), row.id);
            script.after_raises(move |s| {
                s.register_chat_window_channel(0, &shortcut, id);
            });
            // The client's own join (`0x49eb70`).
            let _ = commands
                .0
                .send(channels.join_command(name, String::new(), true, false));
        }
        Cascade::Deferred => {}
    }
    // Both acting arms fire it; chat frames re-read channels on it.
    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    state.pending = false;
}

/// Leave by the full name: the packet and the mask bit. No window loses its entry: the strip
/// matches that name verbatim, which none holds.
fn cascade_leave(channels: &mut ChannelState, commands: &NetCommands, name: String) {
    let _ = commands.0.send(channels.leave_command(name.clone()));
    channels.note_zone_channel_left(&name);
    channels.note_leave_asked(&name);
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

        cascade_leave(
            &mut channels,
            &NetCommands(tx),
            "GuildRecruitment - City".into(),
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
        crate::ui_script::raises::fire_raises(app.world_mut());
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
