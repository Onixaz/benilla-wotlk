//! The queue half of `CGBattlefieldInfo` (bindings `0x549ad0`-`0x54e6d0`): the battleground and
//! arena lists `PVPBattlegroundFrame.lua`, `BattlefieldFrame.lua` and `ArenaFrame.lua` read, the
//! two queue slots behind the minimap icon, and the Wintergrasp queue. The VM owns what the
//! bindings write (the listed type, the instances, the selection, the battlemaster); the app
//! pushes `BattlemasterList.dbc` resolved for the player, each list packet as it lands and the
//! slots every frame, since their clocks move.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg};
use super::Model;

/// `BattlemasterList.dbc`'s random battleground (`[rec] == 0x20`), which sorts first, enters at
/// level 80 alone and answers `isRandom`.
pub const RANDOM_BATTLEGROUND: u32 = 32;

/// `InstanceType` of an arena row; a battleground row is 3.
pub const INSTANCE_TYPE_ARENA: u32 = 4;
const INSTANCE_TYPE_BATTLEGROUND: u32 = 3;

/// One `BattlemasterList.dbc` row resolved for the player by the app.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlemasterRow {
    pub id: u32,
    pub name: String,
    /// `InstanceType`: 3 a battleground, 4 an arena.
    pub instance_type: u32,
    pub max_group_size: i32,
    pub groups_allowed: bool,
    pub min_level: i32,
    /// `GetBracketForLevel` (`0x54ba00`): the player's level is in one of the first map's
    /// `PvpDifficulty.dbc` brackets, or 80 for the random battleground.
    pub can_enter: bool,
    /// The row's holiday world state is set.
    pub holiday: bool,
    /// The first map's description for the player's side; `None` with no side, where
    /// `GetBattlefieldInfo` pushes nothing in its place (`0x54d7f1`).
    pub description: Option<String>,
    /// The first map has a `Map.dbc` row; `GetBattlefieldInfo` answers nothing without one.
    pub has_map: bool,
}

/// A battleground's reward pair, as the list packet carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldRewardView {
    pub has_win: bool,
    pub win_honor: u32,
    pub win_arena: u32,
    pub loss_honor: u32,
}

/// `SMSG_BATTLEFIELD_LIST` as the app hands it on (`0x54e390`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldListPush {
    pub battlemaster: u64,
    pub from_anywhere: bool,
    pub bg_type: u32,
    pub holiday: BattlefieldRewardView,
    pub random: BattlefieldRewardView,
    pub instances: Vec<u32>,
}

/// One queue slot (`m_status[i]`, `0xbea4b8`) reduced by the app at this frame's clock.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldQueueSlot {
    /// 0 none, 1 queued, 2 confirm, 3 active; anything else answers `"error"`.
    pub status: u32,
    /// `GetBattlefieldStatus`' name: the map's (`0x54bcc0`), else the battleground type's, else
    /// `None`, a real nil.
    pub name: Option<String>,
    /// The queue word's battleground type.
    pub bg_type: u32,
    pub instance_id: u32,
    pub min_level: u32,
    pub max_level: u32,
    /// The arena team size, 0 for a battleground.
    pub team_size: u32,
    pub rated: bool,
    /// `deadline − now` in ms, 0 when unset or past.
    pub port_expiration_ms: u32,
    pub estimated_wait_ms: u32,
    /// `now − stamp` in ms, 0 when unset.
    pub time_waited_ms: u32,
}

/// The one world-PvP queue (`m_worldPvPQueues`, `0xbea4a8`) as the app resolved it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorldPvpQueueView {
    pub queue_id: i32,
    /// 0 none, 1 queued, 2 confirm, 3 active.
    pub status: u32,
    pub area_name: Option<String>,
    /// Status 2's `expire − GetUITime()`, seconds; 0 otherwise.
    pub expire_in: i32,
}

/// What the app pushes every frame: the clocks, the active instance and the gates.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldFrame {
    pub slots: [BattlefieldQueueSlot; 2],
    /// The slot whose instance the player is in (`m_activeIndex`, `0xacd16c`).
    pub active: Option<u8>,
    /// The active map's `InstanceType` (`m_instanceType`, `0xbea570`): 4 inside an arena.
    pub instance_type: u32,
    /// `m_arenaFaction` (`0xbea54a`).
    pub arena_faction: u8,
    /// `GetBattlefieldInstanceExpiration()`, ms.
    pub instance_expiration_ms: u32,
    /// `GetBattlefieldInstanceRunTime()`, ms.
    pub run_time_ms: u32,
    pub world_pvp: WorldPvpQueueView,
    /// The current area's `AreaTable.dbc` flags carry `0x8000000` (`0x54c810`).
    pub can_hearth_and_resurrect: bool,
    /// The first of `AcceptBattlefieldPort`'s refusals the player now meets (`0x54da10`), the
    /// GlobalStrings key it shows; `None` on an arena map, which skips them.
    pub port_refusal: Option<&'static str>,
}

/// A `JoinBattlefield` that passed its gates, resolved to the opcode it sends (`0x54cc70`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BattlefieldJoin {
    Battleground {
        battlemaster: u64,
        bg_type: u32,
        instance_id: u32,
        as_group: bool,
    },
    Arena {
        battlemaster: u64,
        slot: u8,
        as_group: bool,
        rated: bool,
    },
}

/// A battle-manager (Wintergrasp) send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BattlefieldMgrRequest {
    EntryInviteResponse { queue_id: i32, accept: bool },
    QueueInviteResponse { queue_id: i32, accept: bool },
    QueueRequest { queue_id: i32 },
    ExitRequest { queue_id: i32 },
}

/// The VM's `CGBattlefieldInfo` mirror.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BattlefieldInfo {
    /// Every `BattlemasterList.dbc` row, as pushed.
    rows: Vec<BattlemasterRow>,
    /// `m_battlegroundTypes` (`0xbea720`): the battleground rows' ids in their sorted order.
    order: Vec<u32>,
    /// `m_bgTypeID` (`0xbea53c`), the type the list and the join name.
    bg_type: u32,
    /// `m_instances` (`0xbea5bc`).
    instances: Vec<u32>,
    /// `m_battlemaster` (`0xbea568`), 0 for the PvP window's list.
    battlemaster: u64,
    /// `m_selectedInstance` (`0xbea538`), an instance id.
    selected: u32,
    /// `m_allowJoin` (`0xacd174`).
    allow_join: bool,
    holiday: BattlefieldRewardView,
    random: BattlefieldRewardView,
    frame: BattlefieldFrame,
    joins: Vec<BattlefieldJoin>,
    /// `RequestBattlegroundInstanceInfo`'s sends: the type asked.
    list_requests: Vec<u32>,
    /// `(slot, accept)`, 0-based.
    port_requests: Vec<(u8, bool)>,
    /// `CloseBattlefield()` calls: the app drops the battlemaster's interaction.
    closes: u32,
    leaves: u32,
    hearth_requests: u32,
    mgr_requests: Vec<BattlefieldMgrRequest>,
}

impl Default for BattlefieldInfo {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            order: Vec::new(),
            bg_type: 0,
            instances: Vec::new(),
            battlemaster: 0,
            selected: 0,
            allow_join: true,
            holiday: Default::default(),
            random: Default::default(),
            frame: Default::default(),
            joins: Vec::new(),
            list_requests: Vec::new(),
            port_requests: Vec::new(),
            closes: 0,
            leaves: 0,
            hearth_requests: 0,
            mgr_requests: Vec::new(),
        }
    }
}

impl BattlefieldInfo {
    fn row(&self, id: u32) -> Option<&BattlemasterRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// `QSortBattlefields` (`0x54a650`) as a key: the random battleground first, then a running
    /// holiday, then the lower minimum level, then the lower id.
    fn sort_order(&mut self) {
        let rows = &self.rows;
        self.order.sort_by_key(|&id| {
            let r = rows.iter().find(|r| r.id == id);
            (
                id != RANDOM_BATTLEGROUND,
                !r.is_some_and(|r| r.holiday),
                r.map_or(0, |r| r.min_level),
                id,
            )
        });
    }

    /// The listed type's row, the gate most bindings open with.
    fn listed(&self) -> Option<&BattlemasterRow> {
        self.row(self.bg_type)
    }
}

/// The reference's 1-based index, `(u32)tonumber − 1`: 0 and negatives wrap past any bound.
fn index0(n: i32) -> u32 {
    (n as u32).wrapping_sub(1)
}

/// The reference's five-entry status table with its `"error"` default (`0x54bc70`).
fn status_text(status: u32) -> &'static str {
    match status {
        0 => "none",
        1 => "queued",
        2 => "confirm",
        3 => "active",
        _ => "error",
    }
}

/// The active player's gate (`ClntObjMgrObjectPtr(active, TYPEMASK_PLAYER)`).
fn player_exists(model: &Model) -> bool {
    model.unit("player").is_some_and(|u| u.exists)
}

fn n(v: u32) -> Value {
    Value::Number(f64::from(v))
}

fn opt_string(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

fn event(model: &mut Model, name: &str) {
    model.pending_events.push((name.to_string(), Vec::new()));
}

impl super::UiScript {
    /// Push `BattlemasterList.dbc` resolved for the player. The first push builds the type list
    /// as `InitializeGame` does (`0x54e720`), before any holiday is known; `SortBGList` re-sorts.
    pub fn set_battlemaster_rows(&mut self, rows: Vec<BattlemasterRow>) {
        if self.model_ref().battlefield.rows == rows {
            return;
        }
        let mut model = self.model_mut();
        let bf = &mut model.battlefield;
        let first = bf.rows.is_empty();
        bf.rows = rows;
        if first {
            bf.order = bf
                .rows
                .iter()
                .filter(|r| r.instance_type == INSTANCE_TYPE_BATTLEGROUND)
                .map(|r| r.id)
                .collect();
            let holidays: Vec<bool> = bf.rows.iter().map(|r| r.holiday).collect();
            for r in &mut bf.rows {
                r.holiday = false;
            }
            bf.sort_order();
            for (r, h) in bf.rows.iter_mut().zip(holidays) {
                r.holiday = h;
            }
        }
    }

    /// `SMSG_BATTLEFIELD_LIST` (`0x54e390`): the type, rewards and instances, then the event its
    /// source names. A battlemaster opens the session (`SetBattlemaster`, `0x5496d0`).
    pub fn apply_battlefield_list(&mut self, list: BattlefieldListPush) {
        let mut model = self.model_mut();
        let bf = &mut model.battlefield;
        bf.bg_type = list.bg_type;
        bf.holiday = list.holiday;
        bf.random = list.random;
        bf.instances = list.instances;
        bf.allow_join = true;
        if list.bg_type == RANDOM_BATTLEGROUND && list.battlemaster != 0 {
            bf.battlemaster = list.battlemaster;
            event(&mut model, "NPC_PVPQUEUE_ANYWHERE");
        } else if list.from_anywhere {
            event(&mut model, "PVPQUEUE_ANYWHERE_SHOW");
        } else if list.battlemaster != 0 {
            bf.battlemaster = list.battlemaster;
            event(&mut model, "BATTLEFIELDS_SHOW");
        } else {
            bf.battlemaster = 0;
            event(&mut model, "BATTLEFIELDS_CLOSED");
            event(&mut model, "BATTLEFIELDS_SHOW");
        }
    }

    /// The battlemaster walked out of range or the session ended: `SetBattlemaster(0)`.
    pub fn close_battlemaster(&mut self) {
        let mut model = self.model_mut();
        if model.battlefield.battlemaster == 0 {
            return;
        }
        model.battlefield.battlemaster = 0;
        event(&mut model, "BATTLEFIELDS_CLOSED");
    }

    /// The battlemaster the VM holds, 0 with none.
    pub fn battlemaster(&self) -> u64 {
        self.model_ref().battlefield.battlemaster
    }

    /// The world-enter reset (`EnterWorld`, `0x54ca50`): the selection, type and battlemaster.
    pub fn reset_battlefield_on_world_enter(&mut self) {
        let mut model = self.model_mut();
        let bf = &mut model.battlefield;
        bf.selected = 0;
        bf.bg_type = 0;
        bf.battlemaster = 0;
        bf.allow_join = true;
        bf.instances.clear();
    }

    /// Push the slots and clocks, every frame.
    pub fn set_battlefield_frame(&mut self, frame: BattlefieldFrame) {
        if self.model_ref().battlefield.frame == frame {
            return;
        }
        self.model_mut().battlefield.frame = frame;
    }

    pub fn take_battlefield_joins(&mut self) -> Vec<BattlefieldJoin> {
        drain_field!(self, battlefield.joins)
    }

    /// `RequestBattlegroundInstanceInfo` sends: the battleground type for `CMSG_BATTLEFIELD_LIST`.
    pub fn take_battlefield_list_requests(&mut self) -> Vec<u32> {
        drain_field!(self, battlefield.list_requests)
    }

    /// `AcceptBattlefieldPort` sends: `(slot, accept)`, 0-based.
    pub fn take_battlefield_port_requests(&mut self) -> Vec<(u8, bool)> {
        drain_field!(self, battlefield.port_requests)
    }

    /// `CloseBattlefield()` calls since the last drain.
    pub fn take_battlefield_closes(&mut self) -> u32 {
        drain_field!(self, battlefield.closes)
    }

    /// `LeaveBattlefield()` calls since the last drain.
    pub fn take_battlefield_leave_requests(&mut self) -> u32 {
        drain_field!(self, battlefield.leaves)
    }

    /// `HearthAndResurrectFromArea()` calls since the last drain.
    pub fn take_hearth_and_resurrect_requests(&mut self) -> u32 {
        drain_field!(self, battlefield.hearth_requests)
    }

    pub fn take_battlefield_mgr_requests(&mut self) -> Vec<BattlefieldMgrRequest> {
        drain_field!(self, battlefield.mgr_requests)
    }
}

/// The reward getters' five values (`0x54a300`, `0x54a370`): a boolean, then four numbers.
fn reward(r: &BattlefieldRewardView) -> MultiValue {
    MultiValue::from_vec(vec![
        Value::Boolean(r.has_win),
        n(r.win_honor),
        n(r.win_arena),
        n(r.loss_honor),
        Value::Number(0.0),
    ])
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumBattlefields",
        lua.create_function(|lua, ()| Ok(read(lua).battlefield.instances.len() as i64))?,
    )?;

    // Six values (`0x54d770`): name, the side's description, max group, canEnter, isHoliday,
    // isRandom; nothing without a player, a listed row or its map.
    g.set(
        "GetBattlefieldInfo",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let Some(r) = model
                .battlefield
                .listed()
                .filter(|r| r.has_map && player_exists(&model))
            else {
                return Ok(MultiValue::new());
            };
            let mut out = vec![Value::String(lua.create_string(&r.name)?)];
            if let Some(d) = &r.description {
                out.push(Value::String(lua.create_string(d)?));
            }
            out.push(Value::Number(f64::from(r.max_group_size)));
            out.push(flag(r.can_enter));
            out.push(flag(r.holiday));
            out.push(flag(r.id == RANDOM_BATTLEGROUND));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The reference raises `GetBattlefieldInfo`'s usage, its own mislabel (`0x54d900`).
    g.set(
        "GetBattlefieldInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(lua, index, "Usage: GetBattlefieldInfo(index)")?);
            let model = read(lua);
            if !player_exists(&model) {
                return Ok(MultiValue::new());
            }
            Ok(match model.battlefield.instances.get(index as usize) {
                Some(&id) => MultiValue::from_vec(vec![Value::Number(f64::from(id as i32))]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    g.set(
        "IsBattlefieldArena",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(flag(
                model
                    .battlefield
                    .listed()
                    .is_some_and(|r| r.instance_type == INSTANCE_TYPE_ARENA),
            ))
        })?,
    )?;

    // Two values (`0x549ad0`): inside an arena, and the active slot's rated flag.
    g.set(
        "IsActiveBattlefieldArena",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let f = &model.battlefield.frame;
            let rated = f
                .active
                .and_then(|i| f.slots.get(usize::from(i)))
                .is_some_and(|s| s.rated);
            Ok(MultiValue::from_vec(vec![
                flag(f.instance_type == INSTANCE_TYPE_ARENA),
                flag(rated),
            ]))
        })?,
    )?;

    g.set(
        "CanJoinBattlefieldAsGroup",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(flag(
                model.battlefield.listed().is_some_and(|r| r.groups_allowed),
            ))
        })?,
    )?;

    // JoinBattlefield(index [, asGroup [, isRated]]) (`0x54d990` into `0x54cc70`). An arena row
    // takes the index as its bracket slot; a battleground's names an instance, 0 off the list or
    // with no battlemaster.
    g.set(
        "JoinBattlefield",
        lua.create_function(
            |lua, (index, as_group, rated): (Value, Option<Value>, Option<Value>)| {
                let index = index0(number_arg(lua, index, "Usage: JoinBattlefield(index)")?);
                let as_group = bool_or_default(as_group.as_ref(), false);
                let rated = bool_or_default(rated.as_ref(), false);
                let mut model = crate::script::model_mut_of(lua);
                if !model.battlefield.allow_join {
                    return Ok(());
                }
                let row = model.battlefield.listed().cloned();
                let arena = row
                    .as_ref()
                    .is_some_and(|r| r.instance_type == INSTANCE_TYPE_ARENA);
                if let Some(r) = row.as_ref().filter(|_| as_group) {
                    let party = model.party.members.len() as i32;
                    let raid = model.party.raid.len() as i32;
                    if r.max_group_size < party || r.max_group_size < raid {
                        model.ui_errors.push("ERR_GROUP_JOIN_BATTLEGROUND_TOO_MANY");
                        return Ok(());
                    }
                }
                if !arena && model.instance_type == Some(INSTANCE_TYPE_BATTLEGROUND) {
                    model.ui_errors.push("ERR_BATTLEGROUND_NOT_IN_BATTLEGROUND");
                    return Ok(());
                }
                let bf = &model.battlefield;
                let join = if arena {
                    BattlefieldJoin::Arena {
                        battlemaster: bf.battlemaster,
                        slot: index as u8,
                        as_group,
                        rated,
                    }
                } else {
                    let instance_id = bf
                        .instances
                        .get(index as usize)
                        .filter(|_| bf.battlemaster != 0)
                        .copied()
                        .unwrap_or(0);
                    BattlefieldJoin::Battleground {
                        battlemaster: bf.battlemaster,
                        bg_type: bf.bg_type,
                        instance_id,
                        as_group,
                    }
                };
                model.battlefield.joins.push(join);
                Ok(())
            },
        )?,
    )?;

    // Fires `BATTLEFIELDS_CLOSED` and drops the battlemaster (`0x549b40`).
    g.set(
        "CloseBattlefield",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            event(&mut model, "BATTLEFIELDS_CLOSED");
            model.battlefield.battlemaster = 0;
            model.battlefield.closes += 1;
            Ok(())
        })?,
    )?;

    // Stores the instance id at that position, 0 off the list (`0x54bb40`).
    g.set(
        "SetSelectedBattlefield",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: SetSelectedBattlefield(index)",
            )?);
            let mut model = crate::script::model_mut_of(lua);
            let bf = &mut model.battlefield;
            bf.selected = bf.instances.get(index as usize).copied().unwrap_or(0);
            Ok(())
        })?,
    )?;

    // The stored id's 1-based position; 0 when the list no longer holds it (`0x54bbd0`).
    g.set(
        "GetSelectedBattlefield",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let bf = &model.battlefield;
            Ok(bf
                .instances
                .iter()
                .position(|&id| id == bf.selected)
                .map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // Seven values on every leg (`0x54bc30`): `nil, nil, 0, 0, 0, 0, nil` off the two slots.
    g.set(
        "GetBattlefieldStatus",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: GetBattlefieldStatus(index)",
            )?);
            let model = read(lua);
            let Some(s) = model.battlefield.frame.slots.get(index as usize) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    n(0),
                    n(0),
                    n(0),
                    n(0),
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(status_text(s.status))?),
                opt_string(lua, s.name.as_deref())?,
                n(s.instance_id),
                n(s.min_level),
                n(s.max_level),
                n(s.team_size),
                flag(s.rated),
            ]))
        })?,
    )?;

    // Whole seconds left to accept (`0x549b80`, `/ 1000` in integers); 0 off the slots.
    g.set(
        "GetBattlefieldPortExpiration",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: GetBattlefieldPortExpiration(index)",
            )?);
            let model = read(lua);
            Ok(model
                .battlefield
                .frame
                .slots
                .get(index as usize)
                .map_or(0, |s| i64::from(s.port_expiration_ms / 1000)))
        })?,
    )?;

    for (name, usage, get) in [
        (
            "GetBattlefieldEstimatedWaitTime",
            "Usage: GetBattlefieldEstimatedWaitTime(index)",
            (|s: &BattlefieldQueueSlot| s.estimated_wait_ms) as fn(&BattlefieldQueueSlot) -> u32,
        ),
        (
            "GetBattlefieldTimeWaited",
            "Usage: GetBattlefieldTimeWaited(index)",
            |s: &BattlefieldQueueSlot| s.time_waited_ms,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let index = index0(number_arg(lua, index, usage)?);
                let model = read(lua);
                Ok(model
                    .battlefield
                    .frame
                    .slots
                    .get(index as usize)
                    .map_or(0, |s| i64::from(get(s))))
            })?,
        )?;
    }

    g.set(
        "GetBattlefieldInstanceExpiration",
        lua.create_function(|lua, ()| {
            Ok(i64::from(
                read(lua).battlefield.frame.instance_expiration_ms,
            ))
        })?,
    )?;

    g.set(
        "GetBattlefieldInstanceRunTime",
        lua.create_function(|lua, ()| Ok(i64::from(read(lua).battlefield.frame.run_time_ms)))?,
    )?;

    g.set(
        "GetBattlefieldArenaFaction",
        lua.create_function(|lua, ()| Ok(flag(read(lua).battlefield.frame.arena_faction != 0)))?,
    )?;

    // AcceptBattlefieldPort(index, accept) (`0x54da10`): nil off the two slots or on a refusal,
    // else 1; a decline of the listed type also fires `BATTLEFIELDS_CLOSED`.
    g.set(
        "AcceptBattlefieldPort",
        lua.create_function(|lua, (index, accept): (Value, Option<Value>)| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: AcceptBattlefieldPort(index, accept)",
            )?);
            let mut model = crate::script::model_mut_of(lua);
            let Some(slot) = model.battlefield.frame.slots.get(index as usize).cloned() else {
                return Ok(Value::Nil);
            };
            let accept = bool_or_default(accept.as_ref(), false);
            if accept {
                if let Some(key) = model.battlefield.frame.port_refusal {
                    model.ui_errors.push(key);
                    return Ok(Value::Nil);
                }
            }
            model.battlefield.port_requests.push((index as u8, accept));
            if !accept && slot.bg_type == model.battlefield.bg_type {
                event(&mut model, "BATTLEFIELDS_CLOSED");
            }
            Ok(Value::Integer(1))
        })?,
    )?;

    // Sends the active slot's queue word, 0 with none (`0x54c250`).
    g.set(
        "LeaveBattlefield",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).battlefield.leaves += 1;
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBattlegroundTypes",
        lua.create_function(|lua, ()| Ok(read(lua).battlefield.order.len() as i64))?,
    )?;

    // Five values (`0x54e010`): name, canEnter, isHoliday, isRandom, id; nothing off the list.
    g.set(
        "GetBattlegroundInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlegroundInfo(index)")?;
            let model = read(lua);
            let bf = &model.battlefield;
            let row = usize::try_from(index - 1)
                .ok()
                .and_then(|i| bf.order.get(i))
                .and_then(|&id| bf.row(id))
                .filter(|_| player_exists(&model));
            let Some(r) = row else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                flag(r.can_enter),
                flag(r.holiday),
                flag(r.id == RANDOM_BATTLEGROUND),
                n(r.id),
            ]))
        })?,
    )?;

    // Re-sorts the type list with today's holidays and answers true (`0x54c9f0`).
    g.set(
        "SortBGList",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).battlefield.sort_order();
            Ok(true)
        })?,
    )?;

    // Lists the type at that position (`0x54e5b0`): the type switches and the instances clear
    // at once; the list packet refills them.
    g.set(
        "RequestBattlegroundInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: RequestBattlegroundInstanceInfo(index)",
            )?);
            let mut model = crate::script::model_mut_of(lua);
            if !player_exists(&model) {
                return Ok(());
            }
            let bf = &mut model.battlefield;
            let Some(id) = bf.order.get(index as usize).copied() else {
                return Ok(());
            };
            if bf.row(id).is_none() {
                return Ok(());
            }
            bf.list_requests.push(id);
            bf.instances.clear();
            bf.bg_type = id;
            Ok(())
        })?,
    )?;

    g.set(
        "GetHolidayBGHonorCurrencyBonuses",
        lua.create_function(|lua, ()| Ok(reward(&read(lua).battlefield.holiday)))?,
    )?;
    g.set(
        "GetRandomBGHonorCurrencyBonuses",
        lua.create_function(|lua, ()| Ok(reward(&read(lua).battlefield.random)))?,
    )?;

    // Four values (`0x54c8a0`), nothing off the one queue.
    g.set(
        "GetWorldPVPQueueStatus",
        lua.create_function(|lua, index: Value| {
            let index = index0(number_arg(
                lua,
                index,
                "Usage: GetWorldPVPQueueStatus(index)",
            )?);
            if index != 0 {
                return Ok(MultiValue::new());
            }
            let model = read(lua);
            let q = &model.battlefield.frame.world_pvp;
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(status_text(q.status))?),
                opt_string(lua, q.area_name.as_deref())?,
                Value::Number(f64::from(q.queue_id)),
                Value::Number(if q.status == 2 {
                    f64::from(q.expire_in)
                } else {
                    0.0
                }),
            ]))
        })?,
    )?;

    // The four battle-manager sends take their queue unchecked (`lua_tonumber`, `0x54e160`).
    for (name, kind) in [
        ("BattlefieldMgrEntryInviteResponse", 0u8),
        ("BattlefieldMgrQueueInviteResponse", 1),
        ("BattlefieldMgrQueueRequest", 2),
        ("BattlefieldMgrExitRequest", 3),
    ] {
        g.set(
            name,
            lua.create_function(
                move |lua, (queue, accept): (Option<Value>, Option<Value>)| {
                    let queue_id =
                        super::binding_abi::ftol(super::binding_abi::coerced_number(lua, queue));
                    let accept = bool_or_default(accept.as_ref(), false);
                    let request = match kind {
                        0 => BattlefieldMgrRequest::EntryInviteResponse { queue_id, accept },
                        1 => BattlefieldMgrRequest::QueueInviteResponse { queue_id, accept },
                        2 => BattlefieldMgrRequest::QueueRequest { queue_id },
                        _ => BattlefieldMgrRequest::ExitRequest { queue_id },
                    };
                    crate::script::model_mut_of(lua)
                        .battlefield
                        .mgr_requests
                        .push(request);
                    Ok(())
                },
            )?,
        )?;
    }

    g.set(
        "HearthAndResurrectFromArea",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).battlefield.hearth_requests += 1;
            Ok(())
        })?,
    )?;
    g.set(
        "CanHearthAndResurrectFromArea",
        lua.create_function(|lua, ()| {
            Ok(flag(read(lua).battlefield.frame.can_hearth_and_resurrect))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Probe".into()),
                ..Default::default()
            }),
        );
        s
    }

    fn pending(s: &UiScript) -> Vec<String> {
        s.model_ref()
            .pending_events
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }

    fn row(id: u32, name: &str, ty: u32, min_level: i32) -> BattlemasterRow {
        BattlemasterRow {
            id,
            name: name.into(),
            instance_type: ty,
            max_group_size: 5,
            groups_allowed: true,
            min_level,
            can_enter: true,
            holiday: false,
            description: Some(format!("{name} text")),
            has_map: true,
        }
    }

    fn rows() -> Vec<BattlemasterRow> {
        vec![
            row(1, "Alterac Valley", 3, 51),
            row(2, "Warsong Gulch", 3, 10),
            row(3, "Arathi Basin", 3, 20),
            row(6, "All Arenas", 4, 10),
            row(RANDOM_BATTLEGROUND, "Random Battleground", 3, 80),
        ]
    }

    #[test]
    fn the_type_list_sorts_random_first_then_level_and_resorts_on_holiday() {
        let mut s = vm();
        s.set_battlemaster_rows(rows());
        assert_eq!(
            s.eval::<i64>("return GetNumBattlegroundTypes()").unwrap(),
            4,
            "arena rows are not battleground types"
        );
        let names = |s: &UiScript| {
            s.eval::<String>(
                "local t = {} for i = 1, GetNumBattlegroundTypes() do \
                 t[#t+1] = (GetBattlegroundInfo(i)) end return table.concat(t, ',')",
            )
            .unwrap()
        };
        assert_eq!(
            names(&s),
            "Random Battleground,Warsong Gulch,Arathi Basin,Alterac Valley"
        );
        let mut r = rows();
        r[0].holiday = true;
        s.set_battlemaster_rows(r);
        assert_eq!(
            names(&s),
            "Random Battleground,Warsong Gulch,Arathi Basin,Alterac Valley",
            "a new holiday waits for SortBGList"
        );
        assert!(s.eval::<bool>("return SortBGList()").unwrap());
        assert_eq!(
            names(&s),
            "Random Battleground,Alterac Valley,Warsong Gulch,Arathi Basin"
        );
        let got = s
            .eval::<String>(
                "local n, c, h, r, id = GetBattlegroundInfo(1) \
                 return n .. tostring(c) .. tostring(h) .. tostring(r) .. id",
            )
            .unwrap();
        assert_eq!(got, "Random Battleground1nil132");
        assert_eq!(s.arity("GetBattlegroundInfo(9)").unwrap(), 0);
    }

    #[test]
    fn requesting_a_type_switches_it_at_once() {
        let mut s = vm();
        s.set_battlemaster_rows(rows());
        s.run("RequestBattlegroundInstanceInfo(2)").unwrap();
        assert_eq!(s.take_battlefield_list_requests(), vec![2]);
        let got = s
            .eval::<String>("local n, d, g = GetBattlefieldInfo() return n .. '|' .. d .. '|' .. g")
            .unwrap();
        assert_eq!(got, "Warsong Gulch|Warsong Gulch text|5");
        assert_eq!(s.arity("GetBattlefieldInfo()").unwrap(), 6);
        s.run("RequestBattlegroundInstanceInfo(0) RequestBattlegroundInstanceInfo(9)")
            .unwrap();
        assert!(s.take_battlefield_list_requests().is_empty());
    }

    #[test]
    fn a_list_packet_fires_the_event_its_source_names() {
        let mut s = vm();
        s.set_battlemaster_rows(rows());
        s.apply_battlefield_list(BattlefieldListPush {
            battlemaster: 0x42,
            bg_type: 2,
            instances: vec![7, 3],
            ..Default::default()
        });
        assert_eq!(pending(&s), ["BATTLEFIELDS_SHOW"]);
        assert_eq!(s.battlemaster(), 0x42);
        assert_eq!(s.eval::<i64>("return GetNumBattlefields()").unwrap(), 2);
        s.run("JoinBattlefield(2, 1)").unwrap();
        assert_eq!(
            s.take_battlefield_joins(),
            vec![BattlefieldJoin::Battleground {
                battlemaster: 0x42,
                bg_type: 2,
                instance_id: 3,
                as_group: true
            }]
        );
        s.run("CloseBattlefield()").unwrap();
        assert_eq!(s.battlemaster(), 0);
        s.run("JoinBattlefield(2)").unwrap();
        assert_eq!(
            s.take_battlefield_joins(),
            vec![BattlefieldJoin::Battleground {
                battlemaster: 0,
                bg_type: 2,
                instance_id: 0,
                as_group: false
            }],
            "no battlemaster: first available"
        );
    }

    #[test]
    fn an_arena_join_takes_the_index_as_its_slot() {
        let mut s = vm();
        s.set_battlemaster_rows(rows());
        s.apply_battlefield_list(BattlefieldListPush {
            battlemaster: 0x77,
            bg_type: 6,
            ..Default::default()
        });
        assert_eq!(s.eval::<i64>("return IsBattlefieldArena()").unwrap(), 1);
        s.run("JoinBattlefield(1, 1, 1) JoinBattlefield(3)")
            .unwrap();
        assert_eq!(
            s.take_battlefield_joins(),
            vec![
                BattlefieldJoin::Arena {
                    battlemaster: 0x77,
                    slot: 0,
                    as_group: true,
                    rated: true
                },
                BattlefieldJoin::Arena {
                    battlemaster: 0x77,
                    slot: 2,
                    as_group: false,
                    rated: false
                },
            ]
        );
    }

    #[test]
    fn the_status_answers_seven_values_and_the_port_refuses() {
        let mut s = vm();
        let mut frame = BattlefieldFrame::default();
        frame.slots[0] = BattlefieldQueueSlot {
            status: 2,
            name: Some("Warsong Gulch".into()),
            bg_type: 2,
            instance_id: 4,
            min_level: 10,
            max_level: 19,
            port_expiration_ms: 79_900,
            ..Default::default()
        };
        s.set_battlefield_frame(frame.clone());
        let got = s
            .eval::<String>(
                "local st, n, id, lo, hi, size, rated = GetBattlefieldStatus(1) \
                 return st .. n .. id .. lo .. hi .. size .. tostring(rated)",
            )
            .unwrap();
        assert_eq!(got, "confirmWarsong Gulch410190nil");
        assert_eq!(s.arity("GetBattlefieldStatus(3)").unwrap(), 7);
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldPortExpiration(1)")
                .unwrap(),
            79,
            "whole seconds"
        );
        frame.port_refusal = Some("ERR_NOT_IN_COMBAT");
        s.set_battlefield_frame(frame);
        assert!(s
            .eval::<bool>("return AcceptBattlefieldPort(1, 1) == nil")
            .unwrap());
        assert!(s.take_battlefield_port_requests().is_empty());
        assert_eq!(
            s.eval::<i64>("return AcceptBattlefieldPort(1, 0)").unwrap(),
            1
        );
        assert_eq!(s.take_battlefield_port_requests(), vec![(0, false)]);
        assert!(s
            .eval::<bool>("return AcceptBattlefieldPort(3, 1) == nil")
            .unwrap());
    }

    #[test]
    fn the_active_arena_answers_two_values() {
        let mut s = vm();
        let mut frame = BattlefieldFrame {
            active: Some(1),
            instance_type: INSTANCE_TYPE_ARENA,
            ..Default::default()
        };
        frame.slots[1].rated = true;
        s.set_battlefield_frame(frame);
        assert_eq!(
            s.eval::<String>(
                "local a, r = IsActiveBattlefieldArena() return tostring(a) .. tostring(r)"
            )
            .unwrap(),
            "11"
        );
    }
}
