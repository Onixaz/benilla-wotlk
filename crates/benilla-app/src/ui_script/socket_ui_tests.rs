//! The stock 3.3.5a socketing window (`Blizzard_ItemSocketingUI`) on the production load, over a
//! backpack holding a two-socket item (red and blue), a red gem and a meta gem.

use std::collections::HashMap;

use benilla_ui::script::{ContainerSlot, ContainerState, GemView, ItemTemplateView, UiScript};

const ARMOR: u64 = 0x4000_0000_0000_0001;
const RED_GEM: u64 = 0x4000_0000_0000_0002;
const META_GEM: u64 = 0x4000_0000_0000_0003;
const PLAIN: u64 = 0x4000_0000_0000_0004;

/// Errors from the socketing code; the production load carries unrelated 3.3.5a gaps.
fn socket_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Socket", "socket"].iter().any(|k| e.contains(k)))
        .collect()
}

fn slot(guid: u64, item_id: u32, name: &str) -> ContainerSlot {
    ContainerSlot {
        guid,
        item_id,
        count: 1,
        quality: Some(3),
        texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
        link: Some(format!("|cff0070dd|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

fn gem(name: &str, gem_type: u32, enchant: u32, enchant_name: &str) -> ItemTemplateView {
    ItemTemplateView {
        name: name.into(),
        quality: 3,
        icon: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
        gem_properties: enchant,
        gem: Some(GemView {
            gem_type,
            enchant,
            enchant_name: Some(enchant_name.into()),
            condition: None,
        }),
        ..Default::default()
    }
}

fn push_items(s: &mut UiScript) {
    s.set_item_template(
        1000,
        ItemTemplateView {
            name: "Socketed Chestguard".into(),
            quality: 3,
            inventory_type: 5,
            icon: Some("Interface\\Icons\\INV_Chest_Plate01".into()),
            socket_colors: [2, 8, 0],
            socket_bonus: 3301,
            socket_bonus_name: Some("+6 Stamina".into()),
            ..Default::default()
        },
    );
    s.set_item_template(2000, gem("Bold Gem", 2, 3000, "+8 Strength"));
    s.set_item_template(2001, gem("Meta Gem", 1, 3001, "+21 Critical Strike"));
    s.set_item_template(
        2002,
        ItemTemplateView {
            name: "Plain Shirt".into(),
            inventory_type: 4,
            ..Default::default()
        },
    );
    let slots = HashMap::from([
        (1, slot(ARMOR, 1000, "Socketed Chestguard")),
        (2, slot(RED_GEM, 2000, "Bold Gem")),
        (3, slot(META_GEM, 2001, "Meta Gem")),
        (4, slot(PLAIN, 2002, "Plain Shirt")),
    ]);
    s.set_container(
        0,
        Some(ContainerState {
            num_slots: 16,
            slots,
            ..Default::default()
        }),
    );
}

fn description(s: &mut UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {} for i = 1, ItemSocketingDescription:NumLines() do \
         t[i] = _G['ItemSocketingDescriptionTextLeft' .. i]:GetText() end return t",
    )
    .unwrap()
}

/// `SocketContainerItem` opens the stock window over a socketed item and locks it; a gem moves from
/// the cursor into its socket, a meta gem is refused in a coloured one, the description previews
/// the placed gem, and the Socket button sends the item and its gems.
#[test]
fn the_socketing_window_places_a_gem_and_accepts_it() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("socketing", false, "", |_| {});
    let _ = s.take_errors();
    push_items(&mut s);

    // A socketless item opens nothing and raises nothing.
    s.run("SocketContainerItem(0, 4)").unwrap();
    s.tick(0.0);
    assert_eq!(s.socket_item(), 0);

    s.run("SocketContainerItem(0, 1)").unwrap();
    s.tick(0.0);
    let errors = socket_errors(s.take_errors());
    assert!(errors.is_empty(), "socketing window errors: {errors:#?}");
    s.run(
        r#"
        assert(IsAddOnLoaded("Blizzard_ItemSocketingUI"), "the socketing addon loaded")
        assert(ItemSocketingFrame:IsShown(), "the window is up")
        assert(GetNumSockets() == 2)
        assert(GetSocketTypes(1) == "Red" and GetSocketTypes(2) == "Blue")
        assert(ItemSocketingSocket1:IsShown() and ItemSocketingSocket2:IsShown())
        assert(not ItemSocketingSocket3:IsShown(), "the third socket hides")
        assert(select(3, GetContainerItemInfo(0, 1)), "the socketed item is locked")
        assert(GetSocketItemInfo() == "Socketed Chestguard")
        assert(ItemSocketingSocketButton:IsEnabled() == 0, "nothing to socket yet")
        PickupContainerItem(0, 1)
        assert(not CursorHasItem(), "a locked item does not pick up")
        "#,
    )
    .unwrap();

    // The red gem from the bag into the red socket.
    s.run("PickupContainerItem(0, 2) ClickSocketButton(1)")
        .unwrap();
    s.tick(0.0);
    s.run(
        r#"
        local name, _, matches = GetNewSocketInfo(1)
        assert(name == "Bold Gem", tostring(name))
        assert(matches == 1, "a red gem fits a red socket")
        assert(not CursorHasItem(), "the cursor let go of the gem")
        assert(ItemSocketingSocketButton:IsEnabled() == 1, "a gem is placed")
        assert(GetNewSocketLink(1):find("Bold Gem"))
        "#,
    )
    .unwrap();
    let lines = description(&mut s);
    assert!(
        lines.iter().any(|l| l == "|cffffffff+8 Strength|r"),
        "the preview shows the placed gem: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("Blue Socket")),
        "the preview prints no empty socket: {lines:?}"
    );

    // A meta gem is refused in the blue socket and stays on the cursor.
    s.run("PickupContainerItem(0, 3) ClickSocketButton(2)")
        .unwrap();
    assert_eq!(
        s.take_ui_errors(),
        ["ERR_SOCKETING_META_GEM_ONLY_IN_METASLOT"]
    );
    s.run(
        r#"
        assert(GetNewSocketInfo(2) == nil, "the meta gem was refused")
        assert(CursorHasItem(), "and is still held")
        ClearCursor()
        "#,
    )
    .unwrap();

    s.run("ItemSocketingSocketButton:Click()").unwrap();
    assert_eq!(s.take_socket_sends(), [(ARMOR, [RED_GEM, 0, 0])]);

    // Closing the window unlocks the item.
    s.run("HideUIPanel(ItemSocketingFrame)").unwrap();
    s.tick(0.0);
    assert_eq!(s.socket_item(), 0);
    s.run("assert(not select(3, GetContainerItemInfo(0, 1)), 'unlocked')")
        .unwrap();
    let errors = socket_errors(s.take_errors());
    assert!(errors.is_empty(), "socketing window errors: {errors:#?}");
}

/// The socketed item's enchants changing re-reads the window: the placed gem's object going empties
/// its socket, and the new socket enchant shows as the existing gem.
#[test]
fn a_socket_result_shows_the_gem_as_existing() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("socketing", false, "", |_| {});
    let _ = s.take_errors();
    push_items(&mut s);
    s.run("SocketContainerItem(0, 1) PickupContainerItem(0, 2) ClickSocketButton(1)")
        .unwrap();
    s.tick(0.0);
    s.run("AcceptSockets()").unwrap();
    assert_eq!(s.take_socket_sends().len(), 1);

    // The server destroys the gem and fills socket 1's enchant.
    let mut bag = HashMap::from([(1, slot(ARMOR, 1000, "Socketed Chestguard"))]);
    let armor = bag.get_mut(&1).unwrap();
    armor.gems = [2000, 0, 0];
    armor.enchants = vec![benilla_ui::script::EnchantView {
        slot: 2,
        id: 3000,
        gem_type: Some(2),
        name: "+8 Strength".into(),
        ..Default::default()
    }];
    s.set_container(
        0,
        Some(ContainerState {
            num_slots: 16,
            slots: bag,
            ..Default::default()
        }),
    );
    s.tick(0.0);
    s.run(
        r#"
        assert(GetNewSocketInfo(1) == nil, "the gem's object went")
        local name, _, matches = GetExistingSocketInfo(1)
        assert(name == "Bold Gem", tostring(name))
        assert(matches == 1)
        assert(GetExistingSocketLink(1):find("Bold Gem"))
        assert(ItemSocketingSocketButton:IsEnabled() == 0, "nothing new to socket")
        "#,
    )
    .unwrap();
    let errors = socket_errors(s.take_errors());
    assert!(errors.is_empty(), "socketing window errors: {errors:#?}");
}

/// A purchase sold back to the open vendor inside its window raises the stock refund confirm over
/// the record's honor cost, and its Yes sends the refund.
#[test]
fn a_refundable_purchase_raises_the_refund_confirm() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("refund", false, "", |_| {});
    let _ = s.take_errors();
    const CAPE: u64 = 0x4000_0000_0000_0010;
    s.set_item_template(
        16342,
        ItemTemplateView {
            name: "Sergeant's Cape".into(),
            quality: 3,
            inventory_type: 16,
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            num_slots: 16,
            slots: HashMap::from([(2, slot(CAPE, 16342, "Sergeant's Cape"))]),
            ..Default::default()
        }),
    );
    s.set_played_time(100_000);
    s.set_refund_record(
        CAPE,
        Some(benilla_ui::script::RefundView {
            honor: 1600,
            played_at_purchase: 99_900,
            ..Default::default()
        }),
    );
    s.set_merchant(Some(benilla_ui::script::MerchantState::default()));
    s.run("MerchantFrame:Show()").unwrap();
    let _ = s.take_errors();
    s.run(
        r#"
        local fake = { GetID = function() return 2 end,
            GetParent = function() return { GetID = function() return 0 end } end }
        assert(ContainerFrame_GetExtendedPriceString(fake), "the confirm shows")
        local name = StaticPopup_Visible("CONFIRM_REFUND_TOKEN_ITEM")
        assert(name, "the refund confirm is up")
        _G[name .. "Button1"]:Click()
        "#,
    )
    .unwrap();
    assert_eq!(s.take_refund_sends(), [CAPE]);
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| {
            ["StaticPopup", "ContainerFrame", "Merchant"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect();
    assert!(errors.is_empty(), "refund confirm errors: {errors:#?}");
}
