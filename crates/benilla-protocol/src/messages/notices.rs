//! Small server notices: the light override, the UI clock, arena and refer-a-friend errors, the
//! level grant offer, the unlearn list, the inebriation threshold, the dance moves, the message of
//! the day, a channel's member count, a whois answer, the realm split and realm-first awards.

use std::io;

use crate::wire::{read_cstring, read_packed_guid, read_u32_le, read_u64_le, read_u8};

/// `SMSG_OVERRIDE_LIGHT` (`0x526530`): `Light.dbc` row `light` blends as row `override_id` over
/// `transition_ms`; an override of 0 fades the override out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrideLight {
    pub light: u32,
    pub override_id: u32,
    pub transition_ms: u32,
}

/// `SMSG_CROSSED_INEBRIATION_THRESHOLD` (`0x6d01b0`): a player's drunk state changed, by the item
/// that did it, 0 for none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InebriationThreshold {
    pub player: u64,
    pub threshold: u32,
    pub item: u32,
}

pub(super) fn read_override_light(r: &mut &[u8]) -> io::Result<OverrideLight> {
    Ok(OverrideLight {
        light: read_u32_le(r)?,
        override_id: read_u32_le(r)?,
        transition_ms: read_u32_le(r)?,
    })
}

/// `SMSG_WORLD_STATE_UI_TIMER_UPDATE` (`0x526530`): the server's UI clock, Unix seconds.
pub(super) fn read_ui_time(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// `SMSG_ARENA_ERROR` (`0x6cce90`): the error code, then the team type for error 0 only.
pub(super) fn read_arena_error(r: &mut &[u8]) -> io::Result<(u32, Option<u8>)> {
    let error = read_u32_le(r)?;
    let team_type = if error == 0 { Some(read_u8(r)?) } else { None };
    Ok((error, team_type))
}

/// `SMSG_PROPOSE_LEVEL_GRANT` (`0x526530`): the granter's packed guid.
pub(super) fn read_propose_level_grant(r: &mut &[u8]) -> io::Result<u64> {
    read_packed_guid(r)
}

/// `SMSG_REFER_A_FRIEND_FAILURE` (`0x526530`): the error code, then a name for error 9 only.
pub(super) fn read_refer_a_friend_failure(r: &mut &[u8]) -> io::Result<(u32, String)> {
    let error = read_u32_le(r)?;
    let name = if error == 9 {
        read_cstring(r)?
    } else {
        String::new()
    };
    Ok((error, name))
}

/// `SMSG_SEND_UNLEARN_SPELLS` (`0x6e2240`): a count, then that many spell ids.
pub(super) fn read_unlearn_spells(r: &mut &[u8]) -> io::Result<Vec<u32>> {
    let count = read_u32_le(r)?;
    let mut spells = Vec::with_capacity(crate::wire::capacity_hint(count, 64));
    for _ in 0..count {
        spells.push(read_u32_le(r)?);
    }
    Ok(spells)
}

pub(super) fn read_inebriation_threshold(r: &mut &[u8]) -> io::Result<InebriationThreshold> {
    Ok(InebriationThreshold {
        player: read_u64_le(r)?,
        threshold: read_u32_le(r)?,
        item: read_u32_le(r)?,
    })
}

/// `SMSG_LEARNED_DANCE_MOVES` (`0x5758a0`): one 64-bit mask of learned moves.
pub(super) fn read_learned_dance_moves(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

/// `SMSG_MOTD` (`0x551660`): a line count, then that many lines. Deviation: no line is cut at the
/// handler's 100-byte buffer.
pub(super) fn read_motd(r: &mut &[u8]) -> io::Result<Vec<String>> {
    let count = read_u32_le(r)?;
    let mut lines = Vec::with_capacity(crate::wire::capacity_hint(count, 16));
    for _ in 0..count {
        lines.push(read_cstring(r)?);
    }
    Ok(lines)
}

/// `SMSG_CHANNEL_MEMBER_COUNT` (`0x4fb540`): the channel, its flags and its count, which the
/// handler reads and drops.
pub(super) fn read_channel_member_count(r: &mut &[u8]) -> io::Result<(String, u8, u32)> {
    Ok((read_cstring(r)?, read_u8(r)?, read_u32_le(r)?))
}

/// `SMSG_REALM_SPLIT` (`0x4d8200`): the client's state, the realm's split state and the date the
/// choice closes; an empty body reads 0, 0 and `""`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealmSplit {
    pub client_state: u32,
    pub split_state: u32,
    pub date: String,
}

pub(super) fn read_realm_split(r: &mut &[u8]) -> io::Result<RealmSplit> {
    if r.is_empty() {
        return Ok(RealmSplit::default());
    }
    Ok(RealmSplit {
        client_state: read_u32_le(r)?,
        split_state: read_u32_le(r)?,
        date: read_cstring(r)?,
    })
}

/// `SMSG_SERVER_FIRST_ACHIEVEMENT` (`0x50b010`): the earner's name and guid, the achievement, and
/// whether the name prints as a player link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFirstAchievement {
    pub name: String,
    pub guid: u64,
    pub achievement: u32,
    pub linked: bool,
}

pub(super) fn read_server_first_achievement(r: &mut &[u8]) -> io::Result<ServerFirstAchievement> {
    Ok(ServerFirstAchievement {
        name: read_cstring(r)?,
        guid: read_u64_le(r)?,
        achievement: read_u32_le(r)?,
        linked: read_u32_le(r)? != 0,
    })
}

/// `CMSG_REALM_SPLIT` body: the state, `0xFFFFFFFF` to ask (`0x4de2e0`).
pub fn realm_split(state: u32) -> Vec<u8> {
    state.to_le_bytes().to_vec()
}

/// `CMSG_ACCEPT_LEVEL_GRANT` body: the granter's packed guid (`0x521b00`).
pub fn accept_level_grant(granter: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    crate::wire::write_packed_guid(granter, &mut body).expect("a Vec takes every byte");
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    fn parse(op: u16, body: &[u8]) -> ServerPacket {
        parse_server(op, body).unwrap()
    }

    #[test]
    fn every_notice_decodes_its_12340_body() {
        let mut b = Vec::new();
        for v in [1u32, 2, 3000] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        assert!(matches!(
            parse(opcode::SMSG_OVERRIDE_LIGHT, &b),
            ServerPacket::OverrideLight(OverrideLight {
                light: 1,
                override_id: 2,
                transition_ms: 3000
            })
        ));
        assert!(matches!(
            parse(
                opcode::SMSG_WORLD_STATE_UI_TIMER_UPDATE,
                &1_700_000_000u32.to_le_bytes()
            ),
            ServerPacket::UiTime(1_700_000_000)
        ));
        let mut b = 0u32.to_le_bytes().to_vec();
        b.push(3);
        assert!(matches!(
            parse(opcode::SMSG_ARENA_ERROR, &b),
            ServerPacket::ArenaError {
                error: 0,
                team_type: Some(3)
            }
        ));
        assert!(matches!(
            parse(opcode::SMSG_ARENA_ERROR, &1u32.to_le_bytes()),
            ServerPacket::ArenaError {
                error: 1,
                team_type: None
            }
        ));
        assert!(matches!(
            parse(opcode::SMSG_PROPOSE_LEVEL_GRANT, &[0x01, 0x2A]),
            ServerPacket::ProposeLevelGrant { granter: 0x2A }
        ));
        let mut b = 9u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"Bob\0");
        match parse(opcode::SMSG_REFER_A_FRIEND_FAILURE, &b) {
            ServerPacket::ReferAFriendFailure { error: 9, name } => assert_eq!(name, "Bob"),
            other => panic!("{}", other.name()),
        }
        let mut b = 2u32.to_le_bytes().to_vec();
        b.extend_from_slice(&133u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        match parse(opcode::SMSG_SEND_UNLEARN_SPELLS, &b) {
            ServerPacket::UnlearnSpells(s) => assert_eq!(s, vec![133, 5]),
            other => panic!("{}", other.name()),
        }
        let mut b = 0x2Au64.to_le_bytes().to_vec();
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&2894u32.to_le_bytes());
        assert!(matches!(
            parse(opcode::SMSG_CROSSED_INEBRIATION_THRESHOLD, &b),
            ServerPacket::InebriationThreshold(InebriationThreshold {
                player: 0x2A,
                threshold: 2,
                item: 2894
            })
        ));
        assert!(matches!(
            parse(opcode::SMSG_LEARNED_DANCE_MOVES, &[0; 8]),
            ServerPacket::LearnedDanceMoves(0)
        ));
        assert_eq!(accept_level_grant(0x2A), vec![0x01, 0x2A]);
    }

    #[test]
    fn the_639_notices_decode_their_12340_bodies() {
        let mut b = 2u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"Welcome\0to the realm\0");
        match parse(opcode::SMSG_MOTD, &b) {
            ServerPacket::Motd(lines) => assert_eq!(lines, ["Welcome", "to the realm"]),
            other => panic!("{}", other.name()),
        }
        let mut b = b"world\0".to_vec();
        b.push(0x18);
        b.extend_from_slice(&42u32.to_le_bytes());
        assert!(matches!(
            parse(opcode::SMSG_CHANNEL_MEMBER_COUNT, &b),
            ServerPacket::ChannelMemberCount {
                flags: 0x18,
                count: 42,
                ..
            }
        ));
        match parse(opcode::SMSG_WHOIS, b"Account: X\0") {
            ServerPacket::Whois(text) => assert_eq!(text, "Account: X"),
            other => panic!("{}", other.name()),
        }
        let mut b = 0xFFFF_FFFFu32.to_le_bytes().to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"01/01/01\0");
        match parse(opcode::SMSG_REALM_SPLIT, &b) {
            ServerPacket::RealmSplit(s) => {
                assert_eq!((s.client_state, s.split_state), (0xFFFF_FFFF, 1));
                assert_eq!(s.date, "01/01/01");
            }
            other => panic!("{}", other.name()),
        }
        // The handler's empty-body arm fires zeros.
        assert!(matches!(
            parse(opcode::SMSG_REALM_SPLIT, &[]),
            ServerPacket::RealmSplit(RealmSplit {
                client_state: 0,
                split_state: 0,
                ..
            })
        ));
        let mut b = b"Bob\0".to_vec();
        b.extend_from_slice(&0x2Au64.to_le_bytes());
        b.extend_from_slice(&457u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        match parse(opcode::SMSG_SERVER_FIRST_ACHIEVEMENT, &b) {
            ServerPacket::ServerFirstAchievement(a) => assert_eq!(
                a,
                ServerFirstAchievement {
                    name: "Bob".into(),
                    guid: 0x2A,
                    achievement: 457,
                    linked: true
                }
            ),
            other => panic!("{}", other.name()),
        }
        assert_eq!(realm_split(0xFFFF_FFFF), vec![0xFF; 4]);
    }
}
