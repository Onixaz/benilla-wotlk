//! The guild-recruitment cascade (`0x507d30`), the one place the client joins or leaves the
//! `GuildRecruitment` channel on its own. Under AUTO (`[0xac7a3c] == 1`, the chat's
//! guild-recruitment mode) and once the chat is ready (`[0xbcefe8]`), with the active player held
//! and the `GUILD_REQ` row found (`0x4fe260`): a guilded player leaves a held record and an
//! unguilded one with no record joins it, the row's Shortcut composed as the channel verbs compose
//! it; both then signal `UPDATE_CHAT_WINDOWS`. It runs at the end of each zone update (`0x508254`),
//! from the mode setter's move to 1 (`0x508282`), at the active player's guild id change
//! (`0x6e1beb`) and at every player object's init, the active player's and any other's
//! (`0x6e82c5`).

#[cfg(test)]
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::channels::ZoneChannelWalk;
use super::edit::ChannelState;

/// The cascade's own state: whether a run is owed, and the guild id its watcher last saw.
#[derive(Resource, Default)]
pub(crate) struct GuildRecruitmentCascade {
    /// A run is owed by one of its callers.
    pending: bool,
    /// The last `PLAYER_GUILDID` seen; `None` makes the first sight a change.
    last_guild_id: Option<u32>,
    /// A `guildRecruitmentChannel` move the cascade system applies, as it owns the VM.
    cvar: Option<u8>,
}

impl GuildRecruitmentCascade {
    /// One of the cascade's callers ran: a zone update, the mode setter, a player's init.
    pub(crate) fn request(&mut self) {
        self.pending = true;
    }

    /// The active player's guild id handler (`0x6e1bb0`): a moved id runs the cascade (`0x6e1beb`).
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

/// The cascade after the zone walk, the tail of the zone update. It calls Lua only with the
/// `World` lent: the mode latch and the join's window entry are read and written so, the leave's
/// strip is a plain model write, and `UPDATE_CHAT_WINDOWS` fires in its turn.
pub(super) fn guild_recruitment_cascade(world: &mut World) {
    if world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .is_none()
    {
        return;
    }
    // The mode setter (`0x508260`), called with 0: unmoved is a no-op (`0x50826c`), and before the
    // chat is ready it only stores the mode (`0x50826e`); then 1 runs the cascade (`0x508282`) and
    // 0 leaves the channel and refreshes the windows (`0x50828d`-`0x5082ab`).
    let mode = world
        .get_resource_mut::<GuildRecruitmentCascade>()
        .and_then(|mut s| s.cvar.take());
    if let Some(mode) = mode {
        let moved = crate::ui_script::safe_world::lend(world, |s| {
            s.set_guild_recruitment_mode_from_cvar(mode)
        });
        let ready = world
            .get_resource::<ZoneChannelWalk>()
            .is_some_and(ZoneChannelWalk::ready);
        if moved == Some(true) && ready {
            if mode == 1 {
                world.resource_mut::<GuildRecruitmentCascade>().request();
            } else if let Ok(Some(relist)) = world.run_system_cached(cvar_leave) {
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
    /// The leave ran; `relist` when it freed a record still in state 1.
    Left { relist: bool },
    /// The join ran; the window entry it registers when it went on (`0x507cea`).
    Joined(Option<(String, u32)>),
}

/// The mode setter's leave at 0 (`0x50828d` -> `0x5093f0`), with the row found: `None` without
/// one, else whether the leave freed a record and relists.
fn cvar_leave(
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    zone: Option<Res<crate::area::ZoneIds>>,
    commands: Res<NetCommands>,
    mut vm: NonSendMut<benilla_ui::script::UiScript>,
) -> Option<bool> {
    let row = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())?
        .clone();
    let zone = zone.map_or(0, |z| z.zone);
    info!(
        "chat: guildRecruitmentChannel 0 — leaving {:?}",
        row.shortcut
    );
    Some(
        cascade_leave(
            &mut channels,
            &commands,
            &mut vm,
            &row,
            areas.as_deref(),
            zone,
        )
        .unwrap_or(false),
    )
}

/// One run of the cascade under the `auto` latch (`0x507d30`): `None` when it returns before
/// acting, else what it did ahead of `UPDATE_CHAT_WINDOWS`.
fn cascade_run(
    In(auto): In<bool>,
    mut state: ResMut<GuildRecruitmentCascade>,
    mut channels: ResMut<ChannelState>,
    walk: Res<ZoneChannelWalk>,
    (areas, zone): (Option<Res<AreaTableRes>>, Option<Res<crate::area::ZoneIds>>),
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    (commands, mut vm): (Res<NetCommands>, NonSendMut<benilla_ui::script::UiScript>),
) -> Option<CascadeRan> {
    // The active player's guild id handler, for as long as the object is held.
    let guild_id = self_q.iter().next().map(|s| s.0.player_guild_id());
    if let Some(id) = guild_id {
        state.observe_guild_id(id);
    }
    if !std::mem::take(&mut state.pending) {
        return None;
    }
    // AUTO (`0x507d36`), the ready flag (`0x507d43`), the active player (`0x507d70`), the
    // `GUILD_REQ` row (`0x507d7d`).
    if !auto || !walk.ready() {
        return None;
    }
    let guild_id = guild_id?;
    let row = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())?
        .clone();
    // The record by the row's ChannelID, in any state (`0x507d85` -> `0x4fe020`).
    let held = channels
        .joined
        .iter()
        .flatten()
        .any(|s| channels.channels.zone_channel_id(&s.name) == row.id);
    let zone = zone.map_or(0, |z| z.zone);
    if guild_id != 0 {
        // Guilded with a record: the forced leave (`0x507da7`); with none, nothing (`0x507dc0`).
        if !held {
            return None;
        }
        info!(
            "chat: guild recruitment cascade — guilded, leaving {:?}",
            row.shortcut
        );
        let relist = cascade_leave(
            &mut channels,
            &commands,
            &mut vm,
            &row,
            areas.as_deref(),
            zone,
        )
        .unwrap_or(false);
        return Some(CascadeRan::Left { relist });
    }
    // Unguilded with a record: nothing (`0x507dc6`); with none, the join (`0x507dea`).
    if held {
        return None;
    }
    // The join composes the Shortcut as the verbs do; an empty subject returns before the claim
    // and the window's entry (`0x507b9e`).
    let Some(name) = super::channels::verb_name(&row, areas.as_deref(), zone) else {
        return Some(CascadeRan::Joined(None));
    };
    info!("chat: guild recruitment cascade — unguilded, joining {name:?}");
    // The claim, quiet: a full table adds no line (`0x50774f`) and no window entry.
    match channels.claim_for_join(&name, 0) {
        super::edit::JoinRecord::Claimed(n) => {
            // Voice 0, permanent 1 (`0x507dcb`, `0x507dc9`).
            channels.set_join_flags(n, false, true);
            let _ = commands
                .0
                .send(channels.join_command(name, String::new(), true, false));
        }
        super::edit::JoinRecord::Held => {}
        super::edit::JoinRecord::Full | super::edit::JoinRecord::Refused => {
            return Some(CascadeRan::Joined(None));
        }
    }
    Some(CascadeRan::Joined(Some((row.shortcut.clone(), row.id))))
}

/// The forced leave (`0x5093f0` with its third argument 1) of the `GUILD_REQ` row's Shortcut
/// (`0xb759e0`, copied at `0x4fe2af`): the Shortcut composes as the verbs compose it, an empty
/// subject returning before anything (`0x5094e3`); the Shortcut is the strip key (`0x5094f3`),
/// stripped from all ten windows before any record (`0x50952f`-`0x509560`). Then with no record
/// nothing (`0x5095b2`); otherwise the packet under the record's name in any state (`0x5095c0`), the
/// mask bit (`0x509639`), and a record still in state 1 freed (`0x50963f`-`0x50967a`). `Some(true)`
/// when it freed one, which relists (`0x509680`).
fn cascade_leave(
    channels: &mut ChannelState,
    commands: &NetCommands,
    vm: &mut benilla_ui::script::UiScript,
    row: &benilla_formats::ChatChannelRow,
    areas: Option<&AreaTableRes>,
    zone: u32,
) -> Option<bool> {
    super::channels::verb_name(row, areas, zone)?;
    vm.strip_chat_window_channel(&row.shortcut);
    let n = channels
        .joined
        .iter()
        .position(|s| {
            s.as_ref()
                .is_some_and(|s| channels.channels.zone_channel_id(&s.name) == row.id)
        })
        .map(|i| i as u32 + 1)?;
    let slot = channels.slot(n)?;
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

    const GUILD_RECRUITMENT: u32 = 25;
    const CAPITAL: u32 = 1519;
    const FOREST: u32 = 12;

    fn area(flags: u32, name: &str) -> benilla_formats::AreaTableRow {
        benilla_formats::AreaTableRow {
            map_id: 0,
            zone_id: 0,
            explore_flag: 0,
            flags,
            faction_group_mask: 0,
            exploration_level: 0,
            name: name.to_string(),
            ambient_multiplier: 0.0,
        }
    }

    fn areas() -> AreaTableRes {
        AreaTableRes(benilla_formats::AreaTableCatalog::from_rows(vec![
            (CAPITAL, area(0x100, "Stormwind City")),
            (FOREST, area(0, "Elwynn Forest")),
            (1, area(0x200, "City")),
        ]))
    }

    fn recruitment_row() -> benilla_formats::ChatChannelRow {
        benilla_formats::ChatChannelRow {
            id: GUILD_RECRUITMENT,
            // `GUILD_REQ`, the row the cascade finds (`0x4fe296`).
            flags: 0x2_0032,
            pattern: "GuildRecruitment - %s".into(),
            shortcut: "GuildRecruitment".into(),
        }
    }

    fn channels() -> ChannelState {
        ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![recruitment_row()]),
            zone_mask: Some(1 << (GUILD_RECRUITMENT - 1)),
            ..Default::default()
        }
    }

    /// The forced leave strips the Shortcut the join registered from every window (`0x5094f3`,
    /// `0x50952f`-`0x509560`), whatever the case, then sends under the record's name.
    #[test]
    fn the_cascades_leave_strips_the_shortcut_from_every_window() {
        let mut script = UiScript::new().expect("a VM");
        assert!(script.register_chat_window_channel(0, "GuildRecruitment", GUILD_RECRUITMENT));
        assert!(script.register_chat_window_channel(2, "guildrecruitment", GUILD_RECRUITMENT));
        let mut channels = channels();
        channels.claim_slot("GuildRecruitment - City");
        let (tx, rx) = crossbeam_channel::unbounded();
        let areas = areas();

        assert_eq!(
            cascade_leave(
                &mut channels,
                &NetCommands(tx),
                &mut script,
                &recruitment_row(),
                Some(&areas),
                FOREST,
            ),
            Some(false),
            "a confirmed record stays for its YOU_LEFT, found by ChannelID whatever the zone"
        );
        assert!(
            matches!(rx.try_recv(), Ok(crate::net::ClientCommand::LeaveChannel { name, .. }) if name == "GuildRecruitment - City"),
            "the CMSG_LEAVE_CHANNEL goes out"
        );
        assert_eq!(channels.zone_mask, Some(0), "the mask bit clears");
        assert!(
            script
                .chat_window_looks()
                .iter()
                .all(|l| l.channels.is_empty()),
            "no window keeps the entry the join registered"
        );
    }

    /// The forced leave of a record whose join is unconfirmed sends and frees it (`0x50963f`-
    /// `0x50967a`), which relists; with no record it sends nothing (`0x5095b2`); with an empty zone
    /// subject it returns before the strip (`0x5094e3`).
    #[test]
    fn the_cascades_leave_frees_a_pending_record_and_skips_a_missing_one() {
        let mut channels = channels();
        let mut script = UiScript::new().expect("a VM");
        assert!(script.register_chat_window_channel(0, "GuildRecruitment", GUILD_RECRUITMENT));
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let areas = areas();
        let leave = |channels: &mut ChannelState, script: &mut UiScript, zone: u32| {
            cascade_leave(
                channels,
                &commands,
                script,
                &recruitment_row(),
                Some(&areas),
                zone,
            )
        };
        assert_eq!(leave(&mut channels, &mut script, 0), None);
        assert_eq!(
            script.chat_window_looks()[0].channels.len(),
            1,
            "no zone text: nothing, not even the strip"
        );
        assert_eq!(leave(&mut channels, &mut script, CAPITAL), None);
        assert!(rx.try_recv().is_err(), "no record, no leave");
        assert!(
            script.chat_window_looks()[0].channels.is_empty(),
            "the strip runs ahead of the record lookup, with no record too"
        );
        channels.claim_for_join("GuildRecruitment - City", 0);
        assert_eq!(
            leave(&mut channels, &mut script, CAPITAL),
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

    /// An app running the cascade for an unguilded active player in `zone`, the chat ready.
    fn cascade_app(zone: u32) -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
        let mut app = App::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .insert_resource(areas())
            .insert_resource(crate::area::ZoneIds { zone, area: zone })
            .insert_resource(channels())
            .insert_resource(ZoneChannelWalk::ready_in(zone))
            .init_resource::<GuildRecruitmentCascade>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .add_systems(Update, guild_recruitment_cascade);
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
        ));
        let mut script = UiScript::new().expect("VM");
        script.set_guild_recruitment_mode(1);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_WINDOWS')
                 f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = tostring((GetChatWindowChannels(1))) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn joins(rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|c| match c {
                crate::net::ClientCommand::JoinChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }

    fn seen(app: &App) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap()
    }

    /// The join arm writes the record and window 1's list, then signals `UPDATE_CHAT_WINDOWS`
    /// (`0x81b530` runs its listeners in the caller): the listener reads the window's new entry. In
    /// a capital the Shortcut composes with the city record's name.
    #[test]
    fn the_joins_listener_reads_the_windows_new_entry() {
        let (mut app, rx) = cascade_app(CAPITAL);
        app.update();
        assert_eq!(
            seen(&app),
            "GuildRecruitment",
            "the listener reads the joined entry"
        );
        assert_eq!(joins(&rx), ["GuildRecruitment - City"]);
    }

    /// 12340's cascade has no capital gate (`0x507d30`): outside a capital the unguilded player's
    /// join composes with the zone text, as the verbs compose (`0x507b94`).
    #[test]
    fn outside_a_capital_the_join_takes_the_zone_text() {
        let (mut app, rx) = cascade_app(FOREST);
        app.update();
        assert_eq!(joins(&rx), ["GuildRecruitment - Elwynn Forest"]);
        assert_eq!(seen(&app), "GuildRecruitment");
    }

    /// Any player object's init runs the cascade (`0x6e82c5`): with the record held it does
    /// nothing (`0x507dc6`); once the record is gone, a remote player's arrival joins again.
    #[test]
    fn a_players_init_reruns_the_cascade() {
        let (mut app, rx) = cascade_app(CAPITAL);
        app.update();
        assert_eq!(joins(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<GuildRecruitmentCascade>()
            .request();
        app.update();
        assert!(joins(&rx).is_empty(), "the record is held: nothing");
        assert_eq!(seen(&app), "GuildRecruitment", "and no event either");
        app.world_mut()
            .resource_mut::<ChannelState>()
            .free_slot("GuildRecruitment - City");
        app.world_mut()
            .resource_mut::<GuildRecruitmentCascade>()
            .request();
        app.update();
        assert_eq!(joins(&rx), ["GuildRecruitment - City"]);
    }

    /// Before the chat is ready (`0x507d43`) or under STANDARD (`0x507d36`) a run does nothing and
    /// is not kept for later: each caller runs the cascade once.
    #[test]
    fn a_run_before_the_ready_flag_is_spent() {
        let (mut app, rx) = cascade_app(CAPITAL);
        app.insert_resource(ZoneChannelWalk::default());
        app.update();
        assert!(joins(&rx).is_empty());
        app.insert_resource(ZoneChannelWalk::ready_in(CAPITAL));
        app.update();
        assert!(joins(&rx).is_empty(), "the earlier run is not retried");
    }
}
