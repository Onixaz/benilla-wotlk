//! The trainer feed's tests: the laws, service resolution, the snapshot and the re-evaluator.

use super::*;
use benilla_formats::{
    ItemDisplay, ItemDisplayCatalog, SpellDisplay, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::TrainerTooltip;
use std::collections::HashMap;

/// An empty spell catalog, as before `Spell.dbc` loads.
fn empty_catalog() -> SpellCatalog {
    SpellCatalog::from_displays(HashMap::new())
}

/// What [`service_icon`] needs; a template is in flight unless a test seeds one.
use crate::items::TestDeps as Deps;

/// Header labels that name their key, so each assertion says which lookup the arm made.
fn probe_strings(key: &str) -> Option<String> {
    match key {
        "TRADESKILL_SERVICE_STEP" => Some("<STEP>".into()),
        "TRADESKILL_SERVICE_LEARN" => Some("<LEARN>".into()),
        "KNOWN_TALENTS_HEADER" => Some("<KNOWN>".into()),
        _ => None,
    }
}

/// [`resolve_service`] with no icon sources.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
) -> TrainerService {
    let deps = Deps::new();
    super::resolve_service(
        wire,
        trainer_type,
        spells,
        skill_lines,
        (0, 0),
        known,
        None,
        &deps.items,
        &deps.commands,
        &probe_strings,
    )
}

fn wire(spell: u32, state: u8, cost: u32, req_level: u8, req_skill: u32) -> TrainerSpell {
    TrainerSpell {
        spell,
        state,
        cost,
        can_learn_primary_prof: false,
        is_primary_prof_first_rank: false,
        req_level,
        req_skill,
        req_skill_value: if req_skill != 0 { 100 } else { 0 },
        req_spells: [0, 0, 0],
    }
}

/// [`snapshot`] with no icon sources.
fn snap(open: &TrainerOpen, spells: &SpellCatalog) -> Option<TrainerState> {
    let deps = Deps::new();
    snapshot(
        open,
        spells,
        None,
        (0, 0),
        &BTreeSet::new(),
        None,
        &deps.items,
        &deps.commands,
        &probe_strings,
    )
}

/// Wrapper 100 teaches 200 by a slot-0 `LEARN_SPELL`, 200 creates item 777, and 777's display 5
/// has the item art; every icon is distinct, so a wrong arm is named by the assertion.
fn icon_catalog() -> SpellCatalog {
    let wrapper = SpellDisplay {
        name: "Copper Shortsword".into(),
        icon: Some("WRAPPER".into()),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        name: "Copper Shortsword".into(),
        icon: Some("TAUGHT".into()),
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]))
}

/// The product item's template and `ItemDisplayInfo` row, landed.
fn landed_item(deps: &mut Deps) -> ItemDisplays {
    let mut t = crate::items::test_template("Copper Shortsword");
    t.display_info_id = 5;
    deps.items.insert_template(777, Some(t));
    ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::from([(
        5,
        ItemDisplay {
            icon: Some("ITEM".into()),
            ..Default::default()
        },
    )])))
}

fn icon_of(
    spells: &SpellCatalog,
    trainer_type: u32,
    wire_spell: u32,
    land: bool,
) -> Option<String> {
    let mut deps = Deps::new();
    let icons = land.then(|| landed_item(&mut deps));
    service_icon(
        wire_spell,
        trainer_type,
        spells,
        icons.as_ref(),
        &deps.items,
        &deps.commands,
    )
}

#[test]
fn trainer_icon_substitutes_the_product_only_at_a_tradeskill_trainer() {
    let spells = icon_catalog();

    // All three gates pass and the template landed: the created item's icon.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM")
    );

    // Gate 1 fails (a class trainer): the wire spell's own icon, never the taught spell's.
    assert_eq!(
        icon_of(&spells, 0, 100, true).as_deref(),
        Some("WRAPPER"),
        "a class trainer serves the wrapper's own icon, not the taught spell's"
    );

    // Gate 2 fails: 200 creates an item but has no learn effect, so its own icon.
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 200, true).as_deref(),
        Some("TAUGHT"),
        "200 IS the wire spell here, so its own icon is the right answer"
    );

    // A spell with no record: nil, where every failure ends (`0x4d911c`).
    assert_eq!(icon_of(&spells, TRAINER_TYPE_TRADESKILL, 999, true), None);
}

#[test]
fn trainer_icon_falls_back_when_the_taught_spell_makes_no_item() {
    let wrapper = SpellDisplay {
        icon: Some("WRAPPER".into()),
        effects: [SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_trigger_spell: [200, 0, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        icon: Some("TAUGHT".into()),
        ..Default::default() // effect_item_type all zero
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]));
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("WRAPPER")
    );
}

/// `LEARN_PET_SPELL` (57) counts like `LEARN_SPELL` (36), in any slot (`0x4d8ff5`/`0x4d8ffa`).
#[test]
fn trainer_icon_scans_every_effect_slot_for_either_learn_effect() {
    let wrapper = SpellDisplay {
        icon: Some("WRAPPER".into()),
        effects: [0, SPELL_EFFECT_LEARN_PET_SPELL, 0],
        effect_trigger_spell: [0, 200, 0],
        ..Default::default()
    };
    let taught = SpellDisplay {
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(100, wrapper), (200, taught)]));
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM"),
        "a pet-learn effect in slot 1 substitutes just the same"
    );
}

/// The client pushes nil here and repaints from the cache callback (`0x4d9140`).
#[test]
fn trainer_icon_is_nil_until_the_product_template_lands_and_asks_once() {
    let spells = icon_catalog();
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));

    let first = service_icon(
        100,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert_eq!(first, None, "no template yet → nil, not the wrapper's icon");
    assert_eq!(
        deps.queried_entries(),
        vec![777],
        "and the created item's template was asked for"
    );

    let _ = service_icon(
        100,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert!(
        deps.queried_entries().is_empty(),
        "asked once, not per frame"
    );
}

/// On the shipped `Spell.dbc`: 2756 is the wrapper a Blacksmithing trainer sends, 2739 the recipe
/// it teaches, 2847 the sword it makes. Skips without client data.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn trainer_icon_on_real_data_reaches_for_the_crafted_item() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    // The two textures a wrong law produces.
    let wrapper_icon = spells.get(2756).unwrap().icon.clone();
    let recipe_icon = spells.get(2739).unwrap().icon.clone();
    assert_eq!(
        recipe_icon.as_deref(),
        Some("Interface\\Icons\\Spell_Shadow_SealOfKings"),
        "the blue crown on the row IS the recipe spell's own icon"
    );

    // A tradeskill trainer: gate 3 fires for item 2847 and the icon waits on its template.
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));
    let icon = service_icon(
        2756,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert_eq!(
        icon, None,
        "waiting on the item template, not painting the crown"
    );
    assert_eq!(
        deps.queried_entries(),
        vec![2847],
        "the law reached for the crafted sword's template"
    );

    // A class trainer with the same wire spell: the wrapper's own icon, not the recipe's.
    let deps = Deps::new();
    let class_icon = service_icon(2756, 0, &spells, None, &deps.items, &deps.commands);
    assert_eq!(class_icon, wrapper_icon);
    assert_ne!(
        class_icon, recipe_icon,
        "the taught spell's icon is never painted at a trainer"
    );
}

/// On real data: the warrior wrapper 1605 is not in `SkillLineAbility`, so grouping hops to the
/// taught 78 (Arms, 26) while the buy id stays 1605. Skips without client data.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn resolve_hops_the_learn_wrapper_to_the_taught_ability() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    let svc = resolve_service(
        &wire(1605, trainer_spell_state::GREEN, 10, 1, 0),
        0,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(svc.spell_id, 1605, "the buy id stays the wire wrapper");
    assert_eq!(
        svc.group_key, 26,
        "grouped under the taught ability's Arms line"
    );
    assert_eq!(svc.group_name, "Arms");
    assert_eq!(
        svc.name.as_deref(),
        Some("Heroic Strike"),
        "the WIRE spell's own name — which here happens to match the taught one"
    );
}

/// Only a profession-learn row tells the two apart: 2020 is "Apprentice Blacksmith" with no rank,
/// teaching 2018 "Blacksmithing"/"Apprentice". Skips without client data.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn display_name_is_the_wire_spell_not_the_taught_one() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    // A Blacksmithing trainer's learn row as the wire carries it (reqLevel 5, no skill gate).
    let svc = resolve_service(
        &wire(2020, trainer_spell_state::GREEN, 9, 5, 0),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(svc.name.as_deref(), Some("Apprentice Blacksmith"));
    assert_eq!(svc.subtext.as_deref(), None);
    assert_ne!(
        spells.get(2020).map(|d| d.name.as_str()),
        spells.get(2018).map(|d| d.name.as_str()),
        "the two names really do differ on the shipped data — the test is not vacuous"
    );

    // The group law puts it first: 2020 carries Effect 44 `SKILL_STEP`, a recipe wrapper does not.
    assert_eq!(svc.group_key, 1);
    assert_eq!(svc.group_name, "<STEP>");
    let recipe = resolve_service(
        &wire(2743, trainer_spell_state::RED, 50, 0, 164),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &BTreeSet::new(),
    );
    assert_eq!(recipe.group_key, 2);
    assert_eq!(recipe.group_name, "<LEARN>");
    assert_eq!(recipe.name.as_deref(), Some("Copper Chain Pants"));
}

#[test]
fn category_maps_the_wire_state_byte() {
    assert_eq!(
        category(trainer_spell_state::GREEN),
        TrainerServiceCategory::Available
    );
    assert_eq!(
        category(trainer_spell_state::RED),
        TrainerServiceCategory::Unavailable
    );
    assert_eq!(
        category(trainer_spell_state::GRAY),
        TrainerServiceCategory::Used
    );
    // An unexpected value is gated, never learnable.
    assert_eq!(category(99), TrainerServiceCategory::Unavailable);
}

#[test]
fn resolve_reads_cost_state_and_gates_with_no_catalog() {
    // No catalog: names and icon nil, the wire fields intact, the skill name a fallback.
    let spells = empty_catalog();
    let mut w = wire(2018, trainer_spell_state::RED, 1000, 20, 164);
    w.req_spells = [78, 0, 0];
    let svc = resolve_service(&w, TRAINER_TYPE_TRADESKILL, &spells, None, &BTreeSet::new());
    assert_eq!(svc.spell_id, 2018);
    assert!(svc.name.is_none(), "no catalog → name in flight");
    assert_eq!(svc.cost, 1000);
    assert_eq!(svc.category, TrainerServiceCategory::Unavailable);
    assert_eq!(svc.level_req, 20);
    // The skill gate follows the category; the ability gate is unmet as the player lacks 78.
    assert_eq!(
        svc.skill_req,
        Some(TrainerSkillReq {
            name: "Skill 164".to_string(),
            rank: 100,
            met: false,
        })
    );
    assert_eq!(
        svc.ability_reqs,
        vec![TrainerAbilityReq {
            name: "Spell 78".to_string(),
            met: false,
        }]
    );
    assert!(svc.is_trade_skill, "trainer_type 2 → tradeskill");
    assert!(svc.prof_first_rank == w.is_primary_prof_first_rank);
    // No catalog at a tradeskill trainer: the learn group, as type 2 never drops a row.
    assert_eq!(svc.group_key, 2);
    assert_eq!(svc.group_name, "<LEARN>");
    // No skill gate when req_skill is 0.
    let plain = resolve_service(
        &wire(78, trainer_spell_state::GREEN, 50, 5, 0),
        0,
        &spells,
        None,
        &BTreeSet::new(),
    );
    assert_eq!(plain.skill_req, None);
    assert!(plain.ability_reqs.is_empty());
    assert!(!plain.is_trade_skill);
}

/// `GetTrainerServiceAbilityReq` (`0x4d96e0`): a level-gated service's known prerequisite still
/// reads met.
#[test]
fn ability_req_met_tracks_known_spells_not_the_service_category() {
    let spells = empty_catalog();
    let mut w = wire(845, trainer_spell_state::RED, 100, 20, 0); // unavailable (gated by level)
    w.req_spells = [78, 0, 0]; // requires Heroic Strike (78)

    let unknown = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert_eq!(unknown.category, TrainerServiceCategory::Unavailable);
    assert!(!unknown.ability_reqs[0].met, "prereq unknown → unmet");

    let known: BTreeSet<u32> = [78].into_iter().collect();
    let learned = resolve_service(&w, 0, &spells, None, &known);
    assert_eq!(learned.category, TrainerServiceCategory::Unavailable);
    assert!(
        learned.ability_reqs[0].met,
        "prereq known → met, decoupled from the unavailable service"
    );
}

/// On real data the prerequisite reads as the client's `"%s (%s)"`: 78 is Heroic Strike Rank 1.
/// Skips without client data.
#[test]
fn ability_req_shows_the_required_rank_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    let mut w = wire(846, trainer_spell_state::RED, 100, 20, 0);
    w.req_spells = [78, 0, 0]; // Heroic Strike Rank 1

    let known: BTreeSet<u32> = [78].into_iter().collect();
    let svc = resolve_service(&w, 0, &spells, None, &known);
    assert_eq!(
        svc.ability_reqs,
        vec![TrainerAbilityReq {
            name: "Heroic Strike (Rank 1)".to_string(),
            met: true,
        }],
        "the prereq shows its rank and reads met because the player knows it"
    );
    let svc = resolve_service(&w, 0, &spells, None, &BTreeSet::new());
    assert!(!svc.ability_reqs[0].met);
    assert_eq!(svc.ability_reqs[0].name, "Heroic Strike (Rank 1)");
}

#[test]
fn snapshot_is_none_when_closed_and_lists_services_when_open() {
    let spells = empty_catalog();
    let mut open = TrainerOpen::default();
    assert!(snap(&open, &spells).is_none());

    open.open(
        0x42,
        0,
        vec![
            wire(78, trainer_spell_state::GREEN, 100, 10, 0),
            wire(79, trainer_spell_state::GRAY, 200, 0, 0),
        ],
        "Learn from me.".into(),
    );
    let state = snap(&open, &spells).expect("open → Some");
    assert_eq!(state.greeting, "Learn from me.");
    assert_eq!(state.services.len(), 2);
    assert_eq!(
        state.services[0].category,
        TrainerServiceCategory::Available
    );
    assert_eq!(state.services[1].category, TrainerServiceCategory::Used);

    open.clear();
    assert!(snap(&open, &spells).is_none());
    assert_eq!(open.trainer, None);
}
/// A wrapper spell whose slot `slot` carries `effect` and triggers `trigger`.
fn wrapper(effect: u32, slot: usize, trigger: u32) -> SpellDisplay {
    let mut d = SpellDisplay {
        name: "Learn Something".into(),
        icon: Some("WRAPPER".into()),
        ..Default::default()
    };
    d.effects[slot] = effect;
    d.effect_trigger_spell[slot] = trigger;
    d
}

/// A plain taught ability: no tradeskill bit, so the spell route.
fn taught_ability() -> SpellDisplay {
    SpellDisplay {
        name: "Heroic Strike".into(),
        ..Default::default()
    }
}

/// A taught recipe: the tradeskill bit, `Effect[0] == CREATE_ITEM`, and a distinct product in
/// every slot so a wrong-slot read is named by the assertion.
fn taught_recipe() -> SpellDisplay {
    SpellDisplay {
        name: "Copper Shortsword".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [2847, 3333, 4444],
        ..Default::default()
    }
}

/// A taught spell with the tradeskill bit shows its created item's tooltip: 2756 → 2739 → 2847.
#[test]
fn tooltip_takes_the_item_arm_when_the_taught_spell_is_a_recipe() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2756, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 2739)),
        (2739, taught_recipe()),
    ]));
    assert_eq!(service_tooltip(2756, &spells), TrainerTooltip::Item(2847));
}

#[test]
fn tooltip_hops_to_the_taught_spell_where_the_icon_pins_the_wire() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 200)),
        (200, taught_ability()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: false,
        },
    );
    // The icon law on the same row paints the wire spell's art.
    let deps = Deps::new();
    assert_eq!(
        super::service_icon(100, 0, &spells, None, &deps.items, &deps.commands),
        Some("WRAPPER".into()),
    );
}

/// `altCaster` gates the builder's totem and reagent blocks.
#[test]
fn tooltip_sets_alt_caster_only_for_a_pet_learn_wrapper() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, wrapper(SPELL_EFFECT_LEARN_PET_SPELL, 0, 200)),
        (200, taught_ability()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: true,
        },
    );
}

/// Slot 0's trigger is not in the catalog, so slot 1 wins.
#[test]
fn tooltip_scan_advances_past_an_unresolvable_trigger() {
    let mut w = wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 999_999);
    w.effects[1] = SPELL_EFFECT_LEARN_SPELL;
    w.effect_trigger_spell[1] = 200;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, w), (200, taught_ability())]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: false,
        },
    );
}

/// `Attributes & 0x20` alone picks the item arm (`0x52e610`'s redirect at `0x52e6d2`);
/// `Effect[0] == 24` picks the matched slot, otherwise slot 0.
#[test]
fn tooltip_item_slot_follows_the_effect_gate_not_the_attribute_gate() {
    // The learn effect in slot 1, slot 0 empty. Built per use: SpellDisplay is not Clone.
    let slot1_wrapper = || {
        let mut w = wrapper(SPELL_EFFECT_LEARN_SPELL, 1, 200);
        w.effects[0] = 0;
        w
    };
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, slot1_wrapper()),
        (200, taught_recipe()),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Item(3333),
        "Effect[0]==CREATE_ITEM -> EffectItemType[matched slot]"
    );

    // The bit without Effect[0]==24: still the item arm, off slot 0.
    let mut odd = taught_recipe();
    odd.effects[0] = 6;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, slot1_wrapper()), (200, odd)]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Item(2847),
        "the bit alone still routes to the item builder, from slot 0"
    );
}

/// No learn effect, or no `Spell.dbc`: the wire spell.
#[test]
fn tooltip_falls_back_to_the_wire_spell() {
    let spells = SpellCatalog::from_displays(HashMap::from([(100, taught_ability())]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 100,
            alt_caster: false,
        },
    );
    assert_eq!(
        service_tooltip(100, &empty_catalog()),
        TrainerTooltip::Spell {
            spell_id: 100,
            alt_caster: false,
        },
    );
}

/// The builder resets per list packet (`0x4d75d9`), and an open window repaints through the
/// re-evaluator (`0x4d7d40`), never a second list, so every list opens and resets.
#[test]
fn every_list_packet_begins_a_window_session() {
    const TRAINER: u64 = 0xabc;
    let list = || vec![wire(1, trainer_spell_state::GREEN, 10, 1, 0)];
    let mut open = TrainerOpen::default();

    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(open.fresh_list, "a first list opens the window: reset");
    open.fresh_list = false; // the feed consumes it

    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(
        open.fresh_list,
        "the same trainer's list again is a session start again"
    );

    // A pending re-derivation never survives a list: the packet carries the server's own states.
    open.fresh_list = false;
    open.trigger_re_derive();
    assert!(open.re_derive);
    open.open(TRAINER, 0, list(), "Greetings".into());
    assert!(
        !open.re_derive,
        "a fresh list is the server's states — nothing pending against it"
    );

    // A trigger with no window open does nothing (`0x4d7d46`'s early-out).
    open.clear();
    open.trigger_re_derive();
    assert!(!open.re_derive);
}

mod re_derive {
    //! The re-evaluator, one test per leg of `0x596b00`, on synthetic catalogs.
    use super::super::reeval::{re_derive, PlayerView, SkillSlot};
    use super::*;
    use benilla_formats::{LearnEffect, SlaInfo};

    const BLACKSMITHING: u32 = 164;
    /// A profession opener: teaches 2018 and steps Blacksmithing to 1.
    const OPENER: u32 = 2020;
    const TAUGHT: u32 = 2018;
    /// Two ranks of a class ability, each its own trainer row: RANK1's next rank is RANK2.
    const RANK1: u32 = 100;
    const RANK2: u32 = 101;
    /// A learn wrapper teaching RANK1.
    const WRAPPER: u32 = 500;
    /// A wrapper whose second slot is some other effect.
    const WRAPPER_THEN_OTHER: u32 = 501;
    /// Another effect ahead of a learn slot.
    const OTHER_THEN_WRAPPER: u32 = 502;
    /// A pet-learn row: no player learn slot, so it stands for itself.
    const PET_LEARN: u32 = 700;
    /// A record with no effect at all.
    const EFFECTLESS: u32 = 800;

    fn display(effects: [u32; 3]) -> SpellDisplay {
        SpellDisplay {
            effects,
            ..Default::default()
        }
    }

    fn catalog() -> SpellCatalog {
        // Every row has a record: the admission gate (`0x596b9c`) keeps a row without one.
        let displays = HashMap::from([
            (OPENER, display([SPELL_EFFECT_LEARN_SPELL, 44, 0])),
            (RANK1, display([6, 2, 6])),
            (RANK2, display([6, 2, 6])),
            (WRAPPER, display([SPELL_EFFECT_LEARN_SPELL, 0, 0])),
            (
                WRAPPER_THEN_OTHER,
                display([SPELL_EFFECT_LEARN_SPELL, 6, 0]),
            ),
            (
                OTHER_THEN_WRAPPER,
                display([6, SPELL_EFFECT_LEARN_SPELL, 0]),
            ),
            (PET_LEARN, display([SPELL_EFFECT_LEARN_PET_SPELL, 0, 0])),
            (EFFECTLESS, display([0, 0, 0])),
        ]);
        let none = LearnEffect::None;
        SpellCatalog::from_displays_and_effects(
            displays,
            HashMap::from([
                (
                    OPENER,
                    [
                        LearnEffect::Spell(TAUGHT),
                        LearnEffect::SkillStep {
                            skill: BLACKSMITHING,
                            step: 1,
                        },
                        none,
                    ],
                ),
                (WRAPPER, [LearnEffect::Spell(RANK1), none, none]),
                (
                    WRAPPER_THEN_OTHER,
                    [LearnEffect::Spell(RANK1), LearnEffect::Other, none],
                ),
                (
                    OTHER_THEN_WRAPPER,
                    [LearnEffect::Other, LearnEffect::Spell(TAUGHT), none],
                ),
            ]),
        )
    }

    fn skill_lines() -> SkillLineCatalog {
        let row = |skill_id, forward| SlaInfo {
            skill_id,
            req_skill_value: 0,
            forward_spell_id: forward,
            trivial_low: 0,
            trivial_high: 0,
        };
        SkillLineCatalog::from_abilities([(RANK1, row(1, RANK2)), (RANK2, row(1, 0))])
    }

    fn service(spell: u32) -> TrainerSpell {
        // The wire state is RED on purpose: every case below must ignore it (`0x596bad`).
        let mut w = wire(spell, trainer_spell_state::RED, 10, 1, 0);
        w.req_skill_value = 0;
        w
    }

    fn player() -> PlayerView {
        PlayerView {
            known: BTreeSet::new(),
            level: 10,
            skills: Vec::new(),
        }
    }

    fn knowing(spells: &[u32]) -> PlayerView {
        PlayerView {
            known: spells.iter().copied().collect(),
            ..player()
        }
    }

    fn state(wire: &TrainerSpell, player: &PlayerView) -> u8 {
        re_derive(wire, &catalog(), &skill_lines(), player)
    }

    #[test]
    fn the_wire_state_is_discarded_and_a_met_service_reads_green() {
        assert_eq!(
            state(&service(RANK1), &player()),
            trainer_spell_state::GREEN
        );
    }

    /// A class trainer lists the ability itself; once learned it stands for itself and grays, so
    /// the bought row leaves the default filter on the learn's own re-evaluation.
    #[test]
    fn a_class_ability_the_player_just_learned_reads_gray() {
        let mut open = TrainerOpen::default();
        open.open(
            0xabc,
            0,
            vec![service(RANK1), service(RANK2)],
            String::new(),
        );
        super::super::reeval::re_derive_all(
            &mut open.services,
            &catalog(),
            &skill_lines(),
            &knowing(&[RANK1]),
        );
        assert_eq!(open.services[0].state, trainer_spell_state::GRAY);
        assert_eq!(
            category(open.services[0].state),
            TrainerServiceCategory::Used
        );
        assert_eq!(open.services[1].state, trainer_spell_state::GREEN);
    }

    #[test]
    fn a_higher_known_rank_counts_as_known() {
        // Rank 2 in the book, rank 1 offered: `Spell_KnownHigherRank` (`0x726160`) says known.
        assert_eq!(
            state(&service(RANK1), &knowing(&[RANK2])),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn a_wrapper_grays_on_the_spell_it_teaches() {
        assert_eq!(
            state(&service(WRAPPER), &knowing(&[RANK1])),
            trainer_spell_state::GRAY
        );
        assert_eq!(
            state(&service(WRAPPER), &knowing(&[WRAPPER])),
            trainer_spell_state::GREEN,
            "the wrapper's own id is never counted"
        );
    }

    /// The step is counted like the taught spell (`0x596bfc`), so an opener grays only with both.
    #[test]
    fn an_opener_grays_only_with_its_spell_known_and_its_step_held() {
        let step_held = |mut p: PlayerView| {
            p.skills.push(SkillSlot {
                skill_id: BLACKSMITHING,
                step: 1,
                value_plus_perm: 1,
            });
            p
        };
        assert_eq!(
            state(&service(OPENER), &knowing(&[TAUGHT])),
            trainer_spell_state::GREEN
        );
        assert_eq!(
            state(&service(OPENER), &step_held(player())),
            trainer_spell_state::GREEN
        );
        assert_eq!(
            state(&service(OPENER), &step_held(knowing(&[TAUGHT]))),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn another_effect_after_a_taught_spell_is_skipped() {
        assert_eq!(
            state(&service(WRAPPER_THEN_OTHER), &knowing(&[RANK1])),
            trainer_spell_state::GRAY
        );
    }

    /// The row's own spell ends the loop (`0x596ce8`): the learn slot after it is never read.
    #[test]
    fn another_effect_first_stands_for_the_row_and_ends_the_loop() {
        assert_eq!(
            state(&service(OTHER_THEN_WRAPPER), &knowing(&[TAUGHT])),
            trainer_spell_state::GREEN
        );
        assert_eq!(
            state(
                &service(OTHER_THEN_WRAPPER),
                &knowing(&[OTHER_THEN_WRAPPER])
            ),
            trainer_spell_state::GRAY
        );
    }

    #[test]
    fn a_pet_learn_row_stands_for_itself() {
        assert_eq!(
            state(&service(PET_LEARN), &player()),
            trainer_spell_state::GREEN
        );
        assert_eq!(
            state(&service(PET_LEARN), &knowing(&[PET_LEARN])),
            trainer_spell_state::GRAY
        );
    }

    /// Nothing counted is never "all known" (`0x596d03`).
    #[test]
    fn a_record_with_no_effect_never_grays() {
        assert_eq!(
            state(&service(EFFECTLESS), &knowing(&[EFFECTLESS])),
            trainer_spell_state::GREEN
        );
    }

    #[test]
    fn a_skill_line_below_the_requirement_reads_red() {
        let mut w = service(WRAPPER);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 75;
        let mut p = player();
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 40,
        });
        assert_eq!(state(&w, &p), trainer_spell_state::RED);
        // Met (value plus permanent bonus reaches it): the leg leaves the state alone.
        p.skills[0].value_plus_perm = 75;
        assert_eq!(state(&w, &p), trainer_spell_state::GREEN);
    }

    /// No slot for the line writes nothing (`0x596d95`), so green where the server sends red.
    #[test]
    fn a_skill_line_the_player_lacks_entirely_reads_green() {
        let mut w = service(WRAPPER);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 75;
        assert_eq!(state(&w, &player()), trainer_spell_state::GREEN);
    }

    #[test]
    fn an_unknown_required_ability_reads_red() {
        let mut w = service(OPENER);
        w.req_spells = [RANK1, 0, 0];
        assert_eq!(state(&w, &player()), trainer_spell_state::RED);
        // Known, or known at a higher rank, satisfies it.
        assert_eq!(state(&w, &knowing(&[RANK1])), trainer_spell_state::GREEN);
        assert_eq!(state(&w, &knowing(&[RANK2])), trainer_spell_state::GREEN);
    }

    #[test]
    fn a_level_requirement_above_one_gates_on_the_players_level() {
        let mut w = service(WRAPPER);
        w.req_level = 20;
        assert_eq!(state(&w, &player()), trainer_spell_state::RED);
        let mut p = player();
        p.level = 20;
        assert_eq!(state(&w, &p), trainer_spell_state::GREEN);
        // `req_level <= 1` is not a gate at all (`0x596fa3`).
        w.req_level = 1;
        p.level = 0;
        assert_eq!(state(&w, &p), trainer_spell_state::GREEN);
    }

    #[test]
    fn a_known_row_stays_gray_whatever_its_other_gates_say() {
        // Every later leg wants the state still 0 (`0x596d2b`).
        let mut w = service(RANK1);
        w.req_skill = BLACKSMITHING;
        w.req_skill_value = 300;
        w.req_level = 60;
        w.req_spells = [OPENER, 0, 0];
        let mut p = knowing(&[RANK1]);
        p.skills.push(SkillSlot {
            skill_id: BLACKSMITHING,
            step: 1,
            value_plus_perm: 1,
        });
        assert_eq!(state(&w, &p), trainer_spell_state::GRAY);
    }

    /// The admission gate (`0x596b9c`).
    #[test]
    fn a_row_without_a_spell_record_keeps_its_wire_state() {
        let w = wire(999_999, trainer_spell_state::RED, 10, 1, 0);
        assert_eq!(state(&w, &player()), trainer_spell_state::RED);
        let w = wire(999_999, trainer_spell_state::GRAY, 10, 1, 0);
        assert_eq!(state(&w, &player()), trainer_spell_state::GRAY);
    }
}

/// A mistyped key yields an empty header, which [`probe_strings`] cannot catch. Skips without
/// client data.
#[test]
fn the_group_header_keys_resolve_in_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let vm = benilla_ui::script::UiScript::new().expect("VM");
    vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    for key in [
        "TRADESKILL_SERVICE_STEP",
        "TRADESKILL_SERVICE_LEARN",
        "KNOWN_TALENTS_HEADER",
    ] {
        let text = vm.lua().globals().get::<String>(key).unwrap_or_default();
        assert!(!text.is_empty(), "{key} missing");
    }
}
