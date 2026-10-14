//! The stock 3.3.5a bag windows (`ContainerFrame.lua`) and backpack button
//! (`MainMenuBarBagButtons.lua`) on the production load, over a backpack and one worn bag.

use benilla_ui::script::{
    ContainerSlot, ContainerState, QuestLogEntryView, QuestLogState, ScriptValue, UiScript,
};

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
