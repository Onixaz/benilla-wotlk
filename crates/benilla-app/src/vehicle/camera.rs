//! The camera of our player's seat (`CVehicleCamera_C`). While our passenger is entering or seated
//! (states 1 to 3, `0x74c0e0`):
//! - a seat that drives the zoom (`flagsB & 0x400`) sets its range, its own minimum
//!   (`0x4000`) and maximum (`0x8000`) or the defaults, and its preferred distance (`0x800`),
//!   raised to unless `0x1000` and lowered to unless `0x2000`, which widens a default bound it
//!   passes; leaving restores the distance before.
//! - the camera orbits the seat's camera offset (`0x759200`) around our body, or around the
//!   vehicle for a seat that follows it (`flagsB & 0x200`): `(off.y, -off.x, off.z)` turned by
//!   the focus's facing.
//!
//! - the vehicle's camera pitch and yaw offsets ease in on the enter and out on the exit
//!   (`0x74c0e0`, `0x6007b0`, `0x6006a0`), over the seat's own camera timing (`flagsB & 0x40`
//!   entering, `0x80` exiting: its delay, then its duration), else over the passenger's motion
//!   (`0x75af00`), else at once (`0x75aac0`).
//!
//! - seated in a vehicle whose record fades it (`flags & 0x80000`), the camera fades it, and us,
//!   by its distance to the vehicle's model over the record's scalars of the model's radius
//!   (columns 15-16, `0x606f90`).
//!
//! Not built: the seat camera's own position and facing chasers (columns 53-54), the tracking
//! style, and the hierarchy of vehicle cameras a nested vehicle chains.

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::VehicleSeatRec;

use crate::net::{GuidIndex, NetEntity, SelfPlayer};
use crate::player::camera_zoom::{VehicleSeatZoom, VehicleZoom, ZoomLimit, CAM_DIST_MIN};
use crate::player::{head_height, CameraPivot, RemoteView, VehicleFocus};

use super::{Passenger, Vehicle, VehicleTables};

/// A seat's zoom range, `(min, max)`, each bound `None` for the default.
type ZoomRange = (Option<f32>, Option<f32>);

/// The seat's zoom range and its zoom ask (`0x74c0e0`); `None` for a seat that leaves the zoom
/// alone.
pub(super) fn seat_zoom(
    seat: &VehicleSeatRec,
    default_max: f32,
) -> Option<(ZoomRange, VehicleZoom)> {
    let b = seat.flags_b;
    if b & 0x400 == 0 {
        return None;
    }
    let mut min = (b & 0x4000 != 0).then_some(seat.camera_seat_zoom_min);
    let mut max = (b & 0x8000 != 0).then_some(seat.camera_seat_zoom_max);
    let preferred = (b & 0x800 != 0).then_some(seat.camera_entering_zoom);
    // A preferred distance outside a default bound moves that bound to it.
    if let Some(p) = preferred {
        if min.is_none() && CAM_DIST_MIN > p {
            min = Some(p);
        }
        if max.is_none() && default_max < p {
            max = Some(p);
        }
    }
    Some((
        (min, max),
        VehicleZoom::Enter {
            preferred,
            raise: b & 0x1000 == 0,
            lower: b & 0x2000 == 0,
        },
    ))
}

/// The seat's camera offset in the focus's frame, `(off.y, -off.x, off.z)` (`0x759200`).
fn camera_offset(seat: &VehicleSeatRec) -> Option<Vec3> {
    let o = seat.camera_offset;
    (o != [0.0; 3]).then(|| wow_to_bevy([o[1], -o[0], o[2]]))
}

/// What the zoom feed holds between frames: the seat it last answered and the ask count, the
/// offsets' eases, and the seat's camera timing for the exit.
#[derive(Default)]
pub(super) struct CameraMemory {
    seat: Option<(u32, u8)>,
    count: u32,
    pitch: Ease,
    yaw: Ease,
    /// The offsets a change heads for once the passenger's motion times it.
    at_motion: Option<(f32, f32)>,
    /// The seat left's own exit timing (`flagsB & 0x80`): delay and duration.
    exit_timing: Option<(f32, f32)>,
}

/// One camera offset easing between two values over a window (`0x5fe8d0`, `0x6007b0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Ease {
    from: f32,
    to: f32,
    start_s: f64,
    end_s: f64,
}

impl Ease {
    /// The value at `now`: `from` before the window, `to` after, the cosine ease between
    /// (`0x5fffd0`).
    pub(super) fn at(&self, now: f64) -> f32 {
        if self.end_s <= self.start_s || now >= self.end_s {
            return self.to;
        }
        let t = ((now - self.start_s) / (self.end_s - self.start_s)).max(0.0) as f32;
        let eased = 0.5 - (t * std::f32::consts::PI).cos() * 0.5;
        self.from + eased * (self.to - self.from)
    }

    /// Head for `to` over `[start, end]` from wherever the value stands at `now`; an empty window
    /// sets it.
    pub(super) fn set(&mut self, to: f32, now: f64, start_s: f64, end_s: f64) {
        self.from = self.at(now);
        self.to = to;
        self.start_s = start_s;
        self.end_s = end_s;
    }
}

/// A seat's own camera timing, `(delay, duration)`: the enter's under `flagsB & 0x40` (columns
/// 46-47), the exit's under `0x80` (48-49) (`0x75aac0`).
fn seat_timing(seat: &VehicleSeatRec, entering: bool) -> Option<(f32, f32)> {
    if entering {
        (seat.flags_b & 0x40 != 0)
            .then_some((seat.camera_entering_delay, seat.camera_entering_duration))
    } else {
        (seat.flags_b & 0x80 != 0)
            .then_some((seat.camera_exiting_delay, seat.camera_exiting_duration))
    }
}

/// Feed the seat's zoom range and focus to the camera.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_vehicle_camera(
    me: Query<
        (
            Entity,
            &Transform,
            Option<&Passenger>,
            Option<&CameraPivot>,
            Option<&NetEntity>,
            Option<&super::passenger::Transit>,
        ),
        With<SelfPlayer>,
    >,
    units: Query<(
        &Transform,
        Option<&CameraPivot>,
        Option<&NetEntity>,
        Option<&crate::entities::CollisionHeight>,
    )>,
    vehicles: Query<&Vehicle>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    limit: Option<Res<ZoomLimit>>,
    zoom: Option<ResMut<VehicleSeatZoom>>,
    focus: Option<ResMut<VehicleFocus>>,
    mut memory: Local<CameraMemory>,
    (rig, time, models): (
        Option<ResMut<crate::player::CameraControl>>,
        Res<Time<Real>>,
        Query<(&GlobalTransform, &benilla_assets::ModelAnimations)>,
    ),
) {
    let (Some(limit), Some(mut zoom), Some(mut focus)) = (limit, zoom, focus) else {
        return;
    };
    let Ok((own, own_tf, passenger, own_pivot, own_net, transit)) = me.single() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let seated = passenger.filter(|p| matches!(p.state, 1..=3));
    let vehicle = seated.and_then(|p| Some((p, *index.0.get(&p.vehicle)?)));
    let seat = vehicle.and_then(|(p, e)| {
        let v = vehicles.get(e).ok()?;
        Some((p, e, v.id, tables.seat(v.id, usize::from(p.seat))?))
    });
    // The zoom, on a change of seat (`0x74c0e0`'s early out on the same one).
    let key = seat.map(|(p, _, id, _)| (id, p.seat));
    if key != memory.seat {
        memory.seat = key;
        let ask = match seat.and_then(|(.., s)| seat_zoom(s, limit.max)) {
            Some((range, enter)) => {
                zoom.range = Some(range);
                enter
            }
            None => {
                zoom.range = None;
                VehicleZoom::Leave
            }
        };
        memory.count = memory.count.wrapping_add(1);
        zoom.ask = Some((memory.count, ask));
        // The offsets head for the new seat's vehicle's, or none: over the seat's own timing,
        // else over the passenger's motion, else at once.
        let rec = seat.and_then(|(_, _, id, _)| tables.vehicle(id));
        let to = rec.map_or((0.0, 0.0), |r| (r.camera_pitch_offset, r.camera_yaw_offset));
        let timing = match seat {
            Some((.., s)) => seat_timing(s, true),
            None => memory.exit_timing,
        };
        let animated = passenger.is_some_and(|p| matches!(p.state, 1 | 2 | 4 | 5));
        memory.at_motion = None;
        match timing {
            Some((delay, duration)) => {
                let start = now + f64::from(delay.max(0.0));
                let end = start + f64::from(duration.max(0.0));
                memory.pitch.set(to.0, now, start, end);
                memory.yaw.set(to.1, now, start, end);
            }
            None if animated => memory.at_motion = Some(to),
            None => {
                memory.pitch.set(to.0, now, now, now);
                memory.yaw.set(to.1, now, now, now);
            }
        }
        memory.exit_timing = seat.and_then(|(.., s)| seat_timing(s, false));
    }
    // A change timed by the passenger's motion eases over it once it starts (`0x75af00`), or at
    // once should the passenger settle without one.
    if let Some(to) = memory.at_motion {
        let moving = passenger.is_some_and(|p| matches!(p.state, 2 | 5));
        let window = transit
            .filter(|_| moving)
            .and_then(|t| Some((t.start_s, t.end_s?)));
        let settled = passenger.is_none_or(|p| matches!(p.state, 0 | 3));
        if let Some((start, end)) = window.or(settled.then_some((now, now))) {
            memory.at_motion = None;
            memory.pitch.set(to.0, now, start, end);
            memory.yaw.set(to.1, now, start, end);
        }
    }
    if let Some(mut rig) = rig {
        // Seated in a vehicle whose record fades it near the camera (`flags & 0x80000`): the
        // window is the record's scalars of its model's radius (`0x606f90`).
        let fade =
            seat.filter(|(p, ..)| p.state == super::SEATED)
                .and_then(|(_, vehicle, id, _)| {
                    let rec = tables.vehicle(id).filter(|r| r.flags & 0x8_0000 != 0)?;
                    let (tf, anims) = models.get(vehicle).ok()?;
                    let radius = anims
                        .clips
                        .iter()
                        .find(|c| c.seq_index == 0)
                        .map(|c| c.bounds_radius)?;
                    Some(crate::player::VehicleFade {
                        vehicle,
                        at: tf.translation(),
                        gone: rec.camera_fade_dist_scalar_min * radius,
                        whole: rec.camera_fade_dist_scalar_max * radius,
                    })
                });
        if rig.vehicle_fade != fade {
            rig.vehicle_fade = fade;
        }
        let (pitch, yaw) = (memory.pitch.at(now), memory.yaw.at(now));
        if rig.vehicle_pitch_offset != pitch || rig.vehicle_yaw_offset != yaw {
            rig.vehicle_pitch_offset = pitch;
            rig.vehicle_yaw_offset = yaw;
        }
    }
    // The focus: the vehicle for a seat that follows it, else our body, at the offset.
    focus.0 = seat.and_then(|(_, vehicle, _, s)| {
        let follow = s.flags_b & 0x200 != 0;
        let offset = camera_offset(s);
        if !follow && offset.is_none() {
            return None;
        }
        let (entity, tf, pivot, net, ceiling) = if follow {
            let (tf, pivot, net, height) = units.get(vehicle).ok()?;
            // The vehicle is a camera target under its unit ceiling (`0x718080`).
            let ceiling = crate::player::pivot_ceiling(height.map(|h| h.0), false);
            (vehicle, tf, pivot, net, ceiling)
        } else {
            (own, own_tf, own_pivot, own_net, f32::INFINITY)
        };
        let feet = tf.translation + tf.rotation * offset.unwrap_or(Vec3::ZERO);
        Some(RemoteView {
            entity,
            feet,
            pivot_height: crate::player::clamp_pivot(
                head_height(pivot, net.map_or(1.0, |n| n.scale)),
                ceiling,
            ),
            facing: tf.rotation.to_euler(EulerRot::YXZ).0,
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(flags_b: u32) -> VehicleSeatRec {
        VehicleSeatRec {
            flags_b,
            camera_entering_zoom: 20.0,
            camera_seat_zoom_min: 5.0,
            camera_seat_zoom_max: 12.0,
            ..Default::default()
        }
    }

    #[test]
    fn a_seat_drives_the_zoom_only_under_its_bit() {
        assert!(seat_zoom(&seat(0), 15.0).is_none());
        let (range, ask) = seat_zoom(&seat(0x400 | 0x4000 | 0x8000), 15.0).unwrap();
        assert_eq!(range, (Some(5.0), Some(12.0)));
        assert_eq!(
            ask,
            VehicleZoom::Enter {
                preferred: None,
                raise: true,
                lower: true
            }
        );
        // A preferred distance past a default bound widens it.
        let (range, _) = seat_zoom(&seat(0x400 | 0x800), 15.0).unwrap();
        assert_eq!(range, (None, Some(20.0)));
    }

    /// The offsets ease on the cosine over their window from where they stand, and an empty
    /// window sets them (`0x5fffd0`, `0x6007b0`).
    #[test]
    fn an_offset_eases_in_over_its_window() {
        let mut e = Ease::default();
        e.set(0.25, 0.0, 1.0, 3.0);
        assert_eq!(e.at(0.5), 0.0, "before the window");
        assert!((e.at(2.0) - 0.125).abs() < 1e-6, "half way is half");
        assert_eq!(e.at(3.5), 0.25);
        e.set(0.0, 3.5, 3.5, 3.5);
        assert_eq!(e.at(3.5), 0.0, "an empty window sets it");
    }

    /// The seat's own camera timing under its flags.
    #[test]
    fn a_seat_times_its_camera_under_its_flags() {
        let s = VehicleSeatRec {
            flags_b: 0x40,
            camera_entering_delay: 0.5,
            camera_entering_duration: 1.0,
            camera_exiting_delay: 0.2,
            camera_exiting_duration: 0.4,
            ..Default::default()
        };
        assert_eq!(seat_timing(&s, true), Some((0.5, 1.0)));
        assert_eq!(seat_timing(&s, false), None);
        let exit = VehicleSeatRec { flags_b: 0x80, ..s };
        assert_eq!(seat_timing(&exit, false), Some((0.2, 0.4)));
    }

    #[test]
    fn the_offset_swaps_its_axes() {
        let s = VehicleSeatRec {
            camera_offset: [1.0, 2.0, 3.0],
            ..Default::default()
        };
        assert_eq!(camera_offset(&s), Some(wow_to_bevy([2.0, -1.0, 3.0])));
        assert_eq!(camera_offset(&VehicleSeatRec::default()), None);
    }
}
