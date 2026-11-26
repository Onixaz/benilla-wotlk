//! The guild-recruitment cascade (`0x507d30`), the one place the client joins or leaves the
//! `GuildRecruitment` channel on its own. Under AUTO (`[0xac7a3c] == 1`, the chat's
//! guild-recruitment mode) and once the chat is ready (`[0xbcefe8]`), with the active player held
//! and the `GUILD_REQ` row found (`0x4fe260`): a guilded player leaves a held record and an
//! unguilded one with no record joins it, the row's Shortcut composed as the channel verbs compose
//! it; both then signal `UPDATE_CHAT_WINDOWS`. Each caller runs it in its own step: the zone
//! update's tail (`0x508254`), the mode setter's move to 1 (`0x508282`), the chat cache's mode line
//! (`0x5087dd`), the active player's guild id handler (`0x6e1beb`) and every player object's init
//! (`0x6e82c5`).

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::channels::ZoneChannelWalk;
use super::edit::ChannelState;

/// Whether the chat is ready (`[0xbcefe8]`), set by the first zone update's walk (`0x508249`).
fn ready(world: &World) -> bool {
    world
        .get_resource::<ZoneChannelWalk>()
        .is_some_and(ZoneChannelWalk::ready)
}

/// The VM's handle from the `World`, for a step that holds none; `None` with no VM, where there is
/// no chat to act on.
fn world_vm(world: &World) -> Option<UiScript> {
    world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::share)
}

/// The cascade (`0x507d30`) as a step of its caller, on the `World`'s VM.
pub(crate) fn update_guild_status_step(world: &mut World) {
    if let Some(mut vm) = world_vm(world) {
        update_guild_status(world, &mut vm);
    }
}

/// The cascade on `vm`, the calling VM's handle: its gates and its join or leave, then
/// `UPDATE_CHAT_WINDOWS` with the `World` lent, its listeners run in the caller (`0x81b530`).
pub(crate) fn update_guild_status(world: &mut World, vm: &mut UiScript) {
    // AUTO (`0x507d36`) and the ready flag (`0x507d43`), read at the call.
    if vm.guild_recruitment_mode() != 1 || !ready(world) {
        return;
    }
    if let Ok(Some(ran)) = world.run_system_cached_with(cascade_run, vm.share()) {
        signal_chat_windows(world, vm, ran);
    }
}

/// What ends an acting cascade or the setter's leave, with the `World` lent: the join's window
/// entry or the leave's relist (`0x509680`), then `UPDATE_CHAT_WINDOWS`, its listeners run in the
/// caller (`0x507db3`, `0x507df6`, `0x5082ab`).
fn signal_chat_windows(world: &mut World, vm: &mut UiScript, ran: CascadeRan) {
    crate::ui_script::safe_world::lend_to(world, vm, |s| {
        match ran {
            CascadeRan::Joined(Some((shortcut, id))) => {
                s.register_chat_window_channel(0, &shortcut, id);
            }
            CascadeRan::Left { relist: true } => s.channels_updated(),
            _ => {}
        }
        s.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    });
}

/// The mode setter (`0x508260`) from `guildRecruitmentChannel`'s callback (`0x512750`), which
/// passes `fromJoin` 0, on the calling VM: unmoved is a no-op (`0x50826c`), a move is stored
/// (`0x508275`) and before the chat is ready that is all (`0x50826e`); then 1 runs the cascade
/// (`0x508282`) and any other value leaves the row's channel and refreshes the windows
/// (`0x50828d`-`0x5082ab`).
pub(crate) fn set_mode_from_cvar(world: &mut World, vm: &mut UiScript, mode: i32) {
    if !vm.set_guild_recruitment_mode_from_cvar(mode) || !ready(world) {
        return;
    }
    if mode == 1 {
        update_guild_status(world, vm);
        return;
    }
    if let Ok(Some(relist)) = world.run_system_cached_with(cvar_leave, vm.share()) {
        signal_chat_windows(world, vm, CascadeRan::Left { relist });
    }
}

/// A host-side `guildRecruitmentChannel` move (a config fold, the console): the callback on the
/// `World`'s VM; a script's `SetCVar` ran it in its call.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut commands: Commands) {
    if ev.from_vm || !ev.is("guildRecruitmentChannel") {
        return;
    }
    let mode = benilla_ui::script::sstr_to_int(&ev.new);
    commands.queue(move |world: &mut World| {
        if let Some(mut vm) = world_vm(world) {
            set_mode_from_cvar(world, &mut vm, mode);
        }
    });
}

/// The chat's zone update (`0x508090`) as a step of its caller, on the `World`'s VM.
pub(crate) fn zone_update_step(world: &mut World) {
    if let Some(mut vm) = world_vm(world) {
        zone_update(world, &mut vm);
    }
}

/// The active player's flags handler on a landing: the zone update unless a cinematic runs
/// (`0x728ff9`-`0x729002`).
pub(crate) fn zone_update_on_landing(world: &mut World) {
    if !world
        .get_resource::<crate::cinematic::Cinematic>()
        .is_some_and(crate::cinematic::Cinematic::is_playing)
    {
        zone_update_step(world);
    }
}

/// The cinematic's end, past `CINEMATIC_STOP`: the zone update with the active player held and
/// off a taxi (`0x51d8e2`-`0x51d915`).
pub(crate) fn zone_update_after_cinematic(world: &mut World) {
    let flags = world
        .query_filtered::<&ObjectStore, With<SelfPlayer>>()
        .iter(world)
        .next()
        .map(|s| s.0.unit_flags());
    if zone_text_runs_zone_update(flags, false, false) {
        zone_update_step(world);
    }
}

/// Whether the zone text's update on a new, non-empty zone text runs the chat's zone update
/// (`0x5205f0`-`0x520680`): with the active player held (`0x520656`), off a taxi (`0x52066a`), no
/// cinematic running (`0x52066c`) and no trigger waiting on the world (`0x52067e`); `flags` is the
/// active player's flags word.
pub(crate) fn zone_text_runs_zone_update(
    flags: Option<u32>,
    cinematic: bool,
    waiting: bool,
) -> bool {
    flags.is_some_and(|f| f & UNIT_FLAG_TAXI_FLIGHT == 0) && !cinematic && !waiting
}

/// `UNIT_FIELD_FLAGS` bit `0x100000`, taxi flight.
const UNIT_FLAG_TAXI_FLIGHT: u32 = 0x10_0000;

/// The chat's zone update (`0x508090`) on `vm`, the calling VM: nothing before the chat cache is
/// loaded (`[0xbcefe4]`, `0x508099`); with the real zone text it holds (`[0xbd0780]`, `0x5080a5`)
/// the walk leg, [`super::channels::zone_walk`], which may stop it (`0x5080dd`); then the cascade
/// (`0x508254`), at once for an empty text (`0x5080ac`).
pub(crate) fn zone_update(world: &mut World, vm: &mut UiScript) {
    let loaded = world
        .get_resource::<ChannelState>()
        .is_some_and(|c| c.zone_mask.is_some());
    if !loaded {
        return;
    }
    let real_zone = vm.real_zone_text();
    if !real_zone.is_empty()
        && !world
            .run_system_cached_with(super::channels::zone_walk, real_zone)
            .unwrap_or(false)
    {
        return;
    }
    update_guild_status(world, vm);
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
    In(mut vm): In<UiScript>,
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    zone: Option<Res<crate::area::ZoneIds>>,
    commands: Res<NetCommands>,
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

/// One run of the cascade past its AUTO and ready gates, on the calling VM: `None` when it
/// returns before acting, else what it did ahead of `UPDATE_CHAT_WINDOWS`.
fn cascade_run(
    In(mut vm): In<UiScript>,
    mut channels: ResMut<ChannelState>,
    (areas, zone): (Option<Res<AreaTableRes>>, Option<Res<crate::area::ZoneIds>>),
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) -> Option<CascadeRan> {
    // The active player (`0x507d70`), its guild id read off its descriptor at the call
    // (`0x507d8a`), then the `GUILD_REQ` row (`0x507d7d`).
    let guild_id = self_q.iter().next()?.0.player_guild_id();
    let row = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())?
        .clone();
    // The record by the row's ChannelID, in any state (`0x507d85` -> `0x4fe020`).
    let held = channels.joined.iter().flatten().any(|s| s.id == row.id);
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
    match channels.claim_for_join(&name, row.id, 0) {
        super::edit::JoinRecord::Claimed(n) => {
            // Voice 0, permanent 1 (`0x507dcb`, `0x507dc9`).
            channels.set_join_flags(n, false, true);
            let _ =
                commands
                    .0
                    .send(channels.join_command(row.id, name, String::new(), true, false));
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
    vm: &mut UiScript,
    row: &benilla_formats::ChatChannelRow,
    areas: Option<&AreaTableRes>,
    zone: u32,
) -> Option<bool> {
    super::channels::verb_name(row, areas, zone)?;
    vm.strip_chat_window_channel(&row.shortcut);
    let n = channels
        .joined
        .iter()
        .position(|s| s.as_ref().is_some_and(|s| s.id == row.id))
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
        channels.claim_for_join("GuildRecruitment - City", GUILD_RECRUITMENT, 0);
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

    /// A `World` for an unguilded active player in `zone`, the chat ready and AUTO, with a VM whose
    /// `UPDATE_CHAT_WINDOWS` listener records window 1's first channel; `guildRecruitmentChannel`
    /// is registered, as the CVar table seeds it.
    fn cascade_world(
        zone: u32,
    ) -> (
        World,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        let mut world = World::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.insert_resource(areas());
        world.insert_resource(crate::area::ZoneIds { zone, area: zone });
        world.insert_resource(channels());
        world.insert_resource(ZoneChannelWalk::ready_chat());
        world.init_resource::<crate::ui_script::live::UiEvents>();
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
        ));
        let mut script = UiScript::new().expect("VM");
        script.seed_cvars(vec![benilla_ui::script::SeededCvar {
            name: "guildRecruitmentChannel".into(),
            value: "1".into(),
            default: "1".into(),
            device_default: None,
            latched: false,
            per_account: true,
            per_character: false,
        }]);
        script.set_guild_recruitment_mode(1);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_WINDOWS')
                 f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = tostring((GetChatWindowChannels(1))) end)",
            )
            .unwrap();
        world.insert_non_send_resource(script);
        (world, rx)
    }

    fn joins(rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|c| match c {
                crate::net::ClientCommand::JoinChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }

    fn leaves(rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .filter_map(|c| match c {
                crate::net::ClientCommand::LeaveChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }

    fn seen(world: &World) -> String {
        world
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap()
    }

    /// `chunk` run on the `World`'s VM with the `World` lent, as a script runs.
    fn run(world: &mut World, chunk: &str) {
        crate::ui_script::safe_world::lend(world, |s| s.run(chunk).expect("the chunk runs"));
    }

    fn set_guild_id(world: &mut World, id: u32) {
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut store = q.single_mut(world).expect("our player");
        store.0 = benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_GUILDID,
            id,
        )]);
    }

    /// The join arm writes the record and window 1's list, then signals `UPDATE_CHAT_WINDOWS`
    /// (`0x81b530` runs its listeners in the caller): the listener reads the window's new entry. In
    /// a capital the Shortcut composes with the city record's name.
    #[test]
    fn the_joins_listener_reads_the_windows_new_entry() {
        let (mut world, rx) = cascade_world(CAPITAL);
        update_guild_status_step(&mut world);
        assert_eq!(
            seen(&world),
            "GuildRecruitment",
            "the listener reads the joined entry"
        );
        assert_eq!(joins(&rx), ["GuildRecruitment - City"]);
    }

    /// 12340's cascade has no capital gate (`0x507d30`): outside a capital the unguilded player's
    /// join composes with the zone text, as the verbs compose (`0x507b94`).
    #[test]
    fn outside_a_capital_the_join_takes_the_zone_text() {
        let (mut world, rx) = cascade_world(FOREST);
        update_guild_status_step(&mut world);
        assert_eq!(joins(&rx), ["GuildRecruitment - Elwynn Forest"]);
        assert_eq!(seen(&world), "GuildRecruitment");
    }

    /// Each caller runs the cascade at its own step (`0x6e82c5`): with the record held it does
    /// nothing (`0x507dc6`); once the record is gone, the next caller joins again.
    #[test]
    fn each_caller_runs_the_cascade_once() {
        let (mut world, rx) = cascade_world(CAPITAL);
        update_guild_status_step(&mut world);
        assert_eq!(joins(&rx).len(), 1);
        update_guild_status_step(&mut world);
        assert!(joins(&rx).is_empty(), "the record is held: nothing");
        assert_eq!(seen(&world), "GuildRecruitment", "and no event either");
        world
            .resource_mut::<ChannelState>()
            .free_slot("GuildRecruitment - City");
        update_guild_status_step(&mut world);
        assert_eq!(joins(&rx), ["GuildRecruitment - City"]);
    }

    /// The guild id is read off the active player's descriptor at the call (`0x507d8a`): a guilded
    /// player leaves the held record, then the event fires (`0x507da7`, `0x507db3`).
    #[test]
    fn a_guilded_player_leaves_the_held_record_at_the_call() {
        let (mut world, rx) = cascade_world(CAPITAL);
        update_guild_status_step(&mut world);
        assert_eq!(joins(&rx).len(), 1);
        set_guild_id(&mut world, 42);
        update_guild_status_step(&mut world);
        assert_eq!(leaves(&rx), ["GuildRecruitment - City"]);
        assert_eq!(
            seen(&world),
            "GuildRecruitment,nil",
            "the strip ran before the event"
        );
    }

    /// Before the chat is ready (`0x507d43`) or under STANDARD (`0x507d36`) a run does nothing and
    /// is not kept for later: each caller runs the cascade once.
    #[test]
    fn a_run_before_the_ready_flag_is_spent() {
        let (mut world, rx) = cascade_world(CAPITAL);
        world.insert_resource(ZoneChannelWalk::default());
        update_guild_status_step(&mut world);
        assert!(joins(&rx).is_empty());
        world.insert_resource(ZoneChannelWalk::ready_chat());
        world
            .non_send_resource_mut::<UiScript>()
            .set_guild_recruitment_mode(0);
        update_guild_status_step(&mut world);
        assert!(joins(&rx).is_empty(), "STANDARD: nothing");
        assert_eq!(seen(&world), "");
    }

    /// `SetCVar("guildRecruitmentChannel", ...)` runs its callback inside the call (`0x51275f`):
    /// 0 leaves the row's channel and fires `UPDATE_CHAT_WINDOWS` before `SetCVar` returns
    /// (`0x50829f`, `0x5082ab`); 1 runs the cascade there (`0x508282`); an unmoved value is a no-op
    /// (`0x50826c`).
    #[test]
    fn set_cvar_runs_the_mode_setter_inside_the_call() {
        let (mut world, rx) = cascade_world(CAPITAL);
        update_guild_status_step(&mut world);
        assert_eq!(joins(&rx).len(), 1);
        run(
            &mut world,
            "SetCVar('guildRecruitmentChannel', '0') AFTER_LEAVE = table.concat(SEEN, ',')",
        );
        let after: String = world
            .non_send_resource::<UiScript>()
            .eval("return AFTER_LEAVE")
            .unwrap();
        assert_eq!(
            after, "GuildRecruitment,nil",
            "the event fired inside SetCVar"
        );
        assert_eq!(leaves(&rx), ["GuildRecruitment - City"]);
        assert_eq!(
            world
                .non_send_resource::<UiScript>()
                .guild_recruitment_mode(),
            0
        );
        run(&mut world, "SetCVar('guildRecruitmentChannel', '0')");
        assert_eq!(seen(&world), "GuildRecruitment,nil", "unmoved: nothing");
        world
            .resource_mut::<ChannelState>()
            .free_slot("GuildRecruitment - City");
        run(
            &mut world,
            "SetCVar('guildRecruitmentChannel', '1') AFTER_JOIN = #SEEN",
        );
        let after: i64 = world
            .non_send_resource::<UiScript>()
            .eval("return AFTER_JOIN")
            .unwrap();
        assert_eq!(after, 3, "the cascade's event fired inside SetCVar");
        assert_eq!(joins(&rx), ["GuildRecruitment - City"]);
    }

    /// Before the chat is ready the setter only stores the latch (`0x508275`, `0x50826e`).
    #[test]
    fn set_cvar_before_the_ready_flag_only_stores() {
        let (mut world, rx) = cascade_world(CAPITAL);
        world.insert_resource(ZoneChannelWalk::default());
        run(&mut world, "SetCVar('guildRecruitmentChannel', '0')");
        assert_eq!(
            world
                .non_send_resource::<UiScript>()
                .guild_recruitment_mode(),
            0
        );
        assert!(leaves(&rx).is_empty());
        assert_eq!(seen(&world), "");
    }

    /// The zone update ends in the cascade once the chat cache is loaded (`0x50809f`) and the chat
    /// ready (`0x5080ca`); a landing skips it while a cinematic runs (`0x728ff9`), and a cinematic's
    /// end skips it on a taxi (`0x51d913`).
    #[test]
    fn the_zone_updates_callers_run_the_cascade() {
        let (mut world, rx) = cascade_world(CAPITAL);
        world.resource_mut::<ChannelState>().zone_mask = None;
        zone_update_step(&mut world);
        assert!(joins(&rx).is_empty(), "the chat cache is not loaded");
        world.resource_mut::<ChannelState>().zone_mask = Some(0);
        world.insert_resource(crate::cinematic::Cinematic::playing_for_test());
        zone_update_on_landing(&mut world);
        assert!(joins(&rx).is_empty(), "a cinematic runs");
        world.insert_resource(crate::cinematic::Cinematic::default());
        zone_update_on_landing(&mut world);
        assert_eq!(joins(&rx), ["GuildRecruitment - City"], "the landing's run");
        world
            .resource_mut::<ChannelState>()
            .free_slot("GuildRecruitment - City");
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        q.single_mut(&mut world).unwrap().0 = benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_FLAGS,
            UNIT_FLAG_TAXI_FLIGHT,
        )]);
        zone_update_after_cinematic(&mut world);
        assert!(joins(&rx).is_empty(), "still on a taxi");
        q.single_mut(&mut world).unwrap().0 = benilla_protocol::ObjectFields::from_pairs(&[]);
        zone_update_after_cinematic(&mut world);
        assert_eq!(
            joins(&rx),
            ["GuildRecruitment - City"],
            "the cinematic end's run"
        );
    }

    /// `General`, the first mask row the walk joins, and the recruitment row the cascade joins.
    fn general_row() -> benilla_formats::ChatChannelRow {
        benilla_formats::ChatChannelRow {
            id: 1,
            flags: 0x8_0003,
            pattern: "General - %s".into(),
            shortcut: "General".into(),
        }
    }

    /// [`cascade_world`] before the first zone update: the chat not ready, the mask holding
    /// `General` alone, the real zone text `real_zone` pushed and the zone id `zone` stored.
    fn walk_world(
        zone: u32,
        real_zone: &str,
    ) -> (
        World,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
    ) {
        let (mut world, rx) = cascade_world(zone);
        world.insert_resource(ZoneChannelWalk::default());
        world.insert_resource(ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                general_row(),
                recruitment_row(),
            ]),
            zone_mask: Some(1),
            ..Default::default()
        });
        world
            .non_send_resource_mut::<UiScript>()
            .set_zone_texts(benilla_ui::script::ZoneTexts {
                real_zone: real_zone.into(),
                ..Default::default()
            });
        (world, rx)
    }

    /// The zone update (`0x508090`) is the walk: nothing before the cache load (`0x508099`), only
    /// the cascade with no real zone text (`0x5080ac`), a stop with no area record (`0x5080dd`),
    /// else the walk joins the mask's rows, sets the ready flag (`0x508249`) and runs the cascade
    /// (`0x508254`); once ready, later updates run the cascade alone (`0x5080ca`).
    #[test]
    fn the_zone_update_walks_once_then_runs_the_cascade() {
        let (mut world, rx) = walk_world(FOREST, "");
        world.resource_mut::<ChannelState>().zone_mask = None;
        zone_update_step(&mut world);
        world.resource_mut::<ChannelState>().zone_mask = Some(1);
        zone_update_step(&mut world);
        assert!(joins(&rx).is_empty(), "unloaded, then no real zone text");
        assert!(!ready(&world));

        let (mut world, rx) = walk_world(999, "Nowhere");
        zone_update_step(&mut world);
        assert!(joins(&rx).is_empty(), "no area record: no walk, no cascade");
        assert!(!ready(&world));

        let (mut world, rx) = walk_world(FOREST, "Elwynn Forest");
        zone_update_step(&mut world);
        assert_eq!(
            joins(&rx),
            [
                "General - Elwynn Forest",
                "GuildRecruitment - Elwynn Forest"
            ],
            "the walk's join, then the cascade's"
        );
        assert!(ready(&world));
        assert_eq!(seen(&world), "GuildRecruitment");
        zone_update_step(&mut world);
        assert!(
            joins(&rx).is_empty(),
            "walked once; the cascade holds its record"
        );
    }

    /// The zone text's update runs the zone update before its zone event (`0x520680`, `0x5206a6`),
    /// as the area feed queues them: a `ZONE_CHANGED_NEW_AREA` listener at login finds the walk's
    /// record by name in any state (`0x4fe8e4`) and the cascade's `UPDATE_CHAT_WINDOWS` already
    /// fired (`0x507df6`), every join sent.
    #[test]
    fn the_login_walk_runs_before_the_zone_event() {
        let (mut world, rx) = walk_world(FOREST, "Elwynn Forest");
        run(
            &mut world,
            "local f = CreateFrame('Frame') f:RegisterEvent('ZONE_CHANGED_NEW_AREA')
             f:SetScript('OnEvent', function()
               AT_ZONE = GetChannelName('General - Elwynn Forest') .. ':' .. #SEEN
             end)",
        );
        {
            let mut ui = world.resource_mut::<crate::ui_script::live::UiEvents>();
            ui.world(zone_update_step);
            ui.fire("ZONE_CHANGED_NEW_AREA", vec![]);
        }
        crate::ui_script::live::fire_handler_events(&mut world);
        let at_zone: String = world
            .non_send_resource::<UiScript>()
            .eval("return AT_ZONE")
            .unwrap();
        assert_eq!(at_zone, "1:1", "the number, and the cascade's event before");
        assert_eq!(
            joins(&rx),
            [
                "General - Elwynn Forest",
                "GuildRecruitment - Elwynn Forest"
            ]
        );
    }

    /// The zone text's update runs the chat's zone update only with the active player held, off a
    /// taxi, with no cinematic running and none waiting on the world (`0x520656`-`0x52067e`).
    #[test]
    fn the_zone_text_gates_its_zone_update() {
        assert!(zone_text_runs_zone_update(Some(0), false, false));
        assert!(!zone_text_runs_zone_update(None, false, false));
        assert!(!zone_text_runs_zone_update(
            Some(UNIT_FLAG_TAXI_FLIGHT),
            false,
            false
        ));
        assert!(!zone_text_runs_zone_update(Some(0), true, false));
        assert!(!zone_text_runs_zone_update(Some(0), false, true));
    }
}
