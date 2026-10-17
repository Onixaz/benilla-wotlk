//! The vendor sends. Buying names the item entry and the row's slot; selling and repairing name the
//! item guid in our bags.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_LIST_INVENTORY`: answered by `SMSG_LIST_INVENTORY`.
    pub fn list_inventory(&mut self, vendor_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_LIST_INVENTORY,
            &messages::list_inventory(vendor_guid),
        )
    }

    /// `CMSG_BUY_ITEM`: `count` stacks of row `slot` into the first free bag slot; answered by `SMSG_BUY_ITEM`
    /// with the new stock, or by `SMSG_BUY_FAILED`.
    pub fn buy_item(&mut self, vendor_guid: u64, entry: u32, slot: u32, count: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_ITEM,
            &messages::buy_item(vendor_guid, entry, slot, count),
        )
    }

    /// `CMSG_BUY_ITEM_IN_SLOT`: the merchant cursor's drop. `bag_guid` is the container's guid, or
    /// the player's own for the backpack and the equipment slots.
    pub fn buy_item_in_slot(
        &mut self,
        vendor_guid: u64,
        entry: u32,
        slot: u32,
        bag_guid: u64,
        bag_slot: u8,
        count: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_ITEM_IN_SLOT,
            &messages::buy_item_in_slot(vendor_guid, entry, slot, bag_guid, bag_slot, count),
        )
    }

    /// `CMSG_SELL_ITEM`: `count` 0 sells the whole stack. Success is silent; a refusal comes back
    /// as `SMSG_SELL_ITEM`'s error shape.
    pub fn sell_item(&mut self, vendor_guid: u64, item_guid: u64, count: u32) -> Result<()> {
        self.send(
            opcode::CMSG_SELL_ITEM,
            &messages::sell_item(vendor_guid, item_guid, count),
        )
    }

    /// `CMSG_BUYBACK_ITEM`: `slot` is the absolute player-array buyback slot, 74 to 85; a refusal
    /// is `SMSG_BUY_FAILED`.
    pub fn buyback_item(&mut self, vendor_guid: u64, slot: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BUYBACK_ITEM,
            &messages::buyback_item(vendor_guid, slot),
        )
    }

    /// `CMSG_REPAIR_ITEM`: `item_guid` 0 repairs everything, from the guild bank with `guild_bank`;
    /// there is no reply packet.
    pub fn repair_item(
        &mut self,
        vendor_guid: u64,
        item_guid: u64,
        guild_bank: bool,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_REPAIR_ITEM,
            &messages::repair_item(vendor_guid, item_guid, guild_bank),
        )
    }
}
