//! The `MovementInfo` body every `MSG_MOVE_*` carries, its flag bits, and the movement verbs.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_u32_le, Vector3d};

// MovementFlags bits (vmangos `MovementInfo.h`). ON_TRANSPORT is 1.12's 0x0200_0000
// (`MovementInfo.h:56`), not TBC's 0x200, which 1.12 names `MOVEFLAG_UNUSED10`.
pub(super) const MOVEMENT_FLAG_ON_TRANSPORT: u32 = 0x0200_0000;
pub(super) const MOVEMENT_FLAG_JUMPING: u32 = 0x2000;
#[cfg(test)]
pub(super) const MOVEMENT_FLAG_SWIMMING: u32 = 0x20_0000;
pub(super) const MOVEMENT_FLAG_SPLINE_ENABLED: u32 = 0x40_0000;
/// 1.12.1's flying bit, 3.3.5a's `FLYING` (`0x200_0000`) once mapped.
const MOVEMENT_FLAG_FLYING: u32 = 0x100_0000;
/// 3.3.5a's `ASCENDING` (`0x40_0000`) in the 1.12.1 word: its `MOVEFLAG_UNUSED10`, since 1.12.1's
/// `0x40_0000` is the spline bit.
pub const CLASSIC_ASCENDING: u32 = 0x200;
/// 3.3.5a's `DESCENDING` (`0x80_0000`) in the 1.12.1 word, where that bit is unused.
pub const CLASSIC_DESCENDING: u32 = 0x80_0000;

/// `MSG_MOVE_TIME_SKIPPED` inbound: a packed guid and the ms that mover's client skipped
/// (reference handler `0x603b40`). Our own `CMSG_MOVE_TIME_SKIPPED` has the same shape.
pub(super) fn read_move_time_skipped(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    let guid = crate::wire::read_packed_guid(r)?;
    let lag_ms = read_u32_le(r)?;
    Ok((guid, lag_ms))
}

/// 3.3.5a's `MovementFlags` (AzerothCore `UnitDefines.h`), where its bit differs from 1.12.1's.
mod wotlk_flag {
    pub const ON_TRANSPORT: u32 = 0x200;
    pub const DISABLE_GRAVITY: u32 = 0x400;
    pub const ROOT: u32 = 0x800;
    pub const FALLING: u32 = 0x1000;
    pub const SWIMMING: u32 = 0x20_0000;
    /// Set by StartAscend `0x9898e0`, cleared with [`DESCENDING`] by StopAscend `0x989940`.
    pub const ASCENDING: u32 = 0x40_0000;
    pub const DESCENDING: u32 = 0x80_0000;
    pub const FLYING: u32 = 0x200_0000;
    pub const SPLINE_ELEVATION: u32 = 0x400_0000;
    pub const SPLINE_ENABLED: u32 = 0x800_0000;
    /// `MovementFlags2`: the transport tail carries a second time.
    pub const FLAG2_INTERPOLATED_MOVEMENT: u16 = 0x400;
    /// `MovementFlags2`: pitch is on the wire without swimming or flying.
    pub const FLAG2_ALWAYS_ALLOW_PITCHING: u16 = 0x20;
}

/// 3.3.5a's movement flags in 1.12.1's bits. 3.3.5a moved transport to `0x200` and spline to
/// `0x0800_0000`, shifted root through the pending bits down by one and added ascending,
/// descending, can-fly and pending-root. Ascending and descending ride 1.12.1's two unused bits,
/// [`CLASSIC_ASCENDING`] and [`CLASSIC_DESCENDING`]; can-fly and pending-root are dropped. Its
/// gravity off is 1.12.1's levitating and its flying 1.12.1's `0x0100_0000`.
pub(crate) fn classic_flags_from_wotlk(flags: u32) -> u32 {
    // Forward through walking, and swimming, spline elevation, water walk, safe fall and hover,
    // keep their bit.
    let mut classic = flags & (0x1ff | 0x20_0000 | 0x400_0000 | 0x7000_0000);
    let moved: [(u32, u32); 15] = [
        (wotlk_flag::ON_TRANSPORT, MOVEMENT_FLAG_ON_TRANSPORT),
        (wotlk_flag::ASCENDING, CLASSIC_ASCENDING),
        (wotlk_flag::DESCENDING, CLASSIC_DESCENDING),
        (wotlk_flag::DISABLE_GRAVITY, 0x400),
        (wotlk_flag::ROOT, 0x1000),
        (wotlk_flag::FALLING, MOVEMENT_FLAG_JUMPING),
        (0x2000, 0x4000),      // falling far
        (0x4000, 0x8000),      // pending stop
        (0x8000, 0x1_0000),    // pending strafe stop
        (0x1_0000, 0x2_0000),  // pending forward
        (0x2_0000, 0x4_0000),  // pending backward
        (0x4_0000, 0x8_0000),  // pending strafe left
        (0x8_0000, 0x10_0000), // pending strafe right
        (wotlk_flag::FLYING, 0x100_0000),
        (wotlk_flag::SPLINE_ENABLED, MOVEMENT_FLAG_SPLINE_ENABLED),
    ];
    for (wotlk, shared) in moved {
        if flags & wotlk != 0 {
            classic |= shared;
        }
    }
    classic
}

/// 1.12.1's movement flags in 3.3.5a's bits, the inverse of [`classic_flags_from_wotlk`] on every
/// bit it produces.
pub(crate) fn wotlk_flags_from_classic(classic: u32) -> u32 {
    let mut flags = classic & (0x1ff | 0x20_0000 | 0x400_0000 | 0x7000_0000);
    let moved: [(u32, u32); 15] = [
        (MOVEMENT_FLAG_ON_TRANSPORT, wotlk_flag::ON_TRANSPORT),
        (CLASSIC_ASCENDING, wotlk_flag::ASCENDING),
        (CLASSIC_DESCENDING, wotlk_flag::DESCENDING),
        (0x400, wotlk_flag::DISABLE_GRAVITY),
        (0x1000, wotlk_flag::ROOT),
        (MOVEMENT_FLAG_JUMPING, wotlk_flag::FALLING),
        (0x4000, 0x2000),      // falling far
        (0x8000, 0x4000),      // pending stop
        (0x1_0000, 0x8000),    // pending strafe stop
        (0x2_0000, 0x1_0000),  // pending forward
        (0x4_0000, 0x2_0000),  // pending backward
        (0x8_0000, 0x4_0000),  // pending strafe left
        (0x10_0000, 0x8_0000), // pending strafe right
        (MOVEMENT_FLAG_FLYING, wotlk_flag::FLYING),
        (MOVEMENT_FLAG_SPLINE_ENABLED, wotlk_flag::SPLINE_ENABLED),
    ];
    for (shared, wotlk) in moved {
        if classic & shared != 0 {
            flags |= wotlk;
        }
    }
    flags
}

/// 3.3.5a's `MovementInfo` (AzerothCore `Unit::BuildMovementPacket`): `u32` flags, `u16` extra
/// flags, time, position, facing; with `ONTRANSPORT` a packed transport guid, its local pose, a
/// `u32` transport time, the `i8` seat, and a second time under `INTERPOLATED_MOVEMENT`; pitch when
/// swimming, flying or `ALWAYS_ALLOW_PITCHING`; fall time; the jump under `FALLING` as `zspeed,
/// sin, cos, xyspeed`; the spline elevation. The flags come back in 1.12.1's bits
/// ([`classic_flags_from_wotlk`]); the extra flags, transport time and seat are read and dropped.
pub(crate) fn read_movement_info_wotlk(r: &mut impl Read) -> io::Result<MovementInfo> {
    use crate::wire::{read_packed_guid, read_u16_le, read_u8};
    let flags = read_u32_le(r)?;
    let flags2 = read_u16_le(r)?;
    let timestamp = read_u32_le(r)?;
    let position = Vector3d::read(r)?;
    let orientation = read_f32_le(r)?;
    let transport = if flags & wotlk_flag::ON_TRANSPORT != 0 {
        let pose = TransportPose {
            guid: read_packed_guid(r)?,
            pos: Vector3d::read(r)?,
            orientation: read_f32_le(r)?,
        };
        let _transport_time = read_u32_le(r)?;
        let _seat = read_u8(r)?;
        if flags2 & wotlk_flag::FLAG2_INTERPOLATED_MOVEMENT != 0 {
            let _transport_time_2 = read_u32_le(r)?;
        }
        Some(pose)
    } else {
        None
    };
    let pitch = if flags & (wotlk_flag::SWIMMING | wotlk_flag::FLYING) != 0
        || flags2 & wotlk_flag::FLAG2_ALWAYS_ALLOW_PITCHING != 0
    {
        read_f32_le(r)?
    } else {
        0.0
    };
    let fall_time = read_u32_le(r)?;
    let jump = if flags & wotlk_flag::FALLING != 0 {
        let zspeed = read_f32_le(r)?;
        let sin_angle = read_f32_le(r)?;
        let cos_angle = read_f32_le(r)?;
        Some(JumpInfo {
            zspeed,
            cos_angle,
            sin_angle,
            xy_speed: read_f32_le(r)?,
        })
    } else {
        None
    };
    if flags & wotlk_flag::SPLINE_ELEVATION != 0 {
        let _ = read_f32_le(r)?;
    }
    Ok(MovementInfo {
        flags: classic_flags_from_wotlk(flags),
        timestamp,
        position,
        orientation,
        transport,
        pitch,
        fall_time,
        jump,
    })
}

// --- movement vocabulary ---

/// A mover speed, each with its own force-change opcode pair; the order is the `LIVING` block's
/// speed array and vmangos `UnitMoveType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedKind {
    Walk,
    Run,
    RunBack,
    Swim,
    SwimBack,
    TurnRate,
}

impl SpeedKind {
    /// The `CMSG_FORCE_*_SPEED_CHANGE_ACK` opcode answering this kind's SMSG.
    pub fn ack_opcode(self) -> u16 {
        use super::opcode;
        match self {
            SpeedKind::Walk => opcode::CMSG_FORCE_WALK_SPEED_CHANGE_ACK,
            SpeedKind::Run => opcode::CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
            SpeedKind::RunBack => opcode::CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK,
            SpeedKind::Swim => opcode::CMSG_FORCE_SWIM_SPEED_CHANGE_ACK,
            SpeedKind::SwimBack => opcode::CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK,
            SpeedKind::TurnRate => opcode::CMSG_FORCE_TURN_RATE_CHANGE_ACK,
        }
    }
}

/// A mode the server grants the controlling client, one `MOVEMENTFLAGS` bit each (vmangos
/// `MovementInfo.h:28-62`, reference setters `0x7c7280`-`0x7c7370`). Root stops translation and
/// falling but not turning (`0x618054`); water walk makes liquid walkable (aura 104); feather fall
/// caps the fall at 7 yd/s instead of 60.148 (aura 105, `0x7c5d20`); hover lifts ground contact by
/// 1.0 yd (aura 106, `0x6367b0`). Levitate (spell 1706) grants the last three at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMode {
    Root,
    WaterWalk,
    FeatherFall,
    Hover,
}

impl MoveMode {
    /// This mode's `MOVEMENTFLAGS` bit.
    pub fn flag(self) -> u32 {
        match self {
            MoveMode::Root => 0x0000_1000,
            MoveMode::WaterWalk => 0x1000_0000,
            MoveMode::FeatherFall => 0x2000_0000,
            MoveMode::Hover => 0x4000_0000,
        }
    }

    /// The `CMSG_*_ACK` answering this mode's SMSG. Only root acks apply and remove on different
    /// opcodes (vmangos routes both to `HandleMoveRootAck`).
    pub fn ack_opcode(self, apply: bool) -> u16 {
        use super::opcode;
        match (self, apply) {
            (MoveMode::Root, true) => opcode::CMSG_FORCE_MOVE_ROOT_ACK,
            (MoveMode::Root, false) => opcode::CMSG_FORCE_MOVE_UNROOT_ACK,
            (MoveMode::WaterWalk, _) => opcode::CMSG_MOVE_WATER_WALK_ACK,
            (MoveMode::FeatherFall, _) => opcode::CMSG_MOVE_FEATHER_FALL_ACK,
            (MoveMode::Hover, _) => opcode::CMSG_MOVE_HOVER_ACK,
        }
    }

    /// Whether the ack ends in a `u32 apply`: not root's, whose handler infers it from the opcode
    /// (vmangos `Server/Packets/Movement.cpp:38-59`).
    pub fn ack_carries_apply(self) -> bool {
        !matches!(self, MoveMode::Root)
    }
}

/// A mode set on a unit we do not control, by the twelve `SMSG_SPLINE_MOVE_*`: a bare packed guid,
/// no counter, no ack (client handler `0x603c80`, any unit). vmangos sends the four shared modes
/// only for units no player moves, and walk/run always. Walk mode and swimming have no acked form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplineMode {
    Root,
    WaterWalk,
    FeatherFall,
    Hover,
    /// `MOVEFLAG_WALK_MODE` (`0x100`); `apply` is the flag's direction, so `SET_WALK_MODE` applies
    /// and `SET_RUN_MODE` removes (the reference `0x617e80` feeds `SetRunMode`, `0x7c71c0`).
    WalkMode,
    /// `MOVEFLAG_SWIMMING` (`0x20_0000`), by `START_SWIM`/`STOP_SWIM`. The reference handles them
    /// (`0x61a130`, `0x61a160`); vmangos never sends them (`Opcodes.cpp:888`).
    Swimming,
}

impl SplineMode {
    /// This mode's `MOVEMENTFLAGS` bit, in the same word as [`MoveMode::flag`].
    pub fn flag(self) -> u32 {
        match self {
            SplineMode::Root => 0x0000_1000,
            SplineMode::WaterWalk => 0x1000_0000,
            SplineMode::FeatherFall => 0x2000_0000,
            SplineMode::Hover => 0x4000_0000,
            SplineMode::WalkMode => 0x0000_0100,
            SplineMode::Swimming => 0x0020_0000,
        }
    }
}

/// What a relayed move's opcode means beyond its pose. The 23 `[packed guid][MovementInfo]` relays
/// share one reference handler (`0x603bb0`, `0x601580`), and for most the opcode adds nothing the
/// flags word does not say. The variants beyond `Pose` name the opcodes a receiver tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayVerb {
    /// Every other relay (start, stop, strafe, jump, turn, swim, land, facing, modes, knockback).
    #[default]
    Pose,
    /// `MSG_MOVE_HEARTBEAT`, the periodic mid-move pulse. Both reference blends apply to it
    /// (`0x619030`, `0x619090`); the tag they skip, `0x26`, is only pushed for a teleport.
    Heartbeat,
    /// `MSG_MOVE_TELEPORT`, an observed near-teleport (Blink, `.tele`): the one relay the client
    /// never smooths toward (tag `0x26`). It also re-bases the mover, zeroes its interpolation
    /// (`0x617e90`) and past 30 yd re-anchors the world.
    Teleport,
    /// `MSG_MOVE_ROOT` (`true`) / `MSG_MOVE_UNROOT` (`false`): the opcode, not the flags word,
    /// decides. The client then runs `SetRoot` (`0x7c7340`, also wiping motion) or `ClearRoot`
    /// (`0x7c7370`) unconditionally; vmangos's flags already agree (`MovementHandler.cpp:1064`).
    Root(bool),
}

impl RelayVerb {
    /// The verb a relay opcode carries.
    pub fn of(opcode: u16) -> Self {
        use super::opcode as op;
        match opcode {
            op::MSG_MOVE_HEARTBEAT => Self::Heartbeat,
            op::MSG_MOVE_TELEPORT => Self::Teleport,
            op::MSG_MOVE_ROOT => Self::Root(true),
            op::MSG_MOVE_UNROOT => Self::Root(false),
            _ => Self::Pose,
        }
    }
}

/// The jump tail: `zspeed, sinAngle, cosAngle, xyspeed` in 3.3.5a's `MovementInfo` under `FALLING`
/// (AzerothCore `ReadMovementInfo`). The launch is `(cos, sin) * xy_speed` in world XY, frozen at take-off,
/// so a receiver can replay the arc under `g = 19.291105` (vmangos `ExtrapolateMovement`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct JumpInfo {
    /// Take-off vertical speed (yd/s), down-positive: the 1.12 client sends `-7.955547` for a
    /// rising jump, so the up-speed is `-zspeed`.
    pub zspeed: f32,
    pub cos_angle: f32,
    pub sin_angle: f32,
    /// Horizontal launch speed (yd/s); the frozen ground speed at take-off.
    pub xy_speed: f32,
}

/// The `ON_TRANSPORT` tail of `MovementInfo` and of a `LIVING` block: the transport guid, then
/// position and facing in the transport's own frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportPose {
    /// The transport GameObject's guid, `HIGH_MO_TRANSPORT` or `HIGH_TRANSPORT`.
    pub guid: u64,
    pub pos: Vector3d,
    pub orientation: f32,
}

/// The body of every `MSG_MOVE_*`, in both directions, and of the teleport ack.
pub struct MovementInfo {
    pub flags: u32,
    pub timestamp: u32,
    pub position: Vector3d,
    pub orientation: f32,
    /// `Some` iff `ON_TRANSPORT` is set: the sender's pose in the transport's frame.
    pub transport: Option<TransportPose>,
    /// Pitch in radians, up-positive; on the wire only while swimming or flying, else `0.0`.
    pub pitch: f32,
    /// Milliseconds airborne, a `u32` (vmangos `MovementInfo::fallTime`): the jump arc's clock.
    pub fall_time: u32,
    /// `Some` iff `JUMPING` is set: the launch an observer replays the arc from.
    pub jump: Option<JumpInfo>,
}

impl MovementInfo {
    /// `[packed mover guid][MovementInfo]` in 3.3.5a's layout, the shape every client movement
    /// packet opens with and [`read_movement_info_wotlk`] reads back after the guid. AzerothCore
    /// drops a packet whose guid is not the mover (`MovementHandler.cpp`, `HandleMovementOpcodes`).
    /// The flags go out in 3.3.5a's bits ([`wotlk_flags_from_classic`]) with no extra flags; the
    /// transport time is the move's own, and the seat is `-1`, no vehicle seat.
    pub(super) fn write(&self, mover: u64, w: &mut Vec<u8>) {
        crate::wire::write_packed_guid(mover, w).expect("a Vec write cannot fail");
        self.write_body(w);
    }

    /// The `MovementInfo` alone, as an ack carries it after its own guid and counter.
    pub(super) fn write_body(&self, w: &mut Vec<u8>) {
        let flags = wotlk_flags_from_classic(self.flags);
        w.extend_from_slice(&flags.to_le_bytes());
        w.extend_from_slice(&0u16.to_le_bytes());
        w.extend_from_slice(&self.timestamp.to_le_bytes());
        w.extend_from_slice(&self.position.x.to_le_bytes());
        w.extend_from_slice(&self.position.y.to_le_bytes());
        w.extend_from_slice(&self.position.z.to_le_bytes());
        w.extend_from_slice(&self.orientation.to_le_bytes());
        // Each conditional tail must travel with its flag or the server's reader desyncs.
        if flags & wotlk_flag::ON_TRANSPORT != 0 {
            let t = self.transport.unwrap_or(TransportPose {
                guid: 0,
                pos: Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                orientation: 0.0,
            });
            crate::wire::write_packed_guid(t.guid, w).expect("a Vec write cannot fail");
            w.extend_from_slice(&t.pos.x.to_le_bytes());
            w.extend_from_slice(&t.pos.y.to_le_bytes());
            w.extend_from_slice(&t.pos.z.to_le_bytes());
            w.extend_from_slice(&t.orientation.to_le_bytes());
            w.extend_from_slice(&self.timestamp.to_le_bytes());
            w.push(0xff);
        }
        if flags & (wotlk_flag::SWIMMING | wotlk_flag::FLYING) != 0 {
            w.extend_from_slice(&self.pitch.to_le_bytes());
        }
        w.extend_from_slice(&self.fall_time.to_le_bytes());
        if flags & wotlk_flag::FALLING != 0 {
            let j = self.jump.unwrap_or_default();
            w.extend_from_slice(&j.zspeed.to_le_bytes());
            w.extend_from_slice(&j.sin_angle.to_le_bytes());
            w.extend_from_slice(&j.cos_angle.to_le_bytes());
            w.extend_from_slice(&j.xy_speed.to_le_bytes());
        }
        if flags & wotlk_flag::SPLINE_ELEVATION != 0 {
            w.extend_from_slice(&0f32.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Vector3d;

    const MOVER: u64 = 0x0000_0000_0000_03e9;

    fn info(flags: u32) -> MovementInfo {
        MovementInfo {
            flags,
            timestamp: 12345,
            position: Vector3d {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            orientation: 0.5,
            transport: None,
            pitch: -0.7,
            fall_time: 42,
            jump: None,
        }
    }

    /// Write, then read back the way the server does: the packed guid, then 3.3.5a's info.
    fn round_trip(mi: &MovementInfo) -> (u64, MovementInfo, usize) {
        let mut bytes = Vec::new();
        mi.write(MOVER, &mut bytes);
        let mut r = &bytes[..];
        let guid = crate::wire::read_packed_guid(&mut r).expect("guid");
        let back = read_movement_info_wotlk(&mut r).expect("parse");
        assert!(r.is_empty(), "the reader consumes every byte written");
        (guid, back, bytes.len())
    }

    /// A walk report: the mover's packed guid (mask 0x03: two bytes), the forward bit, a zero
    /// `u16` extra-flags word, then the 3.3.5a body: 3 + 4 + 2 + 4 + 16 + 4 = 33 bytes.
    #[test]
    fn a_walk_report_is_the_packed_mover_then_the_wotlk_body() {
        let mut bytes = Vec::new();
        info(0x1).write(MOVER, &mut bytes);
        #[rustfmt::skip]
        let want: &[u8] = &[
            0x03, 0xe9, 0x03,       // packed guid 0x3e9
            0x01, 0x00, 0x00, 0x00, // flags: FORWARD
            0x00, 0x00,             // extra flags
            0x39, 0x30, 0x00, 0x00, // timestamp 12345
            0x00, 0x00, 0x80, 0x3F, // x = 1.0
            0x00, 0x00, 0x00, 0x40, // y = 2.0
            0x00, 0x00, 0x40, 0x40, // z = 3.0
            0x00, 0x00, 0x00, 0x3F, // orientation = 0.5
            0x2a, 0x00, 0x00, 0x00, // fall time 42
        ];
        assert_eq!(bytes, want);
    }

    #[test]
    fn every_shared_flag_maps_to_wotlk_and_back() {
        // Every bit `classic_flags_from_wotlk` can produce survives the round trip.
        for bit in 0..32 {
            let wotlk = 1u32 << bit;
            let classic = classic_flags_from_wotlk(wotlk);
            if classic == 0 {
                continue;
            }
            assert_eq!(
                classic_flags_from_wotlk(wotlk_flags_from_classic(classic)),
                classic,
                "1.12.1 bits {classic:#x} (3.3.5a {wotlk:#x})"
            );
            assert_eq!(wotlk_flags_from_classic(classic), wotlk, "{classic:#x}");
        }
        assert_eq!(wotlk_flags_from_classic(MOVEMENT_FLAG_JUMPING), 0x1000);
        assert_eq!(wotlk_flags_from_classic(MOVEMENT_FLAG_ON_TRANSPORT), 0x200);
        assert_eq!(wotlk_flags_from_classic(0x1000), 0x800, "root");
        assert_eq!(wotlk_flags_from_classic(CLASSIC_ASCENDING), 0x40_0000);
        assert_eq!(wotlk_flags_from_classic(CLASSIC_DESCENDING), 0x80_0000);
        assert_eq!(
            classic_flags_from_wotlk(0x40_0000),
            CLASSIC_ASCENDING,
            "not 1.12.1's spline bit"
        );
    }

    /// An ascending swimmer's report carries both bits in 3.3.5a's places, and the pitch tail.
    #[test]
    fn an_ascending_swimmer_round_trips() {
        let (_, back, _) = round_trip(&info(MOVEMENT_FLAG_SWIMMING | CLASSIC_ASCENDING));
        assert_eq!(back.flags, MOVEMENT_FLAG_SWIMMING | CLASSIC_ASCENDING);
        assert_eq!(back.pitch, -0.7);
        let mut bytes = Vec::new();
        info(MOVEMENT_FLAG_SWIMMING | CLASSIC_DESCENDING).write(MOVER, &mut bytes);
        assert_eq!(
            &bytes[3..7],
            &0x00a0_0000u32.to_le_bytes(),
            "SWIMMING | DESCENDING"
        );
    }

    #[test]
    fn swim_pitch_round_trips_when_swimming() {
        let (guid, back, _) = round_trip(&info(MOVEMENT_FLAG_SWIMMING));
        assert_eq!(guid, MOVER);
        assert_eq!(back.flags, MOVEMENT_FLAG_SWIMMING);
        assert_eq!(back.pitch, -0.7, "the swim pitch survives the round-trip");
        assert_eq!(back.orientation, 0.5);
        assert_eq!(back.fall_time, 42, "fall_time still lands after the pitch");
    }

    #[test]
    fn transport_tail_round_trips_when_riding() {
        let mut riding = info(MOVEMENT_FLAG_ON_TRANSPORT);
        riding.transport = Some(TransportPose {
            guid: 0x1FC0_0000_0000_2B10, // a HIGH_MO_TRANSPORT-shaped guid
            pos: Vector3d {
                x: -5.5,
                y: 2.25,
                z: 8.0,
            },
            orientation: 1.5,
        });
        let (_, back, len) = round_trip(&riding);
        let (_, _, walking) = round_trip(&info(0));
        assert_eq!(
            len - walking,
            5 + 16 + 4 + 1,
            "packed guid (mask + 4 bytes), the pose, transport time and seat"
        );
        assert_eq!(back.flags, MOVEMENT_FLAG_ON_TRANSPORT);
        let t = back.transport.expect("transport pose survives");
        assert_eq!(t.guid, 0x1FC0_0000_0000_2B10);
        assert_eq!((t.pos.x, t.pos.y, t.pos.z), (-5.5, 2.25, 8.0));
        assert_eq!(t.orientation, 1.5);
        assert_eq!(back.fall_time, 42, "fall_time lands after the tail");
    }

    #[test]
    fn the_jump_tail_rides_falling_in_sin_cos_order() {
        let mut jumping = info(MOVEMENT_FLAG_JUMPING);
        jumping.jump = Some(JumpInfo {
            zspeed: -7.955547,
            cos_angle: 0.6,
            sin_angle: 0.8,
            xy_speed: 7.0,
        });
        let mut bytes = Vec::new();
        jumping.write(MOVER, &mut bytes);
        assert_eq!(&bytes[3..7], &0x1000u32.to_le_bytes(), "FALLING is 0x1000");
        let tail = &bytes[bytes.len() - 16..];
        assert_eq!(&tail[4..8], &0.8f32.to_le_bytes(), "sin before cos");
        let (_, back, _) = round_trip(&jumping);
        assert_eq!(back.jump, jumping.jump);
    }

    #[test]
    fn no_pitch_tail_when_not_swimming() {
        let (_, _, swimming) = round_trip(&info(MOVEMENT_FLAG_SWIMMING));
        let (_, back, dry) = round_trip(&info(0));
        assert_eq!(
            swimming - dry,
            4,
            "the pitch adds one f32, and only when swimming"
        );
        assert_eq!(back.pitch, 0.0, "a dry packet parses a level (0) pitch");
        assert_eq!(back.fall_time, 42);
    }
}
