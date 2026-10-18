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
const SPLINE_FLAG_WALKMODE: u32 = 0x1000;
const SPLINE_FLAG_FLYING: u32 = 0x2000;
const SPLINE_FLAG_FINAL_POINT: u32 = 0x8000;
const SPLINE_FLAG_FINAL_TARGET: u32 = 0x1_0000;
const SPLINE_FLAG_FINAL_ANGLE: u32 = 0x2_0000;
const SPLINE_FLAG_CYCLIC: u32 = 0x8_0000;

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
        1 => Object::Movement {
            guid: read_packed_guid(r)?,
            movement: read_movement_block(r)?.block,
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
/// position, facing, corpse facing), or a `STATIONARY_POSITION`; then the flag-gated tail.
fn read_movement_block(r: &mut impl Read) -> io::Result<WotlkMovement> {
    let update_flag = read_u16_le(r)?;
    let mut position = None;
    let mut mover = None;
    let mut speeds = None;
    let mut transport = None;
    let mut transport_progress = None;
    let mut spline = None;
    let mut rotation = None;

    if update_flag & UPDATE_FLAG_LIVING != 0 {
        let info = read_movement_info_wotlk(r)?;
        position = Some((info.position, info.orientation));
        transport = info.transport;
        mover = Some(MoverState {
            flags: info.flags,
            pitch: info.pitch,
        });
        // Walk, run, run back, swim, swim back, flight, flight back, turn rate, pitch rate.
        let mut s = [0.0f32; 9];
        for slot in &mut s {
            *slot = read_f32_le(r)?;
        }
        speeds = Some(s);
        if info.flags & crate::messages::movement::MOVEMENT_FLAG_SPLINE_ENABLED != 0 {
            spline = read_create_spline(r)?;
        }
    } else if update_flag & UPDATE_FLAG_POSITION != 0 {
        let _transport = read_packed_guid(r)?;
        let pos = Vector3d::read(r)?;
        // The transport offset, or the position again off a transport.
        let _local = Vector3d::read(r)?;
        let orientation = read_f32_le(r)?;
        let _corpse_orientation = read_f32_le(r)?;
        position = Some((pos, orientation));
    } else if update_flag & UPDATE_FLAG_STATIONARY_POSITION != 0 {
        let pos = Vector3d::read(r)?;
        position = Some((pos, read_f32_le(r)?));
    }

    if update_flag & UPDATE_FLAG_UNKNOWN != 0 {
        let _ = read_u32_le(r)?;
    }
    if update_flag & UPDATE_FLAG_LOWGUID != 0 {
        let _ = read_u32_le(r)?;
    }
    if update_flag & UPDATE_FLAG_HAS_TARGET != 0 {
        let _ = read_packed_guid(r)?;
    }
    if update_flag & UPDATE_FLAG_TRANSPORT != 0 {
        transport_progress = Some(read_u32_le(r)?);
    }
    if update_flag & UPDATE_FLAG_VEHICLE != 0 {
        let _vehicle_id = read_u32_le(r)?;
        let _orientation = read_f32_le(r)?;
    }
    if update_flag & UPDATE_FLAG_ROTATION != 0 {
        rotation = Some(unpack_rotation(read_u64_le(r)? as i64));
    }

    Ok(WotlkMovement {
        block: MovementBlock {
            position,
            mover,
            speeds,
            transport_progress,
            transport,
            spline,
        },
        rotation,
    })
}

/// `PacketBuilder::WriteCreate`: flags, the final facing, time passed, duration, id, two duration
/// mods, vertical acceleration, effect start time, the nodes, the `u8` evaluation mode and the
/// final destination. Every mode is initialised as Catmull-Rom (`Spline.cpp`, `initializers`), so
/// the nodes carry a virtual point at each end, as in 1.12.1.
fn read_create_spline(r: &mut impl Read) -> io::Result<Option<CreateSpline>> {
    let spline_flags = read_u32_le(r)?;
    if spline_flags & SPLINE_FLAG_FINAL_ANGLE != 0 {
        let _ = read_f32_le(r)?;
    } else if spline_flags & SPLINE_FLAG_FINAL_TARGET != 0 {
        let _ = read_u64_le(r)?;
    } else if spline_flags & SPLINE_FLAG_FINAL_POINT != 0 {
        let _ = Vector3d::read(r)?;
    }
    let time_passed_ms = read_u32_le(r)?;
    let duration_ms = read_u32_le(r)?;
    let id = read_u32_le(r)?;
    let _duration_mod = read_f32_le(r)?;
    let _duration_mod_next = read_f32_le(r)?;
    let _vertical_acceleration = read_f32_le(r)?;
    let _effect_start_time = read_u32_le(r)?;
    let amount_of_nodes = read_u32_le(r)?;
    let mut nodes = Vec::with_capacity(capacity_hint(amount_of_nodes, 0xFFFF));
    for _ in 0..amount_of_nodes {
        let v = Vector3d::read(r)?;
        nodes.push([v.x, v.y, v.z]);
    }
    let _mode = read_u8(r)?;
    let _final_node = Vector3d::read(r)?;
    Ok((nodes.len() >= 4).then(|| CreateSpline {
        path: nodes[1..nodes.len() - 1].to_vec(),
        id,
        time_passed_ms,
        duration_ms,
        flying: spline_flags & SPLINE_FLAG_FLYING != 0,
        cyclic: spline_flags & SPLINE_FLAG_CYCLIC != 0,
        run_mode: spline_flags & SPLINE_FLAG_WALKMODE == 0,
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
