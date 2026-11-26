//! The camera-view verbs from the reference's Lua table (`0xad2060`): `SetView`, `SaveView`,
//! `ResetView`, `NextView`, `PrevView` and `FlipCameraYaw`. Each acts on engine state the VM does
//! not hold, so each call queues a [`CameraViewRequest`] for the app's camera rig.
//!
//! 12340 has eight internal views (`0..8`: `""`, `A`..`E`, `Com`, `Barber Shop`). `SetView`
//! (`0x6039b0`), `SaveView` (`0x5ff260`) and `ResetView` (`0x604c80`) raise their usage string on a
//! non-number argument, pass `1..=5` straight through as internal indices `1..=5` (`A`..`E`), and
//! return silently for any other number. `NextView` (`0x604ce0`) and `PrevView` (`0x604d10`) step
//! within `1..=5` without wrapping. `FlipCameraYaw` (`0x5ff2c0`) raises its usage string on a
//! non-number argument and keeps its argument's fraction.

use mlua::{Lua, Value};

/// The number of internal camera views (`0xad1b54`, `0xad1ba8`): `""`, `A`..`E`, `Com`,
/// `Barber Shop`.
pub const CAMERA_VIEW_COUNT: u8 = 8;

/// The user-selectable view range (`A`..`E`), accepted by `SetView`/`SaveView`/`ResetView` and
/// walked by `NextView`/`PrevView`.
pub const CAMERA_USER_VIEW_MIN: u8 = 1;
pub const CAMERA_USER_VIEW_MAX: u8 = 5;

/// A camera-view intent for the app. Indices are internal (`1..=5` for Lua's `A`..`E`),
/// range-checked here where the reference checks them, in the Lua handler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraViewRequest {
    /// `SetView(n)`: make internal view `n` (`1..=5`, `A`..`E`) live.
    Set(u8),
    /// `SaveView(n)`: store the live pose into internal view `n` (`1..=5`, `A`..`E`).
    Save(u8),
    /// `ResetView(n)`: put internal view `n` (`1..=5`, `A`..`E`) back to its shipped default.
    Reset(u8),
    /// `NextView()`: the view above the current one within `1..=5`, if any.
    Next,
    /// `PrevView()`: the view below the current one within `1..=5`, if any.
    Prev,
    /// `FlipCameraYaw(degrees)`: add degrees to the camera's yaw; `Bindings.xml` passes 180.
    FlipYaw(f32),
}

impl super::UiScript {
    /// Drain the camera-view intents queued since the last call.
    pub fn take_camera_view_requests(&mut self) -> Vec<CameraViewRequest> {
        drain_field!(self, camera_view_requests)
    }
}

/// The indexed verbs' shared prologue: raises `usage` when `v` fails `lua_isnumber`, else returns
/// the internal view index (`1..=5`) or `None` for an out-of-range number.
fn view_arg(lua: &Lua, v: Value, usage: &str) -> mlua::Result<Option<u8>> {
    // `lua_isnumber` / `lua_tonumber` take a numeric string; the cast truncates toward zero, as the
    // reference does.
    let Some(num) = lua.coerce_number(v).ok().flatten() else {
        return Err(mlua::Error::runtime(usage));
    };
    let n = num as i64 as i32;
    Ok(
        (i32::from(CAMERA_USER_VIEW_MIN)..=i32::from(CAMERA_USER_VIEW_MAX))
            .contains(&n)
            .then_some(n as u8),
    )
}

/// Register the six camera-view globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `SetView(viewModeIndex)` (`0x6039b0`).
    g.set(
        "SetView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n, "Usage: SetView(viewModeIndex)")? {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Set(view));
            }
            Ok(())
        })?,
    )?;

    // `SaveView(viewModeIndex)` (`0x5ff260`): the camera's target distance, pitch and yaw go into
    // the slot and each to its archived CVar.
    g.set(
        "SaveView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n, "Usage: SaveView(viewModeIndex)")? {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Save(view));
            }
            Ok(())
        })?,
    )?;

    // `ResetView(viewModeIndex)` (`0x604c80`): re-parses the view's default string into the slot
    // and re-applies the view if it is live.
    g.set(
        "ResetView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n, "Usage: ResetView(viewModeIndex)")? {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Reset(view));
            }
            Ok(())
        })?,
    )?;

    // `NextView()` (`0x604ce0`) and `PrevView()` (`0x604d10`): neither reads the Lua stack.
    g.set(
        "NextView",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.camera_view_requests.push(CameraViewRequest::Next);
            Ok(())
        })?,
    )?;
    g.set(
        "PrevView",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.camera_view_requests.push(CameraViewRequest::Prev);
            Ok(())
        })?,
    )?;

    // `FlipCameraYaw(degrees)` (`0x5ff2c0`): adds `degrees × π/180` to the camera's yaw, fraction
    // kept; a non-number raises the usage string.
    g.set(
        "FlipCameraYaw",
        lua.create_function(|lua, degrees: Value| {
            let Some(d) = lua.coerce_number(degrees).ok().flatten() else {
                return Err(mlua::Error::runtime("Usage: FlipCameraYaw(degrees)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            model
                .camera_view_requests
                .push(CameraViewRequest::FlipYaw(d as f32));
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn indexed_verbs_pass_one_through_five_as_internal_indices() {
        let mut s = UiScript::new().expect("new script");
        s.eval::<()>(
            r#"
            SetView(1)
            SetView("2")
            SaveView(3.9)
            ResetView(5)
            SetView(0)
            SetView(6)
            SaveView(-1)
            ResetView(7)
            NextView()
            PrevView()
            FlipCameraYaw(180.5)
            "#,
        )
        .expect("valid calls succeed");
        assert_eq!(
            s.take_camera_view_requests(),
            vec![
                CameraViewRequest::Set(1),
                CameraViewRequest::Set(2),
                CameraViewRequest::Save(3),
                CameraViewRequest::Reset(5),
                CameraViewRequest::Next,
                CameraViewRequest::Prev,
                CameraViewRequest::FlipYaw(180.5),
            ]
        );
    }

    #[test]
    fn non_numeric_arguments_raise_reference_usage() {
        let s = UiScript::new().expect("new script");
        for (call, expected) in [
            ("SetView()", "Usage: SetView(viewModeIndex)"),
            ("SetView('x')", "Usage: SetView(viewModeIndex)"),
            ("SaveView()", "Usage: SaveView(viewModeIndex)"),
            ("ResetView(nil)", "Usage: ResetView(viewModeIndex)"),
            ("FlipCameraYaw()", "Usage: FlipCameraYaw(degrees)"),
        ] {
            let err = s
                .eval::<()>(call)
                .expect_err("non-numeric call should raise")
                .to_string();
            assert!(err.contains(expected), "{call}: got {err:?}");
        }
    }
}
