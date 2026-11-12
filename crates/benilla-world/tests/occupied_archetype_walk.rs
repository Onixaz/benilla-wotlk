//! The patched `bevy_ecs` (`third_party/bevy_ecs/BENILLA.md`) walks only the archetypes that hold
//! an entity. A query over sparse-set markers must still return every matching entity once, in
//! ascending archetype order, through each way a query is driven, as archetypes empty and refill.

use std::collections::BTreeSet;

use bevy::ecs::archetype::ArchetypeId;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, TaskPool};

/// A table component every entity of the test carries.
#[derive(Component)]
struct Base;

/// Eight sparse-set markers, so their 256 combinations span several words of the occupancy set.
#[derive(Component)]
#[component(storage = "SparseSet")]
struct Mark<const N: usize>;

/// Give `e` the markers named by the bits of `bits`, one insert each, as the client's units gain
/// theirs: every prefix mints an archetype of its own.
fn mark(world: &mut World, e: Entity, bits: u32) {
    let mut e = world.entity_mut(e);
    macro_rules! bit {
        ($($n:literal),*) => {$(
            if bits & (1 << $n) != 0 {
                e.insert(Mark::<$n>);
            }
        )*};
    }
    bit!(0, 1, 2, 3, 4, 5, 6, 7);
}

/// Every entity with `Base` and `Mark<0>`, through the plain iterator, the fold path, the size
/// hint and the parallel walk, checked against the entities themselves.
fn check(world: &mut World, alive: &[(Entity, u32)], when: &str) {
    let want: BTreeSet<Entity> = alive
        .iter()
        .filter(|(_, bits)| bits & 1 != 0)
        .map(|(e, _)| *e)
        .collect();
    let mut query = world.query_filtered::<(Entity, &Base), With<Mark<0>>>();

    let stepped: Vec<Entity> = query.iter(world).map(|(e, _)| e).collect();
    assert_eq!(stepped.len(), want.len(), "{when}: the iterator's count");
    assert_eq!(
        stepped.iter().copied().collect::<BTreeSet<_>>(),
        want,
        "{when}: the iterator's entities"
    );
    let archetypes: Vec<ArchetypeId> = stepped
        .iter()
        .map(|&e| world.entity(e).archetype().id())
        .collect();
    assert!(
        archetypes.windows(2).all(|w| w[0] <= w[1]),
        "{when}: ascending archetype order"
    );

    let folded = query.iter(world).fold(Vec::new(), |mut acc, (e, _)| {
        acc.push(e);
        acc
    });
    assert_eq!(folded, stepped, "{when}: the fold path");

    assert_eq!(
        query.iter(world).size_hint().1,
        Some(want.len()),
        "{when}: the size hint"
    );
    assert_eq!(
        query.iter(world).count(),
        want.len(),
        "{when}: an archetypal count"
    );

    let seen = std::sync::Mutex::new(BTreeSet::new());
    query.par_iter(world).for_each(|(e, _)| {
        assert!(seen.lock().unwrap().insert(e), "{when}: visited twice");
    });
    assert_eq!(
        seen.into_inner().unwrap(),
        want,
        "{when}: the parallel walk"
    );
}

#[test]
fn a_sparse_query_visits_every_entity_as_archetypes_empty_and_refill() {
    ComputeTaskPool::get_or_init(TaskPool::default);
    let mut world = World::new();
    let spawn = |world: &mut World, n: u32| {
        let bits = n.wrapping_mul(37) & 0xff;
        let e = world.spawn(Base).id();
        mark(world, e, bits);
        (e, bits)
    };
    let mut alive: Vec<(Entity, u32)> = (0..600).map(|n| spawn(&mut world, n)).collect();
    assert!(
        world.archetypes().len() > 256,
        "the walk spans several words"
    );
    check(&mut world, &alive, "spawned");

    // Empty most archetypes: every entity but each seventh goes.
    let (kept, gone): (Vec<_>, Vec<_>) =
        alive.into_iter().enumerate().partition(|(i, _)| i % 7 == 0);
    for (_, (e, _)) in gone {
        world.despawn(e);
    }
    alive = kept.into_iter().map(|(_, row)| row).collect();
    check(&mut world, &alive, "mostly despawned");

    // Move the rest between archetypes: clear the first marker on half, set it on the others.
    for (i, (e, bits)) in alive.iter_mut().enumerate() {
        if i % 2 == 0 {
            world.entity_mut(*e).remove::<Mark<0>>();
            *bits &= !1;
        } else {
            world.entity_mut(*e).insert(Mark::<0>);
            *bits |= 1;
        }
    }
    check(&mut world, &alive, "moved");

    // Refill the emptied archetypes.
    alive.extend((1000..1400).map(|n| spawn(&mut world, n)));
    check(&mut world, &alive, "refilled");

    // Every archetype emptied at once, then filled again.
    world.clear_entities();
    check(&mut world, &[], "cleared");
    alive = (2000..2300).map(|n| spawn(&mut world, n)).collect();
    check(&mut world, &alive, "after the clear");
}
