//! CharProc 17, the item swap: a kit play links an effect on the unit naming an item, and while it
//! lives one of the unit's item slots shows that item instead of its own (`0x6f8650`).
//!
//! The effect asks the item cache for its item and, on the answer (or at once on a hit), reads the
//! item's `Item.dbc` row for its slot and display (`0x6f8600`). The display lands on the slot only
//! for a unit with a character component, and past the head only for a player who can wear items in
//! its current form (`0x6f82d0`). Every rebuild of the unit's dress re-applies the live effects,
//! newest first, so the oldest wins a shared slot (`0x716e20`); the end of one effect puts the
//! slot's own item back whatever else holds it (`0x6f8700`).

use bevy::prelude::*;

/// The character component's item slots, head to cape; 11 has no inventory type (`0x4e7a30`).
pub(crate) const ITEM_SLOTS: usize = 12;

/// The component item slot an `InventoryType` dresses (`0x4e7a30`); `None` for any other type.
pub(crate) fn slot_for_inv_type(inv_type: u8) -> Option<usize> {
    Some(match inv_type {
        1 => 0,      // head
        3 => 1,      // shoulders
        4 => 2,      // shirt
        5 | 20 => 3, // chest, robe
        6 => 4,      // waist
        7 => 5,      // legs
        8 => 6,      // feet
        9 => 7,      // wrists
        10 => 8,     // hands
        19 => 9,     // tabard
        16 => 10,    // cloak
        _ => return None,
    })
}

/// One type-17 effect on a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ItemSwap {
    spell_id: u32,
    kit_id: u32,
    item: u32,
    /// The slot and display latched from the item's `Item.dbc` row (`0x6f8634`); `None` while the
    /// answer is out, or for an item with no row or no slot.
    target: Option<(usize, u32)>,
    /// The item cache has not answered yet.
    pending: bool,
}

/// What a unit's dress lets an effect apply: a character component at all, and the form gate
/// that opens the slots past the head (`0x6f8343`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SwapGate {
    pub(crate) component: bool,
    pub(crate) wears_items: bool,
}

impl SwapGate {
    fn admits(self, slot: usize) -> bool {
        self.component && (slot == 0 || self.wears_items)
    }
}

/// A unit's type-17 effects and what they show. On the net entity root; absent on a unit no item
/// swap ever reached.
#[derive(Component, Debug, Clone, Default, PartialEq)]
#[component(storage = "SparseSet")]
pub(crate) struct ItemSwaps {
    /// Newest first, as the unit's effect list links at the head (`0x6f86b7`).
    effects: Vec<ItemSwap>,
    /// The display an effect put on each item slot, `None` where the unit's own item shows.
    shown: [Option<u32>; ITEM_SLOTS],
    /// The dress inputs at the last rebuild; a change re-applies every effect.
    rebuilt: Option<u64>,
}

impl ItemSwaps {
    /// A kit play's type-17 slot: a new effect, waiting on its item (`0x726c12`).
    pub(crate) fn begin(&mut self, spell_id: u32, kit_id: u32, item: u32) {
        self.effects.insert(
            0,
            ItemSwap {
                spell_id,
                kit_id,
                item,
                target: None,
                pending: true,
            },
        );
    }

    /// End every effect `ends` matches: a resolved one puts its slot's own item back and an
    /// unanswered one drops its wait (`0x6f8700`). Whether any ended.
    fn end(&mut self, ends: impl Fn(&ItemSwap) -> bool) -> bool {
        let before = self.effects.len();
        let shown = &mut self.shown;
        self.effects.retain(|e| {
            if !ends(e) {
                return true;
            }
            if let Some((slot, _)) = e.target {
                shown[slot] = None;
            }
            false
        });
        self.effects.len() != before
    }

    /// The spell clear (`0x743b40`).
    pub(crate) fn end_spell(&mut self, spell_id: u32) -> bool {
        self.end(|e| e.spell_id == spell_id)
    }

    /// The kit clear (`0x744bd0`).
    pub(crate) fn end_kit(&mut self, kit_id: u32) -> bool {
        self.end(|e| e.kit_id == kit_id)
    }

    /// Whether an effect still waits on the item cache.
    pub(crate) fn waiting(&self) -> bool {
        self.effects.iter().any(|e| e.pending)
    }

    /// The display an effect shows on `slot`, if any.
    pub(crate) fn shown(&self, slot: usize) -> Option<u32> {
        self.shown[slot]
    }

    /// Re-apply every effect after a dress rebuild, newest first so the oldest lands last
    /// (`0x716e20`); a key equal to the last rebuild's changes nothing.
    pub(crate) fn rebuild(&mut self, key: u64, gate: SwapGate, has_display: impl Fn(u32) -> bool) {
        if self.rebuilt == Some(key) {
            return;
        }
        self.rebuilt = Some(key);
        self.shown = [None; ITEM_SLOTS];
        for e in &self.effects {
            apply(&mut self.shown, e.target, gate, &has_display);
        }
    }

    /// Settle the waiting effects `answer` has an answer for: `None` still waits, `Some(row)` is
    /// the item's `(InventoryType, display)` row, absent for an item with none (`0x6f8600`).
    pub(crate) fn answer(
        &mut self,
        mut answer: impl FnMut(u32) -> Option<Option<(u8, u32)>>,
        gate: SwapGate,
        has_display: impl Fn(u32) -> bool,
    ) {
        for e in self.effects.iter_mut().filter(|e| e.pending) {
            let Some(row) = answer(e.item) else {
                continue;
            };
            e.pending = false;
            e.target = row.and_then(|(inv, display)| Some((slot_for_inv_type(inv)?, display)));
            apply(&mut self.shown, e.target, gate, &has_display);
        }
    }
}

/// One effect's apply: a display with an `ItemDisplayInfo` row on a slot the gate admits
/// (`0x6f82d0`).
fn apply(
    shown: &mut [Option<u32>; ITEM_SLOTS],
    target: Option<(usize, u32)>,
    gate: SwapGate,
    has_display: &impl Fn(u32) -> bool,
) {
    if let Some((slot, display)) = target {
        if display != 0 && gate.admits(slot) && has_display(display) {
            shown[slot] = Some(display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: SwapGate = SwapGate {
        component: true,
        wears_items: true,
    };
    const HAT: u32 = 900;
    const ROBE: u32 = 901;

    fn row(item: u32) -> Option<Option<(u8, u32)>> {
        match item {
            1 => Some(Some((1, HAT))),
            2 => Some(Some((20, ROBE))),
            3 => Some(Some((1, 902))),
            _ => Some(None),
        }
    }

    fn any_display(_: u32) -> bool {
        true
    }

    #[test]
    fn a_cached_item_applies_at_once_and_its_end_restores_the_slot() {
        let mut s = ItemSwaps::default();
        s.rebuild(1, PLAYER, any_display);
        s.begin(10, 100, 1);
        s.answer(row, PLAYER, any_display);
        assert_eq!(s.shown(0), Some(HAT));
        assert!(!s.waiting());
        assert!(s.end_spell(10));
        assert_eq!(s.shown(0), None);
    }

    #[test]
    fn a_late_template_applies_when_it_answers() {
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 2);
        s.answer(|_| None, PLAYER, any_display);
        assert!(s.waiting());
        assert_eq!(s.shown(3), None);
        s.answer(row, PLAYER, any_display);
        assert_eq!(s.shown(3), Some(ROBE), "a robe dresses the chest slot");
    }

    #[test]
    fn an_effect_ended_before_its_answer_applies_nothing() {
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 1);
        assert!(s.end_kit(100));
        s.answer(row, PLAYER, any_display);
        assert_eq!(s.shown(0), None);
        assert!(!s.waiting());
    }

    #[test]
    fn an_item_with_no_row_or_no_slot_shows_nothing() {
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 7);
        s.begin(11, 101, 4);
        s.answer(
            |i| if i == 4 { Some(Some((13, 5))) } else { row(i) },
            PLAYER,
            any_display,
        );
        assert_eq!(s.shown, [None; ITEM_SLOTS]);
    }

    #[test]
    fn past_the_head_only_a_player_who_wears_items_takes_the_swap() {
        let npc = SwapGate {
            component: true,
            wears_items: false,
        };
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 1);
        s.begin(11, 101, 2);
        s.answer(row, npc, any_display);
        assert_eq!((s.shown(0), s.shown(3)), (Some(HAT), None));
        let beast = SwapGate::default();
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 1);
        s.answer(row, beast, any_display);
        assert_eq!(s.shown(0), None, "no character component, no swap");
    }

    #[test]
    fn a_display_without_a_row_is_not_applied() {
        let mut s = ItemSwaps::default();
        s.begin(10, 100, 1);
        s.answer(row, PLAYER, |d| d != HAT);
        assert_eq!(s.shown(0), None);
    }

    #[test]
    fn a_rebuild_reapplies_with_the_oldest_effect_winning_a_shared_slot() {
        let mut s = ItemSwaps::default();
        s.rebuild(1, PLAYER, any_display);
        s.begin(10, 100, 1);
        s.answer(row, PLAYER, any_display);
        s.begin(11, 101, 3);
        s.answer(row, PLAYER, any_display);
        assert_eq!(s.shown(0), Some(902), "the newest apply lands on the slot");
        s.rebuild(2, PLAYER, any_display);
        assert_eq!(s.shown(0), Some(HAT), "the rebuild walks newest first");
        s.rebuild(2, PLAYER, any_display);
        assert_eq!(s.shown(0), Some(HAT));
    }

    #[test]
    fn ending_one_of_two_on_a_slot_restores_the_own_item_until_a_rebuild() {
        let mut s = ItemSwaps::default();
        s.rebuild(1, PLAYER, any_display);
        s.begin(10, 100, 1);
        s.begin(11, 101, 3);
        s.answer(row, PLAYER, any_display);
        assert!(s.end_spell(11));
        assert_eq!(s.shown(0), None);
        s.rebuild(2, PLAYER, any_display);
        assert_eq!(s.shown(0), Some(HAT));
    }

    #[test]
    fn a_form_change_rebuild_applies_a_swap_the_form_refused() {
        let cat = SwapGate {
            component: false,
            wears_items: false,
        };
        let mut s = ItemSwaps::default();
        s.rebuild(1, cat, any_display);
        s.begin(10, 100, 2);
        s.answer(row, cat, any_display);
        assert_eq!(s.shown(3), None);
        s.rebuild(2, PLAYER, any_display);
        assert_eq!(s.shown(3), Some(ROBE));
    }
}
