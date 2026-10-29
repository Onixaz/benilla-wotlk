//! Frame methods: the `toplevel` flag, `Raise` and `Lower`, and the raise every trigger runs.
//!
//! The flag is bit `0x1` of `[frame+0xb4]`, set by the reference from XML or `SetToplevel`, and
//! setting it raises nothing. `CSimpleTop::Raise`:
//!
//! 1. Walks up `+0x9c` to the nearest toplevel self-or-ancestor; with none it does nothing.
//! 2. With `force`, resolves the frame's layout and tests whether it is overlapped: a visible frame
//!    of its stratum at its level or above, neither itself nor a descendant, meets its rect.
//! 3. Only then compresses the stratum's levels below its bound, hidden frames included
//!    (`0x495060`), and sets its level to the new bound, moving same-stratum children by the same
//!    delta (`0x4910a0`).
//!
//! The stratum never changes. The overlapped bit is recomputed per raise, never stored: only the
//! `force = 0` trigger reads a stored bit, and it is not wired, so neither is the per-tick pass
//! that keeps it. The wired triggers are `Raise`, Show, drag start and mouse-down.

use mlua::{Lua, Table, Value};

use crate::layout::Rect;
use crate::script::{Model, UiScript};
use crate::widget::FrameHandle;

use super::frame_handle_of;

/// Populate `m`'s toplevel and raise methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetToplevel's argument is optional and defaults to true, unlike `SetMovable`'s.
    m.set(
        "SetToplevel",
        lua.create_function(|lua, (this, flag): (Table, Option<Value>)| {
            let on = match flag {
                None => true,
                Some(v) => !matches!(v, Value::Nil | Value::Boolean(false)),
            };
            let h = frame_handle_of(lua, &this)?;
            crate::script::model_mut_of(lua).arena.set_toplevel(h, on);
            Ok(())
        })?,
    )?;
    m.set(
        "IsToplevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(model.arena.is_toplevel(h)))
        })?,
    )?;
    // Raise() (`force = 1`): legal on any frame, and silent when no toplevel self-or-ancestor
    // exists.
    m.set(
        "Raise",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            raise(&mut model, h);
            Ok(())
        })?,
    )?;
    // Lower() does nothing in the reference, which is `xor eax,eax; ret 4`.
    m.set("Lower", lua.create_function(|_, _this: Table| Ok(()))?)?;
    Ok(())
}

/// `CSimpleTop::Raise(frame, force = 1)`, the module doc's three steps; true, as in the reference,
/// when a toplevel self-or-ancestor was found, whether or not it moved.
pub(in crate::script) fn raise(model: &mut Model, frame: FrameHandle) -> bool {
    // 1. The nearest toplevel self-or-ancestor, else nothing.
    let Some(t) = nearest_toplevel(model, frame) else {
        return false;
    };
    // 2. Resolve the pending layout, then test for overlap. Deviation: an `OnSizeChanged` that
    //    resolve produces fires at the next `UiScript::resolve`, because a Lua handler must not run
    //    in the middle of a level compaction.
    UiScript::resolve_layout(model);
    if !overlapped(model, t) {
        return true;
    }
    // 3. The raise (`0x4951a0`): compress the stratum, then land on its new bound.
    let Some(strata) = model.arena.frame(t).map(|f| f.strata) else {
        return true;
    };
    let top = model.arena.compress_strata(strata);
    model.arena.set_frame_level(t, top, true);
    true
}

/// The Show trigger: on becoming visible, raise only if the frame's own toplevel bit is set, so
/// showing a child never lifts its window.
pub(in crate::script) fn raise_on_show(model: &mut Model, h: FrameHandle) {
    if model.arena.is_toplevel(h) {
        raise(model, h);
    }
}

/// Step 1: the first frame up `+0x9c` with the toplevel bit.
fn nearest_toplevel(model: &Model, frame: FrameHandle) -> Option<FrameHandle> {
    // A chain longer than the frame count is a cycle, which `SetParent` already refuses.
    let bound = model.arena.iter_frames().count();
    let mut cur = Some(frame);
    let mut guard = 0usize;
    while let Some(h) = cur {
        let f = model.arena.frame(h)?;
        if f.toplevel {
            return Some(h);
        }
        cur = f.parent;
        guard += 1;
        if guard > bound {
            break;
        }
    }
    None
}

/// Step 2: a visible frame of `t`'s stratum at `t`'s level or above, neither `t` nor its
/// descendant, whose rect meets `t`'s with area.
fn overlapped(model: &Model, t: FrameHandle) -> bool {
    let Some(tf) = model.arena.frame(t) else {
        return false;
    };
    let (strata, level) = (tf.strata, tf.level);
    let Some(t_rect) = model.resolved.get(&t).copied() else {
        // No resolved rect: not on screen, so nothing overlaps it.
        return false;
    };
    model.arena.iter_frames().any(|(h, f)| {
        h != t
            && f.effective_visible
            && f.strata == strata
            && f.level >= level
            && !model.arena.is_ancestor(t, h)
            && model
                .resolved
                .get(&h)
                .is_some_and(|r| rects_overlap(t_rect, *r))
    })
}

/// Whether two rects intersect with area: the reference reads both, intersects them and tests the
/// result.
fn rects_overlap(a: Rect, b: Rect) -> bool {
    let i = crate::script::clip::intersect_rect(a, b);
    i.width() > 0.0 && i.height() > 0.0
}
