//! A seated passenger drawn on its seat (`0x74a7f0` in state 3): its model hangs from the vehicle
//! model's seat attachment (`s_seatAttachments[seat.attachmentID]`, `0xa2d3f0`), placed by
//! `0x7490f0`: turned by its facing less the seat's yaw, rolled and pitched by the seat, scaled
//! to its own size, its own `passengerAttachmentID` point (`0x748400`) on the seat's offset. A
//! vehicle model without that attachment carries the passenger at the vehicle's own frame. The
//! rig moves as a mount's rider does ([`crate::entities::mount`]), so nothing riding the
//! passenger is rebuilt; it holds the seat's ride anim (`0x747b20`).

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::VehicleSeatRec;
use benilla_world::model_fade::DespawnFade;
use benilla_world::rig_anim::{RigFrame, RigPose};

use crate::entities::BoneAttach;
use crate::net::{GuidIndex, NetEntity};
use crate::transport::TransportRider;

use super::{Passenger, Vehicle, VehicleTables, SEATED};

/// The seat attachment table, by `VehicleSeat.attachmentID` (`0xa2d3f0`).
const SEAT_ATTACHMENTS: [u16; 22] = [
    20, 34, 19, 21, 22, 17, 23, 24, 25, 15, 16, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 0,
];

/// `ANIM_COUNT`, the "no anim" answer of the seat's anim getters.
const NO_ANIM: i32 = 506;

/// On a passenger drawn on a seat: the vehicle and slot, and the seat anchor its rig roots under.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeatRide {
    vehicle: Entity,
    seat: u8,
    anchor: Entity,
    /// The anchor's local transform as last written.
    local: Transform,
}

/// On a seat anchor: the passenger whose rig it carries, so it goes when the passenger does.
#[derive(Component)]
pub(crate) struct SeatAnchor(Entity);

/// The ride anim a live seated passenger holds (`0x747b20`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeatPose(pub(crate) u16);

/// `0x747b20` in state 3: the seat's ride loop when the seat animates its rider (`flags & 0x2`),
/// else its start; the start's one-shot lead-in ahead of the loop is not played.
pub(super) fn ride_anim(seat: &VehicleSeatRec) -> Option<u16> {
    if seat.flags & 0x2 == 0 {
        return None;
    }
    let anim = match seat.passenger_anims[3] {
        -1 => seat.passenger_anims[2],
        loop_ => loop_,
    };
    u16::try_from(anim)
        .ok()
        .filter(|&a| i32::from(a) != NO_ANIM)
}

/// `0x7490f0`'s local transform: the seat's turn, roll and pitch on the passenger's facing,
/// `scale` the passenger's size over the frame it hangs from, and its own point `pivot` (model
/// space, `None` without one) carried onto the seat's offset.
pub(super) fn seat_transform(
    seat: &VehicleSeatRec,
    facing: f32,
    scale: f32,
    pivot: Option<Vec3>,
) -> Transform {
    // The WoW axes in Bevy's: Z up is +Y, X forward is -Z, Y left is -X.
    let rotation = Quat::from_rotation_y(facing - seat.passenger_yaw)
        * Quat::from_rotation_z(-seat.passenger_roll)
        * Quat::from_rotation_x(-seat.passenger_pitch);
    let offset = wow_to_bevy(seat.attachment_offset);
    let carried = pivot.map_or(Vec3::ZERO, |p| rotation * (-p * scale));
    Transform {
        translation: offset + carried,
        rotation,
        scale: Vec3::splat(scale),
    }
}

/// The vehicle model's seat attachment: its bone and its offset from that bone.
fn seat_point(seat: &VehicleSeatRec, points: Option<&BoneAttach>) -> Option<(u16, Vec3)> {
    let id = SEAT_ATTACHMENTS.get(usize::try_from(seat.attachment_id).ok()?)?;
    points?.points.get(id).copied()
}

/// The seat attachment's offset from its bone, which the anchor under that bone carries.
fn joint_offset(seat: &VehicleSeatRec, points: Option<&BoneAttach>) -> Option<Vec3> {
    seat_point(seat, points).map(|(_, offset)| offset)
}

/// Seat each seated passenger's rig on its seat, keep its turn, and stand an unseated one back on
/// its own frame.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn ride_seats(
    mut commands: Commands,
    passengers: Query<
        (
            Entity,
            &NetEntity,
            Option<&Passenger>,
            Option<&TransportRider>,
            Option<&SeatRide>,
            Option<&BoneAttach>,
            &GlobalTransform,
        ),
        Or<(With<Passenger>, With<SeatRide>)>,
    >,
    vehicles: Query<(&Vehicle, &GlobalTransform, Option<&BoneAttach>), Without<DespawnFade>>,
    mut poses: Query<&mut RigPose>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    creatures: Option<Res<crate::entities::Creatures>>,
) {
    for (entity, net, passenger, rider, ride, own_points, own_tf) in &passengers {
        let seated = passenger
            .filter(|p| p.state == SEATED)
            .and_then(|p| Some((p, *index.0.get(&p.vehicle)?)));
        let wanted = seated.and_then(|(p, v)| {
            let (vehicle, vehicle_tf, points) = vehicles.get(v).ok()?;
            let seat = tables.seat(vehicle.id, usize::from(p.seat))?;
            Some((p, v, seat, vehicle_tf, points))
        });
        // Leave a seat the passenger left, or one whose vehicle went.
        if let Some(r) = ride {
            let kept = wanted.is_some_and(|(p, v, ..)| v == r.vehicle && p.seat == r.seat);
            if !kept {
                let ground = crate::entities::mount::ground_frame(
                    &mut commands,
                    entity,
                    net,
                    creatures.as_deref(),
                );
                if let Ok(mut rig) = poses.get_mut(entity) {
                    crate::entities::mount::reseat_rig(&mut commands, &mut rig, ground, false);
                }
                if let Ok(mut ec) = commands.get_entity(r.anchor) {
                    ec.despawn();
                }
                commands.entity(entity).remove::<(SeatRide, SeatPose)>();
                continue;
            }
        }
        let Some((p, vehicle, seat, vehicle_tf, points)) = wanted else {
            continue;
        };
        let facing = rider.map_or(0.0, |r| r.local_orientation);
        // `0x748400`: the passenger's own point, made model-space from its bone's bind pivot.
        let pivot = u16::try_from(seat.passenger_attachment_id)
            .ok()
            .and_then(|id| own_points?.points.get(&id).copied())
            .and_then(|(bone, offset)| Some(poses.get(entity).ok()?.bind_pivot(bone)? + offset));
        let scale = own_tf.scale().x / vehicle_tf.scale().x.max(0.001);
        let local = seat_transform(seat, facing, scale, pivot);
        let local = match joint_offset(seat, points) {
            Some(offset) => Transform {
                translation: local.translation + offset,
                ..local
            },
            None => local,
        };
        if let Some(r) = ride {
            if r.local != local {
                commands.entity(r.anchor).insert(local);
                commands.entity(entity).insert(SeatRide { local, ..*r });
            }
            continue;
        }
        // The rig is up on both sides before it seats; a model still loading waits a frame.
        if poses.get(entity).is_err() {
            continue;
        }
        let attachment = usize::try_from(seat.attachment_id)
            .ok()
            .and_then(|i| SEAT_ATTACHMENTS.get(i))
            .copied();
        let joint = attachment
            .and_then(|a| points?.points.get(&a).copied())
            .and_then(|(bone, offset)| {
                let joint =
                    poses
                        .get_mut(vehicle)
                        .ok()?
                        .anchor_for(&mut commands, vehicle, bone)?;
                Some((joint, offset))
            });
        // Under the attachment, or at the vehicle's own frame without one (`0x7490f0`'s tail).
        let (parent, local) = match joint {
            Some((joint, offset)) => (
                joint,
                Transform {
                    translation: local.translation + offset,
                    ..local
                },
            ),
            None => (vehicle, local),
        };
        let anchor = commands
            .spawn((
                local,
                Visibility::default(),
                RigFrame(entity),
                SeatAnchor(entity),
            ))
            .id();
        commands.entity(parent).add_child(anchor);
        if let Ok(mut rig) = poses.get_mut(entity) {
            crate::entities::mount::reseat_rig(&mut commands, &mut rig, anchor, false);
        }
        commands.entity(entity).insert(SeatRide {
            vehicle,
            seat: p.seat,
            anchor,
            local,
        });
        match ride_anim(seat) {
            Some(anim) => commands.entity(entity).insert(SeatPose(anim)),
            None => commands.entity(entity).remove::<SeatPose>(),
        };
    }
}

/// A seat anchor whose passenger is gone goes with it, or the passenger's attachments would
/// stay drawn on the seat.
pub(super) fn sweep_seat_anchors(
    mut commands: Commands,
    anchors: Query<(Entity, &SeatAnchor)>,
    rides: Query<&SeatRide>,
) {
    for (anchor, &SeatAnchor(passenger)) in &anchors {
        if rides.get(passenger).is_ok_and(|r| r.anchor == anchor) {
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
            flags: 0x2,
            attachment_offset: [1.0, 0.0, 2.0],
            passenger_anims: [-1, -1, 120, 121, -1, -1],
            ..Default::default()
        }
    }

    /// The ride loop over the start; no ride flag, no anim; 506 is none.
    #[test]
    fn the_ride_anim_is_the_seat_s_loop() {
        assert_eq!(ride_anim(&seat()), Some(121));
        let mut s = seat();
        s.passenger_anims[3] = -1;
        assert_eq!(ride_anim(&s), Some(120));
        s.passenger_anims[2] = NO_ANIM;
        assert_eq!(ride_anim(&s), None);
        let still = VehicleSeatRec { flags: 0, ..seat() };
        assert_eq!(ride_anim(&still), None);
    }

    /// The passenger's own point lands on the seat's offset, whatever the turn.
    #[test]
    fn the_pivot_lands_on_the_seat_offset() {
        let pivot = Vec3::new(0.0, 1.5, 0.2);
        for facing in [0.0, 1.0, -2.5] {
            let t = seat_transform(&seat(), facing, 2.0, Some(pivot));
            let landed = t.transform_point(pivot);
            assert!(
                (landed - wow_to_bevy([1.0, 0.0, 2.0])).length() < 1.0e-5,
                "{facing}: {landed}"
            );
        }
        // No point of its own: the model origin sits on the offset.
        let t = seat_transform(&seat(), 0.5, 1.0, None);
        assert_eq!(t.translation, wow_to_bevy([1.0, 0.0, 2.0]));
    }

    /// The seat's yaw turns the passenger against its facing.
    #[test]
    fn the_seat_yaw_turns_the_passenger() {
        let s = VehicleSeatRec {
            passenger_yaw: 0.5,
            ..seat()
        };
        let t = seat_transform(&s, 0.5, 1.0, None);
        assert!(t.rotation.angle_between(Quat::IDENTITY) < 1.0e-6);
    }
}
