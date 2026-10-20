//! `GetNumWorldStateUI` and `GetWorldStateUIInfo`, behind the always-up PvP readout, over rows the
//! app resolves; the reference re-walks the admitted DBC row ids on demand. The info is eleven
//! values, `uiType` first (`0x548d40`, `mov eax,0xb` at `0x548f22`).

use mlua::{Lua, MultiValue, Value};

/// One row of the readout: the eleven values `GetWorldStateUIInfo` returns, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldStateUiView {
    /// `uiType`: the row's `WorldStateUI.dbc` `Type`, 0, 1 (world PvP) or 3.
    pub ui_type: u32,
    /// `state`: the row's `StateVariable` world state, 0 when unknown, or 1 (on) when it has none.
    pub ui_state: i32,
    /// The label, macro-expanded, the only expanded string of the eleven.
    pub text: String,
    /// The static icon's texture path, or `""`.
    pub icon: String,
    /// The icon that replaces it while the state is live (Warsong Gulch's enemy flag), or `""`.
    pub dynamic_icon: String,
    pub tooltip: String,
    pub dynamic_tooltip: String,
    /// The extra widget the row drives, `"CAPTUREPOINT"` on the one row that has one, or `""`.
    pub extended_ui: String,
    /// That widget's three world states, resolved to values; the DBC holds ids.
    pub extended_ui_state: [i32; 3],
}

#[derive(Default)]
pub(crate) struct WorldStateUiState {
    pub(crate) rows: Vec<WorldStateUiView>,
}

impl super::UiScript {
    /// Push the readout's rows, queuing `UPDATE_WORLD_STATES` (event `0x20e`) on any change.
    /// Deviation: the reference fires it only after an init rebuild (`0x48fa12`) or a zone-defense
    /// channel flip (`0x49bdd6`); we fire on a value change too, because our rows are resolved
    /// values its Lua would re-read live.
    pub fn set_world_state_ui(&mut self, rows: Vec<WorldStateUiView>) {
        let mut model = self.model_mut();
        if model.worldstate.rows != rows {
            model.worldstate.rows = rows;
            model
                .pending_events
                .push(("UPDATE_WORLD_STATES".to_string(), Vec::new()));
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x548720`: the count, no validation, always one return.
    g.set(
        "GetNumWorldStateUI",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.worldstate.rows.len() as i64)
        })?,
    )?;

    // A non-number raises (the `luaL_error` longjmps), an out-of-range index answers the single
    // number 0 (`0x548d40`), and no string is ever nil: an empty DBC column reads `""`.
    g.set(
        "GetWorldStateUIInfo",
        lua.create_function(|lua, index: Value| {
            let index = match index {
                Value::Integer(i) => i as f64,
                Value::Number(n) => n,
                // A numeric string too, as `lua_isnumber` coerces.
                Value::String(ref s) => match s.to_str().ok().and_then(|s| s.trim().parse().ok()) {
                    Some(n) => n,
                    None => return Err(mlua::Error::runtime("Usage: GetWorldStateUIInfo(index)")),
                },
                _ => return Err(mlua::Error::runtime("Usage: GetWorldStateUIInfo(index)")),
            };
            let row = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                // Truncated toward zero by `__ftol`, then 1-based.
                usize::try_from(index.trunc() as i64)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| model.worldstate.rows.get(i).cloned())
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![Value::Integer(0)]));
            };
            let s = |v: &str| -> mlua::Result<Value> { Ok(Value::String(lua.create_string(v)?)) };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(r.ui_type)),
                Value::Integer(i64::from(r.ui_state)),
                s(&r.text)?,
                s(&r.icon)?,
                s(&r.dynamic_icon)?,
                s(&r.tooltip)?,
                s(&r.dynamic_tooltip)?,
                s(&r.extended_ui)?,
                Value::Integer(i64::from(r.extended_ui_state[0])),
                Value::Integer(i64::from(r.extended_ui_state[1])),
                Value::Integer(i64::from(r.extended_ui_state[2])),
            ]))
        })?,
    )?;

    Ok(())
}
