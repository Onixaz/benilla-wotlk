//! Small server notices: the light override, the UI clock, arena and refer-a-friend errors, the
//! level grant offer, the unlearn list, the inebriation threshold, the dance moves and dance cache,
//! the message of the day, a channel's member count, the whois and rwhois answers, the realm split,
//! realm-first awards, the bot check and the one-line [`WorldNotice`]s.

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

/// The notices that read a fixed body and show at most one line, or flip one CVar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldNotice {
    /// `SMSG_PLAYERBINDERROR` (`0x6e3492`): no body.
    PlayerBindError,
    /// `SMSG_REMOVED_FROM_PVP_QUEUE` (`0x52656c`): the reason.
    RemovedFromPvpQueue { reason: u32 },
    /// `SMSG_INVALID_PROMOTION_CODE` (`0x52654a`): no body.
    InvalidPromotionCode,
    /// `SMSG_OFFER_PETITION_ERROR` (`0x5cea0d`): the player offered to, a full guid.
    OfferPetitionError { player: u64 },
    /// `SMSG_ARENA_TEAM_CHANGE_FAILED_QUEUED` (`0x6e40aa`): nonzero shows the line.
    ArenaTeamChangeFailedQueued { failed: u32 },
    /// `SMSG_BATTLEGROUND_INFO_THROTTLED` (`0x6cbea0`): no body.
    BattlegroundInfoThrottled,
    /// `SMSG_PLAY_TIME_WARNING` (`0x6cd280`, `0x6cd28b`): the warning flags and the seconds left,
    /// signed as the handler divides them.
    PlayTimeWarning { flags: u32, seconds: i32 },
    /// `SMSG_REPORT_PVP_AFK_RESULT` (`0x6e3fbb`): the result; two bytes and a guid follow, unused.
    ReportPvpAfkResult { result: u8 },
}

impl WorldNotice {
    pub fn opcode_name(self) -> &'static str {
        match self {
            WorldNotice::PlayerBindError => "SMSG_PLAYERBINDERROR",
            WorldNotice::RemovedFromPvpQueue { .. } => "SMSG_REMOVED_FROM_PVP_QUEUE",
            WorldNotice::InvalidPromotionCode => "SMSG_INVALID_PROMOTION_CODE",
            WorldNotice::OfferPetitionError { .. } => "SMSG_OFFER_PETITION_ERROR",
            WorldNotice::ArenaTeamChangeFailedQueued { .. } => {
                "SMSG_ARENA_TEAM_CHANGE_FAILED_QUEUED"
            }
            WorldNotice::BattlegroundInfoThrottled => "SMSG_BATTLEGROUND_INFO_THROTTLED",
            WorldNotice::PlayTimeWarning { .. } => "SMSG_PLAY_TIME_WARNING",
            WorldNotice::ReportPvpAfkResult { .. } => "SMSG_REPORT_PVP_AFK_RESULT",
        }
    }
}

/// Read one [`WorldNotice`] body; a byte the server appends is left unread.
pub(super) fn read_world_notice(opcode: u16, r: &mut &[u8]) -> io::Result<WorldNotice> {
    use super::opcode as op;
    Ok(match opcode {
        op::SMSG_PLAYERBINDERROR => WorldNotice::PlayerBindError,
        op::SMSG_REMOVED_FROM_PVP_QUEUE => WorldNotice::RemovedFromPvpQueue {
            reason: read_u32_le(r)?,
        },
        op::SMSG_INVALID_PROMOTION_CODE => WorldNotice::InvalidPromotionCode,
        op::SMSG_OFFER_PETITION_ERROR => WorldNotice::OfferPetitionError {
            player: read_u64_le(r)?,
        },
        op::SMSG_ARENA_TEAM_CHANGE_FAILED_QUEUED => WorldNotice::ArenaTeamChangeFailedQueued {
            failed: read_u32_le(r)?,
        },
        op::SMSG_BATTLEGROUND_INFO_THROTTLED => WorldNotice::BattlegroundInfoThrottled,
        op::SMSG_PLAY_TIME_WARNING => WorldNotice::PlayTimeWarning {
            flags: read_u32_le(r)?,
            seconds: read_u32_le(r)? as i32,
        },
        op::SMSG_REPORT_PVP_AFK_RESULT => {
            let result = read_u8(r)?;
            let (_, _, _) = (read_u8(r)?, read_u8(r)?, read_u64_le(r)?);
            WorldNotice::ReportPvpAfkResult { result }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a world notice",
            ))
        }
    })
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

/// One dance-cache record (`0x98d300`): a packed guid, a name, a run of `u32`s and a `u32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanceRecord {
    pub guid: u64,
    pub name: String,
    pub data: Vec<u32>,
    pub tail: u32,
}

/// `SMSG_DANCE_QUERY_RESPONSE` (`0x635ab0`): the id, then the record unless its top bit marks a
/// miss (`0x98d312`).
pub(super) fn read_dance_query_response(r: &mut &[u8]) -> io::Result<(u32, Option<DanceRecord>)> {
    let id = read_u32_le(r)?;
    if id & 0x8000_0000 != 0 {
        return Ok((id & 0x7FFF_FFFF, None));
    }
    let guid = read_packed_guid(r)?;
    let name = read_cstring(r)?;
    let count = read_u32_le(r)?;
    let data = (0..count)
        .map(|_| read_u32_le(r))
        .collect::<io::Result<_>>()?;
    let tail = read_u32_le(r)?;
    Ok((
        id,
        Some(DanceRecord {
            guid,
            name,
            data,
            tail,
        }),
    ))
}

/// `SMSG_RWHOIS` (`0x6b32c0`): a signed count; -1 failed, 0 found nothing, a negative other count
/// shows nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RWhois {
    Failed,
    NotFound,
    Accounts(Vec<RWhoisAccount>),
}

/// One `SMSG_RWHOIS` record: the account, its lines, and the line printed in the other colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RWhoisAccount {
    pub account: String,
    pub lines: Vec<String>,
    pub highlight: u32,
}

pub(super) fn read_rwhois(r: &mut &[u8]) -> io::Result<RWhois> {
    let count = read_u32_le(r)? as i32;
    Ok(match count {
        -1 => RWhois::Failed,
        0 => RWhois::NotFound,
        n => RWhois::Accounts(
            (0..n.max(0))
                .map(|_| {
                    let account = read_cstring(r)?;
                    let n = read_u32_le(r)? as i32;
                    let highlight = read_u32_le(r)?;
                    let lines = (0..n.max(0))
                        .map(|_| read_cstring(r))
                        .collect::<io::Result<_>>()?;
                    Ok(RWhoisAccount {
                        account,
                        lines,
                        highlight,
                    })
                })
                .collect::<io::Result<_>>()?,
        ),
    })
}

/// `SMSG_CHECK_FOR_BOTS` (`0x6b9682`): the 16-byte challenge.
pub(super) fn read_check_for_bots(r: &mut &[u8]) -> io::Result<[u8; 16]> {
    let mut key = [0u8; 16];
    io::Read::read_exact(r, &mut key)?;
    Ok(key)
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

    #[test]
    fn the_dance_cache_packets_decode_their_12340_bodies() {
        let mut b = 9u32.to_le_bytes().to_vec();
        b.extend_from_slice(&[0x01, 0x2A]); // packed guid 0x2A
        b.extend_from_slice(b"Jig\0");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes());
        match parse(opcode::SMSG_DANCE_QUERY_RESPONSE, &b) {
            ServerPacket::DanceQueryResponse { id, record } => {
                assert_eq!(id, 9);
                assert_eq!(
                    record,
                    Some(DanceRecord {
                        guid: 0x2A,
                        name: "Jig".into(),
                        data: vec![5, 6],
                        tail: 7
                    })
                );
            }
            other => panic!("{}", other.name()),
        }
        assert!(matches!(
            parse(
                opcode::SMSG_DANCE_QUERY_RESPONSE,
                &(9u32 | 0x8000_0000).to_le_bytes()
            ),
            ServerPacket::DanceQueryResponse {
                id: 9,
                record: None
            }
        ));
        assert!(matches!(
            parse(opcode::SMSG_INVALIDATE_DANCE, &9u32.to_le_bytes()),
            ServerPacket::InvalidateDance { id: 9 }
        ));
    }

    #[test]
    fn rwhois_reads_its_count_arms_and_records() {
        assert!(matches!(
            parse(opcode::SMSG_RWHOIS, &(-1i32).to_le_bytes()),
            ServerPacket::RWhois(RWhois::Failed)
        ));
        assert!(matches!(
            parse(opcode::SMSG_RWHOIS, &0u32.to_le_bytes()),
            ServerPacket::RWhois(RWhois::NotFound)
        ));
        match parse(opcode::SMSG_RWHOIS, &(-2i32).to_le_bytes()) {
            ServerPacket::RWhois(RWhois::Accounts(a)) => assert!(a.is_empty()),
            other => panic!("{}", other.name()),
        }
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"ACC\0");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"one\0two\0");
        match parse(opcode::SMSG_RWHOIS, &b) {
            ServerPacket::RWhois(RWhois::Accounts(a)) => assert_eq!(
                a,
                [RWhoisAccount {
                    account: "ACC".into(),
                    lines: vec!["one".into(), "two".into()],
                    highlight: 1
                }]
            ),
            other => panic!("{}", other.name()),
        }
    }

    #[test]
    fn the_bot_check_reads_sixteen_bytes() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        assert!(matches!(
            parse(opcode::SMSG_CHECK_FOR_BOTS, &key),
            ServerPacket::CheckForBots { key: k } if k == key
        ));
        assert!(crate::messages::parse_server(opcode::SMSG_CHECK_FOR_BOTS, &key[..15]).is_err());
    }

    /// Each one-line notice reads its 12340 body, and a byte past an empty body is ignored.
    #[test]
    fn the_world_notices_decode_their_12340_bodies() {
        let notice = |op, body: &[u8]| match parse(op, body) {
            ServerPacket::Notice(n) => n,
            other => panic!("{}", other.name()),
        };
        use WorldNotice as N;
        assert_eq!(
            notice(opcode::SMSG_PLAYERBINDERROR, &[]),
            N::PlayerBindError
        );
        assert_eq!(
            notice(opcode::SMSG_PLAYERBINDERROR, &[7]),
            N::PlayerBindError
        );
        assert_eq!(
            notice(opcode::SMSG_REMOVED_FROM_PVP_QUEUE, &2u32.to_le_bytes()),
            N::RemovedFromPvpQueue { reason: 2 }
        );
        assert_eq!(
            notice(opcode::SMSG_INVALID_PROMOTION_CODE, &[]),
            N::InvalidPromotionCode
        );
        assert_eq!(
            notice(opcode::SMSG_OFFER_PETITION_ERROR, &0x2Au64.to_le_bytes()),
            N::OfferPetitionError { player: 0x2A }
        );
        assert_eq!(
            notice(
                opcode::SMSG_ARENA_TEAM_CHANGE_FAILED_QUEUED,
                &1u32.to_le_bytes()
            ),
            N::ArenaTeamChangeFailedQueued { failed: 1 }
        );
        assert_eq!(
            notice(opcode::SMSG_BATTLEGROUND_INFO_THROTTLED, &[]),
            N::BattlegroundInfoThrottled
        );
        let mut b = 0x1000u32.to_le_bytes().to_vec();
        b.extend_from_slice(&3_900u32.to_le_bytes());
        assert_eq!(
            notice(opcode::SMSG_PLAY_TIME_WARNING, &b),
            N::PlayTimeWarning {
                flags: 0x1000,
                seconds: 3_900
            }
        );
        let mut b = vec![5u8, 1, 2];
        b.extend_from_slice(&0x2Au64.to_le_bytes());
        assert_eq!(
            notice(opcode::SMSG_REPORT_PVP_AFK_RESULT, &b),
            N::ReportPvpAfkResult { result: 5 }
        );
        assert!(parse_server(opcode::SMSG_REPORT_PVP_AFK_RESULT, &[5]).is_err());
    }
}
