//! `SMSG_UPDATE_LFG_LIST` (`0x55b770`) against hand-built bytes, and the raid browser's sends.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, opcode, LfgPacket};

fn u32le(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn u64le(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// A difference list with one removed guid, a group with every field and a member with the full
/// stats block, then a solo player with the area and status alone.
#[test]
fn an_update_list_reads_to_its_end() {
    let mut b = Vec::new();
    u32le(&mut b, 2);
    u32le(&mut b, 249);
    b.push(1);
    u32le(&mut b, 1);
    u64le(&mut b, 0x77);
    // One group of one, out of three.
    u32le(&mut b, 1);
    u32le(&mut b, 3);
    u64le(&mut b, 0x1F10_0000_0000_0005);
    u32le(&mut b, 0x92);
    b.extend_from_slice(b"lfm\0");
    b.extend_from_slice(&[1, 0, 0]);
    u64le(&mut b, 0x4000);
    u32le(&mut b, 0b101);
    // Two players, out of nine.
    u32le(&mut b, 2);
    u32le(&mut b, 9);
    u64le(&mut b, 0x10);
    u32le(&mut b, 0x1 | 0x2 | 0x4 | 0x8 | 0x10);
    b.extend_from_slice(&[80, 1, 2, 10, 20, 30]);
    for v in 1..=6u32 {
        u32le(&mut b, v);
    }
    b.extend_from_slice(&5.5f32.to_le_bytes());
    b.extend_from_slice(&6.5f32.to_le_bytes());
    for v in 7..=11u32 {
        u32le(&mut b, v);
    }
    b.extend_from_slice(&200.0f32.to_le_bytes());
    for v in 12..=17u32 {
        u32le(&mut b, v);
    }
    b.extend_from_slice(b"\0");
    b.push(1);
    u64le(&mut b, 0x1F10_0000_0000_0005);
    b.push(0x3);
    u64le(&mut b, 0x11);
    u32le(&mut b, 0x20 | 0x40);
    u32le(&mut b, 4395);
    b.push(1);

    let p = messages::parse_server(opcode::SMSG_UPDATE_LFG_LIST, &b).unwrap();
    let events = decode(p);
    let [SessionEvent::Lfg(LfgPacket::UpdateList(list))] = events.as_slice() else {
        panic!("expected the list");
    };
    assert_eq!((list.lfg_type, list.dungeon), (2, 249));
    assert_eq!(list.removed.as_deref(), Some(&[0x77][..]));
    assert_eq!((list.total_groups, list.total_players), (3, 9));
    let g = &list.groups[0];
    assert_eq!(g.comment.as_deref(), Some("lfm"));
    assert_eq!(g.roles, Some([1, 0, 0]));
    assert_eq!(g.instance, Some((0x4000, 0b101)));
    let p = &list.players[0];
    let s = p.stats.as_ref().unwrap();
    assert_eq!(
        (s.level, s.class, s.race, s.talents),
        (80, 1, 2, [10, 20, 30])
    );
    assert_eq!(
        (s.armor, s.crit_spell, s.mp5, s.mp5_combat),
        (1, 6, 5.5, 6.5)
    );
    assert_eq!(
        (s.attack_power, s.unknown, s.avg_item_level),
        (7, 11, 200.0)
    );
    assert_eq!((s.defense, s.expertise), (12, 17));
    assert_eq!(p.comment.as_deref(), Some(""));
    assert_eq!(p.leader, Some(true));
    assert_eq!(p.party, Some(0x1F10_0000_0000_0005));
    assert_eq!(p.roles, Some(0x3));
    assert_eq!(p.area, None);
    let solo = &list.players[1];
    assert_eq!(
        (solo.area, solo.status, solo.stats.is_none()),
        (Some(4395), Some(1), true)
    );
}

/// AzerothCore's empty answer to a join: a whole list with nothing in it.
#[test]
fn an_empty_list_has_no_difference() {
    let mut b = Vec::new();
    u32le(&mut b, 2);
    u32le(&mut b, 249);
    b.push(0);
    for _ in 0..4 {
        u32le(&mut b, 0);
    }
    match messages::parse_server(opcode::SMSG_UPDATE_LFG_LIST, &b).unwrap() {
        benilla_protocol::ServerPacket::Lfg(LfgPacket::UpdateList(l)) => {
            assert!(l.removed.is_none() && l.groups.is_empty() && l.players.is_empty());
        }
        other => panic!("{}", other.name()),
    }
}

/// `CMSG_LFG_LFR_JOIN` / `_LEAVE` (`0x558ed0`, `0x558f90`): the `u32` search key.
#[test]
fn the_search_sends_its_key() {
    assert_eq!(messages::lfr_search(2 << 24 | 249), vec![249, 0, 0, 2]);
}
