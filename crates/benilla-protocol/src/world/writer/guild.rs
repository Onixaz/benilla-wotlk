//! The guild sends. Members are addressed by name, and rank 0 is the guild master. The server acks
//! a member or rank change with a whole fresh `SMSG_GUILD_ROSTER` and refuses one with
//! `SMSG_GUILD_COMMAND_RESULT`, so state updates when the roster lands, not at the send.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_GUILD_QUERY`: a guild's name, rank names and tabard by id, for the ask-once cache.
    pub fn guild_query(&mut self, guild_id: u32) -> Result<()> {
        self.send(opcode::CMSG_GUILD_QUERY, &messages::guild_query(guild_id))
    }

    /// `CMSG_GUILD_CREATE`: `STATUS_NEVER` in vmangos, so no reply; guilds are founded by charter.
    pub fn guild_create(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_CREATE, &messages::guild_create(name))
    }

    /// `CMSG_GUILD_INVITE`: the invitee gets `SMSG_GUILD_INVITE`; we hear back only on a refusal.
    pub fn guild_invite(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_INVITE, &messages::guild_invite(name))
    }

    /// `CMSG_GUILD_ACCEPT`, empty: takes the server's pending invite; a silent no-op without one.
    pub fn guild_accept(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_ACCEPT, &messages::guild_accept())
    }

    /// `CMSG_GUILD_DECLINE`, empty: the inviter gets `SMSG_GUILD_DECLINE`; we hear nothing.
    pub fn guild_decline(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_DECLINE, &messages::guild_decline())
    }

    /// `CMSG_GUILD_INFO`, empty: answered by `SMSG_GUILD_INFO`, the founding date and counts.
    pub fn guild_info(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_INFO, &messages::guild_info())
    }

    /// `MSG_GUILD_BANK_MONEY_WITHDRAWN`, empty: answered in kind with today's remaining withdrawal.
    pub fn guild_bank_money_withdrawn(&mut self) -> Result<()> {
        self.send(
            opcode::MSG_GUILD_BANK_MONEY_WITHDRAWN,
            &messages::guild_bank_money_withdrawn(),
        )
    }

    /// `CMSG_GUILD_ROSTER`, empty: a refresh; the server also pushes the roster after every change.
    pub fn guild_roster(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_ROSTER, &messages::guild_roster())
    }

    /// `CMSG_GUILD_PROMOTE`: the server moves the member to `rank - 1`, toward guild master.
    pub fn guild_promote(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_PROMOTE, &messages::guild_promote(name))
    }

    /// `CMSG_GUILD_DEMOTE`: `rank + 1`, away from guild master.
    pub fn guild_demote(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_DEMOTE, &messages::guild_demote(name))
    }

    /// `CMSG_GUILD_LEAVE`, empty: refused (`LEADER_LEAVE`) for a guild master while others remain.
    pub fn guild_leave(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_LEAVE, &messages::guild_leave())
    }

    /// Kick a member by name (`CMSG_GUILD_REMOVE`); needs the REMOVE right and a rank above theirs.
    pub fn guild_remove(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_REMOVE, &messages::guild_remove(name))
    }

    /// `CMSG_GUILD_DISBAND`, empty: guild master only; every member gets `GE_DISBANDED`.
    pub fn guild_disband(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_DISBAND, &messages::guild_disband())
    }

    /// `CMSG_GUILD_LEADER`: guild master only; everyone gets both names in `GE_LEADER_CHANGED`.
    pub fn guild_leader(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_LEADER, &messages::guild_leader(name))
    }

    /// `CMSG_GUILD_MOTD`: `""` clears the message of the day.
    pub fn guild_motd(&mut self, motd: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_MOTD, &messages::guild_motd(motd))
    }

    /// `CMSG_GUILD_RANK`: a rank's name and rights together, with no partial form. Guild master
    /// only; rank 0 always gets all rights, and a name over [`messages::GUILD_RANK_MAX_LENGTH`]
    /// gets the session kicked by vmangos, so the caller caps it.
    pub fn guild_rank(&mut self, rank_id: u32, rights: u32, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GUILD_RANK,
            &messages::guild_rank(rank_id, rights, name),
        )
    }

    /// `CMSG_GUILD_ADD_RANK`: a new bottom rank with only guild chat rights; ignored at
    /// [`messages::GUILD_RANKS_MAX_COUNT`] ranks.
    pub fn guild_add_rank(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_GUILD_ADD_RANK, &messages::guild_add_rank(name))
    }

    /// `CMSG_GUILD_DEL_RANK`, empty: always the lowest rank; refused with `RANK_IN_USE` while held.
    pub fn guild_del_rank(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GUILD_DEL_RANK, &messages::guild_del_rank())
    }

    /// `CMSG_GUILD_SET_PUBLIC_NOTE`: needs [`messages::guild_rank_right::EDIT_PUBLIC_NOTE`].
    pub fn guild_set_public_note(&mut self, name: &str, note: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GUILD_SET_PUBLIC_NOTE,
            &messages::guild_set_public_note(name, note),
        )
    }

    /// `CMSG_GUILD_SET_OFFICER_NOTE`: needs [`messages::guild_rank_right::EDIT_OFFICER_NOTE`].
    pub fn guild_set_officer_note(&mut self, name: &str, note: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GUILD_SET_OFFICER_NOTE,
            &messages::guild_set_officer_note(name, note),
        )
    }

    /// `CMSG_GUILD_INFO_TEXT`: the free-text info pane; it comes back as the roster's `info`.
    pub fn guild_info_text(&mut self, text: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GUILD_INFO_TEXT,
            &messages::guild_info_text(text),
        )
    }
}
