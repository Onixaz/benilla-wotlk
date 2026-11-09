//! The `StatusBar` methods. The fill is a left-anchored crop, the art sliced and never squeezed:
//! `SetValue` sets the UV's `u1` to the fill fraction and the right edge with it, applied at
//! extract. Only StatusBar frames answer these, so a duck-typing addon sees nil on every other
//! kind.

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::region::region_wrapper;
use super::{event, RegionData};
use crate::order::DrawLayer;
use crate::widget::{KindState, RegionKind, StatusBarState};

/// Registry key of the StatusBar method table.
pub(super) const REG_STATUSBAR_METHODS: &str = "__benilla_statusbar_methods";

/// Run `f` over a frame's StatusBar state; errors on any other receiver, since the method table
/// is a plain Lua value a caller can misapply.
fn with_bar<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut StatusBarState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::StatusBar(sb) => Ok(f(sb)),
        _ => Err(mlua::Error::runtime("not a StatusBar")),
    }
}

/// Store the value clamped to the range, `Some` when it changed (the caller fires
/// `OnValueChanged`); a degenerate range pins to `min`.
fn store_value(sb: &mut StatusBarState, v: f32) -> Option<f32> {
    let clamped = v.clamp(sb.min, sb.max.max(sb.min));
    (clamped != sb.value).then(|| {
        sb.value = clamped;
        clamped
    })
}

/// Get or create the bar texture region, `ARTWORK` by default as in the reference; `layer`
/// re-layers an existing one.
fn ensure_bar(lua: &Lua, this: &Table, layer: Option<DrawLayer>) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);

    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::StatusBar(sb) => sb.bar,
        _ => return Err(mlua::Error::runtime("not a StatusBar")),
    };

    let rh = match existing {
        Some(rh) => {
            if let (Some(l), Some(region)) = (layer, model.arena.region_mut(rh)) {
                region.draw_layer = l;
            }
            rh
        }
        None => {
            let rh = model
                .arena
                .create_region(
                    h,
                    RegionKind::Texture,
                    layer.unwrap_or(DrawLayer::Artwork),
                    0,
                )
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            model.region_data.insert(rh, RegionData::default());
            model.region_id(rh); // unanchored, no layout node until something anchors it
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::StatusBar(sb) = &mut frame.kind_state {
                    sb.bar = Some(rh);
                }
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, Value, Value)| {
            let (Some(min), Some(max)) = (lua.coerce_number(min)?, lua.coerce_number(max)?) else {
                return Err(usage_error(lua, &this));
            };
            // The binding (`0x9713c0`) bounds both and their spread; XML takes the same two lines.
            if let Some(message) = range_error(min, max) {
                return Err(mlua::Error::runtime(message));
            }
            set_min_max(lua, &this, min as f32, max as f32)
        })?,
    )?;
    m.set(
        "GetMinMaxValues",
        lua.create_function(|lua, this: Table| with_bar(lua, &this, |sb| (sb.min, sb.max)))?,
    )?;
    m.set(
        "SetValue",
        lua.create_function(|lua, (this, v): (Table, f32)| {
            let changed = with_bar(lua, &this, |sb| store_value(sb, v))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValue",
        lua.create_function(|lua, this: Table| with_bar(lua, &this, |sb| sb.value))?,
    )?;
    m.set(
        "SetOrientation",
        lua.create_function(|lua, (this, o): (Table, String)| {
            let vertical = match o.to_ascii_uppercase().as_str() {
                "HORIZONTAL" => false,
                "VERTICAL" => true,
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "SetOrientation: unknown orientation '{o}'"
                    )))
                }
            };
            with_bar(lua, &this, |sb| sb.vertical = vertical)
        })?,
    )?;
    m.set(
        "GetOrientation",
        lua.create_function(|lua, this: Table| {
            let v = with_bar(lua, &this, |sb| sb.vertical)?;
            Ok(if v { "VERTICAL" } else { "HORIZONTAL" }.to_string())
        })?,
    )?;

    // A region's two `SetTexture` forms, `(path [, layer])` and `(r, g, b [, a])`, on the bar.
    m.set(
        "SetStatusBarTexture",
        lua.create_function(
            |lua, (this, a1, a2, a3, a4): (Table, Value, Value, Value, Value)| {
                let layer = match &a2 {
                    Value::String(s) => s.to_str().ok().and_then(|l| draw_layer_from_str(&l)),
                    _ => None,
                };
                let id = ensure_bar(lua, &this, layer)?;
                let rh = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    *model.id_to_region.get(&id).expect("bar region id")
                };
                let mut model = crate::script::model_mut_of(lua);
                let data = model.region_data.entry(rh).or_default();
                match &a1 {
                    Value::String(s) => {
                        data.texture = Some(s.to_str()?.to_string());
                        data.fill = None;
                    }
                    // A solid fill and a path clear each other; `SetStatusBarColor` is a separate
                    // tint that multiplies either.
                    Value::Number(_) | Value::Integer(_) => {
                        data.texture = None;
                        data.fill = Some([
                            num_f32(&a1),
                            num_f32(&a2),
                            num_f32(&a3),
                            match &a4 {
                                Value::Nil => 1.0,
                                v => num_f32(v),
                            },
                        ]);
                    }
                    _ => {}
                }
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetStatusBarTexture",
        lua.create_function(|lua, this: Table| {
            let bar = with_bar(lua, &this, |sb| sb.bar)?;
            let id = {
                let mut model = crate::script::model_mut_of(lua);
                bar.map(|rh| model.region_id(rh))
            };
            match id {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // The reference reads r, g, b by a bare `lua_tonumber`, so nil, a table or a non-numeric string
    // is 0.0 and the call never raises (a numeric string also reads 0.0 here). Stock hands colour
    // setters nils: `QuestLogFrame.lua:337` passes three to `SetVertexColor`.
    m.set(
        "SetStatusBarColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    crate::script::object::as_f32(&r),
                    crate::script::object::as_f32(&g),
                    crate::script::object::as_f32(&b),
                );
                let id = ensure_bar(lua, &this, None)?;
                let mut model = crate::script::model_mut_of(lua);
                let rh = *model.id_to_region.get(&id).expect("bar region id");
                model.region_data.entry(rh).or_default().vertex_color =
                    Some([r, g, b, a.unwrap_or(1.0)]);
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetStatusBarColor",
        lua.create_function(|lua, this: Table| {
            let bar = with_bar(lua, &this, |sb| sb.bar)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let c = bar
                .and_then(|rh| model.region_data.get(&rh))
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    lua.set_named_registry_value(REG_STATUSBAR_METHODS, m)?;
    Ok(())
}

/// The binding's and the XML load's bound on a StatusBar's range.
pub(crate) fn range_error(min: f64, max: f64) -> Option<&'static str> {
    const LIMIT: f64 = 1.0e12;
    if !(-LIMIT..=LIMIT).contains(&min) || !(-LIMIT..=LIMIT).contains(&max) {
        Some("Min or Max out of range") // `0xaa1794`
    } else if max - min > LIMIT {
        Some("Min and Max too far apart") // `0xaa17ac`
    } else {
        None
    }
}

/// `Usage: <name>:SetMinMaxValues(min, max)` (`0xaa1770`), shared by the StatusBar and Slider.
pub(super) fn usage_error(lua: &Lua, this: &Table) -> mlua::Error {
    let name = frame_handle_of(lua, this).ok().and_then(|h| {
        let model = crate::script::model_mut_of(lua);
        model.arena.frame(h).and_then(|f| f.name.clone())
    });
    mlua::Error::runtime(format!(
        "Usage: {}:SetMinMaxValues(min, max)",
        name.as_deref().unwrap_or("<unnamed>")
    ))
}

/// The setter (`0x961800`) past the binding's checks: a reversed pair clamps to `min = max`; an
/// unchanged range (`0x961819`) fires nothing and leaves the value, a new one fires
/// `OnMinMaxChanged` first (`0x961859`), then re-clamps a held value and fires `OnValueChanged`.
pub(crate) fn set_min_max(lua: &Lua, this: &Table, min: f32, max: f32) -> mlua::Result<()> {
    let min = min.min(max);
    let moved = with_bar(lua, this, |sb| {
        if sb.range_set && (sb.min, sb.max) == (min, max) {
            return None;
        }
        (sb.min, sb.max) = (min, max);
        sb.range_set = true;
        Some(store_value(sb, sb.value))
    })?;
    let Some(changed) = moved else {
        return Ok(());
    };
    fire_min_max_changed(lua, this, Some((min, max)))?;
    fire_value_changed(lua, this, changed)
}

/// Fire `OnMinMaxChanged(min, max)` when `range` carries a new range: the StatusBar's (slot
/// `+0x2bc`, `0x961560`) and the Slider's (`+0x2c0`, `0x96bbb0`), each run by its
/// `SetMinMaxValues` only when the pair changed or was never set (`0x961819`, `0x96c484`), and
/// with no load guard. Errors are recorded in [`Model::errors`].
pub(super) fn fire_min_max_changed(
    lua: &Lua,
    this: &Table,
    range: Option<(f32, f32)>,
) -> mlua::Result<()> {
    let Some((min, max)) = range else {
        return Ok(());
    };
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = crate::script::model_mut_of(lua);
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnMinMaxChanged",
        vec![Value::Number(f64::from(min)), Value::Number(f64::from(max))],
    ) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
    Ok(())
}

/// Fire `OnValueChanged` (the StatusBar's script slot, `+0x32c`) for a changed value, outside any
/// model borrow.
fn fire_value_changed(lua: &Lua, this: &Table, changed: Option<f32>) -> mlua::Result<()> {
    let Some(value) = changed else { return Ok(()) };
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = crate::script::model_mut_of(lua);
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnValueChanged",
        vec![Value::Number(f64::from(value))],
    ) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
    Ok(())
}

/// `OnValueChanged` for a bar the engine moved, the nameplate health bars: the reference re-sets
/// them from a GUID-watch callback on every server update, and that `SetValue` fires the script as
/// any other does.
pub(super) fn fire_engine_value_changed(lua: &Lua, bar: crate::widget::FrameHandle, value: f32) {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        model.frame_id(bar)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnValueChanged",
        vec![Value::Number(f64::from(value))],
    ) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
}

fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}
