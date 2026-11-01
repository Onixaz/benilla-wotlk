//! The small notices' sends: the UI clock request and the level grant accept.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Ask for the server's UI clock (empty body), answered by `SMSG_WORLD_STATE_UI_TIMER_UPDATE`.
    pub fn ui_time_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_WORLD_STATE_UI_TIMER_UPDATE, &[])
    }

    /// Accept a refer-a-friend level grant from `granter`.
    pub fn accept_level_grant(&mut self, granter: u64) -> Result<()> {
        self.send(
            opcode::CMSG_ACCEPT_LEVEL_GRANT,
            &messages::accept_level_grant(granter),
        )
    }
}
