//! The three mirror timers (breath, fatigue, feign death) as `SMSG_START_MIRROR_TIMER` last stated
//! them, the client's table at `0xbd0b80`, and the two globals that read it: `GetMirrorTimerInfo`
//! (`0x51cc10`), which stock `MirrorTimer.lua` polls on `PLAYER_ENTERING_WORLD`, and
//! `GetMirrorTimerProgress` (`0x517aa0`).

use mlua::{Lua, Value};

/// The tokens by record index, `MirrorTimerTypeToken` (`0x513e00`); an empty record is `"UNKNOWN"`.
pub const MIRROR_TIMER_TOKENS: [&str; 3] = ["EXHAUSTION", "BREATH", "FEIGNDEATH"];

/// One running timer, a 0x1c-byte record: value, max value and scale in ms as the wire sends them,
/// the paused byte, the owning spell's name, and the `GetTime()` ms it was stated at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MirrorTimer {
    pub value: i32,
    pub max_value: i32,
    pub scale: i32,
    pub paused: i32,
    /// The owning spell's name when its row exists, which `GetMirrorTimerLabel` (`0x5199a0`) reads
    /// before `<TOKEN>_LABEL`.
    pub spell_name: Option<String>,
    /// Stamped by [`super::UiScript::start_mirror_timer`].
    pub started_ms: u32,
}

const USAGE: &str = "Usage: GetMirrorTimerInfo(\"timer\")";
const INVALID: &str = "ERROR: Invalid timer (Ex: \"BREATH\"";

impl super::UiScript {
    /// `SMSG_START_MIRROR_TIMER`: record `index` (the wire type) takes the timer, stamped now
    /// (`0x519c40`).
    pub fn start_mirror_timer(&mut self, index: usize, mut timer: MirrorTimer) {
        let mut model = self.model_mut();
        timer.started_ms = now_ms(model.now);
        if let Some(slot) = model.mirror_timers.get_mut(index) {
            *slot = Some(timer);
        }
    }

    /// `SMSG_STOP_MIRROR_TIMER`, and each record when the client leaves the world
    /// (`CGGameUI::CancelMirrorTimers`, `0x513ea0`): the record empties to `"UNKNOWN"`.
    pub fn stop_mirror_timer(&mut self, index: usize) {
        if let Some(slot) = self.model_mut().mirror_timers.get_mut(index) {
            *slot = None;
        }
    }

    /// Record `index` as the table holds it, its stamp included.
    pub fn mirror_timer(&self, index: usize) -> Option<MirrorTimer> {
        self.model_ref().mirror_timers.get(index).cloned().flatten()
    }

    /// Put a record back as it was, its stamp kept: the host's copy into a rebuilt VM, whose
    /// `GetTime` clock carries on (`Self::set_now`), so the timer reads on where it was.
    pub fn restore_mirror_timer(&mut self, index: usize, timer: Option<MirrorTimer>) {
        if let Some(slot) = self.model_mut().mirror_timers.get_mut(index) {
            *slot = timer;
        }
    }

    /// The bar's caption, `GetMirrorTimerLabel` (`0x5199a0`): the owning spell's name, else the
    /// `<TOKEN>_LABEL` string, "" when there is none.
    pub fn mirror_timer_label(&self, index: usize, spell_name: Option<&str>) -> String {
        label(&self.lua, index, spell_name)
    }
}

/// `OsGetAsyncTimeMs` as the VM keeps it: `GetTime()` in whole ms, wrapping as a `u32` does.
fn now_ms(now: f64) -> u32 {
    (now * 1000.0) as u64 as u32
}

fn label(lua: &Lua, index: usize, spell_name: Option<&str>) -> String {
    if let Some(name) = spell_name {
        return name.to_string();
    }
    let token = MIRROR_TIMER_TOKENS.get(index).copied().unwrap_or("UNKNOWN");
    super::get_text::get_text(lua, &format!("{token}_LABEL"), -1, 0)
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
}

/// Register `GetMirrorTimerInfo` and `GetMirrorTimerProgress`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // A number or `Usage:` (`0x51cc26`), truncated; 1..3 or the invalid-timer error (`0x51cce5`).
    // Six values: token, value, max value, scale, paused, label.
    g.set(
        "GetMirrorTimerInfo",
        lua.create_function(|lua, index: Value| {
            let index = lua
                .coerce_number(index)?
                .ok_or_else(|| mlua::Error::RuntimeError(USAGE.into()))?;
            let slot = (index.trunc() as i64)
                .checked_sub(1)
                .and_then(|i| usize::try_from(i).ok())
                .filter(|&i| i < MIRROR_TIMER_TOKENS.len())
                .ok_or_else(|| mlua::Error::RuntimeError(INVALID.into()))?;
            let timer = crate::script::model_mut_of(lua).mirror_timers[slot].clone();
            let (token, timer) = match timer {
                Some(t) => (MIRROR_TIMER_TOKENS[slot], t),
                None => ("UNKNOWN", MirrorTimer::default()),
            };
            let label = label(lua, slot, timer.spell_name.as_deref());
            Ok((
                token,
                f64::from(timer.value),
                f64::from(timer.max_value),
                f64::from(timer.scale),
                f64::from(timer.paused),
                label,
            ))
        })?,
    )?;

    // A string or `Usage:` (`0x517ab7`), matched case-blind (`0x513e40`); an unknown name raises
    // (`0x517b39`). The value now: `(now - stated) * scale + value`, in the client's int math.
    g.set(
        "GetMirrorTimerProgress",
        lua.create_function(|lua, timer: Value| {
            let name = match timer {
                Value::String(s) => s.to_string_lossy(),
                Value::Integer(n) => n.to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(mlua::Error::RuntimeError(USAGE.into())),
            };
            let slot = MIRROR_TIMER_TOKENS
                .iter()
                .position(|t| t.eq_ignore_ascii_case(&name))
                .ok_or_else(|| mlua::Error::RuntimeError(INVALID.into()))?;
            let model = crate::script::model_mut_of(lua);
            let timer = model.mirror_timers[slot].clone().unwrap_or_default();
            let elapsed = now_ms(model.now).wrapping_sub(timer.started_ms) as i32;
            Ok(f64::from(
                elapsed.wrapping_mul(timer.scale).wrapping_add(timer.value),
            ))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn an_empty_record_answers_unknown_and_zeros() {
        let s = UiScript::new().unwrap();
        let info: (String, f64, f64, f64, f64, String) =
            s.eval("return GetMirrorTimerInfo(2)").unwrap();
        assert_eq!(info, ("UNKNOWN".into(), 0.0, 0.0, 0.0, 0.0, String::new()));
    }

    #[test]
    fn a_started_timer_reads_back_and_progresses_on_the_clock() {
        let mut s = UiScript::new().unwrap();
        s.run("BREATH_LABEL = 'Breath'").unwrap();
        s.start_mirror_timer(
            1,
            MirrorTimer {
                value: 60_000,
                max_value: 60_000,
                scale: -1,
                ..Default::default()
            },
        );
        let info: (String, f64, f64, f64, f64, String) =
            s.eval("return GetMirrorTimerInfo(2)").unwrap();
        assert_eq!(
            info,
            (
                "BREATH".into(),
                60_000.0,
                60_000.0,
                -1.0,
                0.0,
                "Breath".into()
            )
        );
        s.tick(1.5);
        assert_eq!(
            s.eval::<f64>("return GetMirrorTimerProgress('breath')")
                .unwrap(),
            58_500.0,
            "1.5 s at scale -1 off 60 s, the name matched case-blind"
        );
        s.stop_mirror_timer(1);
        assert_eq!(
            s.eval::<String>("return (GetMirrorTimerInfo(2))").unwrap(),
            "UNKNOWN"
        );
    }

    #[test]
    fn the_spell_name_wins_the_label() {
        let mut s = UiScript::new().unwrap();
        s.start_mirror_timer(
            1,
            MirrorTimer {
                spell_name: Some("Water Breathing".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            s.eval::<String>("return select(6, GetMirrorTimerInfo(2))")
                .unwrap(),
            "Water Breathing"
        );
    }

    #[test]
    fn a_bad_argument_raises() {
        let s = UiScript::new().unwrap();
        for call in [
            "GetMirrorTimerInfo()",
            "GetMirrorTimerInfo(0)",
            "GetMirrorTimerInfo(4)",
            "GetMirrorTimerProgress()",
            "GetMirrorTimerProgress('UNKNOWN')",
        ] {
            assert!(s.run(call).is_err(), "{call} raises");
        }
    }
}
