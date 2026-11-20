//! `ConfirmBinder` and `CheckBinderDist`, the engine globals the `CONFIRM_BINDER` dialog calls (`StaticPopup.lua`): each reads the latched innkeeper live at the call (`0x51bf90`, `0x51b7a0`).

use mlua::Lua;

/// Register the two binder globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `ConfirmBinder()`: the dialog's Accept, the binder talk with no guid (`0x51bfc3`).
    g.set(
        "ConfirmBinder",
        lua.create_function(|_, ()| {
            super::read_live(|l| l.confirm_binder());
            Ok(())
        })?,
    )?;

    // `CheckBinderDist()`, polled by the dialog's OnUpdate: 1 or nil (`0x51b7a0`).
    g.set(
        "CheckBinderDist",
        lua.create_function(|_, ()| {
            Ok(super::binding_abi::flag(
                super::read_live(|l| l.binder_in_reach()).unwrap_or(false),
            ))
        })?,
    )?;

    Ok(())
}
