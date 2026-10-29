//! The per-character camera pose: 3.3.5a keeps it in two per-character CVars,
//! `cameraSavedDistance` and `cameraSavedPitch` (`CameraRegisterCVars` `0x5fd942`, `0x5fd99a`),
//! so it lives in the character's `config-cache.wtf` with the other rows of that scope.
//!
//! `CGCamera::SaveCameraSettings` writes them with `%f` at the UI shutdown, inside the character
//! slot's save; `0x5ff3e0` restores them once that file has loaded, the distance clamped to 0..50
//! and the pitch, saved in degrees, back to radians. No yaw is kept.

use bevy::prelude::*;

use super::camera::{CameraControl, FlyCam, CAM_PITCH_LIMIT};
use super::camera_zoom::{CAM_DIST_MAX, CAM_DIST_MIN};
use super::Player;
use benilla_world::view::WorldCamera;

/// The two rows, the reference's names.
pub(crate) const CVAR_DISTANCE: &str = "cameraSavedDistance";
pub(crate) const CVAR_PITCH: &str = "cameraSavedPitch";

/// A saved pitch (degrees, positive looking down) as [`FlyCam::pitch`] (radians, positive looking
/// up). The reference converts without a sign flip (`× 0.017453292` at `0x5ff3e0`), its own pitch
/// being positive-down; the flip is ours. The saved-view CVars carry pitch in the same units, so
/// [`super::camera_view`] converts through here too.
pub(super) fn pitch_from_file(degrees: f32) -> f32 {
    (-degrees.to_radians()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT)
}

/// The inverse of [`pitch_from_file`].
pub(super) fn pitch_to_file(radians: f32) -> f32 {
    -radians.to_degrees()
}

/// `CGCamera::SaveCameraSettings`: the zoom's target, not the arm a wall pulled in, and the pitch,
/// into the two rows as `%f` writes them. Called by the character slot's save.
pub(crate) fn save_to_cvars(world: &mut World) {
    let Some(target) = world
        .get_resource::<CameraControl>()
        .map(|r| r.target_distance)
    else {
        return;
    };
    let Some(pitch) = world
        .query_filtered::<&FlyCam, With<WorldCamera>>()
        .iter(world)
        .next()
        .map(|c| c.pitch)
    else {
        return;
    };
    if let Some(mut cvars) = world.get_resource_mut::<crate::cvars::Cvars>() {
        cvars.set(CVAR_DISTANCE, &format!("{target:.6}"));
        cvars.set(CVAR_PITCH, &format!("{:.6}", pitch_to_file(pitch)));
    }
}

/// `0x5ff3e0`, once the character's `config-cache.wtf` has run in: the pose from the rows, all
/// three rig distances set so the login opens at the zoom rather than gliding to it; the pitch
/// reaches the camera through the login seize.
pub(crate) fn restore_from_cvars(world: &mut World) {
    let read = |world: &World, name: &str| {
        world
            .get_resource::<crate::cvars::Cvars>()
            .and_then(|c| c.num(name))
            .filter(|v| v.is_finite())
    };
    let distance = read(world, CVAR_DISTANCE).map(|d| d.clamp(CAM_DIST_MIN, CAM_DIST_MAX));
    let pitch = read(world, CVAR_PITCH).map(pitch_from_file);
    if let (Some(d), Some(mut rig)) = (distance, world.get_resource_mut::<CameraControl>()) {
        rig.distance = d;
        rig.target_distance = d;
        rig.collision_distance = d;
    }
    let Some(p) = pitch else {
        return;
    };
    let mut cams = world.query_filtered::<&mut FlyCam, With<WorldCamera>>();
    if let Ok(mut cam) = cams.single_mut(world) {
        cam.pitch = p;
    }
    if let Some(mut player) = world.get_resource_mut::<Player>() {
        player.login_pitch = Some(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The saved positive pitch looks down and ours looks up, so the sign inverts on the way in.
    #[test]
    fn the_pitch_round_trips_through_the_row() {
        let pitch = pitch_from_file(24.2);
        assert!(pitch < 0.0, "a positive saved pitch looks DOWN for us");
        assert!((pitch_from_file(pitch_to_file(pitch)) - pitch).abs() < 1e-6);
        assert!(pitch_from_file(-4.75) > 0.0, "the camera below, looking up");
        assert_eq!(pitch_from_file(400.0), -CAM_PITCH_LIMIT);
    }
}
