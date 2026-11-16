//! Two Region-map predicates, each with a frame arm and a region arm: `IsMouseOver([top [, bottom [,
//! left [, right]]]])` (`0x49e100`) and `IsDragging()` (`0x49e0b0`), both answering a Lua boolean.
//!
//! `IsMouseOver` reads each offset as a number in the object's units, 0 when absent or not a
//! number, and answers false for an object with no laid-out rect; otherwise whether the cursor lies
//! strictly inside the rect with each offset added to its edge, y up (`0x488570`):
//! `left + left_off < x < right + right_off` and `bottom + bottom_off < y < top + top_off`.
//! `FloatingChatFrame.lua` fades a chat window in on it every frame.
//!
//! `IsDragging` is true for a frame whose own mouse drag has started (the threshold crossed and
//! `OnDragStart` fired, until the release) or whose parent is dragging (`0x48eb70`); a texture or
//! font string asks its owner frame (`0x488350`).

use mlua::{Lua, Table, Value};

use crate::layout::Rect;
use crate::script::region_map::{set_shared, Side};
use crate::script::Model;
use crate::widget::FrameHandle;

/// The offsets `IsMouseOver` reads at arguments 2 to 5.
type Offsets = (Value, Value, Value, Value);

/// The hit test of `0x488570` against a rect in screen units and the scale that converts them to
/// the object's units, which the offsets are in.
fn hit(
    model: &Model,
    rect: Option<Rect>,
    scale: f32,
    (top, bottom, left, right): &Offsets,
) -> bool {
    let off = |v: &Value| match v {
        Value::Integer(n) => *n as f64,
        Value::Number(n) => *n,
        _ => 0.0,
    };
    let Some(rect) = rect else {
        return false;
    };
    let s = f64::from(scale);
    let edge = |v: f32| f64::from(v) / s;
    let (cx, cy) = model.cursor_pos;
    let (x, y) = (f64::from(cx) / s, f64::from(cy) / s);
    edge(rect.left) + off(left) < x
        && x < edge(rect.right) + off(right)
        && edge(rect.bottom) + off(bottom) < y
        && y < edge(rect.top) + off(top)
}

/// Whether `h` or an ancestor is the source of a started mouse drag.
fn frame_dragging(model: &Model, h: FrameHandle) -> bool {
    let Some(source) = model.drag.as_ref().filter(|d| d.started).map(|d| d.source) else {
        return false;
    };
    let mut at = Some(h);
    while let Some(f) = at {
        if f == source {
            return true;
        }
        at = model.arena.frame(f).and_then(|fr| fr.parent);
    }
    false
}

/// The frame arms, into the shared frame method table `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    set_shared(
        lua,
        m,
        Side::Frame,
        "IsMouseOver",
        |lua, (this, top, bottom, left, right): (Table, Value, Value, Value, Value)| {
            let h = super::frame_handle_of(lua, &this)?;
            super::layout_methods::settle(lua);
            let model = crate::script::model_mut_of(lua);
            let scale = super::layout_methods::eff_scale(&model, h);
            let rect = model.resolved.get(&h).copied();
            Ok(hit(&model, rect, scale, &(top, bottom, left, right)))
        },
    )?;
    set_shared(lua, m, Side::Frame, "IsDragging", |lua, this: Table| {
        let h = super::frame_handle_of(lua, &this)?;
        Ok(frame_dragging(&crate::script::model_mut_of(lua), h))
    })?;
    Ok(())
}

/// The region arms, into the full region method table `m`.
pub(in crate::script) fn install_region(lua: &Lua, m: &Table) -> mlua::Result<()> {
    set_shared(
        lua,
        m,
        Side::Region,
        "IsMouseOver",
        |lua, (this, top, bottom, left, right): (Table, Value, Value, Value, Value)| {
            let rh = crate::script::region::region_handle_of(lua, &this)?;
            super::layout_methods::settle(lua);
            let model = crate::script::model_mut_of(lua);
            let scale = model
                .arena
                .region(rh)
                .map_or(1.0, |r| super::layout_methods::eff_scale(&model, r.owner));
            let rect = model.region_resolved.get(&rh).copied();
            Ok(hit(&model, rect, scale, &(top, bottom, left, right)))
        },
    )?;
    set_shared(lua, m, Side::Region, "IsDragging", |lua, this: Table| {
        let rh = crate::script::region::region_handle_of(lua, &this)?;
        let model = crate::script::model_mut_of(lua);
        Ok(model
            .arena
            .region(rh)
            .is_some_and(|r| frame_dragging(&model, r.owner)))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

    #[test]
    fn the_offsets_move_the_edges_the_cursor_is_tested_against() {
        let mut ui = UiScript::new().unwrap();
        ui.run(
            "F = CreateFrame('Frame', nil, UIParent)
             F:SetWidth(100) F:SetHeight(50)
             F:SetPoint('BOTTOMLEFT', UIParent, 'BOTTOMLEFT', 200, 100)
             T = F:CreateTexture() T:SetAllPoints(F)
             S = F:CreateFontString() S:SetAllPoints(F)",
        )
        .unwrap();
        ui.mouse_move(250.0, 120.0);
        for obj in ["F", "T", "S"] {
            assert!(ui
                .eval::<bool>(&format!("return {obj}:IsMouseOver()"))
                .unwrap());
            // A positive bottom offset raises the bottom edge past the cursor.
            assert!(!ui
                .eval::<bool>(&format!("return {obj}:IsMouseOver(0, 25)"))
                .unwrap());
        }
        // 20 above the top edge: outside, until the top offset reaches it.
        ui.mouse_move(250.0, 170.0);
        assert!(!ui.eval::<bool>("return T:IsMouseOver()").unwrap());
        assert!(ui
            .eval::<bool>("return F:IsMouseOver(28, -2, -2, 2)")
            .unwrap());
        // On the edge is outside: the test is strict.
        ui.mouse_move(200.0, 120.0);
        assert!(!ui.eval::<bool>("return F:IsMouseOver()").unwrap());
        assert!(!ui.eval::<bool>("return S:IsMouseOver()").unwrap());
    }

    #[test]
    fn is_dragging_answers_a_boolean_for_frames_and_regions() {
        let ui = UiScript::new().unwrap();
        ui.run("F = CreateFrame('Frame') T = F:CreateTexture() S = F:CreateFontString()")
            .unwrap();
        for obj in ["F", "T", "S"] {
            assert_eq!(
                ui.eval::<String>(&format!("return type({obj}:IsDragging())"))
                    .unwrap(),
                "boolean"
            );
            assert!(!ui
                .eval::<bool>(&format!("return {obj}:IsDragging()"))
                .unwrap());
        }
    }
}
