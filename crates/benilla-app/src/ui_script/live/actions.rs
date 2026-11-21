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

impl<'w, 's> LiveView<'w, 's> {
    /// `f` over the usable walk's reads (`0x80b5d0`) as the view holds them at the call.
    pub(super) fn usable_reads<R>(
        &self,
        f: impl FnOnce(&crate::ui_action::usable_cache::Reads<'_, 'w, 's>) -> R,
    ) -> Option<R> {
        let (objects, items, commands) = self.containers.as_ref()?.item_reads();
        let reads = crate::ui_action::usable_cache::Sources {
            spells: self.spells.as_deref(),
            actions: self.actions.as_deref(),
            slots: self.slots.as_deref(),
            bound: self.bound.as_deref(),
            pet_bar: self.pet_bar.as_deref(),
            cooldowns: self.cooldowns.as_deref(),
            me: self.me(),
            target_guid: self.selection.as_deref().and_then(|s| s.guid).unwrap_or(0),
            stores: &self.stores,
            objects,
            auras: &self.auras,
            place: self.place.as_deref(),
            world: &self.usable_world,
            factions: self.factions.as_deref(),
            reputations: self.reputations.as_deref(),
            mods: self.spell_mods.as_deref(),
            runes: self.runes.as_deref(),
            rune_costs: self.rune_costs.as_deref(),
            items: Some(items),
            commands: Some(commands),
        }
        .reads()?;
        Some(f(&reads))
    }

    /// `IsUsableItem` (`0x51c450`): a held copy (mask 0, `0x51c4dc`); not on hold under its first
    /// on-use spell (`0x80ac10`); in an arena, unless the template's flags carry `0x200000`,
    /// neither used up by its charges nor on a cooldown past 15 minutes (`0x51c538`-`0x51c5ae`);
    /// then the use spell's walk with no target and its power check, or usable with no spell row.
    /// The charges and cooldowns are the template's on-use block's: a use enchant is not built.
    pub(super) fn item_usable_now(&self, item_id: u32) -> (bool, bool) {
        const ARENA_USABLE: u32 = 0x0020_0000;
        const COOLDOWN_CAP_MS: i32 = 900_000;
        let (Some((_, me)), Some((objects, items, commands))) =
            (self.me(), self.containers.as_ref().map(|c| c.item_reads()))
        else {
            return (false, false);
        };
        if item_id == 0
            || crate::ui_items::count_of(
                &me.0,
                objects,
                item_id,
                crate::ui_items::InventoryScope::DEFAULT,
            ) == 0
        {
            return (false, false);
        }
        let template = items.template(item_id, 0, commands);
        let use_spell = template.and_then(|t| t.use_spell);
        if let (Some(u), Some(cooldowns)) = (use_spell, self.cooldowns.as_deref()) {
            let category = match u.category {
                0 => self
                    .spells
                    .as_deref()
                    .and_then(|s| s.catalog.get(u.spell_id))
                    .map_or(0, |d| d.category),
                c => c,
            };
            if cooldowns.has_on_hold_record(u.spell_id, item_id, category) {
                return (false, false);
            }
        }
        if let Some(t) = template.filter(|t| t.flags & ARENA_USABLE == 0) {
            if self.usable_world.in_arena() {
                let (cooldown, category) =
                    use_spell.map_or((0, 0), |u| (u.cooldown_ms, u.category_cooldown_ms));
                if t.spell_charges_0 < 0 || cooldown > COOLDOWN_CAP_MS || category > COOLDOWN_CAP_MS
                {
                    return (false, false);
                }
            }
        }
        let Some(spell) = use_spell.map(|u| u.spell_id).filter(|&id| {
            self.spells
                .as_deref()
                .is_some_and(|s| s.catalog.get(id).is_some())
        }) else {
            return (true, false);
        };
        self.usable_reads(|r| r.spell_usable(spell, false).pair())
            .unwrap_or_default()
    }

    /// The 0-based bar `slot` through the resolvers.
    pub(super) fn resolve_slot(&self, slot: u32) -> Option<Resolved> {
        let bar = self.slots.as_deref()?;
        let Some(button) = u8::try_from(slot).ok().and_then(|s| bar.buttons.get(&s)) else {
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
                    self.bound.as_deref().and_then(|b| b.0.get(&button.action))
                {
                    out.spell = *s;
                }
            }
            k if k == ACTION_KIND_PET => {
                let bar = self.pet_bar.as_deref();
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
            let Some(bar) = self.pet_bar.as_deref() else {
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
            self.selection.as_deref().and_then(|s| s.guid)?
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
            let Some(cooldowns) = self.cooldowns.as_deref() else {
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
                .as_deref()
                .and_then(|b| {
                    b.cooldowns
                        .info(r.spell, 0, d(r.spell), now)
                        .ui_triple(anchor, ui_now)
                })
                .unwrap_or(IDLE);
        }
        benilla_ui::script::LiveState::spell_cooldown(self, r.spell, anchor, ui_now).unwrap_or(IDLE)
    }

    /// The bar slot's word (`[0xc1e358 + slot * 4]`), 0 for an empty one.
    pub(super) fn slot_word(&self, slot: u32) -> Option<u32> {
        let bar = self.slots.as_deref()?;
        Some(
            u8::try_from(slot)
                .ok()
                .and_then(|s| bar.buttons.get(&s))
                .map_or(0, |b| (u32::from(b.kind) << 24) | b.action),
        )
    }

    /// `GetActionTexture`'s icon for a spell, item or pet-action slot (`0x5a97f0`): the spell's
    /// resolver ([`crate::ui_action::slots::spell_icon`]); an item's art, the question mark while
    /// its template or display row is missing; a pet action's spell icon or mode or order global.
    pub(super) fn slot_icon(&self, slot: u32) -> Option<Option<benilla_ui::script::ActionIcon>> {
        use benilla_ui::script::ActionIcon;
        let bar = self.slots.as_deref()?;
        let Some(button) = u8::try_from(slot).ok().and_then(|s| bar.buttons.get(&s)) else {
            return Some(None);
        };
        let containers = self.containers.as_ref()?;
        let (objects, items, commands) = containers.item_reads();
        let icon = match button.kind {
            ACTION_KIND_SPELL => self.spells.as_deref().and_then(|spells| {
                let d = spells.catalog.get(button.action)?;
                let me = self.me();
                crate::ui_action::slots::spell_icon(
                    button.action,
                    d,
                    spells,
                    me.map(|(_, s)| s),
                    me.and_then(|(e, _)| self.auras.get(e).ok()),
                    objects,
                    items,
                    containers.icons(),
                    commands,
                )
                .map(ActionIcon::Path)
            }),
            ACTION_KIND_ITEM => Some(ActionIcon::Path(
                items
                    .template(button.action, 0, commands)
                    .and_then(|t| crate::ui_items::item_icon(containers.icons(), t.display_info_id))
                    .unwrap_or_else(|| crate::ui_action::slots::MISSING_ITEM_ICON.to_string()),
            )),
            k if k == ACTION_KIND_PET => self.pet_bar.as_deref().and_then(|bar| {
                crate::ui_pet::temp_slot_icon(
                    bar,
                    button.action,
                    self.spells.as_deref().map(|s| &s.catalog),
                )
            }),
            _ => None,
        };
        Some(icon)
    }

    /// `GetActionCount` (`0x5a7d10`): an item's count (`0x5a8a60`), else a player spell's reagent
    /// uses (`0x801770`).
    pub(super) fn slot_count(&self, r: Resolved) -> u32 {
        let (Some((_, me)), Some((objects, items, commands))) =
            (self.me(), self.containers.as_ref().map(|c| c.item_reads()))
        else {
            return 0;
        };
        let spells = self.spells.as_deref();
        if r.item != 0 {
            return crate::ui_action::slots::item_count(
                me, objects, items, commands, spells, r.item,
            );
        }
        if r.spell == 0 || r.pet {
            return 0;
        }
        spells.and_then(|s| s.catalog.get(r.spell)).map_or(0, |d| {
            crate::ui_action::slots::reagent_uses(d, |id| {
                crate::ui_items::count_of(
                    &me.0,
                    objects,
                    id,
                    crate::ui_items::InventoryScope::DEFAULT,
                )
            })
        })
    }

    /// `IsConsumableAction` (`0x5a9360`): an item that is ammo, thrown or used up by its charges,
    /// else a player spell that takes reagents.
    pub(super) fn slot_consumable(&self, r: Resolved) -> bool {
        if r.item != 0 {
            let consumable = self
                .containers
                .as_ref()
                .map(|c| c.item_reads())
                .and_then(|(_, items, commands)| items.template(r.item, 0, commands))
                .is_some_and(|t| t.is_consumable());
            if consumable {
                return true;
            }
        }
        !r.pet
            && self
                .spells
                .as_deref()
                .and_then(|s| s.catalog.get(r.spell))
                .is_some_and(|d| d.reagents.iter().any(|&(id, n)| id != 0 && n > 0))
    }

    /// `IsStackableAction` (`0x5a8470`): an item whose template stacks past one.
    pub(super) fn slot_stackable(&self, r: Resolved) -> bool {
        r.item != 0
            && self
                .containers
                .as_ref()
                .map(|c| c.item_reads())
                .and_then(|(_, items, commands)| items.template(r.item, 0, commands))
                .is_some_and(|t| t.stackable > 1)
    }

    /// A pet-action slot's spell (`0x5a80e0`), `None` for a mode, an order or an empty action.
    pub(super) fn slot_pet_spell(&self, r: Resolved) -> Option<u32> {
        r.pet_word
            .filter(|w| !w.is_empty() && w.is_spell())
            .map(|w| w.action())
    }

    /// The slot write (`0x5aae80`, `0x5aaa90` for 0): stored and sent, then the slot's usable bytes
    /// recomputed (`0x5aa390`); `Some(false)` on a locked bar.
    pub(super) fn write_slot_word(&mut self, slot: u32, word: u32) -> Option<bool> {
        let Ok(slot) = u8::try_from(slot) else {
            return Some(false);
        };
        if usize::from(slot) >= crate::ui_action::usable_cache::BAR_SLOTS {
            return Some(false);
        }
        let net = self.commands.as_deref()?;
        let written = self.slots.as_deref_mut()?.write_slot(slot, word, net);
        if written {
            if let Some(fresh) = self.usable_reads(|r| r.slot(slot)) {
                if let Some(caches) = self.usable.as_deref_mut() {
                    caches.slots[usize::from(slot)] = fresh;
                }
            }
        }
        Some(written)
    }
}
