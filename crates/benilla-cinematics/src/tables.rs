//! The small fixed tables of ISO/IEC 14496-2 (and the H.263 tables it shares), and the decoders
//! built from them.

use std::sync::OnceLock;

use crate::bits::Vlc;
use crate::coef;

/// `mcbpc` of an I-VOP (Table B-6): symbol `i` is `cbpc = i & 3`, with `dquant` for `i >= 4`;
/// symbol 8 is stuffing.
const MCBPC_I: [(u32, u8); 9] = [
    (1, 1),
    (1, 3),
    (2, 3),
    (3, 3),
    (1, 4),
    (1, 6),
    (2, 6),
    (3, 6),
    (1, 9),
];

/// `mcbpc` of a P-VOP (Table B-7), symbol `type * 4 + cbpc` for types inter, inter+q, inter4v,
/// intra, intra+q; symbol 20 is stuffing.
const MCBPC_P: [(u32, u8); 21] = [
    (0b1, 1),
    (0b0011, 4),
    (0b0010, 4),
    (0b000101, 6),
    (0b011, 3),
    (0b0000111, 7),
    (0b0000110, 7),
    (0b000000101, 9),
    (0b010, 3),
    (0b0000101, 7),
    (0b0000100, 7),
    (0b00000101, 8),
    (0b00011, 5),
    (0b00000100, 8),
    (0b00000011, 8),
    (0b0000011, 7),
    (0b000100, 6),
    (0b000000100, 9),
    (0b000000011, 9),
    (0b000000010, 9),
    (0b000000001, 9),
];

/// `cbpy` (Table B-8), symbol the intra pattern; an inter macroblock inverts it.
const CBPY: [(u32, u8); 16] = [
    (3, 4),
    (5, 5),
    (4, 5),
    (9, 4),
    (3, 5),
    (7, 4),
    (2, 6),
    (11, 4),
    (2, 5),
    (3, 6),
    (5, 4),
    (10, 4),
    (4, 4),
    (8, 4),
    (6, 4),
    (3, 2),
];

/// `dct_dc_size_luminance` (Table B-13), symbol the size.
const DC_LUMA: [(u32, u8); 13] = [
    (3, 3),
    (3, 2),
    (2, 2),
    (2, 3),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (1, 8),
    (1, 9),
    (1, 10),
    (1, 11),
];

/// `dct_dc_size_chrominance` (Table B-14), symbol the size.
const DC_CHROMA: [(u32, u8); 13] = [
    (3, 2),
    (2, 2),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (1, 8),
    (1, 9),
    (1, 10),
    (1, 11),
    (1, 12),
];

/// `mb_type` of a B-VOP (Table B-4): direct, interpolate, backward, forward.
const MB_TYPE_B: [(u32, u8); 4] = [(1, 1), (1, 2), (1, 3), (1, 4)];

/// `dquant` (Table 6-30).
pub(crate) const DQUANT: [i32; 4] = [-1, -2, 1, 2];

/// `intra_dc_vlc_thr` (Table 6-21): the intra DC VLC is used while the running QP is below it.
pub(crate) const DC_THRESHOLD: [i32; 8] = [99, 13, 15, 17, 19, 21, 23, 0];

pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

pub(crate) const ALT_HORIZONTAL: [u8; 64] = [
    0, 1, 2, 3, 8, 9, 16, 17, 10, 11, 4, 5, 6, 7, 15, 14, 13, 12, 19, 18, 24, 25, 32, 33, 26, 27,
    20, 21, 22, 23, 28, 29, 30, 31, 34, 35, 40, 41, 48, 49, 42, 43, 36, 37, 38, 39, 44, 45, 46, 47,
    50, 51, 56, 57, 58, 59, 52, 53, 54, 55, 60, 61, 62, 63,
];

pub(crate) const ALT_VERTICAL: [u8; 64] = [
    0, 8, 16, 24, 1, 9, 2, 10, 17, 25, 32, 40, 48, 56, 57, 49, 41, 33, 26, 18, 3, 11, 4, 12, 19,
    27, 34, 42, 50, 58, 35, 43, 51, 59, 20, 28, 5, 13, 6, 14, 21, 29, 36, 44, 52, 60, 37, 45, 53,
    61, 22, 30, 7, 15, 23, 31, 38, 46, 54, 62, 39, 47, 55, 63,
];

/// `dc_scaler` for luminance (Table 7-1).
pub(crate) fn dc_scale_luma(qp: i32) -> i32 {
    match qp {
        ..=4 => 8,
        5..=8 => 2 * qp,
        9..=24 => qp + 8,
        _ => 2 * qp - 16,
    }
}

/// `dc_scaler` for chrominance (Table 7-1).
pub(crate) fn dc_scale_chroma(qp: i32) -> i32 {
    match qp {
        ..=4 => 8,
        5..=24 => (qp + 13) / 2,
        _ => qp - 6,
    }
}

/// One texture table: its VLC (escape is the last symbol) and the largest level and run per
/// `(last, run)` and `(last, level)` that escape modes 1 and 2 add.
pub(crate) struct Tcoef {
    pub vlc: Vlc,
    /// `(last, run, level)` per symbol.
    pub entries: Vec<(bool, u8, u8)>,
    pub escape: usize,
    pub max_level: [[u8; 64]; 2],
    pub max_run: [[u8; 64]; 2],
}

impl Tcoef {
    fn new(table: &[(u32, u8, u8, u8, u8)]) -> Self {
        let mut codes: Vec<(u32, u8)> = table.iter().map(|e| (e.0, e.1)).collect();
        codes.push((0b0000011, 7));
        let mut max_level = [[0u8; 64]; 2];
        let mut max_run = [[0u8; 64]; 2];
        for &(_, _, last, run, level) in table {
            let l = usize::from(last);
            max_level[l][usize::from(run)] = max_level[l][usize::from(run)].max(level);
            max_run[l][usize::from(level)] = max_run[l][usize::from(level)].max(run);
        }
        Self {
            vlc: Vlc::new(&codes),
            entries: table.iter().map(|e| (e.2 == 1, e.3, e.4)).collect(),
            escape: table.len(),
            max_level,
            max_run,
        }
    }
}

pub(crate) struct Tables {
    pub mcbpc_i: Vlc,
    pub mcbpc_p: Vlc,
    pub cbpy: Vlc,
    pub dc_luma: Vlc,
    pub dc_chroma: Vlc,
    pub mb_type_b: Vlc,
    pub mv: Vlc,
    pub intra: Tcoef,
    pub inter: Tcoef,
}

pub(crate) fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| Tables {
        mcbpc_i: Vlc::new(&MCBPC_I),
        mcbpc_p: Vlc::new(&MCBPC_P),
        cbpy: Vlc::new(&CBPY),
        dc_luma: Vlc::new(&DC_LUMA),
        dc_chroma: Vlc::new(&DC_CHROMA),
        mb_type_b: Vlc::new(&MB_TYPE_B),
        mv: Vlc::new(&coef::MV),
        intra: Tcoef::new(&coef::INTRA_TCOEF),
        inter: Tcoef::new(&coef::INTER_TCOEF),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table builds as a prefix code (`Vlc::new` asserts it) and every scan is a
    /// permutation of the 64 positions.
    #[test]
    fn the_tables_are_prefix_codes_and_the_scans_permutations() {
        let _ = tables();
        for scan in [ZIGZAG, ALT_HORIZONTAL, ALT_VERTICAL] {
            let mut seen = [false; 64];
            for &i in &scan {
                assert!(!seen[usize::from(i)]);
                seen[usize::from(i)] = true;
            }
        }
    }

    #[test]
    fn dc_scalers_follow_table_7_1() {
        let luma: Vec<i32> = (1..=31).map(dc_scale_luma).collect();
        let chroma: Vec<i32> = (1..=31).map(dc_scale_chroma).collect();
        assert_eq!(
            luma,
            [
                8, 8, 8, 8, 10, 12, 14, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
                31, 32, 34, 36, 38, 40, 42, 44, 46
            ]
        );
        assert_eq!(
            chroma,
            [
                8, 8, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17,
                18, 18, 19, 20, 21, 22, 23, 24, 25
            ]
        );
    }
}
