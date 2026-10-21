//! `SpellRuneCost.dbc`: the runes a death knight spell spends, by `Spell.dbc`'s `RuneCostID`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const SPELL_RUNE_COST: &str = "DBFilesClient\\SpellRuneCost.dbc";

/// One row: runes of each kind spent, read at `+4`, `+8` and `+0xc` (`0x5ee6a5`, `0x5ee6f2`,
/// `0x5ee73b`), and the runic power the cast grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpellRuneCost {
    pub id: u32,
    pub blood: u32,
    pub unholy: u32,
    pub frost: u32,
    pub runic_power: u32,
}

/// Every row, by id.
#[derive(Debug, Clone, Default)]
pub struct SpellRuneCostCatalog {
    rows: HashMap<u32, SpellRuneCost>,
}

impl SpellRuneCostCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = SpellRuneCost>) -> Self {
        Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&SpellRuneCost> {
        self.rows.get(&id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn spell_rune_cost_schema() -> Schema {
    let mut s = Schema::new("SpellRuneCost");
    for name in ["ID", "Blood", "Unholy", "Frost", "RunicPower"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load `SpellRuneCost.dbc` off the chain.
pub fn load_spell_rune_costs(chain: &mut Chain) -> Result<SpellRuneCostCatalog> {
    let bytes = chain
        .read_file(SPELL_RUNE_COST)
        .with_context(|| format!("reading {SPELL_RUNE_COST}"))?;
    let rs = parse(&bytes, spell_rune_cost_schema(), "SpellRuneCost")?;
    let rows = rs.records().iter().filter_map(|r| {
        Some(SpellRuneCost {
            id: u32_at(r, 0)?,
            blood: u32_at(r, 1)?,
            unholy: u32_at(r, 2)?,
            frost: u32_at(r, 3)?,
            runic_power: u32_at(r, 4)?,
        })
    });
    Ok(SpellRuneCostCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The death knight's opening strikes spend one rune of their kind: Icy Touch (45477) frost,
    /// Plague Strike (45462) unholy, Blood Strike (45902) blood, and Death Strike (49998) frost and
    /// unholy together.
    #[test]
    fn the_starting_strikes_cost_their_runes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let costs = load_spell_rune_costs(&mut chain).expect("SpellRuneCost.dbc");
        assert!(!costs.is_empty());
        let cost = |spell: u32| {
            let id = spells.get(spell).expect("spell row").rune_cost_id;
            let c = costs.get(id).expect("rune cost row");
            (c.blood, c.unholy, c.frost)
        };
        assert_eq!(cost(45477), (0, 0, 1));
        assert_eq!(cost(45462), (0, 1, 0));
        assert_eq!(cost(45902), (1, 0, 0));
        assert_eq!(cost(49998), (0, 1, 1));
    }
}
