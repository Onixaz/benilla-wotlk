//! The bridge's session handlers: the connection edges, our own teleport and worldport, the
//! server clocks, reputations and the player's login-scoped stores. Registered ahead of every
//! window plugin, so the teardown runs before the windows' own `Disconnected` listeners.

use benilla_protocol::messages::{Character, GameClock};
use benilla_protocol::JumpInfo;
use bevy::prelude::*;

use crate::items::Items;
use crate::names::NameCache;
use crate::ui_quest::QuestGiver;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;

use super::{
    CharActionResultMessage, CharListMessage, CharacterLoginFailedMessage,
    CinematicTriggeredMessage, ClientControlMessage, DisconnectedMessage, DroppedOpcodes,
    EnteredWorldMessage, GameTime, GuidIndex, HomeBind, KnockBackMessage, LoggedOutMessage,
    LoginFailedMessage, LoginQueuedMessage, LoginStageMessage, NetCommands, NetHandlerApp,
    NetStatus, ObjectStore, PendingTransfer, Proficiencies, RealmListMessage, Reputations,
    RosterFailedMessage, SelfGuid, ServerTime, ServerWallClock, TeleportMessage, WorldAuthMessage,
    WorldportMessage,
};

/// Registers the session handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::LoginStage, on_login_edge)
        .net_handler(K::WorldAuthenticated, on_login_edge)
        .net_handler(K::RosterFailed, on_login_edge)
        .net_handler(K::AddonReply, on_login_edge)
        .net_handler(K::LoginQueued, on_login_edge)
        .net_handler(K::LoginFailed, on_login_edge)
        .net_handler(K::RealmList, on_login_edge)
        .net_handler(K::CharacterList, on_character_list)
        .net_handler(K::CharActionResult, on_login_edge)
        .net_handler(K::CharacterLoginFailed, on_login_edge)
        .net_handler(K::CinematicTriggered, on_login_edge)
        .net_handler(K::Connected, on_connected)
        .net_handler(K::LoggedOut, on_logged_out)
        .net_handler(K::Disconnected, on_disconnected)
        .net_handler(K::Teleport, on_mover_edge)
        .net_handler(K::KnockBack, on_mover_edge)
        .net_handler(K::ClientControl, on_mover_edge)
        .net_handler(K::Worldport, on_worldport)
        .net_handler(K::TransferPending, on_transfer)
        .net_handler(K::TransferAborted, on_transfer)
        .net_handler(K::TimeSpeed, on_clock)
        .net_handler(K::GameTimeSet, on_clock)
        .net_handler(K::GameTimeSync, on_clock)
        .net_handler(K::GameSpeed, on_clock)
        .net_handler(K::ServerUnixTime, on_clock)
        .net_handler(K::Reputations, on_reputations)
        .net_handler(K::ReputationDelta, on_reputation_delta)
        .net_handler(K::ReputationAtWar, on_reputation_at_war)
        .net_handler(K::ReputationVisible, on_reputations)
        .net_handler(K::ForcedReactions, on_reputations)
        .add_systems(
            OnExit(crate::char_select::ClientState::InWorld),
            clear_forced_reactions,
        )
        .net_handler(K::BindPoint, on_player_store)
        .net_handler(K::Proficiency, on_player_store)
        .net_handler(K::PacketDropped, on_packet_dropped)
        .net_handler(K::Pong, on_pong);
}

/// The edges the bridge publishes as messages, and the two resources they park in.
#[derive(SystemParam)]
pub(crate) struct Edges<'w> {
    teleports: MessageWriter<'w, TeleportMessage>,
    worldports: MessageWriter<'w, WorldportMessage>,
    char_lists: MessageWriter<'w, CharListMessage>,
    realm_lists: MessageWriter<'w, RealmListMessage>,
    char_actions: MessageWriter<'w, CharActionResultMessage>,
    char_login_failures: MessageWriter<'w, CharacterLoginFailedMessage>,
    entered_world: MessageWriter<'w, EnteredWorldMessage>,
    addon_reply: ResMut<'w, super::AddonInfoReply>,
    account_expansion: ResMut<'w, super::AccountExpansion>,
    logged_out: MessageWriter<'w, LoggedOutMessage>,
    knockbacks: MessageWriter<'w, KnockBackMessage>,
    login_stages: MessageWriter<'w, LoginStageMessage>,
    login_queued: MessageWriter<'w, LoginQueuedMessage>,
    world_auth: MessageWriter<'w, WorldAuthMessage>,
    roster_failed: MessageWriter<'w, RosterFailedMessage>,
    login_failures: MessageWriter<'w, LoginFailedMessage>,
    disconnects: MessageWriter<'w, DisconnectedMessage>,
    client_control: MessageWriter<'w, ClientControlMessage>,
    cinematics: MessageWriter<'w, CinematicTriggeredMessage>,
    transfer: ResMut<'w, PendingTransfer>,
}

/// The bridge state the session edges write.
#[derive(SystemParam)]
pub(crate) struct Bridge<'w, 's> {
    commands: Commands<'w, 's>,
    index: ResMut<'w, GuidIndex>,
    self_guid: ResMut<'w, SelfGuid>,
    status: ResMut<'w, NetStatus>,
    names: ResMut<'w, NameCache>,
    items: ResMut<'w, Items>,
    cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    reputations: ResMut<'w, Reputations>,
    server_time: ResMut<'w, ServerTime>,
    wall_clock: ResMut<'w, ServerWallClock>,
    home_bind: ResMut<'w, HomeBind>,
    proficiencies: ResMut<'w, Proficiencies>,
    dropped: ResMut<'w, DroppedOpcodes>,
    net: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    transports: Query<'w, 's, &'static crate::transport::Transport>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
}

fn on_login_edge(In(ev): In<SessionEvent>, mut e: Edges) {
    match ev {
        SessionEvent::LoginStage { stage } => login_stage(stage, &mut e.login_stages),
        SessionEvent::WorldAuthenticated {
            realm,
            epoch,
            generation,
            expansion,
        } => {
            // The character screens read the account's expansion before any world entry.
            e.account_expansion.0 = expansion;
            e.world_auth.write(WorldAuthMessage {
                realm,
                epoch,
                generation,
            });
        }
        SessionEvent::AddonReply { hidden } => e.addon_reply.0 = Some(hidden),
        SessionEvent::RosterFailed { epoch } => {
            e.roster_failed.write(RosterFailedMessage { epoch });
        }
        SessionEvent::LoginQueued { position, realm } => {
            e.login_queued.write(LoginQueuedMessage { position, realm });
        }
        SessionEvent::LoginFailed {
            refusal,
            reason,
            terminal,
            dial: _,
        } => login_failed(refusal, reason, terminal, &mut e.login_failures),
        SessionEvent::RealmList { realms, failed } => {
            e.realm_lists.write(RealmListMessage { realms, failed });
        }
        SessionEvent::CharActionResult { action, code } => {
            char_action_result(action, code, &mut e.char_actions)
        }
        SessionEvent::CharacterLoginFailed { result } => {
            character_login_failed(result, &mut e.char_login_failures)
        }
        SessionEvent::CinematicTriggered { cinematic_id } => {
            cinematic_triggered(cinematic_id, &mut e.cinematics)
        }
        _ => {}
    }
}

fn on_character_list(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::CharacterList {
        characters,
        realm,
        cause,
        epoch,
        generation: _,
    } = ev
    {
        character_list(
            characters,
            realm,
            cause,
            epoch,
            &mut b.status,
            &mut e.char_lists,
        );
    }
}

fn on_connected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Connected {
        self_guid: guid,
        name,
        billing_time_rested,
        expansion,
        tutorial_flags,
        addon_info,
    } = ev
    {
        e.account_expansion.0 = expansion;
        connected(
            guid,
            name,
            billing_time_rested,
            tutorial_flags,
            addon_info,
            &mut e.addon_reply,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut e.entered_world,
        );
    }
}

fn on_logged_out(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::LoggedOut = ev {
        logged_out(
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut e.logged_out,
        );
    }
}

/// The bridge's half of the session end; each window's own listener runs after it.
fn on_disconnected(In(ev): In<SessionEvent>, mut e: Edges, mut b: Bridge) {
    if let SessionEvent::Disconnected { reason, end } = ev {
        disconnected(
            reason,
            end,
            &mut b.commands,
            &mut b.index,
            &mut b.self_guid,
            &mut b.status,
            &mut b.names,
            &mut b.items,
            &mut b.cooldowns,
            &mut e.transfer,
            &mut e.disconnects,
        );
    }
}

/// Teleport, knockback and client control, forwarded unjudged to the controller, which applies
/// and answers them.
fn on_mover_edge(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    b: Bridge,
    movers: Query<&super::Guid, With<super::ActiveMover>>,
) {
    match ev {
        SessionEvent::Teleport {
            guid,
            counter,
            position,
            orientation,
        } => teleport(
            guid,
            counter,
            position,
            orientation,
            &b.self_guid,
            &mut e.teleports,
        ),
        SessionEvent::KnockBack {
            guid,
            counter,
            launch,
        } => knock_back(
            guid,
            counter,
            launch,
            &b.self_guid,
            movers.single().ok().map(|g| g.0),
            &mut e.knockbacks,
        ),
        SessionEvent::ClientControl { mover, allow_move } => {
            e.client_control
                .write(ClientControlMessage { mover, allow_move });
        }
        _ => {}
    }
}

fn on_worldport(
    In(ev): In<SessionEvent>,
    mut e: Edges,
    mut b: Bridge,
    mut group: ResMut<crate::ui_party::GroupState>,
) {
    if let SessionEvent::Worldport {
        map_id,
        position,
        orientation,
        needs_ack,
    } = ev
    {
        // Every streamed roster member is about to be purged; the reference deactivates each.
        crate::ui_party::net::roster_deactivated(&mut group, &b.index, &b.stores, &b.auras, &b.net);
        worldport(
            map_id,
            position,
            orientation,
            needs_ack,
            &mut b.commands,
            &mut b.index,
            &mut e.transfer,
            &b.transports,
            &mut e.worldports,
        );
    }
}

fn on_transfer(In(ev): In<SessionEvent>, mut e: Edges) {
    match ev {
        SessionEvent::TransferPending {
            map_id,
            transport_entry,
        } => transfer_pending(map_id, transport_entry, &mut e.transfer),
        SessionEvent::TransferAborted { map, reason, .. } => {
            transfer_aborted(map, reason, &mut e.transfer)
        }
        _ => {}
    }
}

fn on_clock(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
            packed,
            zone,
        } => time_speed(
            hours,
            minutes,
            day_serial,
            timescale,
            (packed, zone),
            &mut b.server_time,
        ),
        SessionEvent::GameTimeSet(clock) => game_time(clock, false, &mut b.server_time),
        SessionEvent::GameTimeSync(clock) => game_time(clock, true, &mut b.server_time),
        SessionEvent::GameSpeed { timescale } => game_speed(timescale, &mut b.server_time),
        SessionEvent::ServerUnixTime {
            unix_time,
            reset_in,
        } => server_unix_time(unix_time, reset_in, &mut b.wall_clock),
        _ => {}
    }
}

fn on_reputations(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::Reputations { standings } => reputations(standings, &mut b.reputations),
        SessionEvent::ReputationVisible { list_id } => {
            reputation_visible(list_id, &mut b.reputations)
        }
        // `0x5d15d0` replaces the list, then re-reads every reaction (`0x71f8f0`), which the
        // reaction readers do off this resource's change.
        SessionEvent::ForcedReactions { reactions } => {
            info!("net: {} forced reactions", reactions.len());
            b.reputations.1 = reactions;
        }
        _ => {}
    }
}

/// Leaving the world empties the forced reactions, as the reference's reputation store does
/// (`0x5d24c0`).
fn clear_forced_reactions(mut reputations: ResMut<Reputations>) {
    reputations.1.clear();
}

/// The chat line reads the deltas against the store, so it must run before the overwrite.
fn on_reputation_delta(
    In(ev): In<SessionEvent>,
    mut b: Bridge,
    factions: Option<Res<crate::target::ring::Factions>>,
    mut chat_log: ResMut<crate::ui_chat::ChatLog>,
    mut center: MessageWriter<crate::ui_unit::CombatTextEvent>,
    mut quest: ResMut<QuestGiver>,
) {
    if let SessionEvent::ReputationDelta {
        bonus,
        show_effect: _,
        standings,
    } = ev
    {
        // Our race and class pick each faction's base, our gender the rank labels.
        let me = b
            .self_guid
            .0
            .and_then(|g| b.index.0.get(&g).copied())
            .and_then(|e| b.stores.get(e).ok())
            .map(|s| {
                (
                    s.0.unit_race().unwrap_or(0),
                    s.0.unit_class().unwrap_or(0),
                    s.0.unit_gender() == Some(1),
                )
            })
            .unwrap_or((0, 0, false));
        let mut lines = FactionLines {
            catalog: factions.as_deref().map(|f| f.catalog()),
            me,
            bonus,
            log: &mut chat_log,
            texts: Vec::new(),
        };
        reputation_delta(standings, &mut b.reputations, &mut quest, &mut lines);
        center.write_batch(lines.texts);
    }
}

/// `SMSG_SET_FACTION_ATWAR` (`0x5d0850`): the slot's at-war bit set or cleared (`0x5d0896`); the
/// reputation pane's `UPDATE_FACTION` and the unit reactions read the store.
fn on_reputation_at_war(In(ev): In<SessionEvent>, mut b: Bridge) {
    if let SessionEvent::ReputationAtWar { list_id, at_war } = ev {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_ATWAR") else {
            return;
        };
        let reps = &mut b.reputations;
        if reps.0.len() <= i {
            reps.0.resize(i + 1, (0, 0));
        }
        let flags = &mut reps.0[i].0;
        if at_war {
            *flags |= benilla_formats::faction_flags::AT_WAR;
        } else {
            *flags &= !benilla_formats::faction_flags::AT_WAR;
        }
    }
}

/// The player's login-scoped stores the bridge defines.
fn on_player_store(In(ev): In<SessionEvent>, mut b: Bridge) {
    match ev {
        SessionEvent::BindPoint { area } => b.home_bind.0 = Some(area),
        SessionEvent::Proficiency {
            item_class,
            subclass_mask,
        } => {
            b.proficiencies.0.insert(item_class, subclass_mask);
        }
        _ => {}
    }
}

fn on_packet_dropped(In(ev): In<SessionEvent>, mut b: Bridge) {
    if let SessionEvent::PacketDropped {
        opcode,
        unparseable,
    } = ev
    {
        packet_dropped(opcode, unparseable, &mut b.dropped);
    }
}

/// The read thread consumes the pong inline, as the reference's receive path hands `SMSG_PONG` to
/// its pong handler; reaching here means that bypass is gone and every latency reading is a frame
/// late, so it warns.
fn on_pong(In(ev): In<SessionEvent>) {
    if let SessionEvent::Pong { sequence } = ev {
        warn!("net: pong seq={sequence} reached the drain — the read thread's RTT bypass is gone");
    }
}

/// The pre-logon handshake reached a new stage.
fn login_stage(stage: benilla_protocol::LoginStage, out: &mut MessageWriter<LoginStageMessage>) {
    out.write(LoginStageMessage { stage });
}

/// A login failed before the roster; the IO thread is back at its pre-logon park and
/// [`crate::login`] decides what follows.
fn login_failed(
    refusal: Option<benilla_protocol::LoginRefusal>,
    reason: String,
    terminal: bool,
    out: &mut MessageWriter<LoginFailedMessage>,
) {
    out.write(LoginFailedMessage {
        refusal,
        reason,
        terminal,
    });
}

/// `SMSG_CHAR_CREATE`/`SMSG_CHAR_DELETE`: the glue screen turns the code into its string.
fn char_action_result(
    action: benilla_protocol::CharAction,
    code: u8,
    out: &mut MessageWriter<CharActionResultMessage>,
) {
    out.write(CharActionResultMessage { action, code });
}

/// `SMSG_CHAR_ENUM`: the roster and the connected realm, for character select.
fn character_list(
    characters: Vec<Character>,
    realm: Option<benilla_protocol::RealmInfo>,
    cause: benilla_protocol::RosterCause,
    epoch: u64,
    status: &mut NetStatus,
    char_lists: &mut MessageWriter<CharListMessage>,
) {
    info!(
        "net: character select — {} character(s) on the account",
        characters.len()
    );
    // A roster means a live link: clear the reason a lost link left, or the select screen keeps
    // its "Server down" note until the next world entry.
    status.last_reason = None;
    char_lists.write(CharListMessage {
        characters,
        realm,
        cause,
        epoch,
    });
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: the announced entry is void; `crate::char_select` says why.
fn character_login_failed(result: u8, out: &mut MessageWriter<CharacterLoginFailedMessage>) {
    warn!("net: character login refused (result {result:#04x})");
    out.write(CharacterLoginFailedMessage { result });
}

/// `SMSG_TRIGGER_CINEMATIC`, handed to [`crate::cinematic`], which sends the ack at the end of
/// playback. While a cinematic runs unacked, vmangos anchors visibility to the camera
/// (`Player::UpdateCinematic`), so every path there must still ack.
fn cinematic_triggered(
    cinematic_id: u32,
    triggered: &mut MessageWriter<CinematicTriggeredMessage>,
) {
    info!("net: cinematic {cinematic_id} triggered");
    triggered.write(CinematicTriggeredMessage { cinematic_id });
}

/// The first in-world event: records our guid and seeds the name cache with our own name.
fn connected(
    guid: u64,
    name: String,
    billing_time_rested: u32,
    tutorial_flags: Option<Vec<u8>>,
    addon_info: Option<Vec<String>>,
    addon_reply: &mut crate::net::AddonInfoReply,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    entered_world: &mut MessageWriter<EnteredWorldMessage>,
) {
    self_guid.0 = Some(guid);
    status.connected = true;
    status.last_reason = None;
    info!("net: in world as {name} (guid {guid})");
    // The reference clears player and pet names at every world entry, since a guid may name a new
    // character; creature templates survive.
    names.clear_world_session();
    // Our own name came with the login, so "player" never queries.
    names.insert_player(guid, name, None);
    // Set before the world-entry UI load reads it, and every login, so a silent server inherits
    // no earlier answer.
    addon_reply.0 = addon_info;
    entered_world.write(EnteredWorldMessage {
        billing_time_rested,
        tutorial_flags,
    });
}

/// `SMSG_LOGOUT_COMPLETE`: back to character select.
fn logged_out(
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    logged_out: &mut MessageWriter<LoggedOutMessage>,
) {
    // A logout ends the character's session, so the avatar goes too, unlike a reconnectable
    // disconnect; clearing `SelfGuid` first makes the following teardown total.
    info!("net: logged out — back to character select");
    if let Some(guid) = self_guid.0.take() {
        if let Some(e) = index.0.remove(&guid) {
            commands.entity(e).despawn();
        }
    }
    logged_out.write(LoggedOutMessage);
}

/// The world session ended, the link kept or not: tears down the streamed world and clears every
/// session-scoped cache.
fn disconnected(
    reason: String,
    end: benilla_protocol::SessionEnd,
    commands: &mut Commands,
    index: &mut GuidIndex,
    self_guid: &mut SelfGuid,
    status: &mut NetStatus,
    names: &mut NameCache,
    items: &mut Items,
    cooldowns: &mut crate::spell::Cooldowns,
    pending_transfer: &mut PendingTransfer,
    disconnects: &mut MessageWriter<DisconnectedMessage>,
) {
    // Whether the session is over is settled once, here, and carried to every reader.
    let msg = DisconnectedMessage::new(reason.clone(), end);
    let over = msg.session_over;
    disconnects.write(msg);
    warn!("net: {reason} — tearing down the streamed world");
    // An unfinished far teleport dies with the socket.
    pending_transfer.0 = None;
    status.connected = false;
    // A logout or a refused login ends the character, the link kept: nothing reconnects it.
    let character_over = over || end == benilla_protocol::SessionEnd::LoggedOut;
    // The RTT ring is not cleared here: the read thread wipes it as it re-enters its cycle loop.
    // Every streamed entity despawns at once, not faded, so none races the reconnect's creates.
    // The self avatar stays as the local puppet for the reconnect to refresh, unless the
    // character is over, and then it goes as on logout.
    let keep = if character_over { None } else { self_guid.0 };
    if character_over {
        self_guid.0 = None;
    }
    index.0.retain(|guid, e| {
        if Some(*guid) == keep {
            return true;
        }
        commands.entity(*e).despawn();
        false
    });
    // A failure to show at character select; a kept link has none.
    if end != benilla_protocol::SessionEnd::LoggedOut {
        status.last_reason = Some(reason);
    }
    // In-flight name queries died with the socket.
    names.clear_pending();
    items.clear_session();
    // `SMSG_INITIAL_SPELLS` resends every running cooldown at world entry and `seed_initial`
    // appends, so the list must not outlive the session.
    cooldowns.clear_session();
}

/// `MSG_MOVE_TELEPORT_ACK`: only our own is forwarded to the controller, whose ack resumes our
/// movement.
fn teleport(
    guid: u64,
    counter: u32,
    position: [f32; 3],
    orientation: f32,
    self_guid: &SelfGuid,
    teleports: &mut MessageWriter<TeleportMessage>,
) {
    if self_guid.0 == Some(guid) {
        teleports.write(TeleportMessage {
            guid,
            counter,
            position,
            orientation,
        });
    }
}

/// `SMSG_MOVE_KNOCK_BACK`: a ballistic launch our controller flies and acks, for our body or the
/// unit we drive (`0x72d1b0`); AzerothCore names a possessed unit when it is our mover. Observers
/// get `MSG_MOVE_KNOCK_BACK`, so one naming any other unit is dropped.
fn knock_back(
    guid: u64,
    counter: u32,
    launch: JumpInfo,
    self_guid: &SelfGuid,
    active_mover: Option<u64>,
    knockbacks: &mut MessageWriter<KnockBackMessage>,
) {
    if self_guid.0 == Some(guid) || active_mover == Some(guid) {
        knockbacks.write(KnockBackMessage {
            guid,
            counter,
            launch,
        });
    }
}

/// `SMSG_TRANSFER_PENDING`: latched for the worldport; its transport block makes `NEW_WORLD`'s
/// coordinates boat-local.
fn transfer_pending(map_id: u32, transport_entry: Option<u32>, pending: &mut PendingTransfer) {
    match transport_entry {
        Some(entry) => info!("net: transfer pending → map {map_id} riding transport {entry}"),
        None => info!("net: transfer pending → map {map_id}"),
    }
    pending.0 = Some(crate::net::PendingTransferInfo {
        map_id,
        transport_entry,
    });
}

/// `SMSG_TRANSFER_ABORTED`: clears the latch; [`crate::ui_transfer`] shows the refusal.
fn transfer_aborted(map: u32, reason: u8, pending: &mut PendingTransfer) {
    warn!("net: transfer to map {map} aborted (reason {reason})");
    pending.0 = None;
}

/// `SMSG_NEW_WORLD`/`SMSG_LOGIN_VERIFY_WORLD`: drops every tracked object, then hands the app
/// the destination. A transport whose timetable touches the new map is spared, so a ridden boat
/// sails through the map change and the server's re-create refreshes it in place.
fn worldport(
    map_id: u32,
    position: [f32; 3],
    orientation: f32,
    needs_ack: bool,
    commands: &mut Commands,
    index: &mut GuidIndex,
    pending: &mut PendingTransfer,
    transports: &Query<&crate::transport::Transport>,
    worldports: &mut MessageWriter<WorldportMessage>,
) {
    index.0.retain(|guid, e| {
        if transports.get(*e).is_ok_and(|t| t.touches_map(map_id)) {
            // info, not debug: rare, and the crossing depends on it firing for the ridden boat.
            info!("worldport: sparing transport {guid:#x} (its path touches map {map_id})");
            return true;
        }
        commands.entity(*e).try_despawn();
        false
    });
    let announced = pending.0.take();
    if let Some(p) = announced {
        // vmangos pairs every NEW_WORLD with a same-map TRANSFER_PENDING; a mismatch only warns.
        if p.map_id != map_id {
            warn!(
                "worldport: NEW_WORLD map {map_id} ≠ announced transfer map {} — using {map_id}",
                p.map_id
            );
        }
    }
    let transport_entry = announced.and_then(|p| p.transport_entry);
    worldports.write(WorldportMessage {
        map_id,
        position,
        orientation,
        needs_ack,
        transport_entry,
    });
}

/// `SMSG_LOGIN_SETTIMESPEED`: the game clock that drives day/night lighting and the calendar.
fn time_speed(
    hours: u8,
    minutes: u8,
    day_serial: u32,
    timescale: f32,
    (packed, zone): (u32, i32),
    server_time: &mut ServerTime,
) {
    if server_time.0.is_none() {
        info!("net: server game-time {hours:02}:{minutes:02} (drives lighting)");
    }
    let clock = GameTime::new(hours, minutes, day_serial, timescale).dated(packed, zone);
    server_time.1 = clock.timescale();
    server_time.0 = Some(clock);
}

/// `SMSG_GAMETIME_SET` sets the clock to the packet's time (`0x7e2bb0`), `SMSG_GAMETIME_UPDATE`
/// syncs it, never back (`0x7e2a37`); both run at the speed the clock holds.
fn game_time(clock: GameClock, sync: bool, server_time: &mut ServerTime) {
    debug!(
        "net: server game-time {} to {:02}:{:02}",
        if sync { "synced" } else { "set" },
        clock.hours,
        clock.minutes
    );
    let next = GameTime::new(clock.hours, clock.minutes, clock.day_serial, server_time.1)
        .dated(clock.packed, clock.zone);
    server_time.0 = Some(match &server_time.0 {
        Some(now) if sync => now.synced(next),
        _ => next,
    });
}

/// `SMSG_GAMESPEED_SET`: the clock runs on from its current time at the new speed, which a clock
/// set later keeps (`0x7e2893`).
fn game_speed(timescale: f32, server_time: &mut ServerTime) {
    let timescale = timescale.clamp(GameTime::MIN_SPEED, GameTime::MAX_SPEED);
    server_time.1 = timescale;
    if let Some(clock) = server_time.0.as_mut() {
        clock.set_timescale(timescale);
    }
}

/// `SMSG_QUERY_TIME_RESPONSE`: the server wall clock that absolute descriptor stamps (the
/// timed-quest timer) are dated in; its skew against ours is logged once per session.
fn server_unix_time(unix_time: u32, reset_in: u32, clock: &mut ServerWallClock) {
    if clock.0.is_none() {
        let local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        info!(
            "net: server wall clock {unix_time} (local clock differs by {}s)",
            local - i64::from(unix_time)
        );
    }
    clock.sample(unix_time, reset_in);
}

/// `SMSG_INITIALIZE_FACTIONS`: the login reputation store.
fn reputations(standings: Vec<(u8, i32)>, reputations: &mut Reputations) {
    info!("net: reputation store ({} slots)", standings.len());
    reputations.0 = standings;
}

/// What a standing change prints through: the faction table, our race, class and gender, the
/// packet's bonus, the chat log, and the combat texts owed.
struct FactionLines<'a> {
    catalog: Option<&'a benilla_formats::FactionCatalog>,
    me: (u8, u8, bool),
    bonus: f32,
    log: &'a mut crate::ui_chat::ChatLog,
    texts: Vec<crate::ui_unit::CombatTextEvent>,
}

/// The stored flag `0x40` whose rows always print their change (`0x5d211d`).
const FACTION_FLAG_ALWAYS_LOGGED: u8 = 0x40;
/// The stored flag `0x80` that lets an invisible faction print its rank change (`0x5d2236`).
const FACTION_FLAG_RANK_SHOWN: u8 = 0x80;

/// `SMSG_SET_FACTION_STANDING` (`0x5d20a0`), row by row:
///
/// - The line (`0x5d211d`-`0x5d2142`): a faction with flag `0x40` always, in the generic form
///   with `HIDDEN`; any other only as the packet's first row and not `HIDDEN`.
/// - A new standing is stored and its delta printed (`0x5d217b`, `0x5d21b2`).
/// - A faction neither `VISIBLE` nor `HIDDEN` is revealed (`0x5d21d9`).
/// - A rank below Unfriendly sets `AT_WAR` (`0x5d21f7`); a rise in rank clears it (`0x5d2200`).
/// - A rank change prints `FACTION_STANDING_CHANGED` (`0x5d2243`) unless the faction is `HIDDEN`,
///   a header without `0x80`, or revealed by this row.
fn reputation_delta(
    standings: Vec<(u32, i32)>,
    reputations: &mut Reputations,
    quest: &mut QuestGiver,
    lines: &mut FactionLines,
) {
    use benilla_formats::faction_flags as flag;
    let (race, class, female) = lines.me;
    for (row, (list_id, standing)) in standings.into_iter().enumerate() {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_STANDING") else {
            continue;
        };
        if reputations.0.len() <= i {
            reputations.0.resize(i + 1, (0, 0));
        }
        let (mut flags, old) = reputations.0[i];
        let mode = if flags & FACTION_FLAG_ALWAYS_LOGGED != 0 {
            Some(flags & flag::HIDDEN != 0)
        } else if row == 0 && flags & flag::HIDDEN == 0 {
            Some(false)
        } else {
            None
        };
        // The wire's list id is `Faction.dbc`'s `rep_index`.
        let faction = lines.catalog.and_then(|c| {
            c.reputation_factions()
                .find(|(_, f)| i32::try_from(list_id).is_ok_and(|l| f.rep_index == l))
                .map(|(id, f)| {
                    (
                        c.faction_name(id).unwrap_or_default(),
                        f.base_for(race, class),
                    )
                })
        });
        let base = faction.map_or(0, |(_, base)| base);
        let old_rank = benilla_formats::reputation_rank(base + old);
        reputations.0[i].1 = standing;
        if let (Some(generic), Some((name, _))) = (mode, faction) {
            lines
                .texts
                .extend(crate::combat_log::chat::faction_change_line(
                    name,
                    standing - old,
                    generic,
                    lines.bonus,
                    lines.log,
                ));
        }
        let new_rank = benilla_formats::reputation_rank(base + standing);
        let revealed = flags & (flag::VISIBLE | flag::HIDDEN) == 0;
        if revealed {
            flags |= flag::VISIBLE;
        }
        if new_rank < 2 {
            flags |= flag::AT_WAR;
        } else if new_rank > old_rank {
            flags &= !flag::AT_WAR;
        }
        reputations.0[i].0 = flags;
        let shown = flags & flag::HIDDEN == 0
            && (flags & flag::HEADER == 0 || flags & FACTION_FLAG_RANK_SHOWN != 0);
        if new_rank != old_rank && shown && !revealed {
            if let Some((name, _)) = faction {
                crate::combat_log::chat::faction_rank_line(name, new_rank, female, lines.log);
            }
        }
    }
    // Standing feeds questgiver status (`SatisfyQuestReputation`); the reference re-asks here too.
    quest.bump_reask();
}

/// `SMSG_SET_FACTION_VISIBLE`: sets `FACTION_FLAG_VISIBLE` on the slot and nothing else; it
/// carries no standing, and the reputation pane lists a faction by this bit.
fn reputation_visible(list_id: u32, reputations: &mut Reputations) {
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return;
    };
    if reputations.0.len() <= i {
        reputations.0.resize(i + 1, (0, 0));
    }
    reputations.0[i].0 |= benilla_formats::faction_flags::VISIBLE;
}

/// A wire `repListId` as a store index, or `None` (logged) past the `FACTION_LIST_LEN` array
/// (128 in 12340); unchecked, `0xFFFF_FFFF` would resize the store to 34 GB.
fn reputation_slot(list_id: u32, opcode: &str) -> Option<usize> {
    let i = usize::try_from(list_id).ok()?;
    if i >= benilla_protocol::messages::FACTION_LIST_LEN {
        warn!(
            "net: {opcode} names reputation slot {list_id}, past the {}-entry faction list — dropped",
            benilla_protocol::messages::FACTION_LIST_LEN
        );
        return None;
    }
    Some(i)
}

/// The dropped-packet tally: counts each drop and logs an opcode's first drop at info, one line
/// per opcode per run.
fn packet_dropped(opcode: u16, unparseable: bool, dropped: &mut DroppedOpcodes) {
    let tally = dropped.0.entry(opcode).or_default();
    if tally.unknown + tally.unparseable == 0 {
        info!(
            "net: dropped packet {opcode:#06x} ({}) — {} (first occurrence; tallying)",
            benilla_protocol::messages::opcode_name(opcode).unwrap_or("?"),
            if unparseable {
                "parser errored"
            } else {
                "no parser"
            },
        );
    }
    if unparseable {
        tally.unparseable += 1;
    } else {
        tally.unknown += 1;
    }
}

#[cfg(test)]
mod game_clock_tests {
    use super::*;

    fn clock(hours: u8, minutes: u8) -> GameClock {
        GameClock::new(((hours as u32) << 6) | minutes as u32, 0)
    }

    /// `SMSG_GAMETIME_SET` and `SMSG_GAMETIME_UPDATE` move the time and keep the speed.
    #[test]
    fn a_game_time_keeps_the_running_speed() {
        for sync in [false, true] {
            let mut server_time = ServerTime(Some(GameTime::new(6, 0, 0, 0.5)), 0.5);
            game_time(clock(21, 15), sync, &mut server_time);
            let now = server_time.0.unwrap();
            assert_eq!(now.timescale(), 0.5);
            assert!((21 * 60 + 15..21 * 60 + 17).contains(&now.minute_of_day()));
        }
    }

    /// A set goes back to the packet's time; a sync holds a clock that is ahead.
    #[test]
    fn a_set_runs_back_and_a_sync_holds() {
        let ahead = || {
            ServerTime(
                Some(GameTime::new(14, 5, 0, GameTime::MIN_SPEED)),
                GameTime::MIN_SPEED,
            )
        };
        let mut server_time = ahead();
        game_time(clock(14, 2), false, &mut server_time);
        assert_eq!(server_time.0.unwrap().minute_of_day(), 14 * 60 + 2);
        let mut server_time = ahead();
        game_time(clock(14, 2), true, &mut server_time);
        assert_eq!(server_time.0.unwrap().minute_of_day(), 14 * 60 + 5);
    }

    /// A speed before any time is kept for the time that follows; with none, the clock's 1/60.
    #[test]
    fn a_clock_packet_before_settimespeed() {
        let mut server_time = ServerTime::default();
        game_time(clock(8, 0), false, &mut server_time);
        assert_eq!(server_time.0.unwrap().timescale(), GameTime::MIN_SPEED);
        let mut server_time = ServerTime::default();
        game_speed(2.0, &mut server_time);
        assert!(server_time.0.is_none());
        game_time(clock(8, 0), true, &mut server_time);
        assert_eq!(server_time.0.unwrap().timescale(), 2.0);
    }

    /// `SMSG_GAMESPEED_SET` sets the speed of the running clock, clamped (`0x76cfa0`).
    #[test]
    fn a_game_speed_sets_the_running_clock() {
        let mut server_time = ServerTime(
            Some(GameTime::new(6, 0, 0, GameTime::MIN_SPEED)),
            GameTime::MIN_SPEED,
        );
        game_speed(0.25, &mut server_time);
        assert_eq!(server_time.0.unwrap().timescale(), 0.25);
        game_speed(0.0, &mut server_time);
        assert_eq!(server_time.0.unwrap().timescale(), GameTime::MIN_SPEED);
        assert_eq!(server_time.1, GameTime::MIN_SPEED);
    }
}

#[cfg(test)]
mod knock_back_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x7;
    const EYE: u64 = 0xF130_006F_5F29_221A;

    fn knocked(guid: u64, active_mover: Option<u64>) -> usize {
        let mut world = World::new();
        world.init_resource::<Messages<KnockBackMessage>>();
        world
            .run_system_once(move |mut out: MessageWriter<KnockBackMessage>| {
                knock_back(
                    guid,
                    1,
                    JumpInfo::default(),
                    &SelfGuid(Some(ME)),
                    active_mover,
                    &mut out,
                );
            })
            .unwrap();
        world.resource::<Messages<KnockBackMessage>>().len()
    }

    /// `0x72d1b0` launches the unit named: our body, or the unit we drive.
    #[test]
    fn a_knockback_lands_on_our_body_or_the_unit_we_drive() {
        assert_eq!(knocked(ME, Some(ME)), 1);
        assert_eq!(knocked(EYE, Some(EYE)), 1, "a possessed mover is launched");
        assert_eq!(
            knocked(EYE, Some(ME)),
            0,
            "a unit we only watch is not ours to fly"
        );
    }
}

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One teardown of `end` over a session in the world as guid 7, whose avatar is spawned:
    /// whether the self guid survived and the reason the select screen would show.
    fn teardown(end: benilla_protocol::SessionEnd) -> (Option<u64>, Option<String>, bool) {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(7)))
            .init_resource::<NetStatus>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<PendingTransfer>()
            .add_message::<DisconnectedMessage>();
        let avatar = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(7, avatar);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut index: ResMut<GuidIndex>,
                      mut self_guid: ResMut<SelfGuid>,
                      mut status: ResMut<NetStatus>,
                      mut names: ResMut<NameCache>,
                      mut items: ResMut<Items>,
                      mut cooldowns: ResMut<crate::spell::Cooldowns>,
                      mut transfer: ResMut<PendingTransfer>,
                      mut out: MessageWriter<DisconnectedMessage>| {
                    disconnected(
                        "character login refused".into(),
                        end,
                        &mut commands,
                        &mut index,
                        &mut self_guid,
                        &mut status,
                        &mut names,
                        &mut items,
                        &mut cooldowns,
                        &mut transfer,
                        &mut out,
                    );
                },
            )
            .unwrap();
        (
            app.world().resource::<SelfGuid>().0,
            app.world().resource::<NetStatus>().last_reason.clone(),
            app.world().get_entity(avatar).is_ok(),
        )
    }

    /// A refused login (or a logout) ends the character with the link kept: the self guid goes as
    /// on a logout, and no failure is left for character select to show as "Server down".
    #[test]
    fn a_teardown_on_a_kept_link_ends_the_character_and_reports_no_failure() {
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::LoggedOut);
        assert_eq!(guid, None, "the refused character's guid outlived it");
        assert_eq!(reason, None, "a kept link is no failure");
        assert!(!avatar);
    }

    /// An unattended run's lost link keeps the avatar for the reconnect, and its reason.
    #[test]
    fn a_reconnectable_loss_keeps_the_avatar_and_its_reason() {
        let _lock = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
        let (guid, reason, avatar) = teardown(benilla_protocol::SessionEnd::Lost);
        assert_eq!(guid, Some(7));
        assert!(reason.is_some());
        assert!(avatar);
    }
}

#[cfg(test)]
mod reputation_delta_tests {
    use super::*;

    /// One `SMSG_SET_FACTION_STANDING` through the row rule against the install's `Faction.dbc`,
    /// as a Human Warrior (Stormwind's base 4000, Friendly).
    #[test]
    fn the_rows_pick_their_lines_and_the_rank_moves_the_war_flag() {
        use benilla_formats::faction_flags as flag;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("dbc");
        let slot = |name: &str| {
            catalog
                .reputation_factions()
                .find(|(id, _)| catalog.faction_name(*id) == Some(name))
                .map(|(_, f)| f.rep_index as u32)
                .unwrap_or_else(|| panic!("{name} has a reputation slot"))
        };
        let (stormwind, darnassus) = (slot("Stormwind"), slot("Darnassus"));
        let mut reps = Reputations::default();
        reps.0.resize(128, (0, 0));
        reps.0[stormwind as usize].0 = flag::VISIBLE;
        reps.0[darnassus as usize].0 = flag::VISIBLE | flag::AT_WAR;
        let mut quest = QuestGiver::default();
        let mut log = crate::ui_chat::ChatLog::default();
        let mut lines = FactionLines {
            catalog: Some(&catalog),
            me: (1, 1, false),
            bonus: 0.0,
            log: &mut log,
            texts: Vec::new(),
        };
        // Stormwind first, up to Honored: its change line and text, and the rank line. Darnassus
        // second changes silently but still prints its rank and drops below Unfriendly.
        let darnassus_base = catalog
            .reputation_factions()
            .find(|(_, f)| f.rep_index == darnassus as i32)
            .map(|(_, f)| f.base_for(1, 1))
            .unwrap();
        reputation_delta(
            vec![(stormwind, 5100), (darnassus, -9000 - darnassus_base)],
            &mut reps,
            &mut quest,
            &mut lines,
        );
        assert_eq!(lines.texts.len(), 1, "one FACTION text, the first row's");
        assert_eq!(
            log.pending_len(),
            3,
            "change + rank for Stormwind, rank for Darnassus"
        );
        assert_eq!(reps.0[stormwind as usize], (flag::VISIBLE, 5100));
        assert_ne!(
            reps.0[darnassus as usize].0 & flag::AT_WAR,
            0,
            "Hated sets war"
        );

        // Back up past Hostile: a rise in rank clears the war flag.
        let mut log = crate::ui_chat::ChatLog::default();
        let mut lines = FactionLines {
            catalog: Some(&catalog),
            me: (1, 1, false),
            bonus: 0.0,
            log: &mut log,
            texts: Vec::new(),
        };
        reputation_delta(
            vec![(darnassus, -darnassus_base)],
            &mut reps,
            &mut quest,
            &mut lines,
        );
        assert_eq!(
            reps.0[darnassus as usize].0 & flag::AT_WAR,
            0,
            "a rise clears war"
        );

        // A hidden faction prints nothing, first row or not, and is not revealed.
        reps.0[stormwind as usize].0 = flag::HIDDEN;
        let mut log = crate::ui_chat::ChatLog::default();
        let mut lines = FactionLines {
            catalog: Some(&catalog),
            me: (1, 1, false),
            bonus: 0.0,
            log: &mut log,
            texts: Vec::new(),
        };
        reputation_delta(vec![(stormwind, 40000)], &mut reps, &mut quest, &mut lines);
        assert!(lines.texts.is_empty());
        assert_eq!(log.pending_len(), 0);
        assert_eq!(reps.0[stormwind as usize].0 & flag::VISIBLE, 0);
    }
}
