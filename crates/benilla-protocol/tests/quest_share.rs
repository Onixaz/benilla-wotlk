//! The party quest-share wire: the Share Quest push, the escort confirm, and the verdict relay.
//! `MSG_QUEST_PUSH_RESULT`'s guid is the sharer going up and the member concerned coming down.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, opcode, quest_flags, QuestShareMsg};
use benilla_protocol::ServerPacket;
use common::hx;

/// The three CMSG bodies (vmangos `Server/Packets/Quest.cpp:63-77`).
#[test]
fn cmsg_bodies_golden() {
    // CMSG_PUSHQUESTTOPARTY / CMSG_QUEST_CONFIRM_ACCEPT: one u32 quest id and no member guid; the
    // server addresses the group itself.
    assert_eq!(
        messages::push_quest_to_party(0x1234),
        hx("34120000"),
        "CMSG_PUSHQUESTTOPARTY body"
    );
    assert_eq!(
        messages::quest_confirm_accept(0x1234),
        hx("34120000"),
        "CMSG_QUEST_CONFIRM_ACCEPT body"
    );

    // MSG_QUEST_PUSH_RESULT going up: the u64 guid of the sharer we answer, the u32 quest, then
    // the u8 verdict, 13 bytes (`0x58cab5`-`0x58cb13`).
    let body =
        messages::quest_push_result(0x1234_5678_9abc_def0, 0x1234, QuestShareMsg::DECLINE_QUEST);
    assert_eq!(body.len(), 13);
    assert_eq!(
        body,
        hx("f0debc9a785634123412000003"),
        "MSG_QUEST_PUSH_RESULT (client) body"
    );
}

/// `MSG_QUEST_PUSH_RESULT` coming down (`Quest.cpp:81-85`): the same two fields, the guid now the
/// party member the verdict concerns.
#[test]
fn push_result_decodes_member_and_verdict() {
    let body = hx("f0debc9a7856341200");
    let p = messages::parse_server(opcode::MSG_QUEST_PUSH_RESULT, &body).unwrap();
    match &p {
        ServerPacket::QuestPushResult(r) => {
            assert_eq!(r.member, 0x1234_5678_9abc_def0);
            assert_eq!(r.msg, QuestShareMsg::SHARING_QUEST);
        }
        other => panic!("expected QuestPushResult, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::QuestPushResult { member, msg }] => {
            assert_eq!(*member, 0x1234_5678_9abc_def0);
            assert_eq!(*msg, QuestShareMsg::SHARING_QUEST);
        }
        other => panic!("push result decoded to {} events", other.len()),
    }
}

/// Every verdict byte survives encode, parse and decode; an unmapped one is data, not an error.
#[test]
fn every_verdict_byte_round_trips() {
    for raw in 0..=u8::MAX {
        let mut sent = 1u64.to_le_bytes().to_vec();
        sent.extend_from_slice(&7u32.to_le_bytes());
        sent.push(raw);
        assert_eq!(
            messages::quest_push_result(1, 7, QuestShareMsg(raw)),
            sent,
            "verdict {raw} encodes"
        );
        // The server's relay has no quest id.
        let mut body = 1u64.to_le_bytes().to_vec();
        body.push(raw);
        let p = messages::parse_server(opcode::MSG_QUEST_PUSH_RESULT, &body).unwrap();
        match &p {
            ServerPacket::QuestPushResult(r) => {
                assert_eq!(r.msg, QuestShareMsg(raw), "verdict {raw}");
                assert_eq!(r.member, 1);
            }
            other => panic!("verdict {raw} parsed as {}", other.name()),
        }
        match decode(p).as_slice() {
            [SessionEvent::QuestPushResult { member: 1, msg }] => {
                assert_eq!(*msg, QuestShareMsg(raw), "verdict {raw} decodes");
            }
            other => panic!("verdict {raw} decoded to {other:?}"),
        }
    }
}

/// `SMSG_QUEST_CONFIRM_ACCEPT` (`Quest.cpp:131-136`): `u32` questId, cstring title, `u64`
/// senderGuid; the title sits between the two numbers.
#[test]
fn confirm_accept_decodes_id_title_sender() {
    let mut body = 1234u32.to_le_bytes().to_vec();
    body.extend_from_slice(b"Escort Duty\0");
    body.extend_from_slice(&0x0000_0000_0000_002Au64.to_le_bytes());

    let p = messages::parse_server(opcode::SMSG_QUEST_CONFIRM_ACCEPT, &body).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::QuestConfirmAccept(c)] => {
            assert_eq!(c.quest_id, 1234);
            assert_eq!(c.title, "Escort Duty");
            assert_eq!(c.sender, 0x2A);
        }
        other => panic!("confirm accept decoded to {} events", other.len()),
    }
}

/// The two quest flags the share flow reads (vmangos `QuestDef.h:145-160`), the client's only
/// sign that a quest is shareable or an escort.
#[test]
fn share_quest_flags_are_the_documented_bits() {
    assert_eq!(quest_flags::PARTY_ACCEPT, 0x2);
    assert_eq!(quest_flags::SHARABLE, 0x8);
}
