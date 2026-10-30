//! `GemProperties.dbc`: a gem's socket enchant and colour, which the item tooltip's socket bonus
//! matches against each socket's colour.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const GEM_PROPERTIES: &str = "DBFilesClient\\GemProperties.dbc";

/// The gem colours by the enchant a socketed gem leaves in its slot.
#[derive(Debug, Clone, Default)]
pub struct GemPropertiesCatalog {
    types: HashMap<u32, u32>,
}

impl GemPropertiesCatalog {
    /// Build from `(enchant id, type)` rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, u32)>) -> Self {
        Self {
            types: rows.into_iter().collect(),
        }
    }

    /// `Type`, the colour mask (1 meta, 2 red, 4 yellow, 8 blue), of the gem behind a socket
    /// enchant.
    pub fn gem_type(&self, enchant_id: u32) -> Option<u32> {
        self.types.get(&enchant_id).copied()
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

pub(crate) fn gem_properties_schema() -> Schema {
    let mut s = Schema::new("GemProperties");
    for name in ["ID", "EnchantID", "MaxcountInv", "MaxcountItem", "Type"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load `GemProperties.dbc` from the patch chain, keyed by the gem's enchant.
pub fn load_gem_properties(chain: &mut Chain) -> Result<GemPropertiesCatalog> {
    let bytes = chain
        .read_file(GEM_PROPERTIES)
        .with_context(|| format!("reading {GEM_PROPERTIES}"))?;
    let rs = parse(&bytes, gem_properties_schema(), "GemProperties")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 1)?, u32_at(r, 4)?)))
        .filter(|&(enchant, _)| enchant != 0);
    Ok(GemPropertiesCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_enchant_names_one_gem_colour() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain.read_file(GEM_PROPERTIES).expect("read");
        let rs = parse(&bytes, gem_properties_schema(), "GemProperties").expect("parse");
        // The tooltip keys a socketed gem's colour by its enchant, so no two colours may share one.
        let mut seen: HashMap<u32, u32> = HashMap::new();
        for r in rs.records() {
            let (enchant, ty) = (u32_at(r, 1).unwrap(), u32_at(r, 4).unwrap());
            if enchant == 0 {
                continue;
            }
            if let Some(&prev) = seen.get(&enchant) {
                assert_eq!(prev, ty, "enchant {enchant} names two gem colours");
            }
            seen.insert(enchant, ty);
        }
        let cat = load_gem_properties(&mut chain).expect("load");
        assert!(cat.len() > 100, "{} gem enchants", cat.len());
    }
}
