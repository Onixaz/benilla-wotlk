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
    BattlefieldInstance, BattlefieldJoin, BattlefieldListPush, BattlefieldQueueSlot,
    BattlefieldRewardView, BattlegroundType, BattlemasterRow, ScriptValue, UiScript,
    WorldPvpQueueView,
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

/// An arena map's `InstanceType`.
const INSTANCE_TYPE_ARENA: u32 = 4;

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
    /// The active map's `InstanceType` (`0xbea570`), cleared only by the active slot's queue-0 arm (`0x54b17e`) and the world leave.
    instance_type: u32,
    /// The instance's expiration and start time, the second kept as the status's arrival
    /// and the wire's elapsed span so no `Instant` precedes the clock's origin.
    instance_expiration: Option<Instant>,
    run_started: Option<(Instant, Duration)>,
}

impl BattlefieldQueue {
    /// `SMSG_BATTLEFIELD_STATUS` (`0x54ae40`); whether the queue-0 arm cleared the active slot.
    pub(crate) fn apply(&mut self, status: BattlefieldStatus, instance_type: Option<u32>) -> bool {
        self.apply_at(status, instance_type, Instant::now())
    }

    fn apply_at(
        &mut self,
        status: BattlefieldStatus,
        instance_type: Option<u32>,
        now: Instant,
    ) -> bool {
        let index = status.slot as usize;
        if index >= self.slots.len() {
            return false;
        }
        if status.queue == 0 {
            let active = self.active == Some(index);
            if active {
                self.instance_expiration = None;
                self.run_started = None;
                self.active_map = 0;
                self.instance_type = 0;
                self.active = None;
            }
            self.slots[index] = None;
            return active;
        }
        if let Some(a) = status.active {
            self.active_map = a.map_id;
            // A map with no row keeps the type (`0x54afad`).
            if let Some(t) = instance_type {
                self.instance_type = t;
            }
            let ms = |v: u32| Duration::from_millis(u64::from(v));
            self.instance_expiration = (a.shutdown_ms != 0).then(|| now + ms(a.shutdown_ms));
            self.run_started = (a.run_ms != 0).then(|| (now, ms(a.run_ms)));
            self.arena_faction = a.arena_faction;
            self.active = Some(index);
        } else {
            // Any other status zeroes the instance clocks, whichever slot it names (`0x54b0f2`).
            self.instance_expiration = None;
            self.run_started = None;
            if self.active == Some(index) {
                self.active = None;
            }
        }
        self.slots[index] = Some((status, now));
        false
    }

    /// Whether the active map is an arena (`[0xbea570] == 4`).
    pub(crate) fn in_arena(&self) -> bool {
        self.instance_type == INSTANCE_TYPE_ARENA
    }

    /// The world leave (`0x54e330`) zeroes the instance type (`0x54e37d`).
    pub(crate) fn leave_world(&mut self) {
        self.instance_type = 0;
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

    /// The active map id (`0xbea564`), 0 once the queue-0 arm clears it.
    pub(crate) fn active_map_id(&self) -> u32 {
        self.active_map
    }

    /// The active slot's battleground type (`0x54cf75`), 0 without an active slot.
    pub(crate) fn active_bg_type(&self) -> u32 {
        self.active
            .and_then(|i| self.slots[i].as_ref())
            .map_or(0, |(s, _)| {
                benilla_protocol::messages::queue_bg_type(s.queue)
            })
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

    /// The battle-manager notices' writes and each one's event, in handler order (`0x54b610`-`0x5499c0`).
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

/// A world entry's status request, sent once its zero has run after `PLAYER_ENTERING_WORLD`.
#[derive(Resource, Default)]
pub(crate) struct StatusRequestOwed(bool);

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

/// One `BattlemasterList.dbc` row resolved for this player: the bracket for their level, the
/// holiday's world state and their side's map description.
fn battlemaster_row(
    table: &BattlemasterCatalog,
    r: &benilla_formats::BattlemasterRec,
    maps: Option<&MapCatalog>,
    level: u32,
    side: Option<usize>,
    states: Option<&WorldStates>,
) -> BattlemasterRow {
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
        holiday: holiday(r, states),
        description: side.map(|s| {
            map.and_then(|m| maps?.description(m, s))
                .unwrap_or_default()
                .to_string()
        }),
        has_map,
    }
}

/// The row's holiday world state is set.
fn holiday(r: &benilla_formats::BattlemasterRec, states: Option<&WorldStates>) -> bool {
    r.holiday_world_state != 0 && states.is_some_and(|s| s.get(r.holiday_world_state) != 0)
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

/// `AcceptBattlefieldPort`'s refusals (`0x54da10`) in order, skipped on a battleground map; the VM reads fatigue last.
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
    if player.is_some_and(Player::on_transport) {
        return Some("ERR_CLIENT_LOCKED_OUT");
    }
    None
}

/// What the queue's verbs read when called (`0x549ad0`-`0x54e6d0`).
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
    /// One slot at this moment (`0x54bc30`), `None` off the two.
    pub(crate) fn slot(&self, index: usize) -> Option<BattlefieldQueueSlot> {
        if index >= 2 {
            return None;
        }
        let slot = self.queue.as_deref().and_then(|q| q.slots[index].as_ref());
        let active_map = self.queue.as_deref().map_or(0, |q| q.active_map);
        let table = self.table.as_deref().map(|t| &t.0);
        let maps = self.maps.as_deref().map(|m| &m.0);
        Some(slot_view(slot, active_map, table, maps, Instant::now()))
    }

    /// The active instance's globals at this moment.
    pub(crate) fn instance(&self) -> BattlefieldInstance {
        let Some(queue) = self.queue.as_deref() else {
            return BattlefieldInstance::default();
        };
        let now = Instant::now();
        BattlefieldInstance {
            active: queue.active.map(|i| i as u8),
            instance_type: queue.instance_type,
            arena_faction: queue.arena_faction,
            instance_expiration_ms: queue.instance_expiration_ms(now),
            run_time_ms: queue.run_time_ms(now),
        }
    }

    /// The world-PvP slot (`0xbea4a8`) against the UI clock (`0x548700`).
    pub(crate) fn world_pvp(&self) -> WorldPvpQueueView {
        let world_pvp = self.world_pvp.as_deref().map(|w| w.0).unwrap_or_default();
        let ui_now = self.ui_clock.as_deref().map_or_else(
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i32)
            },
            crate::ui_notices::UiClock::now,
        );
        WorldPvpQueueView {
            queue_id: world_pvp.queue_id as i32,
            status: world_pvp.status,
            area_name: self
                .areas
                .as_deref()
                .and_then(|a| a.0.name(world_pvp.area_id))
                .map(str::to_string),
            expire_in: (world_pvp.expire as i32).wrapping_sub(ui_now),
        }
    }

    /// The current area's flags carry `0x8000000` (`0x54c810`).
    pub(crate) fn can_hearth_and_resurrect(&self) -> bool {
        let area_id = self.area.as_deref().and_then(|a| a.0).unwrap_or(0);
        self.areas
            .as_deref()
            .and_then(|a| a.0.get(area_id))
            .is_some_and(|r| r.flags & AREA_FLAG_HEARTH_AND_RESURRECT != 0)
    }

    /// The first port refusal we meet; `0x54a700` tests for a battleground map, type 3.
    pub(crate) fn port_refusal(&self) -> Option<&'static str> {
        let maps = self.maps.as_deref().map(|m| &m.0);
        let on_battleground_map = maps
            .zip(self.map.as_deref())
            .and_then(|(m, id)| m.instance_type(id.0))
            == Some(INSTANCE_TYPE_BATTLEGROUND);
        self.me
            .iter()
            .next()
            .and_then(|s| port_refusal(s, self.player.as_deref(), on_battleground_map))
    }

    /// `BattlemasterList.dbc`'s row `id` for us now, none before the table or our create.
    pub(crate) fn row(&self, id: u32) -> Option<BattlemasterRow> {
        let table = &self.table.as_deref()?.0;
        let store = self.me.iter().next()?;
        Some(battlemaster_row(
            table,
            table.get(id)?,
            self.maps.as_deref().map(|m| &m.0),
            store.0.unit_level().unwrap_or(0),
            side(store),
            self.states.as_deref(),
        ))
    }

    /// The battleground rows the type list sorts (`0x54a650`), none before our create.
    pub(crate) fn types(&self) -> Vec<BattlegroundType> {
        let (Some(table), Some(_)) = (self.table.as_deref(), self.me.iter().next()) else {
            return Vec::new();
        };
        table
            .0
            .rows()
            .iter()
            .filter(|r| r.instance_type == INSTANCE_TYPE_BATTLEGROUND)
            .map(|r| BattlegroundType {
                id: r.id,
                min_level: r.min_level,
                holiday: holiday(r, self.states.as_deref()),
            })
            .collect()
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

/// A join's or leave's line, `name-realm` for another realm (`0x54b55a`).
fn player_line(joined: bool, name: &str) -> UiError {
    if joined {
        UiError::strings("ERR_BG_PLAYER_JOINED_SS", &[name, name])
    } else {
        UiError::s("ERR_BG_PLAYER_LEFT_S", name)
    }
}

/// A cached player's name as the join and leave callbacks read it, never asking (`0x54a569`).
fn cached_full_name(names: &NameCache, guid: u64) -> Option<String> {
    let name = names.peek(guid)?;
    Some(match names.player_realm(guid) {
        Some(realm) => format!("{name}-{realm}"),
        None => name.to_string(),
    })
}

/// The VM's sends (`0x54cc70`, `0x54e5b0`, `0x54da10`, `0x54c250`) and the battlemaster walk-away close (`0x5496d0`).
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
    // The interaction close's battlemaster arm clears it and signals BATTLEFIELDS_CLOSED (`0x51309f` -> `0x5496e1`).
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

/// A cross-map worldport leaves the world first (`0x528c30`).
pub(crate) fn leave_world_on_worldport(
    ports: Option<MessageReader<crate::net::WorldportMessage>>,
    mut commands: Commands,
) {
    if ports.is_some_and(|mut p| p.read().filter(|w| w.needs_ack).count() > 0) {
        commands.queue(leave_world);
    }
}

/// The cascade's status request (`0x54cc40`), owed to the world entry that zeroes ahead of it.
pub(crate) fn request_status_on_world_enter(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    owed: Option<ResMut<StatusRequestOwed>>,
) {
    if cascades.read().next().is_some() {
        if let Some(mut owed) = owed {
            owed.0 = true;
        }
    }
}

/// The battlefield's world entry after `PLAYER_ENTERING_WORLD` (`0x528230` -> `0x54ca50`).
fn enter_world(
    state: Option<ResMut<Battlefield>>,
    queue: Option<ResMut<BattlefieldQueue>>,
    positions: Option<ResMut<crate::ui_battlefield_positions::BattlefieldPositions>>,
    ui: Option<ResMut<UiEvents>>,
) {
    if let Some(mut state) = state {
        state.battlemaster = None;
    }
    if let Some(mut positions) = positions {
        positions.enter_world();
    }
    // Zero both status dwords (`0x54cb57`, `0x54cb5d`).
    if let Some(mut queue) = queue {
        queue.zero_statuses();
    }
    if let Some(mut ui) = ui {
        ui.write(UiScript::reset_battlefield_on_world_enter);
    }
}

/// The world entry's status request (`0x54cc40`), after its zero.
fn send_owed_status_request(mut owed: ResMut<StatusRequestOwed>, commands: Res<NetCommands>) {
    if std::mem::take(&mut owed.0) {
        let _ = commands.0.send(ClientCommand::BattlefieldStatusRequest);
    }
}

/// The world leave's inspect (`0x528cd3`), duel (`0x528d23`) and battlefield (`0x528d46`) parts.
pub(crate) fn leave_world(world: &mut World) {
    crate::ui_inspect::leave_world(world);
    if let Some(mut duel) = world.get_resource_mut::<crate::ui_duel::DuelState>() {
        duel.leave_world();
    }
    if let Some(mut queue) = world.get_resource_mut::<BattlefieldQueue>() {
        queue.leave_world();
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.write(UiScript::leave_battlefield_world);
    }
    crate::ui_script::live::fire_handler_events(world);
}

/// Queue the battlefield's world entry (`0x54ca50`) for when the calling system's commands apply.
pub(crate) fn queue_enter_world(commands: &mut Commands) {
    commands.queue(|world: &mut World| {
        if let Err(e) = world.run_system_cached(enter_world) {
            warn!("ui_battlefield: the world entry did not run: {e}");
        }
        crate::ui_battlefield_score::enter_world(world);
        crate::ui_arena::enter_world(world);
        if world.contains_resource::<NetCommands>()
            && world.contains_resource::<StatusRequestOwed>()
        {
            if let Err(e) = world.run_system_cached(send_owed_status_request) {
                warn!("ui_battlefield: the status request did not go out: {e}");
            }
        }
        // The UI init clears the reload byte once its world entry returns (`0x52ae91`).
        if let Some(mut reload) = world.get_resource_mut::<crate::ui_script::ReloadEntry>() {
            reload.0 = false;
        }
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
        cached_full_name, player_line, reward, verdict_line, Battlefield, BattlefieldListPush,
        BattlefieldQueue, BattlemasterTable, UiScript, WorldPvp,
    };
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp, SelfPlayer};
    use crate::ui_action::{show_in_turn, UiError};
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldStatus, on_status)
            .net_handler(K::BattlefieldList, on_list)
            .net_handler(K::GroupJoinedBattleground, on_verdict)
            .net_handler(K::BattlegroundPlayer, on_player)
            .net_handler(K::PlayerName, on_name_answer)
            .net_handler(K::PlayerNameMiss, on_name_answer)
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

    /// `SMSG_BATTLEFIELD_STATUS` (`0x54ae40`): the slot, on status 3 the score rebuild (`0x54b0d6`), then `UPDATE_BATTLEFIELD_STATUS` (`0x54b1a9`).
    pub(super) fn on_status(
        In(ev): In<SessionEvent>,
        mut queue: ResMut<BattlefieldQueue>,
        mut positions: ResMut<crate::ui_battlefield_positions::BattlefieldPositions>,
        mut ui: ResMut<UiEvents>,
        mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
        catalog: Option<Res<crate::world_state_ui::WorldStateUiRes>>,
        maps: Option<Res<benilla_assets::MapCatalogRes>>,
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
        let instance_type = status
            .active
            .and_then(|a| maps.as_deref()?.0.instance_type(a.map_id));
        // The active slot's queue-0 arm zeroes the positions and score columns (`0x54b165`, `0x54b174`).
        if queue.apply(status, instance_type) {
            positions.clear_players();
            ui.write(UiScript::clear_battlefield_score_columns);
        }
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

    /// `SMSG_BATTLEFIELD_LIST` (`0x54e390`): written into the VM in turn, then its source's event.
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

    /// `SMSG_GROUP_JOINED_BATTLEGROUND` (`0x54b1c0`): its line, shown in the handler's turn.
    fn on_verdict(
        In(ev): In<SessionEvent>,
        table: Option<Res<BattlemasterTable>>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::GroupJoinedBattleground(verdict) = ev else {
            return;
        };
        let table = table.as_deref().map(|t| &t.0);
        if let Some(line) = verdict_line(verdict, table, &names, &commands) {
            show_in_turn(&mut ui, "ui_battlefield", line);
        }
    }

    /// A join or leave (`0x54b510`): the line now with the name cached (`0x54b5a9`), else from its name callback.
    pub(super) fn on_player(
        In(ev): In<SessionEvent>,
        mut state: ResMut<Battlefield>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::BattlegroundPlayer { guid, joined } = ev else {
            return;
        };
        match names.full_name(guid, &commands) {
            Some(name) => show_in_turn(&mut ui, "ui_battlefield", player_line(joined, &name)),
            None => state.players.push((guid, joined)),
        }
    }

    /// A name answer runs the join and leave callbacks filed for it (`0x54a530`, `0x54a5c0`); an unnamed answer shows none (`0x54a567`).
    pub(super) fn on_name_answer(
        In(ev): In<SessionEvent>,
        mut state: ResMut<Battlefield>,
        names: Res<NameCache>,
        mut ui: ResMut<UiEvents>,
    ) {
        use benilla_protocol::messages::NameQueryMiss;
        let guid = match ev {
            SessionEvent::PlayerName { guid, .. } => guid,
            SessionEvent::PlayerNameMiss {
                miss: NameQueryMiss::Retry,
                ..
            } => return,
            SessionEvent::PlayerNameMiss { guid, .. } => guid,
            _ => return,
        };
        if !state.players.iter().any(|&(g, _)| g == guid) {
            return;
        }
        let name = cached_full_name(&names, guid);
        state.players.retain(|&(g, joined)| {
            if g != guid {
                return true;
            }
            if let Some(name) = &name {
                show_in_turn(&mut ui, "ui_battlefield", player_line(joined, name));
            }
            false
        });
    }

    /// `SMSG_BATTLEFIELD_PORT_DENIED` (`0x54d4b0`): `ERR_PLAYER_DEAD`, with our player held.
    fn on_port_denied(
        In(ev): In<SessionEvent>,
        me: Query<(), With<SelfPlayer>>,
        mut ui: ResMut<UiEvents>,
    ) {
        if matches!(ev, SessionEvent::BattlefieldPortDenied) && !me.is_empty() {
            show_in_turn(&mut ui, "ui_battlefield", UiError::key("ERR_PLAYER_DEAD"));
        }
    }

    /// The battle-manager notices (`0x54b610`): the entry's `CLEARED_AFK` line first (`0x5498f6`), then the write and the event.
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
            .init_resource::<StatusRequestOwed>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<WorldPvp>()
            .add_systems(Startup, load_battlemaster_list.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    leave_world_on_worldport
                        .in_set(benilla_world::schedule::WorldStage::Net)
                        .after(crate::net::enter_world_on_self_create),
                    // Owed ahead of the world entry, which sends it after the mail query's.
                    request_status_on_world_enter
                        .in_set(UiFeed)
                        .after(crate::ui_mail::send_query_next_mail_time_on_enter)
                        .before(crate::ui_unit::EnterWorldFeed),
                    crate::ui_session::close_npc_session_out_of_range::<Battlefield>
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
        q.apply_at(status(0, 1), None, now);
        q.apply_at(status(1, 1), None, now);
        q.apply_at(status(2, 1), None, now);
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
            None,
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
        q.apply_at(active, None, now);
        assert_eq!(q.active_map(), Some(489));
        assert_eq!(q.active_queue(), WSG);
        let later = now + Duration::from_secs(10);
        assert_eq!(q.run_time_ms(later), 40_000);
        assert_eq!(q.instance_expiration_ms(later), 110_000);

        // A queued status on the other slot zeroes the clocks and keeps the active index.
        q.apply_at(status(1, 1), None, later);
        assert_eq!(q.run_time_ms(later), 0);
        assert_eq!(q.active_map(), Some(489));

        let mut confirm = status(1, 2);
        confirm.invite = Some(BattlefieldInvite {
            map_id: 529,
            joined_queue: 0,
            expire_ms: 80_000,
        });
        q.apply_at(confirm.clone(), None, later);
        let v = slot_view(q.slots[1].as_ref(), q.active_map, None, None, later);
        assert_eq!((v.status, v.port_expiration_ms), (2, 80_000));

        q.apply_at(
            BattlefieldStatus {
                slot: 0,
                ..Default::default()
            },
            None,
            later,
        );
        assert_eq!(
            q.active_map(),
            None,
            "clearing the active slot drops the instance"
        );
    }

    /// The instance type outlives the active index; only the queue-0 arm (`0x54b17e`) and the world leave (`0x54e37d`) clear it.
    #[test]
    fn the_arena_type_outlives_the_active_index() {
        let mut q = BattlefieldQueue::default();
        let now = Instant::now();
        let mut active = status(0, 3);
        active.active = Some(BattlefieldActive {
            map_id: 559,
            joined_queue: 0,
            shutdown_ms: 0,
            run_ms: 0,
            arena_faction: 0,
        });
        assert!(!q.apply_at(active.clone(), Some(4), now));
        assert!(q.in_arena());
        assert!(!q.apply_at(status(0, 1), None, now), "the slot re-queued");
        assert_eq!(q.active_map(), None, "the active index dropped");
        assert!(q.in_arena(), "the type stays");
        q.leave_world();
        assert!(!q.in_arena());
        q.apply_at(active, Some(4), now);
        let zero = BattlefieldStatus {
            slot: 0,
            ..Default::default()
        };
        assert!(
            q.apply_at(zero.clone(), None, now),
            "the active slot's queue-0 arm"
        );
        assert!(!q.in_arena());
        assert!(!q.apply_at(zero, None, now), "no active slot left to clear");
    }

    /// The cascade only owes the request; the world entry zeroes, then sends it (`0x54cb57`, `0x54cb5d`, `0x54cc40`).
    #[test]
    fn the_world_entry_zeroes_then_sends_the_owed_request() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<BattlefieldQueue>()
            .init_resource::<Battlefield>()
            .init_resource::<StatusRequestOwed>()
            .add_message::<WorldEnterCascadeMessage>()
            .add_systems(Update, request_status_on_world_enter);
        let now = Instant::now();
        app.world_mut()
            .resource_mut::<BattlefieldQueue>()
            .apply_at(status(0, 1), None, now);
        app.world_mut().write_message(WorldEnterCascadeMessage);
        app.update();
        let status_now = |app: &App| {
            app.world().resource::<BattlefieldQueue>().slots()[0]
                .as_ref()
                .map(|(s, _)| s.status)
        };
        assert_eq!(rx.try_iter().count(), 0, "the cascade sends nothing");
        assert_eq!(status_now(&app), Some(1), "held until the world entry");

        let entry = |app: &mut App| {
            app.world_mut().run_system_cached(enter_world).unwrap();
            app.world_mut()
                .run_system_cached(send_owed_status_request)
                .unwrap();
        };
        entry(&mut app);
        assert!(matches!(
            rx.try_iter().collect::<Vec<_>>().as_slice(),
            [ClientCommand::BattlefieldStatusRequest]
        ));
        assert_eq!(status_now(&app), Some(0), "zeroed ahead of the request");

        // An answer lands; a world entry with no cascade owes nothing.
        app.world_mut()
            .resource_mut::<BattlefieldQueue>()
            .apply_at(status(0, 1), None, now);
        app.world_mut()
            .run_system_cached(send_owed_status_request)
            .unwrap();
        assert_eq!(rx.try_iter().count(), 0);
        assert_eq!(status_now(&app), Some(1));
    }

    /// A join waits on its name; the answer shows each line filed for it in its turn (`0x54a530`, `0x54a5c0`), a retry keeps them, a denial drops them silently (`0x54a567`).
    #[test]
    fn a_name_answer_runs_the_join_and_leave_callbacks_filed_for_it() {
        use benilla_protocol::messages::NameQueryMiss;
        use benilla_protocol::SessionEvent;
        use bevy::ecs::system::RunSystemOnce;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut w = World::new();
        w.insert_resource(NetCommands(tx));
        w.init_resource::<Battlefield>();
        w.init_resource::<NameCache>();
        w.init_resource::<UiEvents>();
        let player = |guid, joined| SessionEvent::BattlegroundPlayer { guid, joined };
        for ev in [player(0x10, true), player(0x10, false), player(0x11, true)] {
            w.run_system_once_with(net::on_player, ev).unwrap();
        }
        assert!(w.resource::<UiEvents>().is_empty(), "no name: no line yet");
        assert_eq!(w.resource::<Battlefield>().players.len(), 3);

        let retry = SessionEvent::PlayerNameMiss {
            guid: 0x10,
            miss: NameQueryMiss::Retry,
        };
        w.run_system_once_with(net::on_name_answer, retry).unwrap();
        assert_eq!(
            w.resource::<Battlefield>().players.len(),
            3,
            "a retry keeps them"
        );

        w.resource_mut::<NameCache>()
            .insert_player(0x10, "Brisca".into(), None);
        let named = SessionEvent::PlayerName {
            guid: 0x10,
            name: "Brisca".into(),
            realm: String::new(),
            race: 1,
            class: 1,
            gender: 0,
            declensions: None,
        };
        w.run_system_once_with(net::on_name_answer, named).unwrap();
        assert_eq!(w.resource::<UiEvents>().names(), ["<world>", "<world>"]);
        assert_eq!(w.resource::<Battlefield>().players, [(0x11, true)]);

        *w.resource_mut::<UiEvents>() = UiEvents::default();
        let denied = SessionEvent::PlayerNameMiss {
            guid: 0x11,
            miss: NameQueryMiss::Deny,
        };
        w.run_system_once_with(net::on_name_answer, denied).unwrap();
        assert!(w.resource::<UiEvents>().is_empty(), "no record: no line");
        assert!(w.resource::<Battlefield>().players.is_empty());
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
