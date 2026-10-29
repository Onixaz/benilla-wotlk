//! The `Cooldown` widget's Lua face (method table `0xad13a4`, registered by `0x5ebe10` over the
//! frame table) and its per-frame update; the state and paint are
//! [`CooldownState`](crate::widget::CooldownState)'s. Stock `Cooldown.xml` declares
//! `CooldownFrameTemplate` as one, and `CooldownFrame_SetTimer` drives it with `SetCooldown`.

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::{event, Model};
use crate::widget::{CooldownState, CooldownStep, FrameHandle, KindState};

pub(super) const REG_COOLDOWN_METHODS: &str = "__benilla_cooldown_methods";

/// Run `f` over a frame's Cooldown state; errors on any other receiver, since a caller can lift
/// the method table onto another frame.
fn with_cooldown<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut CooldownState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Cooldown(c)) => Ok(f(c)),
        Some(_) => Err(mlua::Error::runtime("not a Cooldown")),
        None => Err(mlua::Error::runtime("stale frame handle")),
    }
}

/// `GetTime()` in whole ms, the reference's tick count (`0x86ae20`), wrapping as a `u32` does.
fn now_ms(model: &Model) -> u32 {
    (model.now * 1000.0) as u64 as u32
}

/// A `lua_isnumber` argument (`0x84df20`), seconds, as the binding's truncated ms (`× 1000.0`,
/// `0x9e6070`, then `fistp` under a truncating control word), kept to its low 32 bits.
fn arg_ms(lua: &Lua, v: &Value) -> Option<u32> {
    let secs = lua.coerce_number(v.clone()).ok().flatten()?;
    Some((secs * 1000.0).trunc() as i64 as u32)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetCooldown(start, duration) (`0x5ece80`): both numbers, else Usage (`0xa1d21c`); then the
    // clock restarts (`0x5ecd70`) and the frame shows (`0x5ece69`).
    m.set(
        "SetCooldown",
        lua.create_function(|lua, (this, start, duration): (Table, Value, Value)| {
            let (Some(start), Some(duration)) = (arg_ms(lua, &start), arg_ms(lua, &duration))
            else {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetCooldown(start, duration)",
                    who.as_deref()
                        .unwrap_or(super::object::anchor_args::UNNAMED)
                )));
            };
            with_cooldown(lua, &this, |c| c.set(start, duration))?;
            let h = frame_handle_of(lua, &this)?;
            event::set_shown(lua, h, true);
            Ok(())
        })?,
    )?;
    // SetReverse(flag) (`0x5ec9a0`) and SetDrawEdge(flag) (`0x5eca30`) store `lua_toboolean`; the
    // getters answer the stored flag as a boolean (`0x84e4d0`).
    m.set(
        "SetReverse",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            with_cooldown(lua, &this, |c| c.reverse = truthy(&on))
        })?,
    )?;
    m.set(
        "GetReverse",
        lua.create_function(|lua, this: Table| with_cooldown(lua, &this, |c| c.reverse))?,
    )?;
    m.set(
        "SetDrawEdge",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            with_cooldown(lua, &this, |c| c.draw_edge = truthy(&on))
        })?,
    )?;
    m.set(
        "GetDrawEdge",
        lua.create_function(|lua, this: Table| with_cooldown(lua, &this, |c| c.draw_edge))?,
    )?;

    lua.set_named_registry_value(REG_COOLDOWN_METHODS, m)?;
    Ok(())
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Boolean(false))
}

/// The Cooldown widget's layer update, in the cooldown's walk slot after the base update: the clock
/// advances, and a cooldown whose flash ended hides, firing its visibility events. A hidden
/// cooldown's clock stands still until it shows again, as the walk reaches only shown frames.
pub(super) fn update(lua: &Lua, h: FrameHandle) {
    // A step that moved nothing drawn (an unarmed box) leaves the mutation count where it stood
    // (`ModelCell`).
    let hide = {
        let mut model = crate::script::model_mut_of(lua);
        let quiet = model.epoch();
        let now = now_ms(&model);
        let step = match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            // Unarmed too: a shown `CreateFrame("Cooldown")` runs an empty flash and hides itself;
            // only the paint waits for `SetCooldown`.
            Some(KindState::Cooldown(c)) => {
                let before = c.sweep_key();
                let step = c.update(now);
                Some((step, c.sweep_key() != before))
            }
            _ => None,
        };
        if !step.is_some_and(|(_, swept)| swept) {
            model.set_epoch(quiet);
        }
        step.is_some_and(|(step, _)| step == CooldownStep::Hide)
    };
    if hide {
        event::set_shown(lua, h, false);
    }
}
