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
//! Not built: the seat's camera timing and fade, the vehicle's yaw and pitch offsets and the
//! tracking style, and the hierarchy of vehicle cameras a nested vehicle chains.

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::VehicleSeatRec;

use crate::net::{GuidIndex, NetEntity, SelfPlayer};
use crate::player::camera_zoom::{VehicleSeatZoom, VehicleZoom, ZoomLimit, CAM_DIST_MIN};
use crate::player::{head_height, CameraPivot, RemoteView, VehicleFocus};

use super::{Passenger, Vehicle, VehicleTables};

/// The seat's zoom range, each bound `None` for the default, and its zoom ask (`0x74c0e0`);
/// `None` for a seat that leaves the zoom alone.
pub(super) fn seat_zoom(
    seat: &VehicleSeatRec,
    default_max: f32,
) -> Option<((Option<f32>, Option<f32>), VehicleZoom)> {
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

/// What the zoom feed holds between frames: the seat it last answered and the ask count.
#[derive(Default)]
pub(super) struct CameraMemory {
    seat: Option<(u32, u8)>,
    count: u32,
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
        ),
        With<SelfPlayer>,
    >,
    units: Query<(&Transform, Option<&CameraPivot>, Option<&NetEntity>)>,
    vehicles: Query<&Vehicle>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    limit: Option<Res<ZoomLimit>>,
    zoom: Option<ResMut<VehicleSeatZoom>>,
    focus: Option<ResMut<VehicleFocus>>,
    mut memory: Local<CameraMemory>,
) {
    let (Some(limit), Some(mut zoom), Some(mut focus)) = (limit, zoom, focus) else {
        return;
    };
    let Ok((own, own_tf, passenger, own_pivot, own_net)) = me.single() else {
        return;
    };
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
    }
    // The focus: the vehicle for a seat that follows it, else our body, at the offset.
    focus.0 = seat.and_then(|(_, vehicle, _, s)| {
        let follow = s.flags_b & 0x200 != 0;
        let offset = camera_offset(s);
        if !follow && offset.is_none() {
            return None;
        }
        let (entity, tf, pivot, net) = if follow {
            let (tf, pivot, net) = units.get(vehicle).ok()?;
            (vehicle, tf, pivot, net)
        } else {
            (own, own_tf, own_pivot, own_net)
        };
        let feet = tf.translation + tf.rotation * offset.unwrap_or(Vec3::ZERO);
        Some(RemoteView {
            entity,
            feet,
            pivot_height: head_height(pivot, net.map_or(1.0, |n| n.scale)),
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
