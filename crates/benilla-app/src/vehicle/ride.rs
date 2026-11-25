//! A seated passenger drawn on its seat (`0x74a7f0` in state 3): its model hangs from the vehicle
//! model's seat attachment (`s_seatAttachments[seat.attachmentID]`, `0xa2d3f0`), the model being a
//! mounted unit's mount before its body (`0x6e6f80`), placed by
//! `0x7490f0`: turned by its facing less the seat's yaw, rolled and pitched by the seat, scaled
//! to its own size, its own `passengerAttachmentID` point (`0x748400`) on the seat's offset. A
//! vehicle model without that attachment carries the passenger at the vehicle's own frame. The
//! rig moves as a mount's rider does ([`crate::entities::mount`]), so nothing riding the
//! passenger is rebuilt; it holds the seat's ride anim (`0x747b20`).

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::VehicleSeatRec;
use benilla_protocol::messages::TransportPose;
use benilla_world::model_fade::DespawnFade;
use benilla_world::rig_anim::{RigFrame, RigPose};

use crate::entities::mount::{MountBody, MountChild};
use crate::entities::BoneAttach;
use crate::net::{GuidIndex, NetEntity};
use crate::transport::TransportRider;

use super::transit::TransitAnchor;
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
    /// The passenger state whose [`SeatPose`] was last written (`3` seated or `4` exit pre-delay).
    state: u8,
    pub(super) anchor: Entity,
    /// The anchor's local transform as last written.
    local: Transform,
    /// The vehicle's key-bone slot armed by this seat's `0x20000` ride loop (`0x756f80`), if any.
    loop_bone: Option<u8>,
}

/// On a seat anchor: the passenger whose rig it carries, so it goes when the passenger does.
#[derive(Component)]
pub(crate) struct SeatAnchor(Entity);

/// The anims a passenger holds in its current seat state (`0x747b20`, `0x747bd0`), each a
/// one-shot start before its loop. The lower pair replaces the body's base pick; the upper pair
/// rides the torso while the seat animates it (`flags & 0x4`), under any one-shot. The start flags
/// clear on each state change (`0x7489c0`), which a fresh component is; an enter's two states and
/// an exit's two states are each one.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SeatPose {
    /// The base's start and loop.
    pub(crate) lower: [Option<u16>; 2],
    /// The torso's start and loop.
    pub(crate) upper: [Option<u16>; 2],
    /// An exit's end, played once as the passenger stands back on its own (`0x74b9c0`).
    pub(crate) end: Option<u16>,
    /// The seat plays the end even falling, swimming or flying (`flags & 0x200000`).
    pub(crate) end_forced: bool,
}

/// On a vehicle: the whole-model ride loops seated passengers' `0x20000` seats play on it
/// (`0x7489c0`, through `0x756f80`), keyed by seat index so exiting one seat never clears
/// another seat's loop. Per-bone loops (`bone < 35`) live in [`benilla_world::rig_anim::KeyBoneAnims`].
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct VehicleSeatLoop(pub(crate) Vec<(u8, u16)>);

impl VehicleSeatLoop {
    /// The active whole-model seat ride loop anim, if any seat holds one.
    pub(crate) fn active_anim(&self) -> Option<u16> {
        self.0.last().map(|&(_, anim)| anim)
    }

    /// Arm `anim` for `seat`, replacing any prior entry for that seat.
    pub(crate) fn set(&mut self, seat: u8, anim: u16) {
        self.0.retain(|&(s, _)| s != seat);
        self.0.push((seat, anim));
    }

    /// Clear `seat`'s entry; returns `true` when no seat loops remain.
    pub(crate) fn clear(&mut self, seat: u8) -> bool {
        self.0.retain(|&(s, _)| s != seat);
        self.0.is_empty()
    }
}

/// A seat anim column: `-1` and 506, the getters' "no anim", are none.
fn seat_anim(id: i32) -> Option<u16> {
    u16::try_from(id).ok().filter(|&a| i32::from(a) != NO_ANIM)
}

/// The passenger's anims in seat state `state` (`0x747b20`, `0x747bd0`): the enter's (states 1
/// and 2) where the seat animates the enter (`flags & 0x1`); the ride's (3) where it animates the
/// ride (`0x2`), with the upper pair under `0x4`; the exit's (4 and 5) where the seat's exit flag
/// holds (`exit_anims`, `0x74bb60`), always with its end. `None` when the state has none.
pub(crate) fn seat_pose(seat: &VehicleSeatRec, state: u8, exit_anims: bool) -> Option<SeatPose> {
    let pair = |start: i32, loop_: i32| [seat_anim(start), seat_anim(loop_)];
    let a = &seat.passenger_anims;
    let pose = match state {
        1 | 2 if seat.flags & 0x1 != 0 => SeatPose {
            lower: pair(a[0], a[1]),
            ..SeatPose::default()
        },
        3 => SeatPose {
            lower: if seat.flags & 0x2 != 0 {
                pair(a[2], a[3])
            } else {
                [None; 2]
            },
            upper: if seat.flags & 0x4 != 0 {
                pair(a[4], a[5])
            } else {
                [None; 2]
            },
            ..SeatPose::default()
        },
        4 | 5 => {
            let end = seat_anim(seat.exit_anims[2]);
            SeatPose {
                lower: if exit_anims {
                    pair(seat.exit_anims[0], seat.exit_anims[1])
                } else {
                    [None; 2]
                },
                end,
                end_forced: end.is_some() && seat.flags & 0x20_0000 != 0,
                ..SeatPose::default()
            }
        }
        _ => return None,
    };
    (pose != SeatPose::default()).then_some(pose)
}

/// `0x74bb60`: the seat's exit anims play for a unit whose extra flags carry `0x40` under seat flag
/// `0x8`, for any other under `0x8000`.
pub(crate) fn plays_exit_anims(seat: &VehicleSeatRec, flags2: u16) -> bool {
    let flag = if flags2 & 0x40 != 0 { 0x8 } else { 0x8000 };
    seat.flags & flag != 0
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
pub(super) fn seat_point(
    seat: &VehicleSeatRec,
    points: Option<&BoneAttach>,
) -> Option<(u16, Vec3)> {
    let id = SEAT_ATTACHMENTS.get(usize::try_from(seat.attachment_id).ok()?)?;
    points?.points.get(id).copied()
}

/// The seat attachment's offset from its bone, which the anchor under that bone carries.
fn joint_offset(seat: &VehicleSeatRec, points: Option<&BoneAttach>) -> Option<Vec3> {
    seat_point(seat, points).map(|(_, offset)| offset)
}

/// A seated unit's position is its vehicle's (`0x749d80`): its rig hangs from the seat, and what
/// reads the unit's own frame, ours or a remote's, follows the vehicle with it.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn follow_vehicles(
    index: Res<GuidIndex>,
    vehicles: Query<&Transform, (With<Vehicle>, Without<Passenger>)>,
    mut passengers: Query<
        (
            &Passenger,
            &mut Transform,
            Option<&mut crate::net::RemoteMotion>,
            Option<&TransportRider>,
            Has<crate::net::Embodied>,
        ),
        Without<Vehicle>,
    >,
    me: Query<(), (With<crate::net::Embodied>, Without<Passenger>)>,
    mut player: Option<ResMut<crate::player::Player>>,
) {
    if let (Some(player), false) = (player.as_deref_mut(), me.is_empty()) {
        if player.seat_tail.is_some() {
            player.seat_tail = None;
        }
    }
    for (passenger, mut transform, motion, rider, driven) in &mut passengers {
        if driven {
            if let Some(player) = player.as_deref_mut() {
                let tail = rider
                    .filter(|_| passenger.state == SEATED)
                    .map(|r| TransportPose {
                        guid: r.transport_guid,
                        pos: benilla_protocol::wire::Vector3d {
                            x: r.local_pos[0],
                            y: r.local_pos[1],
                            z: r.local_pos[2],
                        },
                        orientation: r.local_orientation,
                        seat: r.seat,
                    });
                if player.seat_tail != tail {
                    player.seat_tail = tail;
                }
            }
        }
        if passenger.state != SEATED {
            continue;
        }
        let Some(&vehicle) = index.0.get(&passenger.vehicle) else {
            continue;
        };
        let Ok(at) = vehicles.get(vehicle).map(|t| t.translation) else {
            continue;
        };
        if transform.translation != at {
            transform.translation = at;
        }
        if let Some(mut motion) = motion {
            motion.wow_pos = benilla_assets::coords::bevy_to_wow(at);
        }
        if driven {
            if let Some(player) = player.as_deref_mut() {
                player.pos = at;
            }
        }
    }
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
            Has<TransitAnchor>,
            Option<&super::aim::MoveFlags2>,
        ),
        Or<(With<Passenger>, With<SeatRide>)>,
    >,
    mut vehicles: Query<
        (
            &Vehicle,
            &GlobalTransform,
            Option<&BoneAttach>,
            Option<&MountChild>,
            Option<&mut VehicleSeatLoop>,
            Option<&benilla_assets::ModelAnimations>,
            Option<&mut benilla_world::rig_anim::KeyBoneAnims>,
        ),
        Without<DespawnFade>,
    >,
    mounts: Query<(&GlobalTransform, Option<&BoneAttach>), With<MountBody>>,
    mut poses: Query<&mut RigPose>,
    index: Res<GuidIndex>,
    tables: Res<VehicleTables>,
    creatures: Option<Res<crate::entities::Creatures>>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
) {
    let catalog = anim_data.as_deref().map(|d| &d.0);
    for (entity, net, passenger, rider, ride, own_points, own_tf, in_transit, flags2) in &passengers
    {
        // Seated, or in the exit's pre-delay, which holds the seat (`0x74a7f0`).
        let seated = passenger
            .filter(|p| p.state == SEATED || p.state == 4)
            .and_then(|p| Some((p, *index.0.get(&p.vehicle)?)));
        let wanted = seated.and_then(|(p, v)| {
            let (vehicle, vehicle_tf, points, mount, ..) = vehicles.get(v).ok()?;
            let seat = tables.seat(vehicle.id, usize::from(p.seat))?;
            // The model the seat hangs from; a mount still building waits a frame.
            let (model, model_tf, points) = match mount {
                Some(&MountChild(m)) => {
                    let (tf, points) = mounts.get(m).ok()?;
                    (m, tf, Some(points?))
                }
                None => (v, vehicle_tf, points),
            };
            let j_offset = joint_offset(seat, points);
            let attachment = usize::try_from(seat.attachment_id)
                .ok()
                .and_then(|i| SEAT_ATTACHMENTS.get(i))
                .copied();
            let seat_point = attachment.and_then(|a| points?.points.get(&a).copied());
            Some((p, v, seat, model, *model_tf, j_offset, seat_point))
        });
        // Leave a seat the passenger left, or one whose vehicle went.
        if let Some(r) = ride {
            let kept = wanted.is_some_and(|(p, v, ..)| v == r.vehicle && p.seat == r.seat);
            if !kept {
                // A rig an exit's motion already carries stays on it.
                if !in_transit {
                    let ground = crate::entities::mount::ground_frame(
                        &mut commands,
                        entity,
                        net,
                        creatures.as_deref(),
                    );
                    if let Ok(mut rig) = poses.get_mut(entity) {
                        crate::entities::mount::reseat_rig(&mut commands, &mut rig, ground, false);
                    }
                }
                if let Ok(mut ec) = commands.get_entity(r.anchor) {
                    ec.despawn();
                }
                // An exit's motion owns the anims from here: its transit gives the exit's, in
                // this frame's commands or an earlier one's, and drops them as it lands. Only a
                // seat left without one drops the ride's here.
                if in_transit || passenger.is_some_and(|p| p.state == 5) {
                    commands.entity(entity).remove::<SeatRide>();
                } else {
                    commands.entity(entity).remove::<(SeatRide, SeatPose)>();
                }
                // Only this seat's own ride loop on the vehicle ends with the ride.
                if let Ok((_, _, _, _, seat_loop, _, bone_anims)) = vehicles.get_mut(r.vehicle) {
                    if let Some(kb) = r.loop_bone {
                        if let Some(mut kba) = bone_anims {
                            kba.stop(kb);
                        }
                    } else if let Some(mut seat_loop) = seat_loop {
                        if seat_loop.clear(r.seat) {
                            if let Ok(mut ec) = commands.get_entity(r.vehicle) {
                                ec.remove::<VehicleSeatLoop>();
                            }
                        }
                    }
                }
                continue;
            }
        }
        let Some((p, vehicle, seat, model, vehicle_tf, j_offset, seat_point)) = wanted else {
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
        let local = match j_offset {
            Some(offset) => Transform {
                translation: local.translation + offset,
                ..local
            },
            None => local,
        };
        let exit_anims = plays_exit_anims(seat, flags2.map_or(0, |f| f.0));
        let pose = seat_pose(seat, p.state, exit_anims);
        if let Some(r) = ride {
            let state_changed = r.state != p.state;
            if r.local != local {
                commands.entity(r.anchor).insert(local);
            }
            if r.local != local || state_changed {
                commands.entity(entity).insert(SeatRide {
                    state: p.state,
                    local,
                    ..*r
                });
            }
            if state_changed {
                // Entering the exit's pre-delay (state 4) switches to the exit's anims (`0x747b20`).
                match pose {
                    Some(pose) => commands.entity(entity).insert(pose),
                    None => commands.entity(entity).remove::<SeatPose>(),
                };
            }
            continue;
        }
        // The rig is up on both sides before it seats; a model still loading waits a frame.
        if poses.get(entity).is_err() {
            continue;
        }
        let joint = seat_point.and_then(|(bone, offset)| {
            let joint = poses
                .get_mut(model)
                .ok()?
                .anchor_for(&mut commands, model, bone)?;
            Some((joint, offset))
        });
        // Under the attachment, or at the vehicle's own frame without one (`0x7490f0`'s tail).
        let parent = joint.map_or(model, |(joint, _)| joint);
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
        debug!(
            "vehicle: passenger {entity} seated on its anchor, state {} anim {:?}",
            p.state, pose
        );
        // A seat flagged `0x20000` loops its ride anim (column 35) on the vehicle while seated
        // (`0x7489c0`, through `0x756f80`): on its key bone when `< 35` (excluding 26), else on
        // the whole model.
        let mut loop_bone = None;
        if let Some(anim) = seat_anim(seat.vehicle_anims[2]).filter(|_| seat.flags & 0x2_0000 != 0)
        {
            if let Ok((_, _, _, _, seat_loop, anims, bone_anims)) = vehicles.get_mut(vehicle) {
                match super::passenger::vehicle_anim_bone(seat.vehicle_anim_bones[2]) {
                    Some(kb) => {
                        if let Some(anims) = anims.filter(|a| a.pose.key_bone(kb).is_some()) {
                            if let Some(clip) =
                                crate::creature_anim::resolved_clip_index(anims, anim, catalog)
                            {
                                let duration = anims.clips[clip].duration;
                                if let Some(mut kba) = bone_anims {
                                    kba.play(kb, clip, duration, true);
                                } else {
                                    let mut kba = benilla_world::rig_anim::KeyBoneAnims::default();
                                    kba.play(kb, clip, duration, true);
                                    commands.entity(vehicle).insert(kba);
                                }
                                loop_bone = Some(kb);
                            }
                        }
                    }
                    None => {
                        if let Some(mut seat_loop) = seat_loop {
                            seat_loop.set(p.seat, anim);
                        } else {
                            commands
                                .entity(vehicle)
                                .insert(VehicleSeatLoop(vec![(p.seat, anim)]));
                        }
                    }
                }
            }
        }
        commands.entity(entity).insert(SeatRide {
            vehicle,
            seat: p.seat,
            state: p.state,
            anchor,
            local,
            loop_bone,
        });
        match pose {
            Some(pose) => commands.entity(entity).insert(pose),
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

    /// A seated unit's frame is the vehicle's as it moves, and our own carries its seat's tail
    /// onto the wire until it leaves.
    #[test]
    fn a_seated_unit_follows_its_vehicle_and_owes_the_tail() {
        use crate::player::Player;
        use bevy::ecs::system::RunSystemOnce;
        const BIKE: u64 = 0x20;
        let mut w = World::new();
        let bike = w
            .spawn((
                Vehicle {
                    id: 7,
                    raw_facing: 0.0,
                },
                Transform::from_xyz(4.0, 0.0, 2.0),
            ))
            .id();
        let mut index = GuidIndex::default();
        index.0.insert(BIKE, bike);
        w.insert_resource(index);
        w.init_resource::<Player>();
        let me = w
            .spawn((
                Passenger {
                    vehicle: BIKE,
                    seat: 1,
                    state: SEATED,
                },
                TransportRider {
                    transport_guid: BIKE,
                    local_pos: [1.0, 2.0, 3.0],
                    local_orientation: 0.5,
                    seat: 1,
                },
                Transform::default(),
                crate::net::Embodied,
            ))
            .id();
        w.run_system_once(follow_vehicles).unwrap();
        assert_eq!(
            w.entity(me).get::<Transform>().unwrap().translation,
            Vec3::new(4.0, 0.0, 2.0)
        );
        let player = w.resource::<Player>();
        assert_eq!(player.pos, Vec3::new(4.0, 0.0, 2.0));
        let tail = player.seat_tail.expect("seated, the tail is owed");
        assert_eq!((tail.guid, tail.seat), (BIKE, 1));
        assert_eq!(tail.orientation, 0.5);

        w.entity_mut(me).remove::<Passenger>();
        w.run_system_once(follow_vehicles).unwrap();
        assert!(
            w.resource::<Player>().seat_tail.is_none(),
            "out of the seat"
        );
    }

    /// The ride's start then loop, and the torso's under `0x4`; no ride flag, no base; 506 and
    /// -1 are none.
    #[test]
    fn the_ride_pose_is_the_seat_s_start_and_loop() {
        let pose = seat_pose(&seat(), SEATED, false).unwrap();
        assert_eq!(pose.lower, [Some(120), Some(121)]);
        assert_eq!(pose.upper, [None, None]);
        let mut s = seat();
        s.passenger_anims[2] = NO_ANIM;
        assert_eq!(
            seat_pose(&s, SEATED, false).unwrap().lower,
            [None, Some(121)]
        );
        s.flags |= 0x4;
        s.passenger_anims[4] = -1;
        s.passenger_anims[5] = 140;
        assert_eq!(
            seat_pose(&s, SEATED, false).unwrap().upper,
            [None, Some(140)]
        );
        let still = VehicleSeatRec { flags: 0, ..seat() };
        assert_eq!(seat_pose(&still, SEATED, false), None);
    }

    /// The enter's pair under `0x1`; the exit's pair under its flag, its end always.
    #[test]
    fn the_enter_and_exit_poses_follow_their_flags() {
        let s = VehicleSeatRec {
            flags: 0x1 | 0x8000,
            passenger_anims: [37, 38, -1, -1, -1, -1],
            exit_anims: [37, 38, 39],
            ..Default::default()
        };
        assert_eq!(seat_pose(&s, 1, false).unwrap().lower, [Some(37), Some(38)]);
        assert_eq!(seat_pose(&s, 2, false), seat_pose(&s, 1, false));
        assert!(plays_exit_anims(&s, 0));
        assert!(!plays_exit_anims(&s, 0x40), "0x40 asks for seat flag 0x8");
        let exit = seat_pose(&s, 5, true).unwrap();
        assert_eq!((exit.lower, exit.end), ([Some(37), Some(38)], Some(39)));
        assert_eq!(seat_pose(&s, 5, false).unwrap().lower, [None, None]);
        let no_enter = VehicleSeatRec { flags: 0, ..s };
        assert_eq!(seat_pose(&no_enter, 1, false), None);
        let forced_without_end = VehicleSeatRec {
            flags: 0x20_0000,
            exit_anims: [-1, -1, -1],
            ..Default::default()
        };
        assert_eq!(seat_pose(&forced_without_end, 5, false), None);
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

    /// Exiting one seat on a multi-seat vehicle leaves another seated passenger's ride loop intact.
    #[test]
    fn vehicle_seat_loop_tracks_each_seat_independently() {
        let mut loops = VehicleSeatLoop::default();
        loops.set(1, 111);
        assert_eq!(loops.active_anim(), Some(111));
        assert!(!loops.clear(0), "seat 0 had no loop; seat 1 remains");
        assert_eq!(loops.active_anim(), Some(111));
        assert!(loops.clear(1), "last loop cleared");
        assert_eq!(loops.active_anim(), None);
    }
}
