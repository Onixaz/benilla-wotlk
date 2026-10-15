//! `Screenshot()`, the engine verb behind the print-screen key: the `SCREENSHOT` binding calls
//! `TakeScreenshot()` (`WorldFrame.lua:46`), which hides the status text and calls this. It
//! returns nothing; the outcome arrives later as `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`,
//! so the "Screen Captured" message never lands in the capture.

use mlua::Lua;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "Screenshot",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).screenshot_asks += 1;
            Ok(())
        })?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// `Screenshot()` calls since the last drain, each one capture.
    pub fn take_screenshot_asks(&mut self) -> u32 {
        drain_field!(self, screenshot_asks)
    }
}
