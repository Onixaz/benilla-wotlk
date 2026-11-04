//! `ItemLimitCategory.dbc`: the shared carry limits items name by category.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_LIMIT_CATEGORY: &str = "DBFilesClient\\ItemLimitCategory.dbc";

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLimitCategory {
    pub id: u32,
    /// The enUS `Name_Lang` slot, the `%s` of the limit-category lines.
    pub name: String,
    /// `Quantity` (column 18): how many the category allows, their `%d`.
    pub quantity: u32,
    /// `Flags` (column 19).
    pub flags: u32,
}

/// Every row, by id.
#[derive(Debug, Clone, Default)]
pub struct ItemLimitCategoryCatalog {
    rows: HashMap<u32, ItemLimitCategory>,
}

impl ItemLimitCategoryCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = ItemLimitCategory>) -> Self {
        Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&ItemLimitCategory> {
        self.rows.get(&id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn item_limit_category_schema() -> Schema {
    let mut s = Schema::new("ItemLimitCategory");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("Quantity", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Load `ItemLimitCategory.dbc` from the patch chain.
pub fn load_item_limit_categories(chain: &mut Chain) -> Result<ItemLimitCategoryCatalog> {
    let bytes = chain
        .read_file(ITEM_LIMIT_CATEGORY)
        .with_context(|| format!("reading {ITEM_LIMIT_CATEGORY}"))?;
    let rs = parse(&bytes, item_limit_category_schema(), "ItemLimitCategory")?;
    let rows = rs.records().iter().filter_map(|r| {
        Some(ItemLimitCategory {
            id: u32_at(r, 0)?,
            name: str_at(&rs, r, 1).unwrap_or_default(),
            quantity: u32_at(r, 18)?,
            flags: u32_at(r, 19)?,
        })
    });
    Ok(ItemLimitCategoryCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_rows_carry_a_name_and_a_quantity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_limit_categories(&mut chain).expect("ItemLimitCategory.dbc");
        assert!(!cat.is_empty());
        let named = cat
            .rows
            .values()
            .filter(|r| !r.name.is_empty() && r.quantity > 0)
            .count();
        assert!(named > 0, "some row names a category with a limit");
    }
}
