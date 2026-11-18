//! The bags' field watches, fired in the update's handler turn as 12340's object watches fire them: the player's inventory, bag, pack, keyring and token slots (`0x6dedd0`, `0x5d9b40`, `0x5d9790`), the watched bags' own slots (`0x5d9600`) and an item's count or enchants (`0x5d7320`), each unlocking the item that landed before its `BAG_UPDATE`.

use benilla_ui::script::{item_lock_args, ScriptValue};
use bevy::prelude::*;

use crate::net::{ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::ui_script::live::UiEvents;

use super::slot_guid_count;

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

/// `BAG_UPDATE` for a container's guid (`0x5d7070`): ours is the backpack, keyring and tokens (`0x5d7098`-`0x5d70ba`), a watched bag its id (`0x5d70ff`), anything else nothing.
fn contents_update(world: &mut World, guid: u64) {
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

/// The item unlock a slot's landing makes (`0x513770` -> `0x707020`): each send whose slots moved on releases, `ITEM_LOCK_CHANGED` naming each of its slots.
fn unlock_landed(world: &mut World) {
    let unlocked = world
        .run_system_cached(
            |me: Query<&ObjectStore, With<SelfPlayer>>,
             objects: Objects,
             pending: Option<ResMut<PendingItemOps>>| {
                let (Some(mut pending), Some(player)) = (pending, me.iter().next()) else {
                    return Vec::new();
                };
                if pending.is_empty() {
                    return Vec::new();
                }
                pending.resolve(|bag, slot| slot_guid_count(Some(player), bag, slot, &objects))
            },
        )
        .unwrap_or_default();
    fire_lock_changes(world, unlocked);
}

/// `ITEM_LOCK_CHANGED` for each `(bag, slot)` whose lock flipped, located as the item's signal locates it (`0x60bfe0`).
pub(crate) fn fire_lock_changes(world: &mut World, slots: Vec<(i64, u32)>) {
    if slots.is_empty() {
        return;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        for (bag, slot) in slots {
            ui.fire("ITEM_LOCK_CHANGED", item_lock_args(bag, slot));
        }
    }
}

/// The watched bags' refresh (`0x5d9960`): a bag gone fires `BAG_CLOSED` (`0x5d99f9`), one there fires `BAG_UPDATE` (`0x5d9aa1`), each by id.
pub(crate) fn update_containers(world: &mut World) {
    let slots: Option<[u64; 11]> = world
        .run_system_cached(|objects: Objects| {
            let me = objects.player_object()?;
            Some(std::array::from_fn(|i| {
                me.player_slot(bag_inv_slot(i)).unwrap_or(0)
            }))
        })
        .ok()
        .flatten();
    let Some(slots) = slots else {
        return;
    };
    let mut watched = world.get_resource_or_init::<WatchedBags>().0;
    let mut events = Vec::new();
    for (i, &guid) in slots.iter().enumerate() {
        if guid == watched[i] {
            continue;
        }
        if watched[i] != 0 {
            events.push(("BAG_CLOSED", i as i64 + 1));
        }
        watched[i] = guid;
        if guid != 0 {
            events.push(("BAG_UPDATE", i as i64 + 1));
        }
    }
    world.resource_mut::<WatchedBags>().0 = watched;
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        for (event, id) in events {
            ui.fire(event, vec![ScriptValue::Int(id)]);
        }
    }
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
    // The inventory watch (`0x6dedd0`), on every slot but the pack's (`0x6e51fd`): a changed slot's item unlocks (`0x6dee39`), a vault or bank-bag slot fires its event (`0x6deed6`).
    if !(23..=38).contains(&slot) {
        if old != new {
            unlock_landed(world);
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
        // The bag slots' watch (`0x5d9b40`): our contents, then the watched bags (`0x5d9bf9`, `0x5d9bfe`).
        19..=22 | 67..=73 => {
            if let Some(me) = me {
                contents_update(world, me);
            }
            update_containers(world);
        }
        // The pack, keyring and token slots' watch (`0x5d9790`): the item unlocks (`0x5d9863`), then our contents (`0x5d98e3`).
        23..=38 | 86..=149 => {
            unlock_landed(world);
            if let Some(me) = me {
                contents_update(world, me);
            }
        }
        _ => {}
    }
}

/// A watched bag's own slot field moved (`0x5d9600`): the landed item unlocks (`0x5d96c6`), then the bag's `BAG_UPDATE` (`0x5d9746`).
pub(crate) fn bag_slot_changed(world: &mut World, bag: u64) {
    let watched = world
        .get_resource::<WatchedBags>()
        .is_some_and(|w| bag != 0 && w.0.contains(&bag));
    if !watched {
        return;
    }
    unlock_landed(world);
    contents_update(world, bag);
}

/// An item's count or enchants moved (`0x707720`, `0x707f50`): ours unlocks (`0x7077d2`), then its container's `BAG_UPDATE` (`0x707805` -> `0x5d7320`), then a vault or bank-bag slot holding it or its bag fires that slot's event (`0x707814` -> `0x5e84c0`, `0x5e855e`).
pub(crate) fn item_changed(world: &mut World, item: u64) {
    let found = world
        .run_system_cached_with(
            |In(item): In<u64>, objects: Objects, self_guid: Option<Res<crate::net::SelfGuid>>| {
                let fields = objects.object(item)?;
                let me = objects.player_object()?;
                let contained = fields.item_contained()?;
                let slot_of = |g: u64| (0..150u8).find(|&s| me.player_slot(s) == Some(g));
                let ours = self_guid.and_then(|s| s.0) == Some(contained);
                let slot = slot_of(item).or_else(|| (!ours).then(|| slot_of(contained)).flatten());
                Some((fields.item_owner(), contained, slot))
            },
            item,
        )
        .ok()
        .flatten();
    let Some((owner, contained, slot)) = found else {
        return;
    };
    let me = world
        .get_resource::<crate::net::SelfGuid>()
        .and_then(|s| s.0);
    if owner.is_none() || owner != me {
        return;
    }
    unlock_landed(world);
    contents_update(world, contained);
    let bank = slot.filter(|s| (39..=73).contains(s));
    if let (Some(s), Some(mut ui)) = (bank, world.get_resource_mut::<UiEvents>()) {
        ui.fire(
            "PLAYERBANKSLOTS_CHANGED",
            vec![ScriptValue::Int(i64::from(s) - 38)],
        );
    }
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
