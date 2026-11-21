//! The pet spellbook, the spellbook window's second tab, built from the packed words
//! `SMSG_PET_SPELLS` carries after the ten bar slots
//! ([`benilla_protocol::messages::PetSpells::spells`]).
//!
//! Unlike the player book, its add-gate refuses only `Attributes & 0x80` (`DO_NOT_DISPLAY`); it has
//! no tabs, so a button's 1..12 id is the book id; and each slot carries the raw word's autocast
//! bits (31, 30) and the word itself, which `PickupSpell` puts on the cursor. The order is shared:
//! the reference sorts it with the player book's comparator.
//!
//! A book cooldown is the pet's own store, the one `GetPetActionCooldown` reads (with `isPet`), so
//! bar and book share one timer. `CastSpell(id, "pet")` sends `CMSG_PET_ACTION` with a word built
//! for the send, so the book casts spells not on the bar. The autocast verbs set the word inside
//! their call ([`set_autocast`]).

use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::PetActionEntry;
use benilla_ui::script::{PetBookState, SpellSlotView};

use crate::ui_action::Spells;
use crate::ui_pet::{PetBar, PetUnit};

/// The pet's book as its verbs read it at the call (`[0xbe7d98]`): every word with a `Spell.dbc` record not `DO_NOT_DISPLAY`, re-sorted as each `SMSG_PET_SPELLS` sorts it (`0x53cad4`).
pub(crate) fn book_live(
    bar: &PetBar,
    spells: &Spells,
    classes: Option<&crate::chr_classes::ChrClassTable>,
    pet: &PetUnit,
    (anchor, ui_now): (Instant, f64),
) -> PetBookState {
    let now = Instant::now();
    let pet_auras = pet.auras(bar.spells.pet_guid);
    let mut book = PetBookState {
        // `HasPetSpells`'s token is the player's class, not the pet's (`player.fields + 0x79`), named by `ChrClasses.dbc` field 4.
        token: pet.player_store().and_then(|s| {
            let class = u32::from(s.0.unit_class()?);
            classes.map(|t| t.0.pet_name_token(class).to_string())
        }),
        slots: Vec::new(),
    };
    // The book verbs never look at the pet list (`0x53dd34` reads the count alone): the words stand until the next `SMSG_PET_SPELLS` clears or refills them (`0x5d6c96`, `0x5d6d88`, `0x5d6cc0`).
    for &entry in &bar.spells.spells {
        // The reference's first gate: no `Spell.dbc` record, no book slot.
        let Some(d) = spells.catalog.get(entry.action()) else {
            continue;
        };
        if d.in_pet_book() {
            book.slots
                .push(slot_view(entry, d, bar, pet_auras, now, anchor, ui_now));
        }
    }
    book.slots
        .sort_by_key(|s| crate::ui_spellbook::spell_sort_key_of(&s.name, s.rank.as_deref()));
    book
}

/// One book slot, fully resolved.
fn slot_view(
    entry: PetActionEntry,
    d: &SpellDisplay,
    bar: &PetBar,
    pet_auras: Option<&crate::unit_auras::UnitAuras>,
    now: Instant,
    anchor: Instant,
    ui_now: f64,
) -> SpellSlotView {
    let spell_id = entry.action();
    SpellSlotView {
        spell_id,
        name: d.name.clone(),
        rank: d.rank.clone(),
        texture: d.icon.clone(),
        passive: d.passive,
        // `IsSelectedSpell`'s pet arm: lit while the spell's aura is on the pet
        // (`[pet.fields + 0xa4]`), the bar's own predicate.
        current: crate::ui_action::toggle::active_action_toggle(spell_id, d, pet_auras),
        // The pet's own cooldown store, the one `GetPetActionCooldown` reads.
        cooldown: bar
            .cooldowns
            .info(spell_id, 0, Some(d), now)
            .ui_triple(anchor, ui_now),
        // Off the raw word; the reference also requires a `Spell.dbc` record, which every slot has.
        autocast: Some((entry.autocast_allowed(), entry.autocast_on())),
        packed: entry.packed,
    }
}

/// `CastSpell(id, "pet", onSelf)` (`0x53bc40`): the spell word through the bar's pet-action
/// dispatcher, at the player with `onSelf`, else at the selection.
pub(crate) fn cast_pet_spell(
    p: &mut crate::ui_pet::PetPress,
    spell_id: u32,
    on_self: bool,
    sink: &mut dyn crate::ui_action::signals::SignalSink,
) -> crate::ui_pet::PetCast {
    if p.bar.spells.pet_guid == 0 {
        debug!("ui_pet_book: dropping a queued pet book cast — the bar is gone");
        return crate::ui_pet::PetCast::Done;
    }
    let target = p.spell_target(on_self);
    debug!("ui_pet_book: cast {spell_id} (target {target:?})");
    p.cast_book_spell(spell_id, target, sink)
}

/// The book's set (`0x5d5e10`): the last word for the spell (`0x5d3410`) must hold bit 31; set bit 30, `None` flipping, and copy it onto every bar slot equal under `& 0x3FFFFFFF` (`0x5d5e90`).
pub(crate) fn set_autocast(
    spells: &mut benilla_protocol::messages::PetSpells,
    spell_id: u32,
    state: Option<bool>,
) -> Option<bool> {
    let word = spells
        .spells
        .iter_mut()
        .rev()
        .find(|w| w.packed & 0x00FF_FFFF == spell_id)
        .filter(|w| w.autocast_allowed())?;
    let on = state.unwrap_or(!word.autocast_on());
    *word = word.with_autocast(on);
    let action = word.packed & 0x3FFF_FFFF;
    for slot in &mut spells.bar {
        if slot.packed & 0x3FFF_FFFF == action {
            *slot = slot.with_autocast(on);
        }
    }
    Some(on)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        PetSpells, PET_ACT_DISABLED, PET_ACT_ENABLED, PET_ACT_PASSIVE,
    };

    fn word(kind: u8, spell_id: u32) -> PetActionEntry {
        PetActionEntry::from((u32::from(kind) << 24) | spell_id)
    }

    #[test]
    fn the_pet_books_gate_is_do_not_display_alone() {
        let shown = |attributes: u32| SpellDisplay {
            attributes,
            ..Default::default()
        };
        let summons = benilla_formats::SummonPropertiesCatalog::default();
        assert!(shown(0).in_pet_book());
        assert!(!shown(0x80).in_pet_book(), "DO_NOT_DISPLAY hides it");
        // The one the player book also refuses.
        assert!(
            shown(0x20).in_pet_book(),
            "IS_TRADESKILL is not a pet-book gate"
        );
        assert!(!shown(0x20).in_spellbook(&summons));
        // A passive shows in both.
        assert!(shown(0x40).in_pet_book() && shown(0x40).in_spellbook(&summons));
    }

    #[test]
    fn the_book_toggle_mirrors_onto_the_bar() {
        const CLAW: u32 = 16827;
        let mut spells = PetSpells {
            pet_guid: 0x2A,
            spells: vec![
                word(PET_ACT_DISABLED, CLAW), // autocastable, off
                word(PET_ACT_PASSIVE, 3025),  // a passive: not autocastable
            ],
            ..Default::default()
        };
        // Claw in bar slot 4 and another spell in slot 5: the compare is by action.
        spells.bar[4] = word(PET_ACT_DISABLED, CLAW);
        spells.bar[5] = word(PET_ACT_DISABLED, 2649);

        assert_eq!(set_autocast(&mut spells, CLAW, None), Some(true));
        assert!(spells.spells[0].autocast_on(), "the book word flipped");
        assert!(spells.bar[4].autocast_on(), "…and so did the bar slot");
        assert!(
            !spells.bar[5].autocast_on(),
            "a different action is untouched"
        );

        // Flipping back reverses both.
        assert_eq!(set_autocast(&mut spells, CLAW, None), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(!spells.bar[4].autocast_on());

        // A word not autocast-allowed aborts before anything is written.
        assert_eq!(
            set_autocast(&mut spells, 3025, None),
            None,
            "a passive cannot be toggled"
        );
        assert_eq!(
            set_autocast(&mut spells, 99, None),
            None,
            "nor can a spell the pet lacks"
        );
    }

    /// `ACT_ENABLED` and `ACT_DISABLED` decode to one type, so only bit 30 tells them apart.
    #[test]
    fn the_flip_reads_bit_thirty_not_the_type_byte() {
        const GROWL: u32 = 2649;
        let mut spells = PetSpells {
            spells: vec![word(PET_ACT_ENABLED, GROWL)],
            ..Default::default()
        };
        assert!(
            spells.spells[0].autocast_on(),
            "ACT_ENABLED means bit 30 is set"
        );
        assert_eq!(set_autocast(&mut spells, GROWL, None), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(
            spells.spells[0].autocast_allowed(),
            "…and it stays ALLOWED — only bit 30 moves"
        );
    }
}
