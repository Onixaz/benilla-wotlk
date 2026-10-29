//! The app side of the container seam around `benilla_ui::script`'s `container` module.
//!
//! [`feed`] builds each bag from the player's descriptor (the backpack's `PACK_SLOT` array, the
//! equipped bags at `INV_SLOT` 19-22 with their own `CONTAINER_FIELD_SLOT` arrays, the keyring at
//! 86.. with no container object, like the bank), the object index and the template cache, and
//! fires `BAG_UPDATE(bagID)` per changed bag. [`drain`] sends the Lua intents over [`wire_pos`]:
//! an equippable item that starts no quest goes out as `CMSG_AUTOEQUIP_ITEM`, anything else
//! through [`send_item_use`]. A move or split locks both ends and a destroy its slot in
//! [`PendingItemOps`], until the descriptor resolves them or the server answers a non-zero
//! `SMSG_INVENTORY_CHANGE_FAILURE`.

use benilla_protocol::messages::{
    BAG_PLAYER_INVENTORY, SLOT_BAG_FIRST, SLOT_BANK_BAG_FIRST, SLOT_BANK_FIRST, SLOT_KEYRING_FIRST,
    SLOT_PACK_FIRST,
};
pub(crate) use benilla_protocol::messages::{BANK_BAGS, BANK_SLOTS, KEYRING_SLOTS};
use benilla_protocol::ObjectFields;
use benilla_ui::script::EQUIPMENT_BAG;
use bevy::prelude::*;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

mod drain;
mod equip_error;
pub(crate) use equip_error::equip_error_key;
pub(crate) mod feed;
mod net;

pub(crate) use drain::send_auto_equip;
use drain::{
    drain_bag_autostores, drain_container_autoequips, drain_container_destroys,
    drain_container_moves, drain_container_repairs,
};
// `UseContainerItem` and `UseInventoryItem`, applied in call order by `crate::script_calls`.
pub(crate) use drain::ScriptItemUse;
use feed::{
    feed_containers, feed_item_sets, feed_item_stats, feed_player_req, feed_random_properties,
};

/// The backpack's capacity (`PLAYER_FIELD_PACK_SLOT_1..`).
pub(super) const PACK_SLOTS: u8 = 16;
/// Worn equipment, `INV_SLOT` 0..18 (head through tabard): the first region of the walker.
pub(super) const EQUIPMENT_SLOTS: u8 = 19;
/// The first equipped-bag inventory slot (`INV_SLOT` 19..22 hold bags 1..4).
pub(super) const BAG_SLOT_FIRST: u8 = 19;
/// Equipped bag count (live-API bag ids 1..=4).
pub(super) const BAGS: u8 = 4;
/// The bank's first item slot in the player array; its 28 stream at login like the backpack.
pub(super) const BANK_SLOT_FIRST: u8 = SLOT_BANK_FIRST;
/// The first bank-bag slot; as with an equipped bag, a bank bag's slot number is its wire bag
/// byte.
pub(super) const BANK_BAG_SLOT_FIRST: u8 = SLOT_BANK_BAG_FIRST;
/// `BANK_CONTAINER` (`BankFrame.lua:1`).
pub(crate) const BANK_CONTAINER: i64 = -1;
/// Bank bags are containers 5..=11 (`NUM_BAG_SLOTS + 1..`, `BankFrame.xml` ids 5-11).
pub(crate) const BANK_BAG_ID_FIRST: i64 = 5;
/// The last bank-bag container.
pub(crate) const BANK_BAG_ID_LAST: i64 = BANK_BAG_ID_FIRST + BANK_BAGS as i64 - 1;
/// `KEYRING_CONTAINER` (`MainMenuBarBagButtons.lua:1`).
pub(crate) const KEYRING_CONTAINER: i64 = -2;
/// The first keyring slot in the player array.
pub(super) const KEYRING_SLOT_FIRST: u8 = SLOT_KEYRING_FIRST;
/// The bank-bag buttons' live ids, `BankButtonIDToInvSlotID(5..=11, 1)`: 68..=74.
const BANK_BAG_LIVE: std::ops::RangeInclusive<u32> =
    BANK_BAG_SLOT_FIRST as u32 + 1..=(BANK_BAG_SLOT_FIRST + BANK_BAGS) as u32;
/// `BAG_FAMILY_KEYS`: what the server routes into the keyring (`Player::_CanStoreItem`) and the
/// reference's `HasKey` searches for.
pub(super) const BAG_FAMILY_KEYS: u32 = 9;

/// Lua `(bag, 1-based slot)` to the wire `(bag_index, slot)`, the one mapping every drain shares.
/// The backpack, the bank's generic slots, the keyring and the doll ([`EQUIPMENT_BAG`]: live id
/// minus one, 1..=23 and the bank-bag buttons 68..=74) all land on [`BAG_PLAYER_INVENTORY`], so a
/// move between them is `CMSG_SWAP_INV_ITEM`; an equipped or bank bag is addressed by its own
/// player-array slot, with a 0-based inner slot.
pub(crate) fn wire_pos(bag: i64, slot1: u32) -> Option<(u8, u8)> {
    let slot0 = u8::try_from(slot1.checked_sub(1)?).ok()?;
    match bag {
        0 if slot0 < PACK_SLOTS => Some((BAG_PLAYER_INVENTORY, SLOT_PACK_FIRST + slot0)),
        1..=4 if slot0 < 36 => Some((SLOT_BAG_FIRST + (bag as u8 - 1), slot0)),
        BANK_CONTAINER if slot0 < BANK_SLOTS => {
            Some((BAG_PLAYER_INVENTORY, BANK_SLOT_FIRST + slot0))
        }
        BANK_BAG_ID_FIRST..=BANK_BAG_ID_LAST if slot0 < 36 => {
            Some((BANK_BAG_SLOT_FIRST + (bag - BANK_BAG_ID_FIRST) as u8, slot0))
        }
        KEYRING_CONTAINER if slot0 < KEYRING_SLOTS => {
            Some((BAG_PLAYER_INVENTORY, KEYRING_SLOT_FIRST + slot0))
        }
        EQUIPMENT_BAG if (1..=23).contains(&slot1) || BANK_BAG_LIVE.contains(&slot1) => {
            Some((BAG_PLAYER_INVENTORY, slot0))
        }
        _ => None,
    }
}

/// One inventory refusal off the wire, with the fills of the only two reasons that format one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EquipError {
    /// The wire `InventoryResult`.
    pub reason: u8,
    /// Reason 1's `%d`, the packet's `requiredLevel`.
    pub required_level: Option<u32>,
    /// Reason 16's `%s` source: the destination bag's player slot (255 names no bag), which the
    /// drain resolves to the bag's `BagFamily` name.
    pub bag_slot: u8,
}

/// Inventory refusals (`SMSG_INVENTORY_CHANGE_FAILURE`) queued for the UI error line.
#[derive(Resource, Default)]
pub(crate) struct EquipErrors(pub Vec<EquipError>);

/// The item guid in a Lua-space bag slot; for [`EQUIPMENT_BAG`], `slot0` is the wire slot, as in
/// [`wire_pos`].
pub(crate) fn slot_guid(
    store: &ObjectFields,
    bag: i64,
    slot0: u8,
    objects: &Objects,
) -> Option<u64> {
    match bag {
        0 => store.player_pack_slot(slot0).filter(|g| *g != 0),
        1..=4 => {
            let bag_guid = store
                .player_inv_slot(BAG_SLOT_FIRST + bag as u8 - 1)
                .filter(|g| *g != 0)?;
            objects
                .object(bag_guid)?
                .container_slot(slot0)
                .filter(|g| *g != 0)
        }
        BANK_CONTAINER => store.player_bank_slot(slot0).filter(|g| *g != 0),
        KEYRING_CONTAINER => store.player_keyring_slot(slot0).filter(|g| *g != 0),
        BANK_BAG_ID_FIRST..=BANK_BAG_ID_LAST => {
            let bag_guid = store
                .player_bank_bag_slot((bag - BANK_BAG_ID_FIRST) as u8)
                .filter(|g| *g != 0)?;
            objects
                .object(bag_guid)?
                .container_slot(slot0)
                .filter(|g| *g != 0)
        }
        // The doll: the bank-bag buttons (wire 67..73) read their own array, since the INV
        // array's accessor caps at 23.
        EQUIPMENT_BAG
            if (BANK_BAG_SLOT_FIRST..BANK_BAG_SLOT_FIRST + BANK_BAGS).contains(&slot0) =>
        {
            store
                .player_bank_bag_slot(slot0 - BANK_BAG_SLOT_FIRST)
                .filter(|g| *g != 0)
        }
        EQUIPMENT_BAG => store.player_inv_slot(slot0).filter(|g| *g != 0),
        _ => None,
    }
}

/// `(item guid, stack count)` at a Lua-space `(bag, 1-based slot)`, `(0, 0)` when empty:
/// [`PendingItemOps`]'s baseline, since a partial split or destroy changes only the count.
pub(crate) fn slot_guid_count(
    store: Option<&ObjectStore>,
    bag: i64,
    slot1: u32,
    objects: &Objects,
) -> (u64, u32) {
    let Some(store) = store else {
        return (0, 0);
    };
    let slot0 = slot1.saturating_sub(1) as u8;
    match slot_guid(&store.0, bag, slot0, objects) {
        Some(guid) => {
            let count = objects
                .object(guid)
                .and_then(|f| f.item_stack_count())
                .unwrap_or(1);
            (guid, count)
        }
        None => (0, 0),
    }
}

/// An item template's icon path: its `DisplayInfoID` joined through `ItemDisplayInfo.dbc`. The
/// reference has no shared icon resolver (five registered getters and a C-side setter, each with
/// its own law), but every arm that ends at an item ends at this join, `ItemTemplate+0x18` → →
/// `rec+0x14`; each window keeps its own law above it.
pub(crate) fn item_icon(
    icons: Option<&crate::entities::ItemDisplays>,
    display_info_id: u32,
) -> Option<String> {
    icons
        .and_then(|i| i.catalog.get(display_info_id))
        .and_then(|d| d.icon.clone())
}

/// The walker's section mask (the reference's `ebx`) over the player's 113-guid slot array. A
/// container in an enabled section is always recursed into: sections gate only at the player's
/// root, and no caller sets the recursion bit `0x10`. Buyback (74-85) has no bit and is never
/// walked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct InventoryScope {
    /// `0x01`: worn equipment, slots 0-18.
    equipment: bool,
    /// `0x02`: the four equipped bag slots 19-22, and their contents.
    bags: bool,
    /// `0x04`: the backpack, slots 23-38.
    backpack: bool,
    /// `0x08`: bank items (39-66) and the seven bank-bag slots (67-73) with their contents.
    bank: bool,
    /// `0x40`: the keyring band.
    keyring: bool,
}

impl InventoryScope {
    /// `0x47`, what mask 0 becomes: ORs it in at the local player's root (the `CGPlayer_C` ctor
    /// sets that flag). No bank.
    pub(crate) const DEFAULT: Self = Self {
        equipment: true,
        bags: true,
        backpack: true,
        bank: false,
        keyring: true,
    };
    /// `0x01`: the equip-vs-use fork's first stage, "is a copy already worn?".
    pub(crate) const EQUIPMENT_ONLY: Self = Self {
        equipment: true,
        bags: false,
        backpack: false,
        bank: false,
        keyring: false,
    };
    /// `0x4F`: [`Self::DEFAULT`] plus the bank, what mask `8` becomes. The quest surfaces pass it:
    /// `GetQuestLogLeaderBoard`, the ADD_ITEM toast, the turn-in predicate and
    /// `GetAbandonQuestItems`.
    pub(crate) const QUEST_ITEMS: Self = Self {
        bank: true,
        ..Self::DEFAULT
    };
    /// `0x0F`, the tool walk's mask (`0x7548f0`): worn gear, bags, backpack and bank.
    pub(crate) const TOOLS: Self = Self {
        equipment: true,
        bags: true,
        backpack: true,
        bank: true,
        keyring: false,
    };
    /// Bags and backpack only, narrower than any reference mask: the action-bar and reagent counts
    /// use it, where the reference's `0x47` also counts worn gear and the keyring.
    pub(crate) const CARRIED: Self = Self {
        equipment: false,
        bags: true,
        backpack: true,
        bank: false,
        keyring: false,
    };
}

/// The reference's inventory walk: one ascending pass over the player's slot array, recursing into
/// each container as it is passed, `scope` gating the sections. `visit` gets the wire
/// `(bag_index, slot)` and the guid, and returns `Some` to stop. The order is the reference's and
/// load-bearing: equipment 0-18, each bag slot 19-22 then its contents, backpack 23-38, bank 39-66,
/// each bank bag 67-73 then its contents, keyring. The first carried item (the backpack and the
/// bags' contents) `pick` accepts, by guid: the arena charter turn-in's scan (`0x6dd680`).
pub(crate) fn find_carried(
    store: &ObjectFields,
    objects: &Objects,
    mut pick: impl FnMut(u64) -> bool,
) -> Option<u64> {
    walk_inventory(store, objects, InventoryScope::CARRIED, |_, _, guid| {
        pick(guid).then_some(guid)
    })
}

/// [`find_carried`] with the item's Lua `(bag, slot)`, the backpack as bag 0; equipped bags
/// themselves are skipped.
pub(crate) fn find_carried_at(
    store: &ObjectFields,
    objects: &Objects,
    mut pick: impl FnMut(u64) -> bool,
) -> Option<(i64, u32, u64)> {
    walk_inventory(
        store,
        objects,
        InventoryScope::CARRIED,
        |bag, slot, guid| {
            let at = if bag == BAG_PLAYER_INVENTORY {
                (slot >= SLOT_PACK_FIRST).then(|| (0, u32::from(slot - SLOT_PACK_FIRST) + 1))?
            } else {
                (i64::from(bag - SLOT_BAG_FIRST) + 1, u32::from(slot) + 1)
            };
            pick(guid).then_some((at.0, at.1, guid))
        },
    )
}

fn walk_inventory<T>(
    store: &ObjectFields,
    objects: &Objects,
    scope: InventoryScope,
    mut visit: impl FnMut(u8, u8, u64) -> Option<T>,
) -> Option<T> {
    // A container's contents, addressed by its player-array slot as the wire bag byte.
    let contents =
        |bag_slot: u8, bag_guid: u64, visit: &mut dyn FnMut(u8, u8, u64) -> Option<T>| {
            let bag_fields = objects.object(bag_guid)?;
            let num_slots = bag_fields.container_num_slots().unwrap_or(0).min(36) as u8;
            (0..num_slots).find_map(|j| {
                let guid = bag_fields.container_slot(j).unwrap_or(0);
                (guid != 0).then(|| visit(bag_slot, j, guid)).flatten()
            })
        };

    if scope.equipment {
        for i in 0..EQUIPMENT_SLOTS {
            let guid = store.player_inv_slot(i).unwrap_or(0);
            if guid != 0 {
                if let Some(hit) = visit(BAG_PLAYER_INVENTORY, i, guid) {
                    return Some(hit);
                }
            }
        }
    }
    if scope.bags {
        for bag in 0..BAGS {
            let bag_slot = BAG_SLOT_FIRST + bag;
            let bag_guid = store.player_inv_slot(bag_slot).unwrap_or(0);
            if bag_guid == 0 {
                continue;
            }
            // The bag object is a candidate itself, before its contents.
            if let Some(hit) = visit(BAG_PLAYER_INVENTORY, bag_slot, bag_guid) {
                return Some(hit);
            }
            if let Some(hit) = contents(bag_slot, bag_guid, &mut visit) {
                return Some(hit);
            }
        }
    }
    if scope.backpack {
        for i in 0..PACK_SLOTS {
            let guid = store.player_pack_slot(i).unwrap_or(0);
            if guid != 0 {
                if let Some(hit) = visit(BAG_PLAYER_INVENTORY, SLOT_PACK_FIRST + i, guid) {
                    return Some(hit);
                }
            }
        }
    }
    if scope.bank {
        for i in 0..BANK_SLOTS {
            let guid = store.player_bank_slot(i).unwrap_or(0);
            if guid != 0 {
                if let Some(hit) = visit(BAG_PLAYER_INVENTORY, BANK_SLOT_FIRST + i, guid) {
                    return Some(hit);
                }
            }
        }
        for bag in 0..BANK_BAGS {
            let bag_slot = BANK_BAG_SLOT_FIRST + bag;
            let bag_guid = store.player_bank_bag_slot(bag).unwrap_or(0);
            if bag_guid == 0 {
                continue;
            }
            if let Some(hit) = visit(BAG_PLAYER_INVENTORY, bag_slot, bag_guid) {
                return Some(hit);
            }
            if let Some(hit) = contents(bag_slot, bag_guid, &mut visit) {
                return Some(hit);
            }
        }
    }
    if scope.keyring {
        for i in 0..KEYRING_SLOTS {
            let guid = store.player_keyring_slot(i).unwrap_or(0);
            if guid != 0 {
                if let Some(hit) = visit(BAG_PLAYER_INVENTORY, KEYRING_SLOT_FIRST + i, guid) {
                    return Some(hit);
                }
            }
        }
    }
    None
}

/// How many of `entry` the player holds within `scope`, summing each copy's
/// `ITEM_FIELD_STACK_COUNT`: the reference's `(itemId, mask)`, whose per-item predicate tests
/// `OBJECT_FIELD_ENTRY` and adds `[+0x20]`. A quest objective counts banked copies
/// ([`InventoryScope::QUEST_ITEMS`]); an action-button or reagent count does not.
pub(crate) fn count_of(
    store: &ObjectFields,
    objects: &Objects,
    entry: u32,
    scope: InventoryScope,
) -> u32 {
    let mut total = 0u32;
    walk_inventory::<()>(store, objects, scope, |_, _, guid| {
        if let Some(fields) = objects.object(guid) {
            if fields.object_entry() == Some(entry) {
                total += fields.item_stack_count().unwrap_or(1);
            }
        }
        None
    });
    total
}

/// Every carried entry's count in one walk ([`InventoryScope::CARRIED`]): [`count_of`] for a
/// caller asking about many entries in a frame.
pub(crate) fn carried_counts(
    store: &ObjectFields,
    objects: &Objects,
) -> std::collections::HashMap<u32, u32> {
    let mut counts = std::collections::HashMap::new();
    walk_inventory::<()>(store, objects, InventoryScope::CARRIED, |_, _, guid| {
        if let Some(fields) = objects.object(guid) {
            if let Some(entry) = fields.object_entry() {
                *counts.entry(entry).or_insert(0) += fields.item_stack_count().unwrap_or(1);
            }
        }
        None
    });
    counts
}

/// The `TotemCategory` of every item in `scope`; an uncached template is asked for and skipped.
pub(crate) fn tool_categories(
    store: &ObjectFields,
    objects: &Objects,
    items: &crate::items::Items,
    commands: &NetCommands,
    scope: InventoryScope,
) -> Vec<u32> {
    let mut categories = Vec::new();
    walk_inventory::<()>(store, objects, scope, |_, _, guid| {
        let entry = objects.object(guid).and_then(|f| f.object_entry());
        if let Some(t) = entry.and_then(|e| items.template(e, guid, commands)) {
            if t.totem_category != 0 {
                categories.push(t.totem_category);
            }
        }
        None
    });
    categories
}

/// How far [`find_item`] looks and which copies count: the two mode bits the reference's callers
/// pass the walker.
#[derive(Clone, Copy, Default)]
pub(crate) struct ItemSearch {
    /// Mode `1` alone: equipment slots 0-18 only, the equip-vs-use fork's "already worn?" stage.
    pub(crate) equipment_only: bool,
    /// Mode bit `0x20`: skip a copy whose live `ITEM_FIELD_SPELL_CHARGES[0]` is 0, set by the use
    /// leg for a template with finite charges. Containers are never skipped.
    pub(crate) live_charges_only: bool,
}

/// Where a copy of `entry` is: the wire `(bag_index, slot)` and the instance guid, the first hit
/// of [`walk_inventory`] (predicate `OBJECT_FIELD_ENTRY` equality). The bank is not in scope.
pub(crate) fn find_item(
    store: &ObjectFields,
    objects: &Objects,
    entry: u32,
    search: ItemSearch,
) -> Option<(u8, u8, u64)> {
    let scope = if search.equipment_only {
        InventoryScope::EQUIPMENT_ONLY
    } else {
        InventoryScope::DEFAULT
    };
    walk_inventory(store, objects, scope, |bag, slot, guid| {
        let f = objects.object(guid)?;
        if f.object_entry() != Some(entry) {
            return None;
        }
        // Under the charges filter the instance needs uses left; a container is exempt.
        if search.live_charges_only
            && f.container_num_slots().is_none_or(|n| n == 0)
            && f.item_spell_charges(0).is_some_and(|c| c == 0)
        {
            return None;
        }
        Some((bag, slot, guid))
    })
}

/// Every occupied slot `scope` reaches, in the walker's order, for a predicate judged afterwards
/// against the template cache.
pub(crate) fn collect_inventory(
    store: &ObjectFields,
    objects: &Objects,
    scope: InventoryScope,
) -> Vec<(u8, u8, u64)> {
    let mut out = Vec::new();
    walk_inventory(store, objects, scope, |bag, slot, guid| {
        out.push((bag, slot, guid));
        None::<()>
    });
    out
}

/// The reference's `HasKey()`, the gate on the keyring's existence in the UI: the walker with
/// predicate, `BagFamily == 9` (`template+0x1d0`), at mode `0x4f`, so a key in the bank counts. A
/// slot whose template is still in flight reads as no key until the answer lands.
pub(crate) fn has_key(
    store: &ObjectFields,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> bool {
    // Every guid mode `0x4f` reaches, in the walker's order: each bag's contents follow its slot.
    fn contents(bag_guid: u64, objects: &Objects, out: &mut Vec<u64>) {
        let Some(f) = objects.object(bag_guid) else {
            return;
        };
        let n = f.container_num_slots().unwrap_or(0).min(36) as u8;
        out.extend((0..n).map(|j| f.container_slot(j).unwrap_or(0)));
    }
    let mut guids = Vec::new();
    for i in 0..EQUIPMENT_SLOTS {
        guids.push(store.player_inv_slot(i).unwrap_or(0));
    }
    for bag in 0..BAGS {
        let bag_guid = store.player_inv_slot(BAG_SLOT_FIRST + bag).unwrap_or(0);
        guids.push(bag_guid);
        contents(bag_guid, objects, &mut guids);
    }
    for i in 0..PACK_SLOTS {
        guids.push(store.player_pack_slot(i).unwrap_or(0));
    }
    for i in 0..BANK_SLOTS {
        guids.push(store.player_bank_slot(i).unwrap_or(0));
    }
    for bag in 0..BANK_BAGS {
        let bag_guid = store.player_bank_bag_slot(bag).unwrap_or(0);
        guids.push(bag_guid);
        contents(bag_guid, objects, &mut guids);
    }
    for i in 0..KEYRING_SLOTS {
        guids.push(store.player_keyring_slot(i).unwrap_or(0));
    }
    guids.into_iter().any(|guid| {
        if guid == 0 {
            return false;
        }
        let Some(entry) = objects.object(guid).and_then(|f| f.object_entry()) else {
            return false;
        };
        items
            .template(entry, guid, commands)
            .is_some_and(|t| t.bag_family == BAG_FAMILY_KEYS)
    })
}
/// A test's object index: a world to seed with items, and the [`Objects`] lookup over it. Seed
/// first, then read: `get()` borrows the world while its answer lives, so call sites write
/// `&objs.get()` inline.
#[cfg(test)]
pub(crate) struct TestObjects {
    world: World,
    state: bevy::ecs::system::SystemState<Objects<'static, 'static>>,
}

#[cfg(test)]
impl TestObjects {
    pub(crate) fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<crate::net::GuidIndex>();
        let state = bevy::ecs::system::SystemState::new(&mut world);
        Self { world, state }
    }

    /// One item instance in the index, as the wire's `ItemCreate` makes it.
    pub(crate) fn spawn(&mut self, guid: u64, fields: ObjectFields) {
        crate::items::test_spawn_item(&mut self.world, guid, fields, false);
    }

    /// The same for a container instance (`TYPEMASK_CONTAINER`).
    pub(crate) fn spawn_container(&mut self, guid: u64, fields: ObjectFields) {
        crate::items::test_spawn_item(&mut self.world, guid, fields, true);
    }

    pub(crate) fn get(&mut self) -> Objects<'_, '_> {
        self.state.get(&self.world)
    }
}

/// The wire's player-array bag index, `INVENTORY_SLOT_BAG_0`; with it, [`ItemUse::slot`] is the
/// equipment index (0-18 worn, 19-22 the equipped bags).
pub(crate) const PLAYER_ARRAY: u8 = 255;

/// One resolved item-use click, as [`send_item_use`] needs it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ItemUse {
    /// The live instance's guid; `None` falls through to a plain use.
    pub(crate) guid: Option<u64>,
    /// Template `StartQuest`: non-zero diverts to the quest offer, ahead of the cast tail.
    pub(crate) start_quest: u32,
    /// The wire position (`255` = the player array).
    pub(crate) bag_index: u8,
    /// The wire slot, 0-based.
    pub(crate) slot: u8,
    /// The template entry: the cooldown store keys item records on `(use_spell, entry)` (the
    /// client's `[eax+8]==spellId && [eax+0xc]==itemID`).
    pub(crate) entry: u32,
    /// The ON_USE spell, the reference's answer: the first block with `SpellId != 0` and
    /// `SpellTrigger == 0`.
    pub(crate) use_spell: Option<u32>,
    /// The GameObject a key is used on, `CGItem::Use`'s target argument.
    pub(crate) on_object: Option<u64>,
    /// The template's `ITEM_FLAG_CHARTER` (`0x2000`), a signable petition.
    pub(crate) is_charter: bool,
}

/// What using an item sends: the reference's `CGItem::Use`, the one fork behind the bag click, the
/// doll click and the action bar.
///
/// A non-zero `StartQuest` sends `CMSG_QUESTGIVER_QUERY_QUEST` with the item's own guid, and the
/// server answers with the quest details, the item as giver. Never `CMSG_USE_ITEM`: vmangos
/// `HandleUseItemOpcode` refuses an item with no on-use spell as `EQUIP_ERR_ITEM_NOT_FOUND`, and no
/// quest starter has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemUseRoute {
    /// Rung 3: offer the quest and return, before the cast tail.
    QuestOffer { npc: u64, quest: u32 },
    /// The toggle-cancel arm: the ON_USE spell is live as a cancelable active-icon aura, so the
    /// click sends `CMSG_CANCEL_AURA` and casts nothing; a mount item clicked while mounted
    /// dismounts.
    ToggleCancel(u32),
    /// The cast tail (`TryCast`): the whole ladder.
    Cast(u32),
    /// No ON_USE block: `TryCast` gets spell id 0, whose record is null, and bails with nothing
    /// sent.
    Nothing,
    /// The charter arm, rung 9 (template `Flags & 0x2000`), whose sole caller, sends
    /// `CMSG_PETITION_SHOW_SIGNATURES` (`0x1BE`), one `u64` guid, instead of `CMSG_USE_ITEM`. It
    /// comes after the readable and `CMSG_OPEN_ITEM` rungs.
    ShowPetition { item: u64 },
    /// Rung 15 of 20: using the weapon a disarm took raises `ERR_CANT_USE_DISARMED` (`0x16b`)
    /// locally and sends nothing. Only the worn hand the disarm hides (slot 15 or 16) is refused,
    /// and the rung sits above the bind confirm and the cast tail.
    CantUseDisarmed,
}

/// [`ItemUseRoute`]'s decision, in the reference's order: quest offer, charter, disarmed refusal,
/// toggle scan, cast tail. The toggle predicate and the disarmed hand are caster state, passed in;
/// a `None` guid cannot address a questgiver, so it falls through to the ordinary path.
pub(crate) fn item_use_route(
    it: ItemUse,
    aura_cancels: impl Fn(u32) -> bool,
    disarmed_hand: Option<u8>,
) -> ItemUseRoute {
    if let Some(npc) = it.guid.filter(|_| it.start_quest != 0) {
        return ItemUseRoute::QuestOffer {
            npc,
            quest: it.start_quest,
        };
    }
    // A charter with no resolved instance falls through: the reference sends nothing on a zero
    // guid.
    if let Some(item) = it.guid.filter(|_| it.is_charter) {
        return ItemUseRoute::ShowPetition { item };
    }
    // Rung 15: in the player array `slot` is the equipment index, which the reference finds by
    // scanning the worn guids for the item.
    if disarmed_hand.is_some() && it.bag_index == PLAYER_ARRAY && Some(it.slot) == disarmed_hand {
        return ItemUseRoute::CantUseDisarmed;
    }
    match it.use_spell {
        Some(spell) if aura_cancels(spell) => ItemUseRoute::ToggleCancel(spell),
        Some(spell) => ItemUseRoute::Cast(spell),
        None => ItemUseRoute::Nothing,
    }
}

/// Send what [`item_use_route`] decides; the only place a `CMSG_USE_ITEM` leaves benilla. The
/// reference's cast tail calls, whose body, `TryCast`: the function the spellbook, `/cast` and the
/// action bar run, with the item as an argument no gate skips for, and `SendCast` picks the opcode
/// from it. So an item use takes the whole cast ladder, [`crate::spell::CastLadder::send`]. The
/// toggle-cancel scan above the tail (`CancelAura`) is the action button's predicate,
/// [`crate::ui_action::toggle::active_action_toggle`], applied to the item's spell.
///
/// Returns whether anything left for the server.
pub(crate) fn send_item_use(
    it: ItemUse,
    ctx: &crate::spell::cast_target::CastContext,
    ladder: &mut crate::spell::CastLadder,
    script: &mut benilla_ui::script::UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    suppress: bool,
    // Passed in, not carried on `CastLadder`, so no system reaches it twice.
    ui_errors: &mut crate::ui_action::UiErrorKeys,
) -> bool {
    // The toggle predicate over the caster's live aura slots.
    let aura_cancels = |spell: u32| {
        let Some(d) = ladder.spells.as_ref().and_then(|s| s.catalog.get(spell)) else {
            return false;
        };
        crate::ui_action::toggle::active_action_toggle(spell, d, ctx.self_auras)
    };
    // The disarm ladder, asked of the caster's own inventory.
    let disarmed_hand = ctx.rel.self_store.and_then(|store| {
        crate::items::disarmed_equipment_slot(
            store,
            &ladder.objects,
            &ladder.items,
            &ladder.commands,
        )
    });
    match item_use_route(it, aura_cancels, disarmed_hand) {
        ItemUseRoute::CantUseDisarmed => {
            debug!(
                "ui_items: item use refused — that hand is disarmed (slot {})",
                it.slot
            );
            ui_errors
                .0
                .push(crate::ui_action::UiError::key("ERR_CANT_USE_DISARMED"));
            false
        }
        ItemUseRoute::ToggleCancel(spell) => {
            debug!("ui_items: item use {spell} re-pressed — its aura cancels, no cast");
            let _ = ladder
                .commands
                .0
                .send(crate::net::ClientCommand::CancelAura { spell_id: spell });
            true
        }
        ItemUseRoute::QuestOffer { npc, quest } => {
            let _ = ladder
                .commands
                .0
                .send(crate::net::ClientCommand::QuestgiverQuery { npc, quest });
            true
        }
        ItemUseRoute::ShowPetition { item } => {
            let _ = ladder
                .commands
                .0
                .send(crate::net::ClientCommand::PetitionShowSignatures { item });
            true
        }
        // The bind-on-use deferral, the last rung: every arm above has claimed the click first, so
        // re-pressing a bind-on-use trinket to cancel its aura never asks. It covers `Nothing` too:
        // four of the reference's five predecessors are on-use spell lookup failures, so the
        // reference asks with no usable on-use spell.
        ItemUseRoute::Nothing | ItemUseRoute::Cast(_)
            if !suppress
                && it.guid.is_some_and(|g| {
                    gate.use_binds(&ladder.objects, &ladder.items, &ladder.commands, g)
                }) =>
        {
            gate.defer_use(script, it);
            false
        }
        ItemUseRoute::Nothing => {
            debug!(
                "ui_items: the item at wire {}/{} has no ON_USE block — nothing sent (TryCast's null-rec bail)",
                it.bag_index, it.slot
            );
            false
        }
        ItemUseRoute::Cast(spell) => {
            ladder.send(
                spell,
                ctx,
                crate::spell::CastCommit::Item {
                    bag_index: it.bag_index,
                    slot: it.slot,
                    entry: it.entry,
                    item_guid: it.guid.unwrap_or(0),
                    on_object: it.on_object,
                },
            );
            true
        }
    }
}

/// The client's quality colour escape for an item link: the reference indexes the seven-entry table
/// and clamps anything `>= 7` to index 1, white, which is the catch-all arm.
pub(super) fn quality_color(quality: u32) -> &'static str {
    match quality {
        0 => "ff9d9d9d",
        2 => "ff1eff00",
        3 => "ff0070dd",
        4 => "ffa335ee",
        5 => "ffff8000",
        6 => "ffe6cc80",
        _ => "ffffffff",
    }
}

/// One item hyperlink, as the client's builds every one it shows: `SStrPrintf` over
/// `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"`, the colour escape, item id, enchant id, random-property id,
/// suffix factor, name, then the `|r` reset.
pub(crate) fn item_link_full(
    item_id: u32,
    enchant_id: u32,
    random_property_id: u32,
    suffix_factor: u32,
    name: &str,
    quality: u32,
) -> String {
    format!(
        "|c{}|Hitem:{item_id}:{enchant_id}:{random_property_id}:{suffix_factor}|h[{name}]|h|r",
        quality_color(quality)
    )
}

/// [`item_link_full`] with zero enchant, random-property and suffix ids, for a caller with no roll
/// in hand; a rolled item's link carries the roll and its suffixed name.
pub(crate) fn item_link(item_id: u32, name: &str, quality: u32) -> String {
    item_link_full(item_id, 0, 0, 0, name, quality)
}

/// `INVTYPE_AMMO`: loaded with `CMSG_SET_AMMO`, not the equip swap.
pub(super) const INVTYPE_AMMO: u32 = 24;

/// The `InventoryType` → live-API equip slots map (wire `EQUIPMENT_SLOT_*` + 1, bag icons 20..23;
/// empty is not equippable), from vmangos `ItemPrototype::GetAllowedEquipSlots`
/// (`Objects/Item.cpp:577-696`), which `Player::FindEquipSlot` (`Objects/Player.cpp:8440`) walks.
/// The client's own check, `IsValidForSlot`, is a fixed slot mask per `InventoryType` that answers
/// 1 for every 0-based slot from 23, plus the RANGED leg below; it has no dual-wield input.
/// `INVTYPE_WEAPON` offers both hands here, where vmangos offers the off hand only to a dual
/// wielder and refuses the equip otherwise (`EQUIP_ERR_CANT_DUAL_WIELD`, `Player.cpp:9915`).
pub(super) fn find_equip_slot(inventory_type: u32, has_relic_slot: bool) -> Vec<u8> {
    // Live-API ids: wire `EQUIPMENT_SLOT_*` + 1. `AmmoSlot` is 0 (`GetInventorySlotInfo`), loaded
    // by entry with `CMSG_SET_AMMO`.
    const AMMO: u8 = 0;
    const HEAD: u8 = 1;
    const NECK: u8 = 2;
    const SHOULDERS: u8 = 3;
    const BODY: u8 = 4; // the shirt slot (EQUIPMENT_SLOT_BODY)
    const CHEST: u8 = 5;
    const WAIST: u8 = 6;
    const LEGS: u8 = 7;
    const FEET: u8 = 8;
    const WRISTS: u8 = 9;
    const HANDS: u8 = 10;
    const FINGER1: u8 = 11;
    const FINGER2: u8 = 12;
    const TRINKET1: u8 = 13;
    const TRINKET2: u8 = 14;
    const BACK: u8 = 15;
    const MAINHAND: u8 = 16;
    const OFFHAND: u8 = 17;
    const RANGED: u8 = 18;
    const TABARD: u8 = 19;
    const BAG0: u8 = 20;
    const BAG1: u8 = 21;
    const BAG2: u8 = 22;
    const BAG3: u8 = 23;

    match inventory_type {
        1 => vec![HEAD],                // INVTYPE_HEAD
        2 => vec![NECK],                // INVTYPE_NECK
        3 => vec![SHOULDERS],           // INVTYPE_SHOULDERS
        4 => vec![BODY],                // INVTYPE_BODY (the shirt)
        5 | 20 => vec![CHEST],          // INVTYPE_CHEST / INVTYPE_ROBE (same slot)
        6 => vec![WAIST],               // INVTYPE_WAIST
        7 => vec![LEGS],                // INVTYPE_LEGS
        8 => vec![FEET],                // INVTYPE_FEET
        9 => vec![WRISTS],              // INVTYPE_WRISTS
        10 => vec![HANDS],              // INVTYPE_HANDS
        11 => vec![FINGER1, FINGER2],   // INVTYPE_FINGER
        12 => vec![TRINKET1, TRINKET2], // INVTYPE_TRINKET
        13 => vec![MAINHAND, OFFHAND],  // INVTYPE_WEAPON (no dual-wield test)
        14 => vec![OFFHAND],            // INVTYPE_SHIELD
        // The RANGED slot is the relic slot for Paladin, Shaman and Druid (`ChrClasses.dbc` field
        // 16): `IsValidForSlot`'s slot `0x11` leg takes it iff `(InventoryType == 28) ==
        // hasRelicSlot`, so the ranged types and RELIC exclude each other.
        15 if !has_relic_slot => vec![RANGED], // INVTYPE_RANGED
        16 => vec![BACK],                      // INVTYPE_CLOAK
        17 => vec![MAINHAND],                  // INVTYPE_2HWEAPON
        18 => vec![BAG0, BAG1, BAG2, BAG3],    // INVTYPE_BAG
        19 => vec![TABARD],                    // INVTYPE_TABARD
        21 => vec![MAINHAND],                  // INVTYPE_WEAPONMAINHAND
        22 => vec![OFFHAND],                   // INVTYPE_WEAPONOFFHAND
        23 => vec![OFFHAND],                   // INVTYPE_HOLDABLE
        24 => vec![AMMO],                      // INVTYPE_AMMO → the ammo slot (loaded via SET_AMMO)
        25 | 26 if !has_relic_slot => vec![RANGED], // INVTYPE_THROWN / INVTYPE_RANGEDRIGHT
        28 if has_relic_slot => vec![RANGED],  // INVTYPE_RELIC, the same slot
        // NON_EQUIP (0), QUIVER (27), a RANGED-slot type on the wrong side of the relic test, and
        // anything past MAX_INVTYPE (29): not equippable.
        _ => Vec::new(),
    }
}

/// `ItemSet.dbc`: the tooltip SET block's rows (name, members, bonuses, skill).
#[derive(Resource)]
pub(crate) struct ItemSets(pub(crate) benilla_formats::ItemSetCatalog);

/// `ItemSubClass.dbc`: the slot and type line's alternate-proficiency and hidden-name gates.
#[derive(Resource)]
pub(crate) struct ItemSubClasses(pub(crate) benilla_formats::ItemSubClassCatalog);

/// `ItemBagFamily.dbc`: reason 16's `%s`, what a specialised bag accepts ("Only Arrows can be
/// placed in that.").
#[derive(Resource)]
pub(crate) struct ItemBagFamilies(pub(crate) benilla_formats::ItemBagFamilyCatalog);

/// `ItemClass.dbc`: an item class's name ("Weapon", "Container"), `GetItemInfo`'s `itemType`.
#[derive(Resource)]
pub(crate) struct ItemClasses(pub(crate) benilla_formats::ItemClassCatalog);

/// Startup: the item-tooltip DBCs; one that fails to load leaves its resource absent.
fn load_item_dbcs(mut commands: Commands, world_assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match benilla_formats::load_item_sets(&mut chain) {
        Ok(cat) => {
            info!("ui_items: ItemSet.dbc loaded ({} sets)", cat.len());
            commands.insert_resource(ItemSets(cat));
        }
        Err(e) => warn!("ui_items: ItemSet.dbc failed to load: {e:#}"),
    }
    match benilla_formats::load_item_sub_classes(&mut chain) {
        Ok(cat) => {
            info!("ui_items: ItemSubClass.dbc loaded ({} rows)", cat.len());
            commands.insert_resource(ItemSubClasses(cat));
        }
        Err(e) => warn!("ui_items: ItemSubClass.dbc failed to load: {e:#}"),
    }
    match benilla_formats::load_item_classes(&mut chain) {
        Ok(cat) => {
            info!("ui_items: ItemClass.dbc loaded ({} classes)", cat.len());
            commands.insert_resource(ItemClasses(cat));
        }
        Err(e) => warn!("ui_items: ItemClass.dbc failed to load: {e:#}"),
    }
    match benilla_formats::load_auction_houses(&mut chain) {
        Ok(cat) => {
            info!("ui_items: AuctionHouse.dbc loaded ({} houses)", cat.len());
            commands.insert_resource(crate::ui_auction::AuctionHouses(cat));
        }
        Err(e) => warn!("ui_items: AuctionHouse.dbc failed to load: {e:#}"),
    }
    match benilla_formats::load_item_bag_families(&mut chain) {
        Ok(cat) => {
            info!(
                "ui_items: ItemBagFamily.dbc loaded ({} families)",
                cat.len()
            );
            commands.insert_resource(ItemBagFamilies(cat));
        }
        Err(e) => warn!("ui_items: ItemBagFamily.dbc failed to load: {e:#}"),
    }
}

pub(crate) struct UiItemsPlugin;

impl Plugin for UiItemsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // The icons come from the equipment renderer's `ItemDisplays`.
        app.init_resource::<EquipErrors>()
            .init_resource::<PendingItemOps>()
            // The soulbind confirmations' pending records: the client's pending-equip array and
            // its one bind-on-use cell.
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<LockTransitions>()
            .init_resource::<net::BagOpens>()
            // After the chain opens: unordered, it can run first and skip every item DBC.
            .add_systems(
                Startup,
                load_item_dbcs.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    // The pending-lock clear, ahead of both feeds that push `locked`.
                    feed::resolve_item_locks
                        .before(feed_containers)
                        .before(crate::ui_char::feed_char),
                    // The slot cooldowns must be in the VM before `feed_action_state`'s
                    // synchronous `BAG_UPDATE_COOLDOWN` makes the bag handlers re-read them.
                    feed_containers
                        .in_set(UnitFeed)
                        .before(crate::ui_action::CooldownEvents),
                    // The item-tooltip store answers before the input pass, so a re-hover the
                    // next frame sees the answer.
                    // After the book feed, so a `$?s` reads this frame's learns.
                    feed_item_stats
                        .in_set(UnitFeed)
                        .after(crate::ui_action::SlotIdentityFeed),
                    feed_item_sets
                        .in_set(UnitFeed)
                        .after(crate::ui_action::SlotIdentityFeed),
                    // The roll table, pushed whole once per VM, before the first hover.
                    feed_random_properties.in_set(UnitFeed),
                    feed_player_req.in_set(UnitFeed),
                    // After the input pass, so a repair-mode click goes out the same frame; a
                    // plain `UseContainerItem` is a script call.
                    drain_container_repairs.after(UiInput),
                    // Pick, place and split: `CMSG_SWAP_INV_ITEM`, `CMSG_SWAP_ITEM` or
                    // `CMSG_SPLIT_ITEM`.
                    drain_container_moves.after(UiInput),
                    // The delete-confirm popup's accept: `CMSG_DESTROYITEM`.
                    drain_container_destroys.after(UiInput),
                    // `AutoEquipCursorItem`: `CMSG_AUTOEQUIP_ITEM`.
                    drain_container_autoequips.after(UiInput),
                    drain_bag_autostores.after(UiInput),
                    // The soulbind confirmations' answers (`EquipPendingItem`,
                    // `CancelPendingEquip`, `ConfirmBindOnUse`). A dialog is answered in a later
                    // frame than it was raised, so no order against the other drains is needed.
                    drain::drain_bind_confirm_answers.after(UiInput),
                    drain::drain_bind_on_use_confirms.after(UiInput),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        find_equip_slot, item_use_route, wire_pos, ItemUse, ItemUseRoute, KEYRING_CONTAINER,
    };
    use benilla_ui::script::EQUIPMENT_BAG;

    /// The keyring's 32 slots are player slots 86-117; 81-85 are buyback.
    #[test]
    fn wire_pos_maps_the_keyring_onto_the_player_grid() {
        assert_eq!(wire_pos(KEYRING_CONTAINER, 1), Some((255, 86)));
        assert_eq!(wire_pos(KEYRING_CONTAINER, 32), Some((255, 117)));
        assert_eq!(
            wire_pos(KEYRING_CONTAINER, 33),
            None,
            "118 is the first currency token slot"
        );
        assert_eq!(wire_pos(KEYRING_CONTAINER, 0), None);
    }

    /// Rung 15 turns on the worn position alone, and beats the cast tail below it.
    #[test]
    fn using_the_hand_a_disarm_took_is_refused_locally() {
        let sword = 0x4000_0000_0000_0001_u64;
        // A weapon with an ON_USE spell, worn at `slot`, clicked out of the player array.
        let worn = |slot: u8| ItemUse {
            entry: 7,
            guid: Some(sword),
            start_quest: 0,
            bag_index: super::PLAYER_ARRAY,
            slot,
            use_spell: Some(8690),
            on_object: None,
            is_charter: false,
        };
        let never = |_: u32| false;

        assert_eq!(
            item_use_route(worn(15), never, Some(15)),
            ItemUseRoute::CantUseDisarmed
        );
        // A disarmed dual-wielder's other hand is not hidden.
        assert_eq!(
            item_use_route(worn(16), never, Some(15)),
            ItemUseRoute::Cast(8690)
        );
        // The same item in a bag is not worn, so it is never this refusal.
        assert_eq!(
            item_use_route(
                ItemUse {
                    bag_index: 1,
                    ..worn(15)
                },
                never,
                Some(15)
            ),
            ItemUseRoute::Cast(8690)
        );
        // Flag down: nothing is hidden.
        assert_eq!(
            item_use_route(worn(15), never, None),
            ItemUseRoute::Cast(8690)
        );
        // The off hand hidden: the ladder's empty or non-weapon main-hand rung.
        assert_eq!(
            item_use_route(worn(16), never, Some(16)),
            ItemUseRoute::CantUseDisarmed
        );
    }

    #[test]
    fn the_item_use_fork_routes_quest_offer_cast_and_nothing() {
        // An Unsent Letter (entry 2874, StartQuest 373): the item's guid is the questgiver, and
        // the quest arm wins even over an on-use spell.
        let letter = 0x4000_0000_0000_0BAD_u64;
        let it = |guid, start_quest, use_spell| ItemUse {
            entry: 0,
            guid,
            start_quest,
            bag_index: 255,
            slot: 23,
            use_spell,
            on_object: None,
            is_charter: false,
        };
        let never = |_| false;
        assert_eq!(
            item_use_route(it(Some(letter), 373, None), never, None),
            ItemUseRoute::QuestOffer {
                npc: letter,
                quest: 373
            }
        );
        assert_eq!(
            item_use_route(it(Some(letter), 0, Some(8690)), never, None),
            ItemUseRoute::Cast(8690),
            "a hearthstone takes the cast tail"
        );
        assert_eq!(
            item_use_route(it(Some(letter), 0, None), never, None),
            ItemUseRoute::Nothing,
            "no ON_USE block — the ref sends nothing"
        );
        // No resolved instance: nothing to address, so the ordinary path runs.
        assert_eq!(
            item_use_route(it(None, 373, Some(8690)), never, None),
            ItemUseRoute::Cast(8690)
        );
    }

    /// The charter template (entry 5863) has no ON_USE spell, no `StartQuest` and
    /// `InventoryType` 0, so without the flag the click reaches `Nothing`.
    #[test]
    fn a_charter_click_opens_the_petition_instead_of_casting() {
        let charter = 0x4000_0000_0000_5863_u64;
        let it = |guid, is_charter, use_spell| ItemUse {
            entry: 5863,
            guid,
            start_quest: 0,
            bag_index: 255,
            slot: 23,
            use_spell,
            on_object: None,
            is_charter,
        };
        let never = |_| false;
        assert_eq!(
            item_use_route(it(Some(charter), true, None), never, None),
            ItemUseRoute::ShowPetition { item: charter },
            "the charter opens its petition window"
        );
        // Without the flag, the same item is the reference's silent no-op.
        assert_eq!(
            item_use_route(it(Some(charter), false, None), never, None),
            ItemUseRoute::Nothing
        );
        // No resolved instance: it falls through, as the quest fork does.
        assert_eq!(
            item_use_route(it(None, true, None), never, None),
            ItemUseRoute::Nothing
        );
        assert_eq!(
            item_use_route(it(None, true, Some(8690)), never, None),
            ItemUseRoute::Cast(8690)
        );
    }

    /// `CGItem::Use`'s toggle scan sits above the cast tail. The predicate is stubbed here (it is
    /// tested in `ui_action::toggle`), so this pins the fork and its order.
    #[test]
    fn a_live_aura_makes_the_item_click_cancel_instead_of_cast() {
        const SUMMON_HORSE: u32 = 17462;
        let it = |start_quest, use_spell| ItemUse {
            entry: 0,
            guid: Some(0x4000_0000_0000_0BAD),
            start_quest,
            bag_index: 255,
            slot: 23,
            use_spell,
            on_object: None,
            is_charter: false,
        };
        let mounted = |spell: u32| spell == SUMMON_HORSE;
        assert_eq!(
            item_use_route(it(0, Some(SUMMON_HORSE)), mounted, None),
            ItemUseRoute::ToggleCancel(SUMMON_HORSE),
            "mounted: the click dismounts — CMSG_CANCEL_AURA, no CMSG_USE_ITEM",
        );
        assert_eq!(
            item_use_route(it(0, Some(SUMMON_HORSE)), |_| false, None),
            ItemUseRoute::Cast(SUMMON_HORSE),
            "not mounted: the very same click casts",
        );
        // Order: the quest offer forks, above the toggle scan.
        assert_eq!(
            item_use_route(it(373, Some(SUMMON_HORSE)), mounted, None),
            ItemUseRoute::QuestOffer {
                npc: 0x4000_0000_0000_0BAD,
                quest: 373
            },
        );
        // An item with no ON_USE block has nothing to cancel, whatever the predicate says.
        assert_eq!(
            item_use_route(it(0, None), |_| true, None),
            ItemUseRoute::Nothing
        );
    }

    /// Against vmangos `ItemPrototype::GetAllowedEquipSlots` (`Objects/Item.cpp:577-696`), except
    /// `INVTYPE_WEAPON`'s off hand.
    #[test]
    fn find_equip_slot_matches_the_vmangos_table() {
        assert_eq!(find_equip_slot(1, false), vec![1], "HEAD");
        assert_eq!(find_equip_slot(2, false), vec![2], "NECK");
        assert_eq!(find_equip_slot(4, false), vec![4], "BODY (shirt)");
        assert_eq!(find_equip_slot(5, false), vec![5], "CHEST");
        assert_eq!(find_equip_slot(20, false), vec![5], "ROBE aliases CHEST");
        assert_eq!(
            find_equip_slot(11, false),
            vec![11, 12],
            "FINGER, two slots"
        );
        assert_eq!(
            find_equip_slot(12, false),
            vec![13, 14],
            "TRINKET, two slots"
        );
        assert_eq!(
            find_equip_slot(13, false),
            vec![16, 17],
            "WEAPON offers both hands (dual-wield simplified)"
        );
        assert_eq!(find_equip_slot(14, false), vec![17], "SHIELD -> off hand");
        assert_eq!(find_equip_slot(15, false), vec![18], "RANGED");
        assert_eq!(find_equip_slot(16, false), vec![15], "CLOAK -> back");
        assert_eq!(
            find_equip_slot(17, false),
            vec![16],
            "2HWEAPON -> main hand only"
        );
        assert_eq!(find_equip_slot(19, false), vec![19], "TABARD");
        assert_eq!(find_equip_slot(21, false), vec![16], "WEAPONMAINHAND");
        assert_eq!(find_equip_slot(22, false), vec![17], "WEAPONOFFHAND");
        assert_eq!(find_equip_slot(18, false), vec![20, 21, 22, 23], "BAG");
        assert_eq!(
            find_equip_slot(24, false),
            vec![0],
            "AMMO -> the ammo slot (id 0)"
        );
        // Not equippable for anyone, relic slot or not.
        for t in [0u32, 27, 100] {
            for relic in [false, true] {
                assert!(find_equip_slot(t, relic).is_empty(), "inventory type {t}");
            }
        }
    }

    /// `IsValidForSlot`'s slot `0x11` leg is an equality: a relic class is offered the slot for a
    /// relic and never for a ranged weapon, everyone else the reverse.
    #[test]
    fn the_ranged_slot_takes_a_relic_or_a_weapon_never_both() {
        const RANGED: u8 = 18;
        // Paladin, Shaman, Druid.
        assert_eq!(find_equip_slot(28, true), vec![RANGED], "RELIC");
        for t in [15u32, 25, 26] {
            assert!(
                find_equip_slot(t, true).is_empty(),
                "a relic class is offered no ranged slot for inventory type {t}"
            );
        }
        // Everyone else.
        assert!(
            find_equip_slot(28, false).is_empty(),
            "RELIC, no relic slot"
        );
        for (t, who) in [(15u32, "RANGED"), (25, "THROWN"), (26, "RANGEDRIGHT")] {
            assert_eq!(find_equip_slot(t, false), vec![RANGED], "{who}");
        }
    }

    /// Doll live id `n` (1..=23) is player-array slot `n - 1`; ammo (0) is refused, as
    /// `pickup_inventory_item`'s range guard does.
    #[test]
    fn wire_pos_maps_equipment_bag_to_the_player_grid() {
        assert_eq!(wire_pos(EQUIPMENT_BAG, 1), Some((255, 0)), "HeadSlot");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 19), Some((255, 18)), "TabardSlot");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 16), Some((255, 15)), "MainHandSlot");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 0), None, "ammo — out of scope");
        // The four equipped-bag icons map onto the wire's bag inventory slots (19..22).
        assert_eq!(wire_pos(EQUIPMENT_BAG, 20), Some((255, 19)), "Bag0Slot");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 23), Some((255, 22)), "Bag3Slot");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 24), None, "past the bag icons");
        // A backpack and a doll position share wire bag 255, so a move between them is
        // `CMSG_SWAP_INV_ITEM`.
        assert_eq!(
            wire_pos(0, 1).map(|(b, _)| b),
            wire_pos(EQUIPMENT_BAG, 1).map(|(b, _)| b)
        );
    }

    #[test]
    fn wire_pos_maps_the_bank_spaces() {
        use super::BANK_CONTAINER;
        // The 28 item slots: live 1..28 → (255, 39..66).
        assert_eq!(wire_pos(BANK_CONTAINER, 1), Some((255, 39)));
        assert_eq!(wire_pos(BANK_CONTAINER, 28), Some((255, 66)));
        assert_eq!(wire_pos(BANK_CONTAINER, 29), None, "past the vault");
        assert_eq!(wire_pos(BANK_CONTAINER, 0), None);
        // Bank bags: container 5 is the bag in player-array slot 67, container 11 in 73.
        assert_eq!(wire_pos(5, 1), Some((67, 0)));
        assert_eq!(wire_pos(11, 36), Some((73, 35)));
        assert_eq!(wire_pos(12, 1), None, "past the bank bags");
        // The bank-bag buttons in doll space: live 68..74 → wire 67..73; the gap 24..67 refuses.
        assert_eq!(
            wire_pos(EQUIPMENT_BAG, 68),
            Some((255, 67)),
            "BankFrameBag1"
        );
        assert_eq!(
            wire_pos(EQUIPMENT_BAG, 74),
            Some((255, 73)),
            "BankFrameBag7"
        );
        assert_eq!(wire_pos(EQUIPMENT_BAG, 67), None, "the doll-space gap");
        assert_eq!(wire_pos(EQUIPMENT_BAG, 75), None, "past the bank bags");
    }
}

/// [`find_item`]'s order, which is the reference walk's.
#[cfg(test)]
mod find_item_tests {
    use super::{find_item, ItemSearch, TestObjects};
    use benilla_protocol::ObjectFields;

    // Descriptor field indices, raw.
    const ENTRY: u16 = 3; // OBJECT_FIELD_ENTRY
    const CHARGES: u16 = 16; // ITEM_FIELD_SPELL_CHARGES[0]
    const NUM_SLOTS: u16 = 48; // CONTAINER_FIELD_NUM_SLOTS
    const SLOT_1: u16 = 50; // CONTAINER_FIELD_SLOT_1 (2 fields per guid)
    const INV_SLOT_HEAD: u16 = 486; // PLAYER_FIELD_INV_SLOT_HEAD (2 per guid, 23 slots)
    const PACK_SLOT_1: u16 = 532; // PLAYER_FIELD_PACK_SLOT_1 (2 per guid, 16 slots)
    const KEYRING_SLOT_1: u16 = 648; // PLAYER_FIELD_KEYRING_SLOT_1 (2 per guid, player slots 86..)

    const TRINKET: u32 = 12_930;
    const BAG: u32 = 4_500;

    /// A player with the given `(player slot, guid)` pairs in the equipment (0..22), backpack
    /// (23..38) or keyring (86..) array.
    fn player(slots: &[(u16, u64)]) -> ObjectFields {
        let mut pairs = Vec::new();
        for &(idx, guid) in slots {
            let base = if idx < 23 {
                INV_SLOT_HEAD + 2 * idx
            } else if idx < 86 {
                PACK_SLOT_1 + 2 * (idx - 23)
            } else {
                KEYRING_SLOT_1 + 2 * (idx - 86)
            };
            pairs.push((base, guid as u32));
            pairs.push((base + 1, (guid >> 32) as u32));
        }
        ObjectFields::from_pairs(&pairs)
    }

    /// A plain item instance of `entry`, optionally with live charges.
    fn item(objects: &mut TestObjects, guid: u64, entry: u32, charges: Option<i32>) {
        let mut pairs = vec![(ENTRY, entry)];
        if let Some(c) = charges {
            pairs.push((CHARGES, c as u32));
        }
        objects.spawn(guid, ObjectFields::from_pairs(&pairs));
    }

    /// A container instance holding `contents` at its own inner slots.
    fn bag(objects: &mut TestObjects, guid: u64, entry: u32, contents: &[(u8, u64)]) {
        let mut pairs = vec![(ENTRY, entry), (NUM_SLOTS, 16)];
        for &(i, item_guid) in contents {
            pairs.push((SLOT_1 + 2 * u16::from(i), item_guid as u32));
            pairs.push((SLOT_1 + 2 * u16::from(i) + 1, (item_guid >> 32) as u32));
        }
        objects.spawn_container(guid, ObjectFields::from_pairs(&pairs));
    }

    const ALL: ItemSearch = ItemSearch {
        equipment_only: false,
        live_charges_only: false,
    };
    const WORN: ItemSearch = ItemSearch {
        equipment_only: true,
        live_charges_only: false,
    };
    const CHARGED: ItemSearch = ItemSearch {
        equipment_only: false,
        live_charges_only: true,
    };

    /// A copy worn in trinket slot 13 wins over one in the backpack.
    #[test]
    fn equipment_is_searched_before_everything_else() {
        let mut objs = TestObjects::new();
        item(&mut objs, 0xE1, TRINKET, None);
        item(&mut objs, 0xB1, TRINKET, None);
        let store = player(&[(13, 0xE1), (23, 0xB1)]);
        assert_eq!(
            find_item(&store, &objs.get(), TRINKET, ALL),
            Some((255, 13, 0xE1))
        );
    }

    /// The equipment-only stage, which decides use in place against equip, stops at the doll.
    #[test]
    fn the_equipment_only_stage_ignores_the_bags() {
        let mut objs = TestObjects::new();
        item(&mut objs, 0xB1, TRINKET, None);
        let store = player(&[(23, 0xB1)]);
        assert_eq!(find_item(&store, &objs.get(), TRINKET, WORN), None);
        assert_eq!(
            find_item(&store, &objs.get(), TRINKET, ALL),
            Some((255, 23, 0xB1))
        );
    }

    /// The walk recurses into each container as it passes it.
    #[test]
    fn bag_contents_precede_the_backpack() {
        let mut objs = TestObjects::new();
        item(&mut objs, 0xC1, TRINKET, None);
        item(&mut objs, 0xB1, TRINKET, None);
        bag(&mut objs, 0xBA, BAG, &[(2, 0xC1)]);
        let store = player(&[(19, 0xBA), (23, 0xB1)]);
        assert_eq!(
            find_item(&store, &objs.get(), TRINKET, ALL),
            Some((19, 2, 0xC1)),
            "bag 1's inner slot 2, addressed by the bag's own player-array index"
        );
    }

    /// A bag is a placeable action (`InventoryType` 18 passes `PlaceAction`'s filter).
    #[test]
    fn an_equipped_bag_is_found_as_itself() {
        let mut objs = TestObjects::new();
        bag(&mut objs, 0xBA, BAG, &[]);
        let store = player(&[(19, 0xBA)]);
        assert_eq!(
            find_item(&store, &objs.get(), BAG, ALL),
            Some((255, 19, 0xBA))
        );
    }

    #[test]
    fn the_charge_filter_skips_a_spent_copy() {
        let mut objs = TestObjects::new();
        item(&mut objs, 0xB1, TRINKET, Some(0));
        item(&mut objs, 0xB2, TRINKET, Some(3));
        let store = player(&[(23, 0xB1), (24, 0xB2)]);
        assert_eq!(
            find_item(&store, &objs.get(), TRINKET, ALL),
            Some((255, 23, 0xB1)),
            "without the filter the first copy wins, spent or not"
        );
        assert_eq!(
            find_item(&store, &objs.get(), TRINKET, CHARGED),
            Some((255, 24, 0xB2)),
            "with it, the spent copy is skipped"
        );
    }

    /// The keyring is the walk's last band (mode bit `0x40`).
    #[test]
    fn a_key_in_the_keyring_is_found_last() {
        const KEY: u32 = 7_146; // The Scarlet Key
        let mut objs = TestObjects::new();
        item(&mut objs, 0xE1, KEY, None);
        let store = player(&[(86, 0xE1)]);
        assert_eq!(
            find_item(&store, &objs.get(), KEY, ALL),
            Some((255, 86, 0xE1))
        );

        // A copy anywhere earlier still wins.
        item(&mut objs, 0xE2, KEY, None);
        let store = player(&[(86, 0xE1), (23, 0xE2)]);
        assert_eq!(
            find_item(&store, &objs.get(), KEY, ALL),
            Some((255, 23, 0xE2)),
            "the backpack copy precedes the keyring one"
        );
    }

    /// `HasKey()` at mode `0x4f`: a key counts anywhere, the bank included.
    #[test]
    fn has_key_finds_a_key_anywhere_the_reference_looks() {
        use super::{has_key, BAG_FAMILY_KEYS};
        use crate::items::{test_template, Items};
        use crate::net::NetCommands;

        const KEY: u32 = 7_146; // The Scarlet Key (bag_family 9)
        const BREAD: u32 = 4_540;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let mut objs = TestObjects::new();
        let mut items = Items::default();
        let mut key_tpl = test_template("The Scarlet Key");
        key_tpl.bag_family = BAG_FAMILY_KEYS;
        items.insert_template(KEY, Some(key_tpl));
        items.insert_template(BREAD, Some(test_template("Tough Hunk of Bread")));
        item(&mut objs, 0xF1, KEY, None);
        item(&mut objs, 0xF2, BREAD, None);

        assert!(!has_key(&player(&[]), &objs.get(), &items, &commands));
        // A non-key in the backpack is not a key.
        assert!(!has_key(
            &player(&[(23, 0xF2)]),
            &objs.get(),
            &items,
            &commands
        ));
        // A key in keyring slot 1.
        assert!(has_key(
            &player(&[(86, 0xF1)]),
            &objs.get(),
            &items,
            &commands
        ));
        // And in the backpack, before it has been filed.
        assert!(has_key(
            &player(&[(23, 0xF1)]),
            &objs.get(),
            &items,
            &commands
        ));

        // The bank: mode `0x4f` reaches it, `find_item`'s `0x47` does not.
        let mut banked = std::collections::HashMap::new();
        banked.insert(39u16, 0xF1u64);
        let store = bank_player(&banked);
        assert!(
            has_key(&store, &objs.get(), &items, &commands),
            "a key in the bank still gives you a keyring"
        );
        assert_eq!(
            find_item(&store, &objs.get(), KEY, ALL),
            None,
            "...while the ordinary item search never reaches the bank"
        );
    }

    /// A player with items in the bank band (`PLAYER_FIELD_BANK_SLOT_1`).
    fn bank_player(slots: &std::collections::HashMap<u16, u64>) -> ObjectFields {
        const BANK_SLOT_1: u16 = 1516; // PLAYER_FIELD_BANK_SLOT_1 (2 per guid, player slots 39..66)
        let mut pairs = Vec::new();
        for (&idx, &guid) in slots {
            let base = BANK_SLOT_1 + 2 * (idx - 39);
            pairs.push((base, guid as u32));
            pairs.push((base + 1, (guid >> 32) as u32));
        }
        ObjectFields::from_pairs(&pairs)
    }
}

/// [`count_of`]'s scope: the quest surfaces pass mask `8`, rewritten to `0x4F`, and count banked
/// copies; mask `0` becomes `0x47`, which does not.
#[cfg(test)]
mod count_of_tests {
    use super::{count_of, InventoryScope, TestObjects};
    use benilla_protocol::ObjectFields;

    // Descriptor field indices, raw.
    const ENTRY: u16 = 3; // OBJECT_FIELD_ENTRY
    const STACK: u16 = 14; // ITEM_FIELD_STACK_COUNT
    const NUM_SLOTS: u16 = 48; // CONTAINER_FIELD_NUM_SLOTS
    const SLOT_1: u16 = 50; // CONTAINER_FIELD_SLOT_1 (2 fields per guid)
    const INV_SLOT_HEAD: u16 = 486; // player slots 0..22
    const PACK_SLOT_1: u16 = 532; // player slots 23..38
    const BANK_SLOT_1: u16 = 1516; // player slots 39..66
    const BANK_BAG_SLOT_1: u16 = 1572; // player slots 67..73
    const KEYRING_SLOT_1: u16 = 648; // player slots 86..117

    const AMMO: u32 = 3_030; // the collect-quest item under test

    /// A player with the given `(player slot, guid)` pairs, each band its own descriptor array.
    fn player(slots: &[(u16, u64)]) -> ObjectFields {
        let mut pairs = Vec::new();
        for &(slot, guid) in slots {
            let base = match slot {
                0..=22 => INV_SLOT_HEAD + 2 * slot,
                23..=38 => PACK_SLOT_1 + 2 * (slot - 23),
                39..=66 => BANK_SLOT_1 + 2 * (slot - 39),
                67..=73 => BANK_BAG_SLOT_1 + 2 * (slot - 67),
                86..=117 => KEYRING_SLOT_1 + 2 * (slot - 86),
                _ => panic!("slot {slot} is buyback or out of range"),
            };
            pairs.push((base, guid as u32));
            pairs.push((base + 1, (guid >> 32) as u32));
        }
        ObjectFields::from_pairs(&pairs)
    }

    /// An item instance of `entry` holding `stack` copies.
    fn stack(objects: &mut TestObjects, guid: u64, entry: u32, stack: u32) {
        objects.spawn(
            guid,
            ObjectFields::from_pairs(&[(ENTRY, entry), (STACK, stack)]),
        );
    }

    /// A container instance holding `contents` at its own inner slots.
    fn bag(objects: &mut TestObjects, guid: u64, contents: &[(u8, u64)]) {
        let mut pairs = vec![(ENTRY, 4_500), (NUM_SLOTS, 16)];
        for &(slot, held) in contents {
            pairs.push((SLOT_1 + 2 * u16::from(slot), held as u32));
            pairs.push((SLOT_1 + 2 * u16::from(slot) + 1, (held >> 32) as u32));
        }
        objects.spawn_container(guid, ObjectFields::from_pairs(&pairs));
    }

    /// Mask `8` is the bit that adds the bank.
    #[test]
    fn a_quest_objective_counts_banked_copies_and_nothing_else_does() {
        let store = player(&[(23, 0xA1), (39, 0xB1)]); // one stack in the backpack, one in the bank
        let mut objs = TestObjects::new();
        stack(&mut objs, 0xA1, AMMO, 3);
        stack(&mut objs, 0xB1, AMMO, 5);
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::QUEST_ITEMS),
            8,
            "mask 0x4F sees the bank"
        );
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::CARRIED),
            3,
            "an action-bar/reagent count must NOT see the bank"
        );
    }

    /// Sections gate only at the player's root, so a bank bag's contents are walked.
    #[test]
    fn a_quest_objective_counts_the_contents_of_bank_bags() {
        let store = player(&[(67, 0xBB)]); // a bag in bank-bag slot 1
        let mut objs = TestObjects::new();
        bag(&mut objs, 0xBB, &[(0, 0xC1), (4, 0xC2)]);
        stack(&mut objs, 0xC1, AMMO, 2);
        stack(&mut objs, 0xC2, AMMO, 6);
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::QUEST_ITEMS),
            8
        );
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::CARRIED),
            0
        );
    }

    /// Worn gear and the keyring: in `0x47` and the quest scope, not [`InventoryScope::CARRIED`].
    #[test]
    fn the_quest_scope_also_reaches_worn_gear_and_the_keyring() {
        let store = player(&[(5, 0xE1), (86, 0xF1)]); // a worn copy and one in the keyring
        let mut objs = TestObjects::new();
        stack(&mut objs, 0xE1, AMMO, 1);
        stack(&mut objs, 0xF1, AMMO, 1);
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::QUEST_ITEMS),
            2
        );
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::CARRIED),
            0,
            "the carried scope reaches neither worn gear nor the keyring — the named narrowing"
        );
    }

    #[test]
    fn carried_bags_are_unchanged_in_both_scopes() {
        let store = player(&[(19, 0xBA), (25, 0xA2)]);
        let mut objs = TestObjects::new();
        bag(&mut objs, 0xBA, &[(2, 0xC1)]);
        stack(&mut objs, 0xC1, AMMO, 4);
        stack(&mut objs, 0xA2, AMMO, 1);
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::QUEST_ITEMS),
            5
        );
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::CARRIED),
            5
        );
    }

    /// Buyback (slots 74-85) has no mask bit.
    #[test]
    fn buyback_is_never_counted() {
        // The walk has no buyback accessor; the bands on either side prove it still reaches them.
        let store = player(&[(38, 0xA1), (73, 0xBB)]);
        let mut objs = TestObjects::new();
        stack(&mut objs, 0xA1, AMMO, 1);
        bag(&mut objs, 0xBB, &[(0, 0xC1)]);
        stack(&mut objs, 0xC1, AMMO, 1);
        assert_eq!(
            count_of(&store, &objs.get(), AMMO, InventoryScope::QUEST_ITEMS),
            2,
            "last backpack slot + last bank bag, with buyback between them untouched"
        );
    }
}
