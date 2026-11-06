//! Click to Move's walks (`AutoInteract`, "Toggles auto-move to interact target"). With the option
//! on, a right-click on an NPC, a body to loot or skin, or a GameObject beyond the verb's reach
//! walks the player there and the verb runs on arrival; a right-click on the ground walks to the
//! point (mode 4), one on the sky walks that way until stopped (mode 8), and an attack on an enemy
//! out of melee walks to where it stood (mode 10). It is [`super::follow`]'s auto-move: one cell
//! holds either, so arming one ends the other, the same input cancels both, and nothing goes on the
//! wire but the movement; an attack on an enemy already in melee only turns to face it (mode 2).

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_8, PI};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::net::{Embodied, Guid, GuidIndex, NetEntity, ObjectStore, SelfPlayer, UnitSpeeds};

use super::follow::{self, FollowInput, FollowState};
use super::state::Player;

/// The verb an approach owes, by its mode; the pending dispatcher runs it at the stop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ApproachVerb {
    /// Mode 5: the NPC-service dispatch.
    Talk,
    /// Mode 6: the unit loot.
    Loot,
    /// Mode 7: the GameObject use.
    Use,
    /// Mode 9: the skin cast.
    Skin,
    /// Mode 10: the combat enter.
    Attack,
}

/// What the start gate checks the target for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Subject {
    Unit { dead: bool },
    Corpse,
    GameObject,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refused {
    Silent,
    /// `ERR_AUTOFOLLOW_TOO_FAR` (`0x126`).
    TooFar,
}

/// Talk's stop off the target's combat reach, `0.5 × reach + 2.0` (`0x71b40a`-`0x71b427`).
pub(crate) fn talk_stop(combat_reach: f32) -> f32 {
    0.5 * combat_reach + 2.0
}

/// Use and Skin stop at this fraction of the verb's range.
pub(crate) const RANGE_STOP_FRACTION: f32 = 0.9;
/// The ground walk stops half a yard short.
const GROUND_STOP: f32 = 0.5;
/// No approach starts or lasts at 80 yd or more: the rows of modes 5-7, 9 and 10; the ground and
/// sky rows have none.
const LEASH_SQ: f32 = 6400.0;
/// The sky walk's swim pitch: straight up or down past this sine, level within the next.
const SKY_PITCH_STEEP: f32 = 0.866_02;
const SKY_PITCH_LEVEL: f32 = 0.173_64;
/// `automoveturnspeednarrow` and `automoveturnspeedwide`, deg/s.
const TURN_NARROW: f32 = 800.0;
const TURN_WIDE: f32 = 1200.0;
/// Less ground than this in a frame while lined up and holding forward is stuck.
const STUCK: f32 = 1.0 / 360.0;
/// The fifth frame in a row of losing ground or not moving ends the walk (`0x731969`, `0x731c62`).
const STUCK_FRAMES: u8 = 5;
/// A pitch this close to its aim has arrived (`0x72f1f0`).
const PITCH_DEADZONE: f32 = 0.001;
/// A goal more than `sqrt 2` yd off (`0x731dd3`) and steeper than 45° takes flight (`0x731d72`).
const FLIGHT_MIN_DIST_SQ: f32 = 2.0;

/// The approach in flight, and the verb an arrival owes.
#[derive(Resource, Default)]
pub(crate) struct Approach {
    /// `AutoInteract`, the auto-interact check's last term.
    pub(crate) enabled: bool,
    goal: Option<Goal>,
    /// The pending cells the arrival fills for the dispatcher.
    pub(crate) arrived: Option<(ApproachVerb, u64)>,
}

/// What a walk heads for, by its mode.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Toward {
    /// Modes 5-7 and 9: the object's live position.
    Object { verb: ApproachVerb, guid: u64 },
    /// Mode 4.
    Ground(Anchor),
    /// Mode 10: where the enemy stood, the enemy owed the swing.
    Melee { at: Anchor, guid: u64 },
    /// Mode 8: a facing, with no stop.
    Sky(f32),
    /// Mode 2: a facing, turned to and done.
    Face(f32),
}

/// A point fixed at the arm, held in the frame of the transport the player stands on and carried
/// back to the world each frame.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Anchor {
    at: Vec3,
    frame: Option<Entity>,
}

impl Toward {
    fn is_talk(self) -> bool {
        matches!(
            self,
            Toward::Object {
                verb: ApproachVerb::Talk,
                ..
            }
        )
    }

    /// Only the ground walk scales its stop with speed (`0x731cee`).
    fn scales_with_speed(self) -> bool {
        matches!(self, Toward::Ground(_))
    }

    /// Every mode but face, follow, talk and sky counts a frame that loses ground (`0x731f34`).
    fn counts_lost_ground(self) -> bool {
        !self.is_talk() && !matches!(self, Toward::Sky(_) | Toward::Face(_))
    }

    fn leashed(self) -> bool {
        matches!(self, Toward::Object { .. } | Toward::Melee { .. })
    }
}

struct Goal {
    toward: Toward,
    /// The stop distance.
    stop: f32,
    /// Latched by the first turn.
    turn_rate: Option<f32>,
    /// Bit 1: the facing has lined up once.
    aligned: bool,
    /// Last frame's position.
    last_pos: Vec3,
    held: bool,
    /// The swim pitch the sky arm sets, applied on the walk's first frame.
    pitch: Option<f32>,
    /// `0xca11d4`: frames in a row of losing ground or not moving.
    stuck: u8,
    /// The ground or melee point in the world, for the destination ring.
    marker: Option<Vec3>,
}

impl Goal {
    /// One more stuck frame; true on the fifth in a row.
    fn strike(&mut self) -> bool {
        self.stuck += 1;
        self.stuck >= STUCK_FRAMES
    }
}

impl Approach {
    /// The reference's auto-move test for this mode family: an approach is in flight.
    pub(crate) fn active(&self) -> bool {
        self.goal.is_some()
    }

    pub(crate) fn stop(&mut self) {
        self.goal = None;
    }

    /// The point a ground or melee walk heads for, the destination ring's anchor (`0x7275c0`).
    pub(crate) fn marker(&self) -> Option<Vec3> {
        self.goal.as_ref().and_then(|g| g.marker)
    }

    /// The facing turn (mode 2), which an attack round leaves alone.
    pub(crate) fn facing(&self) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|g| matches!(g.toward, Toward::Face(_)))
    }

    /// Our own attack round ends any auto-move but the facing turn (`0x75699a`, `0x7569a3`).
    pub(crate) fn end_on_own_round(&mut self, follow: &mut FollowState) {
        if self.facing() {
            return;
        }
        if self.active() {
            info!("approach: ended by our own attack round");
            self.stop();
        }
        if follow.guid.is_some() {
            info!("follow: ended by our own attack round");
            follow.stop();
        }
    }

    #[cfg(test)]
    pub(crate) fn stop_distance(&self) -> Option<f32> {
        self.goal.as_ref().map(|g| g.stop)
    }

    #[cfg(test)]
    pub(crate) fn stuck_frames(&self) -> Option<u8> {
        self.goal.as_ref().map(|g| g.stuck)
    }
}

/// The auto-interact check: alive, driving our own body, and the option on.
pub(crate) fn can_auto_interact(enabled: bool, me: Option<&ObjectStore>, player: &Player) -> bool {
    enabled
        && me.is_some_and(|s| s.0.unit_health().unwrap_or(0) > 0)
        && player.foreign_mover.is_none()
        && !player.control_lost
}

/// The start: the gates (the auto-interact check for the rest) and the arm.
#[derive(SystemParam)]
pub(crate) struct AutoMove<'w, 's> {
    pub(crate) approach: ResMut<'w, Approach>,
    follow: ResMut<'w, FollowState>,
    player: Res<'w, Player>,
    me: Query<'w, 's, (&'static Guid, &'static ObjectStore), With<SelfPlayer>>,
    stand: MessageWriter<'w, super::StandStateRequest>,
    transports: Query<'w, 's, &'static Transform>,
}

impl AutoMove<'_, '_> {
    pub(crate) fn can_auto_interact(&self) -> bool {
        can_auto_interact(
            self.approach.enabled,
            self.me.single().ok().map(|(_, s)| s),
            &self.player,
        )
    }

    /// The interaction walks' gate: the verb's own kind of target, never ourselves.
    pub(crate) fn start(
        &mut self,
        verb: ApproachVerb,
        guid: u64,
        subject: Subject,
        at: Vec3,
        stop: f32,
    ) -> Result<(), Refused> {
        let Ok((me, store)) = self.me.single() else {
            return Err(Refused::Silent);
        };
        let admitted = match (verb, subject) {
            (ApproachVerb::Talk, Subject::Unit { .. }) => true,
            (ApproachVerb::Loot | ApproachVerb::Skin, Subject::Corpse) => true,
            (ApproachVerb::Loot | ApproachVerb::Skin, Subject::Unit { dead }) => dead,
            (ApproachVerb::Use, Subject::GameObject) => true,
            _ => false,
        };
        if !admitted
            || guid == me.0
            || self.player.foreign_mover.is_some()
            || self.player.control_lost
            || store.0.unit_health().unwrap_or(0) == 0
        {
            return Err(Refused::Silent);
        }
        self.arm(Toward::Object { verb, guid }, at, stop)
    }

    /// The right-click on the ground.
    pub(crate) fn walk_to_ground(&mut self, at: Vec3) -> bool {
        if !self.can_auto_interact() {
            return false;
        }
        let anchor = self.anchor(at);
        self.arm(Toward::Ground(anchor), at, GROUND_STOP).is_ok()
    }

    /// The right-click on the sky, along the press's ray.
    pub(crate) fn walk_toward_sky(&mut self, ray: Vec3) -> bool {
        let dir = ray.normalize_or_zero();
        if dir == Vec3::ZERO || !self.can_auto_interact() {
            return false;
        }
        let facing = follow::bearing_to(Vec3::new(dir.x, 0.0, dir.z));
        let at = self.player.pos;
        if self.arm(Toward::Sky(facing), at, 0.0).is_err() {
            return false;
        }
        // Swimming or flying the arm pitches along the ray (`0x72f0ce`).
        if let Some(goal) = self.approach.goal.as_mut() {
            goal.pitch = (self.player.swimming || self.player.flying).then(|| sky_pitch(dir.y));
        }
        true
    }

    /// The reference's melee walk, for an attack on an enemy out of melee: to where it stands,
    /// stopping `stop` short. Deviation: the reference re-projects the point's height onto the
    /// first walkable facet a zero-size box gather finds at the enemy; here it keeps the enemy's
    /// height, read only afloat. Deviation: the reference walks astray on a deck; this rides it.
    pub(crate) fn walk_into_melee(
        &mut self,
        guid: u64,
        at: Vec3,
        stop: f32,
    ) -> Result<(), Refused> {
        if !self.can_auto_interact() {
            return Err(Refused::Silent);
        }
        let anchor = self.anchor(at);
        self.arm(Toward::Melee { at: anchor, guid }, at, stop)
    }

    /// For an attack on an enemy in melee: turn to face `at`.
    pub(crate) fn face(&mut self, at: Vec3) -> bool {
        if !self.can_auto_interact() {
            return false;
        }
        let delta = at - self.player.pos;
        let facing = follow::bearing_to(Vec3::new(delta.x, 0.0, delta.z));
        self.arm(Toward::Face(facing), at, 0.0).is_ok()
    }

    /// Deviation: the reference converts back through the transport the player stands on each
    /// frame, so stepping off mid-walk sends the point astray; here the deck is fixed at the arm.
    fn anchor(&self, at: Vec3) -> Anchor {
        let frame = self.player.ride.as_ref().map(|r| r.entity);
        match frame.and_then(|e| self.transports.get(e).ok().map(|tf| (e, tf))) {
            Some((e, tf)) => Anchor {
                at: tf.compute_affine().inverse().transform_point3(at),
                frame: Some(e),
            },
            None => Anchor { at, frame: None },
        }
    }

    /// The canceller, then the leash on the world point `at`, then the goal.
    fn arm(&mut self, toward: Toward, at: Vec3, stop: f32) -> Result<(), Refused> {
        let standing = self
            .me
            .single()
            .is_ok_and(|(_, s)| s.0.unit_stand_state() == 0);
        self.follow.stop();
        self.approach.stop();
        if toward.leashed() && self.player.pos.distance_squared(at) >= LEASH_SQ {
            return Err(Refused::TooFar);
        }
        info!("approach: {toward:?}, stopping {stop:.2} yd short");
        self.approach.goal = Some(Goal {
            toward,
            stop,
            turn_rate: None,
            aligned: false,
            last_pos: self.player.pos,
            held: false,
            pitch: None,
            stuck: 0,
            marker: matches!(toward, Toward::Ground(_) | Toward::Melee { .. }).then_some(at),
        });
        if !standing {
            self.stand.write(super::StandStateRequest { state: 0 });
        }
        Ok(())
    }
}

/// The swim pitch the sky walk takes from its ray.
fn sky_pitch(y: f32) -> f32 {
    if y.abs() > SKY_PITCH_STEEP {
        FRAC_PI_2.copysign(y)
    } else if y.abs() > SKY_PITCH_LEVEL {
        y.asin()
    } else {
        0.0
    }
}

fn speed_scale(speed: f32) -> f32 {
    (speed / follow::SPEED_NORM).max(1.0)
}

/// The walk's distance squared: flat unless swimming, flying or able to fly (`0x731901`).
pub(super) fn goal_distance_sq(delta: Vec3, with_height: bool) -> f32 {
    if with_height {
        delta.length_squared()
    } else {
        delta.x * delta.x + delta.z * delta.z
    }
}

/// Whether [`goal_distance_sq`] counts the height for this mover.
pub(super) fn reads_height(player: &Player) -> bool {
    player.swimming || player.flying || player.modes.can_fly
}

/// The arrive distance: only the ground walk's stop scales, by speed over 7, never below 1×.
fn arrive_distance(toward: Toward, stop: f32, speed: f32) -> f32 {
    if toward.scales_with_speed() {
        stop * speed_scale(speed)
    } else {
        stop
    }
}

/// The rate the first turn latches, rad/s: wide for a turn of more than 120°.
fn turn_rate(remaining: f32) -> f32 {
    let deg = if remaining.abs() > 2.0 * PI / 3.0 {
        TURN_WIDE
    } else {
        TURN_NARROW
    };
    deg.to_radians()
}

/// The heading a walk steers by; a goal straight over or under the feet keeps the facing.
pub(super) fn bearing_or(face_yaw: f32, delta: Vec3) -> f32 {
    if delta.x * delta.x + delta.z * delta.z > f32::EPSILON {
        follow::bearing_to(Vec3::new(delta.x, 0.0, delta.z))
    } else {
        face_yaw
    }
}

/// One turn step, the rate latched by the first (`0x731650`): whether the facing still has to turn.
pub(super) fn steer_turn(
    player: &mut Player,
    bearing: f32,
    latched: &mut Option<f32>,
    dt: f32,
) -> bool {
    let remaining = follow::wrap_pi(bearing - player.face_yaw);
    if remaining.abs() <= follow::TURN_DEADZONE {
        return false;
    }
    let rate = *latched.get_or_insert_with(|| turn_rate(remaining));
    player.face_yaw = follow::steer(player.face_yaw, bearing, rate, dt);
    true
}

/// A frame's pitch: still moving, which the not-moving test waits for, and over 22.5° off its aim.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) struct PitchStep {
    pub(super) pitching: bool,
    pub(super) overshoot: bool,
}

/// The elevation to a goal (`0x71b580`): straight up or down with no ground between.
fn elevation(delta: Vec3) -> f32 {
    let dir = delta.normalize_or_zero();
    if dir.x.abs() < PITCH_DEADZONE && dir.z.abs() < PITCH_DEADZONE {
        return if delta.y > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 };
    }
    dir.y.asin()
}

/// The walk's pitch afloat: π rad/s to the elevation, level at the surface (`0x72f1f0`).
pub(super) fn steer_pitch(player: &mut Player, delta: Vec3, dt: f32) -> PitchStep {
    if !(player.swimming || player.flying) || delta.length_squared() <= STUCK * STUCK {
        return PitchStep::default();
    }
    let at_surface = player.liquid_surface.is_some_and(|surface| {
        let depth = surface - player.pos.y;
        depth > 0.0
            && (depth - super::swim::swim_enter_depth(player.collision_height.0)).abs()
                < PITCH_DEADZONE
    });
    if delta.y > 0.0 && at_surface {
        player.mover_pitch = 0.0;
        return PitchStep::default();
    }
    let want = elevation(delta);
    let pitch = player.mover_pitch;
    if (want - pitch).abs() < PITCH_DEADZONE {
        return PitchStep::default();
    }
    let step = PI * dt;
    let next = if want <= pitch {
        want.max(pitch - step)
    } else {
        want.min(pitch + step)
    };
    player.mover_pitch = next;
    PitchStep {
        pitching: true,
        overshoot: (next - want).abs() > FRAC_PI_8,
    }
}

/// Whether a walk on foot takes flight: it may fly and the goal is over 45° steep (`0x731d91`).
pub(super) fn takes_flight(player: &Player, delta: Vec3) -> bool {
    !(player.swimming || player.flying)
        && player.modes.can_fly
        && delta.length_squared() > FLIGHT_MIN_DIST_SQ
        && delta.normalize_or_zero().y.abs() > FRAC_PI_4.cos()
}

/// One frame of a walk in the reference's order (`0x7317a0`), after [`follow::steer_follow`].
pub(super) fn steer_approach(
    time: Res<Time>,
    mut approach: ResMut<Approach>,
    mut player: ResMut<Player>,
    mover: Query<&UnitSpeeds, With<Embodied>>,
    index: Res<GuidIndex>,
    targets: Query<(&Transform, Option<&ObjectStore>, Option<&NetEntity>)>,
    input: FollowInput,
) {
    if !approach.active() {
        return;
    }
    if input.cancels(&player) {
        info!("approach: cancelled by the player's own movement input");
        approach.stop();
        return;
    }
    // The current speed off last frame's word, as the reference reads the mover's own.
    let speed = mover
        .single()
        .map_or(0.0, |s| crate::net::current_speed(&s.0, player.move_flags));
    let Some(goal) = approach.goal.as_mut() else {
        return;
    };
    let toward = goal.toward;
    if let Some(pitch) = goal.pitch.take() {
        player.mover_pitch = pitch;
    }
    let target = match toward {
        Toward::Object { verb, guid } => {
            let Some((tf, store, kind)) = index.0.get(&guid).and_then(|e| targets.get(*e).ok())
            else {
                info!("approach: the target is gone");
                approach.stop();
                return;
            };
            // A unit on a taxi ends it, a dead one too unless the verb wants a body.
            if kind.is_some_and(|k| matches!(k.kind, EntityKind::Unit | EntityKind::Player)) {
                let store = store.map(|s| &s.0);
                let taxi =
                    store.is_some_and(|s| s.unit_flags() & super::UNIT_FLAG_TAXI_FLIGHT != 0);
                let dead = store.is_none_or(|s| s.unit_health().unwrap_or(0) == 0);
                if taxi || (dead && !matches!(verb, ApproachVerb::Loot | ApproachVerb::Skin)) {
                    info!("approach: the target left reach (taxi {taxi}, dead {dead})");
                    approach.stop();
                    return;
                }
            }
            Some(tf.translation)
        }
        Toward::Ground(anchor) | Toward::Melee { at: anchor, .. } => match anchor.frame {
            None => Some(anchor.at),
            Some(frame) => {
                let Ok((tf, ..)) = targets.get(frame) else {
                    info!("approach: the transport is gone");
                    approach.stop();
                    return;
                };
                Some(tf.transform_point(anchor.at))
            }
        },
        Toward::Sky(_) | Toward::Face(_) => None,
    };
    if goal.marker.is_some() {
        goal.marker = target;
    }
    let last_pos = std::mem::replace(&mut goal.last_pos, player.pos);
    let delta = target.map(|t| t - player.pos);
    let d2 = delta.map(|d| goal_distance_sq(d, reads_height(&player)));
    if toward.leashed() && d2.is_some_and(|d2| d2 >= LEASH_SQ) {
        info!("approach: beyond 80 yd");
        approach.stop();
        return;
    }
    // Once lined up, a frame that ends farther off than the last counts, but not Talk's.
    let mut stuck = false;
    if let Some(target) = target {
        if goal.aligned
            && toward.counts_lost_ground()
            && last_pos.distance_squared(target) < player.pos.distance_squared(target)
        {
            stuck = true;
            super::move_trace::auto_move("losing-ground", player.pos, target);
            if goal.strike() {
                info!("approach: no longer closing");
                approach.stop();
                return;
            }
        }
    }
    let dt = super::mover::move_step(&time).as_secs_f32();
    let bearing = match (toward, delta) {
        (Toward::Sky(facing) | Toward::Face(facing), _) => facing,
        (_, Some(d)) => bearing_or(player.face_yaw, d),
        (_, None) => player.face_yaw,
    };
    let turning = steer_turn(&mut player, bearing, &mut goal.turn_rate, dt);
    if !turning {
        // The facing walk ends once lined up, never holding forward.
        if let Toward::Face(_) = toward {
            approach.goal = None;
            return;
        }
        goal.aligned = true;
    }
    if let Toward::Face(_) = toward {
        player.auto_turning = turning;
        return;
    }
    let pitch = delta.map_or_else(PitchStep::default, |d| steer_pitch(&mut player, d, dt));
    player.auto_turning = turning || pitch.pitching;
    // Holding forward on a row with the stuck flag, a frame that covers nothing counts.
    if goal.held
        && !turning
        && !pitch.pitching
        && player.pos.distance_squared(last_pos) < STUCK * STUCK
    {
        stuck = true;
        super::move_trace::auto_move("not-moving", player.pos, target.unwrap_or(player.pos));
        if goal.strike() {
            info!("approach: stuck");
            approach.stop();
            return;
        }
    }
    if let (Some(d2), Some(target)) = (d2, target) {
        let arrive = arrive_distance(toward, goal.stop, speed);
        if d2 <= arrive * arrive {
            super::move_trace::auto_move("arrived", player.pos, target);
            info!("approach: arrived at {:.2} yd", d2.sqrt());
            // The end's facing flag writes the camera's facing alone (`0xca11d8`), not the body's.
            match toward {
                Toward::Object { verb, guid } => approach.arrived = Some((verb, guid)),
                Toward::Melee { guid, .. } => {
                    approach.arrived = Some((ApproachVerb::Attack, guid));
                }
                _ => {}
            }
            approach.goal = None;
            return;
        }
    }
    if !stuck {
        goal.stuck = 0;
    }
    if let Some(delta) = delta.filter(|d| !pitch.pitching && takes_flight(&player, *d)) {
        super::move_trace::auto_move("take-off", player.pos, player.pos + delta);
        info!("approach: taking flight for a steep goal");
        player.auto_take_off = true;
        player.auto_forward = goal.held;
        return;
    }
    // Rooted, or a pitch far off its aim, lets go of forward, which spares the stuck test.
    let hold = !player.modes.rooted && !(pitch.pitching && pitch.overshoot);
    goal.held = hold;
    player.auto_forward = hold;
}

fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut approach: ResMut<Approach>) {
    if ev.is("AutoInteract") {
        approach.enabled = ev.flag();
        debug!("approach: Click to Move {}", approach.enabled);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Approach>().add_observer(on_cvar);
}

#[cfg(test)]
mod tests {
    use super::super::state::{MoveSpeed, PlayerRide};
    use super::*;
    use benilla_protocol::messages::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    const fn object(verb: ApproachVerb) -> Toward {
        Toward::Object { verb, guid: 0 }
    }

    const POINT: Anchor = Anchor {
        at: Vec3::ZERO,
        frame: None,
    };

    /// Talk's stop off the target's reach, and only the ground walk scales (`0x731cee`).
    #[test]
    fn talk_stops_inside_the_service_reach_and_never_scales() {
        assert_eq!(talk_stop(1.5), 2.75);
        assert_eq!(talk_stop(6.0), 5.0);
        for verb in [
            ApproachVerb::Talk,
            ApproachVerb::Loot,
            ApproachVerb::Use,
            ApproachVerb::Skin,
        ] {
            assert_eq!(arrive_distance(object(verb), 2.75, 14.0), 2.75, "{verb:?}");
        }
    }

    /// The melee walk's stop is Talk's kind, never scaled; the ground's is, and never under 1×.
    #[test]
    fn the_ground_stop_scales_with_speed_and_the_melee_stop_does_not() {
        let ground = Toward::Ground(POINT);
        assert_eq!(arrive_distance(ground, GROUND_STOP, 7.0), 0.5);
        assert_eq!(arrive_distance(ground, GROUND_STOP, 14.0), 1.0);
        assert_eq!(arrive_distance(ground, GROUND_STOP, 3.5), 0.5);
        assert_eq!(arrive_distance(ground, GROUND_STOP, 0.0), 0.5, "at rest");
        let melee = Toward::Melee {
            at: POINT,
            guid: VENDOR,
        };
        assert_eq!(arrive_distance(melee, 3.67, 14.0), 3.67);
    }

    /// Flat for a walker at any height, true for a mover that swims, flies or may fly (`0x731901`).
    #[test]
    fn the_goal_is_flat_unless_the_mover_swims_flies_or_may_fly() {
        let above = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(goal_distance_sq(above, false), 9.0);
        assert_eq!(goal_distance_sq(above, true), 25.0);
        let mut player = Player::default();
        assert!(!reads_height(&player));
        player.swimming = true;
        assert!(reads_height(&player));
        player.swimming = false;
        player.flying = true;
        assert!(reads_height(&player));
        player.flying = false;
        player.modes.can_fly = true;
        assert!(reads_height(&player));
    }

    /// A walker stops at the flat stop under a goal on a ledge; the leash reads the same distance.
    #[test]
    fn a_walk_to_an_npc_above_stops_at_the_flat_distance() {
        let at = Vec3::new(10.0, 30.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        for _ in 0..300 {
            if app.world().resource::<Approach>().arrived.is_some() {
                break;
            }
            frame(&mut app, true);
        }
        let left = app.world().resource::<Player>().pos;
        assert!(app.world().resource::<Approach>().arrived.is_some());
        let flat = (at - left).with_y(0.0).length();
        assert!(flat <= STOP && flat > STOP - 7.0 * DT - 1e-3, "{flat}");
    }

    /// The sky walk's swim pitch off its ray: straight up or down when steep, level when shallow.
    #[test]
    fn the_sky_swim_pitch_clamps_steep_and_levels_shallow() {
        assert_eq!(sky_pitch(0.9), FRAC_PI_2);
        assert_eq!(sky_pitch(-0.9), -FRAC_PI_2);
        assert!((sky_pitch(0.5) - 0.5_f32.asin()).abs() < 1e-6);
        assert_eq!(sky_pitch(0.1), 0.0);
    }

    const ME: u64 = 0x1;
    const VENDOR: u64 = 0xF00D;
    const DT: f32 = 1.0 / 60.0;
    /// Talk's stop at the default combat reach.
    const STOP: f32 = 2.75;

    /// Our body at the origin facing -Z, alive (and seated when `stand_state` is set), a vendor at
    /// `at`, and both steers chained as the plugin runs them.
    fn world(at: Vec3, stand_state: u32) -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Approach>()
            .init_resource::<FollowState>()
            .init_resource::<Player>()
            .init_resource::<GuidIndex>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::bindings::BindingsState>()
            .init_resource::<super::super::camera::CameraControl>()
            .init_resource::<super::super::view_subject::ViewSubject>()
            .insert_resource(MoveSpeed {
                value: 7.0,
                env_override: false,
            })
            .add_message::<super::super::StandStateRequest>()
            .add_systems(Update, (follow::steer_follow, steer_approach).chain());
        app.world_mut().resource_mut::<Approach>().enabled = true;
        app.world_mut().spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
                (138, stand_state),
            ])),
        ));
        let vendor = app
            .world_mut()
            .spawn((Guid(VENDOR), Transform::from_translation(at)))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        app
    }

    fn start(app: &mut App, verb: ApproachVerb, subject: Subject, at: Vec3) -> Result<(), Refused> {
        app.world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.start(verb, VENDOR, subject, at, STOP))
            .unwrap()
    }

    /// One frame: the steers, then the controller's walk along the facing while forward is held.
    fn frame(app: &mut App, walks: bool) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(DT));
        app.update();
        let mut player = app.world_mut().resource_mut::<Player>();
        if walks && player.auto_forward {
            let forward = Quat::from_rotation_y(player.face_yaw) * Vec3::NEG_Z;
            player.pos += forward * 7.0 * DT;
        }
    }

    #[test]
    fn a_vendor_out_of_reach_is_walked_to_and_owed_the_talk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        frame(&mut app, true);
        assert!(
            app.world().resource::<Player>().auto_forward,
            "forward from the first frame"
        );
        let mut frames = 1;
        while app.world().resource::<Approach>().arrived.is_none() {
            assert!(frames < 300, "never arrived");
            frame(&mut app, true);
            frames += 1;
        }
        let player = app.world().resource::<Player>();
        let left = player.pos.distance(at);
        assert!(
            left <= STOP && left > STOP - 7.0 * DT - 1e-3,
            "stopped {left} yd off, not at the 2.75 yd stop"
        );
        assert!(!player.auto_forward, "forward let go at the stop");
        assert!(
            (follow::wrap_pi(player.face_yaw - follow::bearing_to(at - player.pos))).abs() < 2e-3,
            "facing the vendor"
        );
        let approach = app.world().resource::<Approach>();
        assert_eq!(approach.arrived, Some((ApproachVerb::Talk, VENDOR)));
        assert!(!approach.active());
    }

    #[test]
    fn the_arm_ends_a_follow_and_refuses_past_80_yards() {
        let near = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(near, 0);
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(0xBEEF, "Friend".into());
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                near
            ),
            Ok(())
        );
        assert!(
            app.world().resource::<FollowState>().guid.is_none(),
            "one auto-move cell"
        );

        let far = Vec3::new(80.0, 0.0, 0.0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                far
            ),
            Err(Refused::TooFar)
        );
        assert!(
            !app.world().resource::<Approach>().active(),
            "the arm cancelled the last walk"
        );
    }

    #[test]
    fn the_gate_admits_each_verb_its_own_kind_of_target() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        for (verb, subject, ok) in [
            (ApproachVerb::Talk, Subject::Unit { dead: false }, true),
            (ApproachVerb::Talk, Subject::GameObject, false),
            (ApproachVerb::Loot, Subject::Unit { dead: true }, true),
            (ApproachVerb::Loot, Subject::Unit { dead: false }, false),
            (ApproachVerb::Loot, Subject::Corpse, true),
            (ApproachVerb::Skin, Subject::Unit { dead: false }, false),
            (ApproachVerb::Use, Subject::GameObject, true),
            (ApproachVerb::Use, Subject::Unit { dead: false }, false),
        ] {
            let got = start(&mut app, verb, subject, at);
            assert_eq!(got.is_ok(), ok, "{verb:?} at {subject:?}: {got:?}");
        }
    }

    #[test]
    fn a_seated_player_stands_to_walk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 1);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        let asks: Vec<u8> = app
            .world_mut()
            .resource_mut::<Messages<super::super::StandStateRequest>>()
            .drain()
            .map(|m| m.state)
            .collect();
        assert_eq!(asks, vec![0]);
    }

    #[test]
    fn the_players_own_movement_cancels_the_walk() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        frame(&mut app, true);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Forward);
        frame(&mut app, true);
        let approach = app.world().resource::<Approach>();
        assert!(!approach.active() && approach.arrived.is_none());
    }

    #[test]
    fn a_walk_that_cannot_move_gives_up_once_lined_up() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(&mut app, ApproachVerb::Use, Subject::GameObject, at),
            Ok(())
        );
        frame(&mut app, false);
        frame(&mut app, false);
        assert!(
            app.world().resource::<Approach>().active(),
            "still turning, where standing still is not stuck"
        );
        for _ in 0..60 {
            frame(&mut app, false);
        }
        let approach = app.world().resource::<Approach>();
        assert!(!approach.active(), "stuck against a wall");
        assert!(approach.arrived.is_none(), "nothing owed");
    }

    /// A Use walk lined up on its first frame, so every later frame runs both stuck tests.
    fn lined_up_walk() -> App {
        let at = Vec3::new(0.0, 0.0, -14.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(&mut app, ApproachVerb::Use, Subject::GameObject, at),
            Ok(())
        );
        frame(&mut app, false);
        app
    }

    /// Four frames that cover nothing leave the walk on; the fifth ends it (`0x731c62`).
    #[test]
    fn a_walk_ends_on_the_fifth_frame_without_moving() {
        let mut app = lined_up_walk();
        for n in 1..=4 {
            frame(&mut app, false);
            let approach = app.world().resource::<Approach>();
            assert_eq!(approach.stuck_frames(), Some(n), "frame {n}");
        }
        frame(&mut app, false);
        assert!(!app.world().resource::<Approach>().active());
    }

    /// A clean frame resets the one counter (`0x731d52`).
    #[test]
    fn a_clean_frame_resets_the_stuck_count() {
        let mut app = lined_up_walk();
        for _ in 0..3 {
            frame(&mut app, false);
        }
        frame(&mut app, true);
        assert_eq!(app.world().resource::<Approach>().stuck_frames(), Some(4));
        frame(&mut app, false);
        assert_eq!(app.world().resource::<Approach>().stuck_frames(), Some(0));
        assert!(app.world().resource::<Approach>().active());
    }

    /// Walk the body one step back along the goal's line for a frame.
    fn back_off(app: &mut App) {
        app.world_mut().resource_mut::<Player>().pos.z += 0.1;
        frame(app, false);
    }

    /// Four frames losing ground leave the walk on, the fifth ends it, not Talk's (`0x7319fc`).
    #[test]
    fn a_walk_ends_on_the_fifth_frame_losing_ground_but_not_talk() {
        let mut app = lined_up_walk();
        for _ in 0..4 {
            back_off(&mut app);
            assert!(app.world().resource::<Approach>().active());
        }
        back_off(&mut app);
        assert!(!app.world().resource::<Approach>().active());

        let at = Vec3::new(0.0, 0.0, -14.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        frame(&mut app, false);
        for _ in 0..20 {
            back_off(&mut app);
        }
        assert!(app.world().resource::<Approach>().active());
    }

    /// A rooted walk lets go of forward, which spares the stuck tests.
    #[test]
    fn a_root_lets_go_of_forward_and_spares_the_walk() {
        let mut app = lined_up_walk();
        app.world_mut().resource_mut::<Player>().modes.rooted = true;
        for _ in 0..30 {
            frame(&mut app, false);
        }
        let approach = app.world().resource::<Approach>();
        assert!(approach.active());
        assert!(!app.world().resource::<Player>().auto_forward);
    }

    #[test]
    fn the_first_turn_picks_the_wide_rate_past_120_degrees() {
        assert_eq!(turn_rate(0.5), 800f32.to_radians());
        assert_eq!(turn_rate(-2.0 * PI / 3.0), 800f32.to_radians());
        assert_eq!(turn_rate(2.2), 1200f32.to_radians());
        assert_eq!(turn_rate(-PI), 1200f32.to_radians());
    }

    fn run_until_idle(app: &mut App, frames: usize) -> usize {
        for n in 0..frames {
            if !app.world().resource::<Approach>().active() {
                return n;
            }
            frame(app, true);
        }
        frames
    }

    /// The ground walk stops half a yard short at walking pace and owes nothing.
    #[test]
    fn a_ground_walk_stops_half_a_yard_short_and_owes_nothing() {
        let point = Vec3::new(0.0, 0.0, -10.0);
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(point))
            .unwrap());
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app.world().resource::<Player>().pos.distance(point);
        assert!(left <= 0.5 && left > 0.5 - 7.0 * DT - 1e-3, "{left}");
        assert!(app.world().resource::<Approach>().arrived.is_none());
    }

    /// The ground row has no leash (its entry is 0): a point past 80 yd still walks.
    #[test]
    fn a_ground_walk_has_no_leash() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        let far = Vec3::new(0.0, 0.0, -120.0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(far))
            .unwrap());
        for _ in 0..30 {
            frame(&mut app, true);
        }
        assert!(app.world().resource::<Approach>().active());
    }

    /// With the option off, neither world walk arms.
    #[test]
    fn the_world_walks_need_click_to_move() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        app.world_mut().resource_mut::<Approach>().enabled = false;
        let armed = app
            .world_mut()
            .run_system_once(|mut auto: AutoMove| {
                auto.walk_to_ground(Vec3::new(0.0, 0.0, -10.0))
                    || auto.walk_toward_sky(Vec3::new(1.0, 0.5, 0.0))
            })
            .unwrap();
        assert!(!armed);
    }

    /// The sky walk turns to the ray's heading and holds forward with no stop (row 8's `+9` flag).
    #[test]
    fn a_sky_walk_turns_to_the_ray_and_keeps_going() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        assert!(app
            .world_mut()
            .run_system_once(|mut auto: AutoMove| auto.walk_toward_sky(Vec3::new(1.0, 0.4, 0.0)))
            .unwrap());
        for _ in 0..600 {
            frame(&mut app, true);
        }
        let player = app.world().resource::<Player>();
        let want = follow::bearing_to(Vec3::X);
        assert!(follow::wrap_pi(player.face_yaw - want).abs() < 1e-3);
        assert!(player.auto_forward);
        assert!(player.pos.x > 60.0, "walked {}", player.pos.x);
        assert!(app.world().resource::<Approach>().active());
    }

    /// A ground point on a deck rides the deck.
    #[test]
    fn a_ground_point_on_a_transport_rides_it() {
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        let boat = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<Player>().ride = Some(PlayerRide {
            entity: boat,
            guid: 0xB0A7,
            local_pos: Vec3::ZERO,
            boat_yaw: 0.0,
        });
        let point = Vec3::new(0.0, 0.0, -10.0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(point))
            .unwrap());
        app.world_mut()
            .entity_mut(boat)
            .insert(Transform::from_xyz(-6.0, 0.0, 0.0));
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app
            .world()
            .resource::<Player>()
            .pos
            .distance(Vec3::new(-6.0, 0.0, -10.0));
        assert!(left <= 0.5, "{left}");
    }

    /// Mode 10 stops at melee reach less its offset and owes the combat enter (`0x72b3a0`).
    #[test]
    fn a_melee_walk_stops_at_its_stop_and_owes_the_swing() {
        let at = Vec3::new(0.0, 0.0, -10.0);
        let stop = 5.0_f32 - 1.333_333_3;
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        assert_eq!(
            app.world_mut()
                .run_system_once(move |mut auto: AutoMove| auto.walk_into_melee(VENDOR, at, stop))
                .unwrap(),
            Ok(())
        );
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app.world().resource::<Player>().pos.distance(at);
        assert!(left <= stop && left > stop - 7.0 * DT - 1e-3, "{left}");
        assert_eq!(
            app.world().resource::<Approach>().arrived,
            Some((ApproachVerb::Attack, VENDOR))
        );
    }

    /// Only a ground or melee walk exposes its point, until it arrives (`0x7275c0`).
    #[test]
    fn only_a_ground_or_melee_walk_exposes_its_anchor() {
        let point = Vec3::new(0.0, 0.0, -10.0);
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(point))
            .unwrap());
        assert_eq!(app.world().resource::<Approach>().marker(), Some(point));
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        assert_eq!(app.world().resource::<Approach>().marker(), None);
        assert_eq!(
            app.world_mut()
                .run_system_once(move |mut auto: AutoMove| auto.walk_into_melee(VENDOR, point, 3.67))
                .unwrap(),
            Ok(())
        );
        assert_eq!(app.world().resource::<Approach>().marker(), Some(point));
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.face(point))
            .unwrap());
        assert_eq!(app.world().resource::<Approach>().marker(), None);
        let vendor = Vec3::new(40.0, 0.0, 0.0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                vendor
            ),
            Ok(())
        );
        assert_eq!(app.world().resource::<Approach>().marker(), None);
    }

    /// A ground walk on a deck carries its ring with the deck.
    #[test]
    fn the_marker_rides_the_transport() {
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        let boat = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<Player>().ride = Some(PlayerRide {
            entity: boat,
            guid: 0xB0A7,
            local_pos: Vec3::ZERO,
            boat_yaw: 0.0,
        });
        let point = Vec3::new(0.0, 0.0, -10.0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(point))
            .unwrap());
        app.world_mut()
            .entity_mut(boat)
            .insert(Transform::from_xyz(-6.0, 0.0, 0.0));
        frame(&mut app, false);
        let marker = app.world().resource::<Approach>().marker().unwrap();
        assert!(
            marker.distance(Vec3::new(-6.0, 0.0, -10.0)) < 1e-4,
            "{marker}"
        );
    }

    /// Our round ends every walk and a follow, but not the facing turn (mask 4, `0x7569a3`).
    #[test]
    fn our_own_round_ends_the_auto_move_but_not_the_facing_turn() {
        let at = Vec3::new(14.0, 0.0, 0.0);
        let mut app = world(at, 0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.face(at))
            .unwrap());
        let end = |app: &mut App| {
            app.world_mut()
                .run_system_once(|mut a: ResMut<Approach>, mut f: ResMut<FollowState>| {
                    a.end_on_own_round(&mut f);
                })
                .unwrap();
        };
        end(&mut app);
        assert!(app.world().resource::<Approach>().facing());

        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(at))
            .unwrap());
        end(&mut app);
        assert!(!app.world().resource::<Approach>().active());

        app.world_mut()
            .resource_mut::<FollowState>()
            .start(VENDOR, "Friend".into());
        end(&mut app);
        assert!(app.world().resource::<FollowState>().guid.is_none());
    }

    /// Mode 2: ends a follow, turns to the enemy and is done, never walking.
    #[test]
    fn a_face_turns_to_the_enemy_ends_a_follow_and_never_walks() {
        let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(0xBEEF, "Friend".into());
        let enemy = Vec3::new(3.0, 0.0, 0.0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.face(enemy))
            .unwrap());
        assert!(app.world().resource::<FollowState>().guid.is_none());
        let mut walked = false;
        let frames = (0..120)
            .take_while(|_| {
                frame(&mut app, true);
                walked |= app.world().resource::<Player>().auto_forward;
                app.world().resource::<Approach>().active()
            })
            .count();
        assert!(frames < 119, "never lined up");
        assert!(!walked);
        let player = app.world().resource::<Player>();
        assert_eq!(player.pos, Vec3::ZERO);
        assert!(follow::wrap_pi(player.face_yaw - follow::bearing_to(enemy)).abs() < 1e-3);
    }

    /// The deck-to-world carry on a turning deck: the point turns with it.
    #[test]
    fn a_ground_point_on_a_turning_transport_turns_with_it() {
        let mut app = world(Vec3::new(40.0, 0.0, 0.0), 0);
        let boat = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().resource_mut::<Player>().ride = Some(PlayerRide {
            entity: boat,
            guid: 0xB0A7,
            local_pos: Vec3::ZERO,
            boat_yaw: 0.0,
        });
        let point = Vec3::new(0.0, 0.0, -10.0);
        assert!(app
            .world_mut()
            .run_system_once(move |mut auto: AutoMove| auto.walk_to_ground(point))
            .unwrap());
        app.world_mut()
            .entity_mut(boat)
            .insert(Transform::from_rotation(Quat::from_rotation_y(FRAC_PI_2)));
        assert!(run_until_idle(&mut app, 300) < 300, "never arrived");
        let left = app
            .world()
            .resource::<Player>()
            .pos
            .distance(Vec3::new(-10.0, 0.0, 0.0));
        assert!(left <= 0.5, "{left}");
    }

    /// Swimming or flying, the sky walk pitches along its ray (`0x72f0ce`); on foot it stays level.
    #[test]
    fn a_swimming_or_flying_sky_walk_pitches_along_the_ray() {
        for (swimming, flying, want) in [
            (true, false, FRAC_PI_4),
            (false, true, FRAC_PI_4),
            (false, false, 0.0),
        ] {
            let mut app = world(Vec3::new(14.0, 0.0, 0.0), 0);
            {
                let mut player = app.world_mut().resource_mut::<Player>();
                player.swimming = swimming;
                player.flying = flying;
            }
            assert!(app
                .world_mut()
                .run_system_once(|mut auto: AutoMove| {
                    auto.walk_toward_sky(Vec3::new(1.0, 1.0, 0.0))
                })
                .unwrap());
            frame(&mut app, false);
            let pitch = app.world().resource::<Player>().mover_pitch;
            assert!((pitch - want).abs() < 1e-5, "{pitch}");
        }
    }

    /// The pitch steps at π rad/s toward the elevation, afloat and not on foot (`0x72f1f0`).
    #[test]
    fn a_swimming_or_flying_walk_pitches_toward_its_goal() {
        let delta = Vec3::new(0.0, 10.0, -10.0);
        for (swimming, flying, stepped) in [
            (true, false, true),
            (false, true, true),
            (false, false, false),
        ] {
            let mut player = Player {
                swimming,
                flying,
                ..Player::default()
            };
            let step = steer_pitch(&mut player, delta, 0.1);
            assert_eq!(step.pitching, stepped);
            let want = if stepped { PI * 0.1 } else { 0.0 };
            assert!((player.mover_pitch - want).abs() < 1e-5);
        }
        // Inside its last step it lands on the elevation, then it has arrived.
        let mut player = Player {
            flying: true,
            mover_pitch: FRAC_PI_4 - 0.01,
            ..Player::default()
        };
        assert!(steer_pitch(&mut player, delta, 0.1).pitching);
        assert!((player.mover_pitch - FRAC_PI_4).abs() < 1e-5);
        assert!(!steer_pitch(&mut player, delta, 0.1).pitching);
    }

    /// More than 22.5° off its aim forward waits; the not-moving test waits while it pitches.
    #[test]
    fn forward_waits_for_a_pitch_far_off_its_aim() {
        let at = Vec3::new(0.0, 40.0, -1.0);
        let mut app = world(at, 0);
        app.world_mut().resource_mut::<Player>().flying = true;
        assert_eq!(
            start(&mut app, ApproachVerb::Use, Subject::GameObject, at),
            Ok(())
        );
        frame(&mut app, false);
        assert!(!app.world().resource::<Player>().auto_forward);
        for _ in 0..10 {
            frame(&mut app, false);
        }
        assert_eq!(app.world().resource::<Approach>().stuck_frames(), Some(0));
        let forward = (0..30).any(|_| {
            frame(&mut app, false);
            app.world().resource::<Player>().auto_forward
        });
        assert!(forward, "held once within 22.5 degrees of its aim");
    }

    /// At the swimmer's rest line with a goal above it levels and does not pitch (`0x72f1f0`).
    #[test]
    fn a_swimmer_at_the_surface_stays_level_under_a_goal_above() {
        let h = 2.0;
        let mut player = Player {
            swimming: true,
            mover_pitch: 0.4,
            liquid_surface: Some(super::super::swim::swim_enter_depth(h)),
            collision_height: crate::entities::CollisionHeight(h),
            ..Player::default()
        };
        let step = steer_pitch(&mut player, Vec3::new(0.0, 8.0, -8.0), 0.1);
        assert_eq!(step, PitchStep::default());
        assert_eq!(player.mover_pitch, 0.0);
        // A goal below dives as anywhere else.
        assert!(steer_pitch(&mut player, Vec3::new(0.0, -8.0, -8.0), 0.1).pitching);
    }

    /// A mover that may fly takes off for a goal over 45° and `sqrt 2` yd off (`0x731d91`).
    #[test]
    fn a_steep_goal_takes_flight_only_for_a_mover_that_may_fly() {
        let steep = Vec3::new(0.0, 30.0, -10.0);
        let mut player = Player::default();
        assert!(!takes_flight(&player, steep), "no can-fly");
        player.modes.can_fly = true;
        assert!(takes_flight(&player, steep));
        assert!(takes_flight(&player, -steep), "down as well");
        assert!(
            !takes_flight(&player, Vec3::new(0.0, 5.0, -10.0)),
            "shallow"
        );
        assert!(
            !takes_flight(&player, Vec3::new(0.0, 1.0, 0.0)),
            "too close"
        );
        player.flying = true;
        assert!(!takes_flight(&player, steep), "already flying");
    }

    /// The take-off frame asks the controller's flight edge and keeps what forward held.
    #[test]
    fn a_can_fly_walk_asks_for_flight_at_a_high_ledge() {
        let at = Vec3::new(0.0, 30.0, -5.0);
        let mut app = world(at, 0);
        app.world_mut().resource_mut::<Player>().modes.can_fly = true;
        assert_eq!(
            start(&mut app, ApproachVerb::Use, Subject::GameObject, at),
            Ok(())
        );
        frame(&mut app, false);
        assert!(app.world().resource::<Player>().auto_take_off);
    }

    /// Nothing ends a walk or a follow for a goal straight overhead.
    #[test]
    fn a_goal_straight_overhead_ends_nothing() {
        let at = Vec3::new(0.0, 30.0, 0.0);
        let mut app = world(at, 0);
        assert_eq!(
            start(
                &mut app,
                ApproachVerb::Talk,
                Subject::Unit { dead: false },
                at
            ),
            Ok(())
        );
        app.world_mut().resource_mut::<Player>().pos.x = 0.1;
        frame(&mut app, true);
        assert!(
            app.world().resource::<Approach>().active() || {
                app.world().resource::<Approach>().arrived.is_some()
            }
        );
        app.world_mut().resource_mut::<Approach>().stop();
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(VENDOR, "Friend".into());
        app.world_mut().resource_mut::<Player>().pos = Vec3::ZERO;
        frame(&mut app, true);
        assert!(app.world().resource::<FollowState>().guid.is_some());
    }

    fn follow_world(at: Vec3) -> App {
        let mut app = world(at, 0);
        app.world_mut()
            .resource_mut::<FollowState>()
            .start(VENDOR, "Friend".into());
        app
    }

    /// The follow stops at a flat 3 yd at any speed, whose stop never scales.
    #[test]
    fn a_follow_stops_at_a_flat_three_yards() {
        let at = Vec3::new(0.0, 0.0, -12.0);
        let mut app = follow_world(at);
        for _ in 0..300 {
            frame(&mut app, true);
        }
        let player = app.world().resource::<Player>();
        let left = player.pos.distance(at);
        assert!(left <= 3.0 && left > 3.0 - 7.0 * DT - 1e-3, "{left}");
        assert!(!player.auto_forward);
        assert!(app.world().resource::<FollowState>().guid.is_some());
    }

    /// A followee 30 yd off or more ends the follow (`0x731949`), on the distance the stop reads.
    #[test]
    fn a_followee_past_the_leash_ends_the_follow() {
        let mut app = follow_world(Vec3::new(0.0, 0.0, -30.0));
        frame(&mut app, true);
        assert!(app.world().resource::<FollowState>().guid.is_none());

        let mut app = follow_world(Vec3::new(0.0, 0.0, -29.9));
        frame(&mut app, false);
        assert!(app.world().resource::<FollowState>().guid.is_some());

        // A flat reading: 40 yd up is under the leash for a walker, past it for a flyer.
        let at = Vec3::new(0.0, 40.0, -20.0);
        let mut app = follow_world(at);
        frame(&mut app, false);
        assert!(app.world().resource::<FollowState>().guid.is_some());
        let mut app = follow_world(at);
        app.world_mut().resource_mut::<Player>().flying = true;
        frame(&mut app, false);
        assert!(app.world().resource::<FollowState>().guid.is_none());
    }

    /// A followee that dies or boards a taxi ends the follow (`0x731857`, `0x73185d`).
    #[test]
    fn a_dead_or_flying_followee_ends_the_follow() {
        let flags = benilla_protocol::field::FIELD_UNIT_FLAGS;
        for pairs in [
            vec![(22, 0), (28, 100)],
            vec![
                (22, 100),
                (28, 100),
                (flags, super::super::UNIT_FLAG_TAXI_FLIGHT),
            ],
        ] {
            let at = Vec3::new(0.0, 0.0, -12.0);
            let mut app = follow_world(at);
            let vendor = app.world().resource::<GuidIndex>().0[&VENDOR];
            app.world_mut()
                .entity_mut(vendor)
                .insert(ObjectStore(ObjectFields::from_pairs(&pairs)));
            frame(&mut app, true);
            assert!(app.world().resource::<FollowState>().guid.is_none());
        }
    }

    /// The follow's first turn latches 800 or 1200 °/s as the walks' does (`0x731650`).
    #[test]
    fn a_follow_turns_at_the_auto_move_rates() {
        for (at, rate) in [
            (Vec3::new(10.0, 0.0, 0.0), 800.0_f32),
            (Vec3::new(1.0, 0.0, 10.0), 1200.0),
        ] {
            let mut app = follow_world(at);
            frame(&mut app, false);
            let turned = app.world().resource::<Player>().face_yaw.abs();
            let want = rate.to_radians() * DT;
            assert!((turned - want).abs() < 1e-3, "{turned} vs {want}");
        }
    }

    /// The follow's frame sets the camera's Move reading only with `autoInteract` off.
    #[test]
    fn a_follow_reads_as_move_only_with_click_to_move_off() {
        for (on, as_move) in [(true, false), (false, true)] {
            let mut app = follow_world(Vec3::new(10.0, 0.0, 0.0));
            app.world_mut().resource_mut::<Approach>().enabled = on;
            frame(&mut app, false);
            let player = app.world().resource::<Player>();
            assert!(player.auto_turning);
            assert_eq!(player.auto_as_move, as_move);
        }
    }
}
