//! Frame methods: movable and resizable windows, and the drag pumps behind `StartMoving` and
//! `StartSizing`. The gesture that usually calls them, `OnDragStart`/`OnDragStop`, is
//! [`crate::script::cursor`]'s.
//!
//! The flags are bits of `[frame+0xb4]`, set: movable `0x100`, resizable `0x200`, userPlaced
//! `0x1000`. A drag (the drag start, `0x494060`) flattens the frame to one TOPLEFT against the
//! screen at its current rect with an explicit size (the flatten, `0x48a5f0`), so a frame a clamp
//! holds off its anchors moves from where it is drawn; it then raises it, sets userPlaced and takes
//! the one drag slot. The pump (`0x493ce0` → `0x48abd0`) adds each cursor delta, over the frame's
//! scale, into that anchor's offsets; stopping (`0x493a10`) unflattens it to the nearest of the
//! screen's nine points (the unflatten, `0x48a840`).

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Point};
use crate::script::{Model, SCREEN};
use crate::widget::FrameHandle;

use super::frame_handle_of;
use super::layout_methods::eff_scale;

/// The one in-flight move, the reference's drag slot, held in [`Model::moving`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameMove {
    /// The frame being moved (`root+0xcfc`).
    frame: FrameHandle,
    /// The cursor position at the last pump (`root+0xd08`/`+0xd0c`), UI px, y-up.
    sample: (f32, f32),
    /// Whether mouse-up ends the move, the drag mode as Lua sees it: the reference ends modes 1
    /// (modifier drag) and 2 (title region), never 3 (`StartMoving`).
    pub(crate) auto_stop: bool,
}

/// An in-flight `StartSizing` drag: which frame, which grip, and the cursor at the last pump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameSizing {
    frame: FrameHandle,
    /// The edges the named grip moves: `LEFT` the left edge, `BOTTOMRIGHT` the bottom and right.
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
    sample: (f32, f32),
}

/// Populate `m`'s movable and resizable methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetMovable / IsMovable: a pure flag write, so clearing it mid-drag stops nothing. The
    // argument is Lua truthiness, as the reference's `toboolean`.
    m.set(
        "SetMovable",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            set_flag(lua, &this, Flag::Movable, flag)
        })?,
    )?;
    m.set(
        "IsMovable",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::Movable))?,
    )?;
    // SetResizable / IsResizable: the same pure flag write.
    m.set(
        "SetResizable",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            set_flag(lua, &this, Flag::Resizable, flag)
        })?,
    )?;
    m.set(
        "IsResizable",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::Resizable))?,
    )?;
    // ── Resize bounds ──
    //
    // `SetMinResize`, `GetMinResize`, `SetMaxResize`, `GetMaxResize`. Both arguments must pass
    // `lua_isnumber`, else `Usage:`; the pair is stored raw, with no layout touch and no
    // `min > max` check. The getters always answer two numbers, 0 meaning unbounded.
    for (name, upper) in [("MinResize", false), ("MaxResize", true)] {
        m.set(
            format!("Set{name}"),
            lua.create_function(move |lua, args: MultiValue| {
                let mut it = args.into_iter();
                let this = match it.next() {
                    Some(Value::Table(t)) => t,
                    _ => return Err(mlua::Error::runtime("expected a frame")),
                };
                let h = frame_handle_of(lua, &this)?;
                let pair = (it.next(), it.next());
                // `lua_isnumber`: a number, or a string Lua 5.0's `luaV_tonumber` converts.
                let num = |v: &Option<Value>| match v {
                    Some(Value::Number(n)) => Some(*n as f32),
                    Some(Value::Integer(i)) => Some(*i as f32),
                    Some(Value::String(s)) => s.to_str().ok()?.trim().parse::<f32>().ok(),
                    _ => None,
                };
                let (Some(w), Some(hgt)) = (num(&pair.0), num(&pair.1)) else {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    let (verb, a, b) = if upper {
                        ("SetMaxResize", "maxWidth", "maxHeight")
                    } else {
                        ("SetMinResize", "minWidth", "minHeight")
                    };
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:{verb}({a}, {b})"
                    )));
                };
                let mut model = crate::script::model_mut_of(lua);
                if let Some(f) = model.arena.frame_mut(h) {
                    if upper {
                        f.max_resize = (w, hgt);
                    } else {
                        f.min_resize = (w, hgt);
                    }
                }
                Ok(())
            })?,
        )?;
        m.set(
            format!("Get{name}"),
            lua.create_function(move |lua, this: Table| {
                let h = frame_handle_of(lua, &this)?;
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                let (w, hgt) = model.arena.frame(h).map_or((0.0, 0.0), |f| {
                    if upper {
                        f.max_resize
                    } else {
                        f.min_resize
                    }
                });
                Ok((f64::from(w), f64::from(hgt)))
            })?,
        )?;
    }

    // SetUserPlaced / IsUserPlaced: the bit that makes the layout cache save the frame's position;
    // the one guarded setter, refusing a frame that is neither movable nor resizable.
    m.set(
        "SetUserPlaced",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let ok = model
                .arena
                .frame(h)
                .is_some_and(|f| f.movable || f.resizable);
            if !ok {
                return Err(not_flagged(&model, h, "movable or resizable"));
            }
            if let Some(f) = model.arena.frame_mut(h) {
                f.user_placed = flag;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "IsUserPlaced",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::UserPlaced))?,
    )?;
    m.set(
        "StartMoving",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            start_moving(&mut model, h)
        })?,
    )?;
    // StartSizing(point) (stock caller `FloatingChatFrame.lua:600`): a resize from the named grip,
    // ended by `StopMovingOrSizing`. The reference's per-grip switch moves the named edges with the
    // cursor and keeps the opposite ones.
    m.set(
        "StartSizing",
        lua.create_function(|lua, (this, point): (Table, Option<String>)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            if !model.arena.frame(h).is_some_and(|f| f.resizable) {
                return Err(not_flagged(&model, h, "resizable"));
            }
            // Same "do not start a second one" guard `start_moving` carries.
            if model.sizing.is_some() || model.moving.is_some() {
                return Ok(());
            }
            let p = point.unwrap_or_default().to_ascii_uppercase();
            let (left, right) = (p.contains("LEFT"), p.contains("RIGHT"));
            let (top, bottom) = (p.contains("TOP"), p.contains("BOTTOM"));
            // The raise and userPlaced belong to the drag entry, whatever the grip.
            flatten(&mut model, h);
            super::toplevel::raise(&mut model, h);
            set_user_placed(&mut model, h);
            let sample = model.cursor_pos;
            // A CENTER grip is a move: the pump's case 4 shifts the anchor offsets and returns
            // before the size clamp.
            if p == "CENTER" {
                model.moving = Some(FrameMove {
                    frame: h,
                    sample,
                    // `StartSizing` enters as drag mode 3, which mouse-up never ends.
                    auto_stop: false,
                });
                return Ok(());
            }
            // A name that is no point starts an inert drag: raised and userPlaced, moving nothing.
            model.sizing = Some(FrameSizing {
                frame: h,
                left,
                right,
                top,
                bottom,
                sample,
            });
            Ok(())
        })?,
    )?;
    // StopMovingOrSizing: clears the slot only when it holds this frame.
    m.set(
        "StopMovingOrSizing",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            if model.sizing.is_some_and(|sz| sz.frame == h) {
                model.sizing = None;
                unflatten(&mut model, h);
            }
            if model.moving.is_some_and(|mv| mv.frame == h) {
                model.moving = None;
                unflatten(&mut model, h);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// A bit of the frame's flag word (`[frame+0xb4]`), kept here as an arena field.
#[derive(Clone, Copy)]
enum Flag {
    /// `0x100`.
    Movable,
    /// `0x200`.
    Resizable,
    /// `0x1000`.
    UserPlaced,
}

fn set_flag(lua: &Lua, this: &Table, which: Flag, value: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    if let Some(f) = model.arena.frame_mut(h) {
        match which {
            Flag::Movable => f.movable = value,
            Flag::Resizable => f.resizable = value,
            Flag::UserPlaced => f.user_placed = value,
        }
    }
    Ok(())
}

/// `IsMovable`/`IsResizable`/`IsUserPlaced`: 1 or nil, never a boolean.
fn get_flag(lua: &Lua, this: &Table, which: Flag) -> mlua::Result<Value> {
    let h = frame_handle_of(lua, this)?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model");
    Ok(crate::script::binding_abi::flag(
        model.arena.frame(h).is_some_and(|f| match which {
            Flag::Movable => f.movable,
            Flag::Resizable => f.resizable,
            Flag::UserPlaced => f.user_placed,
        }),
    ))
}

/// The family's refusal, naming the frame as the reference does. The wording is ours: the
/// reference's own strings are unread, known only by their symbol names.
fn not_flagged(model: &Model, h: FrameHandle, want: &str) -> mlua::Error {
    let who = model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| "<anonymous>".into());
    mlua::Error::runtime(format!("Frame {who} is not {want}"))
}

/// `StartMoving`: errors on a frame that is not movable, ignores a second move, then raises the
/// frame, sets userPlaced and takes the slot, in that order.
fn start_moving(model: &mut Model, h: FrameHandle) -> mlua::Result<()> {
    if !model.arena.frame(h).is_some_and(|f| f.movable) {
        return Err(not_flagged(model, h, "movable"));
    }
    // The `root+0xcfc != 0` guard; what the reference does beyond not starting a second move is
    // untraced.
    if model.moving.is_some() {
        return Ok(());
    }
    flatten(model, h);
    // The drag-start raise; the worker applies the toplevel gate.
    super::toplevel::raise(model, h);
    set_user_placed(model, h);
    model.moving = Some(FrameMove {
        frame: h,
        sample: model.cursor_pos,
        // Mode 3: only `StopMovingOrSizing` ends it.
        auto_stop: false,
    });
    Ok(())
}

/// Begin a title-region move, drag mode 2, with no movable check, since nothing on that path reads
/// the bit. False when a move is already in flight.
pub(crate) fn start_title_move(model: &mut Model, h: FrameHandle) -> bool {
    if model.moving.is_some() {
        return false;
    }
    flatten(model, h);
    super::toplevel::raise(model, h);
    set_user_placed(model, h);
    model.moving = Some(FrameMove {
        frame: h,
        sample: model.cursor_pos,
        auto_stop: true,
    });
    true
}

/// End a move the mouse-up ends (drag modes 1 and 2), unflattening the frame as every stop does.
pub(crate) fn stop_auto_move(model: &mut Model) {
    if let Some(mv) = model.moving.take() {
        unflatten(model, mv.frame);
    }
}

/// The flatten (`0x48a5f0`) against the screen: one TOPLEFT at the frame's current rect, clamp
/// included, and that rect's size made explicit, in the frame's own units.
fn flatten(model: &mut Model, h: FrameHandle) {
    crate::script::UiScript::resolve_layout(model);
    let Some(rect) = model.resolved.get(&h).copied() else {
        return;
    };
    let screen = model.screen;
    let inv = 1.0 / eff_scale(model, h);
    let anchor = Anchor::new(
        Point::TopLeft,
        SCREEN,
        Point::TopLeft,
        (rect.left - screen.left) * inv,
        (rect.top - screen.top) * inv,
    );
    let size = (
        (rect.right - rect.left) * inv,
        (rect.top - rect.bottom) * inv,
    );
    set_single_anchor(model, h, anchor, Some(size));
}

/// The unflatten (`0x48a840`): re-anchor to whichever of the screen's nine points lies nearest
/// its counterpart on the frame, keeping the size, so a later screen change carries it along.
fn unflatten(model: &mut Model, h: FrameHandle) {
    crate::script::UiScript::resolve_layout(model);
    let Some(rect) = model.resolved.get(&h).copied() else {
        return;
    };
    let screen = model.screen;
    // `Point`'s order, row by row from the top-left, as the reference's rect-points helper lays
    // them out.
    let at = |r: &crate::layout::Rect, i: usize| {
        let x = [r.left, (r.left + r.right) * 0.5, r.right][i % 3];
        let y = [r.top, (r.top + r.bottom) * 0.5, r.bottom][i / 3];
        (x, y)
    };
    let mut best = (0usize, f32::MAX, 0.0f32, 0.0f32);
    for i in 0..9 {
        let ((fx, fy), (sx, sy)) = (at(&rect, i), at(&screen, i));
        let (dx, dy) = (sx - fx, sy - fy);
        let distance = dx * dx + dy * dy;
        if distance < best.1 {
            best = (i, distance, fx - sx, fy - sy);
        }
    }
    const POINTS: [Point; 9] = [
        Point::TopLeft,
        Point::Top,
        Point::TopRight,
        Point::Left,
        Point::Center,
        Point::Right,
        Point::BottomLeft,
        Point::Bottom,
        Point::BottomRight,
    ];
    let point = POINTS[best.0];
    let inv = 1.0 / eff_scale(model, h);
    let anchor = Anchor::new(point, SCREEN, point, best.2 * inv, best.3 * inv);
    set_single_anchor(model, h, anchor, None);
}

/// Replace the frame's anchors with `anchor`, and its size with `size` when given.
fn set_single_anchor(model: &mut Model, h: FrameHandle, anchor: Anchor, size: Option<(f32, f32)>) {
    let input = model.layout_inputs.entry(h).or_default();
    let old: Vec<u32> = input.anchors.iter().map(|a| a.relative_to).collect();
    input.anchors = vec![anchor];
    if let Some((w, hgt)) = size {
        input.width = w;
        input.height = hgt;
    }
    model.touch_layout_retarget_frame(h, &old, &[SCREEN]);
}

/// Set userPlaced as the drag entry does.
fn set_user_placed(model: &mut Model, h: FrameHandle) {
    if let Some(f) = model.arena.frame_mut(h) {
        f.user_placed = true;
    }
}

/// Apply the `SetMinResize`/`SetMaxResize` bounds to a proposed size per axis, as the reference
/// does: only an exact 0.0 is unbounded, min applies before max so max wins a crossed pair, and
/// with no bound there is no floor, so a size can go negative.
fn clamp_resize(model: &Model, h: FrameHandle, w: f32, hgt: f32) -> (f32, f32) {
    let (min, max) = model
        .arena
        .frame(h)
        .map_or(((0.0, 0.0), (0.0, 0.0)), |f| (f.min_resize, f.max_resize));
    let axis = |v: f32, lo: f32, hi: f32| {
        let v = if lo != 0.0 && v < lo { lo } else { v };
        if hi != 0.0 && v > hi {
            hi
        } else {
            v
        }
    };
    (axis(w, min.0, max.0), axis(hgt, min.1, max.1))
}

/// Pump an in-flight `StartSizing`: the gripped edges follow the cursor, the opposite ones stay.
pub(crate) fn advance_size(model: &mut Model, pos: (f32, f32)) {
    let Some(sz) = model.sizing else { return };
    if model.arena.frame(sz.frame).is_none() {
        model.sizing = None;
        return;
    }
    let (dx, dy) = (pos.0 - sz.sample.0, pos.1 - sz.sample.1);
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let inv = 1.0 / eff_scale(model, sz.frame);
    let (dx, dy) = (dx * inv, dy * inv);
    let dw = if sz.right {
        dx
    } else if sz.left {
        -dx
    } else {
        0.0
    };
    let dh = if sz.top {
        dy
    } else if sz.bottom {
        -dy
    } else {
        0.0
    };
    let Some((w0, h0)) = model
        .layout_inputs
        .get(&sz.frame)
        .map(|i| (i.width, i.height))
    else {
        return;
    };
    // The resize bounds are read only here, as only the resize pump reads them in the reference.
    let (new_w, new_h) = clamp_resize(model, sz.frame, w0 + dw, h0 + dh);
    // The rebate: the residual a clamp swallowed comes back out of the drag delta, so the moving
    // edge stops at the bound instead of the window walking. The delta is already in local units,
    // and an ungripped axis is not rebated.
    let rebate = |d: f32, want: f32, got: f32, grew: bool, gripped: bool| {
        if !gripped {
            return d;
        }
        let residual = got - want;
        if grew {
            d + residual
        } else {
            d - residual
        }
    };
    let dx = rebate(dx, w0 + dw, new_w, sz.right, sz.left || sz.right);
    let dy = rebate(dy, h0 + dh, new_h, sz.top, sz.top || sz.bottom);
    let Some(input) = model.layout_inputs.get_mut(&sz.frame) else {
        return;
    };
    input.width = new_w;
    input.height = new_h;
    let mut moved = input.width.to_bits() != w0.to_bits() || input.height.to_bits() != h0.to_bits();
    // The flattened frame hangs from its TOPLEFT, so a LEFT or TOP grip moves that point with its
    // edge and the opposite edges stay (the drag step, `0x48abd0`); RIGHT and BOTTOM change the
    // size alone.
    if !input.anchors.is_empty() && (sz.left || sz.top) {
        for a in &mut input.anchors {
            if sz.left {
                a.x_off += dx;
            }
            if sz.top {
                a.y_off += dy;
            }
        }
        moved |= (sz.left && dx != 0.0) || (sz.top && dy != 0.0);
    }
    // A nonzero cursor delta can still move nothing (a grip dragged across its axis, or held at a
    // bound), so the layout is touched only on a real change.
    if moved {
        // No anchor is retargeted, so only this node is touched.
        model.touch_layout_frame(sz.frame);
    }
    model.sizing = Some(FrameSizing { sample: pos, ..sz });
}

/// Pump an in-flight move to the cursor at `pos`. Called from
/// [`crate::script::UiScript::mouse_move`] before its hover-boundary early return, since a frame
/// dragged under the cursor crosses no boundary.
pub(crate) fn advance_move(model: &mut Model, pos: (f32, f32)) {
    let Some(mv) = model.moving else { return };
    if model.arena.frame(mv.frame).is_none() {
        model.moving = None;
        return;
    }
    let (dx, dy) = (pos.0 - mv.sample.0, pos.1 - mv.sample.1);
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    // Anchor offsets are in the frame's own units, so the screen delta divides by its scale, as the
    // reference's case 4 does.
    let inv = 1.0 / eff_scale(model, mv.frame);
    let (dx, dy) = (dx * inv, dy * inv);
    if let Some(input) = model.layout_inputs.get_mut(&mv.frame) {
        if !input.anchors.is_empty() {
            for a in &mut input.anchors {
                a.x_off += dx;
                a.y_off += dy;
            }
            // No target moves, so only this node is touched; a zero delta already returned above.
            model.touch_layout_frame(mv.frame);
        }
    }
    model.moving = Some(FrameMove { sample: pos, ..mv });
}
