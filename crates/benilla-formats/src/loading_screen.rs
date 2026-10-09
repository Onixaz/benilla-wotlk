//! `LoadingScreens.dbc`: a `Map.dbc` `LoadingScreenID` → the loading art's BLP path (column 2), in
//! the layout of the client the chain was opened for (`benilla_expansions::dbc::LoadingScreens`).
//! The open world has one per continent (3 Kalimdor, 4 Eastern Kingdoms); the other rows are per
//! instance.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};
use benilla_expansions::dbc::LoadingScreens as LoadingScreensLayout;

use crate::dbc::{parse, str_at, u32_at};

const LOADING_SCREENS: &str = "DBFilesClient\\LoadingScreens.dbc";

/// `LoadingScreenID` → BLP path.
pub struct LoadingScreenCatalog {
    paths: HashMap<u32, String>,
}

impl LoadingScreenCatalog {
    pub fn path(&self, id: u32) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// The shared columns typed; the rest are `u32` placeholders, so only the record has to add up.
fn schema(l: &LoadingScreensLayout) -> Schema {
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
        schema(&chain.expansion().profile().dbc.loading_screens),
        "LoadingScreens",
    )?;
    let mut paths = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 2)) {
            paths.insert(id, path);
        }
    }
    Ok(LoadingScreenCatalog { paths })
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
        let wotlk = chain.expansion() == benilla_expansions::Expansion::Wotlk;

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

        let resolve = |map_id| {
            maps.loading_screen_id(map_id)
                .and_then(|id| screens.path(id))
        };
        assert!(resolve(0)
            .unwrap()
            .ends_with("LoadScreenEasternKingdom.blp"));
        assert!(resolve(1).unwrap().ends_with("LoadScreenKalimdor.blp"));
        if wotlk {
            assert!(resolve(571).unwrap().ends_with("LoadScreenNorthrend.blp"));
        }
    }
}
