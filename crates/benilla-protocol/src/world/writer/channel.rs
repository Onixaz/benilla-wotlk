//! The channel sends: join, leave, list and moderation, all with cstring-only bodies.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Join a channel; the client joins zone channels this way ("General - Elwynn Forest"), with
    /// its `ChatChannels.dbc` id ([`messages::join_channel_wotlk`]).
    pub fn join_channel(
        &mut self,
        name: &str,
        password: &str,
        channel_id: u32,
        auto_joined: bool,
        voice: bool,
    ) -> Result<()> {
        let body = messages::join_channel_wotlk(channel_id, auto_joined, voice, name, password);
        self.send(opcode::CMSG_JOIN_CHANNEL, &body)
    }

    /// Leave a channel (`CMSG_LEAVE_CHANNEL`), naming its `ChatChannels.dbc` id.
    pub fn leave_channel(&mut self, name: &str, channel_id: u32) -> Result<()> {
        let body = messages::leave_channel_wotlk(channel_id, name);
        self.send(opcode::CMSG_LEAVE_CHANNEL, &body)
    }

    /// Ask a channel's members (`CMSG_CHANNEL_LIST`, `/chatlist`), answered by `SMSG_CHANNEL_LIST`.
    pub fn channel_list(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_CHANNEL_LIST, &messages::channel_list(name))
    }

    /// Watch the channel the pane shows (`CMSG_SET_CHANNEL_WATCH`).
    pub fn set_channel_watch(&mut self, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_SET_CHANNEL_WATCH,
            &messages::channel_list(name),
        )
    }

    /// Ask the pane's roster (`CMSG_CHANNEL_DISPLAY_LIST`, `0x4fe530`), answered by a display
    /// `SMSG_CHANNEL_LIST`.
    pub fn channel_display_list(&mut self, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_DISPLAY_LIST,
            &messages::channel_list(name),
        )
    }

    /// Set a channel's password (`CMSG_CHANNEL_PASSWORD`); owner only.
    pub fn channel_password(&mut self, name: &str, password: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_PASSWORD,
            &messages::channel_password(name, password),
        )
    }

    /// Transfer channel ownership (`CMSG_CHANNEL_SET_OWNER`); owner only.
    pub fn channel_set_owner(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_SET_OWNER,
            &messages::channel_set_owner(name, player),
        )
    }

    /// Ask who owns a channel (`CMSG_CHANNEL_OWNER`), answered by a `CHANNEL_OWNER` notify.
    pub fn channel_owner(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_CHANNEL_OWNER, &messages::channel_owner(name))
    }

    /// Grant moderator (`CMSG_CHANNEL_MODERATOR`); owner only.
    pub fn channel_moderator(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_MODERATOR,
            &messages::channel_moderator(name, player),
        )
    }

    /// Revoke moderator (`CMSG_CHANNEL_UNMODERATOR`).
    pub fn channel_unmoderator(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_UNMODERATOR,
            &messages::channel_unmoderator(name, player),
        )
    }

    /// Mute a player on the channel (`CMSG_CHANNEL_MUTE`); moderator only.
    pub fn channel_mute(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_MUTE,
            &messages::channel_mute(name, player),
        )
    }

    /// Unmute a player (`CMSG_CHANNEL_UNMUTE`).
    pub fn channel_unmute(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_UNMUTE,
            &messages::channel_unmute(name, player),
        )
    }

    /// Invite a player to a channel, gated on `CONFIG_UINT32_CHANNEL_INVITE_MIN_LEVEL`.
    pub fn channel_invite(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_INVITE,
            &messages::channel_invite(name, player),
        )
    }

    /// Kick a player off the channel (`CMSG_CHANNEL_KICK`); moderator only.
    pub fn channel_kick(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_KICK,
            &messages::channel_kick(name, player),
        )
    }

    /// Ban a player from the channel (`CMSG_CHANNEL_BAN`); moderator only.
    pub fn channel_ban(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_BAN,
            &messages::channel_ban(name, player),
        )
    }

    /// Unban a player (`CMSG_CHANNEL_UNBAN`).
    pub fn channel_unban(&mut self, name: &str, player: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_UNBAN,
            &messages::channel_unban(name, player),
        )
    }

    /// Toggle join/leave announcements (`CMSG_CHANNEL_ANNOUNCEMENTS`); owner or moderator only.
    pub fn channel_announcements(&mut self, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_ANNOUNCEMENTS,
            &messages::channel_announcements(name),
        )
    }

    /// Toggle moderated mode, where only moderators speak (`CMSG_CHANNEL_MODERATE`); owner only.
    pub fn channel_moderate(&mut self, name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_CHANNEL_MODERATE,
            &messages::channel_moderate(name),
        )
    }
}
