//! World states, the server's key/value table (`SMSG_INIT_WORLD_STATES` `0x2C2`,
//! `SMSG_UPDATE_WORLD_STATE` `0x2C3`) that NPC-text `$<n>w`/`$<n>e` tokens and scoreboards read.
//!
//! On `0x2C2` the reference takes map, zone and area as its UI filter (`0x548970`) before the
//! pairs run. Ids and values stay raw dwords: `$<n>e` reads the table at the negated key, so
//! `$2077e` looks up `0xFFFFF7E3`.

use std::io;

use crate::wire::{read_u16_le, read_u32_le};

/// `SMSG_INIT_WORLD_STATES`: the whole table for a zone, sent on login and every zone change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitWorldStates {
    /// The map the states are scoped to (`[0xbe8f54]`).
    pub map: u32,
    /// The zone (`[0xbe8f50]`) and the area within it (`[0xbe8f4c]`), either of which a
    /// `WorldStateUI.dbc` row's `AreaID` may name.
    pub zone: u32,
    pub area: u32,
    /// `(id, value)` verbatim.
    pub states: Vec<(u32, u32)>,
}

/// Read `SMSG_INIT_WORLD_STATES` (`0x526530`): `u32 map, u32 zone, u32 area, u16 count`, then
/// `count` `(id, value)` pairs.
pub(in crate::messages) fn read_init_world_states(r: &mut &[u8]) -> io::Result<InitWorldStates> {
    let map = read_u32_le(r)?;
    let zone = read_u32_le(r)?;
    let area = read_u32_le(r)?;
    let count = read_u16_le(r)?;
    let mut states = Vec::new();
    for _ in 0..count {
        states.push((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok(InitWorldStates {
        map,
        zone,
        area,
        states,
    })
}

/// Read `SMSG_UPDATE_WORLD_STATE`: one `(id, value)` dword pair.
pub(in crate::messages) fn read_update_world_state(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AzerothCore's Gadgetzan init: Kalimdor, Tanaris (440), Gadgetzan (976), its pairs.
    #[test]
    fn init_reads_the_scope_triple_and_every_pair() {
        let mut body = Vec::new();
        for v in [1u32, 440, 976] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&2u16.to_le_bytes());
        for (id, value) in [(2259u32, 0u32), (3191, 8)] {
            body.extend_from_slice(&id.to_le_bytes());
            body.extend_from_slice(&value.to_le_bytes());
        }
        let mut r = body.as_slice();
        let got = read_init_world_states(&mut r).unwrap();
        assert_eq!((got.map, got.zone, got.area), (1, 440, 976));
        assert_eq!(got.states, vec![(2259, 0), (3191, 8)]);
        assert!(r.is_empty(), "the whole body is read");
    }

    #[test]
    fn init_truncated_run_errors() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&u16::MAX.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        assert!(read_init_world_states(&mut body.as_slice()).is_err());
    }

    /// A top-bit value stays unsigned; only the `%d` render downstream gives it a sign.
    #[test]
    fn update_reads_a_raw_dword_pair() {
        let mut body = Vec::new();
        body.extend_from_slice(&2264u32.to_le_bytes());
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(
            read_update_world_state(&mut body.as_slice()).unwrap(),
            (2264, 0xFFFF_FFFF)
        );
    }
}
