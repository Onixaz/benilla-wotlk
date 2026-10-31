//! The party and raid bindings. The app pushes a roster snapshot ([`UiScript::set_party`]) that
//! the getters read; the verbs queue a [`PartyRequest`] the app drains
//! ([`UiScript::take_party_requests`]) into the send. Per-member game state is the unit snapshot
//! under `party1`..`party4` ([`super::unit::UnitState`]).
//!
//! The raid roster ([`PartyState::raid`]) is one list, the player included, because the reference's
//! count bounds the very array `GetRaidRosterInfo` indexes. The saved-instance list
//! ([`SavedInstanceInfo`]) is a separate push, as a roster push replaces [`PartyState`] whole.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, number_arg, optional_string};
use super::Model;

/// One party member; everything else about them is the unit snapshot under their `partyN` token.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyMemberInfo {
    pub name: String,
    /// The identity `UnitInParty` matches any token against; `0` never matches.
    pub guid: u64,
}

/// One saved lockout from `SMSG_RAID_INSTANCE_INFO` (`0x501030`), with the `Map.dbc` and
/// `MapDifficulty.dbc` reads `GetSavedInstanceInfo` (`0x4ff2c0`) makes resolved by the app
/// ([`UiScript::set_saved_instances`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedInstanceInfo {
    /// The `Map.dbc` id, which `SetSavedInstanceExtend` sends back.
    pub map: u32,
    /// The `Map.dbc` name; `None` with no row, which answers the miss tuple.
    pub name: Option<String>,
    pub instance: u64,
    /// Seconds until the lockout resets, as the server sent them.
    pub reset: u32,
    /// The difficulty, stored 0-based and exposed 1-based.
    pub difficulty: u32,
    pub locked: bool,
    pub extended: bool,
    /// `Map.dbc` `InstanceType` is 2.
    pub is_raid: bool,
    /// The map's player cap over the difficulty.
    pub max_players: u32,
    /// The `MapDifficulty` row's difficulty string through `GetText`; empty with no row.
    pub difficulty_name: String,
}

/// One raid roster row, `GetRaidRosterInfo`'s fourteen returns in push order (`0x573690`). The
/// binding, not the row, applies the subgroup's +1, the offline zone and the master-looter test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RaidMemberInfo {
    /// Return 1, from the name cache. Empty means not cached, which the reference answers with the
    /// miss tuple, as for an empty slot (`0x573690`).
    pub name: String,
    /// The identity `UnitInRaid` matches a token against; `0` never matches, as the reference's
    /// membership test rejects a zero guid (`0x60ca20`).
    pub guid: u64,
    /// Return 2, exposed as stored: 0 member, 1 assistant, 2 leader,
    /// the convention addons read (`ChatLog.lua:351-353`).
    pub rank: u32,
    /// Return 3, stored 0-based as the wire's `GroupMemberEntry::flags` bits 0-2 carry it; the
    /// binding exposes it 1-based.
    pub subgroup: u32,
    /// Return 4, `0` until an object or a stats packet carries it, as in the reference's third arm.
    pub level: u32,
    /// Return 5, the localized class name.
    pub class: Option<String>,
    /// Return 6, the class token (`"WARRIOR"`), the one unlocalized string.
    pub class_file: Option<String>,
    /// Return 7 while [`Self::online`]; offline, the binding pushes the `PLAYER_OFFLINE` global
    /// instead and never reads this.
    pub zone: Option<String>,
    /// Return 8, which also picks return 7's offline arm.
    pub online: bool,
    /// Return 9, unnamed: `1` for a streamed member whose health (`[obj+0x110 +0x40]`) is 0 or
    /// below, or an unstreamed one whose roster status (`[+0x18]`) has both `0x1` and `0x4`,
    /// vmangos's `ONLINE` and `DEAD`, from which the app fills it. Addons read it as `isDead`.
    pub ninth: bool,
    /// The member's assignment bits, main tank 0x2 and main assist 0x4: return 10's role and
    /// `GetPartyAssignment`'s source.
    pub assignments: u8,
    /// The LFG role bits, tank 0x2, healer 0x4, damage 0x8: returns 12 to 14.
    pub roles: u8,
}

/// The party and raid roster snapshot the app pushes whole when it changes
/// ([`UiScript::set_party`]); the default is ungrouped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartyState {
    /// The other party members in `party1`..`party4` order, empty when ungrouped. The player is
    /// never in it, as in `SMSG_GROUP_LIST`; in a raid it is our own subgroup's slice.
    pub members: Vec<PartyMemberInfo>,
    /// `GetPartyLeaderIndex`: `0` the player, `1..=4` that `members` slot.
    pub leader_index: u32,
    /// The cached group-leader guid, `0` when ungrouped, fed from the wire rather than derived from
    /// [`Self::leader_index`]. `UnitIsPartyLeader` compares a resolved token's guid with it and,
    /// unlike `IsPartyLeader`, has no zero guard, so an unresolvable token answers `1` while solo.
    pub leader_guid: u64,
    /// The player's own guid, `0` out of world. With [`Self::leader_guid`] it is the ready-check
    /// timeout's leader gate, a bare guid compare, so a solo player never matches.
    pub own_guid: u64,
    /// The whole raid roster, 1-based for `GetRaidRosterInfo`, empty outside a raid. It includes
    /// the player, as the reference's array does, so `UnitInRaid("player")` answers `1`;
    /// `GetNumRaidMembers()` is its length.
    pub raid: Vec<RaidMemberInfo>,
    /// `GetLootMethod`'s method: `"freeforall"`, `"roundrobin"`, `"master"`, `"group"` or
    /// `"needbeforegreed"`. Empty, the default, reads as `"freeforall"`, the reference's answer
    /// before any group.
    pub loot_method: String,
    /// The master looter as a party index (`0` the player, `1..=4` a `members` slot),
    /// `GetLootMethod`'s second return.
    pub master_looter: Option<u32>,
    /// `GetLootThreshold`'s item quality, `2..=4` once grouped.
    pub loot_threshold: u32,
    /// The party's and the raid's real member counts: the group outside a battleground, which a
    /// battleground's list leaves as it was (`0x6d8d42`-`0x6d8d84`).
    pub real_party_members: u32,
    pub real_raid_members: u32,
    /// The master looter's guid, 0 unless the method is master loot: `GetRaidRosterInfo`'s eleventh
    /// return.
    pub loot_master_guid: u64,
    /// The party slots' assignment bits, as [`RaidMemberInfo::assignments`].
    pub member_assignments: [u8; 4],
}

/// Outbound party and raid intents, drained by the app ([`UiScript::take_party_requests`]) into
/// the matching send.
#[derive(Clone, Debug, PartialEq)]
pub enum PartyRequest {
    /// `AcceptGroup([tank, healer, dps])` (`0x51ab20`): role bits 2, 4 and 8.
    Accept { roles: u32 },
    /// `DeclineGroup()`.
    Decline,
    /// `LeaveParty()`.
    Leave,
    /// `InviteUnit(name [, a, b, c])` (`0x51a6e0`): a name, and flag bits 2, 4, 8 from arguments 2-4.
    InviteUnit { name: String, flags: u32 },
    /// `UninviteUnit(unit [, reason])` (`0x51a7a0`): a token or name, "" the target; the reason.
    UninviteUnit { unit: String, reason: String },
    /// `PromoteToLeader(unit [, exact])` (`0x573e10`): a token or a member's name.
    PromoteToLeader(String),
    /// `SetLootMethod(method[, masterName][, threshold])`; the app resolves the master looter's
    /// name to a roster member.
    LootMethod {
        method: String,
        master_name: Option<String>,
        /// The reference reads it for every method, the master looter's name only for `"master"`.
        threshold: Option<u32>,
    },
    /// `SetLootThreshold(n)`.
    LootThreshold(u32),
    /// `SetRaidTarget(unit, index)`: icon 1 to 8, or 0 to clear; the app resolves the token to a
    /// guid for `MSG_RAID_TARGET_UPDATE`.
    SetRaidTarget { unit: String, index: u8 },
    // ── The raid-management verbs ───────────────────────────────────────────────────────────
    /// `ConvertToRaid()`, leader only (`CMSG_GROUP_RAID_CONVERT`).
    ConvertToRaid,
    /// `SetRaidSubgroup(index, group)`, both 1-based. The wire (`CMSG_GROUP_CHANGE_SUB_GROUP`)
    /// takes a name and a 0-based subgroup, which the app resolves.
    SetSubgroup { index: u32, group: u32 },
    /// `SwapRaidSubgroup(index, other)`: trade two raid rows' subgroups
    /// (`CMSG_GROUP_SWAP_SUB_GROUP`, two names on the wire).
    SwapSubgroup { index: u32, other: u32 },
    /// `PromoteToAssistant(name)` / `DemoteAssistant(name)` (`CMSG_GROUP_ASSISTANT_LEADER`).
    AssistantLeader { name: String, grant: bool },
    /// `SetPartyAssignment`/`ClearPartyAssignment` (`MSG_PARTY_ASSIGNMENT`, `0x6d7200`): 0 main
    /// tank, 1 main assist, set or clear, on the member's guid (0 with no name given).
    PartyAssignment {
        assignment: u8,
        set: bool,
        guid: u64,
    },
    /// `SetSavedInstanceExtend(index, extend)` (`CMSG_SET_SAVED_INSTANCE_EXTEND`, `0x50dc10`).
    SavedInstanceExtend {
        map: u32,
        difficulty: u32,
        extend: bool,
    },
    /// `DoReadyCheck()`, leader only (`MSG_RAID_READY_CHECK`, empty body).
    ReadyCheckStart,
    /// `ConfirmReadyCheck(ready)` (`MSG_RAID_READY_CHECK`, one byte).
    ReadyCheckAnswer(bool),
    /// `RequestRaidInfo()` (`CMSG_REQUEST_RAID_INFO`).
    RequestRaidInfo,
    /// `SetDungeonDifficulty(n)` (`0x526050`), past its gates: the 0-based difficulty, which the
    /// app stores as the player's and the group's, then sends `MSG_SET_DUNGEON_DIFFICULTY`.
    SetDungeonDifficulty(u32),
    /// `SetRaidDifficulty(n)` (`0x5261a0`), past its gates: as the dungeon's, for the raid.
    SetRaidDifficulty(u32),
}

impl super::UiScript {
    /// Push the roster snapshot, replacing it, and run `SMSG_GROUP_LIST`'s ready-check leg over the
    /// new roster. The app fires the `PARTY_*` events.
    pub fn set_party(&mut self, state: PartyState) {
        set_field!(self, party, state);
        // The ready-check leg (from the `0x7d` handler): a member who left drops their pending
        // flag, and nobody left pending force-closes the check.
        let lua = self.lua();
        let mut model = self.model_mut();
        if model.ready_check.deadline.is_some() {
            let roster: Vec<u64> = model.party.raid.iter().map(|m| m.guid).collect();
            model.ready_check.unanswered.retain(|g| roster.contains(g));
            if model.ready_check.unanswered.is_empty() {
                ready_check_force_close(lua, &mut model);
            }
        }
    }

    /// The `MSG_RAID_READY_CHECK` open form arrived. The leader force-closes if nobody is left
    /// pending; anyone else arms the 30 s deadline, which their leader-gated tick never acts on.
    /// The app fires the `READY_CHECK` popup.
    pub fn ready_check_request(&mut self, we_lead: bool) {
        let lua = self.lua();
        let now = clock(lua);
        let mut model = self.model_mut();
        if we_lead {
            if model.ready_check.deadline.is_some() && !ready_check_pending_online(&model) {
                ready_check_force_close(lua, &mut model);
            }
        } else {
            model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
            model.ready_check.ready.clear();
            model.ready_check.initiator = model.party.leader_guid;
        }
    }

    /// One member's answer, relayed to the leader (guid matched in full). Any answer clears the
    /// member's pending flag, as the handler stores a constant 0 whatever the status byte; a `0`
    /// also prints `RAID_MEMBER_NOT_READY` for them. Once no member is both pending and online the
    /// check closes at once, so an offline member never holds it open but is still listed as AFK.
    pub fn ready_check_answered(&mut self, guid: u64, ready: bool) {
        let lua = self.lua();
        let mut model = self.model_mut();
        let Some(member) = model.party.raid.iter().find(|m| m.guid == guid) else {
            return; // an unmatched guid writes nothing
        };
        let name = member.name.clone();
        model.ready_check.unanswered.retain(|g| *g != guid);
        if ready {
            model.ready_check.ready.push(guid);
        } else {
            let template: String = lua
                .globals()
                .get::<String>("RAID_MEMBER_NOT_READY")
                .unwrap_or_default();
            model
                .ready_check
                .lines
                .push(template.replacen("%s", &name, 1));
        }
        if model.ready_check.deadline.is_some() && !ready_check_pending_online(&model) {
            ready_check_force_close(lua, &mut model);
        }
    }

    /// Drain the ready-check lines composed since the last call; the reference prints each as a
    /// `CHAT_MSG_SYSTEM` line (`(text, 10)`).
    pub fn take_ready_check_lines(&mut self) -> Vec<String> {
        drain_field!(self, ready_check.lines)
    }

    /// Drain the system lines the party bindings composed (`SetSavedInstanceExtend`'s lock line).
    pub fn take_party_lines(&mut self) -> Vec<String> {
        drain_field!(self, party_lines)
    }

    /// Drain the party/loot intents queued since the last call.
    pub fn take_party_requests(&mut self) -> Vec<PartyRequest> {
        drain_field!(self, party_requests)
    }

    /// Push the saved raid-lockout list, replacing it; the app fires `UPDATE_INSTANCE_INFO`.
    pub fn set_saved_instances(&mut self, saved: Vec<SavedInstanceInfo>) {
        set_field!(self, saved_instances, saved);
    }
}

/// Whether the player leads the group. `IsRaidLeader` (`0x573a60`) and `IsPartyLeader` (`0x52ccd0`)
/// both compare the cached leader guid with the player's own and read no raid flag, so
/// `IsRaidLeader()` is true for a party leader.
fn leads_the_group(model: &Model) -> bool {
    // Stand-in for that compare: `leader_index` is `0` when we lead but also when ungrouped, where
    // the reference's zero leader guid matches nobody, hence the empty-roster guard. In a raid both
    // cover only our subgroup: a leader elsewhere also reads `0`, and a leader alone in theirs has
    // no `members`, so there it can differ from the guid compare.
    !model.party.members.is_empty() && model.party.leader_index == 0
}

/// `GetRaidRosterInfo`'s fourteen values for one row, or the miss tuple for `None`: out of range,
/// an empty slot and a name-cache miss push `nil, 0, 1, 1` and ten `nil`s (`0x573690`). Takes the
/// row by value because every arm re-enters Lua.
fn raid_roster_info(
    lua: &Lua,
    row: Option<RaidMemberInfo>,
    loot_master: u64,
) -> mlua::Result<MultiValue> {
    let Some(m) = row else {
        let mut miss = vec![Value::Nil; 14];
        miss[1] = Value::Integer(0);
        miss[2] = Value::Integer(1);
        miss[3] = Value::Integer(1);
        return Ok(MultiValue::from_vec(miss));
    };
    let opt_str = |s: &Option<String>| match s {
        Some(s) => lua.create_string(s).map(Value::String),
        None => Ok(Value::Nil),
    };
    // Return 7: the zone while online, else the `PLAYER_OFFLINE` text (`0x573690`).
    let zone = if m.online {
        opt_str(&m.zone)?
    } else {
        Value::String(lua.create_string(super::get_text::global_text(lua, "PLAYER_OFFLINE"))?)
    };
    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
    // Return 10: main tank outranks main assist.
    let role = if m.assignments & ASSIGNMENT_MAIN_TANK != 0 {
        Value::String(lua.create_string("MAINTANK")?)
    } else if m.assignments & ASSIGNMENT_MAIN_ASSIST != 0 {
        Value::String(lua.create_string("MAINASSIST")?)
    } else {
        Value::Nil
    };
    Ok(MultiValue::from_vec(vec![
        lua.create_string(&m.name).map(Value::String)?,
        Value::Integer(i64::from(m.rank)),
        Value::Integer(i64::from(m.subgroup) + 1),
        Value::Integer(i64::from(m.level)),
        opt_str(&m.class)?,
        opt_str(&m.class_file)?,
        zone,
        flag(m.online),
        flag(m.ninth),
        role,
        // Return 11: master loot names this member (`m_lootMethod == 2`).
        flag(loot_master != 0 && loot_master == m.guid),
        flag(m.roles & 0x2 != 0),
        flag(m.roles & 0x4 != 0),
        flag(m.roles & 0x8 != 0),
    ]))
}

/// A member's assignment bits: main tank, main assist.
pub(crate) const ASSIGNMENT_MAIN_TANK: u8 = 0x2;
pub(crate) const ASSIGNMENT_MAIN_ASSIST: u8 = 0x4;

/// Register the party and raid globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumPartyMembers() → the other members, never counting the player; 0 when ungrouped.
    g.set(
        "GetNumPartyMembers",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.party.members.len() as i64)
        })?,
    )?;

    // GetNumRaidMembers() → the raid roster's length, 0 outside a raid.
    g.set(
        "GetNumRaidMembers",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.party.raid.len() as i64)
        })?,
    )?;

    // GetRealNumPartyMembers() (`0x52c190`) and GetRealNumRaidMembers() (`0x572b80`).
    g.set(
        "GetRealNumPartyMembers",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(f64::from(model.party.real_party_members))
        })?,
    )?;
    g.set(
        "GetRealNumRaidMembers",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(f64::from(model.party.real_raid_members))
        })?,
    )?;

    // GetRaidRosterInfo(index) (`0x573690`) → name, rank, subgroup, level, class, fileName, zone,
    // online, isDead, role, isML, tank, healer, damage; its only raise is a non-number argument.
    g.set(
        "GetRaidRosterInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetRaidRosterInfo(index)")?;
            let (row, loot_master) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                // One unsigned compare after the decrement catches 0 and below and one past the
                // count; a name-cache miss shares the miss arm.
                let row = usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.party.raid.get(i))
                    .filter(|m| !m.name.is_empty())
                    .cloned();
                (row, model.party.loot_master_guid)
            };
            raid_roster_info(lua, row, loot_master)
        })?,
    )?;

    // IsRaidLeader() → 1 or nil, true for a party leader too (`leads_the_group`).
    g.set(
        "IsRaidLeader",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if leads_the_group(&model) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyMember(id) → 1 for a filled 1-based slot, else nil.
    g.set(
        "GetPartyMember",
        lua.create_function(|lua, id: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let n = model.party.members.len() as i64;
            Ok(if id >= 1 && id <= n {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetPartyLeaderIndex() → 0 (the player leads) or 1..4 (that party slot leads).
    g.set(
        "GetPartyLeaderIndex",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.party.leader_index))
        })?,
    )?;

    // IsPartyLeader() → 1 when grouped and leading, else nil; the same predicate as IsRaidLeader.
    g.set(
        "IsPartyLeader",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if leads_the_group(&model) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetLootMethod() → lootmethod, masterlooterPartyID: two returns in 1.12.
    g.set(
        "GetLootMethod",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // Never grouped, the reference reads zero-filled cells past `.data`'s end, and method
            // 0 is `freeforall`.
            let method = if model.party.loot_method.is_empty() {
                "freeforall"
            } else {
                model.party.loot_method.as_str()
            };
            let master = match model.party.master_looter {
                Some(idx) => Value::Integer(i64::from(idx)),
                None => Value::Nil,
            };
            Ok((Value::String(lua.create_string(method)?), master))
        })?,
    )?;

    // GetLootThreshold() → the loot quality threshold.
    g.set(
        "GetLootThreshold",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.party.loot_threshold))
        })?,
    )?;

    // GetOptOutOfLoot() (`0x52c2d0`): 1 while the client's opt-out flag `[0xbd19a0]` is set,
    // else nil. Only `SetOptOutOfLoot` sets it, which is not built, so it reads nil.
    g.set(
        "GetOptOutOfLoot",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // The party verbs queue a `PartyRequest` and return nothing.
    g.set(
        "AcceptGroup",
        lua.create_function(|lua, args: MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let mut roles = 0;
            for (i, bit) in [(0, 2), (1, 4), (2, 8)] {
                if bool_or_default(args.get(i), false) {
                    roles |= bit;
                }
            }
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::Accept { roles });
            Ok(())
        })?,
    )?;
    g.set(
        "DeclineGroup",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::Decline);
            Ok(())
        })?,
    )?;
    g.set(
        "LeaveParty",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::Leave);
            Ok(())
        })?,
    )?;
    // InviteUnit (`0x51a6e0`): no name sends nothing, past 48 bytes raises.
    g.set(
        "InviteUnit",
        lua.create_function(|lua, args: MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let Some(name) = args.first().and_then(|v| optional_string(lua, v)) else {
                return Ok(());
            };
            if name.len() > 48 {
                return Err(mlua::Error::runtime("Name too long"));
            }
            let mut flags = 0;
            for (i, bit) in [(1, 2), (2, 4), (3, 8)] {
                if bool_or_default(args.get(i), false) {
                    flags |= bit;
                }
            }
            let mut model = crate::script::model_mut_of(lua);
            model
                .party_requests
                .push(PartyRequest::InviteUnit { name, flags });
            Ok(())
        })?,
    )?;
    // UninviteUnit (`0x51a7a0`): the reason cut to 64 characters.
    g.set(
        "UninviteUnit",
        lua.create_function(|lua, (unit, reason): (Value, Value)| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let reason: String = optional_string(lua, &reason)
                .unwrap_or_default()
                .chars()
                .take(64)
                .collect();
            let mut model = crate::script::model_mut_of(lua);
            model
                .party_requests
                .push(PartyRequest::UninviteUnit { unit, reason });
            Ok(())
        })?,
    )?;
    // PromoteToLeader(unit [, exact]) (`0x573e10`): a non-string raises the usage error.
    g.set(
        "PromoteToLeader",
        lua.create_function(|lua, unit: Value| {
            let Some(unit) = optional_string(lua, &unit) else {
                return Err(mlua::Error::runtime("Usage: PromoteToLeader(name)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            model
                .party_requests
                .push(PartyRequest::PromoteToLeader(unit));
            Ok(())
        })?,
    )?;
    // SetLootMethod("method" [,master] [,threshold]), the reference's usage string.
    g.set(
        "SetLootMethod",
        lua.create_function(
            |lua, (method, master_name, threshold): (String, Option<String>, Option<u32>)| {
                let mut model = crate::script::model_mut_of(lua);
                model.party_requests.push(PartyRequest::LootMethod {
                    method,
                    master_name,
                    threshold,
                });
                Ok(())
            },
        )?,
    )?;
    g.set(
        "SetLootThreshold",
        lua.create_function(|lua, n: u32| {
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::LootThreshold(n));
            Ok(())
        })?,
    )?;
    // SetRaidTarget(unit, index) is the engine verb. `SetRaidTargetIcon` is FrameXML's toggle
    // around it (`TargetFrame.lua:486`), and `TargetFrame.lua:488`, `:490` and `Bindings.xml:1160`
    // call it bare.
    g.set(
        "SetRaidTarget",
        lua.create_function(|lua, (unit, index): (String, u8)| {
            let mut model = crate::script::model_mut_of(lua);
            model
                .party_requests
                .push(PartyRequest::SetRaidTarget { unit, index });
            Ok(())
        })?,
    )?;

    // IsRaidOfficer() (`0x573b00`) → 1 when our own raid row holds a rank, else nil.
    g.set(
        "IsRaidOfficer",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let own = model.party.own_guid;
            let officer = own != 0 && model.party.raid.iter().any(|m| m.guid == own && m.rank > 0);
            Ok(if officer {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // ── The raid-management verbs ────────────────────────────────────────────
    // Each only marshals its arguments into a client call or a send (`PromoteToAssistant`
    // `0x573ef0`, `RequestRaidInfo` `0x500be0`), so the verbs here queue a request.
    g.set(
        "ConvertToRaid",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::ConvertToRaid);
            Ok(())
        })?,
    )?;
    // SetRaidSubgroup(index, group) / SwapRaidSubgroup(index, other), the drag's two drops: a
    // non-number raises the usage error rather than queueing a zero.
    g.set(
        "SetRaidSubgroup",
        lua.create_function(|lua, (index, group): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: SetRaidSubgroup(index, group)")?;
            let group = number_arg(lua, group, "Usage: SetRaidSubgroup(index, group)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::SetSubgroup {
                index: index.max(0) as u32,
                group: group.max(0) as u32,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "SwapRaidSubgroup",
        lua.create_function(|lua, (index, other): (Value, Value)| {
            let index = number_arg(lua, index, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let other = number_arg(lua, other, "Usage: SwapRaidSubgroup(index1, index2)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::SwapSubgroup {
                index: index.max(0) as u32,
                other: other.max(0) as u32,
            });
            Ok(())
        })?,
    )?;
    for (binding, grant) in [("PromoteToAssistant", true), ("DemoteAssistant", false)] {
        g.set(
            binding,
            lua.create_function(move |lua, name: String| {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .party_requests
                    .push(PartyRequest::AssistantLeader { name, grant });
                Ok(())
            })?,
        )?;
    }
    g.set(
        "DoReadyCheck",
        lua.create_function(|lua, ()| {
            let now = clock(lua);
            let mut model = crate::script::model_mut_of(lua);
            // The worker flags every roster member but the caller as pending and arms the 30 s
            // deadline, then sends. A party has no raid roster, so nothing is flagged and the
            // leader's echo closes it.
            let own = model.party.own_guid;
            model.ready_check.unanswered = model
                .party
                .raid
                .iter()
                .map(|m| m.guid)
                .filter(|g| *g != own)
                .collect();
            model.ready_check.deadline = Some(now + READY_CHECK_SECONDS);
            model.ready_check.ready.clear();
            model.ready_check.initiator = own;
            model.party_requests.push(PartyRequest::ReadyCheckStart);
            Ok(())
        })?,
    )?;
    // GetReadyCheckStatus(unit) (`0x574180`): nil unless a check is armed and unexpired and the
    // player leads the party (`0x57426e`) or holds raid rank (`0x5741d1`); then the initiator reads
    // "ready", a pending member "waiting", one who answered ready "ready", the rest "notready". In
    // a party the player, who is no `partyN` slot, reads only as the initiator (`0x5742d1`).
    g.set(
        "GetReadyCheckStatus",
        lua.create_function(|lua, token: Option<String>| {
            let now = clock(lua);
            let model = crate::script::model_mut_of(lua);
            let guid = token
                .as_deref()
                .and_then(|t| model.unit(t))
                .map_or(0, |u| u.guid);
            let party = &model.party;
            let check = &model.ready_check;
            let armed = check.deadline.is_some_and(|d| now < d);
            let status = if party.raid.is_empty() {
                let leads = !party.members.is_empty() && party.leader_guid == party.own_guid;
                let member = party.members.iter().any(|m| m.guid != 0 && m.guid == guid);
                match (leads && armed, member) {
                    (false, _) => None,
                    (true, false) => (guid != 0 && guid == check.initiator).then_some("ready"),
                    (true, true) => Some(ready_check_answer(check, guid)),
                }
            } else {
                let officer = party
                    .raid
                    .iter()
                    .any(|m| m.guid == party.own_guid && m.rank > 0);
                let member = party.raid.iter().any(|m| m.guid != 0 && m.guid == guid);
                (officer && armed && member).then(|| ready_check_answer(check, guid))
            };
            Ok(status)
        })?,
    )?;
    // CheckReadyCheckTime(), called every frame from stock `UIParent.xml`'s `OnUpdate`, alone
    // drives the 30 s expiry. Once an armed deadline passes, and only on the leader's client, it
    // disarms and prints `RAID_MEMBERS_AFK` with the pending names joined by `", "`, or
    // `READY_CHECK_NO_AFK`, as a `CHAT_MSG_SYSTEM` line. Both strings are read raw from `_G` with
    // an empty default, not through `GetText`; nothing is sent.
    g.set(
        "CheckReadyCheckTime",
        lua.create_function(|lua, ()| {
            let now = clock(lua);
            let mut model = crate::script::model_mut_of(lua);
            ready_check_tick(lua, &mut model, now);
            Ok(())
        })?,
    )?;
    // ConfirmReadyCheck(ready) reads its argument as truth: the reference's No button passes no
    // argument, Yes passes `1`.
    g.set(
        "ConfirmReadyCheck",
        lua.create_function(|lua, ready: Value| {
            let ready = !matches!(ready, Value::Nil | Value::Boolean(false));
            let mut model = crate::script::model_mut_of(lua);
            model
                .party_requests
                .push(PartyRequest::ReadyCheckAnswer(ready));
            Ok(())
        })?,
    )?;
    g.set(
        "RequestRaidInfo",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.party_requests.push(PartyRequest::RequestRaidInfo);
            Ok(())
        })?,
    )?;

    // GetNumSavedInstances() (`0x4fca50`) / GetSavedInstanceInfo(index) (`0x4ff2c0`): ten values;
    // past the list or with no `Map.dbc` row, nil, 0, nil, 0, false, false, 0, false, 0, "".
    g.set(
        "GetNumSavedInstances",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.saved_instances.len() as i64)
        })?,
    )?;
    g.set(
        "GetSavedInstanceInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetSavedInstanceInfo(index)")?;
            let row = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                usize::try_from(index.wrapping_sub(1))
                    .ok()
                    .and_then(|i| model.saved_instances.get(i))
                    .filter(|r| r.name.is_some())
                    .cloned()
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                    Value::Boolean(false),
                    Value::Boolean(false),
                    Value::Integer(0),
                    Value::Boolean(false),
                    Value::Integer(0),
                    Value::String(lua.create_string("")?),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(r.name.as_deref().unwrap_or_default())?),
                Value::Integer((r.instance & 0xFFFF_FFFF) as i64),
                Value::Integer(i64::from(r.reset)),
                Value::Integer(i64::from(r.difficulty) + 1),
                Value::Boolean(r.locked),
                Value::Boolean(r.extended),
                Value::Integer((r.instance >> 32) as i64),
                Value::Boolean(r.is_raid),
                Value::Integer(i64::from(r.max_players)),
                Value::String(lua.create_string(&r.difficulty_name)?),
            ]))
        })?,
    )?;
    // SetSavedInstanceExtend(index, extend) (`0x50dc10`): stores the flag, prints the lock line
    // as system chat and sends the request; an index past the list does nothing.
    g.set(
        "SetSavedInstanceExtend",
        lua.create_function(|lua, (index, extend): (Value, Value)| {
            const USAGE: &str = "Usage: SetSavedInstanceExtend(index, extend)";
            let Value::Boolean(extend) = extend else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let index = number_arg(lua, index, USAGE)?;
            let template = super::get_text::global_text(
                lua,
                if extend {
                    "RAID_INSTANCE_LOCK_EXTENDED"
                } else {
                    "RAID_INSTANCE_LOCK_NOT_EXTENDED"
                },
            );
            let with_difficulty = super::get_text::global_text(lua, "DUNGEON_NAME_WITH_DIFFICULTY");
            let mut model = crate::script::model_mut_of(lua);
            let Some(row) = usize::try_from(index.wrapping_sub(1))
                .ok()
                .and_then(|i| model.saved_instances.get_mut(i))
            else {
                return Ok(());
            };
            row.extended = extend;
            // The map name with difficulty (`0x4fe070`): "Name (difficulty)" when the row has one.
            let name = row.name.clone().unwrap_or_default();
            let shown = if row.difficulty_name.is_empty() {
                name
            } else {
                crate::strings::fill(
                    &with_difficulty,
                    &[
                        crate::strings::Arg::S(&name),
                        crate::strings::Arg::S(&row.difficulty_name),
                    ],
                )
            };
            let request = PartyRequest::SavedInstanceExtend {
                map: row.map,
                difficulty: row.difficulty,
                extend,
            };
            let line = crate::strings::fill(&template, &[crate::strings::Arg::S(&shown)]);
            model.party_lines.push(line);
            model.party_requests.push(request);
            Ok(())
        })?,
    )?;

    // SetRaidRosterSelection(index) / GetRaidRosterSelection(): a client-side cursor; the
    // reference's writes a global and sends nothing.
    g.set(
        "SetRaidRosterSelection",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetRaidRosterSelection(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.raid_selection = i64::from(index);
            Ok(())
        })?,
    )?;
    g.set(
        "GetRaidRosterSelection",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.raid_selection)
        })?,
    )?;

    Ok(())
}

/// The ready-check deadline, `0x7530` ms after the arm.
const READY_CHECK_SECONDS: f64 = 30.0;

/// The ready-check state: the armed deadline, the members whose pending flag (`[entry+0x158]`) is
/// set, and summary lines not yet pushed to chat. Outside [`PartyState`] because a roster push
/// replaces that and this survives it.
#[derive(Debug, Default)]
pub(crate) struct ReadyCheckState {
    pub(crate) deadline: Option<f64>,
    pub(crate) unanswered: Vec<u64>,
    /// The members whose answer was ready (`[entry+0x324]`; a party slot's `0x572470`), which `GetReadyCheckStatus` reads.
    pub(crate) ready: Vec<u64>,
    /// Who started the check (`[0xbeb620]`): `GetReadyCheckStatus` answers `"ready"` for them.
    pub(crate) initiator: u64,
    pub(crate) lines: Vec<String>,
}

/// A rostered member's `GetReadyCheckStatus` answer in an armed check (`0x574216`-`0x574233`).
fn ready_check_answer(check: &ReadyCheckState, guid: u64) -> &'static str {
    if guid == check.initiator || check.ready.contains(&guid) {
        "ready"
    } else if check.unanswered.contains(&guid) {
        "waiting"
    } else {
        "notready"
    }
}

/// `GetTime()`'s clock, the session seconds `UiScript::tick` advances.
fn clock(lua: &Lua) -> f64 {
    crate::script::clock::now(lua)
}

/// The timeout worker.
fn ready_check_tick(lua: &Lua, model: &mut Model, now: f64) {
    let Some(deadline) = model.ready_check.deadline else {
        return;
    };
    if now < deadline {
        return;
    }
    if model.party.own_guid != model.party.leader_guid {
        return;
    }
    model.ready_check.deadline = None;
    // A pending member the roster cannot name is skipped (its query arm off).
    let names: Vec<&str> = model
        .ready_check
        .unanswered
        .iter()
        .filter_map(|g| model.party.raid.iter().find(|m| m.guid == *g))
        .map(|m| m.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    let raw = |key: &str| lua.globals().get::<String>(key).unwrap_or_default();
    let text = if names.is_empty() {
        raw("READY_CHECK_NO_AFK")
    } else {
        raw("RAID_MEMBERS_AFK").replacen("%s", &names.join(", "), 1)
    };
    model.ready_check.lines.push(text);
}

/// The `0x322` handler's close test: a member holds the check open only while pending and online
/// (`[entry+0x18]` bit 0, `SMSG_GROUP_LIST`'s online bit). An empty roster closes at once.
fn ready_check_pending_online(model: &Model) -> bool {
    model
        .ready_check
        .unanswered
        .iter()
        .any(|g| model.party.raid.iter().any(|m| m.guid == *g && m.online))
}

/// The force-close sites: an armed deadline becomes `now - 1` and the worker runs at once.
fn ready_check_force_close(lua: &Lua, model: &mut Model) {
    if model.ready_check.deadline.is_none() {
        return;
    }
    let now = model.now;
    model.ready_check.deadline = Some(now - 1.0);
    ready_check_tick(lua, model, now);
}

#[cfg(test)]
mod tests {
    use crate::script::{PartyMemberInfo, PartyRequest, PartyState, UiScript};

    fn two_member_party() -> PartyState {
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 1, // Alice (party1) leads
            leader_guid: 0xA11CE,
            own_guid: 0x5E1F,
            raid: Vec::new(),
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
            real_party_members: 0,
            real_raid_members: 0,
            loot_master_guid: 0,
            member_assignments: [0; 4],
        }
    }

    #[test]
    fn read_natives_report_the_pushed_roster() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());

        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetPartyMember(1)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPartyMember(2)").unwrap(), 1);
        assert!(s.eval::<bool>("return GetPartyMember(3) == nil").unwrap());
        assert!(s.eval::<bool>("return GetPartyMember(0) == nil").unwrap());
        assert_eq!(s.eval::<i64>("return GetPartyLeaderIndex()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        assert_eq!(method, "group");
        assert_eq!(master, None);
        assert_eq!(s.eval::<i64>("return GetLootThreshold()").unwrap(), 2);
    }

    #[test]
    fn is_party_leader_reports_when_the_player_leads() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.leader_index = 0; // the player leads
        s.set_party(party);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);
    }

    #[test]
    fn get_loot_method_reports_the_assigned_master() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.loot_method = "master".into();
        party.master_looter = Some(2); // Bob (party2)
        s.set_party(party);
        let (method, master) = s.eval::<(String, i64)>("return GetLootMethod()").unwrap();
        assert_eq!(method, "master");
        assert_eq!(master, 2);
    }

    #[test]
    fn empty_state_reports_the_solo_player_shape() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumPartyMembers()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetPartyMember(1) == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());
        let (method, master) = s
            .eval::<(String, Option<i64>)>("return GetLootMethod()")
            .unwrap();
        // Never grouped reads method 0, `freeforall`; the `group` above is a real party's.
        assert_eq!(method, "freeforall");
        assert_eq!(master, None);
    }

    // ── The raid trio (`UnitInRaid`, `GetRaidRosterInfo`, `IsRaidLeader`) ────────────────────

    fn raider(name: &str, guid: u64) -> crate::script::RaidMemberInfo {
        crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            rank: 0,
            subgroup: 0,
            level: 60,
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            zone: Some("Molten Core".into()),
            online: true,
            ninth: false,
            assignments: 0,
            roles: 0,
        }
    }

    fn ten_player_raid() -> PartyState {
        let mut raid: Vec<crate::script::RaidMemberInfo> = (0u64..10)
            .map(|i| raider(&format!("Raider{i}"), 0x100 + i))
            .collect();
        raid[0].rank = 2; // the leader
        raid[0].name = "Me".into();
        raid[3].subgroup = 1; // stored 0-based, exposed 2
        PartyState {
            raid,
            ..Default::default()
        }
    }

    /// Every return site in `0x573690` pushes fourteen, the miss arm included.
    #[test]
    fn get_raid_roster_info_returns_fourteen_values_on_every_path() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());

        for index in ["1", "10", "0", "-1", "41", "9999"] {
            assert_eq!(
                s.arity(&format!("GetRaidRosterInfo({index})")).unwrap(),
                14,
                "index {index} must still be a fourteen-value answer"
            );
        }
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetRaidRosterInfo(1)").unwrap(), 14);
    }

    /// The miss arm pushes `nil, 0, 1, 1` and ten nils (`0x573690`).
    #[test]
    fn get_raid_roster_info_misses_with_the_fixed_tuple() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let miss = "local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(99) \
                    return a == nil, b, c, d, e == nil, f == nil, g == nil, h == nil, i == nil";
        let (a, b, c, d, e, f, g, h, i): (bool, i64, i64, i64, bool, bool, bool, bool, bool) =
            s.eval(miss).unwrap();
        assert!(a, "name is nil");
        assert_eq!(
            (b, c, d),
            (0, 1, 1),
            "rank 0, subgroup 1, level 1 — numbers"
        );
        assert!(e && f && g && h && i, "the last five are nil");

        // An in-range member with no cached name takes the same arm (`0x573690`).
        let mut party = ten_player_raid();
        party.raid[1].name = String::new();
        s.set_party(party);
        let (name_nil, rank): (bool, i64) = s
            .eval("local a,b = GetRaidRosterInfo(2) return a == nil, b")
            .unwrap();
        assert!(name_nil && rank == 0, "an uncached name is a full miss");
    }

    #[test]
    fn get_raid_roster_info_reports_the_pushed_row() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        let (name, rank, subgroup, level, class, file, zone, online, ninth): (
            String,
            i64,
            i64,
            i64,
            String,
            String,
            String,
            i64,
            Option<i64>,
        ) = s
            .eval("local a,b,c,d,e,f,g,h,i = GetRaidRosterInfo(1) return a,b,c,d,e,f,g,h,i")
            .unwrap();
        assert_eq!(name, "Me");
        assert_eq!(rank, 2, "as stored, no adjustment");
        assert_eq!(subgroup, 1, "stored 0 → exposed 1");
        assert_eq!(level, 60);
        assert_eq!((class.as_str(), file.as_str()), ("Warrior", "WARRIOR"));
        assert_eq!(zone, "Molten Core");
        assert_eq!(online, 1);
        assert_eq!(ninth, None, "1/nil, never true/false");
        // Subgroup is the one adjusted field: member 4 is stored in subgroup 1 and reads 2.
        assert_eq!(
            s.eval::<i64>("local _,_,g = GetRaidRosterInfo(4) return g")
                .unwrap(),
            2
        );
    }

    /// Return 8 goes nil on the same arm (`0x573690`).
    #[test]
    fn get_raid_roster_info_puts_player_offline_in_the_zone_slot() {
        let mut s = UiScript::new().unwrap();
        let mut party = ten_player_raid();
        party.raid[1].online = false;
        party.raid[1].zone = Some("Molten Core".into()); // ignored on the offline arm
        s.set_party(party);
        // The binding reads whatever `PLAYER_OFFLINE` the VM holds.
        s.run(r#"PLAYER_OFFLINE = "Offline""#).unwrap();
        let (zone, online): (String, Option<i64>) = s
            .eval("local _,_,_,_,_,_,g,h = GetRaidRosterInfo(2) return g, h")
            .unwrap();
        assert_eq!(zone, "Offline");
        assert_eq!(online, None);
    }

    /// A non-number raises the usage error (`0x573690`).
    #[test]
    fn get_raid_roster_info_raises_only_on_a_non_number() {
        let s = UiScript::new().unwrap();
        let err = s.arity("GetRaidRosterInfo({})").unwrap_err();
        assert!(
            format!("{err}").contains("Usage: GetRaidRosterInfo(index)"),
            "got {err}"
        );
        // A numeric string coerces (`lua_isnumber`) and does not raise.
        assert_eq!(s.arity(r#"GetRaidRosterInfo("3")"#).unwrap(), 14);
    }

    /// The 1 is the constant double.
    #[test]
    fn unit_in_raid_answers_one_not_an_index() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        s.set_unit("player", Some(unit(true, 0x100)));
        s.set_unit("target", Some(unit(true, 0x105))); // roster row 6
        s.set_unit("mouseover", Some(unit(true, 0xDEAD)));

        assert_eq!(s.eval::<i64>(r#"return UnitInRaid("player")"#).unwrap(), 1);
        assert_eq!(
            s.eval::<i64>(r#"return UnitInRaid("target")"#).unwrap(),
            1,
            "row 6 still answers 1 — this is not an index"
        );
        assert!(s
            .eval::<bool>(r#"return UnitInRaid("mouseover") == nil"#)
            .unwrap());
        // Nil for a missing, wrong-typed or unmatched token: no guid, and `UnitInRaid` (`0x60ca20`)
        // rejects guid 0.
        for call in ["UnitInRaid()", "UnitInRaid(nil)", r#"UnitInRaid("party3")"#] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call} must answer nil"
            );
        }
        // A token matching no prefix names nobody, a number coerced to a string first.
        for call in ["UnitInRaid(7)", r#"UnitInRaid("nosuchtoken")"#] {
            assert!(
                s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
                "{call}"
            );
        }
    }

    /// `IsRaidLeader` (`0x573a60`) reads the same leader guid as `IsPartyLeader` (`0x52ccd0`).
    #[test]
    fn is_raid_leader_is_true_for_a_party_leader() {
        let mut s = UiScript::new().unwrap();
        let mut party = two_member_party();
        party.leader_index = 0; // we lead a party, with no raid roster
        s.set_party(party);
        assert!(
            s.eval::<i64>("return GetNumRaidMembers()").unwrap() == 0,
            "no raid, and that is exactly the point"
        );
        assert_eq!(s.eval::<i64>("return IsRaidLeader()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return IsPartyLeader()").unwrap(), 1);

        s.set_party(two_member_party()); // leader_index 1 = Alice
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
        assert!(s.eval::<bool>("return IsPartyLeader() == nil").unwrap());

        // Ungrouped: the cached leader guid is 0 and matches nobody.
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return IsRaidLeader() == nil").unwrap());
    }

    #[test]
    fn get_num_raid_members_bounds_the_roster_it_indexes() {
        let mut s = UiScript::new().unwrap();
        s.set_party(ten_player_raid());
        assert_eq!(s.eval::<i64>("return GetNumRaidMembers()").unwrap(), 10);
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers()) ~= nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetRaidRosterInfo(GetNumRaidMembers() + 1) == nil")
            .unwrap());
    }

    #[test]
    fn intent_natives_queue_the_exact_request_sequence() {
        let mut s = UiScript::new().unwrap();
        assert!(s.take_party_requests().is_empty());

        s.run("AcceptGroup()").unwrap();
        s.run("AcceptGroup(nil, 1, 1)").unwrap();
        s.run("DeclineGroup()").unwrap();
        s.run("LeaveParty()").unwrap();
        s.run(r#"InviteUnit("Bob")"#).unwrap();
        s.run(r#"InviteUnit("Bob", 1, nil, "1")"#).unwrap();
        s.run(r#"UninviteUnit("party2")"#).unwrap();
        s.run(r#"UninviteUnit("Bob", "afk")"#).unwrap();
        s.run(r#"PromoteToLeader("party2", 1)"#).unwrap();
        s.run(r#"SetLootMethod("master", "Bob")"#).unwrap();
        s.run(r#"SetLootThreshold(3)"#).unwrap();

        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::Accept { roles: 0 },
                PartyRequest::Accept { roles: 4 | 8 },
                PartyRequest::Decline,
                PartyRequest::Leave,
                PartyRequest::InviteUnit {
                    name: "Bob".into(),
                    flags: 0
                },
                PartyRequest::InviteUnit {
                    name: "Bob".into(),
                    flags: 2 | 8
                },
                PartyRequest::UninviteUnit {
                    unit: "party2".into(),
                    reason: String::new()
                },
                PartyRequest::UninviteUnit {
                    unit: "Bob".into(),
                    reason: "afk".into()
                },
                PartyRequest::PromoteToLeader("party2".into()),
                PartyRequest::LootMethod {
                    method: "master".into(),
                    master_name: Some("Bob".into()),
                    threshold: None,
                },
                PartyRequest::LootThreshold(3),
            ]
        );
        assert!(s.take_party_requests().is_empty());
    }

    #[test]
    fn set_loot_method_without_a_master_name_queues_none() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SetLootMethod("freeforall")"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::LootMethod {
                method: "freeforall".into(),
                master_name: None,
                threshold: None,
            }]
        );

        // The third argument is read for every method.
        s.run(r#"SetLootMethod("group", nil, 4)"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::LootMethod {
                method: "group".into(),
                master_name: None,
                threshold: Some(4),
            }]
        );
    }

    #[test]
    fn set_raid_target_icon_queues_the_token_and_index() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SetRaidTarget("target", 8)"#).unwrap();
        s.run(r#"SetRaidTarget("party2", 0)"#).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::SetRaidTarget {
                    unit: "target".into(),
                    index: 8,
                },
                PartyRequest::SetRaidTarget {
                    unit: "party2".into(),
                    index: 0,
                },
            ]
        );
    }

    /// `IsRaidOfficer` (`0x573b00`): our own raid row's rank above 0, nil outside a raid.
    #[test]
    fn is_raid_officer_reads_our_own_rows_rank() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return IsRaidOfficer() == nil").unwrap());
        let mut state = ten_player_raid();
        state.own_guid = state.raid[1].guid;
        s.set_party(state.clone());
        assert!(
            s.eval::<bool>("return IsRaidOfficer() == nil").unwrap(),
            "a plain member"
        );
        state.raid[1].rank = 1;
        s.set_party(state.clone());
        assert_eq!(
            s.eval::<i64>("return IsRaidOfficer()").unwrap(),
            1,
            "an assistant"
        );
        state.own_guid = state.raid[0].guid;
        s.set_party(state);
        assert_eq!(
            s.eval::<i64>("return IsRaidOfficer()").unwrap(),
            1,
            "the leader"
        );
    }

    // ── The identity predicates ─────────────────────────────────────────────

    fn unit(exists: bool, guid: u64) -> crate::script::UnitState {
        crate::script::UnitState {
            exists,
            guid,
            ..Default::default()
        }
    }

    #[test]
    fn unit_is_unit_compares_guids_and_tokens() {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(unit(true, 0x10)));
        s.set_unit("target", Some(unit(true, 0x10)));
        s.set_unit("party1", Some(unit(true, 0x20)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("target", "player")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsUnit("player", "player")"#)
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("party1", "player") == nil"#)
            .unwrap());
        // Zero guids never match across tokens.
        s.set_unit("target", Some(unit(true, 0)));
        s.set_unit("mouseover", Some(unit(true, 0)));
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("target", "mouseover") == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return UnitIsUnit("pet", "player") == nil"#)
            .unwrap());
    }

    /// A member's own guid, or a unit whose owner is a member: the party's for
    /// `UnitPlayerOrPetInParty`, the raid roster's for `UnitPlayerOrPetInRaid`.
    #[test]
    fn unit_player_or_pet_in_party_reads_the_owner() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());
        s.set_unit("player", Some(unit(true, 0x10)));
        let mut alices_pet = unit(true, 0xC0FFEE);
        alices_pet.owner = 0xA11CE;
        s.set_unit("target", Some(alices_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1,
            "Alice's pet is in the party by its owner"
        );
        assert!(
            s.eval::<bool>(r#"return UnitPlayerOrPetInRaid("target") == nil"#)
                .unwrap(),
            "no raid: the raid twin says nil"
        );
        let mut strangers_pet = unit(true, 0xC0FFEE);
        strangers_pet.owner = 0xDEAD;
        s.set_unit("target", Some(strangers_pet));
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
        // A member's own guid still answers, as UnitInParty does.
        s.set_unit("target", Some(unit(true, 0xB0B)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInParty("target")"#)
                .unwrap(),
            1
        );
        s.set_party(ten_player_raid());
        let mut raiders_pet = unit(true, 0xC0FFEE);
        raiders_pet.owner = ten_player_raid().raid[0].guid;
        s.set_unit("target", Some(raiders_pet));
        assert_eq!(
            s.eval::<i64>(r#"return UnitPlayerOrPetInRaid("target")"#)
                .unwrap(),
            1
        );
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>(r#"return UnitPlayerOrPetInParty("target") == nil"#)
            .unwrap());
    }

    #[test]
    fn unit_in_party_matches_roster_guids() {
        let mut s = UiScript::new().unwrap();
        s.set_party(two_member_party());
        s.set_unit("player", Some(unit(true, 0x10)));
        s.set_unit("party1", Some(unit(true, 0xA11CE)));
        s.set_unit("target", Some(unit(true, 0xA11CE)));
        assert_eq!(s.eval::<i64>(r#"return UnitInParty("target")"#).unwrap(), 1);
        assert_eq!(s.eval::<i64>(r#"return UnitInParty("party1")"#).unwrap(), 1);
        s.set_unit("target", Some(unit(true, 0xDEAD)));
        assert!(s
            .eval::<bool>(r#"return UnitInParty("target") == nil"#)
            .unwrap());
        // Ungrouped: everything is nil, the player included.
        s.set_party(crate::script::PartyState::default());
        assert!(s
            .eval::<bool>(r#"return UnitInParty("player") == nil"#)
            .unwrap());
    }

    #[test]
    fn unit_can_cooperate_needs_a_friendly_player() {
        let mut s = UiScript::new().unwrap();
        let mut friendly = unit(true, 0x30);
        friendly.is_player = true;
        friendly.reaction = 5;
        s.set_unit("target", Some(friendly.clone()));
        assert_eq!(
            s.eval::<i64>(r#"return UnitCanCooperate("player", "target")"#)
                .unwrap(),
            1
        );
        let mut hostile = friendly.clone();
        hostile.reaction = 2;
        s.set_unit("target", Some(hostile));
        assert!(s
            .eval::<bool>(r#"return UnitCanCooperate("player", "target") == nil"#)
            .unwrap());
        let mut npc = friendly;
        npc.is_player = false;
        s.set_unit("target", Some(npc));
        assert!(s
            .eval::<bool>(r#"return UnitCanCooperate("player", "target") == nil"#)
            .unwrap());
    }

    /// The refusals of `InviteUnit` (`0x51a6e0`) and `PromoteToLeader` (`0x573e10`).
    #[test]
    fn the_party_verbs_refuse_what_12340_refuses() {
        let mut s = UiScript::new().unwrap();
        s.run("InviteUnit() InviteUnit(nil, 1)").unwrap();
        assert!(s.run(&format!("InviteUnit('{}')", "a".repeat(49))).is_err());
        assert!(s.run("PromoteToLeader()").is_err());
        s.run(&format!("UninviteUnit('', '{}')", "r".repeat(70)))
            .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::UninviteUnit {
                unit: String::new(),
                reason: "r".repeat(64)
            }]
        );
        s.run(&format!("InviteUnit('{}')", "a".repeat(48))).unwrap();
        assert_eq!(s.take_party_requests().len(), 1, "48 bytes is allowed");
    }

    #[test]
    fn the_raid_verbs_queue_what_they_name() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            ConvertToRaid()
            SetRaidSubgroup(7, 3)
            SwapRaidSubgroup(7, 12)
            PromoteToLeader("Alice")
            PromoteToAssistant("Bob")
            DemoteAssistant("Bob")
            DoReadyCheck()
            RequestRaidInfo()
            "#,
        )
        .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ConvertToRaid,
                PartyRequest::SetSubgroup { index: 7, group: 3 },
                PartyRequest::SwapSubgroup {
                    index: 7,
                    other: 12
                },
                PartyRequest::PromoteToLeader("Alice".into()),
                PartyRequest::AssistantLeader {
                    name: "Bob".into(),
                    grant: true
                },
                PartyRequest::AssistantLeader {
                    name: "Bob".into(),
                    grant: false
                },
                PartyRequest::ReadyCheckStart,
                PartyRequest::RequestRaidInfo,
            ]
        );
        assert!(s.take_party_requests().is_empty(), "the drain empties");
    }

    /// The reference's No button passes no argument, Yes passes `1`.
    #[test]
    fn confirm_ready_check_treats_an_absent_argument_as_not_ready() {
        let mut s = UiScript::new().unwrap();
        s.run("ConfirmReadyCheck(1) ConfirmReadyCheck() ConfirmReadyCheck(false) ConfirmReadyCheck(0)")
            .unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![
                PartyRequest::ReadyCheckAnswer(true),
                PartyRequest::ReadyCheckAnswer(false),
                PartyRequest::ReadyCheckAnswer(false),
                // `0` is true in Lua.
                PartyRequest::ReadyCheckAnswer(true),
            ]
        );
    }

    fn lockout(
        map: u32,
        name: &str,
        instance: u64,
        difficulty: u32,
    ) -> crate::script::SavedInstanceInfo {
        crate::script::SavedInstanceInfo {
            map,
            name: Some(name.into()),
            instance,
            reset: 86_400,
            difficulty,
            locked: true,
            extended: false,
            is_raid: true,
            max_players: 25,
            difficulty_name: "25 Player".into(),
        }
    }

    /// Ten values (`0x4ff2c0`): the instance id in two halves, the difficulty 1-based.
    #[test]
    fn saved_instance_info_answers_ten_values() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 0);
        s.set_saved_instances(vec![
            lockout(533, "Naxxramas", 0x2_0000_04d2, 1),
            crate::script::SavedInstanceInfo {
                name: None,
                ..lockout(9999, "", 7, 0)
            },
        ]);
        assert_eq!(s.eval::<i64>("return GetNumSavedInstances()").unwrap(), 2);
        assert_eq!(s.arity("GetSavedInstanceInfo(1)").unwrap(), 10);
        assert!(s
            .eval::<bool>(
                "local n, id, reset, diff, locked, ext, hi, raid, max, dname = GetSavedInstanceInfo(1)                  return n == 'Naxxramas' and id == 1234 and reset == 86400 and diff == 2                  and locked == true and ext == false and hi == 2 and raid == true and max == 25                  and dname == '25 Player'"
            )
            .unwrap());
        // Past the list, or a map with no row.
        for miss in ["0", "2", "3", "-1"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local n, id, reset, diff, locked, ext, hi, raid, max, dname = GetSavedInstanceInfo({miss})                      return select('#', GetSavedInstanceInfo({miss})) == 10 and n == nil and id == 0                      and reset == nil and diff == 0 and locked == false and ext == false and hi == 0                      and raid == false and max == 0 and dname == ''"
                ))
                .unwrap(),
                "index {miss} is a miss"
            );
        }
        let err = s
            .run(r#"GetSavedInstanceInfo("x")"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Usage: GetSavedInstanceInfo(index)"), "{err}");
    }

    /// `SetSavedInstanceExtend` (`0x50dc10`) flips the flag, prints the lock line with the
    /// difficulty and queues the request; a non-boolean raises.
    #[test]
    fn set_saved_instance_extend_stores_prints_and_sends() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"RAID_INSTANCE_LOCK_EXTENDED = "Your instance lock for %s has been extended."
               DUNGEON_NAME_WITH_DIFFICULTY = "%1$s (%2$s)""#,
        )
        .unwrap();
        s.set_saved_instances(vec![lockout(533, "Naxxramas", 1234, 1)]);
        s.run("SetSavedInstanceExtend(1, true) SetSavedInstanceExtend(5, true)")
            .unwrap();
        assert!(s
            .eval::<bool>("return select(6, GetSavedInstanceInfo(1)) == true")
            .unwrap());
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::SavedInstanceExtend {
                map: 533,
                difficulty: 1,
                extend: true
            }]
        );
        assert_eq!(
            s.take_party_lines(),
            vec!["Your instance lock for Naxxramas (25 Player) has been extended.".to_string()]
        );
        let err = s
            .run("SetSavedInstanceExtend(1, 1)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: SetSavedInstanceExtend(index, extend)"),
            "{err}"
        );
    }

    /// Fourteen values (`0x573690`): role, master looter and the three LFG roles after isDead.
    #[test]
    fn raid_roster_info_answers_fourteen_values() {
        let mut s = UiScript::new().unwrap();
        let mut state = ten_player_raid();
        state.raid[1].assignments = super::ASSIGNMENT_MAIN_TANK | super::ASSIGNMENT_MAIN_ASSIST;
        state.raid[1].roles = 0x2 | 0x8;
        state.raid[2].assignments = super::ASSIGNMENT_MAIN_ASSIST;
        state.loot_master_guid = state.raid[2].guid;
        s.set_party(state);
        assert_eq!(s.arity("GetRaidRosterInfo(2)").unwrap(), 14);
        assert!(s
            .eval::<bool>(
                "local r = {GetRaidRosterInfo(2)}                  return r[10] == 'MAINTANK' and r[11] == nil and r[12] == 1 and r[13] == nil and r[14] == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local r = {GetRaidRosterInfo(3)} return r[10] == 'MAINASSIST' and r[11] == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "return select('#', GetRaidRosterInfo(99)) == 14 and select(2, GetRaidRosterInfo(99)) == 0"
            )
            .unwrap());
        // `GetPartyAssignment` lists the rows, or answers for one member.
        assert_eq!(
            s.eval::<Vec<i64>>("return {GetPartyAssignment('MAINASSIST')}")
                .unwrap(),
            vec![2, 3]
        );
    }

    #[test]
    fn the_raid_roster_selection_is_a_local_cursor() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetRaidRosterSelection()").unwrap(), 0);
        s.run("SetRaidRosterSelection(11)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetRaidRosterSelection()").unwrap(),
            11
        );
        assert!(
            s.take_party_requests().is_empty(),
            "nothing about a selection goes on the wire"
        );
    }

    #[test]
    fn get_raid_target_index_reads_the_fed_mark() {
        let mut s = UiScript::new().unwrap();
        let mut marked = unit(true, 0x40);
        marked.raid_target = 8;
        s.set_unit("target", Some(marked));
        assert_eq!(
            s.eval::<i64>(r#"return GetRaidTargetIndex("target")"#)
                .unwrap(),
            8
        );
        s.set_unit("target", Some(unit(true, 0x40)));
        assert!(s
            .eval::<bool>(r#"return GetRaidTargetIndex("target") == nil"#)
            .unwrap());
    }

    // ── The ready-check timeout ─────────────────────────────────────────────

    /// Us, Alice and Bob, with the two summary strings set.
    fn raid_we_lead(s: &UiScript) -> PartyState {
        s.run(
            r#"RAID_MEMBERS_AFK = "The following players are AFK: %s"
               READY_CHECK_NO_AFK = "No players are AFK""#,
        )
        .unwrap();
        let row = |name: &str, guid: u64| crate::script::RaidMemberInfo {
            name: name.into(),
            guid,
            online: true,
            ..Default::default()
        };
        PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Alice".into(),
                    guid: 0xA11CE,
                },
                PartyMemberInfo {
                    name: "Bob".into(),
                    guid: 0xB0B,
                },
            ],
            leader_index: 0,
            leader_guid: 0x5E1F,
            own_guid: 0x5E1F,
            raid: vec![
                row("Probefour", 0x5E1F),
                row("Alice", 0xA11CE),
                row("Bob", 0xB0B),
            ],
            loot_method: "group".into(),
            master_looter: None,
            loot_threshold: 2,
            real_party_members: 0,
            real_raid_members: 0,
            loot_master_guid: 0,
            member_assignments: [0; 4],
        }
    }

    #[test]
    fn the_leaders_check_times_out_into_the_afk_list_and_disarms() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        assert_eq!(s.take_party_requests(), vec![PartyRequest::ReadyCheckStart]);

        s.set_now(129.9);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "not before the deadline"
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Alice, Bob".to_string()]
        );
        s.set_now(200.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(
            s.take_ready_check_lines().is_empty(),
            "disarmed by the summary"
        );
    }

    /// The answer that leaves nobody pending closes the check at once.
    #[test]
    fn an_answer_clears_its_member_and_the_last_one_closes_the_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "Bob is still pending"
        );
        s.ready_check_answered(0xB0B, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()],
            "nobody left pending ⇒ the summary now, not at 130"
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
    }

    /// The leader arm stores a constant 0 whatever the status byte.
    #[test]
    fn a_not_ready_answer_prints_its_line_and_still_clears_the_member() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.run(r#"RAID_MEMBER_NOT_READY = "%s is not ready""#)
            .unwrap();
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xDEAD, false);
        assert!(
            s.take_ready_check_lines().is_empty(),
            "an unmatched guid writes nothing"
        );
        s.ready_check_answered(0xA11CE, false);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["Alice is not ready".to_string()]
        );
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice answered — not ready is not AFK"
        );
    }

    /// The close tests online; the summary's walk does not.
    #[test]
    fn an_offline_member_never_blocks_the_close_but_is_still_listed() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party
            .raid
            .iter_mut()
            .for_each(|m| m.online = m.guid != 0xB0B); // Bob is offline
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["The following players are AFK: Bob".to_string()],
            "Alice's answer was the last ONLINE one, so the check closed and listed offline Bob"
        );
    }

    /// A member's client arms the deadline, never clears it, and never prints.
    #[test]
    fn the_tick_is_inert_for_a_non_leader() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"READY_CHECK_NO_AFK = "No players are AFK""#)
            .unwrap();
        s.set_party(two_member_party()); // Alice leads; we are 0x5E1F
        s.set_now(100.0);
        s.ready_check_request(false);
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert!(s.take_ready_check_lines().is_empty());
        assert_eq!(s.model_mut().ready_check.deadline, Some(130.0));
    }

    #[test]
    fn a_roster_change_with_nobody_left_pending_closes_the_check() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party.clone());
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_answered(0xA11CE, true);
        let mut without_bob = party;
        without_bob.raid.retain(|m| m.guid != 0xB0B);
        without_bob.members.retain(|m| m.guid != 0xB0B);
        s.set_party(without_bob);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// A party has no raid roster, so `DoReadyCheck` flags nobody.
    #[test]
    fn the_leaders_own_echo_closes_an_empty_check_at_once() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid.clear();
        s.set_party(party);
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        s.ready_check_request(true);
        assert_eq!(
            s.take_ready_check_lines(),
            vec!["No players are AFK".to_string()]
        );
    }

    /// Both keys are read raw from `_G` with an empty default.
    #[test]
    fn a_missing_summary_string_prints_an_empty_line() {
        let mut s = UiScript::new().unwrap();
        let party = raid_we_lead(&s);
        s.set_party(party);
        s.set_now(100.0);
        s.run("RAID_MEMBERS_AFK = nil DoReadyCheck()").unwrap();
        s.set_now(130.0);
        s.run("CheckReadyCheckTime()").unwrap();
        assert_eq!(s.take_ready_check_lines(), vec![String::new()]);
    }

    /// `GetReadyCheckStatus` (`0x574180`): nil outside an armed check and for a raid member
    /// without rank; in one, the initiator and a ready answer read "ready", a pending member
    /// "waiting", a not-ready answer "notready".
    #[test]
    fn ready_check_status_reads_each_members_answer_while_armed() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid[0].rank = 2;
        s.set_party(party.clone());
        for (token, guid) in [("player", 0x5E1F), ("party1", 0xA11CE), ("party2", 0xB0B)] {
            s.set_unit(token, Some(unit(true, guid)));
        }
        let status = |s: &mut UiScript, token: &str| {
            s.eval::<Option<String>>(&format!("return GetReadyCheckStatus('{token}')"))
                .unwrap()
        };
        s.set_now(100.0);
        assert_eq!(status(&mut s, "party1"), None, "no check armed");

        s.run("DoReadyCheck()").unwrap();
        assert_eq!(status(&mut s, "player").as_deref(), Some("ready"));
        assert_eq!(status(&mut s, "party1").as_deref(), Some("waiting"));
        s.ready_check_answered(0xA11CE, false);
        assert_eq!(status(&mut s, "party1").as_deref(), Some("notready"));
        assert_eq!(status(&mut s, "party2").as_deref(), Some("waiting"));
        assert_eq!(status(&mut s, "target"), None, "no raid member");

        // Without raid rank the player reads nothing, armed or not.
        party.raid[0].rank = 0;
        s.set_party(party);
        assert_eq!(status(&mut s, "party2"), None);
    }

    /// In a party the gate is leading it, and the player, no `partyN` slot, reads as the
    /// initiator alone (`0x5742a0`).
    #[test]
    fn a_party_ready_check_status_needs_the_lead() {
        let mut s = UiScript::new().unwrap();
        let mut party = raid_we_lead(&s);
        party.raid.clear();
        s.set_party(party.clone());
        for (token, guid) in [("player", 0x5E1F), ("party1", 0xA11CE)] {
            s.set_unit(token, Some(unit(true, guid)));
        }
        let status = |s: &mut UiScript, token: &str| {
            s.eval::<Option<String>>(&format!("return GetReadyCheckStatus('{token}')"))
                .unwrap()
        };
        s.set_now(100.0);
        s.run("DoReadyCheck()").unwrap();
        assert_eq!(status(&mut s, "player").as_deref(), Some("ready"));
        assert!(status(&mut s, "party1").is_some());
        s.set_now(130.0);
        assert_eq!(status(&mut s, "party1"), None, "expired");

        party.leader_guid = 0xA11CE;
        s.set_party(party);
        s.set_now(100.0);
        assert_eq!(
            status(&mut s, "party1"),
            None,
            "a member who does not lead reads nothing"
        );
    }
}
