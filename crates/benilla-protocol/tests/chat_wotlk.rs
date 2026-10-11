//! 3.3.5a `SMSG_MESSAGECHAT`: lines captured from AzerothCore must decode whole into the shared
//! `ChatMessage`, in 1.12.1's type numbering, and never through the 1.12.1 reader.

mod common;

use benilla_protocol::messages::{
    self, opcode, ChatMessage, CHAT_MSG_MONSTER_SAY, CHAT_MSG_SYSTEM,
};
use benilla_protocol::ServerPacket;
use common::hx;

/// `.gps`'s first answer line, a system line (type 0): 47 bytes.
const GPS_OUTDOORS: &str = concat!(
    "00",                                 // type SYSTEM
    "00000000",                           // language
    "0000000000000000",                   // sender
    "00000000",                           // flags
    "0000000000000000",                   // receiver
    "11000000",                           // length 17
    "596f7520617265206f7574646f6f727300", // "You are outdoors"
    "00",                                 // chat tag
);

/// The login MOTD's first line: 150 bytes of colour-coded system text.
const MOTD: &str = concat!(
    "00000000000000000000000000000000000000000000000000",
    "78000000",
    "7c63666630306666303054686973207365727665722072756e732077697468207c6366663030636366666d6f",
    "642d706c61796572626f74737c72207c63666663636363636368747470733a2f2f6769746875622e636f6d2f",
    "6d6f642d706c61796572626f74732f6d6f642d706c61796572626f74737c7200",
    "00",
);

fn decode(body: &[u8]) -> (ChatMessage, usize) {
    match messages::parse_server_with_tail(opcode::SMSG_MESSAGECHAT, body).unwrap() {
        (ServerPacket::MessageChat(m), tail) => (m, tail),
        (other, _) => panic!("not a chat line: {}", other.name()),
    }
}

#[test]
fn a_captured_system_line_decodes_whole_as_the_shared_system_type() {
    let (m, tail) = decode(&hx(GPS_OUTDOORS));
    assert_eq!(tail, 0);
    assert_eq!(m.chat_type, CHAT_MSG_SYSTEM);
    assert_eq!(m.text, "You are outdoors");
    assert_eq!((m.sender_guid, m.target_guid, m.chat_tag), (0, 0, 0));

    let (m, tail) = decode(&hx(MOTD));
    assert_eq!(tail, 0);
    assert_eq!(m.chat_type, CHAT_MSG_SYSTEM);
    assert!(m.text.starts_with("|cff00ff00This server runs with "));
    assert!(m.text.ends_with("mod-playerbots|r"));
}

/// A creature's say, built to `BuildChatPacket`'s monster shape: the name rides inline and a
/// creature receiver's name follows its guid.
#[test]
fn a_monster_say_reads_the_inline_names() {
    let creature: u64 = 0xF130_0000_0001_0002;
    let mut body = vec![0x0C]; // MONSTER_SAY
    body.extend(7u32.to_le_bytes()); // language
    body.extend(creature.to_le_bytes());
    body.extend(0u32.to_le_bytes()); // flags
    body.extend(5u32.to_le_bytes());
    body.extend(b"Wolf\0");
    body.extend(creature.to_le_bytes()); // receiver: a creature, so its name follows
    body.extend(5u32.to_le_bytes());
    body.extend(b"Wolf\0");
    body.extend(4u32.to_le_bytes());
    body.extend(b"Grr\0");
    body.push(0);
    let (m, tail) = decode(&body);
    assert_eq!(tail, 0);
    assert_eq!(m.chat_type, CHAT_MSG_MONSTER_SAY);
    assert_eq!(m.sender_name.as_deref(), Some("Wolf"));
    assert_eq!((m.sender_guid, m.target_guid), (creature, creature));
    assert_eq!(m.text, "Grr");
}

/// A 3.3.5a-only lane (here `ACHIEVEMENT`, `0x30`) is read through and then refused, never handed
/// on under a guessed 1.12.1 type.
#[test]
fn a_lane_without_a_shared_type_fails_visibly() {
    let mut body = vec![0x30];
    body.extend(0u32.to_le_bytes());
    body.extend(0x21u64.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    body.extend(0x21u64.to_le_bytes()); // receiver
    body.extend(13u32.to_le_bytes());
    body.extend(b"%s has done\0\0");
    body.push(0);
    body.extend(6u32.to_le_bytes()); // achievement id
    let Err(err) = messages::parse_server_with_tail(opcode::SMSG_MESSAGECHAT, &body) else {
        panic!("a 3.3.5a-only lane must not decode");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}
