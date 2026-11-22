//! The bag-slot verbs: `PutItemInBag`, `PutItemInBackpack` and `PickupBagFromSlot`.

use mlua::{Lua, MultiValue, Value};

use super::{
    clear_cursor, clear_cursor_with, set_cursor, set_item_lock, CursorItem, CursorPayload,
};
use super::{doll::pickup_inventory_item, EQUIPMENT_BAG};

/// The ids `PutItemInBag` and `PickupBagFromSlot` take: what their argument reader (`0x5e7c00`)
/// accepts above each floor gate (`0x5ea7a9`, `0x5ea866`: 0-based 19 and up), so the equipped
/// bags 20-23, the bank's slots 40-67, its bag slots 68-74 and the keyring 87-118.
fn accepts(live: u32) -> bool {
    (20..=23).contains(&live)
        || (40..=67).contains(&live)
        || BANK_BAG_INV_SLOTS.contains(&live)
        || (87..=118).contains(&live)
}

/// The live container id of a bag slot, the inverse of `ContainerIDToInventoryID`; a bank item or keyring slot has none, and its target is the item in the slot itself (`0x5e8dbf`).
fn autostore_container(live: u32) -> Option<i64> {
    match live {
        20..=23 => Some(i64::from(live) - 19),
        68..=74 => Some(i64::from(live) - 63),
        _ => None,
    }
}

/// `Bag0Slot`, the first equipped bag slot: only a container names it in `equip_slots`.
const BAG0_SLOT: u8 = 20;

/// The seven bank bag slots' ids (`BankButtonIDToInvSlotID(5..=11, isBag)`).
pub(super) const BANK_BAG_INV_SLOTS: std::ops::RangeInclusive<u32> = 68..=74;

/// Whether the held item is a container, the reference's `TYPEMASK_CONTAINER` test: only
/// `INVTYPE_BAG` maps to the bag slots in `FindEquipSlot`, so no other item names Bag0Slot.
fn is_container(item: &CursorItem) -> bool {
    item.equip_slots.contains(&BAG0_SLOT)
}

/// Whether the held item may go in doll slot `id`: its `equip_slots`, or any item at a bank bag
/// slot, which `FindEquipSlot` never names: the reference's slot check (`0x6d4a00`) passes every
/// 0-based slot from 23 (`0x7084ce`), so the swap goes out and the server refuses a non-bag.
pub(super) fn fits_slot(item: &CursorItem, id: u32) -> bool {
    if BANK_BAG_INV_SLOTS.contains(&id) {
        return true;
    }
    u8::try_from(id).is_ok_and(|id| item.equip_slots.contains(&id))
}

/// `PutItemInBag(inventorySlot)` (`0x5ea780` -> `0x5e8d30`): true (the reference's `1`) when the click was consumed against an occupied slot, else false (nil). An empty cursor's nil is what lets `BagSlotButton_OnClick` open the bag.
pub(super) fn put_item_in_bag(model: &mut crate::script::ModelGuard<'_>, live: u32) -> bool {
    if !accepts(live) {
        return false;
    }
    // A bank bag slot with no banker open does nothing (`0x5e8d75`).
    if BANK_BAG_INV_SLOTS.contains(&live) && crate::script::read_live(|l| l.banker()) == Some(None)
    {
        return false;
    }
    let occupied = usize::try_from(live)
        .ok()
        .and_then(|slot| model.inv_slot("player", slot))
        .is_some();
    if !occupied {
        // An empty slot: the doll handler equips the bag, and the answer is nil whether or not it went (`0x5e8dde`).
        pickup_inventory_item(model, live);
        return false;
    }
    // A locked bag only clears the cursor (`0x5e8e09`), as the held bag's own slot does (`0x5e8e1c`).
    let held = match model.cursor.clone() {
        Some(CursorPayload::Item(held)) => Some(held),
        _ => None,
    };
    let own = held
        .as_ref()
        .is_some_and(|h| h.bag == EQUIPMENT_BAG && h.slot == live);
    if own || super::doll::is_inventory_item_locked(model, live) {
        clear_cursor(model);
        return true;
    }
    // No held item: a held vendor row is bought into the bag (`0x5e8fae`), then the cursor clears and the answer is nil (`0x5e8fb6`).
    let Some(held) = held else {
        buy_held_row(model, autostore_container(live), live);
        clear_cursor(model);
        return false;
    };
    // An item already inside this bag just drops (`0x5e8e3b`).
    if autostore_container(live) == Some(held.bag) {
        clear_cursor(model);
        return true;
    }
    // A bag off a bag slot, or any container, swaps with this one: the slot's bag locks (`0x5e8ea1`, `0x5e8ede`), then the swap runs (`0x5e8f0c`).
    if is_container(&held) || (held.bag == EQUIPMENT_BAG && (20..=23).contains(&held.slot)) {
        set_item_lock(model, EQUIPMENT_BAG, live, true);
        crate::script::container::swap_held(model, &held, EQUIPMENT_BAG, live);
        return true;
    }

    // Store into the slot's item, a bag or not: `CMSG_AUTOSTORE_BAG_ITEM` (`0x10B`), or `CMSG_SPLIT_ITEM` (`0x10E`) for a split (`0x5e8f45`, `0x5e8f60`). The client picks no slot: the store carries none, the split carries `0xFF`.
    let dst_bag = autostore_container(live);
    model.bag_autostores.push(crate::script::BagAutoStore {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag: dst_bag.unwrap_or(0),
        dst_player_slot: dst_bag.is_none().then_some((live - 1) as u8),
        count: held.count,
    });
    // The cursor clears with its item still locked until the answer (`0x5e8f69`).
    clear_cursor_with(model, false, true);
    true
}

/// A held vendor row bought into Lua container `bag`, the vendor open (`0x5e8f7d`-`0x5e8fae`); a row that no longer resolves buys nothing, where the reference dereferences it unchecked.
fn buy_held_row(model: &mut crate::script::ModelGuard<'_>, bag: Option<i64>, live: u32) {
    let Some(CursorPayload::Merchant(held)) = model.cursor.clone() else {
        return;
    };
    let resolves = crate::script::merchant::merchant_row(
        model,
        crate::script::MerchantList::Items,
        held.row as usize,
        false,
    )
    .is_some_and(|it| it.item_id == held.item_id);
    if resolves {
        // The buy names the slot's item as its bag, a bag or not (`0x5e8fae`).
        let act = match bag {
            Some(bag) => crate::script::MerchantAct::BuyInBag { bag, row: held.row },
            None => crate::script::MerchantAct::BuyInItem {
                player_slot: (live - 1) as u8,
                row: held.row,
            },
        };
        crate::script::merchant::merchant_act(model, act);
    }
}

/// `PutItemInBackpack()` (`0x5ea7f0`), `PutItemInBag(0xFF)` whose target is the player's own guid: no empty-slot leg and no swap, so even a bag is stored into the backpack; no held item clears the cursor and answers nil (`0x5e8fb6`), and an item already in the backpack just drops (`0x5e8e47`).
pub(super) fn put_item_in_backpack(model: &mut crate::script::ModelGuard<'_>) -> bool {
    let Some(CursorPayload::Item(held)) = model.cursor.clone() else {
        buy_held_row(model, Some(0), 0);
        clear_cursor(model);
        return false;
    };
    if held.bag == 0 {
        clear_cursor(model);
        return true;
    }
    model.bag_autostores.push(crate::script::BagAutoStore {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag: 0,
        dst_player_slot: None,
        count: held.count,
    });
    // The cursor clears with its item still locked until the answer (`0x5e8f69`).
    clear_cursor_with(model, false, true);
    true
}

/// `PickupBagFromSlot(inventorySlot)`: clears a held payload before it looks at the slot, where the
/// reference clears only a held item; with nothing held it skips the clear and its `CURSOR_UPDATE`,
/// as the reference does. Takes only a container, and never places or swaps. The client has no
/// empty-bag rule: the server refuses with `EQUIP_ERR_CAN_ONLY_DO_WITH_EMPTY_BAGS`.
pub(super) fn pickup_bag_from_slot(model: &mut crate::script::ModelGuard<'_>, live: u32) {
    if !accepts(live) {
        return;
    }
    if model.cursor.is_some() {
        clear_cursor(model);
    }
    let picked = usize::try_from(live)
        .ok()
        .and_then(|slot| model.inv_slot("player", slot))
        .filter(|s| s.item_id != 0 && !super::doll::is_inventory_item_locked(model, live))
        .map(|s| CursorItem {
            bag: EQUIPMENT_BAG,
            slot: live,
            item_id: s.item_id,
            texture: s.icon.clone(),
            link: s.link.clone(),
            quality: Some(u32::try_from(s.quality).unwrap_or(0)),
            count: None,
            bar_placeable: s.bar_placeable,
            equip_slots: s.equip_slots.clone(),
        })
        .filter(is_container);
    // The setter, then the lock (`0x5e8d17`).
    if let Some(item) = picked {
        set_cursor(model, CursorPayload::Item(item));
        set_item_lock(model, EQUIPMENT_BAG, live, true);
    }
}

/// Register the three verbs as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The number 1 or nil, never a boolean: the reference pushes Lua tag 3, a number.
    fn consumed(yes: bool) -> mlua::Result<MultiValue> {
        Ok(MultiValue::from_vec(vec![if yes {
            Value::Number(1.0)
        } else {
            Value::Nil
        }]))
    }

    g.set(
        "PutItemInBag",
        lua.create_function(|lua, live: Value| {
            let Some(live) = super::doll::live_id(lua, &live)? else {
                return consumed(false);
            };
            let yes = {
                let mut model = crate::script::model_mut_of(lua);
                put_item_in_bag(&mut model, live)
            };
            consumed(yes)
        })?,
    )?;
    g.set(
        "PutItemInBackpack",
        lua.create_function(|lua, ()| {
            let yes = {
                let mut model = crate::script::model_mut_of(lua);
                put_item_in_backpack(&mut model)
            };
            consumed(yes)
        })?,
    )?;
    g.set(
        "PickupBagFromSlot",
        lua.create_function(|lua, live: Value| {
            let Some(live) = super::doll::live_id(lua, &live)? else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            pickup_bag_from_slot(&mut model, live);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::EQUIPMENT_BAG;
    use crate::script::{
        BagAutoStore, BankBagSlots, ContainerMove, ContainerSlot, ContainerState, InvSlotView,
        InventorySlots, UiScript,
    };

    /// With nothing held the reference skips the clear, so a drag from an empty bag slot signals
    /// nothing.
    #[test]
    fn an_empty_cursor_skips_the_clear_and_its_signal() {
        let s = UiScript::new().unwrap();
        crate::script::tick::fired::take();
        s.run("PickupBagFromSlot(20)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert!(crate::script::tick::fired::names().is_empty());
    }

    /// An item that is a bag: `equip_slots` is `find_equip_slot(INVTYPE_BAG)`.
    fn bag_slot_view(item_id: u32) -> InvSlotView {
        InvSlotView {
            item_id,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        }
    }

    /// A backpack holding one bag in slot 1 and one ordinary (head-slot) item in slot 2.
    fn backpack() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                item_id: 4500,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                equip_slots: vec![20, 21, 22, 23],
                ..Default::default()
            },
        );
        slots.insert(
            2,
            ContainerSlot {
                item_id: 1234,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Helmet_01".into()),
                equip_slots: vec![1],
                ..Default::default()
            },
        );
        ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// Bank bag slot 1 (live id 68) holds a bag; the equipped bag slots stay empty.
    fn one_bank_bag() -> BankBagSlots {
        let mut bags: BankBagSlots = Default::default();
        bags[0] = Some(bag_slot_view(4500));
        bags
    }

    #[test]
    fn an_empty_cursor_answers_nil_and_sends_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_bank_bag_slots(one_bank_bag());
        assert!(s.eval::<bool>("return PutItemInBag(68) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBag(20) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBackpack() == nil").unwrap());
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    /// The nil after the equip is the reference's, so the same click opens the bag.
    #[test]
    fn an_empty_bag_slot_equips_the_held_bag_and_still_answers_nil() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(68) == nil").unwrap());
        assert!(s.cursor_item().is_none(), "placed, not kept");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 68,
                count: None,
            }]
        );
        assert!(s.take_bag_autostores().is_empty(), "an equip, not a store");
    }

    /// A non-bag on an empty bank bag slot swaps there too (`0x5e8dde`, `0x5e89c0`), for the
    /// server to refuse.
    #[test]
    fn an_empty_bank_bag_slot_sends_an_ordinary_item_for_the_server_to_refuse() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(68)").unwrap());

        assert!(s.eval::<bool>("return PutItemInBag(68) == nil").unwrap());
        assert!(s.cursor_item().is_none(), "sent, not kept");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 2,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 68,
                count: None,
            }]
        );
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn an_occupied_bag_slot_swaps_with_a_held_bag_and_answers_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(68)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 68,
                count: None,
            }]
        );
    }

    #[test]
    fn an_occupied_bag_slot_auto_stores_an_ordinary_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 2)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(68)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty(), "a store, not a swap");
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: 0,
                src_slot: 2,
                // Container 5 is bank bag slot 1 (`ContainerIDToInventoryID(5) == 68`).
                dst_bag: 5,
                dst_player_slot: None,
                count: None,
            }]
        );
    }

    /// A key's slot is no bag: the store names the key itself as its bag, by the player slot holding it, for the server to refuse (`0x5e8f60` -> `0x6dbb30`), and the cursor clears with the item still locked (`0x5e8f69`).
    #[test]
    fn an_occupied_keyring_slot_takes_the_store_for_the_server_to_refuse() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        let mut keys = std::collections::HashMap::new();
        keys.insert(
            1,
            ContainerSlot {
                item_id: 5396,
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(
            -2,
            Some(ContainerState {
                bag_family: 0,
                name: None,
                num_slots: 32,
                slots: keys,
            }),
        );
        s.run("PickupContainerItem(0, 2)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBag(87)").unwrap(), 1);
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: 0,
                src_slot: 2,
                dst_bag: 0,
                // Live id 87 is the first keyring slot, player slot 86.
                dst_player_slot: Some(86),
                count: None,
            }]
        );
        assert!(s
            .eval::<bool>("return select(3, GetContainerItemInfo(0, 2)) == 1")
            .unwrap());
    }

    /// The backpack button stores whatever is held, a bag included, and the cursor clears with the item still locked (`0x5e8f69`); an item already in the backpack just drops (`0x5e8e47`), and no held item clears the cursor and answers nil (`0x5e8fb6`).
    #[test]
    fn the_backpack_button_always_auto_stores_even_a_bag() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_container(1, Some(backpack()));
        s.run("PickupContainerItem(1, 1)").unwrap();

        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert!(s.take_container_moves().is_empty());
        assert_eq!(
            s.take_bag_autostores(),
            vec![BagAutoStore {
                src_bag: 1,
                src_slot: 1,
                dst_bag: 0,
                dst_player_slot: None,
                count: None,
            }]
        );
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(1, 1) return locked")
            .unwrap());

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.eval::<i64>("return PutItemInBackpack()").unwrap(), 1);
        assert!(s.cursor_item().is_none() && s.take_bag_autostores().is_empty());
        assert!(!s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());

        s.run(r#"local f = CreateFrame("Frame") f:RegisterEvent("CURSOR_UPDATE") f:SetScript("OnEvent", function() Cleared = true end)"#).unwrap();
        assert!(s
            .eval::<bool>("return PutItemInBackpack() == nil and Cleared")
            .unwrap());
    }

    #[test]
    fn the_answer_is_the_number_one_not_the_boolean_true() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(one_bank_bag());
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s
            .eval::<bool>("return PutItemInBag(68) == 1 and type(PutItemInBackpack()) ~= 'boolean'")
            .unwrap());
    }

    #[test]
    fn equipment_slots_are_below_the_floor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        let mut doll: InventorySlots = Default::default();
        doll[16] = Some(bag_slot_view(9999));
        s.set_inventory_slots(doll);
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return PutItemInBag(16) == nil").unwrap());
        assert!(s.eval::<bool>("return PutItemInBag(0) == nil").unwrap());
        assert!(s.cursor_item().is_some(), "untouched");
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_bag_autostores().is_empty());
    }

    #[test]
    fn pickup_bag_from_slot_takes_the_bag_and_returns_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_bank_bag_slots(one_bank_bag());
        assert_eq!(
            s.arity("PickupBagFromSlot(68)").unwrap(),
            0,
            "the delegate's eax is never tested — no return values at all"
        );
        let held = s.cursor_item().expect("picked up");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 68, 4500)
        );
        assert!(s.eval::<bool>("return IsInventoryItemLocked(68)").unwrap());
    }

    #[test]
    fn pickup_bag_from_slot_drops_whatever_was_held_first() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_bank_bag_slots(Default::default());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("PickupBagFromSlot(69)").unwrap();
        assert!(s.cursor_item().is_none(), "dropped, and nothing picked up");
        assert!(s.take_container_moves().is_empty(), "no place path exists");
    }

    #[test]
    fn pickup_bag_from_slot_only_takes_containers_and_never_a_locked_one() {
        let mut s = UiScript::new().unwrap();
        // Bank slot 3 is live id 42: inside the gate, but an ordinary item.
        let mut vault = std::collections::HashMap::new();
        vault.insert(
            3,
            ContainerSlot {
                item_id: 1234,
                count: 1,
                equip_slots: vec![1],
                ..Default::default()
            },
        );
        s.set_container(
            -1,
            Some(ContainerState {
                bag_family: 0,
                name: Some("Bank".into()),
                num_slots: 24,
                slots: vault,
            }),
        );
        s.run("PickupBagFromSlot(42)").unwrap();
        assert!(s.cursor_item().is_none(), "not a container — silent no-op");

        let mut bags = one_bank_bag();
        bags[0] = Some(InvSlotView {
            locked: true,
            ..bag_slot_view(4500)
        });
        s.set_bank_bag_slots(bags);
        s.run("PickupBagFromSlot(68)").unwrap();
        assert!(s.cursor_item().is_none(), "locked — refused");
    }
}
