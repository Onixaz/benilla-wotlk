//! `WorldMapContinent.dbc`: each continent's ADT tile bounds and its place on the world map sheet,
//! read by the builder and the world-mode projections. vmangos does not load it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const WORLD_MAP_CONTINENT: &str = "DBFilesClient\\WorldMapContinent.dbc";

/// One `WorldMapContinent.dbc` row.
#[derive(Clone, Debug)]
pub struct WorldMapContinent {
    /// `Map.dbc` id: 0 Azeroth, 1 Kalimdor.
    pub map_id: u32,
    /// ADT tile column bounds, which the reference turns into the continent's sheet rect.
    pub left_boundary: u32,
    pub right_boundary: u32,
    /// ADT tile row bounds.
    pub top_boundary: u32,
    pub bottom_boundary: u32,
    /// World-sheet offsets, x on the u axis and y on v (:
    /// `u = offset_x/62.625 + 0.5 - wy·k·scale`).
    pub offset_x: f32,
    pub offset_y: f32,
    /// The world-sheet scale, 0.75 on both rows; there is no separate world zoom.
    pub scale: f32,
    /// The taxi map's rect in world units; no map projection reads it.
    pub taxi_min: (f32, f32),
    pub taxi_max: (f32, f32),
    /// The last column (`+0x34`), which the world map's builder copies as the continent's click
    /// flag on the Azeroth sheet (`0x547eb0`): 0 on Outland, drawn on the cosmic sheet instead.
    pub world_map_id: u32,
}

/// `WorldMapContinent.dbc` rows keyed by `MapID`.
pub struct WorldMapContinentCatalog {
    by_map_id: HashMap<u32, WorldMapContinent>,
}

impl WorldMapContinentCatalog {
    pub fn get(&self, map_id: u32) -> Option<&WorldMapContinent> {
        self.by_map_id.get(&map_id)
    }

    pub fn len(&self) -> usize {
        self.by_map_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_map_id.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("WorldMapContinent");
    for name in [
        "ID",
        "MapID",
        "LeftBoundary",
        "RightBoundary",
        "TopBoundary",
        "BottomBoundary",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in [
        "OffsetX", "OffsetY", "Scale", "TaxiMinX", "TaxiMinY", "TaxiMaxX", "TaxiMaxY",
    ] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    // 12340 closes the record with a column wowdev names `WorldMapID`: 1 on Azeroth, Kalimdor and
    // Northrend, 0 on Outland.
    s.add_field(SchemaField::new("WorldMapID", FieldType::UInt32));
    s
}

/// Read `WorldMapContinent.dbc` off the patch chain into a [`WorldMapContinentCatalog`].
pub fn load_world_map_continent_catalog(chain: &mut Chain) -> Result<WorldMapContinentCatalog> {
    let bytes = chain
        .read_file(WORLD_MAP_CONTINENT)
        .context("reading WorldMapContinent.dbc")?;
    let rs = parse(&bytes, schema(), "WorldMapContinent")?;
    let mut by_map_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(map_id) = u32_at(r, 1) else { continue };
        let (
            Some(left_boundary),
            Some(right_boundary),
            Some(top_boundary),
            Some(bottom_boundary),
            Some(offset_x),
            Some(offset_y),
            Some(scale),
            Some(taxi_min_x),
            Some(taxi_min_y),
            Some(taxi_max_x),
            Some(taxi_max_y),
        ) = (
            u32_at(r, 2),
            u32_at(r, 3),
            u32_at(r, 4),
            u32_at(r, 5),
            f32_at(r, 6),
            f32_at(r, 7),
            f32_at(r, 8),
            f32_at(r, 9),
            f32_at(r, 10),
            f32_at(r, 11),
            f32_at(r, 12),
        )
        else {
            continue;
        };
        by_map_id.insert(
            map_id,
            WorldMapContinent {
                map_id,
                left_boundary,
                right_boundary,
                top_boundary,
                bottom_boundary,
                offset_x,
                offset_y,
                scale,
                taxi_min: (taxi_min_x, taxi_min_y),
                taxi_max: (taxi_max_x, taxi_max_y),
                world_map_id: u32_at(r, 13).unwrap_or(0),
            },
        );
    }
    Ok(WorldMapContinentCatalog { by_map_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_world_map_continent_has_the_four_continents() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_world_map_continent_catalog(&mut chain).expect("load WorldMapContinent");
        assert_eq!(cat.len(), 4, "Azeroth, Kalimdor, Outland, Northrend");

        let azeroth = cat.get(0).expect("Azeroth (mapId 0)");
        assert_eq!(
            (
                azeroth.left_boundary,
                azeroth.right_boundary,
                azeroth.top_boundary,
                azeroth.bottom_boundary
            ),
            (26, 44, 8, 60)
        );
        assert!((azeroth.offset_x - 16.875).abs() < 0.01);
        assert!((azeroth.offset_y - (-1.5)).abs() < 0.01);
        assert!((azeroth.scale - 0.7).abs() < 0.001);
        assert!((azeroth.taxi_min.0 - (-16530.0)).abs() < 0.1);
        assert!((azeroth.taxi_min.1 - (-16530.0)).abs() < 0.1);
        assert!((azeroth.taxi_max.0 - 12270.0).abs() < 0.1);
        assert!((azeroth.taxi_max.1 - 12270.0).abs() < 0.1);

        let kalimdor = cat.get(1).expect("Kalimdor (mapId 1)");
        assert_eq!(
            (
                kalimdor.left_boundary,
                kalimdor.right_boundary,
                kalimdor.top_boundary,
                kalimdor.bottom_boundary
            ),
            (16, 46, 9, 52)
        );
        assert!((kalimdor.scale - 0.7).abs() < 0.001);

        let outland = cat.get(530).expect("Outland (mapId 530)");
        assert!((outland.scale - 0.75).abs() < 0.001);
        assert!(cat.get(571).is_some(), "Northrend (mapId 571)");
    }
}
