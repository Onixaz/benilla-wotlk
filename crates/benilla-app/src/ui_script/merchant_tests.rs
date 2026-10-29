//! The stock merchant window (`MerchantFrame.xml`), engine-only, fed a synthetic stock and purse.

use benilla_ui::script::{
    ContainerState, ExtractedQuad, ItemStatsHead, MerchantItem, MerchantState, QuadContent,
    ScriptValue, SoundRequest, UiScript,
};

use super::test_ui::{bag_open, stock_ui};

/// Put a bag of `num_slots` empty slots in bag `id`: the stock `OpenBag` opens nothing for a
/// container with no slots (`ContainerFrame.lua`).
fn equip_bag(s: &mut UiScript, id: i64, name: &str, num_slots: u32) {
    s.set_container(
        id,
        Some(ContainerState {
            bag_family: 0,
            name: Some(name.into()),
            num_slots,
            slots: std::collections::HashMap::new(),
        }),
    );
}

/// The rect of the first bare-frame quad sized `w` by `h`; every frame emits one.
fn frame_rect(quads: &[ExtractedQuad], w: f32, h: f32) -> benilla_ui::layout::Rect {
    quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Frame => q
                .rect
                .filter(|r| (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bare-frame quad sized {w}x{h}"))
}

/// Two items and a purse: `MERCHANT_SHOW` opens the window in the left UIPanel slot, the rows and
/// coins paint, a right-click buys, and `MERCHANT_CLOSED` hides it and vacates the slot.
#[test]

fn shipped_merchant_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.resolve();
    let has_icon = |quads: &[ExtractedQuad], needle: &str| {
        quads.iter().any(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
            })
    };
    assert!(
        !has_icon(&s.extract(), "INV_Drink_18"),
        "merchant window starts hidden"
    );

    s.set_money(12_345); // 1g 23s 45c
    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                price: 25,
                quantity: 1,
                num_available: -1,
                item_id: 159,
                stats: None,
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
            MerchantItem {
                name: Some("Linen Bandage".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Bandage_01".into()),
                price: 100,
                quantity: 1,
                num_available: 5,
                item_id: 1251,
                stats: None,
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return MerchantFrame:IsVisible()").unwrap());
    s.resolve();
    let quads = s.extract();
    assert!(has_icon(&quads, "INV_Drink_18"), "row 1 icon visible");
    assert!(
        has_icon(&quads, "INV_Misc_Bandage_01"),
        "row 2 icon visible"
    );
    let has_text = |t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };
    assert!(has_icon(&quads, "UI-MoneyIcons"), "coin icons render");
    assert!(has_text("25"), "row 1 price shows the copper count '25'");
    assert!(
        has_text("23") && has_text("45"),
        "purse shows its silver/copper counts"
    );

    // A filled row's socket is bright, an empty one's 0.4 (`MerchantFrame.lua`).
    let socket_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("UI-EmptySlot") => Some(color.unwrap_or([1.0; 4])),
            _ => None,
        })
        .collect();
    // Rows 11 and 12 are hidden on the merchant tab (`MerchantFrame.lua`).
    assert_eq!(
        socket_colors.len(),
        11,
        "10 merchant rows + the buyback slot render their socket plate"
    );
    // Three bright: the merchant page never tints the buyback slot (`MerchantFrame.lua`).
    assert_eq!(
        socket_colors.iter().filter(|c| c[0] > 0.9).count(),
        3,
        "the 2 filled rows and the buyback slot keep the socket full-bright"
    );
    assert_eq!(
        socket_colors
            .iter()
            .filter(|c| (c[0] - 0.4).abs() < 0.01)
            .count(),
        8,
        "the 8 empty rows dim the socket to 0.4 (ref MerchantFrame.lua l.115)"
    );
    assert_eq!(
        quads
            .iter()
            .filter(
                |q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-Merchant-LabelSlots"))
            )
            .count(),
        11,
        "10 row label plates + the buyback slot's render"
    );

    // `UpdateUIPanelPositions` seats the left panel at TOPLEFT 0, -104: top at 768 - 104.
    let win = frame_rect(&quads, 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "merchant window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // The four quadrants, each pinned to its corner, whole texture (`MerchantFrame.xml`).
    let quad_of = |needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let quad_rect = |needle: &str, w: f32, h: f32| {
        let q = quad_of(needle);
        assert!(
            matches!(
                &q.content,
                QuadContent::Texture {
                    tex_coords: None,
                    ..
                }
            ),
            "{needle} quadrant samples its whole texture (no TexCoords)"
        );
        let r = q.rect.unwrap_or_else(|| panic!("no rect for {needle}"));
        assert!(
            (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5,
            "{needle} is {w}×{h}, got {}×{}",
            r.width(),
            r.height()
        );
        r
    };
    let tl = quad_rect("UI-Merchant-TopLeft", 256.0, 256.0);
    assert_eq!(
        (tl.left, tl.top),
        (win.left, win.top),
        "TopLeft quadrant pinned to the window's TOPLEFT"
    );
    let tr = quad_rect("UI-Merchant-TopRight", 128.0, 256.0);
    assert_eq!(
        (tr.right, tr.top),
        (win.right, win.top),
        "TopRight quadrant pinned to the window's TOPRIGHT"
    );
    let bl = quad_rect("UI-Merchant-BotLeft", 256.0, 256.0);
    assert_eq!(
        (bl.left, bl.bottom),
        (win.left, win.bottom),
        "BotLeft quadrant pinned to the window's BOTTOMLEFT"
    );
    let br = quad_rect("UI-Merchant-BotRight", 128.0, 256.0);
    assert_eq!(
        (br.right, br.bottom),
        (win.right, win.bottom),
        "BotRight quadrant pinned to the window's BOTTOMRIGHT"
    );

    let tex_rect = |needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let text_rect = |t: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no text quad for {t:?}"))
    };

    let icon_rect = tex_rect("INV_Drink_18");
    assert!(
        (icon_rect.width() - 37.0).abs() < 0.5 && (icon_rect.height() - 37.0).abs() < 0.5,
        "row-left icon is a 37px square, got {}×{}",
        icon_rect.width(),
        icon_rect.height()
    );

    // The name sits above the price (`MerchantFrame.xml`); rects are y-up.
    let name_rect = text_rect("Refreshing Spring Water");
    let price_rect = text_rect("25");
    let overlap = name_rect.left < price_rect.right
        && price_rect.left < name_rect.right
        && name_rect.bottom < price_rect.top
        && price_rect.bottom < name_rect.top;
    assert!(
        !overlap,
        "name {name_rect:?} must not overlap price {price_rect:?}"
    );
    assert!(
        name_rect.bottom >= price_rect.top,
        "name {name_rect:?} sits above price {price_rect:?}"
    );

    // A right-click buys; a plain left-click is `PickupMerchantItem` (`MerchantFrame.lua`).
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert!(
        s.take_merchant_buys().is_empty(),
        "left-click does not buy (pickup pending the cursor arc)"
    );
    s.mouse_button(cx, cy, "RightButton", true);
    s.mouse_button(cx, cy, "RightButton", false);
    assert_eq!(s.take_merchant_buys(), vec![(1, 1)]);

    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.resolve();
    assert!(
        !has_icon(&s.extract(), "INV_Drink_18"),
        "MERCHANT_CLOSED hides the window"
    );
    assert!(
        s.eval::<bool>("return GetUIPanel(\"left\") == nil")
            .unwrap(),
        "HideUIPanel vacated the left slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The window's OnShow plays `igCharacterInfoOpen` and its OnHide `igCharacterInfoClose`
/// (`MerchantFrame.xml`); nothing plays at load, where the frame starts hidden.
#[test]

fn merchant_show_hide_plays_open_and_close_kits() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );

    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "opening the vendor window plays igCharacterInfoOpen"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "closing the vendor window plays igCharacterInfoClose"
    );
}

/// `MerchantFrame_OnShow` calls `OpenBackpack` before the window's own sound
/// (`MerchantFrame.lua`), so the bag's kit (`ContainerFrame.lua`) leads each pair.
#[test]

fn vendor_open_opens_the_backpack_and_layers_the_sound() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);
    let _ = s.take_sounds(); // ignore anything from load (frames are hidden; nothing should)

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igBackPackOpen".into()),
            SoundRequest::KitName("igCharacterInfoOpen".into()),
        ],
        "vendor open opens the backpack (bag kit) then plays its own panel kit"
    );
    assert!(bag_open(&s, 0), "the backpack is open alongside the vendor");

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igBackPackClose".into()),
            SoundRequest::KitName("igCharacterInfoClose".into()),
        ],
        "vendor close closes the backpack it opened, both close kits play"
    );
    assert!(!bag_open(&s, 0), "…and the window goes with it");
}

/// A bag the player already had open stays open when the vendor closes: the was-open memory, per
/// bag in `BENILLA_BAG_WAS_OPEN` as in the stock `backpackWasOpen` (`ContainerFrame.lua`).
#[test]

fn vendor_leaves_an_already_open_backpack_alone() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(bag_open(&s, 0), "fixture: the player's own bag is up first");
    let _ = s.take_sounds();
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoOpen".into())],
        "bag already open → only the panel kit on vendor open"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igCharacterInfoClose".into())],
        "the pre-opened bag is left open → only the panel close kit"
    );
    assert!(
        bag_open(&s, 0),
        "the player's own open bag survives the vendor closing"
    );
}

/// A centre panel opened over a vendor keeps a backpack the player had open before it: the stock
/// `ShowUIPanel` calls `CloseAllBags` (`UIParent.lua`), whose `CloseBackpack()` honours
/// the was-open memory (`ContainerFrame.lua`), and then closes bags 1 and up,
/// the ones the vendor opened included.
#[test]

fn a_centre_panel_over_a_vendor_keeps_the_backpack_the_player_had_open() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);
    equip_bag(&mut s, 1, "Small Pouch", 6);
    equip_bag(&mut s, 4, "Small Pouch", 6);

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(
        bag_open(&s, 0),
        "fixture: the player's own backpack is up first"
    );
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(
        bag_open(&s, 1) && bag_open(&s, 4),
        "fixture: the vendor opens the equipped bags"
    );

    // A centre-area panel, as the Help micro button's `HelpFrame` is (`UIParent.lua`).
    s.run(
        r#"local f = CreateFrame("Frame", "CentrePanelProbe", UIParent)
           f:Hide()
           UIPanelWindows["CentrePanelProbe"] = { area = "center", pushable = 0 }
           ShowUIPanel(CentrePanelProbe)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return CentrePanelProbe:IsShown() == 1")
            .unwrap(),
        "fixture: the centre panel opened"
    );
    assert!(
        bag_open(&s, 0),
        "the backpack the player had open before the vendor stays open"
    );
    assert!(
        !bag_open(&s, 1) && !bag_open(&s, 4),
        "bags 1-4 close, the vendor's included"
    );
}

/// Deviation: the vendor opens every equipped bag, not only the backpack as the stock
/// `OpenBackpack` does, so every bag is up to sell from. `ContainerFrameAdapters.xml` replaces the
/// verb after the stock file loads; with the stock body live, bag 2 would not open.
#[test]

fn vendor_opens_and_closes_all_equipped_bags() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(0);
    equip_bag(&mut s, 0, "Backpack", 16);
    equip_bag(&mut s, 2, "Small Pouch", 6);

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(bag_open(&s, 0), "backpack opens");
    assert!(bag_open(&s, 2), "the equipped bag opens with the vendor");
    assert!(
        !bag_open(&s, 1) && !bag_open(&s, 3) && !bag_open(&s, 4),
        "an unequipped slot has no container, so nothing opens for it"
    );

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !bag_open(&s, 0) && !bag_open(&s, 2),
        "every bag the vendor opened closes with it"
    );
}

/// Switching vendors is a close then an open, since `ShowUIPanel` returns early on a visible frame
/// (`UIParent.lua`). The close's OnHide queues a `CloseMerchant` the feed must consume, or
/// the drain would clear the vendor just reopened.
#[test]

fn merchant_switch_plays_close_then_open_and_queues_the_consumable_close() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    let _ = s.take_sounds();
    let _ = s.take_merchant_close();

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igCharacterInfoClose".into()),
            SoundRequest::KitName("igCharacterInfoOpen".into()),
        ],
        "switching vendors plays the close then the open kit"
    );
    assert!(
        s.take_merchant_close(),
        "the switch's MERCHANT_CLOSED queued a CloseMerchant intent — the feed must consume it so \
         the re-opened vendor is not cleared by the drain"
    );
}

/// Hovering a row's icon: the icon button's own 37px glow and a tooltip it owns at `ANCHOR_RIGHT`
/// (`MerchantItemButton_OnEnter`), "Retrieving item information" while the template is in flight
/// (`0x623760`), then the item with no buy-price line.
#[test]
fn shipped_merchant_hover_scopes_highlight_and_anchors_item_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Vendor Blade".into()),
                texture: Some("Interface\\Icons\\INV_Sword_04".into()),
                price: 8000,
                quantity: 1,
                num_available: -1,
                item_id: 2131,
                // An uncommon main-hand sword: 5.0–9.0 physical at 2600ms.
                stats: Some(ItemStatsHead {
                    quality: 2,
                    inventory_type: 21,
                    class: 2,
                    subclass: 7,
                    dmg_min: 5.0,
                    dmg_max: 9.0,
                    dmg_type: 0,
                    delay_ms: 2600,
                    armor: 0,
                    block: 0,
                    sell_price: 0,
                }),
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
            MerchantItem {
                name: Some("Chipped Buckler".into()),
                texture: Some("Interface\\Icons\\INV_Shield_09".into()),
                price: 124,
                quantity: 1,
                num_available: -1,
                item_id: 2129,
                // A common shield: armor + block, no damage.
                stats: Some(ItemStatsHead {
                    quality: 1,
                    inventory_type: 14,
                    class: 4,
                    subclass: 6,
                    dmg_min: 0.0,
                    dmg_max: 0.0,
                    dmg_type: 0,
                    delay_ms: 0,
                    armor: 85,
                    block: 1,
                    sell_price: 0,
                }),
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();

    let rect_of_tex = |quads: &[ExtractedQuad], needle: &str| {
        quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
            })
            .and_then(|q| q.rect)
            .unwrap_or_else(|| panic!("no texture quad for {needle}"))
    };
    let has_text = |quads: &[ExtractedQuad], t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };

    let quads = s.extract();
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("ButtonHilight-Square"))
        }),
        "no highlight before hover"
    );
    assert!(!has_text(&quads, "Main Hand"), "no tooltip before hover");
    let sword_icon = rect_of_tex(&quads, "INV_Sword_04");

    // Hover the icon: a row is an inert container plus an icon-sized `$parentItemButton` that
    // takes the mouse (`MerchantFrame.xml`), so hovering the name shows nothing.
    let (hx, hy) = (
        (sword_icon.left + sword_icon.right) * 0.5,
        (sword_icon.bottom + sword_icon.top) * 0.5,
    );
    s.mouse_move(hx, hy);
    assert!(s.errors().is_empty(), "OnEnter errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let hl = rect_of_tex(&quads, "ButtonHilight-Square");
    assert!(
        (hl.width() - 37.0).abs() < 0.5 && (hl.height() - 37.0).abs() < 0.5,
        "highlight is the 37px icon square, got {}×{}",
        hl.width(),
        hl.height()
    );
    assert_eq!(
        (hl.left, hl.top),
        (sword_icon.left, sword_icon.top),
        "highlight sits on the icon, not the row"
    );

    assert!(
        has_text(&quads, "Retrieving item information"),
        "the template is in flight"
    );
    // The templates land, and the hover re-enters.
    for (id, view) in [
        (
            2131,
            benilla_ui::script::ItemTemplateView {
                name: "Vendor Blade".into(),
                quality: 2,
                class: 2,
                subclass: 7,
                inventory_type: 21,
                damages: vec![(5.0, 9.0, 0)],
                delay_ms: 2600,
                ..Default::default()
            },
        ),
        (
            2129,
            benilla_ui::script::ItemTemplateView {
                name: "Chipped Buckler".into(),
                quality: 1,
                class: 4,
                subclass: 6,
                inventory_type: 14,
                armor: 85,
                block: 1,
                ..Default::default()
            },
        ),
    ] {
        s.set_item_template(id, view);
    }
    s.mouse_move(600.0, 400.0);
    s.mouse_move(hx, hy);
    s.resolve();
    let quads = s.extract();
    for line in [
        "Vendor Blade",
        "Main Hand",
        "5 - 9 Damage",
        "Speed 2.60",
        "(2.7 damage per second)",
    ] {
        assert!(has_text(&quads, line), "tooltip line {line:?} missing");
    }
    // Two "Vendor Blade" quads: the row's gold name and the tooltip's uncommon-green header.
    let name_colors: Vec<[f32; 4]> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if t == "Vendor Blade" => Some(*c),
            _ => None,
        })
        .collect();
    assert!(
        name_colors
            .iter()
            .any(|c| (c[0] - 0.12).abs() < 0.01 && (c[1] - 1.0).abs() < 0.01 && c[2].abs() < 0.01),
        "the tooltip name line is quality-green, got {name_colors:?}"
    );
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), .. }
                if t.starts_with("Buy Price"))
        }),
        "the tooltip carries no buy-price line"
    );

    // `ANCHOR_RIGHT`: the tooltip's BOTTOMLEFT on the owner's TOPRIGHT, the owner the icon button.
    let (tip_left, tip_bottom): (f32, f32) = s
        .eval("return GameTooltip:GetLeft(), GameTooltip:GetBottom()")
        .unwrap();
    assert_eq!(
        (tip_left, tip_bottom),
        (sword_icon.right, sword_icon.top),
        "tooltip BOTTOMLEFT sits on the icon's TOPRIGHT"
    );

    let shield_icon = rect_of_tex(&quads, "INV_Shield_09");
    s.mouse_move(
        (shield_icon.left + shield_icon.right) * 0.5,
        (shield_icon.bottom + shield_icon.top) * 0.5,
    );
    assert!(
        s.errors().is_empty(),
        "row-2 OnEnter errors: {:?}",
        s.errors()
    );
    s.resolve();
    let quads = s.extract();
    for line in ["Chipped Buckler", "Off Hand", "85 Armor", "1 Block"] {
        assert!(
            has_text(&quads, line),
            "shield tooltip line {line:?} missing"
        );
    }
    assert!(!has_text(&quads, "Main Hand"), "row 1's tooltip cleared");

    s.mouse_move(600.0, 400.0);
    s.resolve();
    let quads = s.extract();
    assert!(!has_text(&quads, "Off Hand"), "tooltip hidden on leave");
    assert!(
        !quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("ButtonHilight-Square"))
        }),
        "highlight gone on leave"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The two tabs (`MerchantFrame.lua`): the merchant page has the buyback slot and repair
/// pair; the buyback page retitles the window and fills the rows from `GetBuybackItemInfo`.
#[test]

fn merchant_tabs_drive_buyback_page_and_repair_pair() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(500);

    let buyback_item = |name: &str, price: u32| MerchantItem {
        name: Some(name.into()),
        texture: Some("Interface\\Icons\\INV_Misc_Cape_01".into()),
        price,
        quantity: 1,
        stats: Some(ItemStatsHead {
            quality: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            link: None,
            max_stack: Some(1),
            extended_cost: None,
        }],
        buyback: vec![
            buyback_item("Bandit Cloak", 116),
            buyback_item("Cracked Sword", 20), // the latest sale: the merchant page's slot
        ],
        can_repair: true,
    }));
    s.set_repair_all_cost(76, true);
    s.fire_event(
        "MERCHANT_SHOW",
        vec![ScriptValue::Str("Kurdram Stonehammer".into())],
    );
    assert!(s.errors().is_empty(), "show errors: {:?}", s.errors());

    // Merchant page: the latest sale in the buyback slot (`MerchantFrame.lua`), repair up.
    assert!(s
        .eval::<bool>("return MerchantBuyBackItemName:GetText() == 'Cracked Sword'")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantRepairAllButton:IsShown() and \
             MerchantRepairAllButton:IsEnabled() ~= 0"
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return not MerchantItem11:IsShown() and BuybackFrameTopLeft:IsShown() == nil"
        )
        .unwrap());

    // The slot's item button buys back the latest sale, slot 2 of 2.
    s.run("MerchantBuyBackItemItemButton:Click()").unwrap();
    assert_eq!(s.take_merchant_buybacks(), vec![2]);

    s.run("MerchantRepairAllButton:Click()").unwrap();
    assert_eq!(s.take_repair_all(), Some(false));
    s.take_sounds();

    s.run("MerchantFrameTab2:Click()").unwrap();
    assert!(s.errors().is_empty(), "tab errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return MerchantNameText:GetText() == 'Merchant Buyback'")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantItem1Name:GetText() == 'Bandit Cloak' \
             and MerchantItem2Name:GetText() == 'Cracked Sword'",
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return BuybackFrameTopLeft:IsShown() == 1 \
             and not MerchantBuyBackItem:IsShown() \
             and not MerchantRepairAllButton:IsShown()",
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "return MerchantFrameTab2LeftDisabled:IsShown() == 1 \
             and MerchantFrameTab1LeftDisabled:IsShown() == nil",
        )
        .unwrap());

    // A buyback-page row click is `BuybackItem` (`MerchantFrame.lua`).
    s.run("MerchantItem1ItemButton:Click(\"LeftButton\")")
        .unwrap();
    assert_eq!(s.take_merchant_buybacks(), vec![1]);

    s.run("MerchantFrameTab1:Click()").unwrap();
    assert!(s
        .eval::<bool>(
            "return MerchantItem1Name:GetText() == 'Refreshing Spring Water' \
             and BuybackFrameTopLeft:IsShown() == nil",
        )
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The tabs fit their labels once, in `<OnShow>` (`CharacterFrameTemplates.xml`), so the
/// measurer must answer synchronously, as the app's does.
#[test]

fn merchant_tabs_fit_their_labels() {
    benilla_formats::wow_data_or_skip!();
    /// `2 * $parentLeft:GetWidth()` (`UIPanelTemplates.lua`): the two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.set_money(0);
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![ScriptValue::Str("Vendor".into())]);
    s.resolve();

    let (l1, l2, w1, w2, m1, m2): (f64, f64, f64, f64, f64, f64) = s
        .eval(
            "return MerchantFrameTab1Text:GetStringWidth(), MerchantFrameTab2Text:GetStringWidth(), \
             MerchantFrameTab1:GetWidth(), MerchantFrameTab2:GetWidth(), \
             MerchantFrameTab1MiddleDisabled:GetWidth(), MerchantFrameTab2Middle:GetWidth()",
        )
        .unwrap();
    assert!(l1 > 0.0 && l2 > 0.0, "both labels measured synchronously");
    assert_eq!(
        (w1, w2),
        (l1 + SIDES, l2 + SIDES),
        "text + 40, not the fixed 115"
    );
    assert_eq!((m1, m2), (l1, l2), "middle slices stretch to the text");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A hovered row arms the Buy cursor (UnableBuy when unaffordable, Inspect with ctrl held), which
/// the frame's OnUpdate re-arms every frame (`MerchantFrame.xml`); leaving clears it.
#[test]

fn shipped_merchant_frame_arms_the_buy_cursor_on_hover() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::UiCursorMode;

    let mut s = stock_ui(1024.0, 768.0);

    // A purse of 50c: row 1 (25c) is affordable, row 2 (100c) is not.
    s.set_money(50);
    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                price: 25,
                quantity: 1,
                num_available: -1,
                item_id: 159,
                stats: None,
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
            MerchantItem {
                name: Some("Linen Bandage".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Bandage_01".into()),
                price: 100,
                quantity: 1,
                num_available: 5,
                item_id: 1251,
                stats: None,
                link: None,
                max_stack: Some(1),
                extended_cost: None,
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    s.resolve();
    assert_eq!(s.ui_cursor(), None, "no override before any hover");

    // The row button's inline `<OnEnter>`, fired as the engine fires it: `this` bound first.
    s.run("MerchantItem1ItemButton:GetScript(\"OnEnter\")(MerchantItem1ItemButton)")
        .unwrap();
    s.tick(0.016);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Buy),
        "the coin arms over an affordable vendor item"
    );

    s.set_modifiers(false, true, false);
    s.tick(0.016);
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Inspect),
        "Ctrl-hover shows the inspect cursor"
    );
    s.set_modifiers(false, false, false);

    // OnLeave resets the cursor and clears `itemHover` (`MerchantFrame.xml`).
    s.run("MerchantItem1ItemButton:GetScript(\"OnLeave\")(MerchantItem1ItemButton)")
        .unwrap();
    s.tick(0.016);
    assert_eq!(s.ui_cursor(), None, "leaving the row resets the cursor");

    s.run("MerchantItem2ItemButton:GetScript(\"OnEnter\")(MerchantItem2ItemButton)")
        .unwrap();
    s.tick(0.016);
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::UnableBuy),
        "an unaffordable vendor item grays the coin"
    );

    // Closing resets it with no OnLeave, from `MerchantFrame_OnHide` (`MerchantFrame.lua`).
    s.fire_event("MERCHANT_CLOSED", vec![]);
    s.tick(0.016);
    assert_eq!(s.ui_cursor(), None, "closing the window resets the cursor");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The trade partner's money (`TradeRecipientMoneyFrame`) shows a single digit, not "...".
#[test]

fn trade_recipient_money_renders_the_digit_not_ellipsis() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    let target = benilla_ui::script::TradeSideState {
        gold: 5, // a partner offering 5 copper → the recipient trio should show "5"
        ..Default::default()
    };
    s.set_trade(Some(benilla_ui::script::TradeState {
        target,
        ..Default::default()
    }));
    s.fire_event("TRADE_SHOW", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
    let quads = s.extract();

    let has = |t: &str| {
        quads
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };
    let texts: Vec<&str> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text { text: Some(x), .. } => Some(x.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !has("..."),
        "recipient money truncated to '...' (the bug). all text quads: {texts:?}"
    );
    assert!(
        has("5"),
        "recipient money shows the digit. all text quads: {texts:?}"
    );
}

/// Any modified click on a row goes to `MerchantItemButton_OnModifiedClick` (`MerchantFrame.xml`),
/// whose `HandleModifiedItemClick` posts the link on shift and dresses up a dressable item on ctrl;
/// neither buys, and only an unmodified right-click does.
#[test]
fn ctrl_and_shift_on_a_vendor_row_take_the_modified_click_and_never_buy() {
    benilla_formats::wow_data_or_skip!();
    const WATER_LINK: &str = "|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r";
    let mut s = stock_ui(1024.0, 768.0);

    s.set_money(12_345);
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            // As `ui_merchant` builds it from the row's item template.
            link: Some(WATER_LINK.into()),
            max_stack: Some(1),
            extended_cost: None,
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let icon = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Drink_18"))
        })
        .and_then(|q| q.rect)
        .expect("no row icon quad");
    let (x, y) = (
        (icon.left + icon.right) * 0.5,
        (icon.bottom + icon.top) * 0.5,
    );

    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    assert_eq!(
        s.take_merchant_buys(),
        vec![(1, 1)],
        "an unmodified right-click still buys"
    );

    s.set_modifiers(false, true, false);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.take_merchant_buys().is_empty(),
        "a ctrl-held right-click must not buy (ref l.332-333)"
    );

    // The ctrl arm is `HandleModifiedItemClick`'s DRESSUP leg, and water is not dressable.
    assert!(
        s.take_dressup_intents().is_empty(),
        "an undressable item opens no dressing room"
    );

    s.run("ChatEdit_ActivateChat(ChatFrame1EditBox)").unwrap();
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrame1EditBox:GetText()")
            .unwrap(),
        format!(" {WATER_LINK}"),
        "the vendor row's full escaped link landed in the chat box, after `ChatEdit_InsertLink`'s space"
    );
    assert!(
        s.take_merchant_buys().is_empty(),
        "a shift-click must not also buy"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Shift is `SPLITSTACK`: `MerchantItemButton_OnModifiedClick` opens the split on a single row whose
/// item stacks, capped by what the purse buys, and the split buys that many; a bundle row or one
/// that does not stack opens nothing, and the modified click never buys.
#[test]
fn shift_click_opens_the_split_on_a_stacking_row() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_money(1_000);
    s.set_merchant(Some(MerchantState {
        items: vec![
            MerchantItem {
                name: Some("Refreshing Spring Water".into()),
                price: 25,
                quantity: 5,
                num_available: -1,
                item_id: 159,
                max_stack: Some(20),
                ..Default::default()
            },
            MerchantItem {
                name: Some("Flask of Oil".into()),
                price: 100,
                quantity: 1,
                num_available: -1,
                item_id: 814,
                max_stack: Some(20),
                ..Default::default()
            },
            MerchantItem {
                name: Some("Simple Dagger".into()),
                price: 40,
                quantity: 1,
                num_available: -1,
                item_id: 2139,
                max_stack: Some(1),
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let shift_click = |s: &mut UiScript, row: u32, button: &str| {
        let (x, y) = super::test_ui::centre_of(s, &format!("MerchantItem{row}ItemButton"));
        s.set_modifiers(true, false, false);
        s.mouse_button(x, y, button, true);
        s.mouse_button(x, y, button, false);
        s.set_modifiers(false, false, false);
    };
    let split = |s: &UiScript| {
        s.eval::<(bool, Option<i64>)>(
            "return StackSplitFrame:IsShown() and true or false, StackSplitFrame.maxStack",
        )
        .unwrap()
    };

    // `GetMerchantItemMaxStack` (`0x5842d0`) answers 1 for a row sold in bundles.
    for button in ["LeftButton", "RightButton"] {
        shift_click(&mut s, 1, button);
        assert!(!split(&s).0, "a bundle row opens no split");
        assert!(s.cursor_payload().is_none(), "picks nothing up");
        assert!(s.take_merchant_buys().is_empty(), "and buys nothing");
    }

    shift_click(&mut s, 2, "RightButton");
    assert_eq!(
        split(&s),
        (true, Some(10)),
        "1000 copper buys ten 100 c flasks, under the stack of 20"
    );
    s.run("StackSplitFrame.owner.SplitStack(StackSplitFrame.owner, 3)")
        .unwrap();
    assert_eq!(s.take_merchant_buys(), vec![(2, 3)], "the split buys three");
    s.run("StackSplitFrame:Hide()").unwrap();

    shift_click(&mut s, 3, "LeftButton");
    assert!(!split(&s).0, "a row that does not stack opens no split");
    assert!(s.take_merchant_buys().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
