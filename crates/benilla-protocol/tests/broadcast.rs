//! The unsolicited world broadcasts, bytes built from the vmangos layout: `SMSG_SERVER_MESSAGE`,
//! `SMSG_ZONE_UNDER_ATTACK`, `SMSG_DEFENSE_MESSAGE` and the bodyless `SMSG_CHAT_RESTRICTED`.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

/// `SMSG_SERVER_MESSAGE` (vmangos `Server/Packets/Misc.cpp:341-345`): a `u32` type and a cstring. A
/// countdown fills its row's `%s`; a cancellation sends empty text (`World.cpp:2742`/`2761`), which
/// the reference prints as the bare row.
#[test]
fn server_message_decodes_a_countdown_and_a_cancellation() {
    let body = hx("020000003135204d696e7574657300"); // type 2 (RESTART_TIME), "15 Minutes"
    match messages::parse_server(messages::opcode::SMSG_SERVER_MESSAGE, &body).unwrap() {
        ServerPacket::ServerMessage {
            message_type,
            ref text,
        } => {
            assert_eq!(message_type, 2);
            assert_eq!(text, "15 Minutes");
        }
        other => panic!("expected ServerMessage, got {}", other.name()),
    }
    match &decode(messages::parse_server(messages::opcode::SMSG_SERVER_MESSAGE, &body).unwrap())[..]
    {
        [SessionEvent::ServerMessage { message_type, text }] => {
            assert_eq!(*message_type, 2);
            assert_eq!(text, "15 Minutes");
        }
        other => panic!("server message decode: {} events", other.len()),
    }

    let body = hx("0400000000"); // type 4 (SHUTDOWN_CANCELLED), empty text
    match messages::parse_server(messages::opcode::SMSG_SERVER_MESSAGE, &body).unwrap() {
        ServerPacket::ServerMessage {
            message_type,
            ref text,
        } => {
            assert_eq!(message_type, 4);
            assert!(text.is_empty());
        }
        other => panic!("expected ServerMessage, got {}", other.name()),
    }
}

/// `SMSG_ZONE_UNDER_ATTACK` (vmangos `Server/Packets/Misc.cpp:451-454`): one `u32`
/// `AreaTable.dbc` id; 40 is Westfall.
#[test]
fn zone_under_attack_decodes() {
    let body = hx("28000000");
    let p = messages::parse_server(messages::opcode::SMSG_ZONE_UNDER_ATTACK, &body).unwrap();
    assert!(matches!(p, ServerPacket::ZoneUnderAttack { area_id: 40 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ZoneUnderAttack { area_id: 40 }]
    ));
}

/// `SMSG_DEFENSE_MESSAGE` (vmangos `Maps/Map.cpp:1868-1884`): `u32 zoneId`, `u32 strlen + 1`
/// and the cstring; the reference reads and discards the length. 139 is the Eastern Plaguelands.
#[test]
fn defense_message_decodes() {
    let body = hx(
        "8b000000300000004e6f7274687061737320546f77657220686173206265656e2\
         074616b656e2062792074686520416c6c69616e63652100",
    );
    match messages::parse_server(messages::opcode::SMSG_DEFENSE_MESSAGE, &body).unwrap() {
        ServerPacket::DefenseMessage { zone_id, ref text } => {
            assert_eq!(zone_id, 139);
            assert_eq!(text, "Northpass Tower has been taken by the Alliance!");
        }
        other => panic!("expected DefenseMessage, got {}", other.name()),
    }
    match &decode(messages::parse_server(messages::opcode::SMSG_DEFENSE_MESSAGE, &body).unwrap())[..]
    {
        [SessionEvent::DefenseMessage { zone_id, text }] => {
            assert_eq!(*zone_id, 139);
            assert_eq!(text, "Northpass Tower has been taken by the Alliance!");
        }
        other => panic!("defense message decode: {} events", other.len()),
    }
}

/// `SMSG_CHAT_RESTRICTED` (vmangos `Server/Packets/Chat.cpp:21-23`) has an empty body; the
/// reference's handler reads nothing either.
#[test]
fn chat_restricted_is_bodyless() {
    let p = messages::parse_server(messages::opcode::SMSG_CHAT_RESTRICTED, &[]).unwrap();
    assert!(matches!(p, ServerPacket::ChatRestricted));
    assert!(matches!(decode(p)[..], [SessionEvent::ChatRestricted]));
}
