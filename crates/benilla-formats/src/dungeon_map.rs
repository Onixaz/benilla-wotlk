//! `DungeonMap.dbc`: the floors of a multi-level map, each a world rect on its map and the
//! `WorldMapArea` row a zoom out returns to. 12340's world map keeps the selected row in
//! `[0xaccf0c]` (`GetCurrentMapDungeonLevel` `0x544fc0`, `ZoomOut` `0x546a90`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, u32_at};
use crate::Chain;

const DUNGEON_MAP: &str = "DBFilesClient\\DungeonMap.dbc";

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

#[cfg(test)]
mod tests {
    use super::*;

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
