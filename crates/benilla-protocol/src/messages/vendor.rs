//! Vendor messages: the inventory list, buy and sell (opcodes 414-421), buyback and repair.

use std::io;

use crate::wire::{capacity_hint, read_u32_le, read_u64_le, read_u8};

/// One vendor row, the 32-byte record `0x6d9960` copies into `0xc9d7d0`: `slot` is the 1-based
/// list position a buy sends back; `current_count` `0xFFFF_FFFF` is unlimited; `price` is already
/// reputation-discounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorItem {
    pub slot: u32,
    pub entry: u32,
    /// `item_template.display_id` → icon via `ItemDisplayInfo.dbc`.
    pub display_id: u32,
    pub current_count: u32,
    /// Buy price in copper.
    pub price: u32,
    pub max_durability: u32,
    /// Stack size delivered per purchase (`item_template.buy_count`).
    pub buy_count: u32,
    /// `ItemExtendedCost.dbc` id, 0 for a row sold for copper alone (`+0x1c`).
    pub extended_cost: u32,
}

/// The most rows `SMSG_LIST_INVENTORY` carries: `0x6d999f` rejects a count above 150.
pub const VENDOR_ROWS_MAX: u8 = 150;

/// `BuyResult`: the `u8` reason on `SMSG_BUY_FAILED`, as `0x6cdba0`'s switch reads it.
pub mod buy_result {
    pub const CANT_FIND_ITEM: u8 = 0;
    pub const ITEM_ALREADY_SOLD: u8 = 1;
    pub const NOT_ENOUGH_MONEY: u8 = 2;
    pub const SELLER_DONT_LIKE_YOU: u8 = 4;
    pub const DISTANCE_TOO_FAR: u8 = 5;
    pub const ITEM_SOLD_OUT: u8 = 7;
    pub const CANT_CARRY_MORE: u8 = 8;
    pub const RANK_REQUIRE: u8 = 11;
    pub const REPUTATION_REQUIRE: u8 = 12;
}

/// `SellResult`: the `u8` reason on `SMSG_SELL_ITEM`, 7 to 9 added by 3.3.5a (`0x6d157c`).
pub mod sell_result {
    pub const CANT_FIND_ITEM: u8 = 1;
    pub const CANT_SELL_ITEM: u8 = 2;
    pub const CANT_FIND_VENDOR: u8 = 3;
    pub const YOU_DONT_OWN_THAT_ITEM: u8 = 4;
    pub const UNK: u8 = 5;
    pub const ONLY_EMPTY_BAG: u8 = 6;
    pub const CANT_SELL_TO_THIS_MERCHANT: u8 = 7;
    pub const MUST_REPAIR_DURABILITY: u8 = 8;
    pub const INTERNAL_BAG_ERROR: u8 = 9;
}

/// Body of `CMSG_LIST_INVENTORY`: the vendor guid.
pub fn list_inventory(vendor_guid: u64) -> Vec<u8> {
    vendor_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_BUY_ITEM` (`0x6d2de0`): `u64 vendorGuid`, `u32` item entry, `u32` row slot, `u32`
/// count (stacks), `u8` 1, which `BuyMerchantItem` always passes (`0x5855a2`).
pub fn buy_item(vendor_guid: u64, entry: u32, slot: u32, count: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(21);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&slot.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.push(1);
    body
}

/// Body of `CMSG_BUY_ITEM_IN_SLOT` (`0x6d2ea0`): `u64 vendorGuid`, `u32` item entry, `u32` row
/// slot, `u64 bagGuid` (ours for the backpack), `u8 bagSlot`, `u32 count`; a row dropped into a
/// slot sends count 1 (`0x5d834b`).
pub fn buy_item_in_slot(
    vendor_guid: u64,
    entry: u32,
    slot: u32,
    bag_guid: u64,
    bag_slot: u8,
    count: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(29);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&slot.to_le_bytes());
    body.extend_from_slice(&bag_guid.to_le_bytes());
    body.push(bag_slot);
    body.extend_from_slice(&count.to_le_bytes());
    body
}

/// Body of `CMSG_SELL_ITEM` (`0x6d2d40`): `u64 vendorGuid, u64 itemGuid, u32 count`, where 0 sells
/// the whole stack.
pub fn sell_item(vendor_guid: u64, item_guid: u64, count: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&item_guid.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body
}

/// Body of `CMSG_BUYBACK_ITEM` (`0x5855c0`): `u64 vendorGuid, u32 slot`, the absolute buyback slot
/// 74-85 the sort `0x584bb0` stored, not re-based.
pub fn buyback_item(vendor_guid: u64, slot: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&slot.to_le_bytes());
    body
}

/// Body of `CMSG_REPAIR_ITEM` (`0x585ebf`): `u64 vendorGuid, u64 itemGuid, u8 guildBank`; item guid
/// 0 repairs everything, and only `RepairAllItems(1)` sets the guild-bank byte.
pub fn repair_item(vendor_guid: u64, item_guid: u64, guild_bank: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(17);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.extend_from_slice(&item_guid.to_le_bytes());
    body.push(u8::from(guild_bank));
    body
}

/// What `SMSG_LIST_INVENTORY` carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorList {
    pub vendor: u64,
    pub items: Vec<VendorItem>,
    /// An empty list's `u8` reason (0 no inventory, 1 hates you, 2 too far, 3 vendor dead, 4 you
    /// are dead); `None` with rows.
    pub error: Option<u8>,
}

impl VendorList {
    /// Whether the window opens: rows, or the empty list's code 0 (`0x6d9ad0`).
    pub fn opens(&self) -> bool {
        !self.items.is_empty() || self.error == Some(0)
    }
}

/// Read `SMSG_LIST_INVENTORY` (`0x6d9960`): `u64 vendorGuid, u8 count` (at most
/// [`VENDOR_ROWS_MAX`]), then `count` 32-byte rows; an empty list carries one `u8` reason.
pub(super) fn read_list_inventory(r: &mut &[u8]) -> io::Result<VendorList> {
    let vendor = read_u64_le(r)?;
    let count = read_u8(r)?;
    if count > VENDOR_ROWS_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SMSG_LIST_INVENTORY: {count} rows exceed {VENDOR_ROWS_MAX}"),
        ));
    }
    if count == 0 {
        return Ok(VendorList {
            vendor,
            items: Vec::new(),
            error: Some(read_u8(r)?),
        });
    }
    let mut items = Vec::with_capacity(capacity_hint(count, usize::from(VENDOR_ROWS_MAX)));
    for _ in 0..count {
        items.push(VendorItem {
            slot: read_u32_le(r)?,
            entry: read_u32_le(r)?,
            display_id: read_u32_le(r)?,
            current_count: read_u32_le(r)?,
            price: read_u32_le(r)?,
            max_durability: read_u32_le(r)?,
            buy_count: read_u32_le(r)?,
            extended_cost: read_u32_le(r)?,
        });
    }
    Ok(VendorList {
        vendor,
        items,
        error: None,
    })
}

/// Read `SMSG_BUY_ITEM` (`0x6cdd10`): `u64 vendorGuid, u32 vendorSlot` (1-based), `u32 newCount`
/// (`0xFFFF_FFFF` unlimited), `u32 purchaseCount`; the item arrives separately.
pub(super) fn read_buy_item(r: &mut &[u8]) -> io::Result<(u64, u32, u32, u32)> {
    Ok((
        read_u64_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
    ))
}

/// Read `SMSG_SELL_ITEM` (`0x6d1540`): `u64 vendorGuid, u64 itemGuid, u8 reason` (a
/// [`sell_result`] code). Only a failed sell sends it; a sale shows only in `UPDATE_OBJECT`.
pub(super) fn read_sell_item(r: &mut &[u8]) -> io::Result<(u64, u64, u8)> {
    Ok((read_u64_le(r)?, read_u64_le(r)?, read_u8(r)?))
}

/// Read `SMSG_BUY_FAILED` (`0x6cdba0`): `u64 vendorGuid, u32 item, u8 reason` (a [`buy_result`]
/// code).
pub(super) fn read_buy_failed(r: &mut &[u8]) -> io::Result<(u64, u32, u8)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?, read_u8(r)?))
}
