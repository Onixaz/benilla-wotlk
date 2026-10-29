//! The two clocks Lua reads: `GetTime()`, the session seconds [`UiScript::tick`] advances, and
//! `GetGameTime()`, the game clock the host pushes. Both live in the [`Model`], out of an addon's
//! reach, as the reference's live in the client.
//!
//! [`UiScript::tick`]: super::UiScript::tick

use mlua::Lua;

impl super::UiScript {
    /// The current `GetTime()` value in seconds; the app stamps absolute expiries with it, such as
    /// an aura's `expirationTime`.
    pub fn now(&self) -> f64 {
        self.model_ref().now
    }

    /// Start this VM's `GetTime()` clock at `secs`, so a rebuilt VM (a relog, a `ReloadUI`) keeps
    /// the process's clock. The reference's `GetTime` is `GetTickCount` scaled by 0.001, an OS
    /// clock that never restarts, which stock `Cooldown.lua` relies on (`start > 0`). Set once, at
    /// construction; after that only [`Self::tick`] moves it.
    pub fn set_now(&mut self, secs: f64) {
        set_field!(self, now, secs);
    }

    /// Push the game clock `GetGameTime()` answers, `(hour, minute)`; 0:00 until the first push.
    pub fn set_game_time(&mut self, hour: u32, minute: u32) {
        set_field!(self, game_time, (hour, minute));
    }
}

/// `GetTime()`'s clock, for a caller holding no model borrow; one that holds it reads `model.now`.
pub(crate) fn now(lua: &Lua) -> f64 {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .now
}

/// Register `GetTime` and `GetGameTime`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // GetTime(): one number, `GetTickCount` times the f64 0.001. Ours is the session clock the tick
    // advances, so `GetTime()` deltas and the `elapsed` handed to OnUpdate agree; reference
    // FrameXML (CastingBarFrame and kin) anchors cast windows on it.
    g.set("GetTime", lua.create_function(|lua, ()| Ok(now(lua)))?)?;
    // GetGameTime(): two numbers, the hour then the minute, the clock `SMSG_LOGIN_SETTIMESPEED`
    // seeds; minute resolution, no seconds.
    g.set(
        "GetGameTime",
        lua.create_function(|lua, ()| {
            let (hour, minute) = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .game_time;
            Ok((f64::from(hour), f64::from(minute)))
        })?,
    )?;
    Ok(())
}
