//! The arena-team sends: the cache and roster asks and the captain's commands, each keyed by
//! the team id the player's arena-team field slot holds.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// The team cache's ask for a team's name and banner.
    pub fn arena_team_query(&mut self, team_id: u32) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_QUERY, &team_id.to_le_bytes())
    }

    /// `ArenaTeamRoster(team)`: answered by `SMSG_ARENA_TEAM_ROSTER`.
    pub fn arena_team_roster(&mut self, team_id: u32) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_ROSTER, &team_id.to_le_bytes())
    }

    /// `ArenaTeamInviteByName(team, name)`.
    pub fn arena_team_invite(&mut self, team_id: u32, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_ARENA_TEAM_INVITE,
            &messages::arena_team_named(team_id, name),
        )
    }

    /// `ArenaTeamUninviteByName(team, name)`.
    pub fn arena_team_remove(&mut self, team_id: u32, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_ARENA_TEAM_REMOVE,
            &messages::arena_team_named(team_id, name),
        )
    }

    /// `ArenaTeamSetLeaderByName(team, name)`.
    pub fn arena_team_leader(&mut self, team_id: u32, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_ARENA_TEAM_LEADER,
            &messages::arena_team_named(team_id, name),
        )
    }

    /// `ArenaTeamLeave(team)`.
    pub fn arena_team_leave(&mut self, team_id: u32) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_LEAVE, &team_id.to_le_bytes())
    }

    /// `ArenaTeamDisband(team)`.
    pub fn arena_team_disband(&mut self, team_id: u32) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_DISBAND, &team_id.to_le_bytes())
    }

    /// `AcceptArenaTeam()` (`0x6d4840`): empty.
    pub fn arena_team_accept(&mut self) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_ACCEPT, &[])
    }

    /// `DeclineArenaTeam()`: empty.
    pub fn arena_team_decline(&mut self) -> Result<()> {
        self.send(opcode::CMSG_ARENA_TEAM_DECLINE, &[])
    }
}
