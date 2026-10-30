//! The item tooltip's vocabulary: the builder's key tables (`INVTYPE_*`, `ITEM_MOD_*`,
//! `SPELL_SCHOOL%d_CAP`), which [`super::render`] resolves against the player's own
//! `GlobalStrings.lua`, and its colours. Keys, not English: `INVTYPE_SHIELD` and
//! `INVTYPE_WEAPONOFFHAND` both read "Off Hand" in enUS and differ elsewhere. [`CLASS_NAMES`] and
//! [`RACE_NAMES`] are English literals; the reference reads `ChrClasses.dbc` and `ChrRaces.dbc`.

/// The reference's quality colour table, read by `GetItemQualityColor`.
pub(super) const QUALITY_RGB: [[f32; 3]; 7] = [
    [0.616, 0.616, 0.616], // 0 Poor
    [1.0, 1.0, 1.0],       // 1 Common    ffffff
    [0.118, 1.0, 0.0],     // 2 Uncommon
    [0.0, 0.439, 0.867],   // 3 Rare
    [0.639, 0.208, 0.933], // 4 Epic
    [1.0, 0.502, 0.0],     // 5 Legendary
    [0.902, 0.8, 0.502],   // 6 Artifact
];

// The tooltip colours: white ffffffff, red ffff2020, green ff00ff00, gold ffffd200, gray ff808080.
pub(super) const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(super) const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
pub(super) const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
/// The pure red (ffff0000), used by two enchant lines: a negative enchant id in slot 0 or 1, and
/// ITEM_ENCHANT_DISCLAIMER.
pub(super) const ENCHANT_RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
pub(super) const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
pub(super) const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// An equipped set member's cream, (ffffff97).
pub(super) const CREAM: [f32; 4] = [1.0, 1.0, 151.0 / 255.0, 1.0];

/// InventoryType to the slot line's key: the builder's 30-entry table, indexed by `[record+0x2c]`;
/// entries 0 and 29 are the empty string, here `None`. A bag (18) takes CONTAINER_SLOTS instead,
/// and class 6 its class name; 24-27 ship no `GlobalStrings.lua` value, so a thrown weapon (25) or
/// a gun (26) has an empty slot cell.
pub(super) fn invtype_key(t: u32) -> Option<&'static str> {
    Some(match t {
        1 => "INVTYPE_HEAD",
        2 => "INVTYPE_NECK",
        3 => "INVTYPE_SHOULDER",
        4 => "INVTYPE_BODY",
        5 => "INVTYPE_CHEST",
        6 => "INVTYPE_WAIST",
        7 => "INVTYPE_LEGS",
        8 => "INVTYPE_FEET",
        9 => "INVTYPE_WRIST",
        10 => "INVTYPE_HAND",
        11 => "INVTYPE_FINGER",
        12 => "INVTYPE_TRINKET",
        13 => "INVTYPE_WEAPON",
        14 => "INVTYPE_SHIELD",
        15 => "INVTYPE_RANGED",
        16 => "INVTYPE_CLOAK",
        17 => "INVTYPE_2HWEAPON",
        19 => "INVTYPE_TABARD",
        20 => "INVTYPE_ROBE",
        21 => "INVTYPE_WEAPONMAINHAND",
        22 => "INVTYPE_WEAPONOFFHAND",
        23 => "INVTYPE_HOLDABLE",
        24 => "INVTYPE_AMMO",
        25 => "INVTYPE_THROWN",
        26 => "INVTYPE_RANGEDRIGHT",
        28 => "INVTYPE_RELIC",
        _ => return None,
    })
}

/// The damage bias, 0.9999899864196777: an integral max is not bumped, and unlike a true `ceil()` a
/// value within ~1e-5 above an integer is not rounded up.
const DAMAGE_BIAS: f32 = f32::from_bits(0x3f7f_ff58);

/// The builder's `floor(DamageMin)`: subtract the bias unless positive, then truncate toward zero.
pub(super) fn floor_min(m: f32) -> i32 {
    (if m > 0.0 { m } else { m - DAMAGE_BIAS }) as i32
}

/// The builder's `ceil(DamageMax)`: add the bias if positive, then truncate.
pub(super) fn ceil_max(m: f32) -> i32 {
    (if m > 0.0 { m + DAMAGE_BIAS } else { m }) as i32
}

/// A school's name key, `SPELL_SCHOOL%d_CAP`, the one key the builder composes. School 0 has none:
/// a physical weapon takes the school-less `DAMAGE_TEMPLATE` arm.
pub(super) fn school_key(s: u32) -> Option<String> {
    (1..=6).contains(&s).then(|| format!("SPELL_SCHOOL{s}_CAP"))
}

/// Stat type to its `ITEM_MOD_*` line template (`"%c%d Agility"`); the builder's 8-way jump table
/// skips 2.
pub(super) fn stat_key(t: u32) -> Option<&'static str> {
    Some(match t {
        0 => "ITEM_MOD_MANA",
        1 => "ITEM_MOD_HEALTH",
        3 => "ITEM_MOD_AGILITY",
        4 => "ITEM_MOD_STRENGTH",
        5 => "ITEM_MOD_INTELLECT",
        6 => "ITEM_MOD_SPIRIT",
        7 => "ITEM_MOD_STAMINA",
        _ => return None,
    })
}

/// Stat types 12..=48, the green "Equip:" stats (order `0xa25f78`), to their `ITEM_MOD_*` key
/// (`0xad6640`); 40 has none.
pub(super) fn rating_key(t: u32) -> Option<&'static str> {
    Some(match t {
        12 => "ITEM_MOD_DEFENSE_SKILL_RATING",
        13 => "ITEM_MOD_DODGE_RATING",
        14 => "ITEM_MOD_PARRY_RATING",
        15 => "ITEM_MOD_BLOCK_RATING",
        16 => "ITEM_MOD_HIT_MELEE_RATING",
        17 => "ITEM_MOD_HIT_RANGED_RATING",
        18 => "ITEM_MOD_HIT_SPELL_RATING",
        19 => "ITEM_MOD_CRIT_MELEE_RATING",
        20 => "ITEM_MOD_CRIT_RANGED_RATING",
        21 => "ITEM_MOD_CRIT_SPELL_RATING",
        22 => "ITEM_MOD_HIT_TAKEN_MELEE_RATING",
        23 => "ITEM_MOD_HIT_TAKEN_RANGED_RATING",
        24 => "ITEM_MOD_HIT_TAKEN_SPELL_RATING",
        25 => "ITEM_MOD_CRIT_TAKEN_MELEE_RATING",
        26 => "ITEM_MOD_CRIT_TAKEN_RANGED_RATING",
        27 => "ITEM_MOD_CRIT_TAKEN_SPELL_RATING",
        28 => "ITEM_MOD_HASTE_MELEE_RATING",
        29 => "ITEM_MOD_HASTE_RANGED_RATING",
        30 => "ITEM_MOD_HASTE_SPELL_RATING",
        31 => "ITEM_MOD_HIT_RATING",
        32 => "ITEM_MOD_CRIT_RATING",
        33 => "ITEM_MOD_HIT_TAKEN_RATING",
        34 => "ITEM_MOD_CRIT_TAKEN_RATING",
        35 => "ITEM_MOD_RESILIENCE_RATING",
        36 => "ITEM_MOD_HASTE_RATING",
        37 => "ITEM_MOD_EXPERTISE_RATING",
        38 => "ITEM_MOD_ATTACK_POWER",
        39 => "ITEM_MOD_RANGED_ATTACK_POWER",
        41 => "ITEM_MOD_SPELL_HEALING_DONE",
        42 => "ITEM_MOD_SPELL_DAMAGE_DONE",
        43 => "ITEM_MOD_MANA_REGENERATION",
        44 => "ITEM_MOD_ARMOR_PENETRATION_RATING",
        45 => "ITEM_MOD_SPELL_POWER",
        46 => "ITEM_MOD_HEALTH_REGEN",
        47 => "ITEM_MOD_SPELL_PENETRATION",
        48 => "ITEM_MOD_BLOCK_VALUE",
        _ => return None,
    })
}

/// A socket colour mask's lowest of meta, red, yellow and blue (`0xad6704`), naming its
/// `EMPTY_SOCKET_*` key and its `UI-EmptySocket-*` icon.
pub(super) fn socket_color_name(color: u32) -> Option<&'static str> {
    ["Meta", "Red", "Yellow", "Blue"]
        .into_iter()
        .enumerate()
        .find_map(|(i, name)| (color & (1 << i) != 0).then_some(name))
}

/// The playable class ids and names the `ITEM_CLASSES_ALLOWED` list prints.
pub(super) const CLASS_NAMES: [(u32, &str); 9] = [
    (1, "Warrior"),
    (2, "Paladin"),
    (3, "Hunter"),
    (4, "Rogue"),
    (5, "Priest"),
    (7, "Shaman"),
    (8, "Mage"),
    (9, "Warlock"),
    (11, "Druid"),
];

/// The playable race ids and names the `ITEM_RACES_ALLOWED` list prints.
pub(super) const RACE_NAMES: [(u32, &str); 8] = [
    (1, "Human"),
    (2, "Orc"),
    (3, "Dwarf"),
    (4, "Night Elf"),
    (5, "Undead"),
    (6, "Tauren"),
    (7, "Gnome"),
    (8, "Troll"),
];

/// The mask of every listed id; an item allowing all of them shows no line.
pub(super) fn full_mask(ids: &[(u32, &str)]) -> i32 {
    ids.iter().fold(0i32, |m, &(id, _)| m | (1 << (id - 1)))
}

pub(super) fn quality_color(q: u32) -> [f32; 4] {
    let c = QUALITY_RGB.get(q as usize).unwrap_or(&QUALITY_RGB[1]);
    [c[0], c[1], c[2], 1.0]
}

pub(super) fn req_color(ok: bool) -> [f32; 4] {
    if ok {
        WHITE
    } else {
        RED
    }
}
