//! Vehicles as the interface reads them: each unit's passenger state and the seat it rides, which
//! the app keeps, and the `Unit*` verbs over them: `UnitInVehicle` (`0x6133d0`),
//! `UnitUsingVehicle` (`0x6134a0`), `UnitControllingVehicle` (`0x613570`),
//! `UnitInVehicleControlSeat` (`0x613700`), `UnitHasVehicleUI` (`0x613740`),
//! `UnitTargetsVehicleInRaidUI` (`0x613780`) and `UnitVehicleSkin` (`0x6137d0`).

use std::collections::HashMap;

use mlua::{Lua, Value};

use super::binding_abi::{flag, optional_string, string_arg};

mod seats;

pub use seats::{VehicleIndicatorView, VehicleRequest, VehicleTree, VirtualSeat};

/// The `VehicleSeat.dbc` facts the verbs read off a seat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VehicleSeatView {
    /// Column 1.
    pub flags: u32,
    /// Column 45.
    pub flags_b: u32,
    /// Column 44, `UnitVehicleSkin`'s index.
    pub skin: i32,
}

/// One unit's vehicle facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VehicleUnitView {
    /// A held unit's passenger state, 0 none to 3 seated; 0 for an unheld party member.
    pub state: u8,
    /// The seat it rides (`0x613600`): a held unit's seat on its vehicle transport, else the
    /// party record's seat id.
    pub seat: Option<VehicleSeatView>,
    /// Seated, on a transport that is its own `UNIT_FIELD_CHARM` (`0x6135b8`-`0x6135db`).
    pub controlling: bool,
}

/// The vehicle facts the verbs read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleView {
    /// Every unit with vehicle facts, by guid; a guid missing here rides nothing.
    pub units: HashMap<u64, VehicleUnitView>,
    /// Each held unit's root vehicle (`0x74c650` with no stop): the top of its vehicle chain, or
    /// itself when it rides none and has a vehicle.
    pub roots: HashMap<u64, u64>,
    /// Each root with a vehicle record, and its seats.
    pub trees: HashMap<u64, VehicleTree>,
    /// The active mover (`s_activeMover`), 0 for none.
    pub mover: u64,
    /// The vehicle the player is a passenger of, its transport (`GetVehicleUnit`).
    pub player_vehicle: Option<u64>,
    /// The player's seat flags while `0x5fb560` reads them: the player seated, and its transport
    /// or itself the mover.
    pub input_seat_flags: Option<u32>,
}

impl super::UiScript {
    /// Replace the vehicle facts; the app fires the vehicle events itself.
    pub fn set_vehicles(&mut self, view: VehicleView) {
        set_field!(self, vehicles, view);
    }
}

/// The unit `token` names and its facts: whether the object manager holds it, its party status
/// bit `0x200`, and its entry.
fn lookup(lua: &Lua, token: Option<&str>) -> Option<(bool, bool, VehicleUnitView)> {
    let model = lua
        .app_data_ref::<super::ModelCell>()
        .expect("model app_data");
    let u = model.unit(token?)?;
    let facts = model
        .vehicles
        .units
        .get(&u.guid)
        .copied()
        .unwrap_or_default();
    Some((u.has_object, u.in_vehicle, facts))
}

/// The seat `0x613600` finds for the first argument, through `lua_tolstring` and the
/// non-raising resolver, so any argument answers.
fn seat_of(lua: &Lua, token: &Value) -> Option<VehicleSeatView> {
    lookup(lua, optional_string(lua, token).as_deref()).and_then(|(_, _, f)| f.seat)
}

/// A seat-flag verb: a Lua boolean, false with no seat.
fn seat_flag(lua: &Lua, name: &str, test: fn(&VehicleSeatView) -> bool) -> mlua::Result<()> {
    lua.globals().set(
        name,
        lua.create_function(move |lua, token: Value| {
            Ok(seat_of(lua, &token).is_some_and(|s| test(&s)))
        })?,
    )
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // `UnitInVehicle` and `UnitUsingVehicle`: a string argument or the usage error, then 1 or nil,
    // a held unit's passenger state (seated, and any but none), else the party record's status
    // bit `0x200` (`0x61347b`).
    for (name, usage, held) in [
        (
            "UnitInVehicle",
            r#"Usage: UnitInVehicle("unit")"#,
            (|state| state == 3) as fn(u8) -> bool,
        ),
        (
            "UnitUsingVehicle",
            r#"Usage: UnitUsingVehicle("unit")"#,
            |state| state != 0,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, token: Value| {
                let token = string_arg(lua, token, usage)?;
                Ok(flag(lookup(lua, Some(&token)).is_some_and(
                    |(has_object, party_bit, f)| {
                        if has_object {
                            held(f.state)
                        } else {
                            party_bit
                        }
                    },
                )))
            })?,
        )?;
    }
    // `UnitControllingVehicle`: a Lua boolean, a held unit seated on its own charm; the
    // non-raising token path, and no party leg.
    g.set(
        "UnitControllingVehicle",
        lua.create_function(|lua, token: Value| {
            let token = optional_string(lua, &token);
            Ok(lookup(lua, token.as_deref())
                .is_some_and(|(has_object, _, f)| has_object && f.controlling))
        })?,
    )?;
    seats::install(lua)?;
    seat_flag(lua, "UnitInVehicleControlSeat", |s| s.flags & 0x800 != 0)?;
    seat_flag(lua, "UnitHasVehicleUI", |s| s.flags & 0x2000_0000 != 0)?;
    seat_flag(lua, "UnitTargetsVehicleInRaidUI", |s| s.flags_b & 0x8 != 0)?;
    // `UnitVehicleSkin`: nothing with no seat, else the skin's word (`0xad8660`), `""` past it.
    g.set(
        "UnitVehicleSkin",
        lua.create_function(|lua, token: Value| {
            let Some(seat) = seat_of(lua, &token) else {
                return Ok(mlua::MultiValue::new());
            };
            let word = match seat.skin {
                0 => "Natural",
                1 => "Mechanical",
                _ => "",
            };
            Ok(mlua::MultiValue::from_iter([Value::String(
                lua.create_string(word)?,
            )]))
        })?,
    )?;
    Ok(())
}

/// The macro conditions `vehicleui` (`0x5ef430`) and `unithasvehicleui` (`0x5ef470`): the seat
/// flag `0x20000000` of the unit's seat, as `UnitHasVehicleUI`.
pub(super) fn has_vehicle_ui(model: &super::Model, token: &str) -> bool {
    model
        .unit(token)
        .and_then(|u| model.vehicles.units.get(&u.guid))
        .and_then(|f| f.seat)
        .is_some_and(|s| s.flags & 0x2000_0000 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{UiScript, UnitState};

    const ME: u64 = 0x10;
    const P1: u64 = 0x21;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                has_object: true,
                guid: ME,
                ..Default::default()
            }),
        );
        // An unheld party member on a seat, status bit `0x200` set.
        s.set_unit(
            "party1",
            Some(UnitState {
                exists: true,
                guid: P1,
                in_vehicle: true,
                ..Default::default()
            }),
        );
        s.set_vehicles(VehicleView {
            units: HashMap::from([
                (
                    ME,
                    VehicleUnitView {
                        state: 3,
                        seat: Some(VehicleSeatView {
                            flags: 0x2000_0800,
                            flags_b: 0x8,
                            skin: 1,
                        }),
                        controlling: true,
                    },
                ),
                (
                    P1,
                    VehicleUnitView {
                        state: 0,
                        seat: Some(VehicleSeatView {
                            skin: 7,
                            ..Default::default()
                        }),
                        controlling: false,
                    },
                ),
            ]),
            ..Default::default()
        });
        s
    }

    fn eval(s: &UiScript, code: &str) -> String {
        s.eval::<String>(&format!("return tostring({code})"))
            .unwrap()
    }

    #[test]
    fn a_seated_player_answers_from_its_seat() {
        let s = script();
        for (code, want) in [
            ("UnitInVehicle('player')", "1"),
            ("UnitUsingVehicle('player')", "1"),
            ("UnitControllingVehicle('player')", "true"),
            ("UnitInVehicleControlSeat('player')", "true"),
            ("UnitHasVehicleUI('player')", "true"),
            ("UnitTargetsVehicleInRaidUI('player')", "true"),
            ("UnitVehicleSkin('player')", "Mechanical"),
        ] {
            assert_eq!(eval(&s, code), want, "{code}");
        }
    }

    /// An unheld member reads the party record: the status bit and the seat id's seat, and is
    /// never controlling.
    #[test]
    fn an_unheld_member_answers_from_the_party_record() {
        let s = script();
        assert_eq!(eval(&s, "UnitInVehicle('party1')"), "1");
        assert_eq!(eval(&s, "UnitControllingVehicle('party1')"), "false");
        assert_eq!(eval(&s, "UnitHasVehicleUI('party1')"), "false");
        assert_eq!(eval(&s, "UnitVehicleSkin('party1')"), "");
    }

    /// `[vehicleui]` reads the player's seat, `[unithasvehicleui]` the group's unit's.
    #[test]
    fn the_macro_conditions_read_the_seat_flag() {
        let s = script();
        for (code, want) in [
            ("SecureCmdOptionParse('[vehicleui] a; b')", "a"),
            ("SecureCmdOptionParse('[@party1,vehicleui] a; b')", "a"),
            (
                "SecureCmdOptionParse('[@party1,unithasvehicleui] a; b')",
                "b",
            ),
            (
                "SecureCmdOptionParse('[@player,unithasvehicleui] a; b')",
                "a",
            ),
        ] {
            assert_eq!(eval(&s, code), want, "{code}");
        }
    }

    #[test]
    fn nobody_rides_nothing_and_the_usage_raises() {
        let s = script();
        assert_eq!(eval(&s, "UnitInVehicle('target')"), "nil");
        assert_eq!(eval(&s, "UnitHasVehicleUI(nil)"), "false");
        assert_eq!(
            s.eval::<i64>("return select('#', UnitVehicleSkin('target'))")
                .unwrap(),
            0
        );
        assert!(s
            .run("UnitInVehicle()")
            .unwrap_err()
            .to_string()
            .contains(r#"Usage: UnitInVehicle("unit")"#));
    }
}
