//! What each pushed item view's `$`-tokens read of the player, and which of the player's changes
//! reach it. The reference builds an item tooltip at the hover and keeps nothing; the feed pushes
//! views ahead of it, so a view records the player inputs its trigger texts read as it is built
//! ([`ViewReads`]), the feed diffs those inputs frame to frame ([`Seen::advance`] into [`Moved`]), and only
//! the views that read what moved are built again. The modifiers, the skills and the gender byte
//! are not recorded per view: a change of one rebuilds every view with a `$` in a trigger text.

use std::collections::BTreeSet;

use crate::spell::text_caster::{operand_bit, CasterRead, Operands};
use crate::spell::SkillSnapshot;
use crate::unit_auras::UnitAuras;

/// What one built view's trigger texts read of the player through the parser's caster.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ViewReads {
    /// The spells a `$?s` asked the known set about.
    known: Vec<u32>,
    /// The spells a `$?a` asked the player's auras about.
    auras: Vec<u32>,
    /// The expression operands read off the player's fields, an [`operand_bit`] each.
    operands: u64,
    /// A read of a field [`Seen`] does not keep (the cast speed, a power cost's level and base
    /// power): the view is due whenever anything it could have read moved.
    unkept: bool,
}

impl ViewReads {
    /// Record one read the parser made of the player.
    pub(super) fn caster_read(&mut self, read: CasterRead) {
        match read {
            CasterRead::Spell(id) => push_unique(&mut self.known, id),
            CasterRead::Aura(id) => push_unique(&mut self.auras, id),
            CasterRead::Operand(v) => self.operands |= operand_bit(v),
            CasterRead::CastSpeed => self.unkept = true,
        }
    }

    /// A `$c` resolved a power cost off the player's fields.
    pub(super) fn power_cost(&mut self) {
        self.unkept = true;
    }

    /// The view read nothing of the player through the caster.
    pub(super) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `moved` holds an input this view read.
    pub(super) fn hits(&self, moved: &Moved) -> bool {
        (self.unkept && moved.any)
            || moved.operands & self.operands != 0
            || self.known.iter().any(|id| moved.known.contains(id))
            || self.auras.iter().any(|id| moved.auras.contains(id))
    }
}

fn push_unique(list: &mut Vec<u32>, id: u32) {
    if !list.contains(&id) {
        list.push(id);
    }
}

/// What moved since the frame before, in the terms of [`ViewReads`].
#[derive(Debug, Default, PartialEq)]
pub(super) struct Moved {
    /// The player's fields, known set or aura table changed at all.
    pub(super) any: bool,
    /// The spells learned or unlearned.
    known: Vec<u32>,
    /// The spells whose aura came onto or went off the player.
    auras: Vec<u32>,
    /// An [`operand_bit`] each.
    operands: u64,
}

/// One frame's player inputs, as the feed's params give them.
pub(super) struct Inputs<'a> {
    /// Whether a player is streamed: without one the parser has no caster and records nothing.
    pub(super) player: bool,
    pub(super) reads: super::feed::TokenReads,
    /// The spell book `$?s` reads, and the change detection's word that it was written.
    pub(super) known: &'a BTreeSet<u32>,
    pub(super) known_written: bool,
    /// The player's auras, which `$?a` reads, and the same word for their table.
    pub(super) auras: Option<&'a UnitAuras>,
    pub(super) auras_written: bool,
}

/// The player inputs the held views were built against.
pub(crate) struct Seen {
    player: bool,
    /// The skills the per-level terms scale by.
    skills: SkillSnapshot,
    /// The gender byte `$g`/`$G` branch on.
    gender: u8,
    /// The expression operands off the player's fields, and the cost runs beside them.
    operands: Operands,
    known: BTreeSet<u32>,
    /// The spells of the player's auras, ascending.
    auras: Vec<u32>,
}

impl Seen {
    pub(super) fn new(inputs: Inputs) -> Self {
        let (skills, gender, operands) = inputs.reads;
        Seen {
            player: inputs.player,
            skills,
            gender,
            operands,
            known: inputs.known.clone(),
            auras: aura_spells(inputs.auras),
        }
    }

    pub(super) fn gender(&self) -> u8 {
        self.gender
    }

    pub(super) fn operands(&self) -> &Operands {
        &self.operands
    }

    /// Take a frame's inputs in. `true` when one no view records moved (the player arriving, a
    /// skill, the gender, a cost run), which makes every view with a `$` due; else what moved, for
    /// the views that read it.
    pub(super) fn advance(&mut self, inputs: Inputs) -> (bool, Moved) {
        let (skills, gender, operands) = inputs.reads;
        let all_due = self.player != inputs.player
            || self.skills != skills
            || self.gender != gender
            || self.operands.cost_runs() != operands.cost_runs();
        let operand_bits = operands.diff(&self.operands);
        // The book is compared only when written: it is hundreds of spells and rarely moves.
        let known: Vec<u32> = if inputs.known_written && self.known != *inputs.known {
            let moved = self
                .known
                .symmetric_difference(inputs.known)
                .copied()
                .collect();
            self.known.clone_from(inputs.known);
            moved
        } else {
            Vec::new()
        };
        let now = aura_spells(inputs.auras);
        let auras = sorted_difference(&self.auras, &now);
        self.auras = now;
        (self.player, self.skills, self.gender, self.operands) =
            (inputs.player, skills, gender, operands);
        let moved = Moved {
            any: inputs.known_written
                || inputs.auras_written
                || operand_bits != 0
                || !auras.is_empty(),
            known,
            auras,
            operands: operand_bits,
        };
        (all_due, moved)
    }
}

/// The spells on the player, ascending and once each; none without an aura table.
fn aura_spells(auras: Option<&UnitAuras>) -> Vec<u32> {
    let mut spells: Vec<u32> = auras
        .map(|a| a.iter().map(|a| a.spell_id).collect())
        .unwrap_or_default();
    spells.sort_unstable();
    spells.dedup();
    spells
}

/// The ids in exactly one of two ascending lists.
fn sorted_difference(a: &[u32], b: &[u32]) -> Vec<u32> {
    a.iter()
        .filter(|id| b.binary_search(id).is_err())
        .chain(b.iter().filter(|id| a.binary_search(id).is_err()))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use benilla_formats::PlayerValue;

    use super::*;

    fn reads(of: &[CasterRead]) -> ViewReads {
        let mut r = ViewReads::default();
        for &read in of {
            r.caster_read(read);
        }
        r
    }

    /// A frame's inputs with a player, the given book and aura spells, and both written or not.
    struct Frame {
        known: BTreeSet<u32>,
        auras: UnitAuras,
        written: bool,
        gender: u8,
        operands: Operands,
        player: bool,
    }

    fn frame(known: &[u32], auras: &[u32], written: bool) -> Frame {
        Frame {
            known: known.iter().copied().collect(),
            auras: UnitAuras::of_casters(
                &auras
                    .iter()
                    .enumerate()
                    .map(|(slot, &spell)| (slot as u8, spell, 1))
                    .collect::<Vec<_>>(),
            ),
            written,
            gender: 0,
            operands: Operands::ZERO,
            player: true,
        }
    }

    impl Frame {
        fn inputs(&self) -> Inputs<'_> {
            Inputs {
                player: self.player,
                reads: (
                    crate::spell::skill_snapshot(None),
                    self.gender,
                    self.operands.clone(),
                ),
                known: &self.known,
                known_written: self.written,
                auras: Some(&self.auras),
                auras_written: self.written,
            }
        }
    }

    fn step(from: &Frame, to: &Frame) -> (bool, Moved) {
        Seen::new(from.inputs()).advance(to.inputs())
    }

    /// A refreshed aura writes the table and changes no spell on it: no view that asked about an
    /// aura is due, and one that read nothing of the player never is.
    #[test]
    fn an_aura_refresh_rebuilds_no_view() {
        let asks = reads(&[CasterRead::Aura(48266)]);
        let (all_due, moved) = step(
            &frame(&[], &[48266, 55095], false),
            &frame(&[], &[48266, 55095], true),
        );
        assert!(!all_due);
        assert!(moved.any);
        assert!(!asks.hits(&moved));
        assert!(!ViewReads::default().hits(&moved));
    }

    /// An aura coming on or going off reaches the views that asked about that spell alone.
    #[test]
    fn an_aura_change_rebuilds_the_views_that_asked_about_it() {
        let asks = reads(&[CasterRead::Aura(48266)]);
        let other = reads(&[CasterRead::Aura(55095)]);
        let (_, on) = step(&frame(&[], &[], false), &frame(&[], &[48266], true));
        assert!(asks.hits(&on));
        assert!(!other.hits(&on));
        let (_, off) = step(&frame(&[], &[48266], false), &frame(&[], &[], true));
        assert!(asks.hits(&off));
        assert!(!other.hits(&off));
    }

    /// A learn reaches the views whose `$?s` named the spell, and an operand the views that read
    /// that operand.
    #[test]
    fn a_learn_and_an_operand_reach_their_readers_alone() {
        let knows = reads(&[CasterRead::Spell(12345)]);
        let power = reads(&[CasterRead::Operand(PlayerValue::AttackPower)]);
        let (_, learned) = step(&frame(&[], &[], false), &frame(&[12345], &[], true));
        assert!(knows.hits(&learned));
        assert!(!power.hits(&learned));
        let mut stronger = frame(&[], &[], false);
        stronger.operands = Operands::with(PlayerValue::AttackPower, 100.0);
        let (all_due, moved) = step(&frame(&[], &[], false), &stronger);
        assert!(!all_due);
        assert!(power.hits(&moved));
        assert!(!knows.hits(&moved));
    }

    /// A view that read a field the feed keeps no copy of is due on any change, as every view
    /// was before the reads were recorded; with nothing moved it is not.
    #[test]
    fn an_unkept_read_is_due_on_any_change() {
        let speed = reads(&[CasterRead::CastSpeed]);
        let mut cost = ViewReads::default();
        cost.power_cost();
        let (_, refresh) = step(&frame(&[], &[1], false), &frame(&[], &[1], true));
        assert!(speed.hits(&refresh));
        assert!(cost.hits(&refresh));
        let (_, still) = step(&frame(&[], &[1], false), &frame(&[], &[1], false));
        assert!(!still.any);
        assert!(!speed.hits(&still));
    }

    /// The inputs no view records make every view due: a player arriving, the gender byte.
    #[test]
    fn a_player_arriving_or_a_gender_flip_is_everything() {
        let base = frame(&[], &[], false);
        assert!(!step(&base, &frame(&[], &[], false)).0);
        let mut gone = frame(&[], &[], false);
        gone.player = false;
        assert!(step(&gone, &base).0);
        let mut her = frame(&[], &[], false);
        her.gender = 1;
        assert!(step(&base, &her).0);
    }

    /// Each step is diffed against the one before it, not against the first.
    #[test]
    fn a_step_is_diffed_against_the_frame_before() {
        let asks = reads(&[CasterRead::Aura(7)]);
        let mut seen = Seen::new(frame(&[], &[], false).inputs());
        assert!(asks.hits(&seen.advance(frame(&[], &[7], true).inputs()).1));
        assert!(!asks.hits(&seen.advance(frame(&[], &[7], true).inputs()).1));
    }
}
