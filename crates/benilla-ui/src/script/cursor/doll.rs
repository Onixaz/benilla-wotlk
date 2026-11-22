//! The paper-doll cursor verbs: an equipped slot is an item source under
//! [`super::EQUIPMENT_BAG`], so the shared cursor code handles its cancel, clear and world drop.

use crate::script::container::SwapGate;
use mlua::{Lua, Value};

use crate::script::binding_abi::flag;
use crate::script::Model;

use super::{clear_cursor, set_cursor, set_item_lock, CursorItem, CursorPayload, EQUIPMENT_BAG};

/// `PickupInventoryItem(id)`, the doll slot's click, on the bag click's model: a fitting whole item
/// moves in and the cursor clears, with no hop; a misfit on the doll's 23 slots auto-equips
/// (`0x5e89f3`), one off a worn slot going back instead (`0x5e89e5`); a split stays held. Returns
/// whether to repaint.
pub(super) fn pickup_inventory_item(model: &mut crate::script::ModelGuard<'_>, id: u32) -> bool {
    // Head 1 to Tabard 19, the bag slots 20-23, and the bank bag slots, which only `PutItemInBag`
    // reaches. Ammo (0) is refused; its leg is not built.
    if !(1..=23).contains(&id) && !super::bag_verbs::BANK_BAG_INV_SLOTS.contains(&id) {
        return false;
    }
    // Only a held item or vendor row takes the click first. Past them an empty slot or a locked
    // item does nothing, an item-targeting spell binds the item, so a worn weapon can be poisoned,
    // and repair mode repairs a worn item, any other payload staying held.
    let placing = matches!(
        model.cursor,
        Some(CursorPayload::Item(_) | CursorPayload::Merchant(_))
    );
    let picking = crate::script::spellbook::targeting().wants_item;
    if !placing && (picking || crate::script::merchant::repair_mode(model)) {
        let usable = model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0)
            && !is_inventory_item_locked(model, id);
        if usable && picking {
            model.item_picks.push((EQUIPMENT_BAG, id));
        } else if usable && (1..=19).contains(&id) {
            model.inventory_repairs.push(id);
        }
        return false;
    }
    match model.cursor.clone() {
        None => {
            let picked = model
                .inv_slot("player", id as usize)
                .filter(|s| s.item_id != 0 && !is_inventory_item_locked(model, id))
                .map(|s| CursorItem {
                    bag: EQUIPMENT_BAG,
                    slot: id,
                    item_id: s.item_id,
                    texture: s.icon.clone(),
                    link: s.link.clone(),
                    quality: Some(u32::try_from(s.quality).unwrap_or(0)),
                    count: None,
                    bar_placeable: s.bar_placeable,
                    equip_slots: s.equip_slots.clone(),
                });
            match picked {
                // The setter, then the lock (`0x5e893a`).
                Some(item) => {
                    set_cursor(model, CursorPayload::Item(item));
                    set_item_lock(model, EQUIPMENT_BAG, id, true);
                    true
                }
                None => false,
            }
        }
        Some(CursorPayload::Item(held)) if held.bag == EQUIPMENT_BAG && held.slot == id => {
            clear_cursor(model);
            true
        }
        Some(CursorPayload::Item(held)) => {
            // A split stays held, with no event: nothing changed.
            if held.count.is_some() {
                return false;
            }
            // A misfit takes the auto-equip path, the server picking the slot (`0x5e89f3`); every
            // item fits a bank bag slot.
            if !super::bag_verbs::fits_slot(&held, id) {
                return auto_equip_cursor_item(model);
            }
            // A locked item in the slot refuses the swap, the cursor as it was (`0x5e8976`).
            if is_inventory_item_locked(model, id) {
                return false;
            }
            // The swap runs (`0x5e89c0`), then the slot's item locks whatever it did (`0x5e89c7`).
            crate::script::container::swap_held(model, &held, EQUIPMENT_BAG, id);
            set_item_lock(model, EQUIPMENT_BAG, id, true);
            true
        }
        // Mode 5, a buy into this slot: the reference sends `CMSG_BUY_ITEM_IN_SLOT` (`0x1a3`) and
        // the server equips or refuses. A stale row buys nothing: of the reference's three drop
        // sites only this one checks it; the other two dereference it.
        Some(CursorPayload::Merchant(held)) => {
            let row = crate::script::merchant::merchant_row(
                model,
                crate::script::MerchantList::Items,
                held.row as usize,
                false,
            )
            .filter(|it| it.item_id == held.item_id)
            .map(|_| held.row);
            if let Some(row) = row {
                crate::script::merchant::merchant_act(
                    model,
                    crate::script::MerchantAct::BuyInSlot {
                        bag: EQUIPMENT_BAG,
                        slot: id,
                        row,
                    },
                );
            }
            clear_cursor(model);
            true
        }
        Some(
            CursorPayload::Spell(_)
            | CursorPayload::Action(_)
            | CursorPayload::Macro(_)
            | CursorPayload::EquipmentSet(_)
            | CursorPayload::PetAction(_)
            // Mode 10: a stabled pet stays held for the stable window.
            | CursorPayload::StablePet(_)
            // Mode 2: coins stay held for a money frame's drop. On an occupied slot the reference
            // picks the item up over either, as it tests only for a held item or a preview row
            // before the pickup.
            | CursorPayload::Money(_)
            // Modes 11 and 12, from the guild bank, stay held for its own buttons.
            | CursorPayload::GuildBankItem(_)
            | CursorPayload::GuildBankMoney(_),
        ) => false,
    }
}

/// `EquipCursorItem(slot)` past its argument gate (`0x51a3d0`), `id` the 1-based slot: no held item does nothing; the slot's own item clears the cursor (`0x51a505`); an item the slot takes swaps into it, the slot's item locking after the swap whatever it holds (`0x51a4ce`, `0x51a4db`); a misfit off a worn slot clears; anything else auto-equips with the bind question off (`0x51a51a`).
pub(super) fn equip_cursor_item(model: &mut crate::script::ModelGuard<'_>, id: i64) {
    let Some(CursorPayload::Item(held)) = model.cursor.clone() else {
        return;
    };
    // The 0-based slots -1 and 255 skip to the auto-equip (`0x51a46d`, `0x51a479`).
    if let Some(id) = u32::try_from(id).ok().filter(|&id| id != 0 && id != 0x100) {
        if held.bag == EQUIPMENT_BAG && held.slot == id {
            clear_cursor(model);
            return;
        }
        if super::bag_verbs::fits_slot(&held, id) {
            crate::script::container::swap_held(model, &held, EQUIPMENT_BAG, id);
            set_item_lock(model, EQUIPMENT_BAG, id, true);
            return;
        }
        if held.bag == EQUIPMENT_BAG && (1..=19).contains(&held.slot) {
            clear_cursor(model);
            return;
        }
    }
    auto_equip_held(model, true);
}

/// `CursorCanGoInSlot(id)`, the doll slot's highlight on `CURSOR_UPDATE`
/// (`PaperDollFrame.lua:609-615`): `equip_slots`, which follows vmangos `Player::FindEquipSlot`,
/// where the reference's slot test reads a static slot mask per `InventoryType`.
pub(super) fn cursor_can_go_in_slot(model: &Model, id: u32) -> bool {
    match &model.cursor {
        Some(CursorPayload::Item(item)) => super::bag_verbs::fits_slot(item, id),
        _ => false,
    }
}

/// `AutoEquipCursorItem()`, a click or drop on the doll's model (`PaperDollFrame.lua:31-35`): a
/// whole stack from a bag becomes `CMSG_AUTOEQUIP_ITEM`: the server picks the slot and swaps
/// the displaced piece back (`ItemHandler.cpp:138-228`); an item off the doll's 23 slots goes back
/// with nothing sent (`0x6dfcf3`); a split stays held. Returns whether to repaint.
pub(crate) fn auto_equip_cursor_item(model: &mut crate::script::ModelGuard<'_>) -> bool {
    auto_equip_held(model, false)
}

/// [`auto_equip_cursor_item`], `suppress` skipping the record's bind arm (`0x6dfd7c`).
fn auto_equip_held(model: &mut crate::script::ModelGuard<'_>, suppress: bool) -> bool {
    match model.cursor.clone() {
        Some(CursorPayload::Item(item))
            if item.bag == EQUIPMENT_BAG && (1..=23).contains(&item.slot) =>
        {
            clear_cursor(model);
            true
        }
        Some(CursorPayload::Item(item)) if item.bag >= 0 && item.count.is_none() => {
            let gate =
                crate::script::read_live(|l| l.auto_equip_gate(item.bag, item.slot)).flatten();
            #[cfg(any(test, feature = "fixtures"))]
            let gate = gate.or(model
                .swap_gates
                .get(&(item.bag, item.slot))
                .map(|&g| (g, false)));
            let (gate, ammo) = gate.unwrap_or((SwapGate::Send, false));
            let gate = match gate {
                SwapGate::Wait(_) | SwapGate::Ask(_) if suppress => SwapGate::Send,
                gate => gate,
            };
            let queue = |model: &mut crate::script::Model| {
                if suppress {
                    model
                        .container_autoequips_unasked
                        .push((item.bag, item.slot));
                } else {
                    model.container_autoequips.push((item.bag, item.slot));
                }
            };
            match gate {
                // No item object: nothing moves (`0x6dfd45`).
                SwapGate::Refuse => false,
                // The record is out: its callback runs the equip again (`0x6dfd99`, `0x6dbbe0`).
                SwapGate::Wait(entry) => {
                    crate::script::item_stats::wait_item_record(
                        model,
                        entry,
                        crate::script::ItemRecordWait::AutoEquip,
                    );
                    crate::script::item_stats::ask_item_record(model, entry);
                    false
                }
                // `AUTOEQUIP_BIND_CONFIRM` is asked and the cursor stays as it was (`0x6dfe31`).
                SwapGate::Ask(entry)
                    if crate::script::item_stats::item_usable_by_id(model, entry) =>
                {
                    let ask = crate::script::HostAsk::BindAutoEquip {
                        bag: item.bag,
                        slot: item.slot,
                    };
                    match model.ask(&ask) {
                        Some(crate::script::HostAnswer::BindIndex(index)) => model.raise(
                            "AUTOEQUIP_BIND_CONFIRM",
                            vec![crate::script::ScriptValue::Int(i64::from(index))],
                        ),
                        // No host answers in the call: the app's drain files and asks.
                        _ => queue(model),
                    }
                    false
                }
                // The equip goes out and the cursor clears, its item still locked until the answer (`0x6dfe81`, `0x6dfebd`); ammunition is set by its id and unlocks (`0x6dfe51`).
                SwapGate::Send | SwapGate::Ask(_) => {
                    queue(model);
                    super::clear_cursor_with(model, ammo, true);
                    true
                }
            }
        }
        _ => false,
    }
}

/// `UseInventoryItem(id)`, the doll slot's right-click (`PaperDollFrame.lua:658-659`): the cursor
/// clears before the slot is read, so a held payload goes back, never placed; then the app sends
/// `CMSG_USE_ITEM` with bag 255 and the 0-based slot, dropping an empty slot unsent, or in repair
/// mode a worn item is repaired, locked or not.
pub(super) fn use_inventory_item(model: &mut crate::script::ModelGuard<'_>, id: u32) {
    super::clear_cursor(model);
    if !crate::script::merchant::repair_mode(model) {
        model
            .script_calls
            .push(crate::script::ScriptCall::UseInventoryItem(id));
        return;
    }
    if (1..=19).contains(&id)
        && model
            .inv_slot("player", id as usize)
            .is_some_and(|s| s.item_id != 0)
    {
        model.inventory_repairs.push(id);
    }
}

/// `IsInventoryItemLocked(id)`: the item's lock bit (`item+0x394` bit 0), read at the call beside the slot's fed one. Reads [`Model::inv_slot`], since `BankFrameItemButton_UpdateLock` asks about bank slots too.
pub(in crate::script) fn is_inventory_item_locked(model: &Model, id: u32) -> bool {
    let fed = usize::try_from(id)
        .ok()
        .and_then(|i| model.inv_slot("player", i))
        .is_some_and(|s| s.locked);
    fed || super::item_locked(model, EQUIPMENT_BAG, id)
}

/// A verb's slot argument as the live id, through the registrar's reader (`0x5e7c00`).
pub(super) fn live_id(lua: &Lua, v: &Value) -> mlua::Result<Option<u32>> {
    let live = crate::script::char_stats::live_slot_arg(lua, v)?;
    Ok(live.and_then(|id| u32::try_from(id).ok()))
}

/// Register the paper-doll verbs as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupInventoryItem",
        lua.create_function(|lua, id: Value| {
            let Some(id) = live_id(lua, &id)? else {
                return Ok(false);
            };
            let repaint = pickup_inventory_item(&mut crate::script::model_mut_of(lua), id);
            crate::script::calls::press_item_picks(lua);
            Ok(repaint)
        })?,
    )?;
    g.set(
        "EquipCursorItem",
        lua.create_function(|lua, slot: Value| {
            let id =
                crate::script::binding_abi::number_arg(lua, slot, "Usage: EquipCursorItem(slot)")?;
            let mut model = crate::script::model_mut_of(lua);
            equip_cursor_item(&mut model, i64::from(id));
            Ok(())
        })?,
    )?;
    g.set(
        "CursorCanGoInSlot",
        lua.create_function(|lua, id: Value| {
            let Some(id) = live_id(lua, &id)? else {
                return Ok(false);
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(cursor_can_go_in_slot(&model, id))
        })?,
    )?;
    g.set(
        "AutoEquipCursorItem",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            Ok(auto_equip_cursor_item(&mut model))
        })?,
    )?;
    g.set(
        "UseInventoryItem",
        lua.create_function(|lua, id: Value| {
            // The ammo leg returns with the refused slots (`0x5ea630`).
            let Some(id) = live_id(lua, &id)?.filter(|&id| id != 0) else {
                return Ok(());
            };
            use_inventory_item(&mut crate::script::model_mut_of(lua), id);
            crate::script::calls::apply_queued(lua);
            Ok(())
        })?,
    )?;
    g.set(
        "IsInventoryItemLocked",
        lua.create_function(|lua, id: Value| {
            let Some(id) = live_id(lua, &id)? else {
                return Ok(Value::Nil);
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(is_inventory_item_locked(&model, id)))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::{CursorAction, CursorPayload, CursorSpell, EQUIPMENT_BAG};
    use crate::script::live::TestTargeting;
    use crate::script::{ContainerMove, ContainerSlot, ContainerState, InvSlotView, UiScript};

    /// Head (1), a ring in finger slot 11 that also fits 12, and Tabard (19).
    fn doll_slots() -> crate::script::InventorySlots {
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[1] = Some(InvSlotView {
            guid: 0,
            gems: [0; 3],
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 1234,
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            count: 1,
            contents_count: None,
            quality: 2,
            name: Some("Test Helm".into()),
            link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
            locked: false,
            equip_slots: vec![1],
            creator: None,
            enchants: Vec::new(),
        });
        slots[11] = Some(InvSlotView {
            guid: 0,
            gems: [0; 3],
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 555,
            icon: Some("Interface\\Icons\\INV_Jewelry_Ring_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Ring".into()),
            link: Some("|cffffffff|Hitem:555:0:0:0|h[Test Ring]|h|r".into()),
            locked: false,
            equip_slots: vec![11, 12], // either finger: the doll-to-doll swap
            creator: None,
            enchants: Vec::new(),
        });
        slots[19] = Some(InvSlotView {
            guid: 0,
            gems: [0; 3],
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 999,
            icon: Some("Interface\\Icons\\INV_Shirt_White_01".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Test Tabard".into()),
            link: Some("|cffffffff|Hitem:999:0:0:0|h[Test Tabard]|h|r".into()),
            locked: false,
            equip_slots: vec![19],
            creator: None,
            enchants: Vec::new(),
        });
        slots
    }

    fn one_fitting_bag_item() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                guid: 0,
                gems: [0; 3],
                quest_item: false,
                start_quest: 0,
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Helmet_02".into()),
                count: 1,
                quality: Some(3),
                item_id: 2000,
                link: Some("|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r".into()),
                locked: false,
                equip_slots: vec![1], // fits HeadSlot only
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    #[test]
    fn pickup_from_doll_slot_holds_and_locks() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        let held = s.cursor_item().expect("picked up");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 1, 1234)
        );
        assert_eq!(held.equip_slots, vec![1]);
        assert!(s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
    }

    #[test]
    fn pickup_same_doll_slot_cancels_no_move() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());
    }

    #[test]
    fn place_fitting_bag_item_onto_doll_slot_queues_move_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![1]);

        assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(s.cursor_item().is_none(), "a plain clear, no hop");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 1,
                count: None,
            }]
        );
    }

    /// A bag item dropped on a slot it does not fit auto-equips: no swap, the cursor cleared.
    #[test]
    fn a_misfit_from_a_bag_auto_equips() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        // The bag item only fits HeadSlot (1); try placing it on NeckSlot (2).
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return PickupInventoryItem(2)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    /// A worn item dropped on a slot it does not fit goes back, with nothing sent.
    #[test]
    fn a_misfit_from_a_worn_slot_goes_back() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("PickupInventoryItem(1)").unwrap();

        assert!(s.eval::<bool>("return PickupInventoryItem(2)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_container_autoequips().is_empty());
    }

    #[test]
    fn doll_to_doll_ring_swap_fits_via_shared_equip_slots() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(s.eval::<bool>("return PickupInventoryItem(11)").unwrap());
        assert_eq!(s.cursor_item().unwrap().equip_slots, vec![11, 12]);

        assert!(s.eval::<bool>("return PickupInventoryItem(12)").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: EQUIPMENT_BAG,
                src_slot: 11,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 12,
                count: None,
            }]
        );
    }

    #[test]
    fn split_carry_refuses_to_equip() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let mut state = one_fitting_bag_item();
        state.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(state));

        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        let held = s.cursor_item().expect("a split carry");
        assert_eq!(held.count, Some(2));

        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        let held = s.cursor_item().expect("kept — can't equip a partial stack");
        assert_eq!(held.count, Some(2));
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn pickup_inventory_item_refuses_ammo_but_takes_a_bag_slot() {
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        slots[0] = Some(InvSlotView {
            durability: None,
            flags: 0,
            item_id: 2512,
            equip_slots: Vec::new(),
            ..Default::default()
        });
        // An equipped bag in Bag0Slot (id 20): its equip_slots is the four bag slots.
        slots[20] = Some(InvSlotView {
            guid: 0,
            gems: [0; 3],
            duration_ms: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            flags: 0,
            item_id: 4496,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            contents_count: None,
            quality: 1,
            name: Some("Small Brown Pouch".into()),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
            locked: false,
            equip_slots: vec![20, 21, 22, 23],
            creator: None,
            enchants: Vec::new(),
        });
        s.set_inventory_slots(slots);

        // Ammo (0) is refused; Bag0Slot (20) picks up the equipped bag.
        assert!(!s.eval::<bool>("return PickupInventoryItem(0)").unwrap());
        assert!(s.eval::<bool>("return PickupInventoryItem(20)").unwrap());
        let held = s.cursor_item().expect("bag picked up from the bar");
        assert_eq!(
            (held.bag, held.slot, held.item_id),
            (EQUIPMENT_BAG, 20, 4496)
        );
    }

    #[test]
    fn place_a_bag_onto_a_bag_slot_queues_the_equip_move() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            3,
            ContainerSlot {
                guid: 0,
                gems: [0; 3],
                quest_item: false,
                start_quest: 0,
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                count: 1,
                quality: Some(1),
                item_id: 4496,
                link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
                locked: false,
                equip_slots: vec![20, 21, 22, 23], // INVTYPE_BAG → any bag slot
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        s.set_container(
            0,
            Some(ContainerState {
                bag_family: 0,
                name: Some("Backpack".into()),
                num_slots: 16,
                slots,
            }),
        );
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(21)").unwrap());

        assert!(s.eval::<bool>("return PickupInventoryItem(21)").unwrap());
        assert!(s.cursor_item().is_none(), "a plain clear on place");
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 3,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 21,
                count: None,
            }]
        );
    }

    #[test]
    fn cursor_can_go_in_slot_per_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());

        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(2)").unwrap());
        s.run("ClearCursor()").unwrap();

        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorCanGoInSlot(1)").unwrap());
    }

    #[test]
    fn auto_equip_cursor_item_queues_source_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.run("PickupContainerItem(0, 1)").unwrap();

        assert!(s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    /// The auto-equip's bind question returns before the send's clear (`0x6dfe31`), as a record still out does (`0x6dfd99`), whose landing equips the item if it is still the one held (`0x6dbbe0`); no item object moves nothing (`0x6dfd45`).
    #[test]
    fn auto_equip_cursor_item_keeps_the_cursor_through_its_question_and_its_wait() {
        use crate::script::container::SwapGate;
        let item_id = one_fitting_bag_item().slots[&1].item_id;
        let locked = |s: &UiScript| {
            s.eval::<bool>("return select(3, GetContainerItemInfo(0, 1)) == 1")
                .unwrap()
        };

        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.set_swap_gate(0, 1, SwapGate::Ask(item_id));
        s.run("PickupContainerItem(0, 1)").unwrap();
        crate::script::tick::fired::take();
        assert!(!s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(crate::script::tick::fired::names().is_empty());
        assert!(s.cursor_item().is_some() && locked(&s));
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);

        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.set_swap_gate(0, 1, SwapGate::Refuse);
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.cursor_item().is_some() && s.take_container_autoequips().is_empty());

        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.set_swap_gate(0, 1, SwapGate::Wait(item_id));
        s.run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
            .unwrap();
        assert!(s.cursor_item().is_some() && s.take_container_autoequips().is_empty());
        // Another record landing equips nothing; the held item's own does.
        s.set_swap_gate(0, 1, SwapGate::Send);
        s.item_record_callback(item_id + 1, true, crate::script::ItemRecordWait::AutoEquip);
        assert!(s.cursor_item().is_some());
        s.item_record_callback(item_id, true, crate::script::ItemRecordWait::AutoEquip);
        assert!(s.cursor_item().is_none() && locked(&s));
        assert_eq!(s.take_container_autoequips(), vec![(0, 1)]);
    }

    #[test]
    fn auto_equip_cursor_item_puts_a_worn_item_back_and_keeps_a_split_carry() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_none(), "put back");
        assert!(s.take_container_autoequips().is_empty());

        let mut state = one_fitting_bag_item();
        state.slots.get_mut(&1).unwrap().count = 5;
        s.set_container(0, Some(state));
        s.run("SplitContainerItem(0, 1, 2)").unwrap();
        assert!(!s.eval::<bool>("return AutoEquipCursorItem()").unwrap());
        assert!(s.cursor_item().is_some(), "kept");
        assert!(s.take_container_autoequips().is_empty());
    }

    #[test]
    fn use_inventory_item_queues_the_id() {
        let mut s = UiScript::new().unwrap();
        s.run("UseInventoryItem(1)").unwrap();
        s.run("UseInventoryItem(19)").unwrap();
        assert_eq!(s.take_inventory_uses(), vec![1, 19]);
        assert!(s.take_inventory_uses().is_empty(), "drained");
    }

    /// A right-click clears the cursor before it reads the slot: a held item goes back, and the
    /// worn item is still used.
    #[test]
    fn right_click_puts_a_held_item_back_then_uses_the_worn_one() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("UseInventoryItem(19)").unwrap();
        assert!(s.cursor_payload().is_none(), "the ring went back");
        assert!(
            !s.eval::<bool>("return IsInventoryItemLocked(11)").unwrap(),
            "its slot unlocks"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_inventory_uses(), vec![19]);
    }

    #[test]
    fn repair_mode_queues_an_equipped_slot_without_picking_it_up() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_merchant(Some(crate::script::MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor()").unwrap();

        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
        s.run("UseInventoryItem(1)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(s.take_inventory_repairs(), vec![1, 1, 1]);
        assert!(s.take_inventory_uses().is_empty(), "no item-use intent");
        assert!(s.take_inventory_repairs().is_empty(), "drained");
    }

    #[test]
    fn repair_mode_follows_the_doll_clicks_order() {
        use crate::script::cursor::CursorMoney;
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        if let Some(head) = slots[1].as_mut() {
            head.locked = true;
        }
        s.set_inventory_slots(slots);
        s.model_mut().repair_mode = true;

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(
            s.take_inventory_repairs().is_empty(),
            "a locked item does nothing"
        );
        let coins = CursorPayload::Money(CursorMoney { copper: 50 });
        s.model_mut().cursor = Some(coins.clone());
        s.run("PickupInventoryItem(11)").unwrap();
        assert_eq!(
            s.take_inventory_repairs(),
            vec![11],
            "coins do not block it"
        );
        assert_eq!(s.cursor_payload(), Some(coins), "and stay held");

        // A held item takes the click first: the ring goes back, nothing repairs.
        s.model_mut().cursor = None;
        s.model_mut().repair_mode = false;
        s.run("PickupInventoryItem(11)").unwrap();
        s.model_mut().repair_mode = true;
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert!(s.take_inventory_repairs().is_empty());
    }

    #[test]
    fn repair_mode_right_click_puts_a_held_item_back_then_repairs() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.run("PickupInventoryItem(11)").unwrap();
        assert!(s.cursor_item().is_some());
        s.model_mut().repair_mode = true;

        s.run("UseInventoryItem(19)").unwrap();
        assert!(s.cursor_payload().is_none(), "the ring went back");
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_inventory_repairs(), vec![19]);
        assert!(s.take_inventory_uses().is_empty(), "no use either");
    }

    #[test]
    fn is_inventory_item_locked_reads_held_or_fed() {
        let mut s = UiScript::new().unwrap();
        let mut slots = doll_slots();
        slots[19].as_mut().unwrap().locked = true; // the app's own PendingItemOps feed
        s.set_inventory_slots(slots);

        assert!(
            s.eval::<bool>("return IsInventoryItemLocked(19)").unwrap(),
            "fed lock"
        );
        assert!(!s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap());

        s.run("PickupInventoryItem(1)").unwrap();
        assert!(
            s.eval::<bool>("return IsInventoryItemLocked(1)").unwrap(),
            "held-here lock"
        );
    }

    /// `EquipCursorItem` (`0x51a3d0`): nothing held does nothing and nothing is returned; a fitting item swaps into the slot, whose item locks after the swap (`0x51a4ce`, `0x51a4db`); a misfit auto-equips with the bind question off (`0x51a51a`); the slot's own item and a misfit off a worn slot only clear (`0x51a505`); a non-number raises.
    #[test]
    fn equip_cursor_item_swaps_a_fit_and_auto_equips_a_misfit_unasked() {
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));
        assert!(s.eval::<bool>("return EquipCursorItem(1) == nil").unwrap());
        assert!(
            s.cursor_item().is_none(),
            "an empty cursor picks nothing up"
        );

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.eval::<bool>("return EquipCursorItem(1) == nil").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: EQUIPMENT_BAG,
                dst_slot: 1,
                count: None,
            }]
        );
        assert!(s
            .eval::<bool>("return IsInventoryItemLocked(1) == 1")
            .unwrap());

        // A slot the item does not fit: the auto-equip, asked nothing even when its gate would ask.
        let mut s = UiScript::new().unwrap();
        s.set_inventory_slots(doll_slots());
        s.set_container(0, Some(one_fitting_bag_item()));
        s.set_swap_gate(0, 1, crate::script::container::SwapGate::Ask(0));
        s.run("PickupContainerItem(0, 1) EquipCursorItem(5)")
            .unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty() && s.take_container_autoequips().is_empty());
        assert_eq!(s.take_container_autoequips_unasked(), vec![(0, 1)]);

        // The slot's own item, and a worn item on a slot it does not fit, only clear.
        s.run("PickupInventoryItem(1) EquipCursorItem(1)").unwrap();
        assert!(s.cursor_item().is_none());
        s.run("PickupInventoryItem(1) EquipCursorItem(5)").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
        assert!(s.take_container_autoequips_unasked().is_empty());
        assert!(s.run("EquipCursorItem('x')").is_err());
    }

    /// The held-payload test runs first in both seams, the bag's and the doll's; only the doll's click binds an armed spell's item.
    #[test]
    fn the_armed_item_half_reroutes_bag_and_doll_clicks() {
        crate::script::live::lending(&mut crate::script::live::TestView::default(), || {
            let mut s = UiScript::new().unwrap();
            s.set_inventory_slots(doll_slots());
            s.set_container(0, Some(one_fitting_bag_item()));

            // Unarmed, both seams do their ordinary gesture and queue nothing.
            assert!(s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
            assert!(s.cursor_item().is_some());
            assert!(s.take_item_picks().is_empty());

            // Armed but holding: the payload wins.
            s.set_item_pick_armed(true);
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert!(
                s.take_item_picks().is_empty(),
                "a click while carrying an item is a place, not a bind"
            );

            // Armed with an empty cursor: the doll binds and leaves the cursor empty. A fresh pair of items: the place left its two locked (`0x5d82af`, `0x6dfc0d`), and a locked item does not bind (`0x5e882c`).
            assert!(s.cursor_item().is_none());
            assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
            assert!(
                s.take_item_picks().is_empty(),
                "a locked item does not bind"
            );
            let mut s = UiScript::new().unwrap();
            s.set_inventory_slots(doll_slots());
            s.set_container(0, Some(one_fitting_bag_item()));
            s.set_item_pick_armed(true);
            assert!(!s.eval::<bool>("return PickupInventoryItem(1)").unwrap());
            assert_eq!(
                s.take_item_picks(),
                vec![(EQUIPMENT_BAG, 1)],
                "the doll's click binds its item (`0x5e884f`)"
            );
            assert!(s.cursor_item().is_none(), "a bind never stages a payload");
            // The bag's click binds nothing: it takes the item up as on any empty cursor (`0x5d8228`); the stock button routes a targeting click to `UseContainerItem` (`0x5d874c`).
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert!(s.take_item_picks().is_empty());
            assert!(s.cursor_item().is_some_and(|i| (i.bag, i.slot) == (0, 1)));
        });
    }

    /// With the host's state lent `AutoEquipCursorItem` files its question and signals `AUTOEQUIP_BIND_CONFIRM` inside the call (`0x6dfdc8`, `0x6dfe31`), the cursor still holding.
    #[test]
    fn auto_equip_cursor_item_signals_its_question_inside_the_call() {
        use crate::script::container::SwapGate;
        use crate::script::{HostAnswer, HostAsk};
        let item_id = one_fitting_bag_item().slots[&1].item_id;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_fitting_bag_item()));
        s.set_swap_gate(0, 1, SwapGate::Ask(item_id));
        s.run(
            r#"local f = CreateFrame("Frame")
               f:RegisterEvent("AUTOEQUIP_BIND_CONFIRM")
               f:SetScript("OnEvent", function(_, _, index) ASKED = { index, CursorHasItem(), SEEN } end)
               PickupContainerItem(0, 1)"#,
        )
        .unwrap();
        crate::script::tick::fired::take();
        let asked = crate::script::calls::in_call::run(
            &mut s,
            Some(HostAnswer::BindIndex(0)),
            "AutoEquipCursorItem() SEEN = true",
        );
        assert_eq!(asked, [HostAsk::BindAutoEquip { bag: 0, slot: 1 }]);
        assert_eq!(
            crate::script::tick::fired::names(),
            ["AUTOEQUIP_BIND_CONFIRM"]
        );
        assert!(s
            .eval::<bool>("return ASKED[1] == 0 and ASKED[2] == 1 and ASKED[3] == nil")
            .unwrap());
        assert!(s.cursor_item().is_some() && s.take_container_autoequips().is_empty());
    }
}
