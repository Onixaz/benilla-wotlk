//! The state reads 3.3.5a's unit menu gates its rows on; systems benilla does not run answer off.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, optional_string, string_arg};
use super::{Model, UnitState};

/// `PLAYER_FLAGS` bit 9, the PvP-desired flag `GetPVPDesired` (`0x51bca0`) reads.
const PLAYER_FLAG_PVP_DESIRED: u32 = 1 << 9;
/// The aura `PlayerIsPVPInactive` (`0x612e20`) looks for, the PvP-inactive debuff.
const PVP_INACTIVE_AURA: u32 = 43681;
/// `UNIT_DYNAMIC_FLAGS` bit 0x40, the Refer-a-Friend link (`0x6d75bf`).
const UNIT_DYNFLAG_REFER_A_FRIEND: u32 = 0x40;

/// The instance difficulties the app pushes, each 0-based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DifficultyState {
    /// The group's in a party, else the player's (`0x5138d0`).
    pub dungeon: u32,
    /// The player's own `[0xbd0898]`.
    pub player_dungeon: u32,
    /// The group's in a raid, else the player's (`0x515810`).
    pub raid: u32,
    /// The player's own `[0xbd089c]`.
    pub player_raid: u32,
}

/// What `CanGrantLevel` and `CanSummonFriend` read off the player.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReferAFriendState {
    /// `PLAYER_FIELD_BYTES` byte 1, the levels left to grant (`0x6deb08`).
    pub grantable_levels: u8,
    /// The learned summon-friend spell `[0xbe7d88]`, `Effect[0]` 152 (`0x5420bf`); 0 for none.
    pub summon_spell: u32,
}

impl super::UiScript {
    /// Push the instance difficulties.
    pub fn set_difficulty(&mut self, state: DifficultyState) {
        if self.model_ref().difficulty != state {
            self.model_mut().difficulty = state;
        }
    }

    /// Push the group list's LFG-restricted bit.
    pub fn set_lfg_restricted(&mut self, restricted: bool) {
        if self.model_ref().lfg_restricted != restricted {
            self.model_mut().lfg_restricted = restricted;
        }
    }

    /// Push the player's Refer-a-Friend state.
    pub fn set_refer_a_friend(&mut self, state: ReferAFriendState) {
        if self.model_ref().refer_a_friend != state {
            self.model_mut().refer_a_friend = state;
        }
    }
}

/// `CGPlayer_C::IsRAFPlayer` (`0x6d7590`): a held player answers its `UNIT_DYNAMIC_FLAGS` bit 0x40
/// (`0x6d75b9`), else a friend its status bit 0x08 (`0x6d7612`). The group-record leg between them
/// (status bit 0x100, `0x6d75e9`) is not built: the VM holds no roster record status.
pub(crate) fn is_raf_player(model: &Model, unit: Option<&UnitState>) -> bool {
    let Some(unit) = unit else {
        return false;
    };
    if unit.has_object && unit.is_player {
        return unit.dynamic_flags & UNIT_DYNFLAG_REFER_A_FRIEND != 0;
    }
    unit.guid != 0
        && model
            .social
            .friends
            .iter()
            .any(|f| f.guid == unit.guid && f.raf)
}

/// `CGPartyInfo::IsMember` or `IsGuidInMyRaid`: a party slot or a raid row holds the guid.
fn in_my_group(model: &Model, guid: u64) -> bool {
    model.unit_guids.party.contains(&guid) || model.party.raid.iter().any(|m| m.guid == guid)
}

/// `MAINTANK` or `MAINASSIST`, case-insensitively, to its assignment bit; a non-string raises
/// `usage`, another string `invalid`.
fn assignment_flag(
    lua: &Lua,
    assignment: Value,
    usage: &'static str,
    invalid: &'static str,
) -> mlua::Result<u8> {
    let assignment = string_arg(lua, assignment, usage)?;
    if assignment.eq_ignore_ascii_case("MAINTANK") {
        Ok(super::party::ASSIGNMENT_MAIN_TANK)
    } else if assignment.eq_ignore_ascii_case("MAINASSIST") {
        Ok(super::party::ASSIGNMENT_MAIN_ASSIST)
    } else {
        Err(mlua::Error::runtime(invalid))
    }
}

/// A group member's assignment bits: their raid row's, else their party slot's.
fn assignments_of(model: &Model, guid: u64) -> u8 {
    if let Some(m) = model.party.raid.iter().find(|m| m.guid == guid) {
        return m.assignments;
    }
    model
        .party
        .members
        .iter()
        .position(|m| m.guid == guid)
        .map_or(0, |i| model.party.member_assignments[i.min(3)])
}

fn flag_value(b: bool) -> Value {
    if b {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// The spell's book slot is cooling down, as `Spell_C_GetSpellCooldown` answers non-zero.
fn spell_cooling(model: &Model, spell: u32, now: f64) -> bool {
    model
        .spellbook
        .slots
        .iter()
        .find(|s| spell != 0 && s.spell_id == spell)
        .and_then(|s| s.cooldown)
        .is_some_and(|(start_ms, duration_ms, enabled)| {
            !enabled || start_ms as f64 / 1000.0 + f64::from(duration_ms) / 1000.0 > now
        })
}

/// `CGPlayer_C::CanSummonFriend` (`0x6d7630`): 0 when the summon may go, else the refusal code.
fn summon_friend_refusal(model: &Model, token: &str, now: f64) -> u8 {
    let unit = model.unit(token).filter(|u| u.exists);
    let Some(guid) = unit.map(|u| u.guid).filter(|&g| g != 0) else {
        return 8;
    };
    if !is_raf_player(model, unit) {
        return 1;
    }
    if !in_my_group(model, guid) {
        return 9;
    }
    // The record leg for an unheld member (offline 13, the record's level) is not built.
    if unit.is_some_and(|u| u.level >= 61) {
        return 10;
    }
    if spell_cooling(model, model.refer_a_friend.summon_spell, now) {
        11
    } else {
        0
    }
}

/// `CGPlayer_C::CanGrantLevel` (`0x6de9e0`): 0 when a level may be granted, else the refusal code.
fn grant_level_refusal(model: &Model, token: &str) -> u8 {
    let unit = model.unit(token).filter(|u| u.exists);
    if unit.is_none_or(|u| u.guid == 0) {
        return 8;
    }
    let Some(unit) = unit.filter(|u| u.has_object) else {
        return 4;
    };
    if !is_raf_player(model, Some(unit)) {
        return 1;
    }
    let me = model.unit("player");
    let mine = me.and_then(|m| m.faction_group.as_deref());
    if let (Some(mine), Some(theirs)) = (mine, unit.faction_group.as_deref()) {
        if mine != theirs {
            return 5;
        }
    }
    if unit.level > 59 {
        return 7;
    }
    if me.map_or(0, |m| m.level) <= unit.level {
        return 2;
    }
    if !unit.can_assist {
        return 6;
    }
    if model.refer_a_friend.grantable_levels == 0 {
        3
    } else {
        0
    }
}

/// A token or a roster name to its guid, as `Script_GetGUIDFromToken(…, false)` (`0x60abf0`).
pub(super) fn guid_of(model: &Model, unit: &str) -> Option<u64> {
    if let Some(u) = model.unit(unit).filter(|u| u.exists) {
        return Some(u.guid);
    }
    let raid = model
        .party
        .raid
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(unit));
    if let Some(m) = raid {
        return Some(m.guid);
    }
    (1..=4).find_map(|i| {
        model
            .unit(&format!("party{i}"))
            .filter(|u| {
                u.exists
                    && u.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(unit))
            })
            .map(|u| u.guid)
    })
}

fn model_of(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // InCombatLockdown() (`0x511cc0`): 1 from entering combat until leaving it.
    g.set(
        "InCombatLockdown",
        lua.create_function(|lua, ()| {
            let model = model_of(lua);
            Ok(flag(model.unit("player").is_some_and(|u| u.in_combat)))
        })?,
    )?;
    // GetDungeonDifficulty() (`0x515790`): the current dungeon difficulty and the player's, 1-based.
    g.set(
        "GetDungeonDifficulty",
        lua.create_function(|lua, ()| {
            let d = model_of(lua).difficulty;
            Ok((d.dungeon + 1, d.player_dungeon + 1))
        })?,
    )?;
    // GetRaidDifficulty() (`0x515810`): the current raid difficulty and the player's, 1-based.
    g.set(
        "GetRaidDifficulty",
        lua.create_function(|lua, ()| {
            let d = model_of(lua).difficulty;
            Ok((d.raid + 1, d.player_raid + 1))
        })?,
    )?;
    // CanChangePlayerDifficulty() (`0x52d000`): only on a dynamic raid map, which benilla has none of.
    g.set(
        "CanChangePlayerDifficulty",
        lua.create_function(|_, ()| Ok((false, true)))?,
    )?;
    // HasLFGRestrictions() (`0x52c350`): the group list's LFG-restricted bit, a boolean.
    g.set(
        "HasLFGRestrictions",
        lua.create_function(|lua, ()| Ok(model_of(lua).lfg_restricted))?,
    )?;
    // GetPVPDesired() (`0x51bca0`): the player's PvP-desired flag, 1 or 0.
    g.set(
        "GetPVPDesired",
        lua.create_function(|lua, ()| {
            let model = model_of(lua);
            let flags = model.unit("player").map_or(0, |u| u.player_flags);
            Ok(i32::from(flags & PLAYER_FLAG_PVP_DESIRED != 0))
        })?,
    )?;
    // PlayerIsPVPInactive(unit) (`0x612e20`): whether the unit carries the PvP-inactive aura.
    g.set(
        "PlayerIsPVPInactive",
        lua.create_function(|lua, unit: Value| {
            let unit = string_arg(lua, unit, "Usage: PlayerIsPVPInactive(\"unit\")")?;
            let model = model_of(lua);
            let inactive = super::aura::auras_of(&model, &unit)
                .is_some_and(|list| list.iter().any(|a| a.spell_id == PVP_INACTIVE_AURA));
            Ok(inactive)
        })?,
    )?;
    // CanComplainChat (`0x500c80`): benilla keeps no chat line records, so nil.
    g.set(
        "CanComplainChat",
        lua.create_function(|lua, arg: Value| {
            if optional_string(lua, &arg).is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: CanComplainChat(lineID) or CanComplainChat(name) or CanComplainChat(name, text)",
                ));
            }
            Ok(Value::Nil)
        })?,
    )?;
    // CanExitVehicle() (`0x5fb9c0`): the vehicle seat's exit flag; benilla seats no vehicle (#235).
    g.set(
        "CanExitVehicle",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // GetPartyAssignment("assignment" [, member [, exact]]) (`0x52cf60`): with a member, 1 or nil
    // for them (a member outside the group prints `ERR_TARGET_NOT_IN_GROUP_S` and answers
    // nothing); without, every raid row (or party slot) holding the assignment.
    g.set(
        "GetPartyAssignment",
        lua.create_function(|lua, (assignment, member): (Value, Value)| {
            let flag = assignment_flag(
                lua,
                assignment,
                "Usage: GetPartyAssignment(\"assignment\" [,\"raidmember\"] [,exactMatch])",
                "GetPartyAssignment(): Invalid party assignment",
            )?;
            let named = optional_string(lua, &member);
            let mut model = crate::script::model_mut_of(lua);
            if let Some(name) = named {
                let Some(guid) = guid_of(&model, &name).filter(|&g| in_my_group(&model, g)) else {
                    model.ui_errors_s.push(("ERR_TARGET_NOT_IN_GROUP_S", name));
                    return Ok(MultiValue::new());
                };
                let held = assignments_of(&model, guid) & flag != 0;
                return Ok(MultiValue::from_vec(vec![flag_value(held)]));
            }
            let rows: Vec<Value> = if model.party.raid.is_empty() {
                (0..4)
                    .filter(|&i| model.party.member_assignments[i] & flag != 0)
                    .map(|i| Value::Integer(i as i64 + 1))
                    .collect()
            } else {
                model
                    .party
                    .raid
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.assignments & flag != 0)
                    .map(|(i, _)| Value::Integer(i as i64 + 1))
                    .collect()
            };
            Ok(MultiValue::from_vec(rows))
        })?,
    )?;
    // SetPartyAssignment / ClearPartyAssignment("assignment" [, member [, exact]]) (`0x52e1b0`,
    // `0x52e400`): ungrouped prints `ERR_NOT_IN_GROUP`, a member who neither leads nor holds a
    // raid rank prints `ERR_NOT_LEADER`, an unmatched name prints `ERR_UNIT_NOT_FOUND`; else
    // `MSG_PARTY_ASSIGNMENT` goes out.
    for (binding, set) in [
        ("SetPartyAssignment", true),
        ("ClearPartyAssignment", false),
    ] {
        g.set(
            binding,
            lua.create_function(move |lua, (assignment, member): (Value, Value)| {
                let mut model = crate::script::model_mut_of(lua);
                if model.party.members.is_empty() && model.party.raid.is_empty() {
                    model.ui_errors.push("ERR_NOT_IN_GROUP");
                    return Ok(());
                }
                let own = model.party.own_guid;
                let leads = own != 0 && model.party.leader_guid == own;
                let officer = model.party.raid.iter().any(|m| m.guid == own && m.rank > 0);
                if !leads && !officer {
                    model.ui_errors.push("ERR_NOT_LEADER");
                    return Ok(());
                }
                drop(model);
                let usage = if set {
                    "Usage: SetPartyAssignment(\"assignment\" [,\"raidmember\"] [,exactMatch])"
                } else {
                    "Usage: ClearPartyAssignment(\"assignment\" [,\"raidmember\"] [,exactMatch])"
                };
                let flag = assignment_flag(lua, assignment, usage, "Invalid Party assignment")?;
                let named = optional_string(lua, &member);
                let mut model = crate::script::model_mut_of(lua);
                let guid = match named {
                    Some(name) => {
                        match guid_of(&model, &name).filter(|&g| in_my_group(&model, g)) {
                            Some(guid) => guid,
                            None => {
                                let error =
                                    if name.is_empty() || name.eq_ignore_ascii_case("target") {
                                        "ERR_GENERIC_NO_TARGET"
                                    } else {
                                        "ERR_UNIT_NOT_FOUND"
                                    };
                                model.ui_errors.push(error);
                                return Ok(());
                            }
                        }
                    }
                    None => 0,
                };
                let assignment = u8::from(flag == super::party::ASSIGNMENT_MAIN_ASSIST);
                model
                    .party_requests
                    .push(super::party::PartyRequest::PartyAssignment {
                        assignment,
                        set,
                        guid,
                    });
                Ok(())
            })?,
        )?;
    }

    // ── Voice chat, which benilla does not run ─────────────────────────────────────────────────
    // IsVoiceChatEnabled() (`0x4fcbf0`): nil with the voice system down.
    g.set(
        "IsVoiceChatEnabled",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // GetVoiceStatus(unit [, channel]) (`0x7db8d0`): nothing for a non-string, nil with no session.
    g.set(
        "GetVoiceStatus",
        lua.create_function(|lua, unit: Value| {
            let mut out = MultiValue::new();
            if optional_string(lua, &unit).is_some() {
                out.push_back(Value::Nil);
            }
            Ok(out)
        })?,
    )?;
    // UnitIsSilenced (`0x4fccf0`) / IsSilenced (`0x4ffb50`): no voice session; a non-string answers none.
    for name in ["UnitIsSilenced", "IsSilenced"] {
        g.set(
            name,
            lua.create_function(|lua, unit: Value| {
                let mut out = MultiValue::new();
                if optional_string(lua, &unit).is_some() {
                    out.push_back(Value::Nil);
                }
                Ok(out)
            })?,
        )?;
    }
    // CanGrantLevel(unit) (`0x51d230`): 1 when the player check answers 0, else nil.
    g.set(
        "CanGrantLevel",
        lua.create_function(|lua, unit: Value| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let model = model_of(lua);
            let can = model.unit_guids.player != 0 && grant_level_refusal(&model, &unit) == 0;
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;
    // CanSummonFriend(unit) (`0x51d2b0`): 1 when the player check answers 0, else nil.
    g.set(
        "CanSummonFriend",
        lua.create_function(|lua, unit: Value| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let now = super::clock::now(lua);
            let model = model_of(lua);
            let can =
                model.unit_guids.player != 0 && summon_friend_refusal(&model, &unit, now) == 0;
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // UnitIsInMyGuild (`0x60c550`): a unit in view shares our guild, else the name is on our roster.
    g.set(
        "UnitIsInMyGuild",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: UnitIsInMyGuild(\"name\")")?;
            let model = model_of(lua);
            let ours = model
                .unit("player")
                .and_then(|u| u.guild.as_ref())
                .map(|g| g.name.clone());
            if let Some(u) = model.unit(&name).filter(|u| u.exists) {
                let same = ours
                    .as_deref()
                    .is_some_and(|o| u.guild.as_ref().is_some_and(|g| g.name == o));
                return Ok(flag(same));
            }
            let listed = model
                .guild
                .roster
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case(&name));
            Ok(flag(listed))
        })?,
    )?;
    // UnitIsRaidOfficer(unit) (`0x60c8a0`): a raid member whose rank is above member.
    g.set(
        "UnitIsRaidOfficer",
        lua.create_function(|lua, unit: Value| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let model = model_of(lua);
            let officer = guid_of(&model, &unit).is_some_and(|guid| {
                model
                    .party
                    .raid
                    .iter()
                    .any(|m| m.guid == guid && m.rank > 0)
            });
            Ok(flag(officer))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Every gate answers without raising on the arguments the stock menu passes.
    #[test]
    fn the_menu_gates_answer_the_stock_calls() {
        let s = UiScript::new().unwrap();
        s.run(
            "assert(InCombatLockdown() == nil) \
             local d, p = GetDungeonDifficulty() assert(d == 1 and p == 1) \
             assert(GetRaidDifficulty() == 1) \
             local can, ready = CanChangePlayerDifficulty() assert(can == false and ready == true) \
             assert(HasLFGRestrictions() == false) \
             assert(GetPVPDesired() == 0) \
             assert(PlayerIsPVPInactive('target') == false) \
             assert(CanComplainChat('Bob') == nil) \
             assert(CanExitVehicle() == nil and PetCanBeDismissed() == 1) \
             assert(GetPartyAssignment('MAINTANK', 'Bob', 1) == nil) \
             assert(IsVoiceChatEnabled() == nil and GetVoiceStatus('target') == nil) \
             assert(select('#', GetVoiceStatus()) == 0) \
             assert(UnitIsSilenced('Bob', 'Party') == nil and IsMuted('Bob') == nil) \
             assert(IsDisplayChannelOwner() == nil and IsDisplayChannelModerator() == nil) \
             assert(IsArenaTeamCaptain(1) == nil and CanGrantLevel('target') == nil) \
             assert(UnitIsInMyGuild('Bob') == nil and UnitIsRaidOfficer('Bob') == nil)",
        )
        .unwrap();
        assert!(s.run("GetPartyAssignment('HEALER')").is_err());
        assert!(s.run("UnitIsInMyGuild()").is_err());
    }
}
