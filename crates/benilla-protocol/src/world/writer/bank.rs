//! The bank sends. vmangos routes `CMSG_AUTOBANK_ITEM` and `CMSG_AUTOSTORE_BANK_ITEM` by whether
//! the source is a bank position, so either moves an item across the bank boundary.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Open the bank (`CMSG_BANKER_ACTIVATE`, full banker guid), answered by `SMSG_SHOW_BANK`.
    pub fn banker_activate(&mut self, banker_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_BANKER_ACTIVATE,
            &messages::banker_activate(banker_guid),
        )
    }

    /// Right-click a spellclick NPC (`CMSG_SPELLCLICK`, full guid): a vehicle to board, a unit's
    /// click spell.
    pub fn spell_click(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_SPELLCLICK, &guid.to_le_bytes())
    }

    /// Buy the next bank-bag slot; success is silent, failure answers `SMSG_BUY_BANK_SLOT_RESULT`.
    pub fn buy_bank_slot(&mut self, banker_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_BANK_SLOT,
            &messages::buy_bank_slot(banker_guid),
        )
    }

    /// Deposit the item at wire `(bag, slot)` into the bank (`CMSG_AUTOBANK_ITEM`).
    pub fn autobank_item(&mut self, bag: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOBANK_ITEM,
            &messages::autobank_item(bag, slot),
        )
    }

    /// Withdraw the bank item at wire `(bag, slot)` into the bags (`CMSG_AUTOSTORE_BANK_ITEM`).
    pub fn autostore_bank_item(&mut self, bag: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOSTORE_BANK_ITEM,
            &messages::autostore_bank_item(bag, slot),
        )
    }
}
