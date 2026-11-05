//! The channel notices and member roster (vmangos `Chat/Channel.cpp`).

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// `SMSG_CHANNEL_NOTIFY`'s notice byte (`ChatNotify`, vmangos `Chat/Channel.h:35-71`).
pub mod channel_notice {
    pub const JOINED: u8 = 0x00;
    pub const LEFT: u8 = 0x01;
    pub const YOU_JOINED: u8 = 0x02;
    pub const YOU_LEFT: u8 = 0x03;
    pub const WRONG_PASSWORD: u8 = 0x04;
    pub const NOT_MEMBER: u8 = 0x05;
    pub const NOT_MODERATOR: u8 = 0x06;
    pub const PASSWORD_CHANGED: u8 = 0x07;
    pub const OWNER_CHANGED: u8 = 0x08;
    pub const PLAYER_NOT_FOUND: u8 = 0x09;
    pub const NOT_OWNER: u8 = 0x0A;
    pub const CHANNEL_OWNER: u8 = 0x0B;
    pub const MODE_CHANGE: u8 = 0x0C;
    pub const ANNOUNCEMENTS_ON: u8 = 0x0D;
    pub const ANNOUNCEMENTS_OFF: u8 = 0x0E;
    pub const MODERATION_ON: u8 = 0x0F;
    pub const MODERATION_OFF: u8 = 0x10;
    pub const MUTED: u8 = 0x11;
    pub const PLAYER_KICKED: u8 = 0x12;
    pub const BANNED: u8 = 0x13;
    pub const PLAYER_BANNED: u8 = 0x14;
    pub const PLAYER_UNBANNED: u8 = 0x15;
    pub const PLAYER_NOT_BANNED: u8 = 0x16;
    pub const PLAYER_ALREADY_MEMBER: u8 = 0x17;
    pub const INVITE: u8 = 0x18;
    pub const INVITE_WRONG_FACTION: u8 = 0x19;
    pub const WRONG_FACTION: u8 = 0x1A;
    pub const INVALID_NAME: u8 = 0x1B;
    pub const NOT_MODERATED: u8 = 0x1C;
    pub const PLAYER_INVITED: u8 = 0x1D;
    pub const PLAYER_INVITE_BANNED: u8 = 0x1E;
    pub const THROTTLED: u8 = 0x1F;
    /// 3.3.5a only, from here: the 12340 client's arms.
    pub const NOT_IN_AREA: u8 = 0x20;
    pub const NOT_IN_LFG: u8 = 0x21;
    pub const VOICE_ON: u8 = 0x22;
    pub const VOICE_OFF: u8 = 0x23;
    /// A member guid, then the channel's voice flag (`+0x90` bit `0x80`) set with no line
    /// (`0x50e9f2`).
    pub const VOICE_ENABLED: u8 = 0x24;
}

/// What follows the notice byte and channel name, by notice (`Chat/Channel.cpp:804-1008`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelNoticeTail {
    /// The member who joined or left.
    Guid(u64),
    /// Our channel flags; the `u32` after them is always 0. 3.3.5a may append a second line's
    /// text (`0x50e220`).
    YouJoined { flags: u32, text: Option<String> },
    /// 3.3.5a's leave: whether the channel is a constant one, the `SUSPENDED` gate (`0x50e324`).
    YouLeft { constant: bool },
    /// Nothing follows the channel name.
    Empty,
    /// The player who made the change or sent the invite.
    Actor(u64),
    /// A player name; `CHANNEL_OWNER` may send the literal `"Nobody"` or `"PLAYER_NOT_FOUND"`.
    Name(String),
    /// The member and its `ChannelMemberFlags` before and after.
    ModeChange {
        guid: u64,
        old_flags: u8,
        new_flags: u8,
    },
    /// The affected player, then who acted.
    Actors { target: u64, source: u64 },
}

/// One decoded `SMSG_CHANNEL_NOTIFY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelNotify {
    pub notice: u8,
    pub channel: String,
    pub tail: ChannelNoticeTail,
}

/// A notice byte past 0x1F errors, since the length of its tail is unknown.
pub(super) fn read_channel_notify(r: &mut &[u8]) -> io::Result<ChannelNotify> {
    use channel_notice as n;

    let notice = read_u8(r)?;
    let channel = read_cstring(r)?;
    let tail = match notice {
        n::JOINED | n::LEFT => ChannelNoticeTail::Guid(read_u64_le(r)?),
        n::YOU_JOINED => {
            let flags = read_u32_le(r)?;
            let _reserved = read_u32_le(r)?; // always 0 on the wire
            ChannelNoticeTail::YouJoined { flags, text: None }
        }
        n::YOU_LEFT
        | n::WRONG_PASSWORD
        | n::NOT_MEMBER
        | n::NOT_MODERATOR
        | n::NOT_OWNER
        | n::MUTED
        | n::BANNED
        | n::INVITE_WRONG_FACTION
        | n::WRONG_FACTION
        | n::INVALID_NAME
        | n::NOT_MODERATED
        | n::THROTTLED => ChannelNoticeTail::Empty,
        n::PASSWORD_CHANGED
        | n::OWNER_CHANGED
        | n::ANNOUNCEMENTS_ON
        | n::ANNOUNCEMENTS_OFF
        | n::MODERATION_ON
        | n::MODERATION_OFF
        | n::PLAYER_ALREADY_MEMBER
        | n::INVITE => ChannelNoticeTail::Actor(read_u64_le(r)?),
        n::PLAYER_NOT_FOUND
        | n::CHANNEL_OWNER
        | n::PLAYER_NOT_BANNED
        | n::PLAYER_INVITED
        | n::PLAYER_INVITE_BANNED => ChannelNoticeTail::Name(read_cstring(r)?),
        n::MODE_CHANGE => ChannelNoticeTail::ModeChange {
            guid: read_u64_le(r)?,
            old_flags: read_u8(r)?,
            new_flags: read_u8(r)?,
        },
        n::PLAYER_KICKED | n::PLAYER_BANNED | n::PLAYER_UNBANNED => ChannelNoticeTail::Actors {
            target: read_u64_le(r)?,
            source: read_u64_le(r)?,
        },
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("SMSG_CHANNEL_NOTIFY: unknown notice type {other:#04x}"),
            ))
        }
    };
    Ok(ChannelNotify {
        notice,
        channel,
        tail,
    })
}

/// 3.3.5a's `SMSG_CHANNEL_NOTIFY` (AzerothCore `Channel::Make*`, the 12340 client's `0x50e120`):
/// 1.12.1's tails, but `YOU_JOINED` is `u8 flags, u32 channelId, u32 0` and an optional cstring
/// read while bytes remain (`0x50e205`), and `YOU_LEFT` is `u32 channelId, u8 isConstant`. Of the
/// notices past `THROTTLED`, `NOT_IN_AREA` and `NOT_IN_LFG` carry nothing and `VOICE_ON`,
/// `VOICE_OFF` and `VOICE_ENABLED` a guid.
pub(super) fn read_channel_notify_wotlk(r: &mut &[u8]) -> io::Result<ChannelNotify> {
    use channel_notice as n;

    let mut body = *r;
    let notice = read_u8(&mut body)?;
    let tail = match notice {
        n::YOU_JOINED => {
            let channel = read_cstring(&mut body)?;
            let flags = u32::from(read_u8(&mut body)?);
            let _channel_id = read_u32_le(&mut body)?;
            let _reserved = read_u32_le(&mut body)?;
            let text = if body.is_empty() {
                None
            } else {
                Some(read_cstring(&mut body)?)
            };
            (channel, ChannelNoticeTail::YouJoined { flags, text })
        }
        n::YOU_LEFT => {
            let channel = read_cstring(&mut body)?;
            let _channel_id = read_u32_le(&mut body)?;
            let constant = read_u8(&mut body)? != 0;
            (channel, ChannelNoticeTail::YouLeft { constant })
        }
        n::NOT_IN_AREA | n::NOT_IN_LFG => (read_cstring(&mut body)?, ChannelNoticeTail::Empty),
        n::VOICE_ON | n::VOICE_OFF | n::VOICE_ENABLED => {
            let channel = read_cstring(&mut body)?;
            (channel, ChannelNoticeTail::Actor(read_u64_le(&mut body)?))
        }
        _ => return read_channel_notify(r),
    };
    *r = body;
    Ok(ChannelNotify {
        notice,
        channel: tail.0,
        tail: tail.1,
    })
}

/// One edit to the watched channel's roster: `SMSG_USERLIST_ADD`, `_REMOVE` or `_UPDATE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelUserList {
    pub kind: ChannelUserListKind,
    pub guid: u64,
    /// The member's `ChannelMemberFlags`; a removal carries none.
    pub member_flags: u8,
    /// The channel's own flags.
    pub channel_flags: u8,
    /// The server's member count, which the client does not read.
    pub count: u32,
    pub channel: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelUserListKind {
    Add,
    Remove,
    Update,
}

/// The three user-list packets: `u64` guid, the member's flags (not on a removal), the channel's
/// flags, a `u32` count and the channel's name (`0x504144`, `0x504303`, `0x500393`).
pub(super) fn read_channel_user_list(
    r: &mut &[u8],
    kind: ChannelUserListKind,
) -> io::Result<ChannelUserList> {
    let guid = read_u64_le(r)?;
    let member_flags = match kind {
        ChannelUserListKind::Remove => 0,
        _ => read_u8(r)?,
    };
    Ok(ChannelUserList {
        kind,
        guid,
        member_flags,
        channel_flags: read_u8(r)?,
        count: read_u32_le(r)?,
        channel: read_cstring(r)?,
    })
}

/// `SMSG_CHANNEL_LIST` (handler `0x505dc0`): `(display, channel, flags, members)`, each member a
/// guid and its `ChannelMemberFlags`. A non-zero display byte routes the body to the channel pane
/// (`0x505de5` → `0x503990`), zero to the `/chatlist` print.
#[allow(clippy::type_complexity)]
pub(super) fn read_channel_list(r: &mut &[u8]) -> io::Result<(bool, String, u8, Vec<(u64, u8)>)> {
    let display = read_u8(r)? != 0;
    let channel = read_cstring(r)?;
    let flags = read_u8(r)?;
    let count = read_u32_le(r)?;
    // The count is wire-controlled, so it only hints the allocation.
    let mut members = Vec::with_capacity(capacity_hint(count, 256));
    for _ in 0..count {
        let guid = read_u64_le(r)?;
        let member_flags = read_u8(r)?;
        members.push((guid, member_flags));
    }
    Ok((display, channel, flags, members))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(notice: u8, tail: &[u8]) -> Vec<u8> {
        let mut b = vec![notice];
        b.extend_from_slice(b"Trade - City\0");
        b.extend_from_slice(tail);
        b
    }

    /// `YOU_JOINED`'s second line is read only while bytes remain; `YOU_LEFT` keeps its constant
    /// byte; `0x24` reads its guid.
    #[test]
    fn the_wotlk_tails_carry_the_join_text_the_leave_byte_and_the_voice_guid() {
        use channel_notice as n;
        let mut joined = vec![0x0a];
        joined.extend_from_slice(&2u32.to_le_bytes());
        joined.extend_from_slice(&0u32.to_le_bytes());
        let plain = read_channel_notify_wotlk(&mut body(n::YOU_JOINED, &joined).as_slice())
            .expect("parses");
        assert_eq!(
            plain.tail,
            ChannelNoticeTail::YouJoined {
                flags: 0x0a,
                text: None
            }
        );
        joined.extend_from_slice(b"Welcome\0");
        let mut r = &body(n::YOU_JOINED, &joined)[..];
        let tailed = read_channel_notify_wotlk(&mut r).expect("parses");
        assert!(r.is_empty());
        assert_eq!(
            tailed.tail,
            ChannelNoticeTail::YouJoined {
                flags: 0x0a,
                text: Some("Welcome".into())
            }
        );

        let mut left = 2u32.to_le_bytes().to_vec();
        left.push(1);
        let leave =
            read_channel_notify_wotlk(&mut body(n::YOU_LEFT, &left).as_slice()).expect("parses");
        assert_eq!(leave.tail, ChannelNoticeTail::YouLeft { constant: true });

        let voice =
            read_channel_notify_wotlk(&mut body(n::VOICE_ENABLED, &7u64.to_le_bytes()).as_slice())
                .expect("parses");
        assert_eq!(voice.tail, ChannelNoticeTail::Actor(7));
    }
}
