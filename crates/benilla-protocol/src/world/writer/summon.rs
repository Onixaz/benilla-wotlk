//! The summon answer, the client's one summon send.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_SUMMON_RESPONSE`: the `CONFIRM_SUMMON` answer, `agree` for Accept, not for Cancel or a
    /// dead player's refusal.
    pub fn summon_response(&mut self, summoner_guid: u64, agree: bool) -> Result<()> {
        self.send(
            opcode::CMSG_SUMMON_RESPONSE,
            &messages::summon_response(summoner_guid, agree),
        )
    }
}
