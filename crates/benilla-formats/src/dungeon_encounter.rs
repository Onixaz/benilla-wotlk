//! `DungeonEncounter.dbc` (23 columns in 0x5c-byte records): the bosses of a map at one
//! difficulty, each a bit of the completed-encounter mask the dungeon finder's proposal and
//! search results carry. 12340 walks the table in file order (`0x553830`, `0x5538b0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{i32_at, parse, str_at};

const DUNGEON_ENCOUNTER: &str = "DBFilesClient\\DungeonEncounter.dbc";

/// One `DungeonEncounter.dbc` row, with its `SpellIcon.dbc` texture resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DungeonEncounter {
    pub id: i32,
    pub map_id: i32,
    pub difficulty: i32,
    pub order_index: i32,
    /// The encounter's bit in a completed mask.
    pub bit: i32,
    pub name: String,
    /// `None` for no icon row or an empty path, which pushes nil (`0x5538b0`).
    pub icon: Option<String>,
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("DungeonEncounter");
    for name in ["ID", "MapID", "Difficulty", "OrderIndex", "Bit"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s.add_field(SchemaField::new("Name", FieldType::String));
    for i in 1..17 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("SpellIconID", FieldType::Int32));
    s
}

/// Read `DungeonEncounter.dbc` off the patch chain, in file order.
pub fn load_dungeon_encounters(chain: &mut Chain) -> Result<Vec<DungeonEncounter>> {
    let bytes = chain
        .read_file(DUNGEON_ENCOUNTER)
        .context("reading DungeonEncounter.dbc")?;
    let rs = parse(&bytes, schema(), "DungeonEncounter.dbc")?;
    let icons = crate::dbc::load_spell_icon_map(chain)?;
    let int = |r, i| i32_at(r, i).unwrap_or(0);
    Ok(rs
        .records()
        .iter()
        .map(|r| DungeonEncounter {
            id: int(r, 0),
            map_id: int(r, 1),
            difficulty: int(r, 2),
            order_index: int(r, 3),
            bit: int(r, 4),
            name: str_at(&rs, r, 5).unwrap_or_default(),
            icon: u32::try_from(int(r, 22))
                .ok()
                .filter(|&i| i != 0)
                .and_then(|i| icons.get(&i).cloned()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each encounter is named and its bit fits the 32-bit mask; within a map and difficulty no
    /// two encounters share a bit.
    #[test]
    fn the_real_table_gives_each_boss_its_own_bit() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_dungeon_encounters(&mut chain).expect("load DungeonEncounter.dbc");
        assert!(!rows.is_empty());
        let mut seen = std::collections::HashSet::new();
        for r in &rows {
            assert!(!r.name.is_empty(), "row {} is named", r.id);
            assert!((0..32).contains(&r.bit), "row {} bit", r.id);
            assert!(
                seen.insert((r.map_id, r.difficulty, r.bit)),
                "row {} bit is unique",
                r.id
            );
        }
    }
}
