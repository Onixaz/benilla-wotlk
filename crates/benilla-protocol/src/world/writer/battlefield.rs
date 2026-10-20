//! The battleground and arena queue's sends.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Enter or leave a slot's queue by its queue word, as `AcceptBattlefieldPort` sends.
    pub fn battlefield_port(&mut self, queue: u64, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_PORT,
            &messages::battlefield_port(queue, accept),
        )
    }

    /// Ask the scoreboard (`RequestBattlefieldScoreData`); the caller throttles it to 5000 ms.
    pub fn request_battlefield_score_data(&mut self) -> Result<()> {
        self.send(opcode::MSG_PVP_LOG_DATA, &[])
    }

    /// Leave the instance the active slot names (`LeaveBattlefield`).
    pub fn leave_battlefield(&mut self, queue: u64) -> Result<()> {
        self.send(
            opcode::CMSG_LEAVE_BATTLEFIELD,
            &messages::leave_battlefield(queue),
        )
    }

    /// Ask one battleground type's instances from the PvP window, as
    /// `RequestBattlegroundInstanceInfo` sends.
    pub fn battlefield_list(&mut self, bg_type: u32, xp_disabled: bool) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_LIST,
            &messages::battlefield_list(bg_type, xp_disabled),
        )
    }

    /// Queue for a battleground, as `JoinBattlefield` sends for a non-arena type.
    pub fn battlemaster_join(
        &mut self,
        battlemaster: u64,
        bg_type: u32,
        instance_id: u32,
        as_group: bool,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEMASTER_JOIN,
            &messages::battlemaster_join(battlemaster, bg_type, instance_id, as_group),
        )
    }

    /// Queue for an arena bracket, as `JoinBattlefield` sends at an arena master.
    pub fn battlemaster_join_arena(
        &mut self,
        battlemaster: u64,
        slot: u8,
        as_group: bool,
        rated: bool,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEMASTER_JOIN_ARENA,
            &messages::battlemaster_join_arena(battlemaster, slot, as_group, rated),
        )
    }

    /// Ask every queue slot's state; the reference sends it once per world entry.
    pub fn battlefield_status(&mut self) -> Result<()> {
        self.send(opcode::CMSG_BATTLEFIELD_STATUS, &[])
    }

    /// Ask teammates' positions (`RequestBattlefieldPositions`); the caller throttles to 5000 ms.
    pub fn request_battlefield_positions(&mut self) -> Result<()> {
        self.send(opcode::MSG_BATTLEGROUND_PLAYER_POSITIONS, &[])
    }

    /// `HearthAndResurrectFromArea()`: leave a world-PvP zone, resurrected at the hearth.
    pub fn hearth_and_resurrect(&mut self) -> Result<()> {
        self.send(opcode::CMSG_HEARTH_AND_RESURRECT, &[])
    }

    /// `ReportPlayerIsPVPAFK(unit)`: a raid or party member reported away.
    pub fn report_pvp_afk(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_REPORT_PVP_AFK, &guid.to_le_bytes())
    }

    /// `BattlefieldMgrEntryInviteResponse(queue, accept)` (`0x54d4f0`).
    pub fn bf_mgr_entry_invite_response(&mut self, queue_id: u32, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_MGR_ENTRY_INVITE_RESPONSE,
            &messages::bf_mgr_response(queue_id, accept),
        )
    }

    /// `BattlefieldMgrQueueInviteResponse(queue, accept)` (`0x54d630`).
    pub fn bf_mgr_queue_invite_response(&mut self, queue_id: u32, accept: bool) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_MGR_QUEUE_INVITE_RESPONSE,
            &messages::bf_mgr_response(queue_id, accept),
        )
    }

    /// `BattlefieldMgrQueueRequest(queue)` (`0x54d590`).
    pub fn bf_mgr_queue_request(&mut self, queue_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_MGR_QUEUE_REQUEST,
            &queue_id.to_le_bytes(),
        )
    }

    /// `BattlefieldMgrExitRequest(queue)` (`0x54d6d0`).
    pub fn bf_mgr_exit_request(&mut self, queue_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BATTLEFIELD_MGR_EXIT_REQUEST,
            &queue_id.to_le_bytes(),
        )
    }
}
