//! The MPEG-4 Part 2 video decoder, for the subset the cinematics are encoded with (DivX 5.03,
//! Advanced Simple Profile): rectangular progressive VOPs, I/P/B, H.263 quantisation, half-pel
//! motion with one or four vectors, AC/DC prediction, and DivX's packed B-frames. A VOL that
//! asks for anything else (quarter-pel, GMC, interlace, MPEG quantisation, resync markers, data
//! partitioning, scalability, shapes, OBMC) is refused.

use std::fmt;
use std::sync::Arc;

use crate::bits::Bits;
use crate::idct;
use crate::mc::{self, Plane};
use crate::picture::{Frame, Picture};
use crate::tables::{self, Tcoef, ALT_HORIZONTAL, ALT_VERTICAL, ZIGZAG};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mpeg4: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

type Result<T> = std::result::Result<T, DecodeError>;

fn bad<T>(msg: impl Into<String>) -> Result<T> {
    Err(DecodeError(msg.into()))
}

const VOP_START: u8 = 0xb6;
const USER_DATA: u8 = 0xb2;
const VOS_START: u8 = 0xb0;

/// The `video_object_layer` fields this subset reads.
#[derive(Debug, Clone)]
struct Vol {
    width: usize,
    height: usize,
    mb_w: usize,
    mb_h: usize,
    time_resolution: i64,
    time_bits: u32,
    low_delay: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VopType {
    I,
    P,
    B,
}

/// Where each `00 00 01 xx` start code begins in `data`, as `(code, index after it)`.
fn start_codes(data: &[u8]) -> impl Iterator<Item = (u8, usize)> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i + 3 < data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                let found = (data[i + 3], i + 4);
                i += 4;
                return Some(found);
            }
            i += 1;
        }
        None
    })
}

fn parse_vol(data: &[u8]) -> Result<Vol> {
    let mut b = Bits::new(data);
    b.skip(1); // random_accessible_vol
    let object_type = b.read(8);
    let verid = if b.bit() {
        let v = b.read(4);
        b.skip(3); // priority
        v
    } else {
        1
    };
    if b.read(4) == 15 {
        b.skip(16); // par_width, par_height
    }
    // Without vol_control_parameters, Simple and Advanced Simple objects hold no B-VOPs.
    let mut low_delay = matches!(object_type, 1 | 17);
    if b.bit() {
        b.skip(2); // chroma_format
        low_delay = b.bit();
        if b.bit() {
            // vbv_parameters: 79 bits of rates and sizes with their markers.
            b.skip(79);
        }
    }
    if b.read(2) != 0 {
        return bad("only rectangular VOLs are supported");
    }
    b.skip(1);
    let time_resolution = b.read(16) as i64;
    b.skip(1);
    if time_resolution == 0 {
        return bad("vop_time_increment_resolution of 0");
    }
    let time_bits = (32 - ((time_resolution - 1) as u32).leading_zeros()).max(1);
    if b.bit() {
        b.skip(time_bits); // fixed_vop_time_increment
    }
    b.skip(1);
    let width = b.read(13) as usize;
    b.skip(1);
    let height = b.read(13) as usize;
    b.skip(1);
    if width == 0 || height == 0 {
        return bad("VOL of zero size");
    }
    if b.bit() {
        return bad("interlaced VOLs are not supported");
    }
    if !b.bit() {
        return bad("OBMC is not supported");
    }
    if b.read(if verid == 1 { 1 } else { 2 }) != 0 {
        return bad("sprites and GMC are not supported");
    }
    if b.bit() {
        return bad("not_8_bit is not supported");
    }
    if b.bit() {
        return bad("MPEG quantisation is not supported");
    }
    if verid != 1 && b.bit() {
        return bad("quarter-pel motion is not supported");
    }
    if !b.bit() {
        return bad("complexity estimation is not supported");
    }
    if !b.bit() {
        return bad("resync markers are not supported");
    }
    if b.bit() {
        return bad("data partitioning is not supported");
    }
    if verid != 1 {
        if b.bit() {
            return bad("newpred is not supported");
        }
        if b.bit() {
            return bad("reduced resolution VOPs are not supported");
        }
    }
    if b.bit() {
        return bad("scalability is not supported");
    }
    Ok(Vol {
        width,
        height,
        mb_w: width.div_ceil(16),
        mb_h: height.div_ceil(16),
        time_resolution,
        time_bits,
        low_delay,
    })
}

/// DivX's user data, `DivX503b1031` or `DivX503b2510p`: a trailing `p` marks packed B-frames,
/// a P-VOP and the B-VOP shown before it sharing one AVI chunk.
fn divx_packed(user_data: &[u8]) -> Option<bool> {
    let end = user_data
        .windows(3)
        .position(|w| w == [0, 0, 1])
        .unwrap_or(user_data.len());
    let s = std::str::from_utf8(&user_data[..end]).ok()?;
    let rest = s.strip_prefix("DivX")?;
    let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
    let v = digits(rest);
    let rest = &rest[v..];
    let rest = rest
        .strip_prefix("Build")
        .or_else(|| rest.strip_prefix('b'))?;
    let n = digits(rest);
    (v > 0 && n > 0).then(|| rest[n..].starts_with('p'))
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

/// Per-block intra prediction state: the DC (dequantised, `1024` where unavailable) and the
/// first column (`[1..8]`) and first row (`[9..16]`) of quantised levels.
#[derive(Clone, Copy)]
struct IntraPred {
    dc: i32,
    ac: [i16; 16],
}

const NO_INTRA: IntraPred = IntraPred {
    dc: 1024,
    ac: [0; 16],
};

/// Timing that B-VOP direct mode reads (7.6.9.5): `TRD` and `TRB` in ticks.
#[derive(Default)]
struct Clock {
    time_base: i64,
    last_time_base: i64,
    last_non_b: i64,
    pp: i64,
    pb: i64,
}

/// One macroblock's motion for the prediction step.
#[derive(Clone, Copy)]
enum Motion {
    /// One vector for the macroblock.
    One([i32; 2]),
    /// One per 8×8 luma block.
    Four([[i32; 2]; 4]),
}

/// Decodes the cinematics' video packets, one AVI chunk at a time.
pub struct Decoder {
    vol: Option<Vol>,
    packed: bool,
    /// The B-VOP that rode in a packed chunk behind its P-VOP, decoded on the next chunk.
    pending: Option<Vec<u8>>,
    /// The two most recent I/P pictures: `[past, future]` for a B-VOP, `[_, previous]` for a
    /// P-VOP.
    refs: [Option<Arc<Picture>>; 2],
    clock: Clock,
    intra_luma: Vec<IntraPred>,
    intra_chroma: [Vec<IntraPred>; 2],
    mb_qscale: Vec<i32>,
    /// Per-chunk scratch for a B-VOP's forward and backward vectors.
    last_mv: [[i32; 2]; 2],
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            vol: None,
            packed: false,
            pending: None,
            refs: [None, None],
            clock: Clock::default(),
            intra_luma: Vec::new(),
            intra_chroma: [Vec::new(), Vec::new()],
            mb_qscale: Vec::new(),
            last_mv: [[0; 2]; 2],
        }
    }

    /// Displayed size, once a VOL has been read.
    pub fn size(&self) -> Option<(usize, usize)> {
        self.vol.as_ref().map(|v| (v.width, v.height))
    }

    /// How many chunks a frame shows after the chunk that carries it: 1 in a stream with B-VOPs,
    /// whose references show one chunk late, else 0.
    pub fn delay(&self) -> usize {
        self.vol.as_ref().map_or(0, |v| usize::from(!v.low_delay))
    }

    /// Decodes one AVI chunk and returns the frame it completes in display order, if any: none
    /// for a not-coded VOP, and none for the first reference of a stream that holds B-VOPs,
    /// which shows one chunk late.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<Frame>> {
        if let Some(pending) = self.pending.take() {
            // A stream restart in this chunk drops the held B-VOP.
            let restart = start_codes(packet).any(|(c, _)| c == VOS_START);
            if !restart && self.packed {
                return self.decode_buffer(&pending);
            }
        }
        self.decode_buffer(packet)
    }

    /// The frame still held at the end of a stream with B-VOPs.
    pub fn flush(&mut self) -> Option<Frame> {
        let vol = self.vol.as_ref()?;
        if vol.low_delay {
            return None;
        }
        let pic = self.refs[1].take()?;
        Some(Frame {
            pic,
            width: vol.width,
            height: vol.height,
        })
    }

    fn decode_buffer(&mut self, data: &[u8]) -> Result<Option<Frame>> {
        let codes: Vec<(u8, usize)> = start_codes(data).collect();
        for (n, &(code, at)) in codes.iter().enumerate() {
            let end = codes.get(n + 1).map_or(data.len(), |&(_, next)| next - 4);
            match code {
                0x20..=0x2f => {
                    let vol = parse_vol(&data[at..end])?;
                    if self
                        .vol
                        .as_ref()
                        .is_none_or(|v| (v.width, v.height) != (vol.width, vol.height))
                    {
                        self.refs = [None, None];
                        let blocks = vol.mb_w * vol.mb_h;
                        self.intra_luma = vec![NO_INTRA; blocks * 4];
                        self.intra_chroma = [vec![NO_INTRA; blocks], vec![NO_INTRA; blocks]];
                        self.mb_qscale = vec![0; blocks];
                    }
                    self.vol = Some(vol);
                }
                USER_DATA => {
                    if let Some(packed) = divx_packed(&data[at..end]) {
                        self.packed = packed;
                    }
                }
                VOP_START => return self.decode_vop(data, at),
                _ => {}
            }
        }
        Ok(None)
    }

    fn decode_vop(&mut self, data: &[u8], at: usize) -> Result<Option<Frame>> {
        let Some(vol) = self.vol.clone() else {
            return bad("VOP before any VOL");
        };
        let mut b = Bits::new(&data[at..]);
        let kind = match b.read(2) {
            0 => VopType::I,
            1 => VopType::P,
            2 => VopType::B,
            _ => return bad("S-VOPs are not supported"),
        };
        let mut modulo = 0;
        while b.bit() {
            modulo += 1;
            if b.left() <= 0 {
                return bad("truncated VOP header");
            }
        }
        b.skip(1);
        let increment = i64::from(b.read(vol.time_bits));
        b.skip(1);
        let c = &mut self.clock;
        if kind == VopType::B {
            let time = (c.last_time_base + modulo) * vol.time_resolution + increment;
            c.pb = c.pp - (c.last_non_b - time);
            if c.pp <= c.pb || c.pp <= c.pp - c.pb || c.pp <= 0 {
                // A B-VOP outside its references' interval cannot be predicted.
                return Ok(None);
            }
        } else {
            c.last_time_base = c.time_base;
            c.time_base += modulo;
            let time = c.time_base * vol.time_resolution + increment;
            c.pp = time - c.last_non_b;
            c.last_non_b = time;
        }
        if !b.bit() {
            // vop_coded 0: nothing new to show.
            return Ok(None);
        }
        let round_down = kind == VopType::P && b.bit();
        let dc_threshold = tables::DC_THRESHOLD[b.read(3) as usize];
        let mut qscale = b.read(5) as i32;
        if qscale == 0 {
            return bad("vop_quant of 0");
        }
        let f_code = if kind != VopType::I { b.read(3) } else { 1 };
        let b_code = if kind == VopType::B { b.read(3) } else { 1 };
        if f_code == 0 || b_code == 0 {
            return bad("fcode of 0");
        }

        if kind != VopType::I && self.refs[1].is_none() {
            return bad("predicted VOP without a reference");
        }
        if kind == VopType::B && self.refs[0].is_none() {
            // A B-VOP before two references (a stream cut at a B-VOP) is dropped.
            return Ok(None);
        }

        let mut pic = Picture::new(vol.mb_w, vol.mb_h);
        let mut ctx = Vop {
            vol: &vol,
            kind,
            round_down,
            dc_threshold,
            f_code,
            b_code,
            older: self.refs[0].as_deref(),
            newer: self.refs[1].as_deref(),
            pic: &mut pic,
            intra_luma: &mut self.intra_luma,
            intra_chroma: &mut self.intra_chroma,
            mb_qscale: &mut self.mb_qscale,
            last_mv: &mut self.last_mv,
            clock: &self.clock,
        };
        for my in 0..vol.mb_h {
            for mx in 0..vol.mb_w {
                ctx.macroblock(&mut b, mx, my, &mut qscale)
                    .map_err(|e| DecodeError(format!("{} at mb ({mx}, {my})", e.0)))?;
            }
        }

        if self.packed && kind != VopType::B {
            // Packed B-frames: another I- or B-VOP in the rest of this chunk is held for the next.
            let from = at + b.byte_pos();
            if data.len().saturating_sub(from) > 7 {
                let rest = &data[from..];
                if let Some(i) = rest.windows(4).position(|w| w == [0, 0, 1, VOP_START]) {
                    if rest.get(i + 4).is_some_and(|&t| t & 0x40 == 0) {
                        self.pending = Some(rest.to_vec());
                    }
                }
            }
        }

        let pic = Arc::new(pic);
        let frame = |pic: Arc<Picture>| Frame {
            pic,
            width: vol.width,
            height: vol.height,
        };
        Ok(match kind {
            VopType::B => Some(frame(pic)),
            _ => {
                let previous = self.refs[1].replace(pic.clone());
                self.refs[0] = previous.clone();
                if vol.low_delay {
                    Some(frame(pic))
                } else {
                    previous.map(frame)
                }
            }
        })
    }
}

/// The state of one VOP being decoded.
struct Vop<'a> {
    vol: &'a Vol,
    kind: VopType,
    round_down: bool,
    dc_threshold: i32,
    f_code: u32,
    b_code: u32,
    /// The past reference of a B-VOP.
    older: Option<&'a Picture>,
    /// The reference of a P-VOP; the future reference of a B-VOP.
    newer: Option<&'a Picture>,
    pic: &'a mut Picture,
    intra_luma: &'a mut [IntraPred],
    intra_chroma: &'a mut [Vec<IntraPred>; 2],
    mb_qscale: &'a mut [i32],
    last_mv: &'a mut [[i32; 2]; 2],
    clock: &'a Clock,
}

fn set_qscale(q: &mut i32, delta: i32) {
    *q = (*q + delta).clamp(1, 31);
}

/// `motion_code` and `motion_residual` against `pred` (7.6.3), wrapped to the `f_code` range.
fn decode_motion(b: &mut Bits, pred: i32, f_code: u32) -> Result<i32> {
    let t = tables::tables();
    let Some(code) = t.mv.read(b) else {
        return bad("bad motion_code");
    };
    if code == 0 {
        return Ok(pred);
    }
    let negative = b.bit();
    let shift = f_code - 1;
    let mut val = code as i32;
    if shift > 0 {
        val = ((val - 1) << shift | b.read(shift) as i32) + 1;
    }
    if negative {
        val = -val;
    }
    let bits = 5 + f_code;
    let v = pred + val;
    Ok((v << (32 - bits)) >> (32 - bits))
}

impl Vop<'_> {
    fn mb_index(&self, mx: usize, my: usize) -> usize {
        my * self.vol.mb_w + mx
    }

    fn mv_at(&self, bx: isize, by: isize) -> [i32; 2] {
        let gw = 2 * self.vol.mb_w as isize;
        if bx < 0 || by < 0 || bx >= gw {
            return [0, 0];
        }
        self.pic.mv[(by * gw + bx) as usize]
    }

    /// The predicted vector of luma block `n` (7.6.5): the median of left, above and above
    /// right, with the first row predicting from the left alone.
    fn predict_mv(&self, mx: usize, my: usize, n: usize) -> [i32; 2] {
        let bx = (2 * mx + (n & 1)) as isize;
        let by = (2 * my + (n >> 1)) as isize;
        let a = self.mv_at(bx - 1, by);
        if my == 0 && n < 2 {
            return if n == 0 && mx == 0 { [0, 0] } else { a };
        }
        let off = [2, 1, 1, -1][n];
        let b = self.mv_at(bx, by - 1);
        let c = self.mv_at(bx + off, by - 1);
        [median(a[0], b[0], c[0]), median(a[1], b[1], c[1])]
    }

    fn set_mv(&mut self, mx: usize, my: usize, n: usize, v: [i32; 2]) {
        let gw = 2 * self.vol.mb_w;
        self.pic.mv[(2 * my + (n >> 1)) * gw + 2 * mx + (n & 1)] = v;
    }

    fn macroblock(&mut self, b: &mut Bits, mx: usize, my: usize, qscale: &mut i32) -> Result<()> {
        if b.left() < 0 {
            return bad("VOP ends before its macroblocks");
        }
        match self.kind {
            VopType::I => self.intra_mb_i(b, mx, my, qscale),
            VopType::P => self.mb_p(b, mx, my, qscale),
            VopType::B => self.mb_b(b, mx, my, qscale),
        }
    }

    fn intra_mb_i(&mut self, b: &mut Bits, mx: usize, my: usize, qscale: &mut i32) -> Result<()> {
        let t = tables::tables();
        let mcbpc = loop {
            match t.mcbpc_i.read(b) {
                Some(8) => continue,
                Some(s) => break s,
                None => return bad("bad I mcbpc"),
            }
        };
        self.intra_mb(b, mx, my, qscale, mcbpc & 3, mcbpc >= 4)
    }

    fn intra_mb(
        &mut self,
        b: &mut Bits,
        mx: usize,
        my: usize,
        qscale: &mut i32,
        cbpc: usize,
        dquant: bool,
    ) -> Result<()> {
        let t = tables::tables();
        let ac_pred = b.bit();
        let Some(cbpy) = t.cbpy.read(b) else {
            return bad("bad intra cbpy");
        };
        let cbp = (cbpy << 2) | cbpc;
        // The DC VLC switch reads the QP before this macroblock's own dquant.
        let use_dc_vlc = *qscale < self.dc_threshold;
        if dquant {
            set_qscale(qscale, tables::DQUANT[b.read(2) as usize]);
        }
        let mbi = self.mb_index(mx, my);
        self.mb_qscale[mbi] = *qscale;
        for n in 0..6 {
            let mut block = [0i16; 64];
            self.intra_block(
                b,
                mx,
                my,
                n,
                cbp & (32 >> n) != 0,
                use_dc_vlc,
                ac_pred,
                *qscale,
                &mut block,
            )?;
            let (plane, stride, off) = self.block_dst(mx, my, n);
            idct::put(&mut block, &mut plane[off..], stride);
        }
        for n in 0..4 {
            self.set_mv(mx, my, n, [0, 0]);
        }
        self.pic.skipped[mbi] = false;
        self.pic.four_mv[mbi] = false;
        Ok(())
    }

    /// Plane, stride and offset of block `n` of macroblock `(mx, my)` in the picture.
    fn block_dst(&mut self, mx: usize, my: usize, n: usize) -> (&mut [u8], usize, usize) {
        let w = self.pic.w;
        match n {
            0..=3 => (
                &mut self.pic.y,
                w,
                (my * 16 + (n >> 1) * 8) * w + mx * 16 + (n & 1) * 8,
            ),
            4 => (&mut self.pic.u, w / 2, my * 8 * (w / 2) + mx * 8),
            _ => (&mut self.pic.v, w / 2, my * 8 * (w / 2) + mx * 8),
        }
    }

    /// The intra prediction cell of block `n` of `(mx, my)`, offset by `(dx, dy)` blocks, or
    /// `None` outside the picture.
    fn intra_cell(
        &self,
        mx: usize,
        my: usize,
        n: usize,
        dx: isize,
        dy: isize,
    ) -> Option<IntraPred> {
        if n < 4 {
            let gw = 2 * self.vol.mb_w as isize;
            let bx = (2 * mx + (n & 1)) as isize + dx;
            let by = (2 * my + (n >> 1)) as isize + dy;
            (bx >= 0 && by >= 0 && bx < gw).then(|| self.intra_luma[(by * gw + bx) as usize])
        } else {
            let bx = mx as isize + dx;
            let by = my as isize + dy;
            (bx >= 0 && by >= 0 && bx < self.vol.mb_w as isize)
                .then(|| self.intra_chroma[n - 4][(by * self.vol.mb_w as isize + bx) as usize])
        }
    }

    fn intra_cell_mut(&mut self, mx: usize, my: usize, n: usize) -> &mut IntraPred {
        if n < 4 {
            let gw = 2 * self.vol.mb_w;
            &mut self.intra_luma[(2 * my + (n >> 1)) * gw + 2 * mx + (n & 1)]
        } else {
            let w = self.vol.mb_w;
            &mut self.intra_chroma[n - 4][my * w + mx]
        }
    }

    /// The macroblock holding the block left of (`left`) or above block `n`, when that block
    /// lies in another macroblock.
    fn neighbour_mb(&self, mx: usize, my: usize, n: usize, left: bool) -> Option<usize> {
        if left {
            let inside = n == 1 || n == 3;
            (!inside && mx > 0).then(|| self.mb_index(mx - 1, my))
        } else {
            let inside = n == 2 || n == 3;
            (!inside && my > 0).then(|| self.mb_index(mx, my - 1))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn intra_block(
        &mut self,
        b: &mut Bits,
        mx: usize,
        my: usize,
        n: usize,
        coded: bool,
        use_dc_vlc: bool,
        ac_pred: bool,
        qscale: i32,
        block: &mut [i16; 64],
    ) -> Result<()> {
        let t = tables::tables();
        let scale = if n < 4 {
            tables::dc_scale_luma(qscale)
        } else {
            tables::dc_scale_chroma(qscale)
        };
        // DC prediction (7.4.3.1): from the left or above, whichever gradient is smaller.
        let a = self.intra_cell(mx, my, n, -1, 0).unwrap_or(NO_INTRA);
        let bb = self.intra_cell(mx, my, n, -1, -1).unwrap_or(NO_INTRA);
        let c = self.intra_cell(mx, my, n, 0, -1).unwrap_or(NO_INTRA);
        let from_top = (a.dc - bb.dc).abs() < (bb.dc - c.dc).abs();
        let (pred_dc, pred_from) = if from_top { (c.dc, c) } else { (a.dc, a) };
        let pred_dc = (pred_dc + (scale >> 1)) / scale;

        let mut dc = 0i32;
        let mut pos: i32 = if use_dc_vlc {
            let vlc = if n < 4 { &t.dc_luma } else { &t.dc_chroma };
            let Some(size) = vlc.read(b) else {
                return bad("bad dct_dc_size");
            };
            if size > 0 {
                let size = size as u32;
                let v = b.read(size) as i32;
                dc = if v >> (size - 1) == 0 {
                    v - (1 << size) + 1
                } else {
                    v
                };
                if size > 8 {
                    b.skip(1); // marker
                }
            }
            0
        } else {
            -1
        };
        if coded {
            let scan = match (ac_pred, from_top) {
                (false, _) => &ZIGZAG,
                (true, false) => &ALT_VERTICAL,
                (true, true) => &ALT_HORIZONTAL,
            };
            read_coefficients(b, &t.intra, scan, &mut pos, block, 1, 0)?;
        }
        if !use_dc_vlc {
            dc = i32::from(block[0]);
        }
        let level = dc + pred_dc;
        block[0] = level as i16;

        // AC prediction (7.4.3.3): the first column from the left or the first row from above,
        // rescaled when that macroblock's QP differs.
        if ac_pred {
            let nb = self.neighbour_mb(mx, my, n, !from_top);
            let rescale = |v: i16| -> i16 {
                match nb {
                    Some(m) if self.mb_qscale[m] != qscale => {
                        let p = i32::from(v) * self.mb_qscale[m];
                        let half = qscale >> 1;
                        ((if p > 0 { p + half } else { p - half }) / qscale) as i16
                    }
                    _ => v,
                }
            };
            for i in 1..8 {
                if from_top {
                    block[i] = block[i].wrapping_add(rescale(pred_from.ac[8 + i]));
                } else {
                    block[i << 3] = block[i << 3].wrapping_add(rescale(pred_from.ac[i]));
                }
            }
        }
        let cell = self.intra_cell_mut(mx, my, n);
        let mut stored = level * scale;
        if stored & !2047 != 0 {
            stored = if stored < 0 { 0 } else { 2047 };
        }
        cell.dc = stored;
        for i in 1..8 {
            cell.ac[i] = block[i << 3];
            cell.ac[8 + i] = block[i];
        }

        // H.263 inverse quantisation (7.4.4.2).
        block[0] = (level * scale) as i16;
        let qmul = qscale << 1;
        let qadd = (qscale - 1) | 1;
        for c in block[1..].iter_mut() {
            let l = i32::from(*c);
            if l != 0 {
                *c = if l < 0 {
                    l * qmul - qadd
                } else {
                    l * qmul + qadd
                } as i16;
            }
        }
        Ok(())
    }

    /// A non-intra macroblock's prediction cells read as unavailable by later intra blocks.
    fn clear_intra(&mut self, mx: usize, my: usize, qscale: i32) {
        for n in 0..6 {
            *self.intra_cell_mut(mx, my, n) = NO_INTRA;
        }
        let i = self.mb_index(mx, my);
        self.mb_qscale[i] = qscale;
    }

    fn mb_p(&mut self, b: &mut Bits, mx: usize, my: usize, qscale: &mut i32) -> Result<()> {
        let t = tables::tables();
        let mbi = self.mb_index(mx, my);
        let mcbpc = loop {
            if b.bit() {
                // not_coded: the reference's macroblock, unmoved.
                for n in 0..4 {
                    self.set_mv(mx, my, n, [0, 0]);
                }
                self.pic.skipped[mbi] = true;
                self.pic.four_mv[mbi] = false;
                self.clear_intra(mx, my, *qscale);
                let reference = self.newer.expect("checked");
                self.predict_mb(
                    reference,
                    mx,
                    my,
                    Motion::One([0, 0]),
                    false,
                    self.round_down,
                );
                return Ok(());
            }
            match t.mcbpc_p.read(b) {
                Some(20) => continue,
                Some(s) => break s,
                None => return bad("bad P mcbpc"),
            }
        };
        let (ty, cbpc) = (mcbpc >> 2, mcbpc & 3);
        if ty >= 3 {
            return self.intra_mb(b, mx, my, qscale, cbpc, ty == 4);
        }
        let Some(cbpy) = t.cbpy.read(b) else {
            return bad("bad P cbpy");
        };
        let cbp = ((cbpy ^ 15) << 2) | cbpc;
        if ty == 1 {
            set_qscale(qscale, tables::DQUANT[b.read(2) as usize]);
        }
        let motion = if ty == 2 {
            let mut v = [[0; 2]; 4];
            for (n, v) in v.iter_mut().enumerate() {
                let p = self.predict_mv(mx, my, n);
                *v = [
                    decode_motion(b, p[0], self.f_code)?,
                    decode_motion(b, p[1], self.f_code)?,
                ];
                self.set_mv(mx, my, n, *v);
            }
            Motion::Four(v)
        } else {
            let p = self.predict_mv(mx, my, 0);
            let v = [
                decode_motion(b, p[0], self.f_code)?,
                decode_motion(b, p[1], self.f_code)?,
            ];
            for n in 0..4 {
                self.set_mv(mx, my, n, v);
            }
            Motion::One(v)
        };
        self.pic.skipped[mbi] = false;
        self.pic.four_mv[mbi] = ty == 2;
        self.clear_intra(mx, my, *qscale);
        let reference = self.newer.expect("checked");
        self.predict_mb(reference, mx, my, motion, false, self.round_down);
        self.inter_residual(b, mx, my, cbp, *qscale)
    }

    /// Decodes and adds the six blocks' residuals of an inter macroblock.
    fn inter_residual(
        &mut self,
        b: &mut Bits,
        mx: usize,
        my: usize,
        cbp: usize,
        qscale: i32,
    ) -> Result<()> {
        let t = tables::tables();
        let qmul = qscale << 1;
        let qadd = (qscale - 1) | 1;
        for n in 0..6 {
            if cbp & (32 >> n) == 0 {
                continue;
            }
            let mut block = [0i16; 64];
            let mut pos = -1;
            read_coefficients(b, &t.inter, &ZIGZAG, &mut pos, &mut block, qmul, qadd)?;
            let (plane, stride, off) = self.block_dst(mx, my, n);
            idct::add(&mut block, &mut plane[off..], stride);
        }
        Ok(())
    }

    /// Writes (or with `average`, averages in) the prediction of macroblock `(mx, my)` from
    /// `src`.
    fn predict_mb(
        &mut self,
        src: &Picture,
        mx: usize,
        my: usize,
        motion: Motion,
        average: bool,
        round_down: bool,
    ) {
        let w = self.pic.w;
        let (cw, ch) = (src.w / 2, src.h / 2);
        let luma = Plane {
            data: &src.y,
            w: src.w,
            h: src.h,
        };
        let (cx, cy, chx, chy) = match motion {
            Motion::One(v) => {
                let x = (mx * 16) as isize + (v[0] >> 1) as isize;
                let y = (my * 16) as isize + (v[1] >> 1) as isize;
                let off = my * 16 * w + mx * 16;
                mc::predict(
                    luma,
                    x,
                    y,
                    v[0] & 1 != 0,
                    v[1] & 1 != 0,
                    round_down,
                    16,
                    16,
                    &mut self.pic.y[off..],
                    w,
                    average,
                );
                // The chroma vector is half the luma one, a quarter pel rounding to a half.
                (x >> 1, y >> 1, v[0] & 3 != 0, v[1] & 3 != 0)
            }
            Motion::Four(vs) => {
                let (width, height) = (self.vol.width as isize, self.vol.height as isize);
                let mut sum = [0, 0];
                for (n, v) in vs.iter().enumerate() {
                    let mut hx = v[0] & 1 != 0;
                    let mut hy = v[1] & 1 != 0;
                    let x =
                        ((mx * 16 + (n & 1) * 8) as isize + (v[0] >> 1) as isize).clamp(-16, width);
                    let y = ((my * 16 + (n >> 1) * 8) as isize + (v[1] >> 1) as isize)
                        .clamp(-16, height);
                    hx &= x != width;
                    hy &= y != height;
                    let off = (my * 16 + (n >> 1) * 8) * w + mx * 16 + (n & 1) * 8;
                    mc::predict(
                        luma,
                        x,
                        y,
                        hx,
                        hy,
                        round_down,
                        8,
                        8,
                        &mut self.pic.y[off..],
                        w,
                        average,
                    );
                    sum[0] += v[0];
                    sum[1] += v[1];
                }
                let c = [mc::chroma_from_sum(sum[0]), mc::chroma_from_sum(sum[1])];
                let (cwi, chi) = (
                    (self.vol.width >> 1) as isize,
                    (self.vol.height >> 1) as isize,
                );
                let x = ((mx * 8) as isize + (c[0] >> 1) as isize).clamp(-8, cwi);
                let y = ((my * 8) as isize + (c[1] >> 1) as isize).clamp(-8, chi);
                (x, y, c[0] & 1 != 0 && x != cwi, c[1] & 1 != 0 && y != chi)
            }
        };
        let off = my * 8 * (w / 2) + mx * 8;
        for (src, dst) in [(&src.u, &mut self.pic.u), (&src.v, &mut self.pic.v)] {
            let plane = Plane {
                data: src,
                w: cw,
                h: ch,
            };
            mc::predict(
                plane,
                cx,
                cy,
                chx,
                chy,
                round_down,
                8,
                8,
                &mut dst[off..],
                w / 2,
                average,
            );
        }
    }

    fn mb_b(&mut self, b: &mut Bits, mx: usize, my: usize, qscale: &mut i32) -> Result<()> {
        let t = tables::tables();
        let older = self.older.expect("checked");
        let newer = self.newer.expect("checked");
        let mbi = self.mb_index(mx, my);
        if mx == 0 {
            *self.last_mv = [[0; 2]; 2];
        }
        if newer.skipped[mbi] {
            // Skipped in the future reference: skipped here, copied from the past one.
            self.predict_mb(older, mx, my, Motion::One([0, 0]), false, false);
            return Ok(());
        }
        // modb `1`: direct with no delta and no texture; `01`: a type and no texture.
        let (mb_type, cbp, delta) = if b.bit() {
            (0, 0, false)
        } else {
            let no_cbp = b.bit();
            let Some(ty) = t.mb_type_b.read(b) else {
                return bad("bad B mb_type");
            };
            let cbp = if no_cbp { 0 } else { b.read(6) as usize };
            if ty != 0 && cbp != 0 && b.bit() {
                set_qscale(qscale, if b.bit() { 2 } else { -2 });
            }
            (ty, cbp, ty == 0)
        };
        // Types: 0 direct, 1 bidirectional, 2 backward, 3 forward.
        let (fwd, bwd) = if mb_type == 0 {
            let d = if delta {
                [decode_motion(b, 0, 1)?, decode_motion(b, 0, 1)?]
            } else {
                [0, 0]
            };
            let (f, w) = self.direct_mvs(mx, my, d);
            (Some(f), Some(w))
        } else {
            let mut fwd = None;
            let mut bwd = None;
            if mb_type == 1 || mb_type == 3 {
                let p = self.last_mv[0];
                let v = [
                    decode_motion(b, p[0], self.f_code)?,
                    decode_motion(b, p[1], self.f_code)?,
                ];
                self.last_mv[0] = v;
                fwd = Some(Motion::One(v));
            }
            if mb_type == 1 || mb_type == 2 {
                let p = self.last_mv[1];
                let v = [
                    decode_motion(b, p[0], self.b_code)?,
                    decode_motion(b, p[1], self.b_code)?,
                ];
                self.last_mv[1] = v;
                bwd = Some(Motion::One(v));
            }
            (fwd, bwd)
        };
        if let Some(m) = fwd {
            self.predict_mb(older, mx, my, m, false, false);
        }
        if let Some(m) = bwd {
            self.predict_mb(newer, mx, my, m, fwd.is_some(), false);
        }
        self.inter_residual(b, mx, my, cbp, *qscale)
    }

    /// Direct mode's vectors (7.6.9.5.2): the future reference's co-located vector `v` gives
    /// `v·TRB/TRD + delta` forward and, backward, `forward − v` with a delta, or else
    /// `v·(TRB−TRD)/TRD`; per 8×8 block where that macroblock had four vectors.
    fn direct_mvs(&self, mx: usize, my: usize, delta: [i32; 2]) -> (Motion, Motion) {
        let newer = self.newer.expect("checked");
        let (pp, pb) = (self.clock.pp as i32, self.clock.pb as i32);
        let gw = 2 * self.vol.mb_w;
        let one = |n: usize| {
            let v = newer.mv[(2 * my + (n >> 1)) * gw + 2 * mx + (n & 1)];
            let mut f = [0; 2];
            let mut w = [0; 2];
            for c in 0..2 {
                f[c] = v[c] * pb / pp + delta[c];
                w[c] = if delta[c] != 0 {
                    f[c] - v[c]
                } else {
                    v[c] * (pb - pp) / pp
                };
            }
            (f, w)
        };
        if newer.four_mv[self.mb_index(mx, my)] {
            let mut f = [[0; 2]; 4];
            let mut w = [[0; 2]; 4];
            for n in 0..4 {
                (f[n], w[n]) = one(n);
            }
            (Motion::Four(f), Motion::Four(w))
        } else {
            let (f, w) = one(0);
            (Motion::One(f), Motion::One(w))
        }
    }
}

/// Reads one block's run-level pairs (7.4.1) from `pos` (the last position written, −1 for
/// none) into `block` in `scan` order, dequantising each level as `level·qmul ± qadd`.
fn read_coefficients(
    b: &mut Bits,
    table: &Tcoef,
    scan: &[u8; 64],
    pos: &mut i32,
    block: &mut [i16; 64],
    qmul: i32,
    qadd: i32,
) -> Result<()> {
    let deq = |l: i32| {
        if l > 0 {
            l * qmul + qadd
        } else {
            l * qmul - qadd
        }
    };
    // A coded pair's level is a magnitude with a sign bit after it.
    let signed = |b: &mut Bits, l: i32| if b.bit() { -l } else { l };
    loop {
        let Some(sym) = table.vlc.read(b) else {
            return bad("bad texture code");
        };
        let (last, run, level) = if sym != table.escape {
            let (last, run, level) = table.entries[sym];
            (last, i32::from(run), signed(b, deq(i32::from(level))))
        } else if !b.bit() {
            // Escape 1: the level beyond the largest this run can code.
            let Some(sym) = table.vlc.read(b).filter(|&s| s != table.escape) else {
                return bad("bad escape 1 code");
            };
            let (last, run, level) = table.entries[sym];
            let extra = table.max_level[usize::from(last)][usize::from(run)];
            let level = deq(i32::from(level) + i32::from(extra));
            (last, i32::from(run), signed(b, level))
        } else if !b.bit() {
            // Escape 2: the run beyond the longest this level can code.
            let Some(sym) = table.vlc.read(b).filter(|&s| s != table.escape) else {
                return bad("bad escape 2 code");
            };
            let (last, run, level) = table.entries[sym];
            let extra = table.max_run[usize::from(last)][usize::from(level)];
            let run = i32::from(run) + i32::from(extra) + 1;
            (last, run, signed(b, deq(i32::from(level))))
        } else {
            // Escape 3: last, run and a signed level spelled out between markers.
            let last = b.bit();
            let run = b.read(6) as i32;
            b.skip(1);
            let level = b.read_signed(12);
            b.skip(1);
            (last, run, deq(level).clamp(-2048, 2047))
        };
        *pos += run + 1;
        if *pos > 63 || (!last && *pos > 62) {
            return bad("texture runs past the block");
        }
        block[usize::from(scan[*pos as usize])] = level as i16;
        if last {
            return Ok(());
        }
    }
}
