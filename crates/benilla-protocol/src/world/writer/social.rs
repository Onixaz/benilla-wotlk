//! The social sends: friends, ignores and `/who`. The wire adds by name and removes by guid, so a
//! removal must find the guid in the list first.

use anyhow::Result;

use crate::messages::{self, opcode, WhoRequest};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_CONTACT_LIST`: `ShowFriends()`'s refresh of the lists `mask` names; the server also
    /// pushes every list at login.
    pub fn contact_list(&mut self, mask: u32) -> Result<()> {
        self.send(opcode::CMSG_CONTACT_LIST, &messages::contact_list(mask))
    }

    /// `CMSG_ADD_FRIEND` with its note: answered by `SMSG_FRIEND_STATUS`, `FRIEND_ADDED_*` or a
    /// refusal.
    pub fn add_friend(&mut self, name: &str, note: &str) -> Result<()> {
        self.send(opcode::CMSG_ADD_FRIEND, &messages::add_friend(name, note))
    }

    /// `CMSG_SET_CONTACT_NOTES`: a friend's note; the server sends no reply.
    pub fn set_contact_notes(&mut self, guid: u64, note: &str) -> Result<()> {
        self.send(
            opcode::CMSG_SET_CONTACT_NOTES,
            &messages::set_contact_notes(guid, note),
        )
    }

    /// Drop a friend by guid (`CMSG_DEL_FRIEND`); acked with `FRIEND_REMOVED`.
    pub fn del_friend(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_DEL_FRIEND, &messages::del_friend(guid))
    }

    /// Ignore a character by name (`CMSG_ADD_IGNORE`); acked with `FRIEND_IGNORE_ADDED`.
    pub fn add_ignore(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_ADD_IGNORE, &messages::add_ignore(name))
    }

    /// Stop ignoring, by guid (`CMSG_DEL_IGNORE`); acked with `FRIEND_IGNORE_REMOVED`.
    pub fn del_ignore(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_DEL_IGNORE, &messages::del_ignore(guid))
    }

    /// Voice-mute a character by name (`CMSG_ADD_VOICE_IGNORE`); acked with `FRIEND_MUTE_ADDED`.
    pub fn add_mute(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_ADD_VOICE_IGNORE, &messages::add_mute(name))
    }

    /// Unmute, by guid (`CMSG_DEL_VOICE_IGNORE`); acked with `FRIEND_MUTE_REMOVED`.
    pub fn del_mute(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_DEL_VOICE_IGNORE, &messages::del_mute(guid))
    }

    /// `CMSG_WHO`: the server runs one query per session at a time, as an async task, so the reply
    /// lags a tick or two and a second query sent meanwhile is dropped (`ReceivedWhoRequest`).
    pub fn who(&mut self, request: &WhoRequest) -> Result<()> {
        self.send(opcode::CMSG_WHO, &messages::who(request))
    }
}
