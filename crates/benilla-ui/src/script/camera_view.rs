//! The camera-view verbs from the reference's Lua table: `SetView`, `SaveView`, `ResetView`,
//! `NextView`, `PrevView` and `FlipCameraYaw`. Each acts on engine state the VM does not hold, so
//! each call queues a [`CameraViewRequest`] for the app's camera rig.
//!
//! The indexed verbs share one prologue and return silently, never reaching `luaL_error`, on
//! anything but a number that truncates to 1 to 5; `FlipCameraYaw` keeps its argument's fraction.
//! `SaveView(1)` and `ResetView(1)` act on first person (the reference has no view-0 gate; only
//! `Bindings.xml` lacks the bindings). `NextView` and `PrevView` stop at the ends and never wrap.

use mlua::{Lua, Value};

/// The number of camera views, the rows of the reference's default-string table.
pub const CAMERA_VIEW_COUNT: u8 = 5;

/// A camera-view intent for the app. Indices are internal, `0..CAMERA_VIEW_COUNT`, range-checked
/// here where the reference checks them, in the Lua handler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraViewRequest {
    /// `SetView(n)`: make view `n-1` live (distance, pitch and the saved yaw offset).
    Set(u8),
    /// `SaveView(n)`: store the live pose into view `n-1`.
    Save(u8),
    /// `ResetView(n)`: put view `n-1` back to its shipped default.
    Reset(u8),
    /// `NextView()`: the view above the current one, if any.
    Next,
    /// `PrevView()`: the view below the current one, if any.
    Prev,
    /// `FlipCameraYaw(degrees)`: add degrees to the camera's yaw; `Bindings.xml:795` passes 180.
    FlipYaw(f32),
}

impl super::UiScript {
    /// Drain the camera-view intents queued since the last call.
    pub fn take_camera_view_requests(&mut self) -> Vec<CameraViewRequest> {
        drain_field!(self, camera_view_requests)
    }
}

/// The indexed verbs' shared prologue: the internal index, or `None` for a silent return.
fn view_arg(lua: &Lua, v: Value) -> Option<u8> {
    // Is-number and `tonumber` take a numeric string; the cast truncates toward zero, as the
    // reference does.
    let n = lua.coerce_number(v).ok().flatten()? as i64 as i32;
    (1..=i32::from(CAMERA_VIEW_COUNT))
        .contains(&n)
        .then(|| (n - 1) as u8)
}

/// Register the six camera-view globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SetView(n). The reference's also writes the `cameraView` CVar, so the view survives a
    // restart; the app does both.
    g.set(
        "SetView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n) {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Set(view));
            }
            Ok(())
        })?,
    )?;

    // SaveView(n): the camera's target distance, pitch and yaw go into the slot and each to its
    // archived CVar.
    g.set(
        "SaveView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n) {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Save(view));
            }
            Ok(())
        })?,
    )?;

    // ResetView(n): re-parses the view's default string into the slot and re-applies the view if it
    // is live.
    g.set(
        "ResetView",
        lua.create_function(|lua, n: Value| {
            if let Some(view) = view_arg(lua, n) {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::Reset(view));
            }
            Ok(())
        })?,
    )?;

    // NextView() and PrevView(): neither reads the Lua stack.
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

    // FlipCameraYaw(degrees): adds `degrees × π/180` to the camera's yaw, fraction kept; a
    // non-number returns silently.
    g.set(
        "FlipCameraYaw",
        lua.create_function(|lua, degrees: Value| {
            if let Some(d) = lua.coerce_number(degrees).ok().flatten() {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .camera_view_requests
                    .push(CameraViewRequest::FlipYaw(d as f32));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}
