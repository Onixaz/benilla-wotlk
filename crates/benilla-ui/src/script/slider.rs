//! The `Slider` methods of `CSimpleSlider` (`Slider::LoadXML`): a value in `[min, max]` with a step
//! and an orientation, driving a thumb texture along the track, whose placement follows the
//! documented widget model, untraced in the reference. `SetValue` fires `OnValueChanged` (slot
//! `+0x330`) on the first value and then only on a change (`+0x314` bit 2, in `SetValue`), which
//! ends the loop between the stock scrollbar's `OnValueChanged` and its ScrollFrame's
//! `OnVerticalScroll`. The methods exist only on Slider frames, as in the client's per-class method
//! sets.

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::pointer::point_in_rect;
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::layout::Rect;
use crate::order::DrawLayer;
use crate::widget::{
    slider_fraction, slider_grab, FrameHandle, KindState, RegionKind, SliderState,
};

pub(super) const REG_SLIDER_METHODS: &str = "__benilla_slider_methods";

/// Run `f` over a frame's Slider state under one short write borrow; errors unless `this` is a live
/// Slider, since a caller can lift the method table onto another frame.
fn with_slider<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut SliderState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Slider(s) => Ok(f(s)),
        _ => Err(mlua::Error::runtime("not a Slider")),
    }
}

/// Get or create the thumb texture region and return its id; `layer` re-layers an existing thumb,
/// and a new one defaults to OVERLAY.
fn ensure_thumb(lua: &Lua, this: &Table, layer: Option<DrawLayer>) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);

    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::Slider(s) => s.thumb,
        _ => return Err(mlua::Error::runtime("not a Slider")),
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
                    layer.unwrap_or(DrawLayer::Overlay),
                    0,
                )
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            model.region_data.insert(rh, RegionData::default());
            // Unanchored, it is no layout node until something anchors it.
            model.region_id(rh);
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Slider(s) = &mut frame.kind_state {
                    s.thumb = Some(rh);
                }
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

/// `Enable`/`Disable` (`0x9720e0`, `0x972150`): set or clear flag `0x400`, then run `OnEnable` or
/// `OnDisable` on every call, a change or not, unless the frame is still in its XML load
/// (`+0x130`).
fn set_enabled(lua: &Lua, this: &Table, on: bool) -> mlua::Result<()> {
    let h = super::object::frame_handle_of(lua, this)?;
    let fire = {
        let mut model = crate::script::model_mut_of(lua);
        let frame = model
            .arena
            .frame_mut(h)
            .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
        match &mut frame.kind_state {
            KindState::Slider(s) => s.disabled = !on,
            _ => return Err(mlua::Error::runtime("not a Slider")),
        }
        (!model.xml_loading.contains(&h)).then(|| model.frame_id(h))
    };
    if let Some(id) = fire {
        let script = if on { "OnEnable" } else { "OnDisable" };
        if let Err(e) = super::event::fire_widget_handler(lua, id, script, Vec::new()) {
            crate::script::model_mut_of(lua).record_script_error(e.to_string());
        }
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "Enable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, true))?,
    )?;
    m.set(
        "Disable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, false))?,
    )?;
    // `IsEnabled` (`0x972050`): 1 or nil, unlike a Button's 1 or 0.
    m.set(
        "IsEnabled",
        lua.create_function(|lua, this: Table| {
            with_slider(lua, &this, |s| super::binding_abi::flag(!s.disabled))
        })?,
    )?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, f32, f32)| {
            // A reversed pair is kept, not swapped. A held value re-clamps and fires on a move; a
            // fresh slider with no value yet fires nothing (bit 2).
            let (changed, range) = with_slider(lua, &this, |s| {
                let before = (s.range_valid, s.min, s.max);
                let changed = s.set_min_max(min, max);
                (
                    changed,
                    (before != (true, s.min, s.max)).then_some((s.min, s.max)),
                )
            })?;
            super::statusbar::fire_min_max_changed(lua, &this, range)?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetMinMaxValues",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| (s.min, s.max)))?,
    )?;
    m.set(
        "SetValue",
        lua.create_function(|lua, (this, v): (Table, f32)| {
            let changed = with_slider(lua, &this, |s| s.store_value(v))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValue",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| s.value))?,
    )?;
    m.set(
        "SetValueStep",
        // Stores the step, then re-pushes the range as `SetMinMaxValues` does, so a held value
        // moved onto the new step fires `OnValueChanged`.
        lua.create_function(|lua, (this, step): (Table, f32)| {
            let changed = with_slider(lua, &this, |s| s.set_value_step(step))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValueStep",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| s.step))?,
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
            with_slider(lua, &this, |s| s.vertical = vertical)
        })?,
    )?;
    m.set(
        "GetOrientation",
        lua.create_function(|lua, this: Table| {
            let v = with_slider(lua, &this, |s| s.vertical)?;
            Ok(if v { "VERTICAL" } else { "HORIZONTAL" }.to_string())
        })?,
    )?;

    // A 1.12 Slider has no enabled state: `Enable`/`Disable`/`IsEnabled` exist only in the Button
    // table, so a Slider must not answer them.

    // `SetThumbTexture(path [, layer])` or `(r, g, b [, a])`, a region's two `SetTexture` forms, on
    // the thumb region, created on first use.
    m.set(
        "SetThumbTexture",
        lua.create_function(
            |lua, (this, a1, a2, a3, a4): (Table, Value, Value, Value, Value)| {
                let layer = match &a2 {
                    Value::String(s) => s.to_str().ok().and_then(|l| draw_layer_from_str(&l)),
                    _ => None,
                };
                let id = ensure_thumb(lua, &this, layer)?;
                let rh = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    *model.id_to_region.get(&id).expect("thumb region id")
                };
                let mut model = crate::script::model_mut_of(lua);
                let data = model.region_data.entry(rh).or_default();
                match &a1 {
                    Value::String(s) => {
                        data.texture = Some(s.to_str()?.to_string());
                        data.fill = None;
                    }
                    // A solid thumb and a path share one slot; each clears the other.
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
        "GetThumbTexture",
        lua.create_function(|lua, this: Table| {
            let thumb = with_slider(lua, &this, |s| s.thumb)?;
            let id = {
                let mut model = crate::script::model_mut_of(lua);
                thumb.map(|rh| model.region_id(rh))
            };
            match id {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    lua.set_named_registry_value(REG_SLIDER_METHODS, m)?;
    Ok(())
}

/// Fire `OnValueChanged` (slot `+0x330`) when `changed` carries a new value, outside any model
/// borrow; errors are recorded in [`Model::errors`].
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
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
    Ok(())
}

fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

// ── Thumb geometry and drag ──────────────────────────────────────────────────────────────────

/// The thumb's rect at `fraction` along the track, centred across it and inset so it travels
/// `trackLen - thumbLen`; vertical puts fraction 0 at the top. Render and [`begin_drag`] both use
/// it, so they never disagree. `thumb_size` is [`thumb_extent`]'s.
pub(super) fn thumb_rect(r: Rect, thumb_size: (f32, f32), vertical: bool, fraction: f32) -> Rect {
    let (tw, th) = thumb_size;
    if vertical {
        let travel = (r.height() - th).max(0.0);
        let top = r.top - fraction * travel; // f=0 → track top, f=1 → track bottom
        let cx = (r.left + r.right) * 0.5;
        Rect::new(top - th, cx - tw * 0.5, top, cx + tw * 0.5)
    } else {
        let travel = (r.width() - tw).max(0.0);
        let left = r.left + fraction * travel;
        let cy = (r.bottom + r.top) * 0.5;
        Rect::new(cy - th * 0.5, left, cy + th * 0.5, left + tw)
    }
}

/// The thumb region's own width and height, as the reference reads them through the texture's
/// geometry slots (`[vtable+0x1c]`/`+0x20`): the authored span, else the art's texel span, else 0.
/// `None` without a thumb region: the reference gates the value math on `+0x328`, so a thumbless
/// slider captures a press but never moves.
fn thumb_extent(model: &Model, thumb: Option<crate::widget::RegionHandle>) -> Option<(f32, f32)> {
    Some(super::region::virtual_span(model, thumb?))
}

/// The in-flight thumb drag and the grab offset [`slider_grab`] returned, so the thumb tracks the
/// cursor without jumping.
#[derive(Clone, Copy)]
pub(crate) struct SliderDrag {
    pub(crate) slider: FrameHandle,
    /// The grab as distance from the track's leading edge, the axis-neutral frame [`slider_grab`]
    /// and [`slider_fraction`] share with the glue screens.
    pub(crate) grab_offset: f32,
}

/// On a LeftButton press on an enabled Slider `hit`, capture a drag at the [`slider_grab`] offset;
/// a disabled one starts none (`0x96c406`). Returns `(frame id, new value)` when the press jumps
/// the value, for the caller to fire `OnValueChanged` outside the model borrow.
pub(super) fn begin_drag(
    model: &mut Model,
    hit: Option<FrameHandle>,
    x: f32,
    y: f32,
) -> Option<(u32, f32)> {
    let h = hit?;
    let r = model.resolved.get(&h).copied()?;
    let (vertical, fraction, thumb) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Slider(s)) if !s.disabled => (s.vertical, s.fraction(), s.thumb),
        _ => return None,
    };
    // No thumb region: the press still captures, the dispatcher's store being unconditional, but
    // the value never moves (`+0x328`).
    let Some(size) = thumb_extent(model, thumb) else {
        model.slider_drag = Some(SliderDrag {
            slider: h,
            grab_offset: 0.0,
        });
        return None;
    };
    let trect = thumb_rect(r, size, vertical, fraction);
    let on_thumb = point_in_rect(trect, x, y);
    // Distances from the track's leading edge, where the thumb sits at `min`: `top - y` on a
    // vertical track in this y-up arena, `x - left` on a horizontal one.
    let (cursor, thumb_lead, thumb_len) = if vertical {
        (r.top - y, r.top - trect.top, trect.top - trect.bottom)
    } else {
        (x - r.left, trect.left - r.left, trect.right - trect.left)
    };
    model.slider_drag = Some(SliderDrag {
        slider: h,
        grab_offset: slider_grab(cursor, thumb_lead, thumb_len),
    });
    if on_thumb {
        return None; // no value change from the grab itself
    }
    drag_move(model, x, y)
}

/// On a pointer move during a capture, set the value from the cursor through [`slider_fraction`];
/// returns `(frame id, new value)` when it changed, for the caller to fire.
pub(super) fn drag_move(model: &mut Model, x: f32, y: f32) -> Option<(u32, f32)> {
    let SliderDrag {
        slider,
        grab_offset,
    } = *model.slider_drag.as_ref()?;
    let r = model.resolved.get(&slider).copied()?;
    let (min, max, vertical, thumb) = match model.arena.frame(slider).map(|f| &f.kind_state) {
        Some(KindState::Slider(s)) => (s.min, s.max, s.vertical, s.thumb),
        _ => return None, // slider destroyed mid-drag
    };
    let (tw, th) = thumb_extent(model, thumb)?;
    // The leading-edge frame `begin_drag` stored the grab in.
    let (cursor, track_extent, thumb_len) = if vertical {
        (r.top - y, r.height(), th)
    } else {
        (x - r.left, r.width(), tw)
    };
    // `None` when the thumb is as long as its track.
    let fraction = slider_fraction(cursor, grab_offset, track_extent, thumb_len)?;
    let value = min + fraction * (max - min);
    let changed = match model.arena.frame_mut(slider).map(|f| &mut f.kind_state) {
        Some(KindState::Slider(s)) => s.store_value(value),
        _ => None,
    };
    changed.map(|v| (model.frame_id(slider), v))
}

/// Release any in-flight thumb drag (LeftButton up, or the pointer leaving the window).
pub(super) fn end_drag(model: &mut Model) {
    model.slider_drag = None;
}
