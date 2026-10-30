//! A passenger's animated enter and exit (`0x74a7f0` outside state 3): held at its start pose
//! through a pre-delay (states 1 and 4, the exit's on its seat), then carried along an arc to
//! the seat or off it (states 2 and 5). `0x74a200` fixes the motion's length from the distance
//! at the seat's speed and its arc from the seat's gravity scaled into its height limits;
//! `0x747d70` eases the blend by the seat's ease bits; the facing turns with it (`0x747a30`).

use bevy::prelude::*;

use benilla_formats::VehicleSeatRec;
use benilla_world::rig_anim::{RigFrame, RigPose};

use crate::entities::BoneAttach;
use crate::net::{GuidIndex, NetEntity};
use crate::transport::TransportRider;

use super::passenger::Transit;
use super::ride::{seat_transform, SeatPose, SeatRide};
use super::{Passenger, Vehicle, VehicleTables};

/// On a passenger in an enter or exit: the free anchor its rig rides.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransitAnchor(Entity);

/// On a free anchor: the passenger it carries, so it goes when the passenger does.
#[derive(Component)]
pub(super) struct TransitAnchorOf(Entity);

/// `0x74a200`'s motion: its length in seconds, its gravity and the scale that keeps the arc's
/// height within the seat's limits. `distance` is from the start to the end; `entering` picks the
/// enter columns.
pub(super) fn motion(seat: &VehicleSeatRec, entering: bool, distance: f32) -> (f32, f32, f32) {
    let (mut gravity, min_d, max_d, min_h, max_h, speed) = if entering {
        (
            seat.enter_gravity,
            seat.enter_min_duration,
            seat.enter_max_duration,
            seat.enter_min_arc_height,
            seat.enter_max_arc_height,
            seat.enter_speed,
        )
    } else {
        (
            seat.exit_gravity,
            seat.exit_min_duration,
            seat.exit_max_duration,
            seat.exit_min_arc_height,
            seat.exit_max_arc_height,
            seat.exit_speed,
        )
    };
    let max_d = if max_d < 0.0001 { 1.5 } else { max_d.min(10.0) };
    // The vehicle's own velocity along the way is not taken off the enter speed.
    let duration = if entering && distance <= 0.0001 {
        0.0
    } else if speed <= 0.0001 {
        max_d
    } else {
        (distance / speed).clamp(min_d.min(max_d), max_d)
    };
    if gravity.abs() < 0.0001 && (min_h > 0.0 || max_h < 0.0) {
        gravity = 0.1;
    }
    let scale = if duration <= 0.0001 {
        1.0
    } else {
        let height = gravity * duration * duration * 0.125;
        if height < min_h {
            min_h / height
        } else if max_h < height {
            max_h / height
        } else {
            1.0
        }
    };
    (duration, gravity, scale)
}

/// `0x747d70`: the blend through the motion, eased in, out or both by the seat's bits (`0x20` and
/// `0x40` entering, `0x80` and `0x100` exiting).
pub(super) fn blend(seat: &VehicleSeatRec, entering: bool, linear: f32) -> f32 {
    let b = linear.clamp(0.0, 1.0);
    let (ease_in, ease_out) = if entering {
        (0x20, 0x40)
    } else {
        (0x80, 0x100)
    };
    match (seat.flags & ease_in != 0, seat.flags & ease_out != 0) {
        (true, true) => 0.5 - (b * std::f32::consts::PI).cos() * 0.5,
        (true, false) => b * b,
        (false, true) => 1.0 - (1.0 - b) * (1.0 - b),
        (false, false) => b,
    }
}

/// The arc's height at `blend` through a motion of `total` seconds (`0x74a7f0`).
pub(super) fn arc(gravity: f32, scale: f32, total: f32, blend: f32) -> f32 {
    if gravity.abs() <= 0.0001 {
        return 0.0;
    }
    let elapsed = blend * total;
    let g = gravity * elapsed * 0.5;
    (total * g - elapsed * g) * scale
}

/// The yaw `to` brought within half a turn of `from` (`0x747a30`).
fn unwrap_yaw(from: f32, mut to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    while to + PI < from {
        to += TAU;
    }
    while to - PI > from {
        to -= TAU;
    }
    to
}

/// A transform's yaw about +Y.
fn yaw_of(t: &GlobalTransform) -> f32 {
    t.compute_transform().rotation.to_euler(EulerRot::YXZ).0
}

/// The enter's lower anim through its pre-delay and motion (`0x747b20` in states 1 and 2): the
/// seat's enter loop where the seat animates the enter (`flags & 0x1`).
fn enter_anim(seat: &VehicleSeatRec) -> Option<u16> {
    if seat.flags & 0x1 == 0 {
        return None;
    }
    let anim = match seat.passenger_anims[1] {
        -1 => seat.passenger_anims[0],
        loop_ => loop_,
    };
    u16::try_from(anim).ok().filter(|&a| a != 506)
}

/// The world point a seated passenger's origin sits at: the seat's local transform under the
/// vehicle's seat attachment, or under the vehicle's own frame.
#[allow(clippy::too_many_arguments)] // the vehicle's pose and the seat's, one each
fn seat_world(
    seat: &VehicleSeatRec,
    facing: f32,
    vehicle: Entity,
    vehicle_points: Option<&BoneAttach>,
    poses: &Query<&mut RigPose>,
    transforms: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let vehicle_tf = transforms.get(vehicle).ok()?;
    let local = seat_transform(seat, facing, 1.0, None);
    let point = super::ride::seat_point(seat, vehicle_points).and_then(|(bone, offset)| {
        let rig = poses.get(vehicle).ok()?;
        let root = transforms.get(rig.joints_root).ok()?;
        let joint = root.affine() * *rig.model.get(usize::from(bone))?;
        Some(joint.transform_point3(offset + local.translation))
    });
    Some(point.unwrap_or_else(|| vehicle_tf.transform_point(local.translation)))
}

/// Carry each enter and exit's rig on its free anchor, and give it back when the motion ends.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn ride_transits(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &NetEntity,
        &Passenger,
        Option<&mut Transit>,
        Option<&TransportRider>,
        Option<&SeatRide>,
        Option<&TransitAnchor>,
    )>,
    left: Query<(Entity, &NetEntity, &TransitAnchor), Without<Transit>>,
    vehicles: Query<(&Vehicle, Option<&BoneAttach>)>,
    mut poses: Query<&mut RigPose>,
    transforms: Query<&GlobalTransform>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    time: Res<Time<Real>>,
    creatures: Option<Res<crate::entities::Creatures>>,
) {
    let now = time.elapsed_secs_f64();
    // A finished motion's rig stands back on its own frame; a seated one moves on to its seat.
    for (entity, net, &TransitAnchor(anchor)) in &left {
        let ground =
            crate::entities::mount::ground_frame(&mut commands, entity, net, creatures.as_deref());
        if let Ok(mut rig) = poses.get_mut(entity) {
            crate::entities::mount::reseat_rig(&mut commands, &mut rig, ground, false);
        }
        if let Ok(mut ec) = commands.get_entity(anchor) {
            ec.despawn();
        }
        commands
            .entity(entity)
            .remove::<(TransitAnchor, SeatPose)>();
    }
    for (entity, _, p, transit, rider, ride, anchor) in &mut units {
        let Some(mut t) = transit else { continue };
        if !matches!(p.state, 1 | 2 | 5) {
            continue;
        }
        let entering = p.state != 5;
        let Some(&vehicle) = index.0.get(&p.vehicle) else {
            continue;
        };
        let Ok((v, vehicle_points)) = vehicles.get(vehicle) else {
            continue;
        };
        let Some(seat) = tables.seat(v.id, usize::from(p.seat)) else {
            continue;
        };
        let Ok(own) = transforms.get(entity) else {
            continue;
        };
        // The start pose: the seat's anchor leaving a seat, else the model where it stands.
        let from = match t.from {
            Some(f) => f,
            None => {
                let start = ride
                    .and_then(|r| transforms.get(r.anchor).ok())
                    .unwrap_or(own);
                let f = (start.translation(), yaw_of(start));
                t.from = Some(f);
                f
            }
        };
        let facing = rider.map_or(0.0, |r| r.local_orientation);
        // The end: the seat entering (`0x7493b0`), the unit's own place leaving.
        let to = if p.state == 5 {
            Some(own.translation())
        } else {
            seat_world(seat, facing, vehicle, vehicle_points, &poses, &transforms)
        };
        let Some(to) = to else { continue };
        if t.end_s.is_none() && p.state != 1 {
            let (duration, gravity, scale) = motion(seat, entering, from.0.distance(to));
            t.end_s = Some(t.start_s + f64::from(duration));
            t.gravity = gravity;
            t.arc_scale = scale;
        }
        let total = t.end_s.map_or(0.0, |e| (e - t.start_s) as f32);
        let linear = if total < 0.0001 {
            1.0
        } else {
            ((now - t.start_s) as f32 / total).clamp(0.0, 1.0)
        };
        // The pre-delay holds the start pose; the motion runs the arc.
        let (pos, yaw) = if p.state == 1 {
            from
        } else {
            let b = blend(seat, entering, linear);
            let lift = arc(t.gravity, t.arc_scale, total, b);
            let target = unwrap_yaw(from.1, yaw_of(own));
            (
                from.0.lerp(to, b) + Vec3::Y * lift,
                from.1 + (target - from.1) * b,
            )
        };
        let pose = Transform {
            translation: pos,
            rotation: Quat::from_rotation_y(yaw),
            scale: own.scale(),
        };
        match anchor {
            Some(&TransitAnchor(a)) => {
                commands.entity(a).insert(pose);
            }
            None => {
                let Ok(mut rig) = poses.get_mut(entity) else {
                    continue;
                };
                let a = commands
                    .spawn((
                        pose,
                        Visibility::default(),
                        RigFrame(entity),
                        TransitAnchorOf(entity),
                    ))
                    .id();
                crate::entities::mount::reseat_rig(&mut commands, &mut rig, a, false);
                commands.entity(entity).insert(TransitAnchor(a));
                match enter_anim(seat).filter(|_| entering) {
                    Some(anim) => commands.entity(entity).insert(SeatPose(anim)),
                    None => commands.entity(entity).remove::<SeatPose>(),
                };
            }
        }
    }
}

/// A free anchor whose passenger is gone, or moved off it, goes.
pub(super) fn sweep_transit_anchors(
    mut commands: Commands,
    anchors: Query<(Entity, &TransitAnchorOf)>,
    holders: Query<&TransitAnchor>,
) {
    for (anchor, &TransitAnchorOf(passenger)) in &anchors {
        if holders
            .get(passenger)
            .is_ok_and(|&TransitAnchor(a)| a == anchor)
        {
            continue;
        }
        commands.entity(anchor).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat() -> VehicleSeatRec {
        VehicleSeatRec {
            enter_speed: 2.0,
            enter_min_duration: 0.5,
            enter_max_duration: 3.0,
            exit_speed: 4.0,
            ..Default::default()
        }
    }

    /// The length is the distance at the seat's speed within its limits; a missing maximum is
    /// 1.5 s.
    #[test]
    fn the_motion_runs_at_the_seat_speed() {
        assert_eq!(motion(&seat(), true, 4.0).0, 2.0);
        assert_eq!(motion(&seat(), true, 0.2).0, 0.5, "the minimum");
        assert_eq!(motion(&seat(), true, 40.0).0, 3.0, "the maximum");
        assert_eq!(
            motion(&seat(), false, 40.0).0,
            1.5,
            "no exit maximum: 1.5 s"
        );
        assert_eq!(motion(&seat(), true, 0.0).0, 0.0, "already there");
    }

    /// A zero gravity with a floor on the height takes 0.1, scaled up to the floor.
    #[test]
    fn the_arc_keeps_within_the_height_limits() {
        let s = VehicleSeatRec {
            enter_min_arc_height: 1.0,
            enter_max_arc_height: 2.0,
            ..seat()
        };
        let (duration, gravity, scale) = motion(&s, true, 4.0);
        assert_eq!(gravity, 0.1);
        let peak = arc(gravity, scale, duration, 0.5);
        assert!((peak - 1.0).abs() < 1.0e-5, "the peak at the floor: {peak}");
        assert_eq!(arc(gravity, scale, duration, 0.0), 0.0);
        assert!(arc(gravity, scale, duration, 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn the_blend_eases_by_the_seat_bits() {
        let mut s = seat();
        assert_eq!(blend(&s, true, 0.5), 0.5);
        s.flags = 0x20;
        assert_eq!(blend(&s, true, 0.5), 0.25);
        s.flags = 0x40;
        assert_eq!(blend(&s, true, 0.5), 0.75);
        s.flags = 0x60;
        assert!((blend(&s, true, 0.5) - 0.5).abs() < 1.0e-6);
        assert_eq!(blend(&s, false, 0.5), 0.5, "the exit's bits are others");
    }

    #[test]
    fn a_yaw_unwraps_within_half_a_turn() {
        assert!((unwrap_yaw(3.0, -3.0) - (-3.0 + std::f32::consts::TAU)).abs() < 1.0e-6);
        assert_eq!(unwrap_yaw(0.0, 1.0), 1.0);
    }
}
