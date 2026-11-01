//! The app half of the reference's battlefield queue (`0x549ad0`-`0x54e720`): the two queue slots
//! `SMSG_BATTLEFIELD_STATUS` writes, the instance list a battlemaster or the PvP window opens, the
//! join verdicts and the Wintergrasp queue, fed to the stock `PVPBattlegroundFrame.lua`,
//! `BattlefieldFrame.lua`, `ArenaFrame.lua` and the minimap icon. The scoreboard is
//! [`crate::ui_battlefield_score`]'s, the teammate positions [`crate::ui_battlefield_positions`]'.
//!
//! The VM owns what the bindings write (the listed type, the selection, the battlemaster); a
//! list packet is handed on as it lands, and the slots are pushed every frame because their
//! clocks move.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, MapCatalogRes, WorldAssets};
use benilla_formats::{BattlemasterCatalog, MapCatalog};
use benilla_protocol::messages::{
    BattlefieldList, BattlefieldMgrNotice, BattlefieldStatus, GroupJoinedBattleground,
};
use benilla_ui::script::{
    BattlefieldFrame, BattlefieldJoin, BattlefieldListPush, BattlefieldQueueSlot,
    BattlefieldRewardView, BattlemasterRow, ScriptValue, UiScript, WorldPvpQueueView,
};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer, WorldEnterCascadeMessage};
use crate::player::Player;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::NpcSession;
use crate::world_state::WorldStates;

/// `BattlemasterList.dbc` and `PvpDifficulty.dbc`, loaded once.
#[derive(Resource)]
pub(crate) struct BattlemasterTable(pub(crate) BattlemasterCatalog);

/// The random battleground, whose invitation names its real type in the joined queue word.
const RANDOM_BATTLEGROUND: u32 = 32;

/// `PLAYER_FLAGS` bit 25, experience gain off: the list request's second byte (`0x54e66a`).
const PLAYER_FLAGS_NO_XP_GAIN: u32 = 0x0200_0000;

/// `UNIT_FIELD_FLAGS` bit 11, the combat bit `AcceptBattlefieldPort` refuses on (`0x54dafc`).
const UNIT_FLAG_PORT_COMBAT: u32 = 0x800;

/// `PLAYER_FLAGS` bit 4, a ghost.
const PLAYER_FLAGS_GHOST: u32 = 0x10;

/// A battleground map's `InstanceType`.
const INSTANCE_TYPE_BATTLEGROUND: u32 = 3;

/// `AreaTable.dbc`'s world-PvP flag, `CanHearthAndResurrectFromArea`'s test (`0x54c810`).
const AREA_FLAG_HEARTH_AND_RESURRECT: u32 = 0x0800_0000;

/// The two queue slots (`0xbea4b8`), each stamped with the moment its status landed,
/// and the active instance's state beside them.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldQueue {
    slots: [Option<(BattlefieldStatus, Instant)>; 2],
    /// The active slot (`0xacd16c`), set by status 3.
    active: Option<usize>,
    /// The active map id (`0xbea564`): kept when a later status drops the active index, cleared
    /// with the slot.
    active_map: u32,
    /// The arena faction (`0xbea54a`).
    arena_faction: u8,
    /// The instance's expiration and start time, the second kept as the status's arrival
    /// and the wire's elapsed span so no `Instant` precedes the clock's origin.
    instance_expiration: Option<Instant>,
    run_started: Option<(Instant, Duration)>,
    /// A status 3 owes the scoreboard its filter reset and rebuild (`0x54b0a6`).
    became_active: bool,
    /// `UPDATE_BATTLEFIELD_STATUS`'s argument for each status, the 1-based slot.
    events: Vec<u32>,
    /// Tutorials 47 (queued) and 48 (confirm), owed to the next feed.
    tutorials: Vec<u32>,
}

impl BattlefieldQueue {
    /// `SMSG_BATTLEFIELD_STATUS` (`0x54ae40`).
    pub(crate) fn apply(&mut self, status: BattlefieldStatus) {
        self.apply_at(status, Instant::now());
    }

    fn apply_at(&mut self, status: BattlefieldStatus, now: Instant) {
        let index = status.slot as usize;
        if index >= self.slots.len() {
            return;
        }
        self.events.push(status.slot + 1);
        if status.queue == 0 {
            if self.active == Some(index) {
                self.instance_expiration = None;
                self.run_started = None;
                self.active_map = 0;
                self.active = None;
            }
            self.slots[index] = None;
            return;
        }
        if status.status == 2 {
            self.tutorials
                .push(crate::tutorial::id::PORT_TO_BATTLEGROUND);
        }
        if let Some(a) = status.active {
            self.active_map = a.map_id;
            let ms = |v: u32| Duration::from_millis(u64::from(v));
            self.instance_expiration = (a.shutdown_ms != 0).then(|| now + ms(a.shutdown_ms));
            self.run_started = (a.run_ms != 0).then(|| (now, ms(a.run_ms)));
            self.arena_faction = a.arena_faction;
            self.active = Some(index);
            self.became_active = true;
        } else {
            // Any other status zeroes the instance clocks, whichever slot it names (`0x54b0fd`).
            self.instance_expiration = None;
            self.run_started = None;
            if self.active == Some(index) {
                self.active = None;
            }
        }
        if status.status == 1 {
            self.tutorials.push(crate::tutorial::id::BATTLEGROUND_QUEUE);
        }
        self.slots[index] = Some((status, now));
    }

    /// The world-enter reset (`0x54ca50`): both statuses go to none until the server answers the
    /// status request.
    fn zero_statuses(&mut self) {
        for (s, _) in self.slots.iter_mut().flatten() {
            s.status = 0;
        }
    }

    pub(crate) fn slots(&self) -> &[Option<(BattlefieldStatus, Instant)>; 2] {
        &self.slots
    }

    /// The map of the instance the player is in, `None` outside one.
    pub(crate) fn active_map(&self) -> Option<u32> {
        self.active.map(|_| self.active_map)
    }

    /// The active slot's queue word, `LeaveBattlefield`'s payload, 0 with none (`0x54c26c`).
    fn active_queue(&self) -> u64 {
        self.active
            .and_then(|i| self.slots[i].as_ref())
            .map_or(0, |(s, _)| s.queue)
    }

    /// The slot's queue word `AcceptBattlefieldPort` sends, 0 for an empty slot.
    fn queue_word(&self, slot: u8) -> u64 {
        self.slots
            .get(usize::from(slot))
            .and_then(|s| s.as_ref())
            .map_or(0, |(s, _)| s.queue)
    }

    pub(crate) fn instance_expiration_ms(&self, now: Instant) -> u32 {
        self.instance_expiration
            .map_or(0, |d| millis(d.saturating_duration_since(now)))
    }

    pub(crate) fn run_time_ms(&self, now: Instant) -> u32 {
        self.run_started.map_or(0, |(at, elapsed)| {
            millis(now.saturating_duration_since(at) + elapsed)
        })
    }

    pub(crate) fn take_became_active(&mut self) -> bool {
        std::mem::take(&mut self.became_active)
    }
}

fn millis(d: Duration) -> u32 {
    d.as_millis().min(u128::from(u32::MAX)) as u32
}

/// The world-PvP queue (`0xbea4a8`), one slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WorldPvpQueue {
    queue_id: u32,
    area_id: u32,
    status: u32,
    /// The invitation's end on the UI clock.
    expire: u32,
}

impl WorldPvpQueue {
    /// The world PvP queue's write (`0x549500`): the slot holding the queue, else a free one; a
    /// zero area or expiry keeps the old.
    fn set(&mut self, queue_id: u32, status: u32, area_id: u32, expire: u32) {
        if self.queue_id != queue_id && self.queue_id != 0 && self.status != 0 {
            return;
        }
        if area_id != 0 {
            self.area_id = area_id;
        }
        if expire != 0 {
            self.expire = expire;
        }
        self.queue_id = queue_id;
        self.status = status;
    }
}

/// The battlemaster session and what the handlers still owe the next feed.
#[derive(Resource, Default)]
pub(crate) struct Battlefield {
    lists: Vec<BattlefieldList>,
    /// The battlemaster the VM's session names, for the walk-away close.
    battlemaster: Option<u64>,
    /// The walk-away closed the session; the VM's half follows on the feed.
    closed: bool,
    verdicts: Vec<GroupJoinedBattleground>,
    /// Joined (`true`) or left guids awaiting a name.
    players: Vec<(u64, bool)>,
    port_denied: u32,
    world_pvp: WorldPvpQueue,
    mgr_events: Vec<(&'static str, Vec<ScriptValue>)>,
    /// `SMSG_BATTLEFIELD_MGR_ENTERED`'s cleared-AFK line, owed to the chat frame.
    cleared_afk: u32,
}

impl Battlefield {
    pub(crate) fn apply_list(&mut self, list: BattlefieldList) {
        if list.battlemaster != 0 && (!list.from_anywhere || list.bg_type == RANDOM_BATTLEGROUND) {
            self.battlemaster = Some(list.battlemaster);
        } else if !list.from_anywhere {
            self.battlemaster = None;
        }
        self.lists.push(list);
    }

    /// The battlemaster the last list named.
    pub(crate) fn battlemaster(&self) -> Option<u64> {
        self.battlemaster
    }

    fn apply_mgr(&mut self, notice: BattlefieldMgrNotice, area: (u32, u32)) {
        let int = |v: u32| ScriptValue::Int(i64::from(v));
        let b = ScriptValue::Bool;
        match notice {
            BattlefieldMgrNotice::EntryInvite {
                queue_id,
                area_id,
                expire,
            } => {
                self.world_pvp.set(queue_id, 2, area_id, expire);
                self.mgr_events
                    .push(("BATTLEFIELD_MGR_ENTRY_INVITE", vec![int(queue_id)]));
            }
            BattlefieldMgrNotice::Entered {
                queue_id,
                invited,
                on_offense,
                cleared_afk,
            } => {
                if cleared_afk {
                    self.cleared_afk += 1;
                }
                if self.world_pvp.queue_id == queue_id || self.world_pvp.status == 0 {
                    self.world_pvp.queue_id = queue_id;
                    self.world_pvp.status = 0;
                }
                self.mgr_events.push((
                    "BATTLEFIELD_MGR_ENTERED",
                    vec![int(queue_id), b(invited), b(on_offense)],
                ));
            }
            BattlefieldMgrNotice::QueueRequestResponse {
                queue_id,
                area_id: queue_area,
                accepted,
                logging_in,
                warmup,
            } => {
                self.world_pvp
                    .set(queue_id, u32::from(accepted), queue_area, 0);
                self.mgr_events.push((
                    "BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE",
                    vec![
                        int(queue_id),
                        b(accepted),
                        b(warmup == 1),
                        b(area.0 == queue_area || area.1 == queue_area),
                        b(logging_in),
                    ],
                ));
            }
            BattlefieldMgrNotice::EjectPending { queue_id, remote } => {
                self.mgr_events.push((
                    "BATTLEFIELD_MGR_EJECT_PENDING",
                    vec![int(queue_id), b(remote)],
                ));
            }
            BattlefieldMgrNotice::Ejected {
                queue_id,
                reason,
                battle_status,
                relocated,
            } => {
                if matches!(reason, 1 | 2 | 4 | 5 | 7..=11)
                    && (self.world_pvp.queue_id == queue_id || self.world_pvp.status == 0)
                {
                    self.world_pvp.queue_id = queue_id;
                    self.world_pvp.status = 0;
                }
                self.mgr_events.push((
                    "BATTLEFIELD_MGR_EJECTED",
                    vec![
                        int(queue_id),
                        b(reason == 8),
                        b(relocated),
                        b(battle_status == 2),
                        b(reason == 10),
                    ],
                ));
            }
            BattlefieldMgrNotice::QueueInvite { queue_id, warmup } => {
                self.mgr_events.push((
                    "BATTLEFIELD_MGR_QUEUE_INVITE",
                    vec![int(queue_id), b(warmup == 1)],
                ));
            }
            BattlefieldMgrNotice::StateChange { old, new } => {
                self.mgr_events
                    .push(("BATTLEFIELD_MGR_STATE_CHANGE", vec![int(old), int(new)]));
            }
        }
    }

    fn clear_session(&mut self) {
        *self = Self::default();
    }
}

impl NpcSession for Battlefield {
    fn npc(&self) -> Option<u64> {
        self.battlemaster
    }

    fn close(&mut self) {
        if self.battlemaster.take().is_some() {
            self.closed = true;
        }
    }
}

/// The player's side for a map description: 0 Horde, 1 Alliance, `None` for neither.
fn side(store: &ObjectStore) -> Option<usize> {
    match crate::ui_unit::race_pvp_team(store.0.unit_race().unwrap_or(0)) {
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

/// Each `BattlemasterList.dbc` row resolved for this player: the bracket for their level, the
/// holiday's world state and their side's map description.
fn battlemaster_rows(
    table: &BattlemasterCatalog,
    maps: Option<&MapCatalog>,
    level: u32,
    side: Option<usize>,
    states: &WorldStates,
) -> Vec<BattlemasterRow> {
    table
        .rows()
        .iter()
        .map(|r| {
            let map = u32::try_from(r.map_ids[0]).ok();
            let has_map = map.is_some_and(|m| maps.is_some_and(|c| c.instance_type(m).is_some()));
            BattlemasterRow {
                id: r.id,
                name: r.name.clone(),
                instance_type: r.instance_type,
                max_group_size: r.max_group_size,
                groups_allowed: r.groups_allowed,
                min_level: r.min_level,
                can_enter: table.bracket_for_level(r, level).is_some(),
                holiday: r.holiday_world_state != 0 && states.get(r.holiday_world_state) != 0,
                description: side.map(|s| {
                    map.and_then(|m| maps?.description(m, s))
                        .unwrap_or_default()
                        .to_string()
                }),
                has_map,
            }
        })
        .collect()
}

/// `GetBattlefieldStatus`' name for a slot (`0x54bcc0`): an invitation's map (the random
/// battleground's through the type it joined), the active map, else the type's own name.
fn slot_name(
    status: &BattlefieldStatus,
    active_map: u32,
    table: Option<&BattlemasterCatalog>,
    maps: Option<&MapCatalog>,
) -> Option<String> {
    let map = match status.status {
        2 => status.invite.and_then(|i| {
            if status.bg_type() == RANDOM_BATTLEGROUND {
                let joined = benilla_protocol::messages::queue_bg_type(i.joined_queue);
                (i.joined_queue != 0)
                    .then(|| table?.get(joined).map(|r| r.map_ids[0]))
                    .flatten()
                    .and_then(|m| u32::try_from(m).ok())
            } else {
                Some(i.map_id)
            }
        }),
        3 => Some(active_map),
        _ => None,
    };
    if let Some(name) = map.and_then(|m| maps?.name(m)) {
        return Some(name.to_string());
    }
    table
        .and_then(|t| t.get(status.bg_type()))
        .map(|r| r.name.clone())
}

/// One slot's VM view at `now`.
fn slot_view(
    slot: Option<&(BattlefieldStatus, Instant)>,
    active_map: u32,
    table: Option<&BattlemasterCatalog>,
    maps: Option<&MapCatalog>,
    now: Instant,
) -> BattlefieldQueueSlot {
    let Some((s, at)) = slot else {
        return BattlefieldQueueSlot::default();
    };
    let mut view = BattlefieldQueueSlot {
        status: s.status,
        name: slot_name(s, active_map, table, maps),
        bg_type: s.bg_type(),
        instance_id: s.instance_id,
        min_level: u32::from(s.min_level),
        max_level: u32::from(s.max_level),
        team_size: s.team_size(),
        rated: s.rated,
        ..Default::default()
    };
    if let Some(i) = s.invite.filter(|i| i.expire_ms != 0) {
        let deadline = *at + Duration::from_millis(u64::from(i.expire_ms));
        view.port_expiration_ms = millis(deadline.saturating_duration_since(now));
    }
    if let Some((estimate, waited)) = s.queued {
        view.estimated_wait_ms = estimate;
        view.time_waited_ms =
            millis(now.saturating_duration_since(*at) + Duration::from_millis(u64::from(waited)));
    }
    view
}

/// `AcceptBattlefieldPort`'s refusals (`0x54da10`) in its order, skipped on a battleground map. The
/// transport and fatigue legs are not modelled: the server refuses the port on its own there.
fn port_refusal(
    store: &ObjectStore,
    player: &Player,
    on_battleground_map: bool,
) -> Option<&'static str> {
    if on_battleground_map {
        return None;
    }
    let f = &store.0;
    if f.unit_flags() & UNIT_FLAG_PORT_COMBAT != 0 {
        return Some("ERR_NOT_IN_COMBAT");
    }
    if f.unit_health() == Some(0) || f.player_flags() & PLAYER_FLAGS_GHOST != 0 {
        return Some("ERR_PLAYER_DEAD");
    }
    if player.is_airborne() {
        return Some("ERR_NOT_WHILE_FALLING");
    }
    None
}

/// `BattlemasterList.dbc` resolved for the player, before `PLAYER_ENTERING_WORLD`: the reference
/// builds the type list at the game's start (`0x54e720`), and stock `PVPBattlegroundFrame.lua`
/// hides the PvP window's tabs at that event when no type reads `canEnter`.
fn feed_battlemaster_rows(
    script: Option<NonSendMut<UiScript>>,
    table: Option<Res<BattlemasterTable>>,
    maps: Option<Res<MapCatalogRes>>,
    states: Res<WorldStates>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut player: Local<crate::ui_script::VmMemo<crate::ui_script::gate::Watch>>,
) {
    let (Some(mut script), Some(table), Some(store)) = (script, table, self_q.iter().next()) else {
        return;
    };
    let level = store.0.unit_level().unwrap_or(0);
    // The rows read the table, the maps, the world states, the level and the side.
    let (player, vm_reset) = player.get_reset(&script);
    let side_bits = side(store).map_or(0, |s| s as u64 + 1);
    let player_moved = player.moved(u64::from(level) << 8 | side_bits);
    let inputs = [
        ("vm_reset", vm_reset),
        ("player", player_moved),
        ("table", table.is_changed()),
        ("maps", maps.as_ref().is_some_and(|m| m.is_changed())),
        ("states", states.is_changed()),
    ];
    crate::ui_script::gate::trace("feed_battlemaster_rows", &inputs);
    if crate::ui_script::gate::Gate::new(inputs.iter().any(|&(_, open)| open)).skip() {
        return;
    }
    let maps = maps.as_deref().map(|m| &m.0);
    script.set_battlemaster_rows(battlemaster_rows(
        &table.0,
        maps,
        level,
        side(store),
        &states,
    ));
}

/// The VM's pushes and the handlers' lines and events.
#[allow(clippy::too_many_arguments)]
fn feed_battlefield(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<Battlefield>,
    mut queue: ResMut<BattlefieldQueue>,
    table: Option<Res<BattlemasterTable>>,
    maps: Option<Res<MapCatalogRes>>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    area: Option<Res<benilla_world::terrain_stream::CurrentArea>>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    ui_clock: Option<Res<crate::ui_notices::UiClock>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    let table = table.as_deref().map(|t| &t.0);
    let maps = maps.as_deref().map(|m| &m.0);
    let store = self_q.iter().next();

    for list in std::mem::take(&mut state.lists) {
        script.apply_battlefield_list(BattlefieldListPush {
            battlemaster: list.battlemaster,
            from_anywhere: list.from_anywhere,
            bg_type: list.bg_type,
            holiday: reward(list.holiday),
            random: list.random.map(reward).unwrap_or_default(),
            instances: list.instances,
        });
    }
    if std::mem::take(&mut state.closed) {
        script.close_battlemaster();
    }

    let instance_type = (queue.active_map != 0)
        .then(|| maps.and_then(|m| m.instance_type(queue.active_map)))
        .flatten()
        .unwrap_or(0);
    let area_id = area.as_deref().and_then(|a| a.0).unwrap_or(0);
    // The current-map arena test (`0x54a700`) despite its name tests for a battleground map,
    // type 3.
    let on_battleground_map = maps
        .zip(map.as_deref())
        .and_then(|(m, id)| m.instance_type(id.0))
        == Some(INSTANCE_TYPE_BATTLEGROUND);
    let world_pvp = &state.world_pvp;
    // The UI clock, `time()` plus the server's offset (`0x548700`), as `GetWintergraspWaitTime`
    // reads it.
    let ui_now = ui_clock.as_deref().map_or_else(
        || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i32)
        },
        crate::ui_notices::UiClock::now,
    );
    let frame = BattlefieldFrame {
        slots: [0, 1]
            .map(|i| slot_view(queue.slots[i].as_ref(), queue.active_map, table, maps, now)),
        active: queue.active.map(|i| i as u8),
        instance_type,
        arena_faction: queue.arena_faction,
        instance_expiration_ms: queue.instance_expiration_ms(now),
        run_time_ms: queue.run_time_ms(now),
        world_pvp: WorldPvpQueueView {
            queue_id: world_pvp.queue_id as i32,
            status: world_pvp.status,
            area_name: areas
                .as_deref()
                .and_then(|a| a.0.name(world_pvp.area_id))
                .map(str::to_string),
            expire_in: (world_pvp.expire as i32).wrapping_sub(ui_now),
        },
        can_hearth_and_resurrect: areas
            .as_deref()
            .and_then(|a| a.0.get(area_id))
            .is_some_and(|r| r.flags & AREA_FLAG_HEARTH_AND_RESURRECT != 0),
        port_refusal: store.and_then(|s| port_refusal(s, &player, on_battleground_map)),
    };
    script.set_battlefield_frame(frame);

    for id in std::mem::take(&mut queue.tutorials) {
        if let Some(t) = tutorials.as_mut() {
            t.write(crate::tutorial::TutorialEvent::trigger(id));
        }
    }
    for slot in std::mem::take(&mut queue.events) {
        script.fire_event(
            "UPDATE_BATTLEFIELD_STATUS",
            vec![ScriptValue::Int(i64::from(slot))],
        );
    }
    for (name, args) in std::mem::take(&mut state.mgr_events) {
        script.fire_event(name, args);
    }

    let mut lines = Vec::new();
    for verdict in std::mem::take(&mut state.verdicts) {
        lines.extend(verdict_line(&script, verdict, table, &names, &commands));
    }
    for _ in 0..std::mem::take(&mut state.port_denied) {
        lines.extend(crate::ui_action::keyed_line(&script, "ERR_PLAYER_DEAD"));
    }
    let pending = std::mem::take(&mut state.players);
    for (guid, joined) in pending {
        let Some(name) = names.resolve(guid, &commands).map(str::to_string) else {
            state.players.push((guid, joined));
            continue;
        };
        let line = if joined {
            crate::ui_action::keyed_line_s(&script, "ERR_BG_PLAYER_JOINED_SS", &[&name, &name])
        } else {
            crate::ui_action::keyed_line_s(&script, "ERR_BG_PLAYER_LEFT_S", &[&name])
        };
        lines.extend(line);
    }
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_battlefield", lines);
    }
}

fn reward(r: benilla_protocol::messages::BattlefieldReward) -> BattlefieldRewardView {
    BattlefieldRewardView {
        has_win: r.has_win,
        win_honor: r.win_honor,
        win_arena: r.win_arena,
        loss_honor: r.loss_honor,
    }
}

/// `SMSG_GROUP_JOINED_BATTLEGROUND`'s line (`0x54b1c0`): an error by code, or the type joined.
fn verdict_line(
    script: &UiScript,
    verdict: GroupJoinedBattleground,
    table: Option<&BattlemasterCatalog>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<crate::ui_action::Shown> {
    let key = match verdict.result {
        -15 => "ERR_IN_NON_RANDOM_BG",
        -14 => "ERR_IN_RANDOM_BG",
        -13 => "ERR_LFG_CANT_USE_BATTLEGROUND",
        -12 | -11 => {
            let name = verdict
                .player
                .and_then(|g| names.resolve(g, commands))
                .map(str::to_string);
            return match name {
                Some(n) => {
                    crate::ui_action::keyed_line_s(script, "ERR_BATTLEGROUND_JOIN_TIMED_OUT", &[&n])
                }
                None => crate::ui_action::keyed_line(script, "ERR_BATTLEGROUND_JOIN_FAILED"),
            };
        }
        -10 => "ERR_BATTLEGROUND_JOIN_RANGE_INDEX",
        -9 => "ERR_BATTLEGROUND_JOIN_XP_GAIN",
        -8 => "ERR_BATTLEGROUND_NOT_IN_BATTLEGROUND",
        -7 => "ERR_BATTLEGROUND_TEAM_LEFT_QUEUE",
        -6 => "ERR_BATTLEDGROUND_QUEUED_FOR_RATED",
        -5 => "ERR_BATTLEGROUND_CANNOT_QUEUE_FOR_RATED",
        -4 => "ERR_BATTLEGROUND_TOO_MANY_QUEUES",
        -3 => "ERR_ARENA_TEAM_PARTY_SIZE",
        -2 => "ERR_GROUP_JOIN_BATTLEGROUND_DESERTERS",
        -1 => return None,
        id => {
            return match table.and_then(|t| t.get(id as u32)) {
                Some(r) => crate::ui_action::keyed_line_s(
                    script,
                    "ERR_GROUP_JOIN_BATTLEGROUND_S",
                    &[&r.name],
                ),
                None => crate::ui_action::keyed_line(script, "ERR_GROUP_JOIN_BATTLEGROUND_FAIL"),
            };
        }
    };
    crate::ui_action::keyed_line(script, key)
}

/// The VM's sends: the joins (`0x54cc70`), the list requests (`0x54e5b0`), the port answers
/// (`0x54da10`), the leave (`0x54c250`), the hearth and the battle-manager answers.
fn drain_battlefield(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<Battlefield>,
    queue: Res<BattlefieldQueue>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let send = |c| {
        let _ = commands.0.send(c);
    };
    for join in script.take_battlefield_joins() {
        send(match join {
            BattlefieldJoin::Battleground {
                battlemaster,
                bg_type,
                instance_id,
                as_group,
            } => ClientCommand::BattlemasterJoin {
                battlemaster,
                bg_type,
                instance_id,
                as_group,
            },
            BattlefieldJoin::Arena {
                battlemaster,
                slot,
                as_group,
                rated,
            } => ClientCommand::BattlemasterJoinArena {
                battlemaster,
                slot,
                as_group,
                rated,
            },
        });
    }
    let store = self_q.iter().next();
    for bg_type in script.take_battlefield_list_requests() {
        let xp_disabled = store.is_some_and(|s| {
            let f = &s.0;
            f.unit_level() < f.player_max_level() && f.player_flags() & PLAYER_FLAGS_NO_XP_GAIN != 0
        });
        send(ClientCommand::BattlefieldList {
            bg_type,
            xp_disabled,
        });
    }
    for (slot, accept) in script.take_battlefield_port_requests() {
        send(ClientCommand::BattlefieldPort {
            queue: queue.queue_word(slot),
            accept,
        });
    }
    if script.take_battlefield_closes() > 0 {
        state.battlemaster = None;
    }
    for _ in 0..script.take_battlefield_leave_requests() {
        send(ClientCommand::LeaveBattlefield {
            queue: queue.active_queue(),
        });
    }
    for _ in 0..script.take_hearth_and_resurrect_requests() {
        send(ClientCommand::HearthAndResurrect);
    }
    let mgr = script.take_battlefield_mgr_requests();
    if store.is_some() {
        for r in mgr {
            send(ClientCommand::BattlefieldMgr(r));
        }
    }
}

/// The world-enter cascade's battlefield init (`0x54ca50`): the selection, type and battlemaster
/// clear, both statuses go to none, and the bodyless `CMSG_BATTLEFIELD_STATUS` goes out, answered
/// slot by slot.
pub(crate) fn reset_on_world_enter(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    mut state: ResMut<Battlefield>,
    mut queue: ResMut<BattlefieldQueue>,
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    state.lists.clear();
    state.battlemaster = None;
    queue.zero_statuses();
    if let Some(mut script) = script {
        script.reset_battlefield_on_world_enter();
    }
    let _ = commands.0.send(ClientCommand::BattlefieldStatusRequest);
}

fn load_battlemaster_list(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_battlemaster_catalog(&mut chain)
    };
    match loaded {
        Ok(table) => {
            info!(
                "ui_battlefield: BattlemasterList.dbc loaded ({} types)",
                table.len()
            );
            commands.insert_resource(BattlemasterTable(table));
        }
        Err(e) => warn!(
            "ui_battlefield: BattlemasterList.dbc failed to load — the PvP window lists no \
             battleground and no join resolves its type: {e:#}"
        ),
    }
}

/// The queue's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{Battlefield, BattlefieldQueue};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldStatus, on_packet)
            .net_handler(K::BattlefieldList, on_packet)
            .net_handler(K::GroupJoinedBattleground, on_packet)
            .net_handler(K::BattlegroundPlayer, on_packet)
            .net_handler(K::BattlefieldPortDenied, on_packet)
            .net_handler(K::BattlefieldMgr, on_packet)
            .net_handler(K::ArenaUnitDestroyed, on_arena_unit_destroyed)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// `SMSG_ARENA_UNIT_DESTROYED` (`0x54b5e0`), in an arena, fires `ARENA_OPPONENT_UPDATE` for the
    /// opponent or pet slot holding the guid. No opponent slot is tracked (the `arenaN` tokens, #90), so no
    /// slot matches and nothing fires, as the reference does for an unknown guid.
    fn on_arena_unit_destroyed(In(ev): In<SessionEvent>) {
        if let SessionEvent::ArenaUnitDestroyed(guid) = ev {
            debug!("ui_battlefield: arena unit {guid:#x} destroyed — no opponent slot holds it");
        }
    }

    /// The queue and the session end with the session, as the game's start (`0x54e720`) zeroes them
    /// at every login.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut state: ResMut<Battlefield>,
    ) {
        *queue = BattlefieldQueue::default();
        state.clear_session();
    }

    fn on_packet(
        In(ev): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut state: ResMut<Battlefield>,
        area: Option<Res<benilla_world::terrain_stream::CurrentArea>>,
        areas: Option<Res<crate::area::AreaTableRes>>,
    ) {
        let here = area.as_deref().and_then(|a| a.0).unwrap_or(0);
        let parent = areas
            .as_deref()
            .and_then(|a| a.0.get(here))
            .map_or(0, |r| r.zone_id);
        match ev {
            SessionEvent::BattlefieldStatus(s) => queue.apply(s),
            SessionEvent::BattlefieldList(list) => state.apply_list(list),
            SessionEvent::GroupJoinedBattleground(j) => state.verdicts.push(j),
            SessionEvent::BattlegroundPlayer { guid, joined } => state.players.push((guid, joined)),
            SessionEvent::BattlefieldPortDenied => state.port_denied += 1,
            SessionEvent::BattlefieldMgr(n) => state.apply_mgr(n, (here, parent)),
            _ => {}
        }
    }
}

pub(crate) struct BattlefieldPlugin;

impl Plugin for BattlefieldPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Battlefield>()
            .init_resource::<BattlefieldQueue>()
            .add_systems(Startup, load_battlemaster_list.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    reset_on_world_enter
                        .in_set(UiFeed)
                        .after(crate::ui_mail::send_query_next_mail_time_on_enter)
                        .before(feed_battlefield),
                    crate::ui_session::close_npc_session_out_of_range::<Battlefield>
                        .before(feed_battlefield),
                    // After the scoreboard: a status 3 fires the score event before the status
                    // event (`0x54b148`, then `0x54b1a9`).
                    feed_battlefield
                        .after(crate::ui_battlefield_score::feed_battlefield_score)
                        .in_set(UiFeed),
                    feed_battlemaster_rows
                        .in_set(UiFeed)
                        .before(crate::ui_unit::EnterWorldFeed),
                    drain_battlefield.after(UiInput),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldInvite};

    const WSG: u64 = 0x1F90_0002_0000;

    fn status(slot: u32, status: u32) -> BattlefieldStatus {
        BattlefieldStatus {
            slot,
            queue: WSG,
            min_level: 10,
            max_level: 19,
            instance_id: 7,
            status,
            ..Default::default()
        }
    }

    #[test]
    fn the_queue_keeps_two_slots_and_fires_per_slot() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        q.apply_at(status(0, 1), now);
        q.apply_at(status(1, 1), now);
        q.apply_at(status(2, 1), now);
        assert_eq!(q.events, vec![1, 2], "a third slot is dropped unannounced");
        assert_eq!(q.queue_word(1), WSG);
        q.apply_at(
            BattlefieldStatus {
                slot: 1,
                ..Default::default()
            },
            now,
        );
        assert!(q.slots[1].is_none() && q.queue_word(1) == 0);
    }

    #[test]
    fn the_instance_clocks_follow_the_status_handler() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        let mut active = status(0, 3);
        active.active = Some(BattlefieldActive {
            map_id: 489,
            joined_queue: 0,
            shutdown_ms: 120_000,
            run_ms: 30_000,
            arena_faction: 1,
        });
        q.apply_at(active, now);
        assert_eq!(q.active_map(), Some(489));
        assert_eq!(q.active_queue(), WSG);
        assert!(q.take_became_active());
        let later = now + Duration::from_secs(10);
        assert_eq!(q.run_time_ms(later), 40_000);
        assert_eq!(q.instance_expiration_ms(later), 110_000);

        // A queued status on the other slot zeroes the clocks and keeps the active index.
        q.apply_at(status(1, 1), later);
        assert_eq!(q.run_time_ms(later), 0);
        assert_eq!(q.active_map(), Some(489));

        let mut confirm = status(1, 2);
        confirm.invite = Some(BattlefieldInvite {
            map_id: 529,
            joined_queue: 0,
            expire_ms: 80_000,
        });
        q.apply_at(confirm.clone(), later);
        let v = slot_view(q.slots[1].as_ref(), q.active_map, None, None, later);
        assert_eq!((v.status, v.port_expiration_ms), (2, 80_000));

        q.apply_at(
            BattlefieldStatus {
                slot: 0,
                ..Default::default()
            },
            later,
        );
        assert_eq!(
            q.active_map(),
            None,
            "clearing the active slot drops the instance"
        );
    }

    #[test]
    fn the_world_pvp_queue_keeps_its_area_across_a_bare_update() {
        let mut w = WorldPvpQueue::default();
        w.set(1, 1, 4197, 0);
        w.set(1, 2, 0, 1_000);
        assert_eq!(
            (w.queue_id, w.status, w.area_id, w.expire),
            (1, 2, 4197, 1_000)
        );
        w.set(2, 1, 99, 0);
        assert_eq!(w.queue_id, 1, "a busy slot keeps its queue");
    }
}
