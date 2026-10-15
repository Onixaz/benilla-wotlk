//! `TotemCategory.dbc`: the tool kinds a spell or a totem slot asks the player to carry.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const TOTEM_CATEGORY: &str = "DBFilesClient\\TotemCategory.dbc";

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotemCategory {
    pub id: u32,
    /// The enUS `Name_Lang` slot, `ERR_MULTI_CAST_ACTION_TOTEM_S`'s `%s`.
    pub name: String,
    /// `CategoryType` (column 18): only rows of one type cover each other.
    pub category_type: u32,
    /// `CategoryMask` (column 19): the bits this row stands for.
    pub mask: u32,
}

/// Every row, by id.
#[derive(Debug, Clone, Default)]
pub struct TotemCategoryCatalog {
    rows: HashMap<u32, TotemCategory>,
}

impl TotemCategoryCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = TotemCategory>) -> Self {
        Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&TotemCategory> {
        self.rows.get(&id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether `carried` rows of `required`'s type clear all its mask bits (`0x7541f0`).
    pub fn covered_by(&self, required: u32, carried: impl IntoIterator<Item = u32>) -> bool {
        let Some(want) = self.get(required) else {
            return false;
        };
        let mut need = want.mask;
        for row in carried.into_iter().filter_map(|c| self.get(c)) {
            if row.category_type == want.category_type {
                need &= !row.mask;
                if need == 0 {
                    return true;
                }
            }
        }
        false
    }
}

pub(crate) fn totem_category_schema() -> Schema {
    let mut s = Schema::new("TotemCategory");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.add_field(SchemaField::new("CategoryType", FieldType::UInt32));
    s.add_field(SchemaField::new("CategoryMask", FieldType::UInt32));
    s
}

/// Load `TotemCategory.dbc` from the patch chain.
pub fn load_totem_categories(chain: &mut Chain) -> Result<TotemCategoryCatalog> {
    let bytes = chain
        .read_file(TOTEM_CATEGORY)
        .with_context(|| format!("reading {TOTEM_CATEGORY}"))?;
    let rs = parse(&bytes, totem_category_schema(), "TotemCategory")?;
    let rows = rs.records().iter().filter_map(|r| {
        Some(TotemCategory {
            id: u32_at(r, 0)?,
            name: str_at(&rs, r, 1).unwrap_or_default(),
            category_type: u32_at(r, 18)?,
            mask: u32_at(r, 19)?,
        })
    });
    Ok(TotemCategoryCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u32, category_type: u32, mask: u32) -> TotemCategory {
        TotemCategory {
            id,
            name: String::new(),
            category_type,
            mask,
        }
    }

    #[test]
    fn a_tool_covers_its_own_row_and_a_master_tool_covers_every_element() {
        // Earth, Air, Fire, Water and Master Totem, all of type 2; a type-3 rod beside them.
        let cat = TotemCategoryCatalog::from_rows([
            row(2, 2, 0x1),
            row(3, 2, 0x2),
            row(4, 2, 0x4),
            row(5, 2, 0x8),
            row(21, 2, 0xf),
            row(6, 3, 0x1),
        ]);
        assert!(cat.covered_by(2, [2]));
        assert!(!cat.covered_by(4, [2]), "Earth Totem is not Fire Totem");
        assert!(cat.covered_by(4, [6, 2, 4]), "the walk reaches the match");
        for element in [2, 3, 4, 5] {
            assert!(
                cat.covered_by(element, [21]),
                "Master Totem covers {element}"
            );
        }
        assert!(
            !cat.covered_by(21, [2, 3, 4]),
            "three elements leave the fourth bit"
        );
        assert!(
            cat.covered_by(21, [2, 3, 4, 5]),
            "four tools together cover the master row"
        );
        assert!(!cat.covered_by(2, [6]), "a rod is another type");
        assert!(!cat.covered_by(99, [2]), "an unknown row is never covered");
    }

    #[test]
    fn the_shipped_element_rows_share_one_type_and_one_bit_each() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_totem_categories(&mut chain).expect("TotemCategory.dbc");
        let earth = cat.get(2).expect("row 2");
        for (id, bit) in [(2, 0x1), (3, 0x2), (4, 0x4), (5, 0x8)] {
            let r = cat.get(id).expect("element row");
            assert_eq!(r.category_type, earth.category_type, "row {id}");
            assert_eq!(r.mask, bit, "row {id}");
            assert!(!r.name.is_empty(), "row {id} has a name");
        }
        assert_eq!(cat.get(21).expect("row 21").mask, 0xf);
    }
}
