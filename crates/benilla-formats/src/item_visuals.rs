//! The item and enchant glow chain. An ItemVisuals id comes from the item's display
//! (`ItemDisplayInfo` column 22) or its enchant (`SpellItemEnchantment` column 31); its row holds
//! five `ItemVisualEffects` ids, each naming a glow model, and slot `i` hangs on the item model's
//! attachment `i` (the reference attaches each as `(glow, item, i)`). The loaders check each
//! table's field count and record size.
//!
//! The reference skips a visual or effect id that is negative, past the table's max id or a null
//! row, and an empty model path; ItemVisuals row 28 ships two garbage effect ids. The catalog
//! applies the same skips at load.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at, u32_at};

const ITEM_VISUALS: &str = "DBFilesClient\\ItemVisuals.dbc";
const ITEM_VISUAL_EFFECTS: &str = "DBFilesClient\\ItemVisualEffects.dbc";
const SPELL_ITEM_ENCHANTMENT: &str = "DBFilesClient\\SpellItemEnchantment.dbc";

/// An ItemVisuals row's effect slots, which are also the item model's attachment ids 0..4.
pub const ITEM_VISUAL_SLOTS: usize = 5;

/// ItemVisuals id to its five glow-model paths, raw `.mdx` as stored; `None` where the reference
/// skips the slot.
pub struct ItemVisualCatalog {
    visuals: HashMap<u32, [Option<String>; ITEM_VISUAL_SLOTS]>,
}

impl ItemVisualCatalog {
    /// The five slots for an ItemVisuals id, read signed as the reference does, so `-1` and `0`
    /// name nothing.
    pub fn effects(&self, visual_id: i32) -> Option<&[Option<String>; ITEM_VISUAL_SLOTS]> {
        (visual_id > 0).then(|| self.visuals.get(&(visual_id as u32)))?
    }

    /// Build from an explicit map, for tests and fixtures.
    pub fn from_visuals(visuals: HashMap<u32, [Option<String>; ITEM_VISUAL_SLOTS]>) -> Self {
        ItemVisualCatalog { visuals }
    }

    pub fn len(&self) -> usize {
        self.visuals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visuals.is_empty()
    }
}

/// `SpellItemEnchantment.dbc`'s visual, enUS name and `Flags`, one load for the glow chain, the
/// tooltip's enchant line and the bind confirms.
pub struct EnchantCatalog {
    visuals: HashMap<u32, i32>,
    names: HashMap<u32, String>,
    /// `Flags` for every row, 0 included, so its keys are the reference's `enchantTable[id] != 0`.
    flags: HashMap<u32, u32>,
    /// `SrcItemID`, the gem a socket enchant came from, for the rows that name one.
    gem_items: HashMap<u32, u32>,
    /// The sockets a prismatic enchant adds: its first effect 8's `EffectPointsMin` (`0x708140`).
    prismatic: HashMap<u32, u32>,
    /// A gem enchant's activation condition (`ConditionID`, a meta gem's), by enchant id.
    conditions: HashMap<u32, EnchantCondition>,
    /// The skill and level an enchant asks of its wearer, for the rows that ask either.
    requirements: HashMap<u32, EnchantRequirement>,
}

/// An enchant's requirement: `RequiredSkillID` and `RequiredSkillRank`, with the skill's name
/// from `SkillLine.dbc`, and `MinLevel`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnchantRequirement {
    pub skill: u32,
    pub skill_rank: u32,
    /// `None` when the skill has no row; the tooltip then prints "UNKNOWN".
    pub skill_name: Option<String>,
    pub min_level: u32,
}

/// One `SpellItemEnchantmentCondition.dbc` row: up to five tests on the equipped gems' colour
/// counts, read in order while `logic` is nonzero (`0x61c0b0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnchantCondition {
    pub tests: [EnchantConditionTest; 5],
}

/// One test: the left colour (`1` meta .. `4` blue), the operator, and a right colour or, with a
/// right type of 0, a count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnchantConditionTest {
    pub left_type: u8,
    pub operator: u8,
    pub right_type: u8,
    pub right_value: i32,
    pub logic: u8,
}

/// `Flags & 0x1`: applying the enchant binds the item. It is the only gate on the bind confirm,
/// event 402, and the reference reads it to ask whether an item already has one.
const FLAG_BINDS_THE_ITEM: u32 = 0x1;

/// `Flags & 0x2`: both tooltip enchant-line printers return before reading the name, though the
/// replace confirm still reads it. Set on the totem imbues, Firestone and Orb of Fire.
const FLAG_TOOLTIP_HIDES_NAME: u32 = 0x2;

impl EnchantCatalog {
    /// The ItemVisuals id the enchant glows with; signed, as one shipped row carries `-1`.
    pub fn visual(&self, enchant_id: u32) -> Option<i32> {
        self.visuals.get(&enchant_id).copied()
    }

    /// The display name as stored (`"Agility +15"`, `"Crusader"`); `None` when empty.
    pub fn name(&self, enchant_id: u32) -> Option<&str> {
        self.names.get(&enchant_id).map(String::as_str)
    }

    /// Whether applying the enchant binds the item ([`FLAG_BINDS_THE_ITEM`]); `false` for an
    /// unknown id, as the reference skips a missing row.
    pub fn binds_the_item(&self, enchant_id: u32) -> bool {
        self.flag(enchant_id, FLAG_BINDS_THE_ITEM)
    }

    /// Whether the item tooltip prints no line for the enchant ([`FLAG_TOOLTIP_HIDES_NAME`]).
    pub fn tooltip_hides_name(&self, enchant_id: u32) -> bool {
        self.flag(enchant_id, FLAG_TOOLTIP_HIDES_NAME)
    }

    fn flag(&self, enchant_id: u32, bit: u32) -> bool {
        self.flags.get(&enchant_id).is_some_and(|f| f & bit != 0)
    }

    /// Whether the id names a row, the reference's null test after each `enchantTable[id]`; a
    /// missing row raises no confirm.
    pub fn has_row(&self, enchant_id: u32) -> bool {
        self.flags.contains_key(&enchant_id)
    }

    /// The gem item a socket enchant names (`SrcItemID`, `GetInventoryItemGems` `0x5ea4f0`).
    pub fn gem_item(&self, enchant_id: u32) -> Option<u32> {
        self.gem_items.get(&enchant_id).copied()
    }

    /// The skill and level the enchant asks, `None` when it asks neither.
    pub fn requirement(&self, enchant_id: u32) -> Option<&EnchantRequirement> {
        self.requirements.get(&enchant_id)
    }

    /// A gem enchant's condition, for one that names a gem (`SrcItemID`) and a condition row.
    pub fn condition(&self, enchant_id: u32) -> Option<&EnchantCondition> {
        self.conditions.get(&enchant_id)
    }

    /// The sockets the enchant adds, 0 for one with no prismatic effect (8).
    pub fn prismatic_sockets(&self, enchant_id: u32) -> u32 {
        self.prismatic.get(&enchant_id).copied().unwrap_or(0)
    }

    /// Build from explicit maps, for tests; `flags` is also the row set [`Self::has_row`] reads.
    pub fn from_rows(
        visuals: HashMap<u32, i32>,
        names: HashMap<u32, String>,
        flags: HashMap<u32, u32>,
    ) -> Self {
        EnchantCatalog {
            visuals,
            names,
            flags,
            gem_items: HashMap::new(),
            prismatic: HashMap::new(),
            conditions: HashMap::new(),
            requirements: HashMap::new(),
        }
    }

    /// `(enchant id, ItemVisuals id)` for the rows that carry one, in no order.
    pub fn iter_visuals(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.visuals.iter().map(|(k, v)| (*k, *v))
    }

    /// How many enchants carry a glow.
    pub fn visual_count(&self) -> usize {
        self.visuals.len()
    }

    /// How many enchants carry a name.
    pub fn name_count(&self) -> usize {
        self.names.len()
    }
}

/// `ItemVisuals.dbc`: 6 fields, 24-byte records.
pub(crate) fn item_visuals_schema() -> Schema {
    let mut s = Schema::new("ItemVisuals");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..ITEM_VISUAL_SLOTS {
        s.add_field(SchemaField::new(
            format!("Effect{i}"),
            FieldType::UInt32, // read signed, as the reference does
        ));
    }
    s
}

/// `ItemVisualEffects.dbc`: 2 fields, 8-byte records.
pub(crate) fn item_visual_effects_schema() -> Schema {
    let mut s = Schema::new("ItemVisualEffects");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Model", FieldType::String));
    s
}

/// `SpellItemEnchantment.dbc`: 38 columns, 152-byte records, as the 12340 reader (`0x8b7de0`) takes
/// them: id, charges, the four effect arrays, the 17-column name, then the visual, `Flags` and five
/// requirement columns. In memory the name collapses to one pointer, so the visual sits at `+0x3c`
/// (`0x7590e6`) and `Flags` at `+0x40` (`0x52124b` tests bit 0, `0x7510ac` bit 1).
pub(crate) fn spell_item_enchantment_schema() -> Schema {
    let mut s = Schema::new("SpellItemEnchantment");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Charges", FieldType::UInt32));
    for group in ["Effect", "EffectPointsMin", "EffectPointsMax", "EffectArg"] {
        for i in 0..3 {
            s.add_field(SchemaField::new(format!("{group}{i}"), FieldType::UInt32));
        }
    }
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("ItemVisual", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    for name in [
        "SrcItemID",
        "ConditionID",
        "RequiredSkillID",
        "RequiredSkillRank",
        "MinLevel",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// `SpellItemEnchantment` columns benilla reads: the enUS name, the visual, `Flags` and the gem.
const ENCHANT_NAME: usize = 14;
const ENCHANT_VISUAL: usize = 31;
const ENCHANT_FLAGS: usize = 32;
const ENCHANT_SRC_ITEM: usize = 33;
const ENCHANT_CONDITION: usize = 34;
const ENCHANT_REQUIRED_SKILL: usize = 35;
const ENCHANT_REQUIRED_SKILL_RANK: usize = 36;
const ENCHANT_MIN_LEVEL: usize = 37;
const SPELL_ITEM_ENCHANTMENT_CONDITION: &str = "DBFilesClient\\SpellItemEnchantmentCondition.dbc";

/// `SpellItemEnchantmentCondition.dbc`: 31 columns packed in 64 bytes, the id then five each of
/// the left type (u8), left operand (u32), operator (u8), right type (u8), right operand (u32) and
/// logic (u8).
pub(crate) fn spell_item_enchantment_condition_schema() -> Schema {
    let mut s = Schema::new("SpellItemEnchantmentCondition");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for (group, ty) in [
        ("LtOperandType", FieldType::UInt8),
        ("LtOperand", FieldType::UInt32),
        ("Operator", FieldType::UInt8),
        ("RtOperandType", FieldType::UInt8),
        ("RtOperand", FieldType::UInt32),
        ("Logic", FieldType::UInt8),
    ] {
        for i in 0..5 {
            s.add_field(SchemaField::new(format!("{group}{i}"), ty));
        }
    }
    s
}

/// The condition rows by id; an unreadable file leaves every gem unconditioned.
fn load_conditions(chain: &mut Chain) -> HashMap<u32, EnchantCondition> {
    let Ok(bytes) = chain.read_file(SPELL_ITEM_ENCHANTMENT_CONDITION) else {
        return HashMap::new();
    };
    let Ok(rs) = parse(
        &bytes,
        spell_item_enchantment_condition_schema(),
        "SpellItemEnchantmentCondition",
    ) else {
        return HashMap::new();
    };
    let byte = |r, i| u32_at(r, i).unwrap_or(0) as u8;
    rs.records()
        .iter()
        .filter_map(|r| {
            let id = u32_at(r, 0)?;
            let tests = std::array::from_fn(|i| EnchantConditionTest {
                left_type: byte(r, 1 + i),
                operator: byte(r, 11 + i),
                right_type: byte(r, 16 + i),
                right_value: i32_at(r, 21 + i).unwrap_or(0),
                logic: byte(r, 26 + i),
            });
            Some((id, EnchantCondition { tests }))
        })
        .collect()
}
/// The prismatic-socket effect type.
const ENCHANT_PRISMATIC_SOCKET: u32 = 8;

/// Load and join `ItemVisuals.dbc` and `ItemVisualEffects.dbc`, applying the reference's skips.
pub fn load_item_visual_catalog(chain: &mut Chain) -> Result<ItemVisualCatalog> {
    let bytes = chain
        .read_file(ITEM_VISUAL_EFFECTS)
        .with_context(|| format!("reading {ITEM_VISUAL_EFFECTS}"))?;
    let rs = parse(&bytes, item_visual_effects_schema(), "ItemVisualEffects")?;
    // `str_at` drops an empty path, as the reference skips one.
    let mut effects: HashMap<u32, String> = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(model)) = (u32_at(r, 0), str_at(&rs, r, 1)) else {
            continue;
        };
        effects.insert(id, model);
    }

    let bytes = chain
        .read_file(ITEM_VISUALS)
        .with_context(|| format!("reading {ITEM_VISUALS}"))?;
    let rs = parse(&bytes, item_visuals_schema(), "ItemVisuals")?;
    let mut visuals = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let slots = std::array::from_fn(|i| {
            let raw = u32_at(r, 1 + i).unwrap_or(0) as i32;
            (raw > 0)
                .then(|| effects.get(&(raw as u32)).cloned())
                .flatten()
        });
        visuals.insert(id, slots);
    }
    Ok(ItemVisualCatalog { visuals })
}

/// Load the [`EnchantCatalog`] from `SpellItemEnchantment.dbc`.
pub fn load_enchant_catalog(chain: &mut Chain) -> Result<EnchantCatalog> {
    let bytes = chain
        .read_file(SPELL_ITEM_ENCHANTMENT)
        .with_context(|| format!("reading {SPELL_ITEM_ENCHANTMENT}"))?;
    let rs = parse(
        &bytes,
        spell_item_enchantment_schema(),
        "SpellItemEnchantment",
    )?;
    let mut visuals = HashMap::new();
    let mut names = HashMap::new();
    let mut flags = HashMap::with_capacity(rs.records().len());
    let condition_rows = load_conditions(chain);
    let skill_names = crate::skill_lines::load_skill_names(chain);
    let mut requirements = HashMap::new();
    let mut gem_items = HashMap::new();
    let mut prismatic = HashMap::new();
    let mut conditions = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // Effects at columns 2..5, their minimum points at 5..8.
        if let Some(i) = (0..3).find(|&i| u32_at(r, 2 + i) == Some(ENCHANT_PRISMATIC_SOCKET)) {
            prismatic.insert(id, u32_at(r, 5 + i).unwrap_or(0));
        }
        // Every row, `Flags == 0` included: the keys are the row set.
        flags.insert(id, u32_at(r, ENCHANT_FLAGS).unwrap_or(0));
        let skill = u32_at(r, ENCHANT_REQUIRED_SKILL).unwrap_or(0);
        let min_level = u32_at(r, ENCHANT_MIN_LEVEL).unwrap_or(0);
        if skill != 0 || min_level != 0 {
            requirements.insert(
                id,
                EnchantRequirement {
                    skill,
                    skill_rank: u32_at(r, ENCHANT_REQUIRED_SKILL_RANK).unwrap_or(0),
                    skill_name: skill_names.get(&skill).cloned(),
                    min_level,
                },
            );
        }
        let gem = u32_at(r, ENCHANT_SRC_ITEM).unwrap_or(0);
        if gem != 0 {
            gem_items.insert(id, gem);
            // Only a gem's enchant carries a condition (`0x61c0b0`).
            let row = u32_at(r, ENCHANT_CONDITION).unwrap_or(0);
            if let Some(c) = condition_rows.get(&row).filter(|_| row != 0) {
                conditions.insert(id, *c);
            }
        }
        let visual = u32_at(r, ENCHANT_VISUAL).unwrap_or(0) as i32;
        if visual != 0 {
            visuals.insert(id, visual);
        }
        // `str_at` drops an empty name.
        if let Some(name) = str_at(&rs, r, ENCHANT_NAME) {
            names.insert(id, name);
        }
    }
    Ok(EnchantCatalog {
        visuals,
        names,
        flags,
        gem_items,
        prismatic,
        conditions,
        requirements,
    })
}

#[cfg(test)]
mod tests {
    /// The meta gems' enchants carry their conditions.
    #[test]
    fn a_meta_gem_enchant_carries_its_condition() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_enchant_catalog(&mut chain).expect("load");
        assert!(
            cat.conditions.len() > 20,
            "{} conditioned gems",
            cat.conditions.len()
        );
        // Relentless Earthsiege Diamond (enchant 3628): at least one red, one yellow and one blue.
        let c = cat
            .condition(3628)
            .expect("Relentless carries its condition");
        let colours: Vec<(u8, u8, i32, u8)> = c.tests[..3]
            .iter()
            .map(|t| (t.left_type, t.operator, t.right_value, t.logic))
            .collect();
        assert_eq!(colours, [(2, 5, 1, 1), (3, 5, 1, 1), (4, 5, 1, 0)]);
        assert!(
            cat.prismatic.values().any(|&n| n == 1),
            "a prismatic enchant adds one socket"
        )
    }

    use super::*;

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn real_item_visuals_join_their_effect_models() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        assert_eq!(cat.len(), 34, "34 ItemVisuals rows in build 5875");

        let all_blue = ["Spells\\Enchantments\\BlueGlow_Med.mdx"; ITEM_VISUAL_SLOTS];
        let got = cat.effects(2).expect("visual 2");
        assert_eq!(
            got.each_ref().map(|s| s.as_deref().unwrap_or("")),
            all_blue,
            "visual 2 glows on every slot"
        );

        let one = cat.effects(1).expect("visual 1");
        assert_eq!(
            one[3].as_deref(),
            Some("Spells\\Enchantments\\SkullBalls.mdx")
        );
        assert!(
            [0, 1, 2, 4].iter().all(|&i| one[i].is_none()),
            "visual 1 authors only slot 3"
        );

        // A mixed row: slot 0 differs from the other four.
        let rune = cat.effects(30).expect("visual 30");
        assert_eq!(
            rune[0].as_deref(),
            Some("Spells\\Enchantments\\Rune_Intellect.mdx")
        );
        assert_eq!(
            rune[4].as_deref(),
            Some("Spells\\Enchantments\\YellowGlow_Low.mdx")
        );

        // Row 28: slots 0 and 3 hold 90148992 and 455344256, past the effect table's max id (152).
        let junk = cat.effects(28).expect("visual 28");
        assert_eq!(
            junk.each_ref().map(|s| s.is_some()),
            [false, false, false, false, true],
            "only the in-range slot-4 effect survives row 28"
        );
        assert_eq!(
            junk[4].as_deref(),
            Some("Spells\\Enchantments\\Sparkle_A.mdx")
        );

        // The id gate: 0 and the shipped -1 name nothing.
        assert!(cat.effects(0).is_none());
        assert!(cat.effects(-1).is_none());
        assert!(cat.effects(9999).is_none(), "past maxId");
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn real_displays_carrying_a_visual_all_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let visuals = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        let displays = crate::load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");

        let (mut carried, mut minus_one, mut resolved, mut models) = (0, 0, 0, 0);
        for (_, d) in displays.iter() {
            if d.item_visual == 0 {
                continue;
            }
            carried += 1;
            if d.item_visual == -1 {
                minus_one += 1;
            }
            match visuals.effects(d.item_visual) {
                Some(slots) => {
                    resolved += 1;
                    models += slots.iter().flatten().count();
                }
                None => assert_eq!(
                    d.item_visual, -1,
                    "the only unresolvable visual ids on the shipped table are -1"
                ),
            }
        }
        assert_eq!(carried, 365, "displays carrying a nonzero ItemVisuals id");
        assert_eq!(minus_one, 5, "…of which five are the skipped -1");
        assert_eq!(resolved, 360);
        assert_eq!(
            models, 1588,
            "glow-model instances the shipped displays add up to"
        );
    }

    /// Every shipped column-31 value but one `-1` names an ItemVisuals row, which pins the column.
    #[test]
    fn real_enchant_visuals_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let visuals = load_item_visual_catalog(&mut chain).expect("load ItemVisuals");
        let enchants = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");
        assert_eq!(enchants.visual_count(), 225, "enchants carrying a visual");

        let resolved = enchants
            .iter_visuals()
            .filter(|(_, v)| visuals.effects(*v).is_some())
            .count();
        assert_eq!(resolved, 224, "…all but the one -1 row resolve to a row");
        assert_eq!(enchants.visual(2605), Some(-1));

        // Rockbiter 3 (enchant 1) → visual 61 → the slot-3 rock glow.
        let rockbiter = visuals
            .effects(enchants.visual(1).expect("enchant 1 has a visual"))
            .expect("visual 61");
        assert_eq!(
            rockbiter[3].as_deref(),
            Some("Spells\\Enchantments\\Shaman_Rock.mdx")
        );
        // A sharpening stone (enchant 13) → visual 28 → the garbage row's surviving sparkle.
        let sharpened = visuals
            .effects(enchants.visual(13).expect("enchant 13 has a visual"))
            .expect("visual 28");
        assert_eq!(
            sharpened[4].as_deref(),
            Some("Spells\\Enchantments\\Sparkle_A.mdx")
        );
        // A plain +stat enchant carries none (241 "+2 Weapon Damage", 929 "+7 Stamina").
        assert_eq!(enchants.visual(241), None);
        assert_eq!(enchants.visual(929), None);
    }

    /// Names are stored in the table's own word order (`"+15 Agility"`), glowing or not.
    #[test]
    fn real_enchant_names_read_off_the_table() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let enchants = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");

        // 2564, a permanent weapon enchant: a name and a visual (125, GreenGlow_Low) on one row.
        assert_eq!(enchants.name(2564), Some("+15 Agility"));
        assert_eq!(enchants.visual(2564), Some(125));
        // Named, no glow: the +stat family.
        assert_eq!(enchants.name(241), Some("+2 Weapon Damage"));
        assert_eq!(enchants.name(929), Some("+7 Stamina"));
        assert_eq!(enchants.name(1900), Some("Crusader"));
        assert_eq!(enchants.name(999_999), None);
        assert!(
            enchants.name_count() > enchants.visual_count(),
            "far more enchants print a name than carry a glow"
        );
    }

    /// Both bits' rows are named one by one, so a column slip fails.
    #[test]
    fn real_enchant_flags_column() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let e = load_enchant_catalog(&mut chain).expect("load SpellItemEnchantment");

        // Bit 0, the bind confirm's gate: the shaman imbues, the rogue poisons, the head and leg
        // enchants, scopes.
        for (id, name) in [
            (1u32, "Rockbiter 3"),
            (7, "Deadly Poison"),
            (283, "Windfury 1"),
            (2488, "+5 All Resistances"),
            (2606, "+30 Attack Power"),
            (33, "Scope (+3 Damage)"),
        ] {
            assert!(e.binds_the_item(id), "{id} ({name}) must carry Flags & 1");
        }
        // Profession enchants and consumables (stones, oils) do not bind.
        for (id, name) in [
            (2564u32, "+15 Agility"),
            (1900, "Crusader"),
            (803, "Fiery Weapon"),
            (40, "Sharpened (+2 Damage)"),
            (2627, "Wizard Oil"),
        ] {
            assert!(
                !e.binds_the_item(id),
                "{id} ({name}) must NOT carry Flags & 1"
            );
        }

        // Bit 1, the tooltip line suppression: exactly seventeen rows.
        let hidden: std::collections::BTreeSet<u32> =
            (0..4000).filter(|&id| e.tooltip_hides_name(id)).collect();
        assert_eq!(
            hidden,
            [
                124, 285, 303, 543, 563, 564, 1683, 1783, 1803, 1823, 1824, 1825, 2637, 2638, 2639,
                2645, 3014
            ]
            .into_iter()
            .collect(),
            "the totem-granted imbues, Orb of Fire, the Firestones and their 3.3.5a kin"
        );
        // The name stays, for the replace confirm.
        assert_eq!(e.name(124), Some("Flametongue Totem 1"));

        // The row set the confirms gate on.
        assert!(e.has_row(2564) && e.has_row(1) && !e.has_row(999_999));
    }
}
