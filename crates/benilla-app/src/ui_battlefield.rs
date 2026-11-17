//! The app half of the reference's battlefield queue (`0x549ad0`-`0x54e720`): the two queue slots
//! `SMSG_BATTLEFIELD_STATUS` writes, the instance list a battlemaster or the PvP window opens, the
//! join verdicts and the Wintergrasp queue, read by the stock `PVPBattlegroundFrame.lua`,
//! `BattlefieldFrame.lua`, `ArenaFrame.lua` and the minimap icon. The scoreboard is
//! [`crate::ui_battlefield_score`]'s, the teammate positions [`crate::ui_battlefield_positions`]'.
//!
//! Each handler applies its packet and fires its events in turn (`0x54ae40`, `0x54e390`), and the
//! verbs read the queue when called through [`BattlefieldView`]. The VM owns what the bindings
//! write (the listed type, the selection, the battlemaster), which the list handler writes in turn.

use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
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
use crate::ui_action::UiError;
use crate::ui_script::live::UiEvents;
use crate::ui_script::{UiDrain, UiFeed};
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
        if let Some(a) = status.active {
            self.active_map = a.map_id;
            let ms = |v: u32| Duration::from_millis(u64::from(v));
            self.instance_expiration = (a.shutdown_ms != 0).then(|| now + ms(a.shutdown_ms));
            self.run_started = (a.run_ms != 0).then(|| (now, ms(a.run_ms)));
            self.arena_faction = a.arena_faction;
            self.active = Some(index);
        } else {
            // Any other status zeroes the instance clocks, whichever slot it names (`0x54b0fd`).
            self.instance_expiration = None;
            self.run_started = None;
            if self.active == Some(index) {
                self.active = None;
            }
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

    /// Our arena side from the active status, which a streamed player's is compared against.
    pub(crate) fn arena_faction(&self) -> u8 {
        self.arena_faction
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
    /// Whether the slot search finds this slot for `queue_id`: it holds the queue, or it is free,
    /// a zero area or status word (`0x549534`-`0x54953a`).
    fn takes(&self, queue_id: u32) -> bool {
        self.queue_id == queue_id || self.area_id == 0 || self.status == 0
    }

    /// The world PvP queue's write (`0x549500`): the slot holding the queue, else a free one; a
    /// zero area or expiry keeps the old.
    fn set(&mut self, queue_id: u32, status: u32, area_id: u32, expire: u32) {
        if !self.takes(queue_id) {
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

    /// The battle-manager notices' writes and the event each fires after them, in its handler's
    /// order (`0x54b610`-`0x5499c0`).
    fn apply(
        &mut self,
        notice: BattlefieldMgrNotice,
        area: (u32, u32),
    ) -> (&'static str, Vec<ScriptValue>) {
        let int = |v: u32| ScriptValue::Int(i64::from(v));
        let b = ScriptValue::Bool;
        match notice {
            BattlefieldMgrNotice::EntryInvite {
                queue_id,
                area_id,
                expire,
            } => {
                self.set(queue_id, 2, area_id, expire);
                ("BATTLEFIELD_MGR_ENTRY_INVITE", vec![int(queue_id)])
            }
            BattlefieldMgrNotice::Entered {
                queue_id,
                invited,
                on_offense,
                ..
            } => {
                // `0x54993d`-`0x54998a`.
                if self.takes(queue_id) {
                    self.queue_id = queue_id;
                    self.status = 0;
                }
                (
                    "BATTLEFIELD_MGR_ENTERED",
                    vec![int(queue_id), b(invited), b(on_offense)],
                )
            }
            BattlefieldMgrNotice::QueueRequestResponse {
                queue_id,
                area_id: queue_area,
                accepted,
                logging_in,
                warmup,
            } => {
                self.set(queue_id, u32::from(accepted), queue_area, 0);
                (
                    "BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE",
                    vec![
                        int(queue_id),
                        b(accepted),
                        b(warmup == 1),
                        b(area.0 == queue_area || area.1 == queue_area),
                        b(logging_in),
                    ],
                )
            }
            BattlefieldMgrNotice::EjectPending { queue_id, remote } => (
                "BATTLEFIELD_MGR_EJECT_PENDING",
                vec![int(queue_id), b(remote)],
            ),
            BattlefieldMgrNotice::Ejected {
                queue_id,
                reason,
                battle_status,
                relocated,
            } => {
                // `0x549a15`-`0x549a6a`.
                if matches!(reason, 1 | 2 | 4 | 5 | 7..=11) && self.takes(queue_id) {
                    self.queue_id = queue_id;
                    self.status = 0;
                }
                (
                    "BATTLEFIELD_MGR_EJECTED",
                    vec![
                        int(queue_id),
                        b(reason == 8),
                        b(relocated),
                        b(battle_status == 2),
                        b(reason == 10),
                    ],
                )
            }
            BattlefieldMgrNotice::QueueInvite { queue_id, warmup } => (
                "BATTLEFIELD_MGR_QUEUE_INVITE",
                vec![int(queue_id), b(warmup == 1)],
            ),
            BattlefieldMgrNotice::StateChange { old, new } => {
                ("BATTLEFIELD_MGR_STATE_CHANGE", vec![int(old), int(new)])
            }
        }
    }
}

/// The world-PvP queue the battle-manager handlers write and `GetWorldPVPQueueStatus` reads.
#[derive(Resource, Default)]
pub(crate) struct WorldPvp(WorldPvpQueue);

/// The battlemaster session and the joins and leaves still waiting on a name.
#[derive(Resource, Default)]
pub(crate) struct Battlefield {
    /// The battlemaster the VM's session names, for the walk-away close.
    battlemaster: Option<u64>,
    /// The walk-away closed the session; the VM's half follows on the drain.
    closed: bool,
    /// Joined (`true`) or left guids awaiting a name (`0x54a530`, `0x54a5c0`).
    players: Vec<(u64, bool)>,
}

impl Battlefield {
    pub(crate) fn apply_list(&mut self, list: &BattlefieldList) {
        if list.battlemaster != 0 && (!list.from_anywhere || list.bg_type == RANDOM_BATTLEGROUND) {
            self.battlemaster = Some(list.battlemaster);
        } else if !list.from_anywhere {
            self.battlemaster = None;
        }
    }

    /// The battlemaster the last list named.
    pub(crate) fn battlemaster(&self) -> Option<u64> {
        self.battlemaster
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
    states: Option<&WorldStates>,
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
                holiday: r.holiday_world_state != 0
                    && states.is_some_and(|s| s.get(r.holiday_world_state) != 0),
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
    player: Option<&Player>,
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
    if player.is_some_and(Player::is_airborne) {
        return Some("ERR_NOT_WHILE_FALLING");
    }
    None
}

/// What the queue's verbs read when called, as 12340's read its globals (`0x549ad0`-`0x54e6d0`):
/// the queue, the world-PvP slot, the tables, where we stand and the UI clock.
#[derive(SystemParam)]
pub(crate) struct BattlefieldView<'w, 's> {
    queue: Option<Res<'w, BattlefieldQueue>>,
    world_pvp: Option<Res<'w, WorldPvp>>,
    table: Option<Res<'w, BattlemasterTable>>,
    maps: Option<Res<'w, MapCatalogRes>>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    area: Option<Res<'w, benilla_world::terrain_stream::CurrentArea>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    player: Option<Res<'w, Player>>,
    ui_clock: Option<Res<'w, crate::ui_notices::UiClock>>,
    states: Option<Res<'w, WorldStates>>,
}

impl BattlefieldView<'_, '_> {
    /// The slots and clocks at this moment, the active instance and the gates.
    pub(crate) fn frame(&self) -> BattlefieldFrame {
        let Some(queue) = self.queue.as_deref() else {
            return BattlefieldFrame::default();
        };
        let now = Instant::now();
        let table = self.table.as_deref().map(|t| &t.0);
        let maps = self.maps.as_deref().map(|m| &m.0);
        let instance_type = (queue.active_map != 0)
            .then(|| maps.and_then(|m| m.instance_type(queue.active_map)))
            .flatten()
            .unwrap_or(0);
        let area_id = self.area.as_deref().and_then(|a| a.0).unwrap_or(0);
        // The current-map arena test (`0x54a700`) despite its name tests for a battleground map,
        // type 3.
        let on_battleground_map = maps
            .zip(self.map.as_deref())
            .and_then(|(m, id)| m.instance_type(id.0))
            == Some(INSTANCE_TYPE_BATTLEGROUND);
        let world_pvp = self.world_pvp.as_deref().map(|w| w.0).unwrap_or_default();
        // The UI clock, `time()` plus the server's offset (`0x548700`), as
        // `GetWintergraspWaitTime` reads it.
        let ui_now = self.ui_clock.as_deref().map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i32)
            },
            crate::ui_notices::UiClock::now,
        );
        let areas = self.areas.as_deref();
        BattlefieldFrame {
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
                    .and_then(|a| a.0.name(world_pvp.area_id))
                    .map(str::to_string),
                expire_in: (world_pvp.expire as i32).wrapping_sub(ui_now),
            },
            can_hearth_and_resurrect: areas
                .and_then(|a| a.0.get(area_id))
                .is_some_and(|r| r.flags & AREA_FLAG_HEARTH_AND_RESURRECT != 0),
            port_refusal: self
                .me
                .iter()
                .next()
                .and_then(|s| port_refusal(s, self.player.as_deref(), on_battleground_map)),
        }
    }

    /// `BattlemasterList.dbc` resolved for us now: none before the table loads or our create.
    pub(crate) fn rows(&self) -> Vec<BattlemasterRow> {
        let (Some(table), Some(store)) = (self.table.as_deref(), self.me.iter().next()) else {
            return Vec::new();
        };
        battlemaster_rows(
            &table.0,
            self.maps.as_deref().map(|m| &m.0),
            store.0.unit_level().unwrap_or(0),
            side(store),
            self.states.as_deref(),
        )
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
    verdict: GroupJoinedBattleground,
    table: Option<&BattlemasterCatalog>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<UiError> {
    let key = match verdict.result {
        -15 => "ERR_IN_NON_RANDOM_BG",
        -14 => "ERR_IN_RANDOM_BG",
        -13 => "ERR_LFG_CANT_USE_BATTLEGROUND",
        -12 | -11 => {
            let name = verdict.player.and_then(|g| names.resolve(g, commands));
            return Some(match name {
                Some(n) => UiError::s("ERR_BATTLEGROUND_JOIN_TIMED_OUT", n),
                None => UiError::key("ERR_BATTLEGROUND_JOIN_FAILED"),
            });
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
            return Some(match table.and_then(|t| t.get(id as u32)) {
                Some(r) => UiError::s("ERR_GROUP_JOIN_BATTLEGROUND_S", r.name.clone()),
                None => UiError::key("ERR_GROUP_JOIN_BATTLEGROUND_FAIL"),
            });
        }
    };
    Some(UiError::key(key))
}

/// A battleground join's or leave's line, the name as `name-realm` for another realm
/// (`0x54b55a`-`0x54b58a`).
fn player_line(joined: bool, name: &str) -> UiError {
    if joined {
        UiError::strings("ERR_BG_PLAYER_JOINED_SS", &[name, name])
    } else {
        UiError::s("ERR_BG_PLAYER_LEFT_S", name)
    }
}

/// The name-cache callbacks of a join or leave whose name was not cached (`0x54a530`,
/// `0x54a5c0`): its line shows once the name lands.
fn announce_named_players(
    mut state: ResMut<Battlefield>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
) {
    if state.players.is_empty() {
        return;
    }
    state.players.retain(|&(guid, joined)| {
        let Some(name) = names.full_name(guid, &commands) else {
            return true;
        };
        errors.0.push(player_line(joined, &name));
        false
    });
}

/// The VM's sends: the joins (`0x54cc70`), the list requests (`0x54e5b0`), the port answers
/// (`0x54da10`), the leave (`0x54c250`), the hearth and the battle-manager answers; and the
/// walk-away close of the battlemaster session (`0x5496d0` with none).
fn drain_battlefield(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut state: ResMut<Battlefield>,
    queue: Res<BattlefieldQueue>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if std::mem::take(&mut state.closed) {
        script.close_battlemaster();
    }
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

/// The world-enter cascade's status request, between the mail query and the LFG entry as the
/// cascade orders them (`0x528010`): the bodyless `CMSG_BATTLEFIELD_STATUS` of `0x54ca50`,
/// answered slot by slot.
pub(crate) fn request_status_on_world_enter(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    commands: Res<NetCommands>,
) {
    if cascades.read().next().is_some() {
        let _ = commands.0.send(ClientCommand::BattlefieldStatusRequest);
    }
}

/// The battlefield's world entry after `PLAYER_ENTERING_WORLD` (`0x528010` -> `0x54ca50`): the
/// selection, type and battlemaster clear and both statuses go to none, in turn with the events
/// around it.
fn enter_world(
    queue: Option<ResMut<BattlefieldQueue>>,
    state: Option<ResMut<Battlefield>>,
    ui: Option<ResMut<UiEvents>>,
) {
    if let Some(mut queue) = queue {
        queue.zero_statuses();
    }
    if let Some(mut state) = state {
        state.battlemaster = None;
    }
    if let Some(mut ui) = ui {
        ui.write(UiScript::reset_battlefield_on_world_enter);
    }
}

/// Queue the battlefield's world entry (`0x54ca50`) to run once the calling system's commands
/// apply: the queue's globals, the scoreboard's, then the arena slots and their events.
pub(crate) fn queue_enter_world(commands: &mut Commands) {
    commands.queue(|world: &mut World| {
        if let Err(e) = world.run_system_cached(enter_world) {
            warn!("ui_battlefield: the world entry did not run: {e}");
        }
        crate::ui_battlefield_score::enter_world(world);
        crate::ui_arena::enter_world(world);
        crate::ui_script::live::fire_handler_events(world);
    });
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

/// The queue's packet handlers, each firing its events once its changes apply.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use benilla_ui::script::ScriptValue;
    use bevy::prelude::*;

    use super::{
        player_line, reward, verdict_line, Battlefield, BattlefieldListPush, BattlefieldQueue,
        BattlemasterTable, WorldPvp,
    };
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp, SelfPlayer};
    use crate::ui_action::{UiError, UiErrorKeys};
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldStatus, on_status)
            .net_handler(K::BattlefieldList, on_list)
            .net_handler(K::GroupJoinedBattleground, on_verdict)
            .net_handler(K::BattlegroundPlayer, on_player)
            .net_handler(K::BattlefieldPortDenied, on_port_denied)
            .net_handler(K::BattlefieldMgr, on_mgr)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The queue and the session end with the session, as the game's start (`0x54e720`) zeroes them
    /// at every login.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut state: ResMut<Battlefield>,
        mut world_pvp: ResMut<WorldPvp>,
    ) {
        *queue = BattlefieldQueue::default();
        *state = Battlefield::default();
        *world_pvp = WorldPvp::default();
    }

    /// `SMSG_BATTLEFIELD_STATUS` (`0x54ae40`): the slot, the confirm tutorial (48) as it reads the
    /// invitation, on status 3 the map's score columns, the filter reset and the score rebuild
    /// (`0x54b026`, `0x54b0cc`, `0x54b0d6`) that fires `UPDATE_BATTLEFIELD_SCORE` with every name
    /// known, the queued tutorial (47), then `UPDATE_BATTLEFIELD_STATUS` with the 1-based slot
    /// (`0x54b1a9`); a third slot is dropped.
    pub(super) fn on_status(
        In(ev): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut ui: ResMut<UiEvents>,
        mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
        catalog: Option<Res<crate::world_state_ui::WorldStateUiRes>>,
    ) {
        let SessionEvent::BattlefieldStatus(status) = ev else {
            return;
        };
        let slot = status.slot;
        if slot >= 2 {
            return;
        }
        let held = status.queue != 0;
        let state = status.status;
        let active = status.active.is_some();
        queue.apply(status);
        let mut tutorial = |id| {
            if let Some(t) = tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::trigger(id));
            }
        };
        if held && state == 2 {
            tutorial(crate::tutorial::id::PORT_TO_BATTLEGROUND);
        }
        if held && active {
            let columns = queue
                .active_map()
                .zip(catalog.as_deref())
                .map(|(map, cat)| crate::ui_battlefield_score::score_columns(cat, map))
                .unwrap_or_default();
            ui.write(move |s| s.activate_battlefield_scores(columns));
        }
        if held && state == 1 {
            tutorial(crate::tutorial::id::BATTLEGROUND_QUEUE);
        }
        ui.fire(
            "UPDATE_BATTLEFIELD_STATUS",
            vec![ScriptValue::Int(i64::from(slot + 1))],
        );
    }

    /// `SMSG_BATTLEFIELD_LIST` (`0x54e390`): the type, rewards and instances, then the event its
    /// source names, written into the VM in turn.
    fn on_list(In(ev): In<SessionEvent>, mut state: ResMut<Battlefield>, mut ui: ResMut<UiEvents>) {
        let SessionEvent::BattlefieldList(list) = ev else {
            return;
        };
        state.apply_list(&list);
        let push = BattlefieldListPush {
            battlemaster: list.battlemaster,
            from_anywhere: list.from_anywhere,
            bg_type: list.bg_type,
            holiday: reward(list.holiday),
            random: list.random.map(reward).unwrap_or_default(),
            instances: list.instances,
        };
        ui.write(move |s| s.apply_battlefield_list(push));
    }

    /// `SMSG_GROUP_JOINED_BATTLEGROUND` (`0x54b1c0`): its line, shown from the handler.
    fn on_verdict(
        In(ev): In<SessionEvent>,
        table: Option<Res<BattlemasterTable>>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut errors: ResMut<UiErrorKeys>,
    ) {
        let SessionEvent::GroupJoinedBattleground(verdict) = ev else {
            return;
        };
        let table = table.as_deref().map(|t| &t.0);
        errors
            .0
            .extend(verdict_line(verdict, table, &names, &commands));
    }

    /// A battleground join or leave (`0x54b510`): the line at once with the name cached, else on
    /// the name's arrival.
    fn on_player(
        In(ev): In<SessionEvent>,
        mut state: ResMut<Battlefield>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut errors: ResMut<UiErrorKeys>,
    ) {
        let SessionEvent::BattlegroundPlayer { guid, joined } = ev else {
            return;
        };
        match names.full_name(guid, &commands) {
            Some(name) => errors.0.push(player_line(joined, &name)),
            None => state.players.push((guid, joined)),
        }
    }

    /// `SMSG_BATTLEFIELD_PORT_DENIED` (`0x54d4b0`): `ERR_PLAYER_DEAD`, with our player held.
    fn on_port_denied(
        In(ev): In<SessionEvent>,
        me: Query<(), With<SelfPlayer>>,
        mut errors: ResMut<UiErrorKeys>,
    ) {
        if matches!(ev, SessionEvent::BattlefieldPortDenied) && !me.is_empty() {
            errors.0.push(UiError::key("ERR_PLAYER_DEAD"));
        }
    }

    /// The battle-manager notices (`0x54b610`-`0x5499c0`): `SMSG_BATTLEFIELD_MGR_ENTERED`'s third
    /// byte adds `CLEARED_AFK` as a system line first (`0x5498f6`-`0x54992b`), then the queue write
    /// and the event.
    fn on_mgr(
        In(ev): In<SessionEvent>,
        mut world_pvp: ResMut<WorldPvp>,
        mut ui: ResMut<UiEvents>,
        area: Option<Res<benilla_world::terrain_stream::CurrentArea>>,
        areas: Option<Res<crate::area::AreaTableRes>>,
    ) {
        let SessionEvent::BattlefieldMgr(notice) = ev else {
            return;
        };
        let here = area.as_deref().and_then(|a| a.0).unwrap_or(0);
        let parent = areas
            .as_deref()
            .and_then(|a| a.0.get(here))
            .map_or(0, |r| r.zone_id);
        if matches!(
            notice,
            benilla_protocol::messages::BattlefieldMgrNotice::Entered {
                cleared_afk: true,
                ..
            }
        ) {
            ui.system_line("CLEARED_AFK", Vec::new());
        }
        let (event, args) = world_pvp.0.apply(notice, (here, parent));
        ui.fire(event, args);
    }
}

pub(crate) struct BattlefieldPlugin;

impl Plugin for BattlefieldPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Battlefield>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<WorldPvp>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .add_systems(Startup, load_battlemaster_list.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    request_status_on_world_enter
                        .in_set(UiFeed)
                        .after(crate::ui_mail::send_query_next_mail_time_on_enter),
                    // The name callbacks run as the name answers land, in the net drain.
                    announce_named_players
                        .in_set(benilla_world::schedule::WorldStage::Net)
                        .after(crate::net::apply_net_updates),
                    crate::ui_session::close_npc_session_out_of_range::<Battlefield>
                        .after(announce_named_players)
                        .before(drain_battlefield),
                    drain_battlefield.in_set(UiDrain),
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
    fn the_queue_keeps_two_slots() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        q.apply_at(status(0, 1), now);
        q.apply_at(status(1, 1), now);
        q.apply_at(status(2, 1), now);
        assert!(
            q.slots.iter().all(Option::is_some),
            "a third slot is dropped"
        );
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

    /// `0x549534`-`0x54953a`: a slot with no area is free though its status is set; the entered
    /// arm skips a busy slot and still owes the `CLEARED_AFK` line its third byte asks for.
    #[test]
    fn a_slot_with_no_area_is_free_and_entered_skips_a_busy_slot() {
        let mut w = WorldPvpQueue::default();
        w.set(1, 1, 0, 0);
        w.set(2, 1, 4197, 0);
        assert_eq!(
            (w.queue_id, w.area_id),
            (2, 4197),
            "no area: the slot was free"
        );
        let (event, args) = w.apply(
            BattlefieldMgrNotice::Entered {
                queue_id: 3,
                invited: true,
                on_offense: false,
                cleared_afk: true,
            },
            (0, 0),
        );
        assert_eq!((w.queue_id, w.status), (2, 1), "an area and a status: busy");
        assert_eq!((event, args.len()), ("BATTLEFIELD_MGR_ENTERED", 3));
    }
}
