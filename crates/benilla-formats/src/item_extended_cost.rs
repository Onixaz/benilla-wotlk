//! `ItemExtendedCost.dbc`: what a vendor row costs beyond copper, by the row's extended-cost id.
//! The merchant bindings read honor (`+4`), arena points (`+8`), the arena bracket (`+0xc`), five
//! required items and their counts (`+0x10`, `+0x24`) and the personal rating (`+0x38`)
//! (`GetMerchantItemCostInfo 0x5846e0`, `GetMerchantItemCostItem 0x584820`, `GetMerchantItemInfo
//! 0x584e10`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const ITEM_EXTENDED_COST: &str = "DBFilesClient\\ItemExtendedCost.dbc";
const ITEM_PURCHASE_GROUP: &str = "DBFilesClient\\ItemPurchaseGroup.dbc";

/// The required-item slots a record carries.
pub const EXTENDED_COST_ITEMS: usize = 5;

/// The item-id slots an `ItemPurchaseGroup.dbc` record carries (`0x62b789`).
pub const PURCHASE_GROUP_ITEMS: usize = 8;

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
    /// `ItemPurchaseGroup.dbc` id (`[cost+0x3c]`), 0 for none.
    pub purchase_group: u32,
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
                    purchase_group: field(r, 15),
                },
            ))
        })
        .collect())
}

/// One `ItemPurchaseGroup.dbc` record (`0x62b732`..`0x62b79c`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurchaseGroupRow {
    /// The eight item ids checked against the player's items; zero entries are skipped.
    pub item_ids: [u32; PURCHASE_GROUP_ITEMS],
    /// The localized display name formatted into `ITEM_REQ_PURCHASE_GROUP`.
    pub name: String,
}

/// `ItemPurchaseGroup.dbc` keyed by id.
#[derive(Clone, Debug, Default)]
pub struct ItemPurchaseGroups(HashMap<u32, PurchaseGroupRow>);

impl ItemPurchaseGroups {
    pub fn get(&self, id: u32) -> Option<&PurchaseGroupRow> {
        self.0.get(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(u32, PurchaseGroupRow)> for ItemPurchaseGroups {
    fn from_iter<I: IntoIterator<Item = (u32, PurchaseGroupRow)>>(rows: I) -> Self {
        Self(rows.into_iter().collect())
    }
}

/// `ItemPurchaseGroup.dbc`: 26 columns, 104 bytes: id, eight item ids, sixteen locale names and
/// their mask.
pub(crate) fn purchase_group_schema() -> Schema {
    let mut s = Schema::new("ItemPurchaseGroup");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new_array(
        "ItemID",
        FieldType::UInt32,
        PURCHASE_GROUP_ITEMS,
    ));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

/// Load `ItemPurchaseGroup.dbc` from the patch chain.
pub fn load_item_purchase_groups(chain: &mut Chain) -> Result<ItemPurchaseGroups> {
    let bytes = chain
        .read_file(ITEM_PURCHASE_GROUP)
        .with_context(|| format!("reading {ITEM_PURCHASE_GROUP}"))?;
    purchase_groups_from(&bytes)
}

fn purchase_groups_from(bytes: &[u8]) -> Result<ItemPurchaseGroups> {
    let rs = parse(bytes, purchase_group_schema(), "ItemPurchaseGroup")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            let id = u32_at(r, 0)?;
            let item_ids = std::array::from_fn(|i| u32_at(r, 1 + i).unwrap_or(0));
            let name = str_at(&rs, r, 9).unwrap_or_default();
            Some((id, PurchaseGroupRow { item_ids, name }))
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
    fn a_record_reads_its_points_items_rating_and_purchase_group() {
        let table = table_from(&synthesize(&[[
            2276, 7000, 150, 1, 20560, 29434, 0, 0, 0, 3, 1, 0, 0, 0, 1800, 1,
        ]]))
        .unwrap();
        let row = table.get(2276).expect("the record");
        assert_eq!((row.honor, row.arena, row.arena_slot), (7000, 150, 1));
        assert_eq!(row.items[0], (20560, 3));
        assert_eq!(row.items[1], (29434, 1));
        assert_eq!(row.items[2], (0, 0));
        assert_eq!(row.personal_rating, 1800);
        assert_eq!(row.purchase_group, 1);
        assert!(table.get(1).is_none());
    }

    #[test]
    fn the_shipped_purchase_group_table_loads() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let groups = load_item_purchase_groups(&mut chain).expect("load ItemPurchaseGroup.dbc");
        assert!(!groups.is_empty(), "shipped table carries at least one row");
    }
}
