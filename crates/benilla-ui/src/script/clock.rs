//! The two clocks Lua reads: `GetTime()`, the session seconds [`UiScript::tick`] advances and a
//! live VM runs on from between ticks, and
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
        let model = self.model_ref();
        model.clock_wall.set(std::time::Instant::now());
        model.clock_floor.set(0.0);
    }

    /// Push the game clock `GetGameTime()` answers, `(hour, minute)`; 0:00 until the first push.
    pub fn set_game_time(&mut self, hour: u32, minute: u32) {
        set_field!(self, game_time, (hour, minute));
    }
}

/// `GetTime()`'s clock, for a caller holding no model borrow; one that holds it reads `model.now`.
/// A live VM runs on from the host's last set, as the reference's OS clock does under a long
/// handler; a value once read is a floor, so the next set never reads earlier.
pub(crate) fn now(lua: &Lua) -> f64 {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let mut t = model.now;
    if model.clock_live.get() {
        t += model.clock_wall.get().elapsed().as_secs_f64();
    }
    let t = t.max(model.clock_floor.get());
    model.clock_floor.set(t);
    t
}

/// Register `GetTime` and `GetGameTime`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // GetTime() (`0x6081f0`): one number, the OS millisecond clock times the f64 0.001, read live
    // on every call, so a time-sliced handler sees its own budget run down.
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

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// On a live clock a handler spinning on `GetTime()` sees it run on, while the session clock
    /// moves by the host's elapsed alone, and no later read answers earlier.
    #[test]
    fn a_live_read_runs_on_without_advancing_the_session_clock() {
        let mut s = UiScript::new().unwrap();
        s.set_live_clock(true);
        s.set_now(100.0);
        s.run(
            r#"
            local f = CreateFrame("Frame")
            f:SetScript("OnUpdate", function()
                local start = GetTime()
                while GetTime() - start < 0.002 do end
                Last = GetTime()
            end)
            "#,
        )
        .unwrap();
        for _ in 0..5 {
            s.tick(1.0);
        }
        assert_eq!(s.now(), 105.0);
        let last = s.eval::<f64>("return Last").unwrap();
        assert!((105.002..106.0).contains(&last), "{last}");
        assert!(s.eval::<f64>("return GetTime()").unwrap() >= last);
        s.tick(0.0);
        assert!(s.eval::<f64>("return Last").unwrap() >= last);
    }

    /// Off, the clock holds between the host's sets, so a test reads exact values.
    #[test]
    fn a_held_clock_reads_the_host_value_exactly() {
        let mut s = UiScript::new().unwrap();
        s.set_now(7.5);
        assert_eq!(s.eval::<f64>("return GetTime()").unwrap(), 7.5);
        s.tick(0.25);
        assert_eq!(s.eval::<f64>("return GetTime()").unwrap(), 7.75);
    }
}
