//! The stock 3.3.5a bank window (`BankFrame.xml`) on the production load, over the state the app
//! pushes for a bank with every bag slot bought.

use std::collections::HashMap;

use benilla_ui::script::{
    BankBagSlots, BankState, ContainerSlot, ContainerState, InvSlotView, ScriptValue, UiScript,
};

/// Errors from the two windows' own code; the production load carries unrelated 3.3.5a gaps.
pub(super) fn bank_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            ["Bank", "Container", "Money", "Tab"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect()
}

pub(super) fn load(tag: &str) -> UiScript {
    let (mut s, _load_failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    let _ = s.take_errors();
    s.set_money(1_000_000);
    s.set_player_control(true);
    s
}

pub(super) fn item_slot(item_id: u32, count: u32) -> ContainerSlot {
    ContainerSlot {
        item_id,
        count,
        texture: Some(format!("Interface\\Icons\\INV_Test_{item_id}")),
        quality: Some(1),
        link: Some(format!(
            "|cffffffff|Hitem:{item_id}:0:0:0|h[Item {item_id}]|h|r"
        )),
        ..Default::default()
    }
}

pub(super) fn container(num_slots: u32, slots: &[(u32, ContainerSlot)]) -> Option<ContainerState> {
    Some(ContainerState {
        bag_family: 0,
        name: Some("Bag".into()),
        num_slots,
        slots: slots.iter().cloned().collect::<HashMap<_, _>>(),
    })
}

/// With all seven slots bought, the bank shows 28 item buttons and seven bag buttons, each bag's
/// icon at inventory slots 68-74, and the seventh bag opens with its portrait.
#[test]
fn the_stock_bank_shows_28_slots_and_seven_bags() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = load("bank7");
    s.set_container(0, container(16, &[]));
    s.set_container(-1, container(28, &[(28, item_slot(2589, 20))]));
    s.set_container(11, container(16, &[(16, item_slot(2592, 5))]));
    let mut bags: BankBagSlots = Default::default();
    for (i, bag) in bags.iter_mut().enumerate() {
        *bag = Some(InvSlotView {
            item_id: 21841,
            icon: Some(format!("Interface\\Icons\\INV_Misc_Bag_{i}")),
            count: 1,
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        });
    }
    s.set_bank_bag_slots(bags);
    s.set_bank(Some(BankState {
        num_purchased: 7,
        next_cost: 0,
    }));
    s.fire_event("BANKFRAME_OPENED", vec![]);
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "bank errors: {errors:#?}");
    s.run(
        r#"
        assert(BankFrame:IsShown(), "the bank opened")
        assert(BankFrameItem28:IsShown(), "the 28th item button")
        local tex = BankFrameItem28IconTexture:GetTexture()
        assert(tex and tex:find("INV_Test_2589"), "slot 28 paints its item, got " .. tostring(tex))
        local n, full = GetNumBankSlots()
        assert(n == 7 and full == 1, "seven bought and full")
        for i = 1, 7 do
            local b = _G["BankFrameBag" .. i]
            assert(b:GetID() == 4 + i, "BankFrameBag" .. i .. " carries container " .. (4 + i))
            local icon = _G["BankFrameBag" .. i .. "IconTexture"]:GetTexture()
            assert(icon and icon:find("INV_Misc_Bag_" .. (i - 1)), "bag " .. i .. " icon, got " .. tostring(icon))
        end
        assert(ContainerIDToInventoryID(11) == 74, "bank bag 7 is slot 74")
        assert(not pcall(ContainerIDToInventoryID, 0), "the backpack raises")
        OpenBag(11)
        local open
        for i = 1, NUM_CONTAINER_FRAMES do
            local f = _G["ContainerFrame" .. i]
            if f:IsShown() and f:GetID() == 11 then open = i end
        end
        assert(open, "bank bag 7 opened")
        local portrait = _G["ContainerFrame" .. open .. "Portrait"]:GetTexture()
        assert(portrait and portrait:find("INV_Misc_Bag_6"), "its portrait, got " .. tostring(portrait))
        "#,
    )
    .unwrap();
    // A bag placed while the bank is open repaints its button on `PLAYERBANKSLOTS_CHANGED(29)`,
    // the first bag's bank slot.
    let mut bags: BankBagSlots = Default::default();
    bags[0] = Some(InvSlotView {
        item_id: 4500,
        icon: Some("Interface\\Icons\\INV_Misc_Bag_Placed".into()),
        count: 1,
        equip_slots: vec![20, 21, 22, 23],
        ..Default::default()
    });
    s.set_bank_bag_slots(bags);
    s.fire_event("PLAYERBANKSLOTS_CHANGED", vec![ScriptValue::Int(29)]);
    assert!(s
        .eval::<String>("return BankFrameBag1IconTexture:GetTexture()")
        .unwrap()
        .contains("INV_Misc_Bag_Placed"));
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "bank errors: {errors:#?}");
}
