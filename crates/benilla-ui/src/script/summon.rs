//! The five summon globals the `CONFIRM_SUMMON` dialog calls (`StaticPopup.lua:2534-2557`). The
//! reference resolves the summoner, zone and deadline at call time; the app pushes the answers each
//! frame, so a name still in flight reads `""` and fills in on a later tick.

use mlua::Lua;

/// The dialog's three reads, never nil: with nothing pending or resolved they are `""`, `""` and 0,
/// as the reference's getters answer, the empty ones with its shared `""`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummonConfirmUiState {
    /// The summoner's name from the name cache, `""` while the query is out.
    pub summoner: String,
    /// The summoner's zone name out of `AreaTable.dbc`, or `""` if the id names no row.
    pub area: String,
    /// Milliseconds left, 0 when nothing is pending or it ran out; the binding truncates to
    /// seconds, as the reference's getter does.
    pub time_left_ms: u32,
}

impl super::UiScript {
    /// Push the three resolved reads, each frame.
    pub fn set_summon_confirm(&mut self, state: SummonConfirmUiState) {
        let mut model = self.model_mut();
        if model.summon_confirm != state {
            model.summon_confirm = state;
        }
    }

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
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.summon_confirm.summoner)
        })?,
    )?;

    // The summoner's zone straight from `AreaTable.dbc`, with no parent walk and no GlobalString
    // fallback, unlike the innkeeper question's chain.
    g.set(
        "GetSummonConfirmAreaName",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.summon_confirm.area)
        })?,
    )?;

    // Whole seconds, truncated; the dialog reads it once in `OnShow` and counts down locally from
    // there.
    g.set(
        "GetSummonConfirmTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(f64::from(model.summon_confirm.time_left_ms / 1000))
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
