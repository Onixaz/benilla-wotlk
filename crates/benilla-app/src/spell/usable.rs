//! The usable walk, the reference's usability check: the ordered gates behind `IsUsableAction`'s
//! grey tint. Any tripped gate answers `(usable, oom) = (false, false)`; only the power leg, the
//! last, sets `notEnoughMana`.
//!
//! Built: the TRADE_SKILL early-out, dead (leg 1), reagents and totems (3), the equipped item (4),
//! combo points (5), shapeshift form (6), only-stealthed (7), not-in-combat (8), CasterAuraState
//! (9), TargetAuraState and its attack-or-assist fork (10/10b, the only legs that read the current
//! target), the bit-25 cooldown fold (11) and power (12).
//!
//! Not built, each answering usable: leg 2's caster aura-immunity helpers, leg 4's `AttributesEx3`
//! sub-conditions. The assist check in leg 10b is a reaction-rank test (friendly or better), not
//! the reference's.

use benilla_protocol::messages::ItemUseSpell;

use benilla_formats::{
    SpellDisplay, ATTR_CASTABLE_WHILE_DEAD, ATTR_NOT_IN_COMBAT, ATTR_ONLY_STEALTHED,
    SPELL_EFFECT_TRADE_SKILL,
};

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, Reputations};
use crate::spell::Cooldowns;
use crate::spell::{SpellModifiers, OP_COST};
use crate::target::{can_attack, ring_reaction, Factions};

use crate::ui_action::Spells;

/// Leg 8's caster unit flag.
use crate::player::UNIT_FLAG_IN_COMBAT;

/// `Attributes` bit 19: the cost of a non-player caster scales by level (`0x8014da`).
const ATTR_LEVEL_SCALED_COST: u32 = 0x0008_0000;
/// `AttributesEx4` bit 10: the cost adds the weapon term (`0x801371`).
const ATTR_EX4_WEAPON_SPEED_COST: u32 = 0x0000_0400;
/// The weapon term's speed with no usable weapon on an armed hand (`0x801445`).
const UNARMED_COST_SPEED: u32 = 2000;
/// The item flag that keeps a broken item usable (`0x6d71ab`).
const ITEM_FLAG_BREAK_IGNORED: u32 = 0x0000_0008;
/// The unit's second-word flag that disarms the off hand alone (`0x719038`).
const UNIT_FLAG2_DISARM_OFFHAND: u32 = 0x0000_0080;

/// The implicit targets leg 10b forks on: 6, single enemy, asks the attack check; 21, single
/// friend, asks the assist check.
const IMPLICIT_TARGET_ENEMY: u32 = 6;
const IMPLICIT_TARGET_FRIEND: u32 = 21;

/// Everything the walk reads besides the spell. `target_store` is the current target's (not an
/// explicit cast target).
pub(crate) struct UsableCtx<'a> {
    pub(crate) store: &'a ObjectStore,
    pub(crate) target_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
    /// The group, for the reaction's party rung.
    pub(crate) party: &'a dyn crate::target::GroupMembers,
    pub(crate) cooldowns: &'a Cooldowns,
    /// The talent spell-modifier tables leg 12's cost goes through.
    pub(crate) spell_mods: &'a SpellModifiers,
    /// Every carried entry's count for the frame ([`crate::ui_items::carried_counts`]).
    pub(crate) carried: &'a std::collections::HashMap<u32, u32>,
    /// The rune table and costs leg 12 reads for a rune spell; `None` when the caster is not the
    /// active player, which the power check refuses (`0x80182d`).
    pub(crate) runes: Option<RuneCheck<'a>>,
    /// The caster's combo points on the current target: [`super::ComboPoints`]' player bank, or
    /// the pet's for the pet (`GetComboPoints(target)` with the target 0, `0x513920`, `0x5d3520`).
    pub(crate) combo_points: u8,
}

/// The player's runes and `SpellRuneCost.dbc`, for [`RuneCheck::has_runes`].
#[derive(Clone, Copy)]
pub(crate) struct RuneCheck<'a> {
    pub(crate) state: &'a benilla_ui::script::RuneState,
    pub(crate) costs: &'a benilla_formats::SpellRuneCostCatalog,
    /// The active player, whose school step the cost percentage takes.
    pub(crate) player: &'a benilla_protocol::ObjectFields,
}

/// `POWER_RUNE`, the power type the power check sends to the rune check (`0x8017fe`).
pub(crate) const POWER_RUNE: u32 = 5;

/// The rune kinds `SpellRuneCost.dbc` charges, then death, which stands in for any (`0x5ee786`).
const RUNE_BLOOD: u8 = 0;
const RUNE_UNHOLY: u8 = 1;
const RUNE_FROST: u8 = 2;
const RUNE_DEATH: u8 = 3;

impl RuneCheck<'_> {
    /// The rune check (`0x5ee5b0`): the school step and spell mod take 100, then each kind's runes.
    pub(crate) fn has_runes(&self, d: &SpellDisplay, mods: &SpellModifiers) -> bool {
        self.shortfall(d, mods).0
    }

    /// [`Self::has_runes`] with its out-parameter: each kind's shortfall (blood, unholy, frost),
    /// then the death runes still missing once ready death runes are counted.
    fn shortfall(&self, d: &SpellDisplay, mods: &SpellModifiers) -> (bool, [i64; 4]) {
        let mut needed = [0i64; 4];
        let pct = mods.apply(d, OP_COST, school_cost(self.player, d, 100));
        if pct == 0 || d.power_type != POWER_RUNE {
            return (true, needed);
        }
        let Some(cost) = self.costs.get(d.rune_cost_id) else {
            return (true, needed);
        };
        let mut missing = 0i64;
        for (kind, need) in [
            (RUNE_BLOOD, cost.blood),
            (RUNE_UNHOLY, cost.unholy),
            (RUNE_FROST, cost.frost),
        ] {
            if need == 0 {
                continue;
            }
            let need = if pct > 100 {
                i64::from(pct) * i64::from(need) / 100
            } else {
                i64::from(need)
            };
            let have = i64::from(self.state.ready_of_kind(kind));
            if have < need {
                missing += need - have;
                needed[usize::from(kind)] = need - have;
            }
        }
        let death = i64::from(self.state.ready_of_kind(RUNE_DEATH));
        if missing == 0 || death >= missing {
            return (true, needed);
        }
        needed[usize::from(RUNE_DEATH)] = missing - death;
        (false, needed)
    }

    /// The rune cooldown (`0x5ee7e0`): when a rune spell lacks runes, the ready time on the
    /// `GetTime` millisecond clock of the last kind it waits for. Each short kind waits for its
    /// first rune back, or its last when two are short; a recharging death rune that comes back
    /// sooner stands in for the latest rune (`0x5ee8c0`-`0x5ee958`).
    pub(crate) fn rune_ready_ms(&self, d: &SpellDisplay, mods: &SpellModifiers) -> Option<u32> {
        self.costs.get(d.rune_cost_id)?;
        let (has, needed) = self.shortfall(d, mods);
        if has {
            return None;
        }
        let st = self.state;
        let not_ready = |i: usize| st.ready & (1 << i) == 0;
        // The rune's recharge time (`0x5ee1f0`): 0 for a ready rune.
        let recharge = |i: usize| if not_ready(i) { st.recharge_ms[i] } else { 0 };
        let later = |t: u32, than: u32| than == 0 || t.wrapping_sub(than) as i32 >= 0;
        let mut times = [0u32; benilla_ui::script::MAX_RUNES];
        let (mut latest, mut latest_at) = (0u32, 0usize);
        for (i, time) in times.iter_mut().enumerate() {
            let kind = usize::from(st.kinds[i]);
            if kind != usize::from(RUNE_DEATH) && needed.get(kind).is_some_and(|&n| n != 0) {
                *time = recharge(i);
                if later(*time, latest) {
                    (latest, latest_at) = (*time, i);
                }
            }
        }
        for i in 0..times.len() {
            if st.kinds[i] != RUNE_DEATH || !not_ready(i) || st.recharge_ms[i] == 0 {
                continue;
            }
            let t = recharge(i);
            if latest == 0 {
                (latest, latest_at) = (t, i);
                times[i] = t;
            } else if t < latest {
                times[latest_at] = t;
                (latest, latest_at) = (0, 0);
                for (j, &time) in times.iter().enumerate() {
                    if st.kinds[j] != RUNE_DEATH && later(time, latest) {
                        (latest, latest_at) = (time, j);
                    }
                }
            }
        }
        // One short: the pair's sooner nonzero time; two short: the later (`0x5ee966`-`0x5ee9ff`).
        let pair = |a: u32, b: u32, need: i64| match need {
            1 if a == 0 => b,
            1 if b == 0 || a < b => a,
            1 => b,
            2 => a.max(b),
            _ => 0,
        };
        let blood = pair(times[0], times[1], needed[0]);
        let unholy = pair(times[2], times[3], needed[1]);
        let frost = pair(times[4], times[5], needed[2]);
        Some(blood.max(unholy).max(frost))
    }
}

/// The search covers equipment indices `0..=22` (the reference's `cmp ebx,0x17; jl`): the 19 worn
/// slots and the four equipped bags.
pub(crate) const EQUIPMENT_SLOTS: u8 = 23;

/// `AttributesEx3`'s hand restrictions, the source of the search's slot mask: `0x400` main hand
/// only (mask `0x8000`), `0x1000000` off hand only (mask `0x10000`).
const ATTR_EX3_MAIN_HAND_ONLY: u32 = 0x0000_0400;
const ATTR_EX3_OFF_HAND_ONLY: u32 = 0x0100_0000;

/// `ITEM_FLAG_DEPRECATED` (vmangos `ItemPrototype.h`): a worn item carrying it never satisfies
/// the search.
const ITEM_FLAG_DEPRECATED: u32 = 0x0000_0010;

/// `TARGET_FLAG_ITEM` (`Targets`, column 13): the `EquippedItem*` columns describe the clicked
/// item, not the caster's gear, so the search passes.
const TARGET_FLAG_ITEM: u32 = 0x0000_0010;

/// The search's slot mask from `AttributesEx3`: bit 10 index 15 alone, else bit 24 index 16 alone,
/// else every slot.
fn hand_mask(d: &SpellDisplay) -> u32 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_MAINHAND
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        1 << crate::items::EQUIPMENT_SLOT_OFFHAND
    } else {
        u32::MAX
    }
}

/// Whether the equipped-item search runs at all for `d`: the reference's short-circuits to "fits"
/// are `EquippedItemClass < 0`, a zero subclass mask (no item required, not a wildcard), and an
/// item-targeting spell.
fn needs_equipped_item(d: &SpellDisplay) -> bool {
    d.equipped_item_class >= 0
        && d.equipped_item_subclass_mask != 0
        && d.targets & TARGET_FLAG_ITEM == 0
}

/// The equipment slots [`equipped_item_fits`] reads for `d`, a bit each: none unless it searches,
/// else the slots its hand restriction leaves and both hands, which the disarm ladder reads
/// whatever the restriction.
pub(crate) fn worn_slots_read(d: &SpellDisplay) -> u32 {
    if needs_equipped_item(d) {
        (hand_mask(d) | HANDS) & EQUIPMENT_MASK
    } else {
        0
    }
}

/// The main and off hand, the two slots the disarm ladder tells apart.
const HANDS: u32 =
    1 << crate::items::EQUIPMENT_SLOT_MAINHAND | 1 << crate::items::EQUIPMENT_SLOT_OFFHAND;

/// Every equipment index the search covers, as a mask: the 19 worn slots and the four bags.
pub(crate) const EQUIPMENT_MASK: u32 = (1 << EQUIPMENT_SLOTS) - 1;

/// [`equipped_item_fits`] for the cast ladder's rung 7: the same search, never querying a
/// missing template; an uncached one counts as a match.
pub(crate) fn equipped_item_fits_cached(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
) -> bool {
    if !needs_equipped_item(d) {
        return true;
    }
    let mut mask = hand_mask(d);
    if let Some(hidden) = crate::items::disarmed_equipment_slot_cached(store, objects, items) {
        mask &= !(1u32 << hidden);
    }
    equipped_slots_match(
        store,
        mask,
        d.equipped_item_class as u32,
        d.equipped_item_subclass_mask,
        |guid| worn_item_cached(guid, objects, items),
    )
}

/// The worn item `guid` names from the cached records alone: `None` while its object or its
/// template has not streamed.
fn worn_item_cached(guid: u64, objects: &Objects, items: &Items) -> Option<WornItem> {
    let obj = objects.object(guid)?;
    let t = items.template_cached(obj.object_entry()?)?;
    Some(WornItem {
        class: t.class,
        subclass: t.subclass,
        flags: t.flags,
        durability: obj.item_durability(),
        max_durability: obj.item_max_durability(),
    })
}

/// What the equipped-item search learns of one equipment slot, and all it can: the feed compares
/// these frame to frame, so a view built on the search is rebuilt when a slot's answer can move,
/// and not on a durability tick that leaves an item unbroken.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum SlotItem {
    /// No guid: the search passes over the slot.
    Empty,
    /// A guid whose item object or template has not streamed: the search gives the benefit of the
    /// doubt.
    Unresolved,
    Item {
        class: u32,
        subclass: u32,
        /// `ITEM_FLAG_DEPRECATED`.
        deprecated: bool,
        /// `MaxDurability > 0 && Durability == 0`.
        broken: bool,
    },
}

impl SlotItem {
    fn of(guid: u64, worn: impl FnOnce(u64) -> Option<WornItem>) -> SlotItem {
        if guid == 0 {
            return SlotItem::Empty;
        }
        let Some(it) = worn(guid) else {
            return SlotItem::Unresolved;
        };
        SlotItem::Item {
            class: it.class,
            subclass: it.subclass,
            deprecated: it.flags & ITEM_FLAG_DEPRECATED != 0,
            broken: it.max_durability.is_some_and(|m| m > 0) && it.durability == Some(0),
        }
    }

    /// The search's verdict on the slot for a requirement.
    fn fits(self, class: u32, subclass_mask: u32) -> bool {
        match self {
            SlotItem::Empty => false,
            SlotItem::Unresolved => true, // unresolved template: benefit of the doubt
            // The reference's two rejects before the class match: deprecated, and broken.
            SlotItem::Item {
                deprecated: true, ..
            }
            | SlotItem::Item { broken: true, .. } => false,
            SlotItem::Item {
                class: c,
                subclass: sub,
                ..
            } => c == class && subclass_mask & (1 << sub) != 0,
        }
    }
}

/// Equipment slot `slot`'s [`SlotItem`] from the cached records, without an ask.
pub(crate) fn slot_item_cached(
    store: &ObjectStore,
    slot: u8,
    objects: &Objects,
    items: &Items,
) -> SlotItem {
    SlotItem::of(store.0.player_inv_slot(slot).unwrap_or(0), |guid| {
        worn_item_cached(guid, objects, items)
    })
}

/// The equipped-item refusal reason, keyed on `AttributesEx3` alone: main-hand-only `0x1a`,
/// off-hand-only `0x1b`, else `0x19`. All three render "Must have a %s equipped" with the item
/// subclass name.
pub(crate) fn equipped_item_reason(d: &SpellDisplay) -> u8 {
    if d.attributes_ex3 & ATTR_EX3_MAIN_HAND_ONLY != 0 {
        0x1a
    } else if d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0 {
        0x1b
    } else {
        0x19
    }
}

/// Leg 4, shared with the tooltip's requirement line: does a worn item match `EquippedItemClass`
/// and `EquippedItemSubClassMask`? An item whose template has not streamed counts as a match.
pub(crate) fn equipped_item_fits(
    d: &SpellDisplay,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> bool {
    // The short-circuits to "fits": a caster that is not the active player (always ours here), and
    // those of [`needs_equipped_item`].
    if !needs_equipped_item(d) {
        return true;
    }
    let class = d.equipped_item_class as u32;
    let mut mask = hand_mask(d);
    // The disarm ladder strips the one hidden hand from the mask.
    if let Some(hidden) = crate::items::disarmed_equipment_slot(store, objects, items, commands) {
        mask &= !(1u32 << hidden);
    }
    equipped_slots_match(store, mask, class, d.equipped_item_subclass_mask, |guid| {
        let (entry, durability, max_durability) = objects.object(guid).and_then(|o| {
            Some((
                o.object_entry()?,
                o.item_durability(),
                o.item_max_durability(),
            ))
        })?;
        let t = items.template(entry, guid, commands)?;
        Some(WornItem {
            class: t.class,
            subclass: t.subclass,
            flags: t.flags,
            durability,
            max_durability,
        })
    })
}

/// One worn item, as the equipped-item search reads it.
pub(crate) struct WornItem {
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    /// The template flags.
    pub(crate) flags: u32,
    /// The instance durability pair (`[item+0x114]+0xa0`/`+0xa4`), not the template's.
    pub(crate) durability: Option<u32>,
    pub(crate) max_durability: Option<u32>,
}

/// The search body over a caller-supplied resolver; `None` from it (template not landed) counts
/// as a match.
fn equipped_slots_match(
    store: &ObjectStore,
    mask: u32,
    class: u32,
    subclass_mask: u32,
    mut worn: impl FnMut(u64) -> Option<WornItem>,
) -> bool {
    (0..EQUIPMENT_SLOTS)
        .filter(|slot| mask & (1u32 << slot) != 0)
        .any(|slot| {
            let guid = store.0.player_inv_slot(slot).unwrap_or(0);
            SlotItem::of(guid, &mut worn).fits(class, subclass_mask)
        })
}

/// The item arm of the usable compute, gate by gate:
///
/// 1. a held count of 0 answers `(false, false)`. `held` is carried copies
///    ([`crate::ui_items::InventoryScope::CARRIED`]) or a worn copy. The reference's count (mask
///    `0x47`) also takes the keyring and sums spell charges for a charged item; ours does neither,
///    so a key reads count 0 and a spent charged copy stays lit.
/// 2. The item cooldown test answers `(false, false)`. It asks with the item's first on-use spell
///    and the item entry as the key: an on-hold-record test, so a running cooldown or the GCD never
///    greys an item.
/// 3. the item's first on-use spell runs the whole plain-spell walk, [`spell_usable`]. This is
///    where food greys in combat: its rows carry [`ATTR_NOT_IN_COMBAT`], leg 8.
/// 4. an item with no on-use spell, or no template yet, is usable.
///
/// An on-use spell with no catalog row reads grey with `notEnoughMana = 0`.
pub(crate) fn item_usable(
    entry: u32,
    use_spell: Option<&ItemUseSpell>,
    held: bool,
    ctx: &UsableCtx,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> (bool, bool) {
    if !held {
        return (false, false);
    }
    // Gate 4: no on-use spell, or no template yet.
    let Some(use_spell) = use_spell else {
        return (true, false);
    };
    let spell_id = use_spell.spell_id;
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    // Gate 2: the item entry as the reference's `itemId`, and the item's own category where it has
    // one (the `spellcategory[5]` override).
    let category = match use_spell.category {
        0 => d.map_or(0, |d| d.category),
        c => c,
    };
    if ctx.cooldowns.has_on_hold_record(spell_id, entry, category) {
        return (false, false);
    }
    let (Some(d), Some(spells)) = (d, spells) else {
        return (false, false);
    };
    spell_usable(spell_id, d, spells, ctx, objects, items, commands)
}

/// The walk; returns the `IsUsableAction` pair `(usable, not_enough_mana)`.
pub(crate) fn spell_usable(
    spell_id: u32,
    d: &SpellDisplay,
    spells: &Spells,
    ctx: &UsableCtx,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> (bool, bool) {
    // Early-out: a tradeskill "spell" is always usable.
    if d.effects[0] == SPELL_EFFECT_TRADE_SKILL {
        return (true, false);
    }
    // Leg 1: a dead or ghost caster needs the castable-while-dead attribute.
    if ctx.store.0.is_dead_or_ghost() && d.attributes & ATTR_CASTABLE_WHILE_DEAD == 0 {
        return (false, false);
    }
    // Leg 3: every reagent count carried, every totem tool present.
    let carried = |entry: u32| ctx.carried.get(&entry).copied().unwrap_or(0);
    for &(entry, count) in &d.reagents {
        if entry != 0 && carried(entry) < count {
            return (false, false);
        }
    }
    for &totem in &d.totems {
        if totem != 0 && carried(totem) == 0 {
            return (false, false);
        }
    }
    // Leg 4: some worn item matches the class and subclass mask.
    if !equipped_item_fits(d, ctx.store, objects, items, commands) {
        return (false, false);
    }
    // Leg 5 (`0x80b5d0`): a finishing move (`AttributesEx & 0x500000`, one any-of test) fails while
    // the caster holds no combo points on the current target, so a point banked on another mob
    // greys the button. No class gate, unlike Lua `GetComboPoints`: a warrior's Overpower greys
    // here. The player's aura-262 exemption is not built.
    if d.needs_combo_points() && ctx.combo_points == 0 {
        return (false, false);
    }
    // Leg 6, the form check (`0x72bac0`): the form's `SpellShapeshiftForm.dbc` flags decide whether
    // it counts as shapeshifted and whether it refuses every unnamed spell.
    let form = ctx.store.0.unit_shapeshift_form();
    let form_flags = spells.forms.get(&u32::from(form)).map_or(0, |f| f.flags);
    if !d.usable_in_form(form, form_flags) {
        return (false, false);
    }
    // Leg 7: only-stealthed spells need the CREEP vis flag.
    if d.attributes & ATTR_ONLY_STEALTHED != 0 && !ctx.store.0.unit_is_stealthed() {
        return (false, false);
    }
    // Leg 8: not-in-combat spells grey under `UNIT_FLAG_IN_COMBAT`.
    if d.attributes & ATTR_NOT_IN_COMBAT != 0 && ctx.store.0.unit_flags() & UNIT_FLAG_IN_COMBAT != 0
    {
        return (false, false);
    }
    // Leg 9: the caster's own aura state.
    if d.caster_aura_state != 0
        && ctx.store.0.unit_aura_state() & (1 << (d.caster_aura_state - 1)) == 0
    {
        return (false, false);
    }
    // Legs 10/10b: no current target is unusable; then the aura-state bit; then the relation fork.
    if d.target_aura_state != 0 {
        let Some(target) = ctx.target_store else {
            return (false, false);
        };
        if target.0.unit_aura_state() & (1 << (d.target_aura_state - 1)) == 0 {
            return (false, false);
        }
        match d.implicit_target_a1 {
            // The attack check.
            IMPLICIT_TARGET_ENEMY
                if !can_attack(
                    Some(target),
                    ctx.factions,
                    ctx.reputations,
                    ctx.party,
                    Some(ctx.store),
                ) =>
            {
                return (false, false);
            }
            // Reaction friendly or better, standing in for the assist check.
            IMPLICIT_TARGET_FRIEND
                if ring_reaction(
                    ctx.factions,
                    ctx.reputations,
                    ctx.party,
                    Some(target),
                    Some(ctx.store),
                ) < 4 =>
            {
                return (false, false);
            }
            _ => {}
        }
    }
    // Leg 11: only a cooldown-on-event spell greys, while its record is on hold; a running cooldown
    // never greys here.
    if d.cooldown_on_event() && ctx.cooldowns.has_on_hold_record(spell_id, 0, d.category) {
        return (false, false);
    }
    // Leg 12: power, the only notEnoughMana writer. A rune spell asks the rune table instead of a
    // pool (the power check, `0x801826`).
    let affordable = if d.power_type == POWER_RUNE {
        ctx.runes.is_some_and(|r| r.has_runes(d, ctx.spell_mods))
    } else {
        can_afford(d, ctx.store, ctx.spell_mods)
    };
    if !affordable {
        return (false, true);
    }
    (true, false)
}

/// The power-cost lookup (`0x8012f0`) at the caster's own level, the one cost for all its callers.
pub(crate) fn power_cost(d: &SpellDisplay, store: &ObjectStore, mods: &SpellModifiers) -> u32 {
    power_cost_at(d, &store.0, store.0.unit_level().unwrap_or(0), mods)
}

/// The spell's `fint`: `fistp` under the default FPU state, rounding half to even, not truncating.
fn fint(value: f32) -> i32 {
    value.round_ties_even() as i32
}

/// The caster's school step (`0x80145a`-`0x80148b`, `0x5ee61b`-`0x5ee641`) on a cost.
fn school_cost(unit: &benilla_protocol::ObjectFields, d: &SpellDisplay, cost: i32) -> i32 {
    let modified = cost.wrapping_add(unit.unit_power_cost_modifier(d.school_mask)) as f32;
    fint((unit.unit_power_cost_multiplier(d.school_mask) * f64::from(modified)) as f32)
}

/// The weapon term (`0x801371`-`0x801457`): the active player's hand speed / 100.
fn weapon_cost(
    d: &SpellDisplay,
    unit: &benilla_protocol::ObjectFields,
    mods: &SpellModifiers,
) -> u32 {
    if d.attributes_ex4 & ATTR_EX4_WEAPON_SPEED_COST == 0 {
        return 0;
    }
    let off_hand = d.attributes_ex3 & ATTR_EX3_OFF_HAND_ONLY != 0;
    mods.weapon_speed(unit.object_guid(), off_hand)
        .map_or(0, |speed| speed / 100)
}

/// The level scale of a non-player caster's level-scaled spell (`0x8014cc`-`0x801539`).
fn npc_scaled_cost(
    d: &SpellDisplay,
    unit: &benilla_protocol::ObjectFields,
    mods: &SpellModifiers,
    cost: i32,
) -> i32 {
    if unit.object_type() == Some(benilla_protocol::messages::ObjectType::Player)
        || d.attributes & ATTR_LEVEL_SCALED_COST == 0
        || d.spell_level == 0
    {
        return cost;
    }
    let spell = mods.npc_cost_scaler(d.spell_level);
    let caster = mods.npc_cost_scaler(unit.unit_level().unwrap_or(0));
    if spell > 0.0 && caster > 0.0 {
        fint((f64::from(caster) / f64::from(spell) * f64::from(cost)) as f32)
    } else {
        cost
    }
}

/// [`power_cost`] on `unit`'s fields with the per-level term at `level`, `[vtbl+0x13c]` over 5.
pub(crate) fn power_cost_at(
    d: &SpellDisplay,
    unit: &benilla_protocol::ObjectFields,
    level: u32,
    mods: &SpellModifiers,
) -> u32 {
    let base = match cost_basis(d) {
        CostBasis::None => 0,
        CostBasis::BaseMana => unit.unit_base_mana().unwrap_or(0),
        CostBasis::BaseHealth => unit.unit_base_health().unwrap_or(0),
        CostBasis::Fixed(value) => value,
    };
    let level_delta = i64::from(level) - i64::from(d.base_level);
    let cost = i64::from(d.mana_cost)
        + level_delta * i64::from(d.mana_cost_per_level)
        + i64::from(base) * i64::from(d.mana_cost_pct) / 100
        + i64::from(weapon_cost(d, unit, mods));
    let cost = cost.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let cost = mods.apply(d, OP_COST, school_cost(unit, d, cost));
    u32::try_from(npc_scaled_cost(d, unit, mods, cost)).unwrap_or(0)
}

/// The speed the weapon term reads (`0x801396`-`0x801445`): the form's round time, else the hand's.
pub(crate) fn weapon_cost_speed(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    forms: Option<&std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>>,
    off_hand: bool,
) -> u32 {
    let form = u32::from(store.0.unit_shapeshift_form());
    let round_time = forms
        .and_then(|f| f.get(&form))
        .map_or(0, |f| f.combat_round_time);
    if round_time != 0 {
        return round_time;
    }
    let slot = if off_hand {
        crate::items::EQUIPMENT_SLOT_OFFHAND
    } else {
        crate::items::EQUIPMENT_SLOT_MAINHAND
    };
    store
        .0
        .player_inv_slot(slot)
        .filter(|&guid| guid != 0)
        .and_then(|guid| objects.object(guid))
        .filter(|item| held_item_usable(item) && !hand_disarmed(store, objects, items, slot))
        .map_or(UNARMED_COST_SPEED, |item| {
            item.object_entry()
                .and_then(|entry| items.template_cached(entry))
                .map_or(0, |t| t.delay_ms)
        })
}

/// The held-item test (`0x6d71a0`) on the item's own flags and durability.
fn held_item_usable(item: &benilla_protocol::ObjectFields) -> bool {
    let flags = item.item_flags().unwrap_or(0);
    let broken =
        item.item_max_durability().unwrap_or(0) > 0 && item.item_durability().unwrap_or(0) == 0;
    flags & ITEM_FLAG_DEPRECATED == 0 && (flags & ITEM_FLAG_BREAK_IGNORED != 0 || !broken)
}

/// The per-hand disarm test (`0x718fc0`): the disarm ladder, or the off hand's own flag.
fn hand_disarmed(store: &ObjectStore, objects: &Objects, items: &Items, slot: u8) -> bool {
    crate::items::disarmed_equipment_slot_cached(store, objects, items) == Some(slot)
        || (slot == crate::items::EQUIPMENT_SLOT_OFFHAND
            && store.0.unit_flags2() & UNIT_FLAG2_DISARM_OFFHAND != 0)
}

/// What [`power_cost_at`] scales `d`'s percentage cost, a jump table on the power type plus 2, on
/// the caster's fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CostBasis {
    /// No percentage, or a type the table sends to 0: the cost reads no unit field.
    None,
    /// Mana: `UNIT_FIELD_BASE_MANA`, unit block `+0x270`.
    BaseMana,
    /// Health (-2): `UNIT_FIELD_BASE_HEALTH`, unit block `+0x274`.
    BaseHealth,
    /// A constant, not the pool: 1000 for rage, 100 for focus and energy.
    Fixed(u32),
}

pub(crate) fn cost_basis(d: &SpellDisplay) -> CostBasis {
    if d.mana_cost_pct == 0 {
        return CostBasis::None;
    }
    match d.power_type as i32 {
        -2 => CostBasis::BaseHealth,
        0 => CostBasis::BaseMana,
        1 => CostBasis::Fixed(1000),
        2 | 3 => CostBasis::Fixed(100),
        _ => CostBasis::None,
    }
}

/// Whether [`power_cost_at`]'s level term moves the cost: only a spell with a per-level cost.
pub(crate) fn cost_reads_level(d: &SpellDisplay) -> bool {
    d.mana_cost_per_level != 0
}

/// Whether [`power_cost_at`]'s weapon term can move the cost.
pub(crate) fn cost_reads_weapon(d: &SpellDisplay) -> bool {
    d.attributes_ex4 & ATTR_EX4_WEAPON_SPEED_COST != 0
}

/// Whether the caster can afford `d`, for leg 12 and the press-path gate: `UNIT_FIELD_POWER[type]`,
/// or `UNIT_FIELD_HEALTH` for any negative PowerType (Bloodrage's -2), against [`power_cost`].
pub(crate) fn can_afford(d: &SpellDisplay, store: &ObjectStore, mods: &SpellModifiers) -> bool {
    let power_type = d.power_type as i32;
    let avail = if power_type < 0 {
        store.0.unit_health().unwrap_or(0)
    } else {
        store.0.unit_power(power_type as u8).unwrap_or(0)
    };
    avail >= power_cost(d, store, mods)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    // Field indices, private in the protocol crate: health 22, power1 23, max health 28, flags
    // 46, aurastate 125, bytes_1 138, player flags 190.
    fn player(pairs: &[(u16, u32)]) -> ObjectStore {
        // `UNIT_FLAG_PVP_ATTACKABLE` (bit 3), which every player carries: the attack check picks
        // its arm on that bit for both parties.
        let mut base = vec![(22u16, 100u32), (23, 500), (46, 1 << 3)];
        base.extend_from_slice(pairs);
        ObjectStore(ObjectFields::from_pairs(&base))
    }

    /// The disarm ladder hides one weapon, main hand first, so a dual-wielder still qualifies.
    #[test]
    fn a_disarmed_hand_does_not_satisfy_the_equipped_item_requirement() {
        use crate::items::TestDeps;

        // `PLAYER_FIELD_INV_SLOT_HEAD + 2 * slot` for equipment slots 15 and 16.
        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        const DISARMED: u32 = 0x0020_0000;

        // A class-2 weapon requirement.
        let needs_a_weapon = SpellDisplay {
            equipped_item_class: 2,
            // Subclass 7, Sword1H; a zero mask would short-circuit to "fits".
            equipped_item_subclass_mask: 1 << 7,
            ..Default::default()
        };
        let fits = |disarmed: bool, hands: &[(u16, u64)]| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, (1 << 3) | if disarmed { DISARMED } else { 0 })];
            for (i, (field, guid)) in hands.iter().enumerate() {
                pairs.push((*field, *guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(*guid, ObjectFields::from_pairs(&[(3, entry)]));
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(&needs_a_weapon, &store, objects, items, commands)
            })
        };

        // Armed.
        assert!(fits(false, &[(INV_MAINHAND, 0x2a)]));
        assert!(!fits(true, &[(INV_MAINHAND, 0x2a)]));
        // Disarmed dual-wielder: only the main hand is hidden.
        assert!(fits(true, &[(INV_MAINHAND, 0x2a), (INV_OFFHAND, 0x2b)]));
        // With an off hand only, the ladder hides that one.
        assert!(!fits(true, &[(INV_OFFHAND, 0x2b)]));
    }

    /// The reference's slot mask and its two rejects, deprecated and broken.
    #[test]
    fn the_equipped_item_search_masks_by_hand_and_rejects_broken_gear() {
        use crate::items::TestDeps;

        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        // `ITEM_FIELD_DURABILITY` / `_MAXDURABILITY`, instance fields.
        const ITEM_DURABILITY: u16 = 46;
        const ITEM_MAX_DURABILITY: u16 = 47;

        let spell = |ex3: u32| SpellDisplay {
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 7, // Sword1H; a zero mask short-circuits to "fits"
            attributes_ex3: ex3,
            ..Default::default()
        };
        // `hands`: (inv field, guid, template flags, max durability, durability).
        let fits = |d: &SpellDisplay, hands: &[(u16, u64, u32, u32, u32)]| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, 1u32 << 3)];
            for (i, (field, guid, flags, max_dur, dur)) in hands.iter().enumerate() {
                pairs.push((*field, *guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(
                    *guid,
                    ObjectFields::from_pairs(&[
                        (3, entry),
                        (ITEM_DURABILITY, *dur),
                        (ITEM_MAX_DURABILITY, *max_dur),
                    ]),
                );
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        flags: *flags,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(d, &store, objects, items, commands)
            })
        };

        let sound = |field| (field, 0x2au64, 0u32, 0u32, 0u32);

        // Hand-agnostic: either hand satisfies it.
        assert!(fits(&spell(0), &[sound(INV_MAINHAND)]));
        assert!(fits(&spell(0), &[sound(INV_OFFHAND)]));
        // Main hand only (`0x400`).
        assert!(fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));
        assert!(!fits(
            &spell(ATTR_EX3_MAIN_HAND_ONLY),
            &[sound(INV_OFFHAND)]
        ));
        // Off hand only (`0x1000000`).
        assert!(fits(&spell(ATTR_EX3_OFF_HAND_ONLY), &[sound(INV_OFFHAND)]));
        assert!(!fits(
            &spell(ATTR_EX3_OFF_HAND_ONLY),
            &[sound(INV_MAINHAND)]
        ));

        // Deprecated, and broken (`MaxDurability > 0 && Durability == 0`).
        assert!(!fits(
            &spell(0),
            &[(INV_MAINHAND, 0x2a, ITEM_FLAG_DEPRECATED, 0, 0)]
        ));
        assert!(!fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 0)]));
        // Damaged still counts, as does no durability at all.
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 45, 12)]));
        assert!(fits(&spell(0), &[(INV_MAINHAND, 0x2a, 0, 0, 0)]));
    }

    /// A main-hand-only ability has nothing left to match once the disarm hides the main hand.
    #[test]
    fn a_main_hand_only_ability_is_dead_while_that_hand_is_disarmed() {
        use crate::items::TestDeps;

        const INV_MAINHAND: u16 = 486 + 2 * 15;
        const INV_OFFHAND: u16 = 486 + 2 * 16;
        const DISARMED: u32 = 0x0020_0000;

        let fits = |ex3: u32, disarmed: bool| {
            let mut deps = TestDeps::new();
            let mut pairs = vec![(46u16, (1 << 3) | if disarmed { DISARMED } else { 0 })];
            for (i, field) in [INV_MAINHAND, INV_OFFHAND].iter().enumerate() {
                let guid = 0x2a + i as u64;
                pairs.push((*field, guid as u32));
                let entry = 500 + i as u32;
                deps.spawn_item(guid, ObjectFields::from_pairs(&[(3, entry)]));
                deps.items.insert_template(
                    entry,
                    Some(benilla_protocol::messages::ItemInfo {
                        class: 2,
                        subclass: 7,
                        ..crate::items::test_template("Sword")
                    }),
                );
            }
            let store = ObjectStore(ObjectFields::from_pairs(&pairs));
            deps.with_objects(|objects, items, commands| {
                equipped_item_fits(
                    &SpellDisplay {
                        equipped_item_class: 2,
                        equipped_item_subclass_mask: 1 << 7, // Sword1H
                        attributes_ex3: ex3,
                        ..Default::default()
                    },
                    &store,
                    objects,
                    items,
                    commands,
                )
            })
        };

        assert!(fits(ATTR_EX3_MAIN_HAND_ONLY, false));
        assert!(fits(ATTR_EX3_OFF_HAND_ONLY, false));
        // Disarmed: the main hand is hidden first.
        assert!(!fits(ATTR_EX3_MAIN_HAND_ONLY, true));
        assert!(fits(ATTR_EX3_OFF_HAND_ONLY, true));
        assert!(fits(0, true));
    }

    fn ctx<'a>(
        store: &'a ObjectStore,
        cooldowns: &'a Cooldowns,
        reputations: &'a Reputations,
        carried: &'a std::collections::HashMap<u32, u32>,
        spell_mods: &'a SpellModifiers,
    ) -> UsableCtx<'a> {
        UsableCtx {
            store,
            target_store: None,
            factions: None,
            reputations,
            party: &crate::target::NoGroup,
            cooldowns,
            carried,
            spell_mods,
            runes: None,
            combo_points: 0,
        }
    }

    fn walk(d: &SpellDisplay, store: &ObjectStore) -> (bool, bool) {
        walk_with_combo(d, store, 0)
    }

    fn walk_with_combo(d: &SpellDisplay, store: &ObjectStore, combo_points: u8) -> (bool, bool) {
        let cooldowns = Cooldowns::default();
        let reputations = Reputations(Vec::new(), Vec::new());
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let carried = crate::ui_items::carried_counts(&store.0, &objects);
        let spell_mods = SpellModifiers::default();
        spell_usable(
            1,
            d,
            &spells,
            &UsableCtx {
                combo_points,
                ..ctx(store, &cooldowns, &reputations, &carried, &spell_mods)
            },
            &objects,
            &items,
            &commands,
        )
    }

    /// The rune check (`0x5ee5b0`): each kind's ready runes against the row, the shortfall covered
    /// by ready death runes, and a cost cut to nothing is free.
    #[test]
    fn a_rune_spell_needs_its_runes_or_death_runes() {
        use benilla_formats::{SpellRuneCost, SpellRuneCostCatalog};
        use benilla_ui::script::RuneState;

        let costs = SpellRuneCostCatalog::from_rows([SpellRuneCost {
            id: 7,
            unholy: 1,
            frost: 1,
            ..Default::default()
        }]);
        let strike = SpellDisplay {
            power_type: POWER_RUNE,
            rune_cost_id: 7,
            ..Default::default()
        };
        let mods = SpellModifiers::default();
        let check = |ready: u32, kinds: [u8; 8]| {
            let state = RuneState {
                ready,
                kinds,
                ..Default::default()
            };
            RuneCheck {
                state: &state,
                costs: &costs,
                player: &ObjectFields::default(),
            }
            .has_runes(&strike, &mods)
        };
        let base = [0, 0, 1, 1, 2, 2, 0, 0];
        assert!(check(0x3f, base), "all ready");
        assert!(check(0x14, base), "one unholy and one frost");
        assert!(!check(0x0c, base), "no frost");
        // Blood rune 0 turned death covers the missing frost.
        assert!(check(0x0d, [3, 0, 1, 1, 2, 2, 0, 0]));
        assert!(
            !check(0x01, [3, 0, 1, 1, 2, 2, 0, 0]),
            "one death rune short of two"
        );
        // A rune spell with no row, or a non-rune spell, is free.
        let free = SpellDisplay {
            power_type: POWER_RUNE,
            rune_cost_id: 99,
            ..Default::default()
        };
        let state = RuneState {
            ready: 0,
            ..Default::default()
        };
        let none = RuneCheck {
            state: &state,
            costs: &costs,
            player: &ObjectFields::default(),
        };
        assert!(none.has_runes(&free, &mods));
        assert!(!none.has_runes(&strike, &mods));
        // A school multiplier of -1 takes the percentage to 0 (`0x5ee663`): the spell is free.
        let free_school = ObjectFields::from_pairs(&[(COST_MULTIPLIER + 3, (-1.0f32).to_bits())]);
        let frost = SpellDisplay {
            power_type: POWER_RUNE,
            rune_cost_id: 7,
            school_mask: 1 << 3,
            ..Default::default()
        };
        let discounted = RuneCheck {
            player: &free_school,
            ..none
        };
        assert!(discounted.has_runes(&frost, &mods));
        assert!(!discounted.has_runes(&strike, &mods), "another school");
    }

    // The shared places of the two cost runs, seven schools each.
    const COST_MODIFIER: u16 = 1262;
    const COST_MULTIPLIER: u16 = 1269;

    fn pct(value: f32) -> u32 {
        value.to_bits()
    }

    /// The smallest of the mask's schools, for each run on its own; an empty mask reads neither.
    #[test]
    fn the_school_step_picks_the_lowest_value_of_the_mask() {
        let store = player(&[
            (COST_MODIFIER + 2, 12),
            (COST_MODIFIER + 3, (-30i32) as u32),
            (COST_MODIFIER + 4, (-5i32) as u32),
            (COST_MULTIPLIER + 2, pct(-0.1)),
            (COST_MULTIPLIER + 3, pct(0.5)),
            (COST_MULTIPLIER + 4, pct(-0.5)),
        ]);
        let mods = SpellModifiers::default();
        let spell = |school_mask| SpellDisplay {
            mana_cost: 130,
            school_mask,
            ..Default::default()
        };
        // Fire and frost: 130 - 30, times 1 - 0.1.
        assert_eq!(power_cost(&spell(1 << 2 | 1 << 3), &store, &mods), 90);
        // Frost and nature: 130 - 30, times 1 - 0.5.
        assert_eq!(power_cost(&spell(1 << 3 | 1 << 4), &store, &mods), 50);
        // Fire alone raises the cost by 12 before the cut.
        assert_eq!(power_cost(&spell(1 << 2), &store, &mods), 128);
        // A mask of a school with no value, and an empty one, leave the cost.
        assert_eq!(power_cost(&spell(1), &store, &mods), 130);
        assert_eq!(power_cost(&spell(0), &store, &mods), 130);
    }

    /// Flat, then multiplier, then `SPELLMOD_COST`, each rounding as the reference does.
    #[test]
    fn the_school_step_runs_before_the_cost_spell_modifier() {
        let store = player(&[(COST_MODIFIER, 10), (COST_MULTIPLIER, pct(1.0))]);
        let d = SpellDisplay {
            spell_family: 3,
            spell_family_flags: 1 << 5,
            mana_cost: 100,
            school_mask: 1,
            ..Default::default()
        };
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(true, 5, OP_COST, 5);
        mods.set(false, 5, OP_COST, -50);
        // (100 + 10) * 2 = 220, then (220 + 5) * 50 / 100, truncating.
        assert_eq!(power_cost(&d, &store, &mods), 112);
        // `fistp` rounds to nearest with ties to even: 37.5 is 38, 22.5 is 22, 22.6 is 23.
        for (cost, multiplier, shown) in [(50, 0.75, 38), (30, 0.75, 22), (30, 0.7533, 23)] {
            let store = player(&[(COST_MULTIPLIER, pct(multiplier as f32 - 1.0))]);
            let d = SpellDisplay {
                mana_cost: cost,
                school_mask: 1,
                ..Default::default()
            };
            assert_eq!(
                power_cost(&d, &store, &SpellModifiers::default()),
                shown,
                "{cost} at {multiplier}"
            );
        }
    }

    /// A negative result clamps at 0 after the school step, and a free spell pays the flat rise.
    #[test]
    fn the_school_step_moves_a_free_spell_and_clamps_at_zero() {
        let store = player(&[(COST_MODIFIER, (-200i32) as u32), (COST_MODIFIER + 1, 15)]);
        let mods = SpellModifiers::default();
        let spell = |school_mask| SpellDisplay {
            mana_cost: 50,
            school_mask,
            ..Default::default()
        };
        assert_eq!(power_cost(&spell(1), &store, &mods), 0);
        let free = SpellDisplay {
            school_mask: 2,
            ..Default::default()
        };
        assert_eq!(power_cost(&free, &store, &mods), 15);
    }

    /// The power gate pays the modified cost too.
    #[test]
    fn the_gate_reads_the_school_step() {
        let d = SpellDisplay {
            mana_cost: 100,
            school_mask: 4,
            ..Default::default()
        };
        let mods = SpellModifiers::default();
        let store = player(&[(23, 80), (COST_MULTIPLIER + 2, pct(-0.25))]);
        assert_eq!(power_cost(&d, &store, &mods), 75);
        assert!(can_afford(&d, &store, &mods));
        assert!(!can_afford(&d, &player(&[(23, 80)]), &mods));
    }

    /// A pet spell of level 20 cast at another level; the table's values are the test's own.
    fn npc_scaled_mods() -> SpellModifiers {
        let mut mods = SpellModifiers::default();
        mods.set_npc_cost_scaler((1..=100).map(|level| level as f32 / 20.0));
        mods
    }

    const OBJECT_TYPE: u16 = 2;
    const UNIT_LEVEL: u16 = 34;
    const TYPE_UNIT: u32 = 0x9;
    const TYPE_PLAYER: u32 = 0x19;

    #[test]
    fn a_non_player_cost_scales_by_level() {
        let scaled = |attributes, spell_level| SpellDisplay {
            attributes,
            spell_level,
            mana_cost: 90,
            ..Default::default()
        };
        let spell = scaled(0x0008_0000, 20);
        let mods = npc_scaled_mods();
        let unit = |ty: u32, level: u32| {
            ObjectFields::from_pairs(&[(OBJECT_TYPE, ty), (UNIT_LEVEL, level)])
        };
        let cost = |d: &SpellDisplay, u: &ObjectFields| {
            power_cost_at(d, u, u.unit_level().unwrap_or(0), &mods)
        };
        // Level 30 pays half again, level 10 half, and the spell's own level the base.
        assert_eq!(cost(&spell, &unit(TYPE_UNIT, 30)), 135);
        assert_eq!(cost(&spell, &unit(TYPE_UNIT, 10)), 45);
        assert_eq!(cost(&spell, &unit(TYPE_UNIT, 20)), 90);
        // A player is never scaled.
        assert_eq!(cost(&spell, &unit(TYPE_PLAYER, 30)), 90);
        // Neither is a spell without the attribute, or one with no spell level.
        assert_eq!(cost(&scaled(0, 20), &unit(TYPE_UNIT, 30)), 90);
        assert_eq!(cost(&scaled(0x0008_0000, 0), &unit(TYPE_UNIT, 30)), 90);
        // A level past the table reads 0, which leaves the cost.
        assert_eq!(cost(&spell, &unit(TYPE_UNIT, 101)), 90);
        // The caster's own level, never the per-level term's skill level.
        assert_eq!(power_cost_at(&spell, &unit(TYPE_UNIT, 30), 7, &mods), 135);
    }

    /// The pet bar's gate pays the scaled cost through the same lookup.
    #[test]
    fn a_pet_gate_pays_the_scaled_cost() {
        let spell = SpellDisplay {
            attributes: 0x0008_0000,
            spell_level: 20,
            mana_cost: 90,
            ..Default::default()
        };
        let mods = npc_scaled_mods();
        let pet = |mana: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (OBJECT_TYPE, TYPE_UNIT),
                (UNIT_LEVEL, 30),
                (23, mana),
            ]))
        };
        assert!(!can_afford(&spell, &pet(134), &mods));
        assert!(can_afford(&spell, &pet(135), &mods));
    }

    /// `AttributesEx4` bit 10 with the hand `AttributesEx3` bit 24 picks.
    fn shiv(off_hand: bool) -> SpellDisplay {
        SpellDisplay {
            attributes_ex3: if off_hand { 0x0100_0000 } else { 0 },
            attributes_ex4: 0x400,
            mana_cost: 20,
            power_type: 3,
            ..Default::default()
        }
    }

    const MY_GUID: u64 = 0x77;
    const OBJECT_GUID: u16 = 0;
    const UNIT_FLAGS: u16 = 46;
    const UNIT_FLAGS2: u16 = 44;
    const UNIT_BYTES_2: u16 = 164;
    const INV_MAINHAND: u16 = 486 + 2 * 15;
    const INV_OFFHAND: u16 = 486 + 2 * 16;
    const DISARMED: u32 = 0x0020_0000;

    /// The term adds speed / 100 for the active player alone, by the spell's hand.
    #[test]
    fn the_weapon_term_reads_the_spells_hand_on_the_active_player() {
        let mut mods = SpellModifiers::default();
        mods.set_weapon_speed(MY_GUID, [2600, 1450]);
        let mine = ObjectFields::from_pairs(&[(OBJECT_GUID, MY_GUID as u32)]);
        let cost = |d: &SpellDisplay, u: &ObjectFields| power_cost_at(d, u, 0, &mods);
        assert_eq!(cost(&shiv(false), &mine), 46, "main hand, 2.6 s");
        assert_eq!(cost(&shiv(true), &mine), 34, "off hand, 1.45 s floors");
        // Another unit, the charmed one included, is not the active player.
        let other = ObjectFields::from_pairs(&[(OBJECT_GUID, 0x78)]);
        assert_eq!(cost(&shiv(true), &other), 20);
        // A spell without the attribute takes no term.
        let plain = SpellDisplay {
            attributes_ex4: 0,
            ..shiv(true)
        };
        assert_eq!(cost(&plain, &mine), 20);
        // The term runs before the school step.
        let store = ObjectFields::from_pairs(&[
            (OBJECT_GUID, MY_GUID as u32),
            (COST_MULTIPLIER + 1, pct(-0.5)),
        ]);
        let physical = SpellDisplay {
            school_mask: 2,
            ..shiv(true)
        };
        assert_eq!(cost(&physical, &store), 17);
    }

    /// Each branch of the speed read, the fallback numbers among them.
    #[test]
    fn the_weapon_speed_walks_the_form_the_hand_and_its_fallbacks() {
        use crate::items::TestDeps;
        const MAIN: u64 = 0x100;
        const OFF: u64 = 0x101;
        const ITEM_ENTRY: u16 = 3;
        const ITEM_FLAGS: u16 = 21;
        const ITEM_DURABILITY: u16 = 46;
        const ITEM_MAX_DURABILITY: u16 = 47;

        struct Weapon {
            delay: u32,
            flags: u32,
            durability: (u32, u32),
        }
        let sword = |delay| Weapon {
            delay,
            flags: 0,
            durability: (50, 50),
        };
        let speed =
            |player: &[(u16, u32)],
             main: Option<Weapon>,
             off: Option<Weapon>,
             cached: bool,
             forms: Option<&std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>>,
             hand: bool| {
                let mut deps = TestDeps::new();
                let mut pairs = player.to_vec();
                for (field, guid, weapon) in [(INV_MAINHAND, MAIN, main), (INV_OFFHAND, OFF, off)] {
                    let Some(w) = weapon else { continue };
                    pairs.push((field, guid as u32));
                    let entry = 500 + guid as u32;
                    deps.spawn_item(
                        guid,
                        ObjectFields::from_pairs(&[
                            (ITEM_ENTRY, entry),
                            (ITEM_FLAGS, w.flags),
                            (ITEM_DURABILITY, w.durability.0),
                            (ITEM_MAX_DURABILITY, w.durability.1),
                        ]),
                    );
                    if cached {
                        deps.items.insert_template(
                            entry,
                            Some(benilla_protocol::messages::ItemInfo {
                                class: 2,
                                delay_ms: w.delay,
                                ..crate::items::test_template("Sword")
                            }),
                        );
                    }
                }
                let store = ObjectStore(ObjectFields::from_pairs(&pairs));
                deps.with_objects(|objects, items, _| {
                    weapon_cost_speed(&store, objects, items, forms, hand)
                })
            };
        // Each hand reads its own weapon.
        let both = || (Some(sword(2600)), Some(sword(1400)));
        let (m, o) = both();
        assert_eq!(speed(&[], m, o, true, None, false), 2600);
        let (m, o) = both();
        assert_eq!(speed(&[], m, o, true, None, true), 1400);
        // An empty hand reads 2000, the other hand's weapon no matter.
        assert_eq!(speed(&[], Some(sword(2600)), None, true, None, true), 2000);
        // An uncached record reads 0.
        assert_eq!(speed(&[], Some(sword(2600)), None, false, None, false), 0);
        // An item with the unusable flag reads 2000.
        let flagged = Weapon {
            flags: 0x10,
            ..sword(2600)
        };
        assert_eq!(speed(&[], Some(flagged), None, true, None, false), 2000);
        // A broken one reads 2000, unless the flag `0x8` keeps it.
        let broken = |flags| Weapon {
            flags,
            durability: (0, 50),
            ..sword(2600)
        };
        assert_eq!(speed(&[], Some(broken(0)), None, true, None, false), 2000);
        assert_eq!(speed(&[], Some(broken(8)), None, true, None, false), 2600);
        // A 0 durability of a 0 maximum is not broken.
        let unbreakable = Weapon {
            durability: (0, 0),
            ..sword(2600)
        };
        assert_eq!(speed(&[], Some(unbreakable), None, true, None, false), 2600);
        // The disarm flag hides the main hand first, and the off hand then reads its own weapon.
        let disarmed = [(UNIT_FLAGS, DISARMED)];
        let (m, o) = both();
        assert_eq!(speed(&disarmed, m, o, true, None, false), 2000);
        let (m, o) = both();
        assert_eq!(speed(&disarmed, m, o, true, None, true), 1400);
        // With no main-hand weapon the disarm falls on the off hand.
        assert_eq!(
            speed(&disarmed, None, Some(sword(1400)), true, None, true),
            2000
        );
        // The off-hand disarm flag is the off hand's alone.
        let off_disarmed = [(UNIT_FLAGS2, 0x80)];
        let (m, o) = both();
        assert_eq!(speed(&off_disarmed, m, o, true, None, true), 2000);
        let (m, o) = both();
        assert_eq!(speed(&off_disarmed, m, o, true, None, false), 2600);
        // A form with a round time wins over both hands; one without falls through.
        let forms = std::collections::HashMap::from([
            (
                1,
                benilla_formats::ShapeshiftForm {
                    combat_round_time: 1000,
                    ..Default::default()
                },
            ),
            (3, benilla_formats::ShapeshiftForm::default()),
        ]);
        let in_form = |form: u32| [(UNIT_BYTES_2, form << 24)];
        let (m, o) = both();
        assert_eq!(speed(&in_form(1), m, o, true, Some(&forms), false), 1000);
        let (m, o) = both();
        assert_eq!(speed(&in_form(1), m, o, true, Some(&forms), true), 1000);
        let (m, o) = both();
        assert_eq!(speed(&in_form(3), m, o, true, Some(&forms), false), 2600);
    }

    /// A level reads its row, level 1 the first, and a level outside the table reads 0.
    #[test]
    fn the_scaler_reads_by_level_and_zero_outside_the_table() {
        let mods = npc_scaled_mods();
        assert_eq!(mods.npc_cost_scaler(20), 1.0);
        assert_eq!(mods.npc_cost_scaler(0), 0.0);
        assert_eq!(mods.npc_cost_scaler(101), 0.0);
    }

    /// Bloodrage (2687) takes 20% of base health, even when gear raises max health.
    #[test]
    fn bloodrage_cost_uses_base_health_for_cost_and_gates() {
        let bloodrage = SpellDisplay {
            power_type: (-2i32) as u32,
            mana_cost_pct: 20,
            ..Default::default()
        };
        let mods = SpellModifiers::default();
        let with_gear = player(&[(22, 400), (28, 4_000), (163, 1_689)]);
        assert_eq!(power_cost(&bloodrage, &with_gear, &mods), 337);
        assert!(can_afford(&bloodrage, &with_gear, &mods));
        assert_eq!(walk(&bloodrage, &with_gear), (true, false));

        let below_cost = player(&[(22, 336), (28, 4_000), (163, 1_689)]);
        assert!(!can_afford(&bloodrage, &below_cost, &mods));
        assert_eq!(walk(&bloodrage, &below_cost), (false, true));

        let without_gear = player(&[(22, 400), (28, 1_689), (163, 1_689)]);
        assert_eq!(power_cost(&bloodrage, &without_gear, &mods), 337);
    }

    /// The reference's jump table (indexed by the type plus 2): base health for -2, base mana,
    /// fixed bases for rage, focus and energy, and 0 for -1, happiness and past the table.
    #[test]
    fn percentage_cost_bases_follow_power_type() {
        let store = player(&[
            (28, 4_000), // max health
            (29, 2_000), // max mana
            (30, 800),   // max rage
            (31, 80),    // max focus
            (32, 120),   // max energy
            (33, 500),   // max happiness
            (162, 1_500),
            (163, 1_689),
        ]);
        let mods = SpellModifiers::default();
        let cases = [
            (-3, 0),
            (-2, 337),
            (-1, 0),
            (0, 300),
            (1, 200),
            (2, 20),
            (3, 20),
            (4, 0),
            (5, 0),
        ];
        for (power_type, cost) in cases {
            let spell = SpellDisplay {
                power_type: power_type as u32,
                mana_cost_pct: 20,
                ..Default::default()
            };
            assert_eq!(power_cost(&spell, &store, &mods), cost, "type {power_type}");
        }
    }

    /// A -30% cost cell turns an unaffordable 100-mana spell into an affordable 70 with 80 mana.
    #[test]
    fn a_talent_cost_cut_reaches_the_cost_and_the_press_path_gate() {
        // Family 3 (mage), family bit 5.
        let d = SpellDisplay {
            spell_family: 3,
            spell_family_flags: 1 << 5,
            mana_cost: 100,
            ..Default::default()
        };
        let store = ObjectStore(ObjectFields::from_pairs(&[(23, 80)]));

        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        assert_eq!(
            power_cost(&d, &store, &mods),
            100,
            "no packet yet — the flat DBC cost"
        );
        assert!(
            !can_afford(&d, &store, &mods),
            "80 mana does not buy a 100 mana spell"
        );

        mods.set(false, 5, OP_COST, -30);
        assert_eq!(power_cost(&d, &store, &mods), 70);
        assert!(
            can_afford(&d, &store, &mods),
            "the talent brings it inside the pool, so the cast attempt must send the cast"
        );

        // Another class's family takes nothing.
        let other_class = SpellDisplay {
            spell_family: 4,
            ..d
        };
        assert_eq!(power_cost(&other_class, &store, &mods), 100);
    }

    /// Each gate trips alone; only the power leg raises notEnoughMana.
    #[test]
    fn gates_trip_independently_and_only_power_sets_oom() {
        let alive = player(&[]);
        let d = SpellDisplay::default();
        assert_eq!(walk(&d, &alive), (true, false));

        // Leg 1: dead, or a ghost at health 1 (field 190 bit `0x10`); the attribute waives it.
        let dead = player(&[(22, 0), (28, 100)]);
        assert_eq!(walk(&d, &dead), (false, false));
        let ghost = player(&[(22, 1), (28, 100), (190, 0x10)]);
        assert_eq!(walk(&d, &ghost), (false, false));
        let while_dead = SpellDisplay {
            attributes: ATTR_CASTABLE_WHILE_DEAD,
            ..Default::default()
        };
        assert_eq!(walk(&while_dead, &dead), (true, false));
        assert_eq!(walk(&while_dead, &ghost), (true, false));

        // Leg 3: a missing reagent.
        let reagent = SpellDisplay {
            reagents: [
                (17056, 1),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
            ],
            ..Default::default()
        };
        assert_eq!(walk(&reagent, &alive), (false, false));

        // Leg 5: Overpower's shape with no combo points on the target.
        let overpower = SpellDisplay {
            attributes_ex: 0x4810_0200,
            ..Default::default()
        };
        assert!(overpower.needs_combo_points());
        assert_eq!(walk(&overpower, &alive), (false, false));
        assert_eq!(walk_with_combo(&overpower, &alive, 1), (true, false));
        // `PLAYER_FIELD_BYTES` byte 1 is not combo points on 3.3.5a.
        let ranked = player(&[(1222, 0x05_03_01_01)]);
        assert_eq!(walk(&overpower, &ranked), (false, false));

        // Leg 6: a cat-form spell out of form.
        let claw = SpellDisplay {
            stances: 0x1,
            ..Default::default()
        };
        assert_eq!(walk(&claw, &alive), (false, false));
        let in_cat = player(&[(164, 1 << 24)]);
        assert_eq!(walk(&claw, &in_cat), (true, false));

        // Leg 7: only-stealthed vs the CREEP byte.
        let ambush = SpellDisplay {
            attributes: ATTR_ONLY_STEALTHED,
            ..Default::default()
        };
        assert_eq!(walk(&ambush, &alive), (false, false));
        let sneaking = player(&[(138, 0x2 << 24)]);
        assert_eq!(walk(&ambush, &sneaking), (true, false));

        // Leg 8: not-in-combat vs UNIT_FLAG_IN_COMBAT.
        let mount = SpellDisplay {
            attributes: ATTR_NOT_IN_COMBAT,
            ..Default::default()
        };
        assert_eq!(walk(&mount, &alive), (true, false));
        let fighting = player(&[(46, UNIT_FLAG_IN_COMBAT)]);
        assert_eq!(walk(&mount, &fighting), (false, false));

        // Leg 9: CasterAuraState (defense = 1 → bit 0).
        let revenge = SpellDisplay {
            caster_aura_state: 1,
            ..Default::default()
        };
        assert_eq!(walk(&revenge, &alive), (false, false));
        let defended = player(&[(125, 0x1)]);
        assert_eq!(walk(&revenge, &defended), (true, false));

        // Leg 10: TargetAuraState with no current target.
        let execute = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 6,
            ..Default::default()
        };
        assert_eq!(walk(&execute, &alive), (false, false));

        // Leg 12: power, the only oom.
        let costly = SpellDisplay {
            mana_cost: 501,
            ..Default::default()
        };
        assert_eq!(walk(&costly, &alive), (false, true));

        // The early-out beats every gate.
        let tradeskill = SpellDisplay {
            effects: [SPELL_EFFECT_TRADE_SKILL, 0, 0],
            mana_cost: 9999,
            stances: 0x1,
            ..Default::default()
        };
        assert_eq!(walk(&tradeskill, &dead), (true, false));
    }

    /// Leg 10: the aura-state bit gates, and the attack check passes on the default neutral
    /// reaction.
    #[test]
    fn target_aura_state_reads_the_current_target() {
        let me = player(&[]);
        let cooldowns = Cooldowns::default();
        let reputations = Reputations(Vec::new(), Vec::new());
        let spells = Spells::empty_for_tests();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let execute = SpellDisplay {
            target_aura_state: 2,
            implicit_target_a1: 6,
            ..Default::default()
        };

        let healthy = ObjectStore(ObjectFields::from_pairs(&[(22, 100), (125, 0)]));
        let low = ObjectStore(ObjectFields::from_pairs(&[(22, 10), (125, 0x2)]));
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let carried = crate::ui_items::carried_counts(&me.0, &objects);
        for (target, expect) in [(&healthy, false), (&low, true)] {
            let ctx = UsableCtx {
                store: &me,
                target_store: Some(target),
                factions: None,
                reputations: &reputations,
                party: &crate::target::NoGroup,
                cooldowns: &cooldowns,
                carried: &carried,
                spell_mods: &SpellModifiers::default(),
                runes: None,
                combo_points: 0,
            };
            assert_eq!(
                spell_usable(5308, &execute, &spells, &ctx, &objects, &items, &commands),
                (expect, false)
            );
        }
    }
}
