//! `SummonProperties.dbc`: how a summon effect's unit is held; slot 5 is a companion (`0x8009b0`),
//! flag `0x20` keeps the menu's Dismiss off (`PetCanBeDismissed`, `0x5d3820`), and control 3 is a
//! possession (`0x5d6413`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const SUMMON_PROPERTIES: &str = "DBFilesClient\\SummonProperties.dbc";

/// `Control` 3: the summoner possesses the unit.
pub const SUMMON_CONTROL_POSSESSED: u32 = 3;

/// One row's `Control` (1), `Slot` (4) and `Flags` (5).
#[derive(Debug, Clone, Copy, Default)]
struct SummonRow {
    control: u32,
    slot: u32,
    flags: u32,
}

/// The rows the client reads, by id.
#[derive(Debug, Clone, Default)]
pub struct SummonPropertiesCatalog {
    rows: HashMap<u32, SummonRow>,
}

impl SummonPropertiesCatalog {
    /// Build from `(id, slot)` pairs with no flags, for tests.
    pub fn from_slots(rows: impl IntoIterator<Item = (u32, u32)>) -> Self {
        Self::from_rows(rows.into_iter().map(|(id, slot)| (id, slot, 0)))
    }

    /// Build from `(id, slot, flags)` rows with control 0.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, u32, u32)>) -> Self {
        Self::from_records(
            rows.into_iter()
                .map(|(id, slot, flags)| (id, 0, slot, flags)),
        )
    }

    /// Build from `(id, control, slot, flags)` rows.
    pub fn from_records(rows: impl IntoIterator<Item = (u32, u32, u32, u32)>) -> Self {
        Self {
            rows: rows
                .into_iter()
                .map(|(id, control, slot, flags)| {
                    (
                        id,
                        SummonRow {
                            control,
                            slot,
                            flags,
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn slot(&self, id: u32) -> Option<u32> {
        self.rows.get(&id).map(|r| r.slot)
    }

    /// The row's `Flags`, `None` for a row the file lacks.
    pub fn flags(&self, id: u32) -> Option<u32> {
        self.rows.get(&id).map(|r| r.flags)
    }

    /// The row's `Control`, `None` for a row the file lacks.
    pub fn control(&self, id: u32) -> Option<u32> {
        self.rows.get(&id).map(|r| r.control)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
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
    Ok(SummonPropertiesCatalog::from_records(
        rs.records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 1)?, u32_at(r, 4)?, u32_at(r, 5)?))),
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

    /// Eye of Kilrogg (126) summons a possessed unit: its summon row's control is 3.
    #[test]
    fn eye_of_kilrogg_summons_through_a_possessed_row() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let props = load_summon_properties(&mut chain).expect("SummonProperties.dbc");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let row = spells
            .get(126)
            .and_then(|s| s.summon_properties_row())
            .expect("spell 126 summons");
        assert_eq!(
            props.control(row),
            Some(SUMMON_CONTROL_POSSESSED),
            "row {row}"
        );
    }
}
