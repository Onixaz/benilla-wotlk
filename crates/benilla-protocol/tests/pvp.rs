//! The honor wire end to end: an opcode dispatches to its reader, names itself, and reaches its
//! event unchanged.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

/// The 25-byte `MSG_INSPECT_HONOR_STATS` reply, as `0x5e7a50` reads it.
fn inspect_honor_body() -> Vec<u8> {
    hx(concat!(
        "b32a000001000000", // u64 playerGuid
        "0b",               // u8  highestRank
        "1100",             // u16 today's HK
        "0200",             // u16 yesterday's HK
        "80020000",         // u32 today's honor
        "ef200000",         // u32 yesterday's honor
        "430f0000",         // u32 lifetime HK
    ))
}

/// `MSG_INSPECT_HONOR_STATS` (0x2D6) also carries our request, a bare guid; `parse_server` only
/// sees the reply.
#[test]
fn inspect_honor_stats_wire() {
    let body = inspect_honor_body();
    let packet = messages::parse_server(messages::opcode::MSG_INSPECT_HONOR_STATS, &body).unwrap();
    assert_eq!(packet.name(), "MSG_INSPECT_HONOR_STATS");
    let ServerPacket::InspectHonorStats(stats) = packet else {
        panic!("inspect honor stats");
    };
    assert_eq!(stats.player_guid, 0x0000_0001_0000_2AB3);
    assert_eq!(
        (stats.highest_rank, stats.today_hk, stats.yesterday_hk),
        (11, 17, 2)
    );
    assert_eq!(
        (stats.today_honor, stats.yesterday_honor, stats.lifetime_hk),
        (640, 8_431, 3_907)
    );

    match decode(ServerPacket::InspectHonorStats(stats)).as_slice() {
        [SessionEvent::InspectHonorStats(ev)] => assert_eq!(*ev, stats),
        other => panic!("one inspect-honor event, got {other:?}"),
    }

    assert_eq!(
        messages::inspect_honor_stats(0x0000_0001_0000_2AB3),
        hx("b32a000001000000")
    );
}

/// `MSG_INSPECT_ARENA_TEAMS` (0x377): one team slot per packet.
#[test]
fn inspect_arena_teams_wire() {
    let body = hx("b32a00000100000001090000004006000014000000");
    let mut full = body.clone();
    full.extend_from_slice(&hx("0c00000012000000"));
    full.extend_from_slice(&hx("2c060000"));
    let packet = messages::parse_server(messages::opcode::MSG_INSPECT_ARENA_TEAMS, &full).unwrap();
    let ServerPacket::InspectArenaTeam(t) = packet else {
        panic!("inspect arena team");
    };
    assert_eq!(
        (t.slot, t.team_id, t.rating, t.personal_rating),
        (1, 9, 1600, 1580)
    );
    assert!(matches!(
        decode(ServerPacket::InspectArenaTeam(t)).as_slice(),
        [SessionEvent::InspectArenaTeam(_)]
    ));
}

/// `SMSG_PVP_CREDIT` (0x28C): honor, victim guid, and the victim's internal rank (vmangos
/// `SendPVPCredit`, floored at 5 for a player), a `PVP_RANK_<rank>_<team>` key, not the badge.
#[test]
fn pvp_credit_wire() {
    // i32 honor = 143, u64 victimGuid = 0x0000000100002AB3, i32 victimRank = 11.
    let body = hx("8f000000b32a0000010000000b000000");
    assert_eq!(body.len(), 16);
    let packet = messages::parse_server(messages::opcode::SMSG_PVP_CREDIT, &body).unwrap();
    assert_eq!(packet.name(), "SMSG_PVP_CREDIT");
    let ServerPacket::PvpCredit(credit) = packet else {
        panic!("pvp credit");
    };
    assert_eq!(credit.honor, 143);
    assert_eq!(credit.victim_guid, 0x0000_0001_0000_2AB3);
    assert_eq!(credit.victim_rank, 11);

    match decode(ServerPacket::PvpCredit(credit)).as_slice() {
        [SessionEvent::PvpCredit(ev)] => assert_eq!(*ev, credit),
        other => panic!("one pvp-credit event, got {other:?}"),
    }

    // A dishonorable kill rides the same packet with negative honor (`HonorMgr.cpp:807`).
    let dk = hx("fbffffffb32a0000010000000b000000");
    let ServerPacket::PvpCredit(credit) =
        messages::parse_server(messages::opcode::SMSG_PVP_CREDIT, &dk).unwrap()
    else {
        panic!("pvp credit");
    };
    assert_eq!(credit.honor, -5);
}

#[test]
fn truncated_honor_bodies_error_at_the_dispatch() {
    let full = inspect_honor_body();
    assert!(
        messages::parse_server(messages::opcode::MSG_INSPECT_HONOR_STATS, &full[..24]).is_err()
    );
    assert!(messages::parse_server(messages::opcode::SMSG_PVP_CREDIT, &[0u8; 15]).is_err());
}
