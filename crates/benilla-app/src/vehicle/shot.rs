//! A trajectory cast: a spell whose `SpellMissile` row has flag `0x1` leaves with castFlags 2
//! (`0x80b39d`), its caster the active mover. The client flies the missile itself (`0x6fcd60`):
//! from the mover's pitch and launch power, in 0.1 s steps against the world, and sends the launch
//! as SOURCE, the landing as DEST, the pitch and the speed, then the mover's stop report
//! (`0x80b406`-`0x80b46c`).
//!
//! The aim preview with its arc and impact models (`0x6fda20`) is [`super::targeting`]'s, flying
//! this same flight at the rows' middles. An instant cast with a cast animation is held until the
//! animation releases its missile, then aimed and sent (`0x809f80`, `0x80de60`); a cast with a cast
//! time goes at the press and sends its final aim as `CMSG_UPDATE_MISSILE_TRAJECTORY` once its time
//! has run, at the release for a kit with an animation (`0x6fbe50`, `0x6fd6b0`).
//!
//! Not built: the spell visual's launch offset (`0x720bf0`); a sloped mover's tilt of the launch; and the sequence mark the reference writes
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
    /// The cast attempt's record `castCount` (`0x8050dc`).
    pub(crate) count: u8,
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
    random: impl FnMut() -> f32,
    hit: impl FnMut(Vec3, Vec3) -> Option<(Vec3, f32)>,
) -> Aimed {
    fly_traced(from, random, hit, |_| {})
}

/// [`fly`], handing `point` each step's end short of the landing and then the landing, the
/// preview's point list (`0x6fcd60` with `points`): a walled launch hands the launch alone.
pub(crate) fn fly_traced(
    from: &AimFrom,
    mut random: impl FnMut() -> f32,
    mut hit: impl FnMut(Vec3, Vec3) -> Option<(Vec3, f32)>,
    mut point: impl FnMut(Vec3),
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
    let (impact, time) = land(from, m, pitch, speed, yaw, launch, &mut hit, &mut point);
    // The landing closes the list on every exit.
    point(impact);
    Aimed {
        launch,
        impact,
        pitch,
        speed,
        time,
    }
}

/// The flight from `launch`: the landing and its seconds, handing `point` each step's end short of
/// it.
#[allow(clippy::too_many_arguments)] // the flight's resolved launch, one call site
fn land(
    from: &AimFrom,
    m: &SpellMissileRec,
    pitch: f32,
    speed: f32,
    yaw: f32,
    launch: Vec3,
    hit: &mut impl FnMut(Vec3, Vec3) -> Option<(Vec3, f32)>,
    point: &mut impl FnMut(Vec3),
) -> (Vec3, f32) {
    if hit(from.body, launch).is_some() {
        return (launch, 0.0);
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
        if let Some((landing, t)) = hit(at, to) {
            return (landing, ((max - left) as f32 + t) * STEP);
        }
        left -= 1;
        if left <= 0 {
            return (to, max as f32 * STEP);
        }
        point(to);
        velocity.z -= gravity;
        at = to;
    }
}

/// The aim a mover flies from: `launch` its Bevy-space fire point, the wall test lifted from its
/// base by half its collision height.
pub(super) fn aim_from<'a>(
    missile: &'a SpellMissileRec,
    vehicle: Option<&'a VehicleRec>,
    aim: &super::aim::VehicleAim,
    player: &crate::player::Player,
    launch: Vec3,
    height: Option<&crate::entities::CollisionHeight>,
) -> AimFrom<'a> {
    let lift = height.map_or(crate::player::DEFAULT_COLLISION_HEIGHT, |h| h.0) * 0.5;
    AimFrom {
        missile,
        vehicle,
        pitch: aim.pitch,
        power: aim.power,
        launch: to_wow(launch),
        facing: player.facing(),
        body: to_wow(player.pos + Vec3::Y * lift),
    }
}

/// A Bevy point in WoW coordinates.
pub(super) fn to_wow(v: Vec3) -> Vec3 {
    Vec3::from(benilla_assets::coords::bevy_to_wow(v))
}

/// The flight's world test in WoW coordinates: where a segment first meets a body, and its
/// fraction.
pub(super) fn world_hit<'a>(
    world: &'a benilla_world::collision::WorldCollision<'_, '_>,
) -> impl FnMut(Vec3, Vec3) -> Option<(Vec3, f32)> + 'a {
    move |a, b| {
        let (a, b) = (
            benilla_assets::coords::wow_to_bevy(a.to_array()),
            benilla_assets::coords::wow_to_bevy(b.to_array()),
        );
        let (dir, len) = Dir3::new_and_length(b - a).ok()?;
        let d = world.ray_body(a, dir, len)?.distance;
        Some((to_wow(a + dir * d), d / len))
    }
}

/// Aim and send each handed trajectory cast, from the mover we drive.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // a Bevy system's full input set
/// An instant trajectory cast held until its cast animation releases the missile (`0x809f80`):
/// the press plays the cast's animation, and the release sends it, aimed then (`0x72df00` →
/// `0x80de60`). A second press while one is held fails (`0x809f80`'s error 105).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HeldCast {
    cast: TrajectoryCast,
    mover: Entity,
    since_s: f64,
    /// Whether the cast animation one-shot has been observed running on the mover.
    saw_oneshot: bool,
}

/// A trajectory cast with a cast time, whose final aim goes out as
/// `CMSG_UPDATE_MISSILE_TRAJECTORY` once the cast time has run (`0x6fbe50`, `0x6fd6b0`), at its
/// animation's missile release for a cast kit with one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FinalAim {
    caster: u64,
    spell_id: u32,
    mover: Entity,
    due_s: f64,
    /// Waits past the cast time for the animation's release.
    at_release: bool,
    /// Whether the server's `Casting` state was observed for this cast.
    saw_casting: bool,
    /// Whether the release animation one-shot was observed running once `due_s` passed.
    saw_oneshot: bool,
}

/// A locally launched trajectory missile (`0x809f80`, `0x80de60`), drained by
/// [`crate::creature_anim::spell_visual::route_cast_visuals`] to fly immediately at release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocalTrajectoryLaunch {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    pub(crate) dest: Vec3,
    pub(crate) trajectory: crate::creature_anim::Trajectory,
    pub(crate) seq: u64,
}

/// Our trajectory casts whose visuals and missile launched locally (`0x809f80`, `0x80de60`):
/// their server GO plays no cast kit and spawns no missile (`0x80e1b0`'s trajectory arm).
#[derive(Resource, Default)]
pub(crate) struct LocalTrajectoryGos {
    sent: Vec<(Entity, u32, f64)>,
    pub(crate) launches: Vec<LocalTrajectoryLaunch>,
}

impl LocalTrajectoryGos {
    /// Whether `caster`'s GO of `spell_id` is one of ours.
    pub(crate) fn has(&self, caster: Entity, spell_id: u32) -> bool {
        self.sent
            .iter()
            .any(|&(e, s, _)| e == caster && s == spell_id)
    }

    /// Spend the entry the GO consumed.
    pub(crate) fn take(&mut self, caster: Entity, spell_id: u32) -> bool {
        let before = self.sent.len();
        self.sent
            .retain(|&(e, s, _)| !(e == caster && s == spell_id));
        self.sent.len() != before
    }

    /// Record a locally launched trajectory cast waiting for its server GO.
    fn record(&mut self, caster: Entity, spell_id: u32, at: f64) {
        self.sent.push((caster, spell_id, at));
    }
}

/// How long a sent cast waits for its GO before it is forgotten.
const LOCAL_GO_WAIT_S: f64 = 5.0;

/// Grace window for the cast animation's one-shot to start on the mover before releasing a cast
/// whose rig never started one (matching `entities/missile.rs`'s `RELEASE_WAIT_MAX`).
const RELEASE_START_GRACE_S: f64 = 0.25;

/// The casts the send holds between frames.
#[derive(Default)]
pub(super) struct ShotMemory {
    held: Option<HeldCast>,
    final_aim: Option<FinalAim>,
}

/// Whether a held cast or final-aim release should fire this frame: on a release keyframe, when
/// the cast one-shot ends without firing one (`0x72df00`), or after the start grace if no one-shot
/// ever started on the mover.
fn release_ready(
    keyframe: bool,
    live_oneshot: bool,
    saw_oneshot: &mut bool,
    elapsed_s: f64,
) -> bool {
    if keyframe {
        return true;
    }
    *saw_oneshot |= live_oneshot;
    (*saw_oneshot && !live_oneshot) || (!*saw_oneshot && elapsed_s > RELEASE_START_GRACE_S)
}

/// A release keyframe on the mover: the spell's cast-hand and target markers, and the bow's.
const RELEASE_IDENTS: [[u8; 4]; 4] = [*b"$CSL", *b"$CSR", *b"$CST", *b"$BWR"];

/// Aim and send the frame's trajectory casts, and the held ones their release frees.
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
            Option<&crate::net::ObjectStore>,
            Option<&crate::creature_anim::Casting>,
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
    (mut targeting, mods, mut memory, time, mut keys, mut anims, mut local_gos, rigs, anim_data): (
        ResMut<super::targeting::TrajectoryTargeting>,
        Option<Res<crate::spell::SpellModifiers>>,
        Local<ShotMemory>,
        Res<Time<Real>>,
        MessageReader<crate::creature_anim::AnimSoundEvent>,
        (
            Option<MessageWriter<crate::creature_anim::CastEvent>>,
            Option<ResMut<crate::creature_anim::PlaySeq>>,
        ),
        Option<ResMut<LocalTrajectoryGos>>,
        Query<(
            &crate::creature_anim::AnimDriver,
            &AnimationPlayer,
            &AnimationTransitions,
            &benilla_assets::ModelAnimations,
        )>,
        Option<Res<crate::creature_anim::AnimData>>,
    ),
) {
    let now = time.elapsed_secs_f64();
    if let Some(l) = local_gos.as_mut().filter(|l| !l.sent.is_empty()) {
        l.sent.retain(|&(.., at)| now - at < LOCAL_GO_WAIT_S);
    }
    let released = |mover: Entity, keys: &[(Entity, [u8; 4])]| {
        keys.iter()
            .any(|&(e, ident)| e == mover && RELEASE_IDENTS.contains(&ident))
    };
    let catalog = anim_data.as_deref().map(|d| &d.0);
    let oneshot_live = |mover: Entity| {
        rigs.get(mover)
            .ok()
            .is_some_and(|(drv, player, tr, anims)| {
                crate::creature_anim::oneshot_is_live(drv, player, tr, anims, catalog)
            })
    };
    let keys: Vec<(Entity, [u8; 4])> = keys.read().map(|k| (k.entity, k.ident)).collect();
    let active_mover = mover.single().ok();
    // Clear any held cast or pending final aim if the active mover changed or vanished (`0x80de60`),
    // or if a cast-time trajectory spell was interrupted before finishing.
    if let Some(h) = memory.held {
        if active_mover.is_none_or(|(e, ..)| e != h.mover) {
            memory.held = None;
        }
    }
    if let Some(f) = &mut memory.final_aim {
        match active_mover {
            Some((e, .., casting)) if e == f.mover => {
                if casting.is_some_and(|c| c.spell_id == f.spell_id) {
                    f.saw_casting = true;
                } else if f.saw_casting && now < f.due_s {
                    memory.final_aim = None;
                }
            }
            _ => {
                memory.final_aim = None;
            }
        }
    }
    // The held cast goes on its release, aimed now.
    let mut todo: Vec<(TrajectoryCast, bool)> = std::mem::take(&mut casts.0)
        .into_iter()
        .map(|c| (c, false))
        .collect();
    if let Some(h) = &mut memory.held {
        let keyframe = released(h.mover, &keys);
        let live = oneshot_live(h.mover);
        if release_ready(keyframe, live, &mut h.saw_oneshot, now - h.since_s) {
            let h = *h;
            debug!(
                "vehicle: held trajectory cast {} released after {:.3} s{}",
                h.cast.spell_id,
                now - h.since_s,
                if keyframe { "" } else { " (anim end)" }
            );
            memory.held = None;
            todo.push((h.cast, true));
        }
    }
    let final_due = memory.final_aim.as_mut().and_then(|f| {
        if now < f.due_s {
            return None;
        }
        if !f.at_release {
            return Some(*f);
        }
        let keyframe = released(f.mover, &keys);
        let live = oneshot_live(f.mover);
        release_ready(keyframe, live, &mut f.saw_oneshot, now - f.due_s).then_some(*f)
    });
    if todo.is_empty() && final_due.is_none() {
        return;
    }
    let (Some(net), Some(player)) = (net, player) else {
        return;
    };
    let Some((entity, guid, vehicle, flags2, height, store, casting)) = active_mover else {
        memory.final_aim = None;
        debug!("vehicle: {} trajectory casts with no mover", todo.len());
        return;
    };
    // The flight from the mover's aim this frame, for spell `def`'s missile row.
    let fly_now = |missile: &SpellMissileRec| {
        let launch = crate::entities::aim_launch_pos(entity, &units, &joints)?;
        let from = aim_from(
            missile,
            vehicle.and_then(|v| tables.vehicle(v.id)),
            &aim,
            &player,
            launch,
            height,
        );
        Some(fly(&from, rand::random::<f32>, world_hit(&world)))
    };
    let pose = (
        player.move_flags(),
        flags2.map_or(0, |f| f.0),
        benilla_assets::coords::bevy_to_wow(player.pos),
        player.facing().rem_euclid(std::f32::consts::TAU),
    );
    // The final aim of a cast whose time has run (`0x6fd6b0`).
    if let Some(f) = final_due {
        memory.final_aim = None;
        let missile = spells
            .as_deref()
            .and_then(|s| s.catalog.get(f.spell_id))
            .zip(visuals.as_deref())
            .and_then(|(d, v)| v.0.missile(d.missile));
        if let Some(shot) = missile.and_then(fly_now).filter(|_| guid.0 == f.caster) {
            debug!(
                "vehicle: trajectory {} final aim pitch {:.3} speed {:.2}",
                f.spell_id, shot.pitch, shot.speed
            );
            let _ = net.0.send(ClientCommand::UpdateMissileTrajectory {
                caster: f.caster,
                spell_id: f.spell_id,
                shot: TrajectoryShot {
                    source: shot.launch.to_array(),
                    dest: shot.impact.to_array(),
                    pitch: shot.pitch,
                    speed: shot.speed,
                },
                flags: pose.0,
                flags2: pose.1,
                pos: pose.2,
                orientation: pose.3,
            });
        }
    }
    for (cast, freed) in todo {
        // `0x80b3a6`: a caster that is not the active mover fails unreported.
        if guid.0 != cast.caster {
            debug!(
                "vehicle: trajectory cast {} from {:#x}, not the mover",
                cast.spell_id, cast.caster
            );
            continue;
        }
        let Some((def, missile)) = spells
            .as_deref()
            .and_then(|s| s.catalog.get(cast.spell_id))
            .zip(visuals.as_deref())
            .and_then(|(d, v)| Some((d, v.0.missile(d.missile)?)))
        else {
            continue;
        };
        let level = store.and_then(|s| s.0.unit_level()).unwrap_or(0);
        let cast_ms = spells.as_deref().map_or(0, |s| match mods.as_deref() {
            Some(m) => s.cast_time_unclamped_ms(def, level, m),
            None => s.cast_time_unclamped_ms(def, level, &Default::default()),
        });
        let stages = visuals.as_deref().and_then(|v| {
            v.0.stages(crate::creature_anim::spell_visual::spell_visual_id(def))
        });
        let kit = stages.and_then(|st| visuals.as_deref()?.0.kit(st.cast));
        // The aim preview draws the spell sent (`0x6fbf90`), and its cast leg holds a cast the
        // reference holds for its animation (`0x809f80`).
        let instant_missile =
            cast_ms <= 0 && casting.is_none() && stages.is_some_and(|s| s.missile_gate != 0);
        let held =
            cast_ms <= 0 && super::targeting::held_until_release(casting.is_some(), stages, kit);
        if !freed {
            // `0x809f80`: refuse while a cast-time trajectory update is pending (error 105).
            if memory.final_aim.is_some() {
                debug!(
                    "vehicle: trajectory cast {} refused, final aim pending",
                    cast.spell_id
                );
                continue;
            }
            if held {
                if memory.held.is_some() {
                    // `0x809f80`: one held cast at a time; the second fails (error 105).
                    debug!(
                        "vehicle: trajectory cast {} refused, one is held",
                        cast.spell_id
                    );
                    continue;
                }
                // Drop any stale local-GO suppression left behind by a previously failed cast.
                if let Some(l) = local_gos.as_mut() {
                    l.take(entity, cast.spell_id);
                }
                targeting.sent(cast.caster, cast.count, cast.spell_id, true);
                // The press plays the cast's visuals, its kit's animation and sound among them
                // (`0x809f80`), through the GO's own kit play; its release sends the cast.
                let (events, seq) = &mut anims;
                if let Some(events) = events.as_mut() {
                    events.write(crate::creature_anim::CastEvent {
                        entity,
                        spell_id: cast.spell_id,
                        kind: crate::creature_anim::CastEventKind::Go { ends_cast: false },
                        cast_count: cast.count,
                        seq: seq.as_mut().map_or(0, |s| s.next()),
                    });
                }
                debug!(
                    "vehicle: trajectory cast {} held for its animation's release",
                    cast.spell_id
                );
                memory.held = Some(HeldCast {
                    cast,
                    mover: entity,
                    since_s: now,
                    saw_oneshot: false,
                });
                continue;
            }
            targeting.sent(cast.caster, cast.count, cast.spell_id, false);
        }
        let Some(shot) = fly_now(missile) else {
            continue;
        };
        debug!(
            "vehicle: trajectory cast {} pitch {:.3} speed {:.2} lands in {:.2} s at {:?}",
            cast.spell_id, shot.pitch, shot.speed, shot.time, shot.impact
        );
        // A released held cast (or an unheld instant trajectory cast) flies its missile locally
        // at once (`0x809f80`, `0x80de60`); its server GO skips its cast kit and missile spawn
        // (`0x80e1b0`).
        let launch_local = freed || instant_missile;
        if launch_local {
            if !freed {
                if let Some(l) = local_gos.as_mut() {
                    l.take(entity, cast.spell_id);
                }
                let (events, seq) = &mut anims;
                if let Some(events) = events.as_mut() {
                    events.write(crate::creature_anim::CastEvent {
                        entity,
                        spell_id: cast.spell_id,
                        kind: crate::creature_anim::CastEventKind::Go { ends_cast: false },
                        cast_count: cast.count,
                        seq: seq.as_mut().map_or(0, |s| s.next()),
                    });
                }
            }
            let seq = anims.1.as_mut().map_or(0, |s| s.next());
            if let Some(l) = local_gos.as_mut() {
                l.record(entity, cast.spell_id, now);
                l.launches.push(LocalTrajectoryLaunch {
                    caster: entity,
                    spell_id: cast.spell_id,
                    dest: benilla_assets::coords::wow_to_bevy(shot.impact.to_array()),
                    trajectory: crate::creature_anim::Trajectory {
                        pitch: shot.pitch,
                        speed: shot.speed,
                        delay: shot.time,
                        caster: cast.caster,
                        cast_id: cast.count,
                    },
                    seq,
                });
            }
        }
        let _ = net.0.send(ClientCommand::PetCastTrajectory {
            count: cast.count,
            pet_guid: cast.caster,
            spell_id: cast.spell_id,
            shot: TrajectoryShot {
                source: shot.launch.to_array(),
                dest: shot.impact.to_array(),
                pitch: shot.pitch,
                speed: shot.speed,
            },
            flags: pose.0,
            flags2: pose.1,
            pos: pose.2,
            orientation: pose.3,
        });
        // A cast with a cast time sends its final aim once the time has run (`0x6fbe50`), at the
        // missile's release for a kit with an animation (`0x6fbf90`'s tail).
        if cast_ms > 0 && missile.flags & 0x1 != 0 {
            memory.final_aim = Some(FinalAim {
                caster: cast.caster,
                spell_id: cast.spell_id,
                mover: entity,
                due_s: now + f64::from(cast_ms) / 1000.0,
                at_release: super::targeting::held_until_release(false, stages, kit),
                saw_casting: false,
                saw_oneshot: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A released cast's GO is ours once: it is found by caster and spell, then spent.
    #[test]
    fn a_released_cast_s_go_is_spent_once() {
        let mut w = World::new();
        let caster = w.spawn_empty().id();
        let mut gos = LocalTrajectoryGos::default();
        gos.record(caster, 47801, 1.0);
        assert!(gos.has(caster, 47801));
        assert!(!gos.has(caster, 1));
        assert!(gos.take(caster, 47801));
        assert!(!gos.take(caster, 47801), "spent");
    }

    /// A held cast waits through an arbitrarily long live one-shot, releases as soon as the
    /// one-shot ends or a release keyframe fires, and falls back after the start grace if no
    /// one-shot ever started.
    #[test]
    fn release_ready_tracks_keyframe_anim_end_and_start_grace() {
        let mut saw = false;
        // One-shot running: stays held even well past 2 s.
        assert!(!release_ready(false, true, &mut saw, 0.05));
        assert!(saw);
        assert!(!release_ready(false, true, &mut saw, 3.5));
        // One-shot ends: releases immediately.
        assert!(release_ready(false, false, &mut saw, 3.6));

        // Keyframe fires mid-animation: releases immediately.
        let mut saw_kf = false;
        assert!(!release_ready(false, true, &mut saw_kf, 0.1));
        assert!(release_ready(true, true, &mut saw_kf, 0.2));

        // No one-shot ever starts: waits during grace, releases once grace elapses.
        let mut saw_none = false;
        assert!(!release_ready(false, false, &mut saw_none, 0.1));
        assert!(release_ready(false, false, &mut saw_none, 0.3));
    }

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
