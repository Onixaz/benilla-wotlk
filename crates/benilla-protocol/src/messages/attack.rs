//! Melee auto-attack messages: the attack start and stop edges, the per-swing report, and the
//! creature-aggro notice, which vmangos sends from `Unit::Attack`.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8};

/// `SMSG_ATTACKSTART` (vmangos `AttackStart::AppendBodyTo`): two full `u64` guids.
pub(super) fn read_attack_start(r: &mut impl Read) -> io::Result<(u64, u64)> {
    Ok((read_u64_le(r)?, read_u64_le(r)?))
}

/// The 3.3.5a `HitInfo` bits of `SMSG_ATTACKERSTATEUPDATE`, each with the 12340 site reading it.
pub mod hit_info {
    /// A trailing block of one `u32`, ten `f32` and one `u32` follows (`0x755630`).
    pub const TRAILING_BLOCK: u32 = 0x1;
    /// The swing reached the victim: blood, flinch and the hit vocal need it (`0x755e40`).
    pub const AFFECTS_VICTIM: u32 = 0x2;
    /// The off-hand swing: the off-hand weapon's animation and whoosh (`0x755130`, `0x755a60`).
    pub const OFFHAND: u32 = 0x4;
    /// A miss: the `MISS` word (`0x755a60`) and no hit vocal (`0x755e40`).
    pub const MISS: u32 = 0x10;
    /// Fully absorbed: the `ABSORB` word (`0x755a60`), `UNIT_COMBAT`'s `ABSORB` (`0x5132c0`).
    pub const FULL_ABSORB: u32 = 0x20;
    /// Partly absorbed; with [`FULL_ABSORB`] it puts the absorb array on the wire (`0x755630`).
    pub const PARTIAL_ABSORB: u32 = 0x40;
    /// Fully resisted: the `RESIST` word (`0x755a60`), `UNIT_COMBAT`'s `RESIST` (`0x5132c0`).
    pub const FULL_RESIST: u32 = 0x80;
    /// Partly resisted; with [`FULL_RESIST`] it puts the resist array on the wire (`0x755630`).
    pub const PARTIAL_RESIST: u32 = 0x100;
    /// A critical hit: the number's category (`0x755a60`), the vocal's class 3 (`0x755e40`).
    pub const CRIT: u32 = 0x200;
    /// Blocked: `BlockedAmount` follows `meleeSpellId` (`0x755630`), `UNIT_COMBAT`'s `BLOCK`.
    pub const BLOCK: u32 = 0x2000;
    /// A zero-damage hit floats no miss word (`0x755a60`).
    pub const NO_MISS_WORD: u32 = 0x4000;
    /// The large blood splash row (`0x755e40` → `0x755380`).
    pub const LARGE_BLOOD: u32 = 0x8000;
    /// A glancing blow: `UNIT_COMBAT`'s `GLANCING` (`0x5132c0`), the log entry's bit (`0x7512b0`).
    pub const GLANCING: u32 = 0x10000;
    /// A crushing blow: `UNIT_COMBAT`'s `CRUSHING` (`0x5132c0`), the vocal's class 9 (`0x755e40`).
    pub const CRUSHING: u32 = 0x20000;
    /// No swing animation (`0x755130`).
    pub const NO_ANIMATION: u32 = 0x40000;
    /// A rage word follows (`0x755630`), shown by `0x7512b0` → `0x722cf0`.
    pub const RAGE_GAIN: u32 = 0x800000;
    /// A zero-damage swing that still takes the number path and the blood (`0x755a60`).
    pub const FAKE_DAMAGE: u32 = 0x1000000;
}

/// `SMSG_ATTACKERSTATEUPDATE`: one melee swing; the reference plays one animation per packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackerState {
    pub attacker: u64,
    pub victim: u64,
    pub hit_info: u32,
    /// The whole swing's damage.
    pub damage: u32,
    /// The damage past the victim's remaining health.
    pub overkill: u32,
    /// `TargetState`: 1 hit, 2 dodge, 3 parry, 4 interrupt, 5 block, 6 evade, 7 immune, 8 deflect.
    pub victim_state: u32,
    /// The sub-damage blocks' absorb, summed.
    pub absorb: u32,
    /// The sub-damage blocks' resist, summed.
    pub resist: i32,
    /// `BlockedAmount`, on the wire only with [`hit_info::BLOCK`].
    pub blocked: u32,
    /// Nonzero for an on-next-swing ability; the reference only compares it to zero (`0x754fd0`).
    pub melee_spell_id: u32,
    /// The first sub-damage block's school mask, 1 physical, and 0 with no blocks.
    pub school_mask: u32,
}

impl AttackerState {
    /// `0x754fd0`: whether the swing gets a log entry and a floating number; it animates anyway.
    #[must_use]
    pub fn displayed(&self) -> bool {
        self.melee_spell_id == 0 || self.victim_state == 1
    }
}

/// `SMSG_ATTACKERSTATEUPDATE` in the 12340 layout (`0x755630`): optional fields ride on `HitInfo`.
pub(super) fn read_attacker_state(r: &mut impl Read) -> io::Result<AttackerState> {
    let hit_info = read_u32_le(r)?;
    let attacker = read_packed_guid(r)?;
    let victim = read_packed_guid(r)?;
    let damage = read_u32_le(r)?;
    let overkill = read_u32_le(r)?;
    let subs = read_u8(r)?;
    let mut school_mask = 0;
    for i in 0..subs {
        // The school comes from the first block; later blocks split off other schools.
        let block_school = read_u32_le(r)?;
        if i == 0 {
            school_mask = block_school;
        }
        let _damage_f = read_f32_le(r)?;
        let _damage = read_u32_le(r)?;
    }
    let mut absorb = 0u32;
    if hit_info & (hit_info::FULL_ABSORB | hit_info::PARTIAL_ABSORB) != 0 {
        for _ in 0..subs {
            absorb = absorb.wrapping_add(read_u32_le(r)?);
        }
    }
    let mut resist = 0i32;
    if hit_info & (hit_info::FULL_RESIST | hit_info::PARTIAL_RESIST) != 0 {
        for _ in 0..subs {
            resist = resist.wrapping_add(read_u32_le(r)? as i32);
        }
    }
    let victim_state = u32::from(read_u8(r)?);
    let _unread = read_u32_le(r)?;
    let melee_spell_id = read_u32_le(r)?;
    let blocked = if hit_info & hit_info::BLOCK != 0 {
        read_u32_le(r)?
    } else {
        0
    };
    if hit_info & hit_info::RAGE_GAIN != 0 {
        let _rage = read_u32_le(r)?;
    }
    if hit_info & hit_info::TRAILING_BLOCK != 0 {
        let _ = read_u32_le(r)?;
        for _ in 0..10 {
            let _ = read_f32_le(r)?;
        }
        let _ = read_u32_le(r)?;
    }
    // `0x755630`: no damage and a block is `TargetState` 5.
    let victim_state = if damage == 0 && blocked != 0 {
        5
    } else {
        victim_state
    };
    Ok(AttackerState {
        melee_spell_id,
        attacker,
        victim,
        hit_info,
        damage,
        overkill,
        victim_state,
        absorb,
        resist,
        blocked,
        school_mask,
    })
}

/// The server's refusal of `CMSG_ATTACKSWING`; every body is empty. Four opcodes, three variants:
/// the reference (`0x6255b0`) handles `0x148` and `0x149` in one shared arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackSwingError {
    /// `SMSG_ATTACKSWING_NOTINRANGE` (`0x145`): the reference latches code 1 (`0x625a8a`).
    NotInRange,
    /// `SMSG_ATTACKSWING_BADFACING` (`0x146`): the reference latches code 2 (`0x625aa1`).
    BadFacing,
    /// `SMSG_ATTACKSWING_DEADTARGET` (`0x148`) or `SMSG_ATTACKSWING_CANT_ATTACK` (`0x149`): no
    /// message, the reference only stops the attack (`0x625ab8`).
    DeadOrUnattackable,
}

/// `SMSG_ATTACKSTOP` (`AttackStop::AppendBodyTo`): two packed guids and a `u32` victim-is-dead
/// word, dropped because death arrives through the descriptor.
pub(super) fn read_attack_stop(r: &mut impl Read) -> io::Result<(u64, u64)> {
    let attacker = read_packed_guid(r)?;
    let victim = read_packed_guid(r)?;
    let _is_dead = read_u32_le(r)?;
    Ok((attacker, victim))
}

/// `SMSG_AI_REACTION` (`Server/Packets/Misc.cpp:445-449`): a raw `u64` guid and a `u32` reaction,
/// 2 (hostile) when a creature starts a melee attack, 0 (alert) on stealth detection; 1 and 4 are
/// never sent.
pub(super) fn read_ai_reaction(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let unit = read_u64_le(r)?;
    let reaction = read_u32_le(r)?;
    Ok((unit, reaction))
}

/// `CMSG_ATTACKSWING`: the full `u64` victim guid, answered by `SMSG_ATTACKSTART` or an
/// [`AttackSwingError`]. `CMSG_ATTACKSTOP` has an empty body.
pub fn attack_swing(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{hit_info, read_attacker_state, AttackerState};

    /// One swing on the wire in the 12340 layout, with one sub-damage block.
    struct Wire {
        hit_info: u32,
        damage: u32,
        overkill: u32,
        absorb: u32,
        resist: u32,
        victim_state: u8,
        melee_spell_id: u32,
        blocked: u32,
    }

    impl Wire {
        fn plain(damage: u32, victim_state: u8) -> Self {
            Wire {
                hit_info: hit_info::AFFECTS_VICTIM,
                damage,
                overkill: 0,
                absorb: 0,
                resist: 0,
                victim_state,
                melee_spell_id: 0,
                blocked: 0,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&self.hit_info.to_le_bytes());
            b.extend_from_slice(&[0x01, 0x11]); // attacker PackGUID
            b.extend_from_slice(&[0x01, 0x22]); // victim PackGUID
            b.extend_from_slice(&self.damage.to_le_bytes());
            b.extend_from_slice(&self.overkill.to_le_bytes());
            b.push(1); // one sub-damage block
            b.extend_from_slice(&0u32.to_le_bytes()); // school
            b.extend_from_slice(&(self.damage as f32).to_le_bytes());
            b.extend_from_slice(&self.damage.to_le_bytes());
            if self.hit_info & (hit_info::FULL_ABSORB | hit_info::PARTIAL_ABSORB) != 0 {
                b.extend_from_slice(&self.absorb.to_le_bytes());
            }
            if self.hit_info & (hit_info::FULL_RESIST | hit_info::PARTIAL_RESIST) != 0 {
                b.extend_from_slice(&self.resist.to_le_bytes());
            }
            b.push(self.victim_state);
            b.extend_from_slice(&0u32.to_le_bytes()); // unread
            b.extend_from_slice(&self.melee_spell_id.to_le_bytes());
            if self.hit_info & hit_info::BLOCK != 0 {
                b.extend_from_slice(&self.blocked.to_le_bytes());
            }
            if self.hit_info & hit_info::RAGE_GAIN != 0 {
                b.extend_from_slice(&7u32.to_le_bytes());
            }
            if self.hit_info & hit_info::TRAILING_BLOCK != 0 {
                b.extend_from_slice(&0u32.to_le_bytes());
                for _ in 0..10 {
                    b.extend_from_slice(&1.5f32.to_le_bytes());
                }
                b.extend_from_slice(&0u32.to_le_bytes());
            }
            b
        }

        fn parse(&self) -> AttackerState {
            let bytes = self.bytes();
            let mut r = bytes.as_slice();
            let s = read_attacker_state(&mut r).expect("parse");
            assert!(r.is_empty(), "the reader must consume the whole body");
            s
        }
    }

    /// A plain hit carries no absorb, resist or block word.
    #[test]
    fn a_plain_hit_reads_the_12340_layout() {
        let s = Wire {
            overkill: 3,
            ..Wire::plain(16, 1)
        }
        .parse();
        assert_eq!(s.attacker, 0x11);
        assert_eq!(s.victim, 0x22);
        assert_eq!(s.damage, 16);
        assert_eq!(s.overkill, 3);
        assert_eq!(s.victim_state, 1);
        assert_eq!(s.melee_spell_id, 0);
        assert_eq!((s.absorb, s.resist, s.blocked), (0, 0, 0));
        assert!(s.displayed());
    }

    /// Each optional field is read only behind its bit.
    #[test]
    fn the_optional_fields_follow_their_hit_info_bits() {
        let s = Wire {
            hit_info: hit_info::AFFECTS_VICTIM
                | hit_info::PARTIAL_ABSORB
                | hit_info::PARTIAL_RESIST
                | hit_info::BLOCK
                | hit_info::RAGE_GAIN
                | hit_info::TRAILING_BLOCK,
            absorb: 12,
            resist: 7,
            blocked: 30,
            ..Wire::plain(210, 1)
        }
        .parse();
        assert_eq!((s.absorb, s.resist, s.blocked), (12, 7, 30));
        assert_eq!(s.victim_state, 1);
        assert_eq!(s.damage, 210);
    }

    #[test]
    fn a_swing_from_a_spell_that_did_not_land_is_not_displayed() {
        let with_id = |state: u8| {
            Wire {
                melee_spell_id: 78, // Heroic Strike
                ..Wire::plain(0, state)
            }
            .parse()
        };
        // Every state but a hit (1) is suppressed.
        for state in [0, 2, 3, 4] {
            assert!(
                !with_id(state).displayed(),
                "spell swing, victimState {state}"
            );
        }
        assert!(
            with_id(1).displayed(),
            "a landing Heroic Strike still shows"
        );
        for state in [0, 1, 2, 3, 4] {
            assert!(
                Wire::plain(0, state).parse().displayed(),
                "plain swing, victimState {state}"
            );
        }
    }

    #[test]
    fn a_fully_blocked_swing_is_normalized_to_victimstate_blocks() {
        let blocked = |damage: u32, victim_state: u8, blocked: u32| {
            Wire {
                hit_info: hit_info::AFFECTS_VICTIM | hit_info::BLOCK,
                blocked,
                ..Wire::plain(damage, victim_state)
            }
            .parse()
        };
        let s = blocked(0, 1, 90);
        assert_eq!(s.victim_state, 5, "no damage through, something blocked");
        assert_eq!(s.blocked, 90);

        // A partial block lands damage, so the wire's own state stands.
        assert_eq!(blocked(50, 1, 40).victim_state, 1);

        // A dodge blocks nothing and keeps its state, damage or not.
        assert_eq!(Wire::plain(0, 2).parse().victim_state, 2);
    }
}
