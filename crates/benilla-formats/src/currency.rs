//! The currency tab's tables, as 12340's currency list reads them:
//!
//! - `CurrencyTypes.dbc` (`ID`, `ItemID`, `CategoryID`, `BitIndex`): each currency's item, its
//!   category and its `PLAYER_FIELD_KNOWN_CURRENCIES` bit, 1-based (`0x5b0030`).
//! - `CurrencyCategory.dbc` (`ID`, `Flags`, `Name` loc-string): the headers; `Flags & 1` sorts a
//!   category last and makes it the unused bucket (`0x5afc40`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One `CurrencyTypes.dbc` row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CurrencyType {
    pub item_id: u32,
    pub category_id: u32,
    /// The known-currencies bit, 1-based; 0 never shows.
    pub bit_index: i32,
}

/// One `CurrencyCategory.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurrencyCategory {
    pub id: u32,
    pub flags: u32,
    pub name: String,
}

/// Both tables, each in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurrencyTables {
    pub types: Vec<CurrencyType>,
    pub categories: Vec<CurrencyCategory>,
}

pub(crate) fn types_schema() -> Schema {
    let mut s = Schema::new("CurrencyTypes");
    for name in ["ID", "ItemID", "CategoryID", "BitIndex"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.set_key_field("ID");
    s
}

/// `CurrencyCategory.dbc`: 19 columns, the name a 17-column loc-string at 2.
pub(crate) fn categories_schema() -> Schema {
    let mut s = Schema::new("CurrencyCategory");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

/// Load both tables through the patch chain.
pub fn load_currency_tables(chain: &mut Chain) -> Result<CurrencyTables> {
    let read = |chain: &mut Chain, file: &str| {
        chain
            .read_file(file)
            .with_context(|| format!("reading {file}"))
    };
    let rs = parse(
        &read(chain, "DBFilesClient\\CurrencyTypes.dbc")?,
        types_schema(),
        "CurrencyTypes",
    )?;
    let types = rs
        .records()
        .iter()
        .map(|r| CurrencyType {
            item_id: u32_at(r, 1).unwrap_or(0),
            category_id: u32_at(r, 2).unwrap_or(0),
            bit_index: u32_at(r, 3).unwrap_or(0) as i32,
        })
        .collect();
    let rs = parse(
        &read(chain, "DBFilesClient\\CurrencyCategory.dbc")?,
        categories_schema(),
        "CurrencyCategory",
    )?;
    let categories = rs
        .records()
        .iter()
        .map(|r| CurrencyCategory {
            id: u32_at(r, 0).unwrap_or(0),
            flags: u32_at(r, 1).unwrap_or(0),
            name: str_at(&rs, r, 2).unwrap_or_default(),
        })
        .collect();
    Ok(CurrencyTables { types, categories })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped tables load: every type names an item and a bit in the 64-bit field, most name
    /// a category the other table has, and one category carries the unused flag.
    #[test]
    fn the_shipped_currency_tables_load() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).unwrap();
        let t = load_currency_tables(&mut chain).unwrap();
        assert!(!t.types.is_empty() && !t.categories.is_empty());
        for ty in &t.types {
            assert!(ty.item_id != 0);
            assert!((0..=64).contains(&ty.bit_index), "{ty:?}");
        }
        let grouped = t
            .types
            .iter()
            .filter(|ty| t.categories.iter().any(|c| c.id == ty.category_id))
            .count();
        assert!(
            grouped * 2 > t.types.len(),
            "{grouped} of {}",
            t.types.len()
        );
        assert!(t.categories.iter().any(|c| c.flags & 1 != 0));
        assert!(t.categories.iter().all(|c| !c.name.is_empty()));
    }
}
