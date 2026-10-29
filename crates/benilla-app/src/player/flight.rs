//! 3.3.5a flight, `MOVEFLAG_FLYING` under `CAN_FLY`. The jump key takes flight instead of jumping
//! (`JumpOrAscendStart` `0x5fbf80` → the local flight handler `0x718860`); a fall with
//! [`CLEARANCE`] under it takes flight by itself, and a flight past [`LANDING_HOLD`] ends inside it
//! (the flight status check, `0x73a890`). Each edge sends `CMSG_MOVE_SET_FLY` and then resyncs the
//! vertical channel (`0x7413f0`), so a held jump key climbs at once. A flyer strokes as a swimmer
//! does with no surface over it (`0x987b50`), at the flight speeds.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::mover::Outcome;
use super::Player;

/// The clearance (yd) between flight and the ground, `[0xa41b18]`.
const CLEARANCE: f32 = 1.5;
/// How long (s) a fresh flight ignores the ground: the landing arm wants more than 2000 ms
/// (`0x73aa59`).
const LANDING_HOLD: f32 = 2.0;
/// The ground probe's reach either side of the feet (yd), the height-above-ground probe's
/// (`0x714b60`).
const PROBE_REACH: f32 = 1.0;

/// Flight speed (yd/s), AzerothCore's `baseMoveSpeed[MOVE_FLIGHT]`: the fallback until the
/// server's speeds stream in, as [`super::swim::SWIM_SPEED`] is for swimming.
const FLIGHT_SPEED: f32 = 7.0;
/// Backward flight speed (yd/s), `baseMoveSpeed[MOVE_FLIGHT_BACK]`.
const FLIGHT_BACK_SPEED: f32 = 4.5;

/// A flight edge the status check finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    TakeOff,
    Land,
}

/// The flight status check (`0x73a890`), which only a `CAN_FLY` mover runs. A fall past its apex
/// (the rising test, `0x986ea0`) with [`CLEARANCE`] under it takes off; a flyer past
/// [`LANDING_HOLD`] lands inside it. `height` is [`height_above_ground`], probed only when an arm
/// reads it.
pub(super) fn status(
    can_fly: bool,
    flying: bool,
    falling: bool,
    rising: bool,
    since: f32,
    now: f32,
    height: impl FnOnce() -> f32,
) -> Option<Edge> {
    if !can_fly {
        return None;
    }
    if !flying {
        return (falling && !rising && height() >= CLEARANCE).then_some(Edge::TakeOff);
    }
    (now - since > LANDING_HOLD && height() < CLEARANCE).then_some(Edge::Land)
}

/// The height-above-ground probe (`0x714b60`): a ray from a yard over the feet to a yard under
/// them, so 0 to 2 yd, and 2 with nothing in reach.
pub(super) fn height_above_ground(
    collide: &benilla_world::collision::WorldCollision<'_, '_>,
    feet: Vec3,
) -> f32 {
    let reach = 2.0 * PROBE_REACH;
    collide
        .ray_body(feet + Vec3::Y * PROBE_REACH, Dir3::NEG_Y, reach)
        .map_or(reach, |hit| hit.distance)
}

/// Take flight, the flight start (`0x989890`): refused without `CAN_FLY`, or under a root (the
/// reference refuses a pending one; ours applies at once). SWIMMING, FALLING and FALLING_FAR end
/// silently, so the arc owes no landing.
pub(super) fn take_off(player: &mut Player, now: f32) -> bool {
    if !player.modes.can_fly || player.modes.rooted {
        return false;
    }
    player.flying = true;
    player.swimming = false;
    player.airborne_since = None;
    player.fall_far = false;
    player.knock_arc = false;
    player.vel_y = 0.0;
    player.flight_since = now;
    true
}

/// End flight, the flight stop (`0x98c240`): FLYING and the vertical flags clear, the pitch levels,
/// and the body falls from rest wherever no ground holds it.
pub(super) fn land(player: &mut Player) {
    player.flying = false;
    player.vertical.end();
    player.mover_pitch = 0.0;
    player.vel_y = 0.0;
}

/// One flying frame: the swim stroke ([`super::swim::stroke_direction`]) at the flight speeds,
/// with no surface to cap it (`0x987b50`); backward takes `min(flightBack, flight)` (`0x987570`).
/// `basis`, `amounts` and `vertical` are as [`super::swim::drive_step`] takes them.
pub(super) fn drive_step(
    player: &mut Player,
    time: &Time,
    collide: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    basis: (Vec3, Vec3),
    amounts: (f32, f32),
    vertical: f32,
    speeds: Option<benilla_protocol::MoveSpeeds>,
    move_speed: &super::MoveSpeed,
) -> Outcome {
    let dir = super::swim::stroke_direction(player.mover_pitch, basis, amounts, vertical);
    let (flight, back) = match speeds {
        Some(s) if !move_speed.env_override => (s.flight, s.flight_back),
        _ => (FLIGHT_SPEED, FLIGHT_BACK_SPEED),
    };
    let speed = if amounts.0 < 0.0 {
        back.min(flight)
    } else {
        flight
    };
    // The stroke clip's rate numerator, as for a swimmer.
    player.swim_stroke_speed = if dir == Vec3::ZERO { 0.0 } else { speed };
    let out = super::swim::swim_step(player, time, collide, capsule, dir * speed, None, |_| None);
    Outcome {
        held: false,
        grounded: out.grounded,
        jumped: false,
        knocked: false,
        air_nudged: false,
        ground: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> f32 {
        panic!("the probe must not run")
    }

    /// Without `CAN_FLY` the check never runs, whatever the height.
    #[test]
    fn only_a_mover_that_may_fly_runs_the_check() {
        assert_eq!(status(false, false, true, false, 0.0, 9.0, never), None);
        assert_eq!(status(false, true, false, false, 0.0, 9.0, never), None);
    }

    /// A fall takes off only past its apex and with 1.5 yd under it; a grounded mover never probes.
    #[test]
    fn a_fall_past_its_apex_with_clearance_takes_off() {
        assert_eq!(status(true, false, false, false, 0.0, 1.0, never), None);
        assert_eq!(
            status(true, false, true, true, 0.0, 1.0, || 2.0),
            None,
            "rising"
        );
        assert_eq!(
            status(true, false, true, false, 0.0, 1.0, || 2.0),
            Some(Edge::TakeOff)
        );
        assert_eq!(
            status(true, false, true, false, 0.0, 1.0, || CLEARANCE),
            Some(Edge::TakeOff),
            "the take-off arm is `>=`"
        );
        assert_eq!(status(true, false, true, false, 0.0, 1.0, || 1.49), None);
    }

    /// A flight ignores the ground for its first 2 s, then lands under 1.5 yd and not at it.
    #[test]
    fn a_flight_lands_inside_the_clearance_after_the_hold() {
        assert_eq!(status(true, true, false, false, 10.0, 11.9, never), None);
        assert_eq!(
            status(true, true, false, false, 10.0, 12.0, never),
            None,
            "exactly 2 s is not past"
        );
        assert_eq!(
            status(true, true, false, false, 10.0, 12.1, || 0.3),
            Some(Edge::Land)
        );
        assert_eq!(
            status(true, true, false, false, 10.0, 12.1, || CLEARANCE),
            None
        );
    }

    /// Taking flight ends the swim and the arc, and stamps the hold; no `CAN_FLY` or a root refuses.
    #[test]
    fn take_off_needs_can_fly_and_ends_the_swim_and_the_arc() {
        let mut p = Player::default();
        assert!(!take_off(&mut p, 1.0));
        p.modes.can_fly = true;
        p.modes.rooted = true;
        assert!(!take_off(&mut p, 1.0));
        p.modes.rooted = false;
        p.swimming = true;
        p.airborne_since = Some(0.5);
        p.fall_far = true;
        p.vel_y = -9.0;
        assert!(take_off(&mut p, 1.0));
        assert!(p.flying && !p.swimming && !p.fall_far);
        assert_eq!(p.airborne_since, None, "no FALL_LAND owed");
        assert_eq!((p.vel_y, p.flight_since), (0.0, 1.0));
    }

    /// Landing clears flight and the vertical flags and levels the pitch.
    #[test]
    fn landing_levels_the_pitch_and_ends_the_vertical_flags() {
        let mut p = Player {
            flying: true,
            mover_pitch: 0.6,
            ..Default::default()
        };
        p.vertical.resync(true, false, true, true);
        assert_ne!(p.vertical.flags(), 0);
        land(&mut p);
        assert!(!p.flying);
        assert_eq!(p.vertical.flags(), 0);
        assert_eq!(p.mover_pitch, 0.0);
    }
}
