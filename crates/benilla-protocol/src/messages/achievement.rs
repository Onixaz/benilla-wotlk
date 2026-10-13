//! 3.3.5a's achievement feed, as far as the client's completed-achievement list:
//! `SMSG_ALL_ACHIEVEMENT_DATA` (handler `0x5b32f0`) at login and `SMSG_ACHIEVEMENT_EARNED`
//! (`0x5b3020`) per award. Criteria progress is read past and not kept.

use std::io::{self, Read};

use crate::wire::{read_i32_le, read_packed_guid, read_u32_le};

/// `SMSG_ALL_ACHIEVEMENT_DATA`: the player's completed achievements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllAchievementData {
    pub completed: Vec<u32>,
}

/// Read `SMSG_ALL_ACHIEVEMENT_DATA`: (`u32 id`, `u32` packed date) until a negative id, then
/// criteria until a negative id, each `u32 id`, packed counter, packed player guid, `u32 flags`,
/// `u32` packed date and two `u32` timers (`0x5b3375`-`0x5b3412`).
pub(super) fn read_all_achievement_data(r: &mut impl Read) -> io::Result<AllAchievementData> {
    let mut completed = Vec::new();
    loop {
        let id = read_i32_le(r)?;
        if id < 0 {
            break;
        }
        let _date = read_u32_le(r)?;
        completed.push(id as u32);
    }
    loop {
        let id = read_i32_le(r)?;
        if id < 0 {
            break;
        }
        let _counter = read_packed_guid(r)?;
        let _player = read_packed_guid(r)?;
        let _flags = read_u32_le(r)?;
        let _date = read_u32_le(r)?;
        let _timer1 = read_u32_le(r)?;
        let _timer2 = read_u32_le(r)?;
    }
    Ok(AllAchievementData { completed })
}

/// `SMSG_ACHIEVEMENT_EARNED`: `player` earned `achievement`; only the player's own joins the
/// completed list (`0x5b3075`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AchievementEarned {
    pub player: u64,
    pub achievement: u32,
}

/// Read `SMSG_ACHIEVEMENT_EARNED`: packed player guid, `u32 id`, `u32` packed date, `u32`
/// (`0x5b3039`-`0x5b3055`).
pub(super) fn read_achievement_earned(r: &mut impl Read) -> io::Result<AchievementEarned> {
    let player = read_packed_guid(r)?;
    let achievement = read_u32_le(r)?;
    let _date = read_u32_le(r)?;
    let _unknown = read_u32_le(r)?;
    Ok(AchievementEarned {
        player,
        achievement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AzerothCore's `AchievementMgr::SendAllAchievementData` layout: two achievements, one
    /// criterion.
    #[test]
    fn the_list_and_the_criteria_are_read_to_their_terminators() {
        let mut body = Vec::new();
        for (id, date) in [(6u32, 0x1234u32), (7, 0x1235)] {
            body.extend(id.to_le_bytes());
            body.extend(date.to_le_bytes());
        }
        body.extend((-1i32).to_le_bytes());
        body.extend(34u32.to_le_bytes()); // criterion
        body.extend([0x01, 0x05]); // counter 5
        body.extend([0x01, 0x2a]); // player guid
        body.extend(0u32.to_le_bytes());
        body.extend(0x1234u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend((-1i32).to_le_bytes());
        let mut r = body.as_slice();
        let data = read_all_achievement_data(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(data.completed, vec![6, 7]);
    }

    #[test]
    fn an_earned_award() {
        let mut body = vec![0x01, 0x2a];
        body.extend(6u32.to_le_bytes());
        body.extend(0x1234u32.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        let mut r = body.as_slice();
        let e = read_achievement_earned(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((e.player, e.achievement), (0x2a, 6));
    }
}
