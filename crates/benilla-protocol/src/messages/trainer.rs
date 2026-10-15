//! Trainer window wire (opcodes 432-436). The window opens off the gossip trainer option
//! (`GOSSIP_OPTION_TRAINER`), so there is no open verb, only a list refresh and a buy.

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// One 38-byte `SMSG_TRAINER_LIST` service (`NPCHandler.cpp:97-139`); the server already filters
/// the list to the player's class and race.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrainerSpell {
    /// The service spell id, which `CMSG_TRAINER_BUY_SPELL` names to buy it.
    pub spell: u32,
    /// A [`trainer_spell_state`]: the client re-derives it on its first trigger after the list.
    pub state: u8,
    /// In copper, already reputation-discounted by the server.
    pub cost: u32,
    /// Sent as `first_rank && can_learn`; the client enables Learn only when it equals
    /// `is_primary_prof_first_rank`, which greys a third primary profession.
    pub can_learn_primary_prof: bool,
    /// Taking it spends a profession slot, so the client asks for confirmation.
    pub is_primary_prof_first_rank: bool,
    pub req_level: u8,
    /// A `SkillLine.dbc` id, 0 for none.
    pub req_skill: u32,
    pub req_skill_value: u32,
    /// The `SpellChainNode` req and prev spells, then a slot vmangos always sends as 0.
    pub req_spells: [u32; 3],
}

/// `TrainerSpellState` (`Player.h:119-122`); `GREEN_DISABLED` (10) is sent as `GREEN`.
pub mod trainer_spell_state {
    /// Learnable now.
    pub const GREEN: u8 = 0;
    /// Level, skill or prerequisite unmet.
    pub const RED: u8 = 1;
    /// Already known.
    pub const GRAY: u8 = 2;
}

/// `SMSG_TRAINER_BUY_FAILED` error codes (vmangos `SharedDefines.h:1120-1122` `TRAIN_FAIL_*`).
pub mod train_fail {
    /// Not your trainer, out of line of sight, or not a listed service.
    pub const UNAVAILABLE: u32 = 0;
    pub const NOT_ENOUGH_MONEY: u32 = 1;
    pub const NOT_ENOUGH_SKILL: u32 = 2;
}

/// `CMSG_TRAINER_LIST`: asks a trainer for its list.
pub fn trainer_list(trainer_guid: u64) -> Vec<u8> {
    trainer_guid.to_le_bytes().to_vec()
}

/// `CMSG_TRAINER_BUY_SPELL`: the server answers `SMSG_TRAINER_BUY_SUCCEEDED` plus
/// `SMSG_LEARNED_SPELL`, or `SMSG_TRAINER_BUY_FAILED`.
pub fn trainer_buy_spell(trainer_guid: u64, spell_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&trainer_guid.to_le_bytes());
    body.extend_from_slice(&spell_id.to_le_bytes());
    body
}

/// `SMSG_TRAINER_LIST` (`NPCHandler.cpp:141-241`): the type is 0 class, 1 mount, 2 tradeskill or
/// 3 pet, and the title is the greeting line.
pub(super) fn read_trainer_list(
    r: &mut &[u8],
) -> io::Result<(u64, u32, Vec<TrainerSpell>, String)> {
    let trainer = read_u64_le(r)?;
    let trainer_type = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // No protocol bound (`NPCHandler.cpp:170` sums two lists); 1024 is past any real trainer.
    let mut services = Vec::with_capacity(capacity_hint(count, 1024));
    for _ in 0..count {
        // Struct-literal fields evaluate top to bottom, so this reads in wire order.
        services.push(TrainerSpell {
            spell: read_u32_le(r)?,
            state: read_u8(r)?,
            cost: read_u32_le(r)?,
            can_learn_primary_prof: read_u32_le(r)? != 0,
            is_primary_prof_first_rank: read_u32_le(r)? != 0,
            req_level: read_u8(r)?,
            req_skill: read_u32_le(r)?,
            req_skill_value: read_u32_le(r)?,
            req_spells: [read_u32_le(r)?, read_u32_le(r)?, read_u32_le(r)?],
        });
    }
    let title = read_cstring(r)?;
    Ok((trainer, trainer_type, services, title))
}

/// `SMSG_TRAINER_BUY_SUCCEEDED` (trainer, spell) is confirmation only: the spell arrives by
/// `SMSG_LEARNED_SPELL`, whose re-evaluation grays the bought row.
pub(super) fn read_trainer_buy_succeeded(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?))
}

/// `SMSG_TRAINER_BUY_FAILED`: trainer, spell and a [`train_fail`] code.
pub(super) fn read_trainer_buy_failed(r: &mut &[u8]) -> io::Result<(u64, u32, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?, read_u32_le(r)?))
}
