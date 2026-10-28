//! The trainer's state re-evaluator, the reference's `Trainer_ReEvaluateServices` (`0x596b00`):
//! on any of its triggers it re-derives every service's state from the player's book, skills and
//! level, so the wire state lasts only until the first. Not built: its gray for a row in the
//! `SMSG_SEND_UNLEARN_SPELLS` list (`0x596f66`), a packet benilla does not read.

use std::collections::BTreeSet;

use benilla_formats::{LearnEffect, SkillLineCatalog, SpellCatalog};
use benilla_protocol::messages::trainer_spell_state::{GRAY, GREEN, RED};
use benilla_protocol::messages::TrainerSpell;

/// One `PLAYER_SKILL_INFO` slot: the permanent bonus adds only to a non-zero value, as the
/// skill-value getter `0x51a250` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkillSlot {
    pub skill_id: u32,
    pub step: u32,
    pub value_plus_perm: i32,
}

/// Everything `0x596b00` reads off the player, gathered once per re-evaluation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PlayerView {
    /// The player's known-spell set, the bitmap `TalentInfoIsSpellKnown` (`0x7260e0`) tests.
    pub known: BTreeSet<u32>,
    pub level: u32,
    /// The occupied `PLAYER_SKILL_INFO` slots, in slot order.
    pub skills: Vec<SkillSlot>,
}

impl PlayerView {
    /// The first slot holding the line, as both slot scans stop on it (`0x596c3d`, `0x596d84`).
    pub(super) fn skill(&self, skill_id: u32) -> Option<&SkillSlot> {
        self.skills.iter().find(|s| s.skill_id == skill_id)
    }
}

/// `TalentInfoIsSpellKnown(id) || Spell_KnownHigherRank(id)` (`0x7260e0`, `0x726160`).
pub(super) fn known_or_higher(
    id: u32,
    known: &BTreeSet<u32>,
    skill_lines: &SkillLineCatalog,
) -> bool {
    known.contains(&id) || skill_lines.higher_rank_known(id, known)
}

/// One row of `0x596b00`: a wire spell with no `Spell.dbc` record keeps its wire state
/// (`0x596b9c`); every other restarts at 0 (`0x596bad`).
pub(super) fn re_derive(
    wire: &TrainerSpell,
    spells: &SpellCatalog,
    skill_lines: &SkillLineCatalog,
    player: &PlayerView,
) -> u8 {
    if spells.get(wire.spell).is_none() {
        return wire.state;
    }
    // The effect loop, slot by slot: each counted item is known or not.
    let mut counted = 0u32;
    let mut known = 0u32;
    let mut taught_seen = false;
    for effect in spells.learn_effects(wire.spell) {
        let id = match effect {
            LearnEffect::None => continue,
            LearnEffect::SkillStep { skill, step } => {
                // Held when the line's slot has reached the step (`0x596c86`); no slot, not held.
                counted += 1;
                if player.skill(skill).is_some_and(|s| s.step >= step) {
                    known += 1;
                }
                continue;
            }
            LearnEffect::Spell(taught) => {
                taught_seen = true;
                taught
            }
            // After a taught spell any other effect is skipped (`0x596cb4`).
            LearnEffect::Other if taught_seen => continue,
            // A plain ability stands for itself, as a class trainer's every row does.
            LearnEffect::Other => wire.spell,
        };
        counted += 1;
        if known_or_higher(id, &player.known, skill_lines) {
            known += 1;
        }
        // The row's own spell ends the loop (`0x596ce8`).
        if id == wire.spell {
            break;
        }
    }
    // Everything counted is known: gray, and every later leg wants the state still 0 (`0x596d2b`).
    if counted > 0 && known == counted {
        return GRAY;
    }
    // A skill slot below the requirement is unavailable (`0x596df3`, signed); no slot writes
    // nothing. The second skill test (`0x596e1c`) reads a record field the list parser zeroes
    // (`0x6d144f`), so it never fires.
    if wire.req_skill != 0 {
        if let Some(slot) = player.skill(wire.req_skill) {
            if slot.value_plus_perm < wire.req_skill_value as i32 {
                return RED;
            }
        }
    }
    // Every required ability known or known at a higher rank (`0x596f0d`-`0x596f53`).
    let abilities_met = wire
        .req_spells
        .iter()
        .filter(|&&id| id != 0)
        .all(|&id| known_or_higher(id, &player.known, skill_lines));
    if !abilities_met {
        return RED;
    }
    // A level requirement above 1 the player lacks (`0x596fa1`).
    if wire.req_level > 1 && player.level < u32::from(wire.req_level) {
        return RED;
    }
    GREEN
}

/// Every row in place; the reference's group recount (`0x596fe3`-`0x59708a`) is the engine's.
pub(super) fn re_derive_all(
    services: &mut [TrainerSpell],
    spells: &SpellCatalog,
    skill_lines: &SkillLineCatalog,
    player: &PlayerView,
) {
    for wire in services.iter_mut() {
        wire.state = re_derive(wire, spells, skill_lines, player);
    }
}
