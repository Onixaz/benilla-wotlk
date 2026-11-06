//! The relayed `MSG_MOVE_*` values against the object layer: a speed or height relay naming the
//! active mover, the observed collision height and swim-to-fly bit, and the flight spline sync.

use benilla_protocol::{RelayVerb, SessionEvent};
use bevy::prelude::*;

use crate::entities::CollisionHeight;
use crate::net::{
    ActiveMover, GuidIndex, RemoteMotion, SelfGuid, SelfMoveMessage, SwimFlyTransition,
};

const ME: u64 = 0x0000_0000_0000_0007;
const EYE: u64 = 0xF130_006F_5F29_221A;
const MOB: u64 = 0xF130_0000_1234_0001;

fn relay(guid: u64, verb: RelayVerb) -> SessionEvent {
    SessionEvent::UnitMove {
        guid,
        position: [1.0, 2.0, 3.0],
        orientation: 0.5,
        flags: 0,
        pitch: 0.0,
        time: 1_000,
        verb,
        fall_time: 0,
        jump: None,
        transport: None,
    }
}

/// Our body, a possessed unit we drive, and a creature we only watch.
fn world() -> (App, Entity, Entity) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let world = app.world_mut();
    let me = world.spawn_empty().id();
    let eye = world.spawn(ActiveMover).id();
    let mob = world.spawn_empty().id();
    let mut index = world.resource_mut::<GuidIndex>();
    index.0.insert(ME, me);
    index.0.insert(EYE, eye);
    index.0.insert(MOB, mob);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    (app, eye, mob)
}

fn self_moves(world: &World) -> usize {
    world.resource::<Messages<SelfMoveMessage>>().len()
}

/// `0x7406da`: the active mover takes a speed relay's value alone, so neither our body nor the
/// unit we drive is posed by it; a watched unit is, and a plain relay still poses our body.
#[test]
fn a_value_relay_poses_only_a_unit_we_do_not_drive() {
    let (mut app, eye, mob) = world();
    let world = app.world_mut();
    crate::net::handlers::dispatch(
        world,
        vec![
            relay(ME, RelayVerb::Value),
            relay(EYE, RelayVerb::Value),
            relay(MOB, RelayVerb::Value),
        ],
    );
    assert_eq!(self_moves(world), 0, "our pose stays ours");
    assert!(
        world.get::<RemoteMotion>(eye).is_none(),
        "the driven unit is not posed"
    );
    assert!(
        world.get::<RemoteMotion>(mob).is_some(),
        "a watched unit is"
    );

    crate::net::handlers::dispatch(world, vec![relay(ME, RelayVerb::Pose)]);
    assert_eq!(self_moves(world), 1, "a server-authored pose still lands");
}

/// `0x74099f` and `0x6ef1f5`-`0x6ef20a`: a watched unit keeps the relayed height and the second
/// word's swim-to-fly bit.
#[test]
fn a_watched_unit_keeps_its_relayed_height_and_swim_to_fly_bit() {
    let (mut app, _, mob) = world();
    let world = app.world_mut();
    crate::net::handlers::dispatch(
        world,
        vec![
            relay(MOB, RelayVerb::SwimFlyTransition(true)),
            SessionEvent::CollisionHeightChanged {
                guid: MOB,
                height: 3.5,
            },
        ],
    );
    assert!(world.get::<SwimFlyTransition>(mob).is_some());
    assert_eq!(
        world.get::<CollisionHeight>(mob),
        Some(&CollisionHeight(3.5))
    );

    crate::net::handlers::dispatch(
        world,
        vec![SessionEvent::CollisionHeightChanged {
            guid: MOB,
            height: 2.0,
        }],
    );
    assert_eq!(
        world.get::<CollisionHeight>(mob),
        Some(&CollisionHeight(2.0))
    );
}
