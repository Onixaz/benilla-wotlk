//! The spell-visual data (`SpellVisual.dbc` × `SpellVisualKit.dbc`) and the cast-edge router, the
//! one place spell ids resolve to animations, sounds and effect models.

#[cfg(test)]
mod tests;

use std::cell::Cell;

use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;

use benilla_formats::{SpellVisualCatalog, VisualKit, VisualStages};
use benilla_protocol::EntityKind;

use crate::aura_visual::AuraProc;
use crate::entities::ItemDisplays;
use crate::items::{ItemObject, Items};
use crate::net::{FieldChanged, NetCommands, NetEntity, ObjectStore};
use crate::unit_auras::UnitAuras;
use benilla_assets::{LockRecover, WorldAssets};

use super::{
    BaseAnimRecompute, CastEvent, CastEventKind, CastHold, EmoteAnim, SpellGoTargets, WoundAnim,
};

/// The 1.12 client's missile model for an unresolvable visual, shipped in the MPQs.
const ERROR_CUBE: &str = "Spells\\ErrorCube.mdx";

/// The [`SpellKitFx`] reap key for the loot sparkle, outside the spell id range; the reference keys
/// these nodes by attach tag and node flag. A unit wears at most one.
const LOOT_FX_KEY: u32 = u32::MAX;

/// The attachment the loot sparkle, ding and mount poof hang from, else the unit base. A per-effect
/// table value, not a constant: a new user reads its row.
const HARDCODED_FX_ATTACH: u16 = 0x13;

/// The level-up effect's `SpellVisualEffectName` name.
const LEVEL_UP_EFFECT: &str = "HARDCODED Unit Level Up";

/// The slots an area kit plays at a point: head, base and world (`0x74566c`).
const AREA_PLAY_TAGS: [u16; 3] = [0x14, 0x13, benilla_formats::WORLD_EFFECT_TAG];

/// The mount cloud, hardcoded index 6: row 1185, the druid-morph puff.
const MOUNT_POOF_EFFECT: &str = "HARDCODED Mount Poof";

/// `SpellVisual.dbc` × `SpellVisualKit.dbc`; absent without client data.
#[derive(Resource)]
pub(crate) struct SpellVisuals(pub(crate) SpellVisualCatalog);

/// A kit's sound edge (kit field 13 → `SoundEntries.dbc`), rung at kit start, consumed by
/// `crate::sound` in emission order.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SpellKitSound {
    /// Ring this kit at the unit; a looping kit (`SoundEntries` flag 0x200) becomes the unit's
    /// tracked hold loop until [`Self::StopHold`].
    Play { entity: Entity, kit_sound: u32 },
    /// The cast or channel hold ended: stop the unit's hold loop.
    StopHold { entity: Entity },
    /// Ring this kit at a world point, the kit play's `extra` override: a missile's ground arrival.
    /// Always a one-shot: no shipped ground-arrival kit loops.
    PlayAt { pos: Vec3, kit_sound: u32 },
    /// Stop this kit's sound on the unit as its aura leaves (0.15 s fade).
    StopKit { entity: Entity, kit_sound: u32 },
}

/// A kit's camera shake (kit field 14 → `SpellEffectCameraShakes.dbc`), consumed by
/// [`crate::camera_shake`]: once per kit play at every stage, never cancelled.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SpellKitShake {
    /// At the unit's position.
    Play { entity: Entity, group: u32 },
    /// At a world point, the kit play's `extra` override: a missile's ground arrival.
    PlayAt { pos: Vec3, group: u32 },
}

/// Which owner's reap can kill a persistent effect. The reference's reap, by spell id and a force
/// flag, spares stage-2 nodes (flag `0x1000`) unless `force` is set, which only the aura-remove
/// path does, so a cast's GO never takes the same spell's aura models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FxClass {
    /// Precast and channel holds, reaped by the cast router's edges.
    Hold,
    /// Stage 2 under a live aura, reaped only by [`arm_aura_state_fx`].
    AuraState,
}

/// The kit stage an instance's model runs its animation lifecycle as: the reference's per-stage
/// completion callback. Independent of [`FxClass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FxStage {
    /// Stages 0 and 1: destroyed at the first completion.
    OneShot,
    /// Stage 2: after birth, loop `Hold` (158) if the model has one, `Decay` (159) on reap; a model
    /// with no `Hold` stays parked on its birth.
    State,
    /// Stages 3 and 4: re-arm the sequence that just completed, forever.
    Relive,
}

/// One effect model a play hangs on a unit. Record id and tag are the reference's same-slot replace
/// key: the effect add first destroys the owner's nodes with the same record at the same tag, while
/// two rows naming one `.mdx` coexist.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FxSlot {
    /// The M2 attachment, or [`benilla_formats::WORLD_EFFECT_TAG`] for the field-12 world plant,
    /// which the replace walk skips.
    pub(crate) tag: u16,
    /// The `SpellVisualEffectName` record id.
    pub(crate) effect: u32,
    /// The model-cache key.
    pub(crate) path: String,
    /// The model's uniform local scale, 1 for a kit slot; a GameObject's loot sparkle sizes to its
    /// object (`0x70d0f1`).
    pub(crate) scale: f32,
    /// The `SpellVisualKitModelAttach` row that places it, its offset and turn (`0x6f84f0`).
    pub(crate) attach: Option<benilla_formats::ModelAttach>,
}

/// A kit's attach-point effect models (kit fields 3 to 11 → `SpellVisualEffectName`), consumed by
/// `crate::entities::spell_fx` in emission order.
#[derive(Message, Clone)]
pub(crate) enum SpellKitFx {
    /// Attach the kit's models. A persistent one lives until a matching [`Self::Reap`] and replaces
    /// the unit's live persistent instances of the same `(spell_id, class)`; otherwise it ends
    /// after one pass of its first sequence.
    Begin {
        entity: Entity,
        spell_id: u32,
        persistent: bool,
        class: FxClass,
        /// The models' animation lifecycle ([`FxStage`]).
        stage: FxStage,
        effects: Vec<FxSlot>,
    },
    /// Despawn the unit's persistent effects matching `(spell_id, class)`.
    Reap {
        entity: Entity,
        spell_id: u32,
        class: FxClass,
    },
}

/// A kit play whose `CharProc` slots name a beam, the dispatcher's chain case, run at the kit tail
/// and from the channel poll. Consumed by `crate::entities::chain_beam`.
#[derive(Message, Clone, Copy)]
pub(crate) struct ChainProcPlay {
    /// The caster end, owner of the hop array.
    pub(crate) entity: Entity,
    /// `0` for a bare kit push, which never builds a beam.
    pub(crate) spell_id: u32,
    pub(crate) proc: benilla_formats::ChainProc,
    /// The playing kit, which a location beam's area kit must differ from (`0x726b2c`).
    pub(crate) kit_id: u32,
}

/// One projectile per target on the GO's Speed > 0 branch, consumed by `crate::entities::missile`.
#[derive(Message, Clone)]
pub(crate) struct MissileSpawn {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    /// `SpellVisual` column 8's model, or `ERROR_CUBE` for an unresolvable id; `None` when column 8
    /// is below 1 or there is no row, and the spawner flies the wire ammo model.
    pub(crate) path: Option<String>,
    /// The GO's ammo display, flown when [`Self::path`] is `None`.
    pub(crate) ammo_display_id: Option<u32>,
    /// The attach tag the missile homes to
    /// ([`benilla_formats::VisualStages::missile_dest_attachment`]); `None` aims at the target's
    /// base.
    pub(crate) dest_tag: Option<u16>,
    /// `Spell.dbc` Speed; flight time is distance over speed.
    pub(crate) speed: f32,
    /// `None` for a hit; for a miss the `SpellMissInfo`, where the missile still flies.
    pub(crate) targets: Vec<(Entity, Option<u8>)>,
    /// Each reflecting target and the outcome on the caster of the flight it sends back.
    pub(crate) reflects: Vec<(Entity, u8)>,
    /// A reflect of this spell glances off instead (`AttributesEx7 & 0x20000000`, `0x732ff0`).
    pub(crate) reflect_glances: bool,
    /// The model's `SpellVisualEffectName` scale (`0x732ff0`).
    pub(crate) scale: f32,
    /// With no targets and a DEST point on the GO, one projectile flies at the point, arriving as
    /// [`super::CastEventKind::GroundImpact`]. The reference latches on `flags & 0x60` and aims a
    /// SOURCE-only cast at a zeroed DEST point; this launches only with a real DEST
    /// (`flags & 0x40`), a zeroed aim being a quirk not aped.
    pub(crate) ground_aim: Option<Vec3>,
    /// `SpellVisual` field 10: the sound the projectile loops in flight (the missile's `+0x40`).
    pub(crate) missile_sound: Option<u32>,
    /// Launch at the cast animation's release event, not at GO; the reference launches at GO only
    /// when the cast kit plays no body animation.
    pub(crate) awaits_release: bool,
    /// The GO's trajectory, with the spell's `SpellMissile.dbc` row.
    pub(crate) trajectory: Option<(super::Trajectory, Option<benilla_formats::SpellMissileRec>)>,
    /// The visual rows' launch, aim, ground and motion columns.
    pub(crate) shape: crate::entities::MissileShape,
    /// A fixed launch point, for a missile with no caster to leave from (`0x802825`).
    pub(crate) from: Option<Vec3>,
}

/// Load `SpellVisual.dbc` and `SpellVisualKit.dbc` off the patch chain at startup.
pub(super) fn load_spell_visuals(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_visual_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!(
                "anim: {} SpellVisual / {} SpellVisualKit rows",
                cat.len(),
                cat.kit_len()
            );
            commands.insert_resource(SpellVisuals(cat));
        }
        Err(e) => warn!("anim: SpellVisual/SpellVisualKit failed to load: {e:#}"),
    }
}

/// The lookups behind the ranged fallback: caster → ranged weapon display → `ItemDisplayInfo`
/// column 10.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct WeaponVisualSrc<'w, 's> {
    displays: Option<Res<'w, ItemDisplays>>,
    items: Option<Res<'w, Items>>,
    net: Option<Res<'w, NetCommands>>,
    // `Without<ItemObject>`: an item's store overlaps the unit indices, so a unit read of it
    // answers item fields.
    units: Query<'w, 's, (Option<&'static NetEntity>, &'static ObjectStore), Without<ItemObject>>,
}

impl WeaponVisualSrc<'_, '_> {
    /// The caster's ranged weapon's substitute `SpellVisual` id.
    fn caster(&mut self, caster: Entity) -> Option<u32> {
        let (net_entity, store) = self.units.get(caster).ok()?;
        let s = &store.0;
        let display_id = match net_entity?.kind {
            EntityKind::Unit => s
                .unit_virtual_item_entry(2)
                .and_then(|e| self.displays.as_deref()?.item_dbc.get(&e))
                .map(|r| r.display),
            // 17: vmangos `EQUIPMENT_SLOT_RANGED`.
            EntityKind::Player => s
                .player_visible_item_entry(17)
                .filter(|e| *e != 0)
                .and_then(|entry| self.items.as_deref()?.held(entry, self.net.as_deref()?))
                .map(|t| t.display_info_id),
            _ => None,
        }
        .filter(|d| *d != 0)?;
        let visual = self
            .displays
            .as_deref()?
            .catalog
            .get(display_id)?
            .spell_visual;
        (visual != 0).then_some(visual)
    }
}

/// The violence level below 2, as [`track_violence_level`] keeps it from `violenceLevel`.
static LOW_VIOLENCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The spell's visual pick (`0x7fa290`): below violence level 2 a spell's second visual, when it
/// has one.
pub(crate) fn spell_visual_id(d: &benilla_formats::SpellDisplay) -> u32 {
    pick_visual(d, LOW_VIOLENCE.load(std::sync::atomic::Ordering::Relaxed))
}

fn pick_visual(d: &benilla_formats::SpellDisplay, low_violence: bool) -> u32 {
    if low_violence && d.visual_low_violence != 0 {
        d.visual_low_violence
    } else {
        d.visual
    }
}

/// Follow `violenceLevel` into [`LOW_VIOLENCE`] when the CVars change.
pub(super) fn track_violence_level(cvars: Option<Res<crate::cvars::Cvars>>) {
    if let Some(c) = cvars.filter(|c| c.is_changed()) {
        let low = super::blood::violence_level(Some(&c)) < 2;
        LOW_VIOLENCE.store(low, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A spell's effective `SpellVisual` row ([`spell_visual_id`]). A ranged spell (`Attributes & 0x2`)
/// fills its row's zero slots, never state or channel, from the caster's ranged weapon visual; a
/// basic shot has no row and takes it whole.
fn resolve_stages(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualStages> {
    let def = spells.catalog.get(spell_id)?;
    let own = visuals.stages(spell_visual_id(def)).copied();
    if !def.ranged_slot() {
        return own; // no fill
    }
    let Some(weapon) = weapon_visual().and_then(|v| visuals.stages(v)) else {
        return own;
    };
    // No own row: the reference zeroes the row before the same fill.
    Some(own.unwrap_or_default().merged_over_weapon(weapon))
}

/// One stage's kit from `spell_id`'s visual chain.
fn resolve_kit(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    stage: impl Fn(&VisualStages) -> u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualKit> {
    let kit_id = stage(&resolve_stages(spells, visuals, spell_id, weapon_visual)?);
    (kit_id != 0)
        .then(|| visuals.kit(kit_id).copied())
        .flatten()
}

/// [`resolve_kit`] plus one `fx` trace line saying why a basic shot's kit did or did not resolve:
/// `weapon=not-consulted` (not ranged), `weapon=none` (no ranged visual) or the weapon visual id.
fn resolve_kit_traced(
    stage_name: &str,
    entity: Entity,
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    stage: impl Fn(&VisualStages) -> u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualKit> {
    // `None`: the fallback never ran (a non-ranged spell); `Some(v)`: it ran and returned `v`.
    let consulted: Cell<Option<Option<u32>>> = Cell::new(None);
    let kit = resolve_kit(spells, visuals, spell_id, &stage, || {
        let visual = weapon_visual();
        consulted.set(Some(visual));
        visual
    });
    if benilla_assets::trace::enabled() {
        let def = spells.catalog.get(spell_id);
        let weapon = match consulted.get() {
            None => "not-consulted".to_string(),
            Some(None) => "none".to_string(),
            Some(Some(visual)) => visual.to_string(),
        };
        // Re-derived from the recorded weapon visual, never a second lookup.
        let kit_id = resolve_stages(spells, visuals, spell_id, || consulted.get().flatten())
            .map_or(0, |s| stage(&s));
        benilla_assets::trace::line(
            "fx",
            &format!(
                "kit {stage_name} unit={entity} spell={spell_id} own_visual={} ranged_slot={} \
                 weapon={weapon} kit={kit_id} anim={:?}",
                def.map_or(0, spell_visual_id),
                def.is_some_and(|d| d.ranged_slot()),
                kit.and_then(|k| k.anim_id),
            ),
        );
    }
    kit
}

/// The in-flight cast's strike sound for the `$TRD` event: `SpellVisual` dword 14 (Mining's 1143
/// "Mining Impact"). No ranged fallback: `$TRD` rides work animations only.
pub(crate) fn held_strike_sound(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
) -> Option<u32> {
    resolve_stages(spells, visuals, spell_id, || None)?.strike_sound
}

/// Hold a missile's targeting kit on its target, its models and sound (`0x702906`).
pub(crate) fn arm_targeting_kit(
    target: Entity,
    spell_id: u32,
    kit: &VisualKit,
    visuals: &SpellVisualCatalog,
    fx: &mut MessageWriter<SpellKitFx>,
    sounds: &mut MessageWriter<SpellKitSound>,
) {
    let effects = resolve_kit_effects(visuals, kit);
    if !effects.is_empty() {
        fx.write(SpellKitFx::Begin {
            entity: target,
            spell_id,
            persistent: true,
            class: FxClass::Hold,
            stage: FxStage::State,
            effects,
        });
    }
    if let Some(kit_sound) = kit.sound {
        sounds.write(SpellKitSound::Play {
            entity: target,
            kit_sound,
        });
    }
}

/// A reached ground-aim targeting kit at its point: its area-subset models loop there while the
/// caster holds them (`0x744ac0`), and its sound rings there (`0x74547b`).
fn play_point_targeting_kit(
    caster: Entity,
    kit: &VisualKit,
    at: Vec3,
    visuals: &SpellVisualCatalog,
    sounds: &mut MessageWriter<SpellKitSound>,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    for (path, attach, scale) in area_kit_models(visuals, kit) {
        bursts.write(crate::entities::dest_fx::GroundBurst {
            path: path.to_string(),
            pos: at,
            attach,
            scale,
            turn: Quat::IDENTITY,
            held_by: Some(caster),
        });
    }
    if let Some(kit_sound) = kit.sound {
        sounds.write(SpellKitSound::PlayAt { pos: at, kit_sound });
    }
}

/// A kit's effect models on a unit: its slots, its weapon effects (`0x73a6c0`) and its
/// `SpellVisualKitModelAttach` rows (`0x745a7e`); one naming no model path is skipped.
fn resolve_kit_effects(visuals: &SpellVisualCatalog, kit: &VisualKit) -> Vec<FxSlot> {
    let slot = |tag, effect, attach| {
        visuals.effect_path(effect).map(|path| FxSlot {
            tag,
            effect,
            path: path.to_string(),
            scale: 1.0,
            attach,
        })
    };
    let rows = visuals.model_attachments(kit.id).iter().filter_map(|a| {
        let tag = u16::try_from(a.attachment).unwrap_or(benilla_formats::WORLD_EFFECT_TAG);
        slot(tag, a.effect, Some(*a))
    });
    kit.effects()
        .chain(kit.weapon_effects())
        .filter_map(|(tag, effect)| slot(tag, effect, None))
        .chain(rows)
        .collect()
}

/// A kit's weapon effects stow the unit's weapons unless the spell keeps its sheath,
/// `Attributes & 0x40000` (`0x73a717`); a ranged-slot spell then draws its ranged weapon again, as
/// the anim leg that follows does (`0x73b181`).
fn stow_for_weapon_effects(
    sheaths: &mut MessageWriter<super::SheathRequest>,
    spells: &crate::ui_action::Spells,
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
) {
    if kit.weapon_effects().next().is_none() {
        return;
    }
    let def = spells.catalog.get(spell_id);
    if def.is_some_and(|d| d.attributes & 0x40000 != 0) {
        return;
    }
    let sheath = |state| super::SheathRequest {
        entity,
        state,
        ceremony: false,
    };
    sheaths.write(sheath(0));
    if def.is_some_and(|d| d.ranged_slot()) {
        sheaths.write(sheath(2));
    }
}

/// The stage-2 lifecycle a kit's models run: kit flag `0x20` re-arms the birth (`0x7449e2`).
fn state_stage(kit: &VisualKit) -> FxStage {
    if kit.flags & benilla_formats::kit_flag::AURA_LOOPS_BIRTH != 0 {
        FxStage::Relive
    } else {
        FxStage::State
    }
}

/// A server-pushed kit (`SMSG_PLAY_SPELL_VISUAL` at stage 0, `SMSG_PLAY_SPELL_IMPACT` at stage 1),
/// played with spell id 0: its effects end only on their own (`0x800610`/`0x8006c0`).
#[derive(Message, Clone, Copy)]
pub(crate) struct KitPush {
    pub(crate) entity: Entity,
    pub(crate) kit_id: u32,
    /// [`super::PlaySeq`] stamp at the wire drain, in packet order.
    pub(crate) seq: u64,
    /// The kit play's stage: 0 for the visual (`0x800610`) and the environmental damage kit
    /// (`0x755365`), 1 for the impact (`0x8006c0`); stage 0 latches the special idle (`0x73b38e`).
    pub(crate) stage: u8,
}

/// How long a kit waits for its beam before it is dropped unplayed (`0x72af9f`, `0x728196`).
const BEAM_WAIT_SECS: f32 = 10.0;

/// The attachment key of an ordinary play and of every connect but a target-less bolt's (`0x7fb15a`).
pub(crate) const NO_BEAM_KEY: i32 = -1;

/// What a held play does once its beam reaches it.
enum WaitPlay {
    /// The unit's kit play, with the point of a play split between the unit and a point (`0x7280c9`).
    Kit {
        kit: Box<VisualKit>,
        play: KitPlay,
        seq: u64,
        at: Option<Vec3>,
    },
    /// An area kit the unit plays at a point: a location beam's (`0x726b2c`), the GO's instant
    /// area kit (`0x80ee85`) or a landing's (`0x700e20`).
    Area { kit_id: u32, pos: Vec3 },
    /// An aura's state kit (`0x724c86`), armed by [`arm_aura_state_fx`].
    AuraState,
    /// A cast's precast kit, its hold and loop.
    Precast { kit: Box<VisualKit>, seq: u64 },
    /// A channel's kit, its hold, loop and beam.
    Channel { kit: Box<VisualKit> },
    /// A missile's targeting kit, on its target or at a ground aim on its caster (`0x702941`); the
    /// replay carries no fade time, so the flight's end never reaps it (`0x6ff410`).
    Targeting { kit_id: u32, at: Option<Vec3> },
}

/// One play held until a beam reaches its unit, the unit's delayed kit (`0x728050`).
struct BeamKitWait {
    entity: Entity,
    spell_id: u32,
    /// The attachment a connect must carry (`0x722760`): [`NO_BEAM_KEY`], or a location beam's
    /// sequence for its area kit.
    key: i32,
    what: WaitPlay,
    deadline: f32,
    reached: bool,
}

/// The plays waiting on a beam: held by the kit players, reached by the beam tick
/// (`crate::entities::chain_beam`), played by [`route_cast_visuals`] and, for an aura's state
/// kit, [`arm_aura_state_fx`].
#[derive(Resource, Default)]
pub(crate) struct BeamKitWaits {
    waits: Vec<BeamKitWait>,
    /// The clock the deadlines run on, stamped by [`route_cast_visuals`].
    now: f32,
}

impl BeamKitWaits {
    pub(crate) fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    /// A beam reached `entity` with attachment `key`: its plays waiting on `spell_id` with that key
    /// go (`0x722760`).
    pub(crate) fn reach(&mut self, entity: Entity, spell_id: u32, key: i32) {
        for w in &mut self.waits {
            if w.entity == entity && w.spell_id == spell_id && w.key == key {
                w.reached = true;
            }
        }
    }

    fn hold(&mut self, entity: Entity, spell_id: u32, key: i32, what: WaitPlay, now: f32) {
        self.waits.push(BeamKitWait {
            entity,
            spell_id,
            key,
            what,
            deadline: now + BEAM_WAIT_SECS,
            reached: false,
        });
    }

    /// A location beam's area kit, held on its caster under the beam's sequence (`0x726b2c`).
    pub(crate) fn hold_area(
        &mut self,
        caster: Entity,
        spell_id: u32,
        sequence: i32,
        kit_id: u32,
        pos: Vec3,
        now: f32,
    ) {
        let what = WaitPlay::Area { kit_id, pos };
        self.hold(caster, spell_id, sequence, what, now);
    }

    /// A missile's targeting kit, held on its target or, with its ground aim, on its caster, under
    /// the ordinary key.
    pub(crate) fn hold_targeting(
        &mut self,
        unit: Entity,
        spell_id: u32,
        kit_id: u32,
        at: Option<Vec3>,
        now: f32,
    ) {
        let what = WaitPlay::Targeting { kit_id, at };
        self.hold(unit, spell_id, NO_BEAM_KEY, what, now);
    }

    /// The reached targeting plays held at a point: `(unit, kit, point)`.
    #[cfg(test)]
    pub(crate) fn reached_targeting_points(&self) -> Vec<(Entity, u32, Vec3)> {
        let point = |w: &BeamKitWait| match w.what {
            WaitPlay::Targeting {
                kit_id,
                at: Some(at),
            } if w.reached => Some((w.entity, kit_id, at)),
            _ => None,
        };
        self.waits.iter().filter_map(point).collect()
    }

    /// The reached kit and area plays, taken out; an unreached play past its deadline is dropped
    /// unplayed (`0x728140`).
    fn take_reached(&mut self, now: f32) -> Vec<BeamKitWait> {
        self.now = now;
        let (reached, held) = std::mem::take(&mut self.waits)
            .into_iter()
            .filter(|w| w.reached || now < w.deadline)
            .partition(|w| w.reached && !matches!(w.what, WaitPlay::AuraState));
        self.waits = held;
        reached
    }

    /// The reached aura state kits, taken out.
    fn take_reached_auras(&mut self) -> Vec<(Entity, u32)> {
        let mut reached = Vec::new();
        self.waits.retain(|w| {
            let take = w.reached && matches!(w.what, WaitPlay::AuraState);
            if take {
                reached.push((w.entity, w.spell_id));
            }
            !take
        });
        reached
    }
}

/// Whether the spell's first visual draws a beam, the wait's gate (`0x72af76`).
fn draws_beam(spells: &crate::ui_action::Spells, visuals: &SpellVisualCatalog, spell: u32) -> bool {
    spells
        .catalog
        .get(spell)
        .and_then(|d| visuals.stages(d.visual))
        .is_some_and(|s| visuals.has_chain_effect(s))
}

/// Whether a unit's play of `kit` for `spell_id` waits for the spell's beam (`0x72af60`); an
/// object's never does (`0x743570`).
pub(crate) fn kit_waits_for_beam(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    kit: &VisualKit,
) -> bool {
    kit.flags & benilla_formats::kit_flag::BEAM_WAIT != 0 && draws_beam(spells, visuals, spell_id)
}

/// Where a kit play may wait: the waits, and which entities are units.
struct BeamHold<'a> {
    waits: &'a mut BeamKitWaits,
    is_unit: &'a dyn Fn(Entity) -> bool,
}

/// Whether this kit play waits for the spell's beam instead of playing (`0x72af60`).
fn waits_for_beam(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    visuals: &SpellVisualCatalog,
    out: &KitOut,
) -> bool {
    out.waits.as_ref().is_some_and(|h| (h.is_unit)(entity))
        && kit_waits_for_beam(out.spells, visuals, spell_id, kit)
}

/// Hold a unit's kit play on its beam, under the ordinary key.
fn hold_kit(out: &mut KitOut, entity: Entity, spell_id: u32, what: WaitPlay) {
    if let Some(h) = out.waits.as_mut() {
        let now = h.waits.now;
        h.waits.hold(entity, spell_id, NO_BEAM_KEY, what, now);
    }
}

/// The kit lanes [`route_cast_visuals`] writes but does not own, and the beam waits with their
/// clock, bundled into one param because the system is at Bevy's 16-`SystemParam` limit.
type KitSpawnWriters<'w> = (
    MessageWriter<'w, MissileSpawn>,
    MessageWriter<'w, crate::entities::dest_fx::GroundBurst>,
    MessageWriter<'w, ChainProcPlay>,
    MessageWriter<'w, SpellKitShake>,
    MessageWriter<'w, crate::weapon_trail::TrailArm>,
    MessageWriter<'w, BaseAnimRecompute>,
    MessageWriter<'w, DelayedAreaKit>,
    MessageWriter<'w, crate::aura_visual::BodyProcPlay>,
    Option<ResMut<'w, BeamKitWaits>>,
    Res<'w, Time>,
);

/// The writers one kit play fans out to.
struct KitOut<'a, 'w1, 'w2, 'w3, 'w4, 'w5, 'w6, 'w7, 'w8, 'w9, 'w10> {
    oneshots: &'a mut MessageWriter<'w1, EmoteAnim>,
    wounds: &'a mut MessageWriter<'w2, WoundAnim>,
    sounds: &'a mut MessageWriter<'w3, SpellKitSound>,
    fx: &'a mut MessageWriter<'w4, SpellKitFx>,
    /// The kit's beam, the dispatcher's chain case.
    chain: &'a mut MessageWriter<'w5, ChainProcPlay>,
    shakes: &'a mut MessageWriter<'w6, SpellKitShake>,
    /// The kit's weapon-trail arm (type-8 case), a latch drawn from the unit's next animation.
    trails: &'a mut MessageWriter<'w7, crate::weapon_trail::TrailArm>,
    /// The base recompute a stage-2 kit's anim id spends itself on.
    recomputes: &'a mut MessageWriter<'w8, BaseAnimRecompute>,
    /// The stow a kit's weapon effects ask for ([`stow_for_weapon_effects`]).
    sheaths: &'a mut MessageWriter<'w9, super::SheathRequest>,
    /// The spell table, for the plays' attribute gates.
    spells: &'a crate::ui_action::Spells,
    /// The flash and procedural fade cases of the dispatcher.
    body: &'a mut MessageWriter<'w10, crate::aura_visual::BodyProcPlay>,
    /// Where a beam spell's flag-`0x1` kit waits; `None` plays it at once, as a released play does.
    waits: Option<BeamHold<'a>>,
}

/// The dispatcher's flash and fade cases for one kit play, every filled slot in order (`0x7265c0`).
fn play_body_procs(
    body: &mut MessageWriter<crate::aura_visual::BodyProcPlay>,
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
) {
    for proc in kit.char_procs().filter(crate::aura_visual::is_body_proc) {
        body.write(crate::aura_visual::BodyProcPlay {
            entity,
            spell_id,
            proc,
        });
    }
}

/// One kit play's policy on the reference's kit player:
/// - `persistent`: the effect models outlive the play (a precast or channel hold).
/// - `effects`: whether the models spawn; `false` for the impact hand-off's state stage, whose
///   models [`arm_aura_state_fx`] owns.
/// - `sound`: whether the kit sound rings; its leg has no self test at any stage.
#[derive(Clone, Copy)]
struct KitPlay {
    persistent: bool,
    effects: bool,
    sound: bool,
    /// The models' animation lifecycle ([`FxStage`]).
    stage: FxStage,
    /// The stage-2 (state kit) play: the kit player branches around the only anim play, so the id
    /// is only compared with what the unit plays, a mismatch spent on a [`BaseAnimRecompute`].
    stage_2: bool,
}

impl KitPlay {
    /// Stages 0 and 1: self-terminating models, every leg on.
    const DISCRETE: Self = Self {
        persistent: false,
        effects: true,
        sound: true,
        stage: FxStage::OneShot,
        stage_2: false,
    };
}

/// Kit flags `0x4` and `0x10`, either of which skips the Special remap (`0x7386fc`).
const KIT_RAW_SPECIAL: u32 =
    benilla_formats::kit_flag::RAW_ANIM | benilla_formats::kit_flag::NO_SPECIAL_REMAP;

fn play_kit(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    play: KitPlay,
    seq: u64,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
) {
    // The whole play waits for the beam to reach this unit (`0x72af60`).
    if waits_for_beam(entity, spell_id, kit, visuals, out) {
        let what = WaitPlay::Kit {
            kit: Box::new(*kit),
            play,
            seq,
            at: None,
        };
        hold_kit(out, entity, spell_id, what);
        return;
    }
    if let Some(anim_id) = kit.anim_id {
        if play.stage_2 {
            // Stage 2 compares, never plays; the driver holds the armed id and compares.
            out.recomputes.write(BaseAnimRecompute { entity, anim_id });
        } else if (8..=10).contains(&anim_id) {
            // A CombatWound id (8 to 10) lays a severity-0 wound in the blend slot, not the kit's
            // own id.
            out.wounds.write(WoundAnim { entity });
        } else {
            out.oneshots.write(EmoteAnim {
                entity,
                anim_id,
                seq,
                via_player: false,
                remap_special: kit.flags & KIT_RAW_SPECIAL == 0,
            });
        }
    }
    if let Some(kit_sound) = kit.sound.filter(|_| play.sound) {
        out.sounds.write(SpellKitSound::Play { entity, kit_sound });
    }
    // The camera shake plays at every stage, gated on nothing.
    if let Some(group) = kit.shake {
        out.shakes.write(SpellKitShake::Play { entity, group });
    }
    // The beam case runs for every chain slot of every play, ungated by `play.effects`.
    for proc in kit.char_procs().filter_map(|p| p.as_chain()) {
        out.chain.write(ChainProcPlay {
            entity,
            spell_id,
            proc,
            kit_id: kit.id,
        });
    }
    play_body_procs(out.body, entity, spell_id, kit);
    // The trail arm, on the same terms; the reference dispatches it before the kit's anim, so a kit
    // with both fires its trail on its own swing.
    if let Some(trail) = kit.trail_proc() {
        out.trails
            .write(crate::weapon_trail::TrailArm { entity, trail });
    }
    if !play.effects {
        return;
    }
    stow_for_weapon_effects(out.sheaths, out.spells, entity, spell_id, kit);
    let effects = resolve_kit_effects(visuals, kit);
    if !effects.is_empty() {
        out.fx.write(SpellKitFx::Begin {
            entity,
            spell_id,
            persistent: play.persistent,
            class: FxClass::Hold,
            stage: play.stage,
            effects,
        });
    }
}

/// A kit played at a world point rather than on a unit, the area kits' form (`0x745230` with a
/// position): each effect slot's model once at the point, the sound and the shake there. The
/// anim, beam and trail legs need a unit and do not run.
fn play_kit_at(
    kit: &VisualKit,
    pos: Vec3,
    visuals: &SpellVisualCatalog,
    sounds: &mut MessageWriter<SpellKitSound>,
    shakes: &mut MessageWriter<SpellKitShake>,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    let slots = kit.effects().map(|(_, e)| e);
    play_models_at(kit, slots, pos, visuals, sounds, shakes, bursts);
}

/// An area kit at a point: only its head, world and base slots and its attach rows (`0x745230`).
fn play_area_kit_at(
    kit: &VisualKit,
    pos: Vec3,
    visuals: &SpellVisualCatalog,
    sounds: &mut MessageWriter<SpellKitSound>,
    shakes: &mut MessageWriter<SpellKitShake>,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    let slots = area_slots(kit);
    play_models_at(kit, slots, pos, visuals, sounds, shakes, bursts);
}

/// The slots an area or targeting kit plays at a point.
fn area_slots(kit: &VisualKit) -> impl Iterator<Item = u32> + '_ {
    kit.effects()
        .filter(|(tag, _)| AREA_PLAY_TAGS.contains(tag))
        .map(|(_, e)| e)
}

/// The models a kit held at a point plants: `(path, attach row, scale)` in the area subset.
pub(crate) fn area_kit_models<'a>(
    visuals: &'a SpellVisualCatalog,
    kit: &'a VisualKit,
) -> impl Iterator<Item = (&'a str, Option<benilla_formats::ModelAttach>, f32)> + 'a {
    let rows = visuals
        .model_attachments(kit.id)
        .iter()
        .map(|a| (a.effect, Some(*a)));
    area_slots(kit)
        .map(|e| (e, None))
        .chain(rows)
        .filter_map(|(effect, attach)| {
            let path = visuals.effect_path(effect)?;
            Some((path, attach, point_scale(visuals, effect)))
        })
}

/// A model placed at a point: its row's scale from 1 (`0x6f8ae0`).
fn point_scale(visuals: &SpellVisualCatalog, effect: u32) -> f32 {
    visuals.effect_scale(effect).map_or(1.0, |s| s.planted(1.0))
}

/// The models `slots` names and the kit's attach rows at a point, with its sound and shake.
fn play_models_at(
    kit: &VisualKit,
    slots: impl Iterator<Item = u32>,
    pos: Vec3,
    visuals: &SpellVisualCatalog,
    sounds: &mut MessageWriter<SpellKitSound>,
    shakes: &mut MessageWriter<SpellKitShake>,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    let rows = visuals
        .model_attachments(kit.id)
        .iter()
        .map(|a| (a.effect, Some(*a)));
    for (effect, attach) in slots.map(|e| (e, None)).chain(rows) {
        if let Some(path) = visuals.effect_path(effect) {
            bursts.write(crate::entities::dest_fx::GroundBurst {
                path: path.to_string(),
                pos,
                attach,
                scale: point_scale(visuals, effect),
                turn: Quat::IDENTITY,
                held_by: None,
            });
        }
    }
    if let Some(kit_sound) = kit.sound {
        sounds.write(SpellKitSound::PlayAt { pos, kit_sound });
    }
    if let Some(group) = kit.shake {
        shakes.write(SpellKitShake::PlayAt { pos, group });
    }
}

/// `SpellVisual` column 24 at a dest point, deferred: for a GO, to when a missile of the spell's
/// speed would land there, the reference queueing it on the caster stamped
/// `now + distance / speed` (`0x80efb6`, `0x80e7f3`); for a dest-location notify, by its delay.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(crate) struct DelayedAreaKit {
    pub(crate) kit_id: u32,
    pub(crate) pos: Vec3,
    pub(crate) wait: AreaKitWait,
}

/// How long a [`DelayedAreaKit`] waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AreaKitWait {
    /// The flight from the caster at `Spell.dbc` Speed, yards per second, above 0: at once with no
    /// caster position.
    Flight { caster: Entity, speed: f32 },
    /// The flight from a fixed point at Speed.
    FlightFrom { from: Vec3, speed: f32 },
    /// A fixed wait, in seconds.
    Secs(f32),
}

/// How long a [`DelayedAreaKit`] waits: the source-to-point distance over the speed, at once
/// with no source.
fn area_kit_delay(from: Option<Vec3>, pos: Vec3, speed: f32) -> f32 {
    from.map_or(0.0, |c| c.distance(pos) / speed)
}

/// A dest-location cast's visuals (`0x8022d0`): column 23 at once, then a missile or column 24.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn play_dest_loc_casts(
    mut casts: MessageReader<super::DestLocCast>,
    spells: Option<Res<crate::ui_action::Spells>>,
    visuals: Option<Res<SpellVisuals>>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut shakes: MessageWriter<SpellKitShake>,
    mut bursts: MessageWriter<crate::entities::dest_fx::GroundBurst>,
    mut delayed: MessageWriter<DelayedAreaKit>,
    mut missiles: MessageWriter<MissileSpawn>,
    me: Query<Entity, With<crate::net::SelfPlayer>>,
) {
    let (Some(spells), Some(visuals)) = (spells.as_deref(), visuals.as_deref()) else {
        casts.clear();
        return;
    };
    for c in casts.read() {
        let Some(stages) = resolve_stages(spells, &visuals.0, c.spell_id, || None) else {
            continue;
        };
        if let Some(kit) = visuals.0.kit(stages.instant_area_kit) {
            play_area_kit_at(
                kit,
                c.dest,
                &visuals.0,
                &mut sounds,
                &mut shakes,
                &mut bursts,
            );
        }
        let speed = spells.catalog.get(c.spell_id).map_or(0.0, |d| d.speed);
        let missile = stages.missile_gate != 0 && stages.missile_model != 0 && speed > 0.0;
        // Deviation: the packet's trajectory is not read, so such a missile flies straight.
        if let Some(player) = me.iter().next().filter(|_| missile) {
            // An item model (-1 to -5) has no caster to take it from: the error cube (`0x80249a`).
            let item_model = (-5..=-1).contains(&(stages.missile_model as i32));
            let path = (!item_model)
                .then(|| visuals.0.effect_path(stages.missile_model))
                .flatten()
                .unwrap_or(ERROR_CUBE);
            let mut shape =
                crate::entities::MissileShape::of(Some(&stages), Some(&stages), Some(&stages));
            shape.source = None;
            shape.impact_offset = Vec3::ZERO;
            // A flight with no caster arms no targeting kit (`0x7022d0`).
            shape.targeting_kit = 0;
            let scale = visuals
                .0
                .effect_scale(stages.missile_model)
                .map_or(1.0, |s| s.scale);
            missiles.write(MissileSpawn {
                // It lands on our own player, the no-caster arm of `0x700e20`.
                caster: player,
                spell_id: c.spell_id,
                path: Some(path.to_string()),
                ammo_display_id: None,
                dest_tag: None,
                speed,
                targets: Vec::new(),
                reflects: Vec::new(),
                reflect_glances: false,
                scale,
                ground_aim: Some(c.dest),
                missile_sound: stages.missile_sound,
                awaits_release: false,
                trajectory: None,
                shape,
                from: Some(c.src),
            });
            continue;
        }
        if visuals.0.kit(stages.impact_area_kit).is_none() {
            continue;
        }
        delayed.write(DelayedAreaKit {
            kit_id: stages.impact_area_kit,
            pos: c.dest,
            wait: if missile {
                AreaKitWait::FlightFrom { from: c.src, speed }
            } else {
                AreaKitWait::Secs(c.delay_ms as f32 / 1000.0)
            },
        });
    }
}

/// Queue each [`DelayedAreaKit`] and play the due ones at their point ([`play_kit_at`]).
pub(super) fn play_delayed_area_kits(
    mut queued: MessageReader<DelayedAreaKit>,
    casters: Query<&GlobalTransform>,
    visuals: Option<Res<SpellVisuals>>,
    time: Res<Time>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut shakes: MessageWriter<SpellKitShake>,
    mut bursts: MessageWriter<crate::entities::dest_fx::GroundBurst>,
    mut pending: Local<Vec<(f32, u32, Vec3)>>,
) {
    let now = time.elapsed_secs();
    for q in queued.read() {
        let wait = match q.wait {
            AreaKitWait::Flight { caster, speed } => {
                let from = casters.get(caster).ok().map(|g| g.translation());
                area_kit_delay(from, q.pos, speed)
            }
            AreaKitWait::FlightFrom { from, speed } => area_kit_delay(Some(from), q.pos, speed),
            AreaKitWait::Secs(secs) => secs,
        };
        pending.push((now + wait, q.kit_id, q.pos));
    }
    if pending.is_empty() {
        return;
    }
    let Some(visuals) = visuals.as_deref() else {
        pending.clear();
        return; // no client data
    };
    pending.retain(|&(due, kit_id, pos)| {
        if now < due {
            return true;
        }
        if let Some(kit) = visuals.0.kit(kit_id) {
            play_area_kit_at(kit, pos, &visuals.0, &mut sounds, &mut shakes, &mut bursts);
        }
        false
    });
}

/// Whether `SMSG_SPELL_GO` plays column 24 itself (`0x80ef23`): not when a missile carries it to
/// the landing (`HasMissile` and Speed > 0, `0x80ef53`), nor for a row that draws a beam
/// (`0x80ef71`).
fn go_plays_impact_area(visuals: &SpellVisualCatalog, stages: &VisualStages, speed: f32) -> bool {
    !(stages.missile_gate != 0 && speed > 0.0) && !visuals.has_chain_effect(stages)
}

/// One spell landing on one unit.
#[derive(Clone, Copy)]
struct ImpactPlay {
    spell_id: u32,
    /// The ranged fallback of an instant hit's merged row; `None` on arrival (`0x703586`).
    weapon_visual: Option<u32>,
    /// The unit is the caster: the caster impact kit, else the target impact kit (`0x800d00`).
    on_caster: bool,
    /// Play the impact kit's models, sound and shake at this point, not on the unit (`0x702e5a`).
    at: Option<Vec3>,
    /// Lay the victim's severity-0 wound flinch beside the kit.
    flinch: bool,
    seq: u64,
}

/// The impact kit a unit takes: column 14 on the caster, else 15, then column 3 (`0x800d00`).
fn impact_kit<'a>(
    visuals: &'a SpellVisualCatalog,
    stages: &VisualStages,
    on_caster: bool,
) -> Option<&'a VisualKit> {
    let own = if on_caster {
        stages.caster_impact
    } else {
        stages.target_impact
    };
    [own, stages.impact]
        .into_iter()
        .filter(|&id| id != 0)
        .find_map(|id| visuals.kit(id))
}

/// The unit-impact hand-off: the impact kit (stage 1), then the state kit's anim and sound, its
/// models owned by [`arm_aura_state_fx`]. The stage-2 gate is not built: its content is untraced.
/// The flinch comes after the kit for a harmful instant hit, before it for every missile target.
fn play_impact(
    entity: Entity,
    impact: ImpactPlay,
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    let ImpactPlay {
        spell_id,
        weapon_visual,
        on_caster,
        at,
        flinch,
        seq,
    } = impact;
    if flinch {
        out.wounds.write(WoundAnim { entity });
    }
    let Some(stages) = resolve_stages(spells, visuals, spell_id, || weapon_visual) else {
        return;
    };
    if let Some(kit) = impact_kit(visuals, &stages, on_caster) {
        match at {
            None => play_kit(entity, spell_id, kit, KitPlay::DISCRETE, seq, visuals, out),
            // The play waits whole, its point kept with it (`0x72af60`).
            Some(at) if waits_for_beam(entity, spell_id, kit, visuals, out) => {
                let what = WaitPlay::Kit {
                    kit: Box::new(*kit),
                    play: KitPlay::DISCRETE,
                    seq,
                    at: Some(at),
                };
                hold_kit(out, entity, spell_id, what);
            }
            Some(at) => play_kit_split(entity, spell_id, kit, at, seq, visuals, out, bursts),
        }
    }
    if let Some(kit) = (stages.state != 0)
        .then(|| visuals.kit(stages.state))
        .flatten()
    {
        let play = KitPlay {
            effects: false,
            stage_2: true,
            stage: FxStage::State,
            ..KitPlay::DISCRETE
        };
        play_kit(entity, spell_id, kit, play, seq, visuals, out);
    }
}

/// An impact kit split between the unit and a point: the unit keeps the anim, beam and trail
/// legs; the models, sound and shake go to the point.
#[allow(clippy::too_many_arguments)]
fn play_kit_split(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    at: Vec3,
    seq: u64,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    let unit_legs = VisualKit {
        shake: None,
        ..*kit
    };
    let play = KitPlay {
        effects: false,
        sound: false,
        ..KitPlay::DISCRETE
    };
    play_kit(entity, spell_id, &unit_legs, play, seq, visuals, out);
    play_kit_at(kit, at, visuals, out.sounds, out.shakes, bursts);
}

/// A cast's precast kit: the `0x400` ranged hold, the [`CastHold`], the sound and the persistent
/// models; the whole play waits for the spell's beam when its kit says so (`0x72af60`).
#[allow(clippy::too_many_arguments)]
fn play_precast_kit(
    commands: &mut Commands,
    pending: &mut EntityHashMap<Option<u32>>,
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    seq: u64,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
) {
    if waits_for_beam(entity, spell_id, kit, visuals, out) {
        let what = WaitPlay::Precast {
            kit: Box::new(*kit),
            seq,
        };
        hold_kit(out, entity, spell_id, what);
        return;
    }
    let spells = out.spells;
    // The `0x400` weapon-visual hold: a ranged visual play sets it, any other clears it; it keeps
    // a remote shooter drawn.
    let ranged = spells
        .catalog
        .get(spell_id)
        .is_some_and(|d| d.ranged_slot());
    // Fallible: `model_fade::apply_despawn_fade` is unordered against this chain and can despawn
    // the unit before these commands apply.
    if ranged {
        commands.entity(entity).try_insert(super::RangedHold);
    } else {
        commands.entity(entity).try_remove::<super::RangedHold>();
    }
    // A start anim of 0 plays (`0x73b1f9`) but is never offered (`0x722590`).
    let start_anim = kit.start_anim.filter(|&a| a > 0);
    // Deviation: a start-only kit offers nothing after its start (`0x722599`); its hold falls to
    // Stand rather than the idle pick.
    if let Some(anim_id) = kit.anim_id.or(start_anim.map(|_| 0)) {
        commands.entity(entity).try_insert(CastHold {
            anim_id,
            spell_id,
            ranged,
            start_anim,
            seq,
        });
        pending.insert(entity, Some(spell_id));
    }
    if let Some(kit_sound) = kit.sound {
        out.sounds.write(SpellKitSound::Play { entity, kit_sound });
    }
    stow_for_weapon_effects(out.sheaths, spells, entity, spell_id, kit);
    // Persistent until the cast resolves, re-armed forever (stage 4).
    let effects = resolve_kit_effects(visuals, kit);
    if !effects.is_empty() {
        out.fx.write(SpellKitFx::Begin {
            entity,
            spell_id,
            persistent: true,
            class: FxClass::Hold,
            stage: FxStage::Relive,
            effects,
        });
    }
}

/// A channel's kit: the [`CastHold`], the sound, the beams, the trail, the body procs and the
/// persistent models; the whole play waits for the spell's beam when its kit says so (`0x72af60`).
fn play_channel_kit(
    commands: &mut Commands,
    pending: &mut EntityHashMap<Option<u32>>,
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
) {
    if waits_for_beam(entity, spell_id, kit, visuals, out) {
        let what = WaitPlay::Channel {
            kit: Box::new(*kit),
        };
        hold_kit(out, entity, spell_id, what);
        return;
    }
    if let Some(anim_id) = kit.anim_id {
        // Fallible: a unit mid-stream-out is un-indexed but still in this query.
        commands.entity(entity).try_insert(CastHold {
            anim_id,
            spell_id,
            ranged: false, // no basic shot channels
            start_anim: None,
            seq: 0,
        });
        pending.insert(entity, Some(spell_id));
    }
    if let Some(kit_sound) = kit.sound {
        out.sounds.write(SpellKitSound::Play { entity, kit_sound });
    }
    // The CharProc dispatcher's second caller: Drain Life's beam is reached only here, on the
    // rising edge, and lives until the field clears; every chain slot runs.
    for proc in kit.char_procs().filter_map(|p| p.as_chain()) {
        out.chain.write(ChainProcPlay {
            entity,
            spell_id,
            proc,
            kit_id: kit.id,
        });
    }
    // The trail arm from the same caller: kit 370 (Whirlwind Primer) is reached here.
    if let Some(trail) = kit.trail_proc() {
        out.trails
            .write(crate::weapon_trail::TrailArm { entity, trail });
    }
    play_body_procs(out.body, entity, spell_id, kit);
    stow_for_weapon_effects(out.sheaths, out.spells, entity, spell_id, kit);
    // Persistent while the field holds, with the stage-2 lifecycle.
    let effects = resolve_kit_effects(visuals, kit);
    if !effects.is_empty() {
        out.fx.write(SpellKitFx::Begin {
            entity,
            spell_id,
            persistent: true,
            class: FxClass::Hold,
            stage: state_stage(kit),
            effects,
        });
    }
}

/// An area kit a unit plays at a point, or its hold on the spell's beam (`0x72af60`).
#[allow(clippy::too_many_arguments)]
fn play_unit_area_kit(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    pos: Vec3,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
    bursts: &mut MessageWriter<crate::entities::dest_fx::GroundBurst>,
) {
    if waits_for_beam(entity, spell_id, kit, visuals, out) {
        let what = WaitPlay::Area {
            kit_id: kit.id,
            pos,
        };
        hold_kit(out, entity, spell_id, what);
        return;
    }
    play_area_kit_at(kit, pos, visuals, out.sounds, out.shakes, bursts);
}

/// The cast-edge router: wire [`CastEvent`]s and the public channel field → animation intents.
///
/// - Start: the precast kit's anim becomes the [`CastHold`] (stage 4) and its sound rings.
/// - Go: the hold drops and the cast kit plays; the GO's targets play their impacts inline when
///   `Spell.dbc` Speed is 0, else fly a [`MissileSpawn`].
/// - Impact: a missile arrived on this unit ([`play_impact`]).
/// - Fail: the hold drops silently.
/// - Channel: `UNIT_CHANNEL_SPELL` polled per frame; observers have no packet. A set field holds
///   the channel kit's anim and rings its sound once, a cleared one drops it.
pub(super) fn route_cast_visuals(
    mut commands: Commands,
    mut events: MessageReader<CastEvent>,
    mut go_targets: MessageReader<SpellGoTargets>,
    mut pushes: MessageReader<KitPush>,
    mut oneshots: MessageWriter<EmoteAnim>,
    mut wounds: MessageWriter<WoundAnim>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut fx: MessageWriter<SpellKitFx>,
    mut sheaths: MessageWriter<super::SheathRequest>,
    mut spawns: KitSpawnWriters,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut weapon_src: WeaponVisualSrc,
    units: Query<(Entity, &ObjectStore), Without<ItemObject>>,
    holds: Query<&CastHold>,
    mut channel_cache: Local<EntityHashMap<u32>>,
) {
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        return; // no client data
    };
    // Disjoint borrows: the beam writer rides `out` while the missile and burst writers stay free.
    let (missiles, bursts, chains, shakes, trails, recomputes, delayed, body, waits, time) = (
        &mut spawns.0,
        &mut spawns.1,
        &mut spawns.2,
        &mut spawns.3,
        &mut spawns.4,
        &mut spawns.5,
        &mut spawns.6,
        &mut spawns.7,
        &mut spawns.8,
        &spawns.9,
    );
    // Nothing watches the waits for changes.
    let reached = waits.as_mut().map_or_else(Vec::new, |w| {
        w.bypass_change_detection()
            .take_reached(time.elapsed_secs())
    });
    let mut out = KitOut {
        oneshots: &mut oneshots,
        wounds: &mut wounds,
        sounds: &mut sounds,
        fx: &mut fx,
        chain: chains,
        shakes,
        trails,
        recomputes,
        sheaths: &mut sheaths,
        spells,
        body,
        waits: None,
    };
    // `holds` lags one command flush and an instant cast's START and GO drain together, so
    // `pending` overlays this frame's hold writes (else the cast pose sticks).
    let mut pending: EntityHashMap<Option<u32>> = EntityHashMap::default();
    // A reached play goes through the kit player again, past the wait (`0x7227f7`). 12340's tick
    // reads the end bits its last render set (`0x4f91d4` runs before `0x4f91d9`), so this is the
    // frame after the drawn reach there too.
    for w in reached {
        if commands.get_entity(w.entity).is_err() {
            continue;
        }
        match w.what {
            WaitPlay::Kit {
                kit,
                play,
                seq,
                at: None,
            } => play_kit(w.entity, w.spell_id, &kit, play, seq, &visuals.0, &mut out),
            WaitPlay::Kit {
                kit,
                seq,
                at: Some(at),
                ..
            } => play_kit_split(
                w.entity, w.spell_id, &kit, at, seq, &visuals.0, &mut out, bursts,
            ),
            WaitPlay::Area { kit_id, pos } => {
                delayed.write(DelayedAreaKit {
                    kit_id,
                    pos,
                    wait: AreaKitWait::Secs(0.0),
                });
            }
            WaitPlay::Precast { kit, seq } => play_precast_kit(
                &mut commands,
                &mut pending,
                w.entity,
                w.spell_id,
                &kit,
                seq,
                &visuals.0,
                &mut out,
            ),
            WaitPlay::Channel { kit } => play_channel_kit(
                &mut commands,
                &mut pending,
                w.entity,
                w.spell_id,
                &kit,
                &visuals.0,
                &mut out,
            ),
            WaitPlay::Targeting { kit_id, at } => {
                let Some(kit) = visuals.0.kit(kit_id) else {
                    continue;
                };
                match at {
                    None => {
                        arm_targeting_kit(w.entity, w.spell_id, kit, &visuals.0, out.fx, out.sounds)
                    }
                    Some(at) => {
                        play_point_targeting_kit(w.entity, kit, at, &visuals.0, out.sounds, bursts)
                    }
                }
            }
            WaitPlay::AuraState => {}
        }
    }
    // An object's kit never waits (`0x743570`).
    let is_unit = |e: Entity| units.get(e).is_ok_and(|(_, s)| s.is_unit());
    out.waits = waits.as_mut().map(|w| BeamHold {
        waits: w.bypass_change_detection(),
        is_unit: &is_unit,
    });

    let held_spell = |pending: &EntityHashMap<Option<u32>>, entity: Entity| -> Option<u32> {
        match pending.get(&entity) {
            Some(&overlaid) => overlaid,
            None => holds.get(entity).ok().map(|h| h.spell_id),
        }
    };

    for p in pushes.read() {
        // No kit-level scan, but each slot's same-(record, tag) replace swaps the old batch out;
        // the driver's same-id dedup keeps a looping clip running.
        if let Some(kit) = visuals.0.kit(p.kit_id) {
            play_kit(
                p.entity,
                0,
                kit,
                KitPlay::DISCRETE,
                p.seq,
                &visuals.0,
                &mut out,
            );
        }
    }

    for ev in events.read() {
        // A despawn applied at the net sync point can outlive a cast edge from the same batch;
        // skip a dead subject whole.
        if commands.get_entity(ev.entity).is_err() {
            continue;
        }
        match ev.kind {
            CastEventKind::Start => {
                // A replacing cast reaps the prior hold's loop and persistent models first.
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                if let Some(prior) = held_spell(&pending, ev.entity) {
                    out.fx.write(SpellKitFx::Reap {
                        entity: ev.entity,
                        spell_id: prior,
                        class: FxClass::Hold,
                    });
                }
                // A ranged-attribute START draws the ranged weapon; vmangos sends START once per
                // auto-repeat activation (`Spell.cpp:3448`).
                if spells
                    .catalog
                    .get(ev.spell_id)
                    .is_some_and(|d| d.ranged_attack())
                {
                    out.sheaths.write(super::SheathRequest {
                        entity: ev.entity,
                        state: 2,
                        ceremony: false,
                    });
                }
                if let Some(kit) = resolve_kit_traced(
                    "precast",
                    ev.entity,
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.precast,
                    || weapon_src.caster(ev.entity),
                ) {
                    play_precast_kit(
                        &mut commands,
                        &mut pending,
                        ev.entity,
                        ev.spell_id,
                        &kit,
                        ev.seq,
                        &visuals.0,
                        &mut out,
                    );
                }
            }
            CastEventKind::Go => {
                // Spell-id-keyed (the reap with no force): a proc's GO keeps another spell's hold.
                if held_spell(&pending, ev.entity) == Some(ev.spell_id) {
                    commands.entity(ev.entity).try_remove::<CastHold>();
                    pending.insert(ev.entity, None);
                }
                // Reaped whatever the hold: a precast kit with no anim still starts a loop.
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                out.fx.write(SpellKitFx::Reap {
                    entity: ev.entity,
                    spell_id: ev.spell_id,
                    class: FxClass::Hold,
                });
                // A ranged-slot GO redraws ranged before the cast kit (gate `Attributes & 0x2`,
                // stages 0 and 4): what redraws a bow an emote lowered.
                if spells
                    .catalog
                    .get(ev.spell_id)
                    .is_some_and(|d| d.ranged_slot())
                {
                    out.sheaths.write(super::SheathRequest {
                        entity: ev.entity,
                        state: 2,
                        ceremony: false,
                    });
                }
                // For a basic shot the cast kit is the fire clip, from the ranged fallback.
                if let Some(kit) = resolve_kit_traced(
                    "cast",
                    ev.entity,
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.cast,
                    || weapon_src.caster(ev.entity),
                ) {
                    // The `0x400` hold re-asserted per shot, or cleared for a non-ranged play.
                    if spells
                        .catalog
                        .get(ev.spell_id)
                        .is_some_and(|d| d.ranged_slot())
                    {
                        commands.entity(ev.entity).try_insert(super::RangedHold);
                    } else {
                        commands.entity(ev.entity).try_remove::<super::RangedHold>();
                    }
                    play_kit(
                        ev.entity,
                        ev.spell_id,
                        &kit,
                        KitPlay::DISCRETE,
                        ev.seq,
                        &visuals.0,
                        &mut out,
                    );
                }
            }
            CastEventKind::Impact { on_caster, at } => {
                // Every living target flinches; the kit comes off the spell's own row (`0x703586`).
                play_impact(
                    ev.entity,
                    ImpactPlay {
                        spell_id: ev.spell_id,
                        weapon_visual: None,
                        on_caster,
                        at,
                        flinch: true,
                        seq: ev.seq,
                    },
                    spells,
                    &visuals.0,
                    &mut out,
                    bursts,
                );
            }
            CastEventKind::GroundImpact { pos } => {
                // A missile arrived at a point: `SpellVisual` column 24, the impact area kit, which
                // `SMSG_SPELL_GO` leaves to the landing when a missile flies (`0x80ef53`), plays
                // there.
                let area = resolve_kit(
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.impact_area_kit,
                    || None,
                );
                // The landing plays it through its caster's kit player (`0x700e20`).
                if let Some(kit) = area {
                    play_unit_area_kit(
                        ev.entity,
                        ev.spell_id,
                        &kit,
                        pos,
                        &visuals.0,
                        &mut out,
                        bursts,
                    );
                }
            }
            CastEventKind::Fail => {
                if held_spell(&pending, ev.entity) == Some(ev.spell_id) {
                    commands.entity(ev.entity).try_remove::<CastHold>();
                    pending.insert(ev.entity, None);
                }
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                out.fx.write(SpellKitFx::Reap {
                    entity: ev.entity,
                    spell_id: ev.spell_id,
                    class: FxClass::Hold,
                });
            }
        }
    }

    // Speed 0 plays every impact now, Speed > 0 flies one projectile per target. Misses fly too;
    // the reference deflects them off the target, which is not built.
    for go in go_targets.read() {
        let Some(display) = spells.catalog.get(go.spell_id) else {
            continue;
        };
        // Resolved once per GO; every consumer below reads the merged row.
        let wv = weapon_src.caster(go.caster);
        let stages = resolve_stages(spells, &visuals.0, go.spell_id, || wv);
        // The instant area kit (`0x80ee85`): column 23's kit at the dest point when the targets
        // carry one, fired at GO whatever the hit list.
        if let Some(dest) = go.dest {
            if let Some(kit) = stages.and_then(|s| visuals.0.kit(s.instant_area_kit)) {
                play_unit_area_kit(
                    go.caster,
                    go.spell_id,
                    kit,
                    dest,
                    &visuals.0,
                    &mut out,
                    bursts,
                );
            }
            // Then column 24 when no missile carries it: now at Speed 0, else at the would-be
            // landing (`0x80efc8`, `0x80efb6`).
            if let Some(s) = stages.filter(|s| go_plays_impact_area(&visuals.0, s, display.speed)) {
                if let Some(kit) = visuals.0.kit(s.impact_area_kit) {
                    if display.speed > 0.0 {
                        delayed.write(DelayedAreaKit {
                            kit_id: s.impact_area_kit,
                            pos: dest,
                            wait: AreaKitWait::Flight {
                                caster: go.caster,
                                speed: display.speed,
                            },
                        });
                    } else {
                        play_area_kit_at(kit, dest, &visuals.0, out.sounds, out.shakes, bursts);
                    }
                }
            }
        }
        if display.speed <= 0.0 {
            // A hit flinches after its kit iff harmful; the caster takes its own kit (`0x801feb`).
            let flinch = display.is_harmful();
            for &target in &go.hits {
                play_impact(
                    target,
                    ImpactPlay {
                        spell_id: go.spell_id,
                        weapon_visual: wv,
                        on_caster: target == go.caster,
                        at: None,
                        flinch,
                        seq: go.seq,
                    },
                    spells,
                    &visuals.0,
                    &mut out,
                    bursts,
                );
            }
            // A reflect the caster takes plays its own impact kit on it (`0x80f44b`).
            if display.attributes_ex7 & 0x2000_0000 == 0 {
                if let Some(kit) = stages
                    .as_ref()
                    .and_then(|s| impact_kit(&visuals.0, s, true))
                {
                    for _ in 0..go.reflected {
                        let play = KitPlay::DISCRETE;
                        play_kit(
                            go.caster,
                            go.spell_id,
                            kit,
                            play,
                            go.seq,
                            &visuals.0,
                            &mut out,
                        );
                    }
                }
            }
        } else {
            // The spawn gate is Speed alone: basic shots have no visual row.
            let path = stages.and_then(|s| {
                (s.missile_model >= 1).then(|| {
                    visuals
                        .0
                        .effect_path(s.missile_model)
                        .unwrap_or(ERROR_CUBE)
                        .to_string()
                })
            });
            // An out-of-table ordinal reads adjacent `.rdata` in the reference; here, the base.
            let dest_tag = stages.and_then(|s| s.missile_dest_attachment());
            let missile_sound = stages.and_then(|s| s.missile_sound);
            let targets: Vec<(Entity, Option<u8>)> = go
                .hits
                .iter()
                .map(|&e| (e, None))
                .chain(go.misses.iter().map(|&(e, code)| (e, Some(code))))
                .collect();
            // Only an empty hit array consults the ground point.
            let ground_aim = targets.is_empty().then_some(go.dest).flatten();
            if !targets.is_empty() || ground_aim.is_some() {
                // The release gate (inverted).
                let awaits_release = stages
                    .filter(|s| s.cast != 0)
                    .and_then(|s| visuals.0.kit(s.cast))
                    .is_some_and(|k| k.anim_id.is_some());
                missiles.write(MissileSpawn {
                    caster: go.caster,
                    spell_id: go.spell_id,
                    path,
                    ammo_display_id: go.ammo_display_id,
                    dest_tag,
                    speed: display.speed,
                    targets,
                    reflects: go.reflects.clone(),
                    reflect_glances: display.attributes_ex7 & 0x2000_0000 != 0,
                    scale: stages
                        .and_then(|s| visuals.0.effect_scale(s.missile_model))
                        .map_or(1.0, |s| s.scale),
                    ground_aim,
                    missile_sound,
                    awaits_release,
                    trajectory: go
                        .trajectory
                        .map(|t| (t, visuals.0.missile(display.missile).copied())),
                    // The own row (`0x7332e0`), the merged row, the unpicked first (`0x702570`).
                    shape: crate::entities::MissileShape::of(
                        visuals.0.stages(spell_visual_id(display)),
                        stages.as_ref(),
                        visuals.0.stages(display.visual),
                    ),
                    from: None,
                });
            }
        }
    }

    // The channel poll: `UNIT_CHANNEL_SPELL` edges drive an observed unit's channel (the self-only
    // `MSG_CHANNEL_*` do not); a cleared field drops the hold even with no other wire trace.
    for (entity, store) in &units {
        let cur = store.0.unit_channel_spell();
        let prev = channel_cache.get(&entity).copied().unwrap_or(0);
        if cur == prev {
            continue;
        }
        channel_cache.insert(entity, cur);
        if cur != 0 {
            out.sounds.write(SpellKitSound::StopHold { entity });
            if prev != 0 {
                // A channel replacing a channel reaps the old spell's effects first.
                out.fx.write(SpellKitFx::Reap {
                    entity,
                    spell_id: prev,
                    class: FxClass::Hold,
                });
            }
            // No ranged fallback on the channel poll.
            if let Some(kit) = resolve_kit(spells, &visuals.0, cur, |s| s.channel, || None) {
                play_channel_kit(
                    &mut commands,
                    &mut pending,
                    entity,
                    cur,
                    &kit,
                    &visuals.0,
                    &mut out,
                );
            }
        } else {
            if held_spell(&pending, entity) == Some(prev) {
                // Only the ending channel's own hold drops; an in-flight precast survives.
                commands.entity(entity).try_remove::<CastHold>();
                pending.insert(entity, None);
            }
            out.sounds.write(SpellKitSound::StopHold { entity });
            out.fx.write(SpellKitFx::Reap {
                entity,
                spell_id: prev,
                class: FxClass::Hold,
            });
        }
    }
    // Streamed units despawn on range-out; drop their cache rows.
    channel_cache.retain(|e, _| units.contains(*e));
}

/// The lanes an aura's state kit arms on.
struct StateKitWriters<'a, 'w1, 'w2, 'w3, 'w4, 'w5, 'w6> {
    fx: &'a mut MessageWriter<'w1, SpellKitFx>,
    procs: &'a mut MessageWriter<'w2, AuraProc>,
    sounds: &'a mut MessageWriter<'w3, SpellKitSound>,
    recomputes: &'a mut MessageWriter<'w4, BaseAnimRecompute>,
    sheaths: &'a mut MessageWriter<'w5, super::SheathRequest>,
    body: &'a mut MessageWriter<'w6, crate::aura_visual::BodyProcPlay>,
}

/// A state kit's models and proc nodes; `None` when the kit arms nothing at all.
fn state_kit_parts(
    visuals: &SpellVisualCatalog,
    kit: &VisualKit,
) -> Option<(Vec<FxSlot>, Vec<crate::aura_visual::AuraNode>)> {
    let effects = resolve_kit_effects(visuals, kit);
    let nodes: Vec<crate::aura_visual::AuraNode> = kit
        .char_procs()
        .filter_map(crate::aura_visual::node_for)
        .collect();
    let empty = effects.is_empty() && nodes.is_empty() && kit.sound.is_none();
    (!empty || kit.anim_id.is_some()).then_some((effects, nodes))
}

/// One aura state kit's arm: the stow, the body procs, the models, the proc nodes, the sound and
/// the stage-2 anim leg.
fn arm_state_kit(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    (effects, nodes): (Vec<FxSlot>, Vec<crate::aura_visual::AuraNode>),
    spells: &crate::ui_action::Spells,
    w: &mut StateKitWriters,
) {
    debug!(
        "anim: aura state kit armed ({entity}, spell {spell_id}): {} model(s), {} proc node(s)",
        effects.len(),
        nodes.len()
    );
    stow_for_weapon_effects(w.sheaths, spells, entity, spell_id, kit);
    play_body_procs(w.body, entity, spell_id, kit);
    if !effects.is_empty() {
        w.fx.write(SpellKitFx::Begin {
            entity,
            spell_id,
            persistent: true,
            class: FxClass::AuraState,
            stage: state_stage(kit),
            effects,
        });
    }
    if !nodes.is_empty() {
        w.procs.write(AuraProc::Begin {
            entity,
            spell_id,
            nodes,
        });
    }
    // Ungated; a same-frame duplicate of the impact hand-off's collapses downstream.
    if let Some(kit_sound) = kit.sound {
        w.sounds.write(SpellKitSound::Play { entity, kit_sound });
    }
    // The stage-2 anim leg, on the add edge only: an aura refreshed in place keeps its spell, and
    // the slot already armed with it is skipped (`0x7248a8`).
    if let Some(anim_id) = kit.anim_id {
        w.recomputes.write(BaseAnimRecompute { entity, anim_id });
    }
}

/// Arm and reap each aura's state kit (stage 2) off the unit's [`UnitAuras`]: a spell entering
/// the table begins its kit's models persistent, also on a unit streamed in mid-aura (its first
/// `SMSG_AURA_UPDATE_ALL`), and leaving reaps them. 12340's applied core (`0x724820`) plays the
/// spell's `SpellVisual` state kit (column 4, `+0x10`) at stage 2 through the unit's kit player
/// (`0x745230`, `0x724c86`), and its removal core (`0x71e930`) stops the spell's kits with
/// `0x743b40(spellId, force=1)` (`0x71ebda`), then plays the state-done kit ([`StateDoneKit`]).
/// The add edge rings the kit sound and never plays the kit's anim: stage 2 compares it with what
/// the unit plays, a mismatch spent on a [`BaseAnimRecompute`]. `CharProc` columns go out as
/// [`AuraProc`] edges, and a kit arms for any leg (Stealth's kit 312 is one `CharProc`). `armed`
/// holds every visible aura's spell, kit or none, as the applied core stores the slot's spell
/// (`0x724820`) whatever its visual.
///
/// A detection aura's kit shows only to a viewer who shares its vision: the applied core skips the
/// kit when the spell's [`aura_vision_mask`](benilla_formats::SpellDisplay::aura_vision_mask) has
/// no bit in our own `PLAYER_FIELD_BYTES2` byte 3 (`0x724979`), and a change of that byte re-arms
/// or reaps every unit's matching auras (`0x727a70`), so a guard's Stealth Detection eye is hidden
/// from anyone who cannot see stealth.
///
/// Deviation: the reference arms per slot (`unit+0xedc`) and its stop is by spell, so losing one
/// of two slots holding a spell stops the kit the other still holds; this arms per spell and
/// reaps when no slot holds it, as one state instance per spell is the evident intent.
pub(crate) fn arm_aura_state_fx(
    // Re-run on a table change and on first sight (a standing aura is a state); the full sweep
    // runs once per DBC-resource arrival.
    units: Query<(Entity, &UnitAuras)>,
    changed: Query<Entity, Changed<UnitAuras>>,
    self_store: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut fx: MessageWriter<SpellKitFx>,
    mut procs: MessageWriter<AuraProc>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut recomputes: MessageWriter<BaseAnimRecompute>,
    mut done: MessageWriter<StateDoneKit>,
    mut sheaths: MessageWriter<super::SheathRequest>,
    mut body: MessageWriter<crate::aura_visual::BodyProcPlay>,
    (mut waits, time): (Option<ResMut<BeamKitWaits>>, Res<Time>),
    mut armed: Local<EntityHashMap<Vec<u32>>>,
    mut seen_vision: Local<u8>,
) {
    // No active player sees nothing gated (`0x72496a` returns on a null player).
    let vision = self_store
        .iter()
        .next()
        .and_then(|s| s.0.player_aura_vision())
        .unwrap_or(0);
    let vision_changed = std::mem::replace(&mut *seen_vision, vision) != vision;
    let full_sweep = vision_changed
        || visuals.as_ref().is_some_and(|v| v.is_changed())
        || spells.as_ref().is_some_and(|s| s.is_changed());
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        return; // no client data
    };
    let mut writers = StateKitWriters {
        fx: &mut fx,
        procs: &mut procs,
        sounds: &mut sounds,
        recomputes: &mut recomputes,
        sheaths: &mut sheaths,
        body: &mut body,
    };
    // A held state kit the beam reached plays now, past the wait (`0x7227f7`).
    let reached = waits.as_mut().map_or_else(Vec::new, |w| {
        w.bypass_change_detection().take_reached_auras()
    });
    for (entity, spell_id) in reached {
        let kit = resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None);
        if let Some((kit, parts)) = kit
            .filter(|_| units.contains(entity))
            .and_then(|k| state_kit_parts(&visuals.0, &k).map(|p| (k, p)))
        {
            arm_state_kit(entity, spell_id, &kit, parts, spells, &mut writers);
        }
    }
    let scan = if full_sweep {
        units.iter().collect::<Vec<_>>()
    } else {
        changed.iter().filter_map(|e| units.get(e).ok()).collect()
    };
    for (entity, auras) in scan {
        let prev = armed.entry(entity).or_default();
        // Occupied slots this viewer may see, deduped: one state instance per spell however many
        // casters hold it.
        let mut cur: Vec<u32> = auras
            .iter()
            .map(|a| a.spell_id)
            .filter(|&id| {
                let mask = spells.catalog.get(id).map_or(0, |d| d.aura_vision_mask());
                mask == 0 || vision & mask != 0
            })
            .collect();
        cur.sort_unstable();
        cur.dedup();
        for &spell_id in prev.iter() {
            if cur.binary_search(&spell_id).is_err() {
                debug!("anim: aura state kit reaped ({entity}, spell {spell_id})");
                writers.fx.write(SpellKitFx::Reap {
                    entity,
                    spell_id,
                    class: FxClass::AuraState,
                });
                writers.procs.write(AuraProc::Reap { entity, spell_id });
                // A looping state-kit sound dies with the aura's kits (`0x743b40`).
                if let Some(kit_sound) =
                    resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None)
                        .and_then(|k| k.sound)
                {
                    writers
                        .sounds
                        .write(SpellKitSound::StopKit { entity, kit_sound });
                }
                // Then the state-done kit, for any slot the applied core armed (`0x71eb11`).
                done.write(StateDoneKit { entity, spell_id });
            }
        }
        let mut next = Vec::with_capacity(prev.len());
        for &spell_id in &cur {
            // Armed whatever its visual, so its end plays the state-done kit.
            next.push(spell_id);
            if prev.contains(&spell_id) {
                continue; // still armed
            }
            let Some(kit) = resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None) else {
                continue; // no state kit
            };
            let Some(parts) = state_kit_parts(&visuals.0, &kit) else {
                continue; // nothing to arm or reap
            };
            // A beam spell's flag-0x1 state kit waits for the beam to reach this unit (`0x72af60`).
            if let Some(w) = waits
                .as_mut()
                .filter(|_| kit_waits_for_beam(spells, &visuals.0, spell_id, &kit))
            {
                let now = time.elapsed_secs();
                w.bypass_change_detection().hold(
                    entity,
                    spell_id,
                    NO_BEAM_KEY,
                    WaitPlay::AuraState,
                    now,
                );
                continue;
            }
            arm_state_kit(entity, spell_id, &kit, parts, spells, &mut writers);
        }
        *prev = next;
    }
    // Streamed units despawn on range-out; drop their rows.
    armed.retain(|e, _| units.contains(*e));
}

/// Arm and reap the loot sparkle on a dead lootable unit and on a corpse object with
/// `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0: the `"HARDCODED Loot Art"` row at `0x13`. Edge-driven with
/// no viewer or distance logic, since the server strips the flag per viewer; the falling edge reaps
/// with no fade.
pub(super) fn arm_loot_fx(
    // Re-read on an edge of any of the four fields and on first sight (armed at build).
    units: Query<(Entity, &ObjectStore, &crate::net::NetEntity)>,
    arrived: Query<Entity, (Added<ObjectStore>, Without<ItemObject>)>,
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
    mut armed: Local<EntityHashSet>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let full_sweep = visuals.as_ref().is_some_and(|v| v.is_changed());
    let Some((effect, path)) = visuals.as_ref().and_then(|v| v.0.loot_art_effect()) else {
        return; // no client data or no such row
    };
    let scan = if full_sweep {
        units.iter().collect::<Vec<_>>()
    } else {
        use benilla_protocol::field::{
            FIELD_CORPSE_DYNAMIC_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH,
            FIELD_UNIT_MAXHEALTH,
        };
        let mut due: EntityHashSet = arrived.iter().collect();
        due.extend(
            edges
                .read()
                .filter(|e| {
                    e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS)
                        || e.unit_field(FIELD_UNIT_HEALTH)
                        || e.unit_field(FIELD_UNIT_MAXHEALTH)
                        || (e.kind == benilla_protocol::messages::ObjectType::Corpse
                            && e.index == FIELD_CORPSE_DYNAMIC_FLAGS)
                })
                .map(|e| e.entity),
        );
        due.into_iter().filter_map(|e| units.get(e).ok()).collect()
    };
    for (entity, store, net) in scan {
        // A corpse object's sparkle: bit 0 rising spawns the same row at the same tag. A
        // GameObject's is [`arm_go_loot_fx`]'s, and its store's indices are not unit fields.
        let lootable = match net.kind {
            benilla_protocol::EntityKind::Corpse => store.0.corpse_lootable(),
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player => {
                store.0.unit_is_dead() && store.0.unit_lootable()
            }
            _ => continue,
        };
        if lootable && armed.insert(entity) {
            // The Looting tutorial on the rise.
            if let Some(t) = tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::trigger(
                    crate::tutorial::id::LOOTING,
                ));
            }
            fx.write(SpellKitFx::Begin {
                entity,
                spell_id: LOOT_FX_KEY,
                persistent: true,
                class: FxClass::Hold,
                // A unit's loot art loops its first sequence. A corpse's is a bare model with no
                // sequence armed; this plumbing has no such instance, so it runs `OneShot`
                // persistent.
                stage: if net.kind == benilla_protocol::EntityKind::Corpse {
                    FxStage::OneShot
                } else {
                    FxStage::Relive
                },
                effects: vec![FxSlot {
                    tag: HARDCODED_FX_ATTACH,
                    effect,
                    path: path.to_string(),
                    scale: 1.0,
                    attach: None,
                }],
            });
        } else if !lootable && armed.remove(&entity) {
            fx.write(SpellKitFx::Reap {
                entity,
                spell_id: LOOT_FX_KEY,
                class: FxClass::Hold,
            });
        }
    }
    // Streamed units despawn on range-out; drop their rows.
    armed.retain(|e| units.contains(*e));
}

/// `GAMEOBJECT_DYNAMIC` bit `0x08`, the loot sparkle a quest object wears (`0x71121c`).
const GO_DYNFLAG_SPARKLE: u32 = 0x08;

/// The GameObject loot sparkle's uniform local scale (`0x70d0f1`): the object's scale × its model's
/// bounding-box Z extent (stored at model load, `0x713f7a`) × 1.15, clamped to 1..5.
fn go_loot_fx_scale(object_scale: f32, height: f32) -> f32 {
    let s = object_scale * height * 1.15;
    if s.is_nan() {
        1.0
    } else {
        s.clamp(1.0, 5.0)
    }
}

/// Whether a `GAMEOBJECT_DYNAMIC` edge re-runs the sparkle check: only a flip of bit `0x08`
/// (`0x712b68`).
fn go_sparkle_edge(old: u32, new: u32) -> bool {
    (old ^ new) & GO_DYNFLAG_SPARKLE != 0
}

/// The resource-tracking inputs of the object tracking check, the sparkle's second leg.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct GoTrackSrc<'w> {
    templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    locks: Option<Res<'w, crate::go_templates::Locks>>,
    factions: Option<Res<'w, crate::target::Factions>>,
}

/// Arm and reap a GameObject's loot sparkle (`0x711210`): the `"HARDCODED Loot Art"` row at `0x13`
/// while `GAMEOBJECT_DYNAMIC` carries bit `0x08` or our resource tracking finds the object
/// (`0x6dca90`), sized by [`go_loot_fx_scale`]; the model's local matrix is what its flag `0x8000`
/// enables (`0x70d144`). It runs when the bit flips, when the model loads (`0x71403a`), and for every
/// object when our tracking mask or death changes (`0x6d5290`); a model not yet loaded arms nothing
/// (`0x70d0a8`). Losing both legs releases it with no fade (`0x711286`).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn arm_go_loot_fx(
    objects: Query<(
        Entity,
        &ObjectStore,
        &crate::net::NetEntity,
        &crate::net::Guid,
        Option<&crate::entities::OverheadFallback>,
    )>,
    loaded: Query<Entity, Changed<crate::entities::OverheadFallback>>,
    mut edges: MessageReader<FieldChanged>,
    self_store: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    track: GoTrackSrc,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
    mut armed: Local<EntityHashSet>,
    mut seen_self: Local<(u32, bool)>,
) {
    use benilla_protocol::field::FIELD_GAMEOBJECT_DYN_FLAGS;
    let me = self_store.iter().next();
    let mask = me.map_or(0, |m| m.0.player_track_resources());
    // `PLAYER_FLAGS` ghost bit or no health: the tracking check's dead gate.
    let self_dead =
        me.is_some_and(|m| m.0.unit_health().unwrap_or(1) == 0 || m.0.player_flags() & 0x10 != 0);
    let self_moved = std::mem::replace(&mut *seen_self, (mask, self_dead)) != (mask, self_dead);
    // A template or lock arriving matters only while something is tracked.
    let tables_moved = mask != 0
        && (track.templates.as_ref().is_some_and(|t| t.is_changed())
            || track.locks.as_ref().is_some_and(|l| l.is_changed()));
    let full_sweep = self_moved || tables_moved || visuals.as_ref().is_some_and(|v| v.is_changed());
    let Some((effect, path)) = visuals.as_ref().and_then(|v| v.0.loot_art_effect()) else {
        edges.clear();
        return; // no client data or no such row
    };
    let scan: Vec<_> = if full_sweep {
        edges.clear();
        objects.iter().collect()
    } else {
        let mut due: EntityHashSet = loaded.iter().collect();
        due.extend(
            edges
                .read()
                .filter(|e| {
                    e.kind == benilla_protocol::messages::ObjectType::GameObject
                        && e.index == FIELD_GAMEOBJECT_DYN_FLAGS
                        && go_sparkle_edge(e.old, e.new)
                })
                .map(|e| e.entity),
        );
        due.into_iter()
            .filter_map(|e| objects.get(e).ok())
            .collect()
    };
    for (entity, store, net, guid, height) in scan {
        if net.kind != benilla_protocol::EntityKind::GameObject {
            continue;
        }
        let f = &store.0;
        let wanted = f.gameobject_dynamic_flags() & GO_DYNFLAG_SPARKLE != 0
            || crate::go_templates::can_track_object(
                self_dead,
                f.gameobject_type_id(),
                || {
                    let reaction = crate::target::cursor_mode::go_reaction(
                        track.factions.as_deref(),
                        f.gameobject_faction(),
                        me,
                    );
                    crate::target::cursor_mode::go_can_use(
                        f.gameobject_flags(),
                        f.gameobject_dynamic_flags(),
                        reaction,
                    )
                },
                mask,
                track
                    .templates
                    .as_deref()
                    .and_then(|t| t.get(guid.0))
                    .map_or(0, |t| t.lock_id),
                track.locks.as_deref().map(|l| &l.0),
            );
        if wanted {
            // No-op while the effect stands or the model is not loaded (`0x70d089`, `0x70d0af`).
            let Some(height) = height else { continue };
            if !armed.insert(entity) {
                continue;
            }
            let scale = go_loot_fx_scale(net.scale, height.0);
            fx.write(SpellKitFx::Begin {
                entity,
                spell_id: LOOT_FX_KEY,
                persistent: true,
                class: FxClass::Hold,
                // A bare model with no sequence armed, as the corpse's.
                stage: FxStage::OneShot,
                effects: vec![FxSlot {
                    tag: HARDCODED_FX_ATTACH,
                    effect,
                    path: path.to_string(),
                    scale,
                    attach: None,
                }],
            });
        } else if armed.remove(&entity) {
            fx.write(SpellKitFx::Reap {
                entity,
                spell_id: LOOT_FX_KEY,
                class: FxClass::Hold,
            });
        }
    }
    // A despawned object takes its effect with it; drop its row.
    armed.retain(|e| objects.contains(*e));
}

/// The level-up ding: any streamed unit's `UNIT_FIELD_LEVEL` change spawns [`LEVEL_UP_EFFECT`],
/// independent of `SMSG_LEVELUP_INFO`; a unit streaming in does not ding. Its sound is the model's
/// own `$SND` event.
pub(super) fn arm_level_up_fx(
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_LEVEL) {
            continue;
        }
        let Some((effect, path)) = visuals
            .as_ref()
            .and_then(|v| v.0.hardcoded_effect(LEVEL_UP_EFFECT))
        else {
            continue; // no client data or no such row
        };
        debug!(
            "anim: level {} → {}, the ding flashes ({})",
            e.old, e.new, e.entity
        );
        fx.write(SpellKitFx::Begin {
            entity: e.entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            // Self-terminates on its own 1.867 s clip.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: HARDCODED_FX_ATTACH,
                effect,
                path: path.to_string(),
                scale: 1.0,
                attach: None,
            }],
        });
    }
}

/// The mount poof: a `UNIT_FIELD_MOUNTDISPLAYID` change to nonzero on any unit spawns
/// [`MOUNT_POOF_EFFECT`] on the rider as a one-shot. The same-record, same-tag replace is not
/// built, so a mount swap within 2.8 s would show two clouds.
pub(super) fn arm_mount_poof_fx(
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    for e in edges.read() {
        // A dismount spawns nothing; the edge stream skips creation.
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) || e.new == 0 {
            continue;
        }
        let Some((effect, path)) = visuals
            .as_ref()
            .and_then(|v| v.0.hardcoded_effect(MOUNT_POOF_EFFECT))
        else {
            continue; // no client data or no such row
        };
        debug!(
            "anim: mount display {} → {}, the poof puffs ({})",
            e.old, e.new, e.entity
        );
        fx.write(SpellKitFx::Begin {
            entity: e.entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            // Destroyed at first completion; the shipped model runs 2.8 s.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: HARDCODED_FX_ATTACH,
                effect,
                path: path.to_string(),
                scale: 1.0,
                attach: None,
            }],
        });
    }
}

/// The achievement earn effect, hardcoded index 0xb (`0x5b310b`, name at `0xa329a8`).
const ACHIEVEMENT_EFFECT: &str = "HARDCODED Achievement Base";

/// Whether `SMSG_ACHIEVEMENT_EARNED` plays the earn effect (`0x5b30c5`-`0x5b30d3`): not silent,
/// a known row, and the row's flag `0x4` clear. Any player in view, not only ours.
pub(super) fn earn_effect_plays(
    db: Option<&benilla_ui::script::AchievementDb>,
    achievement: u32,
    silent: bool,
) -> bool {
    !silent
        && db
            .and_then(|d| d.get(achievement))
            .is_some_and(|a| a.flags & 0x4 == 0)
}

/// `SMSG_TITLE_EARNED`'s earn effect, hardcoded index 9 (`0x50c681`, name at `0xa329dc`).
const TITLE_EFFECT: &str = "HARDCODED Reputation";

/// The earn effect on `entity` (`0x5b30d5`-`0x5b311e`): a one-shot at `0x13` (`[0xa32b20]`),
/// replacing an earlier one through the same-slot replace (`0x6f8a60`).
pub(crate) fn achievement_earn_fx(visuals: &SpellVisuals, entity: Entity) -> Option<SpellKitFx> {
    hardcoded_one_shot(visuals, entity, ACHIEVEMENT_EFFECT)
}

/// The earned title's effect on our player, the same one-shot (`0x50c643`-`0x50c690`).
pub(crate) fn title_earn_fx(visuals: &SpellVisuals, entity: Entity) -> Option<SpellKitFx> {
    hardcoded_one_shot(visuals, entity, TITLE_EFFECT)
}

fn hardcoded_one_shot(visuals: &SpellVisuals, entity: Entity, name: &str) -> Option<SpellKitFx> {
    let (effect, path) = visuals.0.hardcoded_effect(name)?;
    Some(SpellKitFx::Begin {
        entity,
        spell_id: 0,
        persistent: false,
        class: FxClass::Hold,
        stage: FxStage::OneShot,
        effects: vec![FxSlot {
            tag: HARDCODED_FX_ATTACH,
            effect,
            path: path.to_string(),
            scale: 1.0,
            attach: None,
        }],
    })
}

/// The pending-morph latch, the reference's `[unit+0xd54]`: armed on every aura add and remove of
/// an [`is_morph_spell`] spell; the display rebuild replays its impact kit, the shapeshift cloud
/// both ways. Shift-out works because the aura fields apply before `DISPLAYID`.
#[derive(Resource, Default)]
pub(crate) struct MorphLatch(EntityHashMap<u32>);

/// The latch-arm predicate: an effect slot with `Effect` 6 (APPLY_AURA) whose aura is 36
/// (MOD_SHAPESHIFT) or 56 (TRANSFORM), slot-paired, with no other gate.
fn is_morph_spell(spells: &crate::ui_action::Spells, spell_id: u32) -> bool {
    const SPELL_EFFECT_APPLY_AURA: u32 = 6;
    const SPELL_AURA_MOD_SHAPESHIFT: u32 = 36;
    const SPELL_AURA_TRANSFORM: u32 = 56;
    spells.catalog.get(spell_id).is_some_and(|d| {
        d.effects.iter().zip(&d.effect_apply_aura).any(|(&e, &a)| {
            e == SPELL_EFFECT_APPLY_AURA
                && (a == SPELL_AURA_MOD_SHAPESHIFT || a == SPELL_AURA_TRANSFORM)
        })
    })
}

/// Arm [`MorphLatch`] from aura-slot edges, as the add watcher and the remove tail both arm it: old
/// spell first, new second, so a form swap latches the form entered. A slot edge is a [`UnitAuras`]
/// change diffed against the slots last seen; first sight emits no edge, as the reference's
/// watchers fire on deltas only.
pub(super) fn arm_morph_latch(
    units: Query<(), With<ObjectStore>>,
    changed: Query<(Entity, &UnitAuras), Changed<UnitAuras>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut latch: ResMut<MorphLatch>,
    mut seen: Local<EntityHashMap<std::collections::BTreeMap<u8, u32>>>,
) {
    for (entity, auras) in &changed {
        let now: std::collections::BTreeMap<u8, u32> =
            auras.iter().map(|a| (a.slot, a.spell_id)).collect();
        let Some(before) = seen.insert(entity, now.clone()) else {
            continue;
        };
        let Some(spells) = spells.as_deref() else {
            continue;
        };
        let slots: std::collections::BTreeSet<u8> =
            before.keys().chain(now.keys()).copied().collect();
        for slot in slots {
            let (old, new) = (before.get(&slot).copied(), now.get(&slot).copied());
            if old == new {
                continue;
            }
            for spell_id in [old, new].into_iter().flatten() {
                if is_morph_spell(spells, spell_id) {
                    debug!("anim: morph latch armed ({entity}, spell {spell_id})");
                    latch.0.insert(entity, spell_id);
                }
            }
        }
    }
    seen.retain(|e, _| units.contains(*e));
    // The latch dies with a despawned unit.
    latch.0.retain(|e, _| units.contains(*e));
}

/// An aura's end on a unit whose slot the applied core armed: its spell's state-done kit plays.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) struct StateDoneKit {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
}

/// The removal core's tail (`0x71ebef`-`0x71ec4c`): the `SpellVisual` row's state-done kit
/// (column 5, `+0x14`) through the kit player at stage 8, which takes the one-shot handler with
/// stages 1, 5 and 6 (`0x745230`), so every leg plays and the models end after one pass.
pub(super) fn play_state_done_kits(
    mut ends: MessageReader<StateDoneKit>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut play_seq: ResMut<super::PlaySeq>,
    mut oneshots: MessageWriter<EmoteAnim>,
    mut wounds: MessageWriter<WoundAnim>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut fx: MessageWriter<SpellKitFx>,
    mut chain: MessageWriter<ChainProcPlay>,
    mut shakes: MessageWriter<SpellKitShake>,
    mut trails: MessageWriter<crate::weapon_trail::TrailArm>,
    mut recomputes: MessageWriter<BaseAnimRecompute>,
    mut sheaths: MessageWriter<super::SheathRequest>,
    mut body: MessageWriter<crate::aura_visual::BodyProcPlay>,
    mut waits: Option<ResMut<BeamKitWaits>>,
) {
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        ends.clear();
        return; // no client data
    };
    for end in ends.read() {
        let Some(kit) = resolve_kit(spells, &visuals.0, end.spell_id, |s| s.state_done, || None)
        else {
            continue; // no state-done kit
        };
        debug!(
            "anim: aura state-done kit plays ({}, spell {})",
            end.entity, end.spell_id
        );
        let mut out = KitOut {
            oneshots: &mut oneshots,
            wounds: &mut wounds,
            sounds: &mut sounds,
            fx: &mut fx,
            chain: &mut chain,
            shakes: &mut shakes,
            trails: &mut trails,
            recomputes: &mut recomputes,
            sheaths: &mut sheaths,
            spells,
            body: &mut body,
            // Only a unit carries an aura, so its state-done kit may wait (`0x72af60`).
            waits: waits.as_mut().map(|w| BeamHold {
                waits: w.bypass_change_detection(),
                is_unit: &|_| true,
            }),
        };
        play_kit(
            end.entity,
            end.spell_id,
            &kit,
            KitPlay::DISCRETE,
            play_seq.next(),
            &visuals.0,
            &mut out,
        );
    }
}

/// The rebuild's impact-kit replay: a display swap with the latch armed plays the latched impact
/// kit and clears it, a frame later on the rebuilt body. The same-record, same-tag replace is not
/// built, so a cold-cache shift-in briefly shows two clouds.
pub(super) fn replay_morph_kit(
    mut swaps: MessageReader<crate::entities::DisplaySwapped>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut latch: ResMut<MorphLatch>,
    mut play_seq: ResMut<super::PlaySeq>,
    mut oneshots: MessageWriter<EmoteAnim>,
    mut wounds: MessageWriter<WoundAnim>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut fx: MessageWriter<SpellKitFx>,
    mut chain: MessageWriter<ChainProcPlay>,
    mut shakes: MessageWriter<SpellKitShake>,
    mut trails: MessageWriter<crate::weapon_trail::TrailArm>,
    mut recomputes: MessageWriter<BaseAnimRecompute>,
    mut sheaths: MessageWriter<super::SheathRequest>,
    mut body: MessageWriter<crate::aura_visual::BodyProcPlay>,
    mut waits: Option<ResMut<BeamKitWaits>>,
) {
    for swap in swaps.read() {
        // Cleared even when the kit resolves to nothing.
        let Some(spell_id) = latch.0.remove(&swap.entity) else {
            continue; // no latch
        };
        let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
            continue; // no client data
        };
        let Some(kit) = resolve_kit(spells, &visuals.0, spell_id, |s| s.impact, || None) else {
            continue; // no impact kit
        };
        debug!("anim: morph replay ({}, spell {spell_id})", swap.entity);
        let mut out = KitOut {
            oneshots: &mut oneshots,
            wounds: &mut wounds,
            sounds: &mut sounds,
            fx: &mut fx,
            chain: &mut chain,
            shakes: &mut shakes,
            trails: &mut trails,
            recomputes: &mut recomputes,
            sheaths: &mut sheaths,
            spells,
            body: &mut body,
            // Only a unit latches a morph, so its replayed impact kit may wait (`0x72af60`).
            waits: waits.as_mut().map(|w| BeamHold {
                waits: w.bypass_change_detection(),
                is_unit: &|_| true,
            }),
        };
        play_kit(
            swap.entity,
            spell_id,
            &kit,
            KitPlay::DISCRETE,
            play_seq.next(),
            &visuals.0,
            &mut out,
        );
    }
}
