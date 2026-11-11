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

/// Wire result 187, the one reason byte the end-cast keeps the held model for (`0x7feeb0`).
const LAUNCH_RESULT: u8 = 187;

/// Whether a cast's end drops its held missile model: a failure of a numbered cast whose
/// `SpellMissile` row has flag 1, tested on the reason byte as sent (`0x7feeb0`).
pub(crate) fn end_cast_clears_hold(
    spells: &crate::ui_action::Spells,
    spell_id: u32,
    cast_count: u8,
    wire_reason: u8,
) -> bool {
    wire_reason != LAUNCH_RESULT
        && cast_count != 0
        && spells.catalog.get(spell_id).is_some_and(|d| d.flies_aim)
}

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
    /// The missile model held on the caster during the cast, one per unit: a new hold, the
    /// missile's release or an interrupt drops it at once (`0x722950`).
    Precast,
    /// A missile's targeting kit at stage 2, keyed by the flight's fade stamp, which only its
    /// models carry (`0x744bd0`, `0x6ff410`).
    Targeting(u32),
}

impl FxClass {
    /// Whether a reap of this class takes an instance of `other`: the aura's force clear takes the
    /// targeting kits' stage-2 models too (`0x743b40`).
    pub(crate) fn reaps(self, other: FxClass) -> bool {
        self == other || (self == FxClass::AuraState && matches!(other, FxClass::Targeting(_)))
    }
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
    /// A flagged row's held missile model ([`FxClass::Precast`]): drops the unit's last at once,
    /// then holds `slot` when the unit's body is built and has the slot's attachment (`0x722950`).
    Hold {
        entity: Entity,
        spell_id: u32,
        /// The cast count the end-cast clear matches (`0x722acb`).
        cast_count: u8,
        /// The edge's `PlaySeq` stamp: a [`MissileRelease`] stamped earlier leaves it.
        seq: u64,
        slot: Option<FxSlot>,
    },
}

/// A release of a caster's missiles outside its animation, applied in `seq` order with the
/// [`MissileSpawn`]s by `crate::entities::missile`.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(crate) struct MissileRelease {
    pub(crate) caster: Entity,
    pub(crate) seq: u64,
    pub(crate) kind: ReleaseKind,
}

/// What a [`MissileRelease`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseKind {
    /// Launch the caster's queue now and drop its held model (`0x72e240`).
    Fire,
    /// Drop the held model of this cast count alone (`0x71bff0`).
    Clear(u8),
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
    /// The unit a stage-2 play's guid names (flag `0x1000`, `0x745409`), the beam's source.
    pub(crate) from: Option<Entity>,
}

/// A unit's channel spell changing away from `spell_id` (`0x73eb50`), from the channel poll to
/// [`arm_aura_state_fx`], which clears the spell's effects and unarms its aura slots.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSpellLeft {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
}

/// The clear of every effect of a spell on a unit (`0x743b40` with force 1), as an armed aura slot
/// leaves or a channel ends; consumed by `crate::entities::chain_beam` for persistent beams.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpellEffectClear {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
}

/// The clear of one kit's effects on a unit by kit alone (`0x744bd0`), the precast kit's at a
/// cast's end (`0x7fec2f`); consumed by `crate::entities::chain_beam` for persistent beams and
/// `crate::aura_visual` for anim-rate effects.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KitEffectClear {
    pub(crate) entity: Entity,
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
    /// The GO's `PlaySeq` stamp, which orders it among the [`MissileRelease`]s.
    pub(crate) seq: u64,
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

/// The held missile model of a flagged row: column 8's file at column 16's attachment, `None`
/// for column 16's -1 or a column 8 naming no file (`0x722950`).
fn precast_missile_slot(visuals: &SpellVisualCatalog, stages: &VisualStages) -> Option<FxSlot> {
    let tag = stages.precast_missile_attachment()?;
    let path = visuals.effect_path(stages.missile_model)?;
    Some(FxSlot {
        tag,
        effect: stages.missile_model,
        path: path.to_string(),
        scale: 1.0,
        attach: None,
    })
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

/// The kit lanes [`route_cast_visuals`] writes but does not own, bundled into one param because
/// the system is at Bevy's 16-`SystemParam` limit.
type KitSpawnWriters<'w> = (
    MessageWriter<'w, MissileSpawn>,
    MessageWriter<'w, crate::entities::dest_fx::GroundBurst>,
    MessageWriter<'w, ChainProcPlay>,
    MessageWriter<'w, SpellKitShake>,
    MessageWriter<'w, crate::weapon_trail::TrailArm>,
    MessageWriter<'w, BaseAnimRecompute>,
    MessageWriter<'w, DelayedAreaKit>,
    MessageWriter<'w, crate::aura_visual::BodyProcPlay>,
    MessageWriter<'w, MissileRelease>,
    MessageWriter<'w, ChannelSpellLeft>,
    MessageWriter<'w, KitEffectClear>,
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
    /// The play's caster guid as an entity; only a stage-2 play hands it to the beam.
    caster: Option<Entity>,
    /// The models' reap class.
    class: FxClass,
}

impl KitPlay {
    /// Stages 0 and 1: self-terminating models, every leg on.
    const DISCRETE: Self = Self {
        persistent: false,
        effects: true,
        sound: true,
        stage: FxStage::OneShot,
        stage_2: false,
        caster: None,
        class: FxClass::Hold,
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
            // Of the unit plays built here only stage 2 sets `0x1000` (`0x745409`).
            from: play.caster.filter(|_| play.stage_2),
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
            class: play.class,
            stage: play.stage,
            effects,
        });
    }
}

/// The lanes a unit kit play fans out to from outside the cast router: the missile's targeting kit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitKitLanes<'w> {
    oneshots: MessageWriter<'w, EmoteAnim>,
    wounds: MessageWriter<'w, WoundAnim>,
    pub(crate) sounds: MessageWriter<'w, SpellKitSound>,
    pub(crate) fx: MessageWriter<'w, SpellKitFx>,
    chain: MessageWriter<'w, ChainProcPlay>,
    shakes: MessageWriter<'w, SpellKitShake>,
    trails: MessageWriter<'w, crate::weapon_trail::TrailArm>,
    recomputes: MessageWriter<'w, BaseAnimRecompute>,
    sheaths: MessageWriter<'w, super::SheathRequest>,
    body: MessageWriter<'w, crate::aura_visual::BodyProcPlay>,
    procs: MessageWriter<'w, AuraProc>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
}

impl UnitKitLanes<'_> {
    /// A missile's targeting kit on its target at stage 2, the whole kit player with the caster as
    /// the play's guid and the flight's fade stamp (`0x702906`, `0x702923`). Only the models take
    /// the stamp: the dispatcher's beams and anim rate get none (`0x73b434`), so the flight's
    /// clear leaves them (`0x744cc8`).
    pub(crate) fn play_targeting_kit(
        &mut self,
        target: Entity,
        caster: Entity,
        spell_id: u32,
        kit: &VisualKit,
        fade: u32,
        visuals: &SpellVisualCatalog,
    ) {
        let Some(spells) = self.spells.as_deref() else {
            return; // no client data
        };
        let mut out = KitOut {
            oneshots: &mut self.oneshots,
            wounds: &mut self.wounds,
            sounds: &mut self.sounds,
            fx: &mut self.fx,
            chain: &mut self.chain,
            shakes: &mut self.shakes,
            trails: &mut self.trails,
            recomputes: &mut self.recomputes,
            sheaths: &mut self.sheaths,
            spells,
            body: &mut self.body,
        };
        let play = KitPlay {
            persistent: true,
            stage: state_stage(kit),
            stage_2: true,
            caster: Some(caster),
            class: FxClass::Targeting(fade),
            ..KitPlay::DISCRETE
        };
        // Stage 2 never plays the anim, so no one-shot takes the sequence stamp.
        play_kit(target, spell_id, kit, play, 0, visuals, &mut out);
        play_proc_nodes(&mut self.procs, target, spell_id, kit);
    }
}

/// The dispatcher's node cases (tint, anim rate, alpha) for one kit play, keyed by spell, the anim
/// rate also by kit for a kit clear to match (`0x7265c0`).
fn play_proc_nodes(
    procs: &mut MessageWriter<AuraProc>,
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
) {
    let nodes: Vec<crate::aura_visual::AuraNode> = kit
        .char_procs()
        .filter_map(crate::aura_visual::node_for)
        .collect();
    if !nodes.is_empty() {
        procs.write(AuraProc::Begin {
            entity,
            spell_id,
            nodes,
            kit_id: kit.id,
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
                // No caster queue to order against.
                seq: 0,
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
    /// A missile's arrival; an instant hit plays its impact kit alone, with no guid (`0x801f10`).
    missile: Option<MissileHit>,
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

/// A missile landing on a unit, as its hit reads it (`0x702db0`).
#[derive(Clone, Copy)]
struct MissileHit {
    /// The caster both of its plays carry (`0x702f5f`).
    caster: Entity,
    /// The target's health is above 0 (`0x702f17`).
    alive: bool,
    /// The target holds an aura of the spell (`0x702f25`), `Some(true)` with an effect bit set
    /// (`0x702f3f`).
    aura_active: Option<bool>,
}

impl MissileHit {
    /// The hit on `target` of a missile `caster` fired for `spell_id`.
    fn on(
        caster: Entity,
        target: Option<(&ObjectStore, Option<&UnitAuras>)>,
        spell_id: u32,
    ) -> Self {
        let alive = target.is_some_and(|(s, _)| s.0.unit_health().is_some_and(|h| h > 0));
        let aura_active = target.and_then(|(_, a)| a).and_then(|auras| {
            let mut held = auras.iter().filter(|a| a.spell_id == spell_id).peekable();
            held.peek()?;
            Some(held.any(|a| a.flags & AURA_EFFECT_BITS != 0))
        });
        Self {
            caster,
            alive,
            aura_active,
        }
    }

    /// The state kit plays on a living target holding the aura, an active one when the kit asks.
    fn plays_state(&self, kit: &VisualKit) -> bool {
        self.alive
            && self.aura_active.is_some_and(|active| {
                active || kit.flags & benilla_formats::kit_flag::STATE_NEEDS_ACTIVE_AURA == 0
            })
    }
}

/// An aura's effect-index bits, its active test (`0x728300`, `0x72f8e1`).
const AURA_EFFECT_BITS: u8 = 0x7;

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

/// The unit-impact hand-off: the impact kit (stage 1), then for a missile the state kit's anim,
/// sound and beams when [`MissileHit::plays_state`], its models owned by [`arm_aura_state_fx`].
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
        missile,
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
    let discrete = KitPlay {
        caster: missile.map(|m| m.caster),
        ..KitPlay::DISCRETE
    };
    if let Some(kit) = impact_kit(visuals, &stages, on_caster) {
        match at {
            None => play_kit(entity, spell_id, kit, discrete, seq, visuals, out),
            Some(at) => {
                // The unit keeps the anim, beam and trail legs; the models, sound and shake go
                // to the point.
                let unit_legs = VisualKit {
                    shake: None,
                    ..*kit
                };
                let play = KitPlay {
                    effects: false,
                    sound: false,
                    ..discrete
                };
                play_kit(entity, spell_id, &unit_legs, play, seq, visuals, out);
                play_kit_at(kit, at, visuals, out.sounds, out.shakes, bursts);
            }
        }
    }
    if let Some(kit) = (stages.state != 0)
        .then(|| visuals.kit(stages.state))
        .flatten()
        .filter(|kit| missile.is_some_and(|m| m.plays_state(kit)))
    {
        let play = KitPlay {
            effects: false,
            stage_2: true,
            stage: FxStage::State,
            ..discrete
        };
        play_kit(entity, spell_id, kit, play, seq, visuals, out);
    }
}

/// A cast's end clears its precast kit's effects on the caster, of any spell (`0x7fecc0`, `0x7fec2f`).
fn clear_precast_kit(
    clears: &mut MessageWriter<KitEffectClear>,
    entity: Entity,
    kit: Option<&VisualKit>,
) {
    if let Some(kit) = kit {
        clears.write(KitEffectClear {
            entity,
            kit_id: kit.id,
        });
    }
}

/// A unit the cast router reads: its store, its auras for a missile's hit and whether it is us.
type CastUnit = (
    Entity,
    &'static ObjectStore,
    Option<&'static UnitAuras>,
    Has<crate::net::SelfPlayer>,
);

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
    units: Query<CastUnit, Without<ItemObject>>,
    holds: Query<&CastHold>,
    mut channel_cache: Local<EntityHashMap<u32>>,
) {
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        return; // no client data
    };
    // Disjoint borrows: the beam writer rides `out` while the missile and burst writers stay free.
    let (
        missiles,
        bursts,
        chains,
        shakes,
        trails,
        recomputes,
        delayed,
        body,
        releases,
        channel_left,
    ) = (
        &mut spawns.0,
        &mut spawns.1,
        &mut spawns.2,
        &mut spawns.3,
        &mut spawns.4,
        &mut spawns.5,
        &mut spawns.6,
        &mut spawns.7,
        &mut spawns.8,
        &mut spawns.9,
    );
    let kit_clears = &mut spawns.10;
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
    };

    // `holds` lags one command flush and an instant cast's START and GO drain together, so
    // `pending` overlays this frame's hold writes (else the cast pose sticks).
    let mut pending: EntityHashMap<Option<u32>> = EntityHashMap::default();
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
                    // The `0x400` weapon-visual hold: a ranged visual play sets it, any other
                    // clears it; it keeps a remote shooter drawn.
                    let ranged = spells
                        .catalog
                        .get(ev.spell_id)
                        .is_some_and(|d| d.ranged_slot());
                    // Fallible: `model_fade::apply_despawn_fade` is unordered against this chain
                    // and can despawn the unit before these commands apply.
                    if ranged {
                        commands.entity(ev.entity).try_insert(super::RangedHold);
                    } else {
                        commands.entity(ev.entity).try_remove::<super::RangedHold>();
                    }
                    // A start anim of 0 plays (`0x73b1f9`) but is never offered (`0x722590`).
                    let start_anim = kit.start_anim.filter(|&a| a > 0);
                    // Deviation: a start-only kit offers nothing after its start (`0x722599`); its
                    // hold falls to Stand rather than the idle pick.
                    if let Some(anim_id) = kit.anim_id.or(start_anim.map(|_| 0)) {
                        commands.entity(ev.entity).try_insert(CastHold {
                            anim_id,
                            spell_id: ev.spell_id,
                            ranged,
                            start_anim,
                            seq: ev.seq,
                        });
                        pending.insert(ev.entity, Some(ev.spell_id));
                    }
                    if let Some(kit_sound) = kit.sound {
                        out.sounds.write(SpellKitSound::Play {
                            entity: ev.entity,
                            kit_sound,
                        });
                    }
                    stow_for_weapon_effects(out.sheaths, spells, ev.entity, ev.spell_id, &kit);
                    // Persistent until the cast resolves, re-armed forever (stage 4).
                    let effects = resolve_kit_effects(&visuals.0, &kit);
                    if !effects.is_empty() {
                        out.fx.write(SpellKitFx::Begin {
                            entity: ev.entity,
                            spell_id: ev.spell_id,
                            persistent: true,
                            class: FxClass::Hold,
                            stage: FxStage::Relive,
                            effects,
                        });
                    }
                }
                // Our own start packet launches our missiles and drops any held model right after the
                // start visuals (`0x806a0b`), so a hold of our own never draws.
                if units.get(ev.entity).is_ok_and(|(.., me)| me) {
                    releases.write(MissileRelease {
                        caster: ev.entity,
                        seq: ev.seq,
                        kind: ReleaseKind::Fire,
                    });
                } else if let Some(s) = resolve_stages(spells, &visuals.0, ev.spell_id, || {
                    weapon_src.caster(ev.entity)
                })
                .filter(VisualStages::holds_precast_missile)
                {
                    // A flagged row holds column 8's model at column 16 until the release
                    // (`0x7fa41d`).
                    out.fx.write(SpellKitFx::Hold {
                        entity: ev.entity,
                        spell_id: ev.spell_id,
                        cast_count: ev.cast_count,
                        seq: ev.seq,
                        slot: precast_missile_slot(&visuals.0, &s),
                    });
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
                let kit = resolve_kit(
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.precast,
                    || weapon_src.caster(ev.entity),
                );
                clear_precast_kit(kit_clears, ev.entity, kit.as_ref());
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
            CastEventKind::Impact {
                on_caster,
                at,
                caster,
            } => {
                // Every living target flinches; the kit comes off the spell's own row (`0x703586`).
                play_impact(
                    ev.entity,
                    ImpactPlay {
                        spell_id: ev.spell_id,
                        missile: Some(MissileHit::on(
                            caster,
                            units.get(ev.entity).ok().map(|(_, s, a, _)| (s, a)),
                            ev.spell_id,
                        )),
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
                if let Some(kit) = area {
                    play_area_kit_at(&kit, pos, &visuals.0, out.sounds, out.shakes, bursts);
                }
            }
            CastEventKind::Fail {
                interrupt,
                wire_reason,
            } => {
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
                // An interrupt launches the caster's missiles and drops its held model (`0x72ed80`).
                if interrupt {
                    releases.write(MissileRelease {
                        caster: ev.entity,
                        seq: ev.seq,
                        kind: ReleaseKind::Fire,
                    });
                }
                if end_cast_clears_hold(spells, ev.spell_id, ev.cast_count, wire_reason) {
                    releases.write(MissileRelease {
                        caster: ev.entity,
                        seq: ev.seq,
                        kind: ReleaseKind::Clear(ev.cast_count),
                    });
                }
                let kit = resolve_kit(
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.precast,
                    || weapon_src.caster(ev.entity),
                );
                clear_precast_kit(kit_clears, ev.entity, kit.as_ref());
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
                play_area_kit_at(kit, dest, &visuals.0, out.sounds, out.shakes, bursts);
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
                        missile: None,
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
            // The release gate (inverted).
            let awaits_release = stages
                .filter(|s| s.cast != 0)
                .and_then(|s| visuals.0.kit(s.cast))
                .is_some_and(|k| k.anim_id.is_some());
            let spawns = !targets.is_empty() || ground_aim.is_some();
            if spawns {
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
                    seq: go.seq,
                });
            }
            // A missile row with no cast animation releases the caster's missiles at GO, a held
            // model with them even when this GO made none (`0x80f5c1`).
            if !awaits_release && stages.is_some_and(|s| s.missile_gate != 0) {
                releases.write(MissileRelease {
                    caster: go.caster,
                    seq: go.seq,
                    kind: ReleaseKind::Fire,
                });
            }
        }
    }

    // The channel poll: `UNIT_CHANNEL_SPELL` edges drive an observed unit's channel (the self-only
    // `MSG_CHANNEL_*` do not); a cleared field drops the hold even with no other wire trace.
    for (entity, store, ..) in &units {
        let cur = store.0.unit_channel_spell();
        let prev = channel_cache.get(&entity).copied().unwrap_or(0);
        if cur == prev {
            continue;
        }
        channel_cache.insert(entity, cur);
        // The old spell's effects are cleared and its aura slots unarmed, either way (`0x73eb50`).
        if prev != 0 {
            channel_left.write(ChannelSpellLeft {
                entity,
                spell_id: prev,
            });
        }
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
                if let Some(anim_id) = kit.anim_id {
                    // Fallible: a unit mid-stream-out is un-indexed but still in this query.
                    commands.entity(entity).try_insert(CastHold {
                        anim_id,
                        spell_id: cur,
                        ranged: false, // no basic shot channels
                        start_anim: None,
                        seq: 0,
                    });
                    pending.insert(entity, Some(cur));
                }
                if let Some(kit_sound) = kit.sound {
                    out.sounds.write(SpellKitSound::Play { entity, kit_sound });
                }
                // The CharProc dispatcher's second caller: Drain Life's beam is reached only here,
                // on the rising edge, and lives until the field clears; every chain slot runs.
                for proc in kit.char_procs().filter_map(|p| p.as_chain()) {
                    out.chain.write(ChainProcPlay {
                        entity,
                        spell_id: cur,
                        proc,
                        kit_id: kit.id,
                        from: None,
                    });
                }
                // The trail arm from the same caller: kit 370 (Whirlwind Primer) is reached here.
                if let Some(trail) = kit.trail_proc() {
                    out.trails
                        .write(crate::weapon_trail::TrailArm { entity, trail });
                }
                play_body_procs(out.body, entity, cur, &kit);
                stow_for_weapon_effects(out.sheaths, spells, entity, cur, &kit);
                // Persistent while the field holds, with the stage-2 lifecycle.
                let effects = resolve_kit_effects(&visuals.0, &kit);
                if !effects.is_empty() {
                    out.fx.write(SpellKitFx::Begin {
                        entity,
                        spell_id: cur,
                        persistent: true,
                        class: FxClass::Hold,
                        stage: state_stage(&kit),
                        effects,
                    });
                }
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

/// One aura slot as the applied core keeps it (`unit+0xedc`): the slot's spell, and whether a play
/// stored it there, which only a visible add does and a channel clear undoes (`0x73eb50`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArmedSlot {
    slot: u8,
    spell_id: u32,
    /// An effect bit was set, the reader's add and removal test (`0x72f8e1`).
    active: bool,
    armed: bool,
}

/// What [`arm_aura_state_fx`] keeps between frames.
#[derive(Default)]
pub(crate) struct AuraArmState {
    /// Every streamed unit's live slots in slot order.
    slots: EntityHashMap<Vec<ArmedSlot>>,
    /// Our own `PLAYER_FIELD_BYTES2` byte 3 as last seen.
    vision: u8,
    /// The spell whose state kit waits for the unit's model (`unit+0xa8c`), one per unit.
    deferred: EntityHashMap<u32>,
}

/// The lanes a clear and a play fan out to beyond the model, proc and sound writers, and the
/// display swaps and model loads the applied core answers.
type AuraArmLanes<'w, 's> = (
    MessageWriter<'w, ChainProcPlay>,
    MessageWriter<'w, SpellEffectClear>,
    MessageReader<'w, 's, ChannelSpellLeft>,
    MessageReader<'w, 's, crate::entities::DisplaySwapped>,
    Query<'w, 's, (), With<crate::entities::VisualAttached>>,
    MessageWriter<'w, SpellKitShake>,
    MessageWriter<'w, crate::weapon_trail::TrailArm>,
);

/// One state-kit play the applied core admits: the unit, the spell and the play's guid.
struct StatePlay {
    entity: Entity,
    spell_id: u32,
    from: Option<Entity>,
}

/// The spell-keyed clear (`0x743b40` with force 1): every model class, the proc nodes and fades
/// (`0x71ab80`, `0x7178e0`), the state kit's loop and the persistent beams.
fn clear_spell_effects(
    fx: &mut MessageWriter<SpellKitFx>,
    procs: &mut MessageWriter<AuraProc>,
    sounds: &mut MessageWriter<SpellKitSound>,
    clears: &mut MessageWriter<SpellEffectClear>,
    state_kit: Option<&VisualKit>,
    entity: Entity,
    spell_id: u32,
) {
    for class in [FxClass::AuraState, FxClass::Hold] {
        fx.write(SpellKitFx::Reap {
            entity,
            spell_id,
            class,
        });
    }
    procs.write(AuraProc::Reap { entity, spell_id });
    if let Some(kit_sound) = state_kit.and_then(|k| k.sound) {
        sounds.write(SpellKitSound::StopKit { entity, kit_sound });
    }
    clears.write(SpellEffectClear { entity, spell_id });
}

/// Arm and reap each aura slot's state kit (stage 2) off the unit's [`UnitAuras`], per slot as
/// 12340 does: a slot taking a spell plays its kit, also on a unit streamed in mid-aura (its first
/// `SMSG_AURA_UPDATE_ALL`), and a slot losing it clears the spell. 12340's applied core
/// (`0x724820`) stores the slot's spell whatever its visual, skips a slot already holding it, and
/// plays the spell's `SpellVisual` state kit (column 4, `+0x10`) at stage 2 through the unit's kit
/// player (`0x745230`) with that slot's creator as the play's guid (`0x724c77`). Its removal core
/// (`0x71e930`) runs only for a stored slot: it clears every effect of the spell on the unit
/// (`0x743b40(spellId, force=1)`, `0x71ebda`), so losing one of two slots holding a spell takes
/// what the other drew, then plays the state-done kit ([`StateDoneKit`]). The aura reader removes
/// before it adds (`0x72f5d0`). The add edge rings the kit sound and never plays the kit's anim:
/// stage 2 compares it with what the unit plays, a mismatch spent on a [`BaseAnimRecompute`].
/// `CharProc` columns go out as [`AuraProc`] edges and beams, and a kit arms for any leg (Stealth's
/// kit 312 is one `CharProc`).
///
/// A unit's channel leaving a spell ([`ChannelSpellLeft`]) clears that spell's effects and unstores
/// its slots (`0x73eb50`), so their later removal plays nothing and they replay only on a new add
/// or a display change, which re-adds every slot from the last (`0x728e70`, `0x73e692`).
///
/// A kit with an anim-rate `CharProc` (11) on a unit whose model is not loaded stores the spell
/// instead of playing (`0x724bf0`, `0x724c63`); the model's load plays it with no guid if the unit
/// still holds the spell (`0x73ea6d`, `0x73eacf`).
///
/// A detection aura's kit shows only to a viewer who shares its vision: the applied core skips the
/// kit when the spell's [`aura_vision_mask`](benilla_formats::SpellDisplay::aura_vision_mask) has
/// no bit in our own `PLAYER_FIELD_BYTES2` byte 3 (`0x724979`), and a change of that byte adds or
/// removes every slot whose mask holds a changed bit (`0x727a70`), so a guard's Stealth Detection
/// eye is hidden from anyone who cannot see stealth.
#[allow(clippy::too_many_arguments)]
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
    lanes: AuraArmLanes,
    index: Res<crate::net::GuidIndex>,
    pending: Option<Res<crate::entities::PendingMissiles>>,
    mut state: Local<AuraArmState>,
) {
    let (mut chain, mut clears, mut channel_left, mut swaps, loaded, mut shakes, mut trails) =
        lanes;
    // No active player sees nothing gated (`0x72496a` returns on a null player).
    let vision = self_store
        .iter()
        .next()
        .and_then(|s| s.0.player_aura_vision())
        .unwrap_or(0);
    let vision_changed = std::mem::replace(&mut state.vision, vision) ^ vision;
    let full_sweep = vision_changed != 0
        || visuals.as_ref().is_some_and(|v| v.is_changed())
        || spells.as_ref().is_some_and(|s| s.is_changed());
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        channel_left.clear();
        swaps.clear();
        return; // no client data
    };
    let state_kit = |spell_id: u32| resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None);
    let shown = |spell_id: u32| {
        let mask = spells
            .catalog
            .get(spell_id)
            .map_or(0, |d| d.aura_vision_mask());
        (mask, mask == 0 || vision & mask != 0)
    };
    // The slot's own creator is the play's guid (`0x724c77`); an unstreamed one names no unit.
    let creator = |guid: u64| {
        Some(guid)
            .filter(|&g| g != 0)
            .map(|g| index.0.get(&g).copied().unwrap_or(Entity::PLACEHOLDER))
    };
    let mut plays: Vec<StatePlay> = Vec::new();
    // The applied core's play leg for a stored slot: the missile gate, then the model gate.
    let mut admit = |deferred: &mut EntityHashMap<u32>, entity: Entity, spell_id: u32, from| {
        // A missile row's kit waits for the missile still queued on this unit (`0x724b92`).
        let queued = || pending.as_ref().is_some_and(|p| p.queues(entity, spell_id));
        if resolve_stages(spells, &visuals.0, spell_id, || None)
            .is_some_and(|s| s.missile_gate != 0)
            && queued()
        {
            return;
        }
        let Some(kit) = state_kit(spell_id) else {
            return; // no state kit
        };
        let anim_rate = kit
            .char_procs()
            .any(|p| p.ty == benilla_formats::char_proc_type::ANIM_RATE);
        if anim_rate && !loaded.contains(entity) {
            deferred.insert(entity, spell_id);
            return;
        }
        plays.push(StatePlay {
            entity,
            spell_id,
            from,
        });
    };
    // The router's channel poll ran first this frame.
    for left in channel_left.read() {
        let kit = state_kit(left.spell_id);
        clear_spell_effects(
            &mut fx,
            &mut procs,
            &mut sounds,
            &mut clears,
            kit.as_ref(),
            left.entity,
            left.spell_id,
        );
        for s in state.slots.get_mut(&left.entity).into_iter().flatten() {
            if s.spell_id == left.spell_id {
                s.armed = false;
            }
        }
    }
    let scan = if full_sweep {
        units.iter().collect::<Vec<_>>()
    } else {
        changed.iter().filter_map(|e| units.get(e).ok()).collect()
    };
    for (entity, auras) in scan {
        let prev = state.slots.remove(&entity).unwrap_or_default();
        // The removal core for every stored slot whose spell left, before any add (`0x72f5d0`):
        // an emptied slot always, a replaced one only if it was active.
        let mut removed: Vec<u32> = prev
            .iter()
            .filter(|p| {
                let now = auras.iter().find(|a| a.slot == p.slot);
                p.armed && now.is_none_or(|a| a.spell_id != p.spell_id && p.active)
            })
            .map(|p| p.spell_id)
            .collect();
        let mut added = Vec::new();
        let mut next = Vec::with_capacity(prev.len());
        for aura in auras.iter() {
            let (mask, shown) = shown(aura.spell_id);
            let active = aura.flags & AURA_EFFECT_BITS != 0;
            let old = prev
                .iter()
                .find(|p| p.slot == aura.slot && p.spell_id == aura.spell_id);
            let was = old.is_some_and(|p| p.armed);
            let armed = match old {
                // A vision change on its mask adds or removes it, active or not (`0x727a70`).
                _ if mask & vision_changed != 0 => shown,
                // The reader adds a slot turning active and removes one losing it (`0x72f8e1`).
                Some(p) if p.active == active => p.armed,
                Some(p) => active && (p.armed || shown),
                None => active && shown,
            };
            next.push(ArmedSlot {
                slot: aura.slot,
                spell_id: aura.spell_id,
                active,
                armed,
            });
            match (was, armed) {
                (true, false) => removed.push(aura.spell_id),
                // A stored slot keeps its spell's kits (`0x7248a8`).
                (false, true) => added.push(aura),
                _ => {}
            }
        }
        state.slots.insert(entity, next);
        for spell_id in removed {
            debug!("anim: aura state kit reaped ({entity}, spell {spell_id})");
            let kit = state_kit(spell_id);
            clear_spell_effects(
                &mut fx,
                &mut procs,
                &mut sounds,
                &mut clears,
                kit.as_ref(),
                entity,
                spell_id,
            );
            // Then the state-done kit (`0x71eb11`).
            done.write(StateDoneKit { entity, spell_id });
        }
        for aura in added {
            admit(
                &mut state.deferred,
                entity,
                aura.spell_id,
                creator(aura.caster),
            );
        }
    }
    // A display change re-adds every slot from the last, active or not; a stored one is skipped
    // and a hidden one stays unstored (`0x728e70`).
    for swap in swaps.read() {
        let AuraArmState {
            slots, deferred, ..
        } = &mut *state;
        let (Ok((entity, auras)), Some(slots)) =
            (units.get(swap.entity), slots.get_mut(&swap.entity))
        else {
            continue;
        };
        for s in slots.iter_mut().rev().filter(|s| !s.armed) {
            if !shown(s.spell_id).1 {
                continue;
            }
            s.armed = true;
            let from = auras
                .iter()
                .find(|a| a.slot == s.slot)
                .and_then(|a| creator(a.caster));
            admit(deferred, entity, s.spell_id, from);
        }
    }
    // The model's load plays the stored spell's kit with no guid, if the unit still holds it.
    if !state.deferred.is_empty() {
        state.deferred.retain(|&entity, &mut spell_id| {
            let Ok((_, auras)) = units.get(entity) else {
                return false; // despawned
            };
            if !loaded.contains(entity) {
                return true;
            }
            if auras.iter().any(|a| a.spell_id == spell_id) {
                plays.push(StatePlay {
                    entity,
                    spell_id,
                    from: None,
                });
            }
            false
        });
    }
    for StatePlay {
        entity,
        spell_id,
        from,
    } in plays
    {
        let Some(kit) = state_kit(spell_id) else {
            continue;
        };
        let effects = resolve_kit_effects(&visuals.0, &kit);
        for proc in kit.char_procs().filter_map(|p| p.as_chain()) {
            chain.write(ChainProcPlay {
                entity,
                spell_id,
                proc,
                kit_id: kit.id,
                from,
            });
        }
        debug!(
            "anim: aura state kit armed ({entity}, spell {spell_id}): {} model(s)",
            effects.len()
        );
        stow_for_weapon_effects(&mut sheaths, spells, entity, spell_id, &kit);
        play_body_procs(&mut body, entity, spell_id, &kit);
        if let Some(trail) = kit.trail_proc() {
            trails.write(crate::weapon_trail::TrailArm { entity, trail });
        }
        // The camera shake plays at every stage, gated on nothing.
        if let Some(group) = kit.shake {
            shakes.write(SpellKitShake::Play { entity, group });
        }
        if !effects.is_empty() {
            fx.write(SpellKitFx::Begin {
                entity,
                spell_id,
                persistent: true,
                class: FxClass::AuraState,
                stage: state_stage(&kit),
                effects,
            });
        }
        play_proc_nodes(&mut procs, entity, spell_id, &kit);
        // Ungated; a same-frame duplicate of the impact hand-off's collapses downstream.
        if let Some(kit_sound) = kit.sound {
            sounds.write(SpellKitSound::Play { entity, kit_sound });
        }
        // The stage-2 anim leg, on the add edge only: an aura refreshed in place keeps its
        // spell, and the slot already storing it is skipped (`0x7248a8`).
        if let Some(anim_id) = kit.anim_id {
            recomputes.write(BaseAnimRecompute { entity, anim_id });
        }
    }
    // Streamed units despawn on range-out; drop their rows.
    state.slots.retain(|e, _| units.contains(*e));
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
