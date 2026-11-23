//! `DestructibleModelData.dbc`: a destructible building's (GameObject type 33) models and doodad
//! sets per damage state. The template's `data[18]` names the row (`0x710be0`); state 0 is the
//! intact model the template's display names, states 1 to 3 (damaged, destroyed, rebuilding) the
//! row's WMO displays, each falling back to the state before it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, u32_at};
use crate::Chain;

const DESTRUCTIBLE_MODEL_DATA: &str = "DBFilesClient\\DestructibleModelData.dbc";

/// A building's damage states: intact, damaged, destroyed, rebuilding.
pub const DESTRUCTIBLE_STATES: usize = 4;

/// The 19 columns in file order, as the 12340 reader (`0x8ae770`) takes them.
const COLUMNS: [&str; 19] = [
    "ID",
    "State0ImpactEffectDoodadSet",
    "State0AmbientDoodadSet",
    "State1Wmo",
    "State1DestructionDoodadSet",
    "State1ImpactEffectDoodadSet",
    "State1AmbientDoodadSet",
    "State2Wmo",
    "State2DestructionDoodadSet",
    "State2ImpactEffectDoodadSet",
    "State2AmbientDoodadSet",
    "State3Wmo",
    "State3InitDoodadSet",
    "State3AmbientDoodadSet",
    "EjectDirection",
    "RepairGroundFx",
    "DoNotHighlight",
    "HealEffect",
    "HealEffectSpeed",
];

/// One `DestructibleModelData` row, its per-state columns gathered by state; a column the state
/// has no slot for is 0, as the reference's getters answer 0 for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DestructibleModel {
    /// `State1Wmo` to `State3Wmo`, `GameObjectDisplayInfo` ids; slot 0 is unused, the intact model
    /// being the template's display.
    pub wmo: [u32; DESTRUCTIBLE_STATES],
    /// The doodad set a state change into the state plays once: `State1`/`State2`
    /// `DestructionDoodadSet` and, for state 3, `State3InitDoodadSet` (`0x70b760`).
    pub destruction_set: [i32; DESTRUCTIBLE_STATES],
    /// The set a hit plays, states 0 to 2 (`0x70b720`).
    pub impact_set: [i32; DESTRUCTIBLE_STATES],
    /// The set that loops while the state holds, all four states (`0x70b7a0`).
    pub ambient_set: [i32; DESTRUCTIBLE_STATES],
    pub eject_direction: i32,
    /// A `GameObjectDisplayInfo` id drawn at the base while a rebuild rises; 0 for none.
    pub repair_ground_fx: u32,
    /// Never a mouseover (`0x70df30`) and never brightened (`0x70ca90`).
    pub do_not_highlight: bool,
    /// How a rebuild rises: 0 to 3 pick the curve, 4 none (`0x70dfa0`, `0x70e750`).
    pub heal_effect: i32,
    /// The rise's speed; 0 or less takes the default 12 (`0x70dfa0`).
    pub heal_effect_speed: i32,
}

impl DestructibleModel {
    /// A state's destruction set as a doodad set index: the reference's getters return the low 16
    /// bits, and 0 is none.
    pub fn destruction(&self, state: usize) -> u16 {
        set_index(self.destruction_set.get(state))
    }

    /// A state's impact set; 0 is none.
    pub fn impact(&self, state: usize) -> u16 {
        set_index(self.impact_set.get(state))
    }

    /// A state's ambient set; 0 is none.
    pub fn ambient(&self, state: usize) -> u16 {
        set_index(self.ambient_set.get(state))
    }
}

fn set_index(v: Option<&i32>) -> u16 {
    v.map_or(0, |&v| v as u16)
}

/// Row id → row.
#[derive(Default)]
pub struct DestructibleModels(HashMap<u32, DestructibleModel>);

impl DestructibleModels {
    /// Build from rows, for tests.
    pub fn from_rows(rows: HashMap<u32, DestructibleModel>) -> Self {
        DestructibleModels(rows)
    }

    /// The row a template's `data[18]` names; 0 and an id with no row name none.
    pub fn get(&self, id: u32) -> Option<&DestructibleModel> {
        self.0.get(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 19 int columns, 76-byte rows.
pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("DestructibleModelData");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for name in &COLUMNS[1..] {
        s.add_field(SchemaField::new(*name, FieldType::Int32));
    }
    s.set_key_field("ID");
    s
}

/// Load `DestructibleModelData.dbc` from the patch chain.
pub fn load_destructible_models(chain: &mut Chain) -> Result<DestructibleModels> {
    let bytes = chain
        .read_file(DESTRUCTIBLE_MODEL_DATA)
        .with_context(|| format!("reading {DESTRUCTIBLE_MODEL_DATA}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<DestructibleModels> {
    let rs = parse(bytes, schema(), "DestructibleModelData")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let col = |i: usize| i32_at(r, i).unwrap_or(0);
        rows.insert(
            id,
            DestructibleModel {
                wmo: [0, col(3) as u32, col(7) as u32, col(11) as u32],
                destruction_set: [0, col(4), col(8), col(12)],
                impact_set: [col(1), col(5), col(9), 0],
                ambient_set: [col(2), col(6), col(10), col(13)],
                eject_direction: col(14),
                repair_ground_fx: col(15) as u32,
                do_not_highlight: col(16) != 0,
                heal_effect: col(17),
                heal_effect_speed: col(18),
            },
        );
    }
    Ok(DestructibleModels(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC of 19-int rows.
    fn synthesize(rows: &[[i32; 19]]) -> Vec<u8> {
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&19u32.to_le_bytes()); // fields
        dbc.extend_from_slice(&76u32.to_le_bytes()); // record size
        dbc.extend_from_slice(&1u32.to_le_bytes()); // string block: the empty string
        for row in rows {
            for v in row {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0);
        dbc
    }

    #[test]
    fn columns_gather_by_state_as_the_getters_read_them() {
        let row = std::array::from_fn(|i| i as i32 * 10);
        let table = table_from(&synthesize(&[{
            let mut r: [i32; 19] = row;
            r[0] = 58;
            r
        }]))
        .unwrap();
        let m = table.get(58).unwrap();
        assert_eq!(m.wmo, [0, 30, 70, 110]);
        // The destruction getter's state 3 is the init set; state 0 has none.
        assert_eq!(m.destruction_set, [0, 40, 80, 120]);
        // The impact getter stops at state 2.
        assert_eq!(m.impact_set, [10, 50, 90, 0]);
        assert_eq!(m.ambient_set, [20, 60, 100, 130]);
        assert_eq!(
            (m.eject_direction, m.repair_ground_fx, m.do_not_highlight),
            (140, 150, true)
        );
        assert_eq!((m.heal_effect, m.heal_effect_speed), (170, 180));
        assert_eq!((m.destruction(3), m.impact(3), m.ambient(4)), (120, 0, 0));
    }

    #[test]
    fn a_set_index_is_the_low_16_bits() {
        let m = DestructibleModel {
            ambient_set: [0x1_0002, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(m.ambient(0), 2);
    }

    /// The 12340 table: 41 rows. Row 31 is the Wintergrasp fortress wall (damaged 7897, destroyed
    /// 7874), row 58 the one with a rebuilding WMO and a repair FX, row 67 the Lich King's
    /// platform, whose rebuild does not rise (heal effect 4).
    #[test]
    fn real_destructible_model_data_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let table = load_destructible_models(&mut chain).expect("DestructibleModelData.dbc");
        assert_eq!(table.len(), 41);
        let wall = table.get(31).expect("row 31");
        assert_eq!(wall.wmo, [0, 7897, 7874, 0]);
        assert_eq!((wall.destruction(1), wall.destruction(2)), (1, 1));
        assert_eq!(wall.ambient(3), 3);
        let rebuilt = table.get(58).expect("row 58");
        assert_eq!(rebuilt.wmo, [0, 7540, 7541, 7809]);
        assert_eq!(
            (
                rebuilt.repair_ground_fx,
                rebuilt.heal_effect,
                rebuilt.heal_effect_speed
            ),
            (6331, 1, 25)
        );
        assert_eq!((rebuilt.impact(0), rebuilt.impact(1)), (3, 3));
        let platform = table.get(67).expect("row 67");
        assert_eq!((platform.heal_effect, platform.destruction(3)), (4, 1));
        assert!(platform.do_not_highlight);
    }
}
