//! A vehicle's aim as the interface reads and asks it: `IsVehicleAimAngleAdjustable` and
//! `IsVehicleAimPowerAdjustable` (`0x5f9f70`, `0x5f9fe0`), `VehicleAimGetAngle` and
//! `VehicleAimGetNormAngle` (`0x5f9e10`, `0x5f9e60`), `VehicleAimRequestAngle` and
//! `VehicleAimRequestNormAngle` (`0x5fb820`, `0x5fb8c0`), `VehicleAimIncrement` and
//! `VehicleAimDecrement` (`0x5fb770`, `0x5fb7d0`), the held pitch keys `VehicleAimUpStart` and kin
//! (`0x5fc8e0`, `0x5fc570`, `0x5fc920`, `0x5fc5c0`), and the launch power, `VehicleAimGetNormPower`
//! and `VehicleAimSetNormPower` (`0x5f9550`, `0x5f9f10`).

use mlua::{Lua, Value};

use super::seats::VehicleRequest;

/// A unit's aim: its pitch and its vehicle record's aim facts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimUnit {
    pub guid: u64,
    pub pitch: f32,
    /// `Vehicle.dbc` flags, pitch limits; `None` without a vehicle record.
    pub rec: Option<AimRec>,
}

/// The record's aim columns.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimRec {
    pub flags: u32,
    pub pitch_min: f32,
    pub pitch_max: f32,
}

impl AimRec {
    /// The pitch limits: the record's under `flags & 0x40`, else ±π/2.
    pub fn limits(&self) -> (f32, f32) {
        if self.flags & 0x40 != 0 {
            (self.pitch_min, self.pitch_max)
        } else {
            (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
        }
    }
}

impl super::super::UiScript {
    /// `s_vehicleAimNormPower`, the launch power the trajectory reads.
    pub fn vehicle_aim_norm_power(&self) -> f32 {
        self.model_ref().vehicle_aim_power
    }
}

fn model_of(lua: &Lua) -> mlua::AppDataRef<'_, super::super::ModelCell> {
    lua.app_data_ref::<super::super::ModelCell>()
        .expect("model app_data")
}

/// The number at `v`, `lua_isnumber`'s way, or `None`.
fn number(lua: &Lua, v: Value) -> mlua::Result<Option<f64>> {
    lua.coerce_number(v)
}

/// `0x5f9e60`'s normalised pitch, 0 over a range under 1e-4.
fn norm_angle(aim: &AimUnit) -> f32 {
    let (min, max) = aim.rec.unwrap_or_default().limits();
    let range = max - min;
    if range.abs() <= 0.0001 {
        0.0
    } else {
        (aim.pitch - min) / range
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for (name, bit) in [
        ("IsVehicleAimAngleAdjustable", 0x400),
        ("IsVehicleAimPowerAdjustable", 0x800),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let model = model_of(lua);
                let rec = model.vehicles.aim.and_then(|a| a.rec);
                Ok(super::super::binding_abi::flag(
                    rec.is_some_and(|r| r.flags & bit != 0),
                ))
            })?,
        )?;
    }
    g.set(
        "VehicleAimGetAngle",
        lua.create_function(|lua, ()| {
            Ok(f64::from(
                model_of(lua).vehicles.aim.map_or(0.0, |a| a.pitch),
            ))
        })?,
    )?;
    g.set(
        "VehicleAimGetNormAngle",
        lua.create_function(|lua, ()| {
            Ok(f64::from(
                model_of(lua).vehicles.aim.map_or(0.0, |a| norm_angle(&a)),
            ))
        })?,
    )?;
    // `VehicleAimRequestAngle(angle)`: the mover's, when its record aims (`0x400`).
    g.set(
        "VehicleAimRequestAngle",
        lua.create_function(|lua, angle: Value| {
            let angle = number(lua, angle)?;
            let mut model = super::super::model_mut_of(lua);
            let mover = model.vehicles.mover_aim;
            if let (Some(m), Some(angle)) = (mover, angle) {
                if m.rec.is_some_and(|r| r.flags & 0x400 != 0) {
                    model.vehicle_requests.push(VehicleRequest::Pitch {
                        unit: m.guid,
                        pitch: angle as f32,
                    });
                }
            }
            Ok(())
        })?,
    )?;
    // `VehicleAimRequestNormAngle(norm)`: the aim unit's, over its limits.
    g.set(
        "VehicleAimRequestNormAngle",
        lua.create_function(|lua, norm: Value| {
            let norm = number(lua, norm)?;
            let mut model = super::super::model_mut_of(lua);
            let Some(aim) = model.vehicles.aim else {
                return Ok(());
            };
            let Some(rec) = aim.rec.filter(|r| r.flags & 0x400 != 0) else {
                return Ok(());
            };
            if let Some(norm) = norm {
                let (min, max) = rec.limits();
                model.vehicle_requests.push(VehicleRequest::Pitch {
                    unit: aim.guid,
                    pitch: (norm * f64::from(max - min) + f64::from(min)) as f32,
                });
            }
            Ok(())
        })?,
    )?;
    // `VehicleAimIncrement([delta])` and `VehicleAimDecrement([delta])`: 0.1 by default.
    for (name, sign) in [("VehicleAimIncrement", 1.0), ("VehicleAimDecrement", -1.0)] {
        g.set(
            name,
            lua.create_function(move |lua, delta: Value| {
                let delta = number(lua, delta)?.unwrap_or(0.1);
                super::super::model_mut_of(lua)
                    .vehicle_requests
                    .push(VehicleRequest::AimChange((sign * delta) as f32));
                Ok(())
            })?,
        )?;
    }
    for (name, up, held) in [
        ("VehicleAimUpStart", true, true),
        ("VehicleAimUpStop", true, false),
        ("VehicleAimDownStart", false, true),
        ("VehicleAimDownStop", false, false),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                super::super::model_mut_of(lua)
                    .vehicle_requests
                    .push(VehicleRequest::PitchKey { up, held });
                Ok(())
            })?,
        )?;
    }
    g.set(
        "VehicleAimGetNormPower",
        lua.create_function(|lua, ()| Ok(f64::from(model_of(lua).vehicle_aim_power)))?,
    )?;
    // `VehicleAimSetNormPower(power)`: with an aim unit, clamped to 0..1.
    g.set(
        "VehicleAimSetNormPower",
        lua.create_function(|lua, power: Value| {
            let power = number(lua, power)?;
            let mut model = super::super::model_mut_of(lua);
            if let (Some(_), Some(p)) = (model.vehicles.aim, power) {
                model.vehicle_aim_power = if p < 0.0 {
                    0.0
                } else if p >= 1.0 {
                    1.0
                } else {
                    p as f32
                };
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::VehicleView;
    use super::*;
    use crate::script::UiScript;

    const CART: u64 = 0xF150_0000_0000_0001;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        let aim = AimUnit {
            guid: CART,
            pitch: 0.25,
            rec: Some(AimRec {
                flags: 0x400 | 0x40,
                pitch_min: 0.0,
                pitch_max: 1.0,
            }),
        };
        s.set_vehicles(VehicleView {
            aim: Some(aim),
            mover_aim: Some(aim),
            ..Default::default()
        });
        s
    }

    #[test]
    fn the_aim_reads_its_unit() {
        let s = script();
        assert_eq!(
            s.eval::<Option<i64>>("return IsVehicleAimAngleAdjustable()")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.eval::<Option<i64>>("return IsVehicleAimPowerAdjustable()")
                .unwrap(),
            None
        );
        assert_eq!(s.eval::<f64>("return VehicleAimGetAngle()").unwrap(), 0.25);
        assert_eq!(
            s.eval::<f64>("return VehicleAimGetNormAngle()").unwrap(),
            0.25
        );
        assert_eq!(
            UiScript::new()
                .unwrap()
                .eval::<f64>("return VehicleAimGetAngle()")
                .unwrap(),
            0.0
        );
    }

    #[test]
    fn the_requests_queue_for_the_app() {
        let mut s = script();
        s.run(
            "VehicleAimRequestNormAngle(0.5); VehicleAimRequestAngle(0.75); VehicleAimDecrement(); \
             VehicleAimUpStart(); VehicleAimUpStop()",
        )
        .unwrap();
        assert_eq!(
            s.take_vehicle_requests(),
            [
                VehicleRequest::Pitch {
                    unit: CART,
                    pitch: 0.5
                },
                VehicleRequest::Pitch {
                    unit: CART,
                    pitch: 0.75
                },
                VehicleRequest::AimChange(-0.1),
                VehicleRequest::PitchKey {
                    up: true,
                    held: true
                },
                VehicleRequest::PitchKey {
                    up: true,
                    held: false
                },
            ]
        );
    }

    #[test]
    fn the_power_clamps_and_needs_an_aim_unit() {
        let s = script();
        s.run("VehicleAimSetNormPower(1.5)").unwrap();
        assert_eq!(s.vehicle_aim_norm_power(), 1.0);
        s.run("VehicleAimSetNormPower(-1)").unwrap();
        assert_eq!(s.vehicle_aim_norm_power(), 0.0);
        let bare = UiScript::new().unwrap();
        bare.run("VehicleAimSetNormPower(0.5)").unwrap();
        assert_eq!(bare.vehicle_aim_norm_power(), 0.0);
    }
}
