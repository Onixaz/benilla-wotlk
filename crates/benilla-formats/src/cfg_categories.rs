//! `Cfg_Categories.dbc`, the realm list's category tabs. 12340's category set is every row in file
//! order (`0x4df580` walks the whole table, no Region filter): column 0 is the id a realm's wire
//! category byte is looked up by (`0x4db58f`), and the row's localized name, column 4, is the
//! tab's label (`GetRealmCategories`, `0x4df110`, reads the in-memory record's `+0x10`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const CFG_CATEGORIES: &str = "DBFilesClient\\Cfg_Categories.dbc";

/// One realm category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmCategory {
    /// Column 0, compared for equality with a realm's category byte.
    pub id: u32,
    /// The enUS `Name_lang` slot, the column the reference reads on an enUS or enGB client.
    pub name: String,
}

/// `ID`, `LocaleMask`, `CreateCharsetMask`, `Flags`, then `Name_lang` over sixteen locales and
/// its mask: 21 columns of 84 bytes.
fn cfg_categories_schema() -> Schema {
    let mut s = Schema::new("Cfg_Categories");
    for name in ["ID", "LocaleMask", "CreateCharsetMask", "Flags"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Every category, in file order.
pub fn load_realm_categories(chain: &mut Chain) -> Result<Vec<RealmCategory>> {
    let bytes = chain
        .read_file(CFG_CATEGORIES)
        .with_context(|| format!("reading {CFG_CATEGORIES}"))?;
    let rs = parse(&bytes, cfg_categories_schema(), "Cfg_Categories")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(RealmCategory {
                id: u32_at(r, 0)?,
                // An empty name is pushed as the empty string; only a missing row reads "UNKNOWN".
                name: str_at(&rs, r, 4).unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Option<Vec<(u32, String)>> {
        let data = crate::wow_data_or_skip!(None);
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cats = load_realm_categories(&mut chain).expect("Cfg_Categories.dbc");
        Some(cats.into_iter().map(|c| (c.id, c.name)).collect())
    }

    /// The 37 shipped rows, ids 1 to 37 in file order: Development, United States, Oceanic.
    #[test]
    fn every_row_is_a_category_in_file_order() {
        let Some(all) = rows() else { return };
        assert_eq!(all.len(), 37);
        assert!(all
            .iter()
            .enumerate()
            .all(|(i, (id, _))| *id == i as u32 + 1));
        assert_eq!(all[0].1, "Development");
        assert_eq!(all[1].1, "United States");
        assert_eq!(all[2].1, "Oceanic");
        assert_eq!(all[7].1, "English", "Europe's language tabs follow");
    }
}
