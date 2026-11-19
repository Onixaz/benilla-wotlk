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
    let mut app = watch(VENDOR, 1.0, &[]);
    assert!(app.world().resource::<MerchantOpen>().is_open());
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .remove(&VENDOR);
    app.update();
    assert_eq!(app.world().resource::<InteractTarget>().0, None);
    assert!(!app.world().resource::<MerchantOpen>().is_open());
}
