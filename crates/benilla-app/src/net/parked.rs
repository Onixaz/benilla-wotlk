//! Objects out of range, kept as the reference's lazy-cleanup set (`0x4d6fc0`): a later create,
//! values, movement or near block for the guid revives the object (`0x4d6ae0`) with the state it
//! left with. The oldest of a type goes 120 s after it left, checked once per update packet
//! (`0x4d48c0`), or 10 s after when a new object of that type is made (`0x4d4930`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use benilla_protocol::{EntityKind, ObjectFields};
use benilla_world::vis_chain::VisChainOnly;
use bevy::prelude::*;

use super::{Guid, GuidIndex, NetEntity, ObjectStore, UnitSpeeds};

/// A parked object's life when nothing names it again: the per-packet sweep's age.
pub(super) const PARK_EXPIRE: Duration = Duration::from_secs(120);
/// The age past which a new object of the same type frees the oldest parked one.
pub(super) const ALLOC_EXPIRE: Duration = Duration::from_secs(10);

/// The reference's per-type lists, one per object type id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum ParkType {
    Item,
    Container,
    World(EntityKind),
}

/// What a parked object takes back when it revives.
pub(super) enum Parked {
    World {
        net: NetEntity,
        store: ObjectFields,
        transform: Transform,
        speeds: Option<UnitSpeeds>,
        rider: Option<crate::transport::TransportRider>,
        deck: Option<crate::transport::DeckPassenger>,
    },
    Item {
        store: ObjectFields,
        container: bool,
        countdowns: crate::items::Countdowns,
    },
}

impl Parked {
    /// The descriptor it left with, which the reviving block's fields merge into.
    pub(super) fn store(&self) -> &ObjectFields {
        match self {
            Parked::World { store, .. } | Parked::Item { store, .. } => store,
        }
    }

    fn park_type(&self) -> ParkType {
        match self {
            Parked::World { net, .. } => ParkType::World(net.kind),
            Parked::Item {
                container: true, ..
            } => ParkType::Container,
            Parked::Item { .. } => ParkType::Item,
        }
    }
}

/// The parked objects by guid, and per type in park order.
#[derive(Resource, Default)]
pub(crate) struct ParkedObjects {
    by_guid: HashMap<u64, (Parked, Instant)>,
    fifo: HashMap<ParkType, VecDeque<(u64, Instant)>>,
}

impl ParkedObjects {
    pub(super) fn park(&mut self, guid: u64, parked: Parked, now: Instant) {
        self.fifo
            .entry(parked.park_type())
            .or_default()
            .push_back((guid, now));
        self.by_guid.insert(guid, (parked, now));
    }

    /// Takes the guid's parked state for a revive.
    pub(super) fn take(&mut self, guid: u64) -> Option<Parked> {
        self.by_guid.remove(&guid).map(|(p, _)| p)
    }

    pub(super) fn contains(&self, guid: u64) -> bool {
        self.by_guid.contains_key(&guid)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.by_guid.len()
    }

    /// Frees each type's oldest parked object once it is `age` old, one per type per call.
    pub(super) fn expire_oldest(&mut self, now: Instant, age: Duration) {
        let types: Vec<ParkType> = self.fifo.keys().copied().collect();
        for t in types {
            self.expire_oldest_of(t, now, age);
        }
    }

    /// Frees the type's oldest parked object once it is `age` old; entries left by a revive or a
    /// later re-park of the same guid are skipped.
    pub(super) fn expire_oldest_of(&mut self, t: ParkType, now: Instant, age: Duration) {
        let Some(q) = self.fifo.get_mut(&t) else {
            return;
        };
        while let Some(&(guid, at)) = q.front() {
            if self.by_guid.get(&guid).is_none_or(|(_, live)| *live != at) {
                q.pop_front();
                continue;
            }
            if now.saturating_duration_since(at) >= age {
                q.pop_front();
                self.by_guid.remove(&guid);
                trace_park("expire", guid);
            }
            return;
        }
    }
}

/// The type a new object of `kind` allocates under, for the 10 s rule.
pub(super) fn park_type_of(kind: EntityKind) -> ParkType {
    ParkType::World(kind)
}

/// Reads the entity's state into the parked set; the caller then tears the entity down.
pub(super) fn park_entity(world: &mut World, guid: u64, e: Entity, now: Instant) {
    let Ok(ent) = world.get_entity(e) else {
        return;
    };
    let Some(store) = ent.get::<ObjectStore>().map(|s| s.0.clone()) else {
        return;
    };
    let parked = if let Some(item) = ent.get::<crate::items::ItemObject>() {
        Parked::Item {
            store,
            container: item.container,
            countdowns: ent
                .get::<crate::items::Countdowns>()
                .cloned()
                .unwrap_or_default(),
        }
    } else if let (Some(net), Some(transform)) = (ent.get::<NetEntity>(), ent.get::<Transform>()) {
        Parked::World {
            net: *net,
            store,
            transform: *transform,
            speeds: ent.get::<UnitSpeeds>().copied(),
            rider: ent.get::<crate::transport::TransportRider>().copied(),
            deck: ent.get::<crate::transport::DeckPassenger>().copied(),
        }
    } else {
        return;
    };
    world
        .resource_mut::<ParkedObjects>()
        .park(guid, parked, now);
    trace_park("park", guid);
}

/// Spawns a revived object with what it parked with, its pose replaced by `pose` when the
/// reviving block moves it; the caller inserts its merged [`ObjectStore`].
pub(super) fn respawn(
    guid: u64,
    parked: Parked,
    pose: Option<Transform>,
    commands: &mut Commands,
    index: &mut GuidIndex,
) -> Entity {
    trace_park("revive", guid);
    match parked {
        Parked::Item {
            store,
            container,
            countdowns,
        } => {
            let e = crate::items::spawn_item(commands, index, guid, store, container);
            commands.entity(e).insert(countdowns);
            e
        }
        Parked::World {
            net,
            transform,
            speeds,
            rider,
            deck,
            ..
        } => {
            let mut entity = commands.spawn((
                Guid(guid),
                net,
                pose.unwrap_or(transform),
                Visibility::default(),
            ));
            entity.vis_chain_only();
            if let Some(s) = speeds {
                entity.insert(s);
            }
            if let Some(r) = rider {
                entity.insert(r);
            }
            if let Some(d) = deck {
                entity.insert(d);
            }
            let e = entity.id();
            index.0.insert(guid, e);
            e
        }
    }
}

/// The `park` trace tag: one line per park, revive and expiry.
fn trace_park(what: &str, guid: u64) {
    if benilla_assets::trace::enabled_for("park") {
        benilla_assets::trace::line("park", &format!("{what} {guid:#x}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Parked {
        Parked::World {
            net: NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            store: ObjectFields::default(),
            transform: Transform::default(),
            speeds: None,
            rider: None,
            deck: None,
        }
    }

    /// One type's oldest goes once it is old enough, one per sweep; another type's is untouched.
    #[test]
    fn the_sweep_frees_each_types_oldest_once_old_enough() {
        let t0 = Instant::now();
        let mut p = ParkedObjects::default();
        p.park(1, unit(), t0);
        p.park(2, unit(), t0 + Duration::from_secs(5));
        let go = Parked::World {
            net: NetEntity {
                kind: EntityKind::GameObject,
                display_id: None,
                scale: 1.0,
            },
            store: ObjectFields::default(),
            transform: Transform::default(),
            speeds: None,
            rider: None,
            deck: None,
        };
        p.park(3, go, t0);
        p.expire_oldest(t0 + Duration::from_secs(119), PARK_EXPIRE);
        assert_eq!(p.len(), 3, "nothing is 120 s old yet");
        p.expire_oldest(t0 + Duration::from_secs(130), PARK_EXPIRE);
        assert!(!p.contains(1) && !p.contains(3), "each type's oldest went");
        assert!(p.contains(2), "one per type per sweep");
    }

    /// A revive takes the guid out; a later re-park is timed from its own park, not the first.
    #[test]
    fn a_revived_then_reparked_guid_keeps_its_new_age() {
        let t0 = Instant::now();
        let mut p = ParkedObjects::default();
        p.park(7, unit(), t0);
        assert!(p.take(7).is_some());
        p.park(7, unit(), t0 + Duration::from_secs(100));
        p.expire_oldest_of(
            park_type_of(EntityKind::Unit),
            t0 + Duration::from_secs(125),
            PARK_EXPIRE,
        );
        assert!(
            p.contains(7),
            "the stale first entry does not expire the re-park"
        );
        p.expire_oldest_of(
            park_type_of(EntityKind::Unit),
            t0 + Duration::from_secs(111),
            ALLOC_EXPIRE,
        );
        assert!(
            !p.contains(7),
            "past 10 s, a new object of its type frees it"
        );
    }
}
