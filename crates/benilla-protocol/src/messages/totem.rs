//! `SMSG_TOTEM_CREATED` (`0x526530` arm `0x413`) and `CMSG_TOTEM_DESTROYED` (`0x5197d0`).

use std::io::{self, Read};

use crate::wire::{read_u32_le, read_u64_le, read_u8};

/// `SMSG_TOTEM_CREATED`: a totem now holds `slot` (0 fire, 1 earth, 2 water, 3 air).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotemCreated {
    pub slot: u8,
    pub guid: u64,
    pub duration_ms: u32,
    pub spell_id: u32,
}

/// Read `SMSG_TOTEM_CREATED`: `u8 slot`, `u64 guid`, `u32 duration`, `u32 spell`.
pub(super) fn read_totem_created(r: &mut impl Read) -> io::Result<TotemCreated> {
    Ok(TotemCreated {
        slot: read_u8(r)?,
        guid: read_u64_le(r)?,
        duration_ms: read_u32_le(r)?,
        spell_id: read_u32_le(r)?,
    })
}

/// `CMSG_TOTEM_DESTROYED`: the slot byte alone.
pub fn totem_destroyed(slot: u8) -> Vec<u8> {
    vec![slot]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totem_created_reads_slot_guid_duration_and_spell() {
        let mut body = vec![1u8];
        body.extend_from_slice(&0xF130_0000_1234_5678u64.to_le_bytes());
        body.extend_from_slice(&60_000u32.to_le_bytes());
        body.extend_from_slice(&8071u32.to_le_bytes());
        let t = read_totem_created(&mut body.as_slice()).unwrap();
        assert_eq!(
            t,
            TotemCreated {
                slot: 1,
                guid: 0xF130_0000_1234_5678,
                duration_ms: 60_000,
                spell_id: 8071,
            }
        );
    }

    #[test]
    fn totem_destroyed_is_the_slot_byte() {
        assert_eq!(totem_destroyed(3), vec![3]);
    }
}
