//! A decoded picture: macroblock-aligned 4:2:0 planes, plus what a later B-VOP reads back from
//! its future reference (each macroblock's skip, its 8×8 vectors, and whether it used four).

use std::sync::Arc;

pub(crate) struct Picture {
    /// Macroblock-aligned luma size; chroma is half of each.
    pub w: usize,
    pub h: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Per macroblock: `not_coded` in a P-VOP.
    pub skipped: Vec<bool>,
    /// Per macroblock: four vectors (`inter4v`).
    pub four_mv: Vec<bool>,
    /// Per 8×8 luma block, `2·mb_w` wide: the forward vector in half pels (0 for intra).
    pub mv: Vec<[i32; 2]>,
}

impl Picture {
    pub fn new(mb_w: usize, mb_h: usize) -> Self {
        let (w, h) = (mb_w * 16, mb_h * 16);
        Self {
            w,
            h,
            y: vec![0; w * h],
            u: vec![0; w * h / 4],
            v: vec![0; w * h / 4],
            skipped: vec![false; mb_w * mb_h],
            four_mv: vec![false; mb_w * mb_h],
            mv: vec![[0, 0]; mb_w * mb_h * 4],
        }
    }
}

/// One displayed frame: a decoded picture cropped to the VOL's size. Cloning shares the planes.
#[derive(Clone)]
pub struct Frame {
    pub(crate) pic: Arc<Picture>,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

impl Frame {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Plane `p` (0 Y, 1 U, 2 V) as `(data, stride, width, height)`.
    pub fn plane(&self, p: usize) -> (&[u8], usize, usize, usize) {
        match p {
            0 => (&self.pic.y, self.pic.w, self.width, self.height),
            1 => (
                &self.pic.u,
                self.pic.w / 2,
                self.width.div_ceil(2),
                self.height.div_ceil(2),
            ),
            _ => (
                &self.pic.v,
                self.pic.w / 2,
                self.width.div_ceil(2),
                self.height.div_ceil(2),
            ),
        }
    }

    /// The frame as tightly packed RGBA8, BT.601 studio range (the MPEG-4 default colour
    /// description when the VOL carries none), into `out` of `width · height · 4` bytes.
    pub fn to_rgba(&self, out: &mut [u8]) {
        assert_eq!(out.len(), self.width * self.height * 4);
        let pic = &*self.pic;
        let cw = pic.w / 2;
        for (yy, row) in out.chunks_exact_mut(self.width * 4).enumerate() {
            let ly = &pic.y[yy * pic.w..];
            let cu = &pic.u[(yy / 2) * cw..];
            let cv = &pic.v[(yy / 2) * cw..];
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                // Fixed point at 2^16: 1.164·(Y−16), 1.596·V′, 0.391·U′ + 0.813·V′, 2.018·U′.
                let c = (i32::from(ly[x]) - 16) * 76309;
                let d = i32::from(cu[x / 2]) - 128;
                let e = i32::from(cv[x / 2]) - 128;
                let r = (c + 104597 * e + 32768) >> 16;
                let g = (c - 25675 * d - 53279 * e + 32768) >> 16;
                let b = (c + 132201 * d + 32768) >> 16;
                px[0] = r.clamp(0, 255) as u8;
                px[1] = g.clamp(0, 255) as u8;
                px[2] = b.clamp(0, 255) as u8;
                px[3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(y: [u8; 2], u: u8, v: u8) -> Frame {
        let mut pic = Picture::new(1, 1);
        pic.y[..2].copy_from_slice(&y);
        pic.u[0] = u;
        pic.v[0] = v;
        Frame {
            pic: Arc::new(pic),
            width: 2,
            height: 1,
        }
    }

    /// Studio black and white with neutral chroma land on 0 and 255, and BT.601 red
    /// (Y 81, Cb 90, Cr 240) on red.
    #[test]
    fn converts_studio_range_to_rgb() {
        let mut out = [0u8; 8];
        frame([16, 235], 128, 128).to_rgba(&mut out);
        assert_eq!(out, [0, 0, 0, 255, 255, 255, 255, 255]);
        // The rounded encoding of red decodes within a level of it.
        frame([81, 81], 90, 240).to_rgba(&mut out);
        assert!(out[0] >= 254 && out[1..4] == [0, 0, 255], "{out:?}");
    }
}
