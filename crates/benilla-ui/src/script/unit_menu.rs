//! The state reads 3.3.5a's unit menu gates its rows on; systems benilla does not run answer off.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, optional_string, string_arg};
use super::Model;

/// `PLAYER_FLAGS` bit 9, the PvP-desired flag `GetPVPDesired` (`0x51bca0`) reads.
const PLAYER_FLAG_PVP_DESIRED: u32 = 1 << 9;
/// The aura `PlayerIsPVPInactive` (`0x612e20`) looks for, the PvP-inactive debuff.
const PVP_INACTIVE_AURA: u32 = 43681;

/// A token or a roster name to its guid, as `Script_GetGUIDFromToken(…, false)` (`0x60abf0`).
fn guid_of(model: &Model, unit: &str) -> Option<u64> {
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
    // GetDungeon/RaidDifficulty (`0x515790`/`0x515810`): no difficulty packet is read, so normal.
    for name in ["GetDungeonDifficulty", "GetRaidDifficulty"] {
        g.set(name, lua.create_function(|_, ()| Ok((1, 1)))?)?;
    }
    // CanChangePlayerDifficulty() (`0x52d000`): only on a dynamic raid map, which benilla has none of.
    g.set(
        "CanChangePlayerDifficulty",
        lua.create_function(|_, ()| Ok((false, true)))?,
    )?;
    // HasLFGRestrictions() (`0x52c350`): the LFG group's restricted bit; benilla joins none.
    g.set(
        "HasLFGRestrictions",
        lua.create_function(|_, ()| Ok(false))?,
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
    // PetCanBeDismissed() (`0x5d3820`): 1 unless `SummonProperties` forbids, which benilla does not read.
    g.set("PetCanBeDismissed", lua.create_function(|_, ()| Ok(1))?)?;
    // GetPartyAssignment (`0x52cf60`): the folded roster keeps no tank or assist marks.
    g.set(
        "GetPartyAssignment",
        lua.create_function(|lua, assignment: Value| {
            let assignment = string_arg(
                lua,
                assignment,
                "Usage: GetPartyAssignment(\"assignment\" [,\"raidmember\"] [,exactMatch])",
            )?;
            if !assignment.eq_ignore_ascii_case("MAINTANK")
                && !assignment.eq_ignore_ascii_case("MAINASSIST")
            {
                return Err(mlua::Error::runtime(
                    "GetPartyAssignment(): Invalid party assignment",
                ));
            }
            Ok(Value::Nil)
        })?,
    )?;

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
    // IsMuted(name) (`0x6b6cf0`): the friend list's mute list, which benilla does not keep.
    g.set(
        "IsMuted",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    // IsDisplayChannelOwner/Moderator (`0x4ffa70`/`0x4ffae0`): the channel pane selects none.
    for name in ["IsDisplayChannelOwner", "IsDisplayChannelModerator"] {
        g.set(name, lua.create_function(|_, ()| Ok(Value::Nil))?)?;
    }

    // IsArenaTeamCaptain(index) (`0x60dc70`): benilla reads no arena team.
    g.set(
        "IsArenaTeamCaptain",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    // CanGrantLevel/CanSummonFriend (`0x51d230`/`0x51d2b0`): no recruit-a-friend link.
    for name in ["CanGrantLevel", "CanSummonFriend"] {
        g.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
        )?;
    }

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
