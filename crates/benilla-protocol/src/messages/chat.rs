//! Inbound chat. `SMSG_MESSAGECHAT` (vmangos `Chat/Chat.cpp:2542-2599`) is `u8 type`,
//! `u32 language`, a per-type sender prefix, `u32 len` (NUL included), the text, `u8 chatTag`.
//! Also the small chat-adjacent bodies: player not found, notification, played time, random roll.

use std::io;

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

// ChatMsg values (vmangos `SharedDefines.h:1191-1301`), only those benilla receives or sends.
// Text emotes, channel notices and combat, skill, loot and money lines come from other opcodes.
pub const CHAT_MSG_SAY: u8 = 0x00;
pub const CHAT_MSG_PARTY: u8 = 0x01;
pub const CHAT_MSG_RAID: u8 = 0x02;
pub const CHAT_MSG_GUILD: u8 = 0x03;
pub const CHAT_MSG_OFFICER: u8 = 0x04;
pub const CHAT_MSG_YELL: u8 = 0x05;
pub const CHAT_MSG_WHISPER: u8 = 0x06;
/// Server-to-client only: the "You whisper to Name" self-echo (vmangos `MasterPlayerChat.cpp:54`).
pub const CHAT_MSG_WHISPER_INFORM: u8 = 0x07;
pub const CHAT_MSG_EMOTE: u8 = 0x08;
/// GM dot-command feedback and other server-authored lines; one sender guid, usually 0.
pub const CHAT_MSG_SYSTEM: u8 = 0x0A;
pub const CHAT_MSG_MONSTER_SAY: u8 = 0x0B;
pub const CHAT_MSG_MONSTER_YELL: u8 = 0x0C;
pub const CHAT_MSG_MONSTER_EMOTE: u8 = 0x0D;
pub const CHAT_MSG_CHANNEL: u8 = 0x0E;
pub const CHAT_MSG_AFK: u8 = 0x14;
pub const CHAT_MSG_DND: u8 = 0x15;
/// "Name is ignoring you": one sender guid, the ignoring player (`ChatHandler.cpp:755-763`).
pub const CHAT_MSG_IGNORED: u8 = 0x16;
pub const CHAT_MSG_MONSTER_WHISPER: u8 = 0x1A;
pub const CHAT_MSG_RAID_LEADER: u8 = 0x57;
pub const CHAT_MSG_RAID_WARNING: u8 = 0x58;
pub const CHAT_MSG_RAID_BOSS_WHISPER: u8 = 0x59;
pub const CHAT_MSG_RAID_BOSS_EMOTE: u8 = 0x5A;
/// The server's "your message was filtered" notice: its text is the addressee's name, which the
/// chat frame formats into `CHAT_FILTERED`; the client's spam and profanity filters exempt it.
pub const CHAT_MSG_FILTERED: u8 = 0x5B;
pub const CHAT_MSG_BATTLEGROUND: u8 = 0x5C;
pub const CHAT_MSG_BATTLEGROUND_LEADER: u8 = 0x5D;
/// 3.3.5a's achievement lanes, numbered past 1.12.1's last: "%s has earned the achievement $a!",
/// the earner in the receiver guid and the id after the chat tag.
pub const CHAT_MSG_ACHIEVEMENT: u8 = 0x5E;
pub const CHAT_MSG_GUILD_ACHIEVEMENT: u8 = 0x5F;
/// 3.3.5a's party leader's own party line, `CHAT_MSG_PARTY_LEADER`, past the shared numbering.
pub const CHAT_MSG_PARTY_LEADER: u8 = 0x60;

/// The only chat types the 1.12 client runs its `$`-macro expander over; the rest reach the frame
/// verbatim. This is the wire gate; the re-expansion after a name query omits `0x5A`.
pub const MACRO_EXPANDED_TYPES: [u8; 8] = [
    CHAT_MSG_MONSTER_SAY,        // 0x0B
    CHAT_MSG_MONSTER_YELL,       // 0x0C
    CHAT_MSG_MONSTER_EMOTE,      // 0x0D
    CHAT_MSG_MONSTER_WHISPER,    // 0x1A
    CHAT_MSG_BG_SYSTEM_NEUTRAL,  // 0x52
    CHAT_MSG_BG_SYSTEM_ALLIANCE, // 0x53
    CHAT_MSG_BG_SYSTEM_HORDE,    // 0x54
    CHAT_MSG_RAID_BOSS_EMOTE,    // 0x5A, wire gate only
];

/// The battleground system lines: one guid, the macro subject (`BuildChatPacket`'s `default:`).
pub const CHAT_MSG_BG_SYSTEM_NEUTRAL: u8 = 0x52;
pub const CHAT_MSG_BG_SYSTEM_ALLIANCE: u8 = 0x53;
pub const CHAT_MSG_BG_SYSTEM_HORDE: u8 = 0x54;

/// vmangos `PlayerChatTag`; the server picks GM over DND over AFK (`Player.cpp:1757-1767`).
pub mod chat_tag {
    pub const NONE: u8 = 0;
    pub const AFK: u8 = 1;
    pub const DND: u8 = 2;
    pub const GM: u8 = 3;
}

/// One decoded chat line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// The `ChatMsg` type byte (0x00 say, 0x0A system, 0x0B monster say, …).
    pub chat_type: u8,
    /// The `Language` id (0 universal, 7 common, …).
    pub language: u32,
    /// The speaker, when the type carries a guid (0 for system messages / name-only shapes).
    pub sender_guid: u64,
    /// The trailing addressee guid, 0 when the shape has none; the `$`-macro subject.
    pub target_guid: u64,
    /// The inline speaker name of the monster shapes; player names come from the name query.
    pub sender_name: Option<String>,
    /// The channel name, on `CHAT_MSG_CHANNEL` only.
    pub channel: Option<String>,
    pub text: String,
    /// The [`chat_tag`] byte, shown as the `<AFK>`/`<DND>`/`<GM>` prefix on the sender's name.
    pub chat_tag: u8,
    /// The achievement a `$a` names, on the two achievement lanes (0 elsewhere).
    pub achievement: u32,
}

impl ChatMessage {
    /// Whether this is addon traffic, which the chat frame never prints: 1.12 has no addon opcode
    /// or chat type, so `LANG_ADDON` on any lane is the only mark.
    ///
    /// Its text is `prefix`, a TAB, the payload; with no tab, the 1.12 client reads all as prefix.
    pub fn is_addon(&self) -> bool {
        self.language == super::LANGUAGE_ADDON
    }
}

/// Read the length-prefixed string shape (`u32 len` including NUL + bytes).
fn read_len_string(r: &mut &[u8]) -> io::Result<String> {
    let len = read_u32_le(r)? as usize;
    if len == 0 {
        return Ok(String::new());
    }
    // The NUL is inside `len`; read through it via the cstring reader and sanity-cap first.
    if len > r.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("chat text length {len} exceeds remaining {}", r.len()),
        ));
    }
    read_cstring(r)
}

/// 3.3.5a's own `ChatMsg` numbering of the inbound `u8 type` (AzerothCore `SharedDefines.h`
/// `ChatMsg`, matching the outbound lanes the 12340 client's `SendChatMessage` table sends).
pub mod wotlk_chat_type {
    pub const SYSTEM: u8 = 0x00;
    pub const SAY: u8 = 0x01;
    pub const PARTY: u8 = 0x02;
    pub const RAID: u8 = 0x03;
    pub const GUILD: u8 = 0x04;
    pub const OFFICER: u8 = 0x05;
    pub const YELL: u8 = 0x06;
    pub const WHISPER: u8 = 0x07;
    pub const WHISPER_FOREIGN: u8 = 0x08;
    pub const WHISPER_INFORM: u8 = 0x09;
    pub const EMOTE: u8 = 0x0A;
    pub const MONSTER_SAY: u8 = 0x0C;
    pub const MONSTER_PARTY: u8 = 0x0D;
    pub const MONSTER_YELL: u8 = 0x0E;
    pub const MONSTER_WHISPER: u8 = 0x0F;
    pub const MONSTER_EMOTE: u8 = 0x10;
    pub const CHANNEL: u8 = 0x11;
    pub const AFK: u8 = 0x17;
    pub const DND: u8 = 0x18;
    pub const IGNORED: u8 = 0x19;
    pub const BG_SYSTEM_NEUTRAL: u8 = 0x24;
    pub const BG_SYSTEM_ALLIANCE: u8 = 0x25;
    pub const BG_SYSTEM_HORDE: u8 = 0x26;
    pub const RAID_LEADER: u8 = 0x27;
    pub const RAID_WARNING: u8 = 0x28;
    pub const RAID_BOSS_EMOTE: u8 = 0x29;
    pub const RAID_BOSS_WHISPER: u8 = 0x2A;
    pub const FILTERED: u8 = 0x2B;
    pub const BATTLEGROUND: u8 = 0x2C;
    pub const BATTLEGROUND_LEADER: u8 = 0x2D;
    pub const BATTLENET: u8 = 0x2F;
    pub const ACHIEVEMENT: u8 = 0x30;
    pub const GUILD_ACHIEVEMENT: u8 = 0x31;
    pub const PARTY_LEADER: u8 = 0x33;
}

/// The shared [`ChatMessage::chat_type`], 1.12.1's numbering, of a 3.3.5a type byte; `None` for a
/// 3.3.5a lane with no shared one yet (`PARTY_LEADER`, `WHISPER_FOREIGN`, …).
pub fn wotlk_chat_type_shared(wire: u8) -> Option<u8> {
    use wotlk_chat_type as w;
    Some(match wire {
        w::SYSTEM => CHAT_MSG_SYSTEM,
        w::SAY => CHAT_MSG_SAY,
        w::PARTY => CHAT_MSG_PARTY,
        w::RAID => CHAT_MSG_RAID,
        w::GUILD => CHAT_MSG_GUILD,
        w::OFFICER => CHAT_MSG_OFFICER,
        w::YELL => CHAT_MSG_YELL,
        w::WHISPER => CHAT_MSG_WHISPER,
        w::WHISPER_INFORM => CHAT_MSG_WHISPER_INFORM,
        w::EMOTE => CHAT_MSG_EMOTE,
        w::MONSTER_SAY => CHAT_MSG_MONSTER_SAY,
        w::MONSTER_YELL => CHAT_MSG_MONSTER_YELL,
        w::MONSTER_WHISPER => CHAT_MSG_MONSTER_WHISPER,
        w::MONSTER_EMOTE => CHAT_MSG_MONSTER_EMOTE,
        w::CHANNEL => CHAT_MSG_CHANNEL,
        w::AFK => CHAT_MSG_AFK,
        w::DND => CHAT_MSG_DND,
        w::IGNORED => CHAT_MSG_IGNORED,
        w::BG_SYSTEM_NEUTRAL => CHAT_MSG_BG_SYSTEM_NEUTRAL,
        w::BG_SYSTEM_ALLIANCE => CHAT_MSG_BG_SYSTEM_ALLIANCE,
        w::BG_SYSTEM_HORDE => CHAT_MSG_BG_SYSTEM_HORDE,
        w::RAID_LEADER => CHAT_MSG_RAID_LEADER,
        w::RAID_WARNING => CHAT_MSG_RAID_WARNING,
        w::RAID_BOSS_EMOTE => CHAT_MSG_RAID_BOSS_EMOTE,
        w::RAID_BOSS_WHISPER => CHAT_MSG_RAID_BOSS_WHISPER,
        w::FILTERED => CHAT_MSG_FILTERED,
        w::BATTLEGROUND => CHAT_MSG_BATTLEGROUND,
        w::BATTLEGROUND_LEADER => CHAT_MSG_BATTLEGROUND_LEADER,
        w::ACHIEVEMENT => CHAT_MSG_ACHIEVEMENT,
        w::GUILD_ACHIEVEMENT => CHAT_MSG_GUILD_ACHIEVEMENT,
        w::PARTY_LEADER => CHAT_MSG_PARTY_LEADER,
        _ => return None,
    })
}

/// Read a 3.3.5a `SMSG_MESSAGECHAT` body, or with `gm` an `SMSG_GM_MESSAGECHAT` one: the 12340
/// client parses both (wrappers `0x50eba0`, `0x50ebc0`), as AzerothCore's
/// `ChatHandler::BuildChatPacket` writes them. `u8 type`, `u32 language`, `u64 sender`, `u32`
/// flags, a per-type middle, `u32 len` (NUL included), the text, `u8 chatTag`, then
/// `u32 achievementId` on the two achievement types. The middle is
///
/// - the monster and boss shapes: the sender's length-prefixed name and the receiver guid, then the
///   receiver's length-prefixed name when that guid is neither 0, a player nor a pet;
/// - `WHISPER_FOREIGN`: the sender's name and the receiver guid;
/// - the battleground system lines: the receiver guid, then its name unless it is 0 or a player;
/// - the achievement types: the receiver guid;
/// - every other type: with `gm`, the sender's length-prefixed name; the channel name (cstring)
///   on `CHANNEL`; then the receiver guid.
///
/// 1.12.1's per-type sender prefix and channel `playerRank` are gone. The type is returned in
/// the shared numbering ([`wotlk_chat_type_shared`]); a lane with none is an `Unsupported` error.
pub(super) fn read_message_chat_wotlk(r: &mut &[u8], gm: bool) -> io::Result<ChatMessage> {
    use wotlk_chat_type as w;
    let wire_type = read_u8(r)?;
    let language = read_u32_le(r)?;
    let sender_guid = read_u64_le(r)?;
    let _flags = read_u32_le(r)?;
    let mut sender_name = None;
    let mut channel = None;
    let target_guid;
    match wire_type {
        w::MONSTER_SAY
        | w::MONSTER_PARTY
        | w::MONSTER_YELL
        | w::MONSTER_WHISPER
        | w::MONSTER_EMOTE
        | w::RAID_BOSS_EMOTE
        | w::RAID_BOSS_WHISPER
        | w::BATTLENET => {
            sender_name = Some(read_len_string(r)?);
            target_guid = read_u64_le(r)?;
            if names_receiver(target_guid) {
                let _receiver_name = read_len_string(r)?;
            }
        }
        w::WHISPER_FOREIGN => {
            sender_name = Some(read_len_string(r)?);
            target_guid = read_u64_le(r)?;
        }
        w::BG_SYSTEM_NEUTRAL | w::BG_SYSTEM_ALLIANCE | w::BG_SYSTEM_HORDE => {
            target_guid = read_u64_le(r)?;
            if target_guid != 0 && !client_is_player(target_guid) {
                let _receiver_name = read_len_string(r)?;
            }
        }
        w::ACHIEVEMENT | w::GUILD_ACHIEVEMENT => {
            target_guid = read_u64_le(r)?;
        }
        _ => {
            if gm {
                sender_name = Some(read_len_string(r)?);
            }
            if wire_type == w::CHANNEL {
                channel = Some(read_cstring(r)?);
            }
            target_guid = read_u64_le(r)?;
        }
    }
    let text = read_len_string(r)?;
    let chat_tag = read_u8(r)?;
    let achievement = if matches!(wire_type, w::ACHIEVEMENT | w::GUILD_ACHIEVEMENT) {
        read_u32_le(r)?
    } else {
        0
    };
    let Some(chat_type) = wotlk_chat_type_shared(wire_type) else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("3.3.5a chat type {wire_type:#04x} has no shared chat lane yet"),
        ));
    };
    Ok(ChatMessage {
        chat_type,
        language,
        sender_guid,
        target_guid,
        sender_name,
        channel,
        text,
        chat_tag,
        achievement,
    })
}

/// Whether a monster-shape receiver guid is followed by its name: set, and neither a player nor a
/// pet.
fn names_receiver(guid: u64) -> bool {
    let high = (guid >> 32) as u32;
    guid != 0 && !client_is_player(guid) && high & 0xf0f0_0000 != 0xf040_0000
}

/// The 12340 client's player test on a guid: the high nibble clear and the rest, bar bits 55 and
/// 54 to 52 of the high word, non-zero (`and 0xf0000000`, `and 0xf07fffff`).
fn client_is_player(guid: u64) -> bool {
    guid >> 60 == 0 && guid & 0xf07f_ffff_ffff_ffff != 0
}

/// Read `SMSG_CHAT_PLAYER_NOT_FOUND`: one cstring, the unnormalized name of an offline or
/// misspelled whisper target (vmangos `Server/Packets/Chat.cpp:26-29`).
pub(super) fn read_chat_player_not_found(r: &mut &[u8]) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_NOTIFICATION`: one cstring of server-formatted notice text, which the 1.12 client
/// shows in the red UIErrorsFrame (vmangos `Server/WorldSession.cpp:900-915`).
pub(super) fn read_notification(r: &mut &[u8]) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_PLAYED_TIME` (`0x401390`): total then this-level played time, both `u32` seconds,
/// then the request's show flag; a missing flag reads 0, the handler's zeroed byte.
pub(super) fn read_played_time(r: &mut &[u8]) -> io::Result<(u32, u32, bool)> {
    let total = read_u32_le(r)?;
    let level = read_u32_le(r)?;
    let show = crate::wire::read_u8(r).is_ok_and(|b| b != 0);
    Ok((total, level, show))
}

/// Read the `MSG_RANDOM_ROLL` broadcast (`GroupHandler.cpp:394-422`): `u32` minimum, maximum and
/// roll, then the roller's unpacked `u64` guid; the request on this opcode is only min and max.
pub(super) fn read_random_roll(r: &mut &[u8]) -> io::Result<(u32, u32, u32, u64)> {
    let min = read_u32_le(r)?;
    let max = read_u32_le(r)?;
    let roll = read_u32_le(r)?;
    let guid = read_u64_le(r)?;
    Ok((min, max, roll, guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A party leader's `/p` arrives as 3.3.5a type `0x33` (the chat bubble's 51), in the default
    /// shape: language, sender, flags, receiver, text, tag. It parses to its own lane.
    #[test]
    fn a_party_leader_line_parses_to_its_own_lane() {
        let mut body = vec![wotlk_chat_type::PARTY_LEADER];
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0x77u64.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0x77u64.to_le_bytes());
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(b"pull!\0");
        body.push(0);
        let msg = read_message_chat_wotlk(&mut body.as_slice(), false).expect("parses");
        assert_eq!(msg.chat_type, CHAT_MSG_PARTY_LEADER);
        assert_eq!(msg.sender_guid, 0x77);
        assert_eq!(msg.text, "pull!");
    }
}
