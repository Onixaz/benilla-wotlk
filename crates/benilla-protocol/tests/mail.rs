//! The mail wire, in 12340's shapes: the send verbs, the inbox page with its inline bodies and
//! attachment slots, the result tails, the copied-letter text, the arrival notices and the
//! server-opened mailbox.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, mail_action, mail_delete_reason, mail_error, mail_message_type, opcode, MailEnchant,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

const MAILBOX: u64 = 0x00F1_1000_0000_0042;

/// Parses a whole body, asserting the decoder read every byte.
fn parse_all(op: u16, body: &[u8]) -> ServerPacket {
    let (packet, tail) = messages::parse_server_with_tail(op, body).expect("parses");
    assert_eq!(tail, 0, "{} left {tail} byte(s) unread", packet.name());
    packet
}

#[test]
fn mail_send_bodies_golden() {
    assert_eq!(
        messages::get_mail_list(MAILBOX),
        hx("420000000010f100"),
        "CMSG_GET_MAIL_LIST body"
    );

    // CMSG_SEND_MAIL (`0x570910`): the attachments as `(u8 slot, u64 guid)` pairs after a count.
    assert_eq!(
        messages::send_mail(
            MAILBOX,
            "Bob",
            "Hi",
            "Two items.",
            41,
            0,
            &[(0, 0x4000_0000_0000_0099), (3, 0x4000_0000_0000_009a)],
            12_345,
            0,
        ),
        hx(concat!(
            "420000000010f100",       // mailbox
            "426f6200",               // "Bob\0"
            "486900",                 // "Hi\0"
            "54776f206974656d732e00", // "Two items.\0"
            "29000000",               // stationery 41
            "00000000",               // package 0
            "02",                     // two attachments
            "00",
            "9900000000000040", // slot 0, guid
            "03",
            "9a00000000000040", // slot 3, guid
            "39300000",         // money 12345
            "00000000",         // cod 0
            "0000000000000000", // zero u64
            "00",               // zero u8
        )),
        "CMSG_SEND_MAIL body"
    );
    assert_eq!(
        messages::send_mail(MAILBOX, "Bob", "Hi", "", 41, 0, &[], 0, 0)[8 + 4 + 3 + 1 + 8],
        0,
        "no attachments: a zero count and no pairs"
    );

    let id = hx("420000000010f1004d000000"); // mailbox + mailId 77
    assert_eq!(messages::mail_take_money(MAILBOX, 77), id);
    assert_eq!(messages::mail_mark_as_read(MAILBOX, 77), id);
    assert_eq!(messages::mail_create_text_item(MAILBOX, 77), id);
    // CMSG_MAIL_TAKE_ITEM (`0x5711c0`): the attachment's item guid low part trails.
    assert_eq!(
        messages::mail_take_item(MAILBOX, 77, 0x1234),
        hx("420000000010f1004d00000034120000")
    );
    // CMSG_MAIL_RETURN_TO_SENDER (`0x56f000`): the sender's guid trails.
    assert_eq!(
        messages::mail_return_to_sender(MAILBOX, 77, 0xAA),
        hx("420000000010f1004d000000aa00000000000000")
    );
    // CMSG_MAIL_DELETE (`0x56f140`): the reason trails, 0 for a player's own delete.
    assert_eq!(
        messages::mail_delete(MAILBOX, 77, mail_delete_reason::PLAYER),
        hx("420000000010f1004d00000000000000")
    );
    assert_eq!(
        messages::mail_delete(MAILBOX, 77, mail_delete_reason::ITEM_TAKEN),
        hx("420000000010f1004d00000003000000")
    );
    // CMSG_ITEM_TEXT_QUERY: the letter item's guid alone (`0x669110`, no requester).
    assert_eq!(
        messages::item_text_query(0x4000_0000_0000_0055),
        hx("5500000000000040")
    );
}

/// One attachment as the server writes it: slot, guid low, entry, 7 enchant triples, random
/// property, suffix, count, charges, max durability, durability and a zero byte.
fn push_attachment(body: &mut Vec<u8>, slot: u8, guid_low: u32, entry: u32, count: u32) {
    body.push(slot);
    body.extend_from_slice(&guid_low.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    for i in 0..7u32 {
        // Only the permanent slot carries an enchant here.
        let id = if i == 0 { 1897u32 } else { 0 };
        for v in [id, 0, 0] {
            body.extend_from_slice(&v.to_le_bytes());
        }
    }
    body.extend_from_slice(&(-7i32).to_le_bytes()); // random property
    body.extend_from_slice(&11u32.to_le_bytes()); // suffix factor
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // spell charges
    body.extend_from_slice(&40u32.to_le_bytes()); // max durability
    body.extend_from_slice(&35u32.to_le_bytes()); // durability
    body.push(0);
}

/// One row: the skipped size, id and type, the sender field, the fixed fields, two strings and
/// the attachments.
#[allow(clippy::too_many_arguments)]
fn push_row(
    body: &mut Vec<u8>,
    id: u32,
    kind: u8,
    sender: &[u8],
    money: u32,
    flags: u32,
    subject: &str,
    text: &str,
    attachments: &[(u8, u32, u32, u32)],
) {
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&id.to_le_bytes());
    body.push(kind);
    body.extend_from_slice(sender);
    body.extend_from_slice(&0u32.to_le_bytes()); // cod
    body.extend_from_slice(&0u32.to_le_bytes()); // package
    body.extend_from_slice(&41u32.to_le_bytes()); // stationery
    body.extend_from_slice(&money.to_le_bytes());
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&29.5f32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // template
    body.extend_from_slice(subject.as_bytes());
    body.push(0);
    body.extend_from_slice(text.as_bytes());
    body.push(0);
    body.push(attachments.len() as u8);
    for &(slot, guid, entry, count) in attachments {
        push_attachment(body, slot, guid, entry, count);
    }
}

#[test]
fn mail_list_result_wire() {
    // `u32` total, `u8` shown, then the rows (`0x571c50`).
    let mut body = 7u32.to_le_bytes().to_vec();
    body.push(4);
    push_row(
        &mut body,
        1,
        mail_message_type::NORMAL,
        &0x0000_0000_0000_00AAu64.to_le_bytes(),
        500,
        0x10,
        "Old Friend",
        "Hello there.",
        &[(0, 0x31, 2589, 5), (5, 0x32, 6948, 1)],
    );
    // An auction notice: the sender is the `AuctionHouse.dbc` row.
    push_row(
        &mut body,
        2,
        mail_message_type::AUCTION,
        &7u32.to_le_bytes(),
        0,
        0x4,
        "2589:0:2:1001:5",
        "              6C:100:100:5:5:0:0",
        &[],
    );
    push_row(
        &mut body,
        3,
        mail_message_type::CALENDAR,
        &99u32.to_le_bytes(),
        0,
        0,
        "6C:Raid",
        "1234",
        &[],
    );
    // A type past 5 has no sender field at all.
    push_row(&mut body, 4, 9, &[], 0, 0, "Odd", "", &[]);

    let packet = parse_all(opcode::SMSG_MAIL_LIST_RESULT, &body);
    let ServerPacket::MailList(list) = &packet else {
        panic!("mail list, got {}", packet.name());
    };
    assert_eq!(
        list.total, 7,
        "the server's whole count, past the shown rows"
    );
    assert_eq!(list.mails.len(), 4);

    let m = &list.mails[0];
    assert_eq!(
        (m.message_id, m.message_type),
        (1, mail_message_type::NORMAL)
    );
    assert_eq!((m.sender_guid, m.sender_entry), (0xAA, 0));
    assert_eq!((m.money, m.flags, m.stationery), (500, 0x10, 41));
    assert_eq!(m.expire_days, 29.5);
    assert_eq!(
        (m.subject.as_str(), m.body.as_str()),
        ("Old Friend", "Hello there.")
    );
    assert_eq!(m.attachments.len(), 2);
    let a = &m.attachments[1];
    assert_eq!(
        (a.slot, a.item_guid_low, a.entry, a.count),
        (5, 0x32, 6948, 1)
    );
    assert_eq!(
        a.enchants[0],
        MailEnchant {
            id: 1897,
            duration: 0,
            charges: 0
        }
    );
    assert_eq!((a.random_property_id, a.suffix_factor), (-7, 11));
    assert_eq!((a.max_durability, a.durability), (40, 35));

    let m = &list.mails[1];
    assert_eq!((m.sender_guid, m.sender_entry), (0, 7));
    assert!(m.attachments.is_empty());
    let m = &list.mails[2];
    assert_eq!(
        (m.message_type, m.sender_entry),
        (mail_message_type::CALENDAR, 99)
    );
    let m = &list.mails[3];
    assert_eq!((m.message_id, m.sender_guid, m.sender_entry), (4, 0, 0));
    assert_eq!(m.subject, "Odd");

    match decode(packet).pop().unwrap() {
        SessionEvent::MailList(list) => assert_eq!(list.mails.len(), 4),
        other => panic!("mail list event, got {other:?}"),
    }
}

#[test]
fn send_mail_result_wire() {
    let result = |mail_id: u32, action: u32, error: u32, tail: &[u32]| {
        let mut b = Vec::new();
        for v in [mail_id, action, error].iter().chain(tail) {
            b.extend_from_slice(&v.to_le_bytes());
        }
        match parse_all(opcode::SMSG_SEND_MAIL_RESULT, &b) {
            ServerPacket::SendMailResult(r) => r,
            other => panic!("send mail result, got {}", other.name()),
        }
    };

    let r = result(1, mail_action::SEND, mail_error::OK, &[]);
    assert_eq!((r.mail_id, r.equip_error, r.item), (1, None, None));
    let r = result(2, mail_action::ITEM_TAKEN, mail_error::EQUIP_ERROR, &[50]);
    assert_eq!((r.equip_error, r.item), (Some(50), None));
    // ITEM_TAKEN on OK and on ITEM_HAS_EXPIRED name the item by its guid low part and count.
    let r = result(3, mail_action::ITEM_TAKEN, mail_error::OK, &[0x31, 5]);
    assert_eq!(r.item, Some((0x31, 5)));
    let r = result(
        3,
        mail_action::ITEM_TAKEN,
        mail_error::ITEM_HAS_EXPIRED,
        &[0x31, 5],
    );
    assert_eq!(r.item, Some((0x31, 5)));
    // A refused take carries no tail.
    let r = result(3, mail_action::ITEM_TAKEN, mail_error::INTERNAL_ERROR, &[]);
    assert_eq!(r.item, None);

    let mut b = Vec::new();
    for v in [4u32, mail_action::DELETED, mail_error::OK] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    match decode(parse_all(opcode::SMSG_SEND_MAIL_RESULT, &b))
        .pop()
        .unwrap()
    {
        SessionEvent::SendMailResult(r) => {
            assert_eq!((r.mail_id, r.action), (4, mail_action::DELETED))
        }
        other => panic!("send mail result event, got {other:?}"),
    }
}

#[test]
fn item_text_query_response_wire() {
    // `u8` 0, the letter item's guid and its text (`0x635a40`).
    let mut body = vec![0u8];
    body.extend_from_slice(&0x4000_0000_0000_0055u64.to_le_bytes());
    body.extend_from_slice(b"This is the letter body.\0");
    match decode(parse_all(opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &body))
        .pop()
        .unwrap()
    {
        SessionEvent::ItemText { item_guid, text } => {
            assert_eq!(item_guid, 0x4000_0000_0000_0055);
            assert_eq!(text, "This is the letter body.");
        }
        other => panic!("item text event, got {other:?}"),
    }
    // A nonzero result carries nothing and yields no event.
    assert!(decode(parse_all(opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &[1])).is_empty());
}

#[test]
fn received_mail_wire() {
    // `f32` delay (`0x571a10`); AzerothCore writes `uint32(0)`, the bits of 0.0.
    for seconds in [0.0f32, 3600.0] {
        let packet = parse_all(opcode::SMSG_RECEIVED_MAIL, &seconds.to_le_bytes());
        match decode(packet).pop().unwrap() {
            SessionEvent::ReceivedMail { seconds: got } => assert_eq!(got, seconds),
            other => panic!("received mail event, got {other:?}"),
        }
    }
}

#[test]
fn query_next_mail_time_wire() {
    // None waiting: the time and a zero count.
    let mut none = (-86400.0f32).to_le_bytes().to_vec();
    none.extend_from_slice(&0u32.to_le_bytes());
    match parse_all(opcode::MSG_QUERY_NEXT_MAIL_TIME, &none) {
        ServerPacket::NextMailTime { seconds, senders } => {
            assert_eq!(seconds, -86400.0);
            assert!(senders.is_empty());
        }
        other => panic!("next mail time, got {}", other.name()),
    }

    // Waiting: a player sender and an auction house, each `u64, u32, u32, u32, f32`.
    let mut two = 0.0f32.to_le_bytes().to_vec();
    two.extend_from_slice(&2u32.to_le_bytes());
    for (guid, entry, kind, stationery, delay) in
        [(0xAAu64, 0u32, 0u32, 41u32, 0.0f32), (0, 7, 2, 62, -5.0)]
    {
        two.extend_from_slice(&guid.to_le_bytes());
        two.extend_from_slice(&entry.to_le_bytes());
        two.extend_from_slice(&kind.to_le_bytes());
        two.extend_from_slice(&stationery.to_le_bytes());
        two.extend_from_slice(&delay.to_le_bytes());
    }
    match decode(parse_all(opcode::MSG_QUERY_NEXT_MAIL_TIME, &two))
        .pop()
        .unwrap()
    {
        SessionEvent::NextMailTime { seconds, senders } => {
            assert_eq!(seconds, 0.0);
            assert_eq!(senders.len(), 2);
            assert_eq!((senders[0].guid, senders[0].stationery), (0xAA, 41));
            assert_eq!((senders[1].entry, senders[1].message_type), (7, 2));
            assert_eq!(senders[1].delay, -5.0);
        }
        other => panic!("next mail time event, got {other:?}"),
    }

    // Past three the client reads no further (`0x56db26`), so the rest stays unread.
    let mut four = 0.0f32.to_le_bytes().to_vec();
    four.extend_from_slice(&4u32.to_le_bytes());
    four.extend(std::iter::repeat_n(0u8, 24 * 4));
    let (packet, tail) =
        messages::parse_server_with_tail(opcode::MSG_QUERY_NEXT_MAIL_TIME, &four).unwrap();
    match packet {
        ServerPacket::NextMailTime { senders, .. } => assert_eq!(senders.len(), 3),
        other => panic!("next mail time, got {}", other.name()),
    }
    assert_eq!(tail, 24);
}

#[test]
fn show_mailbox_wire() {
    // `SMSG_SHOW_MAILBOX` (0x297, `0x56dbc0`): the mailbox guid.
    let packet = parse_all(opcode::SMSG_SHOW_MAILBOX, &MAILBOX.to_le_bytes());
    match decode(packet).pop().unwrap() {
        SessionEvent::ShowMailbox { mailbox } => assert_eq!(mailbox, MAILBOX),
        other => panic!("show mailbox event, got {other:?}"),
    }
}
