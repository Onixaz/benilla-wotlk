//! The inverse DCT: a 16-bit fixed-point separable 8×8 transform (rows then columns, constants
//! `cos(iπ/16)·√2·2¹⁴`) that meets IEEE 1180 accuracy, as ISO/IEC 14496-2 7.4.4.3 requires. The
//! row pass keeps 16-bit intermediates and a row that holds only its DC takes the exact `dc·8`.

const W1: i64 = 22725;
const W2: i64 = 21407;
const W3: i64 = 19266;
const W4: i64 = 16383;
const W5: i64 = 12873;
const W6: i64 = 8867;
const W7: i64 = 4520;
const ROW_SHIFT: u32 = 11;
const COL_SHIFT: u32 = 20;

/// The 32-bit wrap of the fixed-point sums, which only a damaged stream reaches.
#[inline]
fn trunc(v: i64) -> i32 {
    v as i32
}

#[inline]
fn row(r: &mut [i16; 8]) {
    if r[1..].iter().all(|&c| c == 0) {
        let dc = (i32::from(r[0]) << 3) as i16;
        r.fill(dc);
        return;
    }
    let c = |i: usize| i64::from(r[i]);
    let mut a0 = W4 * c(0) + (1 << (ROW_SHIFT - 1));
    let mut a1 = a0;
    let mut a2 = a0;
    let mut a3 = a0;
    a0 += W2 * c(2);
    a1 += W6 * c(2);
    a2 -= W6 * c(2);
    a3 -= W2 * c(2);
    let mut b0 = W1 * c(1) + W3 * c(3);
    let mut b1 = W3 * c(1) - W7 * c(3);
    let mut b2 = W5 * c(1) - W1 * c(3);
    let mut b3 = W7 * c(1) - W5 * c(3);
    if r[4..].iter().any(|&v| v != 0) {
        a0 += W4 * c(4) + W6 * c(6);
        a1 += -W4 * c(4) - W2 * c(6);
        a2 += -W4 * c(4) + W2 * c(6);
        a3 += W4 * c(4) - W6 * c(6);
        b0 += W5 * c(5) + W7 * c(7);
        b1 += -W1 * c(5) - W5 * c(7);
        b2 += W7 * c(5) + W3 * c(7);
        b3 += W3 * c(5) - W1 * c(7);
    }
    r[0] = (trunc(a0 + b0) >> ROW_SHIFT) as i16;
    r[7] = (trunc(a0 - b0) >> ROW_SHIFT) as i16;
    r[1] = (trunc(a1 + b1) >> ROW_SHIFT) as i16;
    r[6] = (trunc(a1 - b1) >> ROW_SHIFT) as i16;
    r[2] = (trunc(a2 + b2) >> ROW_SHIFT) as i16;
    r[5] = (trunc(a2 - b2) >> ROW_SHIFT) as i16;
    r[3] = (trunc(a3 + b3) >> ROW_SHIFT) as i16;
    r[4] = (trunc(a3 - b3) >> ROW_SHIFT) as i16;
}

/// The column pass of column `x`: the eight outputs top to bottom.
#[inline]
fn col(b: &[i16; 64], x: usize) -> [i32; 8] {
    let c = |y: usize| i64::from(b[y * 8 + x]);
    // The rounding term rides on the DC as `(2^19 / W4)·W4`.
    let mut a0 = W4 * (c(0) + ((1 << (COL_SHIFT - 1)) / W4));
    let mut a1 = a0;
    let mut a2 = a0;
    let mut a3 = a0;
    a0 += W2 * c(2);
    a1 += W6 * c(2);
    a2 -= W6 * c(2);
    a3 -= W2 * c(2);
    let mut b0 = W1 * c(1) + W3 * c(3);
    let mut b1 = W3 * c(1) - W7 * c(3);
    let mut b2 = W5 * c(1) - W1 * c(3);
    let mut b3 = W7 * c(1) - W5 * c(3);
    a0 += W4 * c(4);
    a1 -= W4 * c(4);
    a2 -= W4 * c(4);
    a3 += W4 * c(4);
    b0 += W5 * c(5);
    b1 -= W1 * c(5);
    b2 += W7 * c(5);
    b3 += W3 * c(5);
    a0 += W6 * c(6);
    a1 -= W2 * c(6);
    a2 += W2 * c(6);
    a3 -= W6 * c(6);
    b0 += W7 * c(7);
    b1 -= W5 * c(7);
    b2 += W3 * c(7);
    b3 -= W1 * c(7);
    [
        trunc(a0 + b0) >> COL_SHIFT,
        trunc(a1 + b1) >> COL_SHIFT,
        trunc(a2 + b2) >> COL_SHIFT,
        trunc(a3 + b3) >> COL_SHIFT,
        trunc(a3 - b3) >> COL_SHIFT,
        trunc(a2 - b2) >> COL_SHIFT,
        trunc(a1 - b1) >> COL_SHIFT,
        trunc(a0 - b0) >> COL_SHIFT,
    ]
}

#[inline]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Transforms `block` and writes the 8×8 result into `dst` (an intra block).
pub(crate) fn put(block: &mut [i16; 64], dst: &mut [u8], stride: usize) {
    for r in block.as_chunks_mut::<8>().0 {
        row(r);
    }
    for x in 0..8 {
        for (y, v) in col(block, x).into_iter().enumerate() {
            dst[y * stride + x] = clip(v);
        }
    }
}

/// Transforms `block` and adds the result onto the prediction in `dst` (an inter block).
pub(crate) fn add(block: &mut [i16; 64], dst: &mut [u8], stride: usize) {
    for r in block.as_chunks_mut::<8>().0 {
        row(r);
    }
    for x in 0..8 {
        for (y, v) in col(block, x).into_iter().enumerate() {
            let d = &mut dst[y * stride + x];
            *d = clip(i32::from(*d) + v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lone DC of `d` reconstructs the flat block `d/8` (to rounding), as the DCT's scaling
    /// demands; and the transform tracks a float reference within one level on a mixed block.
    #[test]
    fn matches_the_float_transform() {
        let mut b = [0i16; 64];
        b[0] = 1024;
        let mut out = [0u8; 64];
        put(&mut b, &mut out, 8);
        assert!(out.iter().all(|&v| v == 128), "{out:?}");

        let mut coefs = [0i16; 64];
        for (i, c) in coefs.iter_mut().enumerate() {
            *c = ((i as i32 * 37 % 23) - 11) as i16 * if i < 10 { 9 } else { 1 };
        }
        coefs[0] = 800;
        let mut b = coefs;
        let mut out = [0u8; 64];
        put(&mut b, &mut out, 8);
        let cu = |u: usize| if u == 0 { (0.5f64).sqrt() } else { 1.0 };
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0;
                for v in 0..8 {
                    for u in 0..8 {
                        s += cu(u)
                            * cu(v)
                            * f64::from(coefs[v * 8 + u])
                            * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos()
                            * ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / 16.0).cos();
                    }
                }
                let want = (s / 4.0).round().clamp(0.0, 255.0);
                let got = f64::from(out[y * 8 + x]);
                assert!((want - got).abs() <= 1.0, "({x},{y}) {got} vs {want}");
            }
        }
    }
}
