//! 3.3.5a's `SMSG_UPDATE_OBJECT` body (AzerothCore `UpdateData::BuildPacket`,
//! `Object::BuildMovementUpdate`), read into the same [`Object`] list the 1.12.1 decoder gives.
//! The update types and the field mask are 1.12.1's; the block count has no has-transport byte
//! after it, the movement block is its own, and the fields are 3.3.5a's ([`ObjectFields::from_wotlk`]).

use std::io::{self, Read};

use crate::messages::movement::read_movement_info_wotlk;
use crate::wire::{
    capacity_hint, read_f32_le, read_packed_guid, read_u16_le, read_u32_le, read_u64_le, read_u8,
    Vector3d,
};

use super::fields::ObjectFields;
use super::movement::{CreateSpline, MovementBlock, MoverState, ObjectType};
use super::Object;
use crate::messages::movement::TransportPose;
use crate::messages::{MonsterMoveFacing, SplineArc};

const UPDATE_FLAG_TRANSPORT: u16 = 0x0002;
const UPDATE_FLAG_HAS_TARGET: u16 = 0x0004;
const UPDATE_FLAG_UNKNOWN: u16 = 0x0008;
const UPDATE_FLAG_LOWGUID: u16 = 0x0010;
const UPDATE_FLAG_LIVING: u16 = 0x0020;
const UPDATE_FLAG_STATIONARY_POSITION: u16 = 0x0040;
const UPDATE_FLAG_VEHICLE: u16 = 0x0080;
const UPDATE_FLAG_POSITION: u16 = 0x0100;
const UPDATE_FLAG_ROTATION: u16 = 0x0200;

/// 3.3.5a's `MoveSplineFlag` bits (AzerothCore `MoveSplineFlag.h`).
const SPLINE_FLAG_PARABOLIC: u32 = 0x800;
const SPLINE_FLAG_WALKMODE: u32 = 0x1000;
const SPLINE_FLAG_FLYING: u32 = 0x2000;
const SPLINE_FLAG_ORIENTATION_FIXED: u32 = 0x4000;
const SPLINE_FLAG_FINAL_POINT: u32 = 0x8000;
const SPLINE_FLAG_FINAL_TARGET: u32 = 0x1_0000;
const SPLINE_FLAG_FINAL_ANGLE: u32 = 0x2_0000;
const SPLINE_FLAG_CATMULLROM: u32 = 0x4_0000;
const SPLINE_FLAG_CYCLIC: u32 = 0x8_0000;
const SPLINE_FLAG_ENTER_CYCLE: u32 = 0x10_0000;

/// Parse a 3.3.5a `SMSG_UPDATE_OBJECT` body: the count, then each `Object`.
pub(super) fn read_update_object(r: &mut impl Read) -> io::Result<Vec<Object>> {
    let amount_of_objects = read_u32_le(r)?;
    let mut objects = Vec::with_capacity(capacity_hint(amount_of_objects, 0xFFFF));
    for _ in 0..amount_of_objects {
        objects.push(read_object(r)?);
    }
    Ok(objects)
}

fn read_object(r: &mut impl Read) -> io::Result<Object> {
    let update_type = read_u8(r)?;
    Ok(match update_type {
        0 => {
            let guid = read_packed_guid(r)?;
            let raw = ObjectFields::read(r)?;
            Object::Values {
                guid,
                mask: ObjectFields::from_wotlk(raw, object_type_of_guid(guid)?),
            }
        }
        // No update flags: the mover body alone (`0x4d6db1`, `0x4d6dd7`).
        1 => Object::Movement {
            guid: read_packed_guid(r)?,
            movement: read_mover_update(r)?,
        },
        2 | 3 => {
            let guid = read_packed_guid(r)?;
            let object_type = ObjectType::from_u8(read_u8(r)?);
            let movement = read_movement_block(r)?;
            let mut mask = ObjectFields::from_wotlk(ObjectFields::read(r)?, object_type);
            mask.place_wotlk_movement_fields(
                object_type,
                movement.block.position,
                movement.rotation,
            );
            Object::Create {
                guid,
                object_type,
                movement: movement.block,
                // As 1.12.1's: a create omits zero fields, so absent reads 0 in its descriptor.
                mask: mask.into_created(object_type),
                spawned: update_type == 3,
            }
        }
        4 => Object::OutOfRange {
            guids: read_guid_list(r)?,
        },
        5 => Object::Near {
            guids: read_guid_list(r)?,
        },
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown object update_type {other}"),
            ))
        }
    })
}

fn read_guid_list(r: &mut impl Read) -> io::Result<Vec<u64>> {
    let count = read_u32_le(r)?;
    let mut guids = Vec::with_capacity(capacity_hint(count, 0xFFFF));
    for _ in 0..count {
        guids.push(read_packed_guid(r)?);
    }
    Ok(guids)
}

/// The object type a 3.3.5a guid's high half names (AzerothCore `HighGuid`), which a values
/// delta needs to read its fields: it carries no type byte. An item guid names the container
/// layout, whose item block is the item's.
fn object_type_of_guid(guid: u64) -> io::Result<ObjectType> {
    Ok(match (guid >> 48) as u16 {
        0x0000 => ObjectType::Player,
        0x4000 => ObjectType::Container,
        // GameObject, Transport, MO transport.
        0xF110 | 0xF120 | 0x1FC0 => ObjectType::GameObject,
        // Unit, Pet, Vehicle.
        0xF130 | 0xF140 | 0xF150 => ObjectType::Unit,
        0xF100 => ObjectType::DynamicObject,
        0xF101 => ObjectType::Corpse,
        high => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("values update for guid {guid:#018x}: no object type has high {high:#06x}"),
            ))
        }
    })
}

/// A movement block and what 3.3.5a moved from the fields into it.
struct WotlkMovement {
    block: MovementBlock,
    /// A gameobject's own rotation (`UPDATEFLAG_ROTATION`).
    rotation: Option<[f32; 4]>,
}

/// `Object::BuildMovementUpdate`: `u16` flags; a `LIVING` mover's `MovementInfo`, nine speeds and
/// its spline, or a `POSITION` (packed transport guid, world position, local or repeated
/// position, facing, transport facing), or a `STATIONARY_POSITION`; then the flag-gated tail.
fn read_movement_block(r: &mut impl Read) -> io::Result<WotlkMovement> {
    let update_flag = read_u16_le(r)?;
    let mut block = MovementBlock {
        position: None,
        mover: None,
        speeds: None,
        transport_progress: None,
        transport: None,
        vehicle: None,
        spline: None,
        attack_target: None,
    };
    let mut rotation = None;

    if update_flag & UPDATE_FLAG_LIVING != 0 {
        block = read_mover_update(r)?;
    } else if update_flag & UPDATE_FLAG_POSITION != 0 {
        let transport = read_packed_guid(r)?;
        let pos = Vector3d::read(r)?;
        // The transport offset, or the position again off a transport.
        let local = Vector3d::read(r)?;
        let orientation = read_f32_le(r)?;
        let transport_orientation = read_f32_le(r)?;
        block.position = Some((pos, orientation));
        block.transport = (transport != 0).then_some(TransportPose {
            guid: transport,
            pos: local,
            orientation: transport_orientation,
            seat: -1,
        });
    } else if update_flag & UPDATE_FLAG_STATIONARY_POSITION != 0 {
        let pos = Vector3d::read(r)?;
        block.position = Some((pos, read_f32_le(r)?));
    }

    if update_flag & UPDATE_FLAG_UNKNOWN != 0 {
        let _ = read_u32_le(r)?;
    }
    if update_flag & UPDATE_FLAG_LOWGUID != 0 {
        let _ = read_u32_le(r)?;
    }
    if update_flag & UPDATE_FLAG_HAS_TARGET != 0 {
        block.attack_target = Some(read_packed_guid(r)?).filter(|&g| g != 0);
    }
    if update_flag & UPDATE_FLAG_TRANSPORT != 0 {
        block.transport_progress = Some(read_u32_le(r)?);
    }
    if update_flag & UPDATE_FLAG_VEHICLE != 0 {
        block.vehicle = Some((read_u32_le(r)?, read_f32_le(r)?));
    }
    if update_flag & UPDATE_FLAG_ROTATION != 0 {
        rotation = Some(unpack_rotation(read_u64_le(r)? as i64));
    }

    Ok(WotlkMovement { block, rotation })
}

/// The mover body (`0x4f5090`), a `LIVING` create's and the whole of a `MOVEMENT` block: its
/// status, nine speeds, and the spline under `MOVEFLAG_SPLINE_ENABLED` (`0x4f510b`).
fn read_mover_update(r: &mut impl Read) -> io::Result<MovementBlock> {
    let info = read_movement_info_wotlk(r)?;
    // Walk, run, run back, swim, swim back, flight, flight back, turn rate, pitch rate.
    let mut speeds = [0.0f32; 9];
    for slot in &mut speeds {
        *slot = read_f32_le(r)?;
    }
    let spline = if info.flags & crate::messages::movement::MOVEMENT_FLAG_SPLINE_ENABLED != 0 {
        read_create_spline(r)?
    } else {
        None
    };
    Ok(MovementBlock {
        position: Some((info.position, info.orientation)),
        mover: Some(MoverState {
            flags: info.flags,
            pitch: info.pitch,
            flags2: info.flags2,
        }),
        speeds: Some(speeds),
        transport_progress: None,
        transport: info.transport,
        vehicle: None,
        spline,
        attack_target: None,
    })
}

/// `PacketBuilder::WriteCreate`: flags, the final facing, time passed, duration, id, two duration
/// mods, vertical acceleration, effect start time, the nodes, the `u8` evaluation mode and the
/// final destination. Every mode is initialised as Catmull-Rom (`Spline.cpp`, `initializers`), so
/// the nodes carry a virtual point at each end, as in 1.12.1.
fn read_create_spline(r: &mut impl Read) -> io::Result<Option<CreateSpline>> {
    let spline_flags = read_u32_le(r)?;
    let final_facing = if spline_flags & SPLINE_FLAG_FINAL_ANGLE != 0 {
        MonsterMoveFacing::Angle(read_f32_le(r)?)
    } else if spline_flags & SPLINE_FLAG_FINAL_TARGET != 0 {
        MonsterMoveFacing::Target(read_u64_le(r)?)
    } else if spline_flags & SPLINE_FLAG_FINAL_POINT != 0 {
        let v = Vector3d::read(r)?;
        MonsterMoveFacing::Spot([v.x, v.y, v.z])
    } else {
        MonsterMoveFacing::None
    };
    let time_passed_ms = read_u32_le(r)?;
    let duration_ms = read_u32_le(r)?;
    let id = read_u32_le(r)?;
    let duration_mod = read_f32_le(r)?;
    let duration_mod_next = read_f32_le(r)?;
    let vertical_acceleration = read_f32_le(r)?;
    let effect_start_ms = read_u32_le(r)?;
    let amount_of_nodes = read_u32_le(r)?;
    let mut nodes = Vec::with_capacity(capacity_hint(amount_of_nodes, 0xFFFF));
    for _ in 0..amount_of_nodes {
        let v = Vector3d::read(r)?;
        nodes.push([v.x, v.y, v.z]);
    }
    // Dead on arrival: the apply rewrites the mode from the flags (`0x6f15c3`).
    let _mode = read_u8(r)?;
    let _final_node = Vector3d::read(r)?;
    Ok((nodes.len() >= 4).then(|| CreateSpline {
        path: nodes[1..nodes.len() - 1].to_vec(),
        id,
        time_passed_ms,
        duration_ms,
        flying: spline_flags & SPLINE_FLAG_FLYING != 0,
        cyclic: spline_flags & SPLINE_FLAG_CYCLIC != 0,
        enter_cycle: spline_flags & SPLINE_FLAG_ENTER_CYCLE != 0,
        duration_mods: [duration_mod, duration_mod_next],
        phantoms: [nodes[0], nodes[nodes.len() - 1]],
        run_mode: spline_flags & SPLINE_FLAG_WALKMODE == 0,
        arc: (spline_flags & SPLINE_FLAG_PARABOLIC != 0).then_some(SplineArc {
            vertical_acceleration,
            effect_start_ms,
        }),
        facing_fixed: spline_flags & SPLINE_FLAG_ORIENTATION_FIXED != 0,
        final_facing,
        catmull_rom: spline_flags & (SPLINE_FLAG_CATMULLROM | SPLINE_FLAG_FLYING) != 0,
    }))
}

/// `GameObject::UpdatePackedRotation`'s inverse: `x` in the top 22 bits over `2^21`, `y` and `z`
/// in 21 bits each over `2^20`, all signed and negated with `w`, so `w` is the non-negative root.
fn unpack_rotation(packed: i64) -> [f32; 4] {
    let x = (packed >> 42) as f32 / (1 << 21) as f32;
    let y = ((packed << 22) >> 43) as f32 / (1 << 20) as f32;
    let z = ((packed << 43) >> 43) as f32 / (1 << 20) as f32;
    let w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
    [x, y, z, w]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GameObject::UpdatePackedRotation`, as AzerothCore writes it.
    fn pack_rotation(q: [f32; 4]) -> i64 {
        const PACK_YZ: i32 = 1 << 20;
        const PACK_X: i32 = PACK_YZ << 1;
        const PACK_YZ_MASK: i64 = ((PACK_YZ as i64) << 1) - 1;
        const PACK_X_MASK: i64 = ((PACK_X as i64) << 1) - 1;
        let w_sign = if q[3] >= 0.0 { 1 } else { -1 };
        let x = i64::from((q[0] * PACK_X as f32) as i32 * w_sign) & PACK_X_MASK;
        let y = i64::from((q[1] * PACK_YZ as f32) as i32 * w_sign) & PACK_YZ_MASK;
        let z = i64::from((q[2] * PACK_YZ as f32) as i32 * w_sign) & PACK_YZ_MASK;
        z | (y << 21) | (x << 42)
    }

    #[test]
    fn a_packed_rotation_unpacks_to_its_quaternion() {
        let half = std::f32::consts::FRAC_PI_4; // a quarter turn about z
        for q in [
            [0.0, 0.0, half.sin(), half.cos()],
            [0.0, 0.0, -0.5, 0.866_025_4],
            [0.1, -0.2, 0.3, (1.0f32 - 0.14).sqrt()],
            [0.0, 0.0, 0.0, 1.0],
        ] {
            let got = unpack_rotation(pack_rotation(q));
            for (a, b) in got.iter().zip(q) {
                assert!((a - b).abs() < 1e-5, "{got:?} vs {q:?}");
            }
        }
        // A negative `w` folds into the vector part: the same rotation.
        let got = unpack_rotation(pack_rotation([0.0, 0.0, 0.6, -0.8]));
        assert!((got[2] + 0.6).abs() < 1e-5 && (got[3] - 0.8).abs() < 1e-5);
    }

    const UNIT: u64 = 0xF130_0000_0000_0001;
    const GO: u64 = 0xF110_0000_0000_0002;
    const SHIP: u64 = 0x1FC0_0000_0000_0003;
    const VICTIM: u64 = 0x0000_0000_0000_0042;

    fn put_f32s(b: &mut Vec<u8>, fs: &[f32]) {
        for f in fs {
            b.extend_from_slice(&f.to_le_bytes());
        }
    }

    fn put_guid(b: &mut Vec<u8>, g: u64) {
        crate::wire::write_packed_guid(g, b).unwrap();
    }

    /// A 3.3.5a mover body: status with `flags` at `pos`, no fall, then nine speeds.
    fn mover_body(b: &mut Vec<u8>, flags: u32, pos: [f32; 3]) {
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1234u32.to_le_bytes());
        put_f32s(b, &[pos[0], pos[1], pos[2], 1.5]);
        b.extend_from_slice(&0u32.to_le_bytes()); // fall time
        put_f32s(b, &[2.5, 7.0, 4.5, 4.72, 2.5, 7.0, 4.5, 3.5, 3.5]);
    }

    /// An empty field block: no mask words.
    fn no_fields(b: &mut Vec<u8>) {
        b.push(0);
    }

    fn parse(blocks: &[Vec<u8>]) -> Vec<Object> {
        let mut b = (blocks.len() as u32).to_le_bytes().to_vec();
        for block in blocks {
            b.extend_from_slice(block);
        }
        let mut r = b.as_slice();
        let objects = read_update_object(&mut r).unwrap();
        assert!(r.is_empty(), "{} trailing bytes", r.len());
        objects
    }

    /// Type 1 is the mover body alone (`0x4d6dd7`): a `u16` flags word read ahead of it would
    /// misalign the values block that follows.
    #[test]
    fn a_movement_block_reads_the_mover_body_with_no_update_flags() {
        let mut moved = vec![1];
        put_guid(&mut moved, UNIT);
        mover_body(&mut moved, 0x1, [10.0, 20.0, 30.0]); // FORWARD
        let mut values = vec![0];
        put_guid(&mut values, UNIT);
        no_fields(&mut values);
        let objects = parse(&[moved, values]);
        let Object::Movement { guid, movement } = &objects[0] else {
            panic!("expected a movement block");
        };
        assert_eq!(*guid, UNIT);
        let (pos, facing) = movement.position.unwrap();
        assert_eq!((pos.x, pos.y, pos.z, facing), (10.0, 20.0, 30.0, 1.5));
        assert_eq!(movement.speeds.unwrap()[1], 7.0);
        assert!(movement.mover.is_some() && movement.spline.is_none());
        assert!(matches!(objects[1], Object::Values { guid: UNIT, .. }));
    }

    /// The spline keeps its final facing and evaluates by `flags & 0x42000`, not by its mode byte.
    #[test]
    fn a_movement_block_spline_keeps_its_final_facing_and_takes_its_mode_from_the_flags() {
        let mut b = vec![1];
        put_guid(&mut b, UNIT);
        mover_body(&mut b, 0x800_0000, [0.0; 3]); // SPLINE_ENABLED
        b.extend_from_slice(&(SPLINE_FLAG_FINAL_TARGET | SPLINE_FLAG_CATMULLROM).to_le_bytes());
        b.extend_from_slice(&VICTIM.to_le_bytes());
        for v in [500u32, 2000, 77] {
            b.extend_from_slice(&v.to_le_bytes()); // time passed, duration, id
        }
        put_f32s(&mut b, &[1.0, 1.0, 0.0]); // duration mods, vertical acceleration
        b.extend_from_slice(&0u32.to_le_bytes()); // effect start
        b.extend_from_slice(&4u32.to_le_bytes());
        for x in [0.0, 0.0, 5.0, 10.0] {
            put_f32s(&mut b, &[x, 0.0, 0.0]);
        }
        b.push(0); // linear on the wire
        put_f32s(&mut b, &[10.0, 0.0, 0.0]);
        let objects = parse(&[b]);
        let Object::Movement { movement, .. } = &objects[0] else {
            panic!("expected a movement block");
        };
        let spline = movement.spline.as_ref().unwrap();
        assert!(matches!(
            spline.final_facing,
            MonsterMoveFacing::Target(VICTIM)
        ));
        assert!(
            spline.catmull_rom,
            "0x40000 picks Catmull-Rom over a zero mode byte"
        );
        assert!(!spline.flying);
        assert_eq!((spline.id, spline.time_passed_ms), (77, 500));
    }

    /// A `POSITION` create keeps the transport guid, the deck-local position and the transport
    /// facing; a zero guid is no transport.
    #[test]
    fn a_position_create_keeps_its_transport_and_local_pose() {
        let block = |transport: u64| {
            let mut b = vec![2];
            put_guid(&mut b, GO);
            b.push(5); // GameObject
            b.extend_from_slice(&UPDATE_FLAG_POSITION.to_le_bytes());
            put_guid(&mut b, transport);
            // World xyz, local xyz, facing, transport facing.
            put_f32s(&mut b, &[100.0, 200.0, 5.0, 0.5, 3.0, -2.0, 0.75, 1.0]);
            no_fields(&mut b);
            b
        };
        let objects = parse(&[block(SHIP), block(0)]);
        let Object::Create { movement, .. } = &objects[0] else {
            panic!("expected a create");
        };
        let (pos, facing) = movement.position.unwrap();
        assert_eq!((pos.x, pos.y, facing), (100.0, 200.0, 0.75));
        let t = movement.transport.unwrap();
        assert_eq!(t.guid, SHIP);
        assert_eq!(
            (t.pos.x, t.pos.y, t.pos.z, t.orientation),
            (0.5, 3.0, -2.0, 1.0)
        );
        assert_eq!(t.seat, -1);
        let Object::Create { movement, .. } = &objects[1] else {
            panic!("expected a create");
        };
        assert!(movement.transport.is_none());
    }

    /// `HAS_TARGET` carries the attack target, and only update type 3 is a fresh spawn.
    #[test]
    fn a_create_keeps_its_attack_target_and_create2_marks_the_spawn() {
        let block = |update_type: u8, target: u64| {
            let mut b = vec![update_type];
            put_guid(&mut b, UNIT);
            b.push(3); // Unit
            b.extend_from_slice(&(UPDATE_FLAG_LIVING | UPDATE_FLAG_HAS_TARGET).to_le_bytes());
            mover_body(&mut b, 0, [1.0, 2.0, 3.0]);
            put_guid(&mut b, target);
            no_fields(&mut b);
            b
        };
        let objects = parse(&[block(3, VICTIM), block(2, 0)]);
        let Object::Create {
            movement, spawned, ..
        } = &objects[0]
        else {
            panic!("expected a create");
        };
        assert_eq!((movement.attack_target, *spawned), (Some(VICTIM), true));
        let Object::Create {
            movement, spawned, ..
        } = &objects[1]
        else {
            panic!("expected a create");
        };
        assert_eq!((movement.attack_target, *spawned), (None, false));
    }

    #[test]
    fn guid_highs_name_their_object_type() {
        assert_eq!(object_type_of_guid(0x10).unwrap(), ObjectType::Player);
        assert_eq!(
            object_type_of_guid(0xF130_0000_0000_0001).unwrap(),
            ObjectType::Unit
        );
        assert_eq!(
            object_type_of_guid(0xF140_0000_0000_0001).unwrap(),
            ObjectType::Unit
        );
        assert_eq!(
            object_type_of_guid(0xF110_0000_0000_0001).unwrap(),
            ObjectType::GameObject
        );
        assert_eq!(
            object_type_of_guid(0x1FC0_0000_0000_0001).unwrap(),
            ObjectType::GameObject
        );
        assert_eq!(
            object_type_of_guid(0x4000_0000_0000_0001).unwrap(),
            ObjectType::Container
        );
        assert_eq!(
            object_type_of_guid(0xF101_0000_0000_0001).unwrap(),
            ObjectType::Corpse
        );
        assert!(object_type_of_guid(0x1F50_0000_0000_0001).is_err());
    }
}
