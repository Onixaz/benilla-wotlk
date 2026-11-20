//! The cast send's rank pick (`0x53d9d0`, from the send `0x80ac90` ahead of its target check and
//! packet): a buff cast on a friendly unit more than ten levels below the spell goes out as the
//! highest rank of the same name in the caster's book that the unit can take. The in-flight id,
//! `UNIT_SPELLCAST_SENT` and the server's `SMSG_SPELL_GO` then all name the rank sent.
//!
//! The pick leaves the spell alone when the caster cannot assist the unit, when the cast carries an
//! item, and for a pet unless it heads the pet list; the callers apply those gates.

use std::collections::BTreeSet;

use benilla_formats::{SpellCatalog, SpellDisplay, SummonPropertiesCatalog};
use benilla_protocol::messages::{ObjectFields, PetActionEntry};

/// `Attributes & 0x04000000` (`SPELL_ATTR0_AURA_IS_DEBUFF`): never re-ranked.
const ATTR_AURA_IS_DEBUFF: u32 = 0x0400_0000;

/// `AttributesEx2 & 0x8` (`SPELL_ATTR2_ALLOW_LOW_LEVEL_BUFF`): never re-ranked.
const ATTR_EX2_ALLOW_LOW_LEVEL_BUFF: u32 = 0x8;

/// How far above the unit's level a rank may stand.
const LEVEL_SPAN: i64 = 10;

/// The book the pick walks: the player's known spells as the spellbook files them, or the pet's.
#[derive(Clone, Copy)]
pub(crate) enum Book<'a> {
    /// The player's spells past the book's add gate ([`SpellDisplay::in_spellbook`]).
    Player {
        spells: &'a BTreeSet<u32>,
        summons: &'a SummonPropertiesCatalog,
    },
    /// The pet's book words past its add gate ([`SpellDisplay::in_pet_book`]).
    Pet(&'a [PetActionEntry]),
}

impl Book<'_> {
    /// The spell to send for `spell_id` at `unit`, whose `summoner` (if it streamed) lends the
    /// level instead.
    pub(crate) fn pick(
        self,
        spell_id: u32,
        catalog: &SpellCatalog,
        unit: &ObjectFields,
        summoner: Option<&ObjectFields>,
    ) -> u32 {
        match self.ranks(spell_id, catalog) {
            Some(ranks) => pick_among(spell_id, catalog, &ranks, unit, summoner),
            None => spell_id,
        }
    }

    /// The book's spells of `spell_id`'s name, in book order, or `None` when the pick never
    /// re-ranks it: what the send needs of the book, taken before the send's own writes.
    pub(crate) fn ranks(self, spell_id: u32, catalog: &SpellCatalog) -> Option<Vec<&SpellDisplay>> {
        let d = catalog.get(spell_id).filter(|d| re_ranks(d))?;
        let named = |other: &&SpellDisplay| other.name == d.name;
        Some(match self {
            Book::Player { spells, summons } => spells
                .iter()
                .filter_map(|&id| catalog.get(id))
                .filter(named)
                .filter(|o| o.in_spellbook(summons))
                .collect(),
            Book::Pet(words) => words
                .iter()
                .filter_map(|w| catalog.get(w.action()))
                .filter(named)
                .filter(|o| o.in_pet_book())
                .collect(),
        })
    }
}

/// The pick over `ranks` ([`Book::ranks`] of `spell_id`) at `unit`, whose `summoner` (if it
/// streamed) lends the level instead.
pub(crate) fn pick_among(
    spell_id: u32,
    catalog: &SpellCatalog,
    ranks: &[&SpellDisplay],
    unit: &ObjectFields,
    summoner: Option<&ObjectFields>,
) -> u32 {
    match fit_level(unit, summoner) {
        Some(level) => pick(spell_id, catalog, ranks.iter().copied(), level),
        None => spell_id,
    }
}

/// The level the pick fits to: the unit's, or its summoner's when the unit names one that has
/// streamed.
fn fit_level(unit: &ObjectFields, summoner: Option<&ObjectFields>) -> Option<u32> {
    match summoner.filter(|_| unit.unit_summoned_by().is_some()) {
        Some(summoner) => summoner.unit_level(),
        None => unit.unit_level(),
    }
}

/// Whether the pick reads this spell at all: neither skip bit, and an aura on some effect.
fn re_ranks(d: &SpellDisplay) -> bool {
    d.attributes & ATTR_AURA_IS_DEBUFF == 0
        && d.attributes_ex2 & ATTR_EX2_ALLOW_LOW_LEVEL_BUFF == 0
        && d.effect_apply_aura.iter().any(|&aura| aura != 0)
}

/// `spell_id`, unless it stands more than ten levels above `level`: then the highest-level spell of
/// its name in `book` that fits, walked from the end so a tie keeps the later entry, or `spell_id`
/// when none fits.
fn pick<'a>(
    spell_id: u32,
    catalog: &SpellCatalog,
    book: impl DoubleEndedIterator<Item = &'a SpellDisplay>,
    level: u32,
) -> u32 {
    let Some(d) = catalog.get(spell_id).filter(|d| re_ranks(d)) else {
        return spell_id;
    };
    let ceiling = i64::from(level) + LEVEL_SPAN;
    if i64::from(d.spell_level) <= ceiling {
        return spell_id;
    }
    let mut best = (spell_id, 0);
    for other in book.rev() {
        if other.name == d.name
            && i64::from(other.spell_level) <= ceiling
            && other.spell_level > best.1
        {
            best = (other.id, other.spell_level);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Renew's ranks 1 to 4 (`Spell.dbc`): spell levels 8, 14, 20 and 26, aura 8.
    const RENEW: [(u32, u32); 4] = [(139, 8), (6074, 14), (6075, 20), (6076, 26)];
    /// A debuff of the same shape, `Attributes & 0x04000000`.
    const DEBUFF: [(u32, u32); 2] = [(9001, 8), (9002, 30)];
    /// A buff that keeps its rank, `AttributesEx2 & 0x8`.
    const LOW_LEVEL_BUFF: [(u32, u32); 2] = [(9011, 8), (9012, 30)];
    /// A same-named spell with no aura on any effect.
    const NO_AURA: [(u32, u32); 2] = [(9021, 8), (9022, 30)];
    /// A Renew the book's add gate hides (`Attributes & 0x80`).
    const HIDDEN_RENEW: u32 = 9031;
    const PET_SPELL: [(u32, u32); 2] = [(9041, 8), (9042, 30)];

    fn row(
        id: u32,
        name: &str,
        spell_level: u32,
        attributes: u32,
        attributes_ex2: u32,
    ) -> SpellDisplay {
        SpellDisplay {
            id,
            name: name.into(),
            spell_level,
            attributes,
            attributes_ex2,
            effect_apply_aura: [8, 0, 0],
            ..SpellDisplay::default()
        }
    }

    fn catalog() -> SpellCatalog {
        let mut rows = HashMap::new();
        for (id, level) in RENEW {
            rows.insert(id, row(id, "Renew", level, 0x1_0000, 0x8_0000));
        }
        for (id, level) in DEBUFF {
            rows.insert(id, row(id, "Hex", level, ATTR_AURA_IS_DEBUFF, 0));
        }
        for (id, level) in LOW_LEVEL_BUFF {
            rows.insert(id, row(id, "Ward", level, 0, ATTR_EX2_ALLOW_LOW_LEVEL_BUFF));
        }
        for (id, level) in NO_AURA {
            rows.insert(
                id,
                SpellDisplay {
                    effect_apply_aura: [0; 3],
                    ..row(id, "Flash", level, 0, 0)
                },
            );
        }
        rows.insert(HIDDEN_RENEW, row(HIDDEN_RENEW, "Renew", 18, 0x80, 0));
        for (id, level) in PET_SPELL {
            rows.insert(id, row(id, "Shield", level, 0, 0));
        }
        SpellCatalog::from_displays(rows)
    }

    fn unit(level: u32, summoner: Option<u64>) -> ObjectFields {
        use benilla_protocol::field::{FIELD_UNIT_LEVEL, FIELD_UNIT_SUMMONEDBY};
        let by = summoner.unwrap_or(0);
        ObjectFields::from_pairs(&[
            (FIELD_UNIT_LEVEL, level),
            (FIELD_UNIT_SUMMONEDBY, by as u32),
            (FIELD_UNIT_SUMMONEDBY + 1, (by >> 32) as u32),
        ])
    }

    fn book(ids: &[u32]) -> BTreeSet<u32> {
        ids.iter().copied().collect()
    }

    fn player_pick(spell_id: u32, known: &BTreeSet<u32>, at: &ObjectFields) -> u32 {
        let summons = SummonPropertiesCatalog::default();
        Book::Player {
            spells: known,
            summons: &summons,
        }
        .pick(spell_id, &catalog(), at, None)
    }

    /// The reported case: a level-20 priest's Renew rank 3 (level 20) on a level-9 ally is more
    /// than ten above it, so rank 2 (level 14, the highest at or under 19) goes out.
    #[test]
    fn a_buff_on_a_low_ally_goes_out_at_the_highest_rank_it_takes() {
        let known = book(&[139, 6074, 6075]);
        assert_eq!(player_pick(6075, &known, &unit(9, None)), 6074);
    }

    /// Within ten levels the pressed rank stands, the bound included (20 at 10 + 10).
    #[test]
    fn within_ten_levels_the_pressed_rank_stands() {
        let known = book(&[139, 6074, 6075]);
        assert_eq!(player_pick(6075, &known, &unit(10, None)), 6075);
        assert_eq!(player_pick(6075, &known, &unit(20, None)), 6075);
    }

    /// The walk takes the highest fitting level among the ranks the caster knows, not the next
    /// rank down: a level-1 ally takes rank 1 even pressed at rank 4.
    #[test]
    fn the_walk_takes_the_highest_fitting_rank_known() {
        let known = book(&[139, 6074, 6075, 6076]);
        assert_eq!(player_pick(6076, &known, &unit(1, None)), 139);
        assert_eq!(player_pick(6076, &known, &unit(5, None)), 6074);
        // A rank the caster never learned is not a candidate.
        let gap = book(&[139, 6075, 6076]);
        assert_eq!(player_pick(6076, &gap, &unit(5, None)), 139);
    }

    /// No known rank fits: the pressed rank goes out and the server judges.
    #[test]
    fn no_fitting_rank_sends_the_pressed_one() {
        let known = book(&[6075, 6076]);
        assert_eq!(player_pick(6076, &known, &unit(1, None)), 6076);
    }

    /// The skip bits and an aura-less spell leave the id alone however low the unit.
    #[test]
    fn debuffs_low_level_buffs_and_aura_less_spells_keep_their_rank() {
        let known = book(&[9001, 9002, 9011, 9012, 9021, 9022]);
        let low = unit(1, None);
        assert_eq!(player_pick(9002, &known, &low), 9002, "AURA_IS_DEBUFF");
        assert_eq!(
            player_pick(9012, &known, &low),
            9012,
            "ALLOW_LOW_LEVEL_BUFF"
        );
        assert_eq!(
            player_pick(9022, &known, &low),
            9022,
            "no aura on any effect"
        );
    }

    /// A known spell the book's add gate hides is no candidate.
    #[test]
    fn a_spell_outside_the_book_is_no_candidate() {
        let known = book(&[139, HIDDEN_RENEW, 6075]);
        assert_eq!(player_pick(6075, &known, &unit(9, None)), 139);
    }

    /// A summoned unit fits to its summoner's level when the summoner has streamed, else its own.
    #[test]
    fn a_summoned_unit_fits_to_its_summoner() {
        let known = book(&[139, 6074, 6075]);
        let summons = SummonPropertiesCatalog::default();
        let player = Book::Player {
            spells: &known,
            summons: &summons,
        };
        let pet = unit(1, Some(0x42));
        let owner = unit(20, None);
        assert_eq!(player.pick(6075, &catalog(), &pet, Some(&owner)), 6075);
        assert_eq!(player.pick(6075, &catalog(), &pet, None), 139);
    }

    /// The pet's pick walks the pet's own book words.
    #[test]
    fn the_pet_walks_its_own_book() {
        let words = [9041, 9042].map(|packed| PetActionEntry { packed });
        let at = unit(1, None);
        assert_eq!(Book::Pet(&words).pick(9042, &catalog(), &at, None), 9041);
        assert_eq!(
            Book::Pet(&words[1..]).pick(9042, &catalog(), &at, None),
            9042
        );
    }
}
