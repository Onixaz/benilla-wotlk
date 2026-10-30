//! `BattlemasterList.dbc` and `PvpDifficulty.dbc`: every battleground and arena type the queue
//! names, and each map's level brackets. 12340 reads the first from the battlefield info's list,
//! join and status bindings and the second only to place a level in a bracket (`0x54ba00`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at, u32_at};
use crate::Chain;

const BATTLEMASTER_LIST: &str = "DBFilesClient\\BattlemasterList.dbc";
const PVP_DIFFICULTY: &str = "DBFilesClient\\PvpDifficulty.dbc";

/// One `BattlemasterList.dbc` row (`0x40` bytes in the client).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattlemasterRec {
    pub id: u32,
    /// The maps the type runs on, `-1` padded; the first is the one the bindings read.
    pub map_ids: [i32; 8],
    /// 3 a battleground, 4 an arena.
    pub instance_type: u32,
    pub groups_allowed: bool,
    /// The enUS name.
    pub name: String,
    pub max_group_size: i32,
    /// The world state that marks the type's holiday weekend, 0 for none.
    pub holiday_world_state: u32,
    pub min_level: i32,
    pub max_level: i32,
}

/// One `PvpDifficulty.dbc` row: a level bracket of one map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PvpBracket {
    pub map_id: i32,
    pub range_index: i32,
    pub min_level: i32,
    pub max_level: i32,
}

/// Both tables, in file order.
pub struct BattlemasterCatalog {
    rows: Vec<BattlemasterRec>,
    brackets: Vec<PvpBracket>,
}

/// The random battleground's id, whose bracket is level 80 alone (`0x54ba1e`).
const RANDOM_BATTLEGROUND: u32 = 32;

impl BattlemasterCatalog {
    pub fn rows(&self) -> &[BattlemasterRec] {
        &self.rows
    }

    pub fn get(&self, id: u32) -> Option<&BattlemasterRec> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// The bracket-for-level lookup (`0x54ba00`): the first map's bracket holding `level`, its
    /// range index; the random battleground answers 0 at level 80 alone. The walk stops at the end
    /// of the map's run of rows, as the reference's does.
    pub fn bracket_for_level(&self, rec: &BattlemasterRec, level: u32) -> Option<i32> {
        if rec.id == RANDOM_BATTLEGROUND {
            return (level == 80).then_some(0);
        }
        let mut found_map = false;
        for b in &self.brackets {
            if b.map_id != rec.map_ids[0] {
                if found_map {
                    return None;
                }
                continue;
            }
            if level >= b.min_level as u32 && level <= b.max_level as u32 {
                return Some(b.range_index);
            }
            found_map = true;
        }
        None
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// `ID`, eight `MapID`, `InstanceType`, `GroupsAllowed`, `Name_lang` (16 locales and a mask),
/// `MaxGroupSize`, `HolidayWorldState`, `MinLevel`, `MaxLevel`: 32 columns.
pub(crate) fn battlemaster_list_schema() -> Schema {
    let mut s = Schema::new("BattlemasterList");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("MapID{i}"), FieldType::Int32));
    }
    s.add_field(SchemaField::new("InstanceType", FieldType::UInt32));
    s.add_field(SchemaField::new("GroupsAllowed", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("MaxGroupSize", FieldType::Int32));
    s.add_field(SchemaField::new("HolidayWorldState", FieldType::UInt32));
    s.add_field(SchemaField::new("MinLevel", FieldType::Int32));
    s.add_field(SchemaField::new("MaxLevel", FieldType::Int32));
    s
}

/// `ID`, `MapID`, `RangeIndex`, `MinLevel`, `MaxLevel`, `Difficulty`: 6 columns.
pub(crate) fn pvp_difficulty_schema() -> Schema {
    let mut s = Schema::new("PvpDifficulty");
    for name in [
        "ID",
        "MapID",
        "RangeIndex",
        "MinLevel",
        "MaxLevel",
        "Difficulty",
    ] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s
}

/// Load both tables from the patch chain.
pub fn load_battlemaster_catalog(chain: &mut Chain) -> Result<BattlemasterCatalog> {
    let bytes = chain
        .read_file(BATTLEMASTER_LIST)
        .with_context(|| format!("reading {BATTLEMASTER_LIST}"))?;
    let rs = parse(&bytes, battlemaster_list_schema(), "BattlemasterList")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut map_ids = [-1i32; 8];
        for (i, m) in map_ids.iter_mut().enumerate() {
            *m = i32_at(r, 1 + i).unwrap_or(-1);
        }
        rows.push(BattlemasterRec {
            id,
            map_ids,
            instance_type: u32_at(r, 9).unwrap_or(0),
            groups_allowed: u32_at(r, 10).unwrap_or(0) != 0,
            name: str_at(&rs, r, 11).unwrap_or_default(),
            max_group_size: i32_at(r, 28).unwrap_or(0),
            holiday_world_state: u32_at(r, 29).unwrap_or(0),
            min_level: i32_at(r, 30).unwrap_or(0),
            max_level: i32_at(r, 31).unwrap_or(0),
        });
    }

    let bytes = chain
        .read_file(PVP_DIFFICULTY)
        .with_context(|| format!("reading {PVP_DIFFICULTY}"))?;
    let rs = parse(&bytes, pvp_difficulty_schema(), "PvpDifficulty")?;
    let brackets = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(PvpBracket {
                map_id: i32_at(r, 1)?,
                range_index: i32_at(r, 2)?,
                min_level: i32_at(r, 3)?,
                max_level: i32_at(r, 4)?,
            })
        })
        .collect();
    Ok(BattlemasterCatalog { rows, brackets })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_types_name_their_maps_and_brackets() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_battlemaster_catalog(&mut chain).expect("BattlemasterList.dbc");
        let maps = crate::load_map_catalog(&mut chain).expect("Map.dbc");

        // Every row's first map is a real map whose instance type matches the row's.
        for r in cat.rows() {
            assert!(r.instance_type == 3 || r.instance_type == 4, "row {}", r.id);
            assert!(r.max_group_size > 0, "row {}", r.id);
            // The random battleground alone carries no level span; its bracket is level 80.
            assert_eq!(r.min_level > 0, r.id != RANDOM_BATTLEGROUND, "row {}", r.id);
            if r.map_ids[0] >= 0 {
                assert_eq!(
                    maps.instance_type(r.map_ids[0] as u32),
                    Some(r.instance_type),
                    "row {}'s first map",
                    r.id
                );
            }
        }
        let random = cat.get(RANDOM_BATTLEGROUND).expect("the random type");
        assert_eq!(cat.bracket_for_level(random, 80), Some(0));
        assert_eq!(cat.bracket_for_level(random, 79), None);

        // A battleground's brackets cover its own level span and nothing below it.
        let wsg = cat
            .rows()
            .iter()
            .find(|r| r.instance_type == 3 && r.min_level == 10)
            .expect("a level-10 battleground");
        assert!(cat.bracket_for_level(wsg, 10).is_some());
        assert!(cat.bracket_for_level(wsg, 80).is_some());
        assert_eq!(cat.bracket_for_level(wsg, 9), None);
    }
}
