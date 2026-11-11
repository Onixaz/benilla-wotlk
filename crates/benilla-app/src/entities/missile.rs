//! Spell missiles: the projectile a cast with a `Spell.dbc` Speed above 0 flies at each target, and
//! the hand-off when it arrives. The router emits one [`MissileSpawn`] per GO; this module owns
//! launch and flight.
//!
//! Launch is release-keyed: a GO whose cast kit plays a body animation parks its projectiles on the
//! caster ([`PendingMissiles`]) until the animation's `$CSL`/`$CSR`/`$CST` (casting hand) or `$BWR`
//! (ranged) event drains them from its live position. A GO with no cast-kit animation launches at
//! once, and a queue whose event never fires launches when the one-shot ends, both from the `$CSL`
//! → `$CSR` → `$CST` → base cascade.
//!
//! The arrival deadline is fixed at GO: the launch flies `distance / Speed` less the time queued,
//! and a release already past it, as at melee range, shows only the impact. In flight the missile
//! homes on the target's live attachment and arrives on time.
//!
//! On arrival the reference picks an arm by whether the target guid still resolves, then by the hit
//! bit `[missile+0x38] & 1`, set only at the spawn:
//! - A live target hit: wound, flinch and impact visual, no word; here [`CastEventKind::Impact`].
//! - A live target missed: the outcome word, `UNIT_COMBAT` and chat, then the disposition; here the
//!   defense clip and [`MissileMiss`].
//! - No live target: the area kit on the caster, then each unit of the missile's list: its kits
//!   ([`CastEventKind::AreaImpact`]), or with outcomes its miss word; here
//!   [`CastEventKind::GroundImpact`] first. A target gone mid-flight lands here at its last aim, and
//!   a missile at an object that is not a unit lands here at once.
//!
//! A multi-target spell's one missile hops its list instead, from each unit landing to the next
//! ([`TargetList::bounce`]).
//!
//! With no visual-chain model a missile flies the wire's ammo (Auto Shot, Shoot, Throw: all
//! `SpellVisual` 0), an `ItemDisplayInfo` row resolved by shape: a model in the right slot is
//! `Item\ObjectComponents\Ammo\`, in the left `…\Weapon\` (thrown), each with its object skin. The
//! reference resolves the row, forking on the wire's ammo InventoryType (`0x19`, thrown), and the
//! shape rule matches it on every shipped row. With no model at all the missile flies invisible and
//! still impacts on schedule.
//!
//! A GO with no unit targets but a point flies one projectile at it (the reference's
//! empty-hit-array arm spawns one, unit slot −1): the Flare, a bomb thrown at empty ground. It aims
//! at a fixed point ([`Aim::Ground`]), arrives as [`CastEventKind::GroundImpact`] on the caster and
//! flies the same straight, arrive-on-time line as a unit missile: the reference's flight is
//! linear, with no gravity.
//!
//! A GO that names a trajectory flies the arc [`trajectory`] describes instead, landing at the
//! packet's delay.
//!
//! The spell's visual rows shape the flight ([`MissileShape`]): the release point gives way to
//! `SpellVisual` column 16's attachment plus the cast offset (`0x72e1b6`), the aim takes the
//! impact offset in the destination attachment's frame (`0x6ff6d0`), a follow flag hugs the ground
//! (`0x6fffc0`), a motion script moves the model off its path ([`motion`]), with one copy per
//! `SpellMissileMotion` count, and a targeting kit rides the target from launch to the flight's
//! end (`0x702986`).
//!
//! A row with flag `0x20` holds its model on the caster from cast start ([`PrecastHolds`]): the
//! first missile made after it takes it and leaves from where it was held, and every launch drops
//! it (`0x72df00`). A [`MissileRelease`] launches a queue at once (an interrupt, our own start, a
//! GO with no cast animation) or drops one cast's hold (the end-cast).
//!
//! A miss ends by its outcome ([`disposition`]): it flies past and fades, sticks in the shield,
//! bursts, glances off at half speed or flies back at the caster.
//!
//! Not built: `$BWR` launches from its event marker, where the reference first tries the ranged
//! weapon's HandArrow/Bullet attachment (`0x23`/`0x24`); homing aims at the attachment point, not
//! the ray-sphere intercept, the same body point in practice; and a ground aim on a moving
//! transport stays where it was, as the GO's dest is read in world space.

use bevy::animation::transition::AnimationTransitions;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::{
    oneshot_is_live, AnimData, AnimDriver, AnimSoundEvent, CastEvent, CastEventKind, DefenseAnim,
    FxClass, MissileRelease, MissileSpawn, ReleaseKind, SpellKitFx, SpellKitSound, SpellVisuals,
};
use benilla_assets::m2_url;

use super::equipment::{HeldWeapons, ItemDisplays};

mod disposition;
mod motion;
mod shape;
mod trajectory;
use super::spell_fx::{
    attach_effect_visuals, ensure_model, EffectHost, FxMaterials, PrecastDrop, SpellFx,
};
use super::{BoneAttach, DisplayModel, ModelHandle};
use disposition::{DeckChase, MissKind};
pub(crate) use motion::{run_missile_motion, MissileMotion, MotionScripts};
pub(crate) use shape::MissileShape;
use shape::{FaceMotion, FollowGround, MissileSource};

/// The release events whose dispatcher arms drain the queue.
const RELEASE_IDENTS: [[u8; 4]; 4] = [*b"$CSL", *b"$CSR", *b"$CST", *b"$BWR"];

/// The launch cascade when no release event names a point, before the unit's base.
const MARKER_CASCADE: [[u8; 4]; 3] = [*b"$CSL", *b"$CSR", *b"$CST"];

/// Seconds a queue waits for a one-shot that never starts, well above the frame or two a release
/// animation takes to arm. The reference's per-animation finish callback always fires; here the end
/// is a polled edge, and a kit animation that never resolves on the model would hang the queue.
const RELEASE_WAIT_MAX: f32 = 0.25;

/// Seconds a latched special idle holds a queue: past it the release goes ahead (`0x734406`).
const SPECIAL_IDLE_HOLD_MAX: f32 = 2.0;

/// The destination attachments tried after the spawn's own tag, as the reference cascades.
const DEST_FALLBACKS: [u16; 2] = [0xf, 0x13];

/// `AnimationData.dbc` Stuck, what a missile lodged in its victim plays (`0x6fee00`).
const STUCK_ANIM: u16 = 191;

/// `AnimationData.dbc` the glancing missile's sequence (`0x702fc0`).
const GLANCE_ANIM: u16 = 198;

/// How long a blocked missile stays in the shield (`0x702fc0`).
const STUCK_SECS: f32 = 5.0;

/// The victim's ShieldBlock and Dodge clips, a miss's own (`0x7022d0`, `0x7030d7`).
const BLOCK_CLIP: u16 = 24;
const DODGE_CLIP: u16 = 30;

/// The burst a resisted missile leaves, hardcoded index 10 (`0x70318d`).
const RESIST_EFFECT: &str = "HARDCODED Resist Spell";

/// `AnimationData.dbc` InFlight, the sequence a projectile model plays in flight: a thrown weapon
/// tumbles end over end and shows its trail only here; a model without it plays its first clip.
const INFLIGHT_ANIM: u16 = 144;

/// The missile's orientation for flight direction `f`, the reference's frame: model +X along the
/// flight, side = up × dir, up = dir × side, so it never rolls however the flight pitches. In Bevy,
/// local −Z (wow +X) maps to `f` and local +Y (wow +Z) to that up.
fn missile_facing(f: Vec3) -> Quat {
    let side = Vec3::Y.cross(f);
    if side.length_squared() < 1e-6 {
        // Straight up or down: any roll will do.
        return Quat::from_rotation_arc(-Vec3::Z, f);
    }
    let up = f.cross(side.normalize()).normalize();
    // Columns are the images of local X, Y, Z: `f × up` (−side, the image of wow +Y), `up`, `−f`.
    Quat::from_mat3(&Mat3::from_cols(f.cross(up), up, -f))
}

/// The projectile's looping flight sound, `SpellVisual` field 10: the reference's first flight step
/// starts it and every later step moves it with the missile. Written by [`spawn_missiles`] and
/// [`move_missiles`] for `crate::sound::missile`.
#[derive(Message, Clone, Copy)]
pub(crate) enum MissileSound {
    /// Start `kit_sound` on the launched `entity`, born at `pos`, the launch point, since the new
    /// entity's `Transform` may not have flushed yet.
    Start {
        entity: Entity,
        kit_sound: u32,
        pos: Vec3,
    },
    /// The projectile arrived or streamed out.
    Stop { entity: Entity },
}

/// What a projectile flies at, the reference's two missile kinds: an owning unit slot, or −1 for
/// the location fallback.
#[derive(Clone, Copy)]
enum Aim {
    /// A unit target, homed to its live attachment, so a moving target bends the path.
    Unit {
        target: Entity,
        /// The attachment it homes to (`SpellVisual` column 10); `None` is the base.
        dest_tag: Option<u16>,
        /// The impact offset in that attachment's frame, Bevy model space (`0x6ff6d0`).
        offset: Vec3,
        /// The wire's `SpellMissInfo` code for a miss, which arrives as the victim's defense clip
        /// instead of the impact; `None` for a hit.
        miss: Option<u8>,
    },
    /// The GO's destination point; arrival is the ground hand-off on the caster.
    Ground(Vec3),
}

/// One projectile in flight.
#[derive(Component)]
pub(super) struct Missile {
    spell_id: u32,
    /// A ground arrival plays on the caster, a unit arrival on the target.
    caster: Entity,
    aim: Aim,
    /// The [`SpellFx`] cache key; `None` flies invisible and still impacts.
    path: Option<String>,
    /// Remaining flight time in seconds, the arrive-on-time integrator's state.
    arrive_in: f32,
    /// The flight time at launch, a scripted arc's progress clock (`0x7015d0`).
    flight_time: f32,
    parts_spawned: bool,
    /// A trajectory GO's flight.
    flight: Option<Flight>,
    /// The flight's own position; the model draws here plus a motion script's offset.
    pos: Vec3,
    /// Where the flight left from, and the aim of its last step.
    launch: Vec3,
    impact: Vec3,
    /// The share of the flight flown, in 3D and over the ground (`0x6ffed0`).
    progress: f32,
    h_progress: f32,
    /// `Spell.dbc` Speed, which a motion script's scalar and offset are relative to.
    base_speed: f32,
    /// A motion script's speed: the flight then closes at it, not on its deadline (`0x7004cd`).
    speed_override: Option<f32>,
    /// Columns 17-20 when the flight hugs the ground.
    follow: Option<FollowGround>,
    /// The model turns with its own motion, not toward the aim (`0x701230`).
    face: Option<FaceMotion>,
    /// A motion script moved the model off its path: the hit kit plays at the model (`0x702e5a`).
    off_path: bool,
    /// The copy's index among a motion's copies; only copy 0 floats a miss word (`0x703017`).
    index: u32,
    /// The unit wearing the targeting kit, reaped when the flight ends (`0x6ff410`).
    targeting: Option<Targeting>,
    /// A motion script's last offset, turn and scale for the model (`0x700550`, `0x6feb20`).
    pose: Option<MotionPose>,
    /// How a miss ends; `None` for a hit.
    kind: Option<MissKind>,
    /// The outcome on the caster of the flight a reflect sends back; 0 is a hit.
    reflect_outcome: u8,
    /// Whether the miss's word and log have gone out (`0x100`).
    spent: bool,
    /// A flight no longer homing: past the target or glancing away.
    fixed_aim: Option<Vec3>,
    /// The last stretch of a fixed flight the model fades over, yards (`0x6ffcd0`).
    fade_over: Option<f32>,
    /// Seconds until the victim's defense clip, and the state that plays it (`0x7015d0`).
    victim_clip: Option<(f32, u32)>,
    /// The flight's yards per second, as launched.
    speed: f32,
    /// The time this cast was launched, a reflect's clock.
    cast_at: f32,
    /// Column 22's kit, re-armed when a reflect turns the flight.
    targeting_kit: u32,
    /// A targeting kit's models held at a ground aim, reaped with the flight (`0x702941`).
    targeting_points: Vec<Entity>,
    /// The carry along the target's transport (`0x6ffd70`).
    deck: DeckChase,
    /// The launch point rides the deck as well, a motion or follow flight (`0x7015d0`).
    launch_rides_deck: bool,
    /// The model's `SpellVisualEffectName` scale (`0x701230`).
    scale: f32,
    /// The targets it carries, and the listed unit it is on.
    list: Option<Box<ListFlight>>,
    /// A ground aim local to a transport, re-placed through it every step (`0x6ff320`).
    aim_deck: Option<(Entity, Vec3)>,
    /// The flight loop, restarted by each new flight off a landing (`0x702a3f`).
    sound: Option<u32>,
}

/// The targets one missile carries (`0x733600`): it lands on them at its point, or hops them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TargetList {
    pub(crate) units: Vec<ListedUnit>,
    /// The outcomes came with the list: a landing words its misses and plays no kits (`0x700fc3`).
    pub(crate) results: bool,
    /// Flag `0x200`: a unit landing hops on to the next listed unit (`0x702a60`).
    pub(crate) bounce: bool,
    /// The extra points; a landing plays the target kit at each (`0x701150`).
    pub(crate) points: Vec<DeckPoint>,
}

/// An extra target point (`0x80e44f`): local to its transport when it has one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DeckPoint {
    /// Bevy coords, local to `deck` when set.
    pub(crate) at: Vec3,
    pub(crate) deck: Option<Entity>,
    /// The world point at the GO, kept once the transport is gone (`0x80e570`).
    pub(crate) world: Vec3,
}

impl DeckPoint {
    /// Where it is now: through its transport while that is here, else its world point.
    pub(crate) fn place(&self, decks: &Query<&GlobalTransform>) -> Vec3 {
        self.deck
            .and_then(|d| decks.get(d).ok())
            .map_or(self.world, |g| deck_place(self.at, g))
    }
}

/// A transport-local point in the world: the transport's turn, then its place (`0x6ff320`).
pub(crate) fn deck_place(local: Vec3, deck: &GlobalTransform) -> Vec3 {
    deck.translation() + deck.rotation() * local
}

/// One listed target, resolved by guid where the missile meets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ListedUnit {
    pub(crate) guid: u64,
    /// Its `SpellMissInfo`, 0 for a hit.
    pub(crate) result: u8,
    /// A reflect's outcome on the caster.
    pub(crate) reflect: u8,
}

/// A missile's list and the unit it is on (`0x702a60`), with what a hop re-aims by.
#[derive(Clone, Debug)]
struct ListFlight {
    list: TargetList,
    current: usize,
    dest_tag: Option<u16>,
    offset: Vec3,
    reflect_glances: bool,
}

/// A blocked missile lodged in its victim: it rides the attachment until it expires (`0x6fee00`).
#[derive(Component)]
pub(super) struct StuckMissile {
    target: Entity,
    tag: u16,
    /// Its place and turn against the attachment and the victim's facing.
    local: Vec3,
    turn: Quat,
    until: f32,
    spell_id: u32,
    targeting: Option<Targeting>,
}

/// A targeting kit a unit wears for a flight, with the fade stamp its clear matches (`0x6ff410`).
#[derive(Clone, Copy, Debug)]
struct Targeting {
    unit: Entity,
    fade: u32,
}

/// A flight's fade stamp: the fire time in ms its targeting kit is played and cleared by.
fn fade_stamp(now: f32) -> u32 {
    (now * 1000.0) as u32
}

/// Clear a targeting kit off its unit by spell and fade stamp: its models alone carry the stamp,
/// so its beams and anim rate stay (`0x6ff410`, `0x744cc8`).
fn reap_targeting(t: Targeting, spell_id: u32, kit_fx: &mut MessageWriter<SpellKitFx>) {
    kit_fx.write(SpellKitFx::Reap {
        entity: t.unit,
        spell_id,
        class: FxClass::Targeting(t.fade),
    });
}

/// How a motion script draws the model against its flight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MotionPose {
    /// Off the flight's position, Bevy space.
    pub(crate) offset: Vec3,
    /// In the model's frame, after it faces its motion.
    pub(crate) turn: Quat,
    pub(crate) scale: f32,
}

/// A trajectory missile's flight state.
struct Flight {
    /// `None` when no arc solves: the missile flies straight and lands on time.
    arc: Option<trajectory::Arc>,
    /// The caster and cast id a server hit names the missile by (`0x6ff2c0`).
    caster: u64,
    cast_id: u8,
    /// The server's hit point (`SMSG_SET_PROJECTILE_POSITION`).
    remote: Option<Vec3>,
    collider: Collider,
}

/// A trajectory missile's unit collision (`0x6fcb60`).
enum Collider {
    Off,
    /// Built on the first flight step, from the units then near the path.
    Pending {
        flags: u32,
        radius: f32,
        impact: Vec3,
    },
    Units {
        units: Vec<Entity>,
        dir: Vec2,
        radius: f32,
    },
}

/// The unit-side inputs of an attachment or marker position read: the base frame, the tables and
/// the pose, read through `RigPose::posed_point` without an anchor entity.
pub(crate) type AttachPosQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        Option<&'static BoneAttach>,
        Option<&'static benilla_world::rig_anim::RigPose>,
    ),
>;

/// A unit's attachment position through the tag cascade with `offset` (Bevy model space) in the
/// attachment's frame, or turned with the unit at its base; `None` only when the unit is gone.
fn attach_world_pos_offset(
    unit: Entity,
    tags: impl IntoIterator<Item = u16>,
    offset: Vec3,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(unit).ok()?;
    let point = bones.zip(pose).and_then(|(b, p)| {
        let (bone, at) = tags
            .into_iter()
            .find_map(|tag| b.points.get(&tag).copied())?;
        p.posed_point(joints.get(p.joints_root).ok()?, bone, at + offset)
    });
    Some(point.unwrap_or_else(|| base.translation() + base.rotation() * offset))
}

/// This frame's aim; `hug` sets a unit aim's height over the target's feet (`0x6ff7e8`).
fn aim_point(
    aim: Aim,
    hug: Option<f32>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    match aim {
        Aim::Unit {
            target,
            dest_tag,
            offset,
            ..
        } => {
            let mut at = attach_world_pos_offset(
                target,
                dest_tag.into_iter().chain(DEST_FALLBACKS),
                offset,
                units,
                joints,
            )?;
            if let Some(h) = hug {
                at.y = units.get(target).ok()?.0.translation().y + h;
            }
            Some(at)
        }
        Aim::Ground(pos) => Some(pos),
    }
}

/// The launch override's point (`0x720bf0`), `None` to keep the release point.
fn source_world_pos(
    caster: Entity,
    source: MissileSource,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    weapons: &HeldWeapons,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(caster).ok()?;
    let (bones, pose) = bones.zip(pose)?;
    let root = joints.get(pose.joints_root).ok()?;
    let at = |(bone, at): (u16, Vec3)| pose.posed_point(root, bone, at + source.offset);
    match source.tag {
        Some(tag) => at(weapons.effect_point(caster, bones, tag)?),
        None => bones
            .points
            .get(&shape::SPELL_HAND_DIRECTED)
            .copied()
            .and_then(at)
            .or_else(|| Some(base.translation() + base.affine().transform_vector3(source.offset))),
    }
}

/// Where one queued GO leaves from: its launch override, else the release point.
fn fire_pos(
    go: &QueuedGo,
    release: Option<Vec3>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    weapons: &HeldWeapons,
) -> Option<Vec3> {
    go.spawn
        .shape
        .source
        .and_then(|s| source_world_pos(go.spawn.caster, s, units, joints, weapons))
        .or(release)
}

/// The caster's launch point: the fired release event's own marker, as the dispatcher passes the
/// event's live position, else the cascade at this pose.
fn launch_world_pos(
    caster: Entity,
    fired: Option<[u8; 4]>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(caster).ok()?;
    let point = bones.zip(pose).and_then(|(b, p)| {
        let (bone, offset) = fired
            .into_iter()
            .chain(MARKER_CASCADE)
            .find_map(|ident| b.markers.get(&ident).copied())?;
        p.posed_point(joints.get(p.joints_root).ok()?, bone, offset)
    });
    Some(point.unwrap_or_else(|| base.translation()))
}

/// Where an aim launches from (`0x6fcd60`): the model's `$AIM` event, else the cast markers and
/// the unit's base (`0x71a720`).
pub(crate) fn aim_launch_pos(
    caster: Entity,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    launch_world_pos(caster, Some(*b"$AIM"), units, joints)
}

/// One GO's projectiles parked on the caster until the release keyframe, a node of the reference's
/// `unit+0xac` list; its model loads from queue time, as the reference creates the M2 there.
struct QueuedGo {
    spawn: MissileSpawn,
    /// The [`SpellFx`] key that becomes [`Missile::path`].
    key: Option<String>,
    /// Seconds queued, subtracted from the flight at launch (the reference's `elapsed` term).
    queued: f32,
    /// A one-shot has played on the caster since queuing, which arms the anim-end flush.
    saw_oneshot: bool,
    /// Its first missile took the caster's held model (`0x73304d`).
    takes_hold: bool,
}

/// Every caster's queue of projectiles awaiting release, the reference's unit `+0xac` list of
/// missile nodes, filled by the spawner and drained by the release event; a caster that streams
/// out drops its queue. It is also a gate: the `$BWR` handler tests the list head and only when a
/// projectile waits re-animates the ranged prop, looks up the attachment and launches, so a shot's
/// flex and launch are one act.
#[derive(Resource, Default)]
pub(crate) struct PendingMissiles(EntityHashMap<Vec<QueuedGo>>);

impl PendingMissiles {
    /// Whether a projectile waits on `caster` for its release keyframe, the reference's list-head
    /// test; [`crate::ranged_flex`] asks it before the drain, in the reference's order.
    pub(crate) fn releasing(&self, caster: Entity) -> bool {
        self.0.get(&caster).is_some_and(|q| !q.is_empty())
    }

    /// Whether a projectile of `spell_id` waits on `caster`, the applied core's skip (`0x724b92`).
    pub(crate) fn queues(&self, caster: Entity, spell_id: u32) -> bool {
        self.0
            .get(&caster)
            .is_some_and(|q| q.iter().any(|g| g.spawn.spell_id == spell_id))
    }

    /// Queues one projectile on `caster` for tests of the gate's consumers; only its presence
    /// matters.
    #[cfg(test)]
    pub(crate) fn queue_a_shot(app: &mut bevy::app::App, caster: Entity) {
        let spawn = MissileSpawn {
            caster,
            spell_id: 75,
            path: None,
            ammo_display_id: None,
            dest_tag: None,
            speed: 40.0,
            targets: Vec::new(),
            reflects: Vec::new(),
            reflect_glances: false,
            scale: 1.0,
            ground_aim: None,
            missile_sound: None,
            awaits_release: true,
            trajectory: None,
            shape: MissileShape::default(),
            from: None,
            list: None,
            aim_deck: None,
            seq: 0,
        };
        app.world_mut()
            .resource_mut::<Self>()
            .0
            .entry(caster)
            .or_default()
            .push(QueuedGo {
                spawn,
                key: None,
                queued: 0.0,
                saw_oneshot: false,
                takes_hold: false,
            });
    }
}

/// The ammo display's flight model by the shape rule (module docs), as a [`SpellFx`] entry keyed
/// `ammo:<display id>`, so two displays sharing a model but not a skin never collide.
fn ensure_ammo_model(
    fx: &mut SpellFx,
    displays: &ItemDisplays,
    display_id: u32,
    asset_server: &AssetServer,
) -> Option<String> {
    let key = format!("ammo:{display_id}");
    if !fx.models.contains_key(&key) {
        let row = displays.catalog.get(display_id)?;
        let (dir_name, col) = if row.model[0].is_some() {
            ("Weapon", 0)
        } else {
            ("Ammo", 1)
        };
        let model = row.model[col].as_ref()?;
        let dir = format!("Item\\ObjectComponents\\{dir_name}");
        let dm = DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(&format!("{dir}\\{model}")))),
            object_texture: row.model_texture[col].clone(),
            dir,
            ..super::empty_shell()
        };
        fx.models.insert(key.clone(), dm);
    }
    Some(key)
}

/// A travelling spell's deferred miss word. `SMSG_SPELL_GO` skips its inline word when the
/// `Spell.dbc` Speed is nonzero; the arrival re-resolves the spell from `[missile+0x18]` and calls
/// the same emitter. Its source class, CVar gates and colour are
/// `crate::combat_text::missile_miss_text`'s.
#[derive(Message, Clone, Copy)]
pub(crate) struct MissileMiss {
    /// The colour law's source class (`K`).
    pub(crate) caster: Entity,
    /// Always the missed target; only the no-target arm's words take each list entry's target.
    /// Unlike the inline site the arrival floats Reflect over the target.
    pub(crate) anchor: Entity,
    /// The colour law's `B` bit resolves off its record.
    pub(crate) spell_id: u32,
    /// The wire's `SpellMissInfo` code (1–11).
    pub(crate) code: u8,
}

/// The melee `$CPP` dispatch's victim states ([`DefenseAnim`]) that play Dodge and ShieldBlock.
const DODGE_STATE: u32 = 2;
const BLOCK_STATE: u32 = 5;

/// What a launch or a landing emits, one parameter as both systems sit at Bevy's ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ArrivalOut<'w, 's> {
    impacts: MessageWriter<'w, CastEvent>,
    defenses: MessageWriter<'w, DefenseAnim>,
    words: MessageWriter<'w, MissileMiss>,
    /// The targeting kit's play, armed at launch and cleared at the flight's end.
    kit: crate::creature_anim::UnitKitLanes<'w>,
    bursts: MessageWriter<'w, super::dest_fx::GroundBurst>,
    visuals: Option<Res<'w, SpellVisuals>>,
    clips: Query<'w, 's, &'static benilla_assets::ModelAnimations>,
    /// Where a beam spell's flag-0x1 targeting kit waits, and what it needs to tell.
    waits: Option<ResMut<'w, crate::creature_anim::BeamKitWaits>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    kinds: Query<'w, 's, &'static crate::net::NetEntity>,
    time: Res<'w, Time>,
    /// A listed guid's unit, looked up where the missile meets it.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    /// Our own player, which plays an area landing whose caster is gone (`0x700ed9`).
    me: Query<'w, 's, Entity, With<crate::net::SelfPlayer>>,
    /// What is placed in the world, and where.
    placed: Query<'w, 's, &'static GlobalTransform>,
}

impl ArrivalOut<'_, '_> {
    /// A listed guid's unit; an object of another type names none (`0x700fb1`).
    fn listed_unit(&self, guid: u64) -> Option<Entity> {
        let unit = *self.index.as_deref()?.0.get(&guid)?;
        let other = self.kinds.get(unit).is_ok_and(|n| !is_unit_kind(n.kind));
        (!other && self.placed.contains(unit)).then_some(unit)
    }
}

/// What an arrival leaves to the flight.
enum AfterArrival {
    End,
    Stick,
    Glance,
    Reflect,
}

/// The arrival hand-off by the reference's per-tick arms; `model` is where it drew last and
/// `turn` its facing.
fn arrival_handoff(
    missile: &Missile,
    at: Vec3,
    model: Vec3,
    turn: Quat,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) -> AfterArrival {
    if missile.spent {
        return AfterArrival::End;
    }
    // A fresh tick sorts the arrival after the frame's packet handlers, as the reference plays it.
    let mut event = |entity, kind| CastEvent {
        entity,
        spell_id: missile.spell_id,
        kind,
        cast_count: 0,
        seq: play_seq.next(),
    };
    match missile.aim {
        Aim::Unit {
            target, miss: None, ..
        } => {
            // At the model if scripted off its path, at the aim if hugging (`0x702e5a`).
            let at = if missile.off_path {
                Some(model)
            } else {
                missile.follow.filter(|f| f.hugs_target()).map(|_| at)
            };
            let kind = CastEventKind::Impact {
                on_caster: target == missile.caster,
                at,
                caster: missile.caster,
            };
            out.impacts.write(event(target, kind));
        }
        Aim::Unit {
            target,
            miss: Some(code),
            ..
        } => {
            if missile.index == 0 {
                out.words.write(MissileMiss {
                    caster: missile.caster,
                    anchor: target,
                    spell_id: missile.spell_id,
                    code,
                });
            }
            // The dodge a resist and a reflect draw on arrival (`0x7030d7`).
            if matches!(missile.kind, Some(MissKind::Resisted | MissKind::Reflect)) {
                out.defenses.write(DefenseAnim {
                    victim: target,
                    victim_state: DODGE_STATE,
                });
            }
            match missile.kind {
                Some(MissKind::Stuck) => return AfterArrival::Stick,
                Some(MissKind::Glance) => return AfterArrival::Glance,
                Some(MissKind::Reflect) => return AfterArrival::Reflect,
                Some(MissKind::Resisted) => resist_burst(at, turn, out),
                Some(MissKind::Wide) | None => {}
            }
        }
        Aim::Ground(_) => land_on_area(missile, at, out, play_seq),
    }
    AfterArrival::End
}

/// A landing with no unit to hit (`0x700e20`): the impact area kit at `at` through the caster, or
/// our own player when it is gone; then, for a spell with a visual row (`0x703591`), each listed
/// unit's impact and state kits, or with outcomes its miss word.
fn land_on_area(
    missile: &Missile,
    at: Vec3,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) {
    let landing = AreaLanding {
        caster: missile.caster,
        spell_id: missile.spell_id,
        list: missile.list.as_deref().map(|l| &l.list),
        words: !missile.spent && missile.index == 0,
    };
    landing.land(at, out, play_seq);
}

/// Who and what an area landing plays.
struct AreaLanding<'a> {
    caster: Entity,
    spell_id: u32,
    list: Option<&'a TargetList>,
    /// The landing may word its misses: copy 0 alone, and once (`0x700fe3`, `0x700ff1`).
    words: bool,
}

impl AreaLanding<'_> {
    fn land(&self, at: Vec3, out: &mut ArrivalOut, play_seq: &mut crate::creature_anim::PlaySeq) {
        let (caster, spell_id) = (self.caster, self.spell_id);
        let mut event = |entity, kind| CastEvent {
            entity,
            spell_id,
            kind,
            cast_count: 0,
            seq: play_seq.next(),
        };
        let host = if out.placed.contains(caster) {
            Some(caster)
        } else {
            out.me.iter().next()
        };
        if let Some(host) = host {
            out.impacts
                .write(event(host, CastEventKind::GroundImpact { pos: at }));
        }
        let Some(list) = self.list else {
            return;
        };
        let has_visual = out
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .zip(out.visuals.as_deref())
            .is_some_and(|(d, v)| v.0.stages(d.visual).is_some());
        if !has_visual {
            return;
        }
        for u in &list.units {
            let Some(unit) = out.listed_unit(u.guid) else {
                continue;
            };
            if !list.results {
                out.impacts
                    .write(event(unit, CastEventKind::AreaImpact { caster }));
            } else if u.result != 0 && self.words {
                out.words.write(MissileMiss {
                    caster,
                    anchor: unit,
                    spell_id,
                    code: u.result,
                });
            }
        }
        // Each point plays the target kit there through the caster, none with it gone (`0x701150`).
        if out.placed.contains(caster) {
            for p in &list.points {
                let pos = p.place(&out.placed);
                out.impacts
                    .write(event(caster, CastEventKind::PointImpact { pos }));
            }
        }
    }
}

/// What a hop off a listed unit did (`0x702a60`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hop {
    /// Past the list's end: the landing stands.
    Done,
    /// Re-aimed at the next listed unit.
    On,
    /// Every unit left names none, so it lands on the area at once (`0x7023fe`).
    Stranded,
}

/// Hop on to the next listed unit from the landing, at the flight's own speed and with that unit's
/// outcome (`0x702a60`, `0x7022d0`); a guid naming no unit is skipped.
fn hop(
    m: &mut Missile,
    now: f32,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    out: &mut ArrivalOut,
) -> Hop {
    let Some(l) = m.list.as_deref_mut().filter(|l| l.list.bounce) else {
        return Hop::Done;
    };
    let mut skipped = false;
    let (unit, next) = loop {
        l.current += 1;
        let Some(&next) = l.list.units.get(l.current) else {
            return if skipped { Hop::Stranded } else { Hop::Done };
        };
        match out.listed_unit(next.guid) {
            Some(unit) => break (unit, next),
            None => skipped = true,
        }
    };
    let route = (next.result != 0).then(|| disposition::classify(next.result, l.reflect_glances));
    let kind = route.map(|(kind, _)| kind);
    let aim = Aim::Unit {
        target: unit,
        dest_tag: if kind == Some(MissKind::Stuck) {
            Some(disposition::SHIELD_ATTACHMENT)
        } else {
            l.dest_tag
        },
        offset: l.offset,
        miss: route.map(|(_, code)| code),
    };
    let Some(at) = aim_point(aim, None, units, joints) else {
        return Hop::Stranded;
    };
    if let Some(t) = m.targeting.take() {
        reap_targeting(t, m.spell_id, &mut out.kit.fx);
    }
    let from = m.impact;
    m.aim = aim;
    m.kind = kind;
    m.reflect_outcome = next.reflect;
    // The hop clears the spent, homing and route bits (`0x702a9f`).
    m.spent = false;
    m.pos = from;
    m.launch = from;
    m.progress = 0.0;
    m.h_progress = 0.0;
    m.arrive_in = from.distance(at) / m.speed.max(f32::EPSILON);
    m.fixed_aim = None;
    m.fade_over = None;
    if kind == Some(MissKind::Wide) {
        m.fixed_aim = Some(disposition::wide_aim(from, at, m.speed, signed_random()));
        m.arrive_in += disposition::PAST_TARGET_SECS;
        m.fade_over = Some(m.speed * disposition::PAST_TARGET_SECS);
    }
    m.flight_time = m.arrive_in;
    // A hop times no victim clip (`0x7025d8`).
    m.victim_clip = None;
    m.deck = DeckChase::default();
    if m.targeting_kit != 0 {
        let fade = fade_stamp(now);
        m.targeting = arm_targeting_kit(unit, m.caster, m.spell_id, m.targeting_kit, fade, out);
    }
    Hop::On
}

/// The resist burst at the landing, at the missile's facing and its row's scale (`0x703191`).
fn resist_burst(at: Vec3, turn: Quat, out: &mut ArrivalOut) {
    let Some(visuals) = out.visuals.as_deref() else {
        return;
    };
    let Some((effect, path)) = visuals.0.hardcoded_effect(RESIST_EFFECT) else {
        return;
    };
    let scale = visuals
        .0
        .effect_scale(effect)
        .map_or(1.0, |s| s.planted(1.0));
    out.bursts.write(super::dest_fx::GroundBurst {
        path: path.to_string(),
        pos: at,
        attach: None,
        scale,
        turn: Quat::from_rotation_y(turn.to_euler(EulerRot::YXZ).0),
        held_by: None,
    });
}

/// Play sequence `anim` on the missile's model, if it has one: applied after the frame's systems.
fn play_missile_sequence(commands: &mut Commands, entity: Entity, key: String, anim: u16) {
    commands.entity(entity).queue(move |mut e: EntityWorldMut| {
        let node = e
            .world()
            .get_resource::<SpellFx>()
            .and_then(|fx| fx.models.get(&key))
            .and_then(|dm| dm.animations.as_ref())
            .and_then(|a| a.find(anim))
            .map(|c| c.node);
        if let (Some(node), Some(mut player)) = (node, e.get_mut::<AnimationPlayer>()) {
            player.stop_all();
            player.play(node);
        }
    });
}

/// A victim's defense clip for this route, due half its length before the landing (`0x7022d0`):
/// `Some` to count down, written at once when already due; only a modelled flight plays one.
fn victim_clip(
    target: Entity,
    kind: Option<MissKind>,
    arrive_in: f32,
    modelled: bool,
    out: &mut ArrivalOut,
) -> Option<(f32, u32)> {
    let (clip, state) = match kind {
        Some(MissKind::Stuck) => (BLOCK_CLIP, BLOCK_STATE),
        Some(MissKind::Glance) => (DODGE_CLIP, DODGE_STATE),
        _ => return None,
    };
    if !modelled {
        return None;
    }
    let half = out
        .clips
        .get(target)
        .ok()
        .and_then(|a| a.find(clip))
        .map_or(0.0, |c| c.duration * 0.5);
    let due = arrive_in - half;
    if due > 0.0 {
        return Some((due, state));
    }
    out.defenses.write(DefenseAnim {
        victim: target,
        victim_state: state,
    });
    None
}

/// Lodge a blocked missile in the victim's attachment (`0x6fee00`); `false` with no model on the
/// flight or no such attachment. The reference parents it to the attachment's whole frame; here it
/// turns with the unit, as the attachment's own rotation is not read.
fn stick(
    entity: Entity,
    missile: &Missile,
    transform: &Transform,
    now: f32,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    commands: &mut Commands,
) -> bool {
    let Some((target, tag, key)) = lodge(missile, units) else {
        return false;
    };
    let Ok((base, ..)) = units.get(target) else {
        return false;
    };
    let Some(anchor) = attach_world_pos_offset(target, [tag], Vec3::ZERO, units, joints) else {
        return false;
    };
    let facing = base.rotation().inverse();
    commands
        .entity(entity)
        .remove::<Missile>()
        .insert(StuckMissile {
            target,
            tag,
            local: facing * (transform.translation - anchor),
            turn: facing * transform.rotation,
            until: now + STUCK_SECS,
            spell_id: missile.spell_id,
            targeting: missile.targeting,
        });
    play_missile_sequence(commands, entity, key, STUCK_ANIM);
    true
}

/// Where a blocked missile lodges (`0x6fee00`): its victim, the attachment and its model's key;
/// `None` with no drawn model or no such attachment.
fn lodge(missile: &Missile, units: &AttachPosQuery) -> Option<(Entity, u16, String)> {
    let Aim::Unit {
        target, dest_tag, ..
    } = missile.aim
    else {
        return None;
    };
    let key = missile.path.clone().filter(|_| missile.parts_spawned)?;
    let (_, Some(bones), _) = units.get(target).ok()? else {
        return None;
    };
    let tag = dest_tag
        .into_iter()
        .chain(DEST_FALLBACKS)
        .find(|t| bones.points.contains_key(t))?;
    Some((target, tag, key))
}

/// Turn a parried missile away at half speed, fading out (`0x702fc0`); `false` straight up or down.
fn glance_off(entity: Entity, m: &mut Missile, commands: &mut Commands) -> bool {
    let [spin, tilt, _] = motion::draw_randoms().map(|r| r * 2.0 - 1.0);
    let Some(dir) = disposition::glance_dir(m.pos - m.impact, spin, tilt) else {
        return false;
    };
    m.speed *= 0.5;
    m.pos = m.impact;
    let reach = m.speed * disposition::PAST_TARGET_SECS;
    m.fixed_aim = Some(m.pos + dir * reach);
    m.fade_over = Some(reach);
    m.arrive_in = disposition::PAST_TARGET_SECS;
    m.flight_time = disposition::PAST_TARGET_SECS;
    m.spent = true;
    if let Some(key) = m.path.clone() {
        play_missile_sequence(commands, entity, key, GLANCE_ANIM);
    }
    true
}

/// Send a reflected missile back at its caster from where it reached (`0x702fc0`, `0x7022d0`).
fn reflect_back(
    m: &mut Missile,
    now: f32,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    out: &mut ArrivalOut,
) -> bool {
    let Aim::Unit {
        dest_tag, offset, ..
    } = m.aim
    else {
        return false;
    };
    let route = (m.reflect_outcome != 0).then(|| disposition::classify(m.reflect_outcome, false));
    let kind = route.map(|(kind, _)| kind);
    let aim = Aim::Unit {
        target: m.caster,
        dest_tag: if kind == Some(MissKind::Stuck) {
            Some(disposition::SHIELD_ATTACHMENT)
        } else {
            dest_tag
        },
        offset,
        miss: route.map(|(_, code)| code),
    };
    let Some(at) = aim_point(aim, None, units, joints) else {
        return false;
    };
    let from = m.impact;
    let distance = from.distance(at);
    let speed = disposition::relaunch_speed(m.speed, distance, now - m.cast_at);
    if let Some(t) = m.targeting.take() {
        reap_targeting(t, m.spell_id, &mut out.kit.fx);
    }
    m.aim = aim;
    m.kind = kind;
    m.reflect_outcome = 0;
    m.pos = from;
    m.launch = from;
    m.progress = 0.0;
    m.h_progress = 0.0;
    m.flight = None;
    m.speed = speed;
    m.arrive_in = distance / speed;
    m.fixed_aim = None;
    m.fade_over = None;
    if kind == Some(MissKind::Wide) {
        m.fixed_aim = Some(disposition::wide_aim(from, at, speed, signed_random()));
        m.arrive_in += disposition::PAST_TARGET_SECS;
        m.fade_over = Some(speed * disposition::PAST_TARGET_SECS);
    }
    m.flight_time = m.arrive_in;
    m.victim_clip = victim_clip(m.caster, kind, m.arrive_in, m.path.is_some(), out);
    if m.targeting_kit != 0 {
        let fade = fade_stamp(now);
        m.targeting = arm_targeting_kit(m.caster, m.caster, m.spell_id, m.targeting_kit, fade, out);
    }
    true
}

/// One draw in -1 to 1, as the reference's signed generator.
fn signed_random() -> f32 {
    motion::draw_randoms()[0] * 2.0 - 1.0
}

/// Move each lodged missile with its victim, and free it on expiry or when the victim is gone.
fn carry_stuck_missiles(
    now: f32,
    stuck: &mut Query<(Entity, &StuckMissile, &mut Transform), Without<Missile>>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    commands: &mut Commands,
    kit_fx: &mut MessageWriter<SpellKitFx>,
) {
    for (entity, s, mut transform) in stuck {
        let anchor = attach_world_pos_offset(s.target, [s.tag], Vec3::ZERO, units, joints);
        let held = anchor
            .filter(|_| now < s.until)
            .zip(units.get(s.target).ok());
        let Some((anchor, (base, ..))) = held else {
            if let Some(t) = s.targeting {
                reap_targeting(t, s.spell_id, kit_fx);
            }
            commands.entity(entity).despawn();
            continue;
        };
        transform.translation = anchor + base.rotation() * s.local;
        transform.rotation = base.rotation() * s.turn;
    }
}

/// End a flight: stop its loop, reap its targeting kit (`0x6ff410`) and despawn it.
fn end_flight(
    entity: Entity,
    missile: &Missile,
    commands: &mut Commands,
    sounds: &mut MessageWriter<MissileSound>,
    kit_fx: &mut MessageWriter<SpellKitFx>,
) {
    sounds.write(MissileSound::Stop { entity });
    for &point in &missile.targeting_points {
        commands.entity(point).despawn();
    }
    if let Some(t) = missile.targeting {
        reap_targeting(t, missile.spell_id, kit_fx);
    }
    commands.entity(entity).despawn();
}

/// Whether an entity kind is a unit, the type a kit play's owner lookup asks for.
fn is_unit_kind(kind: benilla_protocol::EntityKind) -> bool {
    matches!(
        kind,
        benilla_protocol::EntityKind::Player | benilla_protocol::EntityKind::Unit
    )
}

/// Play column 22's kit on the target at the fire, the whole stage-2 kit with the caster as its
/// guid, so its beams run from the caster (`0x702906`, `0x702923`).
fn arm_targeting_kit(
    target: Entity,
    caster: Entity,
    spell_id: u32,
    kit_id: u32,
    fade: u32,
    out: &mut ArrivalOut,
) -> Option<Targeting> {
    let visuals = out.visuals.as_deref()?;
    let kit = visuals.0.kit(kit_id)?;
    // On a unit, a beam spell's flag-0x1 kit waits for the beam to reach it (`0x72af60`).
    let is_unit = out.kinds.get(target).is_ok_and(|n| is_unit_kind(n.kind));
    if let (Some(waits), Some(spells)) = (out.waits.as_mut(), out.spells.as_deref()) {
        if is_unit
            && crate::creature_anim::spell_visual::kit_waits_for_beam(
                spells, &visuals.0, spell_id, kit,
            )
        {
            let now = out.time.elapsed_secs();
            waits
                .bypass_change_detection()
                .hold_targeting(target, spell_id, kit_id, None, now);
            // The replay carries no fade time, so the flight's end has nothing to reap (`0x6ff410`).
            return None;
        }
    }
    out.kit
        .play_targeting_kit(target, caster, spell_id, kit, fade, &visuals.0);
    Some(Targeting { unit: target, fade })
}

/// Hold column 22's kit at a flight's ground aim, on copy 0 alone (`0x702941`): its point models and
/// its sound there, the area subset of a kit played at a point (`0x74547b`).
fn arm_point_targeting_kit(
    caster: Entity,
    spell_id: u32,
    at: Vec3,
    kit_id: u32,
    commands: &mut Commands,
    out: &mut ArrivalOut,
) -> Vec<Entity> {
    let Some(visuals) = out.visuals.as_deref() else {
        return Vec::new();
    };
    let Some(kit) = visuals.0.kit(kit_id) else {
        return Vec::new();
    };
    // Only a unit caster plays it (`0x7028e3`).
    if !out.kinds.get(caster).is_ok_and(|n| is_unit_kind(n.kind)) {
        return Vec::new();
    }
    // The play is the caster's, so a beam spell's flag-0x1 kit waits for the beam to reach it with
    // its point (`0x72af60`).
    if let (Some(waits), Some(spells)) = (out.waits.as_mut(), out.spells.as_deref()) {
        if crate::creature_anim::spell_visual::kit_waits_for_beam(spells, &visuals.0, spell_id, kit)
        {
            let now = out.time.elapsed_secs();
            waits
                .bypass_change_detection()
                .hold_targeting(caster, spell_id, kit_id, Some(at), now);
            return Vec::new();
        }
    }
    if let Some(kit_sound) = kit.sound {
        out.kit
            .sounds
            .write(SpellKitSound::PlayAt { pos: at, kit_sound });
    }
    crate::creature_anim::area_kit_models(&visuals.0, kit)
        .map(|(path, attach, scale)| {
            super::dest_fx::spawn_held_point_fx(commands, path, at, attach, scale)
        })
        .collect()
}

/// Launch one queued GO from `launch`, per target and per motion copy (`0x7336b3`); `held`, the
/// held model's place, is where its first missile leaves from (`0x72e011`).
#[allow(clippy::too_many_arguments)] // the spawner's full output set
fn launch_go(
    go: &QueuedGo,
    launch: Vec3,
    mut held: Option<Vec3>,
    now: f32,
    commands: &mut Commands,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    sounds: &mut MessageWriter<MissileSound>,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) {
    let shape = go.spawn.shape;
    let unit_aim = |target: Entity, miss: Option<u8>| {
        let route = miss.map(|c| disposition::classify(c, go.spawn.reflect_glances));
        let kind = route.map(|(kind, _)| kind);
        let aim = Aim::Unit {
            target,
            // A blocked missile goes for the shield (`0x7022d0`).
            dest_tag: if kind == Some(MissKind::Stuck) {
                Some(disposition::SHIELD_ATTACHMENT)
            } else {
                go.spawn.dest_tag
            },
            offset: shape.impact_offset,
            miss: route.map(|(_, code)| code),
        };
        (aim, kind)
    };
    // A motion row with no function still sets the copies; one that will not compile flies plain.
    let motion = out
        .visuals
        .as_deref()
        .and_then(|v| v.0.missile_motion(shape.motion))
        .map(|m| m.copies());
    let copies = motion.unwrap_or(1);
    // Where a missile with no unit to home on lands on its area at once (`0x7023fe`), and whether
    // it carries the list.
    let mut lands_now: Vec<(Vec3, bool)> = Vec::new();
    let mut first = 0;
    let mut aim_deck = None;
    let aims: Vec<(Aim, Option<MissKind>)> = if !go.spawn.targets.is_empty() {
        go.spawn
            .targets
            .iter()
            .filter_map(|&(target, miss)| {
                // An object that is not a unit is not homed on: it lands at its place.
                if out.kinds.get(target).is_ok_and(|n| !is_unit_kind(n.kind)) {
                    let at = units.get(target).ok()?.0.translation();
                    lands_now.push((at, false));
                    return None;
                }
                Some(unit_aim(target, miss))
            })
            .collect()
    } else if let Some(p) = go.spawn.ground_aim {
        // One ground shot, its offset turned with the caster (`0x7335b2`); a points list's first
        // point takes none (`0x734191`).
        let turn = units
            .get(go.spawn.caster)
            .map_or(Quat::IDENTITY, |(g, ..)| g.rotation());
        let offset = if go.spawn.list.as_ref().is_some_and(|l| !l.points.is_empty()) {
            Vec3::ZERO
        } else {
            turn * shape.impact_offset
        };
        // A transport's point keeps the offset in its frame and follows it (`0x733820`).
        let at = match go.spawn.aim_deck {
            Some((deck, local)) => match joints.get(deck) {
                Ok(g) => {
                    aim_deck = Some((deck, local + offset));
                    deck_place(local + offset, g)
                }
                Err(_) => p + offset,
            },
            None => p + offset,
        };
        vec![(Aim::Ground(at), None)]
    } else if let Some(list) = go.spawn.list.as_ref().filter(|l| l.bounce) {
        // A hopping list starts at its first unit, past any guid naming none (`0x7023db`).
        let start = list
            .units
            .iter()
            .enumerate()
            .find_map(|(i, u)| Some((i, u.result, out.listed_unit(u.guid)?)));
        match start {
            Some((i, result, unit)) => {
                first = i;
                vec![unit_aim(unit, (result != 0).then_some(result))]
            }
            None => {
                // At the first listed object's place; the reference's unset aim is not aped.
                let at = list
                    .units
                    .first()
                    .and_then(|u| out.index.as_deref()?.0.get(&u.guid).copied())
                    .and_then(|e| units.get(e).ok())
                    .map_or(launch, |(g, ..)| g.translation());
                lands_now.push((at, true));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    for (at, listed) in lands_now {
        for index in 0..copies {
            let landing = AreaLanding {
                caster: go.spawn.caster,
                spell_id: go.spawn.spell_id,
                list: go.spawn.list.as_ref().filter(|_| listed),
                words: index == 0,
            };
            landing.land(at, out, play_seq);
        }
    }
    let flight_list = go.spawn.list.as_ref().map(|list| {
        Box::new(ListFlight {
            list: list.clone(),
            current: first,
            dest_tag: go.spawn.dest_tag,
            offset: shape.impact_offset,
            reflect_glances: go.spawn.reflect_glances,
        })
    });
    let hug = shape.follow.filter(|f| f.hugs_target()).map(|f| f.height);
    for (aim_at, kind) in aims {
        let Some(aim) = aim_point(aim_at, hug, units, joints) else {
            continue; // target already gone
        };
        // A flight past its target aims once and runs on (`0x6ff6d0`).
        let wide = kind == Some(MissKind::Wide);
        let flight_from = |from: Vec3| {
            // A trajectory lands at the packet's delay (`0x732ff0`), not at distance over speed.
            let arrive_in = match go.spawn.trajectory {
                Some((t, _)) => t.delay - go.queued,
                None => from.distance(aim) / go.spawn.speed.max(f32::EPSILON) - go.queued,
            };
            let speed = if arrive_in > 0.0 {
                from.distance(aim) / arrive_in
            } else {
                go.spawn.speed
            };
            let fixed_aim = wide.then(|| disposition::wide_aim(from, aim, speed, signed_random()));
            let fade_over = wide.then_some(speed * disposition::PAST_TARGET_SECS);
            let past = if wide {
                disposition::PAST_TARGET_SECS
            } else {
                0.0
            };
            (arrive_in + past, speed, fixed_aim, fade_over)
        };
        let marker_flight = flight_from(launch);
        let arrive_in = marker_flight.0;
        // The first missile made takes the held model, its motion copies new ones (`0x702b60`).
        let first_from = held.take();
        let targeting = match aim_at {
            Aim::Unit { target, .. } if shape.targeting_kit != 0 && arrive_in > 0.0 => {
                let fade = fade_stamp(now);
                arm_targeting_kit(
                    target,
                    go.spawn.caster,
                    go.spawn.spell_id,
                    shape.targeting_kit,
                    fade,
                    out,
                )
            }
            _ => None,
        };
        let mut targeting_points = match aim_at {
            Aim::Ground(at) if shape.targeting_kit != 0 && arrive_in > 0.0 => {
                let (caster, spell_id) = (go.spawn.caster, go.spawn.spell_id);
                arm_point_targeting_kit(caster, spell_id, at, shape.targeting_kit, commands, out)
            }
            _ => Vec::new(),
        };
        let reflect_outcome = match aim_at {
            Aim::Unit { target, .. } => go
                .spawn
                .reflects
                .iter()
                .find(|(e, _)| *e == target)
                .map_or(0, |&(_, r)| r),
            Aim::Ground(_) => 0,
        };
        let clip = match aim_at {
            Aim::Unit { target, .. } => victim_clip(target, kind, arrive_in, go.key.is_some(), out),
            Aim::Ground(_) => None,
        };
        for index in 0..copies {
            let (launch, (arrive_in, speed, fixed_aim, fade_over)) =
                match first_from.filter(|_| index == 0) {
                    Some(from) => (from, flight_from(from)),
                    None => (launch, marker_flight),
                };
            let flight = go.spawn.trajectory.map(|(t, rec)| {
                let arc = trajectory::launch(t.pitch, 0.0, launch, aim, rec.as_ref(), arrive_in);
                // A missile whose row collides with units tests its path (`0x700880`).
                let collider = match (arc, rec) {
                    (Some(_), Some(r)) if r.flags & 0x3C != 0 => Collider::Pending {
                        flags: r.flags,
                        radius: r.collision_radius,
                        impact: aim,
                    },
                    _ => Collider::Off,
                };
                Flight {
                    arc,
                    caster: t.caster,
                    cast_id: t.cast_id,
                    remote: None,
                    collider,
                }
            });
            let missile = Missile {
                spell_id: go.spawn.spell_id,
                caster: go.spawn.caster,
                aim: aim_at,
                path: go.key.clone(),
                arrive_in,
                flight_time: arrive_in,
                // An invisible flight has no parts to wait on.
                parts_spawned: go.key.is_none(),
                flight,
                pos: launch,
                launch,
                impact: aim,
                progress: 0.0,
                h_progress: 0.0,
                base_speed: go.spawn.speed,
                speed_override: None,
                follow: shape.follow,
                face: (shape.follow.is_some() || motion.is_some())
                    .then(|| FaceMotion::new(launch, aim)),
                off_path: false,
                index,
                targeting,
                pose: None,
                kind,
                reflect_outcome,
                spent: false,
                fixed_aim,
                fade_over,
                victim_clip: clip,
                speed,
                cast_at: now - go.queued,
                targeting_kit: shape.targeting_kit,
                targeting_points: if index == 0 {
                    std::mem::take(&mut targeting_points)
                } else {
                    Vec::new()
                },
                deck: DeckChase::default(),
                launch_rides_deck: motion.is_some() || shape.follow.is_some(),
                scale: go.spawn.scale,
                list: flight_list.clone(),
                aim_deck: aim_deck.filter(|_| matches!(aim_at, Aim::Ground(_))),
                sound: go.spawn.missile_sound,
            };
            let dir = (aim - launch).normalize_or(-Vec3::Z);
            // A route that flies on from its landing still takes a step (`0x7022d0`).
            let flies_on = matches!(kind, Some(MissKind::Glance | MissKind::Reflect));
            let mut missile = missile;
            if missile.arrive_in <= 0.0 && !flies_on {
                arrival_handoff(&missile, aim, aim, missile_facing(dir), out, play_seq);
                // A listed flight hops on from its first unit in the same step (`0x702a60`).
                missile.impact = aim;
                match hop(&mut missile, now, units, joints, out) {
                    Hop::On => {}
                    Hop::Done => continue,
                    Hop::Stranded => {
                        land_on_area(&missile, aim, out, play_seq);
                        continue;
                    }
                }
            }
            let scale = Vec3::splat(missile.scale);
            let mut entity = commands.spawn((
                missile,
                Transform::from_translation(launch)
                    .with_rotation(missile_facing(dir))
                    .with_scale(scale),
                Visibility::default(),
            ));
            if motion.is_some() {
                entity.insert(MissileMotion {
                    id: shape.motion,
                    index,
                    count: copies,
                    rand: motion::draw_randoms(),
                    fired_at: now,
                    start_distance: launch.distance(aim),
                });
            }
            let entity = entity.id();
            // The flight loop starts at launch and rides each copy, even an invisible one.
            if let Some(kit_sound) = go.spawn.missile_sound {
                sounds.write(MissileSound::Start {
                    entity,
                    kit_sound,
                    pos: launch,
                });
            }
        }
    }
}

/// Mark a new missile whose spell forces its projected batches (the missile's attribute test),
/// before its parts attach.
pub(super) fn mark_forced_missiles(
    mut commands: Commands,
    missiles: Query<(Entity, &Missile), Added<Missile>>,
    spells: Option<Res<crate::ui_action::Spells>>,
) {
    for (entity, missile) in &missiles {
        if super::spell_fx::spell_forces_projection(spells.as_deref(), missile.spell_id) {
            commands
                .entity(entity)
                .insert(benilla_world::model_render::ForcedProjection);
        }
    }
}

/// The casters' held missile models (`SpellVisual` flag `0x20`): a missile made after the hold
/// takes it, and every launch drops it.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PrecastHolds<'w, 's> {
    held: Query<'w, 's, &'static mut super::spell_fx::FxAttached>,
    emitters: Query<'w, 's, &'static mut benilla_world::particles::ParticleEmitter>,
}

impl PrecastHolds<'_, '_> {
    /// Whether a missile of `caster` made at stamp `seq` takes the held model, which then is
    /// taken (`0x73304d`).
    fn hand(&mut self, caster: Entity, seq: u64) -> bool {
        // Read first: a mutable borrow would mark every caster's effect list changed.
        if !self.held.get(caster).is_ok_and(|a| a.takes_precast(seq)) {
            return false;
        }
        if let Ok(mut a) = self.held.get_mut(caster) {
            a.hand_precast();
        }
        true
    }

    /// Where the taken model is, the place its missile leaves from (`0x72e011`).
    fn handed_place(&self, caster: Entity, joints: &Query<&GlobalTransform>) -> Option<Vec3> {
        let root = self.held.get(caster).ok()?.handed_precast_root()?;
        joints.get(root).ok().map(GlobalTransform::translation)
    }

    /// Drop `caster`'s held model `which` names at once, with no decay.
    fn drop(&mut self, caster: Entity, which: PrecastDrop, commands: &mut Commands) {
        if !self.held.get(caster).is_ok_and(|a| a.holds_precast()) {
            return;
        }
        let Ok(mut a) = self.held.get_mut(caster) else {
            return;
        };
        if let Some(root) = a.release_precast(which) {
            super::spell_fx::end_root(commands, &mut self.emitters, root);
        }
    }
}

/// The router's missile edges, the GOs' spawns and the releases, and the held models they move.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct MissileEdges<'w, 's> {
    spawns: MessageReader<'w, 's, MissileSpawn>,
    releases: MessageReader<'w, 's, MissileRelease>,
    precast: PrecastHolds<'w, 's>,
}

/// The parts of [`spawn_missiles`] that launch a caster's queue.
struct Launcher<'a, 'w, 's, 'j> {
    now: f32,
    units: &'a AttachPosQuery<'w, 's>,
    joints: &'a Query<'w, 's, &'j GlobalTransform>,
    weapons: &'a HeldWeapons<'w, 's>,
}

impl Launcher<'_, '_, '_, '_> {
    /// Launch `gos` from `release`, the GO that took the held model from its place, then drop the
    /// hold `which` names: the launch's tail (`0x72e1ed`).
    #[allow(clippy::too_many_arguments)] // the spawner's full output set
    fn release(
        &self,
        caster: Entity,
        gos: &[QueuedGo],
        release: Option<Vec3>,
        which: Option<PrecastDrop>,
        precast: &mut PrecastHolds,
        commands: &mut Commands,
        sounds: &mut MessageWriter<MissileSound>,
        out: &mut ArrivalOut,
        play_seq: &mut crate::creature_anim::PlaySeq,
    ) {
        let held = precast.handed_place(caster, self.joints);
        for go in gos {
            let Some(launch) = fire_pos(go, release, self.units, self.joints, self.weapons) else {
                continue; // the caster is already gone
            };
            launch_go(
                go,
                launch,
                held.filter(|_| go.takes_hold),
                self.now,
                commands,
                self.units,
                self.joints,
                sounds,
                out,
                play_seq,
            );
        }
        if let Some(which) = which {
            precast.drop(caster, which, commands);
        }
    }
}

/// Takes the router's [`MissileSpawn`]s and [`MissileRelease`]s in wire order and runs the pending
/// queue (module docs): park, drain on the release keyframe or a release, or launch at once, and
/// flush a queue whose keyframe never comes.
pub(super) fn spawn_missiles(
    mut commands: Commands,
    time: Res<Time>,
    mut edges: MissileEdges,
    mut anim_events: MessageReader<AnimSoundEvent>,
    mut pending: ResMut<PendingMissiles>,
    fx: Option<ResMut<SpellFx>>,
    displays: Option<Res<ItemDisplays>>,
    asset_server: Res<AssetServer>,
    units: AttachPosQuery,
    joints: Query<&GlobalTransform>,
    weapons: HeldWeapons,
    casters: Query<(
        &AnimDriver,
        &AnimationPlayer,
        &AnimationTransitions,
        &benilla_assets::ModelAnimations,
    )>,
    anim_data: Option<Res<AnimData>>,
    mut sounds: MessageWriter<MissileSound>,
    mut out: ArrivalOut,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    let launcher = Launcher {
        now,
        units: &units,
        joints: &joints,
        weapons: &weapons,
    };
    // Stamp order; a GO's own release follows its spawn.
    let MissileEdges {
        spawns,
        releases,
        precast,
    } = &mut edges;
    let mut wire: Vec<(u64, Result<&MissileSpawn, MissileRelease>)> = spawns
        .read()
        .map(|sp| (sp.seq, Ok(sp)))
        .chain(releases.read().map(|r| (r.seq, Err(*r))))
        .collect();
    wire.sort_by_key(|&(seq, _)| seq);
    for (_, edge) in wire {
        let spawn = match edge {
            Ok(spawn) => spawn,
            Err(MissileRelease {
                caster,
                seq,
                kind: ReleaseKind::Fire,
            }) => {
                let gos = pending.0.remove(&caster).unwrap_or_default();
                let release = launch_world_pos(caster, None, &units, &joints);
                launcher.release(
                    caster,
                    &gos,
                    release,
                    Some(PrecastDrop::UpTo(seq)),
                    precast,
                    &mut commands,
                    &mut sounds,
                    &mut out,
                    &mut play_seq,
                );
                continue;
            }
            Err(MissileRelease {
                caster,
                seq,
                kind: ReleaseKind::Clear(count),
            }) => {
                precast.drop(caster, PrecastDrop::Cast(seq, count), &mut commands);
                continue;
            }
        };
        // Resolve the model now, so it loads while the release animation plays.
        let key = match (&spawn.path, spawn.ammo_display_id) {
            (Some(path), _) => {
                fx.models
                    .entry(path.clone())
                    .or_insert_with(|| DisplayModel {
                        handle: ModelHandle::M2(asset_server.load(m2_url(path))),
                        ..super::empty_shell()
                    });
                Some(path.clone())
            }
            (None, Some(display_id)) => displays
                .as_deref()
                .and_then(|d| ensure_ammo_model(&mut fx, d, display_id, &asset_server)),
            (None, None) => None,
        };
        // A missile with a caster takes its held model when it is the first made since the hold.
        let takes_hold = spawn.from.is_none() && precast.hand(spawn.caster, spawn.seq);
        let go = QueuedGo {
            spawn: spawn.clone(),
            key,
            queued: 0.0,
            saw_oneshot: false,
            takes_hold,
        };
        if spawn.awaits_release {
            pending.0.entry(spawn.caster).or_default().push(go);
        } else if let Some(from) = spawn.from {
            launch_go(
                &go,
                from,
                None,
                now,
                &mut commands,
                &units,
                &joints,
                &mut sounds,
                &mut out,
                &mut play_seq,
            );
        } else {
            let release = launch_world_pos(spawn.caster, None, &units, &joints);
            launcher.release(
                spawn.caster,
                std::slice::from_ref(&go),
                release,
                // A hold another queued GO took stays for the release that follows (`0x80f5c1`).
                takes_hold.then_some(PrecastDrop::Any),
                precast,
                &mut commands,
                &mut sounds,
                &mut out,
                &mut play_seq,
            );
        }
    }
    // A release ident fired on a caster launches its whole queue from the event's own position.
    for ev in anim_events.read() {
        if !RELEASE_IDENTS.contains(&ev.ident) {
            continue;
        }
        let Some(gos) = pending.0.remove(&ev.entity) else {
            continue;
        };
        let release = launch_world_pos(ev.entity, Some(ev.ident), &units, &joints);
        launcher.release(
            ev.entity,
            &gos,
            release,
            Some(PrecastDrop::Any),
            precast,
            &mut commands,
            &mut sounds,
            &mut out,
            &mut play_seq,
        );
    }
    // Age every queue, and flush one whose one-shot ended without its keyframe or never started.
    let dt = time.delta_secs();
    let catalog = anim_data.as_deref().map(|d| &d.0);
    let mut flushes: Vec<(Entity, Vec<QueuedGo>)> = Vec::new();
    pending.0.retain(|&caster, gos| {
        if !units.contains(caster) {
            return false; // caster streamed out: its queue dies with it
        }
        let caster_anim = casters.get(caster).ok();
        let live = caster_anim.is_some_and(|(drv, player, tr, anims)| {
            oneshot_is_live(drv, player, tr, anims, catalog)
        });
        // A latched special idle of a cast or ranged-attack behavior holds the queue for up to
        // 2000 ms (`0x73442f`, `0x734406`).
        let held = caster_anim
            .and_then(|(drv, ..)| drv.special_idle())
            .is_some_and(|anim| crate::creature_anim::select::is_special_idle_anim(anim, catalog));
        let mut flush = false;
        for go in gos.iter_mut() {
            go.queued += dt;
            go.saw_oneshot |= live;
            flush |= (go.saw_oneshot && !live) || (!go.saw_oneshot && go.queued > RELEASE_WAIT_MAX);
        }
        if held && gos.iter().all(|go| go.queued < SPECIAL_IDLE_HOLD_MAX) {
            flush = false;
        }
        if flush {
            // Launched below: the launch's borrows cannot live inside this closure.
            flushes.push((caster, std::mem::take(gos)));
            return false;
        }
        true
    });
    for (caster, gos) in flushes {
        let release = launch_world_pos(caster, None, &units, &joints);
        launcher.release(
            caster,
            &gos,
            release,
            Some(PrecastDrop::Any),
            precast,
            &mut commands,
            &mut sounds,
            &mut out,
            &mut play_seq,
        );
    }
}

/// Spawn a missile's parts and emitters as its children once its M2 has built, so they ride the
/// mover; an unloadable model flies invisible and still impacts on time.
pub(super) fn attach_missile_models(
    mut commands: Commands,
    mut missiles: Query<(Entity, &mut Missile)>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<benilla_assets::materials::WowModelMaterial>>,
    mut tint_reg: ResMut<super::spell_fx::FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(mut fx) = fx else {
        return;
    };
    for (entity, mut missile) in &mut missiles {
        if missile.parts_spawned {
            continue;
        }
        let Some(key) = missile.path.clone() else {
            continue; // unreachable: the spawn sets `parts_spawned`
        };
        ensure_model(&mut fx, &asset_server, &key);
        let Some(dm) = fx.models.get(&key) else {
            continue; // unreachable: just inserted
        };
        // `false` means still loading; a later pass attaches mid-flight.
        if !attach_effect_visuals(
            &mut commands,
            entity,
            dm,
            time.elapsed_secs(),
            false, // a missile's flat quads are geometry, never ground decals
            // A free world model: its trail stays world-frozen and its pool finishes in place.
            EffectHost::default(),
            // A missile, not a kit effect: no kit stage and no Birth/Hold/Decay, one sequence.
            None,
            &mut FxMaterials {
                store: &mut wow_materials,
                tint: &mut tint_reg,
                uv: &mut uv_reg,
                table: &mut anim_table,
            },
            &ibps,
            &mut palettes,
            Some(INFLIGHT_ANIM),
        ) {
            continue;
        }
        missile.parts_spawned = true;
    }
}

/// The units a trajectory missile may hit, and what decides which (`0x6fcb60`).
#[derive(bevy::ecs::system::SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
pub(super) struct TrajectoryWorld<'w, 's> {
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static GlobalTransform,
            &'static crate::net::NetEntity,
            &'static crate::net::ObjectStore,
            Option<&'static crate::net::UnitSpeeds>,
        ),
    >,
    movers: Query<'w, 's, (), With<crate::net::ActiveMover>>,
    passengers: Query<'w, 's, &'static crate::vehicle::Passenger>,
    riders: Query<'w, 's, &'static crate::transport::TransportRider>,
    index: Res<'w, crate::net::GuidIndex>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    creatures: Option<Res<'w, super::Creatures>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    net: Option<Res<'w, crate::net::NetCommands>>,
}

impl TrajectoryWorld<'_, '_> {
    /// The transport `unit` stands on, by its guid (`0x6ffd70`).
    fn deck_of(&self, unit: Entity) -> Option<Entity> {
        let rider = self.riders.get(unit).ok()?;
        self.index.0.get(&rider.transport_guid).copied()
    }

    /// `unit`'s root vehicle, itself when it rides none.
    fn root(&self, unit: Entity) -> Entity {
        let mut at = unit;
        // A seat tree is a few levels deep; the bound stops a malformed loop.
        for _ in 0..8 {
            let Some(up) = self
                .passengers
                .get(at)
                .ok()
                .and_then(|p| self.index.0.get(&p.vehicle).copied())
            else {
                break;
            };
            at = up;
        }
        at
    }

    /// Whether the caster is ours to collide for: a unit we control (`0x71c930`'s `0x400`), or
    /// our pet when nobody charms it (`0x732ff0`).
    fn ours(&self, caster: Entity) -> bool {
        if self.movers.contains(caster) {
            return true;
        }
        let Ok((.., store, _)) = self.units.get(caster) else {
            return false;
        };
        self.self_guid.0.is_some()
            && store.0.unit_summoned_by() == self.self_guid.0
            && store.0.unit_charmed_by().unwrap_or(0) == 0
    }

    /// A unit's hit sphere for a flight along `dir`.
    fn sphere(&self, unit: Entity, dir: Vec2, radius: f32) -> Option<(Vec3, f32)> {
        let (_, tf, net, ..) = self.units.get(unit).ok()?;
        let model = self
            .creatures
            .as_deref()
            .zip(net.display_id)
            .and_then(|(c, d)| c.missile_collision(d));
        // The unit's true scale (`0x6fc02b`), its transform's eased scale.
        let (scale, _, at) = tf.to_scale_rotation_translation();
        let (c, r) = trajectory::hit_sphere(at, scale.x, model, dir);
        Some((c, r + radius))
    }

    /// The units near the path, best first (`0x6fcb60`): the caster's own vehicle and everyone on
    /// it excluded. Whether a unit is hostile is asked of our own body, not the mover.
    fn pick(
        &self,
        caster: Entity,
        path: &trajectory::Path,
        flags: u32,
        radius: f32,
    ) -> Vec<Entity> {
        let exclude = self.root(caster);
        let me = self.me.single().ok();
        let ranked = self
            .units
            .iter()
            .filter(|(_, _, net, ..)| {
                matches!(
                    net.kind,
                    benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                )
            })
            .filter_map(|(e, _, net, store, speeds)| {
                let player = net.kind == benilla_protocol::EntityKind::Player;
                let type_flags = if player {
                    0
                } else {
                    store
                        .0
                        .object_entry()
                        .and_then(|entry| self.names.as_deref()?.creature_record(entry))
                        .map_or(0, |r| r.type_flags)
                };
                let attackable = self.reputations.as_deref().is_some_and(|reps| {
                    crate::target::can_attack(
                        Some(store),
                        self.factions.as_deref(),
                        reps,
                        crate::target::party_of(self.group.as_deref()),
                        me,
                    )
                });
                if !trajectory::collides(flags, player, type_flags, attackable)
                    || self.root(e) == exclude
                {
                    return None;
                }
                let speed = speeds.map_or(0.0, |s| s.0.run.max(s.0.swim).max(s.0.flight));
                let (c, r) = self.sphere(e, path.dir, radius)?;
                Some((e, path.rank(Vec2::new(c.x, c.z), r, speed)?))
            })
            .collect();
        trajectory::pick(ranked)
    }
}

/// A missile that stopped short of its target lands where it stopped: no target, so the area
/// effect on the caster (`0x703410` with none).
fn land_short(
    missile: &Missile,
    at: Vec3,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) {
    out.impacts.write(CastEvent {
        entity: missile.caster,
        spell_id: missile.spell_id,
        kind: CastEventKind::GroundImpact { pos: at },
        cast_count: 0,
        seq: play_seq.next(),
    });
}

/// One flight step (`0x7015d0`): on its deadline or a script's speed, over the ground or an arc.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn move_missiles(
    mut commands: Commands,
    time: Res<Time>,
    mut missiles: Query<(Entity, &mut Missile, &mut Transform)>,
    mut stuck: Query<(Entity, &StuckMissile, &mut Transform), Without<Missile>>,
    units: AttachPosQuery,
    joints: Query<&GlobalTransform>,
    mut out: ArrivalOut,
    mut sounds: MessageWriter<MissileSound>,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
    world: TrajectoryWorld,
    ground: benilla_world::surface::TerrainHeight,
) {
    let dt = time.delta_secs();
    let clock = time.elapsed_secs();
    carry_stuck_missiles(
        clock,
        &mut stuck,
        &units,
        &joints,
        &mut commands,
        &mut out.kit.fx,
    );
    for (entity, mut missile, mut transform) in &mut missiles {
        // Re-place a transport's point each step; once the transport goes, the aim stays (`0x6ff320`).
        if let Some((deck, local)) = missile.aim_deck {
            match joints.get(deck) {
                Ok(g) => missile.aim = Aim::Ground(deck_place(local, g)),
                Err(_) => missile.aim_deck = None,
            }
        }
        let hug = missile.follow.filter(|f| f.hugs_target()).map(|f| f.height);
        let live = match aim_point(missile.aim, hug, &units, &joints) {
            Some(live) => live,
            // A target gone mid-flight leaves the aim where it last was, to land on its area
            // (`0x70164d`).
            None => {
                let last = missile.impact;
                missile.aim = Aim::Ground(last);
                last
            }
        };
        let aim = missile.fixed_aim.unwrap_or(live);
        missile.impact = aim;
        if let (Some((left, state)), Aim::Unit { target, .. }) = (missile.victim_clip, missile.aim)
        {
            missile.victim_clip = if left > dt {
                Some((left - dt, state))
            } else {
                out.defenses.write(DefenseAnim {
                    victim: target,
                    victim_state: state,
                });
                None
            };
        }
        // The flight rides its target's transport, or its point's (`0x6ffd70`).
        let deck = match missile.aim {
            Aim::Unit { target, .. } => world.deck_of(target),
            Aim::Ground(_) => missile.aim_deck.map(|(d, _)| d),
        }
        .and_then(|d| units.get(d).ok());
        if let Some((deck, ..)) = deck {
            let carry = missile.deck.follow(deck.translation(), dt);
            missile.pos += carry;
            if missile.launch_rides_deck {
                missile.launch += carry;
            }
        }
        if let Some(speed) = missile.speed_override.filter(|_| missile.flight.is_none()) {
            missile.arrive_in = missile.pos.distance(aim) / speed;
        }
        if missile.arrive_in <= dt {
            debug!(
                "missile: spell {} landed, route {:?}, spent {}, speed {:.1}",
                missile.spell_id, missile.kind, missile.spent, missile.speed
            );
            let next = arrival_handoff(
                &missile,
                aim,
                transform.translation,
                transform.rotation,
                &mut out,
                &mut play_seq,
            );
            // Every landing stops the flight loop, a new flight restarts it (`0x70354c`, `0x702a3f`).
            sounds.write(MissileSound::Stop { entity });
            // A unit landing hops a listed flight on (`0x702a60`); a blocked hop lodges, then
            // leaves the shield still playing Stuck (`0x703105`, `0x7025b0`).
            let on_unit = matches!(missile.aim, Aim::Unit { .. });
            let mut hopped = |m: &mut Missile, out: &mut ArrivalOut| {
                m.impact = aim;
                let next = hop(m, clock, &units, &joints, out);
                if next == Hop::Stranded {
                    land_on_area(m, aim, out, &mut play_seq);
                }
                next
            };
            let mut relaunched = false;
            let flies_on = match next {
                AfterArrival::End => {
                    relaunched = on_unit && hopped(&mut missile, &mut out) == Hop::On;
                    relaunched
                }
                AfterArrival::Glance => {
                    let glancing = glance_off(entity, &mut missile, &mut commands);
                    relaunched = hopped(&mut missile, &mut out) == Hop::On;
                    relaunched || glancing
                }
                AfterArrival::Stick => {
                    let lodged = lodge(&missile, &units);
                    match hopped(&mut missile, &mut out) {
                        Hop::On => {
                            relaunched = true;
                            if let Some((.., key)) = lodged {
                                play_missile_sequence(&mut commands, entity, key, STUCK_ANIM);
                            }
                            true
                        }
                        Hop::Stranded => false,
                        Hop::Done => stick(
                            entity,
                            &missile,
                            &transform,
                            clock,
                            &units,
                            &joints,
                            &mut commands,
                        ),
                    }
                }
                AfterArrival::Reflect => {
                    relaunched = reflect_back(&mut missile, clock, &units, &joints, &mut out);
                    relaunched
                }
            };
            if let Some(kit_sound) = missile.sound.filter(|_| relaunched) {
                sounds.write(MissileSound::Start {
                    entity,
                    kit_sound,
                    pos: missile.pos,
                });
            }
            if !flies_on {
                end_flight(
                    entity,
                    &missile,
                    &mut commands,
                    &mut sounds,
                    &mut out.kit.fx,
                );
            }
            continue;
        }
        let previous = missile.pos;
        let m = &mut *missile;
        let dir = if let Some(arc) = m.flight.as_mut().and_then(|f| f.arc.as_mut()) {
            m.pos = arc.advance(dt);
            m.progress = 1.0 - (m.arrive_in - dt) / m.flight_time;
            m.h_progress = m.progress;
            arc.heading().try_normalize()
        } else {
            let to_target = aim - previous;
            let share = dt / m.arrive_in;
            let distance = to_target.length();
            let dir = to_target.try_normalize();
            m.pos = match m.follow {
                Some(f) => shape::follow_ground(
                    previous,
                    dir.unwrap_or(Vec3::Y),
                    distance * share,
                    &f,
                    m.h_progress,
                    aim,
                    |p| ground.at(p),
                ),
                None => previous + to_target * share,
            };
            (m.progress, m.h_progress) = shape::progress(
                m.launch,
                aim,
                m.pos,
                distance * (1.0 - share),
                (m.progress, m.h_progress),
            );
            dir
        };
        // A scripted model draws off its path, turned and scaled (`0x701230`).
        let (offset, turn, scale) = m.pose.map_or((Vec3::ZERO, Quat::IDENTITY, 1.0), |p| {
            (p.offset, p.turn, p.scale)
        });
        let model = m.pos + offset;
        // A missile on its aim heads straight up (`0x7018ae`).
        let aim_dir = dir.unwrap_or(Vec3::Y);
        let heading = m.face.as_mut().map_or(aim_dir, |f| f.face(model, aim_dir));
        if dir.is_some() || m.pose.is_some() {
            transform.rotation = missile_facing(heading) * turn;
        }
        transform.scale = Vec3::splat(scale * m.scale);
        transform.translation = model;
        m.arrive_in -= dt;
        if let Some(range) = m.fade_over {
            let distance = m.pos.distance(aim);
            if distance < range {
                let alpha = disposition::fade_alpha(distance, range);
                commands
                    .entity(entity)
                    .insert(benilla_world::model_fade::ModelFade(alpha));
            }
        }
        let now = m.pos;
        let (caster, spell) = (m.caster, m.spell_id);
        let Some(flight) = m.flight.as_mut() else {
            continue;
        };
        let short = if flight
            .remote
            .is_some_and(|hit| trajectory::passed(previous, now, hit))
        {
            Some(previous)
        } else {
            collide(flight, caster, spell, previous, now, &world)
        };
        if let Some(at) = short {
            m.pos = at;
            transform.translation = at;
            land_short(&missile, at, &mut out, &mut play_seq);
            end_flight(
                entity,
                &missile,
                &mut commands,
                &mut sounds,
                &mut out.kit.fx,
            );
        }
    }
}

/// One step of a trajectory missile's unit collision: the point its step from `previous` to `now`
/// first meets a unit, reported to the server (`0x703640`).
fn collide(
    flight: &mut Flight,
    caster: Entity,
    spell: u32,
    previous: Vec3,
    now: Vec3,
    world: &TrajectoryWorld,
) -> Option<Vec3> {
    if let Collider::Pending {
        flags,
        radius,
        impact,
    } = flight.collider
    {
        flight.collider = match flight.arc.filter(|_| world.ours(caster)) {
            Some(arc) => {
                let path = trajectory::Path::new(previous, impact, arc.pitch, arc.speed);
                Collider::Units {
                    units: world.pick(caster, &path, flags, radius),
                    dir: path.dir,
                    radius,
                }
            }
            None => Collider::Off,
        };
    }
    let Collider::Units { units, dir, radius } = &flight.collider else {
        return None;
    };
    let spheres: Vec<(Vec3, f32)> = units
        .iter()
        .filter_map(|&u| world.sphere(u, *dir, *radius))
        .collect();
    let t = trajectory::segment_hit(previous, now, &spheres)?;
    let at = previous.lerp(now, t);
    if let Some(net) = world.net.as_deref() {
        let _ = net.0.send(crate::net::ClientCommand::Vehicle(
            crate::net::VehicleCommand::ProjectileHit {
                caster: flight.caster,
                spell,
                cast_id: flight.cast_id,
                at: benilla_assets::coords::bevy_to_wow(at),
            },
        ));
    }
    Some(at)
}

/// The server-hit and pre-resurrect handlers.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::SetProjectilePosition,
        on_set_projectile_position,
    )
    .net_handler(
        benilla_protocol::SessionEventKind::PreResurrect,
        on_pre_resurrect,
    );
}

/// `SMSG_PRE_RESURRECT` (`0x716d80`): every missile in flight at the unit is freed where it is
/// (`0x703a80`), with no arrival.
fn on_pre_resurrect(
    In(ev): In<benilla_protocol::SessionEvent>,
    index: Res<crate::net::GuidIndex>,
    missiles: Query<(Entity, &Missile)>,
    mut commands: Commands,
    mut sounds: MessageWriter<MissileSound>,
    mut kit_fx: MessageWriter<SpellKitFx>,
) {
    let benilla_protocol::SessionEvent::PreResurrect { guid } = ev else {
        return;
    };
    let Some(&unit) = index.0.get(&guid) else {
        return;
    };
    for (entity, missile) in &missiles {
        if matches!(missile.aim, Aim::Unit { target, .. } if target == unit) {
            end_flight(entity, missile, &mut commands, &mut sounds, &mut kit_fx);
        }
    }
}

/// `SMSG_SET_PROJECTILE_POSITION` (`0x6ff2c0`): every in-flight trajectory missile of the caster's
/// cast takes the server's hit.
fn on_set_projectile_position(
    In(ev): In<benilla_protocol::SessionEvent>,
    mut missiles: Query<&mut Missile>,
) {
    let benilla_protocol::SessionEvent::SetProjectilePosition {
        caster,
        cast_id,
        position,
    } = ev
    else {
        return;
    };
    let hit = benilla_assets::coords::wow_to_bevy(position);
    for mut missile in &mut missiles {
        if let Some(f) = missile
            .flight
            .as_mut()
            .filter(|f| f.caster == caster && f.cast_id == cast_id)
        {
            f.remote = Some(hit);
        }
    }
}

/// The pending queue, headless; the anim-end flush edge needs a live animation rig and is not
/// covered here.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use bevy::asset::AssetPlugin;

    use super::*;
    use crate::creature_anim::{ChainProcPlay, PlaySeq};

    /// `Spell.dbc` Speed such that the test target's 24-unit range is a 1.000 s flight.
    const SPEED: f32 = 24.0;

    fn app() -> App {
        let mut app = App::new();
        // No TimePlugin: the tests advance `Time` by hand so every queued-duration is exact.
        app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_resource::<Time>();
        app.init_resource::<super::super::spell_fx::SpellFx>();
        app.init_resource::<PendingMissiles>();
        app.init_resource::<PlaySeq>();
        app.add_message::<MissileSpawn>()
            .add_message::<MissileRelease>()
            .add_message::<AnimSoundEvent>()
            .add_message::<MissileSound>()
            .add_message::<CastEvent>()
            .add_message::<DefenseAnim>()
            .add_message::<MissileMiss>()
            .add_message::<SpellKitFx>()
            .add_message::<SpellKitSound>()
            .add_message::<ChainProcPlay>()
            .add_message::<crate::creature_anim::EmoteAnim>()
            .add_message::<crate::creature_anim::WoundAnim>()
            .add_message::<crate::creature_anim::SpellKitShake>()
            .add_message::<crate::weapon_trail::TrailArm>()
            .add_message::<crate::creature_anim::BaseAnimRecompute>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::aura_visual::BodyProcPlay>()
            .add_message::<crate::aura_visual::AuraProc>()
            .add_message::<crate::entities::dest_fx::GroundBurst>();
        app.add_systems(Update, spawn_missiles);
        app
    }

    fn step(app: &mut App, dt: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(dt));
        app.update();
    }

    /// A caster whose `$CSL` marker, on bone 0 of its pose, sits at `hand`.
    fn caster(app: &mut App, hand: Vec3) -> Entity {
        let caster = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                BoneAttach {
                    points: HashMap::new(),
                    markers: HashMap::from([(*b"$CSL", (0u16, Vec3::ZERO))]),
                },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(caster, &[hand]);
        app.world_mut().entity_mut(caster).insert(pose);
        caster
    }

    fn go(caster: Entity, target: Entity, awaits_release: bool) -> MissileSpawn {
        MissileSpawn {
            caster,
            spell_id: 133,
            path: None, // an invisible flight; the queue needs no model
            ammo_display_id: None,
            dest_tag: None,
            speed: SPEED,
            targets: vec![(target, None)],
            reflects: Vec::new(),
            reflect_glances: false,
            scale: 1.0,
            ground_aim: None,
            missile_sound: None,
            awaits_release,
            trajectory: None,
            shape: MissileShape::default(),
            from: None,
            list: None,
            aim_deck: None,
            seq: 1,
        }
    }

    /// The location-fallback flavour of [`go`]: no unit targets, one point on the wire.
    fn ground_go(caster: Entity, at: Vec3) -> MissileSpawn {
        MissileSpawn {
            targets: Vec::new(),
            ground_aim: Some(at),
            ..go(caster, caster, false)
        }
    }

    fn missiles(app: &mut App) -> Vec<(f32, Vec3)> {
        app.world_mut()
            .query::<(&Missile, &Transform)>()
            .iter(app.world())
            .map(|(m, t)| (m.arrive_in, t.translation))
            .collect()
    }

    #[test]
    fn release_keyframe_launches_from_the_marker_minus_queued_time() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.05);
        step(&mut app, 0.05);
        assert!(missiles(&mut app).is_empty(), "parked until the keyframe");
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.05);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1);
        let (arrive_in, at) = launched[0];
        assert_eq!(at, hand, "launched from the fired marker's position");
        // 24 units at speed 24 = 1.0 s, minus the 0.10 s spent queued (the two parked frames;
        // the release frame's drain runs before its backstop aging).
        assert!(
            (arrive_in - 0.90).abs() < 1e-3,
            "flight time {arrive_in} ≠ 1.0 − 0.10 queued"
        );
    }

    #[test]
    fn release_past_deadline_impacts_on_the_spot() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        // 2.4 units at speed 24 = a 0.1 s flight; the queue waits 0.2 s before the keyframe.
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.016);
        assert!(missiles(&mut app).is_empty(), "no flight entity");
        let impacts: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .drain()
            .filter(|e| matches!(e.kind, CastEventKind::Impact { .. }))
            .collect();
        assert_eq!(impacts.len(), 1, "the impact plays at release");
        assert_eq!(impacts[0].entity, target);
        assert_eq!(impacts[0].spell_id, 133);
    }

    /// The location fallback flies exactly one projectile.
    #[test]
    fn a_targetless_dest_go_flies_one_projectile_at_the_point() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let at = Vec3::new(24.0, 1.5, 0.0);
        app.world_mut().write_message(ground_go(caster, at));
        step(&mut app, 0.016);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1, "one shot, not one per (absent) target");
        assert_eq!(launched[0].1, hand, "launched from the caster's marker");
        // 24 units at speed 24, no cast-kit anim to wait on (`awaits_release: false`).
        assert!(
            (launched[0].0 - 1.0).abs() < 1e-3,
            "flight {}",
            launched[0].0
        );
        let ground = app
            .world_mut()
            .query::<&Missile>()
            .iter(app.world())
            .filter(|m| matches!(m.aim, Aim::Ground(p) if p == at))
            .count();
        assert_eq!(ground, 1, "aimed at the point, homing to nothing");
    }

    /// The reference reads the location latch only for an empty hit array.
    #[test]
    fn a_dest_go_that_hit_units_flies_at_the_units_not_the_point() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.ground_aim = Some(Vec3::new(-50.0, 0.0, 0.0));
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let aims: Vec<bool> = app
            .world_mut()
            .query::<&Missile>()
            .iter(app.world())
            .map(|m| matches!(m.aim, Aim::Unit { .. }))
            .collect();
        assert_eq!(aims, vec![true], "one unit-homing shot, no ground shot");
    }

    /// The reference plays the kit on the caster with `extra` = the landing point.
    #[test]
    fn a_ground_arrival_hands_off_to_the_caster_with_the_landing_point() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        // 2.4 units at speed 24 = a 0.1 s flight, parked 0.2 s → it arrives at the release.
        let at = Vec3::new(2.4, 0.0, 0.0);
        let mut spawn = ground_go(caster, at);
        spawn.awaits_release = true;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.016);
        assert!(missiles(&mut app).is_empty(), "no flight entity");
        let events: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .drain()
            .collect();
        assert_eq!(events.len(), 1, "exactly one arrival edge");
        assert_eq!(events[0].entity, caster, "the kit plays on the caster");
        assert_eq!(
            events[0].kind,
            CastEventKind::GroundImpact { pos: at },
            "the ground arm, carrying the landing point"
        );
    }

    #[test]
    fn never_played_oneshot_flushes_after_the_wait_window() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        assert!(missiles(&mut app).is_empty(), "still inside the window");
        step(&mut app, 0.10); // cumulative 0.30 > RELEASE_WAIT_MAX
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1, "flushed by the backstop");
        assert_eq!(launched[0].1, hand, "through the $CSL cascade");
        assert!(
            (launched[0].0 - 0.70).abs() < 1e-3,
            "0.30 s queued subtracted"
        );
    }

    /// A flying app with the systems a miss's whole route needs.
    fn flying_app() -> App {
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .add_systems(Update, move_missiles.after(spawn_missiles));
        app
    }

    /// Run the flight out, collecting what it wrote.
    fn fly_out(app: &mut App) -> (Vec<CastEvent>, Vec<DefenseAnim>, Vec<(Entity, u8)>) {
        let (mut impacts, mut clips, mut words) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..200 {
            step(app, 0.02);
            impacts.extend(
                app.world_mut()
                    .resource_mut::<Messages<CastEvent>>()
                    .drain(),
            );
            clips.extend(
                app.world_mut()
                    .resource_mut::<Messages<DefenseAnim>>()
                    .drain(),
            );
            words.extend(
                app.world_mut()
                    .resource_mut::<Messages<MissileMiss>>()
                    .drain()
                    .map(|w| (w.anchor, w.code)),
            );
        }
        (impacts, clips, words)
    }

    /// Each outcome's word, and the arrival dodge only a resist and a reflect draw (`0x7030d7`).
    /// These flights have no model, so none times a clip before it lands.
    #[test]
    fn a_missed_arrival_words_the_target_and_only_resists_and_reflects_dodge() {
        for (code, word_code, dodges, expect_word) in [
            (3u8, 3u8, false, "Dodge"),
            (5, 5, false, "Block"),
            (1, 1, false, "Miss"),
            (4, 9, false, "Deflect"),
            (7, 7, true, "Immune"),
            (11, 11, true, "Reflect"),
        ] {
            let mut app = flying_app();
            let caster = caster(&mut app, Vec3::ZERO);
            let target = app
                .world_mut()
                .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
                .id();
            let mut spawn = go(caster, target, false);
            spawn.targets = vec![(target, Some(code))];
            app.world_mut().write_message(spawn);
            let (impacts, clips, words) = fly_out(&mut app);
            assert!(missiles(&mut app).is_empty(), "flight over (code {code})");
            assert!(
                !impacts
                    .iter()
                    .any(|e| e.entity == target && matches!(e.kind, CastEventKind::Impact { .. })),
                "a miss never plays the impact (code {code})"
            );
            assert_eq!(
                clips
                    .iter()
                    .map(|c| (c.victim, c.victim_state))
                    .collect::<Vec<_>>(),
                if dodges {
                    vec![(target, DODGE_STATE)]
                } else {
                    Vec::new()
                },
                "code {code}"
            );
            assert_eq!(words, vec![(target, word_code)], "code {code}");
            assert_eq!(
                crate::combat_text::miss_word(word_code).map(|(w, _)| w),
                Some(expect_word),
                "code {code}"
            );
        }
    }

    /// A reflect with no outcome of its own flies back and lands on the caster as a hit.
    #[test]
    fn a_reflect_flies_back_and_hits_the_caster() {
        let mut app = flying_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.targets = vec![(target, Some(11))];
        app.world_mut().write_message(spawn);
        let (impacts, ..) = fly_out(&mut app);
        assert_eq!(
            impacts
                .iter()
                .filter(|e| e.entity == caster
                    && e.kind
                        == CastEventKind::Impact {
                            on_caster: true,
                            at: None,
                            caster,
                        })
                .count(),
            1
        );
    }

    /// A reflect the caster in turn resists words the caster, then ends.
    #[test]
    fn a_reflected_flight_takes_its_own_outcome_on_the_caster() {
        let mut app = flying_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.targets = vec![(target, Some(11))];
        spawn.reflects = vec![(target, 2)];
        app.world_mut().write_message(spawn);
        let (impacts, _, words) = fly_out(&mut app);
        assert_eq!(words, vec![(target, 11), (caster, 2)]);
        assert!(!impacts
            .iter()
            .any(|e| matches!(e.kind, CastEventKind::Impact { .. })));
    }

    /// A modelled block plays the victim's shield clip on the way in, and an invisible one does not.
    #[test]
    fn a_modelled_block_times_the_shield_clip_before_it_lands() {
        for (modelled, clips_expected) in [(true, 1), (false, 0)] {
            let mut app = flying_app();
            let caster = caster(&mut app, Vec3::ZERO);
            let target = app
                .world_mut()
                .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
                .id();
            let mut spawn = go(caster, target, false);
            spawn.targets = vec![(target, Some(5))];
            if modelled {
                let path = "Spells\\Bolt.m2".to_string();
                app.world_mut()
                    .resource_mut::<SpellFx>()
                    .models
                    .insert(path.clone(), super::super::empty_shell());
                spawn.path = Some(path);
            }
            app.world_mut().write_message(spawn);
            let (_, clips, _) = fly_out(&mut app);
            let blocks: Vec<u32> = clips.iter().map(|c| c.victim_state).collect();
            assert_eq!(
                blocks,
                vec![BLOCK_STATE; clips_expected],
                "modelled {modelled}"
            );
        }
    }

    /// A block lands in the shield, rides the victim for 5 s and only then goes.
    #[test]
    fn a_block_sticks_in_the_shield_and_rides_the_victim() {
        let mut app = flying_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn((
                GlobalTransform::from_xyz(2.4, 0.0, 0.0),
                BoneAttach {
                    points: HashMap::from([(disposition::SHIELD_ATTACHMENT, (0u16, Vec3::ZERO))]),
                    markers: HashMap::new(),
                },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(target, &[Vec3::new(2.4, 1.0, 0.0)]);
        app.world_mut().entity_mut(target).insert(pose);
        let path = "Spells/Bolt.m2".to_string();
        app.world_mut()
            .resource_mut::<SpellFx>()
            .models
            .insert(path.clone(), super::super::empty_shell());
        let mut spawn = go(caster, target, false);
        spawn.targets = vec![(target, Some(5))];
        spawn.path = Some(path);
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        for mut m in app
            .world_mut()
            .query::<&mut Missile>()
            .iter_mut(app.world_mut())
        {
            m.parts_spawned = true;
        }
        for _ in 0..30 {
            step(&mut app, 0.02);
        }
        assert_eq!(missiles(&mut app).len(), 0, "off the flight list");
        let mut stuck = app.world_mut().query::<&StuckMissile>();
        assert_eq!(stuck.iter(app.world()).count(), 1, "lodged");
        for _ in 0..30 {
            step(&mut app, 0.2);
        }
        assert_eq!(stuck.iter(app.world()).count(), 0, "gone after 5 s");
    }

    /// A ground flight's targeting kit stands at the aim on copy 0 alone, and goes with the flight.
    #[test]
    fn a_ground_flight_holds_its_targeting_kit_at_the_aim() {
        use benilla_formats::{SpellVisualCatalog, VisualKit};
        const KIT: u32 = 8729;
        const EFFECT: u32 = 3807;
        let mut app = flying_app();
        let mut slots = [None; 9];
        slots[2] = Some(EFFECT);
        slots[3] = Some(EFFECT + 1);
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
            HashMap::new(),
            HashMap::from([(
                KIT,
                VisualKit {
                    effect_slots: slots,
                    ..Default::default()
                },
            )]),
            HashMap::from([
                (EFFECT, "Spells\\Base.m2".to_string()),
                (EFFECT + 1, "Spells\\Hand.m2".to_string()),
            ]),
        )));
        let caster = caster(&mut app, Vec3::ZERO);
        app.world_mut()
            .entity_mut(caster)
            .insert(crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            });
        let at = Vec3::new(24.0, 0.0, 0.0);
        let mut spawn = ground_go(caster, at);
        spawn.shape.targeting_kit = KIT;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let held = |app: &mut App| -> Vec<Vec3> {
            app.world_mut()
                .query_filtered::<&Transform, With<crate::entities::dest_fx::GroundFx>>()
                .iter(app.world())
                .map(|t| t.translation)
                .collect()
        };
        assert_eq!(held(&mut app), vec![at], "the base slot alone, at the aim");
        fly_out(&mut app);
        assert!(held(&mut app).is_empty(), "reaped with the flight");
    }

    /// A GO with no cast-kit animation launches at once, with the full flight time.
    #[test]
    fn immediate_go_launches_at_once() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, false));
        step(&mut app, 0.016);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].1, hand);
        assert!((launched[0].0 - 1.0).abs() < 1e-3, "full flight time");
    }

    /// A held missile model hands over at the release: the first launch leaves from its place, the
    /// next from the marker, and the model goes (`0x72df00`).
    #[test]
    fn the_release_launches_from_the_held_model_and_drops_it() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let held_at = Vec3::new(0.4, 2.0, -0.3);
        let root = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(held_at))
            .id();
        app.world_mut().entity_mut(caster).insert(
            super::super::spell_fx::FxAttached::holding_for_tests(133, Some(root)),
        );
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.05);
        assert!(
            app.world().get_entity(root).is_ok(),
            "held until the release"
        );
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.05);
        let mut from: Vec<Vec3> = missiles(&mut app).into_iter().map(|(_, at)| at).collect();
        from.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert_eq!(from, vec![hand, held_at]);
        assert!(app.world().get_entity(root).is_err(), "the model leaves");
        let att = app
            .world()
            .entity(caster)
            .get::<super::super::spell_fx::FxAttached>()
            .unwrap();
        assert!(!att.holds_precast());
    }

    /// A caster holding a model at `held_at`, keyed to cast count 4 at stamp 2, and its target.
    fn holding_caster(app: &mut App, hand: Vec3, held_at: Vec3) -> (Entity, Entity, Entity) {
        let caster = caster(app, hand);
        let root = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(held_at))
            .id();
        app.world_mut().entity_mut(caster).insert(
            super::super::spell_fx::FxAttached::holding_for_tests(133, Some(root))
                .keyed_for_tests(4, 2),
        );
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        (caster, root, target)
    }

    fn holds(app: &App, caster: Entity) -> bool {
        app.world()
            .get::<super::super::spell_fx::FxAttached>(caster)
            .is_some_and(|a| a.holds_precast())
    }

    fn release(caster: Entity, seq: u64, kind: ReleaseKind) -> MissileRelease {
        MissileRelease { caster, seq, kind }
    }

    /// Only the first missile made takes the held model: a GO's second target leaves from the
    /// marker (`0x73304d`, `0x72e011`).
    #[test]
    fn only_the_first_target_of_a_go_leaves_from_the_held_model() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let held_at = Vec3::new(0.4, 2.0, -0.3);
        let (caster, root, target) = holding_caster(&mut app, hand, held_at);
        let other = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(0.0, 1.5, 24.0))
            .id();
        app.world_mut().write_message(MissileSpawn {
            targets: vec![(target, None), (other, None)],
            seq: 3,
            ..go(caster, target, false)
        });
        step(&mut app, 0.05);
        let mut from: Vec<Vec3> = missiles(&mut app).into_iter().map(|(_, at)| at).collect();
        from.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert_eq!(from, vec![hand, held_at]);
        assert!(app.world().get_entity(root).is_err());
        assert!(!holds(&app, caster));
    }

    /// A release launches the queue at once from the held model and the marker and drops the
    /// hold; one stamped before the hold leaves it (`0x72e240`).
    #[test]
    fn a_release_launches_the_queue_and_drops_the_hold() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let held_at = Vec3::new(0.4, 2.0, -0.3);
        let (caster, root, target) = holding_caster(&mut app, hand, held_at);
        app.world_mut()
            .write_message(release(caster, 1, ReleaseKind::Fire));
        step(&mut app, 0.05);
        assert!(holds(&app, caster), "a release from before the hold");
        app.world_mut().write_message(MissileSpawn {
            seq: 3,
            ..go(caster, target, true)
        });
        app.world_mut()
            .write_message(release(caster, 4, ReleaseKind::Fire));
        step(&mut app, 0.05);
        let from: Vec<Vec3> = missiles(&mut app).into_iter().map(|(_, at)| at).collect();
        assert_eq!(from, vec![held_at], "launched before its keyframe");
        assert!(app.world().get_entity(root).is_err());
        assert!(!holds(&app, caster));
        assert!(!app.world().resource::<PendingMissiles>().releasing(caster));
    }

    /// A release with no queue still drops the hold (`0x72e1ed`).
    #[test]
    fn a_release_with_no_missile_drops_the_hold() {
        let mut app = app();
        let (caster, root, _) = holding_caster(&mut app, Vec3::Y, Vec3::X);
        app.world_mut()
            .write_message(release(caster, 3, ReleaseKind::Fire));
        step(&mut app, 0.05);
        assert!(missiles(&mut app).is_empty());
        assert!(app.world().get_entity(root).is_err());
        assert!(!holds(&app, caster));
    }

    /// A GO launched at once leaves a hold a queued GO took: the release after it launches that GO
    /// from the held model (`0x80f5c1`, `0x72e011`).
    #[test]
    fn an_instant_go_leaves_a_hold_a_queued_go_took() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let held_at = Vec3::new(0.4, 2.0, -0.3);
        let (caster, root, target) = holding_caster(&mut app, hand, held_at);
        app.world_mut().write_message(MissileSpawn {
            seq: 3,
            ..go(caster, target, true)
        });
        step(&mut app, 0.05);
        app.world_mut().write_message(MissileSpawn {
            seq: 4,
            ..go(caster, target, false)
        });
        step(&mut app, 0.05);
        let from: Vec<Vec3> = missiles(&mut app).into_iter().map(|(_, at)| at).collect();
        assert_eq!(from, vec![hand], "the instant GO leaves from the marker");
        assert!(holds(&app, caster), "kept for the queued GO");
        app.world_mut()
            .write_message(release(caster, 5, ReleaseKind::Fire));
        step(&mut app, 0.05);
        let mut from: Vec<Vec3> = missiles(&mut app).into_iter().map(|(_, at)| at).collect();
        from.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert_eq!(from, vec![hand, held_at]);
        assert!(app.world().get_entity(root).is_err());
        assert!(!holds(&app, caster));
    }

    /// The end-cast clear drops only its own cast's hold and launches nothing (`0x71bff0`).
    #[test]
    fn the_end_cast_clear_drops_only_its_cast_s_hold() {
        let mut app = app();
        let (caster, root, target) = holding_caster(&mut app, Vec3::Y, Vec3::X);
        app.world_mut().write_message(MissileSpawn {
            seq: 3,
            ..go(caster, target, true)
        });
        app.world_mut()
            .write_message(release(caster, 4, ReleaseKind::Clear(5)));
        step(&mut app, 0.05);
        assert!(holds(&app, caster), "another cast's count");
        app.world_mut()
            .write_message(release(caster, 5, ReleaseKind::Clear(4)));
        step(&mut app, 0.05);
        assert!(app.world().get_entity(root).is_err());
        assert!(!holds(&app, caster));
        assert!(missiles(&mut app).is_empty(), "a clear launches nothing");
        assert!(app.world().resource::<PendingMissiles>().releasing(caster));
    }

    /// A trajectory shot lands at the packet's delay on an arc above the straight line, and a
    /// server hit lands it where it starts to fly away from that point.
    #[test]
    fn a_trajectory_shot_arcs_and_lands_at_a_server_hit() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .add_systems(Update, move_missiles.after(spawn_missiles));
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(40.0, 0.0, 0.0);
        let trajectory = crate::creature_anim::Trajectory {
            pitch: 0.6,
            delay: 2.0,
            caster: 0xF130_0000_0000_0001,
            cast_id: 7,
        };
        let mut spawn = ground_go(caster, at);
        spawn.trajectory = Some((trajectory, None));
        app.world_mut().write_message(spawn);
        step(&mut app, 0.0);
        assert_eq!(
            missiles(&mut app)[0].0,
            2.0,
            "the packet's delay, not distance / speed"
        );
        step(&mut app, 0.5);
        let (_, mid) = missiles(&mut app)[0];
        assert!(mid.y > 1.0, "above the straight line: {mid}");
        // The server's hit, a little ahead of the shot.
        let hit = mid + Vec3::new(1.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once_with(
                on_set_projectile_position,
                benilla_protocol::SessionEvent::SetProjectilePosition {
                    caster: trajectory.caster,
                    cast_id: trajectory.cast_id,
                    position: benilla_assets::coords::bevy_to_wow(hit),
                },
            )
            .unwrap();
        let mut landed = None;
        for _ in 0..20 {
            step(&mut app, 0.05);
            landed = app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .find_map(|e| match e.kind {
                    CastEventKind::GroundImpact { pos } => Some(pos),
                    _ => None,
                });
            if landed.is_some() {
                break;
            }
        }
        let landed = landed.expect("landed short");
        assert!(landed.distance(hit) < 2.0, "{landed} near {hit}");
        assert!(missiles(&mut app).is_empty());
    }

    /// Column 16 moves the launch to its attachment plus the cast offset (`0x72e1b6`); a model
    /// without that attachment keeps the release point.
    #[test]
    fn a_column_16_source_launches_from_its_attachment_plus_the_cast_offset() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        app.world_mut()
            .get_mut::<BoneAttach>(caster)
            .unwrap()
            .points
            .insert(0x16, (0, Vec3::new(0.2, 0.0, 0.0)));
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        for (tag, expect) in [
            (0x16, hand + Vec3::new(0.2, 0.5, 0.0)),
            (0x25, hand), // no such attachment: the `$CSL` point
        ] {
            let mut spawn = go(caster, target, false);
            spawn.shape.source = Some(MissileSource {
                tag: Some(benilla_formats::EffectAttach::Point(tag)),
                offset: Vec3::new(0.0, 0.5, 0.0),
            });
            app.world_mut().write_message(spawn);
            step(&mut app, 0.016);
            let launched = missiles(&mut app);
            assert_eq!(launched.len(), 1);
            assert!(
                launched[0].1.distance(expect) < 1e-5,
                "tag {tag:#x}: {} vs {expect}",
                launched[0].1
            );
            let all: Vec<Entity> = app
                .world_mut()
                .query_filtered::<Entity, With<Missile>>()
                .iter(app.world())
                .collect();
            for e in all {
                app.world_mut().despawn(e);
            }
        }
    }

    /// The impact offset rides the destination attachment's frame (`0x6ff6d0`).
    #[test]
    fn the_aim_takes_the_impact_offset_at_the_attachment() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        let chest = Vec3::new(24.0, 1.0, 0.0);
        let target = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                BoneAttach {
                    points: HashMap::from([(0x22, (0u16, Vec3::ZERO))]),
                    markers: HashMap::new(),
                },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(target, &[chest]);
        app.world_mut().entity_mut(target).insert(pose);
        let mut spawn = go(caster, target, false);
        spawn.dest_tag = Some(0x22);
        spawn.shape.impact_offset = Vec3::new(0.0, 2.0, 0.0);
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let impact = app
            .world_mut()
            .query::<&Missile>()
            .single(app.world())
            .unwrap()
            .impact;
        assert!(impact.distance(chest + Vec3::Y * 2.0) < 1e-5, "{impact}");
    }

    /// A motion row's count flies that many copies, each scripted with its own index, and the
    /// targeting kit rides the target from launch until the flight ends (`0x702986`, `0x6ff410`).
    #[test]
    fn a_motion_row_flies_its_copies_and_the_targeting_kit_rides_the_target() {
        use benilla_formats::{MissileMotionRec, SpellVisualCatalog, VisualKit};
        const KIT: u32 = 8729;
        const EFFECT: u32 = 3807;
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .add_systems(Update, move_missiles.after(spawn_missiles));
        let mut slots = [None; 9];
        slots[2] = Some(EFFECT);
        app.insert_resource(SpellVisuals(
            SpellVisualCatalog::from_tables_with_paths(
                HashMap::new(),
                HashMap::from([(
                    KIT,
                    VisualKit {
                        effect_slots: slots,
                        sound: Some(77),
                        ..Default::default()
                    },
                )]),
                HashMap::from([(EFFECT, "Spells\\Mark.mdx".to_string())]),
            )
            .with_missile_motion(
                19,
                MissileMotionRec {
                    name: "Spiral Vortex".into(),
                    script: String::new(),
                    flags: 0,
                    missile_count: 3,
                },
            ),
        ));
        app.insert_resource(crate::ui_action::Spells::empty_for_tests());
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.shape.motion = 19;
        spawn.shape.targeting_kit = KIT;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let mut copies: Vec<(u32, u32)> = app
            .world_mut()
            .query::<&MissileMotion>()
            .iter(app.world())
            .map(|m| (m.index, m.count))
            .collect();
        copies.sort_unstable();
        assert_eq!(copies, vec![(0, 3), (1, 3), (2, 3)]);
        let fx: Vec<SpellKitFx> = app
            .world_mut()
            .resource_mut::<Messages<SpellKitFx>>()
            .drain()
            .collect();
        match &fx[..] {
            [SpellKitFx::Begin {
                entity,
                persistent: true,
                effects,
                ..
            }] => {
                assert_eq!(*entity, target);
                assert_eq!(effects.len(), 1);
            }
            other => panic!("one hold on the target, got {}", other.len()),
        }
        let mut reaped = false;
        let mut impacts = 0;
        for _ in 0..80 {
            step(&mut app, 0.05);
            reaped |= app
                .world_mut()
                .resource_mut::<Messages<SpellKitFx>>()
                .drain()
                .any(|f| matches!(f, SpellKitFx::Reap { entity, .. } if entity == target));
            impacts += app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .filter(|e| {
                    e.entity == target
                        && matches!(
                            e.kind,
                            CastEventKind::Impact {
                                on_caster: false,
                                at: None,
                                ..
                            }
                        )
                })
                .count();
        }
        assert!(reaped, "the flight's end reaps the targeting kit");
        assert_eq!(impacts, 3, "every copy lands");
        assert!(missiles(&mut app).is_empty());
    }

    /// The targeting kit plays whole at stage 2 naming the caster, so its beam runs from the caster
    /// to the target, its anim is a recompute and its body and node procs run (`0x702906`,
    /// `0x702923`); the flight's end clears its models by spell and fade stamp, which its beam
    /// does not carry (`0x6ff410`, `0x744cc8`).
    #[test]
    fn the_targeting_kit_plays_whole_from_the_caster_and_its_models_end_with_the_flight() {
        use benilla_formats::{char_proc_type, CharProc, SpellVisualCatalog, VisualKit};
        const KIT: u32 = 8730;
        const EFFECT: u32 = 3807;
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .add_systems(Update, move_missiles.after(spawn_missiles));
        let mut slots = [None; 9];
        slots[2] = Some(EFFECT);
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
            HashMap::new(),
            HashMap::from([(
                KIT,
                VisualKit {
                    id: KIT,
                    effect_slots: slots,
                    anim_id: Some(14),
                    char_proc_slots: [
                        Some(CharProc {
                            ty: char_proc_type::CHAIN_CHANNEL,
                            params: [9.0, 0.0, 1.0, 0.0],
                        }),
                        Some(CharProc {
                            ty: char_proc_type::ANIM_RATE,
                            params: [0.0; 4],
                        }),
                        Some(CharProc {
                            ty: 13, // the flash
                            params: [255.0, 0.0, 0.5, 0.0],
                        }),
                        None,
                    ],
                    ..Default::default()
                },
            )]),
            HashMap::from([(EFFECT, "Spells\\Mark.mdx".to_string())]),
        )));
        app.insert_resource(crate::ui_action::Spells::empty_for_tests());
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.shape.targeting_kit = KIT;
        let spell_id = spawn.spell_id;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let plays: Vec<ChainProcPlay> = app
            .world_mut()
            .resource_mut::<Messages<ChainProcPlay>>()
            .drain()
            .collect();
        let [play] = plays[..] else {
            panic!("one beam play, got {}", plays.len());
        };
        assert_eq!(
            (play.entity, play.from, play.kit_id, play.proc.effect_id),
            (target, Some(caster), KIT, 9)
        );
        let world = app.world_mut();
        let recomputes: Vec<_> = world
            .resource_mut::<Messages<crate::creature_anim::BaseAnimRecompute>>()
            .drain()
            .map(|r| (r.entity, r.anim_id))
            .collect();
        assert_eq!(
            recomputes,
            vec![(target, 14)],
            "stage 2 compares, never plays"
        );
        assert!(world
            .resource_mut::<Messages<crate::creature_anim::EmoteAnim>>()
            .drain()
            .next()
            .is_none());
        let flashes: Vec<_> = world
            .resource_mut::<Messages<crate::aura_visual::BodyProcPlay>>()
            .drain()
            .map(|b| (b.entity, b.proc.ty))
            .collect();
        assert_eq!(flashes, vec![(target, 13)]);
        let nodes: Vec<_> = world
            .resource_mut::<Messages<crate::aura_visual::AuraProc>>()
            .drain()
            .collect();
        let [crate::aura_visual::AuraProc::Begin {
            entity,
            kit_id,
            ref nodes,
            ..
        }] = nodes[..]
        else {
            panic!("one node edge, got {nodes:?}");
        };
        assert_eq!((entity, kit_id, nodes.len()), (target, KIT, 1));
        let fx: Vec<SpellKitFx> = world
            .resource_mut::<Messages<SpellKitFx>>()
            .drain()
            .collect();
        let [SpellKitFx::Begin {
            entity,
            class: FxClass::Targeting(stamp),
            persistent: true,
            ..
        }] = fx[..]
        else {
            panic!("one stamped hold, got {}", fx.len());
        };
        assert_eq!(entity, target);
        let mut reaps = Vec::new();
        for _ in 0..40 {
            step(&mut app, 0.05);
            reaps.extend(
                app.world_mut()
                    .resource_mut::<Messages<SpellKitFx>>()
                    .drain()
                    .filter_map(|f| match f {
                        SpellKitFx::Reap {
                            entity,
                            spell_id,
                            class,
                        } => Some((entity, spell_id, class)),
                        SpellKitFx::Begin { .. } | SpellKitFx::Hold { .. } => None,
                    }),
            );
        }
        assert_eq!(
            reaps,
            vec![(target, spell_id, FxClass::Targeting(stamp))],
            "the stamped models alone; no beam clear exists for the flight"
        );
    }

    /// On a beam spell, a flag-0x1 targeting kit on a unit target waits for the beam to reach the
    /// target (`0x72af60`), and the flight keeps nothing to reap: the replay is not its (`0x6ff410`).
    #[test]
    fn a_beam_spells_flag_1_targeting_kit_waits_for_the_beam() {
        use benilla_formats::{
            char_proc_type, kit_flag, CharProc, SpellCatalog, SpellDisplay, SpellVisualCatalog,
            VisualKit, VisualStages,
        };
        const KIT: u32 = 8729;
        const CAST_KIT: u32 = 900;
        const VISUAL: u32 = 6;
        let mut app = app();
        app.init_resource::<crate::creature_anim::BeamKitWaits>();
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    cast: CAST_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([
                (
                    CAST_KIT,
                    VisualKit {
                        id: CAST_KIT,
                        char_proc_slots: [
                            Some(CharProc {
                                ty: char_proc_type::CHAIN_CAST,
                                params: [1.0, 0.0, 0.0, 0.0],
                            }),
                            None,
                            None,
                            None,
                        ],
                        ..Default::default()
                    },
                ),
                (
                    KIT,
                    VisualKit {
                        id: KIT,
                        sound: Some(77),
                        flags: kit_flag::BEAM_WAIT,
                        ..Default::default()
                    },
                ),
            ]),
        )));
        let spawn0 = go(Entity::PLACEHOLDER, Entity::PLACEHOLDER, false);
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                spawn0.spell_id,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn((
                GlobalTransform::from_xyz(24.0, 0.0, 0.0),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.shape.targeting_kit = KIT;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let rang = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Messages<SpellKitSound>>()
                .drain()
                .any(|s| matches!(s, SpellKitSound::Play { entity, .. } if entity == target))
        };
        assert!(!rang(&mut app), "the targeting kit waits");
        let waits = app.world().resource::<crate::creature_anim::BeamKitWaits>();
        assert!(!waits.is_empty(), "held on the target");
        let mut flights = app.world_mut().query::<&Missile>();
        assert!(
            flights.iter(app.world()).all(|m| m.targeting.is_none()),
            "no reap"
        );
    }

    /// On a beam spell, a ground flight's flag-0x1 targeting kit waits on the caster with its aim
    /// (`0x702941`, `0x72af60`): nothing plants at launch and the flight's end leaves the wait.
    #[test]
    fn a_beam_spells_ground_targeting_kit_waits_on_the_caster_with_its_point() {
        use benilla_formats::{
            char_proc_type, kit_flag, CharProc, SpellCatalog, SpellDisplay, SpellVisualCatalog,
            VisualKit, VisualStages,
        };
        const KIT: u32 = 8729;
        const CAST_KIT: u32 = 900;
        const VISUAL: u32 = 6;
        const EFFECT: u32 = 3807;
        let mut app = flying_app();
        app.init_resource::<crate::creature_anim::BeamKitWaits>();
        let mut slots = [None; 9];
        slots[2] = Some(EFFECT);
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    cast: CAST_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([
                (
                    CAST_KIT,
                    VisualKit {
                        id: CAST_KIT,
                        char_proc_slots: [
                            Some(CharProc {
                                ty: char_proc_type::CHAIN_CAST,
                                params: [1.0, 0.0, 0.0, 0.0],
                            }),
                            None,
                            None,
                            None,
                        ],
                        ..Default::default()
                    },
                ),
                (
                    KIT,
                    VisualKit {
                        id: KIT,
                        effect_slots: slots,
                        flags: kit_flag::BEAM_WAIT,
                        ..Default::default()
                    },
                ),
            ]),
            HashMap::from([(EFFECT, "Spells\\Base.m2".to_string())]),
        )));
        let caster = caster(&mut app, Vec3::ZERO);
        app.world_mut()
            .entity_mut(caster)
            .insert(crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            });
        let at = Vec3::new(24.0, 0.0, 0.0);
        let mut spawn = ground_go(caster, at);
        spawn.shape.targeting_kit = KIT;
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                spawn.spell_id,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let spell_id = spawn.spell_id;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let planted = |app: &mut App| {
            app.world_mut()
                .query_filtered::<(), With<crate::entities::dest_fx::GroundFx>>()
                .iter(app.world())
                .count()
        };
        assert_eq!(planted(&mut app), 0, "nothing plants at the launch");
        fly_out(&mut app);
        assert!(missiles(&mut app).is_empty(), "the flight is over");
        // The beam reaching the caster now still finds the play with its point.
        let mut waits = app
            .world_mut()
            .resource_mut::<crate::creature_anim::BeamKitWaits>();
        assert!(!waits.is_empty(), "held on the caster past the flight");
        waits.reach(
            caster,
            spell_id,
            crate::creature_anim::spell_visual::NO_BEAM_KEY,
        );
        assert_eq!(
            waits.reached_targeting_points(),
            vec![(caster, KIT, at)],
            "its point kept"
        );
    }

    /// A ground-following flight with no terrain streamed flies its line and lands on time.
    #[test]
    fn a_ground_following_flight_with_no_terrain_lands_on_time() {
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .add_systems(Update, move_missiles.after(spawn_missiles));
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.shape.follow = Some(FollowGround {
            height: 0.1,
            rate: 0.3,
            start: 0.75,
            flags: 1,
        });
        app.world_mut().write_message(spawn);
        let mut landed_at = None;
        for i in 0..40 {
            step(&mut app, 0.05);
            if app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .any(|e| matches!(e.kind, CastEventKind::Impact { .. }))
            {
                landed_at = Some(i);
                break;
            }
        }
        // A 1.0 s flight in 0.05 s steps.
        assert!(
            landed_at.is_some_and(|i| (18..=21).contains(&i)),
            "{landed_at:?}"
        );
    }

    /// A script's offset, scale and speed reach the drawn model and the flight (`0x700550`).
    #[test]
    fn a_scripted_model_draws_off_its_flight_and_retimes_it() {
        use benilla_formats::{MissileMotionRec, SpellVisualCatalog};
        let mut app = app();
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::SelfGuid>()
            .init_non_send_resource::<MotionScripts>()
            .add_systems(
                Update,
                (run_missile_motion, move_missiles)
                    .chain()
                    .after(spawn_missiles),
            );
        app.insert_resource(SpellVisuals(
            SpellVisualCatalog::from_tables(HashMap::new(), HashMap::new()).with_missile_motion(
                7,
                MissileMotionRec {
                    name: "Up".into(),
                    script: "transUp = 2
scale = 0.5
speedAbs = 48"
                        .into(),
                    flags: 0,
                    missile_count: 1,
                },
            ),
        ));
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.shape.motion = 7;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.05);
        let (pos, model, scale) = {
            let world = app.world_mut();
            let mut q = world.query::<(&Missile, &Transform)>();
            let (m, t) = q.single(world).unwrap();
            (m.pos, t.translation, t.scale.x)
        };
        assert!(
            (model - (pos + Vec3::Y * 2.0)).length() < 1e-4,
            "{model} vs {pos}"
        );
        assert_eq!(scale, 0.5);
        let mut frames = 1;
        let mut at = None;
        while at.is_none() && frames < 40 {
            step(&mut app, 0.05);
            frames += 1;
            at = app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .find_map(|e| match e.kind {
                    CastEventKind::Impact { at, .. } => Some(at),
                    _ => None,
                });
        }
        assert!((9..=12).contains(&frames), "24 yd at 48: {frames} frames");
        assert!(
            at.flatten().is_some(),
            "off its path, the kit plays at the model"
        );
    }

    /// A flying app whose spell 133 has a visual row, the area landing's gate (`0x703591`).
    fn area_app() -> App {
        use benilla_formats::{SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualStages};
        let mut app = flying_app();
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
            HashMap::from([(77, VisualStages::default())]),
            HashMap::new(),
        )));
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                133,
                SpellDisplay {
                    visual: 77,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        app
    }

    /// A placed object of `kind` at `at`, indexed under `guid`.
    fn listed(app: &mut App, guid: u64, at: Vec3, kind: benilla_protocol::EntityKind) -> Entity {
        let e = app
            .world_mut()
            .spawn((
                GlobalTransform::from_translation(at),
                crate::net::NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
        e
    }

    fn list(units: &[(u64, u8)], results: bool, bounce: bool) -> TargetList {
        TargetList {
            units: units
                .iter()
                .map(|&(guid, result)| ListedUnit {
                    guid,
                    result,
                    reflect: 0,
                })
                .collect(),
            results,
            bounce,
            points: Vec::new(),
        }
    }

    fn located(caster: Entity, at: Vec3, list: TargetList) -> MissileSpawn {
        MissileSpawn {
            list: Some(list),
            ..ground_go(caster, at)
        }
    }

    /// An area landing plays the area kit on the caster, then each listed unit's kits; a guid
    /// naming no unit, or an object that is not one, is passed over (`0x700e20`, `0x700fb1`).
    #[test]
    fn an_area_landing_plays_each_listed_unit_after_the_area_kit() {
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(2.4, 0.0, 0.0);
        let a = listed(&mut app, 1, at, EntityKind::Unit);
        listed(&mut app, 3, at, EntityKind::GameObject);
        let b = listed(&mut app, 4, at, EntityKind::Player);
        let units = list(&[(1, 0), (2, 0), (3, 0), (4, 0)], false, false);
        app.world_mut().write_message(located(caster, at, units));
        let (impacts, _, words) = fly_out(&mut app);
        let kinds: Vec<(Entity, CastEventKind)> =
            impacts.iter().map(|e| (e.entity, e.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (caster, CastEventKind::GroundImpact { pos: at }),
                (a, CastEventKind::AreaImpact { caster }),
                (b, CastEventKind::AreaImpact { caster }),
            ]
        );
        assert!(words.is_empty());
    }

    /// A list that came with outcomes words its misses and plays no unit kit (`0x700fc3`); a spell
    /// with no visual row plays none of the list (`0x703591`).
    #[test]
    fn an_area_landing_with_outcomes_words_only_its_misses() {
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(2.4, 0.0, 0.0);
        listed(&mut app, 1, at, EntityKind::Unit);
        let b = listed(&mut app, 2, at, EntityKind::Unit);
        let units = list(&[(1, 0), (2, 3)], true, false);
        app.world_mut()
            .write_message(located(caster, at, units.clone()));
        let (impacts, _, words) = fly_out(&mut app);
        assert_eq!(impacts.len(), 1, "the area kit alone");
        assert_eq!(words, vec![(b, 3)]);

        app.world_mut()
            .resource_mut::<crate::ui_action::Spells>()
            .catalog = benilla_formats::SpellCatalog::from_displays(HashMap::new());
        app.world_mut().write_message(located(caster, at, units));
        let (_, _, words) = fly_out(&mut app);
        assert!(words.is_empty(), "no visual row, no list");
    }

    /// Of a motion row's copies only the first words a miss (`0x700ff1`); each lands its kit.
    #[test]
    fn only_the_first_copy_words_an_area_landing_s_miss() {
        use benilla_formats::MissileMotionRec;
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let visuals = app
            .world_mut()
            .remove_resource::<SpellVisuals>()
            .expect("area app visuals");
        app.insert_resource(SpellVisuals(visuals.0.with_missile_motion(
            19,
            MissileMotionRec {
                name: "Spread".into(),
                script: String::new(),
                flags: 0,
                missile_count: 3,
            },
        )));
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(2.4, 0.0, 0.0);
        let b = listed(&mut app, 2, at, EntityKind::Unit);
        let mut spawn = located(caster, at, list(&[(2, 7)], true, false));
        spawn.shape.motion = 19;
        app.world_mut().write_message(spawn);
        let (impacts, _, words) = fly_out(&mut app);
        assert_eq!(impacts.len(), 3, "each copy plays the area kit");
        assert_eq!(words, vec![(b, 7)], "the word once");
    }

    /// With its caster gone the area kit plays through our own player (`0x700ed9`).
    #[test]
    fn an_area_landing_with_no_caster_plays_on_our_player() {
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let me = app
            .world_mut()
            .spawn((GlobalTransform::default(), crate::net::SelfPlayer))
            .id();
        let at = Vec3::new(2.4, 0.0, 0.0);
        app.world_mut().write_message(ground_go(caster, at));
        step(&mut app, 0.02);
        app.world_mut().despawn(caster);
        let (impacts, ..) = fly_out(&mut app);
        let hosts: Vec<Entity> = impacts.iter().map(|e| e.entity).collect();
        assert_eq!(hosts, vec![me]);
    }

    /// A unit target gone mid-flight leaves the flight on its last aim, to land on its area there
    /// (`0x70164d`).
    #[test]
    fn a_target_gone_mid_flight_lands_on_its_area_at_the_last_aim() {
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(2.4, 0.0, 0.0);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_translation(at))
            .id();
        app.world_mut().write_message(go(caster, target, false));
        step(&mut app, 0.02);
        app.world_mut().despawn(target);
        let (impacts, ..) = fly_out(&mut app);
        let kinds: Vec<(Entity, CastEventKind)> =
            impacts.iter().map(|e| (e.entity, e.kind)).collect();
        assert_eq!(
            kinds,
            vec![(caster, CastEventKind::GroundImpact { pos: at })]
        );
    }

    /// A missile at an object that is not a unit is not homed: it lands on its area at the object
    /// at once (`0x7023fe`).
    #[test]
    fn a_missile_at_an_object_lands_on_its_area_at_once() {
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(24.0, 0.0, 0.0);
        let chest = listed(&mut app, 9, at, benilla_protocol::EntityKind::GameObject);
        app.world_mut().write_message(go(caster, chest, false));
        step(&mut app, 0.016);
        assert!(missiles(&mut app).is_empty(), "nothing flies");
        let kinds: Vec<(Entity, CastEventKind)> = app
            .world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .drain()
            .map(|e| (e.entity, e.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![(caster, CastEventKind::GroundImpact { pos: at })]
        );
    }

    fn hopping(caster: Entity, units: &[(u64, u8)]) -> MissileSpawn {
        MissileSpawn {
            targets: Vec::new(),
            list: Some(list(units, true, true)),
            ..go(caster, caster, false)
        }
    }

    /// A hopping list flies on from each unit to the next in order, passing over a guid naming
    /// none, each landing taking its own outcome: one missile all along (`0x702a60`).
    #[test]
    fn a_hopping_list_flies_from_unit_to_unit() {
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let a = listed(&mut app, 1, Vec3::new(2.4, 0.0, 0.0), EntityKind::Unit);
        let b = listed(&mut app, 2, Vec3::new(2.4, 0.0, 2.4), EntityKind::Unit);
        let c = listed(&mut app, 3, Vec3::new(0.0, 0.0, 2.4), EntityKind::Unit);
        app.world_mut()
            .write_message(hopping(caster, &[(1, 0), (8, 0), (2, 3), (3, 0)]));
        step(&mut app, 0.016);
        assert_eq!(missiles(&mut app).len(), 1, "one missile for the list");
        let (impacts, _, words) = fly_out(&mut app);
        let hit: Vec<Entity> = impacts
            .iter()
            .filter(|e| matches!(e.kind, CastEventKind::Impact { .. }))
            .map(|e| e.entity)
            .collect();
        assert_eq!(hit, vec![a, c]);
        assert_eq!(words, vec![(b, 3)]);
        assert!(missiles(&mut app).is_empty(), "done after the last unit");
    }

    /// A hop onto guids that all name no unit lands on the area at once, at the last landing
    /// (`0x7023db`, `0x7023fe`).
    #[test]
    fn a_hop_with_no_unit_left_lands_on_the_area() {
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(2.4, 0.0, 0.0);
        let a = listed(&mut app, 1, at, EntityKind::Unit);
        app.world_mut()
            .write_message(hopping(caster, &[(1, 0), (8, 0)]));
        let (impacts, ..) = fly_out(&mut app);
        let kinds: Vec<(Entity, CastEventKind)> =
            impacts.iter().map(|e| (e.entity, e.kind)).collect();
        assert_eq!(kinds.len(), 2);
        assert_eq!(kinds[0].0, a);
        assert_eq!(kinds[1], (caster, CastEventKind::GroundImpact { pos: at }));
    }

    /// A blocked landing lodges in the shield, and a unit left takes it straight out again on the
    /// hop, still playing Stuck, its flight loop restarted; the last unit keeps it (`0x702fc0`,
    /// `0x6fee00`, `0x7022d0`).
    #[test]
    fn a_blocked_hop_lodges_then_flies_on_playing_stuck() {
        use benilla_protocol::EntityKind;
        const LOOP: u32 = 31;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let path = "Spells\\Bolt.m2".to_string();
        let stuck_node = AnimationNodeIndex::new(3);
        let mut model = super::super::empty_shell();
        model.animations = Some(benilla_assets::ModelAnimations {
            graph: Handle::default(),
            clips: vec![benilla_assets::AnimClip {
                anim_id: STUCK_ANIM,
                seq_index: 0,
                node: stuck_node,
                looping: true,
                duration: 1.0,
                move_speed: 0.0,
                blend_time: 0.0,
                bounds_center: Vec3::ZERO,
                bounds_radius: 0.0,
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ZERO,
                events: std::sync::Arc::new([]),
                arm_nodes: None,
                upper_node: None,
                frequency: 0,
                replay: (0, 0),
                poses_bones: false,
            }],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        });
        app.world_mut()
            .resource_mut::<SpellFx>()
            .models
            .insert(path.clone(), model);
        let shielded = |app: &mut App, guid: u64, at: Vec3| {
            let unit = listed(app, guid, at, EntityKind::Unit);
            let shield = BoneAttach {
                points: HashMap::from([(disposition::SHIELD_ATTACHMENT, (0u16, Vec3::ZERO))]),
                markers: HashMap::new(),
            };
            let pose = benilla_world::testing::test_rig_pose(unit, &[at]);
            app.world_mut().entity_mut(unit).insert((shield, pose));
            unit
        };
        let a = shielded(&mut app, 1, Vec3::new(2.4, 0.0, 0.0));
        let b = shielded(&mut app, 2, Vec3::new(2.4, 0.0, 2.4));
        let mut spawn = hopping(caster, &[(1, 5), (2, 5)]);
        spawn.path = Some(path);
        spawn.missile_sound = Some(LOOP);
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let flight = app
            .world_mut()
            .query_filtered::<Entity, With<Missile>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(flight)
            .insert(AnimationPlayer::default())
            .get_mut::<Missile>()
            .unwrap()
            .parts_spawned = true;
        app.world_mut()
            .resource_mut::<Messages<MissileSound>>()
            .clear();
        let mut starts = 0;
        let mut words = Vec::new();
        let mut stuck_in_flight = false;
        for _ in 0..40 {
            step(&mut app, 0.02);
            starts += app
                .world_mut()
                .resource_mut::<Messages<MissileSound>>()
                .drain()
                .filter(|s| {
                    matches!(
                        s,
                        MissileSound::Start {
                            kit_sound: LOOP,
                            ..
                        }
                    )
                })
                .count();
            words.extend(
                app.world_mut()
                    .resource_mut::<Messages<MissileMiss>>()
                    .drain()
                    .map(|w| w.anchor),
            );
            let on_b = app
                .world()
                .get::<Missile>(flight)
                .is_some_and(|m| matches!(m.aim, Aim::Unit { target, .. } if target == b));
            let playing = app
                .world()
                .get::<AnimationPlayer>(flight)
                .is_some_and(|p| p.is_playing_animation(stuck_node));
            stuck_in_flight |= on_b && playing;
        }
        assert_eq!(words, vec![a, b]);
        assert!(stuck_in_flight, "the hop flies on playing Stuck");
        assert_eq!(starts, 1, "the hop restarts the flight loop");
        let lodged: Vec<Entity> = app
            .world_mut()
            .query::<&StuckMissile>()
            .iter(app.world())
            .map(|s| s.target)
            .collect();
        assert_eq!(lodged, vec![b], "the last unit keeps it");
    }

    /// A blocked landing with no model to lodge hops on all the same (`0x702fc0`).
    #[test]
    fn an_unmodelled_blocked_hop_flies_on() {
        use benilla_protocol::EntityKind;
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let a = listed(&mut app, 1, Vec3::new(2.4, 0.0, 0.0), EntityKind::Unit);
        let b = listed(&mut app, 2, Vec3::new(2.4, 0.0, 2.4), EntityKind::Unit);
        app.world_mut()
            .write_message(hopping(caster, &[(1, 5), (2, 0)]));
        let (impacts, _, words) = fly_out(&mut app);
        assert_eq!(words, vec![(a, 5)]);
        let hit: Vec<Entity> = impacts.iter().map(|e| e.entity).collect();
        assert_eq!(hit, vec![b]);
        let stuck = app
            .world_mut()
            .query::<&StuckMissile>()
            .iter(app.world())
            .count();
        assert_eq!(stuck, 0);
    }

    /// A points missile lands at its first point, offset-free, and plays the target kit at each
    /// point through the caster, a transport's point where the transport is at the landing
    /// (`0x734191`, `0x701150`).
    #[test]
    fn a_points_missile_plays_the_target_kit_at_each_point() {
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let deck = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(0.0, 0.0, 10.0))
            .id();
        let first = Vec3::new(2.4, 0.0, 0.0);
        let local = Vec3::new(1.0, 0.0, 0.0);
        let mut spawn = located(
            caster,
            first,
            TargetList {
                points: vec![
                    DeckPoint {
                        at: first,
                        deck: None,
                        world: first,
                    },
                    DeckPoint {
                        at: local,
                        deck: Some(deck),
                        world: Vec3::new(1.0, 0.0, 10.0),
                    },
                ],
                bounce: true,
                ..TargetList::default()
            },
        );
        spawn.shape.impact_offset = Vec3::new(0.0, 5.0, 0.0);
        app.world_mut().write_message(spawn);
        step(&mut app, 0.02);
        app.world_mut()
            .entity_mut(deck)
            .insert(GlobalTransform::from_xyz(0.0, 0.0, 12.0));
        let (impacts, ..) = fly_out(&mut app);
        let kinds: Vec<(Entity, CastEventKind)> =
            impacts.iter().map(|e| (e.entity, e.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (caster, CastEventKind::GroundImpact { pos: first }),
                (caster, CastEventKind::PointImpact { pos: first }),
                (
                    caster,
                    CastEventKind::PointImpact {
                        pos: Vec3::new(1.0, 0.0, 12.0)
                    }
                ),
            ]
        );
    }

    /// A point on a transport is aimed at through it every step, so the flight follows a moving
    /// deck; with the transport gone it lands where the point last was (`0x6ff320`).
    #[test]
    fn a_point_on_a_transport_is_followed_every_step() {
        for gone in [false, true] {
            let mut app = area_app();
            let caster = caster(&mut app, Vec3::ZERO);
            let deck = app
                .world_mut()
                .spawn(GlobalTransform::from_xyz(0.0, 0.0, 10.0))
                .id();
            let local = Vec3::new(1.0, 0.0, 0.0);
            let mut spawn = ground_go(caster, Vec3::new(1.0, 0.0, 10.0));
            spawn.aim_deck = Some((deck, local));
            app.world_mut().write_message(spawn);
            step(&mut app, 0.02);
            app.world_mut()
                .entity_mut(deck)
                .insert(GlobalTransform::from_xyz(0.0, 0.0, 14.0));
            step(&mut app, 0.02);
            if gone {
                app.world_mut().despawn(deck);
            } else {
                app.world_mut()
                    .entity_mut(deck)
                    .insert(GlobalTransform::from_xyz(0.0, 0.0, 16.0));
            }
            let (impacts, ..) = fly_out(&mut app);
            let landed = if gone { 14.0 } else { 16.0 };
            assert_eq!(
                impacts.iter().map(|e| e.kind).collect::<Vec<_>>(),
                vec![CastEventKind::GroundImpact {
                    pos: Vec3::new(1.0, 0.0, landed)
                }],
                "transport gone: {gone}"
            );
        }
    }

    /// A unit list naming no unit lands at once at its first listed object, here a GameObject.
    #[test]
    fn a_hopping_list_with_no_unit_lands_at_its_first_object() {
        let mut app = area_app();
        let caster = caster(&mut app, Vec3::ZERO);
        let at = Vec3::new(6.0, 0.0, 0.0);
        listed(&mut app, 5, at, benilla_protocol::EntityKind::GameObject);
        app.world_mut().write_message(hopping(caster, &[(5, 0)]));
        let (impacts, ..) = fly_out(&mut app);
        let kinds: Vec<(Entity, CastEventKind)> =
            impacts.iter().map(|e| (e.entity, e.kind)).collect();
        assert_eq!(
            kinds,
            vec![(caster, CastEventKind::GroundImpact { pos: at })]
        );
    }
}
