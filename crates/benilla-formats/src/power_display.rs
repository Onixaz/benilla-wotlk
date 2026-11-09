//! `PowerDisplay.dbc`: another name and colour for a power, such as a vehicle's Pyrite or Steam
//! shown over its energy; `Spell.dbc`'s `PowerDisplayID` and `Vehicle.dbc`'s three name a row.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const POWER_DISPLAY: &str = "DBFilesClient\\PowerDisplay.dbc";

/// One row. `tag` is the `GlobalStrings` key naming the power, `UnitPowerType`'s second return.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerDisplay {
    pub id: u32,
    /// The power it stands over (`+4`).
    pub actual_type: u32,
    pub tag: String,
    /// `+0xc` to `+0xe`.
    pub rgb: [u8; 3],
}

/// Every row, by id.
#[derive(Debug, Clone, Default)]
pub struct PowerDisplayCatalog {
    rows: HashMap<u32, PowerDisplay>,
}

impl PowerDisplayCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = PowerDisplay>) -> Self {
        Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&PowerDisplay> {
        self.rows.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PowerDisplay> {
        self.rows.values()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn power_display_schema() -> Schema {
    let mut s = Schema::new("PowerDisplay");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("ActualType", FieldType::UInt32));
    s.add_field(SchemaField::new("GlobalStringBaseTag", FieldType::String));
    for name in ["Red", "Green", "Blue"] {
        s.add_field(SchemaField::new(name, FieldType::UInt8));
    }
    s
}

/// Load `PowerDisplay.dbc` off the chain.
pub fn load_power_displays(chain: &mut Chain) -> Result<PowerDisplayCatalog> {
    let bytes = chain
        .read_file(POWER_DISPLAY)
        .with_context(|| format!("reading {POWER_DISPLAY}"))?;
    let rs = parse(&bytes, power_display_schema(), "PowerDisplay")?;
    let rows = rs.records().iter().filter_map(|r| {
        Some(PowerDisplay {
            id: u32_at(r, 0)?,
            actual_type: u32_at(r, 1)?,
            tag: str_at(&rs, r, 2)?,
            rgb: [
                u32_at(r, 3)? as u8,
                u32_at(r, 4)? as u8,
                u32_at(r, 5)? as u8,
            ],
        })
    });
    Ok(PowerDisplayCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hurl Pyrite Barrel (62490) names the Pyrite row, which stands over energy.
    #[test]
    fn the_real_pyrite_row_reads_its_tag_and_colour() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_power_displays(&mut chain).expect("PowerDisplay.dbc");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let id = spells.get(62490).expect("the spell").power_display_id;
        let row = rows.get(id).expect("its row");
        assert_eq!(row.tag, "POWER_TYPE_PYRITE");
        assert_eq!(row.actual_type, 3);
        assert_eq!(row.rgb, [0, 202, 255]);
    }
}
