//! 3.3.5a's aura feed: `SMSG_AURA_UPDATE_ALL` and `SMSG_AURA_UPDATE`, one handler for both
//! (`0x7300a0`). 3.3.5a carries no aura descriptor fields; a unit's auras exist only as these
//! packets fill its aura array (`unit+0xc50`, 24-byte records).

use std::io::{self, Read};

use crate::wire::{read_packed_guid, read_u32_le, read_u8};

/// Effect-index bits 0-2: a record is live only with one set (`test [rec+0xc],7`, `0x72f7a5`).
pub const AURA_UPDATE_EFF_INDEX_MASK: u8 = 0x07;
/// No caster guid follows: the unit cast it on itself (`0x716553`).
pub const AURA_UPDATE_FLAG_SELF_CAST: u8 = 0x08;
/// The `CANCELABLE`/`NOT_CANCELABLE` filter's bit (`0x72cad0`).
pub const AURA_UPDATE_FLAG_CANCELABLE: u8 = 0x10;
/// A duration pair follows (`0x716578`).
pub const AURA_UPDATE_FLAG_DURATION: u8 = 0x20;
/// Harmful: the `HELPFUL`/`HARMFUL` filter's bit (`0x72ca9e`).
pub const AURA_UPDATE_FLAG_HARMFUL: u8 = 0x80;

/// One aura as the slot parser `0x716510` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraRecord {
    pub spell_id: u32,
    pub flags: u8,
    /// The caster's level.
    pub level: u8,
    /// Stack count, or charges for an unstacking spell; raw, not `count - 1`.
    pub stacks: u8,
    /// The caster's guid; `None` for [`AURA_UPDATE_FLAG_SELF_CAST`], the unit itself.
    pub caster: Option<u64>,
    /// `(max, remaining)` in ms, with [`AURA_UPDATE_FLAG_DURATION`].
    pub duration_ms: Option<(u32, u32)>,
}

impl AuraRecord {
    /// Holds an aura: an effect bit is set (`0x72f7a5`, `0x72c5e6`).
    pub fn is_live(&self) -> bool {
        self.flags & AURA_UPDATE_EFF_INDEX_MASK != 0
    }
    pub fn is_harmful(&self) -> bool {
        self.flags & AURA_UPDATE_FLAG_HARMFUL != 0
    }
    pub fn is_cancelable(&self) -> bool {
        self.flags & AURA_UPDATE_FLAG_CANCELABLE != 0
    }
}

/// One decoded aura packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuraUpdate {
    pub unit: u64,
    /// `SMSG_AURA_UPDATE_ALL`: the unit's whole array, every slot not named here emptied
    /// (`0x72f653`-`0x72f69f`).
    pub all: bool,
    /// `(slot, record)` in wire order; `None` is a spell id of 0, the slot cleared.
    pub slots: Vec<(u8, Option<AuraRecord>)>,
}

/// Read either aura packet (`0x7300a0` → `0x72f5d0`): a packed unit guid, then slots to the end
/// of the body, each a `u8` slot and `u32` spell; a non-zero spell adds `u8` flags, level and
/// stacks, the packed caster guid unless self-cast, and `u32` max and remaining duration when
/// flagged (`0x716510`).
pub(super) fn read_aura_update(r: &mut &[u8], all: bool) -> io::Result<AuraUpdate> {
    let unit = read_packed_guid(r)?;
    let mut slots = Vec::new();
    while !r.is_empty() {
        let slot = read_u8(r)?;
        slots.push((slot, read_record(r)?));
    }
    Ok(AuraUpdate { unit, all, slots })
}

fn read_record(r: &mut impl Read) -> io::Result<Option<AuraRecord>> {
    let spell_id = read_u32_le(r)?;
    if spell_id == 0 {
        return Ok(None);
    }
    let flags = read_u8(r)?;
    let level = read_u8(r)?;
    let stacks = read_u8(r)?;
    let caster = if flags & AURA_UPDATE_FLAG_SELF_CAST != 0 {
        None
    } else {
        Some(read_packed_guid(r)?)
    };
    let duration_ms = if flags & AURA_UPDATE_FLAG_DURATION != 0 {
        Some((read_u32_le(r)?, read_u32_le(r)?))
    } else {
        None
    };
    Ok(Some(AuraRecord {
        spell_id,
        flags,
        level,
        stacks,
        caster,
        duration_ms,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_self_cast_timed_aura_and_a_cleared_slot() {
        // Packed guid 0x2a; slot 3: Ice Barrier 43039, flags SELF|CANCELABLE|DURATION|eff0, level
        // 80, 1 stack, 60000/59000 ms; slot 7 cleared.
        let mut body: Vec<u8> = vec![0x01, 0x2a, 3];
        body.extend(43039u32.to_le_bytes());
        body.extend([0x39, 80, 1]);
        body.extend(60_000u32.to_le_bytes());
        body.extend(59_000u32.to_le_bytes());
        body.push(7);
        body.extend(0u32.to_le_bytes());
        let mut r = body.as_slice();
        let u = read_aura_update(&mut r, false).unwrap();
        assert!(r.is_empty());
        assert_eq!(u.unit, 0x2a);
        assert!(!u.all);
        let ice = u.slots[0].1.unwrap();
        assert_eq!(u.slots[0].0, 3);
        assert_eq!(ice.spell_id, 43039);
        assert_eq!(ice.caster, None);
        assert_eq!(ice.duration_ms, Some((60_000, 59_000)));
        assert!(ice.is_live() && ice.is_cancelable() && !ice.is_harmful());
        assert_eq!(u.slots[1], (7, None));
    }

    #[test]
    fn a_harmful_aura_names_its_caster_and_no_duration_reads_none() {
        // Frost Nova 42917 from caster 0x0102 (packed: mask 0b11), flags HARMFUL|eff0.
        let mut body: Vec<u8> = vec![0x01, 0x05, 0];
        body.extend(42917u32.to_le_bytes());
        body.extend([0x81, 80, 0, 0x03, 0x02, 0x01]);
        let mut r = body.as_slice();
        let u = read_aura_update(&mut r, true).unwrap();
        assert!(u.all);
        let nova = u.slots[0].1.unwrap();
        assert_eq!(nova.caster, Some(0x0102));
        assert_eq!(nova.duration_ms, None);
        assert!(nova.is_harmful() && !nova.is_cancelable());
    }

    #[test]
    fn a_truncated_record_is_an_error() {
        let mut body: Vec<u8> = vec![0x01, 0x05, 0];
        body.extend(42917u32.to_le_bytes());
        body.push(0x81);
        assert!(read_aura_update(&mut body.as_slice(), false).is_err());
    }
}
