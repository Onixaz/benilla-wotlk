//! Scaling items (heirlooms) in the builder (`0x6277f0`): the shown level (`0x61e740`), the
//! level's armor, damage and stats, the range line, the compare's offset 3 arm (`0x6318be`), and
//! the bind region's account and class 10 arms (`0x627e7d`..`0x627f3d`).

use super::{lines_of, script};
use crate::script::*;

const STRINGS: &str = r#"
    ITEM_LEVEL_RANGE_CURRENT = "[RANGE %d %d %d]"
    ITEM_ACCOUNTBOUND        = "[ITEM_ACCOUNTBOUND]"
    ITEM_BIND_TO_ACCOUNT     = "[ITEM_BIND_TO_ACCOUNT]"
"#;

/// Distribution 3: stamina 50%, agility 100%, hit rating (31) 25%, maximum level 80. Row `n`'s
/// budget columns hold `100 * n`, its armor and DPS columns `10 * n`, its spell power `n`.
fn tables() -> ScalingTables {
    let mut t = ScalingTables::default();
    let mut d = ScalingDistributionView {
        stat_ids: [-1; 10],
        max_level: 80,
        ..Default::default()
    };
    d.stat_ids[..3].copy_from_slice(&[7, 3, 31]);
    d.bonuses[..3].copy_from_slice(&[5_000, 10_000, 2_500]);
    t.distributions.insert(3, d);
    t.values = (1..=80)
        .map(|n| {
            let mut row = [10 * n; 24];
            row[1] = n;
            for budget in [2, 3, 4, 5, 17, 18] {
                row[budget] = 100 * n;
            }
            row[16] = n;
            ScalingValuesView(row)
        })
        .collect();
    t
}

/// A plate chest that scales: primary budget, plate chest armor and spell power bits.
fn chest() -> ItemTemplateView {
    ItemTemplateView {
        name: "Heirloom Chest".into(),
        quality: 7,
        class: 4,
        subclass: 4,
        sub_class_display: Some("Plate".into()),
        scaling_armor_names: [Some("Leather".into()), Some("Mail".into())],
        inventory_type: 5,
        bonding: 1,
        flags: 0x800_0000,
        required_level: 1,
        armor: 999,
        stats: vec![(4, 77)],
        scaling_distribution: 3,
        scaling_value: 0x8 | 0x80_0000 | 0x8000,
        ..Default::default()
    }
}

fn setup(level: u32, class_flags: u32, proficiency: u32) -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(STRINGS).unwrap();
    s.set_scaling_tables(tables());
    s.set_item_template(9100, chest());
    s.set_player_req_state(PlayerReqState {
        level,
        class_id: 1,
        class_flags,
        proficiency: [(4, proficiency)].into_iter().collect(),
        ..Default::default()
    });
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT")
        "#,
    )
    .unwrap();
    s
}

fn texts(s: &mut UiScript) -> Vec<String> {
    lines_of(s).into_iter().map(|(t, _)| t).collect()
}

/// A link's level field picks the row: the template's armor and stats give way to the level's,
/// the range line names it, and the rating and spell power lines read the same row.
#[test]
fn a_linked_heirloom_reads_the_links_level() {
    let mut s = setup(40, 0, 1 << 4);
    let read = |s: &mut UiScript, level: u32| {
        s.run(&format!(
            r#"TT:SetOwner(Slot, "ANCHOR_RIGHT")
               TT:SetHyperlink("|Hitem:9100:0:0:0:0:0:0:0:{level}|h[Heirloom Chest]|h")"#
        ))
        .unwrap();
        texts(s)
    };
    let at2 = read(&mut s, 2);
    assert_eq!(
        at2,
        [
            "Heirloom Chest",
            "[ITEM_BIND_TO_ACCOUNT]",
            "[INVTYPE_CHEST]",
            "[ARMOR 20]",
            "[MOD_AGILITY +200]",
            "[MOD_STAMINA +100]",
            "[RANGE 1 80 2]",
            "[ONEQUIP] [MOD_HIT_RATING 50]",
            "[ONEQUIP] [MOD_SPELL_POWER 2]",
        ]
    );
    let at10 = read(&mut s, 10);
    assert!(at10.contains(&"[ARMOR 100]".to_string()), "{at10:?}");
    assert!(at10.contains(&"[RANGE 1 80 10]".to_string()), "{at10:?}");
    // A link with no level field shows the player's.
    s.run(
        r#"TT:SetOwner(Slot, "ANCHOR_RIGHT"); TT:SetHyperlink("|Hitem:9100|h[Heirloom Chest]|h")"#,
    )
    .unwrap();
    assert!(texts(&mut s).contains(&"[RANGE 1 80 40]".to_string()));
}

/// A loot row's requester is the loot's source (`0x62e638`): a unit source's level picks the row,
/// any other source (a chest, a bobber) leaves the player's.
#[test]
fn a_looted_heirloom_reads_the_source_units_level() {
    let mut s = setup(40, 0, 1 << 4);
    let read = |s: &mut UiScript, source_unit: bool, source_level: u32| {
        s.set_loot(Some(LootState {
            source_unit,
            source_level,
            master_candidates: Vec::new(),
            rows: vec![Some(LootRow {
                name: Some("Heirloom Chest".into()),
                texture: None,
                quantity: 1,
                quality: Some(7),
                is_coin: false,
                item_id: 9100,
                link: Some("|cffe6cc80|Hitem:9100:0:0:0:0:0:0:0:0|h[Heirloom Chest]|h|r".into()),
                random_property_id: 0,
                locked: false,
            })],
            fishing: false,
        }));
        s.run(r#"TT:SetOwner(Slot, "ANCHOR_RIGHT"); TT:SetLootItem(1)"#)
            .unwrap();
        texts(s)
    };
    let corpse = read(&mut s, true, 12);
    assert!(corpse.contains(&"[ARMOR 120]".to_string()), "{corpse:?}");
    assert!(
        corpse.contains(&"[RANGE 1 80 12]".to_string()),
        "{corpse:?}"
    );
    let chest = read(&mut s, false, 0);
    assert!(chest.contains(&"[RANGE 1 80 40]".to_string()), "{chest:?}");
    // Above the distribution's maximum the source is held to it, as any requester is.
    let boss = read(&mut s, true, 83);
    assert!(boss.contains(&"[RANGE 1 80 80]".to_string()), "{boss:?}");
}

/// The plate piece reads as mail for a class flagged `0x20` with the mail bit, leather without
/// it, and red plate for an unflagged class (`0x6288f5`).
#[test]
fn a_flagged_class_reads_scaling_plate_as_the_armor_it_wears() {
    for (flags, proficiency, want_right, red) in [
        (0x20, 1 << 3, "Mail", false),
        (0x20, 1 << 2, "Leather", false),
        (0, 1 << 3, "Plate", true),
        (0x20, 1 << 4, "Plate", false),
    ] {
        let mut s = setup(10, flags, proficiency);
        s.run(r#"TT:SetOwner(Slot, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, 9100)"#)
            .unwrap();
        let right: String = s.eval("return TTTextRight3:GetText()").unwrap();
        assert_eq!(
            right, want_right,
            "flags {flags:#x} proficiency {proficiency:#x}"
        );
        let color = super::right_color(&mut s, want_right);
        assert_eq!(color[1] < 0.5, red, "flags {flags:#x}: {color:?}");
    }
}

/// Offset 3 with a vendor open and shift up shows the offered heirloom at its lowest level,
/// headerless; shift, a closed vendor or a plain item answer nil.
#[test]
fn the_compare_offset_three_shows_the_heirloom_at_a_vendor() {
    let mut s = setup(40, 0, 1 << 4);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            ..Default::default()
        }),
    );
    s.set_item_template(
        9101,
        ItemTemplateView {
            name: "Plain".into(),
            inventory_type: 5,
            ..Default::default()
        },
    );
    let link = "|Hitem:9100:0:0:0:0:0:0:0:40|h[Heirloom Chest]|h";
    let ask = |s: &mut UiScript, link: &str, shift: bool| -> Option<i64> {
        s.eval(&format!(
            r#"TT:SetOwner(Slot, "ANCHOR_RIGHT")
               return TT:SetHyperlinkCompareItem("{link}", 3, {shift})"#
        ))
        .unwrap()
    };
    assert_eq!(ask(&mut s, link, false), None, "no vendor open");
    s.set_merchant(Some(MerchantState::default()));
    assert_eq!(ask(&mut s, link, true), None, "shift held");
    assert_eq!(
        ask(&mut s, "|Hitem:9101|h[Plain]|h", false),
        None,
        "a plain item"
    );
    assert_eq!(ask(&mut s, link, false), Some(1));
    let t = texts(&mut s);
    assert_eq!(t[0], "Heirloom Chest", "no header: {t:?}");
    assert!(t.contains(&"[RANGE 1 80 1]".to_string()), "{t:?}");
}

/// `GetItemStats` reads a heirloom link at its level field (`0x6207a0`), and the player's level
/// without one.
#[test]
fn get_item_stats_follows_the_links_level() {
    let s = setup(30, 0, 1 << 4);
    s.run(
        r#"
        local function stam(level)
            local t = GetItemStats("|Hitem:9100:0:0:0:0:0:0:0:" .. level .. "|h[x]|h")
            return t.ITEM_MOD_STAMINA_SHORT, t.RESISTANCE0_NAME
        end
        local a, armor = stam(2)
        assert(a == 100 and armor == 20, tostring(a) .. " " .. tostring(armor))
        assert(stam(10) == 500)
        assert(stam(0) == 1500, "the player's level")
        "#,
    )
    .unwrap();
}

/// The bind region by template flag `0x8000000`, bound state, and a class 10 item in and out of
/// the loot window.
#[test]
fn the_bind_line_follows_the_account_flag_and_skips_class_ten() {
    use crate::script::tooltip_item::bind_key;
    let item = |class, bonding, flags| ItemTemplateView {
        class,
        bonding,
        flags,
        ..Default::default()
    };
    let account = item(4, 1, 0x800_0000);
    assert_eq!(
        bind_key(&account, false, false),
        Some("ITEM_BIND_TO_ACCOUNT")
    );
    assert_eq!(bind_key(&account, true, false), Some("ITEM_ACCOUNTBOUND"));
    let mark = item(10, 1, 0);
    assert_eq!(bind_key(&mark, false, false), None, "class 10 in the bags");
    assert_eq!(bind_key(&mark, true, false), None, "a bound class 10 item");
    assert_eq!(bind_key(&mark, false, true), Some("ITEM_BIND_ON_PICKUP"));
    assert_eq!(bind_key(&mark, true, true), Some("ITEM_BIND_ON_PICKUP"));
    assert_eq!(
        bind_key(&item(10, 4, 0), true, false),
        Some("ITEM_BIND_QUEST")
    );
    for (bonding, key) in [
        (1, Some("ITEM_BIND_ON_PICKUP")),
        (2, Some("ITEM_BIND_ON_EQUIP")),
        (3, Some("ITEM_BIND_ON_USE")),
        (4, Some("ITEM_BIND_QUEST")),
        (5, None),
        (0, None),
    ] {
        assert_eq!(
            bind_key(&item(4, bonding, 0), false, false),
            key,
            "bonding {bonding}"
        );
    }
    assert_eq!(
        bind_key(&item(4, 2, 0), true, false),
        Some("ITEM_SOULBOUND")
    );
    assert_eq!(
        bind_key(&item(4, 5, 0), true, false),
        Some("ITEM_SOULBOUND")
    );
    assert_eq!(
        bind_key(&item(4, 0, 0), true, false),
        Some("ITEM_SOULBOUND")
    );
}

/// Bonus armor: an armor damage modifier above 0 greens the template's armor line (`0x629089`).
#[test]
fn bonus_armor_reads_green() {
    let mut s = setup(10, 0, 0);
    for (id, modifier) in [(9200, 0.0), (9201, 1.0)] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: format!("Cloak {id}"),
                inventory_type: 16,
                armor: 50 + id - 9200,
                armor_damage_modifier: modifier,
                ..Default::default()
            },
        );
    }
    for (id, want) in [(9200, [1.0, 1.0, 1.0, 1.0]), (9201, [0.0, 1.0, 0.0, 1.0])] {
        s.run(&format!(
            r#"TT:SetOwner(Slot, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, {id})"#
        ))
        .unwrap();
        let armor = format!("[ARMOR {}]", 50 + id - 9200);
        let line = lines_of(&mut s).into_iter().find(|(t, _)| *t == armor);
        assert_eq!(line.map(|(_, c)| c), Some(want), "{id}");
    }
}
