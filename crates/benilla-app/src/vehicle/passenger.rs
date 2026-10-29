//! Passengers: a unit whose movement transport is a vehicle or a player sits in one of its seats
//! (`0x6ea1d0` → `0x74cf30` → `0x74b200`), the reference's `CVehiclePassenger_C`. The state runs
//! 0 (none) to 3 (seated) and back; the animated enter and exit states 1, 2, 4 and 5 are the
//! transitions' timing, built with their motion. Each transition of our player or a group member
//! fires the `UNIT_*_VEHICLE` events (`0x749fb0`), and one that touches our own vehicle chain fires
//! `VEHICLE_PASSENGERS_CHANGED` (`0x74ad70`).

use bevy::prelude::*;

use crate::net::{Guid, GuidIndex, ObjectStore, SelfGuid};
use crate::transport::TransportRider;

use benilla_formats::VehicleSeatRec;

use super::{Vehicle, VehicleSignals, VehicleTables};

/// Seated (`m_state` 3).
pub(crate) const SEATED: u8 = 3;

/// A unit seated in a vehicle: the vehicle's guid and the seat slot.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Passenger {
    pub(crate) vehicle: u64,
    pub(crate) seat: u8,
    pub(crate) state: u8,
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

/// A transition the server's own move carries animates (`0x74b840`'s `0x20000000`): the transport
/// change came with a monster move (`0x73c8e0`) or a relayed move (`0x6ef860`'s heartbeat). The
/// object handlers set it; [`track_passengers`] reads and clears it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct TransitionCue;

/// An animated enter or exit under way: when its state began and ends, and what the arc runs
/// with. The end and the arc are `0x74a200`'s, filled once the start pose is known.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transit {
    pub(crate) start_s: f64,
    pub(crate) end_s: Option<f64>,
    /// The model's world position and yaw when the state began.
    pub(crate) from: Option<(Vec3, f32)>,
    pub(crate) gravity: f32,
    pub(crate) arc_scale: f32,
}

impl Transit {
    fn at(now: f64) -> Self {
        Self {
            start_s: now,
            end_s: None,
            from: None,
            gravity: 0.0,
            arc_scale: 1.0,
        }
    }
}

/// The pre-delay states' end (`0x74a200`): the seat's delay, at most 10 s.
fn pre_delay_end(now: f64, delay: f32) -> f64 {
    now + f64::from(delay.min(10.0))
}

/// `0x749aa0`'s first state for an animated enter: the pre-delay with one, else the motion;
/// none for a seat that does not animate it (`flags & 0x1`, a speed).
pub(crate) fn enter_state(seat: &VehicleSeatRec) -> Option<u8> {
    (seat.flags & 0x1 != 0 && seat.enter_speed > 0.0001)
        .then_some(if seat.enter_pre_delay <= 0.0001 { 2 } else { 1 })
}

/// The same for an exit: the seat's exit bit, `0x8` for a voluntary exit and `0x8000` for any
/// other (`0x74bb60`), and a speed.
pub(crate) fn exit_state(seat: &VehicleSeatRec, voluntary: bool) -> Option<u8> {
    let bit = if voluntary { 0x8 } else { 0x8000 };
    (seat.flags & bit != 0 && seat.exit_speed > 0.0001)
        .then_some(if seat.exit_pre_delay <= 0.0001 { 5 } else { 4 })
}

/// Seat or unseat every unit from its movement transport each frame: a held vehicle with a
/// vehicle record seats it (`0x74b810` refuses a unit without one), anything else unseats it.
/// The server's own moves animate the change where the seat does (`0x749aa0`); a transition under
/// way runs out in [`advance_transits`].
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn track_passengers(
    mut commands: Commands,
    units: Query<
        (
            Entity,
            &Guid,
            Option<&TransportRider>,
            Option<&Passenger>,
            Has<TransitionCue>,
            Option<&super::aim::MoveFlags2>,
        ),
        With<ObjectStore>,
    >,
    vehicles: Query<&Vehicle>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    group: Option<Res<crate::ui_party::GroupState>>,
    tables: Res<VehicleTables>,
    mut signals: ResMut<VehicleSignals>,
    mut stop: ResMut<super::SeatStop>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_secs_f64();
    // A seat's stop lasts 2 s at most (`0x74af70`).
    if stop.at.is_some_and(|at| now - at >= super::SEAT_STOP_SECS) {
        stop.at = None;
    }
    let vehicle_of = |guid: u64| index.0.get(&guid).and_then(|&e| vehicles.get(e).ok());
    let passenger_of = |guid: u64| {
        index
            .0
            .get(&guid)
            .and_then(|&e| units.get(e).ok())
            .and_then(|(_, _, _, p, ..)| p.copied())
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
    let ctx = SignalCtx {
        me,
        my_root: root(me),
        group: group.as_deref(),
        tables: &tables,
    };
    for (e, guid, rider, held, cue, flags2) in &units {
        let guid = guid.0;
        if cue {
            commands.entity(e).remove::<TransitionCue>();
        }
        let wanted = rider
            .filter(|r| is_vehicle_transport(r.transport_guid) && r.seat >= 0)
            .filter(|r| vehicle_of(r.transport_guid).is_some())
            .map(|r| Passenger {
                vehicle: r.transport_guid,
                seat: r.seat as u8,
                state: SEATED,
            });
        let held_c = held.copied();
        // A transition under way keeps its course while the transport agrees with it.
        if let Some(h) = held_c {
            let same = |w: &Passenger| w.vehicle == h.vehicle && w.seat == h.seat;
            if matches!(h.state, 1 | 2) && wanted.as_ref().is_some_and(same) {
                continue;
            }
            if matches!(h.state, 4 | 5) && wanted.is_none() {
                continue;
            }
        }
        if wanted == held_c {
            continue;
        }
        let seat_rec = |p: &Passenger| {
            vehicle_of(p.vehicle).and_then(|v| tables.seat(v.id, usize::from(p.seat)))
        };
        let from = held_c.map_or(0, |p| p.state);
        let next = match wanted {
            // `0x749aa0`: a server move animates an enter where the seat does.
            Some(w) => {
                let state = cue
                    .then(|| seat_rec(&w).and_then(enter_state))
                    .flatten()
                    .unwrap_or(SEATED);
                Some(Passenger { state, ..w })
            }
            // An exit animates from a seat that allows it, under a server move or a seat
            // flagged to animate any exit (`flagsB & 0x1000000`).
            None => held_c.and_then(|h| {
                let seat = seat_rec(&h)?;
                let allowed = cue || seat.flags_b & 0x100_0000 != 0;
                let voluntary = flags2.is_some_and(|f| f.0 & 0x40 != 0);
                let state = allowed.then(|| exit_state(seat, voluntary)).flatten()?;
                Some(Passenger { state, ..h })
            }),
        };
        let to = next.map_or(0, |p| p.state);
        let seat_side = next.or(held_c).expect("one side is seated");
        match next {
            Some(p) => {
                commands.entity(e).insert(p);
            }
            None => {
                commands.entity(e).remove::<Passenger>();
            }
        }
        if matches!(to, 1 | 2 | 4 | 5) {
            let mut t = Transit::at(now);
            if let Some(seat) = seat_rec(&seat_side) {
                if to == 1 {
                    t.end_s = Some(pre_delay_end(now, seat.enter_pre_delay));
                } else if to == 4 {
                    t.end_s = Some(pre_delay_end(now, seat.exit_pre_delay));
                }
            }
            commands.entity(e).insert(t);
        } else {
            commands.entity(e).remove::<Transit>();
        }
        // Every state change clears our seat's stop (`0x7489c0`).
        if guid == me {
            stop.at = None;
        }
        signal(
            &ctx,
            &mut signals,
            guid,
            from,
            to,
            &seat_side,
            &root,
            &vehicle_of,
        );
    }
}

/// What a transition's signals read.
struct SignalCtx<'a> {
    me: u64,
    my_root: u64,
    group: Option<&'a crate::ui_party::GroupState>,
    tables: &'a VehicleTables,
}

/// One state change's interface signals: `VEHICLE_PASSENGERS_CHANGED` for one touching our
/// chain (`0x74ad70`), and the transition events for our player or a group member (`0x749fb0`).
#[allow(clippy::too_many_arguments)] // the change and what it reads, one each
fn signal<'v>(
    ctx: &SignalCtx<'_>,
    signals: &mut VehicleSignals,
    guid: u64,
    from: u8,
    to: u8,
    seat_side: &Passenger,
    root: &impl Fn(u64) -> u64,
    vehicle_of: &impl Fn(u64) -> Option<&'v Vehicle>,
) {
    // The vehicle's root against ours, before or after the change, on a step to or from 0 or 3.
    let settles = |s: u8| s == 0 || s == SEATED;
    if (root(seat_side.vehicle) == ctx.my_root || guid == ctx.me) && (settles(from) || settles(to))
    {
        signals.passengers_changed = true;
    }
    let in_group = guid == ctx.me
        || ctx
            .group
            .is_some_and(|g| g.members.iter().any(|m| m.guid == guid));
    if !in_group {
        return;
    }
    let tables = ctx.tables;
    let vehicle = vehicle_of(seat_side.vehicle);
    let rec = vehicle.and_then(|v| tables.vehicle(v.id));
    let seat = vehicle.and_then(|v| tables.seat(v.id, usize::from(seat_side.seat)));
    let entering = matches!(to, 1..=3);
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

/// Run each transition to its next state at its end (`0x74af70`): the pre-delay to the motion,
/// the motion to seated or to none.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn advance_transits(
    mut commands: Commands,
    units: Query<(Entity, &Guid, &Passenger, Option<&Transit>)>,
    vehicles: Query<&Vehicle>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    group: Option<Res<crate::ui_party::GroupState>>,
    tables: Res<VehicleTables>,
    mut signals: ResMut<VehicleSignals>,
    mut stop: ResMut<super::SeatStop>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_secs_f64();
    if units.iter().all(|(.., t)| t.is_none()) {
        return;
    }
    let vehicle_of = |guid: u64| index.0.get(&guid).and_then(|&e| vehicles.get(e).ok());
    // Each passenger's vehicle, for the chain roots the signals compare.
    let rides: std::collections::HashMap<u64, u64> =
        units.iter().map(|(_, g, p, _)| (g.0, p.vehicle)).collect();
    let root = |mut guid: u64| {
        for _ in 0..8 {
            match rides.get(&guid) {
                Some(&v) => guid = v,
                None => break,
            }
        }
        guid
    };
    let me = self_guid.0.unwrap_or(0);
    let ctx = SignalCtx {
        me,
        my_root: root(me),
        group: group.as_deref(),
        tables: &tables,
    };
    let mut changes = Vec::new();
    for (e, guid, p, t) in &units {
        let Some(t) = t else { continue };
        let Some(end) = t.end_s.filter(|&end| now >= end) else {
            continue;
        };
        let from = p.state;
        let to = match from {
            1 => 2,
            2 => SEATED,
            4 => 5,
            5 => 0,
            _ => continue,
        };
        let seat_side = *p;
        // Written through commands, so the passenger's readers see the step whole next frame.
        match to {
            0 => {
                commands.entity(e).remove::<(Passenger, Transit)>();
            }
            SEATED => {
                commands
                    .entity(e)
                    .insert(Passenger {
                        state: SEATED,
                        ..*p
                    })
                    .remove::<Transit>();
            }
            _ => {
                commands
                    .entity(e)
                    .insert((Passenger { state: to, ..*p }, Transit::at(end)));
            }
        }
        if guid.0 == me {
            stop.at = None;
        }
        changes.push((guid.0, from, to, seat_side));
    }
    for (guid, from, to, seat_side) in changes {
        signal(
            &ctx,
            &mut signals,
            guid,
            from,
            to,
            &seat_side,
            &root,
            &vehicle_of,
        );
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
        w.init_resource::<super::super::SeatStop>();
        w.init_resource::<Time<Real>>();
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

    /// A server move onto an animated seat runs the pre-delay and the motion before the seat,
    /// firing entering first and entered last.
    #[test]
    fn a_cued_enter_animates_through_its_states() {
        let (mut w, me, _) = world();
        let mut cat = (*w.resource::<VehicleTables>().catalog).clone();
        let seat = cat.seats.get_mut(&7).unwrap();
        seat.flags |= 0x1;
        seat.enter_speed = 2.0;
        seat.enter_pre_delay = 0.5;
        w.resource_mut::<VehicleTables>().catalog = Arc::new(cat);
        w.entity_mut(me).insert((rider(CART, 0), TransitionCue));
        w.run_system_once(track_passengers).unwrap();
        assert_eq!(w.entity(me).get::<Passenger>().unwrap().state, 1);
        assert!(
            w.entity(me).get::<TransitionCue>().is_none(),
            "the cue is spent"
        );
        let t = *w.entity(me).get::<Transit>().unwrap();
        assert_eq!(t.end_s, Some(t.start_s + 0.5));
        // Past the pre-delay: the motion, whose end the drawing fills; then seated.
        w.resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs(1));
        w.run_system_once(advance_transits).unwrap();
        assert_eq!(w.entity(me).get::<Passenger>().unwrap().state, 2);
        w.entity_mut(me).get_mut::<Transit>().unwrap().end_s = Some(0.0);
        w.run_system_once(advance_transits).unwrap();
        assert_eq!(w.entity(me).get::<Passenger>().unwrap().state, SEATED);
        assert!(w.entity(me).get::<Transit>().is_none());
        let steps: Vec<_> = w
            .resource::<VehicleSignals>()
            .transitions
            .iter()
            .map(|t| (t.from, t.to))
            .collect();
        assert_eq!(steps, [(0, 1), (1, 2), (2, SEATED)]);
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
