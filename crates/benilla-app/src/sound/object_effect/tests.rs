use std::collections::HashMap;

use benilla_formats::{EffectGroup, ObjectEffect, EFFECT_SOUND};

use super::*;
use crate::creature_anim::move_flags as f;

const PACKAGE: u32 = 9;

fn sound(id: u32, kit: u32) -> ObjectEffect {
    ObjectEffect {
        id,
        kind: EFFECT_SOUND,
        rec_id: kit,
    }
}

/// A Demolisher-shaped package: an idle loop on 38, a run loop and a gear shift on 37, a launch
/// on 8, a death on 15, and one group held by both 61 and 62.
fn catalog() -> ObjectEffectCatalog {
    let group = |id, enter: Vec<ObjectEffect>, looping: Vec<ObjectEffect>, exit| EffectGroup {
        id,
        enter,
        looping,
        exit,
    };
    let groups = [
        group(1, vec![], vec![sound(10, 100)], vec![]),
        group(
            2,
            vec![sound(21, 201)],
            vec![sound(20, 200)],
            vec![sound(22, 202)],
        ),
        group(3, vec![sound(30, 300)], vec![], vec![]),
        group(4, vec![sound(40, 400)], vec![], vec![]),
        group(6, vec![], vec![sound(60, 600)], vec![]),
    ];
    let states = HashMap::from([
        (state::STILL, vec![1]),
        (state::MOVING, vec![2]),
        (8, vec![3]),
        (state::DEATH, vec![4]),
        (state::FLYING, vec![6]),
        (state::FLY_FORWARD, vec![6]),
    ]);
    ObjectEffectCatalog::from_parts(
        HashMap::from([(25092, PACKAGE), (5, 77)]),
        HashMap::from([(PACKAGE, states)]),
        groups,
    )
}

fn started(cat: &ObjectEffectCatalog, flags: u32) -> (UnitEffects, Vec<Cue>) {
    let mut cues = Vec::new();
    let m = UnitEffects::start(cat, PACKAGE, flags, &mut cues);
    (m, cues)
}

#[test]
fn a_standing_unit_starts_its_idle_loop() {
    let cat = catalog();
    let (m, cues) = started(&cat, 0);
    assert_eq!(cues, [Cue::Loop(100)]);
    assert!(m.has(state::STARTED) && m.has(state::STILL) && m.has(state::NOT_TURNING));
}

#[test]
fn moving_swaps_the_idle_for_the_run_loop_and_back() {
    let cat = catalog();
    let (mut m, _) = started(&cat, 0);
    let mut cues = Vec::new();
    m.movement(&cat, f::FORWARD, &mut cues);
    // The enter one-shot, the loop, then the left state's loop stops; nothing exits with sound.
    assert_eq!(cues, [Cue::OneShot(201), Cue::Loop(200), Cue::Stop(100)]);
    cues.clear();
    m.movement(&cat, 0, &mut cues);
    assert_eq!(cues, [Cue::Loop(100), Cue::OneShot(202), Cue::Stop(200)]);
}

#[test]
fn a_strafe_alone_counts_as_moving() {
    let cat = catalog();
    let (mut m, _) = started(&cat, 0);
    let mut cues = Vec::new();
    m.movement(&cat, f::STRAFE_LEFT, &mut cues);
    assert!(m.has(state::MOVING) && !m.has(state::STILL));
}

#[test]
fn the_launch_plays_on_its_animation_and_once() {
    let cat = catalog();
    let (mut m, _) = started(&cat, 0);
    let mut cues = Vec::new();
    m.animation(&cat, [Some(107), None, None], &mut cues);
    assert_eq!(cues, [Cue::OneShot(300)]);
    cues.clear();
    // Back to Stand: state 8 is left, and its group has no exit effect.
    m.animation(&cat, [Some(0), None, None], &mut cues);
    assert!(cues.is_empty());
    assert!(!m.has(8) && m.has(19));
}

#[test]
fn a_death_plays_its_one_shot_and_silences_the_engine() {
    let cat = catalog();
    let (mut m, _) = started(&cat, 0);
    let mut cues = Vec::new();
    m.animation(&cat, [Some(1), None, None], &mut cues);
    assert_eq!(cues, [Cue::OneShot(400), Cue::Stop(100)]);
    assert!(!m.has(state::STILL) && m.has(state::STARTED));
}

#[test]
fn a_group_two_states_hold_loops_until_both_leave() {
    let cat = catalog();
    let (mut m, _) = started(&cat, 0);
    let mut cues = Vec::new();
    m.movement(&cat, f::FLYING | f::FORWARD, &mut cues);
    assert_eq!(cues.iter().filter(|c| **c == Cue::Loop(600)).count(), 1);
    cues.clear();
    // Flying back: 62 is left but 61 still holds the group.
    m.movement(&cat, f::FLYING | f::BACKWARD, &mut cues);
    assert!(!m.has(state::FLY_FORWARD) && m.has(state::FLYING));
    assert!(!cues.contains(&Cue::Stop(600)));
    cues.clear();
    m.movement(&cat, 0, &mut cues);
    assert!(cues.contains(&Cue::Stop(600)));
}

#[test]
fn a_tear_down_stops_everything_and_plays_no_exit() {
    let cat = catalog();
    let (mut m, _) = started(&cat, f::FORWARD);
    let mut cues = Vec::new();
    m.tear_down(&cat, &mut cues);
    assert!(cues.contains(&Cue::Stop(200)));
    assert!(!cues.contains(&Cue::OneShot(202)), "no exit one-shot");
    assert!(
        cues.contains(&Cue::Stop(201)),
        "the gear shift one-shot is stopped too"
    );
    assert_eq!(m.states, 0);
}

#[test]
fn the_package_follows_the_display_and_needs_a_package_row() {
    let cat = catalog();
    assert_eq!(package_of(&cat, Some(25092)), PACKAGE);
    assert_eq!(package_of(&cat, Some(5)), 0, "package 77 has no row");
    assert_eq!(package_of(&cat, Some(1)), 0);
    assert_eq!(package_of(&cat, None), 0);
}

#[test]
fn the_animation_map_matches_the_reference_table() {
    assert_eq!(anim_state(107), 8);
    assert_eq!(anim_state(112), 10);
    assert_eq!(anim_state(16), 40);
    assert_eq!(anim_state(1), state::DEATH);
    assert_eq!(anim_state(6), state::DEAD);
    assert_eq!(anim_state(153), 77);
    assert_eq!(anim_state(156), 80);
    assert_eq!(anim_state(2), 0);
}

/// The install's Demolisher starts on its "Off Vehicle" loop (state 1) and its idle engine.
#[test]
fn the_demolisher_starts_its_engine_idle() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_object_effects(&mut chain).expect("object effects");
    let package = package_of(&cat, Some(25092));
    assert_eq!(package, 122);
    let mut cues = Vec::new();
    let mut m = UnitEffects::start(&cat, package, 0, &mut cues);
    assert_eq!(cues, [Cue::Loop(12748), Cue::Loop(13883)]);
    cues.clear();
    m.movement(&cat, f::FORWARD, &mut cues);
    assert_eq!(
        cues,
        [Cue::OneShot(14719), Cue::Loop(13885), Cue::Stop(13883)]
    );
}
