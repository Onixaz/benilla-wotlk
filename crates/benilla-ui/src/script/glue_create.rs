//! The character create verbs (table `0xac4228`, registered by `0x4dfd00`) and the state they
//! keep: the create screen's model frame so far.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::string_arg;

/// The create verbs' slots in the glue model.
#[derive(Default)]
pub(crate) struct GlueCreate {
    /// `CCharacterCreation::m_charCustomizeFrame`: the model frame `SetCharCustomizeFrame` found.
    pub(super) customize_frame: Option<String>,
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `SetCharCustomizeFrame(name)` (`0x4e05b0`): a string or the usage error; the global of that
    // name castable to the model type becomes the frame, else the call changes nothing.
    g.set(
        "SetCharCustomizeFrame",
        lua.create_function(|lua, args: MultiValue| {
            let name = string_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: SetCharCustomizeFrame(\"frameName\")",
            )?;
            let mut m = crate::script::model_mut_of(lua);
            let is_model = m
                .arena
                .lookup(&name)
                .and_then(|h| m.arena.frame(h))
                .is_some_and(|f| f.kind.is_model());
            if is_model {
                m.glue.create.customize_frame = Some(name);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}
