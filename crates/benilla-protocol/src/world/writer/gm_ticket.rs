//! The GM ticket sends. The client's own gates (an open ticket for an edit or delete, a GM answer
//! for a resolve) run before these, in the app; the server's answers are the only truth.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// File a ticket (`CMSG_GMTICKET_CREATE`, sender `0x5ac8f0`), from `map` and `pos` where the
    /// player stands; `more_help` is the follow-up to a GM's answer. Answered by
    /// `SMSG_GMTICKET_CREATE`.
    pub fn gm_ticket_create(
        &mut self,
        map: u32,
        pos: [f32; 3],
        text: &str,
        need_response: bool,
        more_help: bool,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_GMTICKET_CREATE,
            &messages::gm_ticket_create(map, pos, text, need_response, more_help),
        )
    }

    /// Edit the open ticket's text (sender `0x5acb50`).
    pub fn gm_ticket_updatetext(&mut self, text: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GMTICKET_UPDATETEXT,
            &messages::gm_ticket_updatetext(text),
        )
    }

    /// Ask for the open ticket (`0x5acbf0`): on world entry, every 10 minutes with the toast up,
    /// and after a landed write.
    pub fn gm_ticket_get(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GMTICKET_GETTICKET, &[])
    }

    /// Abandon the open ticket (`0x5acc60`); the reply carries 9.
    pub fn gm_ticket_delete(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GMTICKET_DELETETICKET, &[])
    }

    /// Ask whether the ticket queue is open, as `GetGMStatus` (`0x5ad1c0`) does.
    pub fn gm_ticket_system_status(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GMTICKET_SYSTEMSTATUS, &[])
    }

    /// Accept a GM's answer as resolving the ticket (`0x5acce0`).
    pub fn gm_response_resolve(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GMRESPONSE_RESOLVE, &[])
    }
}
