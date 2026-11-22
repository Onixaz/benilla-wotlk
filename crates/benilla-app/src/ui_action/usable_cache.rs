//! The usable caches `IsUsableAction` and `GetShapeshiftFormInfo` read (`0x5a7e60`, `0x53de90`):
//! the bar's usable and no-power bytes per slot, the reactive spells' and the stance bar's, each
//! refreshed only where the reference refreshes it. The full update (`0x53cf10`) walks the bar
//! (`0x5aa470`, `ACTIONBAR_UPDATE_USABLE` on a change), the reactive spells (`SPELL_ACTIVE` combat
//! text on a turn to castable) and the stance bar (`UPDATE_SHAPESHIFT_USABLE` on a change), then
//! signals `SPELL_UPDATE_USABLE`; the power update (`0x53d1b0`), run when the player's or the
//! pet's predicted power moves, re-tests the no-power byte of each usable entry alone.
//!
//! The bar's walk alone runs after every item lock and unlock (`0x51376b`, `0x51379b`), the cursor
//! delete (`0x52244f`) and the equipment manager's sites (`0x5ad8b9`, `0x5ada8a`, `0x5addf7`,
//! `0x5af7be`): a set slot reads the locks of its items and of the slots it fills.

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::{
    ACTION_KIND_EQUIPMENT_SET, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
use benilla_ui::script::ScriptValue;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::spell::{usable, Cooldowns};

use super::{PlayerActions, Spells};

/// `UNIT_FLAG_TAXI_FLIGHT`, which passes a set slot past the control test (`0x5aa03d`).
const UNIT_FLAG_TAXI_FLIGHT: u32 = 0x10_0000;

/// The bar's slots, the length of its usable cache.
pub(crate) const BAR_SLOTS: usize = 144;

/// One cache entry's two bytes: usable by the walk but for power, and short of power.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Usable {
    pub(crate) usable: bool,
    pub(crate) no_power: bool,
}

impl Usable {
    /// The walk's `(usable, not_enough_mana)` pair: a power refusal is a usable entry short of
    /// power.
    fn of_walk((usable, oom): (bool, bool)) -> Self {
        Usable {
            usable: usable || oom,
            no_power: oom,
        }
    }

    /// Castable now: usable with the power.
    pub(crate) fn ready(self) -> bool {
        self.usable && !self.no_power
    }

    /// `IsUsableAction`'s pair: `(1, nil)`, `(nil, 1)` or `(nil, nil)`.
    pub(crate) fn pair(self) -> (bool, bool) {
        (self.ready(), self.usable && self.no_power)
    }
}

/// One spell list entry, `{spell, usable, noPower}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpellUsable {
    pub(crate) spell: u32,
    pub(crate) usable: Usable,
}

/// The caches, which only the update sites write.
#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct UsableCaches {
    /// Per bar slot, 0-based: the bar's usable and no-power bytes.
    pub(crate) slots: [Usable; BAR_SLOTS],
    /// The book's reactive spells (`0x54228c`), one per name.
    pub(crate) reactive: Vec<SpellUsable>,
    /// The stance bar (`[0xbe8e2c]`), in bar order.
    pub(crate) stances: Vec<SpellUsable>,
    /// Whether equipment sets can be used (`0x5ad6b0`: no swap out and `equipmentManager` on), as the site running a walk last read it.
    pub(crate) sets_usable: bool,
}

impl UsableCaches {
    /// Whether the stance bar lists a form (`[0xbe8e28]` nonzero).
    pub(crate) fn lists_forms(&self) -> bool {
        !self.stances.is_empty()
    }
}

impl Default for UsableCaches {
    fn default() -> Self {
        UsableCaches {
            slots: [Usable::default(); BAR_SLOTS],
            reactive: Vec::new(),
            stances: Vec::new(),
            sets_usable: false,
        }
    }
}

/// What an update signals, in its order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UsableEvent {
    BarUsable,
    ReactiveActive(String),
    ShapeshiftUsable,
    SpellUsable,
}

impl UsableEvent {
    /// The event and its arguments.
    pub(crate) fn event(&self) -> (&'static str, Vec<ScriptValue>) {
        match self {
            UsableEvent::BarUsable => ("ACTIONBAR_UPDATE_USABLE", vec![]),
            UsableEvent::ReactiveActive(name) => (
                "COMBAT_TEXT_UPDATE",
                vec![
                    ScriptValue::Str("SPELL_ACTIVE".into()),
                    ScriptValue::Str(name.clone()),
                ],
            ),
            UsableEvent::ShapeshiftUsable => ("UPDATE_SHAPESHIFT_USABLE", vec![]),
            UsableEvent::SpellUsable => ("SPELL_UPDATE_USABLE", vec![]),
        }
    }
}

/// `AttributesExB & 0x2`: never on the stance bar; Ghost Wolf carries it.
const ATTR_EX2_STANCE_BAR_EXCLUDE: u32 = 0x2;
/// `AttributesExB & 0x10`: on the stance bar with no `MOD_SHAPESHIFT` effect (the paladin auras).
const ATTR_EX2_STANCE_BAR_FORCE: u32 = 0x10;
/// `AttributesExD & 0x200`: a reactive spell, whose turn to castable floats its name.
const ATTR_EX4_REACTIVE: u32 = 0x200;
/// `AttributesExB & 0x40000`: a pet-bar spell the player casts (`0x5a80e0`).
const ATTR_EX2_CAST_BY_PLAYER: u32 = 0x0004_0000;

/// The registrar's stance admission (`0x542030`): not excluded, and a `MOD_SHAPESHIFT` effect or
/// the force bit.
pub(crate) fn on_stance_bar(d: &benilla_formats::SpellDisplay) -> bool {
    d.attributes_ex2 & ATTR_EX2_STANCE_BAR_EXCLUDE == 0
        && (d.shapeshift_form.is_some() || d.attributes_ex2 & ATTR_EX2_STANCE_BAR_FORCE != 0)
}

/// The stance bar's order: `StanceBarOrder`, a negative one last, then the spell id.
fn stance_order(d: &benilla_formats::SpellDisplay) -> (i64, u32) {
    let order = if d.stance_bar_order < 0 {
        i64::MAX
    } else {
        i64::from(d.stance_bar_order)
    };
    (order, d.id)
}

/// The book's reactive spells (`0x54228c`), one per name, the highest `spellLevel` kept, in
/// learn order.
pub(crate) fn reactive_spells(
    known: &std::collections::BTreeSet<u32>,
    catalog: &benilla_formats::SpellCatalog,
) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for &id in known {
        let Some(d) = catalog
            .get(id)
            .filter(|d| d.attributes_ex4 & ATTR_EX4_REACTIVE != 0)
        else {
            continue;
        };
        match out
            .iter_mut()
            .find(|o| catalog.get(**o).is_some_and(|e| e.name == d.name))
        {
            Some(o) => {
                if catalog
                    .get(*o)
                    .is_some_and(|e| d.spell_level > e.spell_level)
                {
                    *o = id;
                }
            }
            None => out.push(id),
        }
    }
    out
}

/// The world the walks read, as the update sites see it.
#[derive(SystemParam)]
pub(crate) struct UsableInputs<'w, 's> {
    spells: Option<Res<'w, Spells>>,
    actions: Option<Res<'w, PlayerActions>>,
    slots: Option<Res<'w, crate::ui_action::ActionSlots>>,
    bound: Option<Res<'w, crate::ui_macro::MacroBoundSpells>>,
    pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    cooldowns: Option<Res<'w, Cooldowns>>,
    selection: Option<Res<'w, crate::target::Selection>>,
    me: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    objects: Objects<'w, 's>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    place: Option<Res<'w, crate::area::PlayerPlace>>,
    /// The item locks a set slot reads (`0x5ad940`, `0x5ae040`).
    pending: Option<Res<'w, crate::pending_item_ops::PendingItemOps>>,
    world: UsableWorld<'w>,
    tables: UsableTables<'w>,
}

/// What the walks read beside the player's own state, one parameter: the live view takes it too.
#[derive(SystemParam)]
pub(crate) struct UsableWorld<'w> {
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    /// `Map.dbc`, whose instance type `IsUsableItem`'s arena leg reads (`0x51c538`).
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    combo: Option<Res<'w, crate::spell::ComboPoints>>,
    auto: crate::spell::AutoChecks<'w>,
    /// The client-control flag a slot with no spell reads (`0x5aa063`).
    control: Option<Res<'w, crate::net::ControlFlag>>,
}

/// The tables beside them, one parameter.
#[derive(SystemParam)]
pub(crate) struct UsableTables<'w> {
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    runes: Option<Res<'w, crate::ui_rune::Runes>>,
    rune_costs: Option<Res<'w, crate::ui_rune::RuneCosts>>,
    items: Option<Res<'w, Items>>,
    commands: Option<Res<'w, NetCommands>>,
}

/// Where a walk's reads come from: the update sites' parameters ([`UsableInputs`]) or the view lent
/// to a verb at its call.
pub(crate) struct Sources<'a, 'w, 's> {
    pub(crate) spells: Option<&'a Spells>,
    pub(crate) actions: Option<&'a PlayerActions>,
    pub(crate) slots: Option<&'a crate::ui_action::ActionSlots>,
    pub(crate) bound: Option<&'a crate::ui_macro::MacroBoundSpells>,
    pub(crate) pet_bar: Option<&'a crate::ui_pet::PetBar>,
    pub(crate) cooldowns: Option<&'a Cooldowns>,
    pub(crate) me: Option<(Entity, &'a ObjectStore)>,
    /// The locked target's guid, 0 for none.
    pub(crate) target_guid: u64,
    pub(crate) stores: &'a Query<'w, 's, &'static ObjectStore>,
    pub(crate) objects: &'a Objects<'w, 's>,
    pub(crate) auras: &'a Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    pub(crate) place: Option<&'a crate::area::PlayerPlace>,
    pub(crate) pending: Option<&'a crate::pending_item_ops::PendingItemOps>,
    pub(crate) world: &'a UsableWorld<'w>,
    pub(crate) factions: Option<&'a crate::target::Factions>,
    pub(crate) reputations: Option<&'a crate::net::Reputations>,
    pub(crate) mods: Option<&'a crate::spell::SpellModifiers>,
    pub(crate) runes: Option<&'a crate::ui_rune::Runes>,
    pub(crate) rune_costs: Option<&'a crate::ui_rune::RuneCosts>,
    pub(crate) items: Option<&'a Items>,
    pub(crate) commands: Option<&'a NetCommands>,
}

/// One walk's reads, borrowed for its length.
pub(crate) struct Reads<'a, 'w, 's> {
    spells: &'a Spells,
    actions: &'a PlayerActions,
    slots: &'a crate::ui_action::ActionSlots,
    bound: Option<&'a crate::ui_macro::MacroBoundSpells>,
    pet_bar: Option<&'a crate::ui_pet::PetBar>,
    cooldowns: &'a Cooldowns,
    me: Option<(Entity, &'a ObjectStore)>,
    /// The current target's store, the player's own when we target ourselves (`0x80b5d0`).
    target: Option<(Entity, &'a ObjectStore)>,
    stores: &'a Query<'w, 's, &'static ObjectStore>,
    objects: &'a Objects<'w, 's>,
    auras: &'a Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    factions: Option<&'a crate::target::Factions>,
    reputations: &'a crate::net::Reputations,
    group: Option<&'a crate::ui_party::GroupState>,
    mods: &'a crate::spell::SpellModifiers,
    items: &'a Items,
    commands: &'a NetCommands,
    carried: HashMap<u32, u32>,
    rune_check: Option<usable::RuneCheck<'a>>,
    auto_unshift: bool,
    /// Where the player and the pet stand, as their walks' place legs read it.
    place: (usable::UsablePlace, usable::UsablePlace),
    /// The player's and the pet's combo points on the target.
    combo: (u8, u8),
    pending: Option<&'a crate::pending_item_ops::PendingItemOps>,
    /// Whether we hold the client's control (`0xbcfb8c`), which a slot with no spell needs.
    control: bool,
}

impl UsableWorld<'_> {
    /// Whether the player stands in an arena, `Map.dbc` instance type 4.
    pub(crate) fn in_arena(&self) -> bool {
        const INSTANCE_TYPE_ARENA: u32 = 4;
        let (Some(map), Some(maps)) = (self.map.as_deref(), self.maps.as_deref()) else {
            return false;
        };
        maps.0.instance_type(map.0) == Some(INSTANCE_TYPE_ARENA)
    }
}

impl<'a, 'w, 's> Sources<'a, 'w, 's> {
    /// The reads, `None` without the tables a walk needs.
    pub(crate) fn reads(self) -> Option<Reads<'a, 'w, 's>> {
        let spells = self.spells?;
        let actions = self.actions?;
        let slots = self.slots?;
        let cooldowns = self.cooldowns?;
        let (Some(reputations), Some(mods), Some(items), Some(commands)) =
            (self.reputations, self.mods, self.items, self.commands)
        else {
            return None;
        };
        let me = self.me;
        let target = self
            .objects
            .entity(self.target_guid)
            .and_then(|e| Some((e, self.stores.get(e).ok()?)));
        let carried = me
            .map(|(_, s)| crate::ui_items::carried_counts(&s.0, self.objects))
            .unwrap_or_default();
        let rune_check =
            self.runes
                .zip(self.rune_costs)
                .zip(me)
                .map(|((runes, costs), (_, store))| usable::RuneCheck {
                    state: &runes.state,
                    costs: &costs.0,
                    player: &store.0,
                });
        let auto_unshift = me.is_some_and(|(_, store)| {
            let form = u32::from(store.0.unit_shapeshift_form());
            let row = spells.forms.get(&form);
            self.world.auto.unshift(store, row.map_or(0, |f| f.flags))
        });
        let map = self.world.map.as_deref().map(|m| m.0);
        let knows_cold_weather_flying = actions.spells.contains(&usable::COLD_WEATHER_FLYING);
        let place = (
            usable::UsablePlace::of(self.place, map, Some(knows_cold_weather_flying)),
            usable::UsablePlace::of(self.place, map, None),
        );
        let combo = self.world.combo.as_deref().map_or((0, 0), |c| {
            (
                c.player.on(0, self.target_guid),
                c.pet.on(0, self.target_guid),
            )
        });
        Some(Reads {
            spells,
            actions,
            slots,
            bound: self.bound,
            pet_bar: self.pet_bar,
            cooldowns,
            me,
            target,
            stores: self.stores,
            objects: self.objects,
            auras: self.auras,
            factions: self.factions,
            reputations,
            group: self.world.group.as_deref(),
            mods,
            items,
            commands,
            carried,
            rune_check,
            auto_unshift,
            place,
            combo,
            pending: self.pending,
            control: self.world.control.as_deref().is_none_or(|c| c.0),
        })
    }
}

impl UsableInputs<'_, '_> {
    /// `f` over the reads, `None` without the tables a walk needs.
    fn with_reads<R>(&self, f: impl FnOnce(&Reads<'_, '_, '_>) -> R) -> Option<R> {
        let actions = self.actions.as_deref()?;
        let slots = self.slots.as_deref()?;
        let cooldowns = self.cooldowns.as_deref()?;
        let pet_bar = self.pet_bar.as_deref();
        let bound = self.bound.as_deref();
        let place = self.place.as_deref();
        let pending = self.pending.as_deref();
        let t = &self.tables;
        let reads = Sources {
            spells: self.spells.as_deref(),
            actions: Some(actions),
            slots: Some(slots),
            bound,
            pet_bar,
            cooldowns: Some(cooldowns),
            me: self.me.iter().next(),
            target_guid: self.selection.as_deref().and_then(|s| s.guid).unwrap_or(0),
            stores: &self.stores,
            objects: &self.objects,
            auras: &self.auras,
            place,
            pending,
            world: &self.world,
            factions: t.factions.as_deref(),
            reputations: t.reputations.as_deref(),
            mods: t.mods.as_deref(),
            runes: t.runes.as_deref(),
            rune_costs: t.rune_costs.as_deref(),
            items: t.items.as_deref(),
            commands: t.commands.as_deref(),
        }
        .reads()?;
        Some(f(&reads))
    }
}

impl Reads<'_, '_, '_> {
    /// The player's walk context, against the current target or, with `no_target`, none.
    fn ctx<'b>(&'b self, store: &'b ObjectStore, no_target: bool) -> usable::UsableCtx<'b> {
        let target = self.target.filter(|_| !no_target);
        usable::UsableCtx {
            self_auras: self.me.and_then(|(e, _)| self.auras.get(e).ok()),
            target_auras: target.and_then(|(e, _)| self.auras.get(e).ok()),
            store,
            target_store: target.map(|(_, s)| s),
            factions: self.factions,
            reputations: self.reputations,
            party: crate::target::party_of(self.group),
            cooldowns: self.cooldowns,
            carried: &self.carried,
            spell_mods: self.mods,
            runes: self.rune_check,
            combo_points: self.combo.0,
            auto_unshift: self.auto_unshift,
            place: self.place.0,
        }
    }

    /// `IsUsableSpell`'s walk (`0x541680`): the player's or, with `pet`, the pet's, with no target.
    pub(crate) fn spell_usable(&self, spell: u32, pet: bool) -> Usable {
        if !pet {
            return self.spell(spell);
        }
        match self.spells.catalog.get(spell) {
            Some(d) => self.pet_spell(spell, d, true),
            None => Usable::default(),
        }
    }

    /// A player spell's walk (`0x80b5d0`) with no target, as the book's lists test it.
    fn spell(&self, spell: u32) -> Usable {
        let (Some((_, store)), Some(d)) = (self.me, self.spells.catalog.get(spell)) else {
            return Usable::default();
        };
        let ctx = self.ctx(store, true);
        Usable::of_walk(usable::spell_usable(
            spell,
            d,
            self.spells,
            &ctx,
            self.objects,
            self.items,
            self.commands,
        ))
    }

    /// The power check alone (`0x8017e0`) for the player's spell.
    fn has_power(&self, store: &ObjectStore, d: &benilla_formats::SpellDisplay) -> bool {
        if d.power_type == usable::POWER_RUNE {
            self.rune_check.is_some_and(|r| r.has_runes(d, self.mods))
        } else {
            usable::can_afford(d, store, self.mods)
        }
    }

    /// Whether an item of `set` is locked and not a buyback row (`0x5ad940`), or a slot the set fills holds a locked item (`0x5ae040`).
    fn set_locked(
        &self,
        store: &ObjectStore,
        set: &benilla_protocol::messages::EquipmentSet,
    ) -> bool {
        let locked = |guid: u64| self.pending.is_some_and(|p| p.is_locked(guid));
        let buyback = |guid: u64| (0..12).any(|i| store.0.player_buyback_slot(i) == Some(guid));
        let item = set.items.iter().any(|&guid| {
            guid > 1 && self.objects.object(guid).is_some() && locked(guid) && !buyback(guid)
        });
        let slot = set.items.iter().enumerate().any(|(i, &guid)| {
            guid != 1
                && store
                    .0
                    .player_inv_slot(i as u8)
                    .is_some_and(|worn| worn != 0 && locked(worn))
        });
        item || slot
    }

    /// One bar slot's bytes (`0x5aa3b6`'s compute): through its macro or the pet bar, then the walk; a set's slot with `sets_usable` (`0x5a9fe3`).
    pub(crate) fn slot(&self, slot: u8, sets_usable: bool) -> Usable {
        use crate::ui_macro::BoundSpell;
        let Some(button) = self.slots.buttons.get(&slot) else {
            return Usable::default();
        };
        if button.kind == ACTION_KIND_EQUIPMENT_SET {
            // A usable, saved, unlocked set passes on a taxi flight, else needs our control (`0x5ae5c0`-`0x5aa01e`, `0x5aa03d`, `0x5aa063`).
            let usable = sets_usable
                && self.me.is_some_and(|(_, store)| {
                    self.actions
                        .equipment_sets
                        .iter()
                        .find(|s| s.id == button.action)
                        .is_some_and(|set| !self.set_locked(store, set))
                        && (store.0.unit_flags() & UNIT_FLAG_TAXI_FLIGHT != 0 || self.control)
                });
            return Usable {
                usable,
                no_power: false,
            };
        }
        let (kind, id) = match button.kind {
            ACTION_KIND_MACRO => match self.bound.and_then(|b| b.0.get(&button.action)) {
                Some(BoundSpell::Spell(s)) => (ACTION_KIND_SPELL, *s),
                // A macro that casts nothing needs our control (`0x5aa063`, `0x5aa0c5`).
                Some(BoundSpell::None) => {
                    return Usable {
                        usable: self.control,
                        no_power: false,
                    }
                }
                Some(BoundSpell::Unresolved) | None => return Usable::default(),
            },
            other => (other, button.action),
        };
        let (kind, id) = if kind == super::ACTION_KIND_PET {
            let Some(entry) = self
                .pet_bar
                .and_then(|b| b.spells.bar.get(id as usize).copied())
                .filter(|e| !e.is_empty())
            else {
                return Usable::default();
            };
            if !entry.is_spell() {
                // A mode or an order has no spell: it needs our control (`0x5aa063`, `0x5aa0d6`).
                return Usable {
                    usable: self.control,
                    no_power: false,
                };
            }
            match self.spells.catalog.get(entry.action()) {
                Some(d) if d.attributes_ex2 & ATTR_EX2_CAST_BY_PLAYER != 0 => {
                    (ACTION_KIND_SPELL, entry.action())
                }
                Some(d) => return self.pet_spell(entry.action(), d, false),
                None => return Usable::default(),
            }
        } else {
            (kind, id)
        };
        let Some((_, store)) = self.me else {
            return Usable::default();
        };
        match kind {
            ACTION_KIND_SPELL => {
                let Some(d) = self.spells.catalog.get(id) else {
                    return Usable::default();
                };
                let ctx = self.ctx(store, false);
                Usable::of_walk(usable::spell_usable(
                    id,
                    d,
                    self.spells,
                    &ctx,
                    self.objects,
                    self.items,
                    self.commands,
                ))
            }
            ACTION_KIND_ITEM => {
                let use_spell = self
                    .items
                    .template(id, 0, self.commands)
                    .and_then(|t| t.use_spell);
                // An item with no use spell is a slot with no spell: it needs our control (`0x5aa063`, `0x5aa0a7`).
                if use_spell.is_none() && !self.control {
                    return Usable::default();
                }
                let count = self.carried.get(&id).copied().unwrap_or(0);
                let worn = (0..19).any(|i| {
                    store
                        .0
                        .player_inv_slot(i)
                        .and_then(|g| self.objects.object(g))
                        .and_then(|o| o.object_entry())
                        == Some(id)
                });
                let ctx = self.ctx(store, false);
                Usable::of_walk(usable::item_usable(
                    id,
                    use_spell.as_ref(),
                    count > 0 || worn,
                    &ctx,
                    Some(self.spells),
                    self.objects,
                    self.items,
                    self.commands,
                ))
            }
            _ => Usable::default(),
        }
    }

    /// The pet as caster, its own cooldowns (`0x5a8e40`), against the current target or, with
    /// `no_target`, none; no pet, unusable.
    fn pet_spell(&self, spell: u32, d: &benilla_formats::SpellDisplay, no_target: bool) -> Usable {
        let Some(bar) = self.pet_bar else {
            return Usable::default();
        };
        let Some(pet) = self
            .objects
            .entity(bar.spells.pet_guid)
            .and_then(|e| self.stores.get(e).ok())
        else {
            return Usable::default();
        };
        let target = self.target.filter(|_| !no_target);
        let ctx = usable::UsableCtx {
            self_auras: None,
            target_auras: target.and_then(|(e, _)| self.auras.get(e).ok()),
            store: pet,
            target_store: target.map(|(_, s)| s),
            factions: self.factions,
            reputations: self.reputations,
            party: crate::target::party_of(self.group),
            cooldowns: &bar.cooldowns,
            carried: &HashMap::new(),
            spell_mods: self.mods,
            runes: None,
            place: self.place.1,
            combo_points: self.combo.1,
            auto_unshift: false,
        };
        Usable::of_walk(usable::spell_usable(
            spell,
            d,
            self.spells,
            &ctx,
            self.objects,
            self.items,
            self.commands,
        ))
    }

    /// A slot's spell and caster for the power pass (`0x5a8d30`): the spell, through a macro or the
    /// pet bar, and the pet's store for a pet spell; an item has none.
    fn slot_power(&self, slot: u8) -> Option<bool> {
        use crate::ui_macro::BoundSpell;
        let button = self.slots.buttons.get(&slot)?;
        let spell = match button.kind {
            ACTION_KIND_SPELL => button.action,
            ACTION_KIND_MACRO => match self.bound?.0.get(&button.action)? {
                BoundSpell::Spell(s) => *s,
                _ => return None,
            },
            k if k == super::ACTION_KIND_PET => {
                let bar = self.pet_bar?;
                let entry = bar.spells.bar.get(button.action as usize).copied()?;
                if entry.is_empty() || !entry.is_spell() {
                    return None;
                }
                let d = self.spells.catalog.get(entry.action())?;
                if d.attributes_ex2 & ATTR_EX2_CAST_BY_PLAYER == 0 {
                    let pet = self
                        .objects
                        .entity(bar.spells.pet_guid)
                        .and_then(|e| self.stores.get(e).ok())?;
                    return Some(usable::can_afford(d, pet, self.mods));
                }
                entry.action()
            }
            _ => return None,
        };
        let d = self.spells.catalog.get(spell)?;
        let (_, store) = self.me?;
        Some(self.has_power(store, d))
    }
}

/// The bar's walk (`0x5aa470`): every slot recomputed, `ACTIONBAR_UPDATE_USABLE` on any change.
pub(crate) fn bar_pass(
    r: &Reads<'_, '_, '_>,
    caches: &mut UsableCaches,
    out: &mut Vec<UsableEvent>,
) {
    let mut changed = false;
    let sets_usable = caches.sets_usable;
    for (i, cell) in caches.slots.iter_mut().enumerate() {
        let fresh = r.slot(i as u8, sets_usable);
        changed |= *cell != fresh;
        *cell = fresh;
    }
    if changed {
        out.push(UsableEvent::BarUsable);
    }
}

/// The full update (`0x53cf10`).
fn full_pass(r: &Reads<'_, '_, '_>, caches: &mut UsableCaches) -> Vec<UsableEvent> {
    let mut out = Vec::new();
    bar_pass(r, caches, &mut out);
    if r.me.is_some() {
        for entry in &mut caches.reactive {
            let fresh = r.spell(entry.spell);
            if fresh.ready() && !entry.usable.ready() {
                if let Some(d) = r.spells.catalog.get(entry.spell) {
                    out.push(UsableEvent::ReactiveActive(d.name.clone()));
                }
            }
            entry.usable = fresh;
        }
        let mut changed = false;
        for entry in &mut caches.stances {
            let fresh = r.spell(entry.spell);
            changed |= fresh.ready() != entry.usable.ready();
            entry.usable = fresh;
        }
        if changed {
            out.push(UsableEvent::ShapeshiftUsable);
        }
    }
    out.push(UsableEvent::SpellUsable);
    out
}

/// The power update (`0x53d1b0`): the no-power byte of each usable entry alone.
fn power_pass(r: &Reads<'_, '_, '_>, caches: &mut UsableCaches) -> Vec<UsableEvent> {
    let mut out = Vec::new();
    let mut changed = false;
    for (i, cell) in caches.slots.iter_mut().enumerate() {
        if !cell.usable {
            continue;
        }
        let no_power = r.slot_power(i as u8).is_some_and(|ok| !ok);
        changed |= cell.no_power != no_power;
        cell.no_power = no_power;
    }
    if changed {
        out.push(UsableEvent::BarUsable);
    }
    let Some((_, store)) = r.me else {
        return out;
    };
    for entry in caches.reactive.iter_mut().filter(|e| e.usable.usable) {
        let Some(d) = r.spells.catalog.get(entry.spell) else {
            continue;
        };
        let no_power = !r.has_power(store, d);
        if no_power != entry.usable.no_power && !no_power {
            out.push(UsableEvent::ReactiveActive(d.name.clone()));
        }
        entry.usable.no_power = no_power;
    }
    let mut changed = false;
    for entry in caches.stances.iter_mut().filter(|e| e.usable.usable) {
        let Some(d) = r.spells.catalog.get(entry.spell) else {
            continue;
        };
        let no_power = !r.has_power(store, d);
        changed |= no_power != entry.usable.no_power;
        entry.usable.no_power = no_power;
    }
    if !caches.stances.is_empty() && changed {
        out.push(UsableEvent::ShapeshiftUsable);
    }
    out
}

/// Which update a site runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    /// `0x53cf10`.
    Full,
    /// `0x53d1b0`.
    Power,
    /// The bar alone, `0x5aa470`.
    Bar,
    /// One slot recomputed with no event, as a slot change does (`0x5aa390`).
    Slot(u8),
}

/// `pass` over a copy of `caches`, computed under the reads: the caches it leaves and the events
/// it signals, in order. The copy is written back once the reads end, as an owner's write may not
/// overlap them.
pub(crate) fn run(
    inputs: &UsableInputs,
    caches: &UsableCaches,
    pass: Pass,
) -> Option<(UsableCaches, Vec<UsableEvent>)> {
    let mut caches = caches.clone();
    let events = inputs.with_reads(|r| {
        let caches = &mut caches;
        match pass {
            Pass::Full => full_pass(r, caches),
            Pass::Power => power_pass(r, caches),
            Pass::Bar => {
                let mut out = Vec::new();
                bar_pass(r, caches, &mut out);
                out
            }
            Pass::Slot(slot) => {
                let sets_usable = caches.sets_usable;
                if let Some(cell) = caches.slots.get_mut(usize::from(slot)) {
                    *cell = r.slot(slot, sets_usable);
                }
                Vec::new()
            }
        }
    })?;
    Some((caches, events))
}

/// Run `pass` and write what it leaves; the events it signals, in order.
pub(crate) fn run_into(
    inputs: &UsableInputs,
    caches: &mut ResMut<UsableCaches>,
    pass: Pass,
) -> Vec<UsableEvent> {
    let result = run(inputs, caches.as_ref(), pass);
    let Some((fresh, events)) = result else {
        return Vec::new();
    };
    *caches.as_mut() = fresh;
    events
}

/// A site that runs the update in place, in a system that holds the VM.
#[derive(SystemParam)]
pub(crate) struct UsableRun<'w, 's> {
    inputs: UsableInputs<'w, 's>,
    caches: ResMut<'w, UsableCaches>,
}

impl UsableRun<'_, '_> {
    /// Whether the stance bar lists a form, for the cooldown signals (`0x53bac5`).
    pub(crate) fn lists_forms(&self) -> bool {
        self.caches.lists_forms()
    }

    /// Run `pass` and fire what it signals into `script`, in order.
    pub(crate) fn fire(&mut self, script: &mut benilla_ui::script::UiScript, pass: Pass) {
        let sets_usable = script.can_use_equipment_sets();
        if self.caches.sets_usable != sets_usable {
            self.caches.sets_usable = sets_usable;
        }
        let events = self.run(pass);
        fire_into(script, events);
    }

    /// Run `pass`: what it signals, in order, for a caller that fires with no borrow held.
    pub(crate) fn run(&mut self, pass: Pass) -> Vec<UsableEvent> {
        run_into(&self.inputs, &mut self.caches, pass)
    }
}

/// Fire `events` into a VM a feed holds, in order.
pub(crate) fn fire_into(script: &mut benilla_ui::script::UiScript, events: Vec<UsableEvent>) {
    for e in events {
        let (event, args) = e.event();
        script.fire_event(event, args);
    }
}

/// A handler's site: `pass` runs in turn with the handler's other events, and what it signals
/// fires next.
pub(crate) fn step(pass: Pass) -> impl FnOnce(&mut World) + Send + Sync + 'static {
    move |world: &mut World| {
        // The walk reads whether sets can be used as it runs (`0x5a9fe3`).
        let sets_usable = world
            .get_non_send_resource::<benilla_ui::script::UiScript>()
            .map(|s| s.can_use_equipment_sets());
        if let Err(e) = world.run_system_cached_with(run_step, (pass, sets_usable)) {
            warn!("usable update {pass:?}: {e}");
        }
    }
}

fn run_step(
    In((pass, sets_usable)): In<(Pass, Option<bool>)>,
    inputs: UsableInputs,
    mut caches: ResMut<UsableCaches>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let Some(sets_usable) = sets_usable {
        if caches.sets_usable != sets_usable {
            caches.sets_usable = sets_usable;
        }
    }
    let events = run_into(&inputs, &mut caches, pass);
    for e in events {
        let (event, args) = e.event();
        ui.fire(event, args);
    }
}

/// `SPELL_AURA_ABILITY_IGNORE_AURASTATE` and `SPELL_AURA_MOD_IGNORE_SHAPESHIFT`, whose effects'
/// coming and going run the usable update (`0x724a20`, `0x71ea1e`).
pub(crate) fn moves_usability(d: &benilla_formats::SpellDisplay) -> bool {
    d.effect_apply_aura
        .iter()
        .any(|&a| a == SPELL_AURA_ABILITY_IGNORE_AURASTATE || a == SPELL_AURA_MOD_IGNORE_SHAPESHIFT)
}

const SPELL_AURA_ABILITY_IGNORE_AURASTATE: u32 = 262;
const SPELL_AURA_MOD_IGNORE_SHAPESHIFT: u32 = 275;

/// The aura reader's site (`0x72fbfb`): a table that changed on us, a unit we charm, our charm or
/// creation, the target or the focus, or any read of an arena opponent's, runs the usable update.
pub(crate) fn aura_read(world: &mut World, guid: u64, changed: bool) {
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|g| g.0);
    let focus = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .map(benilla_ui::script::UiScript::focus_guid)
        .filter(|&g| g != 0);
    let target = world
        .get_resource::<crate::target::Selection>()
        .and_then(|s| s.guid);
    let arena = world
        .get_resource::<crate::ui_arena::ArenaOpponents>()
        .is_some_and(|a| a.opponents().contains(&guid));
    let unit = world
        .get_resource::<crate::net::GuidIndex>()
        .and_then(|i| i.0.get(&guid).copied())
        .and_then(|e| world.get::<ObjectStore>(e));
    let ours = |g: Option<u64>| me.is_some() && g.filter(|&g| g != 0) == me;
    let tied = unit.is_some_and(|u| {
        let charmed_by = u.0.unit_charmed_by().filter(|&g| g != 0);
        ours(charmed_by) || ours(charmed_by.or_else(|| u.0.unit_created_by()))
    });
    let watched = Some(guid) == me || tied || Some(guid) == target || Some(guid) == focus;
    if (changed && watched) || arena {
        step(Pass::Full)(world);
    }
}

/// The stance and reactive lists' edits at the registrar, the removal and the clear (`0x542030`,
/// `0x53fad0`, `0x53ebc0`); each list edit signals `UPDATE_SHAPESHIFT_FORMS` once per frame
/// (`0x54252d`, `0x542647`, `0x53fe44`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListEdit {
    /// A learn: a new entry tested at once, or with `replaces` the old rank's entry in place.
    Learn { spell: u32, replaces: Option<u32> },
    /// A removal with notify clear (`0x6e7361`), the plain unlearn; a supersede keeps its entry for
    /// the learn to replace (`0x542eb7`).
    Remove(u32),
    /// The player's init (`0x6e7f50`): the clear (`0x53ebc0`), then each known spell's learn.
    Rebuild,
}

/// The edit's step, in turn with the handler's events.
pub(crate) fn edit_step(edit: ListEdit) -> impl FnOnce(&mut World) + Send + Sync + 'static {
    move |world: &mut World| {
        if let Err(e) = world.run_system_cached_with(run_edit, edit) {
            warn!("usable list edit {edit:?}: {e}");
        }
    }
}

fn run_edit(
    In(edit): In<ListEdit>,
    inputs: UsableInputs,
    mut caches: ResMut<UsableCaches>,
    mut signals: NonSendMut<crate::ui_script::live::UniqueSignals>,
) {
    let mut fresh = caches.clone();
    let signal = inputs
        .with_reads(|r| apply_edit(r, &mut fresh, edit))
        .unwrap_or_else(|| {
            // No tables: the init's clear still clears.
            if edit == ListEdit::Rebuild {
                fresh.stances.clear();
                fresh.reactive.clear();
            }
            false
        });
    *caches.as_mut() = fresh;
    if signal {
        signals.unique_signal("UPDATE_SHAPESHIFT_FORMS");
    }
}

/// Apply `edit`; whether the stance list moved.
fn apply_edit(r: &Reads<'_, '_, '_>, caches: &mut UsableCaches, edit: ListEdit) -> bool {
    // The reactive list follows the book: kept entries keep their bytes, a new one is tested.
    let reactive = reactive_spells(&r.actions.spells, &r.spells.catalog);
    caches.reactive = reactive
        .into_iter()
        .map(|spell| {
            caches
                .reactive
                .iter()
                .find(|e| e.spell == spell)
                .copied()
                .unwrap_or_else(|| SpellUsable {
                    spell,
                    usable: r.spell(spell),
                })
        })
        .collect();
    let stance = |spell: u32| SpellUsable {
        spell,
        usable: r.spell(spell),
    };
    let catalog = &r.spells.catalog;
    let order = |e: &SpellUsable| {
        catalog
            .get(e.spell)
            .map_or((i64::MAX, e.spell), stance_order)
    };
    match edit {
        ListEdit::Rebuild => {
            caches.stances = r
                .actions
                .spells
                .iter()
                .filter(|&&id| catalog.get(id).is_some_and(on_stance_bar))
                .map(|&id| stance(id))
                .collect();
            caches.stances.sort_by_key(order);
            !caches.stances.is_empty()
        }
        ListEdit::Remove(spell) => {
            let before = caches.stances.len();
            caches.stances.retain(|e| e.spell != spell);
            caches.stances.len() != before
        }
        ListEdit::Learn { spell, replaces } => {
            if !catalog.get(spell).is_some_and(on_stance_bar) {
                return false;
            }
            if let Some(old) = replaces {
                return match caches.stances.iter_mut().find(|e| e.spell == old) {
                    Some(entry) => {
                        *entry = stance(spell);
                        true
                    }
                    None => false,
                };
            }
            if caches.stances.iter().any(|e| e.spell == spell) {
                return false;
            }
            caches.stances.push(stance(spell));
            caches.stances.sort_by_key(order);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use benilla_protocol::field::{FIELD_UNIT_AURASTATE, FIELD_UNIT_HEALTH, FIELD_UNIT_POWER1};

    const ME: u64 = 0x10;

    /// A world with our player of `fields`, `known` in the book and `rows` in the catalog.
    fn world(rows: Vec<(u32, SpellDisplay)>, known: &[u32], fields: &[(u16, u32)]) -> World {
        let mut world = World::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        std::mem::forget(rx);
        let mut actions = PlayerActions::default();
        actions.spells.extend(known.iter().copied());
        world.insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(rows.into_iter().collect()),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        });
        world.insert_resource(actions);
        world.init_resource::<crate::ui_action::ActionSlots>();
        world.init_resource::<Cooldowns>();
        world.init_resource::<crate::target::Selection>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<Items>();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<UsableCaches>();
        world.init_resource::<crate::ui_script::live::UiEvents>();
        world.init_non_send_resource::<crate::ui_script::live::UniqueSignals>();
        let me = world
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields)),
            ))
            .id();
        let mut index = crate::net::GuidIndex::default();
        index.0.insert(ME, me);
        world.insert_resource(index);
        world
    }

    /// The events the queue holds, emptied.
    fn fired(world: &mut World) -> Vec<&'static str> {
        let mut ui = world.resource_mut::<crate::ui_script::live::UiEvents>();
        let names = ui.names();
        *ui = Default::default();
        names
    }

    /// The full update (`0x53cf10`): a reactive spell turning castable, here its caster aura state
    /// lighting, sends `SPELL_ACTIVE` once; `SPELL_UPDATE_USABLE` closes every update.
    #[test]
    fn a_reactive_spell_turning_castable_sends_spell_active_once() {
        const REVENGE: u32 = 6572;
        let revenge = SpellDisplay {
            name: "Revenge".into(),
            attributes_ex4: ATTR_EX4_REACTIVE,
            caster_aura_state: 1,
            ..Default::default()
        };
        let fields = |lit: u32| [(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_AURASTATE, lit)];
        let mut world = world(vec![(REVENGE, revenge)], &[REVENGE], &fields(0));
        edit_step(ListEdit::Rebuild)(&mut world);
        let update =
            |world: &mut World, lit| {
                let me = world.resource::<crate::net::GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&fields(lit)),
                ));
                step(Pass::Full)(world);
                fired(world)
            };
        assert_eq!(update(&mut world, 0), ["SPELL_UPDATE_USABLE"]);
        assert_eq!(
            update(&mut world, 1),
            ["COMBAT_TEXT_UPDATE", "SPELL_UPDATE_USABLE"]
        );
        assert_eq!(update(&mut world, 1), ["SPELL_UPDATE_USABLE"], "still lit");
        assert_eq!(update(&mut world, 0), ["SPELL_UPDATE_USABLE"]);
        assert_eq!(
            update(&mut world, 1),
            ["COMBAT_TEXT_UPDATE", "SPELL_UPDATE_USABLE"],
            "lit again"
        );
    }

    /// The registrar's stance list (`0x542030`): admitted by a form or the force bit, never the
    /// exclude bit, ordered by `StanceBarOrder` then id; a supersede replaces in place, a plain
    /// unlearn drops, each edit signalling `UPDATE_SHAPESHIFT_FORMS`.
    #[test]
    fn the_stance_list_follows_the_registrar_and_the_removal() {
        let form = |order: i32, form: u32| SpellDisplay {
            shapeshift_form: Some(form),
            stance_bar_order: order,
            ..Default::default()
        };
        let rows = vec![
            (71, form(1, 18)),
            (2457, form(0, 17)),
            (2458, form(2, 19)),
            (
                465,
                SpellDisplay {
                    attributes_ex2: ATTR_EX2_STANCE_BAR_FORCE,
                    stance_bar_order: -1,
                    ..Default::default()
                },
            ),
            (
                2645,
                SpellDisplay {
                    attributes_ex2: ATTR_EX2_STANCE_BAR_EXCLUDE,
                    shapeshift_form: Some(16),
                    ..Default::default()
                },
            ),
            (133, SpellDisplay::default()),
            (7294, form(1, 18)),
        ];
        let mut world = world(rows, &[133, 465, 2458, 71, 2457, 2645], &[]);
        let list = |world: &World| -> Vec<u32> {
            world
                .resource::<UsableCaches>()
                .stances
                .iter()
                .map(|e| e.spell)
                .collect()
        };
        let signalled = |world: &mut World| {
            let mut signals =
                world.non_send_resource_mut::<crate::ui_script::live::UniqueSignals>();
            let names = signals.names();
            signals.clear();
            names
        };
        edit_step(ListEdit::Rebuild)(&mut world);
        assert_eq!(list(&world), [2457, 71, 2458, 465], "a negative order last");
        assert_eq!(signalled(&mut world), ["UPDATE_SHAPESHIFT_FORMS"]);
        edit_step(ListEdit::Learn {
            spell: 7294,
            replaces: Some(71),
        })(&mut world);
        assert_eq!(list(&world), [2457, 7294, 2458, 465], "replaced in place");
        assert_eq!(signalled(&mut world), ["UPDATE_SHAPESHIFT_FORMS"]);
        edit_step(ListEdit::Remove(2458))(&mut world);
        assert_eq!(list(&world), [2457, 7294, 465]);
        assert_eq!(signalled(&mut world), ["UPDATE_SHAPESHIFT_FORMS"]);
        edit_step(ListEdit::Remove(133))(&mut world);
        assert!(signalled(&mut world).is_empty(), "no entry, no signal");
    }

    /// The power update (`0x53d1b0`): a usable slot's no-power byte follows the pool, signalling
    /// `ACTIONBAR_UPDATE_USABLE` only on a change and never `SPELL_UPDATE_USABLE`.
    #[test]
    fn the_power_update_moves_only_the_no_power_byte() {
        const FIREBALL: u32 = 133;
        let fireball = SpellDisplay {
            mana_cost: 30,
            ..Default::default()
        };
        let fields = |mana: u32| {
            [
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_POWER1, mana),
                (benilla_protocol::field::FIELD_UNIT_MAXPOWER1, 100),
            ]
        };
        let mut world = world(vec![(FIREBALL, fireball)], &[FIREBALL], &fields(100));
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                benilla_protocol::messages::ActionButton {
                    slot: 0,
                    action: FIREBALL,
                    kind: ACTION_KIND_SPELL,
                },
            );
        step(Pass::Full)(&mut world);
        assert_eq!(
            fired(&mut world),
            ["ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE"]
        );
        let pair = |world: &World| world.resource::<UsableCaches>().slots[0].pair();
        assert_eq!(pair(&world), (true, false));
        let power =
            |world: &mut World, mana| {
                let me = world.resource::<crate::net::GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&fields(mana)),
                ));
                step(Pass::Power)(world);
                fired(world)
            };
        assert_eq!(power(&mut world, 10), ["ACTIONBAR_UPDATE_USABLE"]);
        assert_eq!(pair(&world), (false, true), "usable, short of mana");
        assert!(power(&mut world, 20).is_empty(), "still short");
        assert_eq!(power(&mut world, 50), ["ACTIONBAR_UPDATE_USABLE"]);
        assert_eq!(pair(&world), (true, false));
    }

    /// A spell-less slot needs our control (`0x5aa063`): a set (taxi passes it, `0x5aa03d`), a macro, an item (`0x5aa0a7`).
    #[test]
    fn the_spell_less_slots_read_our_control() {
        const CLOTH: u32 = 2589;
        const CLOTH_GUID: u64 = 0x4000_0000_0000_0042;
        let mut world = world(Vec::new(), &[], &[(FIELD_UNIT_HEALTH, 100)]);
        world.resource_mut::<PlayerActions>().equipment_sets.push(
            benilla_protocol::messages::EquipmentSet {
                guid: 1,
                id: 3,
                name: "Tank".into(),
                icon: "INV_Shield_06".into(),
                items: [1; benilla_protocol::messages::EQUIPMENT_SET_SLOTS],
            },
        );
        let mut bound = crate::ui_macro::MacroBoundSpells::default();
        bound.0.insert(5, crate::ui_macro::BoundSpell::None);
        world.insert_resource(bound);
        {
            let mut bar = world.resource_mut::<crate::ui_action::ActionSlots>();
            for (slot, action, kind) in [
                (0, 3, ACTION_KIND_EQUIPMENT_SET),
                (1, 5, ACTION_KIND_MACRO),
                (2, CLOTH, ACTION_KIND_ITEM),
            ] {
                bar.buttons.insert(
                    slot,
                    benilla_protocol::messages::ActionButton { slot, action, kind },
                );
            }
        }
        world.resource_mut::<UsableCaches>().sets_usable = true;
        world
            .resource_mut::<Items>()
            .insert_template(CLOTH, Some(crate::items::test_template("Linen Cloth")));
        crate::items::test_spawn_item(
            &mut world,
            CLOTH_GUID,
            benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_OBJECT_ENTRY,
                CLOTH,
            )]),
            false,
        );
        let usable = |world: &mut World, control: bool, flags: u32| {
            world.insert_resource(crate::net::ControlFlag(control));
            let me = world.resource::<crate::net::GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (FIELD_UNIT_HEALTH, 100),
                    (benilla_protocol::field::FIELD_UNIT_FLAGS, flags),
                    (
                        benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD,
                        CLOTH_GUID as u32,
                    ),
                    (
                        benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD + 1,
                        (CLOTH_GUID >> 32) as u32,
                    ),
                ])));
            step(Pass::Full)(world);
            fired(world);
            let slots = &world.resource::<UsableCaches>().slots;
            (slots[0].usable, slots[1].usable, slots[2].usable)
        };
        assert_eq!(usable(&mut world, true, 0), (true, true, true));
        assert_eq!(
            usable(&mut world, false, 0),
            (false, false, false),
            "no control"
        );
        assert_eq!(
            usable(&mut world, false, UNIT_FLAG_TAXI_FLIGHT),
            (true, false, false),
            "a taxi flight passes the set alone"
        );
    }

    /// `IsUsableAction`'s pair off the two bytes (`0x5a7e60`).
    #[test]
    fn the_pair_reads_usable_then_no_power() {
        let pair = |usable, no_power| Usable { usable, no_power }.pair();
        assert_eq!(pair(true, false), (true, false));
        assert_eq!(pair(true, true), (false, true));
        assert_eq!(pair(false, false), (false, false));
        assert_eq!(
            pair(false, true),
            (false, false),
            "an unusable slot is nil, nil"
        );
        assert_eq!(
            Usable::of_walk((false, true)),
            Usable {
                usable: true,
                no_power: true
            },
            "the walk's power refusal is a usable entry short of power"
        );
    }

    /// The reactive list off the real `Spell.dbc` (`0x54228c`): Overpower and Revenge carry
    /// `AttributesExD & 0x200`, Fireball does not, and two Revenge ranks keep the higher one.
    #[test]
    fn the_reactive_list_keeps_one_flagged_spell_per_name() {
        let Some(spells) = crate::ui_action::real_spells() else {
            return;
        };
        const OVERPOWER: u32 = 7384;
        const REVENGE_1: u32 = 6572;
        const REVENGE_9: u32 = 57823;
        const FIREBALL: u32 = 133;
        let known = [FIREBALL, REVENGE_1, OVERPOWER, REVENGE_9]
            .into_iter()
            .collect();
        let list = reactive_spells(&known, &spells.catalog);
        assert_eq!(list.len(), 2, "{list:?}");
        assert!(list.contains(&OVERPOWER));
        assert!(list.contains(&REVENGE_9), "the higher rank: {list:?}");
    }
}
