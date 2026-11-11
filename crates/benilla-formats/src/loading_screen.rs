//! `LoadingScreens.dbc`: a `Map.dbc` `LoadingScreenID` → the loading art's BLP path (column 2) and
//! WotLK wide-art flag (column 3), in the layout of the client the chain was opened for
//! (`crate::dbc_layout::LoadingScreens`).
//! The open world has one per continent (3 Kalimdor, 4 Eastern Kingdoms); the other rows are per
//! instance.

use std::collections::HashMap;

use crate::dbc_layout::LoadingScreens as LoadingScreensLayout;
use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const LOADING_SCREENS: &str = "DBFilesClient\\LoadingScreens.dbc";

/// `LoadingScreenID` → BLP path.
pub struct LoadingScreenCatalog {
    paths: HashMap<u32, String>,
    wide: HashMap<u32, bool>,
}

impl LoadingScreenCatalog {
    pub fn path(&self, id: u32) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }

    /// Whether the WotLK row permits the adjacent `Wide` loading-screen art.
    pub fn has_wide_screen(&self, id: u32) -> bool {
        self.wide.get(&id).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// The shared columns typed; the rest are `u32` placeholders, so only the record has to add up.
pub(crate) fn schema(l: &LoadingScreensLayout) -> Schema {
    let mut s = Schema::new("LoadingScreens");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("FileName", FieldType::String));
    for i in 3..l.fields {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// Read LoadingScreens.dbc off the patch chain into a [`LoadingScreenCatalog`].
pub fn load_loading_screens(chain: &mut Chain) -> Result<LoadingScreenCatalog> {
    let bytes = chain
        .read_file(LOADING_SCREENS)
        .with_context(|| format!("reading {LOADING_SCREENS}"))?;
    let rs = parse(
        &bytes,
        schema(&crate::dbc_layout::LAYOUTS.loading_screens),
        "LoadingScreens",
    )?;
    let mut paths = HashMap::with_capacity(rs.records().len());
    let mut wide = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 2)) {
            paths.insert(id, path);
            wide.insert(id, u32_at(r, 3).is_some_and(|v| v != 0));
        }
    }
    Ok(LoadingScreenCatalog { paths, wide })
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::load_map_catalog;

    #[test]
    fn resolves_open_world_loading_art() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let maps = load_map_catalog(&mut chain).expect("Map.dbc");
        let screens = load_loading_screens(&mut chain).expect("LoadingScreens.dbc");

        assert_eq!(maps.loading_screen_id(0), Some(4), "Azeroth → 4");
        assert_eq!(maps.loading_screen_id(1), Some(3), "Kalimdor → 3");

        assert_eq!(
            screens.path(4),
            Some("Interface\\Glues\\LoadingScreens\\LoadScreenEasternKingdom.blp")
        );
        assert_eq!(
            screens.path(3),
            Some("Interface\\Glues\\LoadingScreens\\LoadScreenKalimdor.blp")
        );
        assert!(screens.has_wide_screen(3), "Kalimdor has WotLK wide art");
        assert!(
            screens.has_wide_screen(4),
            "Eastern Kingdoms has WotLK wide art"
        );
        assert!(
            !screens.has_wide_screen(1),
            "instance rows have no wide art"
        );

        let resolve = |map_id| {
            maps.loading_screen_id(map_id)
                .and_then(|id| screens.path(id))
        };
        assert!(resolve(0)
            .unwrap()
            .ends_with("LoadScreenEasternKingdom.blp"));
        assert!(resolve(1).unwrap().ends_with("LoadScreenKalimdor.blp"));
        assert!(resolve(571).unwrap().ends_with("LoadScreenNorthrend.blp"));
    }
}
