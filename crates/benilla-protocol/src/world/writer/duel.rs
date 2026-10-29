//! The duel sends. There is no start-duel opcode: a challenge is a `CMSG_CAST_SPELL` of the spell
//! whose `Effect[0]` is `SPELL_EFFECT_DUEL`. Both bodies carry the arbiter guid (`AcceptDuel`,
//! `CancelDuel`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Accept a duel (`CMSG_DUEL_ACCEPTED`); the challenger also auto-accepts its own request.
    pub fn duel_accepted(&mut self, arbiter: u64) -> Result<()> {
        self.send(
            opcode::CMSG_DUEL_ACCEPTED,
            &messages::duel_accepted(arbiter),
        )
    }

    /// Decline, cancel or forfeit a duel; the server reads which from the duel's state.
    pub fn duel_cancelled(&mut self, arbiter: u64) -> Result<()> {
        self.send(
            opcode::CMSG_DUEL_CANCELLED,
            &messages::duel_cancelled(arbiter),
        )
    }
}
