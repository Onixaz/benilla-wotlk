//! The auto-attack sends. The client runs at most one auto-attack, so switching between melee and
//! ranged hands off between `CMSG_ATTACKSTOP` and `CMSG_CANCEL_AUTO_REPEAT_SPELL`.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Start melee auto-attack (`CMSG_ATTACKSWING`, full guid), echoed as `SMSG_ATTACKSTART`.
    pub fn attack_swing(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_ATTACKSWING, &messages::attack_swing(guid))
    }

    /// Stop melee auto-attack (`CMSG_ATTACKSTOP`, empty body). Echoed as `SMSG_ATTACKSTOP`.
    pub fn attack_stop(&mut self) -> Result<()> {
        self.send(opcode::CMSG_ATTACKSTOP, &[])
    }

    /// Stop our ranged auto-repeat; the reference sends it on every local cancel.
    pub fn cancel_auto_repeat(&mut self) -> Result<()> {
        self.send(opcode::CMSG_CANCEL_AUTO_REPEAT_SPELL, &[])
    }
}
