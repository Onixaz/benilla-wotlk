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
/// `MOVEMENTFLAG2_ALWAYS_ALLOW_PITCHING`, the extra flag `0x5fa790` tests before the row's camera
/// bit.
const FLAG2_CAN_PITCH: u16 = 0x20;
/// `MOVEMENTFLAG_SWIMMING` and `MOVEMENTFLAG_FLYING`, the `0x2200000` pair.
const SWIMMING: u32 = 0x20_0000;
const FLYING: u32 = 0x200_0000;

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
    /// `VehicleAimGetNormPower`, the launch power a trajectory cast reads.
    pub(crate) power: f32,
    /// A pitch was set since the aim preview last looked: the pitch setter's closing mark
    /// (`0x6fbf80`), which the preview reads and clears.
    pub(crate) pitched: bool,
    /// The camera aim pitch mouselook last pitched the mover from, `None` while it does not.
    pub(crate) mouse_seen: Option<f32>,
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
    // Marked on both arms: the reference's mark closes the function.
    aim.pitched = true;
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

/// The mouse's say in the aim this frame.
pub(super) struct MouseAim {
    /// Mouselook syncs the mover's facing to the camera (`0x5fa110`).
    pub(super) syncing: bool,
    /// The camera may pitch the mover (`0x5fa790`).
    pub(super) can_pitch: bool,
    /// The row aims with the mouse's vertical motion, the camera's pitch held (`0x5fba60`).
    pub(super) held: bool,
    /// The camera's aim pitch, for the camera route.
    pub(super) camera_pitch: f32,
    /// The held look's pitch motion since the last frame, for the motion route.
    pub(super) delta: f32,
}

/// Mouselook's two routes to the pitch setter. A row that holds the camera's pitch takes the
/// mouse's vertical motion onto the held aim (`0x5fba60`); any other takes the camera's pitch
/// past the row's `MouseLookOffsetPitch` as it syncs the facing (`0x6023d0`, `0x5fbe70`). This
/// pitch's sign is the reference's camera pitch negated, as the mover pitch takes it.
pub(super) fn mouse_aim(
    aim: &mut VehicleAim,
    mover: &MoverPose,
    rec: &VehicleRec,
    mouse: &MouseAim,
    net: &crate::net::NetCommands,
    script: Option<&mut UiScript>,
) {
    if !mouse.syncing {
        aim.mouse_seen = None;
        return;
    }
    if !mouse.can_pitch {
        return;
    }
    if mouse.held {
        if mouse.delta != 0.0 {
            let from = aim.held.unwrap_or(aim.pitch);
            set_pitch(aim, mover, Some(rec), from + mouse.delta, net, script);
        }
        return;
    }
    if aim.mouse_seen != Some(mouse.camera_pitch) {
        aim.mouse_seen = Some(mouse.camera_pitch);
        let pitch = rec.mouse_look_offset_pitch + mouse.camera_pitch;
        set_pitch(aim, mover, Some(rec), pitch, net, script);
    }
}

/// The row aims with the mouse's motion and holds the camera's pitch: `flags & 0x40040000` is
/// `0x40000`, off water and out of flight (`0x756f00`).
pub(super) fn holds_camera_pitch(rec: &VehicleRec, move_flags: u32) -> bool {
    move_flags & (SWIMMING | FLYING) == 0 && rec.flags & 0x4004_0000 == 0x4_0000
}

/// `0x5fa790`'s mover terms: alive, not stunned, standing, and, for a mover whose extra flags
/// pitch it (`0x20`) on land, a row that lets the camera pitch it (`0x40000`).
pub(super) fn camera_may_pitch(
    rec: Option<&VehicleRec>,
    move_flags: u32,
    flags2: u16,
    alive_and_standing: bool,
) -> bool {
    if !alive_and_standing {
        return false;
    }
    if flags2 & FLAG2_CAN_PITCH != 0 && move_flags & (SWIMMING | FLYING) == 0 {
        return rec.is_some_and(|r| r.flags & 0x4_0000 != 0);
    }
    true
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

    fn mouse(syncing: bool, held: bool, camera_pitch: f32, delta: f32) -> MouseAim {
        MouseAim {
            syncing,
            can_pitch: true,
            held,
            camera_pitch,
            delta,
        }
    }

    /// The camera route: the camera's aim pitch past the row's offset, once per change, and a
    /// fresh start after mouselook ends.
    #[test]
    fn mouselook_pitches_the_vehicle_by_the_camera_past_its_offset() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim::default();
        let rec = VehicleRec {
            mouse_look_offset_pitch: 0.25,
            ..rec()
        };
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec,
            &mouse(true, false, 0.25, 0.0),
            &net,
            None,
        );
        assert_eq!(aim.pitch, 0.5);
        assert!(aim.pitched, "the preview sees the pitch");
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec,
            &mouse(true, false, 0.25, 0.0),
            &net,
            None,
        );
        assert_eq!(sent(&rx).len(), 1, "a still camera sends nothing more");
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec,
            &mouse(false, false, 0.25, 0.0),
            &net,
            None,
        );
        assert_eq!(aim.mouse_seen, None, "mouselook ended");
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec,
            &mouse(true, false, 0.25, 0.0),
            &net,
            None,
        );
        assert_eq!(
            sent(&rx).len(),
            1,
            "the next mouselook takes the camera at once"
        );
    }

    /// The motion route: the held aim moves by the mouse's pitch motion, the camera's pitch
    /// untouched.
    #[test]
    fn a_held_camera_pitch_aims_by_the_mouse_motion() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim {
            pitch: 0.25,
            ..Default::default()
        };
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec(),
            &mouse(true, true, 0.9, 0.125),
            &net,
            None,
        );
        assert_eq!(aim.pitch, 0.375);
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec(),
            &mouse(true, true, 0.9, 0.0),
            &net,
            None,
        );
        assert_eq!(sent(&rx), [(opcode::MSG_MOVE_SET_PITCH, 0.375)]);
    }

    /// No pitch while mouselook may not pitch the mover, or does not sync its facing.
    #[test]
    fn a_gated_mouse_does_not_pitch() {
        let (net, rx) = net_pair();
        let mut aim = VehicleAim::default();
        let gated = MouseAim {
            can_pitch: false,
            ..mouse(true, false, 0.5, 0.0)
        };
        mouse_aim(&mut aim, &pose(0), &rec(), &gated, &net, None);
        mouse_aim(
            &mut aim,
            &pose(0),
            &rec(),
            &mouse(false, true, 0.0, 0.5),
            &net,
            None,
        );
        assert!(sent(&rx).is_empty());
    }

    /// The row's two bits pick the route, on land; `0x20` movers need the row's camera bit.
    #[test]
    fn the_row_picks_the_mouse_route() {
        let with = |flags| VehicleRec {
            flags,
            ..Default::default()
        };
        assert!(holds_camera_pitch(&with(0x4_0000), 0));
        assert!(!holds_camera_pitch(&with(0x4004_0000), 0));
        assert!(!holds_camera_pitch(&with(0x4_0000), SWIMMING));
        assert!(camera_may_pitch(Some(&with(0)), 0, 0, true));
        assert!(!camera_may_pitch(Some(&with(0)), 0, FLAG2_CAN_PITCH, true));
        assert!(camera_may_pitch(
            Some(&with(0x4_0000)),
            0,
            FLAG2_CAN_PITCH,
            true
        ));
        assert!(camera_may_pitch(
            Some(&with(0)),
            FLYING,
            FLAG2_CAN_PITCH,
            true
        ));
        assert!(!camera_may_pitch(Some(&with(0x4_0000)), 0, 0, false));
    }
}
