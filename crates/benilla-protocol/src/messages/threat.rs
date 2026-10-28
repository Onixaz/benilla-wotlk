//! Threat lists: a unit's view of who it hates and how much, which the server pushes to every
//! player on that list (`SMSG_THREAT_UPDATE`, `SMSG_HIGHEST_THREAT_UPDATE`) and prunes
//! (`SMSG_THREAT_REMOVE`, `SMSG_THREAT_CLEAR`).

use std::io;

use crate::wire::{capacity_hint, read_packed_guid, read_u32_le};

/// One row of a threat update: a unit on the list and its raw threat (`0x7417a0` reads a packed
/// guid, then a `u32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreatEntry {
    pub target: u64,
    pub threat: u32,
}

/// `SMSG_THREAT_UPDATE` and `SMSG_HIGHEST_THREAT_UPDATE`, which one handler reads (`0x741c90`):
/// the unit whose list it is, the highest-threat guid on the highest variant alone, then the
/// rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreatUpdate {
    pub unit: u64,
    /// `Some` on `SMSG_HIGHEST_THREAT_UPDATE` only; the guid may be 0, which clears the top.
    pub highest: Option<u64>,
    pub entries: Vec<ThreatEntry>,
}

/// The list rows a server can send: AzerothCore caps nothing, so this only bounds the
/// pre-allocation against a hostile count.
const MAX_THREAT_ROWS: usize = 1024;

pub(super) fn read_threat_update(r: &mut &[u8], highest: bool) -> io::Result<ThreatUpdate> {
    let unit = read_packed_guid(r)?;
    let highest = if highest {
        Some(read_packed_guid(r)?)
    } else {
        None
    };
    let count = read_u32_le(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(count, MAX_THREAT_ROWS));
    for _ in 0..count {
        let target = read_packed_guid(r)?;
        let threat = read_u32_le(r)?;
        entries.push(ThreatEntry { target, threat });
    }
    Ok(ThreatUpdate {
        unit,
        highest,
        entries,
    })
}

/// `SMSG_THREAT_REMOVE` (`0x737b20`): the unit, then the guid that left its list, both packed.
pub(super) fn read_threat_remove(r: &mut &[u8]) -> io::Result<(u64, u64)> {
    Ok((read_packed_guid(r)?, read_packed_guid(r)?))
}

/// `SMSG_THREAT_CLEAR` (`0x734b00`): the unit whose whole list goes.
pub(super) fn read_threat_clear(r: &mut &[u8]) -> io::Result<u64> {
    read_packed_guid(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    /// A packed guid: the mask byte, then the nonzero bytes, low first.
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

    const MOB: u64 = 0xF130_0000_0000_00AB;
    const TANK: u64 = 0x10;
    const HEALER: u64 = 0x20;

    fn rows(rows: &[(u64, u32)]) -> Vec<u8> {
        let mut body = (rows.len() as u32).to_le_bytes().to_vec();
        for &(guid, threat) in rows {
            body.extend(packed(guid));
            body.extend(threat.to_le_bytes());
        }
        body
    }

    #[test]
    fn threat_update_wire() {
        let mut body = packed(MOB);
        body.extend(rows(&[(TANK, 1200), (HEALER, 300)]));
        match parse_server(opcode::SMSG_THREAT_UPDATE, &body).unwrap() {
            ServerPacket::ThreatUpdate(u) => assert_eq!(
                u,
                ThreatUpdate {
                    unit: MOB,
                    highest: None,
                    entries: vec![
                        ThreatEntry {
                            target: TANK,
                            threat: 1200
                        },
                        ThreatEntry {
                            target: HEALER,
                            threat: 300
                        },
                    ],
                }
            ),
            other => panic!("expected ThreatUpdate, got {}", other.name()),
        }
    }

    /// The highest variant carries the top guid between the unit and the count.
    #[test]
    fn highest_threat_update_wire() {
        let mut body = packed(MOB);
        body.extend(packed(TANK));
        body.extend(rows(&[(TANK, 1200)]));
        match parse_server(opcode::SMSG_HIGHEST_THREAT_UPDATE, &body).unwrap() {
            ServerPacket::ThreatUpdate(u) => {
                assert_eq!((u.unit, u.highest), (MOB, Some(TANK)));
                assert_eq!(u.entries.len(), 1);
            }
            other => panic!("expected ThreatUpdate, got {}", other.name()),
        }
        assert!(
            parse_server(opcode::SMSG_HIGHEST_THREAT_UPDATE, &body[..body.len() - 1]).is_err(),
            "a short row is refused"
        );
    }

    #[test]
    fn threat_remove_and_clear_wire() {
        let mut body = packed(MOB);
        body.extend(packed(HEALER));
        match parse_server(opcode::SMSG_THREAT_REMOVE, &body).unwrap() {
            ServerPacket::ThreatRemove { unit, target } => {
                assert_eq!((unit, target), (MOB, HEALER))
            }
            other => panic!("expected ThreatRemove, got {}", other.name()),
        }
        match parse_server(opcode::SMSG_THREAT_CLEAR, &packed(MOB)).unwrap() {
            ServerPacket::ThreatClear { unit } => assert_eq!(unit, MOB),
            other => panic!("expected ThreatClear, got {}", other.name()),
        }
    }
}
