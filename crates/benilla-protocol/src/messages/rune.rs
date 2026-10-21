//! The death knight's rune packets, served by the world handler `0x6e2e90`: `SMSG_CONVERT_RUNE`
//! (`0x486`), `SMSG_RESYNC_RUNES` (`0x487`) and `SMSG_ADD_RUNE_POWER` (`0x488`).

use std::io::{self, Read};

use crate::wire::{read_u32_le, read_u8};

/// `SMSG_CONVERT_RUNE`: rune `rune` (0-based) now counts as `kind` (0 blood, 1 unholy, 2 frost,
/// 3 death) (`CGRuneInfo::ConvertRune` `0x5ee020`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvertRune {
    pub rune: u8,
    pub kind: u8,
}

/// `SMSG_RESYNC_RUNES` (`0x5ee440`): the rune count, then each rune's kind and the share of its
/// recharge passed, out of 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResyncRunes {
    /// The wire count as sent; it becomes the live rune count whatever the list holds.
    pub count: u32,
    /// `(kind, progress)` per rune.
    pub runes: Vec<(u8, u8)>,
}

/// `SMSG_CONVERT_RUNE`: `u8 rune`, `u8 kind`.
pub(super) fn read_convert_rune(r: &mut impl Read) -> io::Result<ConvertRune> {
    Ok(ConvertRune {
        rune: read_u8(r)?,
        kind: read_u8(r)?,
    })
}

/// The runes the client keeps (`0xc24304`, eight slots); the reference's loop is bounded only by
/// the count, so no list past them is read.
const MAX_RUNES: u32 = 8;

/// `SMSG_RESYNC_RUNES`: `u32 count`, then `count` pairs of `u8 kind`, `u8 progress`.
pub(super) fn read_resync_runes(r: &mut impl Read) -> io::Result<ResyncRunes> {
    let count = read_u32_le(r)?;
    let mut runes = Vec::with_capacity(count.min(MAX_RUNES) as usize);
    for _ in 0..count.min(MAX_RUNES) {
        runes.push((read_u8(r)?, read_u8(r)?));
    }
    Ok(ResyncRunes { count, runes })
}

/// `SMSG_ADD_RUNE_POWER`: `u32` mask of the runes made ready (`0x5ee520`).
pub(super) fn read_add_rune_power(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_rune_reads_the_rune_then_its_new_kind() {
        let c = read_convert_rune(&mut [4u8, 3].as_slice()).unwrap();
        assert_eq!(c, ConvertRune { rune: 4, kind: 3 });
    }

    #[test]
    fn resync_runes_reads_a_kind_and_progress_pair_per_rune() {
        let mut body = 6u32.to_le_bytes().to_vec();
        for pair in [[0, 255], [0, 0], [1, 255], [1, 128], [2, 255], [3, 255]] {
            body.extend_from_slice(&pair);
        }
        let r = read_resync_runes(&mut body.as_slice()).unwrap();
        assert_eq!(r.count, 6);
        assert_eq!(
            r.runes,
            [(0, 255), (0, 0), (1, 255), (1, 128), (2, 255), (3, 255)]
        );
    }

    #[test]
    fn add_rune_power_reads_the_mask() {
        assert_eq!(
            read_add_rune_power(&mut 0x21u32.to_le_bytes().as_slice()).unwrap(),
            0x21
        );
    }
}
