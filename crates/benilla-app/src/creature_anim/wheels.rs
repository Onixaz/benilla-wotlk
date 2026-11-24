//! The wheel spin (`0x71bd20`, run from the per-frame unit update `0x73dab0`): each wheel key bone
//! (KeyBoneID 27-34) turns about the model's left axis by an accumulated angle, advanced each
//! frame by the distance driven plus, for a wheel off the centre line, its share of the turn, over
//! the wheel's height.

use bevy::prelude::*;

use super::select::unify;
use super::{move_flags, MovementState};
use crate::entities::mount::{MountBody, MountChild};
use crate::net::{
    CreatureSwimming, DisplayFacing, RemoteMotion, Spline, UnitMoveModes, UnitSpeeds,
};

/// The wheel state, on a unit whose model carries any wheel key bone (the wheel mask, `0x715fd0`).
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct Wheels {
    wheels: Vec<Wheel>,
    /// The frame clock (ms) at the last step; the reference's starts at 0.
    last_ms: u64,
}

/// One wheel's bone, pivot and bookkeeping.
struct Wheel {
    bone: u16,
    /// The bind pivot's model-space left (WoW `y`) and up (WoW `z`) coordinates, unscaled.
    left: f32,
    up: f32,
    /// The accumulated spin (rad), wrapped to ±π; it starts at 0 with the model.
    angle: f32,
    /// The animated local rotation the spin last composed on.
    base: Quat,
    /// What we last wrote (`spin * base`). A bone still holding it was not re-keyed this frame, so
    /// `base` stays; composing onto it would accumulate the spin.
    last_out: Quat,
}

impl Wheels {
    /// The wheels of a model from its wheel bones and each one's model-space bind pivot (Bevy
    /// space); `None` for a model with none, which the reference's empty mask skips.
    pub(crate) fn new(bones: impl IntoIterator<Item = (u16, Vec3)>) -> Option<Self> {
        let wheels: Vec<Wheel> = bones
            .into_iter()
            .map(|(bone, pivot)| {
                let [_, left, up] = benilla_assets::coords::bevy_to_wow(pivot);
                Wheel {
                    bone,
                    left,
                    up,
                    angle: 0.0,
                    base: Quat::IDENTITY,
                    last_out: Quat::IDENTITY,
                }
            })
            .collect();
        (!wheels.is_empty()).then_some(Self { wheels, last_ms: 0 })
    }
}

/// The seconds since `last_ms`, clamped to 1-500 ms as the reference clamps the step.
fn elapsed_secs(now_ms: u64, last_ms: u64) -> f32 {
    now_ms.saturating_sub(last_ms).clamp(1, 500) as f32 * 0.001
}

/// The step's distance and turn: forward or a strafe rolls the wheels forward and backward rolls
/// them back, at the current speed; a turn key adds the turn rate (left negative), and the smoothed
/// facing's velocity (positive turning left) is taken off.
fn drive_inputs(
    flags: u32,
    speed: f32,
    turn_rate: f32,
    facing_velocity: f32,
    elapsed: f32,
) -> (f32, f32) {
    use move_flags as f;
    let distance = if flags & (f::FORWARD | f::STRAFE_LEFT | f::STRAFE_RIGHT) != 0 {
        speed * elapsed
    } else if flags & f::BACKWARD != 0 {
        -(speed * elapsed)
    } else {
        0.0
    };
    let turn = if flags & f::TURN_LEFT != 0 {
        (-turn_rate - facing_velocity) * elapsed
    } else if flags & f::TURN_RIGHT != 0 {
        (turn_rate - facing_velocity) * elapsed
    } else {
        -(elapsed * facing_velocity)
    };
    (distance, turn)
}

/// Whether the spin holds its angle this step: on a flying spline, or swimming or flying (the
/// reference's `0x2200000`).
fn holds(flags: u32, flying_spline: bool) -> bool {
    flying_spline || flags & (move_flags::SWIMMING | move_flags::FLYING) != 0
}

/// A wheel's angle after one step, `angle + (left · turn + distance) / up` with the pivot at the
/// model's world `scale`, wrapped to ±π; a pivot within 0.001 of the ground keeps its angle.
fn advance(angle: f32, left: f32, up: f32, scale: f32, turn: f32, distance: f32) -> f32 {
    use std::f32::consts::PI;
    let (left, up) = (left * scale, up * scale);
    if up.abs() <= 0.001 {
        return angle;
    }
    let next = (left * turn + distance) / up + angle;
    // Deviation: a non-finite step keeps the angle, where the reference's whole-turn wrap loop
    // would never end on one.
    if !next.is_finite() {
        return angle;
    }
    if (-PI..=PI).contains(&next) {
        next
    } else {
        super::wrap_pi(next)
    }
}

/// The wheel's rotation by `angle` about the model's left axis (WoW `+Y`, Bevy `−X`): positive
/// rolls the wheel's top forward.
fn spin(angle: f32) -> Quat {
    Quat::from_axis_angle(benilla_assets::coords::wow_to_bevy([0.0, 1.0, 0.0]), angle)
}

/// A mover's movement view, the unit's own or a mount child's host's.
type MoverData = (
    Option<&'static MovementState>,
    Option<&'static RemoteMotion>,
    Option<&'static Spline>,
    Has<CreatureSwimming>,
    Option<&'static UnitMoveModes>,
    Option<&'static UnitSpeeds>,
    Option<&'static DisplayFacing>,
);

/// Spin each unit's wheels and compose the spin onto the wheel bones in
/// [`benilla_world::rig_anim::PosePost`]: the procedural rotation goes outside the animated one,
/// `local' = spin · local`, as the reference multiplies it in after the bone's rotation and scale
/// (`0x8272f0`, read by the bone update under bone flag `0x80`).
#[allow(clippy::type_complexity)] // the unit row, commented inline
pub(super) fn spin_wheels(
    time: Res<Time>,
    player: Option<Res<crate::player::Player>>,
    // A parked rig is skipped, its clock included, so its first step back clamps to 500 ms. A
    // mounted rider's own model is not its object model, so its wheels never spin.
    mut units: Query<
        (Entity, &mut Wheels, Option<&MountBody>),
        (
            Without<benilla_world::rig_anim::AnimParked>,
            Without<MountChild>,
        ),
    >,
    movers: Query<MoverData>,
    mut rigs: Query<&mut benilla_world::rig_anim::RigPose>,
    frames: Query<&GlobalTransform>,
) {
    let now_ms = time.elapsed().as_millis() as u64;
    for (unit, mut wheels, mount_body) in &mut units {
        let elapsed = elapsed_secs(now_ms, wheels.last_ms);
        wheels.last_ms = now_ms;
        // A mount is its rider's object model, so it rolls on the rider's movement.
        let mover = mount_body.map_or(unit, |mb| mb.host);
        let Ok((movement, remote, spline, swimming, modes, speeds, facing)) = movers.get(mover)
        else {
            continue;
        };
        let mut mv = unify(movement, remote, spline, swimming, modes);
        // The active mover reads its streamed word, whose turn keys the body's word swaps for the
        // body chase, and has no facing velocity: the reference zeroes it (`0x735f60`).
        let facing_velocity = if movement.is_some() {
            if let Some(p) = &player {
                mv.flags = p.move_flags();
            }
            0.0
        } else {
            facing.map_or(0.0, DisplayFacing::velocity)
        };
        let turn_rate = speeds.map_or(0.0, |s| s.0.turn_rate);
        let (distance, turn) =
            drive_inputs(mv.flags, mv.speed, turn_rate, facing_velocity, elapsed);
        let held = holds(mv.flags, mv.flying);
        let Ok(mut rig) = rigs.get_mut(unit) else {
            continue;
        };
        // The model's world scale, the length of its frame's first axis.
        let scale = frames
            .get(rig.joints_root)
            .map_or(1.0, |g| g.affine().matrix3.x_axis.length());
        for w in &mut wheels.wheels {
            if !held {
                w.angle = advance(w.angle, w.left, w.up, scale, turn, distance);
            }
            let bone = usize::from(w.bone);
            let Some(cur) = rig.locals.get(bone).map(|t| t.rotation) else {
                continue;
            };
            let base = if cur == w.last_out { w.base } else { cur };
            let out = (spin(w.angle) * base).normalize();
            w.base = base;
            w.last_out = out;
            if out != cur {
                let rig = &mut *rig;
                rig.locals[bone].rotation = out;
                rig.pose_dirty = true;
            }
        }
    }
}

/// Register [`spin_wheels`] in the pose post-pass window, after the body twist, and only while a
/// wheeled model is in the world.
pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        spin_wheels
            .in_set(benilla_world::rig_anim::PosePost)
            .after(super::twist::apply_body_twist)
            .run_if(any_with_component::<Wheels>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    /// The step is the frame clock's gap, clamped to 1-500 ms, a backward clock to 1 ms.
    #[test]
    fn the_step_is_clamped_to_one_to_five_hundred_ms() {
        assert_eq!(elapsed_secs(1_016, 1_000), 0.016);
        assert_eq!(elapsed_secs(1_000, 1_000), 0.001);
        assert_eq!(elapsed_secs(900, 1_000), 0.001);
        assert_eq!(elapsed_secs(10_000, 0), 0.5);
    }

    /// Forward and either strafe roll forward at the speed, backward rolls back, and forward wins
    /// over backward; standing still rolls nothing.
    #[test]
    fn forward_and_strafes_roll_forward_and_backward_rolls_back() {
        use move_flags as f;
        for flags in [
            f::FORWARD,
            f::STRAFE_LEFT,
            f::STRAFE_RIGHT,
            f::FORWARD | f::BACKWARD,
        ] {
            assert_eq!(drive_inputs(flags, 8.0, PI, 0.0, 0.5).0, 4.0, "{flags:#x}");
        }
        assert_eq!(drive_inputs(f::BACKWARD, 4.5, PI, 0.0, 0.5).0, -2.25);
        assert_eq!(drive_inputs(0, 8.0, PI, 0.0, 0.5).0, 0.0);
    }

    /// A left turn is negative and a right one positive, each less the facing velocity; with no
    /// key the turn is the facing velocity's alone, negated.
    #[test]
    fn a_left_turn_is_negative_and_the_facing_velocity_is_taken_off() {
        use move_flags as f;
        assert_eq!(drive_inputs(f::TURN_LEFT, 0.0, 2.0, 0.0, 0.5).1, -1.0);
        assert_eq!(drive_inputs(f::TURN_RIGHT, 0.0, 2.0, 0.0, 0.5).1, 1.0);
        assert_eq!(drive_inputs(f::TURN_LEFT, 0.0, 2.0, 1.0, 0.5).1, -1.5);
        assert_eq!(drive_inputs(f::TURN_RIGHT, 0.0, 2.0, 1.0, 0.5).1, 0.5);
        assert_eq!(drive_inputs(0, 0.0, 2.0, 1.0, 0.5).1, -0.5);
    }

    /// The angle steps by `(left · turn + distance) / up` at the world scale: the Demolisher's front
    /// left wheel (left 2.302, up 1.482) driven 1.482 yd at scale 1 turns 1 rad, and a left turn
    /// slows it as the inner wheel while the right wheel speeds up.
    #[test]
    fn the_angle_steps_by_distance_plus_the_turn_share_over_the_height() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(advance(0.0, 2.302, 1.482, 1.0, 0.0, 1.482), 1.0));
        // At scale 0.75 the pivot shrinks, so the same distance turns the wheel further.
        assert!(close(
            advance(0.0, 2.302, 1.482, 0.75, 0.0, 1.482),
            1.0 / 0.75
        ));
        let turn = -0.1;
        let inner = advance(0.0, 2.302, 1.482, 1.0, turn, 1.0);
        let outer = advance(0.0, -2.302, 1.482, 1.0, turn, 1.0);
        assert!(
            inner < 1.0 / 1.482 && outer > 1.0 / 1.482,
            "{inner} {outer}"
        );
        // A pivot on the ground keeps the angle.
        assert_eq!(advance(0.3, 2.302, 0.0005, 1.0, turn, 1.0), 0.3);
    }

    /// The angle wraps to ±π by whole turns.
    #[test]
    fn the_angle_wraps_to_plus_minus_pi() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(advance(3.0, 0.0, 1.0, 1.0, 0.0, 0.5), 3.5 - 2.0 * PI));
        assert!(close(
            advance(-3.0, 0.0, 1.0, 1.0, 0.0, -0.5),
            2.0 * PI - 3.5
        ));
        assert_eq!(advance(1.0, 0.0, 1.0, 1.0, 0.0, 0.5), 1.5);
    }

    /// A flying spline, swimming or flying holds the angle; walking does not.
    #[test]
    fn a_flying_spline_or_the_swim_and_fly_flags_hold_the_angle() {
        use move_flags as f;
        assert!(holds(0, true));
        assert!(holds(f::SWIMMING, false));
        assert!(holds(f::FLYING | f::FORWARD, false));
        assert!(!holds(f::FORWARD | f::CAN_FLY, false));
    }

    /// Driving forward rolls the wheel's top toward the model's front (WoW `+X`).
    #[test]
    fn driving_forward_rolls_the_top_of_the_wheel_forward() {
        use benilla_assets::coords::wow_to_bevy;
        let angle = advance(0.0, 2.302, 1.482, 1.0, 0.0, 0.1);
        let top = spin(angle) * wow_to_bevy([0.0, 0.0, 1.0]);
        assert!(top.dot(wow_to_bevy([1.0, 0.0, 0.0])) > 0.0, "{top:?}");
        // A quarter turn carries the top to the front.
        let quarter = spin(FRAC_PI_2) * wow_to_bevy([0.0, 0.0, 1.0]);
        assert!((quarter - wow_to_bevy([1.0, 0.0, 0.0])).length() < 1e-5);
    }

    /// A model with no wheel bone gets no wheels; the pivot lands as its left and up coordinates.
    #[test]
    fn the_wheels_take_each_pivots_left_and_up() {
        use benilla_assets::coords::wow_to_bevy;
        assert!(Wheels::new(std::iter::empty()).is_none());
        let w = Wheels::new([(28, wow_to_bevy([2.979, 2.302, 1.482]))]).unwrap();
        assert_eq!(
            (w.wheels[0].bone, w.wheels[0].left, w.wheels[0].up),
            (28, 2.302, 1.482)
        );
    }

    /// The system rolls a wheel forward on the mover's forward run and composes the spin outside
    /// an unkeyed bone's rotation without accumulating it frame over frame.
    #[test]
    fn the_spin_composes_onto_the_wheel_bone_without_accumulating() {
        use benilla_assets::coords::wow_to_bevy;
        use benilla_assets::{ModelJoint, ModelSkeleton};
        let mut app = App::new();
        app.insert_resource(Time::<()>::default());
        app.add_systems(Update, spin_wheels);
        let unit = app.world_mut().spawn(GlobalTransform::IDENTITY).id();
        let joint = |parent: i16, at: [f32; 3]| ModelJoint {
            parent,
            local_translation: wow_to_bevy(at),
            billboard: None,
            parent_arm: None,
        };
        let skeleton = ModelSkeleton {
            joints: vec![joint(-1, [0.0; 3]), joint(0, [0.0, 1.0, 1.0])],
            spine_bone: None,
            head_bone: None,
            wheel_bones: [Some(1), None, None, None, None, None, None, None],
        };
        let pose = benilla_world::rig_anim::RigPose::new(unit, &skeleton);
        let pivot = pose.bind_pivot(1).unwrap();
        let driving = MovementState {
            speed: 1.0,
            flags: move_flags::FORWARD,
            ..Default::default()
        };
        app.world_mut().entity_mut(unit).insert((
            pose,
            Wheels::new([(1, pivot)]).unwrap(),
            driving,
        ));
        let mut angles = Vec::new();
        for ms in [100u64, 200, 300] {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_to(std::time::Duration::from_millis(ms));
            app.update();
            let rot = app
                .world()
                .get::<benilla_world::rig_anim::RigPose>(unit)
                .unwrap()
                .locals[1]
                .rotation;
            let (axis, angle) = rot.to_axis_angle();
            angles.push(angle * axis.dot(wow_to_bevy([0.0, 1.0, 0.0])).signum());
        }
        // 100 ms a frame at 1 yd/s over a 1 yd height: 0.1 rad a frame, never compounded.
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(
            close(angles[0], 0.1) && close(angles[1], 0.2) && close(angles[2], 0.3),
            "{angles:?}"
        );
    }
}
