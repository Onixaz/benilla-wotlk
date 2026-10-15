//! `WorldMapTransforms.dbc`: world regions drawn on another map's sheet, such as the blood elf and
//! draenei zones of map 530 on the Eastern Kingdoms and Kalimdor. 12340's transform `0x543920`
//! takes the first row whose map and region hold a position.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const WORLD_MAP_TRANSFORMS: &str = "DBFilesClient\\WorldMapTransforms.dbc";

/// One `WorldMapTransforms.dbc` row.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldMapTransform {
    pub map_id: u32,
    /// The region on `map_id`, both edges inclusive (`0x543920`).
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
    pub new_map_id: u32,
    /// Added to a position inside the region.
    pub offset_x: f32,
    pub offset_y: f32,
    /// The `DungeonMap` row the region shows, 0 for none.
    pub new_dungeon_map_id: u32,
}

impl WorldMapTransform {
    /// `0x543920`'s containment test for a position on this row's map.
    pub fn holds(&self, map_id: u32, x: f32, y: f32) -> bool {
        self.map_id == map_id
            && (self.x_min..=self.x_max).contains(&x)
            && (self.y_min..=self.y_max).contains(&y)
    }
}

/// `WorldMapTransforms.dbc` rows in file order.
pub struct WorldMapTransformCatalog {
    rows: Vec<WorldMapTransform>,
}

impl WorldMapTransformCatalog {
    pub fn from_rows(rows: Vec<WorldMapTransform>) -> Self {
        Self { rows }
    }

    /// `0x543920`: the first row holding `(map_id, x, y)`.
    pub fn find(&self, map_id: u32, x: f32, y: f32) -> Option<&WorldMapTransform> {
        self.rows.iter().find(|r| r.holds(map_id, x, y))
    }

    pub fn rows(&self) -> &[WorldMapTransform] {
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
    let mut s = Schema::new("WorldMapTransforms");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MapID", FieldType::UInt32));
    for name in ["RegionMinX", "RegionMinY", "RegionMaxX", "RegionMaxY"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("NewMapID", FieldType::UInt32));
    for name in ["RegionOffsetX", "RegionOffsetY"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("NewDungeonMapID", FieldType::UInt32));
    s
}

/// Read `WorldMapTransforms.dbc` off the patch chain into a [`WorldMapTransformCatalog`].
pub fn load_world_map_transform_catalog(chain: &mut Chain) -> Result<WorldMapTransformCatalog> {
    let bytes = chain
        .read_file(WORLD_MAP_TRANSFORMS)
        .context("reading WorldMapTransforms.dbc")?;
    let rs = parse(&bytes, schema(), "WorldMapTransforms")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(WorldMapTransform {
                map_id: u32_at(r, 1)?,
                x_min: f32_at(r, 2)?,
                y_min: f32_at(r, 3)?,
                x_max: f32_at(r, 4)?,
                y_max: f32_at(r, 5)?,
                new_map_id: u32_at(r, 6)?,
                offset_x: f32_at(r, 7)?,
                offset_y: f32_at(r, 8)?,
                new_dungeon_map_id: u32_at(r, 9)?,
            })
        })
        .collect();
    Ok(WorldMapTransformCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eversong Woods' centre on map 530 is drawn on the Eastern Kingdoms (map 0).
    #[test]
    fn real_transforms_put_eversong_on_the_eastern_kingdoms() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_world_map_transform_catalog(&mut chain).expect("load WorldMapTransforms");
        let areas = crate::load_world_map_area_catalog(&mut chain).expect("load WorldMapArea");
        let eversong = areas
            .iter()
            .find(|(_, a)| a.name == "EversongWoods")
            .map(|(_, a)| a)
            .expect("the Eversong row");
        let (x, y) = (
            (eversong.loc_top + eversong.loc_bottom) / 2.0,
            (eversong.loc_left + eversong.loc_right) / 2.0,
        );
        let t = cat.find(eversong.map_id, x, y).expect("a region holds it");
        assert_eq!((eversong.map_id, t.new_map_id), (530, 0));
        assert!(cat.find(0, x, y).is_none(), "map 0 has no region");
    }
}
