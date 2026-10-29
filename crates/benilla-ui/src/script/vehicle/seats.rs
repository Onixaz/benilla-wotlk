//! A vehicle's virtual seats, its own and those of the vehicles riding it, as the seat-list verbs
//! read them: `UnitVehicleSeatCount` (`0x613830`), `UnitVehicleSeatInfo` (`0x6138c0`),
//! `UnitSwitchToVehicleSeat` (`0x6139b0`), `CanSwitchVehicleSeat` (`0x608580`),
//! `CanEjectPassengerFromSeat` (`0x613d20`) and `EjectPassengerFromSeat` (`0x613e10`); and the seat
//! indicator's `GetVehicleUIIndicator` (`0x614e60`) and `GetVehicleUIIndicatorSeat` (`0x614ef0`).

use std::collections::HashMap;
use std::sync::Arc;

use mlua::{Lua, MultiValue, Value};

use super::super::binding_abi::{number_arg, optional_string};

/// One virtual seat (`0x757550`): the vehicle holding it, its slot there, and its occupant.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VirtualSeat {
    pub vehicle: u64,
    pub seat: u8,
    /// `VehicleSeat.dbc` column 1.
    pub flags: u32,
    /// Column 45.
    pub flags_b: u32,
    /// The passenger on the slot (`0x757680`), 0 for none.
    pub occupant: u64,
    /// The occupant's name and realm (`GetUnitName`), `None` for none.
    pub name: Option<String>,
    pub realm: Option<String>,
    /// The occupant's chain reaches the active mover (`FindRootVehicle(mover) == mover`).
    pub under_mover: bool,
}

/// A root vehicle's seats: the count `0x757470` reckons and the seats in the order `0x757550`
/// walks them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleTree {
    pub count: i32,
    pub seats: Vec<VirtualSeat>,
}

/// A request a verb makes, for the app to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleRequest {
    /// `CMSG_EJECT_PASSENGER` for the passenger (`0x757200`).
    Eject(u64),
    /// A seat change to `seat` of `vehicle` (`0x74ca90`): `CMSG_REQUEST_VEHICLE_SWITCH_SEAT`, or the
    /// controlled vehicle's change while the player's transport is the mover.
    SwitchSeat { vehicle: u64, seat: u8 },
}

/// A seat indicator (`VehicleUIIndicator.dbc`): its background and its seats in id order, each
/// the virtual seat it shows and its place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleIndicatorView {
    pub texture: String,
    pub seats: Vec<(u32, f32, f32)>,
}

impl super::super::UiScript {
    /// The seat indicators by id, from client data.
    pub fn set_vehicle_indicators(&mut self, indicators: Arc<HashMap<u32, VehicleIndicatorView>>) {
        self.model_mut().vehicle_indicators = indicators;
    }

    /// Drain the verbs' requests.
    pub fn take_vehicle_requests(&mut self) -> Vec<VehicleRequest> {
        std::mem::take(&mut self.model_mut().vehicle_requests)
    }
}

/// The root vehicle of the unit `token` names (`0x74c650` with no stop) and its seats.
fn tree_of<'m>(
    model: &'m super::super::Model,
    token: &Value,
    lua: &Lua,
) -> Option<(u64, &'m VehicleTree)> {
    let guid = model.unit(&optional_string(lua, token)?)?.guid;
    let root = *model.vehicles.roots.get(&guid)?;
    Some((root, model.vehicles.trees.get(&root)?))
}

/// The root of the active mover's chain and its seat `index` (1-based), as the eject verbs find
/// it.
fn mover_seat(model: &super::super::Model, index: i32) -> Option<&VirtualSeat> {
    let mover = model.vehicles.mover;
    let root = model.vehicles.roots.get(&(mover != 0).then_some(mover)?)?;
    let at = usize::try_from(index.checked_sub(1)?).ok()?;
    model.vehicles.trees.get(root)?.seats.get(at)
}

/// `0x5fb560`: the player's seat flags hold any of `flags`.
pub(super) fn seat_flag_set(model: &super::super::Model, flags: u32) -> bool {
    model
        .vehicles
        .input_seat_flags
        .is_some_and(|f| f & flags != 0)
}

fn model_of(lua: &Lua) -> mlua::AppDataRef<'_, super::super::ModelCell> {
    lua.app_data_ref::<super::super::ModelCell>()
        .expect("model app_data")
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // `UnitVehicleSeatCount`: the root's count, 0 for none.
    g.set(
        "UnitVehicleSeatCount",
        lua.create_function(move |lua, token: Value| {
            Ok(tree_of(&model_of(lua), &token, lua).map_or(0, |(_, t)| t.count))
        })?,
    )?;
    // `UnitVehicleSeatInfo`: nothing past the seats, else the type (`0xad6734`), the occupant's
    // name and realm (nil for none), and the eject (`flagsB & 0x20`) and switch
    // (`flags & 0x4000000`) bits.
    g.set(
        "UnitVehicleSeatInfo",
        lua.create_function(move |lua, (token, index): (Value, Value)| {
            let index = lua.coerce_number(index)?.unwrap_or(0.0) as i32 - 1;
            let model = model_of(lua);
            let Some((root, tree)) = tree_of(&model, &token, lua) else {
                return Ok(MultiValue::new());
            };
            let Some(s) = usize::try_from(index).ok().and_then(|i| tree.seats.get(i)) else {
                return Ok(MultiValue::new());
            };
            let kind = if s.flags & 0x800 == 0 {
                "None"
            } else if s.vehicle == root {
                "Root"
            } else {
                "Child"
            };
            let text = |t: &Option<String>| -> mlua::Result<Value> {
                Ok(match t {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_iter([
                Value::String(lua.create_string(kind)?),
                text(&s.name)?,
                text(&s.realm)?,
                Value::Boolean(s.flags_b & 0x20 != 0),
                Value::Boolean(s.flags & 0x400_0000 != 0),
            ]))
        })?,
    )?;
    // `UnitSwitchToVehicleSeat`: the player only, on the root of its own vehicle; a free seat
    // allowing switches (`0x4000000`) while the player's own seat allows them too.
    g.set(
        "UnitSwitchToVehicleSeat",
        lua.create_function(move |lua, (token, index): (Value, Value)| {
            let index = lua.coerce_number(index)?.unwrap_or(0.0);
            let mut model = super::super::model_mut_of(lua);
            let Some(token) = optional_string(lua, &token) else {
                return Ok(());
            };
            let guid = model.unit(&token).map(|u| u.guid);
            let player = model.unit_guids.player;
            let Some(vehicle) = model
                .vehicles
                .player_vehicle
                .filter(|_| guid == Some(player) && player != 0)
            else {
                return Ok(());
            };
            let Some(root) = model.vehicles.roots.get(&vehicle).copied() else {
                return Ok(());
            };
            let at = (index - 1.0) as i32;
            let target = usize::try_from(at)
                .ok()
                .and_then(|i| model.vehicles.trees.get(&root)?.seats.get(i))
                .filter(|s| s.occupant == 0 && s.flags & 0x400_0000 != 0)
                .map(|s| (s.vehicle, s.seat));
            if let Some((vehicle, seat)) = target.filter(|_| seat_flag_set(&model, 0x400_0000)) {
                model
                    .vehicle_requests
                    .push(VehicleRequest::SwitchSeat { vehicle, seat });
            }
            Ok(())
        })?,
    )?;
    // `CanSwitchVehicleSeat`: the player's seat allows switches.
    g.set(
        "CanSwitchVehicleSeat",
        lua.create_function(move |lua, ()| Ok(seat_flag_set(&model_of(lua), 0x400_0000)))?,
    )?;
    // `CanEjectPassengerFromSeat`: a seat of the mover's root, occupied by a passenger under the
    // mover, on a seat that allows it (`flagsB & 0x20`).
    g.set(
        "CanEjectPassengerFromSeat",
        lua.create_function(move |lua, index: Value| {
            let index = number_arg(lua, index, "Usage: CanEjectPassengerFromSeat(seatIndex)")?;
            Ok(mover_seat(&model_of(lua), index)
                .is_some_and(|s| s.occupant != 0 && s.under_mover && s.flags_b & 0x20 != 0))
        })?,
    )?;
    // `EjectPassengerFromSeat`: the passenger on that seat, whatever its seat allows.
    g.set(
        "EjectPassengerFromSeat",
        lua.create_function(move |lua, index: Value| {
            let index = number_arg(lua, index, "Usage: EjectPassengerFromSeat(seatIndex)")?;
            let mut model = super::super::model_mut_of(lua);
            if let Some(occupant) = mover_seat(&model, index)
                .map(|s| s.occupant)
                .filter(|&o| o != 0)
            {
                model.vehicle_requests.push(VehicleRequest::Eject(occupant));
            }
            Ok(())
        })?,
    )?;
    // `GetVehicleUIIndicator(id)`: the background and the seat count, nothing for an unknown id.
    g.set(
        "GetVehicleUIIndicator",
        lua.create_function(move |lua, id: Value| {
            let id = number_arg(lua, id, "Usage: GetVehicleUIIndicator(indicatorID)")?;
            let model = model_of(lua);
            let Some(ind) = u32::try_from(id)
                .ok()
                .and_then(|id| model.vehicle_indicators.get(&id))
            else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_iter([
                Value::String(lua.create_string(&ind.texture)?),
                Value::Integer(ind.seats.len() as i64),
            ]))
        })?,
    )?;
    // `GetVehicleUIIndicatorSeat(id, index)`: the seat's virtual seat and place, nothing past them.
    g.set(
        "GetVehicleUIIndicatorSeat",
        lua.create_function(move |lua, (id, index): (Value, Value)| {
            const USAGE: &str = "Usage: GetVehicleUIIndicatorSeat(indicatorID, indicatorSeatIndex)";
            let id = number_arg(lua, id, USAGE)?;
            let index = number_arg(lua, index, USAGE)?;
            let model = model_of(lua);
            let seat = u32::try_from(id)
                .ok()
                .and_then(|id| model.vehicle_indicators.get(&id))
                .and_then(|ind| ind.seats.get(usize::try_from(index.wrapping_sub(1)).ok()?));
            Ok(match seat {
                Some(&(virtual_seat, x, y)) => MultiValue::from_iter([
                    Value::Integer(i64::from(virtual_seat)),
                    Value::Number(f64::from(x)),
                    Value::Number(f64::from(y)),
                ]),
                None => MultiValue::new(),
            })
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::VehicleView;
    use super::*;
    use crate::script::{UiScript, UnitGuids, UnitState};

    const ME: u64 = 0x10;
    const P2: u64 = 0x22;
    const TANK: u64 = 0xF150_0000_0000_0001;
    const TURRET: u64 = 0xF150_0000_0000_0002;

    /// A tank driven by us: seat 0 ours (a control seat), a turret riding seat 1 with a free
    /// gunner seat, and seat 2 held by another player.
    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            vehicle: TANK,
            ..Default::default()
        });
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                has_object: true,
                guid: ME,
                ..Default::default()
            }),
        );
        s.set_unit_by_guid(
            TANK,
            Some(UnitState {
                exists: true,
                has_object: true,
                guid: TANK,
                ..Default::default()
            }),
        );
        let seat = |vehicle, seat, flags, flags_b, occupant: u64, name: Option<&str>| VirtualSeat {
            vehicle,
            seat,
            flags,
            flags_b,
            occupant,
            name: name.map(str::to_string),
            realm: None,
            under_mover: occupant != 0 && occupant != ME,
        };
        s.set_vehicles(VehicleView {
            roots: HashMap::from([(ME, TANK), (TANK, TANK), (TURRET, TANK), (P2, TANK)]),
            trees: HashMap::from([(
                TANK,
                VehicleTree {
                    count: 3,
                    seats: vec![
                        seat(TANK, 0, 0x0400_0800, 0, ME, Some("Me")),
                        seat(TURRET, 0, 0x0400_0800, 0, 0, None),
                        seat(TANK, 2, 0, 0x20, P2, Some("Bob")),
                    ],
                },
            )]),
            mover: TANK,
            player_vehicle: Some(TANK),
            input_seat_flags: Some(0x0400_0800),
            ..Default::default()
        });
        s.set_vehicle_indicators(Arc::new(HashMap::from([(
            5,
            VehicleIndicatorView {
                texture: "Interface\\Vehicles\\Tank".into(),
                seats: vec![(1, 0.5, 0.25), (3, 0.1, 0.9)],
            },
        )])));
        s
    }

    #[test]
    fn the_seat_list_walks_the_root() {
        let s = script();
        assert_eq!(
            s.eval::<i64>("return UnitVehicleSeatCount('player')")
                .unwrap(),
            3
        );
        assert_eq!(
            s.eval::<i64>("return UnitVehicleSeatCount('target')")
                .unwrap(),
            0
        );
        let info = |i: i64| {
            s.eval::<(String, Option<String>, Option<String>, bool, bool)>(&format!(
                "return UnitVehicleSeatInfo('vehicle', {i})"
            ))
            .unwrap()
        };
        assert_eq!(
            info(1),
            ("Root".into(), Some("Me".into()), None, false, true)
        );
        assert_eq!(info(2), ("Child".into(), None, None, false, true));
        assert_eq!(
            info(3),
            ("None".into(), Some("Bob".into()), None, true, false)
        );
        assert_eq!(
            s.eval::<i64>("return select('#', UnitVehicleSeatInfo('player', 4))")
                .unwrap(),
            0
        );
    }

    /// A switch to a free seat allowing it is requested; an occupied one is not.
    #[test]
    fn switching_asks_for_a_free_seat() {
        let mut s = script();
        assert!(s.eval::<bool>("return CanSwitchVehicleSeat()").unwrap());
        s.run("UnitSwitchToVehicleSeat('player', 3); UnitSwitchToVehicleSeat('player', 2)")
            .unwrap();
        assert_eq!(
            s.take_vehicle_requests(),
            [VehicleRequest::SwitchSeat {
                vehicle: TURRET,
                seat: 0
            }]
        );
    }

    /// Eject reads the mover's root: the seat's own bit gates the ask, not the send.
    #[test]
    fn eject_reads_the_mover_s_root() {
        let mut s = script();
        assert!(s
            .eval::<bool>("return CanEjectPassengerFromSeat(3)")
            .unwrap());
        assert!(!s
            .eval::<bool>("return CanEjectPassengerFromSeat(1)")
            .unwrap());
        assert!(s.run("CanEjectPassengerFromSeat()").is_err());
        s.run("EjectPassengerFromSeat(3); EjectPassengerFromSeat(2)")
            .unwrap();
        assert_eq!(s.take_vehicle_requests(), [VehicleRequest::Eject(P2)]);
    }

    #[test]
    fn the_indicator_lists_its_seats() {
        let s = script();
        assert_eq!(
            s.eval::<(String, i64)>("return GetVehicleUIIndicator(5)")
                .unwrap(),
            ("Interface\\Vehicles\\Tank".into(), 2)
        );
        assert_eq!(
            s.eval::<(i64, f64, f64)>("return GetVehicleUIIndicatorSeat(5, 2)")
                .unwrap(),
            (3, f64::from(0.1f32), f64::from(0.9f32))
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetVehicleUIIndicator(6))")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetVehicleUIIndicatorSeat(5, 3))")
                .unwrap(),
            0
        );
    }
}
