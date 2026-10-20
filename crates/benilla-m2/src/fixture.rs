//! Synthetic version-264 models for tests: a header with every `M2Array` empty, records appended
//! and pointed at, and a matching skin profile. Built under the `fixture` feature.

pub use crate::timeline::h264 as header;
use crate::{AnimFiles, M2Model};

/// A version-264 file under construction: the header, then appended arrays.
pub struct Model {
    pub b: Vec<u8>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    /// The header through the texture combiner ops, every array empty.
    pub fn new() -> Self {
        let mut b = vec![0u8; 0x138];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&264u32.to_le_bytes());
        Self { b }
    }

    /// Append `bytes`; their offset.
    pub fn data(&mut self, bytes: &[u8]) -> u32 {
        let at = self.b.len() as u32;
        self.b.extend_from_slice(bytes);
        at
    }

    /// Write `(count, ofs)` at `at`.
    pub fn put_arr(&mut self, at: usize, count: usize, ofs: u32) {
        self.b[at..at + 4].copy_from_slice(&(count as u32).to_le_bytes());
        self.b[at + 4..at + 8].copy_from_slice(&ofs.to_le_bytes());
    }

    /// Append `count` records of `bytes` and point header `at` at them.
    pub fn array(&mut self, at: usize, count: usize, bytes: &[u8]) {
        let ofs = self.data(bytes);
        self.put_arr(at, count, ofs);
    }

    /// Set the global flags (`+0x10`).
    pub fn flags(&mut self, flags: u32) {
        self.b[0x10..0x14].copy_from_slice(&flags.to_le_bytes());
    }

    /// A track's `0x14`-byte header with `sets` per-sequence sets of `(ms, value bytes)`.
    pub fn track(&mut self, interp: u16, gseq: u16, sets: &[&[(u32, Vec<u8>)]]) -> Vec<u8> {
        let (mut time_heads, mut value_heads) = (Vec::new(), Vec::new());
        for set in sets {
            let times: Vec<u8> = set.iter().flat_map(|(t, _)| t.to_le_bytes()).collect();
            let values: Vec<u8> = set.iter().flat_map(|(_, v)| v.clone()).collect();
            let (to, vo) = (self.data(&times), self.data(&values));
            time_heads.extend_from_slice(&(set.len() as u32).to_le_bytes());
            time_heads.extend_from_slice(&to.to_le_bytes());
            value_heads.extend_from_slice(&(set.len() as u32).to_le_bytes());
            value_heads.extend_from_slice(&vo.to_le_bytes());
        }
        let (th, vh) = (self.data(&time_heads), self.data(&value_heads));
        let mut out = interp.to_le_bytes().to_vec();
        out.extend_from_slice(&gseq.to_le_bytes());
        for (count, ofs) in [(sets.len(), th), (sets.len(), vh)] {
            out.extend_from_slice(&(count as u32).to_le_bytes());
            out.extend_from_slice(&ofs.to_le_bytes());
        }
        out
    }

    /// An empty track (`interp` 0, no global sequence, no sets).
    pub fn no_track(&mut self) -> Vec<u8> {
        self.track(0, 0xffff, &[])
    }

    /// An event's `0x0c`-byte `M2TrackBase` with `sets` per-sequence sets of times.
    pub fn track_base(&mut self, gseq: u16, sets: &[&[u32]]) -> Vec<u8> {
        let mut heads = Vec::new();
        for set in sets {
            let times: Vec<u8> = set.iter().flat_map(|t| t.to_le_bytes()).collect();
            let at = self.data(&times);
            heads.extend_from_slice(&(set.len() as u32).to_le_bytes());
            heads.extend_from_slice(&at.to_le_bytes());
        }
        let h = self.data(&heads);
        let mut out = 0u16.to_le_bytes().to_vec();
        out.extend_from_slice(&gseq.to_le_bytes());
        out.extend_from_slice(&(sets.len() as u32).to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out
    }

    /// Parse with `skin` and no `.anim` files.
    pub fn parse(&self, skin: &[u8]) -> crate::Result<M2Model> {
        crate::parse_m2(self.b.clone(), skin, AnimFiles::new())
    }
}

/// Little-endian `f32`s.
pub fn floats(fs: &[f32]) -> Vec<u8> {
    fs.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Little-endian `u16`s.
pub fn shorts(vs: &[u16]) -> Vec<u8> {
    vs.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A rotation key as four `i16`s, the inverse of the `M2CompQuat` unpack.
pub fn packed_quat(q: [f32; 4]) -> Vec<u8> {
    q.iter()
        .flat_map(|&c| {
            let v = (c * 32767.0).round() as i32;
            let i = if v > 0 { v - 32768 } else { v + 32767 };
            (i as i16).to_le_bytes()
        })
        .collect()
}

/// A sequence record (`0x40` bytes): id, length and flags; `0x20` keeps its keys in the `.m2`.
pub fn sequence(id: u16, length: u32, flags: u32) -> Vec<u8> {
    let mut r = vec![0u8; 0x40];
    r[0..2].copy_from_slice(&id.to_le_bytes());
    r[4..8].copy_from_slice(&length.to_le_bytes());
    r[0x0c..0x10].copy_from_slice(&flags.to_le_bytes());
    r[0x3c..0x3e].copy_from_slice(&(-1i16).to_le_bytes());
    r
}

/// A bone record (`0x58` bytes) with its three `0x14`-byte tracks.
pub fn bone(parent: i16, flags: u32, pivot: [f32; 3], tracks: [&[u8]; 3]) -> Vec<u8> {
    let mut b = (-1i32).to_le_bytes().to_vec();
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&parent.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    for t in tracks {
        assert_eq!(t.len(), 0x14);
        b.extend_from_slice(t);
    }
    b.extend(floats(&pivot));
    b
}

/// One skin section (`0x30` bytes).
pub struct Section {
    pub id: u16,
    pub triangle_start: u16,
    pub triangle_count: u16,
    pub sort_center: [f32; 3],
    pub sort_radius: f32,
}

impl Section {
    fn bytes(&self) -> Vec<u8> {
        let mut s = vec![0u8; 0x30];
        s[0..2].copy_from_slice(&self.id.to_le_bytes());
        s[8..10].copy_from_slice(&self.triangle_start.to_le_bytes());
        s[10..12].copy_from_slice(&self.triangle_count.to_le_bytes());
        s[0x20..0x2c].copy_from_slice(&floats(&self.sort_center));
        s[0x2c..0x30].copy_from_slice(&self.sort_radius.to_le_bytes());
        s
    }
}

/// A batch record (`0x18` bytes) from its `u16` words by offset (`flags` at 0, `shader` at 2, …).
pub fn batch(words: &[(usize, u16)]) -> Vec<u8> {
    let mut b = vec![0u8; 0x18];
    for &(at, v) in words {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    b
}

/// A `.skin` file: vertex indices, triangles, sections and batches.
pub fn skin(
    indices: &[u16],
    triangles: &[u16],
    sections: &[Section],
    batches: &[Vec<u8>],
) -> Vec<u8> {
    let mut s = vec![0u8; 0x30];
    s[0..4].copy_from_slice(b"SKIN");
    let arr = |s: &mut Vec<u8>, at: usize, count: usize, bytes: &[u8]| {
        let ofs = s.len() as u32;
        s.extend_from_slice(bytes);
        s[at..at + 4].copy_from_slice(&(count as u32).to_le_bytes());
        s[at + 4..at + 8].copy_from_slice(&ofs.to_le_bytes());
    };
    arr(&mut s, 0x04, indices.len(), &shorts(indices));
    arr(&mut s, 0x0c, triangles.len(), &shorts(triangles));
    arr(&mut s, 0x14, indices.len(), &vec![0u8; indices.len() * 4]);
    let secs: Vec<u8> = sections.iter().flat_map(Section::bytes).collect();
    arr(&mut s, 0x1c, sections.len(), &secs);
    arr(&mut s, 0x24, batches.len(), &batches.concat());
    s
}

/// A skin with no geometry.
pub fn empty_skin() -> Vec<u8> {
    skin(&[], &[], &[], &[])
}
