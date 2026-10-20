//! The arena-team family: the team cache's record, the roster and stats the PvP window shows,
//! the captain's sends and the server's results and broadcasts. A team is addressed by its id,
//! which the player's `PLAYER_FIELD_ARENA_TEAM_INFO_1_1` slots carry.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_f32_le, read_u32_le, read_u64_le, read_u8};

/// `SMSG_ARENA_TEAM_QUERY_RESPONSE`: the cache record (`ArenaTeamCache::Unpack`, `0x98d220`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArenaTeamRecord {
    pub team_id: u32,
    pub name: String,
    /// The team size, 2, 3 or 5.
    pub team_type: u32,
    /// `0xAARRGGBB` colours and the two style indices of the banner.
    pub background_color: u32,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
}

pub(super) fn read_arena_team_query_response(r: &mut impl Read) -> io::Result<ArenaTeamRecord> {
    Ok(ArenaTeamRecord {
        team_id: read_u32_le(r)?,
        name: read_cstring(r)?,
        team_type: read_u32_le(r)?,
        background_color: read_u32_le(r)?,
        emblem_style: read_u32_le(r)?,
        emblem_color: read_u32_le(r)?,
        border_style: read_u32_le(r)?,
        border_color: read_u32_le(r)?,
    })
}

/// One roster member (`ArenaTeamMember`, `0x68` bytes in the client).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ArenaTeamMember {
    pub guid: u64,
    pub online: bool,
    pub name: String,
    /// 0 the captain, 1 a member.
    pub rank: u32,
    pub level: u8,
    pub class: u8,
    pub week_games: u32,
    pub week_wins: u32,
    pub season_games: u32,
    pub season_wins: u32,
    pub personal_rating: u32,
    /// The matchmaking rating before and after the last match, sent on request alone.
    pub matchmaking: Option<(f32, f32)>,
}

/// `SMSG_ARENA_TEAM_ROSTER` (handler `0x5a3e10`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ArenaTeamRoster {
    pub team_id: u32,
    pub team_type: u32,
    pub members: Vec<ArenaTeamMember>,
}

pub(super) fn read_arena_team_roster(r: &mut impl Read) -> io::Result<ArenaTeamRoster> {
    let team_id = read_u32_le(r)?;
    let has_ratings = read_u8(r)? != 0;
    let count = read_u32_le(r)?;
    let team_type = read_u32_le(r)?;
    let mut members = Vec::with_capacity(capacity_hint(count, 10));
    for _ in 0..count {
        let mut m = ArenaTeamMember {
            guid: read_u64_le(r)?,
            online: read_u8(r)? != 0,
            name: read_cstring(r)?,
            rank: read_u32_le(r)?,
            level: read_u8(r)?,
            class: read_u8(r)?,
            week_games: read_u32_le(r)?,
            week_wins: read_u32_le(r)?,
            season_games: read_u32_le(r)?,
            season_wins: read_u32_le(r)?,
            personal_rating: read_u32_le(r)?,
            matchmaking: None,
        };
        if has_ratings {
            m.matchmaking = Some((read_f32_le(r)?, read_f32_le(r)?));
        }
        members.push(m);
    }
    Ok(ArenaTeamRoster {
        team_id,
        team_type,
        members,
    })
}

/// `SMSG_ARENA_TEAM_STATS` (handler `0x5a2d50`), dropped for a team the player is not on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ArenaTeamStats {
    pub team_id: u32,
    pub rating: u32,
    pub week_games: u32,
    pub week_wins: u32,
    pub season_games: u32,
    pub season_wins: u32,
    pub rank: u32,
}

pub(super) fn read_arena_team_stats(r: &mut impl Read) -> io::Result<ArenaTeamStats> {
    Ok(ArenaTeamStats {
        team_id: read_u32_le(r)?,
        rating: read_u32_le(r)?,
        week_games: read_u32_le(r)?,
        week_wins: read_u32_le(r)?,
        season_games: read_u32_le(r)?,
        season_wins: read_u32_le(r)?,
        rank: read_u32_le(r)?,
    })
}

/// `SMSG_ARENA_TEAM_COMMAND_RESULT` (handler `0x6ccb20`): a command's verdict, one line.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArenaTeamCommandResult {
    /// 0 create, 1 invite, 3 quit, 14 founder.
    pub command: u32,
    pub team: String,
    pub player: String,
    /// 0 success, else the error the handler's switch names.
    pub error: u32,
}

pub(super) fn read_arena_team_command_result(
    r: &mut impl Read,
) -> io::Result<ArenaTeamCommandResult> {
    Ok(ArenaTeamCommandResult {
        command: read_u32_le(r)?,
        team: read_cstring(r)?,
        player: read_cstring(r)?,
        error: read_u32_le(r)?,
    })
}

/// `SMSG_ARENA_TEAM_EVENT` (handler `0x6cc980`): a membership change broadcast to the team.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArenaTeamEvent {
    /// 3 join, 4 leave, 5 remove, 6 leader is, 7 leader changed, 8 disbanded.
    pub event: u8,
    /// Up to three strings; the handler reads `count` of them into a three-row buffer.
    pub strings: Vec<String>,
    /// The joining or leaving player, read for events 3 and 4 alone.
    pub guid: Option<u64>,
}

pub(super) fn read_arena_team_event(r: &mut impl Read) -> io::Result<ArenaTeamEvent> {
    let event = read_u8(r)?;
    let count = read_u8(r)?;
    let mut strings = Vec::with_capacity(usize::from(count.min(3)));
    for _ in 0..count {
        strings.push(read_cstring(r)?);
    }
    let guid = if matches!(event, 3 | 4) {
        Some(read_u64_le(r)?)
    } else {
        None
    };
    Ok(ArenaTeamEvent {
        event,
        strings,
        guid,
    })
}

/// `SMSG_ARENA_TEAM_INVITE` (handler `0x6cc910`): the inviter's name and the team's.
pub(super) fn read_arena_team_invite(r: &mut impl Read) -> io::Result<(String, String)> {
    Ok((read_cstring(r)?, read_cstring(r)?))
}

/// `CMSG_ARENA_TEAM_INVITE`, `_REMOVE` and `_LEADER` (`0x515cc0`, `0x515eb0`, `0x515ff0`):
/// `u32 teamId`, then the player's name.
pub fn arena_team_named(team_id: u32, name: &str) -> Vec<u8> {
    let mut body = team_id.to_le_bytes().to_vec();
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_reads_the_optional_ratings() {
        let mut b = 9u32.to_le_bytes().to_vec();
        b.push(1);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&0x77u64.to_le_bytes());
        b.push(1);
        b.extend_from_slice(b"Claude\0");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[80, 3]);
        for v in [4u32, 2, 10, 6, 1520] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1500.0f32.to_le_bytes());
        b.extend_from_slice(&1512.5f32.to_le_bytes());
        let roster = read_arena_team_roster(&mut b.as_slice()).unwrap();
        assert_eq!((roster.team_id, roster.team_type), (9, 2));
        let m = &roster.members[0];
        assert_eq!(
            (m.name.as_str(), m.rank, m.level, m.class),
            ("Claude", 0, 80, 3)
        );
        assert_eq!(
            (m.week_games, m.season_wins, m.personal_rating),
            (4, 6, 1520)
        );
        assert_eq!(m.matchmaking, Some((1500.0, 1512.5)));
    }

    #[test]
    fn an_event_reads_its_guid_for_join_and_leave_only() {
        let mut b = vec![3u8, 2];
        b.extend_from_slice(b"Claude\0Team\0");
        b.extend_from_slice(&0x42u64.to_le_bytes());
        let e = read_arena_team_event(&mut b.as_slice()).unwrap();
        assert_eq!((e.event, e.strings.len(), e.guid), (3, 2, Some(0x42)));
        let b = [8u8, 1, b'T', 0];
        let e = read_arena_team_event(&mut b.as_slice()).unwrap();
        assert_eq!((e.event, e.guid), (8, None));
    }

    #[test]
    fn the_record_and_the_named_sends() {
        let mut b = 9u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"Probes\0");
        for v in [2u32, 0xFF10_2030, 7, 0xFF00_00FF, 3, 0xFFFF_FFFF] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let t = read_arena_team_query_response(&mut b.as_slice()).unwrap();
        assert_eq!((t.team_id, t.name.as_str(), t.team_type), (9, "Probes", 2));
        assert_eq!((t.emblem_style, t.border_style), (7, 3));
        assert_eq!(arena_team_named(9, "Ab"), vec![9, 0, 0, 0, b'A', b'b', 0]);
    }
}
