//! A player's body waits for its worn templates ([`Equipment::settled`]), so an answer that never
//! comes leaves the player invisible with nothing said. This names the wait once it outlasts
//! [`WAIT_WARN_SECS`]: the player and every worn entry still without a template.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::items::Items;
use crate::net::{Guid, ObjectStore};

use super::{Equipment, COMPOSITE_SLOTS};

/// Long past a template round trip on any live link; short enough to say so while one looks.
const WAIT_WARN_SECS: f64 = 5.0;

/// The equipment slots `resolve_equipment` holds `settled` on: the composite's, the cloak (14)
/// and the helm (0).
fn gating_slots() -> impl Iterator<Item = u8> {
    COMPOSITE_SLOTS.iter().map(|(slot, _)| *slot).chain([14, 0])
}

/// The worn entries of `store` with no cached template, answered-unknown ones included: those
/// never settle either.
fn missing_templates(store: &ObjectStore, items: &Items) -> Vec<u32> {
    gating_slots()
        .filter_map(|slot| store.0.player_visible_item_entry(slot))
        .filter(|entry| items.template_cached(*entry).is_none())
        .collect()
}

/// Each unsettled player's first unsettled time and whether it has been named.
#[derive(Default)]
pub(in crate::entities) struct Waits(HashMap<Entity, (f64, bool)>);

impl Waits {
    /// One frame's unsettled players at `now`; returns those whose wait crossed
    /// [`WAIT_WARN_SECS`] this frame, with its length. A player absent here (settled or gone) is
    /// forgotten, so a later wait is named afresh.
    fn step(&mut self, now: f64, unsettled: impl Iterator<Item = Entity>) -> Vec<(Entity, f64)> {
        let mut due = Vec::new();
        let mut still = HashMap::with_capacity(self.0.len());
        for entity in unsettled {
            let (since, mut warned) = self.0.get(&entity).copied().unwrap_or((now, false));
            if !warned && now - since >= WAIT_WARN_SECS {
                warned = true;
                due.push((entity, now - since));
            }
            still.insert(entity, (since, warned));
        }
        self.0 = still;
        due
    }
}

/// Warn once per player whose equipment stays unsettled past [`WAIT_WARN_SECS`].
pub(in crate::entities) fn warn_unsettled_equipment(
    time: Res<Time>,
    items: Res<Items>,
    players: Query<(Entity, &Guid, &Equipment, &ObjectStore)>,
    mut waits: Local<Waits>,
) {
    let unsettled = players
        .iter()
        .filter(|(_, _, eq, _)| !eq.settled)
        .map(|(entity, ..)| entity);
    for (entity, waited) in waits.step(time.elapsed_secs_f64(), unsettled) {
        let Ok((_, guid, _, store)) = players.get(entity) else {
            continue;
        };
        warn!(
            "equipment: player {:#x} waits on item template(s) {:?} after {waited:.0} s; its body \
             is not built until they answer",
            guid.0,
            missing_templates(store, &items),
        );
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::ObjectFields;

    use super::*;

    /// `PLAYER_VISIBLE_ITEM_1_CREATOR`, 12 fields per slot, the entry at +2 (1.12.1 layout).
    fn wearing(entries: &[(u8, u32)]) -> ObjectStore {
        let pairs: Vec<(u16, u32)> = entries
            .iter()
            .map(|(slot, entry)| (258 + 2 + 12 * u16::from(*slot), *entry))
            .collect();
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    #[test]
    fn only_the_gating_entries_without_a_template_are_named() {
        let mut items = Items::default();
        items.insert_template(300, Some(crate::items::test_template("Chest")));
        items.insert_template(100, None);
        // Chest (4) answered, legs (6) pending, helm (0) unknown, a weapon (15) not gating.
        let store = wearing(&[(4, 300), (6, 147), (0, 100), (15, 999)]);
        assert_eq!(missing_templates(&store, &items), vec![147, 100]);
    }

    #[test]
    fn a_wait_is_named_once_after_the_threshold() {
        let p = World::new().spawn_empty().id();
        let mut waits = Waits::default();
        assert!(waits.step(10.0, [p].into_iter()).is_empty());
        assert!(waits.step(14.9, [p].into_iter()).is_empty());
        assert_eq!(waits.step(15.0, [p].into_iter()), vec![(p, 5.0)]);
        assert!(waits.step(60.0, [p].into_iter()).is_empty(), "named once");
    }

    #[test]
    fn settling_resets_the_wait() {
        let p = World::new().spawn_empty().id();
        let mut waits = Waits::default();
        waits.step(0.0, [p].into_iter());
        // Settled at 3 s, waiting again from 4 s: the clock restarts there.
        waits.step(3.0, std::iter::empty());
        waits.step(4.0, [p].into_iter());
        assert!(waits.step(8.5, [p].into_iter()).is_empty());
        assert_eq!(waits.step(9.0, [p].into_iter()), vec![(p, 5.0)]);
    }
}
