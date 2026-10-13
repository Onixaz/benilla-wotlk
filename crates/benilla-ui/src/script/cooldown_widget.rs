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
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
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
            let changed = lua
                .app_data_mut::<Model>()
                .expect("model app_data")
                .arena
                .set_shown(h, true);
            event::fire_visibility_changes(lua, changed);
            Ok(())
        })?,
    )?;
    // SetReverse(flag) (`0x5ec9a0`) and SetDrawEdge(flag) (`0x5eca30`) store `lua_toboolean`
    // (`0x815500`); the getters answer the stored flag as a boolean (`0x84e4d0`).
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

/// The shown cooldowns' per-frame update (`0x5ec8f0`), after the tick's `OnUpdate` walk: each
/// advances its clock, and one whose flash ended hides, firing its visibility events. A hidden
/// cooldown's clock stands still until it shows again, as only shown frames are updated.
pub(super) fn tick(lua: &Lua) {
    let hide: Vec<FrameHandle> = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let now = now_ms(&model);
        let ticked: Vec<FrameHandle> = model.arena.ticked_kinds().to_vec();
        ticked
            .into_iter()
            .filter(|&h| {
                let Some(f) = model.arena.frame_mut(h) else {
                    return false;
                };
                if !f.effective_visible {
                    return false;
                }
                match &mut f.kind_state {
                    // Unarmed too: a shown `CreateFrame("Cooldown")` runs an empty flash and hides
                    // itself; only the paint waits for `SetCooldown`.
                    KindState::Cooldown(c) => c.update(now) == CooldownStep::Hide,
                    _ => false,
                }
            })
            .collect()
    };
    for h in hide {
        let changed = lua
            .app_data_mut::<Model>()
            .expect("model app_data")
            .arena
            .set_shown(h, false);
        event::fire_visibility_changes(lua, changed);
    }
}
