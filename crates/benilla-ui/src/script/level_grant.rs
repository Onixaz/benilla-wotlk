//! The refer-a-friend level grant's answer verbs and the server's UI clock offset.

use mlua::Lua;

impl super::UiScript {
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
