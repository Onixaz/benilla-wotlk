//! `CreatureMovementInfo.dbc`: a creature template's smooth-facing chase rate, the row its
//! `movementId` names. A row with a rate over `1e-5` turns the unit's display facing by a damped
//! turn at that rate instead of the box filter (`0x735f60`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const CREATURE_MOVEMENT_INFO: &str = "DBFilesClient\\CreatureMovementInfo.dbc";

/// Row id → `SmoothFacingChaseRate`.
pub struct CreatureMovementInfo(HashMap<u32, f32>);

impl CreatureMovementInfo {
    /// The chase rate of row `movement_id`; `None` for no row (0 is none).
    pub fn chase_rate(&self, movement_id: u32) -> Option<f32> {
        self.0.get(&movement_id).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("CreatureMovementInfo");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new(
        "SmoothFacingChaseRate",
        FieldType::Float32,
    ));
    s.set_key_field("ID");
    s
}

/// Load `CreatureMovementInfo.dbc` from the patch chain.
pub fn load_creature_movement_info(chain: &mut Chain) -> Result<CreatureMovementInfo> {
    let bytes = chain
        .read_file(CREATURE_MOVEMENT_INFO)
        .with_context(|| format!("reading {CREATURE_MOVEMENT_INFO}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<CreatureMovementInfo> {
    let rs = parse(bytes, schema(), "CreatureMovementInfo")?;
    let mut rates = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(rate)) = (u32_at(r, 0), f32_at(r, 1)) {
            rates.insert(id, rate);
        }
    }
    Ok(CreatureMovementInfo(rates))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthesize(rows: &[(u32, f32)]) -> Vec<u8> {
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&2u32.to_le_bytes()); // fields
        dbc.extend_from_slice(&8u32.to_le_bytes()); // record size
        dbc.extend_from_slice(&1u32.to_le_bytes()); // string block
        for (id, rate) in rows {
            dbc.extend_from_slice(&id.to_le_bytes());
            dbc.extend_from_slice(&rate.to_le_bytes());
        }
        dbc.push(0);
        dbc
    }

    #[test]
    fn a_row_answers_its_chase_rate() {
        let table = table_from(&synthesize(&[(1, 2.0), (41, 0.0), (801, 0.25)])).unwrap();
        assert_eq!(table.len(), 3);
        assert_eq!(table.chase_rate(1), Some(2.0));
        assert_eq!(table.chase_rate(41), Some(0.0));
        assert_eq!(table.chase_rate(801), Some(0.25));
        assert_eq!(table.chase_rate(0), None);
    }

    /// The 12340 table: row 1 chases at 2, row 41 not at all, row 441 at 10.
    #[test]
    fn real_creature_movement_info_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let table = load_creature_movement_info(&mut chain).expect("CreatureMovementInfo.dbc");
        assert_eq!(table.chase_rate(1), Some(2.0));
        assert_eq!(table.chase_rate(41), Some(0.0));
        assert_eq!(table.chase_rate(441), Some(10.0));
    }
}
