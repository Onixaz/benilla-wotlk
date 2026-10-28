//! Cast target resolution: what goes in `CMSG_CAST_SPELL`'s target block (reference: `ArmCast
//! 0x6e5250`, `BindTarget 0x6e5b40`). The flag word is seeded from `Spell.dbc` `Targets` and
//! adjusted by the implicit-target switch (jump table `0x6e5484`), then:
//!
//! - word 0: commit at once with mask 0 and no guid, never the selection; the server fills the
//!   target from the spell's implicit targeting.
//! - `Attributes & 0x200`: the candidate is the equipped main hand (`0x6e5361`), bound as an item
//!   with no cursor; an empty hand refuses with "Your weapon hand is empty" (`0x6e504b`).
//! - otherwise the selection meets `BindTarget`'s gates ([`bind_gates`]), then each bit is cleared
//!   by its relation check; then the player behind `autoSelfCast` (`0x6e53d7`) does the same. Only a
//!   fully cleared word commits, as a unit guid.
//! - a word with item, lock, GameObject or location bits enters the targeting cursor carrying
//!   the whole word: its location (`0x6e6320`, `& 0x60`), item (`0x6e6330`, `& 0x4010`),
//!   GameObject (`0x6e62d0`, `& 0x4800`), glyph socket (`0x7fdd60`, `& 0x20000`) and unit
//!   (`0x6e6460`) predicates can hold at once, and the click picks the leg.
//!
//! The unit hand cursor receives a residual unit word without the enemy bit. A word carrying
//! `0x80` instead raises the local no-target/invalid-target refusal; STRING bit 13 remains
//! unmodeled and refuses locally with "Invalid target".

use benilla_formats::SpellDisplay;
use benilla_ui::script::CastAt;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::bind_gates::{bind_gates, unit_alive};
use super::group_relation::{GroupInputs, GroupRoster};
use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::target::{can_assist, can_attack, Factions, Selection};
use crate::unit_auras::UnitAuras;
use benilla_protocol::messages::OwnerFallback;

/// `TARGET_FLAG_*` bits of the client's targeting flag word (`0xcecac0`).
const TF_UNIT: u32 = 0x0002;
const TF_UNIT_RAID: u32 = 0x0004;
const TF_UNIT_PARTY: u32 = 0x0008;
const TF_UNIT_ENEMY: u32 = 0x0080;
const TF_UNIT_ASSIST: u32 = 0x0100;
const TF_CORPSE_ENEMY: u32 = 0x0200;
pub(super) const TF_EXPLICIT_GATE: u32 = 0x0400;
const TF_CORPSE_ALLY: u32 = 0x8000;
/// The corpse words, `0x8600`: `BindTarget`'s dead-unit gate lets a dead unit through for any of
/// them (`6e5c7a`).
pub(super) const CORPSE_WORD_BITS: u32 = TF_CORPSE_ENEMY | TF_EXPLICIT_GATE | TF_CORPSE_ALLY;
/// The unit-shaped bits a selected unit (alive or dead) can satisfy.
const UNIT_BITS: u32 = TF_UNIT
    | TF_UNIT_RAID
    | TF_UNIT_PARTY
    | TF_UNIT_ENEMY
    | TF_UNIT_ASSIST
    | TF_CORPSE_ENEMY
    | TF_EXPLICIT_GATE
    | TF_CORPSE_ALLY;

/// Client cast-failed reasons used when an enemy word cannot bind.
pub(crate) const ERR_NO_TARGET: u8 = 0x09;
pub(crate) const ERR_INVALID_TARGET: u8 = 0x0A;

/// `SPELL_FAILED_MAINHAND_EMPTY`, "Your weapon hand is empty": the reference's own refusal for an
/// imbue with an empty main hand (`0x6e5050`).
pub(crate) const ERR_MAINHAND_EMPTY: u8 = 0x2d;

/// The dest-location bit, the ground-cast wire mask (`BindLocation 0x6e60f0`).
const TF_DEST_LOCATION: u32 = 0x0040;
/// The source-location bit, `BindLocation 0x6e60f0`'s other arm (`0x6e6105`).
const TF_SOURCE_LOCATION: u32 = 0x0020;
/// `TargetingWantsLocation 0x6e6320`'s mask: the terrain click serves both location bits.
const LOCATION_BITS: u32 = TF_SOURCE_LOCATION | TF_DEST_LOCATION;
/// The item bit, asked by the bag and paper-doll click seams (`TargetingWantsItem 0x6e6330`).
const TF_ITEM: u32 = 0x0010;
/// `TARGET_FLAG_LOCKED`: in both the item mask `0x4010` and the GameObject mask `0x4800`, so one
/// lock spell ends on a bag item or a world object. It never reaches the wire: `BindTarget`
/// writes ITEM (`0x6e5f2e`) or GAMEOBJECT (`0x6e5f69`) instead, which vmangos accepts for a
/// lock spell (`Spell.cpp:6755`).
const TF_LOCKED: u32 = 0x4000;
/// `TARGET_FLAG_GAMEOBJECT`, the other bit of `TargetingWantsGameObject 0x6e62d0`'s `0x4800`.
const TF_GAMEOBJECT: u32 = 0x0800;
/// `TARGET_FLAG_GLYPH_SLOT`, the glyph frame's seam (`SpellCanTargetGlyph 0x7fdd60`): every
/// `Targets` bit above `0xffff` on 12340 is this one, alone.
pub(crate) const TF_GLYPH_SLOT: u32 = 0x2_0000;
/// The bits `BindTarget`'s item arm admits and clears (`0x6e5f1e`, `0x6e5f36`).
const ITEM_ARM_BITS: u32 = TF_ITEM | TF_LOCKED;

/// What the wire's target block should carry for this cast.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CastWireTarget {
    /// Word 0: mask 0, no guid; the server resolves the target implicitly.
    SelfImplicit,
    /// Mask `TARGET_FLAG_UNIT` (0x2) and this guid, possibly the player's own.
    Unit(u64),
    /// Mask `TARGET_FLAG_ITEM` (0x10) and this item guid, sent on the press: only the main-hand
    /// imbue binds an item from the cast itself; every other item target comes from a click.
    Item(u64),
    /// No send yet: enter the targeting cursor with this flag word (reference: word != 0 is
    /// `IsTargeting 0x6e48a0`). Each click seam tests the word with its own mask, so one word can
    /// serve several: Opening and Pick Lock take a bag item or a world GameObject.
    Targeting(u32),
    /// Do not send; show this client error.
    Refused(u8),
    /// A refusal the reference raises in its cast tail after `ArmCast` returns false (`0x6e5045`).
    /// That tail runs after the requirement validator `0x6094f0`, so it fires at the cursor-entry
    /// point in [`super::send_spell_cast`], not at the resolver's return.
    RefusedAtArm(u8),
}

/// Where `ArmCast 0x6e5250` takes its candidate guid from (`0x6e5361`-`0x6e539f`). The main hand
/// is taken instead of the selection, never before it; `caster` is the separate `autoSelfCast`
/// fallback (`0x6e53d7`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CastCandidates {
    /// The current selection (the client's `0xb4e2d8`); benilla never passes an explicit guid.
    pub(crate) selection: Option<u64>,
    pub(crate) caster: Option<u64>,
    /// The equipped main hand's item guid (`[player+0x1d3c][15]`), only when it resolves to a
    /// streamed item object (`0x468460` at `0x6e53bc`).
    pub(crate) main_hand_item: Option<u64>,
}

/// What the binder's gates and relation checks read: the selected unit's store and the player's.
#[derive(Clone, Copy)]
pub(crate) struct TargetRelations<'a> {
    pub(crate) target_store: Option<&'a ObjectStore>,
    /// The target's charmer, creator or summoner store, for the group predicates' controller.
    pub(crate) target_owner_store: Option<&'a ObjectStore>,
    pub(crate) self_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
    /// The creature-type gate's inputs, for the candidate and for the player alike.
    pub(crate) types: CreatureTypeSources<'a>,
    /// The party and raid predicates' inputs (`0x606c20`, `0x606d20`).
    pub(crate) group: GroupInputs<'a>,
}

/// A unit's owner as the controller resolve `0x606170` reads it, its charmer
/// else its creator: that object's descriptor, if it has streamed (`0x468460`).
pub(super) fn owner_store<'a>(
    store: Option<&ObjectStore>,
    index: Option<&GuidIndex>,
    stores: &'a Query<&ObjectStore>,
) -> Option<&'a ObjectStore> {
    let owner = store?.0.unit_owner(OwnerFallback::CreatedBy)?;
    stores.get(*index?.0.get(&owner)?).ok()
}

impl TargetRelations<'_> {
    /// `CanAssist 0x7293d0` toward the candidate, which the caster always is for himself. The
    /// binder's assist arm and the world pick's relation leg both ask it.
    pub(super) fn assistable(&self, is_self: bool) -> bool {
        is_self
            || can_assist(
                self.target_store,
                self.factions,
                self.reputations,
                self.self_store,
                self.types.names,
            )
    }

    /// `CanAttack 0x606980` toward the candidate, never the caster. The binder's enemy arm and the
    /// world pick's relation leg both ask it.
    pub(super) fn attackable(&self, is_self: bool) -> bool {
        !is_self
            && can_attack(
                self.target_store,
                self.factions,
                self.reputations,
                self.self_store,
            )
    }
}

/// The targeting inputs [`super::send_spell_cast`] resolves with, built by both cast callers.
pub(crate) struct CastContext<'a> {
    pub(crate) selection_guid: Option<u64>,
    pub(crate) self_guid: Option<u64>,
    pub(crate) auto_self_cast: bool,
    pub(crate) rel: TargetRelations<'a>,
    /// The local range gate's inputs (`IsTargetInRange 0x6e47b0`).
    pub(crate) range: RangeInputs,
    /// The main hand's item guid off the player descriptor (`PLAYER_FIELD_INV_SLOT_HEAD + 2*15`);
    /// the send path checks that it names a streamed item.
    pub(crate) main_hand_item: Option<u64>,
    /// The caster's movement flags (the client's `[unit+0x9e8]`), read by the requirement
    /// validator's moving gate.
    pub(crate) self_move_flags: u32,
    /// `CanAutoDismount 0x71b0c0`'s verdict: a mounted cast dismounts instead of refusing.
    pub(crate) auto_dismount: bool,
    /// The caster's aura table: the toggle-cancel scans and the crowd-control exemption read it.
    pub(crate) self_auras: Option<&'a UnitAuras>,
    /// The caster's combo points on the cast's target, the finisher gate's read
    /// (`GetComboPoints(target)`, the player's at `0x80d518`, the pet's at `0x80d54e`).
    pub(crate) combo_points: u8,
}

/// Positions and the two units for the pre-send range refusal: the client's cast runs
/// `CanTargetUnit 0x6e4440` and `IsTargetInRange 0x6e47b0` before the commit, so an out-of-range
/// press refuses locally and none of the commit tail runs.
#[derive(Clone, Copy)]
pub(crate) struct RangeInputs {
    pub(crate) self_pos: Option<Vec3>,
    pub(crate) target_pos: Option<Vec3>,
    /// The caster's combat reach and motion.
    pub(crate) caster: benilla_formats::RangeUnit,
    /// The bound unit, which `GetMinMaxRange` reads for the ranged pad and the moving bonus.
    pub(crate) target: Option<benilla_formats::RangeUnit>,
}

impl RangeInputs {
    /// The one range compare: the squared 3D distance against `GetMinMaxRange 0x6e3480`'s bounds,
    /// "Out of range." (0x59) past max², "Target too close" (0x76) inside a nonzero min². The
    /// pre-send gate (`IsTargetInRange 0x6e47b0`) and the targeting cursor's unit leg (`0x6e6460`,
    /// `6e677c`–`6e6802`) both ask it. No row or no position passes; the server judges.
    pub(crate) fn refusal(
        &self,
        def: &SpellDisplay,
        row: Option<&benilla_formats::SpellRange>,
        mods: &super::SpellModifiers,
    ) -> Option<u8> {
        let dist_sq = self
            .self_pos
            .zip(self.target_pos)
            .map(|(a, b)| a.distance_squared(b));
        super::validator::cast_range_refusal(def, row, self.caster, self.target, dist_sq, mods)
    }
}

impl Default for RangeInputs {
    fn default() -> Self {
        Self {
            self_pos: None,
            target_pos: None,
            caster: benilla_formats::RangeUnit::still(super::DEFAULT_REACH),
            target: None,
        }
    }
}

/// Everything a cast-sending system needs to build a [`CastContext`].
#[derive(SystemParam)]
pub(crate) struct CastTargeting<'w, 's> {
    pub(crate) selection: Res<'w, Selection>,
    pub(crate) self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    pub(crate) self_auras: Query<'w, 's, &'static UnitAuras, With<SelfPlayer>>,
    /// Any caster's auras, for a cast as the pet.
    unit_auras: Query<'w, 's, &'static UnitAuras>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Option<Res<'w, GuidIndex>>,
    self_guid: Res<'w, SelfGuid>,
    auto_self_cast: Res<'w, AutoSelfCast>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    /// The party slots and raid roster as the frame began; a harness without them has no group.
    roster: Option<Res<'w, GroupRoster>>,
    /// The creature-type gate's template cache and form table; a harness without them resolves
    /// no type.
    names: Option<Res<'w, NameCache>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    self_transform: Query<'w, 's, &'static Transform, With<SelfPlayer>>,
    transforms: Query<'w, 's, &'static Transform>,
    /// `Option`: the body exists only in world, and the cast-result handler must fetch this param
    /// anywhere. With no body the moving gate passes and the server decides.
    player: Option<Res<'w, crate::player::Player>>,
    /// `autoDismount` and `autoDismountFlying`; a harness without them reads the defaults.
    auto_dismount: Option<Res<'w, AutoDismount>>,
    /// The caster's and the selection's reach and motion, for the range gate.
    range_units: super::RangeUnits<'w, 's>,
    /// The player's and the pet's combo banks; a harness without them holds none.
    combo: Option<Res<'w, super::ComboPoints>>,
}

impl CastTargeting<'_, '_> {
    /// This frame's [`CastContext`].
    pub(crate) fn context(&self) -> CastContext<'_> {
        let target_store = self.selection.target.and_then(|e| self.stores.get(e).ok());
        let owner_store = |store| owner_store(store, self.index.as_deref(), &self.stores);
        let self_store = self.self_store.iter().next();
        CastContext {
            selection_guid: self.selection.guid,
            self_guid: self.self_guid.0,
            auto_self_cast: self.auto_self_cast.0,
            rel: TargetRelations {
                target_store,
                target_owner_store: owner_store(target_store),
                self_store,
                factions: self.factions.as_deref(),
                reputations: &self.reputations,
                types: CreatureTypeSources {
                    names: self.names.as_deref(),
                    forms: self.spells.as_deref().map(|s| &s.forms),
                },
                group: GroupInputs {
                    self_guid: self.self_guid.0,
                    target_guid: self.selection.guid,
                    self_owner_store: owner_store(self_store),
                    roster: self.roster.as_deref(),
                },
            },
            range: RangeInputs {
                self_pos: self.self_transform.iter().next().map(|t| t.translation),
                target_pos: self
                    .selection
                    .target
                    .and_then(|e| self.transforms.get(e).ok())
                    .map(|t| t.translation),
                caster: self.range_units.caster(),
                target: self.selection.target.and_then(|e| self.range_units.unit(e)),
            },
            main_hand_item: self
                .self_store
                .iter()
                .next()
                .and_then(|s| s.0.player_inv_slot(EQUIPMENT_SLOT_MAINHAND))
                .filter(|&g| g != 0),
            self_move_flags: self.player.as_deref().map_or(0, |p| p.move_flags()),
            auto_dismount: {
                let cvars = self.auto_dismount.as_deref().copied().unwrap_or_default();
                super::validator::can_auto_dismount(
                    self_store.map_or(0, |s| s.0.unit_flags()),
                    self.player.as_deref().map_or(0, |p| p.move_flags()),
                    cvars.ground,
                    cvars.flying,
                )
            },
            self_auras: self.self_auras.iter().next(),
            combo_points: self.combo_on(false, 0),
        }
    }

    /// `GetComboPoints(target)` on the player's bank or the pet's (`0x513920`, `0x5d3520`), a 0
    /// target reading the selection.
    fn combo_on(&self, pet: bool, target: u64) -> u8 {
        self.combo.as_deref().map_or(0, |c| {
            let bank = if pet { c.pet } else { c.player };
            bank.on(target, self.selection.guid.unwrap_or(0))
        })
    }

    /// This frame's [`CastContext`] aimed where a cast verb's target argument points: the guid
    /// pair `UseAction`, `CastSpell`, `CastSpellByName` and `CastSpellByID` hand the cast entry
    /// `0x80da40` replaces the selection as the bind candidate, with its store, position and reach.
    /// A token naming nobody leaves no candidate, as the sentinel guid `0x60b00c` writes finds no
    /// object; the `autoSelfCast` fallback still applies after it.
    pub(crate) fn context_at(&self, aim: Aim) -> CastContext<'_> {
        let mut ctx = self.context();
        let Aim::Unit(guid) = aim else {
            return ctx;
        };
        let entity = guid.and_then(|g| self.index.as_deref()?.0.get(&g).copied());
        let store = entity.and_then(|e| self.stores.get(e).ok());
        ctx.selection_guid = guid;
        ctx.rel.target_store = store;
        ctx.rel.target_owner_store = owner_store(store, self.index.as_deref(), &self.stores);
        ctx.rel.group.target_guid = guid;
        // A token naming nobody hands the cast a guid no unit banks points on.
        ctx.combo_points = guid.map_or(0, |g| self.combo_on(false, g));
        ctx.range.target_pos = entity
            .and_then(|e| self.transforms.get(e).ok())
            .map(|t| t.translation);
        ctx.range.target = entity.and_then(|e| self.range_units.unit(e));
        ctx
    }

    /// The [`CastContext`] of a cast with the pet as caster (`Spell_C_CastSpell` `0x80cce0` called
    /// with the pet), aimed at `target` or else the selection: the caster's store, position, reach
    /// and auras are the pet's, and it has no main hand and no own movement flags. With
    /// `owner_casts` (`AttributesEx6 & 0x40000`, `0x80cd3e`) the owner, the player, is the unit
    /// the ladder reads.
    pub(crate) fn pet_context(
        &self,
        pet_guid: u64,
        target: Option<u64>,
        owner_casts: bool,
    ) -> CastContext<'_> {
        let mut ctx = match target {
            Some(guid) => self.context_at(Aim::Unit(Some(guid))),
            None => self.context(),
        };
        if owner_casts {
            return ctx;
        }
        let entity = self
            .index
            .as_deref()
            .and_then(|i| i.0.get(&pet_guid).copied());
        let store = entity.and_then(|e| self.stores.get(e).ok());
        ctx.self_guid = Some(pet_guid);
        ctx.rel.self_store = store;
        ctx.rel.group.self_guid = Some(pet_guid);
        ctx.rel.group.self_owner_store = owner_store(store, self.index.as_deref(), &self.stores);
        ctx.range.self_pos = entity
            .and_then(|e| self.transforms.get(e).ok())
            .map(|t| t.translation);
        if let Some(unit) = entity.and_then(|e| self.range_units.unit(e)) {
            ctx.range.caster = unit;
        }
        ctx.main_hand_item = None;
        ctx.self_move_flags = 0;
        ctx.self_auras = entity.and_then(|e| self.unit_auras.get(e).ok());
        ctx.combo_points = self.combo_on(true, target.unwrap_or(0));
        ctx
    }

    /// Resolve a cast verb's target where its call takes effect, after the calls before it
    /// committed: the selection, the active player, or the unit a token names (`None`, nobody).
    pub(crate) fn aim(&self, tokens: &crate::ui_unit::UnitTokens, at: &CastAt) -> Aim {
        match at {
            CastAt::Selection => Aim::Selection,
            CastAt::Player => Aim::Unit(self.self_guid.0),
            CastAt::Unit(token) => Aim::Unit(tokens.resolve(token, &self.selection).map(|u| u.1)),
        }
    }
}

/// Where a cast is aimed once its target argument is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Aim {
    /// The selection as it stands.
    Selection,
    /// An explicit guid, `None` for a token naming nobody.
    Unit(Option<u64>),
}

impl Aim {
    /// TryCast's guid argument, which the attack pick tries before the selection.
    pub(crate) fn passed(self) -> Option<u64> {
        match self {
            Aim::Selection => None,
            Aim::Unit(guid) => guid,
        }
    }
}

/// `EQUIPMENT_SLOT_MAINHAND` (the client reads it at `0x6e538b`, offset `0x78 == 15 * 8`).
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;

/// The `autoSelfCast` CVar, on by default as 12340 registers it (`0x51dbee`, default `"1"`).
#[derive(bevy::prelude::Resource)]
pub(crate) struct AutoSelfCast(pub(crate) bool);

impl Default for AutoSelfCast {
    fn default() -> Self {
        Self(true)
    }
}

/// `autoDismount` and `autoDismountFlying` (`0x51da52`, "1"; `0x51da77`, "0"), the two CVars
/// `CanAutoDismount 0x71b0c0` reads.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub(crate) struct AutoDismount {
    pub(crate) ground: bool,
    pub(crate) flying: bool,
}

impl Default for AutoDismount {
    fn default() -> Self {
        Self {
            ground: true,
            flying: false,
        }
    }
}

/// `autoSelfCast`'s, `autoDismount`'s and `autoDismountFlying`'s change callbacks: flags.
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut auto: ResMut<AutoSelfCast>,
    mut dismount: ResMut<AutoDismount>,
) {
    if ev.is("autoSelfCast") {
        auto.0 = ev.flag();
    } else if ev.is("autoDismount") {
        dismount.ground = ev.flag();
    } else if ev.is("autoDismountFlying") {
        dismount.flying = ev.flag();
    }
}

/// The flag word: `Targets`, then one arm keyed on `EffectImplicitTargetA[0]` (`0x6e525a`-
/// `0x6e52ef`); every implicit target not matched below leaves the word as it is.
pub(crate) fn cast_target_mask(def: &SpellDisplay) -> u32 {
    let mut word = def.targets;
    match def.implicit_target_a1 {
        1 => word &= !TF_EXPLICIT_GATE,
        5 => word &= !TF_CORPSE_ALLY,
        6 | 53 => word |= TF_UNIT_ENEMY,
        // 16 (ground target) sets a cursor-mode flag, not a word bit; [`resolve_cast_target`]
        // tests it.
        21 | 45 => word |= TF_UNIT_ASSIST,
        23 => word |= 0x0800,
        25 | 63 => word |= TF_UNIT,
        26 => word |= 0x4000,
        35 => word |= TF_UNIT_PARTY,
        57 | 61 => word |= TF_UNIT_RAID,
        _ => {}
    }
    word
}

/// `BindTarget 0x6e5b40`'s unit branch past the gates: clear every flag-word bit the candidate
/// satisfies.
///
/// The assist bit asks `CanAssist 0x6066f0`: selectable, friendly or better, and an NPC's owner
/// (or the NPC itself) PvP-enabled. This keeps friendly ambient NPCs and critters unbindable.
/// The enemy bit asks `CanAttack` (`0x606980`). The party bit needs `0x606c20` and then `CanAssist`
/// (`6e5cad`-`6e5cec`): the player, a member of his party, or a unit that answers to one. The raid
/// bit needs `0x606d20` and `CanAssist` (`6e5cf1`-`6e5d2f`): the same, with the raid roster
/// counting as well as the party slots. The corpse check (`0x6067d0`) is assistable with health 0
/// here.
fn clear_satisfied_bits(word: u32, is_self: bool, rel: &TargetRelations) -> u32 {
    let mut word = word;
    let assist = rel.assistable(is_self);
    let dead = rel.target_store.is_some_and(|s| !unit_alive(&s.0));
    if word & TF_UNIT_PARTY != 0 && assist && rel.in_party(is_self) {
        word &= !TF_UNIT_PARTY;
    }
    if word & TF_UNIT_RAID != 0 && assist && rel.in_group(is_self) {
        word &= !TF_UNIT_RAID;
    }
    if word & TF_UNIT_ASSIST != 0 && assist {
        word &= !TF_UNIT_ASSIST;
    }
    if word & TF_UNIT_ENEMY != 0 && rel.attackable(is_self) {
        word &= !TF_UNIT_ENEMY;
    }
    // Bit 1 has no relation check: any unit binds. A dead unit's bind clears bit 10 (`6e5d89`,
    // `6e5ddb`); the gate refuses a living unit under that word before it gets here.
    if word & TF_UNIT != 0 {
        word &= !TF_UNIT;
    }
    if word & TF_EXPLICIT_GATE != 0 && dead {
        word &= !TF_EXPLICIT_GATE;
    }
    if word & TF_CORPSE_ALLY != 0 && assist && dead {
        word &= !TF_CORPSE_ALLY;
    }
    if word & TF_CORPSE_ENEMY != 0 && !is_self && dead {
        word &= !TF_CORPSE_ENEMY;
    }
    word
}

/// Whether the unit arm's relation checks clear the whole standing word for this unit.
fn unit_word_binds(word: u32, is_self: bool, rel: &TargetRelations) -> bool {
    word & UNIT_BITS != 0 && clear_satisfied_bits(word, is_self, rel) == 0
}

/// Whether `BindTarget 0x6e5b40`'s unit branch binds this unit under the standing word: the gates
/// ([`bind_gates`]), then the relation checks. The initial selection, the `autoSelfCast` fallback,
/// a world click, `SpellTargetUnit` and the hover verdict all ask this one predicate. An unknown
/// spell has no row to gate on and leaves the relation checks alone.
pub(super) fn unit_binds(
    def: Option<&SpellDisplay>,
    word: u32,
    is_self: bool,
    rel: &TargetRelations,
) -> bool {
    def.is_none_or(|def| bind_gates(def, word, is_self, rel)) && unit_word_binds(word, is_self, rel)
}

/// The wire target for casting `def`. An unknown spell sends the selection as is, or no target
/// without one, and the server validates.
pub(crate) fn resolve_cast_target(
    def: Option<&SpellDisplay>,
    cand: &CastCandidates,
    auto_self_cast: bool,
    rel: &TargetRelations,
) -> CastWireTarget {
    let Some(def) = def else {
        return match cand.selection {
            Some(guid) => CastWireTarget::Unit(guid),
            None => CastWireTarget::SelfImplicit,
        };
    };
    let word = cast_target_mask(def);
    if word == 0 {
        return CastWireTarget::SelfImplicit;
    }
    // Implicit target 16 enters the cursor before any candidate bind (`0x6e535b`), but after the
    // word-0 commit above (`0x6e5338` comes first).
    if def.implicit_target_a1 == 16 {
        // The reference flags this outside the word; the click seams read only the word, so the
        // DEST bit carries it (shipped arm-16 rows already have it in `Targets`).
        return CastWireTarget::Targeting(word | TF_DEST_LOCATION);
    }
    // `Attributes & 0x200` takes the main hand as the candidate (`0x6e5361`-`0x6e5391`). This
    // must precede the non-unit cursor defer below: an item candidate clears bit 4.
    if def.targets_main_hand_item() {
        match cand.main_hand_item {
            // `BindTarget`'s item arm clears bits 4 and 14 (`0x6e5f35`) and the cleared word
            // commits at once (`0x6e60c1`).
            Some(item) if word & ITEM_ARM_BITS != 0 && word & !ITEM_ARM_BITS == 0 => {
                return CastWireTarget::Item(item)
            }
            // Empty hand: not a cursor. The cast tail re-tests the attribute (`0x6e504b`), clears
            // the word (`0x6e5057`) and raises 0x2d, jumping over the cursor arm (`0x6e50ab`).
            None => return CastWireTarget::RefusedAtArm(ERR_MAINHAND_EMPTY),
            // A residual the item arm cannot clear waits at the cursor in the reference; no
            // shipped row has one (all 22 carry `Targets == 0x10`).
            Some(_) => {}
        }
    }
    // A non-unit bit enters the cursor with the whole word. The reference only reaches the cursor
    // after the unit walk fails (`0x6e50c8`), so deferring first matches it only because no unit
    // candidate clears these bits; a location word with a unit bit beside it (0x42, NPC-cast
    // spell 28218) binds the unit first in the reference, so it refuses here instead.
    if word & !UNIT_BITS != 0 {
        if word & LOCATION_BITS != 0 && word & UNIT_BITS == 0
            || word & (TF_ITEM | TF_LOCKED | TF_GAMEOBJECT | TF_GLYPH_SLOT) != 0
        {
            return CastWireTarget::Targeting(word);
        }
        return CastWireTarget::Refused(ERR_INVALID_TARGET);
    }
    // The selection (`0xb4e2d8`, `0x6e539f`). A unit the gates or the relation checks refuse binds
    // nothing, so the fallback below gets its turn.
    if let Some(guid) = cand.selection {
        let is_self = cand.caster == Some(guid);
        if unit_binds(Some(def), word, is_self, rel) {
            return CastWireTarget::Unit(guid);
        }
    }
    // The fallback: the active player (`0x6e53d7`), behind autoSelfCast, through the same gates.
    if auto_self_cast {
        if let Some(guid) = cand.caster {
            let self_rel = TargetRelations {
                target_store: rel.self_store,
                target_owner_store: None,
                group: GroupInputs {
                    target_guid: Some(guid),
                    ..rel.group
                },
                ..*rel
            };
            if unit_binds(Some(def), word, true, &self_rel) {
                return CastWireTarget::Unit(guid);
            }
        }
    }
    // A standing enemy word refuses locally. Every other residual unit word proceeds to
    // `BindTarget`'s hand cursor.
    if word & TF_UNIT_ENEMY != 0 {
        CastWireTarget::Refused(if cand.selection.is_some() {
            ERR_INVALID_TARGET
        } else {
            ERR_NO_TARGET
        })
    } else {
        CastWireTarget::Targeting(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selection and caster candidates, with an empty main hand.
    fn cands(selection: Option<u64>, caster: Option<u64>) -> CastCandidates {
        CastCandidates {
            selection,
            caster,
            main_hand_item: None,
        }
    }

    fn spell(targets: u32, implicit: u32) -> SpellDisplay {
        SpellDisplay {
            targets,
            implicit_target_a1: implicit,
            ..Default::default()
        }
    }

    #[test]
    fn auto_self_cast_boots_at_the_reference_default() {
        assert!(
            AutoSelfCast::default().0,
            "autoSelfCast defaults to 12340's \"1\" (0x51dbee)"
        );
    }

    #[test]
    fn mask_follows_the_arm_map() {
        assert_eq!(cast_target_mask(&spell(0, 1)), 0, "Ice Armor: self");
        assert_eq!(
            cast_target_mask(&spell(0, 20)),
            0,
            "Battle Shout: no-op arm"
        );
        assert_eq!(cast_target_mask(&spell(0, 6)), TF_UNIT_ENEMY, "Fireball");
        assert_eq!(
            cast_target_mask(&spell(0, 21)),
            TF_UNIT_ASSIST,
            "Arcane Intellect"
        );
        assert_eq!(
            cast_target_mask(&spell(0x8000, 21)),
            TF_CORPSE_ALLY | TF_UNIT_ASSIST,
            "a Targets seed ORs with the switch"
        );
        assert_eq!(
            cast_target_mask(&spell(0x402, 0)),
            TF_UNIT | TF_EXPLICIT_GATE,
            "Skinning: seed only"
        );
    }

    /// Both stores are present because `CanAttack` refuses outright when either side is missing.
    #[test]
    fn resolution_wire_shapes() {
        use benilla_protocol::ObjectFields;
        // `UNIT_FIELD_FLAGS` bit 3 (player-controlled) on us alone selects `CanAttack`'s mixed
        // arm; with no catalog the reaction resolves neutral, so the enemy bit clears. Both carry
        // `UNIT_FIELD_HEALTH` (22): a store with none reads dead to the gates.
        let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(22, 100), (46, 1 << 3)]));
        let it = crate::net::ObjectStore(ObjectFields::from_pairs(&[(22, 100), (35, 0)]));
        let rel = TargetRelations {
            target_store: Some(&it),
            target_owner_store: None,
            self_store: Some(&me),
            factions: None,
            reputations: &Reputations(Vec::new(), Vec::new()),
            types: Default::default(),
            group: Default::default(),
        };
        let ice_armor = spell(0, 1);
        assert_eq!(
            resolve_cast_target(Some(&ice_armor), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::SelfImplicit,
            "a self spell ignores the selection entirely"
        );
        let fireball = spell(0, 6);
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(None, Some(1)), true, &rel),
            CastWireTarget::Refused(ERR_NO_TARGET),
            "enemy words retain the no-target refusal"
        );
        // Neutral (3) is attackable by the mixed arm's `< 4` and not assistable by `>= 4`.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
        let intellect = spell(0, 21);
        assert!(
            !unit_word_binds(TF_UNIT_ASSIST, false, &rel),
            "the cursor's unit binder must reject the same neutral unit as the initial cast arm"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "a friendly-required cast on a non-friend falls back to self"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), false, &rel),
            CastWireTarget::Targeting(TF_UNIT_ASSIST),
            "autoSelfCast off leaves the friendly word for the unit cursor"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(None, Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "no selection at all still self-falls-back"
        );
        // A hostile-required cast never self-binds.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(None, Some(1)), false, &rel),
            CastWireTarget::Refused(ERR_NO_TARGET)
        );
        // Unknown spell: the selection passes through.
        assert_eq!(
            resolve_cast_target(None, &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn assist_word_requires_the_npc_pvp_flag_not_only_a_friendly_reaction() {
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("FactionTemplate.dbc"),
        );
        // Faction templates 35 and 1 are a real friendly pair. Field 46 is UNIT_FIELD_FLAGS.
        let me = ObjectStore(ObjectFields::from_pairs(&[(35, 1), (46, 0x8)]));
        let quiet = ObjectStore(ObjectFields::from_pairs(&[(35, 35), (46, 0)]));
        let pvp = ObjectStore(ObjectFields::from_pairs(&[(35, 35), (46, 0x1000)]));
        let reputations = Reputations(Vec::new(), Vec::new());
        let rel = |target| TargetRelations {
            target_store: Some(target),
            target_owner_store: None,
            self_store: Some(&me),
            factions: Some(&factions),
            reputations: &reputations,
            types: Default::default(),
            group: Default::default(),
        };

        assert!(
            !unit_word_binds(TF_UNIT_ASSIST, false, &rel(&quiet)),
            "a friendly ambient NPC is not assistable"
        );
        assert!(
            unit_word_binds(TF_UNIT_ASSIST, false, &rel(&pvp)),
            "the same friendly NPC becomes assistable with UNIT_FLAG_PVP"
        );
    }

    /// Flamestrike (implicit 16) defers even with a selection, Blizzard (implicit 28) by its
    /// bare DEST word; a word of 0 still commits first (`0x6e5338`).
    #[test]
    fn ground_masks_enter_targeting_mode() {
        let flamestrike = spell(0x40, 16);
        assert_eq!(
            resolve_cast_target(
                Some(&flamestrike),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::Targeting(TF_DEST_LOCATION)
        );
        let blizzard = spell(0x40, 28);
        assert_eq!(
            resolve_cast_target(Some(&blizzard), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(TF_DEST_LOCATION)
        );
        let self_commit_with_ground_arm = spell(0, 16);
        assert_eq!(
            resolve_cast_target(
                Some(&self_commit_with_ground_arm),
                &cands(None, Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::SelfImplicit,
            "word==0 commits before the arm-16 defer — the reference's order"
        );
    }

    /// A bare SOURCE word raises the cursor like DEST (`0x6e6320` tests `& 0x60`); spell 265,
    /// `Targets 0x20` with implicit 15, is the shipped case.
    #[test]
    fn the_source_location_word_enters_targeting_mode() {
        let area_death = spell(0x20, 15);
        assert_eq!(
            resolve_cast_target(
                Some(&area_death),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::Targeting(TF_SOURCE_LOCATION)
        );
        assert_eq!(
            resolve_cast_target(Some(&area_death), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(TF_SOURCE_LOCATION),
            "and with nothing selected — autoSelfCast has no location to give either"
        );
        // Both bits, which `BindLocation` takes over two clicks; no shipped row carries it.
        let both = spell(0x60, 0);
        assert_eq!(
            resolve_cast_target(Some(&both), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(LOCATION_BITS)
        );
    }

    /// STRING (bit 13) has no seam; `0x42` (spell 28218) would bind its unit first in the
    /// reference.
    #[test]
    fn non_unit_masks_refuse() {
        for targets in [0x2000u32, 0x42] {
            let s = spell(targets, 0);
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(Some(42), Some(1)), true, &rel_none()),
                CastWireTarget::Refused(ERR_INVALID_TARGET),
                "Targets {targets:#x} must stay refused"
            );
        }
    }

    /// 100 of the 103 LOCKED rows carry implicit arm 23 (word `0x4800`) and one arm 25
    /// (`0x4002`): the seam tests are masks, so each raises the cursor with its whole word.
    #[test]
    fn the_seam_predicates_are_masks_and_the_word_travels() {
        for (targets, implicit) in [
            (0x10u32, 0),
            (0x4000, 0),
            (0x4000, 23),
            (0x4000, 25),
            (0x0, 23),
        ] {
            let s = spell(targets, implicit);
            let word = cast_target_mask(&s);
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(Some(42), Some(1)), true, &rel_none()),
                CastWireTarget::Targeting(word),
                "Targets {targets:#x} + implicit arm {implicit} must raise the cursor with its word"
            );
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(None, Some(1)), false, &rel_none()),
                CastWireTarget::Targeting(word),
                "no selection and no autoSelfCast change nothing — no unit clears these bits"
            );
        }
        // The shipped lock word carries both seams' bits.
        let opening = cast_target_mask(&spell(0x4000, 23));
        assert_eq!(opening & (TF_ITEM | TF_LOCKED), TF_LOCKED);
        assert_eq!(opening & (TF_GAMEOBJECT | TF_LOCKED), opening);
    }

    /// Rockbiter Weapon 8017 (`Targets 0x10`, `Attributes 0x50200`, implicit 0) binds the main
    /// hand whatever the selection; an empty hand refuses; the same word without the attribute
    /// raises the item cursor.
    #[test]
    fn the_imbue_attribute_binds_the_main_hand_without_a_cursor() {
        let rockbiter = SpellDisplay {
            targets: 0x10,
            attributes: 0x0005_0200,
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            ..Default::default()
        };
        assert!(rockbiter.targets_main_hand_item());
        const WEAPON: u64 = 0xdead_beef;
        let armed = CastCandidates {
            selection: Some(42),
            caster: Some(1),
            main_hand_item: Some(WEAPON),
        };
        assert_eq!(
            resolve_cast_target(Some(&rockbiter), &armed, true, &rel_none()),
            CastWireTarget::Item(WEAPON),
            "the imbue binds the equipped weapon on the press"
        );
        assert_eq!(
            resolve_cast_target(
                Some(&rockbiter),
                &CastCandidates {
                    selection: None,
                    ..armed
                },
                false,
                &rel_none()
            ),
            CastWireTarget::Item(WEAPON),
            "neither the selection nor autoSelfCast has anything to do with it"
        );
        // Unarmed: the cast tail's second test of the bit (`0x6e504b`) refuses, skipping the
        // cursor arm (`0x6e50ab`).
        assert_eq!(
            resolve_cast_target(
                Some(&rockbiter),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::RefusedAtArm(ERR_MAINHAND_EMPTY),
            "no main hand — \"Your weapon hand is empty\", never the item cursor"
        );
        // A stone, poison, oil or enchant: the same word without the attribute.
        let stone = SpellDisplay {
            targets: 0x10,
            attributes: 0x0004_0000,
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            ..Default::default()
        };
        assert!(!stone.targets_main_hand_item());
        assert_eq!(
            resolve_cast_target(Some(&stone), &armed, true, &rel_none()),
            CastWireTarget::Targeting(TF_ITEM),
            "without the attribute the cursor comes up even with a weapon equipped"
        );
    }

    /// A dead unit's bind clears the `0x400` bit whoever it is (`6e5d89`, `6e5ddb`); a living
    /// unit's never does, and the gate refuses it before the relation checks are asked.
    #[test]
    fn a_dead_units_bind_clears_the_0x400_bit() {
        use benilla_protocol::ObjectFields;

        let health = |hp| ObjectStore(ObjectFields::from_pairs(&[(22, hp)]));
        let (corpse, live) = (health(0), health(100));
        let rel = |target| TargetRelations {
            target_store: Some(target),
            ..rel_none()
        };
        for is_self in [false, true] {
            assert!(
                unit_word_binds(0x402, is_self, &rel(&corpse)),
                "a corpse, self {is_self}"
            );
            assert!(
                !unit_word_binds(0x402, is_self, &rel(&live)),
                "a living unit keeps the bit, self {is_self}"
            );
        }
    }

    fn rel_none() -> TargetRelations<'static> {
        static EMPTY: Reputations = Reputations(Vec::new(), Vec::new());
        TargetRelations {
            target_store: None,
            target_owner_store: None,
            self_store: None,
            factions: None,
            reputations: &EMPTY,
            types: Default::default(),
            group: Default::default(),
        }
    }
}
