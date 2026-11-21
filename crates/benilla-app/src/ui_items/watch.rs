//! The bags' field watches, fired in the update's handler turn as 12340's object watches fire them: the player's inventory, bag, pack, keyring and token slots (`0x6dedd0`, `0x5d9b40`, `0x5d9790`), each watched bag's own slot (`0x5d9600`), an item's count (`0x707720`) and its enchants (`0x707f50`), and the item locks they release (`0x513770`).

use benilla_protocol::ObjectFields;
use benilla_ui::script::ScriptValue;
use bevy::prelude::*;

use crate::net::Objects;
use crate::pending_item_ops::PendingItemOps;
use crate::ui_script::live::UiEvents;

/// The bags the UI watches (`0xc23540`): the four worn bags then the seven bank bags, by container id less one.
#[derive(Resource, Default)]
pub(crate) struct WatchedBags(pub(crate) [u64; 11]);

/// The player slot behind watched bag `i` (`0x5d9978`): 19..22 worn, 67..73 in the bank.
fn bag_inv_slot(i: usize) -> u8 {
    if i < 4 {
        19 + i as u8
    } else {
        63 + i as u8
    }
}

/// Runs `f` as its own handler step, so the events it queues fire before the next step changes state, as 12340's signals fire inside each call; with no event queue, at once.
fn step(world: &mut World, f: impl FnOnce(&mut World) + Send + Sync + 'static) {
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => ui.world(f),
        None => f(world),
    }
}

/// `ITEM_FLAG_PETITION`, the template bit the enchant watch returns on (`0x707f6b`).
const ITEM_FLAG_PETITION: u32 = 0x2000;

/// `ITEM_DFLAG_WRAPPED`, the item flag the bag search's state test passes (`0x7547a6`).
const ITEM_DFLAG_WRAPPED: u32 = 0x8;

/// An item's lock-event arguments, located as its signal locates it (`0x60bfe0`): a worn or bag slot by number, the pack as bag 0, the vault and bank bags as -1, the keyring -2, the tokens -4, else the worn or bank bag holding it; `None` where the player holds no such item.
pub(crate) fn lock_args(objects: &Objects, guid: u64) -> Option<Vec<ScriptValue>> {
    let me = objects.player_object()?;
    if guid == 0 {
        return None;
    }
    let pair = |bag: i64, slot: i64| Some(vec![ScriptValue::Int(bag), ScriptValue::Int(slot)]);
    if let Some(s) = (0..150u8).find(|&s| me.player_slot(s) == Some(guid)) {
        let s = i64::from(s);
        return match s {
            0..=22 => Some(vec![ScriptValue::Int(s + 1)]),
            23..=38 => pair(0, s - 22),
            39..=73 => pair(-1, s - 38),
            86..=117 => pair(-2, s - 85),
            118..=149 => pair(-4, s - 117),
            _ => None,
        };
    }
    for (i, bag_slot) in (19..=22u8).chain(67..=73).enumerate() {
        let Some(bag) = me
            .player_slot(bag_slot)
            .filter(|&g| g != 0)
            .and_then(|g| objects.object(g))
        else {
            continue;
        };
        let n = bag.container_num_slots().unwrap_or(0).min(36) as u8;
        if let Some(j) = (0..n).find(|&j| bag.container_slot(j) == Some(guid)) {
            return pair(i as i64 + 1, i64::from(j) + 1);
        }
    }
    None
}

/// The lock edge's two signals at the item's place (`0x706ff0`, `0x707001`; `0x707030`, `0x707041`): `ITEM_LOCK_CHANGED`, then `ITEM_LOCKED` or `ITEM_UNLOCKED`.
pub(crate) fn fire_lock_edge(ui: &mut UiEvents, locked: bool, args: Vec<ScriptValue>) {
    ui.fire("ITEM_LOCK_CHANGED", args.clone());
    if locked {
        ui.fire("ITEM_LOCKED", args);
    } else {
        ui.fire("ITEM_UNLOCKED", args);
    }
}

/// [`fire_lock_edge`] for a caller holding the VM.
pub(crate) fn fire_lock_edge_now(
    script: &mut benilla_ui::script::UiScript,
    locked: bool,
    args: Vec<ScriptValue>,
) {
    script.fire_event("ITEM_LOCK_CHANGED", args.clone());
    if locked {
        script.fire_event("ITEM_LOCKED", args);
    } else {
        script.fire_event("ITEM_UNLOCKED", args);
    }
}

/// One lock or unlock a step makes (`0x706fe0`, `0x707020`), with its edge's arguments; `None`
/// arguments write only.
pub(crate) struct LockStep {
    pub(crate) guid: u64,
    pub(crate) locked: bool,
    pub(crate) args: Option<Vec<ScriptValue>>,
}

/// Write `lock` and, on the bit's edge, signal at its place with the `World` lent before anything
/// after it runs (`0x706feb`, `0x70702b`), so its listener reads the later locks still unwritten.
pub(crate) fn lock_now(world: &mut World, lock: LockStep) {
    let LockStep { guid, locked, args } = lock;
    let changed = world
        .get_resource_mut::<PendingItemOps>()
        .is_some_and(|mut p| if locked { p.lock(guid) } else { p.unlock(guid) });
    if let (true, Some(args)) = (changed, args) {
        crate::ui_script::safe_world::lend(world, |s| fire_lock_edge_now(s, locked, args));
    }
}

/// Run `step`, a send that raises in turn, then fire what it raised and write each lock it made,
/// each lock's edge signalled before the next is written (`0x706fe0` -> `0x706feb`).
pub(crate) fn run_lock_step<I: Send + 'static, M>(
    world: &mut World,
    step: impl IntoSystem<In<I>, Vec<LockStep>, M> + 'static,
    input: I,
) {
    let locks = world
        .run_system_cached_with(step, input)
        .unwrap_or_default();
    crate::ui_script::live::fire_handler_events(world);
    for lock in locks {
        lock_now(world, lock);
    }
}

/// Run `step` on the lent `World` in the caller's turn: for a step that holds the `World` while it
/// raises in turn (an applier inside its call), behind the raises made so far; at once otherwise.
pub(crate) fn world_in_turn(
    script: &mut benilla_ui::script::UiScript,
    step: impl FnOnce(&mut World) + Send + Sync + 'static,
) {
    benilla_ui::script::world_loan::in_turn_or_now(script, move |_| {
        crate::ui_script::safe_world::with_world(step)
            .expect("a step in turn runs where the `World` is lent");
    });
}

/// The lock and its edge, then the send it guards, both in the caller's turn (`0x6e0136` before
/// `0x6e016e`).
pub(crate) fn lock_then_send_in_turn(
    script: &mut benilla_ui::script::UiScript,
    lock: LockStep,
    send: crate::net::ClientCommand,
) {
    world_in_turn(script, move |w| {
        lock_now(w, lock);
        send_now(w, send);
    });
}

/// Put `send` on the wire.
pub(crate) fn send_now(world: &World, send: crate::net::ClientCommand) {
    if let Some(commands) = world.get_resource::<crate::net::NetCommands>() {
        let _ = commands.0.send(send);
    }
}

/// The item unlock (`0x513770` -> `0x707020`): a locked item unlocks and signals at its place; an unlocked or absent one does nothing.
pub(crate) fn unlock_item(world: &mut World, guid: u64) {
    let unlocked = world
        .get_resource_mut::<PendingItemOps>()
        .is_some_and(|mut p| p.unlock(guid));
    if unlocked {
        signal_unlocked(world, guid);
    }
}

/// The unlock edge's signals at the item's place, for an unlock already applied (`0x707030`, `0x707041`).
pub(crate) fn signal_unlocked(world: &mut World, guid: u64) {
    let args = world
        .run_system_cached_with(
            |In(guid): In<u64>, objects: Objects| lock_args(&objects, guid),
            guid,
        )
        .ok()
        .flatten();
    if let (Some(args), Some(mut ui)) = (args, world.get_resource_mut::<UiEvents>()) {
        fire_lock_edge(&mut ui, false, args);
    }
}

/// Whether an item passes the bag search's state test (`0x754792`-`0x7547c7`): wrapped, unbreakable or unbroken, and not flagged `0x10`.
fn search_state_ok(item: &ObjectFields) -> bool {
    let flags = item.item_flags().unwrap_or(0);
    let whole = flags & ITEM_DFLAG_WRAPPED != 0
        || item.item_max_durability().unwrap_or(0) == 0
        || item.item_durability().unwrap_or(0) != 0;
    whole && flags & 0x10 == 0
}

/// The player's items of `entry` in the bag search's order (`0x7546f0`): pack, worn bags each with its contents, keyring, vault and bank bags with `bank`, tokens, then the worn slots.
fn items_of_type(objects: &Objects, entry: u32, bank: bool) -> Vec<u64> {
    let Some(me) = objects.player_object() else {
        return Vec::new();
    };
    let mut ranges: Vec<std::ops::RangeInclusive<u8>> = vec![23..=38, 19..=22, 86..=117];
    if bank {
        ranges.extend([39..=66, 67..=73]);
    }
    ranges.extend([118..=149, 0..=18]);
    let mut found = Vec::new();
    let visit = |guid: u64, found: &mut Vec<u64>| -> Option<&ObjectFields> {
        let item = objects.object(guid)?;
        if !search_state_ok(item) {
            return None;
        }
        if item.object_entry() == Some(entry) {
            found.push(guid);
        }
        Some(item)
    };
    for slot in ranges.into_iter().flatten() {
        let Some(guid) = me.player_slot(slot).filter(|&g| g != 0) else {
            continue;
        };
        let Some(item) = visit(guid, &mut found) else {
            continue;
        };
        let n = item.container_num_slots().unwrap_or(0).min(36) as u8;
        for g in (0..n).filter_map(|j| item.container_slot(j).filter(|&g| g != 0)) {
            visit(g, &mut found);
        }
    }
    found
}

/// The unlock of every item of a kind (`0x754d90`), the bank's with `bank` (flag 8, `0x7077b8`); its callback skips a trade, mail or auction item (`0x753fa0`), and none of those is in this lock set, as each window locks its own.
fn unlock_items_of_type(world: &mut World, entry: u32, bank: bool) {
    let found = world
        .run_system_cached_with(
            |In((entry, bank)): In<(u32, bool)>, objects: Objects| {
                items_of_type(&objects, entry, bank)
            },
            (entry, bank),
        )
        .unwrap_or_default();
    for guid in found {
        step(world, move |world| unlock_item(world, guid));
    }
}

/// `BAG_UPDATE` for a container's guid (`0x5d7070`): ours is the backpack, keyring and tokens (`0x5d7098`-`0x5d70ba`), a watched bag its id (`0x5d70ff`), anything else nothing.
fn contents_update(world: &mut World, guid: u64) {
    // The bar's refresh first (`0x5d7074`).
    crate::ui_action::usable_cache::step(crate::ui_action::usable_cache::Pass::Bar)(world);
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    let ids: Vec<i64> = if me == Some(guid) {
        vec![0, -2, -4]
    } else {
        world
            .get_resource::<WatchedBags>()
            .and_then(|w| w.0.iter().position(|&g| g == guid))
            .map(|i| vec![i as i64 + 1])
            .unwrap_or_default()
    };
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    for id in ids {
        ui.fire("BAG_UPDATE", vec![ScriptValue::Int(id)]);
    }
}

/// The watched bags' refresh (`0x5d9960`), bag by bag: a bag gone fires `BAG_CLOSED` while still watched (`0x5d9a87`), the array takes the new bag (`0x5d9a96`), then one there fires `BAG_UPDATE` (`0x5d9b16`).
pub(crate) fn update_containers(world: &mut World) {
    for i in 0..11 {
        step(world, move |world| refresh_bag(world, i));
    }
}

/// One watched bag's turn of [`update_containers`].
fn refresh_bag(world: &mut World, i: usize) {
    let guid = world
        .run_system_cached_with(
            |In(i): In<usize>, objects: Objects| {
                Some(
                    objects
                        .player_object()?
                        .player_slot(bag_inv_slot(i))
                        .unwrap_or(0),
                )
            },
            i,
        )
        .ok()
        .flatten();
    let Some(guid) = guid else {
        return;
    };
    let old = world.get_resource_or_init::<WatchedBags>().0[i];
    if guid == old {
        return;
    }
    if old != 0 {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("BAG_CLOSED", vec![ScriptValue::Int(i as i64 + 1)]);
        }
    }
    step(world, move |world| {
        world.resource_mut::<WatchedBags>().0[i] = guid;
        if guid == 0 {
            return;
        }
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("BAG_UPDATE", vec![ScriptValue::Int(i as i64 + 1)]);
        }
    });
}

/// The containers' world entry (`0x5d9c20`): the watched bags cleared, then refreshed, each present bag's `BAG_UPDATE` with it.
pub(crate) fn enter_world(world: &mut World) {
    world.get_resource_or_init::<WatchedBags>().0 = [0; 11];
    update_containers(world);
}

/// One of our player slot fields moved, `old` to `new`, as its watches run in turn: the inventory watch our player registered first (`0x6e51de`), then the containers' (`0x5d9c20`).
pub(crate) fn inv_slot_changed(world: &mut World, slot: u8, old: u64, new: u64) {
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    // The inventory watch (`0x6dedd0`), on the worn, vault, bank-bag, keyring and token slots (`0x6e51c5`-`0x6e52c3`): a changed slot's new item unlocks (`0x6dee39`), a worn slot fires its equipment event (`0x6dee81`, `0x6deea2`), a vault or bank-bag slot its bank event (`0x6deed6`).
    if matches!(slot, 0..=22 | 39..=73 | 86..=149) {
        if old != new {
            unlock_item(world, new);
        }
        // The bar's weapon faces (`0x6dee53`, `0x6dee5d`).
        match slot {
            15 | 16 => crate::ui_action::slots::update_weapon_faces(world, false),
            17 => crate::ui_action::slots::update_weapon_faces(world, true),
            _ => {}
        }
        if let (0..=18, Some(mut ui)) = (slot, world.get_resource_mut::<UiEvents>()) {
            let id = i64::from(slot) + 1;
            if new != 0 {
                ui.fire(
                    "PLAYER_EQUIPMENT_CHANGED",
                    vec![ScriptValue::Int(id), ScriptValue::Int(1)],
                );
            } else {
                ui.fire("PLAYER_EQUIPMENT_CHANGED", vec![ScriptValue::Int(id)]);
            }
        }
        if (39..=73).contains(&slot) {
            if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                ui.fire(
                    "PLAYERBANKSLOTS_CHANGED",
                    vec![ScriptValue::Int(i64::from(slot) - 38)],
                );
            }
        }
    }
    match slot {
        // The bag slots' watch (`0x5d9b40`): the bar's item update for each item the bag holds
        // (`0x5d9be3`), our contents, then the watched bags (`0x5d9bf9`, `0x5d9bfe`).
        19..=22 | 67..=73 => {
            for entry in bag_entries(world, new) {
                crate::ui_action::slots::update_item(world, entry);
            }
            if let Some(me) = me {
                contents_update(world, me);
            }
            update_containers(world);
        }
        // The pack, keyring and token slots' watch (`0x5d9790`): a moved slot's old and new items'
        // bar updates (`0x5d9839`, `0x5d984f`), the new item unlocks (`0x5d9863`), then our
        // contents (`0x5d98e3`).
        23..=38 | 86..=149 => {
            if old != new {
                bar_item(world, old);
                bar_item(world, new);
            }
            unlock_item(world, new);
            if let Some(me) = me {
                contents_update(world, me);
            }
        }
        _ => {}
    }
}

/// A watched bag's slot `j` moved (`0x5d9600`), registered on each of its slots (`0x5d9af8`), the
/// slot guid's old halves `old` where they moved: a moved slot's old and new items' bar updates
/// (`0x5d969c`, `0x5d96b2`), the slot's new item unlocks (`0x5d96c6`), then the bag's `BAG_UPDATE`
/// (`0x5d9746`).
pub(crate) fn bag_slot_changed(
    world: &mut World,
    bag: u64,
    j: u8,
    old: (Option<u32>, Option<u32>),
) {
    let watched = world
        .get_resource::<WatchedBags>()
        .is_some_and(|w| bag != 0 && w.0.contains(&bag));
    if !watched {
        return;
    }
    let new = world
        .run_system_cached_with(
            |In((bag, j)): In<(u64, u8)>, objects: Objects| {
                let b = objects.object(bag)?;
                let n = b.container_num_slots().unwrap_or(0);
                (u32::from(j) < n).then(|| b.container_slot(j).unwrap_or(0))
            },
            (bag, j),
        )
        .ok()
        .flatten();
    let Some(new) = new else {
        return;
    };
    let (lo, hi) = old;
    let old =
        (u64::from(hi.unwrap_or((new >> 32) as u32)) << 32) | u64::from(lo.unwrap_or(new as u32));
    if old != new {
        bar_item(world, old);
        bar_item(world, new);
    }
    unlock_item(world, new);
    contents_update(world, bag);
}

/// The entries of the items bag `bag` holds, in slot order.
fn bag_entries(world: &mut World, bag: u64) -> Vec<u32> {
    if bag == 0 {
        return Vec::new();
    }
    world
        .run_system_cached_with(
            |In(bag): In<u64>, objects: Objects| {
                let Some(b) = objects.object(bag) else {
                    return Vec::new();
                };
                let n = b.container_num_slots().unwrap_or(0).min(36) as u8;
                (0..n)
                    .filter_map(|j| b.container_slot(j))
                    .filter(|&g| g != 0)
                    .filter_map(|g| objects.object(g)?.object_entry())
                    .collect()
            },
            bag,
        )
        .unwrap_or_default()
}

/// The item's entry and container when we own it, read as the watches read them.
fn owned_item(world: &mut World, item: u64) -> Option<(u32, u64)> {
    world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects, self_guid: Option<Res<crate::net::SelfGuid>>| {
                let fields = objects.object(item)?;
                let me = self_guid.and_then(|s| s.0)?;
                if fields.item_owner() != Some(me) {
                    return None;
                }
                Some((fields.object_entry().unwrap_or(0), fields.item_contained()?))
            },
            item,
        )
        .ok()
        .flatten()
}

/// An item's entry, and the player slot it is worn in, if any.
fn entry_and_slot(world: &mut World, item: u64) -> Option<(u32, Option<u8>)> {
    world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects| {
                let fields = objects.object(item)?;
                let me = objects.player_object();
                let slot = me.and_then(|me| (0..19u8).find(|&s| me.player_slot(s) == Some(item)));
                Some((fields.object_entry().unwrap_or(0), slot))
            },
            item,
        )
        .ok()
        .flatten()
}

/// The bar's updates for an item of ours: the melee faces when it is worn in a hand, the ranged
/// face in the ranged slot (`0x70792d`, `0x707934`; `0x70add6`, `0x70addd`), then its entry's item
/// update (`0x5aa5c0`).
fn bar_item_with_faces(world: &mut World, item: u64) {
    let Some((entry, slot)) = entry_and_slot(world, item) else {
        return;
    };
    match slot {
        Some(15 | 16) => crate::ui_action::slots::update_weapon_faces(world, false),
        Some(17) => crate::ui_action::slots::update_weapon_faces(world, true),
        _ => {}
    }
    crate::ui_action::slots::update_item(world, entry);
}

/// The bar's item update (`0x5aa5c0`) for the entry of item `guid`, 0 or an unknown guid none.
fn bar_item(world: &mut World, item: u64) {
    if let Some((entry, _)) = entry_and_slot(world, item) {
        crate::ui_action::slots::update_item(world, entry);
    }
}

/// The containers' item refresh (`0x5d7320`): an item we own fires its container's `BAG_UPDATE`.
pub(crate) fn item_updated(world: &mut World, item: u64) {
    if let Some((_, contained)) = owned_item(world, item) {
        contents_update(world, contained);
    }
}

/// The character page's item refresh (`0x5e84c0`): a vault or bank-bag slot holding the item, or the bag holding it, fires that slot's `PLAYERBANKSLOTS_CHANGED` (`0x5e855e`).
fn character_item_updated(world: &mut World, item: u64) {
    let slot = world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects, self_guid: Option<Res<crate::net::SelfGuid>>| {
                let fields = objects.object(item)?;
                let me = objects.player_object()?;
                let contained = fields.item_contained()?;
                let slot_of = |g: u64| (0..150u8).find(|&s| me.player_slot(s) == Some(g));
                let ours = self_guid.and_then(|s| s.0) == Some(contained);
                slot_of(item).or_else(|| (!ours).then(|| slot_of(contained)).flatten())
            },
            item,
        )
        .ok()
        .flatten();
    let bank = slot.filter(|s| (39..=73).contains(s));
    if let (Some(s), Some(mut ui)) = (bank, world.get_resource_mut::<UiEvents>()) {
        ui.fire(
            "PLAYERBANKSLOTS_CHANGED",
            vec![ScriptValue::Int(i64::from(s) - 38)],
        );
    }
}

/// The entry watch's record callback (`0x7078e0`): the bar's faces and item update (`0x70792d`-`0x70793d`), its container's `BAG_UPDATE` (`0x707949`) and the character page's slot event (`0x707955`).
pub(crate) fn entry_record(world: &mut World, item: u64) {
    bar_item_with_faces(world, item);
    item_updated(world, item);
    character_item_updated(world, item);
}

/// The template's stack size and flags, `None` uncached.
fn template_of(world: &World, entry: u32) -> Option<(u32, u32)> {
    world
        .get_resource::<crate::items::Items>()?
        .template_cached(entry)
        .map(|t| (t.stackable, t.flags))
}

/// An item's stack count moved (`0x707720`), on an item we own (`0x707757`): a stackable kind unlocks every item of it, the bank's too (`0x7077c1`), anything else the item alone (`0x7077d2`); then the bar's item update (`0x7077e7`), its container's `BAG_UPDATE` (`0x707805`) and the character page's slot event (`0x707814`).
pub(crate) fn stack_changed(world: &mut World, item: u64) {
    let Some((entry, _)) = owned_item(world, item) else {
        return;
    };
    match template_of(world, entry) {
        Some((stackable, _)) if stackable > 1 => unlock_items_of_type(world, entry, true),
        _ => unlock_item(world, item),
    }
    crate::ui_action::slots::update_item(world, entry);
    item_updated(world, item);
    character_item_updated(world, item);
}

/// An item's spell charges moved (`0x707880`): on an item we own, the bar's item update
/// (`0x7078c3`), then the bags' cooldown signal (`0x7078cb`).
pub(crate) fn charges_changed(world: &mut World, item: u64) {
    let ours = world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects, self_guid: Option<Res<crate::net::SelfGuid>>| {
                let me = self_guid.and_then(|s| s.0);
                me.is_some() && objects.object(item).and_then(|f| f.item_owner()) == me
            },
            item,
        )
        .unwrap_or(false);
    if !ours {
        return;
    }
    bar_item(world, item);
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.fire("BAG_UPDATE_COOLDOWN", Vec::new());
    }
}

/// An item's enchants moved (`0x708bd0` -> `0x707f50`): a charter returns (`0x707f6b`), an unheld owner too (`0x707f9e`); on ours (`0x707fb8`), no unlock, the bar's item update (`0x707fc6`), its container's `BAG_UPDATE` (`0x707fe4`) and the character page's slot event (`0x707ff3`).
pub(crate) fn enchants_changed(world: &mut World, item: u64) {
    let found = world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects| {
                let fields = objects.object(item)?;
                let owner = fields
                    .item_owner()
                    .filter(|&o| objects.object(o).is_some())?;
                Some((fields.object_entry().unwrap_or(0), owner))
            },
            item,
        )
        .ok()
        .flatten();
    let Some((entry, owner)) = found else {
        return;
    };
    if template_of(world, entry).is_some_and(|(_, flags)| flags & ITEM_FLAG_PETITION != 0) {
        return;
    }
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    if Some(owner) != me {
        return;
    }
    crate::ui_action::slots::update_item(world, entry);
    item_updated(world, item);
    character_item_updated(world, item);
}

/// The post-init's unlock on an item we own (`0x70ad20`): a single item unlocks itself (`0x70ad9e`), a stackable kind every item of it outside the bank (`0x70ad8d`); then the bar's faces and item update (`0x70add6`-`0x70adf0`).
pub(crate) fn post_init_unlock(world: &mut World, item: u64) {
    let Some((entry, _)) = owned_item(world, item) else {
        return;
    };
    match template_of(world, entry) {
        Some((stackable, _)) if stackable > 1 => unlock_items_of_type(world, entry, false),
        _ => unlock_item(world, item),
    }
    bar_item_with_faces(world, item);
}

/// `SMSG_OPEN_CONTAINER` (`0x6e32d0` -> `0x5d7110`): `BAG_OPEN` with 0 for our own guid, a watched bag's id for one of them, nothing else.
pub(crate) fn open_container(world: &mut World, item: u64) {
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    let ids: Vec<i64> = if me == Some(item) {
        vec![0]
    } else {
        world
            .get_resource::<WatchedBags>()
            .map(|w| {
                w.0.iter()
                    .enumerate()
                    .filter(|&(_, &g)| g == item && item != 0)
                    .map(|(i, _)| i as i64 + 1)
                    .collect()
            })
            .unwrap_or_default()
    };
    if ids.is_empty() {
        debug!("open container {item:#x}: not one of our bags — no BAG_OPEN");
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        for id in ids {
            ui.fire("BAG_OPEN", vec![ScriptValue::Int(id)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::UiScript;

    const A: u64 = 0x4000_0000_0000_0A01;
    const B: u64 = 0x4000_0000_0000_0B02;

    /// A step that locks both ends of a swap.
    fn swap_locks(In(()): In<()>) -> Vec<LockStep> {
        [A, B]
            .into_iter()
            .map(|guid| LockStep {
                guid,
                locked: true,
                args: Some(vec![ScriptValue::Int(guid as i64 & 0xff)]),
            })
            .collect()
    }

    /// Each lock's listener runs before the next lock is written, and the drain's later reads see
    /// every lock (`0x706fe0` -> `0x706feb`, once per end).
    #[test]
    fn each_locks_listener_reads_the_later_lock_unwritten() {
        let mut world = World::new();
        world.init_resource::<PendingItemOps>();
        world.init_resource::<UiEvents>();
        let script = UiScript::new().expect("a VM");
        let locked = script
            .lua()
            .create_function(|_, guid: i64| {
                Ok(crate::ui_script::safe_world::with_world(|w| {
                    w.resource::<PendingItemOps>()
                        .is_locked(guid as u64 | 0x4000_0000_0000_0000)
                }))
            })
            .unwrap();
        script.lua().globals().set("ProbeLocked", locked).unwrap();
        script
            .run(
                r#"SEEN = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("ITEM_LOCKED")
                   f:SetScript("OnEvent", function(_, _, id)
                       SEEN[#SEEN + 1] = id .. ":" .. tostring(ProbeLocked(0x0A01))
                           .. "/" .. tostring(ProbeLocked(0x0B02))
                   end)"#,
            )
            .unwrap();
        world.insert_non_send_resource(script);
        run_lock_step(&mut world, swap_locks, ());
        let seen = world
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return SEEN")
            .unwrap();
        assert_eq!(seen, ["1:true/false", "2:true/true"]);
        let ops = world.resource::<PendingItemOps>();
        assert!(ops.is_locked(A) && ops.is_locked(B));
    }
}
