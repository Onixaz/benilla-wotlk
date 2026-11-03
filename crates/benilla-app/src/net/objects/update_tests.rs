//! `SMSG_UPDATE_OBJECT` blocks against the object layer: a live re-create, out-of-range parking
//! and every revive path, the movement block, the create's attack target, spawn bool and
//! passenger pose.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL};
use benilla_protocol::wire::Vector3d;
use benilla_protocol::{
    CreateSpline, EntityKind, MonsterMoveFacing, MoveSpeeds, MoverState, ObjectFields,
    SessionEvent, TransportPose,
};
use bevy::prelude::*;

use crate::net::{FreshSpawn, Guid, GuidIndex, ObjectStore, ParkedObjects, SelfGuid, Spline};

const ME: u64 = 0x0000_0000_0000_0007;
const MOB: u64 = 0xF130_0000_1234_0001;
const TRAP: u64 = 0xF110_0000_0000_0042;
const SHIP: u64 = 0x1FC0_0000_0000_0003;

fn speeds(run: f32) -> MoveSpeeds {
    MoveSpeeds {
        walk: 2.5,
        run,
        ..Default::default()
    }
}

fn walk(to: [f32; 3]) -> CreateSpline {
    CreateSpline {
        path: vec![[0.0, 0.0, 0.0], to],
        id: 9,
        time_passed_ms: 0,
        duration_ms: 60_000,
        flying: false,
        cyclic: false,
        run_mode: true,
        arc: None,
        facing_fixed: false,
        final_facing: MonsterMoveFacing::None,
        catmull_rom: false,
    }
}

fn create(guid: u64, kind: EntityKind, position: [f32; 3], fields: &[(u16, u32)]) -> SessionEvent {
    let living = matches!(kind, EntityKind::Unit | EntityKind::Player);
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position,
        orientation: 0.0,
        scale: 1.0,
        speeds: living.then(|| speeds(7.0)),
        mover: living.then_some(MoverState {
            flags: 0,
            pitch: 0.0,
            flags2: 0,
        }),
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: false,
        fields: ObjectFields::from_pairs(fields).into_created(match kind {
            EntityKind::GameObject => benilla_protocol::messages::ObjectType::GameObject,
            EntityKind::Player => benilla_protocol::messages::ObjectType::Player,
            _ => benilla_protocol::messages::ObjectType::Unit,
        }),
    }
}

fn with(ev: SessionEvent, f: impl FnOnce(&mut SessionEvent)) -> SessionEvent {
    let mut ev = ev;
    f(&mut ev);
    ev
}

fn dispatch(world: &mut World, events: Vec<SessionEvent>) {
    crate::net::handlers::dispatch(world, events);
}

fn entity(world: &World, guid: u64) -> Option<Entity> {
    world.resource::<GuidIndex>().0.get(&guid).copied()
}

fn wow_pos(world: &World, guid: u64) -> [f32; 3] {
    let e = entity(world, guid).expect("indexed");
    benilla_assets::coords::bevy_to_wow(world.get::<Transform>(e).unwrap().translation)
}

fn health(world: &World, guid: u64) -> Option<u32> {
    let e = entity(world, guid)?;
    world.get::<ObjectStore>(e)?.0.unit_health()
}

fn world() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let w = app.world_mut();
    w.resource_mut::<SelfGuid>().0 = Some(ME);
    dispatch(
        w,
        vec![
            create(ME, EntityKind::Player, [0.0; 3], &[(FIELD_UNIT_HEALTH, 50)]),
            create(
                MOB,
                EntityKind::Unit,
                [10.0, 0.0, 0.0],
                &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_LEVEL, 9)],
            ),
        ],
    );
    app
}

/// A create for a guid already live takes its fields alone (`0x4d6c4d`): the pose, the path and
/// the speeds stay those it was moving with.
#[test]
fn a_live_recreate_takes_the_fields_and_keeps_the_movement() {
    let mut app = world();
    let w = app.world_mut();
    dispatch(
        w,
        vec![with(
            create(
                MOB,
                EntityKind::Unit,
                [10.0, 0.0, 0.0],
                &[(FIELD_UNIT_HEALTH, 100)],
            ),
            |ev| {
                if let SessionEvent::ObjectCreate { guid, spline, .. } = ev {
                    *guid = 0xF130_0000_1234_0002;
                    *spline = Some(walk([30.0, 0.0, 0.0]));
                }
            },
        )],
    );
    const WALKER: u64 = 0xF130_0000_1234_0002;
    let e = entity(w, WALKER).unwrap();
    assert!(w.get::<Spline>(e).is_some(), "the create's path is walked");
    dispatch(
        w,
        vec![create(
            WALKER,
            EntityKind::Unit,
            [500.0, 500.0, 0.0],
            &[(FIELD_UNIT_HEALTH, 40)],
        )],
    );
    assert_eq!(entity(w, WALKER), Some(e), "the same object");
    assert!(w.get::<Spline>(e).is_some(), "its path is not dropped");
    assert_eq!(wow_pos(w, WALKER), [10.0, 0.0, 0.0], "nor is it re-posed");
    assert_eq!(health(w, WALKER), Some(40), "the fields are applied");
}

/// OUT_OF_RANGE skips our own guid (`0x4d7278`) and parks the rest, and a values block revives a
/// parked object with the fields it left with and the delta on top (`0x4d6e80`).
#[test]
fn out_of_range_spares_our_guid_and_a_values_block_revives_a_parked_object() {
    let mut app = world();
    let w = app.world_mut();
    let me = entity(w, ME).unwrap();
    let mob = entity(w, MOB).unwrap();
    dispatch(w, vec![SessionEvent::ObjectsRemoved(vec![ME, MOB])]);
    assert_eq!(entity(w, ME), Some(me), "our own guid is skipped");
    assert!(w.get::<Guid>(me).is_some());
    assert_eq!(entity(w, MOB), None, "the mob left the index");
    assert!(w.get::<Guid>(mob).is_none(), "its old entity is torn down");
    assert!(w.resource::<ParkedObjects>().contains(MOB), "and parked");

    dispatch(
        w,
        vec![SessionEvent::ObjectValues {
            guid: MOB,
            fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 60)]),
        }],
    );
    let revived = entity(w, MOB).expect("the values block revived it");
    assert_ne!(revived, mob);
    assert_eq!(health(w, MOB), Some(60), "the delta lands");
    assert_eq!(
        w.get::<ObjectStore>(revived).unwrap().0.unit_level(),
        Some(9),
        "on the fields it parked with"
    );
    assert_eq!(wow_pos(w, MOB), [10.0, 0.0, 0.0], "where it parked");
    assert!(!w.resource::<ParkedObjects>().contains(MOB));
}

/// A create revives a parked unit with its movement applied (`0x4d645d`), and `NEAR_OBJECTS`
/// revives one as it parked (`0x4d6f40`).
#[test]
fn a_create_or_near_objects_revives_a_parked_object() {
    let mut app = world();
    let w = app.world_mut();
    dispatch(w, vec![SessionEvent::ObjectsRemoved(vec![MOB])]);
    dispatch(
        w,
        vec![with(
            create(
                MOB,
                EntityKind::Unit,
                [20.0, 5.0, 0.0],
                &[(FIELD_UNIT_HEALTH, 70)],
            ),
            |ev| {
                if let SessionEvent::ObjectCreate { spline, .. } = ev {
                    *spline = Some(walk([40.0, 0.0, 0.0]));
                }
            },
        )],
    );
    let e = entity(w, MOB).expect("revived by the create");
    assert_eq!(
        wow_pos(w, MOB),
        [20.0, 5.0, 0.0],
        "a unit's movement re-applies"
    );
    assert!(w.get::<Spline>(e).is_some());
    assert_eq!(health(w, MOB), Some(70));
    assert_eq!(
        w.get::<ObjectStore>(e).unwrap().0.unit_level(),
        Some(0),
        "a create replaces the descriptor whole"
    );

    dispatch(w, vec![SessionEvent::ObjectsRemoved(vec![MOB])]);
    dispatch(w, vec![SessionEvent::ObjectsNear(vec![ME, MOB])]);
    assert!(entity(w, MOB).is_some(), "NEAR_OBJECTS revives it");
    assert_eq!(health(w, MOB), Some(70), "with what it parked with");
}

/// A movement block applies the mover's whole state (`0x6f1520`), but never to our own mover
/// (`0x4d6de5`).
#[test]
fn a_movement_block_moves_a_unit_but_never_our_own_mover() {
    let mut app = world();
    let w = app.world_mut();
    let movement = |guid| SessionEvent::ObjectMove {
        guid,
        position: [12.0, 3.0, 1.0],
        orientation: 1.0,
        mover: MoverState {
            flags: 0,
            pitch: 0.0,
            flags2: 0,
        },
        speeds: speeds(9.0),
        transport: None,
        spline: Some(walk([50.0, 0.0, 0.0])),
    };
    dispatch(w, vec![movement(MOB), movement(ME)]);
    let mob = entity(w, MOB).unwrap();
    assert_eq!(wow_pos(w, MOB), [12.0, 3.0, 1.0]);
    assert_eq!(w.get::<crate::net::UnitSpeeds>(mob).unwrap().0.run, 9.0);
    assert!(w.get::<Spline>(mob).is_some(), "the spline rides with it");
    assert_eq!(wow_pos(w, ME), [0.0; 3], "our own mover ignores it");
    let me = entity(w, ME).unwrap();
    assert!(w.get::<Spline>(me).is_none());
}

/// `HAS_TARGET` seeds the attack target (`0x98e560`), and update type 3 marks a creature or a
/// GameObject for its first animation pass, never a player.
#[test]
fn a_create_seeds_its_attack_target_and_create2_marks_the_spawn() {
    let mut app = world();
    let w = app.world_mut();
    const FIGHTER: u64 = 0xF130_0000_1234_0009;
    const OTHER: u64 = 0x0000_0000_0000_0010;
    dispatch(
        w,
        vec![
            with(create(FIGHTER, EntityKind::Unit, [1.0; 3], &[]), |ev| {
                if let SessionEvent::ObjectCreate {
                    attack_target,
                    spawned,
                    ..
                } = ev
                {
                    *attack_target = Some(ME);
                    *spawned = true;
                }
            }),
            with(create(TRAP, EntityKind::GameObject, [2.0; 3], &[]), |ev| {
                if let SessionEvent::ObjectCreate { spawned, .. } = ev {
                    *spawned = true;
                }
            }),
            with(create(OTHER, EntityKind::Player, [3.0; 3], &[]), |ev| {
                if let SessionEvent::ObjectCreate { spawned, .. } = ev {
                    *spawned = true;
                }
            }),
        ],
    );
    let fighter = entity(w, FIGHTER).unwrap();
    assert_eq!(
        w.get::<crate::creature_anim::Engaged>(fighter).map(|e| e.0),
        Some(ME)
    );
    assert!(w.get::<FreshSpawn>(fighter).is_some());
    assert!(w.get::<FreshSpawn>(entity(w, TRAP).unwrap()).is_some());
    assert!(w.get::<FreshSpawn>(entity(w, OTHER).unwrap()).is_none());
    assert!(
        w.get::<crate::creature_anim::Engaged>(entity(w, MOB).unwrap())
            .is_none(),
        "no HAS_TARGET, no attack target"
    );
}

/// A GameObject created on a transport rides it from its `POSITION` block's local pose
/// (`0x714250`).
#[test]
fn a_position_create_on_a_transport_rides_the_deck() {
    let mut app = world();
    let w = app.world_mut();
    dispatch(
        w,
        vec![with(
            create(TRAP, EntityKind::GameObject, [100.0, 200.0, 5.0], &[]),
            |ev| {
                if let SessionEvent::ObjectCreate { transport, .. } = ev {
                    *transport = Some(TransportPose {
                        guid: SHIP,
                        pos: Vector3d {
                            x: 1.5,
                            y: -2.0,
                            z: 3.0,
                        },
                        orientation: 0.5,
                        seat: -1,
                    });
                }
            },
        )],
    );
    let e = entity(w, TRAP).unwrap();
    let deck = w
        .get::<crate::transport::DeckPassenger>(e)
        .expect("a deck passenger");
    assert_eq!(deck.transport_guid, SHIP);
    assert_eq!(deck.local_pos, [1.5, -2.0, 3.0]);
    assert!(
        w.get::<crate::transport::TransportRider>(e).is_none(),
        "not a unit rider"
    );
    assert_eq!(
        wow_pos(w, TRAP),
        [100.0, 200.0, 5.0],
        "the world pose until composed"
    );
}
