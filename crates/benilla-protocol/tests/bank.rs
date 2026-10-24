//! The bank wire: the banker, slot-purchase and autostore opcodes, plus the two player fields the
//! vault window reads (the bank bag guids and the purchased-slot count).

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, ObjectFields};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn bank_bodies_golden() {
    // CMSG_BANKER_ACTIVATE (vmangos Npc.h:58-66): a full guid.
    assert_eq!(
        messages::banker_activate(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_BANKER_ACTIVATE body"
    );

    // CMSG_BUY_BANK_SLOT (vmangos Item.h:157-163): only a guid; the server buys the next slot.
    assert_eq!(
        messages::buy_bank_slot(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_BUY_BANK_SLOT body"
    );

    // CMSG_AUTOBANK_ITEM (vmangos Item.h:108-116): srcbag, srcslot.
    assert_eq!(
        messages::autobank_item(255, 23),
        vec![255u8, 23],
        "CMSG_AUTOBANK_ITEM body"
    );

    // CMSG_AUTOSTORE_BANK_ITEM (vmangos Item.h:118-126): the same shape; the server picks the
    // direction by whether the position is a bank slot.
    assert_eq!(
        messages::autostore_bank_item(255, 39),
        vec![255u8, 39],
        "CMSG_AUTOSTORE_BANK_ITEM body"
    );
}

#[test]
fn show_bank_wire() {
    // SMSG_SHOW_BANK (vmangos Npc.cpp:94): one full guid.
    let body = hx("f0debc9a78563412");
    match messages::parse_server(messages::opcode::SMSG_SHOW_BANK, &body).unwrap() {
        ServerPacket::ShowBank { banker } => {
            assert_eq!(banker, 0x1234_5678_9abc_def0);
        }
        other => panic!("show bank, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_SHOW_BANK, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::ShowBank { banker } => {
            assert_eq!(banker, 0x1234_5678_9abc_def0);
        }
        other => panic!("show bank event, got {other:?}"),
    }
}

#[test]
fn buy_bank_slot_result_wire() {
    use messages::bank_slot_result;

    // The result codes (vmangos Player.h:91-94).
    assert_eq!(
        (
            bank_slot_result::FAILED_TOO_MANY,
            bank_slot_result::INSUFFICIENT_FUNDS,
            bank_slot_result::NOTBANKER,
            bank_slot_result::OK,
        ),
        (0, 1, 2, 3)
    );

    // SMSG_BUY_BANK_SLOT_RESULT (vmangos Item.cpp:137-140): one u32 result.
    for &code in &[
        bank_slot_result::FAILED_TOO_MANY,
        bank_slot_result::INSUFFICIENT_FUNDS,
        bank_slot_result::NOTBANKER,
        bank_slot_result::OK,
    ] {
        let body = code.to_le_bytes().to_vec();
        match messages::parse_server(messages::opcode::SMSG_BUY_BANK_SLOT_RESULT, &body).unwrap() {
            ServerPacket::BuyBankSlotResult { result } => {
                assert_eq!(result, code, "result code {code}");
            }
            other => panic!("buy bank slot result, got {}", other.name()),
        }

        let packet =
            messages::parse_server(messages::opcode::SMSG_BUY_BANK_SLOT_RESULT, &body).unwrap();
        match decode(packet).pop().unwrap() {
            SessionEvent::BuyBankSlotResult { result } => {
                assert_eq!(result, code, "event result code {code}");
            }
            other => panic!("buy bank slot result event, got {other:?}"),
        }
    }
}

#[test]
fn player_bank_bag_slot_field() {
    // The shared space keeps the seven bank-bag guids past the 1.12.1 player end, from field 1572
    // (after the 28 bank slots at 1516).
    let f = ObjectFields::from_pairs(&[(1572, 0xEF), (1573, 0)]);
    assert_eq!(f.player_bank_bag_slot(0), Some(0xEF));
    assert_eq!(f.player_bank_bag_slot(1), None, "unsent slot reads None");
    assert_eq!(
        f.player_bank_bag_slot(7),
        None,
        "out of range (only 7 bag slots)"
    );

    // Bag slot 6 (the 7th, last purchasable slot): fields 1584/1585.
    let f6 = ObjectFields::from_pairs(&[(1584, 0x1234), (1585, 0)]);
    assert_eq!(f6.player_bank_bag_slot(6), Some(0x1234));
}

#[test]
fn player_bank_bag_slots_purchased_field() {
    // PLAYER_BYTES_2 (field 194), from byte 0: facialHair, unk, bankBagSlots, restState
    // (vmangos Player.h:347).
    let f = ObjectFields::from_pairs(&[(194, 0x0403_0201)]);
    assert_eq!(f.player_bank_bag_slots_purchased(), Some(3));
    assert_eq!(f.player_facial_hair(), Some(1), "byte 0 unaffected");

    // An unsent field reads None, not 0: unstreamed is not "zero slots bought".
    let empty = ObjectFields::from_pairs(&[]);
    assert_eq!(empty.player_bank_bag_slots_purchased(), None);

    let full = ObjectFields::from_pairs(&[(194, 7 << 16)]);
    assert_eq!(full.player_bank_bag_slots_purchased(), Some(7));
}
