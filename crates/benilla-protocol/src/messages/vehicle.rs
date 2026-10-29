//! Vehicles on the wire: `SMSG_PLAYER_VEHICLE_DATA` (`0x716db0`), the cancelled-ride notice
//! (`0x800510`), the projectile position (`0x8005a0`), and the client's seat and exit requests.

use std::io::{self, Read};

use crate::wire::{read_packed_guid, read_u32_le, read_u64_le, read_u8, Vector3d};

use super::client::packed;
use super::MovementInfo;

/// `SMSG_PLAYER_VEHICLE_DATA`: packed unit guid, `u32` vehicle id, 0 to take its vehicle away.
pub(super) fn read_player_vehicle_data(r: &mut impl Read) -> io::Result<(u64, u32)> {
    Ok((read_packed_guid(r)?, read_u32_le(r)?))
}

/// `SMSG_SET_PROJECTILE_POSITION`: `u64` caster (unpacked), `u8` cast id, the impact point.
pub(super) fn read_set_projectile_position(r: &mut impl Read) -> io::Result<(u64, u8, Vector3d)> {
    Ok((read_u64_le(r)?, read_u8(r)?, Vector3d::read(r)?))
}

/// `CMSG_REQUEST_VEHICLE_SWITCH_SEAT`: the packed guid of the vehicle holding the seat, then the
/// seat (`0x74ca90`).
pub fn request_vehicle_switch_seat(vehicle: u64, seat: u8) -> Vec<u8> {
    let mut body = packed(vehicle);
    body.push(seat);
    body
}

/// `CMSG_EJECT_PASSENGER` (`0x757200`) and `CMSG_PLAYER_VEHICLE_ENTER` (`0x6d27c0`): a bare
/// unpacked guid.
pub fn vehicle_guid(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_DISMISS_CONTROLLED_VEHICLE`: the mover's packed guid and its `MovementInfo`
/// (`0x71ef80`; no counter, as it is not an ack).
pub fn dismiss_controlled_vehicle(mover: u64, info: &MovementInfo) -> Vec<u8> {
    let mut body = packed(mover);
    info.write_body(&mut body);
    body
}

/// `CMSG_CHANGE_SEATS_ON_CONTROLLED_VEHICLE`: the dismiss body, then the packed guid of the
/// vehicle holding the wanted seat and the seat (`0x71f0c0`); a guid of 0 with seat `0xff` or 1
/// asks for the previous or next seat.
pub fn change_seats_on_controlled_vehicle(
    mover: u64,
    info: &MovementInfo,
    vehicle: u64,
    seat: u8,
) -> Vec<u8> {
    let mut body = dismiss_controlled_vehicle(mover, info);
    body.extend(packed(vehicle));
    body.push(seat);
    body
}

/// `CMSG_UPDATE_PROJECTILE_POSITION` (`0x703640`): `u64` caster, `u32` spell, `u8` cast id, the
/// local impact point.
pub fn update_projectile_position(caster: u64, spell: u32, cast_id: u8, at: Vector3d) -> Vec<u8> {
    let mut body = caster.to_le_bytes().to_vec();
    body.extend_from_slice(&spell.to_le_bytes());
    body.push(cast_id);
    for v in [at.x, at.y, at.z] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body
}

/// Read a trailing `f32` triple; for tests of the impact point's order.
#[cfg(test)]
fn xyz(b: &[u8]) -> [f32; 3] {
    use crate::wire::read_f32_le;
    let mut r = b;
    [
        read_f32_le(&mut r).unwrap(),
        read_f32_le(&mut r).unwrap(),
        read_f32_le(&mut r).unwrap(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vehicle_data_and_projectile_position_read_their_layouts() {
        let mut b = vec![0x01, 0x2a];
        b.extend(36u32.to_le_bytes());
        assert_eq!(
            read_player_vehicle_data(&mut b.as_slice()).unwrap(),
            (0x2a, 36)
        );

        let mut b = 0xF150_0000_0000_0001u64.to_le_bytes().to_vec();
        b.push(7);
        for v in [1.0f32, 2.0, 3.0] {
            b.extend(v.to_le_bytes());
        }
        let (caster, cast, at) = read_set_projectile_position(&mut b.as_slice()).unwrap();
        assert_eq!((caster, cast), (0xF150_0000_0000_0001, 7));
        assert_eq!([at.x, at.y, at.z], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_seat_requests_write_their_layouts() {
        assert_eq!(request_vehicle_switch_seat(0x2a, 3), vec![0x01, 0x2a, 3]);
        assert_eq!(vehicle_guid(0x2a), 0x2au64.to_le_bytes().to_vec());
        let info = MovementInfo {
            flags: 0,
            timestamp: 1,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            transport: None,
            pitch: 0.0,
            fall_time: 0,
            jump: None,
        };
        let dismiss = dismiss_controlled_vehicle(0x2a, &info);
        let change = change_seats_on_controlled_vehicle(0x2a, &info, 0, 0xff);
        assert_eq!(&change[..dismiss.len()], dismiss.as_slice());
        assert_eq!(
            &change[dismiss.len()..],
            &[0x00, 0xff],
            "a zero guid packs to its mask"
        );
        let p = update_projectile_position(
            0x2a,
            100,
            4,
            Vector3d {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
        );
        assert_eq!(&p[8..12], &100u32.to_le_bytes());
        assert_eq!(p[12], 4);
        assert_eq!(xyz(&p[13..]), [1.0, 2.0, 3.0]);
    }
}
