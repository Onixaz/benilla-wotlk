//! The honor wire messages: the inspect request/reply pairs and the honor-gain credit.
//! Durable honor state rides the player descriptor; these carry what it cannot, another player's
//! stats and the moment a kill pays out.

use std::io;

use crate::wire::{read_i32_le, read_u16_le, read_u32_le, read_u64_le, read_u8};

/// Another player's honor stats, the `MSG_INSPECT_HONOR_STATS` reply (handler `0x5e7a50`): the
/// target's 3.3.5a honor fields, read into `0xc24208`..`0xc24218`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectHonorStats {
    /// Echoed from the request. There is no error reply: a missing, distant or hostile target gets
    /// no answer at all.
    pub player_guid: u64,
    /// `GetInspectHonorData`'s sixth return, the lifetime highest rank.
    pub highest_rank: u8,
    /// `PLAYER_FIELD_KILLS`, low half then high: today's and yesterday's honorable kills.
    pub today_hk: u16,
    pub yesterday_hk: u16,
    /// `PLAYER_FIELD_TODAY_CONTRIBUTION` and `_YESTERDAY_CONTRIBUTION`.
    pub today_honor: u32,
    pub yesterday_honor: u32,
    /// `PLAYER_FIELD_LIFETIME_HONORABLE_KILLS`.
    pub lifetime_hk: u32,
}

/// Read the `MSG_INSPECT_HONOR_STATS` reply. Our 8-byte request shares the opcode, but only
/// server packets reach this reader.
pub(super) fn read_inspect_honor_stats(r: &mut &[u8]) -> io::Result<InspectHonorStats> {
    Ok(InspectHonorStats {
        player_guid: read_u64_le(r)?,
        highest_rank: read_u8(r)?,
        today_hk: read_u16_le(r)?,
        yesterday_hk: read_u16_le(r)?,
        today_honor: read_u32_le(r)?,
        yesterday_honor: read_u32_le(r)?,
        lifetime_hk: read_u32_le(r)?,
    })
}

/// One of another player's arena teams, the `MSG_INSPECT_ARENA_TEAMS` reply (handler `0x5e7b00`):
/// six words into the inspect store's slot (`0xc241c0 + 0x18 * slot`), kept positionally as
/// `GetInspectArenaTeamData` reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectArenaTeam {
    /// The player inspected; the handler drops a reply for anyone but its inspect target.
    pub player_guid: u64,
    /// 0 2v2, 1 3v3, 2 5v5; the handler drops 3 and above.
    pub slot: u8,
    pub team_id: u32,
    pub rating: u32,
    /// The third to sixth words: games and wins as the server fills them, then the personal
    /// rating last (`GetInspectArenaTeamData`'s fourth to seventh returns).
    pub games: u32,
    pub wins: u32,
    pub played: u32,
    pub personal_rating: u32,
}

pub(super) fn read_inspect_arena_team(r: &mut &[u8]) -> io::Result<InspectArenaTeam> {
    Ok(InspectArenaTeam {
        player_guid: read_u64_le(r)?,
        slot: read_u8(r)?,
        team_id: read_u32_le(r)?,
        rating: read_u32_le(r)?,
        games: read_u32_le(r)?,
        wins: read_u32_le(r)?,
        played: read_u32_le(r)?,
        personal_rating: read_u32_le(r)?,
    })
}

/// Body of the `MSG_INSPECT_HONOR_STATS` request: the raw, unpacked `u64` guid (vmangos
/// `Misc.cpp:93-96`). A refused request gets no answer, and unlike `CMSG_INSPECT` it does not
/// set our selection (`MiscHandler.cpp:962-972`).
pub fn inspect_honor_stats(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `SMSG_PVP_CREDIT` (opcode 652): one honor payout, dishonorable ones included (vmangos
/// `HonorMgr.cpp:1061-1093`). The descriptor holds only totals; this is the only per-payout notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PvpCredit {
    /// Honor, truncated server-side; negative for a dishonorable kill (`HonorMgr.cpp:807`).
    pub honor: i32,
    /// Who we killed; 0 for an honor row with no source (`HonorMgr.cpp:1069-1071`), which
    /// needs a victim-less phrasing rather than a name lookup.
    pub victim_guid: u64,
    /// The victim's internal rank (0..18), indexing the `PVP_RANK_<rank>_<team>` strings; the
    /// badge's visual rank is `internal > 4 ? internal - 4 : -internal` (`HonorMgr.cpp:991`).
    /// A creature sends 19 if a racial leader, else 0; vmangos floors a player victim at 5
    /// (`HonorMgr.cpp:1078-1089`), a server fallback rather than a rule to re-implement.
    pub victim_rank: i32,
}

/// Read `SMSG_PVP_CREDIT`: `i32 honor, u64 victimGuid, i32 victimRank`.
pub(super) fn read_pvp_credit(r: &mut &[u8]) -> io::Result<PvpCredit> {
    Ok(PvpCredit {
        honor: read_i32_le(r)?,
        victim_guid: read_u64_le(r)?,
        victim_rank: read_i32_le(r)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_honor_stats_golden() {
        #[rustfmt::skip]
        let body: [u8; 25] = [
            // u64 playerGuid = 0x0000_0001_0000_2AB3
            0xB3, 0x2A, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            // u8 highestRank = 11
            0x0B,
            // u16 today's HK = 17, u16 yesterday's = 2
            0x11, 0x00, 0x02, 0x00,
            // u32 today's honor = 640, yesterday's = 8431, lifetime HK = 3907
            0x80, 0x02, 0x00, 0x00,
            0xEF, 0x20, 0x00, 0x00,
            0x43, 0x0F, 0x00, 0x00,
        ];
        let mut r = body.as_slice();
        let stats = read_inspect_honor_stats(&mut r).unwrap();
        assert!(r.is_empty(), "the whole body is consumed");
        assert_eq!(
            stats,
            InspectHonorStats {
                player_guid: 0x0000_0001_0000_2AB3,
                highest_rank: 11,
                today_hk: 17,
                yesterday_hk: 2,
                today_honor: 640,
                yesterday_honor: 8_431,
                lifetime_hk: 3_907,
            }
        );
    }

    #[test]
    fn inspect_honor_stats_truncated_body_errors() {
        let full = [0u8; 25];
        for len in [0usize, 8, 13, 24] {
            let mut r = &full[..len];
            assert!(read_inspect_honor_stats(&mut r).is_err(), "{len} bytes");
        }
        let mut r = full.as_slice();
        assert!(read_inspect_honor_stats(&mut r).is_ok());
    }

    #[test]
    fn inspect_arena_team_reads_six_words() {
        let mut b = 0x42u64.to_le_bytes().to_vec();
        b.push(1);
        for v in [9u32, 1600, 20, 12, 18, 1580] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let mut r = b.as_slice();
        let t = read_inspect_arena_team(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            (t.player_guid, t.slot, t.team_id, t.rating),
            (0x42, 1, 9, 1600)
        );
        assert_eq!(
            (t.games, t.wins, t.played, t.personal_rating),
            (20, 12, 18, 1580)
        );
    }

    #[test]
    fn inspect_honor_stats_request_golden() {
        assert_eq!(
            inspect_honor_stats(0x0000_0001_0000_2AB3),
            vec![0xB3, 0x2A, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00],
        );
        // A zero guid is encoded as-is.
        assert_eq!(inspect_honor_stats(0), vec![0u8; 8]);
    }

    #[test]
    fn pvp_credit_golden() {
        #[rustfmt::skip]
        let body: [u8; 16] = [
            // [0..4) i32 honor = 143
            0x8F, 0x00, 0x00, 0x00,
            // [4..12) u64 victimGuid = 0x0000_0001_0000_2AB3
            0xB3, 0x2A, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            // [12..16) i32 victimRank = 11 (internal rank, the PVP_RANK_11_<team> key)
            0x0B, 0x00, 0x00, 0x00,
        ];
        let mut r = body.as_slice();
        let credit = read_pvp_credit(&mut r).unwrap();
        assert!(r.is_empty(), "the whole 16-byte body is consumed");
        assert_eq!(
            credit,
            PvpCredit {
                honor: 143,
                victim_guid: 0x0000_0001_0000_2AB3,
                victim_rank: 11,
            }
        );
    }

    #[test]
    fn pvp_credit_dishonorable_kill_is_negative_honor() {
        let mut body = Vec::with_capacity(16);
        body.extend_from_slice(&(-5i32).to_le_bytes());
        body.extend_from_slice(&0x0000_0001_0000_2AB3u64.to_le_bytes());
        body.extend_from_slice(&5i32.to_le_bytes()); // floored to "Scout" server-side
        let mut r = body.as_slice();
        let credit = read_pvp_credit(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(credit.honor, -5);
        assert_eq!(credit.victim_rank, 5);
    }

    #[test]
    fn pvp_credit_zero_guid_means_no_victim() {
        let mut body = Vec::with_capacity(16);
        body.extend_from_slice(&5i32.to_le_bytes());
        body.extend_from_slice(&0u64.to_le_bytes()); // no victim
        body.extend_from_slice(&0i32.to_le_bytes()); // and so no rank
        let mut r = body.as_slice();
        let credit = read_pvp_credit(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(credit.victim_guid, 0, "zero guid = no victim");
        assert_eq!(credit.victim_rank, 0);
    }

    #[test]
    fn pvp_credit_truncated_body_errors() {
        let full = [0u8; 16];
        for len in [0usize, 4, 12, 15] {
            let mut r = &full[..len];
            assert!(
                read_pvp_credit(&mut r).is_err(),
                "a {len}-byte credit body must be rejected"
            );
        }
        let mut r = full.as_slice();
        assert!(read_pvp_credit(&mut r).is_ok());
    }
}
