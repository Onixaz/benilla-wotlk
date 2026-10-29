//! The trainer feed's tests: the laws, service resolution, the snapshot and the re-evaluator.

use super::*;
use benilla_formats::{
    ItemDisplay, ItemDisplayCatalog, SpellDisplay, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::{TrainerAbilityReq, TrainerServiceCategory, TrainerTooltip};
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
        "TRADESKILL_SERVICE_PASSIVE" => Some("<PASSIVE>".into()),
        "INCREASE_POTENTIAL" => Some("Increases potential in %s by %d".into()),
        _ => None,
    }
}

/// A warrior who knows `known`, with no skills.
fn warrior(known: &[u32]) -> reeval::PlayerView {
    reeval::PlayerView {
        known: known.iter().copied().collect(),
        level: 10,
        skills: Vec::new(),
    }
}

/// [`resolve_service`] with no icon sources, for a human warrior.
fn resolve_service(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    player: &reeval::PlayerView,
) -> TrainerService {
    let deps = Deps::new();
    super::resolve_service(
        wire,
        &ResolveCtx {
            trainer_type,
            spells,
            skill_lines,
            race_class: (1, 1),
            player,
            icons: None,
            items: &deps.items,
            commands: &deps.commands,
            descriptions: &HashMap::from([(78, Some("Teaches a strike.".to_string()))]),
            get: &probe_strings,
        },
    )
}

fn wire(spell: u32, state: u8, cost: u32, req_level: u8, req_skill: u32) -> TrainerSpell {
    TrainerSpell {
        spell,
        state,
        cost,
        talent_cost: 0,
        profession_cost: 0,
        req_level,
        req_skill,
        req_skill_value: if req_skill != 0 { 100 } else { 0 },
        req_spells: [0, 0, 0],
    }
}

/// [`snapshot`] with no icon sources.
fn snap(open: &TrainerOpen, spells: &SpellCatalog) -> Option<TrainerState> {
    let deps = Deps::new();
    let player = warrior(&[]);
    snapshot(
        open,
        &ResolveCtx {
            trainer_type: open.trainer_type,
            spells,
            skill_lines: None,
            race_class: (0, 0),
            player: &player,
            icons: None,
            items: &deps.items,
            commands: &deps.commands,
            descriptions: &HashMap::from([(78, Some("Teaches a strike.".to_string()))]),
            get: &probe_strings,
        },
    )
}

/// Recipe 100 creates item 777 through slot 0; 777's display 5 has the item art, and every icon is
/// distinct, so a wrong arm is named by the assertion.
fn icon_catalog() -> SpellCatalog {
    let recipe = SpellDisplay {
        name: "Copper Shortsword".into(),
        icon: Some("RECIPE".into()),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [777, 0, 0],
        ..Default::default()
    };
    // The product in slot 1 only: slot 0 is the one the icon reads.
    let off_slot = SpellDisplay {
        icon: Some("OFF_SLOT".into()),
        effects: [6, SPELL_EFFECT_CREATE_ITEM, 0],
        effect_item_type: [0, 777, 0],
        ..Default::default()
    };
    SpellCatalog::from_displays(HashMap::from([(100, recipe), (200, off_slot)]))
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

/// `GetTrainerServiceIcon` (`0x595150`): a create effect in slot 0 at a tradeskill trainer.
#[test]
fn trainer_icon_substitutes_the_product_only_at_a_tradeskill_trainer() {
    let spells = icon_catalog();
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 100, true).as_deref(),
        Some("ITEM")
    );
    assert_eq!(
        icon_of(&spells, 0, 100, true).as_deref(),
        Some("RECIPE"),
        "a class trainer serves the spell's own icon"
    );
    assert_eq!(
        icon_of(&spells, TRAINER_TYPE_TRADESKILL, 200, true).as_deref(),
        Some("OFF_SLOT"),
        "a create effect past slot 0 is not read"
    );
    assert_eq!(icon_of(&spells, TRAINER_TYPE_TRADESKILL, 999, true), None);
}

/// The client pushes nil here and repaints from the cache callback.
#[test]
fn trainer_icon_is_nil_until_the_product_template_lands_and_asks_once() {
    let spells = icon_catalog();
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));
    let icon = || {
        service_icon(
            100,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            Some(&icons),
            &deps.items,
            &deps.commands,
        )
    };
    assert_eq!(
        icon(),
        None,
        "no template yet, so nil, not the spell's icon"
    );
    assert_eq!(deps.queried_entries(), vec![777]);
    let _ = icon();
    assert!(
        deps.queried_entries().is_empty(),
        "asked once, not per frame"
    );
}

/// On 12340 a blacksmith lists the recipe itself: 2739 "Copper Shortsword" creates item 2847 in
/// slot 0, so the icon and the item link both wait on 2847's template. Skips without client data.
#[test]
fn trainer_icon_and_link_on_real_data_reach_for_the_crafted_item() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let deps = Deps::new();
    let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::new()));
    let icon = service_icon(
        2739,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&icons),
        &deps.items,
        &deps.commands,
    );
    assert_eq!(icon, None, "waiting on the item template");
    assert_eq!(deps.queried_entries(), vec![2847]);
    assert_eq!(
        service_item_link(
            2739,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            &deps.items,
            &deps.commands
        ),
        None
    );
    assert_eq!(
        service_tooltip(2739, &spells),
        TrainerTooltip::Spell {
            spell_id: 2739,
            alt_caster: false,
        },
        "a recipe with no learn slot shows its own spell"
    );
    // A class trainer: the spell's own icon.
    let class_icon = service_icon(2739, 0, &spells, None, &deps.items, &deps.commands);
    assert_eq!(class_icon, spells.get(2739).unwrap().icon.clone());
}

#[test]
fn the_item_link_is_the_cached_product_at_a_tradeskill_trainer_alone() {
    let spells = icon_catalog();
    let mut deps = Deps::new();
    let _ = landed_item(&mut deps);
    let link = |t| service_item_link(100, t, &spells, &deps.items, &deps.commands);
    assert_eq!(
        link(TRAINER_TYPE_TRADESKILL).as_deref(),
        Some("|cffffffff|Hitem:777:0:0:0|h[Copper Shortsword]|h|r")
    );
    assert_eq!(link(0), None);
}

/// On real data a class trainer lists Heroic Strike (78) itself, under Arms (26) for a warrior.
/// Skips without client data.
#[test]
fn a_class_ability_groups_under_its_own_skill_line() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    let svc = resolve_service(
        &wire(78, trainer_spell_state::GREEN, 10, 1, 0),
        0,
        &spells,
        Some(&skills),
        &warrior(&[]),
    );
    assert_eq!((svc.group_key, svc.group_name.as_str()), (26, "Arms"));
    assert_eq!(svc.skill_line.as_deref(), Some("Arms"));
    assert_eq!(svc.name.as_deref(), Some("Heroic Strike"));
    assert_eq!(svc.subtext.as_deref(), Some("Rank 1"));
    assert!(!svc.skill_step);
}

/// A profession opener on 12340: 2020 "Apprentice Blacksmith" steps Blacksmithing (effect 44 in
/// slot 1), so it files under the step header and answers the step getters. Skips without data.
#[test]
fn a_profession_opener_files_under_the_step_header() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let skills = benilla_formats::load_skill_line_catalog(&mut chain).expect("load skill lines");

    let mut w = wire(2020, trainer_spell_state::GREEN, 9, 5, 0);
    w.profession_cost = 1;
    let svc = resolve_service(
        &w,
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &warrior(&[]),
    );
    assert_eq!(svc.name.as_deref(), Some("Apprentice Blacksmith"));
    assert_eq!((svc.group_key, svc.group_name.as_str()), (1, "<STEP>"));
    assert!(svc.skill_step);
    assert_eq!(svc.skill_line.as_deref(), Some("Blacksmithing"));
    assert!(
        svc.step_increase
            .as_deref()
            .is_some_and(|t| t.starts_with("Increases potential in Blacksmithing by ")),
        "{:?}",
        svc.step_increase
    );
    assert_eq!(svc.profession_cost, 1, "the wire word rides through raw");

    let recipe = resolve_service(
        &wire(2739, trainer_spell_state::RED, 50, 0, 164),
        TRAINER_TYPE_TRADESKILL,
        &spells,
        Some(&skills),
        &warrior(&[]),
    );
    assert_eq!(
        (recipe.group_key, recipe.group_name.as_str()),
        (2, "<LEARN>")
    );
    assert_eq!(recipe.skill_line.as_deref(), Some("Blacksmithing"));
    assert_eq!(recipe.step_increase, None);
}

#[test]
fn a_passive_tradeskill_service_files_under_the_passive_header() {
    let passive = SpellDisplay {
        attributes: 0x40,
        effects: [44, 0, 0],
        ..Default::default()
    };
    let spells = SpellCatalog::from_displays(HashMap::from([(300, passive)]));
    assert_eq!(
        service_group(
            300,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            None,
            (1, 1),
            &probe_strings
        ),
        (3, "<PASSIVE>".to_string()),
        "the passive bit outranks the skill step"
    );
    assert_eq!(
        service_group(
            999,
            TRAINER_TYPE_TRADESKILL,
            &spells,
            None,
            (1, 1),
            &probe_strings
        ),
        (2, "<LEARN>".to_string()),
        "no record: the learn header"
    );
}

/// `GetTrainerServiceDescription` (`0x5959d0`): the parsed spell text, else a recipe's product
/// description from the item cache.
#[test]
fn the_description_is_the_spell_text_else_the_recipes_product() {
    let spells = icon_catalog();
    let w = wire(78, trainer_spell_state::GREEN, 10, 1, 0);
    let svc = resolve_service(&w, 0, &spells, None, &warrior(&[]));
    assert_eq!(svc.description.as_deref(), Some("Teaches a strike."));

    let mut deps = Deps::new();
    let mut t = crate::items::test_template("Copper Shortsword");
    t.description = "A sword.".into();
    deps.items.insert_template(777, Some(t));
    assert_eq!(
        service_item_description(100, &spells, &deps.items, &deps.commands).as_deref(),
        Some("A sword."),
        "recipe 100 has no description of its own"
    );
    assert_eq!(
        service_item_description(200, &spells, &deps.items, &deps.commands),
        None,
        "no tradeskill bit, no product leg"
    );
}

/// Astral Recall (556) is the one trainer row whose description names the bind point, `$z`.
/// Skips without client data.
#[test]
fn the_description_names_the_bind_point() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");
    let durations = benilla_formats::SpellDurationCatalog::default();
    let radii = benilla_formats::SpellRadiusCatalog::default();
    let ctx = benilla_formats::TokenContext {
        durations: &durations,
        radii: &radii,
        ranges: None,
        variables: None,
        skill: &|_| 0,
        lookup: &|id| spells.get(id),
        mods: None,
        unmodified_points: false,
        gender: &|| 0,
        home_area: &|| Some("Stormwind City"),
        caster: None,
        global: &|_| None,
        printf: &crate::ui_script::token_printf,
    };
    let mut texts = HashMap::new();
    let services = [wire(556, trainer_spell_state::GREEN, 0, 30, 0)];
    describe_services(&mut texts, &services, &spells, &ctx);
    let text = texts
        .get(&556)
        .cloned()
        .flatten()
        .expect("Astral Recall has a description");
    assert!(text.contains("Stormwind City"), "{text}");
    assert!(!text.contains("$z"), "{text}");
}

/// A landed product record moves only the pane's fields, which `TRAINER_DESCRIPTION_UPDATE`
/// answers; any other change is a `TRAINER_UPDATE`.
#[test]
fn only_an_item_record_landing_is_a_pane_update() {
    let mut open = TrainerOpen::default();
    open.open(
        0x42,
        TRAINER_TYPE_TRADESKILL,
        vec![wire(100, trainer_spell_state::GREEN, 10, 1, 0)],
        String::new(),
    );
    let before = snap(&open, &icon_catalog()).unwrap();
    let mut landed = before.clone();
    landed.services[0].texture = Some("ITEM".into());
    landed.services[0].item_link = Some("link".into());
    landed.services[0].description = Some("A sword.".into());
    assert!(only_pane_moved(&before, &landed));
    assert!(!only_pane_moved(&before, &before), "nothing moved");
    let mut learned = landed.clone();
    learned.services[0].category = TrainerServiceCategory::Used;
    assert!(
        !only_pane_moved(&landed, &learned),
        "a state change re-sorts"
    );
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
    let spells = empty_catalog();
    let mut w = wire(2018, trainer_spell_state::RED, 1000, 20, 164);
    w.req_spells = [0, 78, 0];
    w.talent_cost = 2;
    let svc = resolve_service(&w, TRAINER_TYPE_TRADESKILL, &spells, None, &warrior(&[]));
    assert_eq!(svc.spell_id, 2018);
    assert!(svc.name.is_none(), "no catalog, so the name is nil");
    assert_eq!(
        (svc.cost, svc.talent_cost, svc.profession_cost),
        (1000, 2, 0)
    );
    assert_eq!(svc.category, TrainerServiceCategory::Unavailable);
    assert_eq!(svc.level_req, 20);
    assert_eq!(
        svc.skill_req, None,
        "a gate whose line has no row reads (nil, 0, 1)"
    );
    assert_eq!(
        svc.ability_reqs,
        [
            None,
            Some(TrainerAbilityReq {
                name: None,
                met: true,
            }),
            None,
        ],
        "slot 1 holds 78, which has no record here, so nil and met"
    );
    assert_eq!((svc.group_key, svc.group_name.as_str()), (2, "<LEARN>"));
}

/// `GetTrainerServiceSkillReq` (`0x595470`) compares the player's skill, not the service state.
#[test]
fn the_skill_gate_reads_the_players_skill_value() {
    let lines = SkillLineCatalog::from_lines([(
        164,
        benilla_formats::SkillLineInfo {
            name: "Blacksmithing".into(),
            category_id: 11,
            icon: None,
            description: String::new(),
            alternate_verb: String::new(),
            can_link: false,
        },
    )]);
    let w = wire(2739, trainer_spell_state::RED, 50, 0, 164); // rank 100
    let mut player = warrior(&[]);
    let req = |p: &reeval::PlayerView| {
        resolve_service(
            &w,
            TRAINER_TYPE_TRADESKILL,
            &empty_catalog(),
            Some(&lines),
            p,
        )
        .skill_req
    };
    assert_eq!(
        req(&player).map(|r| (r.name, r.rank, r.met)),
        Some(("Blacksmithing".into(), 100, false)),
        "no slot fails it"
    );
    player.skills.push(reeval::SkillSlot {
        skill_id: 164,
        step: 1,
        value_plus_perm: 100,
    });
    assert!(req(&player).is_some_and(|r| r.met));
    player.skills[0].value_plus_perm = 99;
    assert!(req(&player).is_some_and(|r| !r.met));
}

/// `GetTrainerServiceAbilityReq` (`0x5955e0`): a level-gated service's known prerequisite still
/// reads met, and the name carries its rank. Skips without client data.
#[test]
fn ability_req_shows_the_required_rank_and_tracks_known_spells_on_real_data() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("load Spell");

    let mut w = wire(845, trainer_spell_state::RED, 100, 20, 0);
    w.req_spells = [78, 0, 0]; // Heroic Strike Rank 1
    let svc = resolve_service(&w, 0, &spells, None, &warrior(&[78]));
    assert_eq!(svc.category, TrainerServiceCategory::Unavailable);
    assert_eq!(
        svc.ability_reqs[0],
        Some(TrainerAbilityReq {
            name: Some("Heroic Strike (Rank 1)".to_string()),
            met: true,
        })
    );
    let svc = resolve_service(&w, 0, &spells, None, &warrior(&[]));
    assert!(svc.ability_reqs[0].as_ref().is_some_and(|r| !r.met));
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
    let state = snap(&open, &spells).expect("open, so Some");
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

/// `SetTrainerService` (`0x62e900`): a taught recipe shows its slot-0 product.
#[test]
fn tooltip_takes_the_item_arm_when_the_taught_spell_is_a_recipe() {
    let spells = SpellCatalog::from_displays(HashMap::from([
        (2756, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 2739)),
        (2739, taught_recipe()),
    ]));
    assert_eq!(service_tooltip(2756, &spells), TrainerTooltip::Item(2847));
    // The learn slot past slot 0 changes nothing: the product is read from slot 0.
    let mut w = wrapper(SPELL_EFFECT_LEARN_SPELL, 1, 2739);
    w.effects[0] = 0;
    let spells = SpellCatalog::from_displays(HashMap::from([(100, w), (2739, taught_recipe())]));
    assert_eq!(service_tooltip(100, &spells), TrainerTooltip::Item(2847));
}

/// The tradeskill bit without a create effect in slot 0 is the spell arm.
#[test]
fn tooltip_needs_a_create_effect_for_the_item_arm() {
    let mut odd = taught_recipe();
    odd.effects[0] = 6;
    let spells = SpellCatalog::from_displays(HashMap::from([
        (100, wrapper(SPELL_EFFECT_LEARN_SPELL, 0, 200)),
        (200, odd),
    ]));
    assert_eq!(
        service_tooltip(100, &spells),
        TrainerTooltip::Spell {
            spell_id: 200,
            alt_caster: false,
        }
    );
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

/// Every list resets the lines (`0x596450`), and an open window repaints through the
/// re-evaluator (`0x596b00`), never a second list, so every list opens and resets.
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
        "a fresh list is the server's states, nothing pending against it"
    );

    // A trigger with no window open does nothing.
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
        // Rank 2 in the book, rank 1 offered: the higher-rank test (`0x726160`) says known.
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
