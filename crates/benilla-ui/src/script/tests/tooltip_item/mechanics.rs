//! Render mechanics around the line order: sell price, wrapped lines, the unit health bar,
//! durability, enchant lines and random-property rolls.

use std::collections::HashMap;

use super::script;
use crate::script::*;

/// The money block (`0x62d71b`) has no merchant gate: `SetBagItem`, `SetInventoryItem` and
/// `SetHyperlink` fire `OnTooltipAddMoney` with no vendor open, an item object's price less its
/// repair cost. Repair mode and a vendor's own goods (`skipSellPrice`) fire nothing. Price 0 prints
/// `ITEM_UNSELLABLE` only at an open vendor and never for a worn slot. A template in flight
/// renders the retrieving line.
#[test]
fn bag_item_money_law_and_fallback() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 4,
            quality: Some(1),
            item_id: 2318,
            link: Some("|cffffffff|Hitem:2318|h[Light Leather]|h|r".into()),
            ..Default::default()
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 9999,
            link: Some("|cffffffff|Hitem:9999|h[Shadowforge Key]|h|r".into()),
            ..Default::default()
        },
    );
    slots.insert(
        3,
        ContainerSlot {
            count: 1,
            quality: Some(2),
            item_id: 8300,
            link: Some("|cff1eff00|Hitem:8300|h[Dented Mace]|h|r".into()),
            ..Default::default()
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
    let mut inv: InventorySlots = Default::default();
    inv[1] = Some(InvSlotView {
        item_id: 9999,
        count: 1,
        name: Some("Shadowforge Key".into()),
        quality: 1,
        ..Default::default()
    });
    inv[16] = Some(InvSlotView {
        item_id: 8300,
        count: 1,
        name: Some("Dented Mace".into()),
        quality: 2,
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    // The bag mace's repair costs more than it sells for; the worn one's costs 40 of its 100.
    let mut costs = RepairCosts::default();
    costs.bags.insert((0, 3), 250);
    costs.equipped.insert(16, 40);
    s.set_repair_costs(costs);
    s.set_item_template(
        2318,
        ItemTemplateView {
            name: "Light Leather".into(),
            quality: 1,
            sell_price: 13,
            ..Default::default()
        },
    );
    s.set_item_template(
        8300,
        ItemTemplateView {
            name: "Dented Mace".into(),
            quality: 2,
            sell_price: 100,
            ..Default::default()
        },
    );
    s.run(
        r#"
        money_fired, max_fired = nil, nil
        local a = CreateFrame("Button", "Slot3"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetScript("OnTooltipAddMoney", function(self, cost, maxcost) money_fired, max_fired = cost, maxcost end)
        function Hover(f)
            money_fired, max_fired = nil, nil
            TT:SetOwner(Slot3, "ANCHOR_RIGHT")
            f()
        end
        function LastLine() return getglobal("TTTextLeft" .. TT:NumLines()):GetText() end
        -- No merchant: the bag stack, the worn item and the link all carry money.
        Hover(function() TT:SetBagItem(0, 1) end)
        assert(money_fired == 52 and max_fired == nil, "SellPrice 13 x stack 4, got " .. tostring(money_fired))
        Hover(function() TT:SetInventoryItem("player", 16) end)
        assert(money_fired == 60, "100 less the 40 repair, got " .. tostring(money_fired))
        Hover(function() TT:SetBagItem(0, 3) end)
        assert(money_fired == 1, "a repair dearer than the price leaves 1, got " .. tostring(money_fired))
        Hover(function() TT:SetHyperlink("item:2318") end)
        assert(money_fired == 13, "a link is one of the item, got " .. tostring(money_fired))
        Hover(function() TT:SetInventoryItem("target", 16) end)
        assert(money_fired == nil, "no inspected item here")
        -- The unresolved key: the retrieving line and nothing else.
        RETRIEVING_ITEM_INFO = "Retrieving item information"
        Hover(function() TT:SetBagItem(0, 2) end)
        assert(TT:NumLines() == 1, "one line")
        assert(TTTextLeft1:GetText() == RETRIEVING_ITEM_INFO, "the retrieving line")
    "#,
    )
    .unwrap();
    assert_eq!(s.take_item_stat_asks(), vec![9999], "the miss asked");
    s.set_item_template(
        9999,
        ItemTemplateView {
            name: "Shadowforge Key".into(),
            quality: 1,
            sell_price: 0,
            ..Default::default()
        },
    );
    s.run(
        r#"
        Hover(function() TT:SetBagItem(0, 2) end)
        assert(money_fired == nil and LastLine() ~= "[ITEM_UNSELLABLE]", "price 0 and no vendor: nothing")
    "#,
    )
    .unwrap();
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            item_id: 2318,
            ..Default::default()
        }],
        can_repair: true,
        ..Default::default()
    }));
    s.run(
        r#"
        Hover(function() TT:SetBagItem(0, 2) end)
        assert(money_fired == nil, "unsellable fires no money")
        assert(LastLine() == "[ITEM_UNSELLABLE]", "ITEM_UNSELLABLE at a vendor")
        Hover(function() TT:SetInventoryItem("player", 1) end)
        assert(LastLine() ~= "[ITEM_UNSELLABLE]", "a worn slot never says unsellable")
        Hover(function() TT:SetMerchantItem(1) end)
        assert(money_fired == nil, "a vendor's own goods skip the sell price")
        Hover(function() TT:SetBagItem(0, 1) end)
        assert(money_fired == 52, "the vendor changes nothing for a priced stack")
        ShowRepairCursor()
        Hover(function() TT:SetBagItem(0, 1) end)
        assert(money_fired == nil, "repair mode: no money")
        HideRepairCursor()
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A wrap line's first measure asks with the wrap column, so one pass answers the wrapped size,
/// and the size holds while the hover clears and rebuilds the content every frame.
#[test]
fn wrap_lines_measure_wrapped_in_one_pass_and_survive_the_reenter_loop() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        7,
        ItemTemplateView {
            name: "Storybook".into(),
            quality: 1,
            charges: 1,
            description:
                "An exceedingly long tale of adventure and woe that would never fit on one line."
                    .into(),
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot4"); a:SetPoint("TOPLEFT", 10, -10); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, 7)
    "#,
    )
    .unwrap();
    s.resolve();
    // One measure pass: the wrap-flagged description asks with the wrap column, others without.
    let answer = |s: &mut UiScript| {
        let reqs = s.fontstrings_needing_measure();
        let answers: Vec<(u32, f32, f32, u64)> = reqs
            .iter()
            .map(|r| {
                if r.text.starts_with('"') {
                    assert_eq!(
                        r.wrap_width,
                        Some(crate::widget::TOOLTIP_WRAP_WIDTH),
                        "a wrap line's FIRST ask carries the wrap column"
                    );
                    (r.id, 250.0, 36.0, r.key) // wrapped: 3 rows tall, inside the column
                } else {
                    (r.id, 60.0, 14.0, r.key)
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.resolve();
    };
    answer(&mut s);
    // name 14 + gap 2 + the charges line 14 + gap 2 + description 36 + 2·pad 20 = 88.
    s.run(
        r#"
        assert(TT:GetWidth() == 270, "wrap column + padding, got " .. TT:GetWidth())
        assert(TT:GetHeight() == 88, "wrap rows counted, got " .. TT:GetHeight())
    "#,
    )
    .unwrap();
    // Clear, rebuild and measure once per frame for three frames: the size holds.
    for _ in 0..3 {
        s.run(
            r#"
            TT:SetOwner(Slot4, "ANCHOR_RIGHT")
            SetItemByIdForTest(TT, 7)
        "#,
        )
        .unwrap();
        s.resolve();
        answer(&mut s);
        s.run(
            r#"
            assert(TT:GetWidth() == 270 and TT:GetHeight() == 88,
                   "re-enter holds the wrapped size, got " .. TT:GetWidth() .. "x" .. TT:GetHeight())
        "#,
        )
        .unwrap();
    }
    assert!(s.take_errors().is_empty());
}

/// The mouseover health bar is unit content: an item render on the same tooltip hides it.
#[test]
fn item_render_hides_the_unit_health_bar() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            health: 30,
            max_health: 50,
            level: 10,
            reaction: 2,
            ..Default::default()
        }),
    );
    s.set_item_template(
        9,
        ItemTemplateView {
            name: "Plain Rock".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot6"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "GameTooltip"); tt:Hide()
        -- The stock default anchor's owner, which the world hover's show needs (`0x61cff0`).
        tt:SetScript("OnTooltipSetDefaultAnchor", function(self) self:SetOwner(Slot6, "ANCHOR_NONE") end)
        local bar = CreateFrame("StatusBar", "GameTooltipStatusBar", tt)
        bar:SetPoint("TOPLEFT", tt, "BOTTOMLEFT", 2, -1); bar:SetWidth(100); bar:SetHeight(8)
    "#,
    )
    .unwrap();
    assert!(s.world_tooltip_unit("mouseover"), "the unit hover shows");
    s.run(r#"assert(GameTooltipStatusBar:IsShown(), "unit hover shows the bar")"#)
        .unwrap();
    s.run(
        r#"
        GameTooltip:SetOwner(Slot6, "ANCHOR_RIGHT")
        SetItemByIdForTest(GameTooltip, 9)
        assert(not GameTooltipStatusBar:IsShown(), "item content hides the unit bar")
        assert(GameTooltip:IsShown(), "the item tooltip itself shows")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// A bag hover shows the instance's live `ITEM_FIELD_DURABILITY` (a death costs 10%, the spirit
/// healer 25%); a template or link hover shows max/max.
#[test]
fn real_instance_hover_renders_live_durability() {
    let mut s = script();
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        2,
        ContainerSlot {
            count: 1,
            quality: Some(2),
            item_id: 2264,
            link: Some("|cff1eff00|Hitem:2264|h[Mantle of Doan]|h|r".into()),
            durability: Some((30, 40)), // the spirit healer's 25% off a 40-max piece
            ..Default::default()
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
    s.set_item_template(
        2264,
        ItemTemplateView {
            name: "Mantle of Doan".into(),
            quality: 2,
            max_durability: 40,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 2)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 30/40]", "live pair on a bag hover, got " .. tostring(found))
        -- The template/link hover of the SAME item keeps the authored full pair.
        SetItemByIdForTest(tt, 2264)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 40/40]", "template hover stays full, got " .. tostring(found))
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Broken gear shows its true `0 / max`: a create block omits a zero `DURABILITY`, which must not
/// read as the template's max.
#[test]
fn broken_instance_hover_renders_zero_durability() {
    let mut s = script();
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 2264,
            durability: Some((0, 40)),
            ..Default::default()
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
    s.set_item_template(
        2264,
        ItemTemplateView {
            name: "Mantle of Doan".into(),
            quality: 2,
            max_durability: 40,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
        found = nil
        for i = 1, tt:NumLines() do
            local t = getglobal("TTTextLeft" .. i):GetText()
            if t and string.find(t, "DURABILITY") then found = t end
        end
        assert(found == "[DURABILITY 0/40]", "broken gear shows its true 0, got " .. tostring(found))
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // Red only when broken.
    let lines = super::lines_of(&mut s);
    let dur = lines
        .iter()
        .find(|(t, _)| t.starts_with("[DURABILITY"))
        .expect("the durability line renders");
    assert_eq!(
        dur.1,
        [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0],
        "broken (0) paints red"
    );
}

/// An instance's enchant prints from its resolved text, in green, between the resistances and
/// durability; a template hover of the same item has no instance and so no enchant line.
#[test]
fn an_enchanted_instance_renders_its_enchant_line_before_durability() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(4),
            item_id: 22816,
            durability: Some((105, 105)),
            // `ITEM_FIELD_ENCHANTMENT` slot 0 (permanent) → enchant 2564 → its name string.
            enchants: vec![EnchantView {
                slot: 0,
                name: "Agility +15".into(),
                ..Default::default()
            }],
            ..Default::default()
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
    s.set_item_template(
        22816,
        ItemTemplateView {
            name: "Hatchet of Sundered Bone".into(),
            quality: 4,
            class: 2,
            resistances: [0, 0, 7, 0, 0, 0],
            max_durability: 105,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let at = |needle: &str| {
        texts
            .iter()
            .position(|t| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no {needle} line in {texts:?}"))
    };
    assert!(
        at("[RESIST_SINGLE +7 [SCHOOL3]]") < at("Agility +15")
            && at("Agility +15") < at("[DURABILITY"),
        "the enchant line sits between resistances and durability: {texts:?}"
    );
    assert_eq!(
        lines[at("Agility +15")].1,
        [0.0, 1.0, 0.0, 1.0],
        "the enchant line is green"
    );

    s.run(r#"SetItemByIdForTest(TT, 22816)"#).unwrap();
    let lines = super::lines_of(&mut s);
    assert!(
        !lines.iter().any(|(t, _)| t.starts_with("Agility")),
        "a template hover carries no instance and so no enchant line: {lines:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Enchant colour goes by slot: slots 0 and 1 are green for a positive id and pure red (`ffff0000`,
/// not the requirement `ffff2020`) for a negative one; the property slots 7..11 are white.
#[test]
fn enchant_line_colour_is_per_slot_and_sign() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(3),
            item_id: 7777,
            enchants: vec![
                EnchantView {
                    slot: 0,
                    name: "Crusader".into(),
                    ..Default::default()
                },
                EnchantView {
                    slot: 1,
                    name: "Cursed".into(),
                    negative: true,
                    ..Default::default()
                },
                EnchantView {
                    slot: 7,
                    name: "Stamina +7".into(),
                    ..Default::default()
                },
                EnchantView {
                    slot: 8,
                    name: "Spirit +3".into(),
                    negative: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
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
    s.set_item_template(
        7777,
        ItemTemplateView {
            name: "Test Blade".into(),
            quality: 3,
            class: 2,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let color_of = |needle: &str| {
        lines
            .iter()
            .find(|(t, _)| t.starts_with(needle))
            .unwrap_or_else(|| panic!("no {needle} line in {lines:?}"))
            .1
    };
    assert_eq!(color_of("Crusader"), [0.0, 1.0, 0.0, 1.0], "slot 0, id > 0");
    assert_eq!(
        color_of("Cursed"),
        [1.0, 0.0, 0.0, 1.0],
        "slot 1, id < 0 → the pure red, NOT the requirement red"
    );
    assert_eq!(
        color_of("|cffffffffStamina +7|r"),
        [1.0, 210.0 / 255.0, 0.0, 1.0],
        "a property slot carries its own white over the gold line"
    );
    assert_eq!(
        color_of("|cffffffffSpirit +3|r"),
        [1.0, 210.0 / 255.0, 0.0, 1.0],
        "…and stays white even with a negative id"
    );
}

/// A temporary enchant prints one line, its countdown template around the name, then the charges
/// joined by `" (%s)"`; with no `SMSG_ITEM_ENCHANT_TIME_UPDATE` it is the bare name.
#[test]
fn temporary_enchant_line_carries_its_countdown_and_charges() {
    let mut s = script();
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(1),
            item_id: 7777,
            enchants: vec![
                EnchantView {
                    slot: 1,
                    name: "Rockbiter Weapon".into(),
                    remaining_ms: Some(275_000), // 4 min 35 s → the MIN arm, ceiled to 5
                    charges: 5,
                    ..Default::default()
                },
                EnchantView {
                    slot: 0,
                    name: "Crusader".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
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
    s.set_item_template(
        7777,
        ItemTemplateView {
            name: "Test Blade".into(),
            quality: 1,
            class: 2,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        texts.contains(&"[ENCH Rockbiter Weapon 5 m] ([CHARGES 5 x])"),
        "one line: name, countdown, charges — got {texts:?}"
    );
    assert!(
        texts.contains(&"Crusader"),
        "a slot with no packet keeps the bare name — got {texts:?}"
    );
}

/// A loot hover shows the roll, never the placeholder: `SetLootItem` builds an instance block
/// (p6 = 1) with the roll at `+0x424`, zeroed enchant slots and no object, so the builder copies the
/// suffix row into slots 7..11 and never reaches `ITEM_RANDOM_ENCHANT`. The name is the suffixed one
/// `GetLootSlotInfo` returns.
#[test]
fn a_looted_roll_shows_its_lines_and_never_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            // The template can roll, so the placeholder's absence below is the rule at work.
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_loot(Some(LootState {
        source_unit: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            name: Some("Bloodrazor of the Monkey".into()),
            texture: None,
            quantity: 1,
            quality: Some(2),
            is_coin: false,
            item_id: 8888,
            link: Some("|cff1eff00|Hitem:8888:0:584:0|h[Bloodrazor of the Monkey]|h|r".into()),
            // The row carries the roll id; the engine resolves it, as the reference reads `+0x424`.
            random_property_id: 584,
            locked: false,
        })],
        fishing: false,
    }));
    s.set_random_properties(
        [(
            584,
            RandomPropertyView {
                suffix: "of the Monkey".into(),
                enchants: vec![
                    EnchantView {
                        slot: 7,
                        name: "Agility +7".into(),
                        ..Default::default()
                    },
                    EnchantView {
                        slot: 8,
                        name: "Stamina +7".into(),
                        ..Default::default()
                    },
                ],
            },
        )]
        .into_iter()
        .collect(),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetLootItem(1)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"[ITEM_RANDOM_ENCHANT]"),
        "a block source never reaches the placeholder arm — got {texts:?}"
    );
    assert_eq!(
        texts.first(),
        Some(&"Bloodrazor of the Monkey"),
        "the plate takes the roll's suffixed name — got {texts:?}"
    );
    for line in ["|cffffffffAgility +7|r", "|cffffffffStamina +7|r"] {
        let l = lines
            .iter()
            .find(|(t, _)| t == line)
            .unwrap_or_else(|| panic!("the roll's slot line {line} — got {texts:?}"));
        assert_eq!(
            l.1,
            [1.0, 210.0 / 255.0, 0.0, 1.0],
            "{line} carries its white over the gold line"
        );
    }
}

/// A loot row with no roll shows neither enchant lines nor the placeholder, though the template can
/// roll: the fork tests the block's presence, not its contents.
#[test]
fn a_looted_item_with_no_roll_shows_neither_line_nor_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_loot(Some(LootState {
        source_unit: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            name: Some("Bloodrazor".into()),
            quantity: 1,
            quality: Some(2),
            item_id: 8888,
            ..Default::default()
        })],
        fishing: false,
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetLootItem(1)
    "#,
    )
    .unwrap();
    let texts: Vec<String> = super::lines_of(&mut s)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert!(
        !texts.iter().any(|t| t == "[ITEM_RANDOM_ENCHANT]"),
        "no placeholder on a block source — got {texts:?}"
    );
}

/// A chat link is a block source too (`SetHyperlink`, p6 = 1): its `|Hitem:` token 2 is the roll,
/// the builder expands it into slots 2..6, and it never reaches the placeholder.
#[test]
fn a_linked_roll_shows_its_lines_and_never_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.set_random_properties(
        [(
            584,
            RandomPropertyView {
                suffix: "of the Monkey".into(),
                enchants: vec![EnchantView {
                    slot: 7,
                    name: "Agility +7".into(),
                    ..Default::default()
                }],
            },
        )]
        .into_iter()
        .collect(),
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        tt:SetHyperlink("|cff1eff00|Hitem:8888:0:584:0|h[Bloodrazor of the Monkey]|h|r")
    "#,
    )
    .unwrap();
    let texts: Vec<String> = super::lines_of(&mut s)
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert!(
        !texts.iter().any(|t| t == "[ITEM_RANDOM_ENCHANT]"),
        "a link never reaches the placeholder arm — got {texts:?}"
    );
    assert_eq!(
        texts.first().map(String::as_str),
        Some("Bloodrazor of the Monkey"),
        "the plate takes the link's own suffixed name — got {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "|cffffffffAgility +7|r"),
        "the roll's slot line — got {texts:?}"
    );
}

/// `ITEM_RANDOM_ENCHANT` prints, green, only for a random-property item with no instance to read a
/// roll from; an instance's known roll prints its slot lines instead.
#[test]
fn random_property_template_hover_shows_the_placeholder() {
    let mut s = script();
    s.set_item_template(
        8888,
        ItemTemplateView {
            name: "Bloodrazor".into(),
            quality: 2,
            class: 2,
            random_property: 42,
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(Slot, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, 8888)
    "#,
    )
    .unwrap();
    let lines = super::lines_of(&mut s);
    let placeholder = lines
        .iter()
        .find(|(t, _)| t == "[ITEM_RANDOM_ENCHANT]")
        .expect("the template hover shows the placeholder");
    assert_eq!(placeholder.1, [0.0, 1.0, 0.0, 1.0], "green");

    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(2),
            item_id: 8888,
            enchants: vec![EnchantView {
                slot: 7,
                name: "Stamina +7".into(),
                ..Default::default()
            }],
            ..Default::default()
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
    s.run(r#"TT:SetBagItem(0, 1)"#).unwrap();
    let lines = super::lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        !texts.contains(&"[ITEM_RANDOM_ENCHANT]") && texts.contains(&"|cffffffffStamina +7|r"),
        "an instance's known roll replaces the placeholder — got {texts:?}"
    );
}
