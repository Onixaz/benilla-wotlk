//! The interact target's set, clear and watchdog, and the cues they hand the sound, the animation
//! and the GameObject.

use super::*;
use crate::ui_merchant::MerchantOpen;

/// `HIGHGUID_UNIT` guids.
const VENDOR: u64 = 0xF130_0000_0000_0042;
const TRAINER: u64 = 0xF130_0000_0000_0043;
/// A `HIGHGUID_GAMEOBJECT` guid.
const SIGN: u64 = 0xF110_0000_0000_0044;
/// A player's guid: a zero high word.
const PARTNER: u64 = 0x0000_0000_0000_002A;

fn world() -> World {
    let mut world = World::new();
    world.init_resource::<InteractTarget>();
    world.init_resource::<InteractRange>();
    world.init_resource::<Messages<InteractCue>>();
    world
}

fn cues(world: &mut World) -> Vec<InteractCue> {
    let mut msgs = world.resource_mut::<Messages<InteractCue>>();
    msgs.drain().collect()
}

/// The set calls the hello and the re-pick for a new unit (`0x51fda4`, `0x51fdaf`); a set to the same unit clears it first with "a new one follows", which re-picks with no goodbye (`0x51fd16` -> `0x518d9b`), and plays no second hello (`0x51fd9e`); a swap to another unit plays no goodbye for the old; the clear plays the goodbye and re-picks (`0x518da5`, `0x518db0`).
#[test]
fn the_set_and_the_clear_call_the_npc_sound_and_the_re_pick_directly() {
    use InteractCue::{RePick, Sound};
    let mut world = world();
    set_interact_target(&mut world, VENDOR);
    assert_eq!(
        cues(&mut world),
        [
            Sound {
                guid: VENDOR,
                hello: true
            },
            RePick(VENDOR)
        ]
    );
    set_interact_target(&mut world, VENDOR);
    assert_eq!(cues(&mut world), [RePick(VENDOR)], "the same unit again");
    set_interact_target(&mut world, TRAINER);
    assert_eq!(
        cues(&mut world),
        [
            RePick(VENDOR),
            Sound {
                guid: TRAINER,
                hello: true
            },
            RePick(TRAINER)
        ],
        "a swap: no goodbye for the old"
    );
    clear_interact_target(&mut world, VENDOR);
    assert!(cues(&mut world).is_empty(), "a clear on another guid");
    clear_interact_target(&mut world, TRAINER);
    assert_eq!(
        cues(&mut world),
        [
            Sound {
                guid: TRAINER,
                hello: false
            },
            RePick(TRAINER)
        ]
    );
    assert_eq!(world.resource::<InteractTarget>().0, None);
}

/// A player target (a trade partner) plays nothing and re-picks nothing (`0x518d99`, `0x51fd6f`); a GameObject starts its interaction on every set, the same one included (`0x51fdc2`), and closes it on the clear (`0x5130c4`).
#[test]
fn a_player_is_silent_and_a_gameobject_starts_and_closes() {
    let mut world = world();
    set_interact_target(&mut world, PARTNER);
    clear_interact_target(&mut world, PARTNER);
    assert!(cues(&mut world).is_empty());
    set_interact_target(&mut world, SIGN);
    set_interact_target(&mut world, SIGN);
    clear_held_interact_target(&mut world);
    let start = InteractCue::GameObject {
        guid: SIGN,
        start: true,
    };
    let close = InteractCue::GameObject {
        guid: SIGN,
        start: false,
    };
    assert_eq!(cues(&mut world), [start, close, start, close]);
}

/// A clear inside a Lua call records its cues on the target, which the delivery hands on after the call.
#[test]
fn a_clear_inside_a_call_is_delivered_after_it() {
    let mut app = App::new();
    app.add_message::<InteractCue>()
        .insert_resource(InteractTarget::holding(VENDOR))
        .add_systems(Update, deliver_interact_cues);
    assert!(app
        .world_mut()
        .resource_mut::<InteractTarget>()
        .clear_in_call(VENDOR));
    assert_eq!(app.world().resource::<InteractTarget>().0, None);
    app.update();
    let mut msgs = app.world_mut().resource_mut::<Messages<InteractCue>>();
    assert_eq!(
        msgs.drain().collect::<Vec<_>>(),
        [
            InteractCue::Sound {
                guid: VENDOR,
                hello: false
            },
            InteractCue::RePick(VENDOR)
        ]
    );
    assert!(app.world().resource::<InteractTarget>().1.is_empty());
}

/// Another window's open takes the target: the window bound to the old one closes inside the set, ahead of the new store (`0x51fd1c` -> `0x512f3a`), and only the window bound to it.
#[test]
fn a_set_closes_the_window_bound_to_the_old_target() {
    let mut world = world();
    world.init_resource::<MerchantOpen>();
    world.resource_mut::<MerchantOpen>().open(VENDOR, vec![]);
    world.resource_mut::<InteractTarget>().0 = Some(VENDOR);
    set_interact_target(&mut world, TRAINER);
    assert!(!world.resource::<MerchantOpen>().is_open());
    assert_eq!(world.resource::<InteractTarget>().0, Some(TRAINER));
    // A window not bound to the target stays: only the walk's match closes.
    world.resource_mut::<MerchantOpen>().open(VENDOR, vec![]);
    set_interact_target(&mut world, VENDOR + 2);
    assert!(world.resource::<MerchantOpen>().is_open());
}

/// The watchdog's app: us at the origin, the target `at` yd along +x with `fields`, its vendor window open and the target set on it.
fn watch(guid: u64, at: f32, fields: &[(u16, u32)]) -> App {
    let mut app = App::new();
    app.init_resource::<InteractTarget>()
        .init_resource::<InteractRange>()
        .init_resource::<GuidIndex>()
        .add_message::<InteractCue>()
        .init_resource::<MerchantOpen>()
        .add_systems(Update, update_interact_target);
    app.world_mut().spawn((
        SelfPlayer,
        Transform::default(),
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
    ));
    let e = app
        .world_mut()
        .spawn((
            Transform::from_xyz(at, 0.0, 0.0),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields)),
        ))
        .id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(guid, e);
    app.world_mut()
        .resource_mut::<MerchantOpen>()
        .open(guid, vec![]);
    set_interact_target(app.world_mut(), guid);
    app
}

/// The set stores the reach the watchdog holds the target to: `(100/9)²` for a player (`0xac8a68`), `(combat reach + 4)²` for a unit (`0x51fd84`-`0x51fd96`).
#[test]
fn the_set_stores_the_reach_by_the_targets_type() {
    let app = watch(PARTNER, 1.0, &[]);
    assert_eq!(
        app.world().resource::<InteractRange>().0,
        PLAYER_INTERACT_SQ
    );
    let reach = benilla_protocol::field::FIELD_UNIT_COMBATREACH;
    const OBJECT_FIELD_TYPE: u16 = 2;
    let app = watch(
        VENDOR,
        1.0,
        &[(OBJECT_FIELD_TYPE, 0x9), (reach, 1.5f32.to_bits())],
    );
    assert_eq!(app.world().resource::<InteractRange>().0, 5.5 * 5.5);
}

/// A target gone is cleared by the watchdog, whose clear closes the window bound to it (`0x51fe2e` -> `0x51ff05`).
#[test]
fn the_watchdog_clears_a_target_gone() {
    const OBJECT_FIELD_TYPE: u16 = 2;
    let mut app = watch(VENDOR, 1.0, &[(OBJECT_FIELD_TYPE, 0x9)]);
    assert!(app.world().resource::<InteractRange>().0 > 0.0, "watched");
    assert!(app.world().resource::<MerchantOpen>().is_open());
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .remove(&VENDOR);
    app.update();
    assert_eq!(app.world().resource::<InteractTarget>().0, None);
    assert!(!app.world().resource::<MerchantOpen>().is_open());
}

/// Template 1, ours, in group 1; template 20 an enemy of group 1, 21 a creature template in group 1's enemies, 22 a contested-guard template friendly to group 1.
fn reaction_factions() -> crate::target::Factions {
    use benilla_formats::{FactionCatalog, FactionTemplate};
    let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
        faction,
        group_mask,
        friend_group_mask,
        enemy_group_mask,
        enemies: [0; 4],
        friends: [0; 4],
    };
    crate::target::Factions::from_catalog(
        FactionCatalog::from_rows(
            std::collections::HashMap::from([
                (1, tpl(1, 1, 1, 0)),
                (20, tpl(20, 0, 0, 1)),
                (21, tpl(21, 8, 0, 1)),
                (22, tpl(22, 0, 1, 0)),
            ]),
            std::collections::HashMap::new(),
        )
        .with_template_flags(22, 0x1000),
    )
}

/// The watchdog app around a GameObject `SIGN` 1 yd off with `fields`, our player carrying `me` and the reaction catalog.
fn watch_sign(fields: &[(u16, u32)], me: &[(u16, u32)]) -> App {
    let mut app = watch(SIGN, 1.0, fields);
    app.insert_resource(reaction_factions());
    let mut q = app
        .world_mut()
        .query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
    let mut store = q.single_mut(app.world_mut()).unwrap();
    store.0 = benilla_protocol::ObjectFields::from_pairs(me);
    app
}

/// Our player: a player object (type `0x19`) on template 1, player-controlled, with `player_flags`.
fn me(player_flags: u32) -> Vec<(u16, u32)> {
    use benilla_protocol::field::*;
    vec![
        (FIELD_OBJECT_TYPE, 0x19),
        (FIELD_UNIT_FACTIONTEMPLATE, 1),
        (FIELD_UNIT_FLAGS, 0x8),
        (FIELD_PLAYER_FLAGS, player_flags),
    ]
}

/// A GameObject in reach with no faction is neutral and stays held; one whose `GAMEOBJECT_FACTION` reacts hostile to us, or held by a ghost, is cleared by the watchdog's interact check, ahead of its range (`0x51fe77` -> `0x71f8b0`: `0x71f8c1`, `0x71f8e3`).
#[test]
fn the_watchdog_clears_a_gameobject_we_cannot_interact_with() {
    use benilla_protocol::field::{FIELD_GAMEOBJECT_FACTION, FIELD_OBJECT_TYPE};
    let go = |faction: u32| {
        vec![
            (FIELD_OBJECT_TYPE, 0x21),
            (FIELD_GAMEOBJECT_FACTION, faction),
        ]
    };
    let mut app = watch_sign(&go(0), &me(0));
    assert!(app.world().resource::<InteractRange>().0 > 0.0, "watched");
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        Some(SIGN),
        "neutral, in reach"
    );
    let mut app = watch_sign(&go(20), &me(0));
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        None,
        "hostile faction"
    );
    assert!(!app.world().resource::<MerchantOpen>().is_open());
    let mut app = watch_sign(&go(0), &me(0x10));
    app.update();
    assert_eq!(app.world().resource::<InteractTarget>().0, None, "a ghost");
}

/// A GameObject's creator in view answers its reaction ahead of its faction (`0x70edf7` -> `0x70edff`): a hostile creator clears a GameObject whose own faction is friendly, and a gone creator falls back to the faction (`0x70ee04`).
#[test]
fn a_gameobjects_creator_answers_its_reaction() {
    use benilla_protocol::field::{
        FIELD_GAMEOBJECT_CREATED_BY, FIELD_GAMEOBJECT_FACTION, FIELD_OBJECT_TYPE,
        FIELD_UNIT_FACTIONTEMPLATE,
    };
    const CREATOR: u64 = 0xF130_0000_0000_0077;
    let fields = [
        (FIELD_OBJECT_TYPE, 0x21),
        (FIELD_GAMEOBJECT_FACTION, 1),
        (FIELD_GAMEOBJECT_CREATED_BY, CREATOR as u32),
        (FIELD_GAMEOBJECT_CREATED_BY + 1, (CREATOR >> 32) as u32),
    ];
    let mut app = watch_sign(&fields, &me(0));
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        Some(SIGN),
        "no creator in view: the faction"
    );
    let creator = app
        .world_mut()
        .spawn((
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_OBJECT_TYPE, 0x9),
                (FIELD_UNIT_FACTIONTEMPLATE, 21),
            ])),
        ))
        .id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(CREATOR, creator);
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        None,
        "the hostile creator"
    );
}

/// The generic reaction's contested leg (`0x71f7f7`-`0x71f80e`): a contested-PvP player is hostile to a guard template that is otherwise friendly, so its GameObject is cleared.
#[test]
fn a_contested_player_is_hostile_to_a_guard_template() {
    use benilla_protocol::field::{FIELD_GAMEOBJECT_FACTION, FIELD_OBJECT_TYPE};
    let go = [(FIELD_OBJECT_TYPE, 0x21), (FIELD_GAMEOBJECT_FACTION, 22)];
    let mut app = watch_sign(&go, &me(0));
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        Some(SIGN),
        "friendly"
    );
    let mut app = watch_sign(&go, &me(0x100));
    app.update();
    assert_eq!(
        app.world().resource::<InteractTarget>().0,
        None,
        "contested"
    );
}

/// The watchdog runs after the frame's creates resolve us (`0x51fe55` -> `0x4038f0` looks the
/// active player up in the object table the frame's packets filled), so a clear's listener reads
/// our own create of the same frame; unordered, its clear could land before our tag.
#[test]
fn the_watchdog_runs_after_our_create_is_tagged() {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(crate::test_support::runs_before(
        &mut app,
        crate::net::tag_self_player,
        update_interact_target
    ));
}
