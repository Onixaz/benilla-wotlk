//! Object effect sounds: a unit whose display carries an `ObjectEffectPackage` plays its package's
//! sounds by state, an Isle of Conquest Demolisher's engine idle while it stands and its run loop
//! and gear shift while it moves, its launch and load as it throws. A per-unit manager holds the
//! entered states and the live loops (`0x6f7270` enter, `0x6f61d0` exit, `0x6f6420` the loop's
//! reference count); it starts on a display change (`0x725df0`) in states 1 and 38 (`0x6f7420`),
//! then follows the unit's movement flags (`0x721300`) and its animation (`0x71e5b0`).
//!
//! Deviations, each with its reason:
//! - a loop stops at once on its state's exit, where the reference fades it over 0.7 s and an
//!   enter inside the fade resumes it: a live channel has no fade here;
//! - the speed → pitch modifier (11 rows, none a siege engine's) is not applied: a live channel
//!   has no pitch control here;
//! - the sounds play at the unit's origin, not at the effect's attachment and offset, and a
//!   one-shot stays where it started;
//! - a loop always loops: its kit's `SoundEntriesAdvanced` usage, which can make it play once, is
//!   not read;
//! - the walk and run states (49-60) are not entered: they need the base-speed test, and no 12340
//!   package lists them;
//! - the animation feed reads the driver's requested mount, base and overlay ids, not the three
//!   played sequences after their fallback, and never a variation, so Stand's and SwimIdle's
//!   alternate states (41, 42) are not entered.
//!
//! GameObject packages (a transport's states 2-5, a door's 71-74) are not built.

use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;

use benilla_formats::ObjectEffectCatalog;
use benilla_protocol::EntityKind;

use crate::creature_anim::select::unify;
use crate::creature_anim::{move_flags, AnimDriver, MovementState};
use crate::entities::mount::{MountBody, MountChild};
use crate::net::{CreatureSwimming, NetEntity, RemoteMotion, Spline, UnitMoveModes};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit_ext, stop_source_kit, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The display → package → effect tables.
#[derive(Resource)]
pub(super) struct ObjectEffects(pub(super) ObjectEffectCatalog);

fn load_object_effects(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_object_effects(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} object effect packages", cat.len());
            commands.insert_resource(ObjectEffects(cat));
        }
        Err(e) => warn!("sound: object effects failed to load: {e:#}"),
    }
}

/// The states this port enters, by the reference's numbers.
mod state {
    /// Entered when the manager starts and never left by a unit (`0x6f7420`).
    pub const STARTED: u32 = 1;
    pub const ASCENDING: u32 = 6;
    pub const DESCENDING: u32 = 7;
    pub const DEATH: u32 = 15;
    /// Any of forward, back or strafe.
    pub const MOVING: u32 = 37;
    pub const STILL: u32 = 38;
    pub const TURNING: u32 = 47;
    pub const NOT_TURNING: u32 = 48;
    pub const FLYING: u32 = 61;
    pub const FLY_FORWARD: u32 = 62;
    pub const FLY_BACK: u32 = 63;
    pub const FLY_LEFT: u32 = 64;
    pub const FLY_RIGHT: u32 = 65;
    pub const FLY_STRAFE: u32 = 66;
    pub const DEAD: u32 = 81;
}

/// The animation → state map (`0x6f1820`, read by `0x6f17f0` at variation 0); 0 for none.
fn anim_state(anim: u16) -> u32 {
    match anim {
        107 => 8,  // AttackThrown
        111 => 9,  // HoldThrown
        112 => 10, // LoadThrown
        108 => 11, // ReadyThrown
        5 => 12,   // Run
        4 => 13,   // Walk
        9 => 14,   // CombatWound
        1 => state::DEATH,
        6 => state::DEAD,
        53 => 16,                              // SpellCastDirected
        8 => 17,                               // StandWound
        25 => 18,                              // ReadyUnarmed
        0 => 19,                               // Stand
        11 => 20,                              // ShuffleLeft
        12 => 21,                              // ShuffleRight
        13 => 22,                              // Walkbackwards
        37 => 23,                              // JumpStart
        39 => 24,                              // JumpEnd
        40 => 25,                              // Fall
        41 => 26,                              // SwimIdle
        42 => 27,                              // Swim
        43 => 28,                              // SwimLeft
        44 => 29,                              // SwimRight
        45 => 30,                              // SwimBackwards
        93 => 31,                              // RunLeft
        92 => 32,                              // RunRight
        135 => 33,                             // Fly
        143 => 34,                             // Sprint
        187 => 35,                             // JumpLandRun
        38 => 36,                              // Jump
        10 => 39,                              // CombatCritical
        16 => 40,                              // AttackUnarmed
        97 => 43,                              // SitGround
        98 => 44,                              // SitGroundUp
        96 => 45,                              // SitGroundDown
        193 => 46,                             // Hover
        226 => 67,                             // Grab
        162 => 68,                             // ShipStart
        163 => 69,                             // ShipMoving
        164 => 70,                             // ShipStop
        148 => 71,                             // Open
        149 => 72,                             // Opened
        146 => 73,                             // Close
        147 => 74,                             // Closed
        150 => 75,                             // Destroy
        151 => 76,                             // Destroyed
        153..=156 => 77 + (anim - 153) as u32, // Custom0..3
        _ => 0,
    }
}

/// The states a death leaves (`0x71e5b0`): moving, still, climbing, turning and every gait.
const DEATH_LEAVES: [u32; 24] = [
    37, 38, 6, 7, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66,
];

/// What the manager asks of the sound output, by kit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cue {
    OneShot(u32),
    Loop(u32),
    Stop(u32),
}

/// A playing loop: one per group and effect, held by each entered state naming the group.
struct LiveLoop {
    group: u32,
    effect: u32,
    kit: Option<u32>,
    holds: u32,
}

/// One unit's package state.
struct UnitEffects {
    package: u32,
    /// Bit `n`: state `n` entered (82 states).
    states: u128,
    loops: Vec<LiveLoop>,
    /// The state each animation slot last entered (mount, base, overlay).
    slot_states: [u32; 3],
    /// The movement flags and animation ids last fed, so a feed runs on a change.
    fed_flags: u32,
    fed_anims: [Option<u16>; 3],
}

impl UnitEffects {
    /// Start on `package` in states 1 and 38, then take the movement flags (`0x6f7420`, `0x725df0`).
    fn start(cat: &ObjectEffectCatalog, package: u32, flags: u32, cues: &mut Vec<Cue>) -> Self {
        let mut m = Self {
            package,
            states: 0,
            loops: Vec::new(),
            slot_states: [0; 3],
            fed_flags: flags,
            fed_anims: [None; 3],
        };
        m.enter(cat, state::STARTED, cues);
        m.enter(cat, state::STILL, cues);
        m.movement(cat, flags, cues);
        m
    }

    fn has(&self, state: u32) -> bool {
        state < 128 && self.states & (1 << state) != 0
    }

    /// Enter `state`: its groups' enter one-shots, then their loops (`0x6f7270`).
    fn enter(&mut self, cat: &ObjectEffectCatalog, state: u32, cues: &mut Vec<Cue>) {
        if state >= 128 || self.has(state) {
            return;
        }
        self.states |= 1 << state;
        for group in cat.state_groups(self.package, state) {
            cues.extend(
                group
                    .enter
                    .iter()
                    .filter_map(|e| e.sound_kit())
                    .map(Cue::OneShot),
            );
            for effect in &group.looping {
                // A loop the group already plays takes one more hold (`0x6f6420`).
                if let Some(live) = self
                    .loops
                    .iter_mut()
                    .find(|l| l.group == group.id && l.effect == effect.id)
                {
                    live.holds += 1;
                    continue;
                }
                let kit = effect.sound_kit();
                self.loops.push(LiveLoop {
                    group: group.id,
                    effect: effect.id,
                    kit,
                    holds: 1,
                });
                cues.extend(kit.map(Cue::Loop));
            }
        }
    }

    /// Leave `state`: its groups' exit one-shots when asked, and each loop its last hold stops
    /// (`0x6f61d0`).
    fn exit(
        &mut self,
        cat: &ObjectEffectCatalog,
        state: u32,
        play_exit: bool,
        cues: &mut Vec<Cue>,
    ) {
        if !self.has(state) {
            return;
        }
        self.states &= !(1 << state);
        for group in cat.state_groups(self.package, state) {
            if play_exit {
                cues.extend(
                    group
                        .exit
                        .iter()
                        .filter_map(|e| e.sound_kit())
                        .map(Cue::OneShot),
                );
            }
            self.loops.retain_mut(|l| {
                if l.group != group.id {
                    return true;
                }
                l.holds = l.holds.saturating_sub(1);
                if l.holds > 0 {
                    return true;
                }
                cues.extend(l.kit.map(Cue::Stop));
                false
            });
        }
    }

    /// The movement feed (`0x721300`): the moving, climbing, turning and flying states from the
    /// flags, then the exits of those they no longer hold.
    fn movement(&mut self, cat: &ObjectEffectCatalog, flags: u32, cues: &mut Vec<Cue>) {
        use move_flags as f;
        let moving = flags & (f::FORWARD | f::BACKWARD | f::STRAFE_LEFT | f::STRAFE_RIGHT) != 0;
        let turning = flags & (f::TURN_LEFT | f::TURN_RIGHT) != 0;
        let flying = flags & f::FLYING != 0;
        let strafing = flags & (f::STRAFE_LEFT | f::STRAFE_RIGHT) != 0;
        self.enter(cat, if moving { state::MOVING } else { state::STILL }, cues);
        if flags & f::ASCENDING != 0 {
            self.enter(cat, state::ASCENDING, cues);
        } else if flags & f::DESCENDING != 0 {
            self.enter(cat, state::DESCENDING, cues);
        }
        self.enter(
            cat,
            if turning {
                state::TURNING
            } else {
                state::NOT_TURNING
            },
            cues,
        );
        if flying {
            self.enter(cat, state::FLYING, cues);
            if flags & f::FORWARD != 0 {
                self.enter(cat, state::FLY_FORWARD, cues);
            }
            if flags & f::BACKWARD != 0 {
                self.enter(cat, state::FLY_BACK, cues);
            }
            if strafing {
                self.enter(cat, state::FLY_STRAFE, cues);
                if flags & f::STRAFE_LEFT != 0 {
                    self.enter(cat, state::FLY_LEFT, cues);
                }
                if flags & f::STRAFE_RIGHT != 0 {
                    self.enter(cat, state::FLY_RIGHT, cues);
                }
            }
        }
        // The reference's walk and run exits are left out with their enters (49-60).
        let ahead = [state::FLYING, state::FLY_FORWARD, state::FLY_BACK];
        if flags & (f::FORWARD | f::BACKWARD) != 0 && flying {
            let other = if flags & f::FORWARD != 0 {
                state::FLY_BACK
            } else {
                state::FLY_FORWARD
            };
            self.exit(cat, other, true, cues);
        } else {
            ahead.iter().for_each(|&s| self.exit(cat, s, true, cues));
        }
        let side = [state::FLY_LEFT, state::FLY_RIGHT, state::FLY_STRAFE];
        if strafing && flying {
            let other = if flags & f::STRAFE_LEFT != 0 {
                state::FLY_RIGHT
            } else {
                state::FLY_LEFT
            };
            self.exit(cat, other, true, cues);
        } else {
            side.iter().for_each(|&s| self.exit(cat, s, true, cues));
        }
        self.exit(
            cat,
            if moving { state::STILL } else { state::MOVING },
            true,
            cues,
        );
        self.exit(
            cat,
            if turning {
                state::NOT_TURNING
            } else {
                state::TURNING
            },
            true,
            cues,
        );
        if flags & f::ASCENDING == 0 {
            self.exit(cat, state::ASCENDING, true, cues);
        }
        if flags & f::DESCENDING == 0 {
            self.exit(cat, state::DESCENDING, true, cues);
        }
    }

    /// The animation feed (`0x71e5b0`): each slot's anim enters its state, a death or dead pose
    /// leaves the movement states, and a state no slot plays any more is left.
    fn animation(
        &mut self,
        cat: &ObjectEffectCatalog,
        anims: [Option<u16>; 3],
        cues: &mut Vec<Cue>,
    ) {
        let now: Vec<(u32, usize)> = anims
            .iter()
            .enumerate()
            .filter_map(|(slot, a)| a.map(|a| (anim_state(a), slot)))
            .collect();
        let previous = self.slot_states;
        for s in &mut self.slot_states {
            if *s != 0 && !now.iter().any(|&(n, _)| n == *s) {
                *s = 0;
            }
        }
        for &(s, slot) in &now {
            // A state a slot already holds, 0 included, is skipped (`j != 3`).
            if self.slot_states.contains(&s) {
                continue;
            }
            if s != 0 {
                self.enter(cat, s, cues);
                if s == state::DEATH || s == state::DEAD {
                    DEATH_LEAVES
                        .iter()
                        .for_each(|&d| self.exit(cat, d, true, cues));
                }
            }
            self.slot_states[slot] = s;
        }
        for p in previous {
            if p != 0 && !now.iter().any(|&(n, _)| n == p) {
                self.exit(cat, p, true, cues);
            }
        }
    }

    /// Stop everything, playing no exit effect (`0x6f70b0`): the loops, then the one-shots.
    fn tear_down(&mut self, cat: &ObjectEffectCatalog, cues: &mut Vec<Cue>) {
        for s in 0..128 {
            self.exit(cat, s, false, cues);
        }
        for group in cat.package_groups(self.package) {
            let one_shots = group.enter.iter().chain(&group.exit);
            cues.extend(one_shots.filter_map(|e| e.sound_kit()).map(Cue::Stop));
        }
    }
}

/// The package a unit's display carries: the mount's display first, as the reference's
/// `+0x9c0 ?: +0x9d4` / `UNIT_FIELD_DISPLAYID` (`0x725df0`); 0 for none or a package with no row.
fn package_of(cat: &ObjectEffectCatalog, display: Option<u32>) -> u32 {
    display
        .and_then(|d| cat.package_for_display(d))
        .filter(|&p| cat.has_package(p))
        .unwrap_or(0)
}

/// Runs each unit's manager: start, swap or drop it on its display, feed it the movement flags
/// and animation when they change, play its cues, and re-arm a live loop whose channel is gone
/// (culled out of range or never opened), as the per-frame update keeps it playing (`0x6f39b0`).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn run_object_effects(
    units: Query<
        (
            Entity,
            &NetEntity,
            &Transform,
            Option<&MountChild>,
            Option<&AnimDriver>,
            (
                Option<&MovementState>,
                Option<&RemoteMotion>,
                Option<&Spline>,
                Has<CreatureSwimming>,
                Option<&UnitMoveModes>,
            ),
        ),
        Without<MountBody>,
    >,
    mounts: Query<(&NetEntity, Option<&AnimDriver>), With<MountBody>>,
    mut managers: Local<EntityHashMap<UnitEffects>>,
    effects: Res<ObjectEffects>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let cat = &effects.0;
    let listener = listener.pos;
    let mut seen = EntityHashSet::default();
    let mut cues = Vec::new();
    for (entity, net, transform, mount_child, driver, (movement, remote, spline, swim, modes)) in
        &units
    {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        let mount = mount_child.and_then(|mc| mounts.get(mc.0).ok());
        let display = mount.and_then(|(m, _)| m.display_id).or(net.display_id);
        let package = package_of(cat, display);
        let flags = unify(movement, remote, spline, swim, modes).flags;
        cues.clear();
        if managers.get(&entity).is_some_and(|m| m.package != package) {
            if let Some(mut old) = managers.remove(&entity) {
                old.tear_down(cat, &mut cues);
            }
        }
        if package != 0 {
            seen.insert(entity);
            let m = managers
                .entry(entity)
                .or_insert_with(|| UnitEffects::start(cat, package, flags, &mut cues));
            if m.fed_flags != flags {
                m.fed_flags = flags;
                m.movement(cat, flags, &mut cues);
            }
            let (base, overlay) = driver.map_or((None, None), |d| d.playing());
            let mount_anim = mount.and_then(|(_, d)| d).and_then(|d| d.playing().0);
            let anims = [mount_anim, base, overlay];
            if m.fed_anims != anims {
                m.fed_anims = anims;
                m.animation(cat, anims, &mut cues);
            }
        }
        play_cues(
            &cues,
            entity,
            transform.translation,
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
        );
        let Some(m) = managers.get(&entity) else {
            continue;
        };
        for kit in m.loops.iter().filter_map(|l| l.kit) {
            let playing = out
                .channels
                .iter()
                .any(|c| c.source() == Some(entity) && c.kit == kit);
            if !playing {
                play_cues(
                    &[Cue::Loop(kit)],
                    entity,
                    transform.translation,
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener,
                );
            }
        }
    }
    // A despawned unit's channels die with the despawn reaper; only its manager is left to drop.
    managers.retain(|e, _| seen.contains(e));
}

#[allow(clippy::too_many_arguments)] // the play path's inputs, as `play_kit_ext` takes them
fn play_cues(
    cues: &[Cue],
    entity: Entity,
    pos: Vec3,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
) {
    for &cue in cues {
        let (kit, force_loop) = match cue {
            Cue::Stop(kit) => {
                stop_source_kit(out, entity, kit);
                continue;
            }
            Cue::OneShot(kit) => (kit, false),
            Cue::Loop(kit) => (kit, true),
        };
        // A kit absent from SoundEntries is silent, as the kit play returns on it.
        if kits.catalog().get(kit).is_none() {
            continue;
        }
        let extras = PlayExtras {
            source: Some(entity),
            force_loop,
            ..default()
        };
        if let Err(e) = play_kit_ext(
            kits,
            assets,
            out,
            config,
            listener,
            KitRef::Id(kit),
            Some(pos),
            SoundCategory::Sfx,
            extras,
        ) {
            warn!("object effect sound (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_object_effects.after(AssetSet::Open))
        .add_systems(
            Update,
            run_object_effects
                .run_if(resource_exists::<ObjectEffects>)
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests;
