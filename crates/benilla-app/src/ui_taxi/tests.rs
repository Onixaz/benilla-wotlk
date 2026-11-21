use super::*;
use benilla_protocol::field::FIELD_UNIT_FLAGS;
use benilla_protocol::ObjectFields;
use bevy::ecs::system::RunSystemOnce;

const MASTER: u64 = 0xF130_0000_0000_0099;

/// A world at the Stormwind flight master (2) knowing Sentinel Hill (4), Ironforge (6) and
/// Thelsamar (8), the player Honored with it; `None` without the install.
fn stormwind() -> Option<(World, crossbeam_channel::Receiver<ClientCommand>)> {
    use benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalogs = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
    let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut world = World::new();
    world.init_resource::<TaxiState>();
    world.init_resource::<GuidIndex>();
    world.init_resource::<crate::ui_session::InteractTarget>();
    world.insert_resource(NetCommands(tx));
    world.insert_resource(crate::net::AccountExpansion(2));
    world.insert_resource(catalogs);
    world.insert_resource(factions);
    world.insert_resource(reps);
    world.spawn((
        SelfPlayer,
        ObjectStore(ObjectFields::from_pairs(
            &crate::target::human_warrior_fields(),
        )),
    ));
    let master = world
        .spawn((
            crate::net::Guid(MASTER),
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_FACTIONTEMPLATE, template),
                (FIELD_UNIT_FLAGS, 0x1000),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(MASTER, master);
    setup_map(&mut world, MASTER, 2, TaxiMask(vec![0b1010_1010]));
    Some((world, rx))
}

/// The slot of the node named `name`.
fn slot(world: &World, name: &str) -> u32 {
    let table = world
        .resource::<TaxiState>()
        .table
        .as_ref()
        .expect("listed");
    table
        .ui
        .nodes
        .iter()
        .position(|n| n.name == name)
        .expect(name) as u32
}

/// `TaxiNodeCost` at the call (`0x591350`): Honored makes 0.1 off a direct hop, as no 3.3.5a field
/// carries an honor rank: Sentinel Hill's direct 110 shows 99, Ironforge's 50 shows 45, and
/// Thelsamar's chain through Ironforge, 50 + 110, shows its tier-2 sum 144. With the flight master
/// gone the fares read 0 at the next call.
#[test]
fn the_fare_is_priced_at_the_call_off_the_flight_master() {
    let Some((mut world, _rx)) = stormwind() else {
        return;
    };
    let cost = |world: &mut World, name: &str| {
        let s = slot(world, name);
        world
            .run_system_once(move |live: TaxiLive| live.cost(s))
            .unwrap()
    };
    assert_eq!(cost(&mut world, "Sentinel Hill, Westfall"), 99);
    assert_eq!(cost(&mut world, "Ironforge, Dun Morogh"), 45);
    assert_eq!(cost(&mut world, "Thelsamar, Loch Modan"), 144);
    world.resource_mut::<GuidIndex>().0.remove(&MASTER);
    assert_eq!(cost(&mut world, "Sentinel Hill, Westfall"), 0);
}

/// `TakeTaxiNode` inside its call (`0x5914b0`): mounted refuses (`0x5914e1`), the current node is
/// the same node, the chain's fare over our coinage refuses, then the chain goes as the express.
#[test]
fn take_taxi_node_refuses_or_sends_inside_its_call() {
    let Some((mut world, rx)) = stormwind() else {
        return;
    };
    let take = |world: &mut World, s: u32| {
        world
            .run_system_once(move |mut live: TaxiLive| live.act(TaxiAct::Take(s), None))
            .unwrap()
            .error
    };
    let here = slot(&world, "Stormwind, Elwynn");
    let thelsamar = slot(&world, "Thelsamar, Loch Modan");
    assert_eq!(take(&mut world, here), Some("ERR_TAXISAMENODE"));
    assert_eq!(take(&mut world, thelsamar), Some("ERR_TAXINOTENOUGHMONEY"));
    let mut me = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
    me.single_mut(&mut world)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE,
            144,
        )]));
    assert_eq!(take(&mut world, thelsamar), None);
    me.single_mut(&mut world)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID,
            2402,
        )]));
    assert_eq!(
        take(&mut world, thelsamar),
        Some("ERR_TAXIPLAYERALREADYMOUNTED")
    );
    // A mount held for passengers is not mounted (`0x5914ea`): the flight goes.
    me.single_mut(&mut world).unwrap().0.hold_mount(2402);
    assert_eq!(take(&mut world, thelsamar), None);
    let sent: Vec<_> = rx
        .try_iter()
        .filter(|c| matches!(c, ClientCommand::ActivateTaxiExpress { .. }))
        .collect();
    assert!(
        matches!(
            sent.as_slice(),
            [
                ClientCommand::ActivateTaxiExpress { guid: MASTER, nodes: a },
                ClientCommand::ActivateTaxiExpress { guid: MASTER, nodes: b },
            ] if a == &[2, 6, 8] && b == &[2, 6, 8]
        ),
        "the chain alone, once on foot and once held: {sent:?}"
    );
}

/// The setup's tail and `TakeTaxiNode` read one mounted test (`0x5937e1`-`0x5937ea`,
/// `0x5914e1`-`0x5914ea`): a mount held for passengers is left alone, a ridden one dismounts.
#[test]
fn the_setup_leaves_a_mount_held_for_passengers() {
    let Some((mut world, rx)) = stormwind() else {
        return;
    };
    let mut me = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
    me.single_mut(&mut world)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID,
            2402,
        )]));
    me.single_mut(&mut world).unwrap().0.hold_mount(2402);
    close_map(&mut world);
    let _ = rx.try_iter().count();
    setup_map(&mut world, MASTER, 2, TaxiMask(vec![0b1010_1010]));
    assert_eq!(world.resource::<TaxiState>().unit, Some(MASTER), "opened");
    let dismounts = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::CancelMountAura))
            .count()
    };
    assert_eq!(dismounts(&rx), 0, "held: no dismount");
    me.single_mut(&mut world)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID,
            2410,
        )]));
    close_map(&mut world);
    setup_map(&mut world, MASTER, 2, TaxiMask(vec![0b1010_1010]));
    assert_eq!(dismounts(&rx), 1, "ridden: one dismount");
}
