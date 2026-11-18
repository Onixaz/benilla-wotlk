//! Lua globals whose whole body pushes one catalog id at the error display's dispatcher.
//! Engine-side refusals share the [`Model::ui_errors`] queue, which the app resolves against
//! GlobalStrings and fires as `UI_ERROR_MESSAGE` (event `0xe0`), the reference's route.
//!
//! `NotWhileDeadError`: no argument, no dead check, no return, and a silent catalog row. FrameXML
//! decides when to call it (`UIParent.lua:663-666`, `ContainerFrame.lua:147` and `:190`).

use mlua::Lua;

/// Catalog id `0x7e`'s GlobalStrings key ("You can't do that when you're dead.").
const NOT_WHILE_DEAD_KEY: &str = "ERR_PLAYER_DEAD";

/// The message display called inline (`0x5216f0`): an error or info line fires its event inside the caller's call (`0x52188a`, `0x521863`) and its sound goes to the app; a chat line goes to the app whole. No GlobalStrings text, no line.
pub(crate) fn show_now(lua: &Lua, key: &'static str) {
    let Some(text) = lua.globals().get::<Option<String>>(key).ok().flatten() else {
        return;
    };
    let event = match crate::messages::kind_of(key) {
        crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
        crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
        crate::messages::MsgKind::Chat => {
            crate::script::model_mut_of(lua).ui_errors.push(key);
            return;
        }
    };
    if crate::messages::by_key(key).is_some() {
        crate::script::model_mut_of(lua).message_sounds.push(key);
    }
    let text = crate::strings::fill(&text, &[]);
    super::tick::fire_event_into(lua, event, vec![super::ScriptValue::Str(text)]);
}

/// Register the error-display globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "NotWhileDeadError",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.ui_errors.push(NOT_WHILE_DEAD_KEY);
            Ok(())
        })?,
    )
}
