//! What a missed missile does when it gets there, the reference's per-outcome flags (`0x6ffbb0`):
//! it flies past, sticks in the shield, bursts, glances off or comes back.

use bevy::prelude::*;

/// A length or a tilt this close to its limit counts as reached (`0x6ff6d0`).
const NEAR: f32 = 2.384_185_8e-7;

/// The most a flight that passes its target strays, 3 degrees (`0x6ff8ec`).
const WIDE_STRAY: f32 = 0.052_359_88;

/// The seconds of flight past the target, and a deflection's whole flight (`0x6ff6d0`, `0x702fc0`).
pub(super) const PAST_TARGET_SECS: f32 = 0.5;

/// The attachment a blocked missile aims for, the shield (`0x7022d0`).
pub(super) const SHIELD_ATTACHMENT: u16 = 0;

/// The stiffness a target's transport motion is chased with (`0x6ffd70`).
const DECK_STIFFNESS: f32 = 5.0;

/// A miss's route to the end of its flight, the flags the launch sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MissKind {
    /// Miss, dodge, evade (`0x8`): flies past and fades.
    Wide,
    /// Block (`0x10`): sticks in the victim for 5 s.
    Stuck,
    /// Resist or immune (`0x20`): bursts where it lands.
    Resisted,
    /// Parry and the rest (`0x40`): glances off at half speed.
    Glance,
    /// Reflect (`0x80`): flies back at its caster.
    Reflect,
}

/// The route for a wire `SpellMissInfo`, and the code its word and log then read (`0x6ffbb0`). A
/// spell that cannot be reflected back glances off instead and keeps its word (`0x6ffc6d`).
pub(super) fn classify(code: u8, reflect_glances: bool) -> (MissKind, u8) {
    match code {
        1 | 3 | 6 => (MissKind::Wide, code),
        2 | 7 | 8 => (MissKind::Resisted, code),
        5 => (MissKind::Stuck, code),
        11 if reflect_glances => (MissKind::Glance, code),
        11 => (MissKind::Reflect, code),
        _ => (MissKind::Glance, 9),
    }
}

/// Where a flight that passes its target aims, fixed at launch: half a second beyond it, strayed
/// up to 3 degrees with `stray` in -1 to 1 (`0x6ff6d0`).
pub(super) fn wide_aim(from: Vec3, at: Vec3, speed: f32, stray: f32) -> Vec3 {
    let delta = at - from;
    let length = delta.length();
    if length < NEAR || (delta.y.abs() / length - 1.0).abs() < NEAR {
        return at;
    }
    let beyond = delta / length * (length + speed * PAST_TARGET_SECS);
    let axis = (Vec3::Y * delta.length_squared() - delta * delta.y).normalize();
    let angle = (stray * std::f32::consts::FRAC_PI_2).sin() * WIDE_STRAY;
    from + Quat::from_axis_angle(axis, angle) * beyond
}

/// The way a glancing missile leaves, from `back` toward where it came from, turned by `spin` and
/// `tilt` in -1 to 1 quarter turns; `None` straight up or down, or at no length (`0x702fc0`).
pub(super) fn glance_dir(back: Vec3, spin: f32, tilt: f32) -> Option<Vec3> {
    let length = back.length();
    if length == 0.0 || length == back.y.abs() {
        return None;
    }
    let dir = back / length;
    let quarter = std::f32::consts::FRAC_PI_2;
    let side = Vec3::new(dir.z, 0.0, -dir.x).normalize();
    let side = Quat::from_axis_angle(dir, spin * quarter) * side;
    Some(Quat::from_axis_angle(side, tilt * quarter) * dir)
}

/// A relaunched flight's speed: what is left of the time since the cast, at most double (`0x7022d0`).
pub(super) fn relaunch_speed(speed: f32, distance: f32, since_cast: f32) -> f32 {
    let limit = speed + speed;
    let left = distance / speed - since_cast;
    if left > 0.0 {
        (distance / left).min(limit)
    } else {
        limit
    }
}

/// The alpha `distance` from the end of a fading flight of length `range` (`0x6ffcd0`).
pub(super) fn fade_alpha(distance: f32, range: f32) -> f32 {
    let t = (distance / range).clamp(0.0, 1.0);
    t * ((3.0 - (t + t)) * t)
}

/// A missile riding its target's transport: the offset that carries it with the deck (`0x6ffd70`).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DeckChase {
    last: Option<Vec3>,
    offset: Vec3,
    velocity: Vec3,
}

impl DeckChase {
    /// This step's carry for a deck now at `deck`; the first sight of a deck only records it.
    pub(super) fn follow(&mut self, deck: Vec3, dt: f32) -> Vec3 {
        let Some(last) = self.last.replace(deck) else {
            *self = Self {
                last: Some(deck),
                ..Self::default()
            };
            return Vec3::ZERO;
        };
        let target = deck - last;
        let x = DECK_STIFFNESS * dt;
        let damping = 1.0 / (x + x * x * x * 0.235 + x * x * 0.48 + 1.0);
        let d = self.offset - target;
        let v = (d * DECK_STIFFNESS + self.velocity) * dt;
        self.offset = (d + v) * damping + target;
        self.velocity = (self.velocity - v * DECK_STIFFNESS) * damping;
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_miss_code_takes_the_reference_route() {
        for (code, route, word) in [
            (1, MissKind::Wide, 1),
            (2, MissKind::Resisted, 2),
            (3, MissKind::Wide, 3),
            (4, MissKind::Glance, 9),
            (5, MissKind::Stuck, 5),
            (6, MissKind::Wide, 6),
            (7, MissKind::Resisted, 7),
            (8, MissKind::Resisted, 8),
            (9, MissKind::Glance, 9),
            (10, MissKind::Glance, 9),
            (11, MissKind::Reflect, 11),
        ] {
            assert_eq!(classify(code, false), (route, word), "code {code}");
        }
        assert_eq!(classify(11, true), (MissKind::Glance, 11));
    }

    #[test]
    fn a_wide_flight_runs_half_a_second_past_and_barely_strays() {
        let at = Vec3::new(20.0, 0.0, 0.0);
        let aim = wide_aim(Vec3::ZERO, at, 10.0, 0.0);
        assert!(aim.distance(Vec3::new(25.0, 0.0, 0.0)) < 1e-4);
        let strayed = wide_aim(Vec3::ZERO, at, 10.0, 1.0);
        assert!((strayed.length() - 25.0).abs() < 1e-3);
        let angle = strayed.angle_between(at);
        assert!((angle - WIDE_STRAY).abs() < 1e-4, "{angle}");
    }

    #[test]
    fn a_vertical_or_empty_wide_flight_keeps_its_aim() {
        let up = Vec3::new(0.0, 9.0, 0.0);
        assert_eq!(wide_aim(Vec3::ZERO, up, 10.0, 1.0), up);
        assert_eq!(wide_aim(up, up, 10.0, 1.0), up);
    }

    #[test]
    fn a_glance_leaves_back_within_its_cone_and_never_from_a_vertical() {
        let back = Vec3::new(-3.0, 0.0, 0.0);
        let dir = glance_dir(back, 0.0, 0.0).unwrap();
        assert!(dir.distance(Vec3::NEG_X) < 1e-5);
        let tilted = glance_dir(back, 0.3, 1.0).unwrap();
        assert!((tilted.length() - 1.0).abs() < 1e-5);
        assert!((tilted.angle_between(Vec3::NEG_X) - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert_eq!(glance_dir(Vec3::Y, 0.5, 0.5), None);
        assert_eq!(glance_dir(Vec3::ZERO, 0.5, 0.5), None);
    }

    #[test]
    fn a_relaunch_makes_up_time_up_to_double_speed() {
        assert_eq!(relaunch_speed(20.0, 40.0, 0.0), 20.0);
        assert_eq!(relaunch_speed(20.0, 40.0, 1.0), 40.0);
        assert_eq!(relaunch_speed(20.0, 40.0, 1.5), 40.0);
        assert_eq!(relaunch_speed(20.0, 40.0, 5.0), 40.0);
        assert!((relaunch_speed(20.0, 40.0, 0.4) - 25.0).abs() < 1e-4);
    }

    #[test]
    fn the_fade_ends_at_nothing_and_starts_whole() {
        assert_eq!(fade_alpha(0.0, 5.0), 0.0);
        assert_eq!(fade_alpha(5.0, 5.0), 1.0);
        assert_eq!(fade_alpha(2.5, 5.0), 0.5);
    }

    #[test]
    fn the_chase_records_a_new_deck_then_settles_on_its_motion() {
        let mut chase = DeckChase::default();
        assert_eq!(chase.follow(Vec3::new(4.0, 0.0, 0.0), 0.1), Vec3::ZERO);
        let mut x = 4.0;
        let mut carry = Vec3::ZERO;
        for _ in 0..400 {
            x += 0.5;
            carry = chase.follow(Vec3::new(x, 0.0, 0.0), 0.05);
        }
        assert!((carry.x - 0.5).abs() < 1e-3, "{carry}");
    }
}
