//! The box's own per-frame update (vtable slot `+0x38`), which the tick's walk runs in the box's
//! place, and its flush, which drains the dirty word.

use mlua::{Lua, Value};

use super::{event, frame_id_of, read_eb, relayout, with_eb, CARET_WIDTH};
use crate::widget::{EditBoxState, FrameHandle};

/// The box's update after its Lua `OnUpdate`: the flush, then the caret blink. The walk runs it for
/// every shown box, with an `OnUpdate` or without.
pub(in crate::script) fn update(lua: &Lua, h: FrameHandle, dt: f32) {
    flush(lua, h);
    blink(lua, h, dt);
}

/// The flush (`0x967020`). It samples bit 0 at entry; on bit 0 the relayout runs and the bit clears;
/// on bit 2 of the word as the relayout left it, the bit clears and then the caret leg runs, with
/// the blink and `OnCursorChanged`; last, `OnTextChanged` fires if bit 0 was set at entry, and bit
/// 3 clears after it. So a handler's edit raises bits for the next flush, a frame later, and never
/// loops inside one: an `OnTextChanged` that sets its own text fires once more, next frame, and an
/// `OnCursorChanged` that moves the caret reports the new place next frame. Besides the walk, the
/// box's `OnKeyDown` and `OnMouseDown` run it before the key or click acts.
///
/// Deviation: the flush's promotion of bit 2 to bit 0 when the caret has left the display window,
/// with its IME legs, and the IME writer's `or 6` are not modelled: the promotion only relayouts,
/// which re-windows a single-line box whose window benilla keeps host-side, and fires nothing,
/// since bit 0 is sampled before it; benilla has no IME composition.
pub(super) fn flush(lua: &Lua, h: FrameHandle) {
    let Some((mut dirty, owed, caret_owed)) =
        read_eb(lua, h, |eb| (eb.dirty, eb.relayout_owed, eb.caret_owed))
    else {
        return;
    };
    let text_changed = dirty & EditBoxState::DIRTY_TEXT != 0;
    if text_changed || owed {
        let done = relayout::relayout(lua, h);
        let left = with_eb(lua, h, |eb| {
            eb.dirty &= !EditBoxState::DIRTY_TEXT;
            eb.relayout_owed = !done;
            eb.dirty
        });
        dirty = left.unwrap_or(0);
    }
    if dirty & EditBoxState::DIRTY_CURSOR != 0 || caret_owed {
        with_eb(lua, h, |eb| eb.dirty &= !EditBoxState::DIRTY_CURSOR);
        let placed = caret(lua, h);
        with_eb(lua, h, |eb| eb.caret_owed = !placed);
    }
    if text_changed {
        text_changed_script(lua, h);
        with_eb(lua, h, |eb| eb.dirty &= !EditBoxState::DIRTY_USER_INPUT);
    }
}

/// `OnTextChanged(userInput)` (`0x965460`): bit 3 as read when the script runs, a real boolean.
fn text_changed_script(lua: &Lua, h: FrameHandle) {
    let user_input = read_eb(lua, h, |eb| eb.dirty & EditBoxState::DIRTY_USER_INPUT != 0);
    let id = frame_id_of(lua, h);
    let args = vec![Value::Boolean(user_input.unwrap_or(false))];
    if let Err(e) = event::fire_widget_handler(lua, id, "OnTextChanged", args) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
}

/// The caret leg: it restarts the blink, fires `OnCursorChanged`, then shows the caret in the
/// focused box and hides it elsewhere. So a box that takes the keyboard, whose focus change raises
/// bit 2, starts its caret solid. `false` while the advance table is pending, and
/// `EditBoxState::caret_owed` has the next flush retry.
fn caret(lua: &Lua, h: FrameHandle) -> bool {
    with_eb(lua, h, |eb| eb.blink_accum = 0.0);
    let placed = cursor_changed(lua, h);
    let focused = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .focused_editbox
        == Some(h);
    with_eb(lua, h, |eb| eb.caret_shown = focused);
    placed
}

/// `OnCursorChanged(x, y, w, h)` in the box's own units (`0x964a90`), `x` the caret's advance along
/// its line, `y` minus the row index times the row pitch, `w` the constant 4.0 and `h` the line
/// height. The arguments are computed only for a box with the script, from the box's advance
/// table, which the installed font engine measures here when the text or font moved. `false` while
/// that table is pending, with no engine installed: the host answers the focused box's a tick
/// later.
///
/// Deviation: `h` is the row pitch the advance answer carries, where the reference passes the line
/// height: 0 for a single-line box, whose answer has no pitch, and for a multi-line box taller than
/// the line height only for a font with extra spacing (none in the stock UI).
fn cursor_changed(lua: &Lua, h: FrameHandle) -> bool {
    let id = frame_id_of(lua, h);
    if !event::has_widget_handler(lua, id, "OnCursorChanged") {
        return true;
    }
    if !super::seam::advances_current(lua, h) {
        return false;
    }
    let args = with_eb(lua, h, |eb| {
        let display = eb.display();
        if eb.advances.len() != display.len() + 1 {
            return None;
        }
        let cursor_d = eb.text_to_display(eb.cursor).min(display.len());
        let (row, x) = if eb.multi_line {
            eb.caret_row_x(cursor_d)
        } else {
            (0, eb.advances[cursor_d])
        };
        let pitch = eb.cell_h;
        // `y` starts at 0 and loses one pitch per row above the caret, so row 0 reports +0, never
        // the -0 that Lua prints as "-0".
        let y = 0.0 - row as f32 * pitch;
        Some((x, y, pitch))
    })
    .flatten();
    let Some((x, y, pitch)) = args else {
        return false;
    };
    // The advance table and the pitch are root units, the seam's.
    let scale = crate::script::object::layout_methods::eff_scale(
        &lua.app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data"),
        h,
    );
    let (x, y, pitch) = (x / scale, y / scale, pitch / scale);
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnCursorChanged",
        vec![
            Value::Number(f64::from(x)),
            Value::Number(f64::from(y)),
            Value::Number(f64::from(CARET_WIDTH)),
            Value::Number(f64::from(pitch)),
        ],
    ) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
    true
}

/// The caret blink (`0x967670`): without window focus (`0x682df0`) a shown caret hides and the
/// accumulator stands, so the blink resumes from it when the focus returns. Past the period (0.5 s
/// by default) the focused box's caret toggles and the accumulator resets; only a period of exactly
/// 0.0 keeps the caret solid.
///
/// Deviation: not gated on the cursor lying inside the scroll window, which benilla's host keeps
/// around the caret.
fn blink(lua: &Lua, h: FrameHandle, dt: f32) {
    let (window, focused) = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        (model.window_focused, model.focused_editbox == Some(h))
    };
    if window && !focused {
        return;
    }
    let Some((shown, period)) = read_eb(lua, h, |eb| (eb.caret_shown, eb.blink_period)) else {
        return;
    };
    if if window { period == 0.0 } else { !shown } {
        return;
    }
    // The blink is no extract input (the host draws the caret from the text-UI seam), so the
    // mutation count stands.
    let quiet = crate::script::model_mut_of(lua).epoch();
    with_eb(lua, h, |eb| {
        if !window {
            eb.caret_shown = false;
            return;
        }
        eb.blink_accum += dt;
        if eb.blink_accum > eb.blink_period {
            eb.caret_shown = !eb.caret_shown;
            eb.blink_accum = 0.0;
        }
    });
    crate::script::model_mut_of(lua).set_epoch(quiet);
}
