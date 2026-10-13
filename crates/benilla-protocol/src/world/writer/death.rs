//! The death and corpse-run sends. Each is server-gated on a death state the client only believes
//! it is in, so refusals are normal and not always a packet; success arrives as descriptor deltas.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Release the spirit while dead and unreleased; the server answers with ghost aura 8326, the
    /// corpse, `SMSG_CORPSE_RECLAIM_DELAY` and the graveyard teleport.
    pub fn repop_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_REPOP_REQUEST, &messages::repop_request())
    }

    /// Ask where our corpse is (`MSG_CORPSE_QUERY`, empty body), answered on the same opcode.
    pub fn corpse_query(&mut self) -> Result<()> {
        self.send(opcode::MSG_CORPSE_QUERY, &[])
    }

    /// Self-resurrect while `PLAYER_SELF_RES_SPELL` is set; the server casts it, with no reply.
    pub fn self_res(&mut self) -> Result<()> {
        self.send(opcode::CMSG_SELF_RES, &[])
    }

    /// Reclaim our corpse as a ghost, past the reclaim delay, within 39 yd.
    pub fn reclaim_corpse(&mut self, corpse_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_RECLAIM_CORPSE,
            &messages::reclaim_corpse(corpse_guid),
        )
    }

    /// Take the spirit healer's res: 50% health, 25% durability loss, sickness from level 11.
    pub fn spirit_healer_activate(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_SPIRIT_HEALER_ACTIVATE,
            &messages::spirit_healer_activate(npc),
        )
    }

    /// Accept or decline a resurrection offer (`CMSG_RESURRECT_RESPONSE`).
    pub fn resurrect_response(&mut self, caster: u64, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_RESURRECT_RESPONSE,
            &messages::resurrect_response(caster, accept),
        )
    }

    /// Ask a new battleground spirit healer's clock, answered by `SMSG_AREA_SPIRIT_HEALER_TIME`.
    pub fn area_spirit_healer_query(&mut self, healer: u64) -> Result<()> {
        self.send(
            opcode::CMSG_AREA_SPIRIT_HEALER_QUERY,
            &messages::area_spirit_healer(healer),
        )
    }

    /// Queue for the healer's next wave, as `AcceptAreaSpiritHeal` sends.
    pub fn area_spirit_healer_queue(&mut self, healer: u64) -> Result<()> {
        self.send(
            opcode::CMSG_AREA_SPIRIT_HEALER_QUEUE,
            &messages::area_spirit_healer(healer),
        )
    }
}
