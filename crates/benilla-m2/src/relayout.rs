//! Version 264 (3.3.5a) models in the version-256 layout. 3.3.5a moves the skin profiles to
//! `<Model>NN.skin`, keys every animation track per sequence (`M2Array<M2Array<T>>`, no ranges),
//! compresses bone rotations to four `i16`s and shrinks the track header from `0x1c` to `0x14`
//! bytes, which moves every record holding a track. [`relayout_264`] writes such a model and its
//! skin profile 0 as a version-256 file, so each reader of the 1.12.1 layout takes it unchanged.
//!
//! The per-sequence tracks become 1.12.1's one timeline: sequence `i` takes the band
//! `[start_i, start_i + length_i]`, and its window `ranges[i]` brackets its own keys. 3.3.5a holds a
//! sequence's first key before it and its last key after it, and a sequence with no keys takes the
//! channel's default value; each band is padded with those keys, where 1.12.1's sampler would
//! otherwise read a neighbouring band's keys.
//!
//! Particle and ribbon emitters are not written: 3.3.5a's carry what 1.12.1's records have no
//! place for (per-channel over-life keys, life, rate, size and spin variance). Their readers take
//! the version-264 records and read the tracks through [`Tracks264`], onto the same bands.
//! [`Relayout::dropped`] names anything else left out.
//!
//! A sequence without flag `0x20` keeps its keys in `<Model><id>-<sub>.anim`, whose bytes the
//! track's sub-array offsets index; an alias (flag `0x40`) plays its owner's keys. [`anim_paths`]
//! names the files a model needs and [`relayout_264`] takes their bytes.

use std::collections::HashMap;

use benilla_bytes::ByteExt;

use crate::error::{Error, Result};
use crate::track::{M2ScalarTrack, M2Track, M2Vec3Track};

/// The MD20 version [`relayout_264`] reads.
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

/// A version-264 model written as version 256.
pub struct Relayout {
    pub bytes: Vec<u8>,
    /// What the version-256 file does not carry, one entry per kind.
    pub dropped: Vec<String>,
}

/// Write the version-264 `m2` and its skin profile 0, `skin`, as a version-256 model; `anims` holds
/// the [`anim_paths`] files, and a sequence whose file is missing keeps no keys.
pub fn relayout_264(m2: &[u8], skin: &[u8], anims: &AnimFiles) -> Result<Relayout> {
    match md20_version(m2) {
        None => return Err(Error::NotMd20),
        Some(VERSION_264) => {}
        Some(v) => return Err(Error::UnsupportedVersion(v)),
    }
    if skin.get(0..4) != Some(&b"SKIN"[..]) {
        return Err(Error::NotSkin);
    }
    let mut w = Writer::new(m2, anims)?;
    w.header()?;
    w.view(skin)?;
    Ok(w.finish())
}

/// The version-264 header's `M2Array`s and fields (the version-256 header has three arrays more:
/// the playable animation lookup, the embedded views and the texture flipbooks).
mod h264 {
    pub const NAME: usize = 0x08;
    pub const FLAGS: usize = 0x10;
    pub const GLOBAL_SEQUENCES: usize = 0x14;
    pub const SEQUENCES: usize = 0x1c;
    pub const SEQUENCE_LOOKUP: usize = 0x24;
    pub const BONES: usize = 0x2c;
    pub const KEY_BONE_LOOKUP: usize = 0x34;
    pub const VERTICES: usize = 0x3c;
    pub const COLORS: usize = 0x48;
    pub const TEXTURES: usize = 0x50;
    pub const TEXTURE_WEIGHTS: usize = 0x58;
    pub const TEXTURE_TRANSFORMS: usize = 0x60;
    pub const REPLACABLE_TEXTURE_LOOKUP: usize = 0x68;
    pub const MATERIALS: usize = 0x70;
    pub const BONE_LOOKUP: usize = 0x78;
    pub const TEXTURE_LOOKUP: usize = 0x80;
    pub const TEXTURE_UNIT_LOOKUP: usize = 0x88;
    pub const TEXTURE_WEIGHT_LOOKUP: usize = 0x90;
    pub const TEXTURE_TRANSFORM_LOOKUP: usize = 0x98;
    /// The bounding and collision boxes and radii, `0x38` bytes.
    pub const BOUNDS: usize = 0xa0;
    pub const COLLISION_TRIANGLES: usize = 0xd8;
    pub const COLLISION_VERTICES: usize = 0xe0;
    pub const COLLISION_NORMALS: usize = 0xe8;
    pub const ATTACHMENTS: usize = 0xf0;
    pub const ATTACHMENT_LOOKUP: usize = 0xf8;
    pub const EVENTS: usize = 0x100;
    pub const LIGHTS: usize = 0x108;
    pub const CAMERAS: usize = 0x110;
    pub const CAMERA_LOOKUP: usize = 0x118;
    /// The texture combiner ops, present under global flag `0x8`.
    pub const TEXTURE_COMBINER_COMBOS: usize = 0x130;
}

/// The version-256 header, at the offsets the 1.12.1 readers take.
mod h256 {
    pub const NAME: usize = 0x08;
    pub const FLAGS: usize = 0x10;
    pub const GLOBAL_SEQUENCES: usize = 0x14;
    pub const SEQUENCES: usize = 0x1c;
    pub const SEQUENCE_LOOKUP: usize = 0x24;
    pub const BONES: usize = 0x34;
    pub const KEY_BONE_LOOKUP: usize = 0x3c;
    pub const VERTICES: usize = 0x44;
    pub const VIEWS: usize = 0x4c;
    pub const COLORS: usize = 0x54;
    pub const TEXTURES: usize = 0x5c;
    pub const TEXTURE_WEIGHTS: usize = 0x64;
    pub const TEXTURE_TRANSFORMS: usize = 0x74;
    pub const REPLACABLE_TEXTURE_LOOKUP: usize = 0x7c;
    pub const MATERIALS: usize = 0x84;
    pub const BONE_LOOKUP: usize = 0x8c;
    pub const TEXTURE_LOOKUP: usize = 0x94;
    pub const TEXTURE_UNIT_LOOKUP: usize = 0x9c;
    pub const TEXTURE_WEIGHT_LOOKUP: usize = 0xa4;
    pub const TEXTURE_TRANSFORM_LOOKUP: usize = 0xac;
    pub const BOUNDS: usize = 0xb4;
    pub const COLLISION_TRIANGLES: usize = 0xec;
    pub const COLLISION_VERTICES: usize = 0xf4;
    pub const COLLISION_NORMALS: usize = 0xfc;
    pub const ATTACHMENTS: usize = 0x104;
    pub const ATTACHMENT_LOOKUP: usize = 0x10c;
    pub const EVENTS: usize = 0x114;
    pub const LIGHTS: usize = 0x11c;
    pub const CAMERAS: usize = 0x124;
    pub const CAMERA_LOOKUP: usize = 0x12c;
    /// The texture combiner ops after the ribbon and particle arrays (`0x134`, `0x13c`), which
    /// stay empty; read under global flag `0x8`, as version 260 places them.
    pub const TEXTURE_COMBINER_COMBOS: usize = 0x144;
    /// The header's length.
    pub const LEN: usize = 0x14c;
}

/// Track header lengths: 3.3.5a's `interp, gseq, times, values`; 1.12.1's adds `ranges` after
/// `gseq`. An event's `M2TrackBase` has no values in either (`0x0c` and `0x14` bytes).
const TRACK_264: usize = 0x14;
const TRACK_256: usize = 0x1c;
const TRACK_BASE_256: usize = 0x14;

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
    /// A bone rotation: 3.3.5a's four `i16`s, 1.12.1's four `f32`s; identity by default.
    PackedQuat,
    /// A `C4Quaternion` (a texture transform's rotation); identity by default.
    Quat,
    Float(f32),
    /// `fixed16`, `i16 / 0x7fff`.
    Fixed16(i16),
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
            Channel::Fixed16(_) => 2,
            Channel::Byte(_) => 1,
            Channel::Spline(n) => 12 * n,
        }
    }

    /// Append the version-256 value read from `v`, which holds [`Self::size_264`] bytes.
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
            Channel::Fixed16(i) => out.extend_from_slice(&i.to_le_bytes()),
            Channel::Byte(b) => out.push(b),
            Channel::Spline(n) => out.resize(out.len() + 12 * n, 0),
        }
    }
}

const ZERO3: Channel = Channel::Vec3([0.0; 3]);
const ONE3: Channel = Channel::Vec3([1.0; 3]);

/// The fields of a version-264 sequence record the relayout reads.
struct Seq {
    id: u16,
    sub: u16,
    length: u32,
    flags: u32,
    alias_next: u16,
}

/// The `m2`'s sequence records.
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

/// One sequence's band on the version-256 timeline.
struct Band<'a> {
    start: u32,
    length: u32,
    flags: u32,
    /// The sequence whose set holds the keys, and the bytes its offsets index (the `.m2` or the
    /// owner's `.anim`); `None` when those bytes are not at hand.
    keys: Option<(usize, &'a [u8])>,
}

/// One track on the version-256 timeline: `ranges` (8 bytes each), `times` (4) and `values` (one
/// per time, in the version-256 layout).
struct Timed {
    interp: u16,
    gseq: u16,
    ranges: Vec<u8>,
    times: Vec<u8>,
    values: Vec<u8>,
}

/// The tracks of a version-264 model's records that [`relayout_264`] does not write, read onto
/// the bands it writes: sequence `i` at `[start_i, start_i + length_i]`, its window `ranges[i]`,
/// each band padded as the relayout pads it.
pub struct Tracks264<'a> {
    w: Writer<'a>,
}

impl<'a> Tracks264<'a> {
    /// The tracks of `m2`, with the [`anim_paths`] files in `anims`.
    pub fn new(m2: &'a [u8], anims: &'a AnimFiles) -> Result<Self> {
        match md20_version(m2) {
            None => Err(Error::NotMd20),
            Some(VERSION_264) => Ok(Self {
                w: Writer::new(m2, anims)?,
            }),
            Some(v) => Err(Error::UnsupportedVersion(v)),
        }
    }

    /// Each sequence's band `(start, end)` and flags, as the relayout's sequence records hold them.
    pub fn sequences(&self) -> Vec<((u32, u32), u32)> {
        self.w
            .bands
            .iter()
            .map(|b| ((b.start, b.start.saturating_add(b.length)), b.flags))
            .collect()
    }

    fn read<V>(
        &mut self,
        at: usize,
        ch: Channel,
        decode: impl Fn(&[u8]) -> V,
    ) -> Result<M2Track<V>> {
        let t = self.w.timed(at, ch)?;
        let size = ch.size_264();
        Ok(M2Track {
            interp: t.interp,
            gseq: t.gseq,
            ranges: t
                .ranges
                .as_chunks::<8>()
                .0
                .iter()
                .map(|r| (r.u32_at(0).unwrap_or(0), r.u32_at(4).unwrap_or(0)))
                .collect(),
            keys: t
                .times
                .as_chunks::<4>()
                .0
                .iter()
                .zip(t.values.chunks_exact(size))
                .map(|(ms, v)| (u32::from_le_bytes(*ms), decode(v)))
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
        self.read(at, Channel::Fixed16(default as i16), |v| {
            f32::from(v.u16_at(0).unwrap_or(0))
        })
    }

    /// The `fixed16` track at `at`, `i16 / 0x7fff`.
    pub fn fixed16(&mut self, at: usize, default: i16) -> Result<M2ScalarTrack> {
        self.read(at, Channel::Fixed16(default), |v| {
            f32::from(v.u16_at(0).unwrap_or(0) as i16) / 32767.0
        })
    }

    /// The `C3Vector` track at `at`.
    pub fn vec3(&mut self, at: usize, default: [f32; 3]) -> Result<M2Vec3Track> {
        self.read(at, Channel::Vec3(default), |v| {
            std::array::from_fn(|i| v.f32_at(i * 4).unwrap_or(0.0))
        })
    }
}

struct Writer<'a> {
    src: &'a [u8],
    out: Vec<u8>,
    bands: Vec<Band<'a>>,
    dropped: Vec<String>,
    /// Keys past their sequence's length, which no band holds.
    keys_past_end: usize,
    /// Tracks with spline interpolation on a channel 1.12.1 keys plainly.
    spline_tracks: usize,
}

impl<'a> Writer<'a> {
    fn new(src: &'a [u8], anims: &'a AnimFiles) -> Result<Self> {
        let mut out = vec![0u8; h256::LEN];
        out[0..4].copy_from_slice(b"MD20");
        out[4..8].copy_from_slice(&256u32.to_le_bytes());
        let seqs = sequences(src)?;
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
                Some(o) if seqs[o].flags & SEQ_IN_FILE != 0 => Some((o, src)),
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
            let length = s.length;
            bands.push(Band {
                start,
                length,
                flags: s.flags,
                keys,
            });
            // One empty millisecond between bands, so each band's end key stays its own.
            start = start.saturating_add(length).saturating_add(1);
        }
        Ok(Self {
            src,
            out,
            bands,
            dropped,
            keys_past_end: 0,
            spline_tracks: 0,
        })
    }

    fn finish(mut self) -> Relayout {
        if self.keys_past_end > 0 {
            self.dropped.push(format!(
                "{} keys past their sequence's end",
                self.keys_past_end
            ));
        }
        if self.spline_tracks > 0 {
            self.dropped.push(format!(
                "{} spline-interpolated tracks on a channel 1.12.1 keys plainly",
                self.spline_tracks
            ));
        }
        Relayout {
            bytes: self.out,
            dropped: self.dropped,
        }
    }

    /// Append `bytes` 16-aligned; its offset.
    fn block(&mut self, bytes: &[u8]) -> Result<u32> {
        let at = self.out.len().next_multiple_of(16);
        self.out.resize(at, 0);
        self.out.extend_from_slice(bytes);
        u32::try_from(at).map_err(|_| Error::Truncated)
    }

    /// Write `(count, ofs)` at version-256 header offset `at`.
    fn set_arr(&mut self, at: usize, count: usize, ofs: u32) -> Result<()> {
        let count = u32::try_from(count).map_err(|_| Error::Truncated)?;
        self.out[at..at + 4].copy_from_slice(&count.to_le_bytes());
        self.out[at + 4..at + 8].copy_from_slice(&ofs.to_le_bytes());
        Ok(())
    }

    /// Copy the version-264 array at header `from` (elements of `size` bytes) to header `to`.
    fn copy_arr(&mut self, from: usize, to: usize, size: usize) -> Result<()> {
        let (count, ofs) = arr(self.src, from)?;
        let bytes = get_n(self.src, ofs, count, size)?;
        let at = self.block(bytes)?;
        self.set_arr(to, count, at)
    }

    /// Convert the version-264 array at header `from`, `size_264` bytes a record, to header `to`:
    /// `record` takes the record's index and offset.
    fn convert_arr(
        &mut self,
        from: usize,
        to: usize,
        size_264: usize,
        record: impl Fn(&mut Self, usize, usize) -> Result<Vec<u8>>,
    ) -> Result<()> {
        let (count, ofs) = arr(self.src, from)?;
        get_n(self.src, ofs, count, size_264)?;
        let mut recs = Vec::new();
        for i in 0..count {
            recs.extend(record(self, i, ofs + i * size_264)?);
        }
        let at = self.block(&recs)?;
        self.set_arr(to, count, at)
    }

    fn header(&mut self) -> Result<()> {
        let src = self.src;
        self.copy_arr(h264::NAME, h256::NAME, 1)?;
        let flags = u32_at(src, h264::FLAGS)?;
        self.out[h256::FLAGS..h256::FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        self.copy_arr(h264::GLOBAL_SEQUENCES, h256::GLOBAL_SEQUENCES, 4)?;
        self.convert_arr(h264::SEQUENCES, h256::SEQUENCES, 64, Self::sequence)?;
        self.copy_arr(h264::SEQUENCE_LOOKUP, h256::SEQUENCE_LOOKUP, 2)?;
        // 3.3.5a has no PlayableAnimationLookup; its slot stays empty.
        self.convert_arr(h264::BONES, h256::BONES, 88, Self::bone)?;
        self.copy_arr(h264::KEY_BONE_LOOKUP, h256::KEY_BONE_LOOKUP, 2)?;
        self.copy_arr(h264::VERTICES, h256::VERTICES, 48)?;
        self.convert_arr(h264::COLORS, h256::COLORS, 2 * TRACK_264, |w, _, r| {
            Ok([
                w.track(r, ONE3)?,
                w.track(r + TRACK_264, Channel::Fixed16(0x7fff))?,
            ]
            .concat())
        })?;
        self.convert_arr(h264::TEXTURES, h256::TEXTURES, 16, Self::texture)?;
        self.convert_arr(
            h264::TEXTURE_WEIGHTS,
            h256::TEXTURE_WEIGHTS,
            TRACK_264,
            |w, _, r| Ok(w.track(r, Channel::Fixed16(0x7fff))?.to_vec()),
        )?;
        self.convert_arr(
            h264::TEXTURE_TRANSFORMS,
            h256::TEXTURE_TRANSFORMS,
            3 * TRACK_264,
            |w, _, r| {
                Ok([
                    w.track(r, ZERO3)?,
                    w.track(r + TRACK_264, Channel::Quat)?,
                    w.track(r + 2 * TRACK_264, ONE3)?,
                ]
                .concat())
            },
        )?;
        self.copy_arr(
            h264::REPLACABLE_TEXTURE_LOOKUP,
            h256::REPLACABLE_TEXTURE_LOOKUP,
            2,
        )?;
        self.copy_arr(h264::MATERIALS, h256::MATERIALS, 4)?;
        self.copy_arr(h264::BONE_LOOKUP, h256::BONE_LOOKUP, 2)?;
        self.copy_arr(h264::TEXTURE_LOOKUP, h256::TEXTURE_LOOKUP, 2)?;
        self.copy_arr(h264::TEXTURE_UNIT_LOOKUP, h256::TEXTURE_UNIT_LOOKUP, 2)?;
        self.copy_arr(h264::TEXTURE_WEIGHT_LOOKUP, h256::TEXTURE_WEIGHT_LOOKUP, 2)?;
        self.copy_arr(
            h264::TEXTURE_TRANSFORM_LOOKUP,
            h256::TEXTURE_TRANSFORM_LOOKUP,
            2,
        )?;
        let bounds = get(src, h264::BOUNDS, 0x38)?;
        self.out[h256::BOUNDS..h256::BOUNDS + 0x38].copy_from_slice(bounds);
        self.copy_arr(h264::COLLISION_TRIANGLES, h256::COLLISION_TRIANGLES, 2)?;
        self.copy_arr(h264::COLLISION_VERTICES, h256::COLLISION_VERTICES, 12)?;
        self.copy_arr(h264::COLLISION_NORMALS, h256::COLLISION_NORMALS, 12)?;
        self.convert_arr(h264::ATTACHMENTS, h256::ATTACHMENTS, 40, Self::attachment)?;
        self.copy_arr(h264::ATTACHMENT_LOOKUP, h256::ATTACHMENT_LOOKUP, 2)?;
        self.convert_arr(h264::EVENTS, h256::EVENTS, 0x24, Self::event)?;
        self.convert_arr(
            h264::LIGHTS,
            h256::LIGHTS,
            0x10 + 7 * TRACK_264,
            Self::light,
        )?;
        self.convert_arr(h264::CAMERAS, h256::CAMERAS, 0x64, Self::camera)?;
        self.copy_arr(h264::CAMERA_LOOKUP, h256::CAMERA_LOOKUP, 2)?;
        if flags & 8 != 0 {
            self.copy_arr(
                h264::TEXTURE_COMBINER_COMBOS,
                h256::TEXTURE_COMBINER_COMBOS,
                2,
            )?;
        }
        Ok(())
    }

    /// Sequence `i`, `0x40` → `0x44` bytes: `length` becomes its band's `start, end`.
    fn sequence(&mut self, i: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 64)?;
        let band = &self.bands[i];
        let mut out = Vec::with_capacity(0x44);
        out.extend_from_slice(&r[0..4]);
        out.extend_from_slice(&band.start.to_le_bytes());
        out.extend_from_slice(&band.start.saturating_add(band.length).to_le_bytes());
        out.extend_from_slice(&r[8..64]);
        Ok(out)
    }

    /// A bone, `0x58` → `0x6c` bytes: 3.3.5a's name hash at `+0x0c` goes, the three tracks widen.
    fn bone(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 88)?;
        let mut out = r[0..12].to_vec();
        out.extend(self.track(rec + 0x10, ZERO3)?);
        out.extend(self.track(rec + 0x24, Channel::PackedQuat)?);
        out.extend(self.track(rec + 0x38, ONE3)?);
        out.extend_from_slice(&r[0x4c..0x58]);
        Ok(out)
    }

    /// A texture, its filename moved with it.
    fn texture(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 16)?;
        let (count, ofs) = arr(r, 8)?;
        let name = get(self.src, ofs, count)?;
        let at = self.block(name)?;
        let mut out = r[0..12].to_vec();
        out.extend_from_slice(&at.to_le_bytes());
        Ok(out)
    }

    /// An attachment, `0x28` → `0x30` bytes: `bone` is a `u16` then padding in 3.3.5a, read as a
    /// `u32` by 1.12.1's reader, so the padding is zeroed.
    fn attachment(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 40)?;
        let mut out = r[0..6].to_vec();
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&r[8..20]);
        out.extend(self.track(rec + 20, Channel::Byte(1))?);
        Ok(out)
    }

    /// An event, `0x24` → `0x2c` bytes: `bone` as for an attachment, its `M2TrackBase` widened.
    fn event(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 0x24)?;
        let mut out = r[0..10].to_vec();
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&r[12..24]);
        out.extend(self.track_base(rec + 24)?);
        Ok(out)
    }

    /// A light, `0x9c` → `0xd4` bytes: type, bone and position, then seven tracks.
    fn light(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 0x10)?;
        let mut out = r.to_vec();
        // Visibility is a byte (wowdev; 1.12.1's reader takes one): every shipped key is `01 00`,
        // the same answer read as a `u16`.
        for (i, ch) in [
            ZERO3,
            Channel::Float(0.0),
            ZERO3,
            Channel::Float(0.0),
            Channel::Float(0.0),
            Channel::Float(0.0),
            Channel::Byte(0),
        ]
        .into_iter()
        .enumerate()
        {
            out.extend(self.track(rec + 0x10 + i * TRACK_264, ch)?);
        }
        Ok(out)
    }

    /// A camera, `0x64` → `0x7c` bytes: the position, target and roll spline tracks widen around
    /// the two base positions.
    fn camera(&mut self, _: usize, rec: usize) -> Result<Vec<u8>> {
        let r = get(self.src, rec, 0x64)?;
        let mut out = r[0..0x10].to_vec();
        out.extend(self.track(rec + 0x10, Channel::Spline(3))?);
        out.extend_from_slice(&r[0x24..0x30]);
        out.extend(self.track(rec + 0x30, Channel::Spline(3))?);
        out.extend_from_slice(&r[0x44..0x50]);
        out.extend(self.track(rec + 0x50, Channel::Spline(1))?);
        Ok(out)
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

    /// Track `at` (`0x14` bytes) as a version-256 track header, its keys on the timeline.
    fn track(&mut self, at: usize, ch: Channel) -> Result<[u8; TRACK_256]> {
        let t = self.timed(at, ch)?;
        self.track_header(t.interp, t.gseq, &t.ranges, &t.times, &t.values)
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
                times.extend_from_slice(&t.to_le_bytes());
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
        let mut key = |times: &mut Vec<u8>, t: u32, v: Option<&[u8]>| {
            times.extend_from_slice(&t.to_le_bytes());
            match v {
                Some(v) => ch.put(v, &mut values),
                None => ch.put_default(&mut values),
            }
        };
        let count = |times: &[u8]| (times.len() / 4) as u32;
        for (band, keys) in self.bands.iter().zip(&per_seq) {
            let first = count(&times);
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
            ranges.extend_from_slice(&first.to_le_bytes());
            ranges.extend_from_slice(&(count(&times) - 1).to_le_bytes());
        }
        self.keys_past_end += past_end;
        Ok(empty(ranges, times, values))
    }

    /// An event's `M2TrackBase` (`0x0c` bytes, times alone) as version 256's (`0x14`).
    fn track_base(&mut self, at: usize) -> Result<[u8; TRACK_BASE_256]> {
        let interp = u16_at(self.src, at)?;
        let gseq = u16_at(self.src, at + 2)?;
        let times_of = |w: &Self, s: usize, src: &[u8]| -> Vec<u32> {
            w.set(at + 4, s).map_or(Vec::new(), |(n, o)| {
                (0..n).map_while(|k| src.u32_at(o + k * 4)).collect()
            })
        };
        let (mut times, mut ranges) = (Vec::new(), Vec::new());
        if gseq != NO_GSEQ {
            for t in times_of(self, 0, self.src) {
                times.extend_from_slice(&t.to_le_bytes());
            }
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
                        times.extend_from_slice(&(self.bands[s].start + t).to_le_bytes());
                        n += 1;
                    }
                }
                ranges.extend_from_slice(&lo.to_le_bytes());
                ranges.extend_from_slice(&n.saturating_sub(1).max(lo).to_le_bytes());
            }
        }
        let mut out = [0u8; TRACK_BASE_256];
        out[0..2].copy_from_slice(&interp.to_le_bytes());
        out[2..4].copy_from_slice(&gseq.to_le_bytes());
        let ranges_at = self.block(&ranges)?;
        let times_at = self.block(&times)?;
        out[4..8].copy_from_slice(&((ranges.len() / 8) as u32).to_le_bytes());
        out[8..12].copy_from_slice(&ranges_at.to_le_bytes());
        out[12..16].copy_from_slice(&((times.len() / 4) as u32).to_le_bytes());
        out[16..20].copy_from_slice(&times_at.to_le_bytes());
        Ok(out)
    }

    /// A version-256 track header over freshly written `ranges` (8 bytes each), `times` (4) and
    /// `values` (one per time).
    fn track_header(
        &mut self,
        interp: u16,
        gseq: u16,
        ranges: &[u8],
        times: &[u8],
        values: &[u8],
    ) -> Result<[u8; TRACK_256]> {
        let n = (times.len() / 4) as u32;
        let ranges_at = self.block(ranges)?;
        let times_at = self.block(times)?;
        let values_at = self.block(values)?;
        let mut out = [0u8; TRACK_256];
        out[0..2].copy_from_slice(&interp.to_le_bytes());
        out[2..4].copy_from_slice(&gseq.to_le_bytes());
        out[4..8].copy_from_slice(&((ranges.len() / 8) as u32).to_le_bytes());
        out[8..12].copy_from_slice(&ranges_at.to_le_bytes());
        out[12..16].copy_from_slice(&n.to_le_bytes());
        out[16..20].copy_from_slice(&times_at.to_le_bytes());
        out[20..24].copy_from_slice(&n.to_le_bytes());
        out[24..28].copy_from_slice(&values_at.to_le_bytes());
        Ok(out)
    }

    /// Skin profile 0 as the one embedded view (`0x2c` bytes): its five arrays and bone count.
    /// A submesh drops 3.3.5a's sort centre and radius (`0x30` → `0x20` bytes); its `level`, the
    /// high half of a start past 65535, must be 0, as 1.12.1's starts are `u16`.
    fn view(&mut self, skin: &[u8]) -> Result<()> {
        let mut view = Vec::with_capacity(0x2c);
        let put = |view: &mut Vec<u8>, count: usize, at: u32| -> Result<()> {
            let count = u32::try_from(count).map_err(|_| Error::Truncated)?;
            view.extend_from_slice(&count.to_le_bytes());
            view.extend_from_slice(&at.to_le_bytes());
            Ok(())
        };
        // Vertex indices, triangles, bone-index properties.
        for (at, size) in [(0x04, 2), (0x0c, 2), (0x14, 4)] {
            let (count, ofs) = arr(skin, at)?;
            let block = self.block(get_n(skin, ofs, count, size)?)?;
            put(&mut view, count, block)?;
        }
        let (count, ofs) = arr(skin, 0x1c)?;
        let mut sections = Vec::with_capacity(get_n(skin, ofs, count, 0x30)?.len() / 0x30 * 0x20);
        for i in 0..count {
            let s = get(skin, ofs + i * 0x30, 0x30)?;
            if u16_at(s, 2)? != 0 {
                return Err(Error::SkinSectionPastU16(i));
            }
            sections.extend_from_slice(&s[0..0x20]);
        }
        let block = self.block(&sections)?;
        put(&mut view, count, block)?;
        // The batches (`M2Batch`, `0x18` bytes) are laid out alike in both versions.
        let (count, ofs) = arr(skin, 0x24)?;
        let block = self.block(get_n(skin, ofs, count, 0x18)?)?;
        put(&mut view, count, block)?;
        view.extend_from_slice(get(skin, 0x2c, 4)?);
        let at = self.block(&view)?;
        self.set_arr(h256::VIEWS, 1, at)
    }
}

fn get(b: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    b.bytes_at(at, len).ok_or(Error::Truncated)
}

/// `count` elements of `size` bytes at `at`.
fn get_n(b: &[u8], at: usize, count: usize, size: usize) -> Result<&[u8]> {
    get(b, at, count.checked_mul(size).ok_or(Error::Truncated)?)
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.u16_at(at).ok_or(Error::Truncated)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.u32_at(at).ok_or(Error::Truncated)
}

/// The `M2Array` at `at`: `(count, offset)`.
fn arr(b: &[u8], at: usize) -> Result<(usize, usize)> {
    Ok((u32_at(b, at)? as usize, u32_at(b, at + 4)? as usize))
}

#[cfg(test)]
mod tests;
