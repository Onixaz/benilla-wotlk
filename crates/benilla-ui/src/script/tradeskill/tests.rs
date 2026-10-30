use super::*;
use crate::script::UiScript;

/// A one-reagent recipe of `group`, slot bit `inv_slots`.
fn recipe(
    spell_id: u32,
    name: &str,
    difficulty: TradeSkillDifficulty,
    group: TradeSkillGroup,
    inv_slots: u32,
) -> TradeSkillRecipe {
    let creates_item = group != TradeSkillGroup::Verb;
    TradeSkillRecipe {
        spell_id,
        name: name.into(),
        difficulty,
        num_available: 2,
        trivial_high: 100,
        group: Some(group),
        item_level: 0,
        inv_slots,
        creates_item,
        product_item: if creates_item { spell_id + 10_000 } else { 0 },
        required_level: None,
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        min_made: 1,
        max_made: 1,
        cooldown_end: None,
        description: None,
        search_text: vec![name.to_lowercase()],
        reagents: vec![TradeSkillReagent {
            item: 10_940,
            name: Some("Strange Dust".into()),
            icon: None,
            need: 1,
            have: 2,
        }],
        tools: vec![("Runed Copper Rod".into(), true)],
    }
}

fn rods() -> TradeSkillGroup {
    TradeSkillGroup::SubClass {
        class: 7,
        subclass: 12,
        name: Some("Enchanting".into()),
    }
}

fn oils() -> TradeSkillGroup {
    TradeSkillGroup::SubClass {
        class: 0,
        subclass: 8,
        name: Some("Other".into()),
    }
}

/// An Enchanting book: three enchants (the verb group), a rod and an oil.
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
                TradeSkillDifficulty::Trivial,
                TradeSkillGroup::Verb,
                1 << 8,
            ),
            recipe(
                7421,
                "Runed Copper Rod",
                TradeSkillDifficulty::Trivial,
                rods(),
                SLOT_NONEQUIP,
            ),
            recipe(
                7443,
                "Enchant Chest - Minor Mana",
                TradeSkillDifficulty::Optimal,
                TradeSkillGroup::Verb,
                1 << 4,
            ),
            recipe(
                7426,
                "Enchant Chest - Minor Absorption",
                TradeSkillDifficulty::Optimal,
                TradeSkillGroup::Verb,
                1 << 4,
            ),
            recipe(
                25124,
                "Minor Wizard Oil",
                TradeSkillDifficulty::Easy,
                oils(),
                SLOT_NONEQUIP,
            ),
        ],
    }
}

const SLOT_NONEQUIP: u32 = 1 << 26;

fn open() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_trade_skill(Some(enchanting()));
    s
}

/// `(name, type)` of every visible entry.
fn rows(s: &mut UiScript) -> Vec<(String, String)> {
    let n = s.eval::<i64>("return GetNumTradeSkills()").unwrap();
    (1..=n)
        .map(|i| {
            s.eval::<(String, String)>(&format!(
                "local n, t = GetTradeSkillInfo({i}) return n or '<nil>', t"
            ))
            .unwrap()
        })
        .collect()
}

fn names(s: &mut UiScript) -> Vec<String> {
    rows(s).into_iter().map(|(n, _)| n).collect()
}

fn queued(s: &UiScript) -> Vec<String> {
    s.model_ref()
        .pending_events
        .iter()
        .map(|(e, _)| e.clone())
        .collect()
}

/// The list refresh: one header per sub-class, the `(-1, -1)` verb group first and named by the
/// alternate verb, then by class and name; recipes by difficulty, then name.
#[test]
fn the_list_groups_under_the_verb_header_and_sub_class_headers() {
    let mut s = open();
    assert_eq!(
        names(&mut s),
        [
            "Enchant",
            "Enchant Chest - Minor Absorption",
            "Enchant Chest - Minor Mana",
            "Enchant Bracer - Minor Health",
            "Other",
            "Minor Wizard Oil",
            "Enchanting",
            "Runed Copper Rod",
        ]
    );
    assert_eq!(rows(&mut s)[0].1, "header");
    assert_eq!(rows(&mut s)[1].1, "optimal");
    assert_eq!(
        s.eval::<(String, String, String)>("return GetTradeSkillSubClasses()")
            .unwrap(),
        ("Enchant".into(), "Other".into(), "Enchanting".into())
    );
}

/// `GetTradeSkillInfo`'s five values: a header's fold flag, a recipe's alternate verb unless it
/// makes an item, and (nil, nil, 0, nil, nil) past the list.
#[test]
fn info_returns_five_values_with_the_alternate_verb() {
    let s = open();
    let (_, t, n, expanded, verb) = s
        .eval::<(String, String, i64, Option<i64>, Option<String>)>("return GetTradeSkillInfo(1)")
        .unwrap();
    assert_eq!(
        (t.as_str(), n, expanded, verb),
        ("header", 0, Some(1), None)
    );
    let verb = s
        .eval::<Option<String>>("return select(5, GetTradeSkillInfo(2))")
        .unwrap();
    assert_eq!(verb.as_deref(), Some("Enchant"));
    let verb = s
        .eval::<Option<String>>("return select(5, GetTradeSkillInfo(8))")
        .unwrap();
    assert_eq!(verb, None, "a rod makes an item");
    assert!(s
        .eval::<bool>(
            "local a, b, c, d, e = GetTradeSkillInfo(99) \
             return a == nil and b == nil and c == 0 and d == nil and e == nil \
             and select('#', GetTradeSkillInfo(99)) == 5"
        )
        .unwrap());
}

/// The recipe sort: difficulty, then trivialHigh and item level descending, then name.
#[test]
fn recipes_sort_by_difficulty_trivial_rank_item_level_and_name() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes
        .retain(|r| r.group == Some(TradeSkillGroup::Verb));
    for r in &mut st.recipes {
        r.difficulty = TradeSkillDifficulty::Optimal;
    }
    st.recipes[0].trivial_high = 50; // Bracer
    st.recipes[1].trivial_high = 120; // Minor Mana
    st.recipes[2].trivial_high = 120; // Minor Absorption
    st.recipes[2].item_level = 5;
    s.set_trade_skill(Some(st));
    assert_eq!(
        names(&mut s)[1..],
        [
            "Enchant Chest - Minor Absorption",
            "Enchant Chest - Minor Mana",
            "Enchant Bracer - Minor Health",
        ]
    );
}

/// A folded header stays, its recipes go; 0 folds and unfolds everything; a recipe index raises.
#[test]
fn collapse_folds_a_header_or_all() {
    let mut s = open();
    s.run("CollapseTradeSkillSubClass(1)").unwrap();
    assert_eq!(names(&mut s)[..3], ["Enchant", "Other", "Minor Wizard Oil"]);
    assert_eq!(
        s.eval::<Option<i64>>("return select(4, GetTradeSkillInfo(1))")
            .unwrap(),
        None
    );
    s.run("CollapseTradeSkillSubClass(0)").unwrap();
    assert_eq!(names(&mut s), ["Enchant", "Other", "Enchanting"]);
    s.run("ExpandTradeSkillSubClass(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
    let err = s
        .eval::<String>("return tostring(select(2, pcall(CollapseTradeSkillSubClass, 2)))")
        .unwrap();
    assert!(err.contains("Bad sub class"), "{err}");
    assert!(queued(&s).iter().all(|e| e == "TRADE_SKILL_UPDATE"));
}

/// The sub-class menu: exclusive shows one, off hides one, 0 shows all.
#[test]
fn the_sub_class_filter_shows_one_or_hides_one() {
    let mut s = open();
    s.run("SetTradeSkillSubClassFilter(3, 1, 1)").unwrap();
    assert_eq!(names(&mut s), ["Enchanting", "Runed Copper Rod"]);
    assert_eq!(
        s.eval::<(Option<i64>, Option<i64>, Option<i64>)>(
            "return GetTradeSkillSubClassFilter(0), GetTradeSkillSubClassFilter(1), \
             GetTradeSkillSubClassFilter(3)"
        )
        .unwrap(),
        (None, None, Some(1))
    );
    s.run("SetTradeSkillSubClassFilter(0, 1, 1) SetTradeSkillSubClassFilter(2, 0)")
        .unwrap();
    assert_eq!(names(&mut s).len(), 6);
    assert!(s
        .eval::<String>("return tostring(select(2, pcall(SetTradeSkillSubClassFilter, 9, 1)))")
        .unwrap()
        .contains("Bad sub class"));
}

/// The slot menu lists the built recipes' slots, narrowed to the picked sub-class, and with all
/// slots shown each one reads nil.
#[test]
fn the_inv_slot_filter_counts_listed_slots() {
    let mut s = open();
    s.run(
        r#"HEADSLOT = "Head" WRISTSLOT = "Wrist" CHESTSLOT = "Chest"
           NONEQUIPSLOT = "Created Items""#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, String, String)>("return GetTradeSkillInvSlots()")
            .unwrap(),
        ("Chest".into(), "Wrist".into(), "Created Items".into())
    );
    assert_eq!(
        s.eval::<(Option<i64>, Option<i64>)>(
            "return GetTradeSkillInvSlotFilter(0), GetTradeSkillInvSlotFilter(1)"
        )
        .unwrap(),
        (Some(1), None)
    );
    s.run("SetTradeSkillInvSlotFilter(2, 1, 1)").unwrap();
    assert_eq!(names(&mut s), ["Enchant", "Enchant Bracer - Minor Health"]);
    assert_eq!(
        s.eval::<Option<i64>>("return GetTradeSkillInvSlotFilter(2)")
            .unwrap(),
        Some(1)
    );
    s.run("SetTradeSkillSubClassFilter(1, 1, 1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return select('#', GetTradeSkillInvSlots())")
            .unwrap(),
        2,
        "the verb group's slots only"
    );
}

/// The name filter folds case, fires TRADE_SKILL_FILTER_UPDATE once per change, and searches
/// each recipe's texts; the level filter reads the product's required level.
#[test]
fn the_name_and_level_filters_search_and_bound() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes[1].required_level = Some(5); // the rod
    st.recipes[4].required_level = Some(45); // the oil
    st.recipes[0].search_text.push("+5 health".into());
    s.set_trade_skill(Some(st));

    s.run("SetTradeSkillItemNameFilter('HEALTH')").unwrap();
    assert_eq!(names(&mut s), ["Enchant", "Enchant Bracer - Minor Health"]);
    assert_eq!(
        s.eval::<String>("return GetTradeSkillItemNameFilter()")
            .unwrap(),
        "health"
    );
    s.run("SetTradeSkillItemNameFilter('health')").unwrap();
    assert_eq!(
        queued(&s),
        ["TRADE_SKILL_FILTER_UPDATE"],
        "the same text is a no-op"
    );
    s.run("SetTradeSkillItemNameFilter('')").unwrap();
    assert!(s
        .eval::<bool>("return GetTradeSkillItemNameFilter() == nil")
        .unwrap());

    s.run("SetTradeSkillItemLevelFilter('40', '50')").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetTradeSkillItemLevelFilter()")
            .unwrap(),
        (40, 50)
    );
    let n = names(&mut s);
    assert!(n.contains(&"Minor Wizard Oil".to_string()));
    assert!(
        !n.contains(&"Runed Copper Rod".to_string()),
        "level 5 is out"
    );
    assert!(
        n.contains(&"Enchant Bracer - Minor Health".to_string()),
        "no product, no level to test"
    );
}

/// Makeable hides a recipe the bags cover none of (a reagent-free one is unlimited, -1); skill-ups
/// hides the gray ones.
#[test]
fn makeable_and_skill_up_filters() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes[0].num_available = 0;
    st.recipes[1].num_available = -1;
    s.set_trade_skill(Some(st));
    s.run("TradeSkillOnlyShowMakeable(1)").unwrap();
    let n = names(&mut s);
    assert!(!n.contains(&"Enchant Bracer - Minor Health".to_string()));
    assert!(n.contains(&"Runed Copper Rod".to_string()));
    s.run("TradeSkillOnlyShowMakeable(nil) TradeSkillOnlyShowSkillUps()")
        .unwrap();
    let n = names(&mut s);
    assert!(!n.contains(&"Runed Copper Rod".to_string()));
    assert!(!n.contains(&"Enchant Bracer - Minor Health".to_string()));
    assert_eq!(n.len(), 5);
}

/// The selection is a spell id: a header clears it, and its index follows it into the hidden tail.
#[test]
fn selection_rides_the_spell() {
    let s = open();
    assert_eq!(s.eval::<i64>("return GetFirstTradeSkill()").unwrap(), 2);
    s.run("SelectTradeSkill(8)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        8
    );
    s.run("CollapseTradeSkillSubClass(7)").unwrap();
    assert!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap()
            > 7,
        "past GetNumTradeSkills"
    );
    s.run("SelectTradeSkill(1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
}

/// `DoTradeSkill` casts once and latches the count; a cast start commits it, each go casts again
/// until the count is spent, and a failure stops it.
#[test]
fn the_repeat_runs_on_the_cast_edges() {
    let mut s = open();
    s.run("SelectTradeSkill(8) DoTradeSkill(8, 5)").unwrap();
    assert_eq!(s.take_trade_skill_casts(), [7421]);
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        2,
        "capped by numAvailable"
    );
    s.trade_skill_cast_started(7421);
    assert!(s.trade_skill_cast_done(7421));
    assert!(!s.trade_skill_cast_done(7421), "the second go spends it");
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        1
    );

    s.run("DoTradeSkill(8, 2)").unwrap();
    s.trade_skill_cast_started(7421);
    s.trade_skill_cast_failed(7421);
    assert!(!s.trade_skill_cast_done(7421));
}

/// Another line resets the filters and selection; the same line keeps them across a close.
#[test]
fn a_line_switch_resets_the_filters() {
    let mut s = open();
    s.run("SetTradeSkillItemNameFilter('oil') SelectTradeSkill(2)")
        .unwrap();
    s.set_trade_skill(None);
    s.set_trade_skill(Some(enchanting()));
    assert_eq!(names(&mut s), ["Other", "Minor Wizard Oil"]);
    let mut other = enchanting();
    other.line = 164;
    s.set_trade_skill(Some(other));
    assert_eq!(names(&mut s).len(), 8);
    assert_eq!(
        s.eval::<i64>("return GetTradeSkillSelectionIndex()")
            .unwrap(),
        0
    );
}

/// A product template in flight holds the list back.
#[test]
fn a_pending_product_holds_the_list() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes[1].group = None;
    s.set_trade_skill(Some(st));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
    s.set_trade_skill(Some(enchanting()));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
}

/// A switch to a line whose product is in flight shows nothing of the old line meanwhile.
#[test]
fn a_pending_switch_drops_the_old_lines_list() {
    let mut s = open();
    let mut other = enchanting();
    other.line = 164;
    other.recipes.reverse();
    other.recipes[0].group = None;
    s.set_trade_skill(Some(other));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return select(3, GetTradeSkillInfo(2))")
            .unwrap(),
        0
    );
}

/// The description, the links and the closed state.
#[test]
fn links_description_and_the_closed_line() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes[0].description = Some("Permanently enchant bracers.".into());
    s.set_trade_skill(Some(st));
    let row = names(&mut s)
        .iter()
        .position(|n| n == "Enchant Bracer - Minor Health")
        .unwrap()
        + 1;
    assert_eq!(
        s.eval::<String>(&format!("return GetTradeSkillDescription({row})"))
            .unwrap(),
        "Permanently enchant bracers."
    );
    assert!(s
        .eval::<bool>("return GetTradeSkillDescription(2) == nil")
        .unwrap());
    assert_eq!(
        s.eval::<String>(&format!("return GetTradeSkillRecipeLink({row})"))
            .unwrap(),
        "|cffffd000|Henchant:7418|h[Enchanting: Enchant Bracer - Minor Health]|h|r"
    );
    assert_eq!(
        s.eval::<String>(&format!("return GetTradeSkillItemLink({row})"))
            .unwrap(),
        "|cffffd000|Henchant:7418|h[Enchant Bracer - Minor Health]|h|r"
    );
    assert_eq!(
        s.eval::<i64>("return select('#', GetTradeSkillItemLink(1))")
            .unwrap(),
        0
    );
    assert!(s
        .eval::<String>("return GetTradeSkillListLink()")
        .unwrap()
        .contains("|Htrade:7411:"));
    assert_eq!(
        s.eval::<(Option<i64>, Option<String>)>("return IsTradeSkillLinked()")
            .unwrap(),
        (None, None)
    );
    s.set_trade_skill(None);
    assert_eq!(
        s.eval::<(String, i64, i64)>("return GetTradeSkillLine()")
            .unwrap(),
        ("UNKNOWN".into(), 0, 0)
    );
}

/// `CloseTradeSkill` queues the close for the app and fires TRADE_SKILL_CLOSE itself.
#[test]
fn close_trade_skill_fires_the_close() {
    let mut s = open();
    s.run("DoTradeSkill(2)").unwrap();
    s.model_mut().pending_events.clear();
    s.run("CloseTradeSkill()").unwrap();
    assert!(s.take_trade_skill_close());
    assert_eq!(
        queued(&s),
        ["UPDATE_TRADESKILL_RECAST", "TRADE_SKILL_CLOSE"]
    );
}
