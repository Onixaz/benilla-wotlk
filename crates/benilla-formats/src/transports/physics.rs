//! `TransportPhysics.dbc`, a `MO_TRANSPORT`'s rocking: a wave on its height, a roll and a pitch
//! on sine clocks, and a bank into its turns, all damped at low speed (`0x7f7b30`). The template's
//! property 99 (`gameobject_template.data5`) names the row (`0x711b50`); without one the transport
//! sails level.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TRANSPORT_PHYSICS: &str = "DBFilesClient\\TransportPhysics.dbc";

/// One `TransportPhysics.dbc` row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportPhysics {
    pub wave_amp: f32,
    pub wave_time_scale: f32,
    pub roll_amp: f32,
    pub roll_time_scale: f32,
    pub pitch_amp: f32,
    pub pitch_time_scale: f32,
    pub max_bank: f32,
    pub max_bank_turn_speed: f32,
    pub speed_damp_thresh: f32,
    pub speed_damp: f32,
}

/// The reference's "near zero" for a time scale, `2^-22`.
const NO_SCALE: f32 = 0.000_000_238_418_58;

/// The rocking at one instant: the height offset and the roll and pitch, in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rocking {
    pub wave: f32,
    pub roll: f32,
    pub pitch: f32,
}

impl TransportPhysics {
    /// The rocking at `time` (the path clock, ms) for a speed and a turn estimate (`0x7f7b30`):
    /// each motion a sine of `time` on its own period, `6283.1855 / scale` ms truncated; under
    /// `speed_damp_thresh` the three are damped toward `speed_damp` by `√(speed / thresh)`.
    pub fn rocking(&self, time: u32, speed: f32, turn: f32) -> Rocking {
        let damp = if speed >= self.speed_damp_thresh {
            1.0
        } else {
            let scale = (speed * (1.0 / self.speed_damp_thresh)).sqrt();
            scale * ((1.0 - self.speed_damp) * scale) + self.speed_damp
        };
        let wave =
            phase(time, self.wave_time_scale).map_or(0.0, |p| p.sin() * self.wave_amp * damp);
        let limit = self.max_bank_turn_speed;
        let bank_turn = if limit >= turn {
            if -limit <= turn {
                turn
            } else {
                -limit
            }
        } else {
            limit
        };
        // A zero limit divides by zero in the reference; it banks not at all here.
        let bank = if limit == 0.0 {
            0.0
        } else {
            bank_turn * (self.max_bank / limit)
        };
        let roll = phase(time, self.roll_time_scale)
            .map_or(0.0, |p| (bank + (p + 0.5).sin() * self.roll_amp) * damp);
        let pitch = phase(time, self.pitch_time_scale)
            .map_or(0.0, |p| damp * ((p + 0.2).sin() * self.pitch_amp));
        Rocking { wave, roll, pitch }
    }
}

/// `2π · (time % period) / period` for a period of `6283.1855 / scale` ms; `None` for a scale
/// under the reference's epsilon, or one so large the period truncates to 0 ms.
fn phase(time: u32, scale: f32) -> Option<f32> {
    if scale.abs() < NO_SCALE {
        return None;
    }
    let period = (6283.1855 / scale) as u32;
    if period == 0 {
        return None;
    }
    Some((time % period) as f32 * 6.283_185_5 / period as f32)
}

pub(crate) fn transport_physics_schema() -> Schema {
    let mut s = Schema::new("TransportPhysics");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for name in [
        "WaveAmp",
        "WaveTimeScale",
        "RollAmp",
        "RollTimeScale",
        "PitchAmp",
        "PitchTimeScale",
        "MaxBank",
        "MaxBankTurnSpeed",
        "SpeedDampThresh",
        "SpeedDamp",
    ] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s
}

/// `TransportPhysics.dbc` by `ID`.
pub struct TransportPhysicsTable {
    by_id: HashMap<u32, TransportPhysics>,
}

impl TransportPhysicsTable {
    /// The row a template's property 99 names; 0 or a missing id is none (`0x711b50`).
    pub fn get(&self, id: u32) -> Option<&TransportPhysics> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Read `TransportPhysics.dbc` off the patch chain.
pub fn load_transport_physics(chain: &mut Chain) -> Result<TransportPhysicsTable> {
    let bytes = chain
        .read_file(TRANSPORT_PHYSICS)
        .context("reading TransportPhysics.dbc")?;
    let rs = parse(&bytes, transport_physics_schema(), "TransportPhysics")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let f = |i: usize| f32_at(r, i).unwrap_or(0.0);
        by_id.insert(
            id,
            TransportPhysics {
                wave_amp: f(1),
                wave_time_scale: f(2),
                roll_amp: f(3),
                roll_time_scale: f(4),
                pitch_amp: f(5),
                pitch_time_scale: f(6),
                max_bank: f(7),
                max_bank_turn_speed: f(8),
                speed_damp_thresh: f(9),
                speed_damp: f(10),
            },
        );
    }
    Ok(TransportPhysicsTable { by_id })
}

/// The orientation `(x, y, z, w)` of `Rz(facing) · Ry(pitch) · Rx(roll)`, the order the reference
/// turns the ship's matrix in (`0x7134a0`, each turn pre-multiplied on its row-vector matrix).
pub(crate) fn yaw_pitch_roll(facing: f32, pitch: f32, roll: f32) -> [f32; 4] {
    let (sz, cz) = (facing * 0.5).sin_cos();
    let (sy, cy) = (pitch * 0.5).sin_cos();
    let (sx, cx) = (roll * 0.5).sin_cos();
    [
        cz * cy * sx - sz * sy * cx,
        cz * sy * cx + sz * cy * sx,
        sz * cy * cx - cz * sy * sx,
        cz * cy * cx + sz * sy * sx,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 1, the ships' (`0.5, 0.5, 0.1, 0.25, 0.05, 0.1, 0.05, 0.7, 25, 0.1`).
    fn ship() -> TransportPhysics {
        TransportPhysics {
            wave_amp: 0.5,
            wave_time_scale: 0.5,
            roll_amp: 0.1,
            roll_time_scale: 0.25,
            pitch_amp: 0.05,
            pitch_time_scale: 0.1,
            max_bank: 0.05,
            max_bank_turn_speed: 0.7,
            speed_damp_thresh: 25.0,
            speed_damp: 0.1,
        }
    }

    /// At full speed and no turn each motion is its own sine: the wave's period is
    /// `6283.1855 / 0.5 = 12566` ms, so a quarter of it is the wave's crest.
    #[test]
    fn at_speed_each_motion_runs_on_its_own_clock() {
        let p = ship();
        let r = p.rocking(0, 30.0, 0.0);
        assert_eq!(r.wave, 0.0);
        assert!((r.roll - 0.5f32.sin() * 0.1).abs() < 1e-6, "{r:?}");
        assert!((r.pitch - 0.2f32.sin() * 0.05).abs() < 1e-6, "{r:?}");
        let crest = p.rocking(12566 / 4, 30.0, 0.0);
        assert!((crest.wave - 0.5).abs() < 1e-3, "{crest:?}");
        // One wave period on, the same wave; the roll's own period is 25132 ms.
        assert_eq!(
            p.rocking(12566 + 100, 30.0, 0.0).wave,
            p.rocking(100, 30.0, 0.0).wave
        );
        assert_eq!(
            p.rocking(25132 + 100, 30.0, 0.0).roll,
            p.rocking(100, 30.0, 0.0).roll
        );
    }

    /// Docked, the rocking damps to `speed_damp` of itself.
    #[test]
    fn a_docked_ship_rocks_at_the_damped_amplitude() {
        let p = ship();
        let (moving, docked) = (p.rocking(3000, 30.0, 0.0), p.rocking(3000, 0.0, 0.0));
        assert!((docked.wave - moving.wave * 0.1).abs() < 1e-6);
        assert!((docked.roll - moving.roll * 0.1).abs() < 1e-6);
        assert!((docked.pitch - moving.pitch * 0.1).abs() < 1e-6);
    }

    /// A turn banks the roll, clamped at `max_bank_turn_speed`.
    #[test]
    fn a_turn_banks_up_to_its_limit() {
        let p = ship();
        let level = p.rocking(0, 30.0, 0.0).roll;
        let banked = p.rocking(0, 30.0, 0.35).roll;
        assert!((banked - level - 0.025).abs() < 1e-6, "{banked} vs {level}");
        let hard = p.rocking(0, 30.0, 5.0).roll;
        assert!((hard - level - 0.05).abs() < 1e-6, "{hard} vs {level}");
        let other = p.rocking(0, 30.0, -5.0).roll;
        assert!((other - level + 0.05).abs() < 1e-6);
    }

    /// A zero time scale is no motion, not a division by zero.
    #[test]
    fn a_zero_time_scale_holds_still() {
        let mut p = ship();
        p.wave_time_scale = 0.0;
        p.roll_time_scale = 0.0;
        p.pitch_time_scale = 0.0;
        assert_eq!(
            p.rocking(1234, 30.0, 0.0),
            Rocking {
                wave: 0.0,
                roll: 0.0,
                pitch: 0.0
            }
        );
    }

    /// The composed orientation turns the bow (+x) by the facing and then lifts it by the pitch:
    /// a positive pitch about +y dips +x toward −z.
    #[test]
    fn yaw_pitch_roll_composes_in_the_reference_order() {
        let q = yaw_pitch_roll(std::f32::consts::FRAC_PI_2, 0.3, 0.0);
        let [x, y, z, w] = q;
        // R(q)·(1, 0, 0), the bow.
        let bow = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        assert!(bow[0].abs() < 1e-5, "{bow:?}");
        assert!((bow[1] - 0.3f32.cos()).abs() < 1e-5, "{bow:?}");
        assert!((bow[2] + 0.3f32.sin()).abs() < 1e-5, "{bow:?}");
    }

    /// The 12340 table: the ships' row and the Skybreaker's.
    #[test]
    fn real_transport_physics_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let table = load_transport_physics(&mut chain).expect("load TransportPhysics.dbc");
        assert_eq!(table.get(1), Some(&ship()));
        let sky = table.get(61).expect("row 61");
        assert_eq!(sky.wave_amp, 0.35);
        assert!(table.get(0).is_none());
    }
}
