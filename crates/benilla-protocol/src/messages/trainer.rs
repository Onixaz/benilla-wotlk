//! Trainer window wire (opcodes 432-436). The window opens off the gossip trainer option
//! (`GOSSIP_OPTION_TRAINER`), so there is no open verb, only a list refresh and a buy.

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// One 38-byte `SMSG_TRAINER_LIST` service, as the player's trainer-list handler (`0x6d12c0`) reads
/// it; the server already filters the list to the player's class and race.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrainerSpell {
    /// The service spell id, which `CMSG_TRAINER_BUY_SPELL` names to buy it.
    pub spell: u32,
    /// A [`trainer_spell_state`]: the client re-derives it on its first trigger after the list.
    pub state: u8,
    /// In copper, already reputation-discounted by the server.
    pub cost: u32,
    /// The talent-point cost, which `GetTrainerServiceCost` returns second.
    pub talent_cost: u32,
    /// The profession-point cost, returned third; the stock window confirms a service that has
    /// one and greys it when the player has no profession slot left.
    pub profession_cost: u32,
    pub req_level: u8,
    /// A `SkillLine.dbc` id, 0 for none.
    pub req_skill: u32,
    pub req_skill_value: u32,
    /// The three required-ability slots, read raw.
    pub req_spells: [u32; 3],
}

/// The state byte `0x594260` maps to a type string: 0 available, 2 used, anything else unavailable.
pub mod trainer_spell_state {
    /// Learnable now.
    pub const GREEN: u8 = 0;
    /// Level, skill or prerequisite unmet.
    pub const RED: u8 = 1;
    /// Already known.
    pub const GRAY: u8 = 2;
}

/// `SMSG_TRAINER_BUY_FAILED` error codes, each a console line in `0x6d8410`.
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

/// `SMSG_TRAINER_LIST` (`0x6d12c0`): the type is 2 at a tradeskill trainer, and the title is the
/// greeting line.
pub(super) fn read_trainer_list(
    r: &mut &[u8],
) -> io::Result<(u64, u32, Vec<TrainerSpell>, String)> {
    let trainer = read_u64_le(r)?;
    let trainer_type = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // No protocol bound (`0x6d12c0` sizes its arrays by the count); 1024 is past any real trainer.
    let mut services = Vec::with_capacity(capacity_hint(count, 1024));
    for _ in 0..count {
        // Struct-literal fields evaluate top to bottom, so this reads in wire order.
        services.push(TrainerSpell {
            spell: read_u32_le(r)?,
            state: read_u8(r)?,
            cost: read_u32_le(r)?,
            talent_cost: read_u32_le(r)?,
            profession_cost: read_u32_le(r)?,
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
