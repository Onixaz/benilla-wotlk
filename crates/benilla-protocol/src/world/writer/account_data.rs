//! The account-data slots' requests and uploads in the world (`0x6b92c0`, `0x6b9100`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_REQUEST_ACCOUNT_DATA`: one slot's bytes, answered by `SMSG_UPDATE_ACCOUNT_DATA`.
    pub fn request_account_data(&mut self, slot: u32) -> Result<()> {
        self.send(
            opcode::CMSG_REQUEST_ACCOUNT_DATA,
            &messages::request_account_data(slot),
        )
    }

    /// `CMSG_UPDATE_ACCOUNT_DATA`: one slot's file bytes under `time`.
    pub fn update_account_data(&mut self, slot: u32, time: u32, bytes: &[u8]) -> Result<()> {
        self.send(
            opcode::CMSG_UPDATE_ACCOUNT_DATA,
            &messages::update_account_data(slot, time, bytes),
        )
    }
}
