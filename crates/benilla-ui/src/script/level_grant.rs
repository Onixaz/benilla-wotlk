//! The refer-a-friend level grant's answer verbs and the server's UI clock offset.

use mlua::{Lua, Value};

use super::binding_abi::{optional_string, string_arg};

/// A unit verb's outcome, for the app to send or show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitRequest {
    /// `GrantLevel(unit)` (`0x523160`): `CMSG_GRANT_LEVEL`.
    GrantLevel(u64),
    /// A refused grant or summon (`0x5219e0`): the code, and the guid whose name 9 and 13 show.
    ReferAFriendRefused { code: u8, guid: u64 },
    /// `SummonFriend(unit)` (`0x523260`): casts `spell` at the friend, selected for the cast;
    /// `restore` is the selection to send back when it was not them.
    SummonFriend {
        guid: u64,
        spell: u32,
        restore: Option<u64>,
    },
    /// `ReportPlayerIsPVPAFK(unit)` (`0x612d50`): the report, then its line (`0x6d74b0`).
    ReportPvpAfk(u64),
}

impl super::UiScript {
    /// Drain the grant, summon and PvP AFK requests since the last call.
    pub fn take_unit_requests(&mut self) -> Vec<UnitRequest> {
        drain_field!(self, unit_requests)
    }

    /// Drain the `AcceptLevelGrant` (true) and `DeclineLevelGrant` (false) calls since the last.
    pub fn take_level_grant_answers(&mut self) -> Vec<bool> {
        drain_field!(self, level_grant_answers)
    }

    /// The server's UI clock less the local clock, seconds (`0x5486e0`): `GetWintergraspWaitTime`
    /// reads `time()` plus it (`0x548700`).
    pub fn set_ui_time_offset(&mut self, offset: i32) {
        set_field!(self, ui_time_offset, offset);
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // AcceptLevelGrant() (`0x521b00`): the app sends `CMSG_ACCEPT_LEVEL_GRANT` to the pending
    // granter, if any, and forgets it.
    g.set(
        "AcceptLevelGrant",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua)
                .level_grant_answers
                .push(true);
            Ok(())
        })?,
    )?;
    // GrantLevel(unit) (`0x523160`): the send, or the refusal's error.
    g.set(
        "GrantLevel",
        lua.create_function(|lua, unit: Value| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let mut model = crate::script::model_mut_of(lua);
            let guid = super::unit_menu::guid_of(&model, &unit).unwrap_or(0);
            let code = super::unit_menu::grant_level_refusal(&model, &unit);
            model.unit_requests.push(match code {
                0 => UnitRequest::GrantLevel(guid),
                code => UnitRequest::ReferAFriendRefused { code, guid },
            });
            Ok(())
        })?,
    )?;
    // SummonFriend(unit) (`0x523260`): the cast at the friend, or the refusal's error.
    g.set(
        "SummonFriend",
        lua.create_function(|lua, unit: Value| {
            let unit = optional_string(lua, &unit).unwrap_or_default();
            let now = super::clock::now(lua);
            let mut model = crate::script::model_mut_of(lua);
            let guid = super::unit_menu::guid_of(&model, &unit).unwrap_or(0);
            let code = super::unit_menu::summon_friend_refusal(&model, &unit, now);
            let target = model.unit_guids.target;
            let spell = model.refer_a_friend.summon_spell;
            model.unit_requests.push(match code {
                0 => UnitRequest::SummonFriend {
                    guid,
                    spell,
                    restore: (target != guid).then_some(target),
                },
                code => UnitRequest::ReferAFriendRefused { code, guid },
            });
            Ok(())
        })?,
    )?;
    // ReportPlayerIsPVPAFK(unit) (`0x612d50`): a group member other than the player.
    g.set(
        "ReportPlayerIsPVPAFK",
        lua.create_function(|lua, unit: Value| {
            let unit = string_arg(lua, unit, "Usage: ReportPlayerIsPVPAFK(\"unit\")")?;
            let mut model = crate::script::model_mut_of(lua);
            let guid = super::unit_menu::guid_of(&model, &unit).unwrap_or(0);
            let me = model.unit_guids.player;
            if me != 0 && guid != me && super::unit_menu::in_my_group(&model, guid) {
                model.unit_requests.push(UnitRequest::ReportPvpAfk(guid));
            }
            Ok(())
        })?,
    )?;
    // DeclineLevelGrant() (`0x517d20`): forgets the granter, sending nothing.
    g.set(
        "DeclineLevelGrant",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua)
                .level_grant_answers
                .push(false);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_answers_queue_in_order() {
        let mut s = UiScript::new().unwrap();
        s.run("AcceptLevelGrant() DeclineLevelGrant()").unwrap();
        assert_eq!(s.take_level_grant_answers(), vec![true, false]);
        assert!(s.take_level_grant_answers().is_empty());
    }
}
