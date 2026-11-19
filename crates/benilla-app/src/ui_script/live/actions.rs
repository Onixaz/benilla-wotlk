//! The bar's per-slot verbs read at the call: the slot resolves to its item (`0x5a8070`), else its
//! spell and whether the pet casts it (`0x5a8c30`), then each verb asks what 12340's asks. A macro
//! resolves through its bound spell; its `/use` item, its spell's pet flag and its own target are
//! not built, so a macro slot reads as its spell cast by the player at the locked target.

use benilla_protocol::messages::{
    PetActionEntry, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
use benilla_ui::script::SpellInfoSource;

use crate::ui_action::ACTION_KIND_PET;

use super::LiveView;

/// `Spell.dbc`'s auto-attack effect, `IsAttackAction`'s test (`0x5a96d0`).
const EFFECT_ATTACK: u32 = 78;

/// `AttributesExB & 0x40000`: a pet-bar spell the player casts (`0x5a8cb5`).
const EX2_CAST_BY_PLAYER: u32 = 0x0004_0000;

/// A bar slot as the resolvers read it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Resolved {
    /// The slot holds a word at all.
    occupied: bool,
    /// Its item (`0x5a8070`), 0 for none.
    item: u32,
    /// Its spell (`0x5a8c30`), 0 for none.
    spell: u32,
    /// The pet casts the spell.
    pet: bool,
    /// A pet action slot's word, for the current test's mode and command arms.
    pet_word: Option<PetActionEntry>,
}

/// Pet action word kinds: a spell, a reaction, a command, and the spell kinds 8-17 (`0x5a8113`).
const PET_KIND_SPELL: u8 = 1;
const PET_KIND_REACTION: u8 = 6;
const PET_KIND_COMMAND: u8 = 7;

fn pet_spell_kind(kind: u8) -> bool {
    kind == PET_KIND_SPELL || (8..=17).contains(&kind)
}

impl LiveView<'_, '_> {
    /// The 0-based bar `slot` through the resolvers.
    pub(super) fn resolve_slot(&self, slot: u32) -> Option<Resolved> {
        let actions = self.actions.get()?;
        let Some(button) = u8::try_from(slot)
            .ok()
            .and_then(|s| actions.buttons.get(&s))
        else {
            return Some(Resolved::default());
        };
        let mut out = Resolved {
            occupied: true,
            ..Resolved::default()
        };
        match button.kind {
            ACTION_KIND_SPELL => out.spell = button.action,
            ACTION_KIND_ITEM => {
                out.item = button.action;
                out.spell = self.item_use_spell(button.action);
            }
            ACTION_KIND_MACRO => {
                use crate::ui_macro::BoundSpell;
                if let Some(BoundSpell::Spell(s)) =
                    self.bound.get().and_then(|b| b.0.get(&button.action))
                {
                    out.spell = *s;
                }
            }
            k if k == ACTION_KIND_PET => {
                let bar = self.pet_bar.get();
                let word = bar
                    .filter(|b| b.spells.pet_guid != 0)
                    .and_then(|b| b.spells.bar.get(button.action as usize).copied())
                    .filter(|w| !w.is_empty());
                out.pet_word = word;
                // `0x5a80e0`: a spell kind's id, else none; the pet casts it unless the record
                // says the player does.
                out.spell = word
                    .filter(|w| pet_spell_kind(w.kind()))
                    .map_or(0, |w| w.action());
                out.pet = self
                    .spells
                    .as_deref()
                    .and_then(|s| s.catalog.get(out.spell))
                    .is_none_or(|d| d.attributes_ex2 & EX2_CAST_BY_PLAYER == 0);
            }
            _ => {}
        }
        Some(out)
    }

    /// An item's use spell (`0x5a89b0`), off its cached template; the use enchant's spell is not
    /// built.
    fn item_use_spell(&self, item_id: u32) -> u32 {
        let Some((_, items, commands)) = self.containers.as_ref().map(|c| c.item_reads()) else {
            return 0;
        };
        items
            .template(item_id, 0, commands)
            .and_then(|t| t.use_spell)
            .map_or(0, |u| u.spell_id)
    }

    /// Run `f` over the call's spell answers.
    fn answers<R>(&self, f: impl FnOnce(&dyn SpellInfoSource) -> R) -> Option<R> {
        let spells = self.spells.as_deref()?;
        Some(f(&crate::ui_spell_info::Answers {
            tables: crate::ui_spell_info::Tables {
                spells,
                skill_lines: self.skill_lines.as_deref().map(|l| &*l.catalog),
                talents: self.talents.as_deref().map(|t| &*t.catalog),
            },
            reads: self,
        }))
    }

    /// `IsAttackAction` (`0x5a96d0`): the spell's first effect is the auto-attack.
    pub(super) fn slot_attack(&self, r: Resolved) -> bool {
        self.spells
            .as_deref()
            .and_then(|s| s.catalog.get(r.spell))
            .is_some_and(|d| d.effects[0] == EFFECT_ATTACK)
    }

    /// `IsCurrentAction` (`0x5aa240`): an item asks the current-item test; the auto-attack our
    /// attack lock; a pet word its reaction against the pet mode (`0x5a7980`), its command against
    /// the mode word's upper bytes or Attack against the pet's attack latch, falling through to the
    /// spell test as the reference's command arm does; anything else the current-spell test.
    pub(super) fn slot_current(&self, r: Resolved) -> bool {
        if !r.occupied {
            return false;
        }
        if r.item != 0 {
            return self.answers(|a| a.is_current_item(r.item)) == Some(true);
        }
        if self.slot_attack(r) {
            return !self.engaged.is_empty();
        }
        if let Some(word) = r.pet_word {
            let Some(bar) = self.pet_bar.get() else {
                return false;
            };
            let id = word.action();
            match word.kind() {
                PET_KIND_REACTION => {
                    let mode = if bar.spells.vehicle() {
                        benilla_protocol::messages::PET_REACT_PASSIVE
                    } else {
                        bar.spells.react_state()
                    };
                    return mode == id;
                }
                PET_KIND_COMMAND
                    if bar.spells.state >> 8 == id
                        || (id == benilla_protocol::messages::PET_COMMAND_ATTACK
                            && bar.attacking) =>
                {
                    return true;
                }
                PET_KIND_COMMAND => {}
                k if (8..=17).contains(&k) => {}
                _ => return false,
            }
        }
        r.spell != 0 && self.answers(|a| a.is_current_spell(r.spell)) == Some(true)
    }

    /// `IsAutoRepeatAction` (`0x5a9470`): a spell the player casts that is the auto-repeat cell's.
    pub(super) fn slot_auto_repeat(&self, r: Resolved) -> bool {
        r.spell != 0 && !r.pet && self.answers(|a| a.is_auto_repeat_spell(r.spell)) == Some(true)
    }

    /// `IsActionInRange` (`0x5a94c0`): a spell with a record, tested from its caster at `target`,
    /// the locked target for 0 (`0x809610`).
    pub(super) fn slot_in_range(&self, r: Resolved, target: u64) -> Option<bool> {
        let target = if target == 0 {
            self.selection.get().and_then(|s| s.guid)?
        } else {
            target
        };
        self.answers(|a| a.spell_in_range(r.spell, r.pet, target))
            .flatten()
    }

    /// `ActionHasRange` (`0x5a95e0`).
    pub(super) fn slot_has_range(&self, r: Resolved) -> bool {
        self.answers(|a| a.spell_has_range(r.spell, r.pet)) == Some(true)
    }

    /// `IsEquippedAction` (`0x5a88b0`): an item with an inventory type worn, a copy with charges
    /// left counting first when the item's use spell has finite charges (`0x5a894b`-`0x5a8990`).
    pub(super) fn slot_equipped(&self, r: Resolved) -> bool {
        let (Some((objects, items, commands)), Some((_, me))) =
            (self.containers.as_ref().map(|c| c.item_reads()), self.me())
        else {
            return false;
        };
        let Some(t) = items.template(r.item, 0, commands) else {
            return false;
        };
        if r.item == 0 || t.inventory_type == 0 {
            return false;
        }
        // The base charges of the first spell slot; a use enchant's are not built.
        let charges = t.spell_charges_0;
        let finite = charges != -1 && charges != 0;
        let find = |equipment_only, live_charges_only| {
            crate::ui_items::find_item(
                &me.0,
                objects,
                r.item,
                crate::ui_items::ItemSearch {
                    equipment_only,
                    live_charges_only,
                },
            )
            .is_some()
        };
        find(true, finite) || (finite && !find(false, true) && find(true, false))
    }

    /// `GetActionCooldown` (`0x5a8e40`): an item's history read by item, else the spell's from
    /// its caster's list; `(0, 0, false)` for a slot naming neither.
    pub(super) fn slot_cooldown(
        &self,
        r: Resolved,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> (i64, u32, bool) {
        const IDLE: (i64, u32, bool) = (0, 0, true);
        let now = std::time::Instant::now();
        let d = |spell| self.spells.as_deref().and_then(|s| s.catalog.get(spell));
        if r.item != 0 {
            let Some(cooldowns) = self.cooldowns.get() else {
                return IDLE;
            };
            return cooldowns
                .info(r.spell, r.item, d(r.spell), now)
                .ui_triple(anchor, ui_now)
                .unwrap_or(IDLE);
        }
        if r.spell == 0 {
            return (0, 0, false);
        }
        if r.pet {
            return self
                .pet_bar
                .get()
                .and_then(|b| {
                    b.cooldowns
                        .info(r.spell, 0, d(r.spell), now)
                        .ui_triple(anchor, ui_now)
                })
                .unwrap_or(IDLE);
        }
        benilla_ui::script::LiveState::spell_cooldown(self, r.spell, anchor, ui_now).unwrap_or(IDLE)
    }
}
