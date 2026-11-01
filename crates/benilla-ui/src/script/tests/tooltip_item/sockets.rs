//! The 3.3.5a stat and socket lines (`0x6277f0`): the green rating stats under the requirements,
//! the empty sockets and the socket bonus, and the druid's feral attack power.

use std::collections::HashMap;

use super::{lines_of, script};
use crate::script::*;

const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// A head piece with a red and a blue socket and a socket bonus, as `Valorous Cryptstalker Tunic`.
fn socketed() -> ItemTemplateView {
    ItemTemplateView {
        name: "Socketed Helm".into(),
        quality: 4,
        class: 4,
        inventory_type: 1,
        stats: vec![(7, 54), (32, 43), (40, 9), (36, 0), (31, 20)],
        max_durability: 60,
        required_level: 80,
        spell_triggers: vec![(1, 28686, "Increases spell power by 87.".into())],
        socket_colors: [2, 8, 0],
        socket_bonus: 3301,
        socket_bonus_name: Some("+6 Stamina".into()),
        ..Default::default()
    }
}

/// The bag's slot 1 holds item 4000 with `enchants`.
fn bag_item(s: &mut UiScript, enchants: Vec<EnchantView>) {
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            quality: Some(4),
            item_id: 4000,
            enchants,
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
}

fn hover_bag(s: &mut UiScript) -> Vec<(String, [f32; 4])> {
    s.run(
        r#"
        if not Slot then
            local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
            a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT")
        end
        TT:SetOwner(Slot, "ANCHOR_RIGHT"); TT:SetBagItem(0, 1)
    "#,
    )
    .unwrap();
    lines_of(s)
}

fn gem(slot: u8, name: &str, gem_type: u32) -> EnchantView {
    EnchantView {
        slot,
        id: 3_000 + u32::from(slot),
        gem_type: Some(gem_type),
        name: name.into(),
        ..Default::default()
    }
}

/// A meta gem whose condition the worn gems miss grays its line and appends the condition line,
/// gray too; with two red gems worn both turn white (`0x61c0b0`, `0x629ce5`).
#[test]
fn a_meta_gem_reads_its_condition_against_the_worn_gems() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    let mut meta = gem(2, "+21 Critical Strike Rating", 1);
    // "at least 2 red gems": left red (2), op 5 (>=), count 2, no further test.
    meta.condition = Some(
        [EnchantConditionTest {
            left_type: 2,
            operator: 5,
            right_type: 0,
            right_value: 2,
            logic: 0,
        }; 5],
    );
    bag_item(&mut s, vec![meta]);
    let line = |s: &mut UiScript| {
        hover_bag(s)
            .into_iter()
            .find(|(t, _)| t.contains("+21 Critical Strike Rating"))
            .expect("the meta gem's line")
            .0
    };
    let unmet = line(&mut s);
    assert!(
        unmet.starts_with(
            "|cff808080+21 Critical Strike Rating|r
  |cff808080"
        ),
        "{unmet}"
    );

    // Two red gems worn in the head.
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(InvSlotView {
        item_id: 9,
        enchants: vec![gem(2, "+8 Strength", 2), gem(3, "+8 Strength", 2)],
        ..Default::default()
    });
    s.set_inventory_slots(slots);
    let met = line(&mut s);
    assert!(
        met.starts_with(
            "|cffffffff+21 Critical Strike Rating|r
  |cffffffff"
        ),
        "{met}"
    );
}

/// Each rating stat is a green ON_EQUIP line in wire order, after the requirements and before the
/// spell triggers; the primary stats stay white above, and stat 40 and a zero value print nothing.
#[test]
fn rating_stats_are_green_equip_lines_under_the_requirements() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT"):SetOwner(Slot, "ANCHOR_RIGHT")
        SetItemByIdForTest(TT, 4000)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let at = |needle: &str| {
        texts
            .iter()
            .position(|t| *t == needle)
            .unwrap_or_else(|| panic!("no {needle:?} in {texts:?}"))
    };
    let crit = at("[ONEQUIP] [MOD_CRIT_RATING 43]");
    let hit = at("[ONEQUIP] [MOD_HIT_RATING 20]");
    assert!(at("[MOD_STAMINA +54]") < at("[MIN_LEVEL 80]"));
    assert!(at("[MIN_LEVEL 80]") < crit, "under the requirements");
    assert!(crit < hit, "in wire order");
    assert!(
        hit < at("[ONEQUIP] Increases spell power by 87."),
        "above the triggers"
    );
    assert_eq!(lines[crit].1, GREEN);
    assert_eq!(
        texts.iter().filter(|t| t.starts_with("[ONEQUIP]")).count(),
        3,
        "stat 40 has no key and a zero value prints nothing: {texts:?}"
    );
    // A `%c` template takes the sign.
    let mut shield = socketed();
    shield.stats = vec![(48, 25)];
    s.set_item_template(4001, shield);
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT'); SetItemByIdForTest(TT, 4001)")
        .unwrap();
    assert!(lines_of(&mut s)
        .iter()
        .any(|(t, _)| t == "[ONEQUIP] [MOD_BLOCK_VALUE +25]"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A template with no instance prints each coloured socket and the bonus gray, between the stats
/// and the durability.
#[test]
fn a_bare_template_lists_its_sockets_and_a_gray_bonus() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT"):SetOwner(Slot, "ANCHOR_RIGHT")
        SetItemByIdForTest(TT, 4000)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let red = texts
        .iter()
        .position(|t| *t == "[EMPTY_SOCKET_RED]")
        .unwrap();
    assert_eq!(texts[red + 1], "[EMPTY_SOCKET_BLUE]");
    assert_eq!(texts[red + 2], "[SOCKET_BONUS +6 Stamina]");
    assert_eq!(texts[red + 3], "[DURABILITY 60/60]");
    assert_eq!(texts[red - 1], "[MOD_STAMINA +54]");
    assert_eq!(lines[red].1, GRAY);
    assert_eq!(lines[red + 2].1, GRAY);
}

/// An item object walks its slots: a gem prints its name in its socket's place, coded white with
/// no unmet condition (`0x629ce5`), an empty
/// coloured socket grays the bonus, the bonus's own enchant in slot 5 prints nothing, and a
/// prismatic enchant in slot 6 adds a colourless socket.
#[test]
fn an_instance_walks_its_sockets_and_greens_a_full_match() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    // A red gem in the red socket, the blue socket empty.
    bag_item(&mut s, vec![gem(2, "+20 Strength", 2)]);
    let lines = hover_bag(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let red = texts
        .iter()
        .position(|t| *t == "|cffffffff+20 Strength|r")
        .unwrap();
    assert_eq!(lines[red].1, GOLD);
    assert_eq!(texts[red + 1], "[EMPTY_SOCKET_BLUE]");
    assert_eq!(lines[red + 2], ("[SOCKET_BONUS +6 Stamina]".into(), GRAY));

    // Both filled, the blue with a purple (red and blue) gem, and the bonus active in slot 5.
    let bonus = EnchantView {
        slot: 5,
        id: 3301,
        name: "+6 Stamina".into(),
        ..Default::default()
    };
    bag_item(
        &mut s,
        vec![
            gem(2, "+20 Strength", 2),
            gem(3, "+10 Strength and +15 Stamina", 10),
            bonus,
        ],
    );
    let lines = hover_bag(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let red = texts
        .iter()
        .position(|t| *t == "|cffffffff+20 Strength|r")
        .unwrap();
    assert_eq!(texts[red + 1], "|cffffffff+10 Strength and +15 Stamina|r");
    assert_eq!(lines[red + 2], ("[SOCKET_BONUS +6 Stamina]".into(), GREEN));
    assert_eq!(
        texts.iter().filter(|t| t.contains("+6 Stamina")).count(),
        1,
        "the active bonus's slot-5 enchant prints no line of its own"
    );

    // A yellow gem in the red socket breaks the match.
    bag_item(
        &mut s,
        vec![gem(2, "+20 Agility", 4), gem(3, "+30 Stamina", 8)],
    );
    let lines = hover_bag(&mut s);
    assert!(lines.contains(&("[SOCKET_BONUS +6 Stamina]".into(), GRAY)));

    // A prismatic enchant opens a third, colourless socket and prints no line itself.
    bag_item(
        &mut s,
        vec![EnchantView {
            slot: 6,
            id: 3729,
            name: "Socket Belt".into(),
            prismatic_sockets: 1,
            ..Default::default()
        }],
    );
    let lines = hover_bag(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let red = texts
        .iter()
        .position(|t| *t == "[EMPTY_SOCKET_RED]")
        .unwrap();
    assert_eq!(texts[red + 1], "[EMPTY_SOCKET_BLUE]");
    assert_eq!(texts[red + 2], "[EMPTY_SOCKET_NO_COLOR]");
    assert!(!texts.contains(&"Socket Belt"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A gem whose enchant asks a skill the player lacks reads red, shows its gem's icon beside it, and
/// names the skill in a red line under it; a met requirement turns the line white and drops the
/// red line (`0x629c04`, `0x629d11`).
#[test]
fn an_unmet_gem_requirement_reds_its_line_and_names_the_skill() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    s.set_item_template(
        5000,
        ItemTemplateView {
            name: "Dragon's Eye".into(),
            icon: Some(r"Interface\Icons\INV_Jewelcrafting_DragonsEye02".into()),
            ..Default::default()
        },
    );
    let mut eye = gem(2, "+34 Strength", 2);
    eye.gem_item = 5000;
    eye.requirement = Some(EnchantRequirementView {
        skill: 755,
        skill_rank: 370,
        skill_name: Some("Jewelcrafting".into()),
        min_level: 0,
    });
    bag_item(&mut s, vec![eye]);
    s.run(
        r#"
        ENCHANT_ITEM_MIN_SKILL = "[ENCHANT_ITEM_MIN_SKILL %s %d]"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        for i = 1, 3 do tt:CreateTexture("TTTexture" .. i, "ARTWORK") end
    "#,
    )
    .unwrap();
    let lines = hover_bag(&mut s);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    let at = texts
        .iter()
        .position(|t| *t == "|cffff2020+34 Strength|r")
        .unwrap_or_else(|| panic!("the red gem line in {texts:?}"));
    assert_eq!(lines[at].1, GOLD);
    assert_eq!(
        lines[at + 1],
        (
            "[ENCHANT_ITEM_MIN_SKILL Jewelcrafting 370]".into(),
            [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0]
        )
    );
    let icon: String = s.eval("return TTTexture1:GetTexture() or ''").unwrap();
    assert!(icon.ends_with("INV_Jewelcrafting_DragonsEye02"), "{icon}");

    let mut req = PlayerReqState::default();
    req.skills.insert(755, 375);
    s.set_player_req_state(req);
    let texts: Vec<String> = hover_bag(&mut s).into_iter().map(|(t, _)| t).collect();
    assert!(
        texts.iter().any(|t| t == "|cffffffff+34 Strength|r"),
        "{texts:?}"
    );
    assert!(!texts
        .iter()
        .any(|t| t.starts_with("[ENCHANT_ITEM_MIN_SKILL")));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A condition on an inspected player's item counts that player's gems, not the viewer's.
#[test]
fn an_inspected_item_counts_its_wearers_gems() {
    let mut s = script();
    s.set_item_template(4000, socketed());
    let mut meta = gem(2, "+21 Critical Strike Rating", 1);
    meta.condition = Some(
        [EnchantConditionTest {
            left_type: 2,
            operator: 5,
            right_type: 0,
            right_value: 2,
            logic: 0,
        }; 5],
    );
    let mut slots: InventorySlots = Default::default();
    slots[1] = Some(InvSlotView {
        item_id: 4000,
        enchants: vec![meta],
        ..Default::default()
    });
    // The viewer wears the same head, no red gems; the inspected player has two.
    s.set_inventory_slots(slots.clone());
    s.set_inspect(Some(InspectView {
        unit: "target".into(),
        guid: 0x42,
        slots,
        gem_counts: [1, 2, 0, 0],
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT")
    "#,
    )
    .unwrap();
    let line = |s: &mut UiScript, unit: &str| {
        s.run(&format!(
            r#"TT:SetOwner(Slot, "ANCHOR_RIGHT"); TT:SetInventoryItem("{unit}", 1)"#
        ))
        .unwrap();
        lines_of(s)
            .into_iter()
            .find(|(t, _)| t.contains("+21 Critical Strike Rating"))
            .expect("the meta gem's line")
            .0
    };
    assert!(line(&mut s, "player").starts_with("|cff808080"));
    assert!(line(&mut s, "target").starts_with("|cffffffff"));
}

/// `GetItemInfo` takes a held item's name, in any case, and an unheld name answers nothing; a link
/// with a negative random property takes its `ItemRandomSuffix` name (`0x709de0`, `0x706d70`).
#[test]
fn get_item_info_finds_a_held_name_and_a_negative_suffix() {
    let mut s = script();
    s.set_item_template(
        4000,
        ItemTemplateView {
            name: "Socketed Helm".into(),
            quality: 4,
            ..Default::default()
        },
    );
    let mut slots = HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            count: 1,
            item_id: 4000,
            link: Some("|cffa335ee|Hitem:4000:0:0:0|h[Socketed Helm]|h|r".into()),
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
    s.set_random_suffixes(HashMap::from([(5, "of the Monkey".to_string())]));
    s.run(r#"ITEM_SUFFIX_TEMPLATE = "%s %s""#).unwrap();
    let (name, link): (Option<String>, Option<String>) = s
        .eval(r#"local n, l = GetItemInfo("socketed helm"); return n, l"#)
        .unwrap();
    assert_eq!(name.as_deref(), Some("Socketed Helm"));
    assert!(link.is_some_and(|l| l.contains("|Hitem:4000:")));
    let none: Option<String> = s.eval(r#"return (GetItemInfo("Unheld Helm"))"#).unwrap();
    assert_eq!(none, None);
    let suffixed: String = s
        .eval(r#"return (GetItemInfo("item:4000:0:0:0:0:0:-5:0"))"#)
        .unwrap();
    assert_eq!(suffixed, "Socketed Helm of the Monkey");
    let missing: String = s
        .eval(r#"return (GetItemInfo("item:4000:0:0:0:0:0:-9:0"))"#)
        .unwrap();
    assert_eq!(
        missing, "Socketed Helm ",
        "a missing row still takes the suffix form"
    );
}

/// A druid reads a main-hand weapon's feral attack power: `(dps - 54.81) * 14`, rounded.
#[test]
fn a_druid_reads_a_weapons_feral_attack_power() {
    let mut s = script();
    s.set_item_template(
        4000,
        ItemTemplateView {
            name: "Staff".into(),
            class: 2,
            subclass: 10,
            inventory_type: 17,
            damages: vec![(200.0, 300.0, 0)],
            delay_ms: 3000,
            ..Default::default()
        },
    );
    let hover = |s: &mut UiScript| {
        s.run(
            r#"
            if not Slot then
                local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
                a:SetWidth(10); a:SetHeight(10)
                CreateFrame("GameTooltip", "TT")
            end
            TT:SetOwner(Slot, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, 4000)
        "#,
        )
        .unwrap();
        lines_of(s)
    };
    assert!(!hover(&mut s)
        .iter()
        .any(|(t, _)| t.starts_with("[FERAL_AP")));
    s.set_player_req_state(PlayerReqState {
        class_id: 11,
        ..Default::default()
    });
    // 250 average over 3 s is 83.33 DPS: (83.33 - 54.81) * 14 = 399.3.
    let lines = hover(&mut s);
    assert!(
        lines.contains(&("[FERAL_AP 399]".into(), GREEN)),
        "{lines:?}"
    );
}
