//! The instance family's sends: "Reset all instances", the lock warning's answer and the
//! difficulty choices.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_RESET_INSTANCES`, empty: the `CONFIRM_RESET_INSTANCES` dialog's Yes. The server sends
    /// one `SMSG_INSTANCE_RESET` per map it reset and a refusal per map it could not.
    pub fn reset_instances(&mut self) -> Result<()> {
        self.send(opcode::CMSG_RESET_INSTANCES, &messages::reset_instances())
    }

    /// `CMSG_INSTANCE_LOCK_RESPONSE`: the `INSTANCE_LOCK` dialog's Accept or Leave.
    pub fn instance_lock_response(&mut self, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_INSTANCE_LOCK_RESPONSE,
            &messages::instance_lock_response(accept),
        )
    }

    /// `MSG_SET_DUNGEON_DIFFICULTY` from the client: the 0-based difficulty.
    pub fn set_dungeon_difficulty(&mut self, difficulty: u32) -> Result<()> {
        self.send(
            opcode::MSG_SET_DUNGEON_DIFFICULTY,
            &messages::set_difficulty(difficulty),
        )
    }

    /// `MSG_SET_RAID_DIFFICULTY` from the client: the 0-based difficulty.
    pub fn set_raid_difficulty(&mut self, difficulty: u32) -> Result<()> {
        self.send(
            opcode::MSG_SET_RAID_DIFFICULTY,
            &messages::set_difficulty(difficulty),
        )
    }
}
