//! Version 264 (3.3.5a) tracks on one timeline. 3.3.5a keys every animation track per sequence
//! (`M2Array<M2Array<T>>`, no ranges); benilla's samplers take one timeline, so [`Tracks264`] puts
//! sequence `i` on the band `[start_i, start_i + length_i]` and gives it the window `ranges[i]`
//! over its own keys. 3.3.5a holds a sequence's first key before it and its last key after it, and
//! a sequence with no keys takes the channel's default value; each band is padded with those
//! keys, where the sampler would otherwise read a neighbouring band's keys.
//!
//! A sequence without flag `0x20` keeps its keys in `<Model><id>-<sub>.anim`, whose bytes the
//! track's sub-array offsets index; an alias (flag `0x40`) plays its owner's keys. [`anim_paths`]
//! names the files a model needs and [`Tracks264::new`] takes their bytes.

use std::collections::HashMap;

use benilla_bytes::ByteExt;

use crate::error::{Error, Result};
use crate::track::{
    M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2SplineKey, M2Track, M2Vec3SplineTrack,
    M2Vec3Track,
};

/// The MD20 version this crate reads.
pub const VERSION_264: u32 = 264;

/// A file's MD20 version, `None` when it is not an MD20 model.
pub fn md20_version(bytes: &[u8]) -> Option<u32> {
    (bytes.get(0..4)? == b"MD20").then(|| bytes.u32_at(4))?
}

/// The file holding skin profile 0 of the model at `model_path` (`…\Name.m2` → `…\Name00.skin`).
pub fn skin_path(model_path: &str) -> String {
    let stem = model_path
        .len()
        .checked_sub(3)
        .filter(|&i| model_path.is_char_boundary(i) && model_path[i..].eq_ignore_ascii_case(".m2"))
        .map_or(model_path, |i| &model_path[..i]);
    format!("{stem}00.skin")
}

/// The `.anim` files of a model's sequences, by the index of the sequence that owns each
/// ([`anim_paths`]).
pub type AnimFiles = HashMap<usize, Vec<u8>>;

/// The `.anim` files the sequences of the version-264 `m2` at `model_path` key into: each owning
/// sequence's index and path, the model path cut at its last `.` then `%04d-%02d.anim` of the
/// owner's id and variation (`0x835a20`). An alias names its owner's file (`0x83da10`).
pub fn anim_paths(m2: &[u8], model_path: &str) -> Result<Vec<(usize, String)>> {
    let seqs = sequences(m2)?;
    let stem = model_path
        .rfind('.')
        .map_or(model_path, |i| &model_path[..i]);
    let mut owners: Vec<usize> = (0..seqs.len())
        .filter_map(|i| owner(&seqs, i))
        .filter(|&o| seqs[o].flags & SEQ_IN_FILE == 0)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    Ok(owners
        .into_iter()
        .map(|o| {
            (
                o,
                format!("{stem}{:04}-{:02}.anim", seqs[o].id, seqs[o].sub),
            )
        })
        .collect())
}

/// The version-264 header's `M2Array`s and fields, the `M2Data` layout.
pub mod h264 {
    pub const FLAGS: usize = 0x10;
    pub const GLOBAL_SEQUENCES: usize = 0x14;
    pub const SEQUENCES: usize = 0x1c;
    pub const SEQUENCE_LOOKUP: usize = 0x24;
    pub const BONES: usize = 0x2c;
    pub const VERTICES: usize = 0x3c;
    pub const COLORS: usize = 0x48;
    pub const TEXTURES: usize = 0x50;
    pub const TEXTURE_WEIGHTS: usize = 0x58;
    pub const TEXTURE_TRANSFORMS: usize = 0x60;
    pub const MATERIALS: usize = 0x70;
    pub const TEXTURE_LOOKUP: usize = 0x80;
    pub const TEXTURE_UNIT_LOOKUP: usize = 0x88;
    pub const TEXTURE_WEIGHT_LOOKUP: usize = 0x90;
    pub const TEXTURE_TRANSFORM_LOOKUP: usize = 0x98;
    /// The bounding and collision boxes and radii, `0x38` bytes.
    pub const BOUNDS: usize = 0xa0;
    pub const COLLISION_TRIANGLES: usize = 0xd8;
    pub const COLLISION_VERTICES: usize = 0xe0;
    pub const ATTACHMENTS: usize = 0xf0;
    pub const ATTACHMENT_LOOKUP: usize = 0xf8;
    pub const EVENTS: usize = 0x100;
    pub const LIGHTS: usize = 0x108;
    pub const CAMERAS: usize = 0x110;
    pub const CAMERA_LOOKUP: usize = 0x118;
    pub const RIBBON_EMITTERS: usize = 0x120;
    pub const PARTICLE_EMITTERS: usize = 0x128;
    /// The texture combiner ops, present under global flag `0x8`.
    pub const TEXTURE_COMBINER_COMBOS: usize = 0x130;
}

/// A 3.3.5a track header: `interp, gseq, times, values`, `0x14` bytes.
pub(crate) const TRACK_264: usize = 0x14;

/// Sequence flag `0x20`: the keys are in the `.m2`; clear, they are in `<Model><id>-<sub>.anim`.
const SEQ_IN_FILE: u32 = 0x20;
/// Sequence flag `0x40`: an alias, whose keys are those of the sequence its `aliasNext` (`+0x3e`)
/// leads to; the chain ends at the first sequence without the flag.
const SEQ_ALIAS: u32 = 0x40;
/// `gseq` on a track keyed per sequence rather than on a global sequence.
const NO_GSEQ: u16 = 0xffff;

/// A track's value type, and the value a sequence without keys takes. The defaults follow
/// 3.3.5a samplers: bone and texture transforms identity, colour and weights 1, lights 0.
#[derive(Clone, Copy)]
enum Channel {
    /// A `C3Vector`.
    Vec3([f32; 3]),
    /// A bone rotation, four `i16`s; identity by default.
    PackedQuat,
    /// A `C4Quaternion` (a texture transform's rotation); identity by default.
    Quat,
    Float(f32),
    /// A 16-bit value: `fixed16` (`i16 / 0x7fff`) or a `u16`.
    Short(i16),
    Byte(u8),
    /// A camera spline key, value then in and out tangents of `n` floats each; never defaulted,
    /// as a camera adds its track to its base position.
    Spline(usize),
}

impl Channel {
    /// One value's length in the version-264 file.
    fn size_264(self) -> usize {
        match self {
            Channel::Vec3(_) => 12,
            Channel::PackedQuat => 8,
            Channel::Quat => 16,
            Channel::Float(_) => 4,
            Channel::Short(_) => 2,
            Channel::Byte(_) => 1,
            Channel::Spline(n) => 12 * n,
        }
    }

    /// One decoded value's length: a packed rotation widens to four `f32`s.
    fn size_out(self) -> usize {
        match self {
            Channel::PackedQuat => 16,
            other => other.size_264(),
        }
    }

    /// Append the value read from `v`, which holds [`Self::size_264`] bytes.
    fn put(self, v: &[u8], out: &mut Vec<u8>) {
        match self {
            // `(v < 0 ? v + 32768 : v - 32767) / 32767` (wowdev's `M2CompQuat`); OpenWoW's
            // `u16·2/65535 − 1` differs by at most 1/32767.
            Channel::PackedQuat => {
                for c in v.as_chunks::<2>().0 {
                    let i = i32::from(i16::from_le_bytes(*c));
                    let q = if i < 0 { i + 32768 } else { i - 32767 };
                    out.extend_from_slice(&(q as f32 / 32767.0).to_le_bytes());
                }
            }
            _ => out.extend_from_slice(v),
        }
    }

    /// Append the default value.
    fn put_default(self, out: &mut Vec<u8>) {
        let floats = |out: &mut Vec<u8>, fs: &[f32]| {
            for f in fs {
                out.extend_from_slice(&f.to_le_bytes());
            }
        };
        match self {
            Channel::Vec3(v) => floats(out, &v),
            Channel::PackedQuat | Channel::Quat => floats(out, &[0.0, 0.0, 0.0, 1.0]),
            Channel::Float(f) => floats(out, &[f]),
            Channel::Short(i) => out.extend_from_slice(&i.to_le_bytes()),
            Channel::Byte(b) => out.push(b),
            Channel::Spline(n) => out.resize(out.len() + 12 * n, 0),
        }
    }
}

/// The fields of a version-264 sequence record the timeline reads.
struct Seq {
    id: u16,
    sub: u16,
    length: u32,
    flags: u32,
    alias_next: u16,
}

/// The `m2`'s sequence records (`0x40` bytes each).
fn sequences(m2: &[u8]) -> Result<Vec<Seq>> {
    let (count, ofs) = arr(m2, h264::SEQUENCES)?;
    get_n(m2, ofs, count, 64)?;
    (0..count)
        .map(|i| {
            let rec = get(m2, ofs + i * 64, 64)?;
            Ok(Seq {
                id: u16_at(rec, 0)?,
                sub: u16_at(rec, 2)?,
                length: u32_at(rec, 4)?,
                flags: u32_at(rec, 12)?,
                alias_next: u16_at(rec, 0x3e)?,
            })
        })
        .collect()
}

/// The sequence whose keys sequence `i` plays: `i` itself, or the end of its alias chain; `None`
/// for a chain that leaves the array or never ends.
fn owner(seqs: &[Seq], i: usize) -> Option<usize> {
    let mut j = i;
    for _ in 0..=seqs.len() {
        if seqs.get(j)?.flags & SEQ_ALIAS == 0 {
            return Some(j);
        }
        j = usize::from(seqs[j].alias_next);
    }
    None
}

/// One sequence's band on the timeline.
struct Band<'a> {
    start: u32,
    length: u32,
    flags: u32,
    /// The sequence whose set holds the keys, and the bytes its offsets index (the `.m2` or the
    /// owner's `.anim`); `None` when those bytes are not at hand.
    keys: Option<(usize, &'a [u8])>,
}

/// One track on the timeline: `ranges`, `times` and `values` (one per time, decoded).
struct Timed {
    interp: u16,
    gseq: u16,
    ranges: Vec<(u32, u32)>,
    times: Vec<u32>,
    values: Vec<u8>,
}

/// An event track on the timeline: a window per sequence, then the times.
pub type EventTimes = (Vec<(u32, u32)>, Vec<u32>);

/// A version-264 model's tracks read onto one timeline: sequence `i` at
/// `[start_i, start_i + length_i]`, its window `ranges[i]`, each band padded with its end keys.
pub struct Tracks264<'a> {
    src: &'a [u8],
    bands: Vec<Band<'a>>,
    dropped: Vec<String>,
    /// Keys past their sequence's length, which no band holds.
    keys_past_end: usize,
    /// Tracks with spline interpolation on a channel benilla keys plainly.
    spline_tracks: usize,
}

impl<'a> Tracks264<'a> {
    /// The tracks of `m2`, with the [`anim_paths`] files in `anims`; a sequence whose file is
    /// missing keeps no keys, which [`Self::dropped`] names.
    pub fn new(m2: &'a [u8], anims: &'a AnimFiles) -> Result<Self> {
        match md20_version(m2) {
            None => return Err(Error::NotMd20),
            Some(VERSION_264) => {}
            Some(v) => return Err(Error::UnsupportedVersion(v)),
        }
        let seqs = sequences(m2)?;
        let mut bands = Vec::with_capacity(seqs.len());
        let mut start = 0u32;
        let mut dropped = Vec::new();
        for (i, s) in seqs.iter().enumerate() {
            let (id, sub) = (s.id, s.sub);
            let keys = match owner(&seqs, i) {
                None => {
                    dropped.push(format!(
                        "sequence {i} ({id}-{sub}): an alias chain that reaches no owner"
                    ));
                    None
                }
                Some(o) if seqs[o].flags & SEQ_IN_FILE != 0 => Some((o, m2)),
                Some(o) => {
                    let file = anims.get(&o).map(|a| (o, a.as_slice()));
                    if file.is_none() {
                        let (oid, osub) = (seqs[o].id, seqs[o].sub);
                        dropped.push(format!(
                            "sequence {i} ({id}-{sub}): its keys are in {oid:04}-{osub:02}.anim, \
                             which was not read"
                        ));
                    }
                    file
                }
            };
            bands.push(Band {
                start,
                length: s.length,
                flags: s.flags,
                keys,
            });
            // One empty millisecond between bands, so each band's end key stays its own.
            start = start.saturating_add(s.length).saturating_add(1);
        }
        Ok(Self {
            src: m2,
            bands,
            dropped,
            keys_past_end: 0,
            spline_tracks: 0,
        })
    }

    /// Each sequence's band `(start, end)` and flags.
    pub fn sequences(&self) -> Vec<((u32, u32), u32)> {
        self.bands
            .iter()
            .map(|b| ((b.start, b.start.saturating_add(b.length)), b.flags))
            .collect()
    }

    /// What the timeline could not hold, one entry per kind: a sequence whose keys were not at
    /// hand, keys past their sequence's end, spline keys on a plainly keyed channel.
    pub fn dropped(&self) -> Vec<String> {
        let mut out = self.dropped.clone();
        if self.keys_past_end > 0 {
            out.push(format!(
                "{} keys past their sequence's end",
                self.keys_past_end
            ));
        }
        if self.spline_tracks > 0 {
            out.push(format!(
                "{} spline-interpolated tracks on a channel keyed plainly",
                self.spline_tracks
            ));
        }
        out
    }

    fn read<V>(
        &mut self,
        at: usize,
        ch: Channel,
        decode: impl Fn(&[u8]) -> V,
    ) -> Result<M2Track<V>> {
        let t = self.timed(at, ch)?;
        Ok(M2Track {
            interp: t.interp,
            gseq: t.gseq,
            ranges: t.ranges,
            keys: t
                .times
                .iter()
                .zip(t.values.chunks_exact(ch.size_out()))
                .map(|(&ms, v)| (ms, decode(v)))
                .collect(),
        })
    }

    /// The `f32` track at `at`; a sequence without keys takes `default`.
    pub fn float(&mut self, at: usize, default: f32) -> Result<M2ScalarTrack> {
        self.read(at, Channel::Float(default), |v| v.f32_at(0).unwrap_or(0.0))
    }

    /// The `u8` track at `at`, as `0..=255` scalars.
    pub fn byte(&mut self, at: usize, default: u8) -> Result<M2ScalarTrack> {
        self.read(at, Channel::Byte(default), |v| f32::from(v[0]))
    }

    /// The `u16` track at `at`, as scalars.
    pub fn short(&mut self, at: usize, default: u16) -> Result<M2ScalarTrack> {
        self.read(at, Channel::Short(default as i16), |v| {
            f32::from(v.u16_at(0).unwrap_or(0))
        })
    }

    /// The `fixed16` track at `at`, `i16 / 0x7fff`. The key is signed (`movsx`, then `fmul` by
    /// `1/0x7fff`): art authors `0x8001` (−1.0) to hide a batch, which the cull (`A ≤ 0`) drops.
    pub fn fixed16(&mut self, at: usize, default: i16) -> Result<M2ScalarTrack> {
        self.read(at, Channel::Short(default), |v| {
            f32::from(v.u16_at(0).unwrap_or(0) as i16) / 32767.0
        })
    }

    /// The `C3Vector` track at `at`.
    pub fn vec3(&mut self, at: usize, default: [f32; 3]) -> Result<M2Vec3Track> {
        self.read(at, Channel::Vec3(default), rd_vec3)
    }

    /// The `C4Quaternion` track at `at` (four `f32`s), identity by default.
    pub fn quat(&mut self, at: usize) -> Result<M2QuatTrack> {
        self.read(at, Channel::Quat, rd_quat)
    }

    /// The bone rotation track at `at` (four `i16`s, `M2CompQuat`), unpacked, identity by default.
    pub fn packed_quat(&mut self, at: usize) -> Result<M2QuatTrack> {
        self.read(at, Channel::PackedQuat, rd_quat)
    }

    /// The cubic `C3Vector` track at `at` (key stride `0x24`).
    pub fn spline_vec3(&mut self, at: usize) -> Result<M2Vec3SplineTrack> {
        self.read(at, Channel::Spline(3), |v| M2SplineKey {
            value: rd_vec3(v),
            in_tan: rd_vec3(&v[12..]),
            out_tan: rd_vec3(&v[24..]),
        })
    }

    /// The cubic scalar track at `at` (key stride `0xc`).
    pub fn spline_f32(&mut self, at: usize) -> Result<M2ScalarSplineTrack> {
        self.read(at, Channel::Spline(1), |v| M2SplineKey {
            value: v.f32_at(0).unwrap_or(0.0),
            in_tan: v.f32_at(4).unwrap_or(0.0),
            out_tan: v.f32_at(8).unwrap_or(0.0),
        })
    }

    /// Sub-array `set` of the `M2Array<M2Array<_>>` at `at`: `(count, offset)`, or none.
    fn set(&self, at: usize, set: usize) -> Option<(usize, usize)> {
        let (sets, ofs) = arr(self.src, at).ok()?;
        (set < sets).then(|| arr(self.src, ofs + set * 8).ok())?
    }

    /// Where sequence `s` of a track with `sets` sets reads its keys: its owner's set, or set 0 when
    /// the track has fewer sets (as OpenWoW reads), and the bytes that set's offsets index.
    fn source(&self, s: usize, sets: usize) -> Option<(usize, &'a [u8])> {
        let (o, _) = self.bands[s].keys?;
        match o < sets {
            true => self.bands[o].keys,
            false => self.bands.first()?.keys.map(|(_, b)| (0, b)),
        }
    }

    /// Set `set` of the track at `at`, `(ms, value)` with values `size` bytes read from `src`
    /// (the `.m2` or a `.anim`); a set reaching past those bytes ends at the last whole key.
    fn keys(&self, at: usize, set: usize, size: usize, src: &'a [u8]) -> Vec<(u32, &'a [u8])> {
        let (Some((tn, to)), Some((vn, vo))) = (self.set(at + 4, set), self.set(at + 12, set))
        else {
            return Vec::new();
        };
        (0..tn.min(vn))
            .map_while(|k| Some((src.u32_at(to + k * 4)?, src.bytes_at(vo + k * size, size)?)))
            .collect()
    }

    /// Track `at` (`0x14` bytes) with its keys on the timeline.
    fn timed(&mut self, at: usize, ch: Channel) -> Result<Timed> {
        let interp = u16_at(self.src, at)?;
        let gseq = u16_at(self.src, at + 2)?;
        let size = ch.size_264();
        let (sets, _) = arr(self.src, at + 4)?;
        let empty = |ranges, times, values| Timed {
            interp,
            gseq,
            ranges,
            times,
            values,
        };
        if interp > 1 && !matches!(ch, Channel::Spline(_)) && sets > 0 {
            self.spline_tracks += 1;
            return Ok(empty(Vec::new(), Vec::new(), Vec::new()));
        }
        let (mut times, mut values, mut ranges) = (Vec::new(), Vec::new(), Vec::new());
        if gseq != NO_GSEQ {
            // A global-sequence track keys one set on its own clock (OpenWoW reads set 0 alone).
            for (t, v) in self.keys(at, 0, size, self.src) {
                times.push(t);
                ch.put(v, &mut values);
            }
            return Ok(empty(Vec::new(), times, values));
        }
        let per_seq: Vec<Vec<(u32, &[u8])>> = (0..self.bands.len())
            .map(|s| {
                self.source(s, sets)
                    .map_or(Vec::new(), |(set, src)| self.keys(at, set, size, src))
            })
            .collect();
        if per_seq.iter().all(Vec::is_empty) {
            return Ok(empty(Vec::new(), Vec::new(), Vec::new()));
        }
        let mut past_end = 0;
        let mut key = |times: &mut Vec<u32>, t: u32, v: Option<&[u8]>| {
            times.push(t);
            match v {
                Some(v) => ch.put(v, &mut values),
                None => ch.put_default(&mut values),
            }
        };
        for (band, keys) in self.bands.iter().zip(&per_seq) {
            let first = times.len() as u32;
            let kept: Vec<(u32, &[u8])> = keys
                .iter()
                .copied()
                .filter(|&(t, _)| t <= band.length)
                .collect();
            past_end += keys.len() - kept.len();
            match (kept.first(), kept.last()) {
                (Some(&(t0, v0)), Some(&(t_last, v_last))) => {
                    if t0 > 0 {
                        key(&mut times, band.start, Some(v0));
                    }
                    for &(t, v) in &kept {
                        key(&mut times, band.start + t, Some(v));
                    }
                    if t_last < band.length {
                        key(&mut times, band.start + band.length, Some(v_last));
                    }
                }
                _ => key(&mut times, band.start, None),
            }
            ranges.push((first, times.len() as u32 - 1));
        }
        self.keys_past_end += past_end;
        Ok(empty(ranges, times, values))
    }

    /// An event's `M2TrackBase` at `at` (`0x0c` bytes, times alone): its windows and its times on
    /// the timeline. A sequence's window over no keys is `(lo, lo)`.
    pub fn times(&mut self, at: usize) -> Result<EventTimes> {
        let gseq = u16_at(self.src, at + 2)?;
        let times_of = |w: &Self, s: usize, src: &[u8]| -> Vec<u32> {
            w.set(at + 4, s).map_or(Vec::new(), |(n, o)| {
                (0..n).map_while(|k| src.u32_at(o + k * 4)).collect()
            })
        };
        let (mut times, mut ranges) = (Vec::new(), Vec::new());
        if gseq != NO_GSEQ {
            times = times_of(self, 0, self.src);
        } else {
            let mut n = 0u32;
            for s in 0..self.bands.len() {
                let lo = n;
                if let Some((set, src)) = self.bands[s].keys {
                    for t in times_of(self, set, src) {
                        if t > self.bands[s].length {
                            self.keys_past_end += 1;
                            continue;
                        }
                        times.push(self.bands[s].start + t);
                        n += 1;
                    }
                }
                ranges.push((lo, n.saturating_sub(1).max(lo)));
            }
        }
        Ok((ranges, times))
    }
}

fn rd_vec3(v: &[u8]) -> [f32; 3] {
    std::array::from_fn(|i| v.f32_at(i * 4).unwrap_or(0.0))
}

fn rd_quat(v: &[u8]) -> [f32; 4] {
    std::array::from_fn(|i| v.f32_at(i * 4).unwrap_or(0.0))
}

pub(crate) fn get(b: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    b.bytes_at(at, len).ok_or(Error::Truncated)
}

/// `count` elements of `size` bytes at `at`.
pub(crate) fn get_n(b: &[u8], at: usize, count: usize, size: usize) -> Result<&[u8]> {
    get(b, at, count.checked_mul(size).ok_or(Error::Truncated)?)
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.u16_at(at).ok_or(Error::Truncated)
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.u32_at(at).ok_or(Error::Truncated)
}

/// The `M2Array` at `at`: `(count, offset)`.
pub(crate) fn arr(b: &[u8], at: usize) -> Result<(usize, usize)> {
    Ok((u32_at(b, at)? as usize, u32_at(b, at + 4)? as usize))
}
