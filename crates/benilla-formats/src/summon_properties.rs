//! `SummonProperties.dbc`: how a summon effect's unit is held; slot 5 is a companion (`0x8009b0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const SUMMON_PROPERTIES: &str = "DBFilesClient\\SummonProperties.dbc";

/// The `Slot` column (4) by row id.
#[derive(Debug, Clone, Default)]
pub struct SummonPropertiesCatalog {
    slots: HashMap<u32, u32>,
}

impl SummonPropertiesCatalog {
    /// Build from `(id, slot)` pairs, for tests.
    pub fn from_slots(rows: impl IntoIterator<Item = (u32, u32)>) -> Self {
        Self {
            slots: rows.into_iter().collect(),
        }
    }

    pub fn slot(&self, id: u32) -> Option<u32> {
        self.slots.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

pub(crate) fn summon_properties_schema() -> Schema {
    let mut s = Schema::new("SummonProperties");
    for name in ["ID", "Control", "Faction", "Title", "Slot", "Flags"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load `SummonProperties.dbc` from the patch chain.
pub fn load_summon_properties(chain: &mut Chain) -> Result<SummonPropertiesCatalog> {
    let bytes = chain
        .read_file(SUMMON_PROPERTIES)
        .with_context(|| format!("reading {SUMMON_PROPERTIES}"))?;
    let rs = parse(&bytes, summon_properties_schema(), "SummonProperties")?;
    Ok(SummonPropertiesCatalog::from_slots(
        rs.records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 4)?))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A companion pet's summon row holds slot 5: Mechanical Squirrel Box (4055) summons through
    /// its `EffectMiscValueB`.
    #[test]
    fn a_companion_summon_row_holds_slot_five() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let props = load_summon_properties(&mut chain).expect("SummonProperties.dbc");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let squirrel = spells.get(4055).expect("spell 4055");
        let row = squirrel.effect_misc_value_b[0] as u32;
        assert_eq!(props.slot(row), Some(5), "row {row}");
    }
}
