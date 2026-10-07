//! `SMSG_MONSTER_MOVE`: a mover, its start, a spline id, a `moveType` facing, then (unless a stop)
//! spline flags, a duration and the waypoints. `SMSG_MONSTER_MOVE_TRANSPORT` inserts the
//! transport's packed guid after the mover's, and every coordinate is then deck-local, waypoints
//! included (vmangos `MoveSplineInit.cpp:138-170`, `PathFinder.cpp:76-84`).

use std::io;

use crate::messages::{MonsterMoveFacing, ServerPacket};
use crate::wire::{
    capacity_hint, packed_to_vector3d, read_f32_le, read_i32_le, read_packed_guid, read_u32_le,
    read_u64_le, read_u8, Vector3d,
};

const MONSTER_MOVE_STOP: u8 = 0x1;
/// `SplineFlags::Flying`, also `Mask_CatmullRom` (tested at `0x6018f0`). Set, the unit keeps the
/// path's own Z along a curve; clear, the 1.12 client drops the spline Z (`0x616cb0`) and takes Z
/// from the terrain (`0x634040`), moving along straight segments.
const SPLINE_FLAG_FLYING: u32 = 0x200;
/// `SPLINEFLAG_RUNMODE`: run speed. The 1.12 client passes it to `CMovement::SetRunMode`
/// (`0x7c71c0`, from `0x7c6ac2`), so a spline without it sets `MOVEFLAG_WALK_MODE` on the unit.
const SPLINE_FLAG_RUNMODE: u32 = 0x100;

/// A stop (`moveType` 1) ends after the head: the 1.12 client reads no spline block and implies
/// `flags = 0x100, count = 1, duration = 0`. Otherwise the path runs `[start, …, endpoint]`.
pub(super) fn read_monster_move(r: &mut &[u8], on_transport: bool) -> io::Result<ServerPacket> {
    let guid = read_packed_guid(r)?;
    let transport = on_transport.then(|| read_packed_guid(r)).transpose()?;
    let start = Vector3d::read(r)?;
    // Echoed in `CMSG_MOVE_SPLINE_DONE` when the spline moves our own player (charge, knockback,
    // taxi); the server checks it against its newest spline id.
    let spline_id = read_u32_le(r)?;
    let move_type = read_u8(r)?;
    // Final facing (jumptable `0x602114`); the 1.12 client snaps the unit to it (`0x7c6f30`).
    let facing = match move_type {
        2 => {
            let spot = Vector3d::read(r)?;
            MonsterMoveFacing::Spot([spot.x, spot.y, spot.z])
        }
        3 => MonsterMoveFacing::Target(read_u64_le(r)?),
        4 => MonsterMoveFacing::Angle(read_f32_le(r)?),
        _ => MonsterMoveFacing::None,
    };
    Ok(if move_type == MONSTER_MOVE_STOP {
        ServerPacket::MonsterMove {
            guid,
            transport,
            start,
            spline_id,
            path: Vec::new(),
            facing,
            stop: true,
            duration_ms: 0,
            flying: false,
            // A stop carries no flags and builds no path; nothing reads this.
            run_mode: true,
        }
    } else {
        let spline_flags = read_u32_le(r)?;
        let duration_ms = read_u32_le(r)?;
        let flying = spline_flags & SPLINE_FLAG_FLYING != 0;
        let run_mode = spline_flags & SPLINE_FLAG_RUNMODE != 0;
        // Both layouts ship only the points after the start, so the head's `start` leads the path.
        let tail = read_monster_move_spline(r, flying)?;
        let path = if tail.is_empty() {
            Vec::new()
        } else {
            std::iter::once(start).chain(tail).collect()
        };
        ServerPacket::MonsterMove {
            guid,
            transport,
            start,
            spline_id,
            path,
            facing,
            stop: false,
            duration_ms,
            flying,
            run_mode,
        }
    })
}

/// The absolute waypoints after the start, endpoint last (vmangos `PacketBuilder`). Flying
/// (`WriteCatmullRomPath`): a `u32` count, then that many absolute points. Ground
/// (`WriteLinearPath`): a `u32` count, the absolute endpoint, then `count - 1` packed
/// `endpoint - waypoint` offsets. The start is not among them: vmangos writes from
/// `firstPoint = 1` (`MoveSplineInit.cpp:169`), and the reference decoder (`0x6018f0`) reads
/// the same `count - 1`.
fn read_monster_move_spline(r: &mut &[u8], catmull_rom: bool) -> io::Result<Vec<Vector3d>> {
    let count = read_u32_le(r)?;
    // 0xFFFF bounds only the pre-allocation against a corrupt `count`.
    if catmull_rom {
        let mut points = Vec::with_capacity(capacity_hint(count, 0xFFFF));
        for _ in 0..count {
            points.push(Vector3d::read(r)?);
        }
        return Ok(points);
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let endpoint = Vector3d::read(r)?;
    let mut points = Vec::with_capacity(capacity_hint(count, 0xFFFF));
    // `count == 1` is a straight hop: no offsets (vmangos: `if (last_idx > 1)`).
    for _ in 1..count {
        let off = packed_to_vector3d(read_i32_le(r)?);
        points.push(Vector3d {
            x: endpoint.x - off.x,
            y: endpoint.y - off.y,
            z: endpoint.z - off.z,
        });
    }
    points.push(endpoint);
    Ok(points)
}

/// 3.3.5a's `MoveSplineFlag` bits (AzerothCore `MoveSplineFlag.h`).
mod wotlk_spline {
    pub const PARABOLIC: u32 = 0x800;
    pub const WALKMODE: u32 = 0x1000;
    pub const FLYING: u32 = 0x2000;
    pub const CATMULLROM: u32 = 0x4_0000;
    pub const CYCLIC: u32 = 0x8_0000;
    pub const ANIMATION: u32 = 0x20_0000;
}

/// 3.3.5a's monster move (AzerothCore `MoveSplineInit::Launch`, `PacketBuilder::WriteMonsterMove`):
/// the mover; for `_TRANSPORT` the transport's packed guid and the `i8` seat; a `u8` 1.12.1 does
/// not have; then 1.12.1's start, spline id and facing. The spline block adds an animation id and
/// start time under `ANIMATION` and a vertical acceleration and start time under `PARABOLIC`; its
/// flags are 3.3.5a's, `Flying | Catmullrom` the smooth path and `Walkmode` the walk.
pub(super) fn read_monster_move_wotlk(
    r: &mut &[u8],
    on_transport: bool,
) -> io::Result<ServerPacket> {
    let guid = read_packed_guid(r)?;
    let transport = if on_transport {
        let transport = read_packed_guid(r)?;
        let _seat = read_u8(r)?;
        Some(transport)
    } else {
        None
    };
    let _unk = read_u8(r)?;
    let start = Vector3d::read(r)?;
    let spline_id = read_u32_le(r)?;
    let move_type = read_u8(r)?;
    let facing = match move_type {
        2 => {
            let spot = Vector3d::read(r)?;
            MonsterMoveFacing::Spot([spot.x, spot.y, spot.z])
        }
        3 => MonsterMoveFacing::Target(read_u64_le(r)?),
        4 => MonsterMoveFacing::Angle(read_f32_le(r)?),
        _ => MonsterMoveFacing::None,
    };
    if move_type == MONSTER_MOVE_STOP {
        return Ok(ServerPacket::MonsterMove {
            guid,
            transport,
            start,
            spline_id,
            path: Vec::new(),
            facing,
            stop: true,
            duration_ms: 0,
            flying: false,
            run_mode: true,
        });
    }
    let spline_flags = read_u32_le(r)?;
    if spline_flags & wotlk_spline::ANIMATION != 0 {
        let _animation_id = read_u8(r)?;
        let _effect_start_time = read_u32_le(r)?;
    }
    let duration_ms = read_u32_le(r)?;
    if spline_flags & wotlk_spline::PARABOLIC != 0 {
        let _vertical_acceleration = read_f32_le(r)?;
        let _effect_start_time = read_u32_le(r)?;
    }
    let smooth = spline_flags & (wotlk_spline::FLYING | wotlk_spline::CATMULLROM) != 0;
    let tail = if smooth {
        let mut points = read_monster_move_spline(r, true)?;
        // A cyclic path carries one fake point (`WriteCatmullRomCyclicPath`): the start ahead of a
        // flying path, a zero after a ground one.
        if spline_flags & wotlk_spline::CYCLIC != 0 && !points.is_empty() {
            if spline_flags & wotlk_spline::FLYING != 0 {
                points.remove(0);
            } else {
                points.pop();
            }
        }
        points
    } else {
        read_linear_path_wotlk(r, start)?
    };
    let path = if tail.is_empty() {
        Vec::new()
    } else {
        std::iter::once(start).chain(tail).collect()
    };
    Ok(ServerPacket::MonsterMove {
        guid,
        transport,
        start,
        spline_id,
        path,
        facing,
        stop: false,
        duration_ms,
        flying: smooth,
        run_mode: spline_flags & wotlk_spline::WALKMODE == 0,
    })
}

/// 3.3.5a's `WriteLinearPath`: a `u32` count, the absolute endpoint, then `count - 1` packed
/// offsets, each `middle - waypoint` where `middle` is halfway from the start to the endpoint
/// (1.12.1 packs them from the endpoint).
fn read_linear_path_wotlk(r: &mut &[u8], start: Vector3d) -> io::Result<Vec<Vector3d>> {
    let count = read_u32_le(r)?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let endpoint = Vector3d::read(r)?;
    let middle = Vector3d {
        x: (start.x + endpoint.x) / 2.0,
        y: (start.y + endpoint.y) / 2.0,
        z: (start.z + endpoint.z) / 2.0,
    };
    let mut points = Vec::with_capacity(capacity_hint(count, 0xFFFF));
    for _ in 1..count {
        let off = packed_to_vector3d(read_i32_le(r)?);
        points.push(Vector3d {
            x: middle.x - off.x,
            y: middle.y - off.y,
            z: middle.z - off.z,
        });
    }
    points.push(endpoint);
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, parse_server_with_tail};
    use crate::wire::write_packed_guid;

    /// The fixed head: packed guid, start, splineId, moveType.
    fn head(guid: u64, start: [f32; 3], move_type: u8) -> Vec<u8> {
        let mut b = Vec::new();
        write_packed_guid(guid, &mut b).unwrap();
        for f in start {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(&7u32.to_le_bytes()); // splineId
        b.push(move_type);
        b
    }

    /// vmangos `WriteLinearPath` from `firstPoint = 1` over `path = [c₀, …, cₙ₋₁]`: the count
    /// `n - 1`, the endpoint, then `c₁ … cₙ₋₂` as packed `endpoint - point` offsets.
    fn append_linear_path(body: &mut Vec<u8>, path: &[[f32; 3]]) {
        let (&endpoint, leading) = path.split_last().expect("a path has an endpoint");
        let last_idx = path.len() - 1;
        body.extend_from_slice(&(last_idx as u32).to_le_bytes()); // last_idx − start + 1
        for f in endpoint {
            body.extend_from_slice(&f.to_le_bytes());
        }
        if last_idx <= 1 {
            return; // vmangos `packet_builder.cpp:92`: `if (last_idx > 1)`
        }
        // `for (i = start; i < last_idx; ++i)` with `start = 1`, packed in ¼-yd units.
        for &p in &leading[1..] {
            let pack = |v: f32, shift: u32, mask: i32| ((v * 4.0).round() as i32 & mask) << shift;
            let off = [endpoint[0] - p[0], endpoint[1] - p[1], endpoint[2] - p[2]];
            let packed = pack(off[0], 0, 0x7FF) | pack(off[1], 11, 0x7FF) | pack(off[2], 22, 0x3FF);
            body.extend_from_slice(&packed.to_le_bytes());
        }
    }

    /// Each opcode reads its own layout; the plain one must not find a transport guid.
    #[test]
    fn monster_move_transport_reads_the_deck_guid_first() {
        let mut body = Vec::new();
        write_packed_guid(0x55, &mut body).unwrap(); // the mover
        write_packed_guid(0x2000_0000_0000_0007, &mut body).unwrap(); // the transport
        for f in [1.5f32, -2.5, 0.75] {
            body.extend_from_slice(&f.to_le_bytes()); // deck-local start
        }
        body.extend_from_slice(&7u32.to_le_bytes()); // splineId
        body.push(0); // moveType: a plain move
        body.extend_from_slice(&0u32.to_le_bytes()); // spline flags: ground, walk
        body.extend_from_slice(&2_000u32.to_le_bytes()); // duration
        append_linear_path(&mut body, &[[1.5, -2.5, 0.75], [4.0, -2.5, 0.75]]);

        match parse_server(opcode::SMSG_MONSTER_MOVE_TRANSPORT, &body).expect("parses") {
            ServerPacket::MonsterMove {
                guid,
                transport,
                start,
                path,
                ..
            } => {
                assert_eq!(guid, 0x55);
                assert_eq!(transport, Some(0x2000_0000_0000_0007));
                assert_eq!((start.x, start.y, start.z), (1.5, -2.5, 0.75));
                assert_eq!(path.len(), 2, "start + endpoint, both deck-local");
                assert_eq!((path[1].x, path[1].y, path[1].z), (4.0, -2.5, 0.75));
            }
            _ => panic!("expected MonsterMove"),
        }

        // The plain opcode over the same bytes must not come back with a transport.
        match parse_server(opcode::SMSG_MONSTER_MOVE, &body) {
            Ok(ServerPacket::MonsterMove { transport, .. }) => assert_eq!(transport, None),
            Ok(_) => panic!("expected MonsterMove"),
            Err(_) => {} // an under-run is fine too: it is not the same read
        }
    }

    #[test]
    fn monster_move_stop_has_no_tail() {
        let body = head(0x1234, [1.0, 2.0, 3.0], MONSTER_MOVE_STOP);
        let p = parse_server(opcode::SMSG_MONSTER_MOVE, &body).expect("a stop parses head-only");
        match p {
            ServerPacket::MonsterMove { stop, path, .. } => {
                assert!(stop, "moveType 1 is a stop");
                assert!(path.is_empty(), "a stop carries no path");
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    #[test]
    fn monster_move_facing_angle_is_captured() {
        // moveType 4 carries a facing angle.
        let mut body = head(0x55, [0.0, 0.0, 0.0], 4);
        body.extend_from_slice(&1.25f32.to_le_bytes()); // facing angle
        body.extend_from_slice(&0u32.to_le_bytes()); // spline flags (ground)
        body.extend_from_slice(&500u32.to_le_bytes()); // duration
        body.extend_from_slice(&1u32.to_le_bytes()); // count = 1 → endpoint only, no packed
        for f in [10.0f32, 0.0, 0.0] {
            body.extend_from_slice(&f.to_le_bytes()); // the single (absolute) endpoint
        }
        let p = parse_server(opcode::SMSG_MONSTER_MOVE, &body).expect("a facing move parses");
        match p {
            ServerPacket::MonsterMove {
                facing,
                path,
                duration_ms,
                stop,
                ..
            } => {
                assert!(!stop);
                assert_eq!(duration_ms, 500);
                assert_eq!(path.len(), 2, "start + endpoint");
                assert!((path[0].x - 0.0).abs() < 1e-6, "anchored at the wire start");
                assert!((path[1].x - 10.0).abs() < 1e-6, "endpoint verbatim");
                match facing {
                    MonsterMoveFacing::Angle(a) => assert!((a - 1.25).abs() < 1e-6),
                    other => panic!("expected an Angle facing, got {other:?}"),
                }
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    #[test]
    fn monster_move_two_point_path_carries_no_offsets() {
        let mut body = head(0x77, [4.0, 8.0, 0.0], 0);
        body.extend_from_slice(&0u32.to_le_bytes()); // spline flags: ground
        body.extend_from_slice(&1_000u32.to_le_bytes()); // duration
        append_linear_path(&mut body, &[[4.0, 8.0, 0.0], [12.0, 8.0, 0.0]]);
        let (p, tail) = parse_server_with_tail(opcode::SMSG_MONSTER_MOVE, &body)
            .expect("a two-point hop parses");
        assert_eq!(tail, 0, "the body is exactly the count and the destination");
        match p {
            ServerPacket::MonsterMove { path, .. } => {
                assert_eq!(path.len(), 2, "start + destination, got {path:?}");
                assert!((path[0].x - 4.0).abs() < 1e-6 && (path[1].x - 12.0).abs() < 1e-6);
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    /// Hand-assembled rather than through `append_linear_path`: the route `c₀ → c₁ → c₂` is
    /// `count = 2`, the endpoint `c₂` and one packed offset `c₂ - c₁`.
    #[test]
    fn monster_move_one_corner_path_keeps_its_corner() {
        let mut body = head(0x77, [0.0, 0.0, 0.0], 0); // c₀ in the head
        body.extend_from_slice(&0u32.to_le_bytes()); // spline flags: ground
        body.extend_from_slice(&2_000u32.to_le_bytes()); // duration
        body.extend_from_slice(&2u32.to_le_bytes()); // count = last_idx − start + 1 = 2
        for f in [10.0f32, 10.0, 0.0] {
            body.extend_from_slice(&f.to_le_bytes()); // endpoint c₂
        }
        // c₂ − c₁ = (0, 10, 0): x 0, y 10·4 = 40 at bit 11, z 0 (`appendPackXYZ`'s ¼-yd fields).
        body.extend_from_slice(&(40i32 << 11).to_le_bytes());
        let (p, tail) =
            parse_server_with_tail(opcode::SMSG_MONSTER_MOVE, &body).expect("a corner path parses");
        assert_eq!(tail, 0, "the one offset is read, not left behind");
        match p {
            ServerPacket::MonsterMove { path, .. } => {
                let got: Vec<[f32; 3]> = path.iter().map(|v| [v.x, v.y, v.z]).collect();
                assert_eq!(
                    got,
                    vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]]
                );
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    #[test]
    fn monster_move_ground_path_decodes_every_waypoint() {
        // ¼-yd multiples, so the packed quantization is exact.
        let want = [
            [0.0f32, 0.0, 0.0], // start
            [10.0, 0.0, 0.0],   // corner east
            [10.0, 10.0, 0.0],  // corner north
            [10.0, 10.0, 5.0],  // endpoint, up a step
        ];
        let mut body = head(0xABCD, want[0], 0); // moveType 0 (normal)
        body.extend_from_slice(&0u32.to_le_bytes()); // spline flags (ground/linear)
        body.extend_from_slice(&4000u32.to_le_bytes()); // duration
        append_linear_path(&mut body, &want);
        let p = parse_server(opcode::SMSG_MONSTER_MOVE, &body).expect("a ground path parses");
        match p {
            ServerPacket::MonsterMove {
                path, flying, stop, ..
            } => {
                assert!(!stop && !flying, "a normal ground move");
                assert_eq!(
                    path.len(),
                    4,
                    "all four waypoints survive, not just the endpoint"
                );
                for (got, exp) in path.iter().zip(want.iter()) {
                    assert!(
                        (got.x - exp[0]).abs() < 1e-4
                            && (got.y - exp[1]).abs() < 1e-4
                            && (got.z - exp[2]).abs() < 1e-4,
                        "waypoint {got:?} != {exp:?}"
                    );
                }
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    #[test]
    fn monster_move_flying_path_reads_absolute_points() {
        let mids = [[3.0f32, 4.0, 50.0], [6.0, 8.0, 55.0]];
        let mut body = head(0x77, [0.0, 0.0, 40.0], 0);
        body.extend_from_slice(&SPLINE_FLAG_FLYING.to_le_bytes()); // flying ⇒ catmull-rom layout
        body.extend_from_slice(&3000u32.to_le_bytes()); // duration
        body.extend_from_slice(&(mids.len() as u32).to_le_bytes()); // count = absolute points
        for pt in mids {
            for f in pt {
                body.extend_from_slice(&f.to_le_bytes());
            }
        }
        let p = parse_server(opcode::SMSG_MONSTER_MOVE, &body).expect("a flying path parses");
        match p {
            ServerPacket::MonsterMove { path, flying, .. } => {
                assert!(flying, "the FLYING flag drives the catmull-rom layout");
                assert_eq!(path.len(), 3, "start + two absolute waypoints");
                assert!(
                    (path[0].z - 40.0).abs() < 1e-6,
                    "anchored at the wire start (Z kept — flying)"
                );
                assert!((path[2].x - 6.0).abs() < 1e-6 && (path[2].z - 55.0).abs() < 1e-6);
            }
            _ => panic!("expected MonsterMove"),
        }
    }

    /// 3.3.5a's head (`MoveSplineInit::Launch`, `WriteCommonMonsterMovePart`): the mover, the
    /// transport and seat when given, the `u8`, start, spline id, moveType.
    fn head_wotlk(guid: u64, transport: Option<u64>, start: [f32; 3], move_type: u8) -> Vec<u8> {
        let mut b = Vec::new();
        write_packed_guid(guid, &mut b).unwrap();
        if let Some(t) = transport {
            write_packed_guid(t, &mut b).unwrap();
            b.push(0xff); // seat −1
        }
        b.push(0);
        for f in start {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(&9u32.to_le_bytes());
        b.push(move_type);
        b
    }

    fn parse_wotlk(op: u16, body: &[u8]) -> ServerPacket {
        let (p, tail) = crate::messages::parse_server_with_tail_as(
            benilla_expansions::Expansion::Wotlk,
            op,
            body,
        )
        .expect("parses");
        assert_eq!(tail, 0, "bytes left unread");
        p
    }

    #[test]
    fn a_3_3_5a_ground_path_unpacks_its_waypoints_from_the_midpoint() {
        // Start (0,0,0) → (4,2,0) → (6,6,0) → endpoint (10,8,0); middle (5,4,0).
        let mut b = head_wotlk(0x55, None, [0.0, 0.0, 0.0], 0);
        b.extend_from_slice(&0u32.to_le_bytes()); // flags: ground, run
        b.extend_from_slice(&3_000u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes()); // last_idx
        for f in [10.0f32, 8.0, 0.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        for p in [[4.0f32, 2.0, 0.0], [6.0, 6.0, 0.0]] {
            // `appendPackXYZ(middle - p)`.
            let (x, y, z) = (5.0 - p[0], 4.0 - p[1], 0.0 - p[2]);
            let packed = ((x / 0.25) as i32 & 0x7FF)
                | (((y / 0.25) as i32 & 0x7FF) << 11)
                | (((z / 0.25) as i32 & 0x3FF) << 22);
            b.extend_from_slice(&packed.to_le_bytes());
        }
        let ServerPacket::MonsterMove {
            path,
            duration_ms,
            flying,
            run_mode,
            transport,
            ..
        } = parse_wotlk(opcode::SMSG_MONSTER_MOVE, &b)
        else {
            panic!("expected MonsterMove")
        };
        let path: Vec<[f32; 3]> = path.iter().map(|v| [v.x, v.y, v.z]).collect();
        assert_eq!(
            path,
            [
                [0.0, 0.0, 0.0],
                [4.0, 2.0, 0.0],
                [6.0, 6.0, 0.0],
                [10.0, 8.0, 0.0]
            ]
        );
        assert_eq!(duration_ms, 3_000);
        assert!(!flying && run_mode && transport.is_none());
    }

    #[test]
    fn a_3_3_5a_flying_walk_on_a_transport_reads_seat_animation_and_parabola() {
        let mut b = head_wotlk(0x55, Some(0x1FC0_0000_0000_0005), [1.0, 2.0, 3.0], 4);
        b.extend_from_slice(&1.5f32.to_le_bytes()); // facing angle
        let flags: u32 = 0x2000 | 0x1000 | 0x20_0000 | 0x800; // flying, walk, animation, parabolic
        b.extend_from_slice(&flags.to_le_bytes());
        b.push(3); // animation id
        b.extend_from_slice(&100u32.to_le_bytes()); // effect start
        b.extend_from_slice(&1_000u32.to_le_bytes()); // duration
        b.extend_from_slice(&19.29f32.to_le_bytes()); // vertical acceleration
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        for f in [5.0f32, 5.0, 5.0, 9.0, 9.0, 9.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        let ServerPacket::MonsterMove {
            transport,
            path,
            facing,
            flying,
            run_mode,
            ..
        } = parse_wotlk(opcode::SMSG_MONSTER_MOVE_TRANSPORT, &b)
        else {
            panic!("expected MonsterMove")
        };
        assert_eq!(transport, Some(0x1FC0_0000_0000_0005));
        assert_eq!(path.len(), 3);
        assert_eq!((path[2].x, path[2].y, path[2].z), (9.0, 9.0, 9.0));
        assert!(matches!(facing, MonsterMoveFacing::Angle(a) if a == 1.5));
        assert!(flying && !run_mode);
    }

    #[test]
    fn a_3_3_5a_cyclic_flying_path_drops_its_fake_lead_point() {
        let mut b = head_wotlk(0x55, None, [0.0, 0.0, 10.0], 0);
        b.extend_from_slice(&(0x2000u32 | 0x8_0000 | 0x10_0000).to_le_bytes());
        b.extend_from_slice(&5_000u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes()); // count + 1
        for f in [0.0f32, 0.0, 10.0, 10.0, 0.0, 10.0, 10.0, 10.0, 10.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        let ServerPacket::MonsterMove { path, .. } = parse_wotlk(opcode::SMSG_MONSTER_MOVE, &b)
        else {
            panic!("expected MonsterMove")
        };
        let path: Vec<[f32; 3]> = path.iter().map(|v| [v.x, v.y, v.z]).collect();
        assert_eq!(
            path,
            [[0.0, 0.0, 10.0], [10.0, 0.0, 10.0], [10.0, 10.0, 10.0]]
        );
    }

    #[test]
    fn a_3_3_5a_stop_ends_after_its_head() {
        let b = head_wotlk(0x55, None, [1.0, 2.0, 3.0], MONSTER_MOVE_STOP);
        assert!(matches!(
            parse_wotlk(opcode::SMSG_MONSTER_MOVE, &b),
            ServerPacket::MonsterMove { stop: true, .. }
        ));
    }
}
