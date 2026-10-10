//! 3.3.5a's `Frame:IsMouseOver([top [, bottom [, left [, right]]]])` (`0x49e100`).
//!
//! Each offset is a number in the frame's units, 0 when absent or not a number; the hit test
//! (`0x488570`) answers false for a frame with no laid-out rect and otherwise whether the cursor
//! lies in the rect grown by the offsets: `left + left_off <= x <= right + right_off` and
//! `bottom + bottom_off <= y <= top + top_off`. `FloatingChatFrame.lua` fades a chat window in on
//! it every frame. No 1.12.1 method table binds it.

use mlua::{Lua, ObjectLike, Table, Value};

use crate::script::Model;

/// Add `IsMouseOver` to the shared frame method table `m`, on a target that has it.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    if !lua.app_data_ref::<Model>().expect("model").is_mouse_over {
        return Ok(());
    }
    m.set(
        "IsMouseOver",
        lua.create_function(
            |lua, (this, top, bottom, left, right): (Table, Value, Value, Value, Value)| {
                let off = |v: &Value| match v {
                    Value::Integer(n) => *n as f64,
                    Value::Number(n) => *n,
                    _ => 0.0,
                };
                let edge = |name: &str| this.call_method::<Option<f64>>(name, ());
                let (Some(l), Some(r), Some(t), Some(b)) = (
                    edge("GetLeft")?,
                    edge("GetRight")?,
                    edge("GetTop")?,
                    edge("GetBottom")?,
                ) else {
                    return Ok(false);
                };
                let scale: f64 = this.call_method("GetEffectiveScale", ())?;
                let (cx, cy) = lua.app_data_ref::<Model>().expect("model").cursor_pos;
                let (x, y) = (f64::from(cx) / scale, f64::from(cy) / scale);
                Ok(l + off(&left) <= x
                    && x <= r + off(&right)
                    && b + off(&bottom) <= y
                    && y <= t + off(&top))
            },
        )?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use benilla_expansions::Expansion;

    use crate::script::UiScript;

    #[test]
    fn the_offsets_grow_the_rect_the_cursor_is_tested_against() {
        let mut ui = UiScript::new_for(Expansion::Wotlk).unwrap();
        ui.run(
            "F = CreateFrame('Frame', nil, UIParent)
             F:SetWidth(100) F:SetHeight(50)
             F:SetPoint('BOTTOMLEFT', UIParent, 'BOTTOMLEFT', 200, 100)",
        )
        .unwrap();
        ui.mouse_move(250.0, 120.0);
        assert!(ui.eval::<bool>("return F:IsMouseOver()").unwrap());
        // 20 above the top edge: outside, until the top offset reaches it.
        ui.mouse_move(250.0, 170.0);
        assert!(!ui.eval::<bool>("return F:IsMouseOver()").unwrap());
        assert!(ui
            .eval::<bool>("return F:IsMouseOver(28, -2, -2, 2)")
            .unwrap());
    }

    #[test]
    fn classic_frames_have_no_is_mouse_over() {
        let ui = UiScript::new_for(Expansion::Classic).unwrap();
        assert!(ui
            .eval::<bool>("return CreateFrame('Frame').IsMouseOver == nil")
            .unwrap());
    }
}
