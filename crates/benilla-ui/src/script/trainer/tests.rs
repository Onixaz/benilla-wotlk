//! Tests for the trainer record array: the row orders, the filters, the collapse, the selection
//! pick and the intents.

use super::*;
use crate::script::UiScript;

fn svc(
    spell_id: u32,
    name: &str,
    cat: TrainerServiceCategory,
    group_key: u32,
    group_name: &str,
) -> TrainerService {
    TrainerService {
        spell_id,
        tooltip: TrainerTooltip::Spell {
            spell_id,
            alt_caster: false,
        },
        name: Some(name.into()),
        subtext: Some("Rank 1".into()),
        texture: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        description: Some(format!("Teaches {name}.")),
        cost: 100,
        category: cat,
        level_req: 10,
        group_key,
        group_name: group_name.into(),
        ..Default::default()
    }
}

/// A class trainer whose tree is `[H:Arms, Heroic Strike, Cleave, H:Fury, Bloodrage]`, the three
/// services available, used and unavailable.
fn trainer() -> TrainerState {
    let mut hs = svc(
        78,
        "Heroic Strike",
        TrainerServiceCategory::Available,
        26,
        "Arms",
    );
    hs.level_req = 1;
    let mut cl = svc(284, "Cleave", TrainerServiceCategory::Used, 26, "Arms");
    cl.level_req = 20;
    let br = svc(
        285,
        "Bloodrage",
        TrainerServiceCategory::Unavailable,
        256,
        "Fury",
    );
    TrainerState {
        greeting: "What would you like to learn?".into(),
        trainer_type: 0,
        groups: Vec::new(),
        services: vec![hs, cl, br],
    }
}

/// A VM with `serviceTypeFilter` registered at its default, 3.
fn script() -> UiScript {
    let s = UiScript::new().unwrap();
    s.register_cvars([("serviceTypeFilter", "3")]);
    s
}

/// [`script`] with every state shown.
fn script_all() -> UiScript {
    let s = script();
    s.run("SetTrainerServiceTypeFilter('all')").unwrap();
    s
}

/// Every visible row as `(name, serviceType)`.
fn visible(s: &mut UiScript) -> Vec<(String, String)> {
    let n = s.eval::<i64>("return GetNumTrainerServices()").unwrap();
    (1..=n)
        .map(|i| {
            s.eval::<(String, String)>(&format!(
                "local n,_,t = GetTrainerServiceInfo({i}) return n,t"
            ))
            .unwrap()
        })
        .collect()
}

fn sel(s: &mut UiScript) -> Option<i64> {
    s.eval::<Option<i64>>("return GetTrainerSelectionIndex()")
        .unwrap()
}

#[test]
fn tree_interleaves_headers_and_ordered_services() {
    let mut s = script_all();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);
    // Past the end: three nils and isExpanded 1 (`0x595090`).
    assert!(s
        .eval::<bool>(
            "local n,s,t,e = GetTrainerServiceInfo(1) return n == nil and s == nil and t == nil and e == 1"
        )
        .unwrap());

    s.set_trainer(Some(trainer()));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    let (hn, hsub, ht, hexp) = s
        .eval::<(String, Option<String>, String, Option<i64>)>(
            "local n,s,t,e = GetTrainerServiceInfo(1) return n,s,t,e",
        )
        .unwrap();
    assert_eq!((hn.as_str(), ht.as_str()), ("Arms", "header"));
    assert_eq!((hsub.as_deref(), hexp), (Some(""), Some(1)));

    let info = |s: &mut UiScript, i: i64| {
        s.eval::<(String, String, Option<i64>)>(&format!(
            "local n,_,t,e = GetTrainerServiceInfo({i}) return n,t,e"
        ))
        .unwrap()
    };
    assert_eq!(
        info(&mut s, 2),
        ("Heroic Strike".into(), "available".into(), Some(1)),
        "a service's isExpanded is 1"
    );
    assert_eq!(info(&mut s, 3), ("Cleave".into(), "used".into(), Some(1)));
    assert_eq!(info(&mut s, 4), ("Fury".into(), "header".into(), Some(1)));
    assert_eq!(
        info(&mut s, 5),
        ("Bloodrage".into(), "unavailable".into(), Some(1))
    );
}

#[test]
fn service_getters_read_the_row_at_a_record_index() {
    let mut s = script_all();
    let mut t = trainer();
    // Heroic Strike, row 2, gets a full gate set; slot 1 of its abilities is empty.
    t.services[0].skill_req = Some(TrainerSkillReq {
        name: "Blacksmithing".into(),
        rank: 100,
        met: true,
    });
    t.services[0].ability_reqs = [
        None,
        Some(TrainerAbilityReq {
            name: Some("Apprentice".into()),
            met: false,
        }),
        None,
    ];
    t.services[0].talent_cost = 1;
    t.services[0].profession_cost = 1;
    s.set_trainer(Some(t));

    assert_eq!(
        s.eval::<(i64, i64, i64)>("return GetTrainerServiceCost(2)")
            .unwrap(),
        (100, 1, 1),
        "both point words read raw"
    );
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceLevelReq(2)")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<String>("return GetTrainerServiceDescription(2)")
            .unwrap(),
        "Teaches Heroic Strike."
    );
    let (skill, rank, has) = s
        .eval::<(String, i64, i64)>("return GetTrainerServiceSkillReq(2)")
        .unwrap();
    assert_eq!((skill.as_str(), rank, has), ("Blacksmithing", 100, 1));
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceNumAbilityReq(2)")
            .unwrap(),
        1
    );
    assert!(
        s.eval::<bool>("local n,h = GetTrainerServiceAbilityReq(2,1) return n == nil and h == 1")
            .unwrap(),
        "the getter reads the raw slot, and slot 1 is empty"
    );
    assert!(s
        .eval::<bool>(
            "local n,h = GetTrainerServiceAbilityReq(2,2) return n == 'Apprentice' and h == nil"
        )
        .unwrap());
    assert!(s
        .eval::<bool>("local n,m = GetTrainerServiceStepReq(2) return n == nil and m == 1")
        .unwrap());

    // Row 1 is a header, so the getters answer their defaults.
    assert_eq!(
        s.eval::<i64>("return GetTrainerServiceLevelReq(1)")
            .unwrap(),
        0
    );
    assert!(s
        .eval::<bool>("return GetTrainerServiceIcon(1) == nil")
        .unwrap());
    assert!(s
        .eval::<bool>(
            "local n,r,h = GetTrainerServiceSkillReq(1) return n == nil and r == 0 and h == 1"
        )
        .unwrap());
}

#[test]
fn the_1_12_trainer_verbs_are_gone() {
    let s = script();
    for name in [
        "IsTalentTrainer",
        "IsTrainerServiceLearnSpell",
        "IsTrainerServiceTradeSkill",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name} == nil")).unwrap(),
            "{name} is not in `0xacf3e8`"
        );
    }
}

/// The filter is the `serviceTypeFilter` CVar (`0x594ba0`): default 3 hides used services, and a
/// line with nothing left in a shown state hides its header.
#[test]
fn state_filter_takes_a_lines_header_with_its_last_service() {
    let mut s = script();
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        4,
        "default 3: Cleave, the used service, is hidden"
    );
    assert!(s
        .eval::<bool>("return GetTrainerServiceTypeFilter('used') == nil")
        .unwrap());

    s.run("SetTrainerServiceTypeFilter('available', 0)")
        .unwrap();
    assert_eq!(
        visible(&mut s),
        [
            ("Fury".to_string(), "header".to_string()),
            ("Bloodrage".to_string(), "unavailable".to_string()),
        ]
    );
    assert_eq!(s.cvar("serviceTypeFilter").as_deref(), Some("2"));

    s.run("SetTrainerServiceTypeFilter('unavailable', 0)")
        .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);
    // The records remain: the getters bound on the whole array, not the visible count.
    assert_eq!(
        s.eval::<String>("return (GetTrainerServiceInfo(1))")
            .unwrap(),
        "Arms",
        "record 1 is still the Arms header, now in the hidden tail"
    );
}

#[test]
fn set_type_filter_takes_all_exclusive_and_raises_on_a_bad_type() {
    let mut s = script();
    s.set_trainer(Some(trainer()));
    s.run("SetTrainerServiceTypeFilter('used', 1, 1)").unwrap();
    assert_eq!(s.cvar("serviceTypeFilter").as_deref(), Some("4"));
    assert_eq!(
        visible(&mut s),
        [
            ("Arms".to_string(), "header".to_string()),
            ("Cleave".to_string(), "used".to_string()),
        ]
    );
    s.run("SetTrainerServiceTypeFilter('all')").unwrap();
    assert_eq!(s.cvar("serviceTypeFilter").as_deref(), Some("7"));

    let err = s
        .run("SetTrainerServiceTypeFilter('known', 1)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Bad service type in SetTrainerServiceTypeFilter"),
        "{err}"
    );
    let err = s
        .run("SetTrainerServiceTypeFilter('used')")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Missing on/off parameter"), "{err}");
    let err = s
        .run("return GetTrainerServiceTypeFilter('known')")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Bad service type in GetTrainerServiceTypeFilter"),
        "{err}"
    );
}

/// A collapsed line keeps its header; its services leave (`0x594ba0`).
#[test]
fn collapse_keeps_the_header_and_raises_on_a_service_row() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    s.run("CollapseTrainerSkillLine(1)").unwrap(); // fold Arms
    assert_eq!(
        visible(&mut s),
        [
            ("Arms".to_string(), "header".to_string()),
            ("Fury".to_string(), "header".to_string()),
            ("Bloodrage".to_string(), "unavailable".to_string()),
        ]
    );
    assert!(s
        .eval::<bool>("local _,_,t,e = GetTrainerServiceInfo(1) return t=='header' and e==nil")
        .unwrap());
    s.run("ExpandTrainerSkillLine(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    s.run("CollapseTrainerSkillLine(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 2);
    s.run("ExpandTrainerSkillLine(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);

    let err = s
        .run("CollapseTrainerSkillLine(2)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Bad skill line in CollapseTrainerSkillLine"),
        "{err}"
    );
}

#[test]
fn collapse_survives_a_content_update_and_resets_per_list() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    s.run("CollapseTrainerSkillLine(1)").unwrap(); // fold Arms
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 3);

    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<i64>("return GetNumTrainerServices()").unwrap(),
        3,
        "Arms stays collapsed across a content update"
    );

    s.reset_trainer_list_state();
    s.set_trainer(Some(trainer()));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
    assert_eq!(
        s.cvar("serviceTypeFilter").as_deref(),
        Some("7"),
        "a list resets the lines, never the state filter"
    );
}

/// The line filter (`0x593eb0`, `0x596010`): lines by sorted index, names from
/// `GetTrainerSkillLines` (`0x594650`).
#[test]
fn the_skill_line_filter_hides_a_line_and_its_header() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    assert_eq!(
        s.eval::<(String, String)>("return GetTrainerSkillLines()")
            .unwrap(),
        ("Arms".to_string(), "Fury".to_string())
    );
    s.run("SetTrainerSkillLineFilter(1, 0)").unwrap();
    assert_eq!(
        visible(&mut s),
        [
            ("Fury".to_string(), "header".to_string()),
            ("Bloodrage".to_string(), "unavailable".to_string()),
        ]
    );
    assert!(s
        .eval::<bool>(
            "return GetTrainerSkillLineFilter(1) == nil and GetTrainerSkillLineFilter(2) == 1 \
             and GetTrainerSkillLineFilter(0) == nil"
        )
        .unwrap());
    s.run("SetTrainerSkillLineFilter(1, 1, 1)").unwrap();
    assert_eq!(
        visible(&mut s).first().map(|(n, _)| n.as_str()),
        Some("Arms")
    );
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 3);
    s.run("SetTrainerSkillLineFilter(0)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
    let err = s
        .run("return GetTrainerSkillLineFilter(3)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Bad skill line in GetTrainerSkillLineFilter"),
        "{err}"
    );
}

/// The buy refuses any state byte but 0 (`0x594da0`).
#[test]
fn buy_queues_an_available_services_spell_id_and_refuses_every_other_row() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    s.run("BuyTrainerService(2)").unwrap();
    assert_eq!(s.take_trainer_buys(), vec![78]);
    assert!(s.take_trainer_buys().is_empty(), "drained");

    s.run("BuyTrainerService(1)").unwrap();
    assert!(s.take_trainer_buys().is_empty(), "a header is not buyable");

    s.run("BuyTrainerService(3)").unwrap();
    s.run("BuyTrainerService(5)").unwrap();
    assert!(
        s.take_trainer_buys().is_empty(),
        "only state byte 0, available, reaches the wire"
    );
}

/// `FilterAndSortServices` picks the first visible available service on a list, a filter or a
/// collapse; `SelectTrainerService` (`0x5943a0`) overrides it until the next.
#[test]
fn every_resort_picks_the_first_available_service() {
    let mut s = script_all();
    assert_eq!(sel(&mut s), None);
    s.set_trainer(Some(trainer()));
    assert_eq!(sel(&mut s), Some(2), "Heroic Strike, the one available");

    s.run("SelectTrainerService(5)").unwrap();
    assert_eq!(sel(&mut s), Some(5));
    s.run("SelectTrainerService(9)").unwrap(); // past the end
    assert_eq!(sel(&mut s), None);
    s.run("SelectTrainerService(1)").unwrap(); // a header
    assert_eq!(sel(&mut s), None);

    s.run("SelectTrainerService(5)").unwrap();
    s.run("SetTrainerServiceTypeFilter('used', 0)").unwrap();
    assert_eq!(sel(&mut s), Some(2), "the filter commit re-picked");

    // No available service shown: the record at array index 1, here Bloodrage under Fury.
    s.run("SetTrainerServiceTypeFilter('available', 0)")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return (GetTrainerServiceInfo(GetTrainerSelectionIndex()))")
            .unwrap(),
        "Bloodrage"
    );

    // The same states again repick nothing; a learned service does.
    s.run("SetTrainerServiceTypeFilter('all')").unwrap();
    s.run("SelectTrainerService(5)").unwrap();
    s.set_trainer(Some(trainer()));
    assert_eq!(
        sel(&mut s),
        Some(5),
        "an unchanged push keeps the player's pick"
    );
    let mut learned = trainer();
    learned.services[0].category = TrainerServiceCategory::Used;
    s.set_trainer(Some(learned));
    assert_eq!(
        sel(&mut s),
        Some(2),
        "no available service is left, so array index 1, Heroic Strike, is picked"
    );

    assert!(!s.take_trainer_close());
    s.run("CloseTrainer()").unwrap();
    assert!(s.take_trainer_close());
    assert!(!s.take_trainer_close(), "drained");
}

#[test]
fn the_pick_lands_on_a_header_or_in_the_tail() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    let shown = |s: &mut UiScript| s.eval::<i64>("return GetNumTrainerServices()").unwrap();
    // Folding Arms hides Heroic Strike; array index 1 is then the Fury header, which selects nothing.
    s.run("CollapseTrainerSkillLine(1)").unwrap();
    assert_eq!(sel(&mut s), None);
    s.run("ExpandTrainerSkillLine(1)").unwrap();
    assert_eq!(sel(&mut s), Some(2));
    // Every state off: nothing shows, and array index 1 is Heroic Strike in the hidden tail.
    s.run("SetTrainerServiceTypeFilter('available', 1, 1) SetTrainerServiceTypeFilter('available', 0)")
        .unwrap();
    assert_eq!(shown(&mut s), 0);
    let picked = sel(&mut s).unwrap();
    assert!(picked > shown(&mut s));
    assert_eq!(
        s.eval::<String>(&format!("return (GetTrainerServiceInfo({picked}))"))
            .unwrap(),
        "Heroic Strike"
    );
}

#[test]
fn clearing_empties_and_resets_selection() {
    let mut s = script_all();
    s.set_trainer(Some(trainer()));
    s.set_trainer(None);
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 0);
    assert_eq!(sel(&mut s), None);
}

#[test]
fn unresolved_skill_line_is_dropped() {
    let mut s = script_all();
    let mut t = trainer();
    t.services.push(svc(
        999,
        "Orphan Spell",
        TrainerServiceCategory::Available,
        0,
        "",
    ));
    s.set_trainer(Some(t));
    assert_eq!(s.eval::<i64>("return GetNumTrainerServices()").unwrap(), 5);
}

/// `QSortSkillLines` (`0x5941b0`): a line whose every service costs points sorts after the rest.
#[test]
fn a_line_of_point_cost_services_sorts_last() {
    let mut s = script_all();
    let mut t = trainer();
    for v in t.services.iter_mut().filter(|v| v.group_key == 26) {
        v.profession_cost = 1;
    }
    s.set_trainer(Some(t));
    let headers: Vec<String> = visible(&mut s)
        .into_iter()
        .filter(|(_, t)| t == "header")
        .map(|(n, _)| n)
        .collect();
    assert_eq!(headers, ["Fury", "Arms"]);
}

#[test]
fn a_tradeskill_trainer_has_its_flag_and_unnamed_lines() {
    let mut s = script_all();
    assert!(s
        .eval::<bool>("return IsTradeskillTrainer() == nil")
        .unwrap());
    let recipe = svc(
        2743,
        "Copper Chain Pants",
        TrainerServiceCategory::Unavailable,
        2,
        "Recipes",
    );
    s.set_trainer(Some(TrainerState {
        greeting: String::new(),
        trainer_type: 2,
        groups: Vec::new(),
        services: vec![recipe],
    }));
    assert!(s.eval::<bool>("return IsTradeskillTrainer() == 1").unwrap());
    assert!(
        s.eval::<bool>(
            "return select('#', GetTrainerSkillLines()) == 1 and GetTrainerSkillLines() == nil"
        )
        .unwrap(),
        "key 2 names no SkillLine row"
    );
}

/// A blacksmith's list in the order `QSortServices_Tradeskill` (`0x5949a0`) and the raw line keys
/// (`0x593ce0`) give it: the skill step first under its own header, `reqLevel` inert, the recipes
/// by `reqSkillValue` then name.
#[test]
fn tradeskill_trainer_sorts_by_line_key_then_skill_then_name() {
    // (wire spell, name, reqSkillValue), reversed below so the order must come from the sort.
    let recipes: &[(u32, &str, u32)] = &[
        (2743, "Copper Chain Pants", 1),
        (2754, "Copper Mace", 15),
        (2755, "Copper Axe", 20),
        (3340, "Copper Chain Boots", 20),
        (2756, "Copper Shortsword", 25),
        (3341, "Rough Grinding Stone", 25),
        (9984, "Copper Claymore", 30),
        (8881, "Copper Dagger", 30),
    ];
    let mut services: Vec<TrainerService> = recipes
        .iter()
        .map(|&(id, name, req)| {
            let mut s = svc(id, name, TrainerServiceCategory::Unavailable, 2, "Recipes");
            s.subtext = None;
            s.level_req = 0;
            s.skill_req = Some(TrainerSkillReq {
                name: "Blacksmithing".into(),
                rank: req,
                met: false,
            });
            s
        })
        .collect();
    services.reverse();
    let mut learn = svc(
        2018,
        "Blacksmithing",
        TrainerServiceCategory::Available,
        1,
        "Development Skills",
    );
    learn.subtext = Some("Apprentice".into());
    learn.level_req = 5; // inert at type 2
    learn.profession_cost = 1;
    services.insert(services.len() / 2, learn);

    let mut s = script_all();
    s.set_trainer(Some(TrainerState {
        greeting: "Care to learn how to turn the ore that you find into weapons?".into(),
        trainer_type: 2,
        groups: Vec::new(),
        services,
    }));

    let rows = visible(&mut s);
    let got: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        got,
        [
            "Development Skills",
            "Blacksmithing",
            "Recipes",
            "Copper Chain Pants",
            "Copper Mace",
            "Copper Axe",
            "Copper Chain Boots",
            "Copper Shortsword",
            "Rough Grinding Stone",
            "Copper Claymore",
            "Copper Dagger",
        ]
    );
}

/// `GetTrainerServiceStepIncrease` (`0x595c40`) answers only for a step not yet known.
#[test]
fn the_step_getters_read_the_service_and_its_state() {
    let mut s = script_all();
    let mut step = svc(
        2018,
        "Blacksmithing",
        TrainerServiceCategory::Available,
        1,
        "Development Skills",
    );
    step.skill_step = true;
    step.step_increase = Some("Increases potential".into());
    step.item_link = Some("|cffffffff|Hitem:2853:0:0:0|h[Copper Bracers]|h|r".into());
    step.skill_line = Some("Blacksmithing".into());
    let mut known = step.clone();
    known.spell_id = 3100;
    known.category = TrainerServiceCategory::Used;
    s.set_trainer(Some(TrainerState {
        greeting: String::new(),
        trainer_type: 2,
        groups: Vec::new(),
        services: vec![step, known],
    }));
    assert!(s
        .eval::<bool>(
            "return IsTrainerServiceSkillStep(2) == 1 and IsTrainerServiceSkillStep(1) == nil"
        )
        .unwrap());
    let row_of = |s: &mut UiScript, id: u32| -> i64 {
        s.eval::<i64>(&format!(
            "for i = 1, 3 do local l = GetTrainerServiceItemLink(i) \
             if l and GetTrainerServiceInfo(i) and select(3, GetTrainerServiceInfo(i)) == '{}' then return i end end return 0",
            if id == 2018 { "available" } else { "used" }
        ))
        .unwrap()
    };
    let (avail, used) = (row_of(&mut s, 2018), row_of(&mut s, 3100));
    assert_eq!(
        s.eval::<Option<String>>(&format!("return GetTrainerServiceStepIncrease({avail})"))
            .unwrap()
            .as_deref(),
        Some("Increases potential")
    );
    assert_eq!(
        s.eval::<Option<String>>(&format!("return GetTrainerServiceStepIncrease({used})"))
            .unwrap(),
        None,
        "a known step has no increase"
    );
    assert_eq!(
        s.eval::<String>(&format!("return GetTrainerServiceSkillLine({avail})"))
            .unwrap(),
        "Blacksmithing"
    );
}
