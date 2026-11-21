//! The bar's slot signal, `ACTIONBAR_SLOT_CHANGED`, fired where 12340 fires it, each slot's usable
//! bytes recomputed first (`0x5aa390`): a write, which also sends the slot (`0x5aae80`,
//! `0x5aaa90`); the table's put, which signals the whole bar with 0 (`0x6d885b`, `0x6e8046`); and
//! the per-slot updates that only signal: an item's (`0x5aa5c0`), the weapon faces' (`0x5aa4e0`,
//! `0x5aa550`), an aura's (`0x5aab10`) and a companion's (`0x5aac00`). The verbs read each slot
//! at the call, its count included (`0x5a8a60`).

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::ScriptValue;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};
use crate::ui_script::live::UiEvents;

use super::usable_cache::{self, Pass};
use super::{ActionSlots, PlayerActions, Spells};

/// An item action's icon when its template has not answered or its display id has no
/// `ItemDisplayInfo` row: the literal the reference's icon resolver falls back to.
pub(crate) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// `ITEM_SPELLTRIGGER_LEARN_SPELL_ID`: the block a used-up item's slot turns into (`0x5aa6f5`).
const TRIGGER_LEARN: u32 = 6;

/// `SPELL_EFFECT_SUMMON`, the companion test's effect (`0x5aac6d`).
const EFFECT_SUMMON: u32 = 28;

/// `slot`'s signal, 1-based, or 0 for the whole bar.
fn signal(world: &mut World, action: u32) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire(
            "ACTIONBAR_SLOT_CHANGED",
            vec![ScriptValue::Int(i64::from(action))],
        );
    }
}

/// One slot changed (`0x5aa390` with the event): its usable bytes, then its signal.
pub(crate) fn slot_changed(world: &mut World, slot: u8) {
    usable_cache::step(Pass::Slot(slot))(world);
    signal(world, u32::from(slot) + 1);
}

/// A table put's slot set without its signal (`0x5aae80` with its ask flag): an item asks its record,
/// a miss linking the bar's callback once (`0x5aaee6`-`0x5aaf0f`), then its usable bytes alone
/// (`0x5aa390` silent).
pub(crate) fn slot_set_silently(world: &mut World, slot: u8) {
    ask_item_record(world, slot);
    usable_cache::step(Pass::Slot(slot))(world);
}

/// The slot's item record, asked; on a miss the bar's update waits on it ([`record_landed`]).
fn ask_item_record(world: &mut World, slot: u8) {
    let item = world
        .get_resource::<ActionSlots>()
        .and_then(|a| a.buttons.get(&slot))
        .filter(|b| b.kind == ACTION_KIND_ITEM)
        .map(|b| b.action);
    let Some(item) = item else {
        return;
    };
    let missing = match (
        world.get_resource::<Items>(),
        world.get_resource::<NetCommands>(),
    ) {
        (Some(items), Some(commands)) => {
            items.template(item, 0, commands).is_none() && !items.template_answered_unknown(item)
        }
        _ => false,
    };
    if let (true, Some(mut waits)) = (
        missing,
        world.get_non_send_resource_mut::<crate::items::RecordWaits>(),
    ) {
        waits.wait(item, crate::items::RecordWait::ActionBar, true);
    }
}

/// The whole bar's signal, 0, after a table's silent put.
pub(crate) fn bar_changed(world: &mut World) {
    signal(world, 0);
}

/// [`slot_changed`] in a handler's turn.
pub(crate) fn changed_in_turn(ui: &mut UiEvents, slot: u8) {
    ui.world(move |world| slot_changed(world, slot));
}

/// A slot's spell as the resolvers read it (`0x5a8c30`): a spell word's, a macro's bound spell's;
/// a pet slot and an item have none here.
fn slot_spell(
    button: &benilla_protocol::messages::ActionButton,
    bound: Option<&crate::ui_macro::MacroBoundSpells>,
) -> Option<u32> {
    use crate::ui_macro::BoundSpell;
    match button.kind {
        ACTION_KIND_SPELL => Some(button.action),
        ACTION_KIND_MACRO => match bound?.0.get(&button.action)? {
            BoundSpell::Spell(s) => Some(*s),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `d` takes `item` as a reagent (`0x8016a0`).
fn uses_reagent(d: &benilla_formats::SpellDisplay, item: u32) -> bool {
    d.reagents.iter().any(|&(id, n)| id == item && n > 0)
}

/// Whether `d` takes any reagent.
fn has_reagents(d: &benilla_formats::SpellDisplay) -> bool {
    d.reagents.iter().any(|&(id, n)| id != 0 && n > 0)
}

/// A spell's reagent uses (`0x801770`): the fewest casts any reagent's carried count allows, 0
/// with no reagent.
pub(crate) fn reagent_uses(d: &benilla_formats::SpellDisplay, carried: impl Fn(u32) -> u32) -> u32 {
    d.reagents
        .iter()
        .filter(|&&(id, n)| id != 0 && n > 0)
        .map(|&(id, n)| carried(id) / n)
        .min()
        .unwrap_or(0)
}

/// What an item slot's count reads (`0x5a8a60`): the carried copies; for an unstackable item
/// with charges that use it up, the charges left; for one whose use spell takes reagents, its
/// reagent uses. The charges are the template's first block's: a use enchant is not built.
pub(crate) fn item_count(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    spells: Option<&Spells>,
    item: u32,
) -> u32 {
    use crate::ui_items::{count_of, InventoryScope};
    let count = count_of(&store.0, objects, item, InventoryScope::CARRIED);
    if count == 0 {
        return 0;
    }
    let Some(template) = items.template(item, 0, commands) else {
        return count;
    };
    if template.stackable > 1 {
        return count;
    }
    if template.spell_charges_0 < 0 {
        return crate::ui_items::charges_of(&store.0, objects, item);
    }
    let Some(d) = template
        .use_spell
        .and_then(|u| spells?.catalog.get(u.spell_id))
    else {
        return count;
    };
    if !has_reagents(d) {
        return count;
    }
    reagent_uses(d, |id| {
        count_of(&store.0, objects, id, InventoryScope::DEFAULT)
    })
}

/// The slots one item's update touches (`0x5aa5c0`), and the used-up slots that turn into the
/// spell the item taught, both by 0-based slot.
#[derive(Default)]
struct ItemTouch {
    signal: Vec<u8>,
    learn: Vec<(u8, u32)>,
}

fn item_touch(
    In(item): In<u32>,
    actions: Option<Res<ActionSlots>>,
    book: Option<Res<PlayerActions>>,
    bound: Option<Res<crate::ui_macro::MacroBoundSpells>>,
    spells: Option<Res<Spells>>,
    items: Option<Res<Items>>,
    commands: Option<Res<NetCommands>>,
    me: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    objects: Objects,
) -> ItemTouch {
    let mut out = ItemTouch::default();
    let (Some(actions), Some(items), Some(commands)) =
        (actions.as_deref(), items.as_deref(), commands.as_deref())
    else {
        return out;
    };
    let bound = bound.as_deref();
    let spells = spells.as_deref();
    let catalog = |id: u32| spells.and_then(|s| s.catalog.get(id));
    let count = me.iter().next().map_or(0, |store| {
        item_count(store, &objects, items, commands, spells, item)
    });
    let mut slots: Vec<_> = actions.buttons.values().collect();
    slots.sort_by_key(|b| b.slot);
    for button in slots {
        if button.kind != ACTION_KIND_ITEM {
            // A spell slot, through its macro too, that takes the item as a reagent.
            if slot_spell(button, bound)
                .and_then(catalog)
                .is_some_and(|d| uses_reagent(d, item))
            {
                out.signal.push(button.slot);
            }
            continue;
        }
        let Some(template) = items.template(button.action, 0, commands) else {
            continue;
        };
        if button.action == item {
            // Used up: the slot turns into the spell the item taught, once known (`0x5aa6c8`).
            let learned = (count == 0)
                .then(|| {
                    template
                        .spells
                        .iter()
                        .find(|s| s.trigger == TRIGGER_LEARN)
                        .map(|s| s.spell_id)
                })
                .flatten()
                .filter(|&s| s != 0 && book.as_ref().is_some_and(|b| b.spells.contains(&s)));
            match learned {
                Some(spell) => out.learn.push((button.slot, spell)),
                None => out.signal.push(button.slot),
            }
            continue;
        }
        // Another unstackable item whose use spell takes this item as a reagent (`0x5aa906`).
        if template.stackable > 1 || template.spell_charges_0 < 0 {
            continue;
        }
        if template
            .use_spell
            .and_then(|u| catalog(u.spell_id))
            .is_some_and(|d| has_reagents(d) && uses_reagent(d, item))
        {
            out.signal.push(button.slot);
        }
    }
    out
}

/// One item's update (`0x5aa5c0`), its entry `item`: each slot it touches signals in slot order;
/// a used-up item's slot that taught a known spell becomes that spell, sent (`0x5aa6c8`).
pub(crate) fn update_item(world: &mut World, item: u32) {
    if item == 0 {
        return;
    }
    let touch = world
        .run_system_cached_with(item_touch, item)
        .unwrap_or_default();
    // In slot order, as the walk goes: a learned spell's write or a signal.
    let mut steps: Vec<(u8, Option<u32>)> = touch
        .learn
        .into_iter()
        .map(|(slot, spell)| (slot, Some(spell)))
        .chain(touch.signal.into_iter().map(|slot| (slot, None)))
        .collect();
    steps.sort_by_key(|&(slot, _)| slot);
    for (slot, learned) in steps {
        match learned {
            Some(spell) => write(world, slot, u32::from(ACTION_KIND_SPELL) << 24 | spell),
            None => slot_changed(world, slot),
        }
    }
}

/// The bar's record callback (`0x5aadb0`), which a table put's slot set links on its item's record
/// (`0x5aaf0f`): a landed record of an item the bar holds runs that item's update.
pub(crate) fn record_landed(world: &mut World, item: u32) {
    let on_bar = world.get_resource::<ActionSlots>().is_some_and(|a| {
        a.buttons
            .values()
            .any(|b| b.kind == ACTION_KIND_ITEM && b.action == item)
    });
    if on_bar {
        update_item(world, item);
    }
}

/// The slot write in a handler's turn (`0x5aae80` with the send and the signal): stored, sent, then
/// [`slot_changed`]; a locked bar refuses it whole.
pub(crate) fn write(world: &mut World, slot: u8, packed: u32) {
    let written = world
        .run_system_cached_with(
            |In((slot, packed)): In<(u8, u32)>,
             mut actions: ResMut<ActionSlots>,
             net: Res<NetCommands>| actions.write_slot(slot, packed, &net),
            (slot, packed),
        )
        .unwrap_or(false);
    if written {
        slot_changed(world, slot);
    }
}

/// The slots whose spell is the melee auto-attack (`0x5aa4e0`) or, with `ranged`, the ranged one
/// (`0x5aa550`): their faces follow the weapon, so each signals.
fn weapon_slots(
    In(ranged): In<bool>,
    actions: Option<Res<ActionSlots>>,
    bound: Option<Res<crate::ui_macro::MacroBoundSpells>>,
    spells: Option<Res<Spells>>,
) -> Vec<u8> {
    let (Some(actions), Some(spells)) = (actions.as_deref(), spells.as_deref()) else {
        return Vec::new();
    };
    let bound = bound.as_deref();
    let mut out: Vec<u8> = actions
        .buttons
        .values()
        .filter(|b| {
            slot_spell(b, bound)
                .and_then(|s| spells.catalog.get(s))
                .is_some_and(|d| {
                    if ranged {
                        // `Attributes & 0x2` and `AttributesExB & 0x20` (`0x5a9760`).
                        d.ranged_icon_substitution()
                    } else {
                        // `Effect[0]` the auto-attack (`0x5a96d0`).
                        d.is_melee_auto_attack()
                    }
                })
        })
        .map(|b| b.slot)
        .collect();
    out.sort_unstable();
    out
}

/// The melee faces' update (`0x5aa4e0`), or with `ranged` the ranged one's (`0x5aa550`).
pub(crate) fn update_weapon_faces(world: &mut World, ranged: bool) {
    let slots = world
        .run_system_cached_with(weapon_slots, ranged)
        .unwrap_or_default();
    for slot in slots {
        slot_changed(world, slot);
    }
}

/// An aura of ours came or went (`0x5aab10`): each slot whose spell, through its macro too, is the
/// aura's signals, its `ActiveIconID` face following the aura.
pub(crate) fn aura_changed(world: &mut World, spell: u32) {
    if spell == 0 {
        return;
    }
    let slots = world
        .run_system_cached_with(
            |In(spell): In<u32>,
             actions: Option<Res<ActionSlots>>,
             bound: Option<Res<crate::ui_macro::MacroBoundSpells>>| {
                let Some(actions) = actions.as_deref() else {
                    return Vec::new();
                };
                let bound = bound.as_deref();
                let mut out: Vec<u8> = actions
                    .buttons
                    .values()
                    .filter(|b| slot_spell(b, bound) == Some(spell))
                    .map(|b| b.slot)
                    .collect();
                out.sort_unstable();
                out
            },
            spell,
        )
        .unwrap_or_default();
    for slot in slots {
        slot_changed(world, slot);
    }
}

/// Our companion came or went (`0x5aac00`), its creature entry `critter`: each slot whose spell
/// summons that creature, its first effect's second misc value a `SummonProperties.dbc` row,
/// signals once.
pub(crate) fn critter_changed(world: &mut World, critter: u32) {
    if critter == 0 {
        return;
    }
    let slots = world
        .run_system_cached_with(
            |In(critter): In<u32>,
             actions: Option<Res<ActionSlots>>,
             bound: Option<Res<crate::ui_macro::MacroBoundSpells>>,
             spells: Option<Res<Spells>>,
             summons: Option<Res<crate::spell::SummonSlots>>| {
                let (Some(actions), Some(spells), Some(summons)) =
                    (actions.as_deref(), spells.as_deref(), summons.as_deref())
                else {
                    return Vec::new();
                };
                let bound = bound.as_deref();
                let mut out: Vec<u8> = actions
                    .buttons
                    .values()
                    .filter(|b| {
                        slot_spell(b, bound)
                            .and_then(|s| spells.catalog.get(s))
                            .is_some_and(|d| {
                                let row = u32::try_from(d.effect_misc_value_b[0]).ok();
                                row.and_then(|r| summons.0.slot(r)).is_some()
                                    && (0..3).any(|i| {
                                        d.effects[i] == EFFECT_SUMMON
                                            && d.effect_misc_value[i] == critter as i32
                                    })
                            })
                    })
                    .map(|b| b.slot)
                    .collect();
                out.sort_unstable();
                out
            },
            critter,
        )
        .unwrap_or_default();
    for slot in slots {
        slot_changed(world, slot);
    }
}

/// A spell slot's icon, by the reference's resolver in order (`0x5a97f0`): the auto-attack faces
/// ([`super::weapon_icon::auto_attack_icon`]), then `ActiveIconID` while the spell's own aura is
/// live and cancelable ([`super::toggle::active_action_toggle`]), then the spell's icon. The
/// spellbook's `GetSpellTexture` never serves `ActiveIconID`.
#[allow(clippy::too_many_arguments)] // the resolver's reads, each at the call
pub(crate) fn spell_icon(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    spells: &Spells,
    store: Option<&ObjectStore>,
    auras: Option<&crate::unit_auras::UnitAuras>,
    objects: &Objects,
    items: &Items,
    icons: Option<&crate::entities::ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    super::weapon_icon::auto_attack_icon(d, store, &spells.forms, objects, items, icons, commands)
        .or_else(|| {
            super::toggle::active_action_toggle(spell_id, d, auras)
                .then(|| d.active_icon.clone())
                .flatten()
        })
        .or_else(|| d.icon.clone())
}
