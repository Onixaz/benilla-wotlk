//! `SMSG_MONSTER_MOVE`: a mover, its start, a spline id, a `moveType` facing, then (unless a stop)
//! spline flags, a duration and the waypoints. `SMSG_MONSTER_MOVE_TRANSPORT` inserts the
//! transport's packed guid after the mover's, and every coordinate is then deck-local, waypoints
//! included (vmangos `MoveSplineInit.cpp:138-170`, `PathFinder.cpp:76-84`).

use std::io;

use crate::messages::{MonsterMoveFacing, ServerPacket, SplineArc};
use crate::wire::{
    capacity_hint, packed_to_vector3d, read_f32_le, read_i32_le, read_packed_guid, read_u32_le,
    read_u64_le, read_u8, Vector3d,
};

const MONSTER_MOVE_STOP: u8 = 0x1;

/// The absolute waypoints after the start, endpoint last (vmangos `PacketBuilder`). Flying
/// (`WriteCatmullRomPath`): a `u32` count, then that many absolute points. Ground
/// (`WriteLinearPath`): a `u32` count, the absolute endpoint, then `count - 1` packed
/// `endpoint - waypoint` offsets. The start is not among them: vmangos writes from `firstPoint = 1`
/// (`MoveSplineInit.cpp:169`), and the reference decoder reads the same `count - 1`.
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
    pub const ORIENTATION_FIXED: u32 = 0x4000;
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
    let (transport, seat) = if on_transport {
        (Some(read_packed_guid(r)?), read_u8(r)? as i8)
    } else {
        (None, -1)
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
            seat,
            start,
            spline_id,
            path: Vec::new(),
            facing,
            stop: true,
            duration_ms: 0,
            flying: false,
            run_mode: true,
            arc: None,
            facing_fixed: false,
        });
    }
    let spline_flags = read_u32_le(r)?;
    if spline_flags & wotlk_spline::ANIMATION != 0 {
        let _animation_id = read_u8(r)?;
        let _effect_start_time = read_u32_le(r)?;
    }
    let duration_ms = read_u32_le(r)?;
    let arc = if spline_flags & wotlk_spline::PARABOLIC != 0 {
        Some(SplineArc {
            vertical_acceleration: read_f32_le(r)?,
            effect_start_ms: read_u32_le(r)?,
        })
    } else {
        None
    };
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
        seat,
        start,
        spline_id,
        path,
        facing,
        stop: false,
        duration_ms,
        flying: smooth,
        run_mode: spline_flags & wotlk_spline::WALKMODE == 0,
        arc,
        facing_fixed: spline_flags & wotlk_spline::ORIENTATION_FIXED != 0,
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
    use crate::messages::{opcode, parse_server_with_tail};
    use crate::wire::write_packed_guid;

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
        let (p, tail) = parse_server_with_tail(op, body).expect("parses");
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
            arc,
            facing_fixed,
            ..
        } = parse_wotlk(opcode::SMSG_MONSTER_MOVE_TRANSPORT, &b)
        else {
            panic!("expected MonsterMove")
        };
        assert_eq!(transport, Some(0x1FC0_0000_0000_0005));
        assert_eq!(path.len(), 3);
        assert_eq!((path[2].x, path[2].y, path[2].z), (9.0, 9.0, 9.0));
        assert!(matches!(facing, MonsterMoveFacing::Angle(a) if a == 1.5));
        assert!(flying && !run_mode && !facing_fixed);
        assert_eq!(
            arc,
            Some(SplineArc {
                vertical_acceleration: 19.29,
                effect_start_ms: 0
            })
        );
    }

    /// A creature knockback (`MotionMaster::MoveKnockbackFrom`): a ground hop with `Parabolic` and
    /// `OrientationFixed`.
    #[test]
    fn a_3_3_5a_knockback_reads_its_arc_and_fixed_facing() {
        let mut b = head_wotlk(0x55, None, [0.0, 0.0, 0.0], 0);
        b.extend_from_slice(&(0x800u32 | 0x4000).to_le_bytes());
        b.extend_from_slice(&900u32.to_le_bytes()); // duration
        b.extend_from_slice(&24.5f32.to_le_bytes()); // vertical acceleration
        b.extend_from_slice(&0u32.to_le_bytes()); // effect start
        b.extend_from_slice(&1u32.to_le_bytes());
        for f in [-12.0f32, 0.0, 0.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        let ServerPacket::MonsterMove {
            path,
            flying,
            arc,
            facing_fixed,
            ..
        } = parse_wotlk(opcode::SMSG_MONSTER_MOVE, &b)
        else {
            panic!("expected MonsterMove")
        };
        assert_eq!(path.len(), 2);
        assert!(!flying && facing_fixed);
        assert_eq!(
            arc,
            Some(SplineArc {
                vertical_acceleration: 24.5,
                effect_start_ms: 0
            })
        );
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
