//! The active mover as the vehicle UI hears of it: a new mover fires `VEHICLE_UPDATE` and sets
//! the aim sliders (`0x747f40`): `VEHICLE_ANGLE_SHOW` and `VEHICLE_POWER_SHOW` with 1 for a
//! vehicle whose record aims (`0x400`) or powers (`0x800`) and which may move, else bare
//! (`0x74c5a0`), then the angle (`VEHICLE_ANGLE_UPDATE`, `0x5fa910`).

use bevy::prelude::*;

use benilla_formats::VehicleRec;
use benilla_ui::script::{ScriptValue, UiScript};

use crate::net::{ActiveMover, Guid, GuidIndex, ObjectStore, SelfGuid};

use super::{Passenger, Vehicle, VehicleTables, SEATED};

/// The mover's aim pitch, which the aim verbs move.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct VehicleAim {
    pub(crate) pitch: f32,
}

/// `0x74ba40`, for the mover: a transition holds it, or its charmer's seat stops it, or nobody
/// it may move holds it.
fn movement_prevented(
    mover: u64,
    store: &ObjectStore,
    passenger_of: impl Fn(u64) -> Option<Passenger>,
    me: u64,
    stopped: bool,
) -> bool {
    if passenger_of(mover).is_some_and(|p| p.state != 0 && p.state != SEATED) {
        return true;
    }
    if let Some(charmer) = store.0.unit_charmed_by() {
        // A charmer seated in a vehicle that stopped its movement (`m_flags & 0x8`); only our own
        // seat stops (`0x747fa0`).
        return charmer == me && stopped && passenger_of(me).is_some_and(|p| p.state == SEATED);
    }
    // `UNIT_FLAG_POSSESSED` (`0x1000000`) or our own body.
    store.0.unit_flags() & 0x100_0000 == 0 && mover != me
}

/// `0x5fa910`: the aim pitch normalised over the record's limits (`flags & 0x40`), else over
/// ±π/2, and the angle itself.
pub(super) fn angle_update(rec: &VehicleRec, pitch: f32) -> [f32; 2] {
    let (min, max) = if rec.flags & 0x40 != 0 {
        (rec.pitch_min, rec.pitch_max)
    } else {
        (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
    };
    let range = max - min;
    let norm = if range.abs() <= 0.0001 {
        0.0
    } else {
        (pitch - min) / range
    };
    [norm, pitch]
}

/// Fire the mover's UI events when the active mover changes (`0x729010` → `0x747f40`).
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
pub(super) fn on_mover_change(
    script: Option<NonSendMut<UiScript>>,
    mover: Query<(&Guid, &ObjectStore, Option<&Vehicle>), With<ActiveMover>>,
    passengers: Query<&Passenger>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    tables: Res<VehicleTables>,
    aim: Res<VehicleAim>,
    stop: Res<super::SeatStop>,
    mut last: Local<u64>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = mover.single().ok();
    let guid = now.map_or(0, |(g, ..)| g.0);
    if std::mem::replace(&mut *last, guid) == guid {
        return;
    }
    let passenger_of = |g: u64| {
        index
            .0
            .get(&g)
            .and_then(|&e| passengers.get(e).ok())
            .copied()
    };
    let rec = now.and_then(|(_, _, v)| tables.vehicle(v?.id));
    let enable = now.is_some_and(|(g, store, _)| {
        !movement_prevented(
            g.0,
            store,
            passenger_of,
            self_guid.0.unwrap_or(0),
            stop.at.is_some(),
        )
    });
    if now.is_some() {
        script.fire_event("VEHICLE_UPDATE", vec![]);
    }
    for (event, flag) in [("VEHICLE_ANGLE_SHOW", 0x400), ("VEHICLE_POWER_SHOW", 0x800)] {
        let shown = enable && rec.is_some_and(|r| r.flags & flag != 0);
        let args = if shown {
            vec![ScriptValue::Int(1)]
        } else {
            vec![]
        };
        script.fire_event(event, args);
    }
    if let Some(rec) = rec {
        let [norm, angle] = angle_update(rec, aim.pitch);
        script.fire_event(
            "VEHICLE_ANGLE_UPDATE",
            vec![
                ScriptValue::Number(f64::from(norm)),
                ScriptValue::Number(f64::from(angle)),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_angle_normalises_over_the_record_s_limits() {
        let mut rec = VehicleRec {
            flags: 0x40,
            pitch_min: 0.0,
            pitch_max: 1.0,
            ..Default::default()
        };
        assert_eq!(angle_update(&rec, 0.25), [0.25, 0.25]);
        rec.flags = 0;
        assert_eq!(angle_update(&rec, 0.0), [0.5, 0.0]);
        rec.flags = 0x40;
        rec.pitch_max = 0.0;
        assert_eq!(angle_update(&rec, 0.3), [0.0, 0.3], "no range");
    }

    #[test]
    fn a_charmed_vehicle_may_move_unless_its_charmer_s_seat_stops_it() {
        const ME: u64 = 0x10;
        const CART: u64 = 0xF150_0000_0000_0001;
        let charmed = ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9), (10, ME as u32), (11, 0)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        );
        let seated = |g: u64| {
            (g == ME).then_some(Passenger {
                vehicle: CART,
                seat: 0,
                state: SEATED,
            })
        };
        assert!(!movement_prevented(CART, &charmed, seated, ME, false));
        assert!(movement_prevented(CART, &charmed, seated, ME, true));
        let free = ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        );
        assert!(
            movement_prevented(CART, &free, |_| None, ME, false),
            "not ours to move"
        );
        assert!(
            !movement_prevented(ME, &free, |_| None, ME, false),
            "our own body"
        );
    }
}
