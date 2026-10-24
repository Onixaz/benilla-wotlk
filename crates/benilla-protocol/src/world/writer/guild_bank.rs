//! The guild bank sends, encoded by [`messages::GuildBankSend`].

use anyhow::Result;

use crate::messages;

use super::WorldWriter;

impl WorldWriter {
    /// One guild bank send.
    pub fn guild_bank(&mut self, send: &messages::GuildBankSend) -> Result<()> {
        let (opcode, body) = send.encode();
        self.send(opcode, &body)
    }
}
