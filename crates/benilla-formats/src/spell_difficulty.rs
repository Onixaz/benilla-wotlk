//! `SpellDifficulty.dbc`: a spell's four per-difficulty variants, by `Spell.dbc`'s column 233.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const SPELL_DIFFICULTY: &str = "DBFilesClient\\SpellDifficulty.dbc";

/// Every row's four variants, by id.
#[derive(Debug, Clone, Default)]
pub struct SpellDifficultyCatalog {
    rows: HashMap<u32, [u32; 4]>,
}

impl SpellDifficultyCatalog {
    /// Build from `(id, variants)` rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, [u32; 4])>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// `0x802850`'s row walk: `spell` swapped for its variant at `index`, the difficulty of a
    /// dungeon or raid map. An empty slot 3 falls to slot 1, then any empty slot to slot 0, then
    /// `spell` itself; no row or no index answers `spell`.
    pub fn variant(&self, row: u32, spell: u32, index: Option<u32>) -> u32 {
        let (Some(row), Some(index)) = (self.rows.get(&row), index) else {
            return spell;
        };
        let at = |i: usize| row.get(i).copied().filter(|&v| v != 0);
        at(index as usize)
            .or_else(|| at(1).filter(|_| index == 3))
            .or_else(|| at(0))
            .unwrap_or(spell)
    }
}

pub(crate) fn spell_difficulty_schema() -> Schema {
    let mut s = Schema::new("SpellDifficulty");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new_array("SpellID", FieldType::UInt32, 4));
    s
}

/// Load `SpellDifficulty.dbc` off the chain.
pub fn load_spell_difficulties(chain: &mut Chain) -> Result<SpellDifficultyCatalog> {
    let bytes = chain
        .read_file(SPELL_DIFFICULTY)
        .with_context(|| format!("reading {SPELL_DIFFICULTY}"))?;
    let rs = parse(&bytes, spell_difficulty_schema(), "SpellDifficulty")?;
    let rows = rs.records().iter().filter_map(|r| {
        Some((
            u32_at(r, 0)?,
            [u32_at(r, 1)?, u32_at(r, 2)?, u32_at(r, 3)?, u32_at(r, 4)?],
        ))
    });
    Ok(SpellDifficultyCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variant_walks_its_slot_then_the_two_fallbacks() {
        let c = SpellDifficultyCatalog::from_rows([
            (1, [10, 11, 12, 13]),
            (2, [10, 11, 0, 0]),
            (3, [10, 0, 0, 0]),
            (4, [0, 0, 0, 0]),
        ]);
        assert_eq!(c.variant(1, 99, Some(2)), 12);
        assert_eq!(c.variant(2, 99, Some(3)), 11, "slot 3 borrows slot 1");
        assert_eq!(c.variant(2, 99, Some(2)), 10, "slot 2 borrows slot 0 alone");
        assert_eq!(c.variant(3, 99, Some(3)), 10, "slot 1 empty too");
        assert_eq!(c.variant(4, 99, Some(1)), 99);
        assert_eq!(c.variant(1, 99, None), 99, "not a dungeon or raid");
        assert_eq!(c.variant(9, 99, Some(1)), 99, "no row");
        assert_eq!(c.variant(1, 99, Some(7)), 10, "past the row");
    }

    /// Light Surge's row names its heroic damage spell (`$65767s1` in Surge of Light's text).
    #[test]
    fn the_real_light_surge_row_lists_its_variants() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_spell_difficulties(&mut chain).expect("SpellDifficulty.dbc");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let row = spells.get(65767).expect("Light Surge").difficulty_id;
        assert_ne!(row, 0);
        assert_eq!(rows.variant(row, 65767, Some(1)), 67274);
    }
}
