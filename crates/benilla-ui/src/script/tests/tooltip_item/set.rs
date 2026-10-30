//! The item-set block (`ITEM_SET_NAME`): a blank, the gold header, the skill line, the member
//! ladder, a blank, then the bonuses by ascending threshold; and the quest reward hovers.

use super::{lines_of, script};
use crate::script::*;

/// Members are cream when equipped and gray otherwise; a bonus is green only with the skill met and
/// enough owned. The bonus arms are two keys: an active bonus formats `ITEM_SET_BONUS` ("Set: %s",
/// no count), an inactive one `ITEM_SET_BONUS_GRAY` ("(%d) Set: %s").
#[test]
fn item_set_block_counts_and_colors() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let mut vest = ItemTemplateView {
        name: "Defias Mark".into(),
        quality: 2,
        class: 4,
        subclass: 2,
        inventory_type: 5,
        item_set: 161,
        ..Default::default()
    };
    vest.stats.clear();
    s.set_item_template(6303, vest);
    // The worn belt: the count reads each worn piece's own set id.
    s.set_item_template(
        6304,
        ItemTemplateView {
            name: "Defias Belt".into(),
            inventory_type: 6,
            item_set: 161,
            ..Default::default()
        },
    );
    // Two members equipped: the chest itself and the belt.
    let mut slots: InventorySlots = Default::default();
    slots[4] = Some(InvSlotView {
        item_id: 6303,
        ..Default::default()
    });
    slots[8] = Some(InvSlotView {
        item_id: 6304,
        ..Default::default()
    });
    s.set_inventory_slots(slots);
    let set_view = ItemSetView {
        name: "Defias Leather".into(),
        members: vec![
            ItemSetMember::new(6303, "Defias Mark", 5),
            ItemSetMember::new(6304, "Defias Belt", 6),
            ItemSetMember::new(6305, "Defias Gloves", 10),
            // Template still in flight: no line yet, and the members after it wait for the total.
            ItemSetMember {
                id: 6306,
                ..Default::default()
            },
            ItemSetMember::new(6307, "Defias Boots", 8),
        ],
        // Stored high-first in DBC slot order (The Gladiator ships 3,2,5,4); the renderer sorts.
        bonuses: vec![
            (4, "+10 Attack Power.".into()),
            (2, "Increases movement speed slightly.".into()),
        ],
        ..Default::default()
    };
    s.set_item_set(161, set_view.clone());
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot10"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, 6303)
        -- The spacer is the reference's own literal — a space and a newline, which is
        -- ONE row carrying one space, not the empty string (an empty AddLine adds no line at all
        -- in the reference: its line add bails before its `inc [esi+0x31c]`).
        assert(TTTextLeft3:GetText() == " \n", "blank before the header, got "
               .. string.format("%q", tostring(TTTextLeft3:GetText())))
        assert(TTTextLeft4:GetText() == "[SET_NAME Defias Leather 2/3]", "got " .. tostring(TTTextLeft4:GetText()))
        assert(TTTextLeft5:GetText() == "  Defias Mark")
        assert(TTTextLeft8:GetText() == "  Defias Boots", "in-flight member renders no line")
        assert(TTTextLeft9:GetText() == " \n", "blank before the bonuses, got "
               .. string.format("%q", tostring(TTTextLeft9:GetText())))
        -- Owned 2 of 5: the (2) bonus is ACTIVE and takes the countless ITEM_SET_BONUS; the
        -- (4) one is not and takes ITEM_SET_BONUS_GRAY, which is the arm that carries the count.
        assert(TTTextLeft10:GetText() == "[SET_BONUS Increases movement speed slightly.]",
               "thresholds sort ascending, got " .. tostring(TTTextLeft10:GetText()))
        assert(TTTextLeft11:GetText() == "[SET_BONUS_GRAY 4 +10 Attack Power.]")
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let color = |lines: &[(String, [f32; 4])], needle: &str| {
        lines
            .iter()
            .find(|(t, _)| t == needle)
            .unwrap_or_else(|| panic!("no line {needle:?}"))
            .1
    };
    let gold = [1.0, 210.0 / 255.0, 0.0, 1.0];
    let cream = [1.0, 1.0, 151.0 / 255.0, 1.0];
    let gray = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let red = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
    assert_eq!(
        color(&lines, "[SET_NAME Defias Leather 2/3]"),
        gold,
        "set name is gold"
    );
    assert_eq!(
        color(&lines, "  Defias Mark"),
        cream,
        "equipped member is cream"
    );
    assert_eq!(
        color(&lines, "  Defias Gloves"),
        gray,
        "unowned member is gray"
    );
    assert_eq!(
        color(&lines, "[SET_BONUS Increases movement speed slightly.]"),
        green,
        "active bonus is green"
    );
    assert_eq!(
        color(&lines, "[SET_BONUS_GRAY 4 +10 Attack Power.]"),
        gray,
        "inactive bonus is gray"
    );
    // The skill gate: an unmet skill reds its line and grays every bonus.
    let mut gated = set_view;
    gated.required_skill = 165;
    gated.required_skill_rank = 250;
    gated.required_skill_name = Some("Leatherworking".into());
    s.set_item_set(161, gated);
    s.run(
        r#"
        TT:SetOwner(Slot10, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, 6303)
        assert(TTTextLeft5:GetText() == "[MIN_SKILL Leatherworking 250]",
               "the skill line sits between header and members")
        assert(TTTextLeft6:GetText() == "  Defias Mark")
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(color(&lines, "[MIN_SKILL Leatherworking 250]"), red);
    // Grayed, the (2) bonus takes the counted key.
    assert_eq!(
        color(
            &lines,
            "[SET_BONUS_GRAY 2 Increases movement speed slightly.]"
        ),
        gray,
        "an unmet skill grays even a met threshold"
    );
    let mut req = PlayerReqState::default();
    req.skills.insert(165, 300);
    s.set_player_req_state(req);
    s.run(r#"TT:SetOwner(Slot10, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, 6303)"#)
        .unwrap();
    let lines = lines_of(&mut s);
    assert_eq!(
        color(&lines, "[MIN_SKILL Leatherworking 250]"),
        [1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        color(&lines, "[SET_BONUS Increases movement speed slightly.]"),
        green,
        "meeting the skill restores the green — and the countless key with it"
    );
    s.set_item_template(
        9999,
        ItemTemplateView {
            name: "Gladiator Piece".into(),
            item_set: 1,
            ..Default::default()
        },
    );
    s.run(r#"TT:SetOwner(Slot10, "ANCHOR_RIGHT"); SetItemByIdForTest(TT, 9999)"#)
        .unwrap();
    assert_eq!(s.take_item_set_asks(), vec![1], "the set ask recorded");
    assert!(s.take_errors().is_empty());
}

/// A worn piece counts by its own set id, so a variant the DBC does not list (a 25-player tier
/// piece) still counts, stands in cream for the member of its slot, and lights the bonus.
#[test]
fn a_worn_variant_outside_the_list_counts_toward_the_set() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    let piece = |name: &str, inventory_type: u32| ItemTemplateView {
        name: name.into(),
        quality: 4,
        class: 4,
        inventory_type,
        item_set: 794,
        ..Default::default()
    };
    s.set_item_template(40503, piece("Valorous Tunic", 5));
    s.set_item_template(40504, piece("Valorous Handguards", 10));
    let mut slots: InventorySlots = Default::default();
    slots[4] = Some(InvSlotView {
        item_id: 40503,
        ..Default::default()
    });
    slots[9] = Some(InvSlotView {
        item_id: 40504,
        ..Default::default()
    });
    s.set_inventory_slots(slots);
    s.set_item_set(
        794,
        ItemSetView {
            name: "Cryptstalker Battlegear".into(),
            members: vec![
                ItemSetMember::new(43000, "Heroes' Handguards", 10),
                ItemSetMember::new(43001, "Heroes' Headpiece", 1),
                ItemSetMember::new(43002, "Heroes' Tunic", 20),
            ],
            bonuses: vec![(2, "Your pet deals an additional 5% damage.".into())],
            ..Default::default()
        },
    );
    s.run(
        r#"
        local a = CreateFrame("Button", "Slot10"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT"):SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(TT, 40503)
    "#,
    )
    .unwrap();
    let lines = lines_of(&mut s);
    let cream = [1.0, 1.0, 151.0 / 255.0, 1.0];
    let gray = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let has = |t: &str, c: [f32; 4]| lines.contains(&(t.to_string(), c));
    assert!(
        has(
            "[SET_NAME Cryptstalker Battlegear 2/3]",
            [1.0, 210.0 / 255.0, 0.0, 1.0]
        ),
        "{lines:?}"
    );
    assert!(has("  Valorous Handguards", cream));
    assert!(has("  Heroes' Headpiece", gray));
    assert!(
        has("  Valorous Tunic", cream),
        "a chest stands in for the robe member"
    );
    assert!(has(
        "[SET_BONUS Your pet deals an additional 5% damage.]",
        green
    ));
    assert!(s.take_errors().is_empty());
}

/// `SetQuestItem` and `SetQuestLogItem`, the reward hovers (`QuestFrameTemplates.xml:148`,
/// `QuestLogFrame.xml:113`), render the row's item by id; a missing row leaves the tooltip empty.
#[test]
fn quest_item_hovers_render_the_rows_item_by_id() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        2024,
        ItemTemplateView {
            name: "Militia Hammer".into(),
            quality: 1,
            ..Default::default()
        },
    );
    let row = || QuestItemView {
        item_id: 2024,
        name: Some("Militia Hammer".into()),
        quality: 1,
        ..Default::default()
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        choices: vec![row()],
        ..QuestState::default()
    }));
    s.run(
        r#"
        local a = CreateFrame("Button", "Q1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT"); tt:SetQuestItem("choice", 1)
        "#,
    )
    .unwrap();
    assert_eq!(lines_of(&mut s)[0].0, "Militia Hammer");

    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestItem("choice", 2)"#)
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return TT:NumLines()").unwrap(),
        0,
        "an out-of-range row shows nothing"
    );
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestItem("reward", 1)"#)
        .unwrap();
    assert_eq!(s.eval::<i64>("return TT:NumLines()").unwrap(), 0);

    s.set_quest_log(QuestLogState {
        entries: vec![QuestLogEntryView {
            quest_id: 7,
            detail: Some(QuestLogDetail {
                rewards: vec![row()],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    });
    // The detail hangs on the selected row, so select it first.
    s.run("SelectQuestLogEntry(1)").unwrap();
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestLogItem("reward", 1)"#)
        .unwrap();
    assert_eq!(lines_of(&mut s)[0].0, "Militia Hammer");
    s.run(r#"TT:SetOwner(Q1, "ANCHOR_RIGHT"); TT:SetQuestLogItem("choice", 1)"#)
        .unwrap();
    assert_eq!(s.eval::<i64>("return TT:NumLines()").unwrap(), 0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
