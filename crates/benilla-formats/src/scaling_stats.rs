//! `ScalingStatDistribution.dbc` and `ScalingStatValues.dbc`, the two tables a scaling item (an
//! heirloom) reads its stats, armor and damage from at the shown level. 12340 looks a distribution
//! up by id (`0x61f582`: bounds `[0xad44d0]`..`[0xad44cc]`, index `[0xad44e0]`) and a values row
//! by index, level minus 1 (`0x61f5ae`: count `[0xad44ec]`, rows `[0xad4500]`, 0x60 bytes each).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};
use crate::Chain;

const DISTRIBUTION: &str = "DBFilesClient\\ScalingStatDistribution.dbc";
const VALUES: &str = "DBFilesClient\\ScalingStatValues.dbc";

/// `ScalingStatValues` columns past the id: the readers index the row by byte offset, column `n`
/// at `n * 4` (`0x61a650` budgets, `0x61a6b0` armor, `0x61a830` damage, `+0x40` spell power).
pub const SCALING_VALUE_COLUMNS: usize = 24;

/// One distribution: up to ten `ItemModType` stats, each taking `bonus / 10000` of the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalingStatDistribution {
    pub id: u32,
    /// Columns 1..=10 (`row + 0x4`); 0 or below is no stat (`0x61f608`).
    pub stat_ids: [i32; 10],
    /// Columns 11..=20 (`row + 0x2c`).
    pub bonuses: [i32; 10],
    /// Column 21 (`row + 0x54`), the highest level the item scales to (`0x61e809`).
    pub max_level: i32,
}

/// Both tables: the distributions by id and the values rows in file order, which 12340 reads by
/// index.
#[derive(Debug, Clone, Default)]
pub struct ScalingStatCatalog {
    pub distributions: HashMap<u32, ScalingStatDistribution>,
    /// Every values row's 24 columns, the id first.
    pub values: Vec<[i32; SCALING_VALUE_COLUMNS]>,
}

pub(crate) fn distribution_schema() -> Schema {
    let mut s = Schema::new("ScalingStatDistribution");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("StatID{i}"), FieldType::Int32));
    }
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("Bonus{i}"), FieldType::Int32));
    }
    s.add_field(SchemaField::new("Maxlevel", FieldType::Int32));
    s
}

pub(crate) fn values_schema() -> Schema {
    let mut s = Schema::new("ScalingStatValues");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 1..SCALING_VALUE_COLUMNS {
        s.add_field(SchemaField::new(format!("Column{i}"), FieldType::Int32));
    }
    s
}

/// Load both tables from the patch chain.
pub fn load_scaling_stats(chain: &mut Chain) -> Result<ScalingStatCatalog> {
    let bytes = chain
        .read_file(DISTRIBUTION)
        .with_context(|| format!("reading {DISTRIBUTION}"))?;
    let rs = parse(&bytes, distribution_schema(), "ScalingStatDistribution")?;
    let distributions = rs
        .records()
        .iter()
        .filter_map(|r| {
            let mut row = ScalingStatDistribution {
                id: u32_at(r, 0)?,
                stat_ids: [0; 10],
                bonuses: [0; 10],
                max_level: i32_at(r, 21)?,
            };
            for i in 0..10 {
                row.stat_ids[i] = i32_at(r, 1 + i)?;
                row.bonuses[i] = i32_at(r, 11 + i)?;
            }
            Some((row.id, row))
        })
        .collect();
    let bytes = chain
        .read_file(VALUES)
        .with_context(|| format!("reading {VALUES}"))?;
    let rs = parse(&bytes, values_schema(), "ScalingStatValues")?;
    let values = rs
        .records()
        .iter()
        .filter_map(|r| {
            let mut row = [0; SCALING_VALUE_COLUMNS];
            for (i, cell) in row.iter_mut().enumerate() {
                *cell = i32_at(r, i)?;
            }
            Some(row)
        })
        .collect();
    Ok(ScalingStatCatalog {
        distributions,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values rows run one per level from 1 (column 1, `Charlevel`), so index `level - 1` is
    /// that level's row, and every distribution's stats are `ItemModType`s.
    #[test]
    fn the_shipped_values_rows_run_one_per_level() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_scaling_stats(&mut chain).expect("scaling tables");
        assert!(!cat.values.is_empty());
        for (i, row) in cat.values.iter().enumerate() {
            assert_eq!(row[1], i as i32 + 1, "row {i} is level {}", i + 1);
        }
        assert!(!cat.distributions.is_empty());
        // Every stat slot names an `ItemModType` or none (-1, or 0 with no bonus).
        for d in cat.distributions.values() {
            assert!(d.stat_ids.iter().all(|&s| (-1..=48).contains(&s)), "{d:?}");
            assert!(d.max_level >= 0, "{d:?}");
        }
    }
}
