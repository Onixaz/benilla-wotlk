//! `ChrClasses.dbc`, the per-class table, as 12340 lays it out: 60 columns in 0xf0-byte records,
//! which the loader packs into 0x30-byte rows (`[0xad3420]`, count `[0xad340c]`, the class-indexed
//! pointers at `[0xad3424]`, max id `[0xad3410]`). The packed offsets below are the readers'.
//!
//! Field 1, the damage bonus stat: `GetDamageBonusStat` (`0x51afb0`) reads it at `row + 4` for the
//! active player's class byte and answers it plus one, a 1-based `UnitStat` index. Agility (1) for
//! the Hunter, Rogue and Shaman, Strength (0) for the rest but the Death Knight's shipped 9.
//!
//! Field 3, the pet name token: the second return of `HasPetSpells` (`0x53dd30`, `row + 0xc`),
//! `"PET"` on every row but the Warlock's `"DEMON"`. FrameXML resolves it with
//! `getglobal("PET_TYPE_"..token)`, so the token is a key, never display text, and is not
//! localized.
//!
//! Fields 4, 21 and 38, the enUS slots of `Name_lang`, `Name_female_lang` and `Name_male_lang`
//! (`row + 0x10`, `0x14`, `0x18`), and field 55, `Filename` (`row + 0x1c`): what
//! `FillLocalizedClassList` (`0x60a510`) writes, token to localized name.
//!
//! Field 56, the class spell family: a talent modifier applies to a spell only when its
//! `SpellFamilyName` equals the local player's. The shipped values are AzerothCore
//! `SpellFamilyNames`.
//!
//! Field 57, the class flags: bit `0x8` is the relic slot, read by `UnitHasRelicSlot`
//! (`0x611330`, `test [row + 0x24], 8`) and nothing else. It is set for Paladin, Shaman, Druid and
//! Death Knight, whose INVSLOT 17 takes a relic.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHR_CLASSES: &str = "DBFilesClient\\ChrClasses.dbc";

/// 12340's column count, which `benilla-dbc` enforces against the header.
const CHR_CLASSES_FIELDS: usize = 60;

/// Field 1, read at `row + 4` by `GetDamageBonusStat`.
const DAMAGE_BONUS_STAT_FIELD: usize = 1;

/// Field 3, read at `row + 0xc` by `HasPetSpells`.
const PET_NAME_TOKEN_FIELD: usize = 3;

/// The enUS slots of the three 17-column name blocks.
const NAME_FIELD: usize = 4;
const NAME_FEMALE_FIELD: usize = 21;
const NAME_MALE_FIELD: usize = 38;

/// Field 55, `Filename`, the class token (`"WARRIOR"`).
const FILENAME_FIELD: usize = 55;

/// Field 56, the class spell family.
const SPELL_FAMILY_FIELD: usize = 56;

/// Field 57, the class flags, and its relic-slot bit (`0x611395`).
const FLAGS_FIELD: usize = 57;
const FLAG_RELIC_SLOT: u32 = 0x8;

/// The literal the reference pushes when the player does not resolve. Every class but the Warlock
/// carries it too, so an unloaded table degrades invisibly.
pub const PET_NAME_TOKEN_FALLBACK: &str = "PET";

#[derive(Debug, Clone)]
struct ChrClass {
    damage_bonus_stat: u32,
    pet_name_token: Option<String>,
    name: Option<String>,
    has_relic_slot: bool,
    spell_family: u32,
}

/// One class's names as `FillLocalizedClassList` reads them; the gendered columns are empty in
/// enUS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChrClassNames {
    /// `Filename`, the class token.
    pub file: String,
    pub name: String,
    pub female: String,
    pub male: String,
}

/// `ChrClasses.dbc`'s read columns by class id, and the names in record order.
#[derive(Debug, Default, Clone)]
pub struct ChrClasses {
    by_id: HashMap<u32, ChrClass>,
    names: Vec<ChrClassNames>,
}

impl ChrClasses {
    /// `HasPetSpells`' second return: always a string, never nil, and the literal
    /// [`PET_NAME_TOKEN_FALLBACK`] for a class with no row.
    pub fn pet_name_token(&self, class: u32) -> &str {
        self.by_id
            .get(&class)
            .and_then(|c| c.pet_name_token.as_deref())
            .unwrap_or(PET_NAME_TOKEN_FALLBACK)
    }

    /// The class's `Name_Lang` name, `GetCharacterInfo`'s third return; `None` for a class with no
    /// row or no name.
    pub fn name(&self, class: u32) -> Option<&str> {
        self.by_id.get(&class)?.name.as_deref()
    }

    /// The class's damage bonus stat, 0-based (Strength 0, Agility 1); `None` for a class with
    /// no row, where `GetDamageBonusStat` answers 0.
    pub fn damage_bonus_stat(&self, class: u32) -> Option<u32> {
        self.by_id.get(&class).map(|c| c.damage_bonus_stat)
    }

    /// Whether the class's INVSLOT 17 is a relic slot, `UnitHasRelicSlot` whole. A class with no
    /// row answers false, as the reference's bound check against `[0xad3410]` falls to its nil leg.
    pub fn has_relic_slot(&self, class: u32) -> bool {
        self.by_id.get(&class).is_some_and(|c| c.has_relic_slot)
    }

    /// The class spell family, matched against a spell's `SpellFamilyName` before a talent
    /// modifier applies. A class with no row answers 0, which matches nothing.
    pub fn spell_family(&self, class: u32) -> u32 {
        self.by_id.get(&class).map_or(0, |c| c.spell_family)
    }

    /// Every row's names in record order, the walk `FillLocalizedClassList` makes.
    pub fn names(&self) -> &[ChrClassNames] {
        &self.names
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("ChrClasses");
    for i in 0..CHR_CLASSES_FIELDS {
        // The token, the three enUS name slots and the file name are strings; the rest, the other
        // locale slots included, stay opaque dwords.
        let ty = match i {
            PET_NAME_TOKEN_FIELD | NAME_FIELD | NAME_FEMALE_FIELD | NAME_MALE_FIELD
            | FILENAME_FIELD => FieldType::String,
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `ChrClasses.dbc`'s read columns from the patch chain.
pub fn load_chr_classes(chain: &mut Chain) -> Result<ChrClasses> {
    let bytes = chain
        .read_file(CHR_CLASSES)
        .with_context(|| format!("reading {CHR_CLASSES}"))?;
    let rs = parse(&bytes, schema(), "ChrClasses.dbc")?;
    let mut table = ChrClasses::default();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        table.by_id.insert(
            id,
            ChrClass {
                damage_bonus_stat: u32_at(r, DAMAGE_BONUS_STAT_FIELD).unwrap_or(0),
                pet_name_token: str_at(&rs, r, PET_NAME_TOKEN_FIELD),
                name: str_at(&rs, r, NAME_FIELD),
                has_relic_slot: u32_at(r, FLAGS_FIELD).is_some_and(|v| v & FLAG_RELIC_SLOT != 0),
                spell_family: u32_at(r, SPELL_FAMILY_FIELD).unwrap_or(0),
            },
        );
        let text = |field| str_at(&rs, r, field).unwrap_or_default();
        table.names.push(ChrClassNames {
            file: text(FILENAME_FIELD),
            name: text(NAME_FIELD),
            female: text(NAME_FEMALE_FIELD),
            male: text(NAME_MALE_FIELD),
        });
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<crate::chain::Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    /// Field 4 holds the class name, so a one-column slip would blank the spellbook tab silently.
    #[test]
    fn warlocks_pet_is_a_demon_and_everyone_elses_is_a_pet() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        assert!(!t.is_empty());
        assert_eq!(t.pet_name_token(9), "DEMON", "Warlock");
        for (class, who) in [
            (1, "Warrior"),
            (3, "Hunter"),
            (6, "Death Knight"),
            (11, "Druid"),
        ] {
            assert_eq!(t.pet_name_token(class), "PET", "{who}");
        }
        // 10 has no row; the reference's out-of-range arm answers the literal.
        assert_eq!(t.pet_name_token(10), PET_NAME_TOKEN_FALLBACK);
        assert_eq!(t.pet_name_token(0), PET_NAME_TOKEN_FALLBACK);
    }

    #[test]
    fn libram_totem_idol_and_sigil_are_the_four_relic_classes() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, who) in [
            (2, "Paladin"),
            (6, "Death Knight"),
            (7, "Shaman"),
            (11, "Druid"),
        ] {
            assert!(t.has_relic_slot(class), "{who} carries a relic");
        }
        for (class, who) in [
            (1, "Warrior"),
            (3, "Hunter"),
            (4, "Rogue"),
            (5, "Priest"),
            (8, "Mage"),
            (9, "Warlock"),
        ] {
            assert!(!t.has_relic_slot(class), "{who} wields a ranged weapon");
        }
        // No row: the reference's nil leg.
        assert!(!t.has_relic_slot(10));
        assert!(!t.has_relic_slot(0));
    }

    /// Hunters, Rogues and Shamans scale their damage off Agility, the rest off Strength; the
    /// Death Knight row ships 9, which `GetDamageBonusStat` pushes plus one, unchecked.
    #[test]
    fn the_damage_bonus_stat_column_reads_as_shipped() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, stat, who) in [
            (1, 0, "Warrior"),
            (2, 0, "Paladin"),
            (3, 1, "Hunter"),
            (4, 1, "Rogue"),
            (5, 0, "Priest"),
            (6, 9, "Death Knight"),
            (7, 1, "Shaman"),
            (8, 0, "Mage"),
            (9, 0, "Warlock"),
            (11, 0, "Druid"),
        ] {
            assert_eq!(t.damage_bonus_stat(class), Some(stat), "{who}");
        }
        assert_eq!(t.damage_bonus_stat(10), None);
    }

    /// The whole column pins field 56: ten distinct values matching AzerothCore
    /// `SpellFamilyNames` are a shape no neighbour has.
    #[test]
    fn every_class_row_carries_its_spell_family() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, family, who) in [
            (1u32, 4u32, "Warrior"),
            (2, 10, "Paladin"),
            (3, 9, "Hunter"),
            (4, 8, "Rogue"),
            (5, 6, "Priest"),
            (6, 15, "Death Knight"),
            (7, 11, "Shaman"),
            (8, 3, "Mage"),
            (9, 5, "Warlock"),
            (11, 7, "Druid"),
        ] {
            assert_eq!(t.spell_family(class), family, "{who}");
        }
        // No row: the reference's zeroed global, which the gate's first conjunct refuses.
        assert_eq!(t.spell_family(10), 0);
        assert_eq!(t.spell_family(0), 0);
    }

    /// `FillLocalizedClassList`'s walk: every row's token and enUS name in record order, the
    /// gendered columns empty.
    #[test]
    fn the_names_walk_every_class_in_record_order() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        let got: Vec<(&str, &str)> = t
            .names()
            .iter()
            .map(|n| (n.file.as_str(), n.name.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("WARRIOR", "Warrior"),
                ("PALADIN", "Paladin"),
                ("HUNTER", "Hunter"),
                ("ROGUE", "Rogue"),
                ("PRIEST", "Priest"),
                ("DEATHKNIGHT", "Death Knight"),
                ("SHAMAN", "Shaman"),
                ("MAGE", "Mage"),
                ("WARLOCK", "Warlock"),
                ("DRUID", "Druid"),
            ]
        );
        assert!(t
            .names()
            .iter()
            .all(|n| n.female.is_empty() && n.male.is_empty()));
    }
}
