//! The movement wire: the `[packed guid][MovementInfo]` relay of other units' moves, teleport
//! acks, speed changes, knockbacks, compressed batches and the movement-mode opcodes.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, MovementInfo};
use benilla_protocol::wire::Vector3d;
use benilla_protocol::ServerPacket;
use common::hx;

/// `SMSG_FORCE_*_SPEED_CHANGE` is `[packed guid][u32 counter][f32 speed]` (vmangos
/// `SendSpeedChangeToController`); the ack is `[packed guid][u32 counter][MovementInfo][f32 speed]`
/// (AzerothCore `HandleForceSpeedChangeAck`).
#[test]
fn force_speed_change_parses_and_ack_body_golden() {
    use benilla_protocol::messages::SpeedKind;

    // SMSG_FORCE_RUN_SPEED_CHANGE: packed guid 8 (mask 0x01, one byte), counter 7, speed 14.0.
    let body = hx("01080700000000006041");
    let packet =
        messages::parse_server(messages::opcode::SMSG_FORCE_RUN_SPEED_CHANGE, &body).unwrap();
    match &packet {
        ServerPacket::ForceSpeedChange {
            guid: 8,
            kind: SpeedKind::Run,
            counter: 7,
            speed,
        } => assert_eq!(*speed, 14.0),
        _ => panic!("expected ForceSpeedChange (run, guid 8, counter 7)"),
    }
    match decode(packet).as_slice() {
        [SessionEvent::ForceSpeedChange {
            guid: 8,
            kind: SpeedKind::Run,
            counter: 7,
            speed,
        }] => assert_eq!(*speed, 14.0),
        other => panic!("expected one ForceSpeedChange event, got {other:?}"),
    }
    // Each kind has its own opcode; walk, swim-back and turn rate sit at 730-735, not 226-231.
    for (op, kind) in [
        (
            messages::opcode::SMSG_FORCE_WALK_SPEED_CHANGE,
            SpeedKind::Walk,
        ),
        (
            messages::opcode::SMSG_FORCE_SWIM_SPEED_CHANGE,
            SpeedKind::Swim,
        ),
        (
            messages::opcode::SMSG_FORCE_TURN_RATE_CHANGE,
            SpeedKind::TurnRate,
        ),
    ] {
        match messages::parse_server(op, &body).unwrap() {
            ServerPacket::ForceSpeedChange { kind: k, .. } => assert_eq!(k, kind),
            _ => panic!("expected ForceSpeedChange for opcode {op:#06x}"),
        }
    }

    let info = MovementInfo {
        flags: 0,
        timestamp: 12345,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 42,
        jump: None,
    };
    assert_eq!(
        messages::force_speed_ack(8, 7, &info, 14.0),
        hx("010807000000000000000000393000000000803f00000040000040400000003f2a00000000006041"),
        "CMSG_FORCE_*_SPEED_CHANGE_ACK body"
    );
    assert_eq!(SpeedKind::Run.ack_opcode(), 0x00E3);
    assert_eq!(SpeedKind::Walk.ack_opcode(), 0x02DB);
    assert_eq!(SpeedKind::TurnRate.ack_opcode(), 0x02DF);
}

/// The knockback and its ack: `SMSG_MOVE_KNOCK_BACK` orders the launch vcos, vsin, speedXY,
/// speedZ; the ack's jump tail orders it zspeed, sin, cos, xyspeed, and the server rejects a
/// mismatched ack. speedZ is down-positive: the reference stores it as-is (`0x7c61f0`) where a
/// jump stores -7.955547, so -12 throws the body up.
#[test]
fn knock_back_parses_and_ack_body_golden() {
    // Packed guid 8, counter 7, then vcos 0.6, vsin 0.8, speedXY 25, speedZ -12.
    let body = hx("0108070000009a99193fcdcc4c3f0000c841000040c1");
    let packet = messages::parse_server(messages::opcode::SMSG_MOVE_KNOCK_BACK, &body).unwrap();
    let launch = match &packet {
        ServerPacket::KnockBack {
            guid: 8,
            counter: 7,
            launch,
        } => *launch,
        other => panic!(
            "expected KnockBack (guid 8, counter 7), got {}",
            other.name()
        ),
    };
    assert_eq!(launch.cos_angle, 0.6, "first float on the wire is vcos");
    assert_eq!(launch.sin_angle, 0.8, "second is vsin");
    assert_eq!(launch.xy_speed, 25.0, "third is the horizontal speed");
    assert_eq!(
        launch.zspeed, -12.0,
        "fourth is speedZ, down-positive — negative throws the body up"
    );
    match decode(packet).as_slice() {
        [SessionEvent::KnockBack {
            guid: 8,
            counter: 7,
            launch: got,
        }] => assert_eq!(*got, launch),
        other => panic!("expected one KnockBack event, got {other:?}"),
    }

    // The ack: the packed guid, the counter, and a MovementInfo with the shared JUMPING bit
    // (0x2000), written as 3.3.5a's FALLING (0x1000), without which no jump tail is written.
    let info = MovementInfo {
        flags: 0x2000,
        timestamp: 12345,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: Some(launch),
    };
    assert_eq!(
        messages::knock_back_ack(8, 7, &info),
        hx("010807000000001000000000393000000000803f00000040000040400000003f00000000000040c1cdcc4c3f9a99193f0000c841"),
        "CMSG_MOVE_KNOCK_BACK_ACK body"
    );
    assert_eq!(messages::opcode::SMSG_MOVE_KNOCK_BACK, 0x00EF);
    assert_eq!(messages::opcode::CMSG_MOVE_KNOCK_BACK_ACK, 0x00F0);
}

/// A misframed batch is a parse error, never a silent drop of the moves it carries.
#[test]
fn a_misframed_batch_errors_rather_than_dropping_moves() {
    // A HEARTBEAT record claiming a 40-byte body with only 4 bytes left in the stream.
    let truncated = hx("070000007801d37ac700020006c10119");
    assert!(messages::parse_server(messages::opcode::SMSG_COMPRESSED_MOVES, &truncated).is_err());

    // A record whose size (1) cannot hold its opcode: misframed, and it must not underflow.
    let undersized = hx("030000007801637cc7000001e200f0");
    assert!(messages::parse_server(messages::opcode::SMSG_COMPRESSED_MOVES, &undersized).is_err());
}

/// The twelve `SMSG_SPLINE_MOVE_*` opcodes (reference dispatch `0x304..=0x31A`, root at `0x31A`):
/// the body is a bare packed guid with no counter, and `SET_RUN_MODE` clears
/// `MOVEFLAG_WALK_MODE`, because the reference (`0x617e80`) passes its bool to `SetRunMode`.
#[test]
fn spline_move_mode_family_parses_golden() {
    use benilla_protocol::messages::SplineMode;

    let body = hx("01aa");
    assert_eq!(body.len(), 2, "the whole body is one packed guid");

    let expected = [
        (
            messages::opcode::SMSG_SPLINE_MOVE_ROOT,
            SplineMode::Root,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_UNROOT,
            SplineMode::Root,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_WATER_WALK,
            SplineMode::WaterWalk,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_LAND_WALK,
            SplineMode::WaterWalk,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_FEATHER_FALL,
            SplineMode::FeatherFall,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_NORMAL_FALL,
            SplineMode::FeatherFall,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_HOVER,
            SplineMode::Hover,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_UNSET_HOVER,
            SplineMode::Hover,
            false,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_START_SWIM,
            SplineMode::Swimming,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_STOP_SWIM,
            SplineMode::Swimming,
            false,
        ),
        // The inversion: the WALK opcode sets the bit, the RUN opcode clears it.
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_WALK_MODE,
            SplineMode::WalkMode,
            true,
        ),
        (
            messages::opcode::SMSG_SPLINE_MOVE_SET_RUN_MODE,
            SplineMode::WalkMode,
            false,
        ),
    ];

    for (op, mode, apply) in expected {
        let packet = messages::parse_server(op, &body)
            .unwrap_or_else(|e| panic!("{op:#06x} must parse from a bare packed guid: {e}"));
        match &packet {
            ServerPacket::SplineMoveMode {
                guid: 0xAA,
                mode: m,
                apply: a,
            } => assert_eq!(
                (*m, *a),
                (mode, apply),
                "{} decoded to the wrong mode/direction",
                packet.name()
            ),
            p => panic!("expected SplineMoveMode for {op:#06x}, got {}", p.name()),
        }
        match decode(packet).as_slice() {
            [SessionEvent::SplineMoveMode {
                guid: 0xAA,
                mode: m,
                apply: a,
            }] => assert_eq!((*m, *a), (mode, apply)),
            other => panic!("expected one SplineMoveMode event for {op:#06x}, got {other:?}"),
        }
    }

    assert_eq!(SplineMode::Root.flag(), messages::MoveMode::Root.flag());
    assert_eq!(
        SplineMode::WaterWalk.flag(),
        messages::MoveMode::WaterWalk.flag()
    );
    assert_eq!(
        SplineMode::FeatherFall.flag(),
        messages::MoveMode::FeatherFall.flag()
    );
    assert_eq!(SplineMode::Hover.flag(), messages::MoveMode::Hover.flag());
}
