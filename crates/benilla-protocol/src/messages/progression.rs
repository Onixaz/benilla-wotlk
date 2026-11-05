//! Character-progression messages: XP, exploration, level-up, talent learning and talent wipe.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_i32_le, read_u32_le, read_u64_le, read_u8};

/// `SMSG_LOG_XPGAIN` (`0x50c720`): an XP award, or a loss when `victim` is our own player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XpGain {
    /// The kill's victim; 0 for non-kill XP.
    pub victim: u64,
    pub total: u32,
    /// The award's kind: 0 a kill, which alone carries `base` and `group_rate`.
    pub xp_type: u8,
    /// The kill's award before the rested bonus; 0 otherwise, as the handler zeroes it.
    pub base: u32,
    /// The kill's group rate; 1.0 otherwise (`0x50c75b`).
    pub group_rate: f32,
    /// The Refer-a-Friend flag, the body's last byte (`0x50c787`).
    pub refer_a_friend: bool,
}

/// Read `SMSG_LOG_XPGAIN`: `u64 victim, u32 total, u8 type` (`0x50c756`), then for a kill `u32
/// base, f32 groupRate` (`0x50c76e`, `0x50c779`), then always the `u8` Refer-a-Friend flag.
pub(super) fn read_xp_gain(r: &mut impl Read) -> io::Result<XpGain> {
    let victim = read_u64_le(r)?;
    let total = read_u32_le(r)?;
    let xp_type = read_u8(r)?;
    let (mut base, mut group_rate) = (0, 1.0);
    if xp_type == 0 {
        base = read_u32_le(r)?;
        group_rate = read_f32_le(r)?;
    }
    let refer_a_friend = read_u8(r)? != 0;
    Ok(XpGain {
        victim,
        total,
        xp_type,
        base,
        group_rate,
        refer_a_friend,
    })
}

/// `SMSG_EXPLORATION_EXPERIENCE`: a first visit to an area, sent even for 0 XP
/// (`Player.cpp:6228-6341`). The XP also arrives as a non-kill [`XpGain`]; this names the area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorationXp {
    /// The `AreaTable.dbc` row id, not the explore-flag bit.
    pub area_id: u32,
    /// The XP awarded; 0 at max level or for an area without a level.
    pub xp: u32,
}

/// Read `SMSG_EXPLORATION_EXPERIENCE`: `u32 areaId, u32 xp` (vmangos `Misc.cpp:552-556`).
pub(super) fn read_exploration_xp(r: &mut impl Read) -> io::Result<ExplorationXp> {
    let area_id = read_u32_le(r)?;
    let xp = read_u32_le(r)?;
    Ok(ExplorationXp { area_id, xp })
}

/// Body of `CMSG_LEARN_TALENT`: `u32` `Talent.dbc` id, `u32` 0-based rank. Requesting rank k
/// learns up to k (`Player.cpp:20807`), so a click sends the current rank count.
pub fn learn_talent(talent_id: u32, requested_rank: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&talent_id.to_le_bytes());
    body.extend_from_slice(&requested_rank.to_le_bytes());
    body
}

/// Inbound `MSG_TALENT_WIPE_CONFIRM`: a class trainer asks whether to unlearn every talent
/// (vmangos `Player.cpp:8414`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentWipeConfirm {
    /// The asking trainer, echoed in the answer and required to be a trainer in range there.
    /// Zero means "you have no talents to reset" (vmangos `SkillHandler.cpp:52`).
    pub trainer: u64,
    /// Cost in copper, climbing with every reset (`Player::GetResetTalentsCost`).
    pub cost: u32,
}

/// Read the inbound `MSG_TALENT_WIPE_CONFIRM`: `u64 trainer, u32 cost` (`Skill.cpp:19-23`).
pub(super) fn read_talent_wipe_confirm(r: &mut impl Read) -> io::Result<TalentWipeConfirm> {
    Ok(TalentWipeConfirm {
        trainer: read_u64_le(r)?,
        cost: read_u32_le(r)?,
    })
}

/// Body of the outbound `MSG_TALENT_WIPE_CONFIRM`, the `CONFIRM_TALENT_WIPE` Accept: the trainer
/// guid alone (vmangos `Skill.cpp:14-17`), as the reference latches it.
pub fn talent_wipe_confirm(trainer_guid: u64) -> Vec<u8> {
    trainer_guid.to_le_bytes().to_vec()
}

/// `SMSG_LEVELUP_INFO`: our own level-up, sent only to us with no guid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelUpInfo {
    /// The level just reached.
    pub level: u32,
    /// Hit points gained, negative on a loss.
    pub health: i32,
    /// Power gains in power-type order: mana, rage, focus, energy, happiness, runes, runic
    /// power. `PLAYER_LEVEL_UP` passes only the first (`0x6e367c`).
    pub powers: [i32; 7],
    /// Stat gains in `SPELL_STAT0..4` order (str, agi, stam, int, spirit), one chat line per
    /// positive entry.
    pub stats: [i32; 5],
}

/// Read `SMSG_LEVELUP_INFO` (`0x6e3571`): `u32` level, `i32` health, 7 powers (`0x6e358a`), 5
/// stats. The reference fires the gains as `%d`, so a loss the server wraps into its `u32` reads
/// negative.
pub(super) fn read_level_up_info(r: &mut impl Read) -> io::Result<LevelUpInfo> {
    let level = read_u32_le(r)?;
    let health = read_i32_le(r)?;
    let mut powers = [0; 7];
    for p in &mut powers {
        *p = read_i32_le(r)?;
    }
    let mut stats = [0; 5];
    for s in &mut stats {
        *s = read_i32_le(r)?;
    }
    Ok(LevelUpInfo {
        level,
        health,
        powers,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_talent_wipe_pair_round_trips() {
        let trainer: u64 = 0xF130_0000_0000_2A01;
        let mut body = trainer.to_le_bytes().to_vec();
        body.extend_from_slice(&15_000u32.to_le_bytes());
        assert_eq!(
            read_talent_wipe_confirm(&mut &body[..]).unwrap(),
            TalentWipeConfirm {
                trainer,
                cost: 15_000
            }
        );
        assert_eq!(talent_wipe_confirm(trainer), trainer.to_le_bytes().to_vec());
    }

    #[test]
    fn a_zero_trainer_is_a_parseable_refusal() {
        let mut body = 0u64.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        let ask = read_talent_wipe_confirm(&mut &body[..]).unwrap();
        assert_eq!(ask.trainer, 0);
    }
}
