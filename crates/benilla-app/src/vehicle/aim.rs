//! The mover's aim pitch: `SetPlayerPitch` (`0x5fb3a0`) clamps a requested pitch to the
//! vehicle record's limits and either sets it at once (`MSG_MOVE_SET_PITCH`, `0x6ee460`) or, for
//! a mover whose extra flags pitch at full speed (`0x10`), turns to it (`0x6ef490`); the held pitch
//! keys turn it while down. A turn runs at the mover's pitch rate between `MSG_MOVE_START_PITCH_UP`
//! or `_DOWN` and `MSG_MOVE_STOP_PITCH`, and an aiming record (`0x400`) tells the interface each
//! new angle (`VEHICLE_ANGLE_UPDATE`, `0x5fa910`).

use bevy::prelude::*;

use benilla_formats::VehicleRec;
use benilla_protocol::messages::opcode;
use benilla_ui::script::{ScriptValue, UiScript};

use crate::net::{ClientCommand, VehicleCommand};

/// `MOVEFLAG_PITCH_UP` and `_DOWN`, the same bits in both words.
const PITCH_UP: u32 = 0x40;
const PITCH_DOWN: u32 = 0x80;
/// `MOVEMENTFLAG2_FULL_SPEED_PITCHING`: a pitch request turns rather than snaps.
const FLAG2_FULL_SPEED_PITCHING: u16 = 0x10;

/// A unit's 3.3.5a extra movement flags, from its create block.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MoveFlags2(pub(crate) u16);

/// The mover's aim, which the aim verbs move.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct VehicleAim {
    /// The mover's pitch.
    pub(crate) pitch: f32,
    /// The last requested aim (`m_vehicleAim`), `None` until one is (`m_vehicleAimValid`).
    pub(crate) held: Option<f32>,
    /// A full-speed mover's pitch-to-angle target.
    pub(crate) target: Option<f32>,
    /// The pitch keys down, up and down.
    pub(crate) keys: [bool; 2],
    /// The turn sent: 1 up, -1 down, 0 none.
    pub(crate) turning: i8,
}

/// The pitch limits a record sets: its own under `flags & 0x40`, else ±π/2.
pub(super) fn limits(rec: &VehicleRec) -> (f32, f32) {
    if rec.flags & 0x40 != 0 {
        (rec.pitch_min, rec.pitch_max)
    } else {
        (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
    }
}

/// The mover's pose for its reports.
pub(super) struct MoverPose {
    pub(super) guid: u64,
    pub(super) flags: u32,
    pub(super) flags2: u16,
    pub(super) pos: [f32; 3],
    pub(super) orientation: f32,
}

impl MoverPose {
    fn send(&self, net: &crate::net::NetCommands, opcode: u16, flags: u32, pitch: f32) {
        let _ = net.0.send(ClientCommand::Vehicle(VehicleCommand::Pitch {
            opcode,
            mover: self.guid,
            flags,
            flags2: self.flags2,
            pos: self.pos,
            orientation: self.orientation,
            pitch,
        }));
    }
}

/// `VEHICLE_ANGLE_UPDATE` for an aiming record: the normalised pitch and the pitch.
fn angle_event(script: Option<&mut UiScript>, rec: Option<&VehicleRec>, pitch: f32) {
    let (Some(script), Some(rec)) = (script, rec.filter(|r| r.flags & 0x400 != 0)) else {
        return;
    };
    let [norm, angle] = super::control::angle_update(rec, pitch);
    script.fire_event(
        "VEHICLE_ANGLE_UPDATE",
        vec![
            ScriptValue::Number(f64::from(norm)),
            ScriptValue::Number(f64::from(angle)),
        ],
    );
}

/// `0x5fb3a0` on the mover.
pub(super) fn set_pitch(
    aim: &mut VehicleAim,
    mover: &MoverPose,
    rec: Option<&VehicleRec>,
    pitch: f32,
    net: &crate::net::NetCommands,
    script: Option<&mut UiScript>,
) {
    let pitch = match rec.map(limits) {
        Some((min, _)) if pitch < min => min,
        Some((_, max)) if max <= pitch => max,
        _ => pitch,
    };
    if mover.flags2 & FLAG2_FULL_SPEED_PITCHING != 0 {
        if aim.held != Some(pitch) {
            aim.target = Some(pitch);
        }
        aim.held = Some(pitch);
        return;
    }
    aim.pitch = pitch;
    mover.send(net, opcode::MSG_MOVE_SET_PITCH, mover.flags, pitch);
    angle_event(script, rec, pitch);
}

/// The step a turn takes this frame toward `toward`, at `rate` radians a second.
fn step(pitch: f32, toward: f32, rate: f32, dt: f32) -> f32 {
    let d = toward - pitch;
    let reach = rate * dt;
    if d.abs() <= reach {
        toward
    } else {
        pitch + reach.copysign(d)
    }
}

/// Turn the mover's pitch toward its target or under its keys, and report a turn's start and
/// stop.
pub(super) fn drive(
    aim: &mut VehicleAim,
    mover: &MoverPose,
    rec: Option<&VehicleRec>,
    rate: f32,
    dt: f32,
    net: &crate::net::NetCommands,
    script: Option<&mut UiScript>,
) {
    let (min, max) = rec.map_or(
        (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2),
        limits,
    );
    let (dir, toward) = match aim.target {
        Some(t) if (t - aim.pitch).abs() > f32::EPSILON => ((t - aim.pitch).signum() as i8, t),
        Some(_) => {
            aim.target = None;
            (0, aim.pitch)
        }
        None => match aim.keys {
            [true, false] => (1, max),
            [false, true] => (-1, min),
            _ => (0, aim.pitch),
        },
    };
    let was = std::mem::replace(&mut aim.turning, dir);
    if dir != was {
        let (op, flags) = match dir {
            1 => (opcode::MSG_MOVE_START_PITCH_UP, mover.flags | PITCH_UP),
            -1 => (opcode::MSG_MOVE_START_PITCH_DOWN, mover.flags | PITCH_DOWN),
            _ => (opcode::MSG_MOVE_STOP_PITCH, mover.flags),
        };
        mover.send(net, op, flags, aim.pitch);
    }
    if dir == 0 {
        return;
    }
    let next = step(aim.pitch, toward, rate, dt).clamp(min, max);
    if next != aim.pitch {
        aim.pitch = next;
        angle_event(script, rec, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net_pair() -> (
        crate::net::NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (crate::net::NetCommands(tx), rx)
    }

    fn pose(flags2: u16) -> MoverPose {
        MoverPose {
            guid: 0xF150_0000_0000_0001,
            flags: 0,
            flags2,
            pos: [0.0; 3],
            orientation: 0.0,
        }
    }

    fn rec() -> VehicleRec {
        VehicleRec {
            flags: 0x40 | 0x400,
            pitch_min: -0.5,
            pitch_max: 1.0,
            ..Default::default()
        }
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<(u16, f32)> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::Vehicle(VehicleCommand::Pitch { opcode, pitch, .. }) => {
                    Some((opcode, pitch))
                }
                _ => None,
            })
            .collect()
    }

    /// A plain mover snaps to the clamped pitch and reports it.
    #[test]
    fn a_pitch_snaps_within_the_record_s_limits() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim::default();
        set_pitch(&mut aim, &pose(0), Some(&rec()), 2.0, &net, None);
        assert_eq!(aim.pitch, 1.0);
        assert_eq!(sent(&rx), [(opcode::MSG_MOVE_SET_PITCH, 1.0)]);
    }

    /// A full-speed mover turns to the target at its rate, starting and stopping the turn.
    #[test]
    fn a_full_speed_mover_turns_to_its_target() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim::default();
        let p = pose(FLAG2_FULL_SPEED_PITCHING);
        set_pitch(&mut aim, &p, Some(&rec()), 0.5, &net, None);
        assert_eq!(aim.pitch, 0.0, "nothing moves until the turn runs");
        drive(&mut aim, &p, Some(&rec()), 1.0, 0.25, &net, None);
        assert!((aim.pitch - 0.25).abs() < 1.0e-6);
        drive(&mut aim, &p, Some(&rec()), 1.0, 0.5, &net, None);
        assert_eq!(aim.pitch, 0.5);
        drive(&mut aim, &p, Some(&rec()), 1.0, 0.5, &net, None);
        let ops: Vec<u16> = sent(&rx).into_iter().map(|(o, _)| o).collect();
        assert_eq!(
            ops,
            [opcode::MSG_MOVE_START_PITCH_UP, opcode::MSG_MOVE_STOP_PITCH]
        );
    }

    /// The down key turns down to the limit and its release stops the turn.
    #[test]
    fn the_keys_turn_to_the_limits() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim {
            keys: [false, true],
            ..Default::default()
        };
        let p = pose(0);
        drive(&mut aim, &p, Some(&rec()), 1.0, 1.0, &net, None);
        assert_eq!(aim.pitch, -0.5);
        aim.keys = [false, false];
        drive(&mut aim, &p, Some(&rec()), 1.0, 1.0, &net, None);
        let ops: Vec<u16> = sent(&rx).into_iter().map(|(o, _)| o).collect();
        assert_eq!(
            ops,
            [
                opcode::MSG_MOVE_START_PITCH_DOWN,
                opcode::MSG_MOVE_STOP_PITCH
            ]
        );
    }
}
