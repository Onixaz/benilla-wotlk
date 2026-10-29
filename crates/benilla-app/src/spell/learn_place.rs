//! The learn registrar's bar side (`0x542030`, reached from `SMSG_LEARNED_SPELL` through
//! `0x6e7b00(spell, slot, 1, 1)`): a new rank replaces the rank below it wherever the bar holds
//! it (`0x5aafd0`), and a low-level character's first rank goes onto the first empty button of
//! the page the spell belongs to (`0x53b970`), through the `PlaceAction` path, so the server
//! stores it as `CMSG_SET_ACTION_BUTTON` the same as a player drop.

use std::collections::HashMap;

use benilla_formats::{ShapeshiftForm, SpellCatalog, SpellDisplay};
use benilla_protocol::messages::ACTION_KIND_SPELL;

use crate::ui_action::PlayerActions;

/// `Attributes & 0x40`, `SPELL_ATTR_PASSIVE` (`0x542d7b`).
const ATTR_PASSIVE: u32 = 0x40;
/// `AttributesEx2 & 0x10`, a stance-bar spell (`0x542d84`).
const ATTR_EX2_DISPLAY_IN_STANCE_BAR: u32 = 0x10;
/// `AttributesEx2 & 0x80000`, castable in any form: the page chooser treats the spell as
/// unstanced (`0x53b98c`).
const ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED: u32 = 0x0008_0000;
/// `SPELL_AURA_MOD_SHAPESHIFT`: a form spell never goes on the bar (`0x542d96` →
/// `0x7fde50(rec, 0x24)`, any of the three `EffectAura` cells).
const AURA_MOD_SHAPESHIFT: u32 = 36;
/// The auto-place's level gate, `UNIT_FIELD_LEVEL <= 0xa` (`0x542d5a`).
const AUTO_PLACE_LEVEL_LIMIT: u32 = 11;
/// The classes whose spells page by form (`0x53b970`'s class test): warrior, rogue, druid.
const CLASS_WARRIOR: u8 = 1;
const CLASS_ROGUE: u8 = 4;
const CLASS_DRUID: u8 = 11;
/// A bar page is twelve buttons (`0x542db2`).
const PAGE_SLOTS: u8 = 12;

/// The player the registrar reads (`0x4d4db0` on the active player, `0x10`): its level, class and
/// current form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Learner {
    /// `UNIT_FIELD_LEVEL`.
    pub(crate) level: u32,
    /// `UNIT_FIELD_BYTES_0` byte 1.
    pub(crate) class: u8,
    /// The unit's shapeshift form, 0 unshifted.
    pub(crate) form: u8,
}

/// The registrar's gates for placing a learned spell (`0x542d4c`..`0x542da0`): the player under
/// level 11, the rank below 2 (`0x542d76`: no rank, or rank 1), not passive, not a stance-bar
/// spell, and no shapeshift aura effect.
pub(crate) fn auto_place_allowed(display: &SpellDisplay, level: u32) -> bool {
    level < AUTO_PLACE_LEVEL_LIMIT
        && display.rank_number() < 2
        && display.attributes & ATTR_PASSIVE == 0
        && display.attributes_ex2 & ATTR_EX2_DISPLAY_IN_STANCE_BAR == 0
        && !display.effect_apply_aura.contains(&AURA_MOD_SHAPESHIFT)
}

/// `0x53b970`: the first wire slot of the page a learned spell lands on. A warrior, rogue or
/// druid spell with a form mask goes to that form's bonus page, `(BonusActionBar + 5) * 12`; an
/// unstanced one goes to the main bar, except a warrior's, which follows the current stance.
/// Every other class, an unknown form, and a form the data lacks mean page one.
pub(crate) fn auto_place_page(
    display: &SpellDisplay,
    learner: Learner,
    forms: &HashMap<u32, ShapeshiftForm>,
) -> u8 {
    if !matches!(learner.class, CLASS_WARRIOR | CLASS_ROGUE | CLASS_DRUID) {
        return 0;
    }
    let unstanced = (display.stances == 0 && display.stances_high == 0)
        || display.attributes_ex2 & ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED != 0;
    let form = if unstanced {
        if learner.class != CLASS_WARRIOR {
            return 0;
        }
        u32::from(learner.form)
    } else if display.stances != 0 {
        // The lowest set bit, 1-based (`0x53b9c5`); the high dword counts on from 32.
        display.stances.trailing_zeros() + 1
    } else {
        display.stances_high.trailing_zeros() + 33
    };
    let Some(row) = forms.get(&form) else {
        return 0;
    };
    u8::try_from((row.bonus_bar + 5) * u32::from(PAGE_SLOTS)).unwrap_or(0)
}

/// The first button of the page with no action (the action bar's has-action test over
/// `page..page+12`, `0x542dc1`), or `None` when the page is full.
pub(crate) fn first_empty_slot(actions: &PlayerActions, page: u8) -> Option<u8> {
    (page..page.saturating_add(PAGE_SLOTS)).find(|slot| !actions.buttons.contains_key(slot))
}

/// `0x5aafd0(0, spell)`, run before the registrar on a live learn (`0x6e7b00`): every spell
/// button whose spell shares the new one's name and sits one rank below it, by
/// [`SpellDisplay::rank_or_level`], in ascending slot order. The reference also re-points a
/// macro bound to the old rank (`[rec+0x564]`); benilla's macro binding re-resolves from the
/// book, so the bar's macros need no write.
pub(crate) fn lower_rank_slots(
    actions: &PlayerActions,
    catalog: &SpellCatalog,
    new: &SpellDisplay,
) -> Vec<u8> {
    let new_rank = new.rank_or_level();
    let mut slots: Vec<u8> = actions
        .buttons
        .values()
        .filter(|b| b.kind == ACTION_KIND_SPELL && b.action != new.id)
        .filter(|b| {
            catalog
                .get(b.action)
                .is_some_and(|old| old.name == new.name && new_rank == old.rank_or_level() + 1)
        })
        .map(|b| b.slot)
        .collect();
    slots.sort_unstable();
    slots
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM, ACTION_KIND_MACRO};

    fn display(name: &str, rank: Option<&str>) -> SpellDisplay {
        SpellDisplay {
            name: name.to_string(),
            rank: rank.map(str::to_string),
            ..Default::default()
        }
    }

    fn button(slot: u8, action: u32, kind: u8) -> ActionButton {
        ActionButton { slot, action, kind }
    }

    fn bar(buttons: &[ActionButton]) -> PlayerActions {
        PlayerActions {
            buttons: buttons.iter().map(|b| (b.slot, *b)).collect(),
            ..Default::default()
        }
    }

    fn form(id: u32, bonus_bar: u32) -> (u32, ShapeshiftForm) {
        (
            id,
            ShapeshiftForm {
                bonus_bar,
                ..Default::default()
            },
        )
    }

    #[test]
    fn the_gates_take_a_first_rank_at_low_level_only() {
        let fireball = display("Fireball", Some("Rank 1"));
        assert!(auto_place_allowed(&fireball, 1));
        assert!(auto_place_allowed(&fireball, 10), "level 10 still places");
        assert!(!auto_place_allowed(&fireball, 11), "`< 0xb`");

        let unranked = display("Attack", None);
        assert!(auto_place_allowed(&unranked, 1), "no subtext is rank 0");
        assert!(
            !auto_place_allowed(&display("Fireball", Some("Rank 2")), 1),
            "a second rank never places"
        );
    }

    #[test]
    fn the_gates_refuse_passive_stance_bar_and_form_spells() {
        let passive = SpellDisplay {
            attributes: ATTR_PASSIVE,
            ..display("Armor", None)
        };
        assert!(!auto_place_allowed(&passive, 1));
        let stance = SpellDisplay {
            attributes_ex2: ATTR_EX2_DISPLAY_IN_STANCE_BAR,
            ..display("Stealth", None)
        };
        assert!(!auto_place_allowed(&stance, 1));
        let shapeshift = SpellDisplay {
            effect_apply_aura: [0, AURA_MOD_SHAPESHIFT, 0],
            ..display("Battle Stance", None)
        };
        assert!(!auto_place_allowed(&shapeshift, 1));
    }

    #[test]
    fn page_is_the_main_bar_outside_the_three_form_classes() {
        let forms: HashMap<u32, ShapeshiftForm> = [form(17, 1)].into_iter().collect();
        let stanced = SpellDisplay {
            stances: 1 << 16,
            ..display("Rend", Some("Rank 1"))
        };
        let mage = Learner {
            level: 1,
            class: 8,
            form: 17,
        };
        assert_eq!(auto_place_page(&stanced, mage, &forms), 0);
    }

    #[test]
    fn a_stanced_spell_pages_by_its_lowest_form_bit() {
        let forms: HashMap<u32, ShapeshiftForm> = [form(17, 1), form(18, 2), form(19, 3)]
            .into_iter()
            .collect();
        // Battle and Berserker stance: bit 16 is the lowest, form 17, bonus bar 1 → slot 72.
        let rend = SpellDisplay {
            stances: (1 << 16) | (1 << 18),
            ..display("Rend", Some("Rank 1"))
        };
        let warrior = Learner {
            level: 1,
            class: CLASS_WARRIOR,
            form: 0,
        };
        assert_eq!(auto_place_page(&rend, warrior, &forms), 72);
        // The high dword counts on from form 33; no 12340 row has one, so it pages to one.
        let high = SpellDisplay {
            stances_high: 1,
            ..display("None", None)
        };
        assert_eq!(auto_place_page(&high, warrior, &forms), 0);
    }

    #[test]
    fn an_unstanced_spell_follows_a_warriors_current_stance_only() {
        let forms: HashMap<u32, ShapeshiftForm> = [form(17, 1), form(18, 2)].into_iter().collect();
        let charge = display("Charge", Some("Rank 1"));
        let defensive = Learner {
            level: 2,
            class: CLASS_WARRIOR,
            form: 18,
        };
        assert_eq!(auto_place_page(&charge, defensive, &forms), 84);
        let unshifted = Learner {
            form: 0,
            ..defensive
        };
        assert_eq!(
            auto_place_page(&charge, unshifted, &forms),
            0,
            "form 0 has no row"
        );
        let rogue = Learner {
            class: CLASS_ROGUE,
            ..defensive
        };
        assert_eq!(auto_place_page(&charge, rogue, &forms), 0);
        // A form-waiving mask is unstanced (`0x80000`).
        let waived = SpellDisplay {
            stances: 1 << 16,
            attributes_ex2: ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED,
            ..display("Charge", Some("Rank 1"))
        };
        assert_eq!(auto_place_page(&waived, defensive, &forms), 84);
    }

    #[test]
    fn the_first_empty_button_of_the_page_is_taken_and_a_full_page_none() {
        let actions = bar(&[
            button(0, 6603, ACTION_KIND_SPELL),
            button(1, 133, ACTION_KIND_SPELL),
            button(3, 1, ACTION_KIND_ITEM),
        ]);
        assert_eq!(first_empty_slot(&actions, 0), Some(2));
        assert_eq!(first_empty_slot(&actions, 12), Some(12));
        let full: Vec<ActionButton> = (72..84).map(|s| button(s, 1, ACTION_KIND_MACRO)).collect();
        assert_eq!(first_empty_slot(&bar(&full), 72), None);
        assert_eq!(first_empty_slot(&bar(&full), 84), Some(84));
    }

    #[test]
    fn the_rank_below_is_found_by_name_and_rank_number() {
        let catalog = SpellCatalog::from_displays(
            [
                (133, display("Fireball", Some("Rank 1"))),
                (143, display("Fireball", Some("Rank 2"))),
                (145, display("Fireball", Some("Rank 3"))),
                (116, display("Frostbolt", Some("Rank 1"))),
            ]
            .into_iter()
            .map(|(id, d)| (id, SpellDisplay { id, ..d }))
            .collect(),
        );
        let actions = bar(&[
            button(5, 133, ACTION_KIND_SPELL),
            button(2, 133, ACTION_KIND_SPELL),
            button(3, 116, ACTION_KIND_SPELL),
            button(4, 133, ACTION_KIND_MACRO),
            button(6, 145, ACTION_KIND_SPELL),
        ]);
        let rank2 = catalog.get(143).unwrap();
        assert_eq!(
            lower_rank_slots(&actions, &catalog, rank2),
            vec![2, 5],
            "both rank-1 buttons, in slot order; the macro, the other spell and rank 3 stay"
        );
        let rank1 = catalog.get(133).unwrap();
        assert!(lower_rank_slots(&actions, &catalog, rank1).is_empty());
    }

    /// A subtext without a number orders by `spellLevel` (`0x53b5a0`).
    #[test]
    fn an_unnumbered_rank_orders_by_spell_level() {
        let catalog = SpellCatalog::from_displays(
            [
                (
                    1,
                    SpellDisplay {
                        spell_level: 4,
                        ..display("Riding", Some("Apprentice"))
                    },
                ),
                (
                    2,
                    SpellDisplay {
                        spell_level: 5,
                        ..display("Riding", Some("Journeyman"))
                    },
                ),
            ]
            .into_iter()
            .map(|(id, d)| (id, SpellDisplay { id, ..d }))
            .collect(),
        );
        let actions = bar(&[button(0, 1, ACTION_KIND_SPELL)]);
        assert_eq!(
            lower_rank_slots(&actions, &catalog, catalog.get(2).unwrap()),
            vec![0]
        );
    }
}
