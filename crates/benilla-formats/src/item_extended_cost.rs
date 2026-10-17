//! `ItemExtendedCost.dbc`: what a vendor row costs beyond copper, by the row's extended-cost id.
//! The merchant bindings read honor (`+4`), arena points (`+8`), the arena bracket (`+0xc`), five
//! required items and their counts (`+0x10`, `+0x24`) and the personal rating (`+0x38`)
//! (`GetMerchantItemCostInfo 0x5846e0`, `GetMerchantItemCostItem 0x584820`, `GetMerchantItemInfo
//! 0x584e10`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const ITEM_EXTENDED_COST: &str = "DBFilesClient\\ItemExtendedCost.dbc";

/// The required-item slots a record carries.
pub const EXTENDED_COST_ITEMS: usize = 5;

/// One record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtendedCostRow {
    pub honor: u32,
    pub arena: u32,
    /// The arena bracket the rating applies to: 0 two, 1 three, 2 five.
    pub arena_slot: u32,
    /// `(item, count)` per slot, zero ids included, in record order.
    pub items: [(u32, u32); EXTENDED_COST_ITEMS],
    /// The personal arena rating required, 0 for none.
    pub personal_rating: u32,
}

/// The table by id.
#[derive(Clone, Debug, Default)]
pub struct ItemExtendedCosts(HashMap<u32, ExtendedCostRow>);

impl ItemExtendedCosts {
    pub fn get(&self, id: u32) -> Option<&ExtendedCostRow> {
        self.0.get(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(u32, ExtendedCostRow)> for ItemExtendedCosts {
    fn from_iter<I: IntoIterator<Item = (u32, ExtendedCostRow)>>(rows: I) -> Self {
        Self(rows.into_iter().collect())
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("ItemExtendedCost");
    for name in ["ID", "HonorPoints", "ArenaPoints", "ArenaBracket"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new_array(
        "ItemID",
        FieldType::UInt32,
        EXTENDED_COST_ITEMS,
    ));
    s.add_field(SchemaField::new_array(
        "ItemCount",
        FieldType::UInt32,
        EXTENDED_COST_ITEMS,
    ));
    s.add_field(SchemaField::new("RequiredArenaRating", FieldType::UInt32));
    s.add_field(SchemaField::new("ItemPurchaseGroup", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

/// Load `ItemExtendedCost.dbc` from the patch chain.
pub fn load_item_extended_costs(chain: &mut Chain) -> Result<ItemExtendedCosts> {
    let bytes = chain
        .read_file(ITEM_EXTENDED_COST)
        .with_context(|| format!("reading {ITEM_EXTENDED_COST}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<ItemExtendedCosts> {
    let rs = parse(bytes, schema(), "ItemExtendedCost")?;
    let field = |r, i| u32_at(r, i).unwrap_or(0);
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            let id = u32_at(r, 0)?;
            Some((
                id,
                ExtendedCostRow {
                    honor: field(r, 1),
                    arena: field(r, 2),
                    arena_slot: field(r, 3),
                    items: std::array::from_fn(|i| (field(r, 4 + i), field(r, 9 + i))),
                    personal_rating: field(r, 14),
                },
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthesize(rows: &[[u32; 16]]) -> Vec<u8> {
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&16u32.to_le_bytes());
        dbc.extend_from_slice(&64u32.to_le_bytes());
        dbc.extend_from_slice(&1u32.to_le_bytes());
        for row in rows {
            for v in row {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0);
        dbc
    }

    #[test]
    fn a_record_reads_its_points_items_and_rating() {
        let table = table_from(&synthesize(&[[
            2276, 7000, 150, 1, 20560, 29434, 0, 0, 0, 3, 1, 0, 0, 0, 1800, 0,
        ]]))
        .unwrap();
        let row = table.get(2276).expect("the record");
        assert_eq!((row.honor, row.arena, row.arena_slot), (7000, 150, 1));
        assert_eq!(row.items[0], (20560, 3));
        assert_eq!(row.items[1], (29434, 1));
        assert_eq!(row.items[2], (0, 0));
        assert_eq!(row.personal_rating, 1800);
        assert!(table.get(1).is_none());
    }
}
