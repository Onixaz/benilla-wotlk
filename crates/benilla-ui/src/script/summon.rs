//! The five summon globals the `CONFIRM_SUMMON` dialog calls (`StaticPopup.lua:2534-2557`), the
//! three getters read through the lent view at call time, as 12340's read the latch
//! (`0x516510`, `0x516580`, `0x5164b0`).

use mlua::Lua;

use super::read_live;

impl super::UiScript {
    /// Drain the `ConfirmSummon()` (true) and `CancelSummon()` (false) calls in order, each a
    /// `CMSG_SUMMON_RESPONSE`.
    pub fn take_summon_answers(&mut self) -> Vec<bool> {
        drain_field!(self, summon_answers)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // A string on every path.
    g.set(
        "GetSummonConfirmSummoner",
        lua.create_function(|lua, ()| {
            lua.create_string(read_live(|l| l.summon_summoner()).unwrap_or_default())
        })?,
    )?;

    // The summoner's zone straight from `AreaTable.dbc`, with no parent walk and no GlobalString
    // fallback, unlike the innkeeper question's chain.
    g.set(
        "GetSummonConfirmAreaName",
        lua.create_function(|lua, ()| {
            lua.create_string(read_live(|l| l.summon_area()).unwrap_or_default())
        })?,
    )?;

    // Whole seconds, truncated; the dialog reads it once in `OnShow` and counts down locally from
    // there.
    g.set(
        "GetSummonConfirmTimeLeft",
        lua.create_function(|_, ()| {
            let ms = read_live(|l| l.summon_time_left_ms()).unwrap_or(0);
            Ok(i64::from(ms / 1000))
        })?,
    )?;

    // The dialog's Accept (`0x51b050`), which the app gates on the player's state.
    g.set(
        "ConfirmSummon",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).summon_answers.push(true);
            Ok(())
        })?,
    )?;

    // The dialog's `OnCancel`, run by Cancel, Escape and the timeout (`0x51b110`).
    g.set(
        "CancelSummon",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).summon_answers.push(false);
            Ok(())
        })?,
    )?;

    Ok(())
}
