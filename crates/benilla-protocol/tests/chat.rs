//! The chat and channel wire: every `SMSG_MESSAGECHAT` shape, the emotes, the channel notices and
//! list, and the small chat replies, bytes built from the vmangos layout.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self};
use benilla_protocol::ServerPacket;
use common::hx;

/// `SMSG_TEXT_EMOTE` (vmangos `Handlers/ChatHandler.cpp:681-702`): unpacked `u64` guid, `u32`
/// textEmote, `u32` emoteNum, `u32` namelen and the target name with its NUL; `SMSG_EMOTE`
/// (`Server/Packets/Misc.cpp:670-674`): `u32` emoteId and `u64` guid.
#[test]
fn text_emote_and_emote_decode() {
    let body = hx("7700000000000000650000000000000004000000426f6200");
    let p = messages::parse_server(messages::opcode::SMSG_TEXT_EMOTE, &body).unwrap();
    // The target name picks the sentence form; the NUL counted in `namelen` is trimmed.
    assert!(matches!(
        &p,
        ServerPacket::TextEmote {
            guid: 0x77,
            text_emote: 101,
            target_name,
        } if target_name == "Bob"
    ));
    assert!(matches!(
        &decode(p)[..],
        [SessionEvent::TextEmote {
            guid: 0x77,
            text_emote: 101,
            target_name,
        }] if target_name == "Bob"
    ));

    // Untargeted, vmangos sends `namelen == 1` and a lone NUL: an empty name, not a one-byte one.
    let body = hx("770000000000000065000000000000000100000000");
    assert!(matches!(
        messages::parse_server(messages::opcode::SMSG_TEXT_EMOTE, &body).unwrap(),
        ServerPacket::TextEmote { ref target_name, .. } if target_name.is_empty()
    ));

    let body = hx("060000008800000000000000");
    let p = messages::parse_server(messages::opcode::SMSG_EMOTE, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::Emote {
            guid: 0x88,
            emote_id: 6,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::Emote {
            guid: 0x88,
            emote_id: 6,
        }]
    ));
}

/// `SMSG_CHANNEL_LIST` (`0x505dc0`): `u8` display, cstring channel, `u8` flags, `u32` count, then
/// `(u64 guid, u8 memberFlags)` rows.
#[test]
fn channel_list_decodes_roster() {
    let body = hx("0147656e6572616c001802000000011000000000000001021000000000000000");
    match messages::parse_server(messages::opcode::SMSG_CHANNEL_LIST, &body).unwrap() {
        ServerPacket::ChannelList {
            display,
            channel,
            flags,
            members,
        } => {
            assert!(display, "a non-zero first byte is the channel pane's reply");
            assert_eq!(channel, "General");
            assert_eq!(flags, 0x18);
            assert_eq!(members, vec![(0x1001, 0x01), (0x1002, 0x00)]);
        }
        _ => panic!("expected ChannelList"),
    }
}

/// `SMSG_CHAT_PLAYER_NOT_FOUND` (cstring name, vmangos `Server/Packets/Chat.cpp:26-29`) and
/// `SMSG_CHAT_WRONG_FACTION` (empty body, `Server/Packets/Chat.cpp:16-18`).
#[test]
fn chat_player_not_found_and_wrong_faction_decode() {
    let body = hx("47686f73746e616d6500");
    match messages::parse_server(messages::opcode::SMSG_CHAT_PLAYER_NOT_FOUND, &body).unwrap() {
        ServerPacket::ChatPlayerNotFound { name } => assert_eq!(name, "Ghostname"),
        _ => panic!("expected ChatPlayerNotFound"),
    }
    assert!(matches!(
        decode(
            messages::parse_server(messages::opcode::SMSG_CHAT_PLAYER_NOT_FOUND, &body).unwrap()
        )[..],
        [SessionEvent::ChatPlayerNotFound { .. }]
    ));

    let p = messages::parse_server(messages::opcode::SMSG_CHAT_WRONG_FACTION, &[]).unwrap();
    assert!(matches!(p, ServerPacket::ChatWrongFaction));
    assert!(matches!(decode(p)[..], [SessionEvent::ChatWrongFaction]));
}

/// `SMSG_NOTIFICATION` (vmangos `Server/WorldSession.cpp:900-915`): one cstring, formatted by the
/// server; a send in a language the sender does not know fails with this reply.
#[test]
fn notification_decodes() {
    let body = hx("596f7520646f206e6f74206b6e6f772074686174206c616e677561676500");
    match messages::parse_server(messages::opcode::SMSG_NOTIFICATION, &body).unwrap() {
        ServerPacket::Notification { ref text } => {
            assert_eq!(text, "You do not know that language")
        }
        other => panic!("expected Notification, got {}", other.name()),
    }
    match &decode(messages::parse_server(messages::opcode::SMSG_NOTIFICATION, &body).unwrap())[..] {
        [SessionEvent::Notification { text }] => assert_eq!(text, "You do not know that language"),
        other => panic!("notification decode: {} events", other.len()),
    }
}

/// `SMSG_PLAYED_TIME` (vmangos `Server/Packets/Misc.cpp:278-282`): total and level, `u32` seconds.
#[test]
fn played_time_decodes() {
    let body = hx("40e20100100e0000"); // total = 123456, level = 3600
    let p = messages::parse_server(messages::opcode::SMSG_PLAYED_TIME, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PlayedTime {
            total: 123_456,
            level: 3600,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayedTime {
            total: 123_456,
            level: 3600,
        }]
    ));
}

/// `MSG_RANDOM_ROLL`'s broadcast (vmangos `Handlers/GroupHandler.cpp:394-422`): `u32` min, max
/// and roll, then an unpacked `u64` guid.
#[test]
fn random_roll_broadcast_decodes() {
    let body = hx("01000000640000002a0000009999000000000000");
    let p = messages::parse_server(messages::opcode::MSG_RANDOM_ROLL, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::RandomRoll {
            min: 1,
            max: 100,
            roll: 42,
            guid: 0x9999,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::RandomRoll {
            min: 1,
            max: 100,
            roll: 42,
            guid: 0x9999,
        }]
    ));
}

/// The `$`-macro types are the reference's wire gate: parser sends 0x0B, 0x0C, 0x0D, 0x1A and 0x5A
/// to the expander, and 0x52 to 0x54 to the one. The chain is the pending-chat gate, which omits
/// 0x5A; it is not this one.
#[test]
fn the_macro_expanded_chat_types_are_the_reference_wire_gate() {
    let mut got = messages::MACRO_EXPANDED_TYPES;
    got.sort_unstable();
    assert_eq!(got, [0x0B, 0x0C, 0x0D, 0x1A, 0x52, 0x53, 0x54, 0x5A]);
    // 0x59 RAID_BOSS_WHISPER takes its own branch: never expanded, remapped to a plain whisper.
    for t in [
        messages::CHAT_MSG_SAY,
        messages::CHAT_MSG_YELL,
        messages::CHAT_MSG_WHISPER,
        messages::CHAT_MSG_GUILD,
        messages::CHAT_MSG_CHANNEL,
        messages::CHAT_MSG_SYSTEM,
        messages::CHAT_MSG_RAID_BOSS_WHISPER,
    ] {
        assert!(
            !messages::MACRO_EXPANDED_TYPES.contains(&t),
            "type {t:#04x} must reach the frame verbatim"
        );
    }
}
