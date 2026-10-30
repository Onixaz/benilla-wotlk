//! The 3.3.5a stat and socket lines (`0x6277f0`): the green rating stats under the requirements,
//! the empty sockets and the socket bonus, and the druid's feral attack power.

use std::collections::HashMap;

use super::{lines_of, script};
use crate::script::*;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];

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

/// An item object walks its slots: a gem prints its name white in its socket's place, an empty
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
    let red = texts.iter().position(|t| *t == "+20 Strength").unwrap();
    assert_eq!(lines[red].1, WHITE);
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
    let red = texts.iter().position(|t| *t == "+20 Strength").unwrap();
    assert_eq!(texts[red + 1], "+10 Strength and +15 Stamina");
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
