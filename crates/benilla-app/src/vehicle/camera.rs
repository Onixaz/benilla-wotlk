//! The camera of our player's seat. While our passenger is entering or seated (states 1 to 3,
//! `0x74c0e0`):
//! - a seat that turns on the vehicle zoom (`flags & 0x1000000`) glides the camera out to
//!   `cameraSavedVehicleDistance` (50 yd when unset) under a 50 yd max (`0x600590`, `0x6000e0`);
//!   leaving it saves that distance and glides back to `cameraSavedDistance`. It takes precedence
//!   over the seat's own zoom below.
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
use crate::player::camera::FlyCam;
use crate::player::camera_zoom::{
    set_vehicle_zoom, VehicleSeatZoom, VehicleZoom, ZoomLimit, CAM_DIST_MIN,
};
use crate::player::{head_height, CameraPivot, RemoteView, VehicleFocus};
use crate::player::{CameraControl, VehicleChase};

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
/// offsets' eases, the seat change waiting for its window, the seat's camera timing for the exit,
/// and the chase facing's course.
#[derive(Default)]
pub(super) struct CameraMemory {
    seat: Option<(u32, u8)>,
    count: u32,
    pitch: Ease,
    yaw: Ease,
    /// The seat change not yet applied, and when it is due.
    pending: Option<SeatCamera>,
    /// The seat left's own exit timing (`flagsB & 0x80`): delay and duration.
    exit_timing: Option<(f32, f32)>,
    /// The chase facing's course: our passenger's state last frame, the facing a transition starts
    /// from and when it began, the facing the camera was relative to last frame, and since when the
    /// mover has been a vehicle we have not yet boarded.
    chase_state: u8,
    chase_start: f32,
    chase_began: f64,
    chase_last: Option<f32>,
    boarding_since: Option<f64>,
}

/// What `0x74c0e0` sets on a change of seat, applied together once its window opens: the vehicle
/// zoom, the seat's zoom range and ask, and the vehicle's camera pitch and yaw offsets.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SeatCamera {
    vehicle_zoom: bool,
    range: Option<ZoomRange>,
    ask: Option<VehicleZoom>,
    offsets: (f32, f32),
    due: Due,
}

/// When a seat change's camera runs (`0x75aac0`, `0x75af40`, `0x75af00`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Due {
    /// The seat's own camera timing: from `start`, over `duration` seconds.
    At { start: f64, duration: f32 },
    /// Over the passenger's motion once it begins, or at once should the passenger settle
    /// without one.
    Motion,
}

/// A seat change's window, `(start, end)` in seconds, once it is open: the seat's own timing from
/// its start, else the passenger's motion once its length is known (states 2 and 5), else now for
/// a passenger that settled (0 or 3) or is gone.
fn window(
    due: Due,
    now: f64,
    state: Option<u8>,
    transit: Option<(f64, Option<f64>)>,
) -> Option<(f64, f64)> {
    match due {
        Due::At { start, duration } => {
            (now >= start).then_some((start, start + f64::from(duration.max(0.0))))
        }
        Due::Motion => {
            let moving = matches!(state, Some(2 | 5));
            let motion = transit
                .filter(|_| moving)
                .and_then(|(start, end)| Some((start, end?)));
            let settled = state.is_none_or(|s| matches!(s, 0 | 3));
            motion.or(settled.then_some((now, now)))
        }
    }
}

/// How long the mover may be a vehicle we have not boarded before the camera stops holding to our
/// body: the server sends the control change just before the boarding move.
const BOARDING_HOLD_SECS: f64 = 1.0;

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

/// The seat's zoom on a change of seat (`0x74c0e0`): whether it turns on the vehicle zoom, its
/// range, and its zoom ask; a vehicle zoom seat asks none, leaving the distance a seat before kept.
fn seat_change(
    seat: Option<&VehicleSeatRec>,
    default_max: f32,
) -> (bool, Option<ZoomRange>, Option<VehicleZoom>) {
    match seat {
        Some(s) if s.flags & 0x1000000 != 0 => (true, None, None),
        Some(s) => match seat_zoom(s, default_max) {
            Some((range, enter)) => (false, Some(range), Some(enter)),
            None => (false, None, Some(VehicleZoom::Leave)),
        },
        None => (false, None, Some(VehicleZoom::Leave)),
    }
}

/// A transform's yaw.
fn yaw_of(t: &Transform) -> f32 {
    t.rotation.to_euler(EulerRot::YXZ).0
}

/// The facing our passenger's camera is relative to while its seat's transition runs (`0x75a1c0`):
/// held at the facing it started from through a pre-delay (states 1 and 4), turned towards its
/// target through the motion (2 and 5) by the passenger's own blend (`0x7598f0` reads `0x747d70`'s),
/// or by the seat's camera timing under `flagsB & 0x40` entering, `0x80` exiting, and on the
/// vehicle's facing while seated in a control seat (`flags & 0x800`, `0x75a930` targets the
/// vehicle). The target is the vehicle's entering a control seat, else the mover's. A transition
/// starts from the facing the camera was relative to (`0x75aac0`). `me` is our passenger's state,
/// its transit and our body's yaw; `mover` the mover's yaw and whether it is a vehicle; `ride` the
/// vehicle's yaw and our seat.
fn chase_facing(
    memory: &mut CameraMemory,
    now: f64,
    (state, transit, body): (Option<u8>, Option<&super::passenger::Transit>, f32),
    mover: Option<(f32, bool)>,
    ride: Option<(f32, &VehicleSeatRec)>,
) -> Option<VehicleChase> {
    let st = state.unwrap_or(0);
    let begins = match st {
        1 | 2 => !matches!(memory.chase_state, 1 | 2),
        4 | 5 => !matches!(memory.chase_state, 4 | 5),
        _ => false,
    };
    let mover_yaw = mover.map(|(yaw, _)| yaw);
    if begins {
        memory.chase_start = memory.chase_last.or(mover_yaw).unwrap_or(body);
        memory.chase_began = now;
    }
    memory.chase_state = st;
    // Deviation: the control change comes just before the boarding move, so until the passenger's
    // transition begins the camera keeps to our body, for a second at most; the reference makes
    // its vehicle camera on the boarding move alone.
    let boarding = st == 0 && mover.is_some_and(|(_, vehicle)| vehicle);
    memory.boarding_since = boarding.then(|| memory.boarding_since.unwrap_or(now));
    let held = |facing: f32| VehicleChase {
        start: facing,
        target: Some(facing),
        progress: 0.0,
    };
    let chase = match (st, ride) {
        (0, _) => memory
            .boarding_since
            .filter(|&since| now - since < BOARDING_HOLD_SECS)
            .map(|_| held(body)),
        (1 | 4, _) => Some(held(memory.chase_start)),
        (2 | 5, Some((vehicle, seat))) => {
            let entering = st == 2;
            let progress = match seat_timing(seat, entering) {
                Some((delay, duration)) => {
                    let t = now - memory.chase_began - f64::from(delay.max(0.0));
                    if duration <= 0.0 {
                        1.0
                    } else {
                        (t / f64::from(duration)).clamp(0.0, 1.0) as f32
                    }
                }
                None => {
                    transit
                        .and_then(|t| Some((t.start_s, t.end_s?)))
                        .map_or(0.0, |(start, end)| {
                            let total = end - start;
                            let linear = if total < 0.0001 {
                                1.0
                            } else {
                                ((now - start) / total) as f32
                            };
                            super::transit::blend(seat, entering, linear)
                        })
                }
            };
            Some(VehicleChase {
                start: memory.chase_start,
                target: (entering && seat.flags & 0x800 != 0).then_some(vehicle),
                progress,
            })
        }
        (super::SEATED, Some((vehicle, seat))) if seat.flags & 0x800 != 0 => Some(VehicleChase {
            start: vehicle,
            target: Some(vehicle),
            progress: 1.0,
        }),
        _ => None,
    };
    memory.chase_last = chase
        .map(|c| c.facing(mover_yaw.unwrap_or(body)))
        .or(mover_yaw);
    chase
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
    movers: Query<(&Transform, Has<Vehicle>), With<crate::net::Embodied>>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    limit: Option<Res<ZoomLimit>>,
    zoom: Option<ResMut<VehicleSeatZoom>>,
    focus: Option<ResMut<VehicleFocus>>,
    mut memory: Local<CameraMemory>,
    (mut rig, time, models, mut cvars, cams): (
        Option<ResMut<CameraControl>>,
        Res<Time<Real>>,
        Query<(&GlobalTransform, &benilla_assets::ModelAnimations)>,
        Option<ResMut<crate::cvars::Cvars>>,
        Query<&FlyCam, With<benilla_world::view::WorldCamera>>,
    ),
) {
    let (Some(limit), Some(mut zoom), Some(mut focus)) = (limit, zoom, focus) else {
        return;
    };
    let Ok((own, own_tf, passenger, own_pivot, own_net, transit)) = me.single() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let state = passenger.map(|p| p.state);
    let seated = passenger.filter(|p| matches!(p.state, 1..=3));
    let vehicle = seated.and_then(|p| Some((p, *index.0.get(&p.vehicle)?)));
    let seat = vehicle.and_then(|(p, e)| {
        let v = vehicles.get(e).ok()?;
        Some((p, e, v.id, tables.seat(v.id, usize::from(p.seat))?))
    });
    // A change of seat (`0x74c0e0`'s early out on the same one) waits for its window: the seat's
    // own camera timing, else the passenger's motion, else now (`0x75aac0`, `0x75af00`).
    let key = seat.map(|(p, _, id, _)| (id, p.seat));
    if key != memory.seat {
        memory.seat = key;
        let (vehicle_zoom, range, ask) = seat_change(seat.map(|(.., s)| s), limit.max);
        let rec = seat.and_then(|(_, _, id, _)| tables.vehicle(id));
        let offsets = rec.map_or((0.0, 0.0), |r| (r.camera_pitch_offset, r.camera_yaw_offset));
        let timing = match seat {
            Some((.., s)) => seat_timing(s, true),
            None => memory.exit_timing,
        };
        let animated = passenger.is_some_and(|p| matches!(p.state, 1 | 2 | 4 | 5));
        let due = match timing {
            Some((delay, duration)) => Due::At {
                start: now + f64::from(delay.max(0.0)),
                duration,
            },
            None if animated => Due::Motion,
            None => Due::At {
                start: now,
                duration: 0.0,
            },
        };
        memory.pending = Some(SeatCamera {
            vehicle_zoom,
            range,
            ask,
            offsets,
            due,
        });
        memory.exit_timing = seat.and_then(|(.., s)| seat_timing(s, false));
    }
    let course = transit.map(|t| (t.start_s, t.end_s));
    let open = memory
        .pending
        .and_then(|c| Some((c, window(c.due, now, state, course)?)));
    if let Some((change, (start, end))) = open {
        memory.pending = None;
        // What is left of the window, so a motion seen a frame late still lands with it.
        let duration = (end - now).max(0.0) as f32;
        // The vehicle zoom first, animated on, and off unless a seat's own zoom follows or a kept
        // distance is about to be restored.
        if let (Some(rig), Some(cvars)) = (rig.as_deref_mut(), cvars.as_deref_mut()) {
            let animate = change.vehicle_zoom
                || (!matches!(change.ask, Some(VehicleZoom::Enter { .. }))
                    && !rig.keeps_exit_zoom());
            let cam = cams.iter().next();
            set_vehicle_zoom(rig, (change.vehicle_zoom, duration, animate), cam, cvars);
        }
        zoom.vehicle = change.vehicle_zoom;
        zoom.range = change.range;
        if let Some(ask) = change.ask {
            memory.count = memory.count.wrapping_add(1);
            zoom.ask = Some((memory.count, ask, duration));
        }
        memory.pitch.set(change.offsets.0, now, start, end);
        memory.yaw.set(change.offsets.1, now, start, end);
    }
    let chase = chase_facing(
        &mut memory,
        now,
        (state, transit, yaw_of(own_tf)),
        movers.single().ok().map(|(t, v)| (yaw_of(t), v)),
        passenger.and_then(|p| {
            let e = *index.0.get(&p.vehicle)?;
            let v = vehicles.get(e).ok()?;
            let yaw = units.get(e).ok().map(|(t, ..)| yaw_of(t))?;
            Some((yaw, tables.seat(v.id, usize::from(p.seat))?))
        }),
    );
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
        if rig.vehicle_chase != chase {
            rig.vehicle_chase = chase;
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
    fn a_vehicle_zoom_seat_wins_over_its_own_zoom_and_asks_none() {
        let mut s = seat(0x400 | 0x8000);
        s.flags = 0x1000000;
        assert_eq!(seat_change(Some(&s), 15.0), (true, None, None));
        let (on, range, ask) = seat_change(Some(&seat(0x400 | 0x8000)), 15.0);
        assert!(!on && range == Some((None, Some(12.0))));
        assert!(matches!(ask, Some(VehicleZoom::Enter { .. })));
        assert_eq!(
            seat_change(None, 15.0),
            (false, None, Some(VehicleZoom::Leave))
        );
        assert_eq!(
            seat_change(Some(&seat(0)), 15.0),
            (false, None, Some(VehicleZoom::Leave))
        );
    }

    #[test]
    fn a_seat_times_its_camera_only_under_its_bits() {
        let mut s = VehicleSeatRec {
            camera_entering_duration: 1.5,
            camera_exiting_duration: 0.75,
            ..Default::default()
        };
        assert_eq!(
            (seat_timing(&s, true), seat_timing(&s, false)),
            (None, None)
        );
        s.flags_b = 0x40 | 0x80;
        assert_eq!(
            (seat_timing(&s, true), seat_timing(&s, false)),
            (Some((0.0, 1.5)), Some((0.0, 0.75)))
        );
    }

    /// A seat change without its own timing waits for the passenger's motion and spans it; one
    /// that settles without a motion runs at once; the seat's own timing runs from its delay
    /// (`0x75a630`'s early out, `0x75af00`).
    #[test]
    fn a_seat_change_waits_for_the_passengers_motion() {
        let motion = Due::Motion;
        assert_eq!(
            window(motion, 1.0, Some(1), Some((1.0, Some(1.5)))),
            None,
            "pre-delay"
        );
        assert_eq!(
            window(motion, 1.6, Some(2), Some((1.5, None))),
            None,
            "length unknown"
        );
        assert_eq!(
            window(motion, 1.6, Some(2), Some((1.5, Some(2.7)))),
            Some((1.5, 2.7))
        );
        assert_eq!(
            window(motion, 3.0, Some(3), None),
            Some((3.0, 3.0)),
            "settled"
        );
        assert_eq!(window(motion, 3.0, None, None), Some((3.0, 3.0)), "gone");
        let own = Due::At {
            start: 2.0,
            duration: 0.5,
        };
        assert_eq!(window(own, 1.9, Some(2), None), None);
        assert_eq!(window(own, 2.0, Some(2), None), Some((2.0, 2.5)));
    }

    fn transit(start_s: f64, end_s: Option<f64>) -> super::super::passenger::Transit {
        super::super::passenger::Transit {
            start_s,
            end_s,
            from: None,
            gravity: 0.0,
            arc_scale: 1.0,
        }
    }

    /// Boarding a control seat from the side: the camera keeps to our body while the control
    /// change waits for the boarding move, then its facing turns to the vehicle's with the
    /// passenger's motion, landing on it as the passenger does (`0x75a1c0`).
    #[test]
    fn the_chase_facing_turns_to_the_vehicle_with_the_motion() {
        let seat = VehicleSeatRec {
            flags: 0x800,
            ..Default::default()
        };
        let (body, vehicle) = (0.0, 1.5);
        let mut m = CameraMemory::default();
        // On foot: nothing to chase.
        assert_eq!(
            chase_facing(&mut m, 0.0, (None, None, body), Some((body, false)), None),
            None
        );
        // The vehicle is the mover before we board it: the camera keeps to our body.
        let c = chase_facing(&mut m, 0.1, (None, None, body), Some((vehicle, true)), None).unwrap();
        assert!((c.lag(vehicle) - (body - vehicle)).abs() < 1e-6);
        // The motion begins: from our body's facing, by its blend.
        let t = transit(0.2, Some(1.2));
        let ride = Some((vehicle, &seat));
        let at = |m: &mut CameraMemory, now: f64| {
            chase_facing(
                m,
                now,
                (Some(2), Some(&t), body),
                Some((vehicle, true)),
                ride,
            )
            .unwrap()
        };
        let start = at(&mut m, 0.2);
        assert!(
            (start.facing(vehicle) - body).abs() < 1e-6,
            "starts on our body's facing"
        );
        let half = at(&mut m, 0.7);
        assert!(
            (half.facing(vehicle) - 0.75).abs() < 1e-5,
            "half way is half"
        );
        let end = at(&mut m, 1.2);
        assert!(end.lag(vehicle).abs() < 1e-6, "lands behind the vehicle");
        // Seated: on the vehicle's facing.
        let seated = chase_facing(
            &mut m,
            1.3,
            (Some(super::super::SEATED), None, body),
            Some((vehicle, true)),
            ride,
        )
        .unwrap();
        assert!(seated.lag(vehicle).abs() < 1e-6);
    }

    /// The exit turns back from the vehicle's facing to the mover's, our body once control returns.
    #[test]
    fn the_chase_facing_turns_back_to_our_body_on_the_exit() {
        let seat = VehicleSeatRec {
            flags: 0x800,
            ..Default::default()
        };
        let (body, vehicle) = (0.5, -1.0);
        let ride = Some((vehicle, &seat));
        let mut m = CameraMemory::default();
        let seated = (Some(super::super::SEATED), None, body);
        chase_facing(&mut m, 0.0, seated, Some((vehicle, true)), ride);
        // Control is back on our body before the exit motion: the camera holds the vehicle's.
        let c = chase_facing(&mut m, 0.1, seated, Some((body, false)), ride).unwrap();
        assert!((c.lag(body) - (vehicle - body)).abs() < 1e-6);
        let t = transit(0.2, Some(0.6));
        let exit = |m: &mut CameraMemory, now: f64| {
            chase_facing(m, now, (Some(5), Some(&t), body), Some((body, false)), ride).unwrap()
        };
        assert!((exit(&mut m, 0.2).facing(body) - vehicle).abs() < 1e-6);
        assert!(exit(&mut m, 0.6).lag(body).abs() < 1e-6, "back on our body");
        // Settled on foot: nothing to chase.
        assert_eq!(
            chase_facing(&mut m, 0.7, (None, None, body), Some((body, false)), None),
            None
        );
    }

    /// The boarding hold gives up after a second when no boarding move comes.
    #[test]
    fn the_boarding_hold_ends_without_a_boarding_move() {
        let mut m = CameraMemory::default();
        let (body, vehicle) = (0.0, 1.0);
        assert!(
            chase_facing(&mut m, 0.0, (None, None, body), Some((vehicle, true)), None).is_some()
        );
        assert!(
            chase_facing(&mut m, 0.9, (None, None, body), Some((vehicle, true)), None).is_some()
        );
        assert_eq!(
            chase_facing(&mut m, 1.0, (None, None, body), Some((vehicle, true)), None),
            None
        );
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
