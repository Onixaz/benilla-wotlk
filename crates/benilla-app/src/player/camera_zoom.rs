//! The zoom: the orbit distance's range, from `cameraDistanceMax` × `cameraDistanceMaxFactor`
//! under the reference's 0 and 50 yd bounds, and the wheel's glide to the chosen distance.

use bevy::prelude::*;

use super::camera::CameraControl;

/// The zoom floor, yd (`ds:` = 0.0): at 0 the eye sits at the framing pivot, inside the head, and
/// the avatar fades out. Deviation: the starting zoom, [`CAM_DIST_DEFAULT`], is 15 yd where the
/// reference's `cameraDistance` is 5.55, for a wider view.
pub(crate) const CAM_DIST_MIN: f32 = 0.0;
/// The zoom ceiling, yd (`ds:` = 50.0): the cap on [`ZoomLimit`] and the top of the distance
/// validator; also the clamp for a distance read back off disk.
pub(crate) const CAM_DIST_MAX: f32 = 50.0;
pub(super) const CAM_DIST_DEFAULT: f32 = 15.0;
/// `cameraDistanceMax`'s registered default, "15.0".
const CAM_DISTANCE_MAX_DEFAULT: f32 = 15.0;
/// `cameraDistanceMaxFactor`'s registered default, "1.0". The CVar has no validator: the options
/// slider's 1 to 2 (`UIOptionsFrame.lua:90`) bounds the slider, not the value.
const CAM_DISTANCE_MAX_FACTOR_DEFAULT: f32 = 1.0;
/// `cameraDistanceMax`'s validator (`(v, 0.0, 50.0)`) accepts `0 ≤ v ≤ 50`; anything else, NaN
/// included (`fcom` unordered sets C0), is refused and the old value stands.
pub(crate) const CAM_DISTANCE_MAX_RANGE: std::ops::RangeInclusive<f32> =
    CAM_DIST_MIN..=CAM_DIST_MAX;
/// Yards per wheel notch, the stock bindings' `CameraZoomIn(1.0)` (`Bindings.xml:707`).
const CAM_ZOOM_STEP: f32 = 1.0;
/// Zoom speed in yd/s, `cameraDistanceMoveSpeed`'s default: the reference glides the distance to
/// the wheel target at this constant speed, not an ease.
const CAM_MOVE_SPEED: f32 = 8.33;

/// The max orbit distance, yd, from `cameraDistanceMax` and `cameraDistanceMaxFactor`: 15 at the
/// reference's defaults. Lowering it pulls the live target in on the next frame.
#[derive(Resource)]
pub(crate) struct ZoomLimit {
    /// The live limit, [`orbit_limit`] of the two below: in `[0, 50]`, never NaN.
    pub(crate) max: f32,
    /// The barber chair's floor, the body's bounds radius (`0x52e480`), while the shop is open;
    /// the wheel there moves a fifth as far (`0x6017e0`, `0x601840`).
    pub(crate) barber_floor: Option<f32>,
    distance_max: f32,
    factor: f32,
}

/// A vehicle seat's zoom: its range (`SetVehicleSeatZoomMin/MaxDistance`), each bound `None` for
/// its default, and its change for the rig to make (`0x74c0e0`'s `ZoomToDistance`) over its
/// duration in seconds, with a count the rig keys its last applied one on.
#[derive(Resource, Default)]
pub(crate) struct VehicleSeatZoom {
    pub(crate) range: Option<(Option<f32>, Option<f32>)>,
    pub(crate) ask: Option<(u32, VehicleZoom, f32)>,
    /// The seat turns the vehicle zoom on (`flags & 0x1000000`): the default max is 50 yd, not
    /// the CVars' limit (`0x6000e0`).
    pub(crate) vehicle: bool,
}

/// A zoom a vehicle seat asks of the rig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum VehicleZoom {
    /// Entering a seat: its preferred distance, raised to (`raise`) or lowered to (`lower`), then
    /// held within its range; the distance before is kept for the exit.
    Enter {
        preferred: Option<f32>,
        raise: bool,
        lower: bool,
    },
    /// Leaving: back to the distance kept at the enter.
    Leave,
}

impl Default for ZoomLimit {
    fn default() -> Self {
        let (distance_max, factor) = (CAM_DISTANCE_MAX_DEFAULT, CAM_DISTANCE_MAX_FACTOR_DEFAULT);
        Self {
            max: orbit_limit(distance_max, factor),
            barber_floor: None,
            distance_max,
            factor,
        }
    }
}

impl ZoomLimit {
    /// `cameraDistanceMax`, through its validator's range ([`CAM_DISTANCE_MAX_RANGE`]); `false`
    /// when refused, the old value standing.
    pub(crate) fn set_distance_max(&mut self, v: f32) -> bool {
        if !CAM_DISTANCE_MAX_RANGE.contains(&v) {
            return false;
        }
        self.distance_max = v;
        self.max = orbit_limit(self.distance_max, self.factor);
        true
    }

    /// `cameraDistanceMaxFactor`, taken as it comes: the reference registers it with no validator.
    pub(crate) fn set_factor(&mut self, factor: f32) {
        self.factor = factor;
        self.max = orbit_limit(self.distance_max, self.factor);
        bevy::log::debug!("camera: zoom limit {} at factor {factor}", self.max);
    }

    /// The orbit's bounds, a vehicle seat's range over the defaults.
    pub(crate) fn bounds(&self, seat: &VehicleSeatZoom) -> (f32, f32) {
        let (min, max) = seat.range.unwrap_or_default();
        let default_max = if seat.vehicle { CAM_DIST_MAX } else { self.max };
        (
            min.map_or(CAM_DIST_MIN, |m| m.max(CAM_DIST_MIN)),
            max.map_or(default_max, |m| m.min(CAM_DIST_MAX)),
        )
    }

    #[cfg(test)]
    pub(crate) fn distance_max(&self) -> f32 {
        self.distance_max
    }

    #[cfg(test)]
    pub(crate) fn factor(&self) -> f32 {
        self.factor
    }
}

/// The reference's orbit limit: `cameraDistanceMax × cameraDistanceMaxFactor` through two ordered
/// x87 compares. Below 0.0 (`fcom ds:`, C0 alone) it is 0.0; else below 50.0 (`fcom ds:`, C0 alone)
/// it stands; else it is 50.0. A NaN product compares unordered (C0, C2 and C3 all set), which
/// passes the first test and fails the second: 50.0.
fn orbit_limit(distance_max: f32, factor: f32) -> f32 {
    let product = distance_max * factor;
    if product < CAM_DIST_MIN {
        CAM_DIST_MIN
    } else if product < CAM_DIST_MAX {
        product
    } else {
        CAM_DIST_MAX
    }
}

/// `cameraSavedVehicleDistance`, the distance the vehicle zoom opens at; below 0 it is 50 yd.
pub(crate) const CVAR_VEHICLE_DISTANCE: &str = "cameraSavedVehicleDistance";
/// The shortest glide a vehicle zoom toggle takes, seconds (`0x600590`).
const VEHICLE_ZOOM_MIN_DURATION: f32 = 0.5;

/// Turn the vehicle zoom on or off (`0x600590`): the camera's settings are saved first, into
/// the vehicle's row while the zoom is on, then the distance glides over `duration` (at least
/// 0.5 s) to `cameraSavedVehicleDistance` (50 when below 0) on the enable, or to
/// `cameraSavedDistance` within 0..50 on the disable, if `animate`. `cam`'s pitch goes into the
/// save.
pub(crate) fn set_vehicle_zoom(
    rig: &mut CameraControl,
    (enabled, duration, animate): (bool, f32, bool),
    cam: Option<&super::camera::FlyCam>,
    cvars: &mut crate::cvars::Cvars,
) {
    if rig.vehicle_zoom == enabled {
        return;
    }
    let duration = duration.max(VEHICLE_ZOOM_MIN_DURATION);
    super::camera_saved::save_settings(rig, cam.map(|c| c.pitch), cvars);
    let saved = |name: &str| cvars.num(name).filter(|v| v.is_finite());
    let distance = if enabled {
        saved(CVAR_VEHICLE_DISTANCE)
            .filter(|&d| d >= 0.0)
            .unwrap_or(CAM_DIST_MAX)
    } else {
        saved(super::camera_saved::CVAR_DISTANCE)
            .unwrap_or(CAM_DIST_DEFAULT)
            .clamp(CAM_DIST_MIN, CAM_DIST_MAX)
    };
    if animate && (distance - rig.distance).abs() >= 0.001 {
        rig.target_distance = distance;
        rig.zoom_glide = Some((distance - rig.distance).abs() / duration);
    }
    rig.vehicle_zoom = enabled;
}

/// Wheel zoom, run every frame: `CAMERAZOOMIN`/`OUT` move the target, and the distance glides to it
/// at a constant `cameraDistanceMoveSpeed`, as the reference's does. `scroll` is this frame's net
/// zoom-in in notches (positive is closer). `max` is [`ZoomLimit::max`], CVar-fed, so it bounds by
/// `max`/`min`, never `f32::clamp`, which panics on a NaN or inverted bound. `floor` is the
/// barber chair's ([`ZoomLimit::barber_floor`]).
pub(super) fn apply_zoom_scroll(
    scroll: f32,
    dt: f32,
    rig: &mut CameraControl,
    (seat_min, max): (f32, f32),
    floor: Option<f32>,
) {
    let (min, step) = match floor {
        Some(f) => (f, CAM_ZOOM_STEP * 0.2),
        None => (seat_min, CAM_ZOOM_STEP),
    };
    if scroll != 0.0 {
        rig.target_distance = (rig.target_distance - scroll * step).max(min).min(max);
        rig.zoom_glide = None;
    }
    // Re-clamp every frame, so lowering the max-distance slider pulls the camera in.
    rig.target_distance = rig.target_distance.max(min).min(max);
    // A vehicle zoom toggle glides at its own speed, which spans its duration, until it lands.
    let max_step = rig.zoom_glide.unwrap_or(CAM_MOVE_SPEED) * dt;
    rig.distance += (rig.target_distance - rig.distance).clamp(-max_step, max_step);
    if rig.distance == rig.target_distance {
        rig.zoom_glide = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(distance_max: f32, factor: f32) -> f32 {
        let mut z = ZoomLimit::default();
        assert!(
            z.set_distance_max(distance_max),
            "{distance_max} is in range"
        );
        z.set_factor(factor);
        z.max
    }

    #[test]
    fn the_limit_is_the_product_of_both_cvars() {
        assert_eq!(ZoomLimit::default().max, 15.0, "the reference's defaults");
        // The factor has no validator: 3 is not held to the slider's 2.
        assert_eq!(limit(15.0, 3.0), 45.0);
        assert_eq!(limit(15.0, 0.5), 7.5, "nor raised to the slider's 1");
        assert_eq!(limit(25.0, 1.0), 25.0);
    }

    #[test]
    fn the_limit_holds_to_zero_and_fifty() {
        assert_eq!(limit(15.0, 4.0), 50.0);
        assert_eq!(limit(50.0, 1.0), 50.0);
        assert_eq!(limit(15.0, f32::INFINITY), 50.0);
        assert_eq!(limit(15.0, -2.0), 0.0);
        assert_eq!(limit(15.0, f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn a_nan_product_fails_both_compares_and_is_fifty() {
        assert_eq!(limit(15.0, f32::NAN), 50.0);
        // 0 × ∞ is NaN too, with both values accepted.
        assert_eq!(limit(0.0, f32::INFINITY), 50.0);
    }

    #[test]
    fn cameradistancemax_is_refused_outside_its_validator_range() {
        let mut z = ZoomLimit::default();
        for bad in [-0.5, 50.5, f32::NAN, f32::INFINITY] {
            assert!(!z.set_distance_max(bad), "{bad} is refused");
            assert_eq!(
                (z.distance_max(), z.max),
                (15.0, 15.0),
                "the old value stands"
            );
        }
        assert!(
            z.set_distance_max(0.0) && z.max == 0.0,
            "both ends are inclusive"
        );
        assert!(z.set_distance_max(50.0) && z.max == 50.0);
    }

    #[test]
    fn a_nan_factor_zooms_without_panicking() {
        let mut z = ZoomLimit::default();
        z.set_factor(f32::NAN);
        let mut rig = CameraControl {
            target_distance: 10.0,
            distance: 10.0,
            ..default()
        };
        apply_zoom_scroll(-1.0, 0.1, &mut rig, (CAM_DIST_MIN, z.max), None);
        assert_eq!(rig.target_distance, 11.0);
        // Even a NaN bound handed straight in bounds nothing and panics nowhere.
        apply_zoom_scroll(-1.0, 0.1, &mut rig, (CAM_DIST_MIN, f32::NAN), None);
        assert_eq!(rig.target_distance, 12.0);
    }

    #[test]
    fn the_barber_chair_floors_the_zoom_and_steps_a_fifth() {
        let mut rig = CameraControl {
            target_distance: 2.0,
            distance: 2.0,
            ..default()
        };
        apply_zoom_scroll(1.0, 0.1, &mut rig, (CAM_DIST_MIN, 15.0), Some(1.5));
        assert!(
            (rig.target_distance - 1.8).abs() < 1e-6,
            "a fifth of a notch"
        );
        apply_zoom_scroll(10.0, 0.1, &mut rig, (CAM_DIST_MIN, 15.0), Some(1.5));
        assert_eq!(rig.target_distance, 1.5, "held at the body's radius");
        // A target under the floor when the chair opens is pushed out to it.
        rig.target_distance = 0.0;
        apply_zoom_scroll(0.0, 0.1, &mut rig, (CAM_DIST_MIN, 15.0), Some(1.5));
        assert_eq!(rig.target_distance, 1.5);
    }

    #[test]
    fn the_vehicle_zoom_glides_out_to_its_saved_distance_and_back() {
        let mut cvars = crate::cvars::Cvars::default();
        let mut rig = CameraControl {
            target_distance: 12.0,
            distance: 12.0,
            ..default()
        };
        // Unset (-1), the vehicle distance is 50; the 12 yd before is saved for the way back.
        set_vehicle_zoom(&mut rig, (true, 0.0, true), None, &mut cvars);
        assert_eq!(
            cvars.num(super::super::camera_saved::CVAR_DISTANCE),
            Some(12.0)
        );
        assert_eq!(rig.target_distance, CAM_DIST_MAX);
        // The glide spans the 0.5 s floor, not the wheel's 8.33 yd/s.
        let seat = VehicleSeatZoom {
            vehicle: true,
            ..default()
        };
        let bounds = ZoomLimit::default().bounds(&seat);
        assert_eq!(
            bounds,
            (CAM_DIST_MIN, CAM_DIST_MAX),
            "the max is 50, not 15"
        );
        for _ in 0..6 {
            apply_zoom_scroll(0.0, 0.1, &mut rig, bounds, None);
        }
        assert!(
            (rig.distance - CAM_DIST_MAX).abs() < 1e-3,
            "{}",
            rig.distance
        );
        assert_eq!(rig.zoom_glide, None, "landed");
        // The wheel picks 40 in the vehicle; leaving saves it and glides back to the 12.
        rig.target_distance = 40.0;
        rig.distance = 40.0;
        set_vehicle_zoom(&mut rig, (false, 2.0, true), None, &mut cvars);
        assert_eq!(cvars.num(CVAR_VEHICLE_DISTANCE), Some(40.0));
        assert_eq!(rig.target_distance, 12.0);
        assert!(
            (rig.zoom_glide.unwrap() - 14.0).abs() < 1e-4,
            "28 yd over 2 s"
        );
        rig.distance = 12.0;
        // The next vehicle opens at the 40 the wheel left.
        set_vehicle_zoom(&mut rig, (true, 0.0, true), None, &mut cvars);
        assert_eq!(rig.target_distance, 40.0);
        // A second enable is no change.
        rig.target_distance = 30.0;
        set_vehicle_zoom(&mut rig, (true, 0.0, true), None, &mut cvars);
        assert_eq!(rig.target_distance, 30.0);
    }

    #[test]
    fn an_unanimated_toggle_saves_and_keeps_the_distance() {
        let mut cvars = crate::cvars::Cvars::default();
        let mut rig = CameraControl {
            target_distance: 20.0,
            distance: 20.0,
            vehicle_zoom: true,
            ..default()
        };
        set_vehicle_zoom(&mut rig, (false, 0.0, false), None, &mut cvars);
        assert_eq!(cvars.num(CVAR_VEHICLE_DISTANCE), Some(20.0));
        assert_eq!((rig.target_distance, rig.vehicle_zoom), (20.0, false));
    }

    #[test]
    fn the_wheel_stops_at_the_limit() {
        let mut rig = CameraControl {
            target_distance: 44.5,
            ..default()
        };
        apply_zoom_scroll(-1.0, 0.1, &mut rig, (CAM_DIST_MIN, limit(15.0, 3.0)), None);
        assert_eq!(rig.target_distance, 45.0);
        apply_zoom_scroll(1000.0, 0.1, &mut rig, (CAM_DIST_MIN, 45.0), None);
        assert_eq!(rig.target_distance, CAM_DIST_MIN);
    }
}
