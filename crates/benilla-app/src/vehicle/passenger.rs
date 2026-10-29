//! Passengers: a unit whose movement transport is a vehicle or a player sits in one of its seats
//! (`0x6ea1d0` → `0x74cf30` → `0x74b200`), the reference's `CVehiclePassenger_C`. The state runs
//! 0 (none) to 3 (seated) and back; the animated enter and exit states 1, 2, 4 and 5 are the
//! transitions' timing, built with their motion. Each transition of our player or a group member
//! fires the `UNIT_*_VEHICLE` events (`0x749fb0`), and one that touches our own vehicle chain fires
//! `VEHICLE_PASSENGERS_CHANGED` (`0x74ad70`).

use bevy::prelude::*;

use crate::net::{Guid, GuidIndex, ObjectStore, SelfGuid};
use crate::transport::TransportRider;

use super::{Vehicle, VehicleSignals, VehicleTables};

/// Seated (`m_state` 3).
pub(crate) const SEATED: u8 = 3;

/// A unit seated in a vehicle: the vehicle's guid and the seat slot.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Passenger {
    pub(crate) vehicle: u64,
    pub(crate) seat: u8,
    pub(crate) state: u8,
    /// Our seat stopped its movement ahead of a local seat change or dismiss (`m_flags & 0x8`).
    pub(crate) stopped: bool,
}

/// A vehicle or a player guid, the transports a passenger rides (`0x74b8b0`): `HighGuid` `0xF150`,
/// or a player's (top nibble clear and a non-zero counter).
pub(crate) fn is_vehicle_transport(guid: u64) -> bool {
    let hi = (guid >> 32) as u32;
    (hi & 0xF0F0_0000) == 0xF050_0000
        || (hi & 0xF000_0000 == 0 && guid & 0x007F_FFFF_FFFF_FFFF != 0)
}

/// One passenger transition, with the seat's facts its events carry (`0x748810`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transition {
    pub(crate) unit: u64,
    pub(crate) from: u8,
    pub(crate) to: u8,
    /// Seat flag `0x20000000`, the has-vehicle-UI bit.
    pub(crate) vehicle_ui: bool,
    /// `"Natural"`, `"Mechanical"` or `""`, from the seat's skin (`0xad8660`).
    pub(crate) skin: &'static str,
    /// The enter (`0x748810`) or exit (`0x74970f`) sound's `SoundEntries` name, or `""`.
    pub(crate) sound: String,
    /// Seat flag `0x800`, the control seat.
    pub(crate) control_seat: bool,
    /// The vehicle's `VehicleUIIndicator` id.
    pub(crate) indicator: u32,
}

/// `0x749fb0`'s event bits for a transition: 1 entering, 2 entered, 4 exiting, 8 exited.
pub(crate) fn event_bits(from: u8, to: u8) -> u8 {
    match to {
        0 if from == SEATED => 12,
        0 => 8,
        1 => 1,
        2 if from == 1 => 0,
        2 => 1,
        SEATED if from == 0 || from == SEATED => 3,
        SEATED => 2,
        4 => 4,
        5 if from == 4 => 0,
        5 => 4,
        _ => 0,
    }
}

/// A seat's skin word (`0x6137d0`'s table): 0 natural, 1 mechanical, else nothing.
pub(crate) fn skin_name(skin: i32) -> &'static str {
    match skin {
        0 => "Natural",
        1 => "Mechanical",
        _ => "",
    }
}

/// Seat or unseat every unit from its movement transport each frame: a held vehicle with a
/// vehicle record seats it (`0x74b810` refuses a unit without one), anything else unseats it.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn track_passengers(
    mut commands: Commands,
    units: Query<(Entity, &Guid, Option<&TransportRider>, Option<&Passenger>), With<ObjectStore>>,
    vehicles: Query<&Vehicle>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    group: Option<Res<crate::ui_party::GroupState>>,
    tables: Res<VehicleTables>,
    mut signals: ResMut<VehicleSignals>,
) {
    let vehicle_of = |guid: u64| index.0.get(&guid).and_then(|&e| vehicles.get(e).ok());
    let passenger_of = |guid: u64| {
        index
            .0
            .get(&guid)
            .and_then(|&e| units.get(e).ok())
            .and_then(|(_, _, _, p)| p.copied())
    };
    // The top of a passenger chain (`0x74c650`): a unit riding nothing is its own root.
    let root = |mut guid: u64| {
        for _ in 0..8 {
            match passenger_of(guid) {
                Some(p) => guid = p.vehicle,
                None => break,
            }
        }
        guid
    };
    let me = self_guid.0.unwrap_or(0);
    let my_root = root(me);
    let in_group = |guid: u64| {
        guid == me
            || group
                .as_deref()
                .is_some_and(|g| g.members.iter().any(|m| m.guid == guid))
    };
    for (e, guid, rider, held) in &units {
        let guid = guid.0;
        let wanted = rider
            .filter(|r| is_vehicle_transport(r.transport_guid) && r.seat >= 0)
            .filter(|r| vehicle_of(r.transport_guid).is_some())
            .map(|r| Passenger {
                vehicle: r.transport_guid,
                seat: r.seat as u8,
                state: SEATED,
                stopped: held.is_some_and(|h| {
                    h.vehicle == r.transport_guid && i8::try_from(h.seat) == Ok(r.seat) && h.stopped
                }),
            });
        if wanted == held.copied() {
            continue;
        }
        // A seat change reads as seated to seated; the old vehicle's seat names the exit.
        let (from, to) = (held.map_or(0, |p| p.state), wanted.map_or(0, |p| p.state));
        let seat_side = wanted.or(held.copied()).expect("one side is seated");
        match wanted {
            Some(p) => {
                commands.entity(e).insert(p);
            }
            None => {
                commands.entity(e).remove::<Passenger>();
            }
        }
        // The vehicle's root against ours, before or after the change (`0x74ad70`).
        if root(seat_side.vehicle) == my_root || guid == me {
            signals.passengers_changed = true;
        }
        if !in_group(guid) {
            continue;
        }
        let vehicle = vehicle_of(seat_side.vehicle);
        let rec = vehicle.and_then(|v| tables.vehicle(v.id));
        let seat = vehicle.and_then(|v| tables.seat(v.id, usize::from(seat_side.seat)));
        let entering = to == SEATED;
        signals.transitions.push(Transition {
            unit: guid,
            from,
            to,
            vehicle_ui: seat.is_some_and(|s| s.flags & 0x2000_0000 != 0),
            skin: seat.map_or("", |s| skin_name(s.ui_skin)),
            sound: seat
                .map(|s| {
                    let id = if entering {
                        s.enter_ui_sound_id
                    } else {
                        s.exit_ui_sound_id
                    };
                    tables.sound_name(id).to_string()
                })
                .unwrap_or_default(),
            control_seat: seat.is_some_and(|s| s.flags & 0x800 != 0),
            indicator: rec.map_or(0, |r| r.ui_indicator_id),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use benilla_formats::{VehicleCatalog, VehicleRec, VehicleSeatRec};
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x10;
    const CART: u64 = 0xF150_0000_0000_0001;
    const STRANGER: u64 = 0x20;
    const OTHER_CART: u64 = 0xF150_0000_0000_0002;

    fn rider(vehicle: u64, seat: i8) -> TransportRider {
        TransportRider {
            transport_guid: vehicle,
            local_pos: [0.0; 3],
            local_orientation: 0.0,
            seat,
        }
    }

    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let mut index = GuidIndex::default();
        let mut spawn = |w: &mut World, guid: u64| {
            let e = w
                .spawn((
                    Guid(guid),
                    ObjectStore(
                        ObjectFields::from_pairs(&[(2, 0x9)]).into_created(ObjectType::Unit),
                    ),
                ))
                .id();
            index.0.insert(guid, e);
            e
        };
        let me = spawn(&mut w, ME);
        let cart = spawn(&mut w, CART);
        let stranger = spawn(&mut w, STRANGER);
        let other = spawn(&mut w, OTHER_CART);
        for v in [cart, other] {
            w.entity_mut(v).insert(Vehicle {
                id: 36,
                raw_facing: 0.0,
            });
        }
        w.entity_mut(stranger).insert(rider(OTHER_CART, 0));
        w.insert_resource(index);
        w.insert_resource(SelfGuid(Some(ME)));
        let mut cat = VehicleCatalog::default();
        cat.vehicles.insert(
            36,
            VehicleRec {
                id: 36,
                seat_ids: [7, 0, 0, 0, 0, 0, 0, 0],
                ui_indicator_id: 5,
                ..Default::default()
            },
        );
        cat.seats.insert(
            7,
            VehicleSeatRec {
                id: 7,
                flags: 0x2000_0000 | 0x800,
                ui_skin: 1,
                ..Default::default()
            },
        );
        w.insert_resource(VehicleTables {
            catalog: Arc::new(cat),
            ..Default::default()
        });
        w.init_resource::<VehicleSignals>();
        (w, me, cart)
    }

    /// Our player boards a control seat with the vehicle UI and leaves it again; a stranger
    /// seated on another vehicle is a passenger with no events.
    #[test]
    fn our_player_boards_and_leaves_with_the_seat_facts() {
        let (mut w, me, _) = world();
        w.entity_mut(me).insert(rider(CART, 0));
        w.run_system_once(track_passengers).unwrap();
        assert_eq!(
            w.entity(me).get::<Passenger>(),
            Some(&Passenger {
                vehicle: CART,
                seat: 0,
                state: SEATED,
                stopped: false,
            })
        );
        let s = w.resource::<VehicleSignals>();
        assert_eq!(
            s.transitions.len(),
            1,
            "the stranger is silent: {:?}",
            s.transitions
        );
        let t = &s.transitions[0];
        assert_eq!((t.unit, t.from, t.to), (ME, 0, SEATED));
        assert!(t.vehicle_ui && t.control_seat);
        assert_eq!((t.skin, t.indicator), ("Mechanical", 5));
        assert!(s.passengers_changed);

        w.resource_mut::<VehicleSignals>().transitions.clear();
        w.entity_mut(me).remove::<TransportRider>();
        w.run_system_once(track_passengers).unwrap();
        assert!(w.entity(me).get::<Passenger>().is_none());
        let t = &w.resource::<VehicleSignals>().transitions[0];
        assert_eq!((t.from, t.to), (SEATED, 0));
    }

    /// A transport without a vehicle record seats nobody (`0x74b810`).
    #[test]
    fn no_vehicle_record_no_seat() {
        let (mut w, me, cart) = world();
        w.entity_mut(cart).remove::<Vehicle>();
        w.entity_mut(me).insert(rider(CART, 0));
        w.run_system_once(track_passengers).unwrap();
        assert!(w.entity(me).get::<Passenger>().is_none());
    }

    #[test]
    fn vehicle_transports_are_vehicles_and_players() {
        assert!(is_vehicle_transport(0xF150_0000_0000_0001), "a vehicle");
        assert!(is_vehicle_transport(0x0000_0000_0000_0010), "a player");
        assert!(!is_vehicle_transport(0x1FC0_0000_0000_2B10), "a boat");
        assert!(!is_vehicle_transport(0xF130_0000_0000_0001), "a creature");
        assert!(!is_vehicle_transport(0), "nobody");
    }

    /// `0x749fb0`: on, off, and a seat change.
    #[test]
    fn transitions_map_to_their_event_bits() {
        assert_eq!(event_bits(0, SEATED), 3, "entering, entered");
        assert_eq!(event_bits(SEATED, 0), 12, "exiting, exited");
        assert_eq!(event_bits(SEATED, SEATED), 3, "a seat change enters again");
        assert_eq!(event_bits(1, 2), 0);
        assert_eq!(event_bits(4, 5), 0);
        assert_eq!(event_bits(0, 1), 1);
    }
}
