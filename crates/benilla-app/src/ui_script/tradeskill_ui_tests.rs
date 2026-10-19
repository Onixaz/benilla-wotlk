//! The stock 3.3.5a crafting window (`Blizzard_TradeSkillUI`) on the production load, opened the
//! way the app opens it (`TRADE_SKILL_UPDATE`, then `TRADE_SKILL_SHOW`) over a book shaped like
//! the live client's: an Enchanting book, whose enchants file under the line's alternate verb,
//! and a Blacksmithing book of products.

use benilla_ui::script::{
    TradeSkillDifficulty, TradeSkillGroup, TradeSkillReagent, TradeSkillRecipe, TradeSkillState,
    UiScript,
};

/// Errors from the window's own code; the production load carries unrelated 3.3.5a gaps.
fn trade_skill_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("TradeSkill"))
        .collect()
}

fn recipe(spell_id: u32, name: &str, group: TradeSkillGroup, slots: u32) -> TradeSkillRecipe {
    let creates_item = group != TradeSkillGroup::Verb;
    TradeSkillRecipe {
        spell_id,
        name: name.into(),
        difficulty: TradeSkillDifficulty::Medium,
        num_available: 3,
        trivial_high: 100,
        group: Some(group),
        item_level: 0,
        inv_slots: slots,
        creates_item,
        product_item: if creates_item { spell_id + 10_000 } else { 0 },
        required_level: None,
        icon: Some("Interface\\Icons\\INV_Misc_Dust_01".into()),
        min_made: 1,
        max_made: 1,
        cooldown_end: None,
        description: (!creates_item).then(|| format!("Permanently enchant: {name}.")),
        search_text: vec![name.to_lowercase()],
        reagents: vec![TradeSkillReagent {
            item: 10_940,
            name: Some("Strange Dust".into()),
            icon: Some("Interface\\Icons\\INV_Enchant_DustStrange".into()),
            need: 1,
            have: 3,
        }],
        tools: vec![("Runed Copper Rod".into(), true)],
    }
}

fn enchanting() -> TradeSkillState {
    TradeSkillState {
        line: 333,
        line_name: "Enchanting".into(),
        alt_verb: "Enchant".into(),
        rank: 300,
        max_rank: 300,
        list_link: Some("|cffffd000|Htrade:7411:300:300:1:A|h[Enchanting]|h|r".into()),
        recipes: vec![
            recipe(
                7418,
                "Enchant Bracer - Minor Health",
                TradeSkillGroup::Verb,
                1 << 8,
            ),
            recipe(
                7443,
                "Enchant Chest - Minor Mana",
                TradeSkillGroup::Verb,
                1 << 4,
            ),
            recipe(
                7421,
                "Runed Copper Rod",
                TradeSkillGroup::SubClass {
                    class: 7,
                    subclass: 12,
                    name: Some("Enchanting".into()),
                },
                1 << 26,
            ),
        ],
    }
}

fn blacksmithing() -> TradeSkillState {
    let mail = || TradeSkillGroup::SubClass {
        class: 4,
        subclass: 3,
        name: Some("Mail".into()),
    };
    TradeSkillState {
        line: 164,
        line_name: "Blacksmithing".into(),
        alt_verb: "Modify".into(),
        rank: 80,
        max_rank: 150,
        list_link: None,
        recipes: vec![
            recipe(2661, "Copper Chain Vest", mail(), 1 << 4),
            recipe(2662, "Copper Chain Pants", mail(), 1 << 6),
            recipe(
                2660,
                "Rough Sharpening Stone",
                TradeSkillGroup::SubClass {
                    class: 0,
                    subclass: 8,
                    name: Some("Other".into()),
                },
                1 << 26,
            ),
        ],
    }
}

/// The app's open: the push, the rebuild's update, then the show.
fn open(s: &mut UiScript, state: TradeSkillState) {
    s.set_trade_skill(Some(state));
    s.fire_event("TRADE_SKILL_UPDATE", vec![]);
    s.fire_event("TRADE_SKILL_SHOW", vec![]);
    s.tick(0.0);
}

fn text(s: &UiScript, expr: &str) -> String {
    s.eval::<Option<String>>(&format!("return {expr}"))
        .unwrap()
        .unwrap_or_else(|| "<nil>".into())
}

/// The reported Enchanting window: the rows carry their labels, the header the alternate verb,
/// the selection its description, reagents and rod, and the Create button reads "Enchant".
#[test]
fn the_stock_enchanting_window_fills_its_rows_and_detail() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("tradeskill", false, "", |_| {});
    let _ = s.take_errors();
    open(&mut s, enchanting());

    let errors = trade_skill_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
    assert_eq!(
        s.eval::<Option<i64>>("return IsAddOnLoaded('Blizzard_TradeSkillUI')")
            .unwrap(),
        Some(1)
    );
    assert!(s
        .eval::<bool>("return TradeSkillFrame:IsShown() == 1")
        .unwrap());
    assert_eq!(text(&s, "TradeSkillSkill1:GetText()"), "Enchant");
    assert!(
        text(&s, "TradeSkillSkill2:GetText()").contains("Enchant Bracer - Minor Health"),
        "row 2: {}",
        text(&s, "TradeSkillSkill2:GetText()")
    );
    assert!(text(&s, "TradeSkillSkill4:GetText()").contains("Enchanting"));
    assert!(text(&s, "TradeSkillSkill5:GetText()").contains("Runed Copper Rod"));
    assert_eq!(
        text(&s, "TradeSkillSkillName:GetText()"),
        "Enchant Bracer - Minor Health"
    );
    assert_eq!(
        text(&s, "TradeSkillDescription:GetText()"),
        "Permanently enchant: Enchant Bracer - Minor Health."
    );
    assert_eq!(text(&s, "TradeSkillReagent1Name:GetText()"), "Strange Dust");
    assert!(text(&s, "TradeSkillRequirementText:GetText()").contains("Runed Copper Rod"));
    assert_eq!(text(&s, "TradeSkillCreateButton:GetText()"), "Enchant");
    assert!(
        !s.eval::<bool>("return TradeSkillCreateAllButton:IsShown() == 1")
            .unwrap(),
        "an enchant has no Create All"
    );
    assert!(s
        .eval::<bool>("return TradeSkillLinkButton:IsShown() == 1")
        .unwrap());
    assert_eq!(
        text(&s, "TradeSkillRankFrameSkillRank:GetText()"),
        "300/300"
    );

    // The search box at rank 300: a name narrows the list on TRADE_SKILL_FILTER_UPDATE.
    assert!(s
        .eval::<bool>("return TradeSkillFrameEditBox:IsShown() == 1")
        .unwrap());
    // The edit box flushes OnTextChanged in a tick; its filter event fires in the next.
    s.run("TradeSkillFrameEditBox:SetText('mana')").unwrap();
    s.tick(0.0);
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 2);
    assert!(
        text(&s, "TradeSkillSkill2:GetText()").contains("Minor Mana"),
        "row 2 after the search: {} / {} / errors {:?}",
        text(&s, "TradeSkillSkill2:GetText()"),
        text(&s, "(GetTradeSkillInfo(2))"),
        s.take_errors()
    );
    s.run("TradeSkillFrameEditBox:SetText(SEARCH)").unwrap();
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 5);

    // The sub-class menu's own click handler: entry 3 is the rods' sub-class.
    s.run("TradeSkillSubClassDropDownButton_OnClick({ GetID = function() return 3 end })")
        .unwrap();
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 2);
    assert_eq!(text(&s, "TradeSkillSkill1:GetText()"), "Enchanting");

    // Collapse-all leaves the headers.
    s.run("TradeSkillSubClassDropDownButton_OnClick({ GetID = function() return 1 end })")
        .unwrap();
    s.run("TradeSkillCollapseAllButton:Click()").unwrap();
    s.tick(0.0);
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 2);

    let errors = trade_skill_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
}

/// A Blacksmithing book: item products read "Create" with Create All, which casts through
/// `DoTradeSkill` and latches its count; a reopen on another line repaints that line.
#[test]
fn the_stock_blacksmithing_window_creates_and_switches_lines() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("tradeskill-bs", false, "", |_| {});
    let _ = s.take_errors();
    open(&mut s, enchanting());
    open(&mut s, blacksmithing());

    let errors = trade_skill_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
    assert_eq!(text(&s, "TradeSkillSkill1:GetText()"), "Other");
    assert_eq!(text(&s, "TradeSkillSkill3:GetText()"), "Mail");
    assert_eq!(
        text(&s, "TradeSkillSkillName:GetText()"),
        "Rough Sharpening Stone"
    );
    assert_eq!(
        text(&s, "TradeSkillCreateButton:GetText()"),
        text(&s, "CREATE")
    );
    assert!(s
        .eval::<bool>("return TradeSkillCreateAllButton:IsShown() == 1")
        .unwrap());
    assert!(!s
        .eval::<bool>("return TradeSkillLinkButton:IsShown() == 1")
        .unwrap());
    assert_eq!(text(&s, "TradeSkillRankFrameSkillRank:GetText()"), "80/150");

    s.run("TradeSkillCreateAllButton:Click()").unwrap();
    assert_eq!(s.take_trade_skill_casts(), [2660]);
    s.tick(0.0);
    assert_eq!(
        s.eval::<i64>("return TradeSkillInputBox:GetNumber()")
            .unwrap(),
        3,
        "the count Create All latched, capped by the reagents"
    );

    // The window's close button: the stock OnHide calls CloseTradeSkill.
    s.run("HideUIPanel(TradeSkillFrame)").unwrap();
    assert!(s.take_trade_skill_close());
    let errors = trade_skill_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
}
