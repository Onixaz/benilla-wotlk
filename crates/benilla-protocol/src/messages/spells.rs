//! The cast pipeline on the wire: the `CMSG_CAST_SPELL` shapes, the server's verdict, the
//! start/launch pair every observer sees, interrupt and pushback notices, the channel timer and the
//! aura messages. The auras themselves arrive in `SMSG_AURA_UPDATE` (`super::aura`).

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_cstring, read_i32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8,
    Vector3d,
};

/// The verdict on our cast. 12340's `SMSG_CAST_FAILED` (0x130, the opcode benilla names
/// `SMSG_CAST_RESULT`) only ever carries a failure; a cast that goes ahead is answered by its
/// `SMSG_SPELL_START`. `Ok` is kept for the pet's 1.12.1-shaped reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastOutcome {
    Ok,
    Failed {
        reason: u8,
        /// The `%s` of the client's message (`0x6e1d8e`): a `SpellFocusObject.dbc` id for
        /// `REQUIRES_SPELL_FOCUS` (0x5e), an `AreaTable.dbc` id for `REQUIRES_AREA` (0x5d), an
        /// `EquippedItemClass` for 0x19-0x1b, a permanent-cooldown flag for `NOT_READY` (0x3c).
        arg: Option<u32>,
    },
}

/// The 12340 client's `SPELL_FAILED_*` key for each wire reason, its switch `0x7fcca0` (jump table
/// `0x7fd11c`); a reason past it reads `SPELL_FAILED_UNKNOWN` (`0x7fd114`).
pub const WOTLK_CAST_FAIL_KEYS: [&str; 187] = [
    "SPELL_FAILED_SUCCESS",
    "SPELL_FAILED_AFFECTING_COMBAT",
    "SPELL_FAILED_ALREADY_AT_FULL_HEALTH",
    "SPELL_FAILED_ALREADY_AT_FULL_MANA",
    "SPELL_FAILED_ALREADY_AT_FULL_POWER",
    "SPELL_FAILED_ALREADY_BEING_TAMED",
    "SPELL_FAILED_ALREADY_HAVE_CHARM",
    "SPELL_FAILED_ALREADY_HAVE_SUMMON",
    "SPELL_FAILED_ALREADY_OPEN",
    "SPELL_FAILED_AURA_BOUNCED",
    "SPELL_FAILED_AUTOTRACK_INTERRUPTED",
    "SPELL_FAILED_BAD_IMPLICIT_TARGETS",
    "SPELL_FAILED_BAD_TARGETS",
    "SPELL_FAILED_CANT_BE_CHARMED",
    "SPELL_FAILED_CANT_BE_DISENCHANTED",
    "SPELL_FAILED_CANT_BE_DISENCHANTED_SKILL",
    "SPELL_FAILED_CANT_BE_MILLED",
    "SPELL_FAILED_CANT_BE_PROSPECTED",
    "SPELL_FAILED_CANT_CAST_ON_TAPPED",
    "SPELL_FAILED_CANT_DUEL_WHILE_INVISIBLE",
    "SPELL_FAILED_CANT_DUEL_WHILE_STEALTHED",
    "SPELL_FAILED_CANT_STEALTH",
    "SPELL_FAILED_CASTER_AURASTATE",
    "SPELL_FAILED_CASTER_DEAD",
    "SPELL_FAILED_CHARMED",
    "SPELL_FAILED_CHEST_IN_USE",
    "SPELL_FAILED_CONFUSED",
    "SPELL_FAILED_DONT_REPORT",
    "SPELL_FAILED_EQUIPPED_ITEM",
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS",
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS_MAINHAND",
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS_OFFHAND",
    "SPELL_FAILED_ERROR",
    "SPELL_FAILED_FIZZLE",
    "SPELL_FAILED_FLEEING",
    "SPELL_FAILED_FOOD_LOWLEVEL",
    "SPELL_FAILED_HIGHLEVEL",
    "SPELL_FAILED_HUNGER_SATIATED",
    "SPELL_FAILED_IMMUNE",
    "SPELL_FAILED_INCORRECT_AREA",
    "SPELL_FAILED_INTERRUPTED",
    "SPELL_FAILED_INTERRUPTED_COMBAT",
    "SPELL_FAILED_ITEM_ALREADY_ENCHANTED",
    "SPELL_FAILED_ITEM_GONE",
    "SPELL_FAILED_ITEM_NOT_FOUND",
    "SPELL_FAILED_ITEM_NOT_READY",
    "SPELL_FAILED_LEVEL_REQUIREMENT",
    "SPELL_FAILED_LINE_OF_SIGHT",
    "SPELL_FAILED_LOWLEVEL",
    "SPELL_FAILED_LOW_CASTLEVEL",
    "SPELL_FAILED_MAINHAND_EMPTY",
    "SPELL_FAILED_MOVING",
    "SPELL_FAILED_NEED_AMMO",
    "SPELL_FAILED_NEED_AMMO_POUCH",
    "SPELL_FAILED_NEED_EXOTIC_AMMO",
    "SPELL_FAILED_NEED_MORE_ITEMS",
    "SPELL_FAILED_NOPATH",
    "SPELL_FAILED_NOT_BEHIND",
    "SPELL_FAILED_NOT_FISHABLE",
    "SPELL_FAILED_NOT_FLYING",
    "SPELL_FAILED_NOT_HERE",
    "SPELL_FAILED_NOT_INFRONT",
    "SPELL_FAILED_NOT_IN_CONTROL",
    "SPELL_FAILED_NOT_KNOWN",
    "SPELL_FAILED_NOT_MOUNTED",
    "SPELL_FAILED_NOT_ON_TAXI",
    "SPELL_FAILED_NOT_ON_TRANSPORT",
    "SPELL_FAILED_NOT_READY",
    "SPELL_FAILED_NOT_SHAPESHIFT",
    "SPELL_FAILED_NOT_STANDING",
    "SPELL_FAILED_NOT_TRADEABLE",
    "SPELL_FAILED_NOT_TRADING",
    "SPELL_FAILED_NOT_UNSHEATHED",
    "SPELL_FAILED_NOT_WHILE_GHOST",
    "SPELL_FAILED_NOT_WHILE_LOOTING",
    "SPELL_FAILED_NO_AMMO",
    "SPELL_FAILED_NO_CHARGES_REMAIN",
    "SPELL_FAILED_NO_CHAMPION",
    "SPELL_FAILED_NO_COMBO_POINTS",
    "SPELL_FAILED_NO_DUELING",
    "SPELL_FAILED_NO_ENDURANCE",
    "SPELL_FAILED_NO_FISH",
    "SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED",
    "SPELL_FAILED_NO_MOUNTS_ALLOWED",
    "SPELL_FAILED_NO_PET",
    "SPELL_FAILED_NO_POWER",
    "SPELL_FAILED_NOTHING_TO_DISPEL",
    "SPELL_FAILED_NOTHING_TO_STEAL",
    "SPELL_FAILED_ONLY_ABOVEWATER",
    "SPELL_FAILED_ONLY_DAYTIME",
    "SPELL_FAILED_ONLY_INDOORS",
    "SPELL_FAILED_ONLY_MOUNTED",
    "SPELL_FAILED_ONLY_NIGHTTIME",
    "SPELL_FAILED_ONLY_OUTDOORS",
    "SPELL_FAILED_ONLY_SHAPESHIFT",
    "SPELL_FAILED_ONLY_STEALTHED",
    "SPELL_FAILED_ONLY_UNDERWATER",
    "SPELL_FAILED_OUT_OF_RANGE",
    "SPELL_FAILED_PACIFIED",
    "SPELL_FAILED_POSSESSED",
    "SPELL_FAILED_REAGENTS",
    "SPELL_FAILED_REQUIRES_AREA",
    "SPELL_FAILED_REQUIRES_SPELL_FOCUS",
    "SPELL_FAILED_ROOTED",
    "SPELL_FAILED_SILENCED",
    "SPELL_FAILED_SPELL_IN_PROGRESS",
    "SPELL_FAILED_SPELL_LEARNED",
    "SPELL_FAILED_SPELL_UNAVAILABLE",
    "SPELL_FAILED_STUNNED",
    "SPELL_FAILED_TARGETS_DEAD",
    "SPELL_FAILED_TARGET_AFFECTING_COMBAT",
    "SPELL_FAILED_TARGET_AURASTATE",
    "SPELL_FAILED_TARGET_DUELING",
    "SPELL_FAILED_TARGET_ENEMY",
    "SPELL_FAILED_TARGET_ENRAGED",
    "SPELL_FAILED_TARGET_FRIENDLY",
    "SPELL_FAILED_TARGET_IN_COMBAT",
    "SPELL_FAILED_TARGET_IS_PLAYER",
    "SPELL_FAILED_TARGET_IS_PLAYER_CONTROLLED",
    "SPELL_FAILED_TARGET_NOT_DEAD",
    "SPELL_FAILED_TARGET_NOT_IN_PARTY",
    "SPELL_FAILED_TARGET_NOT_LOOTED",
    "SPELL_FAILED_TARGET_NOT_PLAYER",
    "SPELL_FAILED_TARGET_NO_POCKETS",
    "SPELL_FAILED_TARGET_NO_WEAPONS",
    "SPELL_FAILED_TARGET_NO_RANGED_WEAPONS",
    "SPELL_FAILED_TARGET_UNSKINNABLE",
    "SPELL_FAILED_THIRST_SATIATED",
    "SPELL_FAILED_TOO_CLOSE",
    "SPELL_FAILED_TOO_MANY_OF_ITEM",
    "SPELL_FAILED_TOTEM_CATEGORY",
    "SPELL_FAILED_TOTEMS",
    "SPELL_FAILED_TRY_AGAIN",
    "SPELL_FAILED_UNIT_NOT_BEHIND",
    "SPELL_FAILED_UNIT_NOT_INFRONT",
    "SPELL_FAILED_WRONG_PET_FOOD",
    "SPELL_FAILED_NOT_WHILE_FATIGUED",
    "SPELL_FAILED_TARGET_NOT_IN_INSTANCE",
    "SPELL_FAILED_NOT_WHILE_TRADING",
    "SPELL_FAILED_TARGET_NOT_IN_RAID",
    "SPELL_FAILED_TARGET_FREEFORALL",
    "SPELL_FAILED_NO_EDIBLE_CORPSES",
    "SPELL_FAILED_ONLY_BATTLEGROUNDS",
    "SPELL_FAILED_TARGET_NOT_GHOST",
    "SPELL_FAILED_TRANSFORM_UNUSABLE",
    "SPELL_FAILED_WRONG_WEATHER",
    "SPELL_FAILED_DAMAGE_IMMUNE",
    "SPELL_FAILED_PREVENTED_BY_MECHANIC",
    "SPELL_FAILED_PLAY_TIME",
    "SPELL_FAILED_REPUTATION",
    "SPELL_FAILED_MIN_SKILL",
    "SPELL_FAILED_NOT_IN_ARENA",
    "SPELL_FAILED_NOT_ON_SHAPESHIFT",
    "SPELL_FAILED_NOT_ON_STEALTHED",
    "SPELL_FAILED_NOT_ON_DAMAGE_IMMUNE",
    "SPELL_FAILED_NOT_ON_MOUNTED",
    "SPELL_FAILED_TOO_SHALLOW",
    "SPELL_FAILED_TARGET_NOT_IN_SANCTUARY",
    "SPELL_FAILED_TARGET_IS_TRIVIAL",
    "SPELL_FAILED_BM_OR_INVISGOD",
    "SPELL_FAILED_EXPERT_RIDING_REQUIREMENT",
    "SPELL_FAILED_ARTISAN_RIDING_REQUIREMENT",
    "SPELL_FAILED_NOT_IDLE",
    "SPELL_FAILED_NOT_INACTIVE",
    "SPELL_FAILED_PARTIAL_PLAYTIME",
    "SPELL_FAILED_NO_PLAYTIME",
    "SPELL_FAILED_NOT_IN_BATTLEGROUND",
    "SPELL_FAILED_NOT_IN_RAID_INSTANCE",
    "SPELL_FAILED_ONLY_IN_ARENA",
    "SPELL_FAILED_TARGET_LOCKED_TO_RAID_INSTANCE",
    "SPELL_FAILED_ON_USE_ENCHANT",
    "SPELL_FAILED_NOT_ON_GROUND",
    "SPELL_FAILED_CUSTOM_ERROR",
    "SPELL_FAILED_CANT_DO_THAT_RIGHT_NOW",
    "SPELL_FAILED_TOO_MANY_SOCKETS",
    "SPELL_FAILED_INVALID_GLYPH",
    "SPELL_FAILED_UNIQUE_GLYPH",
    "SPELL_FAILED_GLYPH_SOCKET_LOCKED",
    "SPELL_FAILED_NO_VALID_TARGETS",
    "SPELL_FAILED_ITEM_AT_MAX_CHARGES",
    "SPELL_FAILED_NOT_IN_BARBERSHOP",
    "SPELL_FAILED_FISHING_TOO_LOW",
    "SPELL_FAILED_ITEM_ENCHANT_TRADE_WINDOW",
    "SPELL_FAILED_SUMMON_PENDING",
    "SPELL_FAILED_MAX_SOCKETS",
    "SPELL_FAILED_PET_CAN_RENAME",
    "SPELL_FAILED_TARGET_CANNOT_BE_RESURRECTED",
];

/// [`WOTLK_CAST_FAIL_KEYS`] in benilla's shared reason numbering: 1.12.1's, then 3.3.5a's own
/// reasons appended in wire order. Generated by key name.
const WOTLK_CAST_FAIL_SHARED: [u8; 187] = [
    0x92, 0x00, 0x01, 0x93, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x94,
    0x95, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
    0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x96, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a,
    0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x97, 0x32, 0x33, 0x34, 0x98, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x99, 0x43, 0x44, 0x45, 0x46, 0x47,
    0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57,
    0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d, 0x5e, 0x5f, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67,
    0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x9a, 0x6e, 0x6f, 0x70, 0x71, 0x72, 0x73, 0x9b, 0x74, 0x75,
    0x76, 0x77, 0x9c, 0x78, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e, 0x7f, 0x80, 0x81, 0x85, 0x86, 0x87, 0x88,
    0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x9d, 0x9e, 0x9f, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5,
    0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe, 0xbf, 0xc0,
];

/// The shared `SpellCastResult` of a 3.3.5a wire reason, the numbering the cast-failure display
/// is keyed on; a reason past the 12340 table is `SPELL_FAILED_UNKNOWN`, shared 0x91.
pub fn wotlk_cast_fail_shared(wire: u8) -> u8 {
    WOTLK_CAST_FAIL_SHARED
        .get(usize::from(wire))
        .copied()
        .unwrap_or(SHARED_CAST_FAIL_UNKNOWN)
}

/// `SPELL_FAILED_UNKNOWN` in the shared numbering.
const SHARED_CAST_FAIL_UNKNOWN: u8 = 0x91;

/// Read `SMSG_CAST_FAILED` as the 12340 handler does (`0x809af0`): `u8 castCount`, `u32 spellId`,
/// `u8 reason`, then up to two argument words, each read only while bytes remain (`0x809b35`,
/// `0x809b48`). There is no status byte. The reason comes back in the shared numbering
/// ([`wotlk_cast_fail_shared`]). `None` stands for an absent argument. The second word is
/// dropped: the client fills that message from the spell's own `EquippedItem*` columns.
pub(super) fn read_cast_result(r: &mut &[u8]) -> io::Result<(u8, u32, CastOutcome)> {
    let cast_count = read_u8(r)?;
    let spell_id = read_u32_le(r)?;
    let reason = wotlk_cast_fail_shared(read_u8(r)?);
    let arg = (r.len() >= 4).then(|| read_u32_le(r)).transpose()?;
    let _arg2 = (r.len() >= 4).then(|| read_u32_le(r)).transpose()?;
    Ok((cast_count, spell_id, CastOutcome::Failed { reason, arg }))
}

// --- Start and go --------------------------------------------------------------------------------
//
// The `SpellCastTargets` mask bits, a `u32` on 12340.
pub(super) const TARGET_FLAG_UNIT: u32 = 0x0002;
pub(super) const TARGET_FLAG_ITEM: u32 = 0x0010;
pub(super) const TARGET_FLAG_SOURCE_LOCATION: u32 = 0x0020;
pub(super) const TARGET_FLAG_DEST_LOCATION: u32 = 0x0040;
const TARGET_FLAG_CORPSE_ENEMY: u32 = 0x0200;
pub(super) const TARGET_FLAG_GAMEOBJECT: u32 = 0x0800;
const TARGET_FLAG_TRADE_ITEM: u32 = 0x1000;
const TARGET_FLAG_STRING: u32 = 0x2000;
const TARGET_FLAG_CORPSE_ALLY: u32 = 0x8000;
const TARGET_FLAG_UNIT_MINIPET: u32 = 0x1_0000;
/// A glyph socket, with no payload: `CMSG_USE_ITEM`'s glyph index names it (`0x80c583`).
pub(super) const TARGET_FLAG_GLYPH_SLOT: u32 = 0x2_0000;
/// Extra targets, read only by `SMSG_SPELL_GO`'s tail (`0x80e44f`).
const TARGET_FLAG_EXTRA_TARGETS: u32 = 0x8_0000;
/// The bits one packed guid answers, as the 12340 reader and writer test them (`0x9ab8c9`).
const TARGET_GUID_BITS: u32 = TARGET_FLAG_UNIT
    | TARGET_FLAG_CORPSE_ENEMY
    | TARGET_FLAG_GAMEOBJECT
    | TARGET_FLAG_CORPSE_ALLY
    | TARGET_FLAG_UNIT_MINIPET;

/// A decoded `SpellCastTargets` as the 12340 client reads it (`0x9ab9c0`): a `u32` mask; one packed
/// guid for any of [`TARGET_GUID_BITS`]; a packed item guid for ITEM or TRADE_ITEM; for SOURCE and
/// DEST each, a packed transport guid and three `f32`s; a string. Item, corpse, source, transport
/// and string targets are read and dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpellCastTargets {
    pub mask: u32,
    pub unit_target: Option<u64>,
    /// The `TARGET_FLAG_GAMEOBJECT` target, such as the chest an open-lock cast opens.
    pub go_target: Option<u64>,
    pub dest: Option<Vector3d>,
}

fn read_spell_cast_targets(r: &mut impl Read) -> io::Result<SpellCastTargets> {
    let mask = read_u32_le(r)?;
    let mut unit_target = None;
    let mut go_target = None;
    if mask & TARGET_GUID_BITS != 0 {
        // One packed guid; UNIT (or a minipet) first, then GAMEOBJECT, then CORPSE_*.
        let guid = read_packed_guid(r)?;
        if mask & (TARGET_FLAG_UNIT | TARGET_FLAG_UNIT_MINIPET) != 0 {
            unit_target = Some(guid);
        } else if mask & TARGET_FLAG_GAMEOBJECT != 0 {
            go_target = Some(guid);
        }
    }
    if mask & (TARGET_FLAG_ITEM | TARGET_FLAG_TRADE_ITEM) != 0 {
        let _item_guid = read_packed_guid(r)?;
    }
    if mask & TARGET_FLAG_SOURCE_LOCATION != 0 {
        let _transport = read_packed_guid(r)?;
        let _src = Vector3d::read(r)?;
    }
    let dest = if mask & TARGET_FLAG_DEST_LOCATION != 0 {
        let _transport = read_packed_guid(r)?;
        Some(Vector3d::read(r)?)
    } else {
        None
    };
    if mask & TARGET_FLAG_STRING != 0 {
        let _string_target = read_cstring(r)?;
    }
    Ok(SpellCastTargets {
        mask,
        unit_target,
        go_target,
        dest,
    })
}

// The `castFlags` bits that gate the START and GO tails, in the 12340 handlers' read order.
const CAST_FLAG_AMMO: u32 = 0x20;
const CAST_FLAG_POWER_LEFT_SELF: u32 = 0x800;
const CAST_FLAG_ADJUST_MISSILE: u32 = 0x2_0000;
const CAST_FLAG_VISUAL_CHAIN: u32 = 0x8_0000;
const CAST_FLAG_RUNE_LIST: u32 = 0x20_0000;
const CAST_FLAG_IMMUNITY: u32 = 0x400_0000;

/// Read the ammo block: `u32 displayId`, `u32 inventoryType` (dropped).
fn read_ammo(r: &mut impl Read) -> io::Result<u32> {
    let display_id = read_u32_le(r)?;
    let _inventory_type = read_u32_le(r)?;
    Ok(display_id)
}

/// A cast's rune block: the caster's ready-rune masks before and after the cast, and for each of
/// the six runes it spent, the share of the recharge already passed out of 255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CastRunes {
    pub before: u8,
    pub after: u8,
    /// By rune index; a rune the cast did not spend reads 0.
    pub progress: [u8; 6],
}

/// The rune block (`0x80677e`, `0x80e353`): the two masks, then one byte for each rune of the six
/// set in `before` and clear in `after`.
fn read_runes(r: &mut impl Read) -> io::Result<CastRunes> {
    let mut runes = CastRunes {
        before: read_u8(r)?,
        after: read_u8(r)?,
        ..Default::default()
    };
    for (i, progress) in runes.progress.iter_mut().enumerate() {
        let bit = 1u8 << i;
        if runes.before & bit != 0 && runes.after & bit == 0 {
            *progress = read_u8(r)?;
        }
    }
    Ok(runes)
}

/// `SMSG_SPELL_START` (handlers `0x80fee0`, `0x806700`): a non-triggered cast began, instants
/// included (`cast_time_ms == 0`). `item_or_caster` is the cast item's guid if any, else the
/// caster's; `caster` is 0 for a GameObject caster.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellStart {
    pub item_or_caster: u64,
    pub caster: u64,
    /// The `castCount` of our `CMSG_CAST_SPELL` this answers; the client's pending-cast key.
    pub cast_count: u8,
    pub spell_id: u32,
    pub cast_flags: u32,
    pub cast_time_ms: u32,
    pub targets: SpellCastTargets,
    pub ammo_display_id: Option<u32>,
    /// The `CAST_FLAG_RUNE_LIST` block.
    pub runes: Option<CastRunes>,
}

/// Read `SMSG_SPELL_START`: the shared head (`0x80fef4`-`0x80ff22`), `u32 castTime`, the targets,
/// then the tail `0x806742`-`0x8067ff` reads by `castFlags`: power, runes, ammo, immunity.
pub(super) fn read_spell_start(r: &mut impl Read) -> io::Result<SpellStart> {
    let item_or_caster = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let cast_count = read_u8(r)?;
    let spell_id = read_u32_le(r)?;
    let cast_flags = read_u32_le(r)?;
    let cast_time_ms = read_u32_le(r)?;
    let targets = read_spell_cast_targets(r)?;
    if cast_flags & CAST_FLAG_POWER_LEFT_SELF != 0 {
        let _power = read_u32_le(r)?;
    }
    let runes = if cast_flags & CAST_FLAG_RUNE_LIST != 0 {
        Some(read_runes(r)?)
    } else {
        None
    };
    let ammo_display_id = if cast_flags & CAST_FLAG_AMMO != 0 {
        Some(read_ammo(r)?)
    } else {
        None
    };
    if cast_flags & CAST_FLAG_IMMUNITY != 0 {
        let _school_immunities = read_u32_le(r)?;
        let _mechanic_immunities = read_u32_le(r)?;
    }
    Ok(SpellStart {
        item_or_caster,
        caster,
        cast_count,
        spell_id,
        cast_flags,
        cast_time_ms,
        targets,
        ammo_display_id,
        runes,
    })
}

/// The one `SpellMissInfo` followed by a byte, the reflected spell's outcome against its new
/// target.
const SPELL_MISS_REFLECT: u8 = 11;

/// `SMSG_SPELL_GO` (handlers `0x80fee0`, `0x80e1b0`): the cast launched. Guids, count and spell as
/// in [`SpellStart`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpellGo {
    pub item_or_caster: u64,
    pub caster: u64,
    pub cast_count: u8,
    pub spell_id: u32,
    pub cast_flags: u32,
    pub hits: Vec<u64>,
    /// `(guid, SpellMissInfo)`.
    pub misses: Vec<(u64, u8)>,
    /// `(guid, outcome)` of each reflect miss, the reflected spell's result on the caster.
    pub reflects: Vec<(u64, u8)>,
    pub targets: SpellCastTargets,
    pub ammo_display_id: Option<u32>,
    /// The `CAST_FLAG_RUNE_LIST` block.
    pub runes: Option<CastRunes>,
}

/// Read `SMSG_SPELL_GO`: the shared head, `u32` timestamp, the hit and miss lists, the targets,
/// then the tail `0x80e30c`-`0x80e463` reads: power, runes, missile, ammo, visual chain, a dest
/// byte and extra targets.
pub(super) fn read_spell_go(r: &mut impl Read) -> io::Result<SpellGo> {
    let item_or_caster = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let cast_count = read_u8(r)?;
    let spell_id = read_u32_le(r)?;
    let cast_flags = read_u32_le(r)?;
    let _timestamp = read_u32_le(r)?;

    // Both counts are `u8`s; no tighter bound.
    let hit_count = read_u8(r)?;
    let mut hits = Vec::with_capacity(capacity_hint(hit_count, usize::from(u8::MAX)));
    for _ in 0..hit_count {
        hits.push(read_u64_le(r)?); // raw guid, never packed
    }
    let miss_count = read_u8(r)?;
    let mut misses = Vec::with_capacity(capacity_hint(miss_count, usize::from(u8::MAX)));
    let mut reflects = Vec::new();
    for _ in 0..miss_count {
        let guid = read_u64_le(r)?;
        let reason = read_u8(r)?;
        if reason == SPELL_MISS_REFLECT {
            reflects.push((guid, read_u8(r)?));
        }
        misses.push((guid, reason));
    }

    let targets = read_spell_cast_targets(r)?;
    if cast_flags & CAST_FLAG_POWER_LEFT_SELF != 0 {
        let _power = read_u32_le(r)?;
    }
    let runes = if cast_flags & CAST_FLAG_RUNE_LIST != 0 {
        Some(read_runes(r)?)
    } else {
        None
    };
    if cast_flags & CAST_FLAG_ADJUST_MISSILE != 0 {
        let _elevation = read_u32_le(r)?;
        let _delay_ms = read_u32_le(r)?;
    }
    let ammo_display_id = if cast_flags & CAST_FLAG_AMMO != 0 {
        Some(read_ammo(r)?)
    } else {
        None
    };
    if cast_flags & CAST_FLAG_VISUAL_CHAIN != 0 {
        let _ = read_u32_le(r)?;
        let _ = read_u32_le(r)?;
    }
    if targets.mask & TARGET_FLAG_DEST_LOCATION != 0 {
        let _dest_byte = read_u8(r)?;
    }
    if targets.mask & TARGET_FLAG_EXTRA_TARGETS != 0 {
        // `u32` count of a point and a raw guid each; unbounded, so nothing is pre-allocated.
        for _ in 0..read_u32_le(r)? {
            let _point = Vector3d::read(r)?;
            let _guid = read_u64_le(r)?;
        }
    }
    Ok(SpellGo {
        item_or_caster,
        caster,
        cast_count,
        spell_id,
        cast_flags,
        hits,
        misses,
        reflects,
        targets,
        ammo_display_id,
        runes,
    })
}

/// Read `SMSG_SPELL_FAILURE` or `SMSG_SPELL_FAILED_OTHER` as their 12340 handlers do
/// (`0x809c70`, `0x806ad0`): packed caster, `u8 castCount`, `u32 spell`, `u8 reason`, the reason
/// returned in the shared numbering.
pub(super) fn read_spell_failed(r: &mut &[u8]) -> io::Result<(u64, u8, u32, u8)> {
    let caster = read_packed_guid(r)?;
    let cast_count = read_u8(r)?;
    let spell_id = read_u32_le(r)?;
    let reason = wotlk_cast_fail_shared(read_u8(r)?);
    Ok((caster, cast_count, spell_id, reason))
}

/// Read `SMSG_SPELL_DELAYED` (`0x801b80`): packed caster, `u32` pushback ms.
pub(super) fn read_spell_delayed(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let caster = read_packed_guid(r)?;
    let delay_ms = read_u32_le(r)?;
    Ok((caster, delay_ms))
}

/// Read `MSG_CHANNEL_START` (`0x801c90`): packed caster, `u32 spell`, `u32 duration`.
pub(super) fn read_channel_start(r: &mut impl Read) -> io::Result<(u64, u32, u32)> {
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let duration_ms = read_u32_le(r)?;
    Ok((caster, spell_id, duration_ms))
}

/// Read `MSG_CHANNEL_UPDATE` (`0x801db0`): packed caster, ms left; 0 ends the channel, whether it
/// finished or was interrupted.
pub(super) fn read_channel_update(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let caster = read_packed_guid(r)?;
    Ok((caster, read_u32_le(r)?))
}

/// Read `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER`: a `u8` `SpellFamilyFlags`
/// bit index (0..63), a `u8` SpellModOp (0..28), then the `i32` sum of every matching modifier.
/// The bit comes first: the client (`0x6e9950`) stores at `bit * 29 + op`, and vmangos
/// (`Player.cpp:17815`) sends one packet per set bit. The client range-checks neither byte.
pub(super) fn read_set_spell_modifier(r: &mut impl Read) -> io::Result<(u8, u8, i32)> {
    let mask_bit = read_u8(r)?;
    let op = read_u8(r)?;
    let value = read_i32_le(r)?;
    Ok((mask_bit, op, value))
}

/// Read `SMSG_UPDATE_AURA_DURATION` (vmangos `SpellAuras.cpp:7511-7523`): an aura slot and its ms
/// left. Sent only to the aura's target and never for a permanent aura, so an occupied slot with
/// no duration lasts until cancelled.
pub(super) fn read_update_aura_duration(r: &mut impl Read) -> io::Result<(u8, u32)> {
    let slot = read_u8(r)?;
    let remaining_ms = read_u32_le(r)?;
    Ok((slot, remaining_ms))
}

/// Read `SMSG_PLAY_SPELL_VISUAL` (vmangos `Packets/Spell.cpp:54-58`): raw `u64` unit, `u32` kit
/// id. The client (`0x6e98d0`) checks the kit against `SpellVisualKit.dbc` and plays it at stage 0.
pub(super) fn read_play_spell_visual(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let unit = read_u64_le(r)?;
    let kit_id = read_u32_le(r)?;
    Ok((unit, kit_id))
}

/// The client's `castCount`, which keys a sent cast to the server's answer: 12340 hands out 1 to
/// 255 and never 0, wrapping to 1 (`0x8050dc`-`0x8050ec`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CastCounter(u8);

impl CastCounter {
    /// The count the next cast carries; advances the counter.
    pub fn take(&mut self) -> u8 {
        let count = self.0.max(1);
        self.0 = count.wrapping_add(1);
        count
    }
}

/// The head of a 12340 `CMSG_CAST_SPELL` (sender `0x80b2f5`): `u8 castCount`, `u32 spellId`,
/// then the shared tail ([`put_cast_tail`]).
fn cast_head(cast_count: u8, spell_id: u32, mask: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(24);
    body.push(cast_count);
    body.extend_from_slice(&spell_id.to_le_bytes());
    put_cast_tail(&mut body, mask);
    body
}

/// The tail `CMSG_CAST_SPELL` and `CMSG_USE_ITEM` share from `0x80b351`: `u8 castFlags` (0: no
/// trajectory block), then the targets' `u32` mask (`0x9ab8c4`).
pub(super) fn put_cast_tail(body: &mut Vec<u8>, mask: u32) {
    body.push(0);
    body.extend_from_slice(&mask.to_le_bytes());
}

/// A guid target after the mask: one packed guid.
pub(super) fn put_target_guid(body: &mut Vec<u8>, guid: u64) {
    crate::wire::write_packed_guid(guid, body).expect("vec write");
}

/// A point target after the mask: a packed transport guid (0, none) and three `f32` WoW world
/// coords, as the writer puts them for SOURCE and DEST (`0x9ab924`, `0x9ab96c`).
pub(super) fn put_target_point(body: &mut Vec<u8>, point: [f32; 3]) {
    crate::wire::write_packed_guid(0, body).expect("vec write");
    for c in point {
        body.extend_from_slice(&c.to_le_bytes());
    }
}

/// A guid-target body: the head and one packed guid.
fn cast_at_guid(cast_count: u8, spell_id: u32, mask: u32, guid: u64) -> Vec<u8> {
    let mut body = cast_head(cast_count, spell_id, mask);
    put_target_guid(&mut body, guid);
    body
}

/// A point-target body: the head and the point.
fn cast_at_point(cast_count: u8, spell_id: u32, mask: u32, point: [f32; 3]) -> Vec<u8> {
    let mut body = cast_head(cast_count, spell_id, mask);
    put_target_point(&mut body, point);
    body
}

/// Body of `CMSG_CAST_SPELL` for a self or unit target. `None` sends mask 0 (`TARGET_FLAG_SELF`)
/// and the server applies the spell's implicit targeting; `Some` sends `TARGET_FLAG_UNIT` and the
/// packed guid.
pub fn cast_spell(cast_count: u8, spell_id: u32, target: Option<u64>) -> Vec<u8> {
    match target {
        None => cast_head(cast_count, spell_id, 0),
        Some(guid) => cast_at_guid(cast_count, spell_id, TARGET_FLAG_UNIT, guid),
    }
}

/// Body of `CMSG_CAST_SPELL` at a GameObject: the open-lock cast on a locked chest, vein or herb
/// (instead of `CMSG_GAMEOBJ_USE`), or a targeting-cursor commit. The mask is `GAMEOBJECT` alone.
pub fn cast_spell_gameobject(cast_count: u8, spell_id: u32, go_guid: u64) -> Vec<u8> {
    cast_at_guid(cast_count, spell_id, TARGET_FLAG_GAMEOBJECT, go_guid)
}

/// The corpse bit a corpse cast carries: `0x8000` for a corpse its caster's reaction toward is
/// friendly, `0x200` for one it is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpseTarget {
    /// `TARGET_FLAG_CORPSE_ALLY`, a resurrection's.
    Ally,
    /// `TARGET_FLAG_CORPSE_ENEMY`.
    Enemy,
}

impl CorpseTarget {
    /// The wire mask bit, which is also the flag-word bit the bind clears.
    pub const fn target_flag(self) -> u32 {
        match self {
            Self::Ally => TARGET_FLAG_CORPSE_ALLY,
            Self::Enemy => TARGET_FLAG_CORPSE_ENEMY,
        }
    }
}

/// Body of `CMSG_CAST_SPELL` at a corpse: the corpse bit and the corpse's packed guid.
pub fn cast_spell_corpse(
    cast_count: u8,
    spell_id: u32,
    target: CorpseTarget,
    corpse_guid: u64,
) -> Vec<u8> {
    cast_at_guid(cast_count, spell_id, target.target_flag(), corpse_guid)
}

/// Body of `CMSG_CAST_SPELL` at an item, such as an enchant or poison: `TARGET_FLAG_ITEM` and the
/// item's packed guid.
pub fn cast_spell_item(cast_count: u8, spell_id: u32, item_guid: u64) -> Vec<u8> {
    cast_at_guid(cast_count, spell_id, TARGET_FLAG_ITEM, item_guid)
}

/// Body of `CMSG_CAST_SPELL` at a glyph socket: `TARGET_FLAG_GLYPH_SLOT` alone, as the spell opcode
/// carries no glyph index (`0x80b2d6` writes it on the item opcode only).
pub fn cast_spell_glyph(cast_count: u8, spell_id: u32) -> Vec<u8> {
    cast_head(cast_count, spell_id, TARGET_FLAG_GLYPH_SLOT)
}

/// Body of `CMSG_CAST_SPELL` at a ground point, for a `Targets & 0x40` spell such as Blizzard.
pub fn cast_spell_at_dest(cast_count: u8, spell_id: u32, dest: [f32; 3]) -> Vec<u8> {
    cast_at_point(cast_count, spell_id, TARGET_FLAG_DEST_LOCATION, dest)
}

/// Body of `CMSG_CAST_SPELL` at a source point, for a `Targets & 0x20` spell; the server centres
/// the area effect there.
pub fn cast_spell_at_source(cast_count: u8, spell_id: u32, src: [f32; 3]) -> Vec<u8> {
    cast_at_point(cast_count, spell_id, TARGET_FLAG_SOURCE_LOCATION, src)
}

/// Body of `CMSG_CANCEL_AURA`: by spell id, not slot. `HandleCancelAuraOpcode`
/// (`SpellHandler.cpp:333-405`) refuses passives, debuffs and `SPELL_ATTR_NO_AURA_CANCEL` spells;
/// the client gates on the aura's `AURA_FLAG_CANCELABLE` bit.
pub fn cancel_aura(spell_id: u32) -> Vec<u8> {
    spell_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_DISMISS_CRITTER`: the `u64` guid of the companion out (`0x80305b`).
pub fn dismiss_critter(critter_guid: u64) -> Vec<u8> {
    critter_guid.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_reasons_map_onto_the_shared_numbering() {
        // NOT_READY: the failure handler's own special case (`0x809bd5 cmp 0x43`), 1.12.1's 0x3c.
        assert_eq!(WOTLK_CAST_FAIL_KEYS[0x43], "SPELL_FAILED_NOT_READY");
        assert_eq!(wotlk_cast_fail_shared(0x43), 0x3c);
        // DONT_REPORT, 1.12.1's 0x17.
        assert_eq!(wotlk_cast_fail_shared(0x1b), 0x17);
        // 3.3.5a's first own reasons land past 1.12.1's 0x91.
        assert_eq!(wotlk_cast_fail_shared(0), 0x92, "SUCCESS");
        assert_eq!(wotlk_cast_fail_shared(3), 0x93, "ALREADY_AT_FULL_MANA");
        // Past the table: UNKNOWN.
        assert_eq!(wotlk_cast_fail_shared(0xbb), 0x91);
        assert_eq!(wotlk_cast_fail_shared(0xff), 0x91);
    }

    #[test]
    fn the_cast_counter_runs_one_to_255_and_skips_zero() {
        let mut c = CastCounter::default();
        assert_eq!((c.take(), c.take()), (1, 2));
        let mut c = CastCounter(255);
        assert_eq!((c.take(), c.take()), (255, 1), "0 is never handed out");
    }

    #[test]
    fn cast_spell_gameobject_body_golden() {
        // count 3, spell 1, flags 0, mask 0x800, packed guid 0x1234 (mask 0x03, bytes 34 12).
        assert_eq!(
            cast_spell_gameobject(3, 1, 0x1234),
            [3, 0x01, 0, 0, 0, 0, 0x00, 0x08, 0, 0, 0x03, 0x34, 0x12],
            "CMSG_CAST_SPELL (GameObject/OPEN_LOCK) body"
        );
        // The self-cast shape: mask 0, no guid.
        assert_eq!(cast_spell(1, 1, None), [1, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// Resurrection (2006) at a released player's corpse, high guid 0xF101, counter 0x2A.
    #[test]
    fn cast_spell_corpse_body_golden() {
        const CORPSE: u64 = 0xF101_0000_0000_002A;
        assert_eq!(
            cast_spell_corpse(7, 2006, CorpseTarget::Ally, CORPSE),
            [
                7, // castCount
                0xD6, 0x07, 0x00, 0x00, // spell id 2006
                0,    // castFlags
                0x00, 0x80, 0x00, 0x00, // TARGET_FLAG_CORPSE_ALLY
                0xC1, 0x2A, 0x01, 0xF1, // packed guid: bytes 0, 6 and 7
            ],
            "CMSG_CAST_SPELL (ally corpse) body"
        );
        assert_eq!(
            cast_spell_corpse(7, 2006, CorpseTarget::Enemy, CORPSE)[7],
            0x02,
            "the enemy arm differs only in the mask"
        );
    }

    #[test]
    fn cast_spell_at_points_golden() {
        // Spell 10 is Blizzard: the mask, a packed transport guid of 0, then x, y, z.
        assert_eq!(
            cast_spell_at_dest(2, 10, [1.0, -2.5, 3.0]),
            [
                2, 0x0A, 0x00, 0x00, 0x00, 0, // count, spell, flags
                0x40, 0x00, 0x00, 0x00, // TARGET_FLAG_DEST_LOCATION
                0x00, // transport guid, none
                0x00, 0x00, 0x80, 0x3F, // x = 1.0
                0x00, 0x00, 0x20, 0xC0, // y = -2.5
                0x00, 0x00, 0x40, 0x40, // z = 3.0
            ],
            "CMSG_CAST_SPELL (ground dest) body"
        );
        let (src, dest) = (
            cast_spell_at_source(2, 265, [1.0, -2.5, 3.0]),
            cast_spell_at_dest(2, 265, [1.0, -2.5, 3.0]),
        );
        assert_eq!(src.len(), dest.len());
        assert_eq!(
            src.iter().zip(&dest).filter(|(a, b)| a != b).count(),
            1,
            "source and dest bodies differ only in the mask"
        );
    }

    /// AzerothCore's login `SMSG_SPELL_GO` for spell 836, captured live: self-cast, one hit,
    /// a unit mask with an empty guid.
    #[test]
    fn spell_go_reads_a_live_body() {
        let body = [
            0x03, 0xee, 0x03, 0x03, 0xee, 0x03, 0x00, 0x44, 0x03, 0x00, 0x00, 0x01, 0x01, 0x04,
            0x00, 0x5d, 0x45, 0xef, 0x13, 0x01, 0xee, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut r = &body[..];
        let go = read_spell_go(&mut r).unwrap();
        assert!(r.is_empty(), "{} bytes left", r.len());
        assert_eq!((go.item_or_caster, go.caster), (0x3ee, 0x3ee));
        assert_eq!(
            (go.cast_count, go.spell_id, go.cast_flags),
            (0, 836, 0x40101)
        );
        assert_eq!(go.hits, vec![0x3ee]);
        assert!(go.misses.is_empty());
        assert_eq!(go.targets.mask, TARGET_FLAG_UNIT);
        assert_eq!(go.targets.unit_target, Some(0));
    }

    /// A reflect miss carries one more byte, the reflected spell's own outcome on the caster.
    #[test]
    fn spell_go_keeps_a_reflects_outcome() {
        let mut body = vec![0x01, 0x05, 0x01, 0x05, 0];
        body.extend(133u32.to_le_bytes());
        body.extend(0u32.to_le_bytes()); // cast flags
        body.extend(0u32.to_le_bytes()); // timestamp
        body.push(0); // no hits
        body.push(2); // two misses
        body.extend(0x10u64.to_le_bytes());
        body.extend([SPELL_MISS_REFLECT, 7]);
        body.extend(0x11u64.to_le_bytes());
        body.push(3);
        body.extend(0u32.to_le_bytes()); // targets: none
        let mut r = &body[..];
        let go = read_spell_go(&mut r).unwrap();
        assert!(r.is_empty(), "{} bytes left", r.len());
        assert_eq!(go.misses, vec![(0x10, SPELL_MISS_REFLECT), (0x11, 3)]);
        assert_eq!(go.reflects, vec![(0x10, 7)]);
    }

    /// Icy Touch (45477) spending frost rune 4: only runes ready before and spent after carry a
    /// progress byte (`0x806799`-`0x8067aa`), and the ammo after the block still lines up.
    #[test]
    fn spell_start_keeps_the_rune_block() {
        let mut body = vec![0x01, 0x05, 0x01, 0x05, 2];
        body.extend(45477u32.to_le_bytes());
        body.extend(
            (CAST_FLAG_POWER_LEFT_SELF | CAST_FLAG_RUNE_LIST | CAST_FLAG_AMMO).to_le_bytes(),
        );
        body.extend(0u32.to_le_bytes());
        body.extend(0u32.to_le_bytes()); // targets: none
        body.extend(0u32.to_le_bytes()); // power left
        body.extend([0x3b, 0x2b, 17]); // rune 3 already spent, rune 4 spent now
        body.extend(5996u32.to_le_bytes());
        body.extend(24u32.to_le_bytes());
        let mut r = &body[..];
        let start = read_spell_start(&mut r).unwrap();
        assert!(r.is_empty(), "{} bytes left", r.len());
        assert_eq!(
            start.runes,
            Some(CastRunes {
                before: 0x3b,
                after: 0x2b,
                progress: [0, 0, 0, 0, 17, 0],
            })
        );
        assert_eq!(start.ammo_display_id, Some(5996));
    }

    #[test]
    fn spell_start_reads_power_and_ammo_tails() {
        // Auto Shot (75), count 4, flags POWER_LEFT_SELF | AMMO, 0 ms, at unit 0x10.
        let mut body = vec![0x01, 0x05, 0x01, 0x05, 4];
        body.extend(75u32.to_le_bytes());
        body.extend((CAST_FLAG_POWER_LEFT_SELF | CAST_FLAG_AMMO).to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend(TARGET_FLAG_UNIT.to_le_bytes());
        body.extend([0x01, 0x10]);
        body.extend(100u32.to_le_bytes()); // power left
        body.extend(5996u32.to_le_bytes()); // ammo display
        body.extend(24u32.to_le_bytes()); // inventory type
        let mut r = &body[..];
        let start = read_spell_start(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((start.cast_count, start.spell_id), (4, 75));
        assert_eq!(start.targets.unit_target, Some(0x10));
        assert_eq!(start.ammo_display_id, Some(5996));
    }

    /// The four observer packets lead with a packed caster guid (mask `0x01`, one byte `0x10`).
    #[test]
    fn the_observer_packets_carry_a_packed_caster() {
        // Count 5, Lesser Heal, wire 0x28 INTERRUPTED: shared 0x23.
        let body = [0x01, 0x10, 5, 0x02, 0x08, 0, 0, 0x28];
        let mut r = &body[..];
        let (caster, count, spell, reason) = read_spell_failed(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((caster, count, spell), (0x10, 5, 2050));
        assert_eq!(WOTLK_CAST_FAIL_KEYS[0x28], "SPELL_FAILED_INTERRUPTED");
        assert_eq!(reason, wotlk_cast_fail_shared(0x28));

        let body = [0x01, 0x10, 0xf4, 0x01, 0, 0];
        assert_eq!(read_spell_delayed(&mut &body[..]).unwrap(), (0x10, 500));
        let body = [0x01, 0x10, 0x0a, 0, 0, 0, 0x40, 0x1f, 0, 0];
        assert_eq!(
            read_channel_start(&mut &body[..]).unwrap(),
            (0x10, 10, 8000)
        );
        let body = [0x01, 0x10, 0, 0, 0, 0];
        assert_eq!(read_channel_update(&mut &body[..]).unwrap(), (0x10, 0));
    }

    #[test]
    fn cast_failed_has_no_status_byte() {
        // count 9, Raptor Strike, wire 0x0c BAD_TARGETS (AzerothCore, no target): shared 0x0a.
        let body = [9, 0x9d, 0x0b, 0, 0, 0x0c];
        let mut r = &body[..];
        let failed = |reason, arg| CastOutcome::Failed { reason, arg };
        assert_eq!(
            read_cast_result(&mut r).unwrap(),
            (9, 2973, failed(0x0a, None))
        );
        assert!(r.is_empty());
        // Wire 0x66 REQUIRES_SPELL_FOCUS with its focus id: shared 0x5e.
        let body = [9, 133, 0, 0, 0, 0x66, 3, 0, 0, 0];
        assert_eq!(
            read_cast_result(&mut &body[..]).unwrap(),
            (9, 133, failed(0x5e, Some(3)))
        );
    }

    #[test]
    fn aura_bodies_golden() {
        // CMSG_CANCEL_AURA: 1126 (Mark of the Wild) = 0x0000_0466.
        assert_eq!(cancel_aura(1126), [0x66, 0x04, 0x00, 0x00]);

        // SMSG_UPDATE_AURA_DURATION: u8 slot, then u32 ms LE. Slot 3, 12_000 ms = 0x0000_2EE0.
        let body = [0x03, 0xE0, 0x2E, 0x00, 0x00];
        let mut r = &body[..];
        assert_eq!(read_update_aura_duration(&mut r).unwrap(), (3, 12_000));
        assert!(
            r.is_empty(),
            "the body is exactly 5 bytes — slot is a byte, not a dword"
        );
    }

    #[test]
    fn set_spell_modifier_body_golden() {
        // mask_bit 35 (0x23), op 14 (0x0e, SPELLMOD_COST), value -30 (0xFFFF_FFE2 LE).
        let body = [0x23, 0x0E, 0xE2, 0xFF, 0xFF, 0xFF];
        let mut r = &body[..];
        assert_eq!(read_set_spell_modifier(&mut r).unwrap(), (35, 14, -30));
        assert!(
            r.is_empty(),
            "the body is exactly 6 bytes — two bytes and a dword, never three dwords"
        );

        // 35 is a legal mask bit (0..=63) but not a legal op (0..=28), so a swap would show.
        let (mask_bit, op, _) = read_set_spell_modifier(&mut &body[..]).unwrap();
        assert!(
            mask_bit >= 29,
            "field 1 would be an out-of-range op if swapped"
        );
        assert!(op < 29, "field 2 is a legal op as read");

        // Both bytes are unsigned, the value signed; the consumer must refuse bit 255.
        assert_eq!(
            read_set_spell_modifier(&mut &[0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00][..]).unwrap(),
            (255, 255, 1)
        );
    }
}

/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS`: a beam's hop list (vmangos `Spell.cpp:4970-4997`, client
/// `0x6e9820`). vmangos sends it only for channeled spells; the 1.12 client also fills the same
/// hop array from `SMSG_SPELL_GO`'s hits (`0x6e800d`), which is how Chain Lightning draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellChainTargets {
    pub caster: u64,
    pub spell_id: u32,
    /// In wire order. The client (`0x605780`) skips any entry equal to the caster; that filter is
    /// the consumer's.
    pub targets: Vec<u64>,
}

/// Read `SMSG_SPELL_UPDATE_CHAIN_TARGETS`; every guid in it is a raw `u64`, never packed.
pub(super) fn read_spell_chain_targets(r: &mut impl Read) -> io::Result<SpellChainTargets> {
    let caster = read_u64_le(r)?;
    let spell_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // The `u32` count is unbounded, so nothing is pre-allocated from it.
    let mut targets = Vec::new();
    for _ in 0..count {
        targets.push(read_u64_le(r)?);
    }
    Ok(SpellChainTargets {
        caster,
        spell_id,
        targets,
    })
}
