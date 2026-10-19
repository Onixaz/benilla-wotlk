//! The dungeon finder's sends, `CGLookingForGroup`'s (`0x552850`-`0x55d440`).

use anyhow::Result;

use crate::messages::{self, opcode, LfgJoin};

use super::WorldWriter;

impl WorldWriter {
    /// `RequestLFGStatus` (`0x555920`), at each world entry and on joining a party.
    pub fn lfg_get_status(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LFG_GET_STATUS, &[])
    }

    /// `RequestLFGPlayerInfo` (`0x555990`).
    pub fn lfg_player_lock_info_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LFG_PLAYER_LOCK_INFO_REQUEST, &[])
    }

    /// `RequestLFGPartyInfo` (`0x555a00`).
    pub fn lfg_party_lock_info_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LFG_PARTY_LOCK_INFO_REQUEST, &[])
    }

    /// `JoinLFG` (`0x5572d0`).
    pub fn lfg_join(&mut self, join: &LfgJoin<'_>) -> Result<()> {
        self.send(opcode::CMSG_LFG_JOIN, &messages::lfg_join(join))
    }

    /// `LeaveLFG` (`0x553ab0`).
    pub fn lfg_leave(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LFG_LEAVE, &[])
    }

    /// `ProposalResponse` (`0x554450`).
    pub fn lfg_proposal_result(&mut self, id: u32, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_LFG_PROPOSAL_RESULT,
            &messages::lfg_proposal_result(id, accept),
        )
    }

    /// `CompleteLFGRoleCheck` (`0x553e20`): 0 declines.
    pub fn lfg_set_roles(&mut self, roles: u8) -> Result<()> {
        self.send(opcode::CMSG_LFG_SET_ROLES, &[roles])
    }

    /// `SetLFGBootVote` (`0x554960`).
    pub fn lfg_set_boot_vote(&mut self, vote: bool) -> Result<()> {
        self.send(opcode::CMSG_LFG_SET_BOOT_VOTE, &[u8::from(vote)])
    }

    /// `LFGTeleport` (`0x556bc0`): `out` leaves the dungeon.
    pub fn lfg_teleport(&mut self, out: bool) -> Result<()> {
        self.send(opcode::CMSG_LFG_TELEPORT, &[u8::from(out)])
    }

    /// `SetLFGComment` while joined (`0x5539a0`).
    pub fn lfg_set_comment(&mut self, comment: &str) -> Result<()> {
        self.send(
            opcode::CMSG_SET_LFG_COMMENT,
            &messages::lfg_comment(comment),
        )
    }
}
