//! The stock 3.3.5a bag windows (`ContainerFrame.lua`) and backpack button
//! (`MainMenuBarBagButtons.lua`) on the production load, over a backpack and one worn bag.

use benilla_ui::script::{
    ContainerMove, ContainerSlot, ContainerState, InvSlotView, InventorySlots, ItemTemplateView,
    QuestLogEntryView, QuestLogState, ScriptValue, UiScript,
};

use super::test_ui::{centre_of, click, hover};

/// Errors from the bag code itself; the production load carries unrelated 3.3.5a gaps.
fn bag_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            ["ContainerFrame", "MainMenuBarBag", "BagSlot"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect()
}

fn item(item_id: u32, icon: &str) -> ContainerSlot {
    ContainerSlot {
        item_id,
        count: 1,
        quality: Some(1),
        texture: Some(format!("Interface\\Icons\\{icon}")),
        link: Some(format!(
            "|cffffffff|Hitem:{item_id}:0:0:0|h[Item {item_id}]|h|r"
        )),
        ..Default::default()
    }
}

/// The backpack: jerky in slot 1, a quest item in 2, a starter not taken in 3, a starter whose
/// quest is in the log in 4; and a six-slot bag with one item.
fn push_bags(s: &mut UiScript) {
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 5,
            ..item(117, "INV_Misc_Food_16")
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            quest_item: true,
            ..item(752, "INV_Misc_Bone_01")
        },
    );
    slots.insert(
        3,
        ContainerSlot {
            start_quest: 123,
            ..item(1_307, "INV_Letter_01")
        },
    );
    slots.insert(
        4,
        ContainerSlot {
            start_quest: 456,
            ..item(1_972, "INV_Letter_02")
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
            bag_family: 0,
        }),
    );
    let mut bag = std::collections::HashMap::new();
    bag.insert(1, item(2_589, "INV_Fabric_Linen_01"));
    s.set_container(
        1,
        Some(ContainerState {
            name: Some("Small Brown Pouch".into()),
            num_slots: 6,
            slots: bag,
            bag_family: 0,
        }),
    );
    s.set_quest_log(QuestLogState {
        entries: vec![QuestLogEntryView {
            quest_id: 456,
            ..Default::default()
        }],
        num_quests: 1,
        ..Default::default()
    });
}

/// The backpack button opens the backpack, which draws its icons and quest marks; the open-all
/// path opens the worn bag too; the free-slot count sums both bags; a left click picks up and a
/// right click uses, all without a script error.
#[test]
fn the_stock_backpack_opens_draws_and_takes_clicks() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("bags", false, "", |_| {});
    let _ = s.take_errors();
    push_bags(&mut s);
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);

    s.run("MainMenuBarBackpackButton:Click('LeftButton')")
        .unwrap();
    let errors = bag_errors(s.take_errors());
    assert!(errors.is_empty(), "bag errors: {errors:#?}");
    s.run(
        r#"
        assert(BACKPACK_CONTAINER == 0 and NUM_BAG_SLOTS == 4, "Constants.lua ran to its end")
        assert(IsBagOpen(0), "the backpack is open")
        assert(ContainerFrame1:IsShown() and ContainerFrame1:GetID() == 0, "in the first frame")
        local function button(slot)
            for i = 1, ContainerFrame1.size do
                local b = _G["ContainerFrame1Item" .. i]
                if b:GetID() == slot then return b, "ContainerFrame1Item" .. i end
            end
        end
        local _, name = button(1)
        local icon = _G[name .. "IconTexture"]:GetTexture()
        assert(icon and icon:find("INV_Misc_Food_16"), "slot 1 draws its icon, got " .. tostring(icon))
        assert(_G[name .. "Count"]:GetText() == "5", "and its count")
        assert(not _G[name .. "IconQuestTexture"]:IsShown(), "no quest mark on jerky")
        local function quest(slot)
            local _, n = button(slot)
            local t = _G[n .. "IconQuestTexture"]
            return t:IsShown() and t:GetTexture()
        end
        assert(quest(2) == TEXTURE_ITEM_QUEST_BORDER, "a quest item has the border, got " .. tostring(quest(2)))
        assert(quest(3) == TEXTURE_ITEM_QUEST_BANG, "a starter not taken has the bang, got " .. tostring(quest(3)))
        assert(quest(4) == TEXTURE_ITEM_QUEST_BORDER, "a starter in the log has the border")
        local free = MainMenuBarBackpackButtonCount:GetText()
        assert(free == "(17)", "12 free in the backpack and 5 in the bag, got " .. tostring(free))
        "#,
    )
    .unwrap();

    // Left click: the item goes on the cursor, which `GetCursorInfo` names.
    s.run(
        r#"
        for i = 1, ContainerFrame1.size do
            local b = _G["ContainerFrame1Item" .. i]
            if b:GetID() == 1 then b:Click("LeftButton") end
        end
        local kind, id = GetCursorInfo()
        assert(kind == "item" and id == 117, "the jerky is held, got " .. tostring(kind) .. " " .. tostring(id))
        ClearCursor()
        "#,
    )
    .unwrap();
    // Right click: the item is used.
    let _ = s.take_container_uses();
    s.run(
        r#"
        for i = 1, ContainerFrame1.size do
            local b = _G["ContainerFrame1Item" .. i]
            if b:GetID() == 1 then b:Click("RightButton") end
        end
        "#,
    )
    .unwrap();
    assert_eq!(s.take_container_uses(), vec![(0, 1)]);

    // The open-all key opens the worn bag beside the backpack, then closes both.
    s.run("CloseAllBags() OpenAllBags()").unwrap();
    assert!(s
        .eval::<bool>("return IsBagOpen(0) ~= nil and IsBagOpen(1) ~= nil")
        .unwrap());
    s.run("OpenAllBags()").unwrap();
    assert!(s
        .eval::<bool>("return IsBagOpen(0) == nil and IsBagOpen(1) == nil")
        .unwrap());

    let errors = bag_errors(s.take_errors());
    assert!(errors.is_empty(), "bag errors: {errors:#?}");
}

/// `MainMenuBarBackpackButton_UpdateFreeSlots` raises tutorial 59 at three free slots
/// (`MainMenuBarBagButtons.lua:162`).
#[test]
fn three_free_slots_trigger_the_bags_full_soon_tutorial() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("bags-full", false, "", |_| {});
    let _ = s.take_errors();
    let slots = (1..=13)
        .map(|i| (i, item(117, "INV_Misc_Food_16")))
        .collect();
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
            bag_family: 0,
        }),
    );
    let _ = s.take_tutorial_triggers();
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);
    assert_eq!(s.take_tutorial_triggers(), vec![58]);
    let errors = bag_errors(s.take_errors());
    assert!(errors.is_empty(), "bag errors: {errors:#?}");
}

/// The open backpack after the production load, over [`push_bags`].
fn open_backpack(tag: &str) -> UiScript {
    let (mut s, _load_failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    let _ = s.take_errors();
    push_bags(&mut s);
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.run("OpenBackpack()").unwrap();
    s.resolve();
    s
}

/// The `ContainerFrame1ItemN` drawing backpack `slot`: the stock frame numbers its buttons in
/// reverse (`ContainerFrame.lua`, `SetID(size - i + 1)`).
fn backpack_button(s: &UiScript, slot: u32) -> String {
    s.eval(&format!(
        "for i = 1, ContainerFrame1.size do            if _G['ContainerFrame1Item' .. i]:GetID() == {slot} then return 'ContainerFrame1Item' .. i end          end"
    ))
    .unwrap()
}

/// With no vendor open, hovering a bag stack and a worn item both end in `Sell Price:` and the
/// tooltip's money frame: `SellPrice` × stack, through `GameTooltip_OnTooltipAddMoney`.
#[test]
fn a_bag_and_a_worn_hover_show_the_sell_price_with_no_vendor_open() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = open_backpack("bag-money");
    s.set_item_template(
        117,
        ItemTemplateView {
            name: "Item 117".into(),
            quality: 1,
            sell_price: 7,
            ..Default::default()
        },
    );
    s.set_item_template(
        25,
        ItemTemplateView {
            name: "Item 25".into(),
            quality: 1,
            inventory_type: 21,
            sell_price: 9,
            ..Default::default()
        },
    );
    let mut inv: InventorySlots = Default::default();
    inv[16] = Some(InvSlotView {
        item_id: 25,
        count: 1,
        name: Some("Item 25".into()),
        quality: 1,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    let jerky = backpack_button(&s, 1);
    hover(&mut s, &jerky);
    let money = |s: &UiScript| -> (bool, String) {
        s.eval(
            "local f = GameTooltipMoneyFrame1              return f ~= nil and f:IsShown() == 1, f and GameTooltipMoneyFrame1CopperButton:GetText() or ''",
        )
        .unwrap()
    };
    assert_eq!(money(&s), (true, "35".into()), "7c each, a stack of 5");
    s.run(
        "GameTooltip:SetOwner(UIParent, 'ANCHOR_NONE') GameTooltip:SetInventoryItem('player', 16)",
    )
    .unwrap();
    assert_eq!(money(&s), (true, "9".into()), "the worn item");
    let errors = bag_errors(s.take_errors());
    assert!(errors.is_empty(), "bag errors: {errors:#?}");
}

/// Real pointer input onto an empty slot: a click picks up and a second click places, and a drag
/// does the same, each queuing the one move `PickupContainerItem` makes.
#[test]
fn a_click_place_and_a_drag_each_move_a_bag_item_into_an_empty_slot() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = open_backpack("bag-moves");
    let _ = s.take_container_moves();
    let mv = |src_slot, dst_slot| ContainerMove {
        src_bag: 0,
        src_slot,
        dst_bag: 0,
        dst_slot,
        count: None,
    };

    let (b1, b5, b6) = (
        backpack_button(&s, 1),
        backpack_button(&s, 5),
        backpack_button(&s, 6),
    );
    click(&mut s, &b1, "LeftButton");
    assert!(s.cursor_item().is_some(), "the first click picks up");
    click(&mut s, &b5, "LeftButton");
    assert!(s.cursor_item().is_none(), "the second click places");
    assert_eq!(s.take_container_moves(), vec![mv(1, 5)]);

    // The move is the server's to make; the model still holds the item at slot 1.
    let (x1, y1) = centre_of(&mut s, &b1);
    let (x6, y6) = centre_of(&mut s, &b6);
    s.mouse_move(x1, y1);
    s.mouse_button(x1, y1, "LeftButton", true);
    s.mouse_move(x1 + 8.0, y1 + 8.0);
    s.mouse_move(x6, y6);
    assert!(
        s.mouse_button(x6, y6, "LeftButton", false),
        "the drag release lands on the slot"
    );
    assert!(s.cursor_item().is_none(), "the drag places");
    assert_eq!(s.take_container_moves(), vec![mv(1, 6)]);
    let errors = bag_errors(s.take_errors());
    assert!(errors.is_empty(), "bag errors: {errors:#?}");
}
