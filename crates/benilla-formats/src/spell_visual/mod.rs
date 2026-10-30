//! `SpellVisual.dbc`, `SpellVisualKit.dbc`, `SpellVisualEffectName.dbc` and
//! `SpellChainEffects.dbc`: what a cast shows at each stage. The 12340 record readers fix each
//! table's columns (`0x8b93f0`, `0x8b9120`, `0x8b8fe0`, `0x8b73b0`). A stage sets lifetime only:
//! every populated slot of a reached kit fires, precast persists until reaped by spell id, and
//! cast and impact end on their own.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

pub mod chain_effects;
pub mod missiles;

pub use chain_effects::{ChainEffect, ChainProc};
pub use missiles::{SpellMissileRec, DEFAULT_MISSILE_GRAVITY};

const SPELL_VISUAL: &str = "DBFilesClient\\SpellVisual.dbc";
const SPELL_VISUAL_KIT: &str = "DBFilesClient\\SpellVisualKit.dbc";
const SPELL_VISUAL_EFFECT_NAME: &str = "DBFilesClient\\SpellVisualEffectName.dbc";

const SPELL_VISUAL_FIELDS: usize = 32;
const SPELL_VISUAL_KIT_FIELDS: usize = 38;
const SPELL_VISUAL_EFFECT_NAME_FIELDS: usize = 7;

/// The kit columns of the emitter slots, in [`KIT_SLOT_TAGS`] order: head, chest, base, left hand,
/// right hand, breath, special 1-3. Columns 9 and 10, the weapon effects, sit between breath and
/// the specials; the slot walk (`0x745230`) plays neither.
const KIT_SLOT_COLUMNS: [usize; 9] = [3, 4, 5, 6, 7, 8, 11, 12, 13];

/// The base slot's index in [`KIT_SLOT_COLUMNS`], column 5.
const KIT_SLOT_BASE: usize = 2;

/// Each emitter slot's M2 `AttachmentID`, as the client's slot walk (`0x745230`) pushes them with
/// the columns of [`KIT_SLOT_COLUMNS`].
pub const KIT_SLOT_TAGS: [u16; 9] = [0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19];

/// The tag for kit column 14, the world effect: the slot walk plays it apart from the attached
/// slots (`0x7454e5`), planted once in world space at the owner's position, facing and scale; it
/// rides no bone and does not turn with the unit.
pub const WORLD_EFFECT_TAG: u16 = u16::MAX;

/// The client's missile destination attachments (`0xadaa20`): `SpellVisual` column 10 indexes it
/// for the M2 attachment the missile homes to on a live target, unless the row's flags make the
/// column an attachment id itself ([`VisualStages::missile_dest_attachment`]).
pub const MISSILE_ATTACH_TABLE: [u16; 16] = [
    0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19, 0xf, 0x10, 0x25, 0x26, 0x16, 0x2f, 0x30,
];

/// `SpellVisual` flags bit: column 10 is the attachment id itself, not an index into
/// [`MISSILE_ATTACH_TABLE`] (`0x733384`).
const FLAG_DEST_ATTACH_IS_ID: u32 = 0x200;

/// The data writes "no value" in an id column as 0 or as this, inconsistently; both read `None`.
const NONE_SENTINEL: u32 = u32::MAX;

/// `CharProc` slots per kit; the dispatcher walks exactly four.
pub const KIT_CHAR_PROCS: usize = 4;

/// One `CharProc` slot, a type key and four float params. The dispatcher switches on the type
/// through a byte table into nine cases, each reading the params it wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharProc {
    /// `CharProcType[i]`; never -1, as an empty slot is `None` in the kit.
    pub ty: i32,
    /// `CharParamZero..Three[i]`: floats the client loads with `fld`, rounding for an integer.
    pub params: [f32; 4],
}

impl CharProc {
    pub fn small_int(&self, i: usize) -> u32 {
        char_proc_small_int(self.params[i])
    }

    /// This slot as a weapon swing trail. The type-8 arm truncates with `_ftol`, not the small-int
    /// decode: `unit+0xd1c = zero | three << 24`, `unit+0xd20 = two`. `CharParamOne` is not read;
    /// it holds 20.0 on 22 of the 34 trail kits and 0 to 25 on the rest. A zero duration fires
    /// nothing, so it reads as `None`.
    pub fn as_weapon_trail(&self) -> Option<TrailProc> {
        if self.ty != char_proc_type::WEAPON_TRAIL {
            return None;
        }
        let ftol = |f: f32| f.trunc() as i32 as u32;
        let duration_ms = ftol(self.params[2]);
        (duration_ms != 0).then(|| TrailProc {
            packed: ftol(self.params[0]) | (ftol(self.params[3]) << 24),
            duration_ms,
        })
    }

    /// This slot as a beam; `None` too when `CharParamZero` truncates to 0, which names no row and
    /// is how the data writes an unused slot. The dispatcher's chain case reads params 0, 2 and 3
    /// only (`0x726ab1`-`0x726af2`): the id through `_ftol`, not the small-int decode, as 3.3.5a
    /// ids run past 255 (Chain Lightning's is 743).
    pub fn as_chain(&self) -> Option<ChainProc> {
        if !char_proc_type::is_chain(self.ty) {
            return None;
        }
        let effect_id = self.params[0] as u32;
        (effect_id != 0).then(|| ChainProc {
            effect_id,
            flag: self.params[2] != 0.0,
            ty: self.ty,
        })
    }
}

/// The 1.12.1 client's read of a small integer from a `CharProc` float param:
/// `bits(param + 512.0) >> 14 & 0xff`, where adding 512 fixes the exponent so the integer sits in
/// known mantissa bits. Used for the dynobject shard index; the client does no bounds check, so
/// callers must.
pub fn char_proc_small_int(param: f32) -> u32 {
    (param + 512.0).to_bits() >> 14 & 0xff
}

/// A decoded weapon trail: the two words latched on the unit (`unit+0xd1c`, `unit+0xd20`) and
/// copied into the trail's `SWING` record (`+0x608`, `+0x60c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailProc {
    /// `0xAARRGGBB`, the dword the reference stores; every shipped `CharParamZero` is below 2^24,
    /// so the OR never collides.
    pub packed: u32,
    /// How long the trail keeps appending, in ms: 600 on ordinary melee kits, 1000 on Charge,
    /// 10000 on Whirlwind.
    pub duration_ms: u32,
}

impl TrailProc {
    pub fn rgb(&self) -> [u8; 3] {
        [
            (self.packed >> 16) as u8,
            (self.packed >> 8) as u8,
            self.packed as u8,
        ]
    }

    /// The starting alpha, 100 on 19 of the 34 trail kits; the fade rate, the segment count and the
    /// end test all derive from it.
    pub fn alpha(&self) -> u8 {
        (self.packed >> 24) as u8
    }
}

/// The `CharProc` type keys benilla models, out of the dispatcher's nine cases.
pub mod char_proc_type {
    /// Body tint: `round(params[0])` is `0x00RRGGBB`, stored with alpha `0xff` in a node on
    /// `unit+0xce0`; each frame the head node's RGB / 255 goes to `model+0x184..0x18c`.
    pub const TINT: i32 = 1;
    /// Body translucency: `params[0]` is the aura's alpha factor, in a spell-keyed node linked at
    /// the head of `unit+0xb50`. The unit's alpha is `baseAlpha` times the head node's factor
    /// alone, never a product over the list, ramped over 1000 ms. Stealth's 0.3 and the ghost's 0.5
    /// are this proc.
    pub const ALPHA: i32 = 14;
    /// Body animation rate, the freeze: the bone-speed setter sets `params[0]` as the rate of the
    /// mount's bone 0, the body's key bone 4 when it has one and the body's bone 0, re-basing so
    /// the current frame holds. Arming an animation never resets it, so rate 0 holds the pose for
    /// the aura's life; the old rates return when the node expires. Of the 15 kits a live spell
    /// reaches, 14 ship 0 (Ice Block, Freeze, Petrify, …) and kit 1744 ships 8947848.0
    /// (`0x888888`), passed through as is.
    pub const ANIM_RATE: i32 = 11;

    /// Weapon swing trail, the ribbon between a weapon's `$WTB` and `$WTT` markers. It latches on
    /// the unit: the next animation start hands it to both weapon-hand trails and clears it, so it
    /// fires once per arm. For 21 of the 34 kits carrying it (Heroic Strike's 324, Hamstring's
    /// 3050, Sunder's 557) it is the whole visual: they fill no effect slot.
    pub const WEAPON_TRAIL: i32 = 8;

    /// A beam, as the data keys it on channel-stage kits (Drain Life, Mind Flay). It reaches the
    /// same dispatcher case as [`CHAIN_CAST`] (`0x72697a`, byte table `0x726ccc`); behaviour keys
    /// off [`ChainProc::flag`](crate::ChainProc::flag).
    pub const CHAIN_CHANNEL: i32 = 0;
    /// A beam, as the data keys it on cast-stage kits (Chain Lightning, Chain Heal).
    pub const CHAIN_CAST: i32 = 12;

    /// Whether this type reaches the beam case. Both chain keys do (`0x726ccc`), so ask this rather
    /// than compare with one constant.
    pub fn is_chain(ty: i32) -> bool {
        ty == CHAIN_CHANNEL || ty == CHAIN_CAST
    }
}

/// One `SpellVisual.dbc` row: the kit ids for the stages (0 for none), then the missile and
/// dest-anchored columns. Columns 14-15 (caster and target impact kits), 16-22 (the missile's
/// source attachment, ground following, motion and targeting kit) and 26-31 (the missile's cast
/// and impact offsets) are not read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisualStages {
    /// Column 1.
    pub precast: u32,
    /// Column 2.
    pub cast: u32,
    /// Column 3.
    pub impact: u32,
    /// Column 4.
    pub state: u32,
    /// Column 5, the kit an ending aura plays at stage 8 (`0x71ebef`).
    pub state_done: u32,
    /// Column 6, the kit `0x7224d0` takes a channel's animation from.
    pub channel: u32,
    /// Column 8: the missile's `SpellVisualEffectName` id; 0 flies the ammo or weapon model, and an
    /// id with no row the client's `Spells\ErrorCube.mdx`. A missile exists whenever `Spell.dbc`
    /// Speed > 0, whatever this holds.
    pub missile_model: u32,
    /// Column 10: an index into [`MISSILE_ATTACH_TABLE`], or an attachment id; read it through
    /// [`Self::missile_dest_attachment`].
    pub missile_attach: u32,
    /// Column 11: the `SoundEntries.dbc` loop the missile plays in flight, its volume shaped by
    /// distance, stopped when it arrives.
    pub missile_sound: Option<u32>,
    /// Column 12: the `SoundEntries.dbc` id the `$TRD` anim event plays at a craft's strike frame,
    /// such as mining's pick clang, sound 1143.
    pub strike_sound: Option<u32>,
    /// Column 7, `HasMissile`: when set and `Spell.dbc` Speed > 0, `SMSG_SPELL_GO` leaves
    /// [`Self::impact_area_kit`] to the missile's landing (`0x80ef53`).
    pub missile_gate: u32,
    /// Column 13, the row's flags; [`FLAG_DEST_ATTACH_IS_ID`] is the one read.
    pub flags: u32,
    /// Column 23: the kit `SMSG_SPELL_GO` plays at the packet's dest point when the targets carry
    /// one (`0x80ee85`).
    pub instant_area_kit: u32,
    /// Column 24: the kit played where the spell lands on the ground: the missile's landing point,
    /// or the dest point at `SMSG_SPELL_GO` when no missile flies (`0x80ef23`).
    pub impact_area_kit: u32,
    /// Column 25: a DynamicObject's kit. Its world effect, else its base slot, is the object's own
    /// model (`0x704b30`), and its CharProc type 9 the shard emitter (`0x704f60`).
    pub persistent_area_kit: u32,
}

impl VisualStages {
    /// The client's ranged weapon-visual merge (`0x720f80`): a ranged spell (`Attributes & 0x2`)
    /// fills each column its own row leaves 0 from the equipped ranged weapon's row. State,
    /// channel, flags and the area kits are never merged, and the missile gate comes across as a
    /// literal 1, bringing the model with it. Hunter shots animate this way: Multi-Shot and the
    /// stings carry precast = cast = 0 and take LoadBow and AttackBow from the bow's
    /// `ItemDisplayInfo` visual.
    #[must_use]
    pub fn merged_over_weapon(&self, weapon: &VisualStages) -> VisualStages {
        let fill = |own: u32, w: u32| if own == 0 { w } else { own };
        let mut out = *self;
        out.precast = fill(self.precast, weapon.precast); // 7210c3
        out.cast = fill(self.cast, weapon.cast); // 7210cf
        out.impact = fill(self.impact, weapon.impact); // 7210db
        if self.missile_gate == 0 && weapon.missile_gate != 0 {
            out.missile_gate = 1; // 72110b: the literal, not the weapon's value
            out.missile_model = weapon.missile_model; // 721112
        }
        out.missile_attach = fill(self.missile_attach, weapon.missile_attach); // 721124
        out.missile_sound = self.missile_sound.or(weapon.missile_sound); // 72116c
        out.strike_sound = self.strike_sound.or(weapon.strike_sound); // 721178
        out
    }

    /// The M2 attachment the missile homes to (`0x733384`): column 10 itself under
    /// [`FLAG_DEST_ATTACH_IS_ID`], else its [`MISSILE_ATTACH_TABLE`] entry; `None` past the table,
    /// where the reference reads on into adjacent data.
    pub fn missile_dest_attachment(&self) -> Option<u16> {
        if self.flags & FLAG_DEST_ATTACH_IS_ID != 0 {
            u16::try_from(self.missile_attach).ok()
        } else {
            MISSILE_ATTACH_TABLE
                .get(self.missile_attach as usize)
                .copied()
        }
    }
}

/// One `SpellVisualKit.dbc` row. Column 1 (the start animation), 9-10 (the weapon effects) and 37
/// (flags) are not read.
// No `Eq`: the `CharProc` params are floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VisualKit {
    /// Column 2: the `AnimationData.dbc` id played on the unit.
    pub anim_id: Option<u16>,
    /// Column 15: the `SoundEntries.dbc` id.
    pub sound: Option<u32>,
    /// `SpellVisualEffectName` ids from [`KIT_SLOT_COLUMNS`], slot `i` attaching at
    /// [`KIT_SLOT_TAGS`]`[i]`.
    pub effect_slots: [Option<u32>; 9],
    /// Column 14, a tenth effect slot the slot walk plays apart from the nine (`0x7454e5`), planted
    /// in world space. The data puts state models here (Frost Nova's ice, Net, Entangling Roots)
    /// and caster-feet rings (Thunderclap), never a projectile.
    pub world_effect: Option<u32>,
    /// Column 16: a `SpellEffectCameraShakes.dbc` group id, never a `CameraShakes` row
    /// ([`crate::SpellShakeGroup`]).
    pub shake: Option<u32>,
    /// Columns 17-36, transposed out of five parallel arrays. The dynobject emitter takes the first
    /// type-9 slot: `params[0]` is its shard model index, `params[1]` the emit rate the graphics
    /// quality scales.
    pub char_proc_slots: [Option<CharProc>; KIT_CHAR_PROCS],
}

impl VisualKit {
    /// The filled `CharProc` slots, in slot order.
    pub fn char_procs(&self) -> impl Iterator<Item = CharProc> + '_ {
        self.char_proc_slots.iter().flatten().copied()
    }

    /// `params[0]` of the first slot of type `ty`: the alpha factor or the packed tint.
    pub fn char_proc_param(&self, ty: i32) -> Option<f32> {
        self.char_procs().find(|p| p.ty == ty).map(|p| p.params[0])
    }

    /// The first slot that decodes as a weapon trail; no shipped kit has two.
    pub fn trail_proc(&self) -> Option<TrailProc> {
        self.char_procs().find_map(|p| p.as_weapon_trail())
    }

    /// The first slot that decodes as a beam.
    pub fn chain_proc(&self) -> Option<ChainProc> {
        self.char_procs().find_map(|p| p.as_chain())
    }

    /// `(attachment id, effect id)` for each populated slot in slot order, then the world effect
    /// at [`WORLD_EFFECT_TAG`]; the client fires them all at every stage (`0x745230`).
    pub fn effects(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.effect_slots
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.map(|id| (KIT_SLOT_TAGS[i], id)))
            .chain(self.world_effect.map(|id| (WORLD_EFFECT_TAG, id)))
    }
}

/// The spell visual tables, each in its own id space.
pub struct SpellVisualCatalog {
    visuals: HashMap<u32, VisualStages>,
    kits: HashMap<u32, VisualKit>,
    /// `SpellVisualEffectName` id → model path (field 2).
    effect_paths: HashMap<u32, String>,
    /// The `"HARDCODED *"` rows, name → (id, path): the effects the engine spawns itself, which the
    /// client resolves by name at boot against 14 baked strings.
    hardcoded: HashMap<String, (u32, String)>,
    chain_effects: HashMap<u32, ChainEffect>,
    /// `SpellMissile.dbc`, by `Spell.dbc` column 227.
    missiles: HashMap<u32, SpellMissileRec>,
}

impl SpellVisualCatalog {
    /// A catalog from explicit tables, for tests; no effect paths, hardcoded rows or beams.
    pub fn from_tables(visuals: HashMap<u32, VisualStages>, kits: HashMap<u32, VisualKit>) -> Self {
        Self::from_tables_with_paths(visuals, kits, HashMap::new())
    }

    pub fn from_tables_with_paths(
        visuals: HashMap<u32, VisualStages>,
        kits: HashMap<u32, VisualKit>,
        effect_paths: HashMap<u32, String>,
    ) -> Self {
        Self {
            visuals,
            kits,
            effect_paths,
            hardcoded: HashMap::new(),
            chain_effects: HashMap::new(),
            missiles: HashMap::new(),
        }
    }

    /// Seed one `SpellMissile` row, for tests.
    #[must_use]
    pub fn with_missile(mut self, id: u32, missile: SpellMissileRec) -> Self {
        self.missiles.insert(id, missile);
        self
    }

    /// A `SpellMissile.dbc` row by id.
    pub fn missile(&self, id: u32) -> Option<&SpellMissileRec> {
        self.missiles.get(&id)
    }

    /// Seed one `SpellChainEffects` row, for tests.
    #[must_use]
    pub fn with_chain_effect(mut self, id: u32, effect: ChainEffect) -> Self {
        self.chain_effects.insert(id, effect);
        self
    }

    /// Seed one `"HARDCODED …"` row, for tests. The id matters: it is half the reference's
    /// same-slot replace key, so an engine-spawned effect dedups like a kit slot's.
    #[must_use]
    pub fn with_hardcoded(mut self, name: &str, id: u32, path: &str) -> Self {
        self.hardcoded
            .insert(name.to_string(), (id, path.to_string()));
        self
    }

    /// A `SpellVisual.dbc` row by id, `Spell.dbc` column 131.
    pub fn stages(&self, visual_id: u32) -> Option<&VisualStages> {
        self.visuals.get(&visual_id)
    }

    pub fn kit(&self, kit_id: u32) -> Option<&VisualKit> {
        self.kits.get(&kit_id)
    }

    /// The number of `SpellVisual` rows loaded.
    pub fn len(&self) -> usize {
        self.visuals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visuals.is_empty()
    }

    /// Every `SpellVisual` row as `(id, stages)`, in no order, for whole-table census tools.
    pub fn visuals(&self) -> impl Iterator<Item = (u32, &VisualStages)> + '_ {
        self.visuals.iter().map(|(id, s)| (*id, s))
    }

    /// Every `SpellVisualKit` id, ascending, for whole-table census tools.
    pub fn kit_ids(&self) -> impl Iterator<Item = u32> + '_ {
        let mut ids: Vec<u32> = self.kits.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
    }

    pub fn kit_len(&self) -> usize {
        self.kits.len()
    }

    /// A `SpellVisualEffectName` id's model path; an empty path reads as `None`.
    pub fn effect_path(&self, effect_id: u32) -> Option<&str> {
        self.effect_paths
            .get(&effect_id)
            .map(String::as_str)
            .filter(|p| !p.is_empty())
    }

    /// An engine-spawned effect's `(id, model path)` by its baked name, compared without case as
    /// the client's boot resolve does.
    pub fn hardcoded_effect(&self, name: &str) -> Option<(u32, &str)> {
        self.hardcoded
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, (id, path))| (*id, path.as_str()))
            .filter(|(_, p)| !p.is_empty())
    }

    /// The lootable-corpse sparkle, `"HARDCODED Loot Art"`.
    pub fn loot_art_effect(&self) -> Option<(u32, &str)> {
        self.hardcoded_effect("HARDCODED Loot Art")
    }

    /// A DynamicObject's own model, `(SpellVisualEffectName id, path)` (`0x704b30`): the
    /// persistent area kit's world effect, else its base slot, column 5.
    pub fn persistent_area_model(&self, stages: &VisualStages) -> Option<(u32, &str)> {
        let kit = self.kit(stages.persistent_area_kit)?;
        let base = kit.effect_slots[KIT_SLOT_BASE];
        [kit.world_effect, base]
            .into_iter()
            .flatten()
            .find(|id| self.effect_paths.contains_key(id))
            .and_then(|id| Some((id, self.effect_path(id)?)))
    }

    /// A `SpellChainEffects` row by id; `None` is the client's null-row no-op.
    pub fn chain_effect(&self, id: u32) -> Option<&ChainEffect> {
        self.chain_effects.get(&id)
    }

    pub fn chain_effect_len(&self) -> usize {
        self.chain_effects.len()
    }

    /// Every `SpellChainEffects` id, ascending, for whole-table census tools.
    pub fn chain_effect_ids(&self) -> impl Iterator<Item = u32> + '_ {
        let mut ids: Vec<u32> = self.chain_effects.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
    }
}

fn n_u32_schema(name: &str, fields: usize) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..fields {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

pub(crate) fn spell_visual_schema() -> Schema {
    n_u32_schema("SpellVisual", SPELL_VISUAL_FIELDS)
}

/// `SpellVisualEffectName`: id, name, model path, then the area effect size and three scale
/// floats, which benilla does not read.
pub(crate) fn effect_name_schema() -> Schema {
    let mut s = Schema::new("SpellVisualEffectName");
    for i in 0..SPELL_VISUAL_EFFECT_NAME_FIELDS {
        let ty = match i {
            1 | 2 => FieldType::String,
            0 => FieldType::UInt32,
            _ => FieldType::Float32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// `SpellVisualKit`: columns 0-16 are ids, 17-20 the signed `CharProcType` keys, 21-36 the float
/// params and 37 the flags.
pub(crate) fn kit_schema() -> Schema {
    let mut s = Schema::new("SpellVisualKit");
    for i in 0..SPELL_VISUAL_KIT_FIELDS {
        let ty = match i {
            CHAR_PROC_TYPE_FIELD..CHAR_PROC_PARAM_FIELD => FieldType::Int32,
            CHAR_PROC_PARAM_FIELD..KIT_FLAGS_FIELD => FieldType::Float32,
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Kit column 17, `CharProcType[0]` (`+0x44`, which `0x724bed` walks for type 11).
const CHAR_PROC_TYPE_FIELD: usize = 17;
/// Kit column 21, `CharParamZero[0]` (`+0x54`); `CharParamOne..Three[0]` follow at every
/// [`KIT_CHAR_PROCS`] columns (`+0x64`, which the shard emitter `0x704f60` reads as its rate).
const CHAR_PROC_PARAM_FIELD: usize = CHAR_PROC_TYPE_FIELD + KIT_CHAR_PROCS;
/// Kit column 37, the flags after the four param arrays.
const KIT_FLAGS_FIELD: usize = CHAR_PROC_PARAM_FIELD + 4 * KIT_CHAR_PROCS;

/// Kit `CharProc` slot `i`. The empty slot is -1; 0 is a real key, the channel beam. A type-0 slot
/// that is padding decodes `CharParamZero` to 0, which the client's null-row test no-ops.
fn char_proc_slot(r: &benilla_dbc::Record, i: usize) -> Option<CharProc> {
    let ty = i32_at(r, CHAR_PROC_TYPE_FIELD + i)?;
    if ty < 0 {
        return None;
    }
    let mut params = [0.0; 4];
    for (p, param) in params.iter_mut().enumerate() {
        *param = f32_at(r, CHAR_PROC_PARAM_FIELD + p * KIT_CHAR_PROCS + i).unwrap_or(0.0);
    }
    Some(CharProc { ty, params })
}

fn some_unless_none(v: u32) -> Option<u32> {
    (v != 0 && v != NONE_SENTINEL).then_some(v)
}

/// `SpellVisual.dbc`'s rows by id.
fn parse_visuals(bytes: &[u8]) -> Result<HashMap<u32, VisualStages>> {
    let set = parse(bytes, spell_visual_schema(), "SpellVisual.dbc")?;
    let mut visuals = HashMap::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        visuals.insert(
            id,
            VisualStages {
                precast: g(1),
                cast: g(2),
                impact: g(3),
                state: g(4),
                state_done: g(5),
                channel: g(6),
                missile_model: g(8),
                missile_attach: g(10),
                missile_sound: u32_at(r, 11).and_then(some_unless_none),
                strike_sound: u32_at(r, 12).and_then(some_unless_none),
                missile_gate: g(7),
                flags: g(13),
                instant_area_kit: g(23),
                impact_area_kit: g(24),
                persistent_area_kit: g(25),
            },
        );
    }
    Ok(visuals)
}

/// `SpellVisualKit.dbc`'s rows by id.
fn parse_kits(bytes: &[u8]) -> Result<HashMap<u32, VisualKit>> {
    let set = parse(bytes, kit_schema(), "SpellVisualKit.dbc")?;
    let mut kits = HashMap::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let anim_id = u32_at(r, 2).and_then(some_unless_none).map(|a| a as u16);
        let sound = u32_at(r, 15).and_then(some_unless_none);
        let mut effect_slots = [None; 9];
        for (slot, &col) in effect_slots.iter_mut().zip(&KIT_SLOT_COLUMNS) {
            *slot = u32_at(r, col).and_then(some_unless_none);
        }
        let mut char_proc_slots = [None; KIT_CHAR_PROCS];
        for (i, slot) in char_proc_slots.iter_mut().enumerate() {
            *slot = char_proc_slot(r, i);
        }
        kits.insert(
            id,
            VisualKit {
                anim_id,
                sound,
                effect_slots,
                world_effect: u32_at(r, 14).and_then(some_unless_none),
                shake: u32_at(r, 16).and_then(some_unless_none),
                char_proc_slots,
            },
        );
    }
    Ok(kits)
}

/// Read the spell visual tables and `SpellMissile.dbc` off the patch chain.
pub fn load_spell_visual_catalog(chain: &mut Chain) -> Result<SpellVisualCatalog> {
    let visuals = parse_visuals(
        &chain
            .read_file(SPELL_VISUAL)
            .context("reading SpellVisual.dbc")?,
    )?;
    let kits = parse_kits(
        &chain
            .read_file(SPELL_VISUAL_KIT)
            .context("reading SpellVisualKit.dbc")?,
    )?;

    let sven_bytes = chain
        .read_file(SPELL_VISUAL_EFFECT_NAME)
        .context("reading SpellVisualEffectName.dbc")?;
    let sven_set = parse(
        &sven_bytes,
        effect_name_schema(),
        "SpellVisualEffectName.dbc",
    )?;
    let mut effect_paths = HashMap::with_capacity(sven_set.records().len());
    let mut hardcoded = HashMap::new();
    for r in sven_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(path) = str_at(&sven_set, r, 2) {
            // Only the rows the client's boot resolve can hit, those named "HARDCODED …".
            if let Some(name) = str_at(&sven_set, r, 1).filter(|n| n.starts_with("HARDCODED ")) {
                hardcoded.insert(name, (id, path.clone()));
            }
            effect_paths.insert(id, path);
        }
    }

    let chain_effects = chain_effects::load(chain).context("reading SpellChainEffects.dbc")?;
    let missiles = missiles::parse_missiles(
        &chain
            .read_file(missiles::SPELL_MISSILE)
            .context("reading SpellMissile.dbc")?,
    )?;

    Ok(SpellVisualCatalog {
        visuals,
        kits,
        effect_paths,
        hardcoded,
        chain_effects,
        missiles,
    })
}

#[cfg(test)]
mod tests;
