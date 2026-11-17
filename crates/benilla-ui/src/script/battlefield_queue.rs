//! The queue half of the reference's battlefield engine (bindings `0x549ad0`-`0x54e6d0`): the
//! battleground and arena lists `PVPBattlegroundFrame.lua`, `BattlefieldFrame.lua` and
//! `ArenaFrame.lua` read, the two queue slots behind the minimap icon, and the Wintergrasp queue.
//! The VM owns what the bindings write (the listed type, the instances, the selection, the
//! battlemaster), which the list handler fills (`0x54e390`); the slots, clocks and
//! `BattlemasterList.dbc` resolved for the player are read live when a verb is called.

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
    /// The bracket-for-level lookup (`0x54ba00`): the player's level is in one of the first map's
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

/// One queue slot (`0xbea4b8`) reduced by the app at the calling verb's clock.
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

/// The one world-PvP queue (`0xbea4a8`) as the app resolved it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorldPvpQueueView {
    pub queue_id: i32,
    /// 0 none, 1 queued, 2 confirm, 3 active.
    pub status: u32,
    pub area_name: Option<String>,
    /// Status 2's expiry less the UI clock, seconds; 0 otherwise.
    pub expire_in: i32,
}

/// The active instance's globals as the queue's verbs read them when called.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldInstance {
    /// The slot whose instance the player is in (`0xacd16c`).
    pub active: Option<u8>,
    /// The active map's `InstanceType` (`0xbea570`): 4 inside an arena.
    pub instance_type: u32,
    /// The arena faction (`0xbea54a`).
    pub arena_faction: u8,
    /// `GetBattlefieldInstanceExpiration()`, ms.
    pub instance_expiration_ms: u32,
    /// `GetBattlefieldInstanceRunTime()`, ms.
    pub run_time_ms: u32,
}

/// A battleground row as the type list's sort reads it (`0x54a650`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BattlegroundType {
    pub id: u32,
    pub min_level: i32,
    /// The row's holiday world state is set.
    pub holiday: bool,
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

/// The VM's mirror of the reference's battlefield engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BattlefieldInfo {
    /// The types whose holiday ran at the last sort (`0xbea720`): none at start (`0x54e720`), today's in `SortBGList` (`0x54c9f0`).
    sort_holidays: Vec<u32>,
    /// The battleground type id (`0xbea53c`), the type the list and the join name.
    bg_type: u32,
    /// The instance list (`0xbea5bc`).
    instances: Vec<u32>,
    /// The battlemaster (`0xbea568`), 0 for the PvP window's list.
    battlemaster: u64,
    /// The selected instance (`0xbea538`), an instance id.
    selected: u32,
    /// The join allowance (`0xacd174`).
    allow_join: bool,
    holiday: BattlefieldRewardView,
    random: BattlefieldRewardView,
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
            sort_holidays: Vec::new(),
            bg_type: 0,
            instances: Vec::new(),
            battlemaster: 0,
            selected: 0,
            allow_join: true,
            holiday: Default::default(),
            random: Default::default(),
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
    /// The battleground types in sorted order (`0x54a650`): random, holiday, lower minimum level, lower id.
    fn order(&self, mut types: Vec<BattlegroundType>) -> Vec<u32> {
        types.sort_by_key(|r| {
            (
                r.id != RANDOM_BATTLEGROUND,
                !self.sort_holidays.contains(&r.id),
                r.min_level,
                r.id,
            )
        });
        types.into_iter().map(|r| r.id).collect()
    }

    /// The listed type's row, the gate most bindings open with.
    fn listed(&self) -> Option<BattlemasterRow> {
        let id = self.bg_type;
        super::read_live(|l| l.battlemaster_row(id)).flatten()
    }
}

/// The battleground rows the type list sorts, at the call; none with no view lent.
fn types() -> Vec<BattlegroundType> {
    super::read_live(|l| l.battleground_types()).unwrap_or_default()
}

/// Queue slot `index` at the call, `None` off the two; an empty slot with no view lent.
fn slot(index: u32) -> Option<BattlefieldQueueSlot> {
    let index = usize::try_from(index).ok()?;
    super::read_live(|l| l.battlefield_slot(index))
        .unwrap_or_else(|| (index < 2).then(Default::default))
}

/// The active instance's globals at the call.
fn instance() -> BattlefieldInstance {
    super::read_live(|l| l.battlefield_instance()).unwrap_or_default()
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

/// The active player's gate: the object manager resolves the active guid as `TYPEMASK_PLAYER`.
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

impl super::UiScript {
    /// `SMSG_BATTLEFIELD_LIST` (`0x54e390`): the type, rewards and instances, then the event its
    /// source names. A battlemaster opens the session (the battlemaster setter, `0x5496d0`).
    pub fn apply_battlefield_list(&mut self, list: BattlefieldListPush) {
        let events: &[&str] = {
            let mut model = self.model_mut();
            let bf = &mut model.battlefield;
            bf.bg_type = list.bg_type;
            bf.holiday = list.holiday;
            bf.random = list.random;
            bf.instances = list.instances;
            bf.allow_join = true;
            if list.bg_type == RANDOM_BATTLEGROUND && list.battlemaster != 0 {
                bf.battlemaster = list.battlemaster;
                &["NPC_PVPQUEUE_ANYWHERE"]
            } else if list.from_anywhere {
                &["PVPQUEUE_ANYWHERE_SHOW"]
            } else if list.battlemaster != 0 {
                bf.battlemaster = list.battlemaster;
                &["BATTLEFIELDS_SHOW"]
            } else {
                bf.battlemaster = 0;
                &["BATTLEFIELDS_CLOSED", "BATTLEFIELDS_SHOW"]
            }
        };
        for &name in events {
            self.fire_event(name, Vec::new());
        }
    }

    /// The battlemaster setter with none (`0x5496d0`): `BATTLEFIELDS_CLOSED` always, then cleared.
    pub fn close_battlemaster(&mut self) {
        self.fire_event("BATTLEFIELDS_CLOSED", Vec::new());
        self.model_mut().battlefield.battlemaster = 0;
    }

    /// The world leave clears the instance list (`0x54e330`).
    pub fn leave_battlefield_world(&mut self) {
        self.model_mut().battlefield.instances.clear();
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
        lua.create_function(|_, ()| {
            let f = instance();
            let rated = f
                .active
                .and_then(|i| slot(u32::from(i)))
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
                let row = model.battlefield.listed();
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
            super::tick::fire_event_into(lua, "BATTLEFIELDS_CLOSED", Vec::new());
            let mut model = crate::script::model_mut_of(lua);
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
            let Some(s) = slot(index) else {
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
            Ok(slot(index).map_or(0, |s| i64::from(s.port_expiration_ms / 1000)))
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
                Ok(slot(index).map_or(0, |s| i64::from(get(&s))))
            })?,
        )?;
    }

    g.set(
        "GetBattlefieldInstanceExpiration",
        lua.create_function(|_, ()| Ok(i64::from(instance().instance_expiration_ms)))?,
    )?;

    g.set(
        "GetBattlefieldInstanceRunTime",
        lua.create_function(|_, ()| Ok(i64::from(instance().run_time_ms)))?,
    )?;

    g.set(
        "GetBattlefieldArenaFaction",
        lua.create_function(|_, ()| Ok(flag(instance().arena_faction != 0)))?,
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
            let Some(slot) = slot(index) else {
                return Ok(Value::Nil);
            };
            let accept = bool_or_default(accept.as_ref(), false);
            let mut model = crate::script::model_mut_of(lua);
            if accept {
                // The fatigue leg reads the mirror timer table last (`0x54dbab`).
                let fatigued = model.mirror_timers[0]
                    .as_ref()
                    .is_some_and(|t| t.scale < 0)
                    .then_some("ERR_NOT_WHILE_FATIGUED");
                let refusal = super::read_live(|l| l.battlefield_port_refusal())
                    .flatten()
                    .or(fatigued);
                if let Some(key) = refusal {
                    model.ui_errors.push(key);
                    return Ok(Value::Nil);
                }
            }
            model.battlefield.port_requests.push((index as u8, accept));
            let closed = !accept && slot.bg_type == model.battlefield.bg_type;
            drop(model);
            if closed {
                super::tick::fire_event_into(lua, "BATTLEFIELDS_CLOSED", Vec::new());
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
        lua.create_function(|lua, ()| Ok(read(lua).battlefield.order(types()).len() as i64))?,
    )?;

    // Five values (`0x54e010`): name, canEnter, isHoliday, isRandom, id; nothing off the list.
    g.set(
        "GetBattlegroundInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlegroundInfo(index)")?;
            let model = read(lua);
            let row = usize::try_from(index - 1)
                .ok()
                .and_then(|i| model.battlefield.order(types()).get(i).copied())
                .filter(|_| player_exists(&model))
                .and_then(|id| super::read_live(|l| l.battlemaster_row(id)).flatten());
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
            crate::script::model_mut_of(lua).battlefield.sort_holidays = types()
                .into_iter()
                .filter(|r| r.holiday)
                .map(|r| r.id)
                .collect();
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
            let Some(id) = bf.order(types()).get(index as usize).copied() else {
                return Ok(());
            };
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
            let q = super::read_live(|l| l.world_pvp_queue()).unwrap_or_default();
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
        lua.create_function(|_, ()| {
            Ok(flag(
                super::read_live(|l| l.can_hearth_and_resurrect()).unwrap_or(false),
            ))
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

    /// The queue state a test view answers from.
    #[derive(Clone, Default)]
    struct BattlefieldFrame {
        slots: [BattlefieldQueueSlot; 2],
        active: Option<u8>,
        instance_type: u32,
        port_refusal: Option<&'static str>,
    }

    /// A view whose queue and rows are fixed.
    #[derive(Default)]
    struct View {
        frame: BattlefieldFrame,
        rows: Vec<BattlemasterRow>,
    }

    impl crate::script::LiveState for View {
        fn unit_flags(&self, _: u64) -> Option<u32> {
            None
        }
        fn unit_health(&self, _: u64) -> Option<u32> {
            None
        }
        fn active_player(&self) -> Option<u64> {
            None
        }
        fn has_control(&self) -> bool {
            true
        }
        fn far_sight(&self) -> bool {
            false
        }
        fn battlefield_slot(&self, index: usize) -> Option<BattlefieldQueueSlot> {
            self.frame.slots.get(index).cloned()
        }
        fn battlefield_instance(&self) -> BattlefieldInstance {
            BattlefieldInstance {
                active: self.frame.active,
                instance_type: self.frame.instance_type,
                ..Default::default()
            }
        }
        fn battlefield_port_refusal(&self) -> Option<&'static str> {
            self.frame.port_refusal
        }
        fn battlemaster_row(&self, id: u32) -> Option<BattlemasterRow> {
            self.rows.iter().find(|r| r.id == id).cloned()
        }
        fn battleground_types(&self) -> Vec<BattlegroundType> {
            self.rows
                .iter()
                .filter(|r| r.instance_type == INSTANCE_TYPE_BATTLEGROUND)
                .map(|r| BattlegroundType {
                    id: r.id,
                    min_level: r.min_level,
                    holiday: r.holiday,
                })
                .collect()
        }
    }

    fn with_rows(rows: Vec<BattlemasterRow>) -> View {
        View {
            rows,
            ..Default::default()
        }
    }

    /// The events `f` fires, in order.
    fn fired(s: &mut UiScript, f: impl FnOnce(&mut UiScript)) -> Vec<String> {
        s.run(
            "BQ_SEEN = {} local f = CreateFrame('Frame') f:RegisterAllEvents() \
             f:SetScript('OnEvent', function(_, e) BQ_SEEN[#BQ_SEEN + 1] = e end)",
        )
        .unwrap();
        f(s);
        s.eval::<String>("local e = table.concat(BQ_SEEN, ',') BQ_SEEN = {} return e")
            .unwrap()
            .split(',')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
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
        let view = with_rows(rows());
        assert_eq!(
            s.with_live(&view, |s| s.eval::<i64>("return GetNumBattlegroundTypes()"))
                .unwrap(),
            4,
            "arena rows are not battleground types"
        );
        let names = |s: &mut UiScript, view: &View| {
            s.with_live(view, |s| {
                s.eval::<String>(
                    "local t = {} for i = 1, GetNumBattlegroundTypes() do \
                     t[#t+1] = (GetBattlegroundInfo(i)) end return table.concat(t, ',')",
                )
            })
            .unwrap()
        };
        assert_eq!(
            names(&mut s, &view),
            "Random Battleground,Warsong Gulch,Arathi Basin,Alterac Valley"
        );
        let mut r = rows();
        r[0].holiday = true;
        let holiday = with_rows(r);
        assert_eq!(
            names(&mut s, &holiday),
            "Random Battleground,Warsong Gulch,Arathi Basin,Alterac Valley",
            "a new holiday waits for SortBGList"
        );
        assert!(s
            .with_live(&holiday, |s| s.eval::<bool>("return SortBGList()"))
            .unwrap());
        assert_eq!(
            names(&mut s, &holiday),
            "Random Battleground,Alterac Valley,Warsong Gulch,Arathi Basin"
        );
        assert_eq!(
            names(&mut s, &view),
            "Random Battleground,Alterac Valley,Warsong Gulch,Arathi Basin",
            "the last sort holds until the next"
        );
        let got = s
            .with_live(&holiday, |s| {
                s.eval::<String>(
                    "local n, c, h, r, id = GetBattlegroundInfo(1) \
                     return n .. tostring(c) .. tostring(h) .. tostring(r) .. id",
                )
            })
            .unwrap();
        assert_eq!(got, "Random Battleground1nil132");
        assert_eq!(s.arity("GetBattlegroundInfo(9)").unwrap(), 0);
        assert_eq!(
            s.eval::<i64>("return GetNumBattlegroundTypes()").unwrap(),
            0,
            "no view, no rows"
        );
    }

    #[test]
    fn requesting_a_type_switches_it_at_once() {
        let mut s = vm();
        let view = with_rows(rows());
        s.with_live(&view, |s| s.run("RequestBattlegroundInstanceInfo(2)"))
            .unwrap();
        assert_eq!(s.take_battlefield_list_requests(), vec![2]);
        let got = s
            .with_live(&view, |s| {
                s.eval::<String>(
                    "local n, d, g = GetBattlefieldInfo() return n .. '|' .. d .. '|' .. g",
                )
            })
            .unwrap();
        assert_eq!(got, "Warsong Gulch|Warsong Gulch text|5");
        assert_eq!(
            s.with_live(&view, |s| s.arity("GetBattlefieldInfo()"))
                .unwrap(),
            6
        );
        s.with_live(&view, |s| {
            s.run("RequestBattlegroundInstanceInfo(0) RequestBattlegroundInstanceInfo(9)")
        })
        .unwrap();
        assert!(s.take_battlefield_list_requests().is_empty());
    }

    #[test]
    fn a_list_packet_fires_the_event_its_source_names() {
        let mut s = vm();
        let shown = fired(&mut s, |s| {
            s.apply_battlefield_list(BattlefieldListPush {
                battlemaster: 0x42,
                bg_type: 2,
                instances: vec![7, 3],
                ..Default::default()
            })
        });
        assert_eq!(shown, ["BATTLEFIELDS_SHOW"], "fired by the list itself");
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
        let view = with_rows(rows());
        s.apply_battlefield_list(BattlefieldListPush {
            battlemaster: 0x77,
            bg_type: 6,
            ..Default::default()
        });
        assert_eq!(
            s.with_live(&view, |s| s.eval::<i64>("return IsBattlefieldArena()"))
                .unwrap(),
            1
        );
        s.with_live(&view, |s| {
            s.run("JoinBattlefield(1, 1, 1) JoinBattlefield(3)")
        })
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
        let view = View {
            frame: frame.clone(),
            ..Default::default()
        };
        let got = s
            .with_live(&view, |s| {
                s.eval::<String>(
                    "local st, n, id, lo, hi, size, rated = GetBattlefieldStatus(1) \
                     return st .. n .. id .. lo .. hi .. size .. tostring(rated)",
                )
            })
            .unwrap();
        assert_eq!(got, "confirmWarsong Gulch410190nil");
        assert_eq!(
            s.with_live(&view, |s| s.arity("GetBattlefieldStatus(3)"))
                .unwrap(),
            7
        );
        assert_eq!(
            s.with_live(&view, |s| s
                .eval::<i64>("return GetBattlefieldPortExpiration(1)"))
                .unwrap(),
            79,
            "whole seconds"
        );
        frame.port_refusal = Some("ERR_NOT_IN_COMBAT");
        let refused = View {
            frame,
            ..Default::default()
        };
        let ask = |s: &mut UiScript, lua: &str| s.with_live(&refused, |s| s.eval::<bool>(lua));
        assert!(ask(&mut s, "return AcceptBattlefieldPort(1, 1) == nil").unwrap());
        assert!(s.take_battlefield_port_requests().is_empty());
        assert!(ask(&mut s, "return AcceptBattlefieldPort(1, 0) == 1").unwrap());
        assert_eq!(s.take_battlefield_port_requests(), vec![(0, false)]);
        assert!(ask(&mut s, "return AcceptBattlefieldPort(3, 1) == nil").unwrap());
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
        let view = View {
            frame,
            ..Default::default()
        };
        let ask = |s: &mut UiScript| {
            s.eval::<String>(
                "local a, r = IsActiveBattlefieldArena() return tostring(a) .. tostring(r)",
            )
            .unwrap()
        };
        assert_eq!(s.with_live(&view, ask), "11");
        assert_eq!(ask(&mut s), "nilnil", "no view: no arena");
    }
}
