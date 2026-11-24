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
        cooldown_on_hold: false,
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

/// The events signalled since the last read.
fn signalled() -> Vec<String> {
    crate::script::tick::fired::names()
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
    assert!(signalled().iter().all(|e| e == "TRADE_SKILL_UPDATE"));
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
    signalled();

    s.run("SetTradeSkillItemNameFilter('HEALTH')").unwrap();
    assert_eq!(names(&mut s), ["Enchant", "Enchant Bracer - Minor Health"]);
    assert_eq!(
        s.eval::<String>("return GetTradeSkillItemNameFilter()")
            .unwrap(),
        "health"
    );
    s.run("SetTradeSkillItemNameFilter('health')").unwrap();
    assert_eq!(
        signalled(),
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

/// `DoTradeSkill` casts once and latches the count once the cast went; a cast start commits it, each go casts again until the count is spent, and a failure stops it.
#[test]
fn the_repeat_runs_on_the_cast_edges() {
    let mut s = open();
    s.run("SelectTradeSkill(8) DoTradeSkill(8, 5)").unwrap();
    assert_eq!(
        s.trade_skill_list_mut(|t| std::mem::take(&mut t.casts)),
        [(7421, 2)],
        "capped by numAvailable"
    );
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        1,
        "no repeat before the cast went (`0x5dacbb`)"
    );
    s.trade_skill_list_mut(|t| t.set_recast(7421, 2));
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        2,
        "capped by numAvailable"
    );
    s.trade_skill_list_mut(|t| {
        t.commit_pending_recast(7421);
        assert!(t.do_recast(7421).0);
        assert!(!t.do_recast(7421).0, "the second go spends it");
    });
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        1
    );

    s.run("DoTradeSkill(8, 2)").unwrap();
    s.trade_skill_list_mut(|t| {
        for (spell, times) in std::mem::take(&mut t.casts) {
            t.set_recast(spell, times);
        }
        t.commit_pending_recast(7421);
        assert_eq!(t.cast_failed(7421), 1);
        assert!(!t.do_recast(7421).0);
    });
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

/// A product record in flight files and counts the recipes (`0x5dd889`) but runs no filter pass
/// (`0x5ddeb7`): no headers, and the filtered count stands until the record lands.
#[test]
fn a_pending_product_files_the_recipes_without_the_filter_pass() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    st.recipes[1].group = None;
    s.set_trade_skill(Some(st));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 0);
    assert_eq!(
        s.trade_skill_list_mut(|l| l.entry_count()),
        enchanting().recipes.len()
    );
    s.set_trade_skill(Some(enchanting()));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
}

/// A switch to a line whose product is in flight indexes the new line's recipes in the order the
/// rebuild reads them, the line's array (`0x5dd910`), under the old filtered count (`0x5dd889`,
/// `0x5ddeb7`).
#[test]
fn a_pending_switch_indexes_the_new_recipes_under_the_old_count() {
    let mut s = open();
    let mut other = enchanting();
    other.line = 164;
    other.recipes.reverse();
    other.recipes[0].group = None;
    let second = other.recipes[1].name.clone();
    let count = other.recipes.len();
    s.set_trade_skill(Some(other));
    assert_eq!(s.eval::<i64>("return GetNumTradeSkills()").unwrap(), 8);
    assert_eq!(s.trade_skill_list_mut(|l| l.entry_count()), count);
    assert_eq!(
        s.eval::<String>("return (GetTradeSkillInfo(2))").unwrap(),
        second
    );
    assert!(s
        .eval::<Option<String>>(&format!("return (GetTradeSkillInfo({}))", count + 1))
        .unwrap()
        .is_none());
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

/// `CloseTradeSkill` clears the line and fires TRADE_SKILL_CLOSE inside its call (`0x5da8b0`).
#[test]
fn close_trade_skill_fires_the_close() {
    let mut s = open();
    s.run("DoTradeSkill(2)").unwrap();
    // The host's cast went, so the repeat stands for the close to stop.
    s.trade_skill_list_mut(|t| {
        for (spell, times) in std::mem::take(&mut t.casts) {
            t.set_recast(spell, times);
        }
    });
    signalled();
    s.run("CloseTradeSkill()").unwrap();
    assert_eq!(
        s.trade_skill_list_mut(|t| t.line),
        0,
        "the close clears the line in the call"
    );
    assert_eq!(
        signalled(),
        ["UPDATE_TRADESKILL_RECAST", "TRADE_SKILL_CLOSE"]
    );
}

/// Each filter verb writes its filter, rebuilds the list and only then signals, inside its call (`0x5dcf16`-`0x5dcf22`, `0x5dcf3d`-`0x5dcf49`, `0x5dd014`-`0x5dd022`, `0x5dd056`-`0x5dd062`, `0x5dd140`-`0x5dd14c`, `0x5dd280`-`0x5dd28c`, `0x5dd3c0`-`0x5dd3cc`, `0x5dd402`-`0x5dd40e`, `0x5dd442`-`0x5dd44e`, `0x5dd4dd`-`0x5dd4e9`, `0x5dd57b`-`0x5dd587`): its listener reads the rebuilt list ahead of the code after the call; a name, a level range or a flag that does not move signals nothing (`0x5dcf9b`, `0x5dd047`, `0x5dd3f9`, `0x5dd439`).
#[test]
fn a_filter_verb_rebuilds_then_signals_inside_its_call() {
    let s = open();
    s.run(
        "seen = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_UPDATE') \
         f:RegisterEvent('TRADE_SKILL_FILTER_UPDATE') \
         f:SetScript('OnEvent', function(_, e) seen[#seen + 1] = e .. ':' .. GetNumTradeSkills() end)",
    )
    .unwrap();
    let call = |verb: &str| {
        s.eval::<Vec<String>>(&format!(
            "seen = {{}} {verb} seen[#seen + 1] = 'after:' .. GetNumTradeSkills() return seen"
        ))
        .unwrap()
    };
    for (verb, event) in [
        ("SetTradeSkillSubClassFilter(3, 1, 1)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillSubClassFilter(0)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillSubClassFilter(3, 0)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillSubClassFilter(0)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillInvSlotFilter(2, 1, 1)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillInvSlotFilter(0)", "TRADE_SKILL_UPDATE"),
        ("CollapseTradeSkillSubClass(0)", "TRADE_SKILL_UPDATE"),
        ("ExpandTradeSkillSubClass(0)", "TRADE_SKILL_UPDATE"),
        ("CollapseTradeSkillSubClass(1)", "TRADE_SKILL_UPDATE"),
        ("ExpandTradeSkillSubClass(1)", "TRADE_SKILL_UPDATE"),
        (
            "SetTradeSkillItemNameFilter('HEALTH')",
            "TRADE_SKILL_FILTER_UPDATE",
        ),
        (
            "SetTradeSkillItemNameFilter('')",
            "TRADE_SKILL_FILTER_UPDATE",
        ),
        ("SetTradeSkillItemLevelFilter(1, 2)", "TRADE_SKILL_UPDATE"),
        ("SetTradeSkillItemLevelFilter(0, 0)", "TRADE_SKILL_UPDATE"),
        ("TradeSkillOnlyShowMakeable(1)", "TRADE_SKILL_UPDATE"),
        ("TradeSkillOnlyShowMakeable(nil)", "TRADE_SKILL_UPDATE"),
        ("TradeSkillOnlyShowSkillUps(1)", "TRADE_SKILL_UPDATE"),
        ("TradeSkillOnlyShowSkillUps(nil)", "TRADE_SKILL_UPDATE"),
    ] {
        let seen = call(verb);
        assert_eq!(seen.len(), 2, "{verb}: {seen:?}");
        let (signal, shown) = seen[0].split_once(':').unwrap();
        assert_eq!(signal, event, "{verb}");
        assert_eq!(seen[1], format!("after:{shown}"), "{verb}");
    }
    for verb in [
        "SetTradeSkillItemNameFilter('')",
        "SetTradeSkillItemLevelFilter(0, 0)",
        "TradeSkillOnlyShowMakeable(nil)",
        "TradeSkillOnlyShowSkillUps(nil)",
    ] {
        assert_eq!(call(verb).len(), 1, "{verb}");
    }
    assert_eq!(
        call("SetTradeSkillSubClassFilter(3, 1, 1)"),
        ["TRADE_SKILL_UPDATE:2", "after:2"]
    );
}

/// `recipe` as the app files it.
fn row(r: &TradeSkillRecipe) -> TradeSkillRow {
    TradeSkillRow {
        spell_id: r.spell_id,
        name: r.name.clone(),
        difficulty: r.difficulty,
        num_available: r.num_available,
        trivial_high: r.trivial_high,
        group: r.group.clone(),
        item_level: r.item_level,
        inv_slots: r.inv_slots,
    }
}

/// A whole rebuild that goes in flight drops the sub-classes and opens both masks (`0x5dd638`,
/// `0x5dd63e`, `0x5dd644`), files only those before the first miss, shown and unfolded
/// (`0x5dde6e`), and returns before the saved flags come back (`0x5ddeb7`); past the miss only a
/// recipe without a product adds its slot bits (`0x5ddb08`, `0x5ddcfc`). The callback's whole
/// rebuild then saves those defaults: every header unfolds and every sub-class shows.
#[test]
fn a_whole_rebuild_in_flight_files_the_sub_classes_before_the_miss_with_default_flags() {
    let mut rows: Vec<TradeSkillRow> = enchanting().recipes.iter().map(row).collect();
    // Verb, rod, verb, verb, oil: the third goes in flight, the fourth has no product and the
    // fifth's product is cached.
    rows[3].inv_slots = 1 << 9;
    rows[4].inv_slots = 1 << 10;
    let mut l = TradeSkillList::default();
    l.open_line(333, 7411);
    l.refresh(true, Some(rows.clone()), &|_| true);
    // Fold the verb header, hide the rods.
    l.sub_class_filter &= !(1 << 2);
    l.collapse_filter &= !1;
    l.apply(true, &|_| true);
    assert!(l.sub_class_flags().iter().any(|&(_, _, folded)| folded));
    assert!(l.sub_class_flags().iter().any(|&(_, shown, _)| !shown));
    l.inv_slots = 0;

    let mut pending = rows.clone();
    pending[2].group = None;
    assert!(!l.refresh(true, Some(pending), &|_| true));
    let filed = l.sub_class_flags();
    assert_eq!(filed, [((-1, -1), true, false), ((7, 12), true, false)]);
    assert_eq!(
        (l.sub_class_filter, l.collapse_filter),
        (u32::MAX, u32::MAX)
    );
    assert_eq!(l.inv_slots, (1 << 8) | SLOT_NONEQUIP | (1 << 9));

    assert!(l.refresh(true, Some(rows), &|_| true));
    assert_eq!(l.sub_class_flags().len(), 3);
    assert!(l
        .sub_class_flags()
        .iter()
        .all(|&(_, shown, folded)| shown && !folded));
    assert_eq!(
        (l.sub_class_filter, l.collapse_filter),
        (u32::MAX, u32::MAX)
    );
    assert_eq!(l.filtered(), 8);
}

/// An in-place rebuild in flight keeps the sub-classes and their flags (`0x5ddd02`).
#[test]
fn an_in_place_rebuild_in_flight_keeps_the_sub_classes() {
    let rows: Vec<TradeSkillRow> = enchanting().recipes.iter().map(row).collect();
    let mut l = TradeSkillList::default();
    l.open_line(333, 7411);
    l.refresh(true, Some(rows.clone()), &|_| true);
    l.collapse_filter &= !1;
    l.apply(true, &|_| true);
    let before = l.sub_class_flags();
    let mut pending = rows;
    pending[0].group = None;
    assert!(!l.refresh(false, Some(pending), &|_| true));
    let after = l.sub_class_flags();
    assert_eq!(before, after);
    assert_eq!(l.collapse_filter, !1);
}

/// The game shutdown (`0x5dcaf0`) clears the line, the list and the repeat but writes neither the
/// skill-ups flag (`0xc235cc`, set only at `0x5dd43d`) nor the picked sub-class (`0xad0e04`): the
/// next login's list still shows skill-ups only and the slot menu still narrows.
#[test]
fn the_game_shutdown_keeps_the_skill_ups_flag_and_the_picked_sub_class() {
    let mut s = open();
    s.run(
        r#"HEADSLOT = "Head" WRISTSLOT = "Wrist" CHESTSLOT = "Chest"
           NONEQUIPSLOT = "Created Items"
           TradeSkillOnlyShowSkillUps(1) SetTradeSkillSubClassFilter(1, 1, 1)"#,
    )
    .unwrap();
    s.trade_skill_list_mut(|t| {
        t.set_recast(7443, 2);
        t.commit_pending_recast(7443);
        t.shutdown();
    });
    assert_eq!(
        s.eval::<i64>("return GetNumTradeSkills()").unwrap(),
        0,
        "no list"
    );
    assert_eq!(
        s.eval::<i64>("return GetTradeskillRepeatCount()").unwrap(),
        1,
        "no repeat"
    );
    s.set_trade_skill(Some(enchanting()));
    let n = names(&mut s);
    assert!(
        !n.contains(&"Runed Copper Rod".to_string()),
        "the gray rod is still filtered: {n:?}"
    );
    assert_eq!(
        s.eval::<i64>("return select('#', GetTradeSkillInvSlots())")
            .unwrap(),
        2,
        "the slot menu still lists the first sub-class's slots"
    );
}

/// An on-hold cooldown still answers: the verb tests only the history's answer (`0x5daa13`), which
/// reads an on-hold record as started now and 1 ms long (`0x807ad4`, `0x807af2`).
#[test]
fn an_on_hold_cooldown_answers_one_millisecond_left() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    for r in &mut st.recipes {
        r.cooldown_end = Some(100.0);
        r.cooldown_on_hold = true;
    }
    s.set_trade_skill(Some(st));
    s.set_now(90.0);
    let left = s
        .eval::<Option<f64>>("return GetTradeSkillCooldown(2)")
        .unwrap();
    assert_eq!(left, Some(0.001));
}

/// A cooldown at its end's millisecond still answers: the history skips only a record with negative
/// time left (`0x807ae5`), and the verb pushes the end less the clock (`0x5daa24`), 0.
#[test]
fn a_cooldown_at_its_end_answers_zero() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    for r in &mut st.recipes {
        r.cooldown_end = Some(100.0);
    }
    s.set_trade_skill(Some(st));
    s.set_now(100.0);
    let left = s
        .eval::<Option<f64>>("return GetTradeSkillCooldown(2)")
        .unwrap();
    assert_eq!(left, Some(0.0));
    s.set_now(100.001);
    let past = s
        .eval::<Option<f64>>("return GetTradeSkillCooldown(2)")
        .unwrap();
    assert_eq!(past, None, "a record past its end is skipped");
}

/// `GetTradeSkillCooldown` reads the OS clock inside the call (`0x5daa17`) and answers the end
/// less that (`0x5daa24`): with the clock run on past the last tick, the answer plus `GetTime()`
/// read next in the same handler is the end, not the end plus the time since the tick.
#[test]
fn the_cooldown_left_is_read_against_the_clock_at_the_call() {
    let mut s = UiScript::new().unwrap();
    let mut st = enchanting();
    for r in &mut st.recipes {
        r.cooldown_end = Some(100.0);
    }
    s.set_trade_skill(Some(st));
    s.set_now(90.0);
    s.set_live_clock(true);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let (left, now) = s
        .eval::<(f64, f64)>("local c = GetTradeSkillCooldown(2) return c, GetTime()")
        .unwrap();
    assert!(now >= 90.05, "the clock ran on past the tick: {now}");
    assert!(
        (left + now - 100.0).abs() < 0.01,
        "left {left} at {now} misses the end"
    );
}
