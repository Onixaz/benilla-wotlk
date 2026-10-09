//! The `ChatMsg` numbering each reference client sends as `CMSG_MESSAGECHAT`'s `u32` `type`.

/// The `type` a client sends for each chat lane; `None` for a lane the client never sends.
#[derive(Debug, PartialEq, Eq)]
pub struct ChatTypes {
    pub say: u32,
    pub party: u32,
    pub raid: u32,
    pub guild: u32,
    pub officer: u32,
    pub yell: u32,
    pub whisper: u32,
    pub emote: u32,
    pub channel: u32,
    pub afk: u32,
    pub dnd: u32,
    pub raid_leader: Option<u32>,
    pub raid_warning: u32,
    pub battleground: u32,
    pub battleground_leader: Option<u32>,
}

/// 1.12.1: vmangos `ChatMsg` (`SharedDefines.h:1191-1301`) under `SUPPORTED_CLIENT_BUILD` 5875,
/// which also enables RAID_LEADER, RAID_WARNING and the two BATTLEGROUND lanes.
pub(crate) const CLASSIC: ChatTypes = ChatTypes {
    say: 0x00,
    party: 0x01,
    raid: 0x02,
    guild: 0x03,
    officer: 0x04,
    yell: 0x05,
    whisper: 0x06,
    emote: 0x08,
    channel: 0x0E,
    afk: 0x14,
    dnd: 0x15,
    raid_leader: Some(0x57),
    raid_warning: 0x58,
    battleground: 0x5C,
    battleground_leader: Some(0x5D),
};

/// 3.3.5a: `SendChatMessage`'s name table (`0x4fbb20`), whose type defaults to SAY = 1
/// (`0x50d1d8`). It has no RAID_LEADER or BATTLEGROUND_LEADER: the client never sends either.
pub(crate) const WOTLK: ChatTypes = ChatTypes {
    say: 0x01,
    party: 0x02,
    raid: 0x03,
    guild: 0x04,
    officer: 0x05,
    yell: 0x06,
    whisper: 0x07,
    emote: 0x0A,
    channel: 0x11,
    afk: 0x17,
    dnd: 0x18,
    raid_leader: None,
    raid_warning: 0x28,
    battleground: 0x2C,
    battleground_leader: None,
};
