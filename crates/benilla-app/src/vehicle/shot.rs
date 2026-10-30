//! A trajectory cast: a spell whose `SpellMissile` row has flag `0x1` leaves with castFlags 2
//! (`0x80b39d`), its caster the active mover. The client flies the missile itself (`0x6fcd60`):
//! from the mover's pitch and launch power, in 0.1 s steps against the world, and sends the launch
//! as SOURCE, the landing as DEST, the pitch and the speed, then the mover's stop report
//! (`0x80b406`-`0x80b46c`).
//!
//! Not built: the aim preview with its arc and impact models (`0x6fda20`); an instant cast with a
//! cast animation, which the reference sends at the animation's release (`0x809f80`, `0x80de60`),
//! and a cast with a cast time, whose final aim follows as `CMSG_UPDATE_MISSILE_TRAJECTORY`
//! (`0x6fd6b0`): both are aimed and sent at the press here; the spell visual's launch offset
//! (`0x720bf0`); a sloped mover's tilt of the launch; and the sequence mark the reference writes
//! into the floats' low bits (`0x7fe520`), which AzerothCore does not read. The launch lifts its
//! wall test from the mover's base by half its collision height, where the reference lifts it to
//! the root's model collision-box centre.

use bevy::prelude::*;

use benilla_formats::{SpellMissileRec, VehicleRec};
use benilla_protocol::messages::TrajectoryShot;

use crate::net::ClientCommand;

/// A pet-bar cast whose missile flies an aim, handed from the cast send to be aimed and sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrajectoryCast {
    pub(crate) caster: u64,
    pub(crate) spell_id: u32,
}

/// The trajectory casts handed this frame.
#[derive(Resource, Default)]
pub(crate) struct TrajectoryCasts(pub(crate) Vec<TrajectoryCast>);

/// The step the flight is flown in, seconds.
const STEP: f32 = 0.1;
/// The most steps one flight takes, 20 s.
const MAX_STEPS: i32 = 200;

/// What an aim starts from, WoW coordinates (Z up).
pub(crate) struct AimFrom<'a> {
    pub(crate) missile: &'a SpellMissileRec,
    pub(crate) vehicle: Option<&'a VehicleRec>,
    /// The mover's pitch.
    pub(crate) pitch: f32,
    /// `VehicleAimGetNormPower`, 0..1.
    pub(crate) power: f32,
    pub(crate) launch: Vec3,
    pub(crate) facing: f32,
    /// Where the launch's wall test starts.
    pub(crate) body: Vec3,
}

/// A flown aim, WoW coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aimed {
    pub(crate) launch: Vec3,
    pub(crate) impact: Vec3,
    pub(crate) pitch: f32,
    pub(crate) speed: f32,
    /// Seconds to the impact.
    pub(crate) time: f32,
}

/// `0x6fcd60`'s cast arm: the pitch from a pitching record (`0x10`, clamped under `0x40`) else
/// the row's middle, the speed from a powered record's power (`0x800`) else the row's middle, each
/// spread by the row and a further ±0.01 (the launch by ±0.1 yd across, ±0.01 up); then the
/// flight in 0.1 s steps under the row's gravity, the first to meet the world (`hit`: a segment's
/// point and its fraction) landing. A launch behind a wall lands on itself.
pub(crate) fn fly(
    from: &AimFrom,
    mut random: impl FnMut() -> f32,
    mut hit: impl FnMut(Vec3, Vec3) -> Option<(Vec3, f32)>,
) -> Aimed {
    let m = from.missile;
    let mut spread = |[lo, hi]: [f32; 2]| lo + (hi - lo) * random();
    let mut pitch = match from.vehicle.filter(|v| v.flags & 0x10 != 0) {
        Some(v) if v.flags & 0x40 != 0 => from.pitch.clamp(v.pitch_min, v.pitch_max),
        Some(_) => from.pitch,
        None => (m.pitch_max + m.pitch_min) * 0.5,
    };
    let mut speed = match from.vehicle.filter(|v| v.flags & 0x800 != 0) {
        Some(_) => from.power * (m.speed_max - m.speed_min) + m.speed_min,
        None => (m.speed_max + m.speed_min) * 0.5,
    };
    pitch += spread(m.pitch_spread);
    speed += spread(m.speed_spread);
    let yaw = from.facing + spread(m.facing_spread);
    pitch += spread([-0.01, 0.01]);
    speed += spread([-0.01, 0.01]);
    let launch = from.launch
        + Vec3::new(
            spread([-0.1, 0.1]),
            spread([-0.1, 0.1]),
            spread([-0.01, 0.01]),
        );
    let aimed = move |impact, time| Aimed {
        launch,
        impact,
        pitch,
        speed,
        time,
    };
    if hit(from.body, launch).is_some() {
        return aimed(launch, 0.0);
    }
    let gravity = m.gravity * STEP * STEP;
    let reach = speed * STEP;
    let mut velocity = Vec3::new(
        yaw.cos() * pitch.cos() * reach,
        yaw.sin() * pitch.cos() * reach,
        pitch.sin() * reach - gravity * 0.5,
    );
    let max = if m.max_duration <= 1.0e-4 {
        MAX_STEPS
    } else {
        ((m.max_duration * 10.0) as i32).min(MAX_STEPS)
    };
    let mut left = max;
    let mut at = launch;
    loop {
        let to = at + velocity;
        if let Some((point, t)) = hit(at, to) {
            return aimed(point, ((max - left) as f32 + t) * STEP);
        }
        left -= 1;
        if left <= 0 {
            return aimed(to, max as f32 * STEP);
        }
        velocity.z -= gravity;
        at = to;
    }
}

/// Aim and send each handed trajectory cast, from the mover we drive.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn send_trajectory_casts(
    mut casts: ResMut<TrajectoryCasts>,
    net: Option<Res<crate::net::NetCommands>>,
    mover: Query<
        (
            Entity,
            &crate::net::Guid,
            Option<&super::Vehicle>,
            Option<&super::aim::MoveFlags2>,
            Option<&crate::entities::CollisionHeight>,
        ),
        With<crate::net::ActiveMover>,
    >,
    player: Option<Res<crate::player::Player>>,
    tables: Res<super::VehicleTables>,
    aim: Res<super::aim::VehicleAim>,
    spells: Option<Res<crate::ui_action::Spells>>,
    visuals: Option<Res<crate::creature_anim::SpellVisuals>>,
    units: crate::entities::AttachPosQuery,
    joints: Query<&GlobalTransform>,
    world: benilla_world::collision::WorldCollision,
) {
    if casts.0.is_empty() {
        return;
    }
    let casts = std::mem::take(&mut casts.0);
    let (Some(net), Some(player)) = (net, player) else {
        return;
    };
    for cast in casts {
        let Ok((entity, guid, vehicle, flags2, height)) = mover.single() else {
            debug!("vehicle: trajectory cast {} with no mover", cast.spell_id);
            continue;
        };
        // `0x80b3a6`: a caster that is not the active mover fails unreported.
        if guid.0 != cast.caster {
            debug!(
                "vehicle: trajectory cast {} from {:#x}, not the mover",
                cast.spell_id, cast.caster
            );
            continue;
        }
        let Some(missile) = spells
            .as_deref()
            .and_then(|s| s.catalog.get(cast.spell_id))
            .zip(visuals.as_deref())
            .and_then(|(d, v)| v.0.missile(d.missile))
        else {
            continue;
        };
        let Some(launch) = crate::entities::aim_launch_pos(entity, &units, &joints) else {
            continue;
        };
        let lift = height.map_or(crate::player::DEFAULT_COLLISION_HEIGHT, |h| h.0) * 0.5;
        let wow = |v: Vec3| Vec3::from(benilla_assets::coords::bevy_to_wow(v));
        let from = AimFrom {
            missile,
            vehicle: vehicle.and_then(|v| tables.vehicle(v.id)),
            pitch: aim.pitch,
            power: aim.power,
            launch: wow(launch),
            facing: player.facing(),
            body: wow(player.pos + Vec3::Y * lift),
        };
        let shot = fly(&from, rand::random::<f32>, |a, b| {
            let (a, b) = (
                benilla_assets::coords::wow_to_bevy(a.to_array()),
                benilla_assets::coords::wow_to_bevy(b.to_array()),
            );
            let (dir, len) = Dir3::new_and_length(b - a).ok()?;
            let d = world.ray_body(a, dir, len)?.distance;
            Some((wow(a + dir * d), d / len))
        });
        debug!(
            "vehicle: trajectory cast {} pitch {:.3} speed {:.2} lands in {:.2} s at {:?}",
            cast.spell_id, shot.pitch, shot.speed, shot.time, shot.impact
        );
        let _ = net.0.send(ClientCommand::PetCastTrajectory {
            pet_guid: cast.caster,
            spell_id: cast.spell_id,
            shot: TrajectoryShot {
                source: shot.launch.to_array(),
                dest: shot.impact.to_array(),
                pitch: shot.pitch,
                speed: shot.speed,
            },
            flags: player.move_flags(),
            flags2: flags2.map_or(0, |f| f.0),
            pos: benilla_assets::coords::bevy_to_wow(player.pos),
            orientation: player.facing().rem_euclid(std::f32::consts::TAU),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missile() -> SpellMissileRec {
        SpellMissileRec {
            flags: 0x1,
            pitch_min: 0.4,
            pitch_max: 0.6,
            speed_min: 10.0,
            speed_max: 50.0,
            gravity: 20.0,
            ..Default::default()
        }
    }

    fn from<'a>(m: &'a SpellMissileRec, v: Option<&'a VehicleRec>) -> AimFrom<'a> {
        AimFrom {
            missile: m,
            vehicle: v,
            pitch: 1.2,
            power: 0.25,
            launch: Vec3::new(0.0, 0.0, 2.0),
            facing: 0.0,
            body: Vec3::ZERO,
        }
    }

    /// The flat ground at z = 0 as a segment test.
    fn ground(a: Vec3, b: Vec3) -> Option<(Vec3, f32)> {
        (b.z <= 0.0 && a.z > 0.0).then(|| {
            let t = a.z / (a.z - b.z);
            (a.lerp(b, t), t)
        })
    }

    /// Without a pitching or powered record the aim takes the row's middles; with the spread at
    /// its midpoint nothing moves them.
    #[test]
    fn a_plain_caster_flies_the_row_s_middles() {
        let m = missile();
        let shot = fly(&from(&m, None), || 0.5, ground);
        assert!((shot.pitch - 0.5).abs() < 1.0e-6);
        assert!((shot.speed - 30.0).abs() < 1.0e-5);
        assert_eq!(shot.launch, Vec3::new(0.0, 0.0, 2.0));
    }

    /// A pitching, clamping, powered record flies the mover's clamped pitch and its power's speed.
    #[test]
    fn a_vehicle_flies_its_pitch_and_power() {
        let m = missile();
        let v = VehicleRec {
            flags: 0x10 | 0x40 | 0x800,
            pitch_min: -0.5,
            pitch_max: 1.0,
            ..Default::default()
        };
        let shot = fly(&from(&m, Some(&v)), || 0.5, ground);
        assert!((shot.pitch - 1.0).abs() < 1.0e-6, "{}", shot.pitch);
        assert!((shot.speed - 20.0).abs() < 1.0e-5, "{}", shot.speed);
    }

    /// The stepped flight lands near the ballistic range on flat ground, along the facing.
    #[test]
    fn the_flight_lands_where_the_arc_meets_the_ground() {
        let m = missile();
        let shot = fly(&from(&m, None), || 0.5, ground);
        let (s, c) = 0.5f32.sin_cos();
        let (vh, vz) = (30.0 * c, 30.0 * s);
        let t = (vz + (vz * vz + 2.0 * 20.0 * 2.0).sqrt()) / 20.0;
        assert!(
            (shot.impact.x - vh * t).abs() < 0.5,
            "{:?} vs {}",
            shot.impact,
            vh * t
        );
        assert!(shot.impact.y.abs() < 1.0e-4 && shot.impact.z.abs() < 1.0e-4);
        assert!((shot.time - t).abs() < 0.1, "{} vs {t}", shot.time);
    }

    /// A launch behind a wall lands on the launch at once.
    #[test]
    fn a_walled_launch_lands_on_itself() {
        let m = missile();
        let shot = fly(&from(&m, None), || 0.5, |_, b| Some((b, 0.5)));
        assert_eq!((shot.impact, shot.time), (shot.launch, 0.0));
    }

    /// A flight that never lands ends at its last step, the row's longest flight.
    #[test]
    fn an_endless_flight_stops_at_its_longest() {
        let mut m = missile();
        m.max_duration = 1.0;
        let shot = fly(&from(&m, None), || 0.5, |_, _| None);
        assert!((shot.time - 1.0).abs() < 1.0e-6);
    }
}
