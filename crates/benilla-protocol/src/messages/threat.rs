//! A unit's threat list: `SMSG_HIGHEST_THREAT_UPDATE` and `SMSG_THREAT_UPDATE` (both `0x741c90`),
//! `SMSG_THREAT_REMOVE` (`0x737b20`) and `SMSG_THREAT_CLEAR` (`0x734b00`).

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_packed_guid, read_u32_le};

/// `SMSG_HIGHEST_THREAT_UPDATE` or `SMSG_THREAT_UPDATE`: entries of `unit`'s threat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreatUpdate {
    /// The unit whose list this is.
    pub unit: u64,
    /// The highest-threat unit `SMSG_HIGHEST_THREAT_UPDATE` names, 0 for none (`0x7417c4`);
    /// `None` for `SMSG_THREAT_UPDATE`, which carries no such guid.
    pub highest: Option<u64>,
    /// `(unit, threat)` in wire order; the threat is the server's value as sent.
    pub entries: Vec<(u64, u32)>,
}

/// Read either update: packed `unit`, the highest's packed guid when `highest`, `u32 count`, then
/// `count` × (packed guid, `u32 threat`).
pub(super) fn read_threat_update(r: &mut impl Read, highest: bool) -> io::Result<ThreatUpdate> {
    let unit = read_packed_guid(r)?;
    let highest = if highest {
        Some(read_packed_guid(r)?)
    } else {
        None
    };
    let count = read_u32_le(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        let guid = read_packed_guid(r)?;
        entries.push((guid, read_u32_le(r)?));
    }
    Ok(ThreatUpdate {
        unit,
        highest,
        entries,
    })
}

/// `SMSG_THREAT_REMOVE`: packed `unit`, packed `victim`.
pub(super) fn read_threat_remove(r: &mut impl Read) -> io::Result<(u64, u64)> {
    Ok((read_packed_guid(r)?, read_packed_guid(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One byte of mask, then the guid's non-zero bytes, low first.
    fn packed(guid: u64) -> Vec<u8> {
        let mut mask = 0u8;
        let mut bytes = Vec::new();
        for (i, b) in guid.to_le_bytes().into_iter().enumerate() {
            if b != 0 {
                mask |= 1 << i;
                bytes.push(b);
            }
        }
        let mut out = vec![mask];
        out.extend(bytes);
        out
    }

    const MOB: u64 = 0xF130_0000_0C35_0001;
    const ME: u64 = 0x10;
    const PAL: u64 = 0x2A;

    #[test]
    fn highest_update_reads_the_highest_before_the_count() {
        let mut body = packed(MOB);
        body.extend(packed(ME));
        body.extend(2u32.to_le_bytes());
        body.extend(packed(ME));
        body.extend(15_000u32.to_le_bytes());
        body.extend(packed(PAL));
        body.extend(9_900u32.to_le_bytes());
        let u = read_threat_update(&mut body.as_slice(), true).unwrap();
        assert_eq!(
            u,
            ThreatUpdate {
                unit: MOB,
                highest: Some(ME),
                entries: vec![(ME, 15_000), (PAL, 9_900)],
            }
        );
    }

    #[test]
    fn update_has_no_highest_and_an_empty_list_reads() {
        let mut body = packed(MOB);
        body.extend(0u32.to_le_bytes());
        let u = read_threat_update(&mut body.as_slice(), false).unwrap();
        assert_eq!(u.highest, None);
        assert!(u.entries.is_empty());
        let mut short = packed(MOB);
        short.extend(1u32.to_le_bytes());
        assert!(read_threat_update(&mut short.as_slice(), false).is_err());
    }

    #[test]
    fn remove_reads_unit_then_victim() {
        let mut body = packed(MOB);
        body.extend(packed(PAL));
        assert_eq!(
            read_threat_remove(&mut body.as_slice()).unwrap(),
            (MOB, PAL)
        );
    }
}
