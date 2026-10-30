//! Trajectory missiles: a GO with `castFlags & 0x20000` flies its missile on a ballistic arc. At
//! launch `0x700880` solves the speed that carries the packet's pitch to the impact under the
//! `SpellMissile.dbc` gravity (`0x76c070`), else times a fallback speed (`0x76bf80`), and scales
//! the flight to land at the packet's delay; `0x7015d0` flies `launch + v·t − g·t²/2`. A missile
//! the server says hit (`SMSG_SET_PROJECTILE_POSITION`, `0x6ff2c0`) lands where it starts to fly
//! away from that point. One we launched, from our own unit or our pet, also tests its path
//! against the units it may hit (`0x6fcb60`) and reports a hit (`CMSG_UPDATE_PROJECTILE_POSITION`,
//! `0x703640`).
//!
//! Bevy's Y is the reference's Z: the arc's math reads only the horizontal length and the rise.
//! The cast that flies the aim is `crate::vehicle`'s; not built: its preview (`0x6fda20`).

use std::f32::consts::FRAC_PI_2;

use benilla_formats::{MissileCollision, SpellMissileRec, DEFAULT_MISSILE_GRAVITY};
use bevy::prelude::*;

/// The most units one missile tests (`0x6fcb60`).
const MAX_UNITS: usize = 15;

/// `SpellMissile` flags: the player and creature collision modes (`0x6fc4a0`).
const PLAYERS: u32 = 0xC;
const HOSTILE_PLAYERS: u32 = 0x4;
const CREATURES: u32 = 0x30;
const HOSTILE_CREATURES: u32 = 0x10;

/// `CreatureTypeFlags` bit for a creature that missiles hit.
const TYPE_FLAG_COLLIDES: u32 = 0x8_0000;

fn horizontal(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// The pitch folded into ±π/2: a launch past straight up flies back over (`0x700880`).
fn fold(pitch: f32) -> f32 {
    if pitch > FRAC_PI_2 {
        pitch - (pitch - FRAC_PI_2) * 2.0
    } else if pitch < -FRAC_PI_2 {
        2.0 * (-FRAC_PI_2 - pitch) + pitch
    } else {
        pitch
    }
}

/// `0x76c070`'s three answers.
#[derive(Debug, PartialEq)]
enum Solve {
    /// Straight up or down, no horizontal reach, or no gravity: time a speed instead.
    Degenerate,
    /// The pitch is too flat to reach.
    Flat,
    Arc {
        time: f32,
        speed: f32,
        velocity: Vec3,
    },
}

/// `0x76c070`: the launch speed that carries `delta` at `pitch` under `gravity`.
fn solve(pitch: f32, delta: Vec3, gravity: f32) -> Solve {
    let (s, c) = pitch.sin_cos();
    let h = horizontal(delta);
    let len = h.length();
    if c.abs() < 1.0e-4 || len < 1.0e-4 || gravity.abs() < 1.0e-4 {
        return Solve::Degenerate;
    }
    let half = (len / c * s - delta.y) / gravity;
    let squared = half + half;
    if squared <= 0.01 {
        return Solve::Flat;
    }
    let time = squared.sqrt();
    let speed = len / time / c;
    let dir = h / len;
    Solve::Arc {
        time,
        speed,
        velocity: Vec3::new(dir.x * speed * c, s * speed, dir.y * speed * c),
    }
}

/// `0x76bf80`: the time `speed` at `pitch` takes to cover `delta`; below 0 for never.
fn travel_time(pitch: f32, speed: f32, delta: Vec3, gravity: f32) -> f32 {
    let gravitates = gravity.abs() > 1.0e-4;
    let reach = pitch.cos() * speed;
    if reach > 1.0e-4 {
        return horizontal(delta).length() / reach;
    }
    if speed <= 1.0e-4 {
        if !gravitates {
            return -1.0;
        }
    } else if !gravitates {
        return delta.y.abs() / speed;
    }
    let vertical = speed * pitch.sin();
    let disc = vertical * vertical - 2.0 * delta.y * gravity;
    if disc < 0.0 {
        return -1.0;
    }
    let root = disc.sqrt();
    let scale = -1.0 / gravity;
    let first = (root - vertical) * scale;
    let second = scale * (-vertical - root);
    if first >= 0.0 && (second < 0.0 || first < second) {
        first
    } else {
        second
    }
}

/// A missile's arc from launch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Arc {
    launch: Vec3,
    velocity: Vec3,
    gravity: f32,
    /// The arc's own time over the clock's, so it lands at the packet's delay, in `[0, 2]`.
    time_scale: f32,
    /// Seconds flown.
    flown: f32,
    /// The pitch and speed the arc left at, for its collision path.
    pub(super) pitch: f32,
    pub(super) speed: f32,
}

impl Arc {
    /// Where the arc is after `dt` more seconds (`0x7015d0`).
    pub(super) fn advance(&mut self, dt: f32) -> Vec3 {
        self.flown += dt;
        let t = self.time_scale * self.flown;
        self.launch + self.velocity * t - Vec3::Y * (self.gravity * t * t * 0.5)
    }

    /// The flight's direction now.
    pub(super) fn heading(&self) -> Vec3 {
        let t = self.time_scale * self.flown;
        self.velocity - Vec3::Y * (self.gravity * t)
    }
}

/// `0x700880`: the arc from `launch` to `impact` at `pitch`, landing in `remaining` seconds; `None`
/// flies the missile straight to land on time. `speed` is the cast's own speed, 0 for a cast we
/// did not aim.
pub(super) fn launch(
    pitch: f32,
    speed: f32,
    launch: Vec3,
    impact: Vec3,
    rec: Option<&SpellMissileRec>,
    remaining: f32,
) -> Option<Arc> {
    let delta = impact - launch;
    let gravity = rec.map_or(DEFAULT_MISSILE_GRAVITY, |r| r.gravity);
    let mut pitch = fold(pitch);
    let mut solved = solve(pitch, delta, gravity);
    if solved == Solve::Flat {
        // One nudge toward the fall, then give up.
        let nudge = if gravity < 0.0 { -0.01 } else { 0.01 };
        pitch = fold(pitch + nudge);
        solved = solve(pitch, delta, gravity);
    }
    let (travel, speed, velocity) = match solved {
        Solve::Flat => return None,
        Solve::Arc {
            time,
            speed,
            velocity,
        } => (time, speed, velocity),
        Solve::Degenerate => {
            let rec = rec?;
            let (travel, speed) = [speed, rec.speed_min, rec.speed_max]
                .into_iter()
                .map(|s| (travel_time(pitch, s, delta, gravity), s))
                .find(|&(t, _)| t >= 1.0e-4)?;
            (travel, speed, delta / travel)
        }
    };
    Some(Arc {
        launch,
        velocity,
        gravity,
        time_scale: (travel / remaining.max(1.0e-4)).clamp(0.0, 2.0),
        flown: 0.0,
        pitch,
        speed,
    })
}

/// A server hit lands the missile at its last point once it flies away from the hit
/// (`0x7015d0`).
pub(super) fn passed(previous: Vec3, now: Vec3, hit: Vec3) -> bool {
    previous.distance_squared(hit) < now.distance_squared(hit)
}

/// The horizontal path a collider ranks units against (`0x6fcb60`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Path {
    start: Vec2,
    end: Vec2,
    pub(super) dir: Vec2,
    perp: Vec2,
    length: f32,
    /// The missile's horizontal speed.
    speed: f32,
    duration: f32,
}

impl Path {
    pub(super) fn new(start: Vec3, end: Vec3, pitch: f32, speed: f32) -> Self {
        let (mut s, e) = (horizontal(start), horizontal(end));
        let d = e - s;
        let (dir, length, speed) = if d.x.abs() >= 0.001 || d.y.abs() >= 0.001 {
            let length = d.length();
            (d / length, length, (pitch.cos() * speed).max(1.0e-4))
        } else {
            // Straight up: a short path along +X that takes the whole flight.
            let time = if speed <= 1.0e-4 {
                1.0
            } else {
                (end - start).length() / speed
            };
            s.x -= 0.01;
            (Vec2::X, 1.0, 0.01 / time)
        };
        Path {
            start: s,
            end: e,
            dir,
            perp: Vec2::new(-dir.y, dir.x),
            length,
            speed,
            duration: length / speed,
        }
    }

    /// `0x6fc4a0`'s rank of a unit whose sphere sits at `at` with `radius`, moving at up to
    /// `unit_speed`: how well it can reach the path in the flight; `None` if it cannot.
    pub(super) fn rank(&self, at: Vec2, radius: f32, unit_speed: f32) -> Option<f32> {
        if radius < 1.0e-4 {
            return None;
        }
        let to_end = self.end - at;
        let rank = if self.speed <= unit_speed {
            -1.0
        } else {
            let distance = self.perp.dot(to_end).abs();
            let along = (self.perp * distance + at - self.start).dot(self.dir);
            if along < 1.0e-4 {
                (radius * radius - (self.start - at).length_squared()) * 10_000.0
            } else if self.length <= along {
                -1.0
            } else {
                let reach = along / self.length * self.duration * unit_speed + radius;
                (reach * reach - distance * distance) / along
            }
        };
        let reach = self.duration * unit_speed + radius;
        let end_rank = (reach * reach - to_end.length_squared()) / self.length;
        if end_rank > 0.0 && end_rank > rank {
            Some(end_rank)
        } else {
            (rank > 0.0).then_some(rank)
        }
    }
}

/// Whether a missile with `flags` may hit a unit (`0x6fc4a0`): a player under the player mode, a
/// creature that takes missile hits under the creature mode, each mode hostile-only, friendly-only
/// or both.
pub(super) fn collides(flags: u32, player: bool, type_flags: u32, attackable: bool) -> bool {
    let (mode, hostile) = if player {
        (flags & PLAYERS, HOSTILE_PLAYERS)
    } else {
        if type_flags & TYPE_FLAG_COLLIDES == 0 {
            return false;
        }
        (flags & CREATURES, HOSTILE_CREATURES)
    };
    let both = if player { PLAYERS } else { CREATURES };
    mode != 0 && (mode == both || (mode & hostile != 0) == attackable)
}

/// A unit's missile hit sphere (`0x6fbff0`): from its base, pushed back along the flight and
/// raised by its model's columns at its scale, its radius the model's or half its height, then
/// lifted by that radius.
pub(super) fn hit_sphere(
    base: Vec3,
    scale: f32,
    model: Option<MissileCollision>,
    dir: Vec2,
) -> (Vec3, f32) {
    let (mut c, r) = match model {
        Some(m) => {
            let r = if m.radius > 0.0 {
                scale * m.radius
            } else {
                m.collision_height * 0.5
            };
            let push = m.push * scale;
            let c = base - Vec3::new(dir.x, 0.0, dir.y) * push + Vec3::Y * (scale * m.raise);
            (c, r)
        }
        None => (base, 1.0),
    };
    c.y += r;
    (c, r)
}

/// `0x6fc360`: the share of the segment from `start` to `end` at which it first meets a sphere,
/// in the spheres' order.
pub(super) fn segment_hit(start: Vec3, end: Vec3, spheres: &[(Vec3, f32)]) -> Option<f32> {
    let delta = end - start;
    let length = delta.length();
    if length < 1.0e-5 {
        return None;
    }
    let dir = delta / length;
    spheres.iter().find_map(|&(c, r)| {
        let to = c - start;
        let projection = to.dot(dir);
        (projection >= 0.0
            && to.length_squared() - projection * projection <= r * r
            && length >= projection)
            .then_some(projection / length)
    })
}

/// The best-ranked `units`, at most [`MAX_UNITS`] (`0x6fcb60`).
pub(super) fn pick(mut ranked: Vec<(Entity, f32)>) -> Vec<Entity> {
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(MAX_UNITS);
    ranked.into_iter().map(|(e, _)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> SpellMissileRec {
        SpellMissileRec {
            gravity: 20.0,
            speed_min: 30.0,
            speed_max: 60.0,
            ..Default::default()
        }
    }

    /// The solved arc lands on the impact at its own time.
    #[test]
    fn the_arc_lands_on_the_impact() {
        let impact = Vec3::new(40.0, -3.0, 10.0);
        let mut arc = launch(0.6, 0.0, Vec3::ZERO, impact, Some(&rec()), 2.0).unwrap();
        let Solve::Arc { time, .. } = solve(0.6, impact, 20.0) else {
            panic!("an arc");
        };
        assert!((arc.time_scale - time / 2.0).abs() < 1.0e-5);
        let at = arc.advance(2.0);
        assert!(at.distance(impact) < 1.0e-3, "{at}");
    }

    /// A pitch past straight up folds back, and a missile with no row falls at the default.
    #[test]
    fn the_pitch_folds_and_gravity_defaults() {
        assert!((fold(FRAC_PI_2 + 0.25) - (FRAC_PI_2 - 0.25)).abs() < 1.0e-6);
        assert!((fold(-FRAC_PI_2 - 0.25) - (-FRAC_PI_2 + 0.25)).abs() < 1.0e-6);
        let arc = launch(0.6, 0.0, Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), None, 1.0).unwrap();
        assert_eq!(arc.gravity, DEFAULT_MISSILE_GRAVITY);
    }

    /// Straight up has no arc to solve: the row's slowest speed that arrives times it.
    #[test]
    fn straight_up_times_the_row_s_speed() {
        let up = Vec3::new(0.0, 10.0, 0.0);
        let arc = launch(FRAC_PI_2, 0.0, Vec3::ZERO, up, Some(&rec()), 1.0).unwrap();
        assert_eq!(arc.speed, 30.0);
        assert!(launch(FRAC_PI_2, 0.0, Vec3::ZERO, up, None, 1.0).is_none());
    }

    /// A pitch too flat to reach flies straight.
    #[test]
    fn a_flat_pitch_flies_straight() {
        let far = Vec3::new(10.0, 50.0, 0.0);
        assert!(launch(0.0, 0.0, Vec3::ZERO, far, Some(&rec()), 1.0).is_none());
    }

    #[test]
    fn a_server_hit_lands_where_the_missile_turns_away() {
        let hit = Vec3::new(10.0, 0.0, 0.0);
        assert!(!passed(Vec3::ZERO, Vec3::new(9.0, 0.0, 0.0), hit));
        assert!(passed(
            Vec3::new(9.5, 0.0, 0.0),
            Vec3::new(11.0, 0.0, 0.0),
            hit
        ));
    }

    #[test]
    fn the_modes_pick_players_and_creatures() {
        assert!(collides(0x4, true, 0, true));
        assert!(!collides(0x4, true, 0, false));
        assert!(collides(0x8, true, 0, false));
        assert!(collides(0xC, true, 0, false));
        assert!(!collides(0x30, true, 0, true), "no player mode");
        assert!(
            !collides(0x30, false, 0, true),
            "a creature missiles pass through"
        );
        assert!(collides(0x10, false, TYPE_FLAG_COLLIDES, true));
        assert!(!collides(0x20, false, TYPE_FLAG_COLLIDES, true));
    }

    /// The sphere rises by its radius, and a model with none takes half its height.
    #[test]
    fn the_hit_sphere_sits_on_the_unit() {
        let m = MissileCollision {
            radius: 0.0,
            push: 1.0,
            raise: 0.5,
            collision_height: 4.0,
        };
        let (c, r) = hit_sphere(Vec3::ZERO, 2.0, Some(m), Vec2::X);
        assert_eq!(r, 2.0);
        assert_eq!(c, Vec3::new(-2.0, 3.0, 0.0));
        assert_eq!(hit_sphere(Vec3::ZERO, 1.0, None, Vec2::X), (Vec3::Y, 1.0));
    }

    #[test]
    fn a_segment_meets_the_first_sphere_it_crosses() {
        let spheres = [(Vec3::new(5.0, 0.5, 0.0), 1.0)];
        let t = segment_hit(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), &spheres).unwrap();
        assert!((t - 0.5).abs() < 1.0e-6);
        assert!(segment_hit(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0), &spheres).is_none());
        assert!(segment_hit(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0), &spheres).is_none());
    }

    /// A unit on the path ranks; one too far to reach it in the flight does not.
    #[test]
    fn a_unit_ranks_by_its_reach() {
        let path = Path::new(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), 0.0, 50.0);
        assert!(path.rank(Vec2::new(50.0, 0.0), 1.0, 7.0).is_some());
        assert!(path.rank(Vec2::new(50.0, 500.0), 1.0, 7.0).is_none());
        assert!(
            path.rank(Vec2::new(50.0, 0.0), 0.0, 7.0).is_none(),
            "no sphere"
        );
    }
}
