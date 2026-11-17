//! Half-pel motion compensation (ISO/IEC 14496-2 7.6.2): a block predicted from a reference
//! plane whose samples outside the macroblock-aligned picture repeat its edge.

/// A reference plane with its macroblock-aligned size.
#[derive(Clone, Copy)]
pub(crate) struct Plane<'a> {
    pub data: &'a [u8],
    pub w: usize,
    pub h: usize,
}

impl Plane<'_> {
    #[inline]
    fn at(&self, x: isize, y: isize) -> u8 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[y * self.w + x]
    }
}

/// Predicts a `bw`×`bh` block whose top-left full-pel sample is `(x, y)`, half a sample right
/// and down per `hx`/`hy`, into `dst`. `round_down` is `vop_rounding_type`. With `average` the
/// prediction is averaged into `dst` (rounding up), the second half of a bidirectional block.
#[allow(clippy::too_many_arguments)]
#[inline]
pub(crate) fn predict(
    src: Plane,
    x: isize,
    y: isize,
    hx: bool,
    hy: bool,
    round_down: bool,
    bw: usize,
    bh: usize,
    dst: &mut [u8],
    stride: usize,
    average: bool,
) {
    let r = i32::from(round_down);
    let inside = x >= 0 && y >= 0 && x as usize + bw < src.w && y as usize + bh < src.h;
    for j in 0..bh {
        for i in 0..bw {
            let (a, b, c, d) = if inside {
                let o = (y as usize + j) * src.w + x as usize + i;
                (
                    src.data[o],
                    src.data[o + 1],
                    src.data[o + src.w],
                    src.data[o + src.w + 1],
                )
            } else {
                let (sx, sy) = (x + i as isize, y + j as isize);
                (
                    src.at(sx, sy),
                    src.at(sx + 1, sy),
                    src.at(sx, sy + 1),
                    src.at(sx + 1, sy + 1),
                )
            };
            let (a, b, c, d) = (i32::from(a), i32::from(b), i32::from(c), i32::from(d));
            let p = match (hx, hy) {
                (false, false) => a,
                (true, false) => (a + b + 1 - r) >> 1,
                (false, true) => (a + c + 1 - r) >> 1,
                (true, true) => (a + b + c + d + 2 - r) >> 2,
            };
            let o = &mut dst[j * stride + i];
            *o = if average {
                ((i32::from(*o) + p + 1) >> 1) as u8
            } else {
                p as u8
            };
        }
    }
}

/// The chroma vector of a four-vector macroblock from the sum of its luma vectors: the sum
/// over eight, with sixteenths rounded per Table 7-9.
pub(crate) fn chroma_from_sum(sum: i32) -> i32 {
    const ROUND: [i32; 16] = [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2];
    ROUND[(sum & 15) as usize] + ((sum >> 3) & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_half_pels_with_both_roundings() {
        let data = [10u8, 21, 30, 41];
        let src = Plane {
            data: &data,
            w: 2,
            h: 2,
        };
        let mut d = [0u8; 1];
        predict(src, 0, 0, true, false, false, 1, 1, &mut d, 1, false);
        assert_eq!(d[0], 16);
        predict(src, 0, 0, true, false, true, 1, 1, &mut d, 1, false);
        assert_eq!(d[0], 15);
        predict(src, 0, 0, true, true, false, 1, 1, &mut d, 1, false);
        assert_eq!(d[0], 26); // (10+21+30+41+2)/4
        predict(src, 0, 0, true, true, true, 1, 1, &mut d, 1, false);
        assert_eq!(d[0], 25);
        // Outside the plane the edge repeats.
        predict(src, -5, 7, false, false, false, 1, 1, &mut d, 1, false);
        assert_eq!(d[0], 30);
        d[0] = 100;
        predict(src, 1, 1, false, false, false, 1, 1, &mut d, 1, true);
        assert_eq!(d[0], 71);
    }

    #[test]
    fn rounds_four_vector_chroma_per_table_7_9() {
        // Four luma vectors of 1 half pel sum to 4: a quarter chroma pel rounds to a half.
        assert_eq!(chroma_from_sum(4), 1);
        assert_eq!(chroma_from_sum(8), 1);
        assert_eq!(chroma_from_sum(16), 2);
        assert_eq!(chroma_from_sum(2), 0);
        assert_eq!(chroma_from_sum(-4), -1);
        assert_eq!(chroma_from_sum(-16), -2);
    }
}
