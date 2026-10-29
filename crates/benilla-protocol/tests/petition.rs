//! The petition wire: the guild and arena charters, laid out as 12340 reads and writes them.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, opcode, petition_result, MAX_PETITION_SIGNATURES};
use benilla_protocol::ServerPacket;
use common::hx;

#[test]
fn cmsg_bodies_golden() {
    // CMSG_PETITION_SHOWLIST (vmangos Server/Packets/Petition.cpp:3-6): one u64, the NPC.
    assert_eq!(
        messages::petition_show_list(0x0000_0000_0000_2a1f),
        hx("1f2a000000000000"),
        "CMSG_PETITION_SHOWLIST body"
    );

    // CMSG_PETITION_SHOW_SIGNATURES (Petition.cpp:8-11): one u64, the charter item.
    assert_eq!(
        messages::petition_show_signatures(0x0000_0000_0001_0203),
        hx("0302010000000000"),
        "CMSG_PETITION_SHOW_SIGNATURES body"
    );

    // CMSG_TURN_IN_PETITION (Petition.cpp:24-27): one u64, the item.
    assert_eq!(
        messages::turn_in_petition(0x0000_0000_0001_0203),
        hx("0302010000000000"),
        "CMSG_TURN_IN_PETITION body"
    );

    // MSG_PETITION_DECLINE outbound (Petition.cpp:19-22): one u64, the item, not the player.
    assert_eq!(
        messages::petition_decline(0x0000_0000_0001_0203),
        hx("0302010000000000"),
        "MSG_PETITION_DECLINE body"
    );

    // CMSG_PETITION_SIGN (Petition.cpp:35-39): u64 item, then an i8 the server skips; the reference
    // defaults it to 1.
    assert_eq!(
        messages::petition_sign(0x0000_0000_0001_0203, 1),
        hx("030201000000000001"),
        "CMSG_PETITION_SIGN body, the client's default byte"
    );
    assert_eq!(
        messages::petition_sign(0x0000_0000_0001_0203, -1),
        hx("0302010000000000ff"),
        "…and a negative argument rides as its two's complement"
    );

    // CMSG_OFFER_PETITION (`0x5cf220`): the team size word, u64 item, u64 target player.
    assert_eq!(
        messages::offer_petition(0, 0x0000_0000_0001_0203, 0x0000_0000_0000_00aa),
        hx("000000000302010000000000aa00000000000000"),
        "CMSG_OFFER_PETITION body"
    );

    // CMSG_PETITION_QUERY (Petition.cpp:13-17): u32 petitionId, u64 itemGuid.
    assert_eq!(
        messages::petition_query(7, 0x0000_0000_0001_0203),
        hx("070000000302010000000000"),
        "CMSG_PETITION_QUERY body"
    );

    // MSG_PETITION_RENAME outbound (Petition.cpp:29-33): u64 item, cstring newName.
    assert_eq!(
        messages::petition_rename(0x0000_0000_0001_0203, "Legacy"),
        hx("03020100000000004c656761637900"),
        "MSG_PETITION_RENAME body"
    );
}

/// `CMSG_PETITION_BUY` is the NPC and a whole packed petition record (`0x98cf70`): the name is its
/// title and the vendor row's index its `muid`, the word before the type.
#[test]
fn petition_buy_body_is_the_packed_record() {
    let body = messages::petition_buy(0x0000_0000_0000_2a1f, "Legacy", 1);
    assert_eq!(
        body,
        hx(concat!(
            "1f2a000000000000", // u64 npcGuid
            "00000000",         // u32 id
            "0000000000000000", // u64 owner
            "4c656761637900",   // cstring title "Legacy"
            "00",               // cstring body text
            // 7 x u32: min, max, deadline, issue, guild, classes, races
            "00000000000000000000000000000000000000000000000000000000",
            "0000",                     // u16 genders
            "000000000000000000000000", // u32 min level, max level, choice count
            "00000000000000000000",     // ten empty choices
            "01000000",                 // u32 muid, the row index
            "00000000",                 // u32 static type
        )),
        "CMSG_PETITION_BUY body"
    );
    assert_eq!(&body[20..27], b"Legacy\0", "the title starts at offset 20");
}

/// `SMSG_PETITION_SHOWLIST` (`0x6d69f0`): six words a row. An arena registrar sends the three
/// team sizes, the required signatures one less than each.
#[test]
fn petition_show_list_parses_six_word_rows() {
    let body = hx(concat!(
        "1f2a000000000000", // u64 npcGuid
        "02",               // u8 count
        // row 1: index 1, entry 23560, display 16161, cost 800000, type 2, signatures 2
        "01000000",
        "085c0000",
        "213f0000",
        "00350c00",
        "02000000",
        "02000000",
        // row 2: index 2, the 3v3 charter
        "02000000",
        "095c0000",
        "213f0000",
        "80841e00",
        "03000000",
        "03000000",
    ));
    let packet = messages::parse_server(opcode::SMSG_PETITION_SHOWLIST, &body).unwrap();
    let ServerPacket::PetitionShowList(list) = &packet else {
        panic!("expected PetitionShowList, got {}", packet.name());
    };
    assert_eq!(list.npc, 0x2a1f);
    assert_eq!(list.entries.len(), 2);
    let r = &list.entries[0];
    assert_eq!(
        (r.index, r.charter_entry, r.charter_display_id),
        (1, 23560, 16161)
    );
    assert_eq!(
        (r.charter_cost, r.charter_type, r.required_signatures),
        (800_000, 2, 2)
    );
    assert_eq!(list.entries[1].charter_cost, 2_000_000);

    let events = decode(packet);
    assert!(
        matches!(events.as_slice(), [SessionEvent::PetitionShowList(l)] if l.npc == 0x2a1f),
        "one PetitionShowList event, got {events:?}"
    );
}

/// `SMSG_PETITION_SHOW_SIGNATURES`: `u64` item, `u64` owner, `u32` petitionId, `u8` count, then
/// 12 bytes per signature, the signer's guid and a zero dword (vmangos
/// `Handlers/PetitionsHandler.cpp:160-168`); nine signatures make a wrong stride fail plainly.
#[test]
fn petition_show_signatures_stride_is_twelve_bytes() {
    let mut body = hx(concat!(
        "0302010000000000", // u64 itemGuid
        "aa00000000000000", // u64 ownerGuid
        "07000000",         // u32 petitionId
        "09",               // u8 signatureCount
    ));
    for signer in 1..=MAX_PETITION_SIGNATURES as u64 {
        body.extend_from_slice(&signer.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
    }

    let packet = messages::parse_server(opcode::SMSG_PETITION_SHOW_SIGNATURES, &body).unwrap();
    let ServerPacket::PetitionShowSignatures(sigs) = &packet else {
        panic!("expected PetitionShowSignatures, got {}", packet.name());
    };
    assert_eq!(sigs.item, 0x0001_0203);
    assert_eq!(sigs.owner, 0xaa);
    assert_eq!(sigs.petition_id, 7);
    assert_eq!(sigs.signatures.len(), MAX_PETITION_SIGNATURES);
    assert_eq!(
        sigs.signatures[MAX_PETITION_SIGNATURES - 1].signer,
        MAX_PETITION_SIGNATURES as u64,
        "the ninth signer, at stride 12"
    );
    assert!(sigs.signatures.iter().all(|s| s.unknown == 0));

    let events = decode(packet);
    assert!(
        matches!(events.as_slice(), [SessionEvent::PetitionShowSignatures(s)] if s.petition_id == 7),
        "one PetitionShowSignatures event, got {events:?}"
    );
}

/// `SMSG_PETITION_QUERY_RESPONSE` (the petition record's unpack, `0x98d090`): `allowedGenders`
/// alone is a `u16`, ten choice strings follow whatever the count says, and `muid` precedes the
/// type.
#[test]
fn petition_query_response_reads_the_12340_record() {
    let body = hx(concat!(
        "07000000",         // u32 petitionId
        "aa00000000000000", // u64 ownerGuid
        "4c656761637900",   // cstring name "Legacy"
        "00",               // cstring bodyText ""
        "01000000",         // u32 minSignatures
        "01000000",         // u32 maxSignatures
        "11111111",         // u32 deadline
        "22222222",         // u32 issue date
        "33333333",         // u32 allowedGuildID
        "44444444",         // u32 allowedClasses
        "55555555",         // u32 allowedRaces
        "6666",             // u16 allowedGenders  <-- sixteen bits
        "77777777",         // u32 allowedMinLevel
        "88888888",         // u32 allowedMaxLevel
        "00000000",         // u32 choiceCount
        "00000000000000000000",
        "99999999", // u32 muid
        "01000000", // u32 static type: arena
    ));
    let packet = messages::parse_server(opcode::SMSG_PETITION_QUERY_RESPONSE, &body).unwrap();
    let ServerPacket::PetitionQueryResponse(r) = &packet else {
        panic!("expected PetitionQueryResponse, got {}", packet.name());
    };
    assert_eq!(
        (r.petition_id, r.owner, r.name.as_str()),
        (7, 0xaa, "Legacy")
    );
    assert_eq!((r.min_signatures, r.max_signatures), (1, 1));
    assert_eq!(r.allowed_genders, 0x6666, "sixteen bits, not thirty-two");
    assert_eq!(r.allowed_min_level, 0x7777_7777);
    assert_eq!(r.allowed_max_level, 0x8888_8888);
    assert_eq!(r.choices.len(), 10, "ten rows whatever the count");
    assert_eq!(r.muid, 0x9999_9999);
    assert_eq!(r.static_type, messages::PETITION_TYPE_ARENA);

    let events = decode(packet);
    assert!(
        matches!(events.as_slice(), [SessionEvent::PetitionQueryResponse(q)] if q.name == "Legacy"),
        "one PetitionQueryResponse event, got {events:?}"
    );
}

/// `SMSG_PETITION_SIGN_RESULTS` (`Petition.cpp:69-74`) carries two guids and a code,
/// `SMSG_TURN_IN_PETITION_RESULTS` (`:76-79`) only the code, from the same `PetitionSigns` enum.
#[test]
fn the_two_results_packets_share_an_enum_but_not_a_layout() {
    let body = hx(concat!(
        "0302010000000000", // u64 itemGuid
        "bb00000000000000", // u64 playerGuid: the signer, in both copies of this packet
        "03000000",         // u32 result = CANT_SIGN_OWN
    ));
    let packet = messages::parse_server(opcode::SMSG_PETITION_SIGN_RESULTS, &body).unwrap();
    let ServerPacket::PetitionSignResults(r) = &packet else {
        panic!("expected PetitionSignResults, got {}", packet.name());
    };
    assert_eq!(r.item, 0x0001_0203);
    assert_eq!(r.player, 0xbb);
    assert_eq!(r.result, petition_result::CANT_SIGN_OWN);

    let packet = messages::parse_server(
        opcode::SMSG_TURN_IN_PETITION_RESULTS,
        &hx("04000000"), // NEED_MORE
    )
    .unwrap();
    assert!(
        matches!(
            packet,
            ServerPacket::TurnInPetitionResults { result } if result == petition_result::NEED_MORE
        ),
        "expected TurnInPetitionResults, got {}",
        packet.name()
    );
    let events = decode(packet);
    assert!(
        matches!(
            events.as_slice(),
            [SessionEvent::TurnInPetitionResults { result: 4 }]
        ),
        "one TurnInPetitionResults event, got {events:?}"
    );
}

/// `MSG_PETITION_DECLINE` sends the charter's item guid but receives the decliner's player guid,
/// both eight bytes; `MSG_PETITION_RENAME` is `u64` item and cstring name both ways.
#[test]
fn msg_petition_decline_differs_by_direction_and_rename_does_not() {
    assert_eq!(
        messages::petition_decline(0x0001_0203),
        hx("0302010000000000"),
        "outbound MSG_PETITION_DECLINE carries the item guid"
    );
    let packet =
        messages::parse_server(opcode::MSG_PETITION_DECLINE, &hx("bb00000000000000")).unwrap();
    assert!(
        matches!(packet, ServerPacket::PetitionDeclined { player } if player == 0xbb),
        "inbound MSG_PETITION_DECLINE carries the player guid, got {}",
        packet.name()
    );

    // The rename echo arrives only on success.
    let body = hx("03020100000000004c656761637900");
    assert_eq!(
        messages::petition_rename(0x0001_0203, "Legacy"),
        body,
        "outbound MSG_PETITION_RENAME"
    );
    let packet = messages::parse_server(opcode::MSG_PETITION_RENAME, &body).unwrap();
    let ServerPacket::PetitionRenamed(r) = &packet else {
        panic!("expected PetitionRenamed, got {}", packet.name());
    };
    assert_eq!(r.item, 0x0001_0203);
    assert_eq!(r.name, "Legacy");

    let events = decode(packet);
    assert!(
        matches!(events.as_slice(), [SessionEvent::PetitionRenamed(p)] if p.name == "Legacy"),
        "one PetitionRenamed event, got {events:?}"
    );
}

#[test]
fn a_freshly_bought_charter_has_no_signatures() {
    let body = hx(concat!(
        "0302010000000000", // item
        "aa00000000000000", // owner
        "07000000",         // petitionId
        "00",               // no signatures
    ));
    let packet = messages::parse_server(opcode::SMSG_PETITION_SHOW_SIGNATURES, &body).unwrap();
    let ServerPacket::PetitionShowSignatures(sigs) = &packet else {
        panic!("expected PetitionShowSignatures, got {}", packet.name());
    };
    assert!(sigs.signatures.is_empty());
    assert_eq!(sigs.owner, 0xaa, "the owner is still named");
}
