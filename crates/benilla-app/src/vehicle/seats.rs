//! A vehicle's virtual seats: its own slots, with each non-player vehicle riding it standing in
//! for its slot by its own seats (`0x757470` counts them, `0x757550` walks them), and each held
//! unit's root vehicle (`0x74c650`), for the seat-list verbs.

use std::collections::HashMap;

use benilla_ui::script::{VehicleTree, VirtualSeat};

use super::passenger::is_vehicle_transport;
use super::VehicleTables;

/// What the walk reads of one held unit.
#[derive(Clone, Debug, Default)]
pub(super) struct UnitFacts {
    /// Its movement transport's guid and seat byte, 0 and -1 for none.
    pub(super) transport: u64,
    pub(super) seat: i8,
    /// Registered as a passenger of its transport (`m_passengers`).
    pub(super) passenger: bool,
    /// Its `Vehicle.dbc` id with a record, `None` without.
    pub(super) vehicle: Option<u32>,
    pub(super) is_player: bool,
    pub(super) name: Option<String>,
}

/// `0x74c650`: from `guid`, up its vehicle transports to the top, stopping at `stop`; a unit
/// riding none is its own root when it has a vehicle. `None` when a transport is not held.
pub(super) fn root_vehicle(units: &HashMap<u64, UnitFacts>, guid: u64, stop: u64) -> Option<u64> {
    let me = units.get(&guid)?;
    if guid == stop && me.vehicle.is_some() {
        return Some(guid);
    }
    if !is_vehicle_transport(me.transport) {
        return me.vehicle.map(|_| guid);
    }
    let mut at = me.transport;
    // A chain is a handful of links; the bound only guards a loop the wire could build.
    for _ in 0..16 {
        let unit = units.get(&at)?;
        if at == stop || !is_vehicle_transport(unit.transport) {
            break;
        }
        at = unit.transport;
    }
    Some(at)
}

/// The passengers of each vehicle, by slot.
fn passengers(units: &HashMap<u64, UnitFacts>) -> HashMap<u64, Vec<(u8, u64)>> {
    let mut by_vehicle: HashMap<u64, Vec<(u8, u64)>> = HashMap::new();
    for (&guid, u) in units {
        if u.passenger && u.seat >= 0 {
            by_vehicle
                .entry(u.transport)
                .or_default()
                .push((u.seat as u8, guid));
        }
    }
    for list in by_vehicle.values_mut() {
        list.sort_unstable();
    }
    by_vehicle
}

/// The seat tree of every root with a vehicle record, and every held unit's root.
pub(super) fn build(
    units: &HashMap<u64, UnitFacts>,
    tables: &VehicleTables,
    mover: u64,
) -> (HashMap<u64, u64>, HashMap<u64, VehicleTree>) {
    let riders = passengers(units);
    let roots: HashMap<u64, u64> = units
        .keys()
        .filter_map(|&g| Some((g, root_vehicle(units, g, 0)?)))
        .collect();
    let mut trees = HashMap::new();
    for &root in roots.values() {
        if trees.contains_key(&root) {
            continue;
        }
        if units.get(&root).and_then(|u| u.vehicle).is_none() {
            continue;
        }
        let mut seats = Vec::new();
        walk(units, &riders, tables, root, mover, &mut seats, 0);
        trees.insert(
            root,
            VehicleTree {
                count: count(units, &riders, tables, root, 0),
                seats,
            },
        );
    }
    (roots, trees)
}

/// A non-player passenger with its own vehicle, which stands in for its slot.
fn nested<'a>(
    units: &'a HashMap<u64, UnitFacts>,
    riders: &'a HashMap<u64, Vec<(u8, u64)>>,
    vehicle: u64,
) -> impl Iterator<Item = (u8, u64)> + 'a {
    riders
        .get(&vehicle)
        .into_iter()
        .flatten()
        .copied()
        .filter(|(_, g)| {
            units
                .get(g)
                .is_some_and(|u| u.vehicle.is_some() && !u.is_player)
        })
}

/// `0x757470`: the slots with a seat id, plus each nested vehicle's count less the slot it fills.
fn count(
    units: &HashMap<u64, UnitFacts>,
    riders: &HashMap<u64, Vec<(u8, u64)>>,
    tables: &VehicleTables,
    vehicle: u64,
    depth: u32,
) -> i32 {
    let Some(rec) = units
        .get(&vehicle)
        .and_then(|u| u.vehicle)
        .and_then(|id| tables.vehicle(id))
    else {
        return 0;
    };
    let mut n = rec.seat_ids.iter().filter(|&&id| id > 0).count() as i32;
    if depth < 8 {
        for (_, g) in nested(units, riders, vehicle) {
            n += count(units, riders, tables, g, depth + 1) - 1;
        }
    }
    n
}

/// `0x757550`: slot by slot, a nested vehicle's seats in its place, else the slot's own seat.
fn walk(
    units: &HashMap<u64, UnitFacts>,
    riders: &HashMap<u64, Vec<(u8, u64)>>,
    tables: &VehicleTables,
    vehicle: u64,
    mover: u64,
    out: &mut Vec<VirtualSeat>,
    depth: u32,
) {
    let Some(id) = units.get(&vehicle).and_then(|u| u.vehicle) else {
        return;
    };
    let Some(rec) = tables.vehicle(id) else {
        return;
    };
    let mut stand_ins = [None; 8];
    for (slot, g) in nested(units, riders, vehicle) {
        if let Some(s) = stand_ins.get_mut(usize::from(slot)) {
            *s = Some(g);
        }
    }
    for (slot, stand_in) in stand_ins.into_iter().enumerate() {
        if let Some(g) = stand_in.filter(|_| depth < 8) {
            walk(units, riders, tables, g, mover, out, depth + 1);
            continue;
        }
        if rec.seat_ids[slot] == 0 {
            continue;
        }
        // `0x7579e0` reads the seat record; a slot whose record is missing lists nothing.
        let Some(seat) = tables.seat(id, slot) else {
            continue;
        };
        // `0x757680`: the passenger whose seat byte names the slot.
        let occupant = riders
            .get(&vehicle)
            .and_then(|l| l.iter().find(|(s, _)| usize::from(*s) == slot))
            .map_or(0, |&(_, g)| g);
        let name = units.get(&occupant).and_then(|u| u.name.clone());
        out.push(VirtualSeat {
            vehicle,
            seat: slot as u8,
            flags: seat.flags,
            flags_b: seat.flags_b,
            occupant,
            name,
            // benilla resolves no cross-realm names, so every occupant is of our realm.
            realm: None,
            under_mover: occupant != 0
                && mover != 0
                && root_vehicle(units, occupant, mover) == Some(mover),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use benilla_formats::{VehicleCatalog, VehicleRec, VehicleSeatRec};

    const ME: u64 = 0x10;
    const BOB: u64 = 0x22;
    const TANK: u64 = 0xF150_0000_0000_0001;
    const TURRET: u64 = 0xF150_0000_0000_0002;

    fn tables() -> VehicleTables {
        let mut cat = VehicleCatalog::default();
        cat.vehicles.insert(
            1,
            VehicleRec {
                id: 1,
                seat_ids: [10, 11, 12, 0, 0, 0, 0, 0],
                ..Default::default()
            },
        );
        cat.vehicles.insert(
            2,
            VehicleRec {
                id: 2,
                seat_ids: [20, 21, 0, 0, 0, 0, 0, 0],
                ..Default::default()
            },
        );
        for id in [10, 11, 12, 20, 21] {
            cat.seats.insert(
                id,
                VehicleSeatRec {
                    id,
                    flags: id,
                    ..Default::default()
                },
            );
        }
        VehicleTables {
            catalog: Arc::new(cat),
            ..Default::default()
        }
    }

    fn unit(transport: u64, seat: i8, vehicle: Option<u32>, is_player: bool) -> UnitFacts {
        UnitFacts {
            transport,
            seat,
            passenger: transport != 0,
            vehicle,
            is_player,
            name: is_player.then(|| "P".into()),
        }
    }

    /// We drive the tank's seat 0; a turret fills its seat 1 and lists its two seats there; Bob
    /// rides the turret's seat 1.
    #[test]
    fn a_nested_vehicle_lists_its_seats_in_its_slot() {
        let units = HashMap::from([
            (TANK, unit(0, -1, Some(1), false)),
            (TURRET, unit(TANK, 1, Some(2), false)),
            (ME, unit(TANK, 0, None, true)),
            (BOB, unit(TURRET, 1, None, true)),
        ]);
        let (roots, trees) = build(&units, &tables(), TANK);
        assert_eq!(roots[&BOB], TANK);
        assert_eq!(roots[&TURRET], TANK);
        assert_eq!(roots[&TANK], TANK);
        let t = &trees[&TANK];
        assert_eq!(t.count, 4, "three slots, the turret's adding one");
        let order: Vec<_> = t
            .seats
            .iter()
            .map(|s| (s.vehicle, s.seat, s.occupant))
            .collect();
        assert_eq!(
            order,
            [
                (TANK, 0, ME),
                (TURRET, 0, 0),
                (TURRET, 1, BOB),
                (TANK, 2, 0)
            ]
        );
        assert!(t.seats[2].under_mover, "Bob's chain reaches the tank");
        assert!(t.seats[0].under_mover, "so does ours");
    }

    /// A transport out of view leaves the chain rootless (`0x74c650` returns null).
    #[test]
    fn an_unheld_transport_has_no_root() {
        let units = HashMap::from([(BOB, unit(TURRET, 1, None, true))]);
        assert_eq!(root_vehicle(&units, BOB, 0), None);
    }
}
