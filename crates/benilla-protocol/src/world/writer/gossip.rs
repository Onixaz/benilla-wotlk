//! The gossip sends. `CMSG_GOSSIP_HELLO` opens an NPC's gossip menu, and selecting an option can
//! open any of its service windows.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Open a gossip menu on an NPC (`CMSG_GOSSIP_HELLO`), answered by `SMSG_GOSSIP_MESSAGE`.
    pub fn gossip_hello(&mut self, npc_guid: u64) -> Result<()> {
        self.send(opcode::CMSG_GOSSIP_HELLO, &messages::gossip_hello(npc_guid))
    }

    /// Choose an option of menu `menu_id` by its echoed `index`; `code` is sent only for a coded
    /// option.
    pub fn gossip_select_option(
        &mut self,
        npc_guid: u64,
        menu_id: u32,
        gossip_list_id: u32,
        code: Option<&str>,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_GOSSIP_SELECT_OPTION,
            &messages::gossip_select_option(npc_guid, menu_id, gossip_list_id, code),
        )
    }

    /// Ask a gossip menu's greeting text (`CMSG_NPC_TEXT_QUERY`); ask once and cache.
    pub fn npc_text_query(&mut self, text_id: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_NPC_TEXT_QUERY,
            &messages::npc_text_query(text_id, guid),
        )
    }
}
