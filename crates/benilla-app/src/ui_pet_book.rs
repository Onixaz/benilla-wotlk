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
//! for the send, so the book casts spells not on the bar. `ToggleSpellAutocast(id, "pet")` sends
//! `CMSG_PET_SPELL_AUTOCAST` (0x2F3) after the reference flips bit 30 locally, mirrors it onto the
//! bar and fires `PET_BAR_UPDATE`.

use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::PetActionEntry;
use benilla_ui::script::{PetBookState, SpellSlotView, UiScript};

use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_pet::{PetBar, PetUnit};
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

pub(crate) struct UiPetBookPlugin;

impl Plugin for UiPetBookPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Before the input pass, so a tab flipped this frame reads a populated book.
                feed_pet_book
                    .in_set(UnitFeed)
                    .before(crate::ui_action::CooldownEvents),
                // After the input pass and this frame's pet book casts; it writes back into
                // `PetBar`, whose next feed carries the mirrored autocast bit.
                drain_pet_book
                    .after(UiInput)
                    .after(crate::script_calls::apply_script_calls),
            ),
        );
    }
}

/// What the feed last pushed, so `SPELLS_CHANGED` fires only on a real change.
#[derive(Default)]
pub(crate) struct FeedMemory {
    pushed: PetBookState,
}

pub(crate) fn feed_pet_book(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    spells: Option<Res<Spells>>,
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    index: Res<GuidIndex>,
    auras: Query<&crate::unit_auras::UnitAuras>,
    clock: Res<crate::ui_script::UiClock>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    // Without `Spell.dbc` the book has no slots at all; try again once it lands.
    let Some(spells) = spells.as_deref() else {
        return;
    };
    let now = Instant::now();
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    let pet_auras = index
        .0
        .get(&bar.spells.pet_guid)
        .and_then(|&e| auras.get(e).ok());

    let mut fresh = PetBookState {
        // `HasPetSpells`'s token is the player's class, not the pet's (the reference reads
        // `player.fields + 0x79`), named by `ChrClasses.dbc` field 4.
        token: self_q.single().ok().and_then(|s| {
            let class = u32::from(s.0.unit_class()?);
            classes
                .as_deref()
                .map(|t| t.0.pet_name_token(class).to_string())
        }),
        slots: Vec::new(),
    };
    if bar.has_bar() {
        for &entry in &bar.spells.spells {
            let spell_id = entry.action();
            let Some(d) = spells.catalog.get(spell_id) else {
                // The reference's first gate: no `Spell.dbc` record, no book slot.
                continue;
            };
            if !d.in_pet_book() {
                continue;
            }
            fresh
                .slots
                .push(slot_view(entry, d, &bar, pet_auras, now, anchor, ui_now));
        }
        // The reference's re-sort, with the player book's own comparator.
        fresh
            .slots
            .sort_by_key(|s| crate::ui_spellbook::spell_sort_key_of(&s.name, s.rank.as_deref()));
    }

    if fresh != memory.pushed {
        debug!("ui_pet_book: fed {} pet spell(s)", fresh.slots.len());
        let changed = book_changed(&fresh, &memory.pushed);
        script.set_pet_book(fresh.clone());
        memory.pushed = fresh;
        // One `SPELLS_CHANGED` for both books off the shared re-sort (it tail-jumps to the event
        // signal, event `0x104`). A cooldown repaints the pet page through the flush's
        // `SPELL_UPDATE_COOLDOWN` (`SpellBookFrame.lua:209`, `221`), which `ui_pet`'s
        // `fire_pet_cooldown_events` fires after this feed's push; autocast, through
        // `PET_BAR_UPDATE` (`227-231`), which `ui_pet`'s feed fires.
        if changed {
            script.fire_event("SPELLS_CHANGED", vec![]);
        }
    }
}

/// Whether the book itself changed; a slot's cooldown reaches the buttons through the flush's
/// `SPELL_UPDATE_COOLDOWN`, its autocast and ring through `PET_BAR_UPDATE`, instead.
fn book_changed(fresh: &PetBookState, old: &PetBookState) -> bool {
    fresh.token != old.token
        || fresh.slots.len() != old.slots.len()
        || fresh.slots.iter().zip(&old.slots).any(|(a, b)| {
            (a.spell_id, &a.name, &a.rank, &a.texture, a.passive)
                != (b.spell_id, &b.name, &b.rank, &b.texture, b.passive)
        })
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
) -> crate::ui_pet::PetCast {
    if p.bar.spells.pet_guid == 0 {
        debug!("ui_pet_book: dropping a queued pet book cast — the bar is gone");
        return crate::ui_pet::PetCast::Done;
    }
    let target = p.spell_target(on_self);
    debug!("ui_pet_book: cast {spell_id} (target {target:?})");
    p.cast_book_spell(spell_id, target)
}

/// Drains the pet book's `ToggleSpellAutocast` intents. Each asks the usability predicate before it
/// looks anything up, so an unusable pet's toggle flips, repaints and sends nothing.
pub(crate) fn drain_pet_book(
    script: Option<NonSendMut<UiScript>>,
    mut bar: ResMut<PetBar>,
    commands: Res<NetCommands>,
    pet: PetUnit,
) {
    let Some(mut script) = script else {
        return;
    };
    let autocasts = script.take_pet_spell_autocasts();
    if autocasts.is_empty() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        debug!("ui_pet_book: dropping queued pet book intents — the bar is gone");
        return;
    }
    for spell_id in autocasts {
        if !pet.actions_usable(&bar) {
            debug!("ui_pet_book: autocast {spell_id} refused, the pet's actions are not usable");
            continue;
        }
        let Some(on) = flip_autocast(&mut bar.spells, spell_id) else {
            continue;
        };
        // The reference's event signal `0x161`: the bar repaints before the packet leaves, and the
        // signal count carries that to the feed.
        bar.bar_signals = bar.bar_signals.wrapping_add(1);
        debug!("ui_pet_book: autocast {spell_id} -> {on}");
        let _ = commands.0.send(ClientCommand::PetSpellAutocast {
            pet_guid,
            spell_id,
            enabled: on,
        });
    }
}

/// The reference's local half: flip bit 30 of the spell's raw word, then copy it onto every bar
/// slot whose type and action match under `& 0x3FFFFFFF`, or the bar's sparkle stays stale until
/// the next `SMSG_PET_SPELLS`. `None` where the reference aborts unsent: the word is not
/// autocast-allowed, as a passive's and a command token's are not.
fn flip_autocast(
    spells: &mut benilla_protocol::messages::PetSpells,
    spell_id: u32,
) -> Option<bool> {
    let word = spells
        .spells
        .iter_mut()
        .find(|w| w.action() == spell_id && w.autocast_allowed())?;
    let on = !word.autocast_on();
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

        assert_eq!(flip_autocast(&mut spells, CLAW), Some(true));
        assert!(spells.spells[0].autocast_on(), "the book word flipped");
        assert!(spells.bar[4].autocast_on(), "…and so did the bar slot");
        assert!(
            !spells.bar[5].autocast_on(),
            "a different action is untouched"
        );

        // Flipping back reverses both.
        assert_eq!(flip_autocast(&mut spells, CLAW), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(!spells.bar[4].autocast_on());

        // A word not autocast-allowed aborts before anything is written.
        assert_eq!(
            flip_autocast(&mut spells, 3025),
            None,
            "a passive cannot be toggled"
        );
        assert_eq!(
            flip_autocast(&mut spells, 99),
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
        assert_eq!(flip_autocast(&mut spells, GROWL), Some(false));
        assert!(!spells.spells[0].autocast_on());
        assert!(
            spells.spells[0].autocast_allowed(),
            "…and it stays ALLOWED — only bit 30 moves"
        );
    }
}
