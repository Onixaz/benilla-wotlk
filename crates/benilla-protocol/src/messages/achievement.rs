//! 3.3.5a's achievement feed: the login list (`SMSG_ALL_ACHIEVEMENT_DATA`, `0x5b32f0`), an
//! inspected player's (`SMSG_RESPOND_INSPECT_ACHIEVEMENTS`, `0x5b34a0`), one award
//! (`SMSG_ACHIEVEMENT_EARNED`, `0x5b3020`), one criterion's progress (`SMSG_CRITERIA_UPDATE`,
//! `0x5b3160`) and the two removals (`SMSG_CRITERIA_DELETED` `0x5b36f0`,
//! `SMSG_ACHIEVEMENT_DELETED` `0x5b3610`). Dates are the packed `WowTime` word as sent.

use std::io::{self, Read};

use crate::wire::{read_i32_le, read_packed_guid, read_u32_le};

/// One criterion's progress as the server sends it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CriteriaProgress {
    pub id: u32,
    pub counter: u64,
    /// Who advanced it (a group criterion names the member).
    pub player: u64,
    /// Bit 0 deletes the row (`0x5b2520`).
    pub flags: u32,
    pub date: u32,
    /// Seconds since the timer started, and since the criterion was first updated; the client
    /// stores `now - timer` (`0x5b3160`).
    pub timer1: u32,
    pub timer2: u32,
}

/// `SMSG_ALL_ACHIEVEMENT_DATA`: the player's completed achievements and criteria progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllAchievementData {
    /// `(achievement id, packed date)` in wire order.
    pub achievements: Vec<(u32, u32)>,
    pub criteria: Vec<CriteriaProgress>,
}

/// `SMSG_RESPOND_INSPECT_ACHIEVEMENTS`: another player's list, as the login list's body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectAchievements {
    pub guid: u64,
    pub data: AllAchievementData,
}

/// `SMSG_ACHIEVEMENT_EARNED`: `player` earned `achievement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AchievementEarned {
    pub player: u64,
    pub achievement: u32,
    pub date: u32,
    /// Nonzero skips the effect and the event (`0x5b3020`).
    pub silent: bool,
}

/// Packed counter, packed player guid, `u32 flags`, packed date and two `u32` timers: one
/// criterion's body after its id (`0x5b3375`-`0x5b3412`, `0x5b3160`).
fn read_criteria_body(r: &mut impl Read, id: u32) -> io::Result<CriteriaProgress> {
    Ok(CriteriaProgress {
        id,
        counter: read_packed_guid(r)?,
        player: read_packed_guid(r)?,
        flags: read_u32_le(r)?,
        date: read_u32_le(r)?,
        timer1: read_u32_le(r)?,
        timer2: read_u32_le(r)?,
    })
}

/// The shared list body: (`u32 id`, `u32` date) until a negative id, then criteria until a
/// negative id.
fn read_list(r: &mut impl Read) -> io::Result<AllAchievementData> {
    let mut data = AllAchievementData::default();
    loop {
        let id = read_i32_le(r)?;
        if id < 0 {
            break;
        }
        data.achievements.push((id as u32, read_u32_le(r)?));
    }
    loop {
        let id = read_i32_le(r)?;
        if id < 0 {
            break;
        }
        data.criteria.push(read_criteria_body(r, id as u32)?);
    }
    Ok(data)
}

pub(super) fn read_all_achievement_data(r: &mut impl Read) -> io::Result<AllAchievementData> {
    read_list(r)
}

/// Packed guid, then the login list's body (`0x5b34a0`).
pub(super) fn read_inspect_achievements(r: &mut impl Read) -> io::Result<InspectAchievements> {
    let guid = read_packed_guid(r)?;
    Ok(InspectAchievements {
        guid,
        data: read_list(r)?,
    })
}

/// Packed player guid, `u32 id`, `u32` packed date, `u32` silent (`0x5b3039`-`0x5b3055`).
pub(super) fn read_achievement_earned(r: &mut impl Read) -> io::Result<AchievementEarned> {
    Ok(AchievementEarned {
        player: read_packed_guid(r)?,
        achievement: read_u32_le(r)?,
        date: read_u32_le(r)?,
        silent: read_u32_le(r)? != 0,
    })
}

/// `u32 id`, then one criterion's body.
pub(super) fn read_criteria_update(r: &mut impl Read) -> io::Result<CriteriaProgress> {
    let id = read_u32_le(r)?;
    read_criteria_body(r, id)
}

/// `SMSG_CRITERIA_DELETED` and `SMSG_ACHIEVEMENT_DELETED`: one `u32` id.
pub(super) fn read_deleted_id(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

/// `CMSG_QUERY_INSPECT_ACHIEVEMENTS`: the packed guid `SetAchievementComparisonUnit` names
/// (`0x5b1b1f`).
pub fn query_inspect_achievements(guid: u64) -> Vec<u8> {
    let mut body = Vec::new();
    crate::wire::write_packed_guid(guid, &mut body).expect("Vec write");
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn criterion(body: &mut Vec<u8>, id: u32, counter: u8, date: u32) {
        body.extend(id.to_le_bytes());
        body.extend([0x01, counter]);
        body.extend([0x01, 0x2a]);
        body.extend(0u32.to_le_bytes());
        body.extend(date.to_le_bytes());
        body.extend(7u32.to_le_bytes());
        body.extend(9u32.to_le_bytes());
    }

    /// AzerothCore's `AchievementMgr::SendAllAchievementData` layout: two achievements, one
    /// criterion.
    #[test]
    fn the_list_and_the_criteria_are_kept() {
        let mut body = Vec::new();
        for (id, date) in [(6u32, 0x1234u32), (7, 0x1235)] {
            body.extend(id.to_le_bytes());
            body.extend(date.to_le_bytes());
        }
        body.extend((-1i32).to_le_bytes());
        criterion(&mut body, 34, 5, 0x1234);
        body.extend((-1i32).to_le_bytes());
        let mut r = body.as_slice();
        let data = read_all_achievement_data(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(data.achievements, vec![(6, 0x1234), (7, 0x1235)]);
        assert_eq!(
            data.criteria,
            vec![CriteriaProgress {
                id: 34,
                counter: 5,
                player: 0x2a,
                flags: 0,
                date: 0x1234,
                timer1: 7,
                timer2: 9,
            }]
        );
    }

    #[test]
    fn an_inspected_list_leads_with_its_guid() {
        let mut body = vec![0x01, 0x07];
        body.extend(6u32.to_le_bytes());
        body.extend(0x1234u32.to_le_bytes());
        body.extend((-1i32).to_le_bytes());
        body.extend((-1i32).to_le_bytes());
        let mut r = body.as_slice();
        let i = read_inspect_achievements(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((i.guid, i.data.achievements), (7, vec![(6, 0x1234)]));
    }

    #[test]
    fn an_earned_award() {
        let mut body = vec![0x01, 0x2a];
        body.extend(6u32.to_le_bytes());
        body.extend(0x1234u32.to_le_bytes());
        body.extend(1u32.to_le_bytes());
        let mut r = body.as_slice();
        let e = read_achievement_earned(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            (e.player, e.achievement, e.date, e.silent),
            (0x2a, 6, 0x1234, true)
        );
    }

    #[test]
    fn a_criteria_update() {
        let mut body = Vec::new();
        criterion(&mut body, 34, 3, 0x99);
        let mut r = body.as_slice();
        let c = read_criteria_update(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((c.id, c.counter, c.timer1, c.timer2), (34, 3, 7, 9));
    }

    #[test]
    fn the_inspect_query_is_a_packed_guid() {
        assert_eq!(query_inspect_achievements(0x2a), vec![0x01, 0x2a]);
    }
}
