//! `DungeonMap.dbc`: the floors of a multi-level map, each a world rect on its map and the
//! `WorldMapArea` row a zoom out returns to. 12340's world map keeps the selected row in
//! `[0xaccf0c]` (`GetCurrentMapDungeonLevel` `0x544fc0`, `ZoomOut` `0x546a90`).
//!
//! `DungeonMapChunk.dbc`: which of those floors a WMO group is on, by height, the table the
//! floor lookup walks in file order (`0x5439e0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, u32_at};
use crate::Chain;

const DUNGEON_MAP: &str = "DBFilesClient\\DungeonMap.dbc";
const DUNGEON_MAP_CHUNK: &str = "DBFilesClient\\DungeonMapChunk.dbc";

/// One `DungeonMap.dbc` row: one floor of a map.
#[derive(Clone, Debug, PartialEq)]
pub struct DungeonMap {
    pub id: u32,
    pub map_id: u32,
    /// The floor `GetCurrentMapDungeonLevel` reads (`+0x8`), 1 for the first.
    pub floor_index: i32,
    /// The world rect the floor's art covers, by the projection's use (`0x544140`): `u` runs
    /// `1 - (y - y_min) / (y_max - y_min)`, `v` likewise over x.
    pub y_min: f32,
    pub y_max: f32,
    pub x_min: f32,
    pub x_max: f32,
    /// The `WorldMapArea` row `ZoomOut` selects from this floor (`+0x1c`); 0 for none.
    pub parent_world_map_id: u32,
}

/// `DungeonMap.dbc` rows in file order, the order 12340's floor walks (`0x546190`) take.
pub struct DungeonMapCatalog {
    rows: Vec<DungeonMap>,
}

impl DungeonMapCatalog {
    pub fn from_rows(rows: Vec<DungeonMap>) -> Self {
        Self { rows }
    }

    /// The row with `id`.
    pub fn get(&self, id: u32) -> Option<&DungeonMap> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Every row, in file order.
    pub fn rows(&self) -> &[DungeonMap] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("DungeonMap");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MapID", FieldType::UInt32));
    s.add_field(SchemaField::new("FloorIndex", FieldType::Int32));
    for name in ["MinY", "MaxY", "MinX", "MaxX"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("ParentWorldMapID", FieldType::UInt32));
    s
}

/// Read `DungeonMap.dbc` off the patch chain into a [`DungeonMapCatalog`].
pub fn load_dungeon_map_catalog(chain: &mut Chain) -> Result<DungeonMapCatalog> {
    let bytes = chain
        .read_file(DUNGEON_MAP)
        .context("reading DungeonMap.dbc")?;
    let rs = parse(&bytes, schema(), "DungeonMap")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(DungeonMap {
                id: u32_at(r, 0)?,
                map_id: u32_at(r, 1)?,
                floor_index: i32_at(r, 2)?,
                y_min: f32_at(r, 3)?,
                y_max: f32_at(r, 4)?,
                x_min: f32_at(r, 5)?,
                x_max: f32_at(r, 6)?,
                parent_world_map_id: u32_at(r, 7)?,
            })
        })
        .collect();
    Ok(DungeonMapCatalog { rows })
}

/// One `DungeonMapChunk.dbc` row: a WMO group's floor from `min_z` up.
#[derive(Clone, Debug, PartialEq)]
pub struct DungeonMapChunk {
    pub id: u32,
    pub map_id: u32,
    /// The group's MOGP id (`+0x8`), `WMOAreaTable`'s `WMOGroupID`.
    pub wmo_group_id: u32,
    /// The `DungeonMap` row (`+0xc`).
    pub dungeon_map_id: i32,
    /// The lowest height the row answers for (`+0x10`).
    pub min_z: f32,
}

pub(crate) fn chunk_schema() -> Schema {
    let mut s = Schema::new("DungeonMapChunk");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MapID", FieldType::UInt32));
    s.add_field(SchemaField::new("WMOGroupID", FieldType::UInt32));
    s.add_field(SchemaField::new("DungeonMapID", FieldType::Int32));
    s.add_field(SchemaField::new("MinZ", FieldType::Float32));
    s
}

/// Read `DungeonMapChunk.dbc` off the patch chain, in file order: the lookup's walk depends on it.
pub fn load_dungeon_map_chunks(chain: &mut Chain) -> Result<Vec<DungeonMapChunk>> {
    let bytes = chain
        .read_file(DUNGEON_MAP_CHUNK)
        .context("reading DungeonMapChunk.dbc")?;
    let rs = parse(&bytes, chunk_schema(), "DungeonMapChunk")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(DungeonMapChunk {
                id: u32_at(r, 0)?,
                map_id: u32_at(r, 1)?,
                wmo_group_id: u32_at(r, 2)?,
                dungeon_map_id: i32_at(r, 3)?,
                min_z: f32_at(r, 4)?,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the floor lookup's walk leans on: a map's rows in one run, a group's rows in one
    /// run within it by rising height, and every row naming a floor of its own map.
    #[test]
    fn real_dungeon_map_chunks_run_by_map_then_group() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let floors = load_dungeon_map_catalog(&mut chain).expect("load DungeonMap");
        let chunks = load_dungeon_map_chunks(&mut chain).expect("load DungeonMapChunk");
        assert!(!chunks.is_empty());
        let mut maps = Vec::new();
        let mut groups = Vec::new();
        for pair in chunks.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.map_id != b.map_id {
                maps.push(a.map_id);
                assert!(!maps.contains(&b.map_id), "map {} runs twice", b.map_id);
                groups.clear();
            } else if a.wmo_group_id != b.wmo_group_id {
                groups.push(a.wmo_group_id);
                assert!(
                    !groups.contains(&b.wmo_group_id),
                    "group {} runs twice",
                    b.wmo_group_id
                );
            } else {
                assert!(a.min_z <= b.min_z, "group {} falls", a.wmo_group_id);
            }
        }
        for c in &chunks {
            let floor = u32::try_from(c.dungeon_map_id)
                .ok()
                .and_then(|id| floors.get(id))
                .expect("a chunk names a DungeonMap row");
            assert_eq!(floor.map_id, c.map_id);
            assert!(c.wmo_group_id != 0);
        }
        // Dalaran's groups split over its two floors.
        let dalaran: std::collections::BTreeSet<u32> = chunks
            .iter()
            .filter(|c| c.map_id == 571)
            .filter_map(|c| floors.get(c.dungeon_map_id as u32))
            .map(|f| f.floor_index as u32)
            .collect();
        assert_eq!(dalaran.into_iter().collect::<Vec<_>>(), [1, 2]);
    }

    /// Dalaran's two floors sit on Northrend and zoom out to Crystalsong Forest.
    #[test]
    fn real_dungeon_map_has_dalarans_floors() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_dungeon_map_catalog(&mut chain).expect("load DungeonMap");
        assert!(!cat.is_empty());
        let floors: Vec<&DungeonMap> = cat
            .rows()
            .iter()
            .filter(|r| r.map_id == 571 && r.parent_world_map_id == 495)
            .collect();
        assert_eq!(
            floors.iter().map(|r| r.floor_index).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(floors
            .iter()
            .all(|r| r.y_max > r.y_min && r.x_max > r.x_min));
    }
}
