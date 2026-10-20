//! Battleground and arena messages: the two queue slots, the instance list, the scoreboard, the
//! teammate positions and the world-PvP (Wintergrasp) queue. The client keeps two queue slots
//! (`CGBattlefieldInfo::m_status`, `0xbea4b8`, stride `0x40`) keyed by an opaque 64-bit queue word
//! the server minted and every later send echoes back.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_f32_le, read_u32_le, read_u64_le, read_u8};

/// A queue word's battleground type, `BattlemasterList.dbc`'s id (`(u32)queue >> 16`, `0x54aed0`).
pub fn queue_bg_type(queue: u64) -> u32 {
    (queue as u32) >> 16
}

/// A queue word's arena team size, 0 for a battleground (`(u8)queue & 0x7f`, `0x54aed8`).
pub fn queue_team_size(queue: u64) -> u32 {
    u32::from(queue as u8 & 0x7F)
}

/// The confirm state's tail: the invitation's map and its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BattlefieldInvite {
    pub map_id: u32,
    /// The queue word the invitation joined; the random battleground's names its real type.
    pub joined_queue: u64,
    /// Milliseconds to accept in, 0 for none (`[slot+0x28] = Δ ? now + Δ : 0`).
    pub expire_ms: u32,
}

/// The active state's tail: the instance the player is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BattlefieldActive {
    pub map_id: u32,
    pub joined_queue: u64,
    /// Milliseconds until the instance shuts, 0 while it runs (`[0xbea574]`).
    pub shutdown_ms: u32,
    /// Milliseconds since it started (`[0xbea578] = now − Δ`).
    pub run_ms: u32,
    /// The arena side, 0 green or 1 gold (`[0xbea54a]`, `GetBattlefieldArenaFaction`).
    pub arena_faction: u8,
}

/// `SMSG_BATTLEFIELD_STATUS` (handler `0x54ae40`): one slot's state. A zero queue word clears it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldStatus {
    /// Slot 0 or 1; the handler drops any other (`0x54ae5a`).
    pub slot: u32,
    pub queue: u64,
    /// The bracket's levels (`[slot+0x1c]`/`[slot+0x20]`).
    pub min_level: u8,
    pub max_level: u8,
    pub instance_id: u32,
    /// A rated arena match (`[slot+0x38]`).
    pub rated: bool,
    /// 0 none, 1 queued, 2 confirm, 3 active.
    pub status: u32,
    /// Status 2's tail.
    pub invite: Option<BattlefieldInvite>,
    /// Status 3's tail.
    pub active: Option<BattlefieldActive>,
    /// Status 1's tail: the estimated wait and the time already waited, in ms.
    pub queued: Option<(u32, u32)>,
}

impl BattlefieldStatus {
    pub fn bg_type(&self) -> u32 {
        queue_bg_type(self.queue)
    }

    pub fn team_size(&self) -> u32 {
        queue_team_size(self.queue)
    }
}

pub(super) fn read_battlefield_status(r: &mut impl Read) -> io::Result<BattlefieldStatus> {
    let slot = read_u32_le(r)?;
    let queue = read_u64_le(r)?;
    let mut s = BattlefieldStatus {
        slot,
        queue,
        ..Default::default()
    };
    if queue == 0 {
        return Ok(s);
    }
    s.min_level = read_u8(r)?;
    s.max_level = read_u8(r)?;
    s.instance_id = read_u32_le(r)?;
    s.rated = read_u8(r)? != 0;
    s.status = read_u32_le(r)?;
    match s.status {
        1 => s.queued = Some((read_u32_le(r)?, read_u32_le(r)?)),
        2 => {
            s.invite = Some(BattlefieldInvite {
                map_id: read_u32_le(r)?,
                joined_queue: read_u64_le(r)?,
                expire_ms: read_u32_le(r)?,
            })
        }
        3 => {
            s.active = Some(BattlefieldActive {
                map_id: read_u32_le(r)?,
                joined_queue: read_u64_le(r)?,
                shutdown_ms: read_u32_le(r)?,
                run_ms: read_u32_le(r)?,
                arena_faction: read_u8(r)?,
            })
        }
        _ => {}
    }
    Ok(s)
}

/// The honor and arena-point reward a battleground pays, win and loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldReward {
    /// Today's first win is already taken (`GetHolidayBGHonorCurrencyBonuses`' first return).
    pub has_win: bool,
    pub win_honor: u32,
    pub win_arena: u32,
    pub loss_honor: u32,
}

fn read_reward(r: &mut impl Read) -> io::Result<BattlefieldReward> {
    Ok(BattlefieldReward {
        has_win: read_u8(r)? != 0,
        win_honor: read_u32_le(r)?,
        win_arena: read_u32_le(r)?,
        loss_honor: read_u32_le(r)?,
    })
}

/// `SMSG_BATTLEFIELD_LIST` (handler `0x54e390`): one battleground type's instances and rewards.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldList {
    /// 0 for a list asked from the PvP window, else the battlemaster clicked.
    pub battlemaster: u64,
    /// The list answers the PvP window's `RequestBattlegroundInstanceInfo`.
    pub from_anywhere: bool,
    /// `BattlemasterList.dbc`'s id (`[0xbea53c]`).
    pub bg_type: u32,
    pub min_level: u8,
    pub max_level: u8,
    /// The holiday battleground's reward.
    pub holiday: BattlefieldReward,
    /// The random battleground's reward, present for the random type alone.
    pub random: Option<BattlefieldReward>,
    /// The instance ids in wire order; wire index 0 is instance 1 in Lua.
    pub instances: Vec<u32>,
}

pub(super) fn read_battlefield_list(r: &mut impl Read) -> io::Result<BattlefieldList> {
    let battlemaster = read_u64_le(r)?;
    let from_anywhere = read_u8(r)? != 0;
    let bg_type = read_u32_le(r)?;
    let min_level = read_u8(r)?;
    let max_level = read_u8(r)?;
    let holiday = read_reward(r)?;
    let random = if read_u8(r)? != 0 {
        Some(read_reward(r)?)
    } else {
        None
    };
    let count = read_u32_le(r)?;
    let mut instances = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        instances.push(read_u32_le(r)?);
    }
    Ok(BattlefieldList {
        battlemaster,
        from_anywhere,
        bg_type,
        min_level,
        max_level,
        holiday,
        random,
        instances,
    })
}

/// One `MSG_PVP_LOG_DATA` scoreboard row (handler `0x54d280`), a `0x48`-byte block in the client.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PvpLogRow {
    pub guid: u64,
    pub killing_blows: u32,
    /// The arena team byte; a battleground row's faction is the client's own derivation.
    pub arena_team: Option<u8>,
    /// Battleground rows only; an arena row reads 0 for all three.
    pub honorable_kills: u32,
    pub deaths: u32,
    pub bonus_honor: u32,
    pub damage_done: u32,
    pub healing_done: u32,
    /// The extra-stat dwords; the client keeps eight and reads past the rest.
    pub stats: Vec<u32>,
}

/// One arena team's line on the scoreboard.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PvpLogTeam {
    pub name: String,
    /// The rating before the match, after it, and the team's matchmaking rating.
    pub rating: u32,
    pub new_rating: u32,
    pub matchmaking: u32,
}

/// `MSG_PVP_LOG_DATA` inbound: the whole scoreboard, rows in wire order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PvpLogData {
    /// Both arena teams, present on an arena's scoreboard alone.
    pub arena_teams: Option<[PvpLogTeam; 2]>,
    /// The match has ended.
    pub ended: bool,
    /// Present only when ended: the winning side (`GetBattlefieldWinner`).
    pub winner: Option<u8>,
    pub rows: Vec<PvpLogRow>,
}

/// Deviation: every row is kept, because the reference takes the count unclamped into its 80
/// row blocks and a larger one overruns them.
pub(super) fn read_pvp_log_data(r: &mut impl Read) -> io::Result<PvpLogData> {
    let arena = read_u8(r)? != 0;
    let arena_teams = if arena {
        let mut teams: [PvpLogTeam; 2] = Default::default();
        for t in &mut teams {
            t.rating = read_u32_le(r)?;
            t.new_rating = read_u32_le(r)?;
            t.matchmaking = read_u32_le(r)?;
        }
        for t in &mut teams {
            t.name = read_cstring(r)?;
        }
        Some(teams)
    } else {
        None
    };
    let ended = read_u8(r)? != 0;
    let winner = if ended { Some(read_u8(r)?) } else { None };
    let count = read_u32_le(r)?;
    let mut rows = Vec::with_capacity(capacity_hint(count, 80));
    for _ in 0..count {
        let mut row = PvpLogRow {
            guid: read_u64_le(r)?,
            killing_blows: read_u32_le(r)?,
            ..Default::default()
        };
        if arena {
            row.arena_team = Some(read_u8(r)?);
        } else {
            row.honorable_kills = read_u32_le(r)?;
            row.deaths = read_u32_le(r)?;
            row.bonus_honor = read_u32_le(r)?;
        }
        row.damage_done = read_u32_le(r)?;
        row.healing_done = read_u32_le(r)?;
        let stat_count = read_u32_le(r)?;
        row.stats = Vec::with_capacity(capacity_hint(stat_count, 8));
        for i in 0..stat_count {
            let v = read_u32_le(r)?;
            if i < 8 {
                row.stats.push(v);
            }
        }
        rows.push(row);
    }
    Ok(PvpLogData {
        arena_teams,
        ended,
        winner,
        rows,
    })
}

/// One position from `MSG_BATTLEGROUND_PLAYER_POSITIONS`, in raw world coordinates; the client
/// prefers a live object's own position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BattlefieldPosition {
    pub guid: u64,
    pub x: f32,
    pub y: f32,
}

/// `MSG_BATTLEGROUND_PLAYER_POSITIONS` (handler `0x54b3f0`): teammates outside our group, then
/// up to two flag carriers.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldPositions {
    pub players: Vec<BattlefieldPosition>,
    /// The carriers in wire order; the client fills its two slots from the last down.
    pub carriers: Vec<BattlefieldPosition>,
}

/// The client's 40-entry store. Deviation: a larger count keeps the first 40, where the
/// reference's handler writes past the store.
pub const BATTLEFIELD_POSITIONS_MAX: usize = 40;

fn read_position(r: &mut impl Read) -> io::Result<BattlefieldPosition> {
    Ok(BattlefieldPosition {
        guid: read_u64_le(r)?,
        x: read_f32_le(r)?,
        y: read_f32_le(r)?,
    })
}

/// Deviation: past two carriers the rest are dropped, where the reference writes past its pair.
pub(super) fn read_battlefield_positions(r: &mut impl Read) -> io::Result<BattlefieldPositions> {
    let count = read_u32_le(r)?;
    let mut players = Vec::with_capacity(capacity_hint(count, BATTLEFIELD_POSITIONS_MAX));
    for i in 0..count {
        let p = read_position(r)?;
        if (i as usize) < BATTLEFIELD_POSITIONS_MAX {
            players.push(p);
        }
    }
    let flags = read_u32_le(r)? as i32;
    let mut carriers = Vec::new();
    for i in 0..flags.max(0) {
        let p = read_position(r)?;
        if i < 2 {
            carriers.push(p);
        }
    }
    Ok(BattlefieldPositions { players, carriers })
}

/// `SMSG_GROUP_JOINED_BATTLEGROUND` (handler `0x54b1c0`): a join's verdict, negative an error,
/// else the `BattlemasterList.dbc` id joined. The two timed-out codes carry the player at fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupJoinedBattleground {
    pub result: i32,
    pub player: Option<u64>,
}

/// `ERR_BATTLEGROUND_JOIN_TIMED_OUT` and `_FAILED`, the two codes followed by a guid.
const JOIN_RESULTS_WITH_GUID: [i32; 2] = [-11, -12];

pub(super) fn read_group_joined_battleground(
    r: &mut impl Read,
) -> io::Result<GroupJoinedBattleground> {
    let result = read_u32_le(r)? as i32;
    let player = if JOIN_RESULTS_WITH_GUID.contains(&result) {
        Some(read_u64_le(r)?)
    } else {
        None
    };
    Ok(GroupJoinedBattleground { result, player })
}

/// The world-PvP battle-manager notices (Wintergrasp), each fired as its own event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BattlefieldMgrNotice {
    /// `SMSG_BATTLEFIELD_MGR_ENTRY_INVITE` (`0x54b610`): an invitation, `expire` on the UI clock.
    EntryInvite {
        queue_id: u32,
        area_id: u32,
        expire: u32,
    },
    /// `SMSG_BATTLEFIELD_MGR_ENTERED` (`0x5498c0`).
    Entered {
        queue_id: u32,
        invited: bool,
        on_offense: bool,
        cleared_afk: bool,
    },
    /// `SMSG_BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE` (`0x54b680`).
    QueueRequestResponse {
        queue_id: u32,
        area_id: u32,
        accepted: bool,
        logging_in: bool,
        warmup: u8,
    },
    /// `SMSG_BATTLEFIELD_MGR_EJECT_PENDING` (`0x54b750`).
    EjectPending { queue_id: u32, remote: bool },
    /// `SMSG_BATTLEFIELD_MGR_EJECTED` (`0x5499c0`).
    Ejected {
        queue_id: u32,
        reason: u8,
        battle_status: u8,
        relocated: bool,
    },
    /// `SMSG_BATTLEFIELD_MGR_QUEUE_INVITE` (`0x54b7a0`).
    QueueInvite { queue_id: u32, warmup: u8 },
    /// `SMSG_BATTLEFIELD_MGR_STATE_CHANGE` (`0x54b7f0`).
    StateChange { old: u32, new: u32 },
}

pub(super) fn read_bf_mgr_entry_invite(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::EntryInvite {
        queue_id: read_u32_le(r)?,
        area_id: read_u32_le(r)?,
        expire: read_u32_le(r)?,
    })
}

pub(super) fn read_bf_mgr_entered(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::Entered {
        queue_id: read_u32_le(r)?,
        invited: read_u8(r)? != 0,
        on_offense: read_u8(r)? != 0,
        cleared_afk: read_u8(r)? != 0,
    })
}

pub(super) fn read_bf_mgr_queue_request_response(
    r: &mut impl Read,
) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::QueueRequestResponse {
        queue_id: read_u32_le(r)?,
        area_id: read_u32_le(r)?,
        accepted: read_u8(r)? != 0,
        logging_in: read_u8(r)? != 0,
        warmup: read_u8(r)?,
    })
}

pub(super) fn read_bf_mgr_eject_pending(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::EjectPending {
        queue_id: read_u32_le(r)?,
        remote: read_u8(r)? != 0,
    })
}

pub(super) fn read_bf_mgr_ejected(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::Ejected {
        queue_id: read_u32_le(r)?,
        reason: read_u8(r)?,
        battle_status: read_u8(r)?,
        relocated: read_u8(r)? != 0,
    })
}

pub(super) fn read_bf_mgr_queue_invite(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::QueueInvite {
        queue_id: read_u32_le(r)?,
        warmup: read_u8(r)?,
    })
}

pub(super) fn read_bf_mgr_state_change(r: &mut impl Read) -> io::Result<BattlefieldMgrNotice> {
    Ok(BattlefieldMgrNotice::StateChange {
        old: read_u32_le(r)?,
        new: read_u32_le(r)?,
    })
}

/// `CMSG_BATTLEFIELD_PORT` (`0x54da10`): the slot's queue word, then a one-byte accept.
pub fn battlefield_port(queue: u64, accept: bool) -> Vec<u8> {
    let mut body = queue.to_le_bytes().to_vec();
    body.push(u8::from(accept));
    body
}

/// `CMSG_LEAVE_BATTLEFIELD` (`0x54c250`): the active slot's queue word, 0 with none active.
pub fn leave_battlefield(queue: u64) -> Vec<u8> {
    queue.to_le_bytes().to_vec()
}

/// `CMSG_BATTLEFIELD_LIST` (`0x54e5b0`): a battleground type, the PvP window's 1, and the
/// experience-off byte (`PLAYER_FLAGS` bit 25 below the level cap, else 0).
pub fn battlefield_list(bg_type: u32, xp_disabled: bool) -> Vec<u8> {
    let mut body = bg_type.to_le_bytes().to_vec();
    body.push(1);
    body.push(u8::from(xp_disabled));
    body
}

/// `CMSG_BATTLEMASTER_JOIN` (`0x54cc70`): the battlemaster (0 from the PvP window), the type,
/// the instance (0 first available) and the as-group byte.
pub fn battlemaster_join(
    battlemaster: u64,
    bg_type: u32,
    instance_id: u32,
    as_group: bool,
) -> Vec<u8> {
    let mut body = battlemaster.to_le_bytes().to_vec();
    body.extend_from_slice(&bg_type.to_le_bytes());
    body.extend_from_slice(&instance_id.to_le_bytes());
    body.push(u8::from(as_group));
    body
}

/// `CMSG_BATTLEMASTER_JOIN_ARENA` (`0x54cc70`): the arena master, the bracket slot (0 2v2,
/// 1 3v3, 2 5v5), the as-group byte and the rated byte.
pub fn battlemaster_join_arena(
    battlemaster: u64,
    slot: u8,
    as_group: bool,
    rated: bool,
) -> Vec<u8> {
    let mut body = battlemaster.to_le_bytes().to_vec();
    body.push(slot);
    body.push(u8::from(as_group));
    body.push(u8::from(rated));
    body
}

/// `CMSG_BATTLEFIELD_MGR_ENTRY_INVITE_RESPONSE` and `_QUEUE_INVITE_RESPONSE`: a queue and a byte.
pub fn bf_mgr_response(queue_id: u32, accept: bool) -> Vec<u8> {
    let mut body = queue_id.to_le_bytes().to_vec();
    body.push(u8::from(accept));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(slot: u32, queue: u64, status: u32) -> Vec<u8> {
        let mut b = slot.to_le_bytes().to_vec();
        b.extend_from_slice(&queue.to_le_bytes());
        b.extend_from_slice(&[10, 19]);
        b.extend_from_slice(&7u32.to_le_bytes());
        b.push(0);
        b.extend_from_slice(&status.to_le_bytes());
        b
    }

    /// AzerothCore's queue word for Warsong Gulch: arena type 0, no marker, type 2, `0x1F90`.
    const WSG_QUEUE: u64 = 0x1F90_0002_0000;

    #[test]
    fn each_status_reads_its_own_tail_and_no_further() {
        let mut b = header(1, WSG_QUEUE, 1);
        for v in [30_000u32, 5_000] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(0xEE);
        let mut r = b.as_slice();
        let s = read_battlefield_status(&mut r).unwrap();
        assert_eq!((s.slot, s.bg_type(), s.team_size()), (1, 2, 0));
        assert_eq!((s.min_level, s.max_level, s.instance_id), (10, 19, 7));
        assert_eq!(s.queued, Some((30_000, 5_000)));
        assert_eq!(r, [0xEE]);

        let mut b = header(0, WSG_QUEUE, 2);
        b.extend_from_slice(&489u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&80_000u32.to_le_bytes());
        b.push(0xEE);
        let mut r = b.as_slice();
        let s = read_battlefield_status(&mut r).unwrap();
        assert_eq!(
            s.invite,
            Some(BattlefieldInvite {
                map_id: 489,
                joined_queue: 0,
                expire_ms: 80_000
            })
        );
        assert_eq!(r, [0xEE]);

        let mut b = header(0, WSG_QUEUE, 3);
        b.extend_from_slice(&489u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        for v in [0u32, 45_000] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[1, 0xEE]);
        let mut r = b.as_slice();
        let s = read_battlefield_status(&mut r).unwrap();
        let a = s.active.unwrap();
        assert_eq!(
            (a.map_id, a.shutdown_ms, a.run_ms, a.arena_faction),
            (489, 0, 45_000, 1)
        );
        assert_eq!(r, [0xEE]);

        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend_from_slice(&0u64.to_le_bytes());
        let s = read_battlefield_status(&mut b.as_slice()).unwrap();
        assert_eq!((s.queue, s.status), (0, 0), "a zero word clears the slot");
    }

    #[test]
    fn the_queue_word_names_type_and_team_size() {
        // A 2v2: arena type 2, the arena marker 0xE, All Arenas (6).
        let q = 0x1F90_0006_0E02u64;
        assert_eq!((queue_bg_type(q), queue_team_size(q)), (6, 2));
    }

    #[test]
    fn the_list_reads_rewards_and_the_random_tail() {
        let mut b = 0x42u64.to_le_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(&32u32.to_le_bytes());
        b.extend_from_slice(&[0, 0, 1]);
        for v in [30u32, 25, 15] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(1);
        b.push(0);
        for v in [45u32, 0, 7] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&2u32.to_le_bytes());
        for id in [11u32, 3] {
            b.extend_from_slice(&id.to_le_bytes());
        }
        let l = read_battlefield_list(&mut b.as_slice()).unwrap();
        assert_eq!(
            (l.battlemaster, l.from_anywhere, l.bg_type),
            (0x42, false, 32)
        );
        assert!(l.holiday.has_win);
        assert_eq!(
            (
                l.holiday.win_honor,
                l.holiday.win_arena,
                l.holiday.loss_honor
            ),
            (30, 25, 15)
        );
        let random = l.random.unwrap();
        assert_eq!(
            (random.has_win, random.win_honor, random.loss_honor),
            (false, 45, 7)
        );
        assert_eq!(l.instances, vec![11, 3], "wire order");
    }

    #[test]
    fn an_arena_scoreboard_carries_teams_and_short_rows() {
        let mut b = vec![1u8];
        for v in [1500u32, 1516, 1490, 1500, 1484, 1510] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(b"Gold\0Green\0");
        b.extend_from_slice(&[1, 0]);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        b.push(1);
        for v in [900u32, 300, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let d = read_pvp_log_data(&mut b.as_slice()).unwrap();
        let teams = d.arena_teams.as_ref().unwrap();
        assert_eq!(
            (teams[0].name.as_str(), teams[0].new_rating),
            ("Gold", 1516)
        );
        assert_eq!(
            (teams[1].name.as_str(), teams[1].matchmaking),
            ("Green", 1510)
        );
        assert_eq!((d.ended, d.winner), (true, Some(0)));
        let r = &d.rows[0];
        assert_eq!((r.guid, r.killing_blows, r.arena_team), (7, 2, Some(1)));
        assert_eq!(
            (r.damage_done, r.healing_done, r.honorable_kills),
            (900, 300, 0)
        );
    }

    #[test]
    fn a_battleground_scoreboard_keeps_eight_stats() {
        let mut b = vec![0u8, 0];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        for v in [3u32, 9, 2, 120, 4000, 1500, 9] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in 1u32..=9 {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let d = read_pvp_log_data(&mut b.as_slice()).unwrap();
        assert!(d.arena_teams.is_none() && !d.ended && d.winner.is_none());
        let r = &d.rows[0];
        assert_eq!(
            (r.killing_blows, r.honorable_kills, r.deaths, r.bonus_honor),
            (3, 9, 2, 120)
        );
        assert_eq!(
            (r.damage_done, r.healing_done, r.arena_team),
            (4000, 1500, None)
        );
        assert_eq!(r.stats, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn positions_read_a_carrier_count() {
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend_from_slice(&0x10u64.to_le_bytes());
        b.extend_from_slice(&1.5f32.to_le_bytes());
        b.extend_from_slice(&(-2.0f32).to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        for g in [0x20u64, 0x21] {
            b.extend_from_slice(&g.to_le_bytes());
            b.extend_from_slice(&[0u8; 8]);
        }
        let p = read_battlefield_positions(&mut b.as_slice()).unwrap();
        assert_eq!(p.players.len(), 1);
        assert_eq!(
            p.carriers.iter().map(|c| c.guid).collect::<Vec<_>>(),
            [0x20, 0x21]
        );
    }

    #[test]
    fn a_timed_out_join_carries_the_player() {
        let mut b = (-11i32).to_le_bytes().to_vec();
        b.extend_from_slice(&0x99u64.to_le_bytes());
        let j = read_group_joined_battleground(&mut b.as_slice()).unwrap();
        assert_eq!((j.result, j.player), (-11, Some(0x99)));
        let b = 2u32.to_le_bytes();
        let j = read_group_joined_battleground(&mut b.as_slice()).unwrap();
        assert_eq!((j.result, j.player), (2, None));
    }

    #[test]
    fn the_sends_echo_the_queue_word() {
        let mut want = WSG_QUEUE.to_le_bytes().to_vec();
        want.push(1);
        assert_eq!(battlefield_port(WSG_QUEUE, true), want);
        assert_eq!(
            leave_battlefield(WSG_QUEUE),
            WSG_QUEUE.to_le_bytes().to_vec()
        );
        assert_eq!(battlefield_list(2, true), vec![2, 0, 0, 0, 1, 1]);
        let mut want = 0u64.to_le_bytes().to_vec();
        want.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(battlemaster_join(0, 2, 0, true), want);
        let mut want = 0x42u64.to_le_bytes().to_vec();
        want.extend_from_slice(&[0, 1, 1]);
        assert_eq!(battlemaster_join_arena(0x42, 0, true, true), want);
    }
}
