use std::io::Cursor;

use super::*;
use crate::parse_m2;

/// A version-264 file under construction: the header, then appended arrays.
struct V264 {
    b: Vec<u8>,
}

impl V264 {
    fn new() -> Self {
        let mut b = vec![0u8; 0x130];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&264u32.to_le_bytes());
        Self { b }
    }

    /// Append `bytes`; their offset.
    fn data(&mut self, bytes: &[u8]) -> u32 {
        let at = self.b.len() as u32;
        self.b.extend_from_slice(bytes);
        at
    }

    fn put_arr(buf: &mut [u8], at: usize, count: usize, ofs: u32) {
        buf[at..at + 4].copy_from_slice(&(count as u32).to_le_bytes());
        buf[at + 4..at + 8].copy_from_slice(&ofs.to_le_bytes());
    }

    /// Append `count` records of `bytes` and point header `at` at them.
    fn array(&mut self, at: usize, count: usize, bytes: &[u8]) {
        let ofs = self.data(bytes);
        Self::put_arr(&mut self.b, at, count, ofs);
    }

    /// A track's `0x14`-byte header with `sets` per-sequence sets of `(ms, value bytes)`.
    fn track(&mut self, interp: u16, gseq: u16, sets: &[&[(u32, Vec<u8>)]]) -> Vec<u8> {
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
}

fn floats(fs: &[f32]) -> Vec<u8> {
    fs.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// A sequence record (`0x40` bytes).
fn sequence(length: u32, flags: u32) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[4..8].copy_from_slice(&length.to_le_bytes());
    r[8..12].copy_from_slice(&1.5f32.to_le_bytes());
    r[12..16].copy_from_slice(&flags.to_le_bytes());
    r[62..64].copy_from_slice(&7u16.to_le_bytes());
    r
}

/// A skin with one three-vertex triangle; `level` is its section's start high half.
fn skin(level: u16) -> Vec<u8> {
    let mut s = vec![0u8; 0x30];
    s[0..4].copy_from_slice(b"SKIN");
    let arr = |s: &mut Vec<u8>, at: usize, count: u32, bytes: &[u8]| {
        let ofs = s.len() as u32;
        s.extend_from_slice(bytes);
        s[at..at + 4].copy_from_slice(&count.to_le_bytes());
        s[at + 4..at + 8].copy_from_slice(&ofs.to_le_bytes());
    };
    let three: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
    arr(&mut s, 0x04, 3, &three);
    arr(&mut s, 0x0c, 3, &three);
    arr(&mut s, 0x14, 3, &[0u8; 12]);
    let mut section = vec![0u8; 0x30];
    section[0..2].copy_from_slice(&1201u16.to_le_bytes());
    section[2..4].copy_from_slice(&level.to_le_bytes());
    section[10..12].copy_from_slice(&3u16.to_le_bytes());
    section[0x2c..0x30].copy_from_slice(&9.0f32.to_le_bytes());
    arr(&mut s, 0x1c, 1, &section);
    let mut batch = vec![0u8; 0x18];
    batch[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
    arr(&mut s, 0x24, 1, &batch);
    s[0x2c..0x30].copy_from_slice(&21u32.to_le_bytes());
    s
}

/// Two in-file sequences (100 and 50 ms), one bone with a translation keyed in the first only, a
/// one-set packed rotation and no scale, one texture, and one particle emitter.
fn model() -> Vec<u8> {
    let mut m = V264::new();
    m.array(
        h264::SEQUENCES,
        2,
        &[sequence(100, SEQ_IN_FILE), sequence(50, SEQ_IN_FILE)].concat(),
    );
    let t = m.track(
        1,
        NO_GSEQ,
        &[
            &[
                (10, floats(&[1.0, 2.0, 3.0])),
                (90, floats(&[4.0, 5.0, 6.0])),
            ],
            &[],
        ],
    );
    let packed: Vec<u8> = [-1i16, 0, 32767, -32768]
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let r = m.track(1, NO_GSEQ, &[&[(0, packed)]]);
    let s = m.track(1, NO_GSEQ, &[]);
    let mut bone = (-1i32).to_le_bytes().to_vec();
    bone.extend_from_slice(&0u32.to_le_bytes());
    bone.extend_from_slice(&(-1i16).to_le_bytes());
    bone.extend_from_slice(&0u16.to_le_bytes());
    bone.extend_from_slice(&0xdead_beefu32.to_le_bytes());
    bone.extend([t, r, s].concat());
    bone.extend(floats(&[7.0, 8.0, 9.0]));
    assert_eq!(bone.len(), 88);
    m.array(h264::BONES, 1, &bone);
    let name = m.data(b"A.blp\0");
    let mut tex = vec![0u8; 8];
    tex.extend_from_slice(&6u32.to_le_bytes());
    tex.extend_from_slice(&name.to_le_bytes());
    m.array(h264::TEXTURES, 1, &tex);
    // The particle emitter array (header +0x128), one 0x1dc-byte record.
    m.array(0x128, 1, &[0u8; 0x1dc]);
    m.b
}

/// A version-256 track as read back: its `ranges` and its `(ms, value bytes)` keys.
type Track256 = (Vec<(u32, u32)>, Vec<(u32, Vec<u8>)>);

/// The version-256 track at `at` in `b`.
fn track_256(b: &[u8], at: usize, size: usize) -> Track256 {
    let u = |o: usize| b.u32_at(o).unwrap() as usize;
    let ranges = (0..u(at + 4))
        .map(|i| (u(u(at + 8) + i * 8) as u32, u(u(at + 8) + i * 8 + 4) as u32))
        .collect();
    let keys = (0..u(at + 12))
        .map(|i| {
            let v = u(at + 24) + i * size;
            (u(u(at + 16) + i * 4) as u32, b[v..v + size].to_vec())
        })
        .collect();
    (ranges, keys)
}

#[test]
fn sequences_take_consecutive_bands() {
    let out = relayout_264(&model(), &skin(0), &AnimFiles::new()).unwrap();
    let b = &out.bytes;
    assert_eq!(md20_version(b), Some(256));
    let ofs = b.u32_at(h256::SEQUENCES + 4).unwrap() as usize;
    let band = |i: usize| (b.u32_at(ofs + i * 0x44 + 4), b.u32_at(ofs + i * 0x44 + 8));
    assert_eq!(band(0), (Some(0), Some(100)));
    assert_eq!(band(1), (Some(101), Some(151)));
    // The fields after `length` move up four bytes, intact.
    assert_eq!(b.f32_at(ofs + 0x0c), Some(1.5));
    assert_eq!(b.u16_at(ofs + 0x42), Some(7));
}

#[test]
fn a_band_holds_its_end_keys_and_an_empty_one_takes_the_default() {
    let out = relayout_264(&model(), &skin(0), &AnimFiles::new()).unwrap();
    let b = &out.bytes;
    let bone = b.u32_at(h256::BONES + 4).unwrap() as usize;
    let (ranges, keys) = track_256(b, bone + 0x0c, 12);
    let v = |fs: &[f32]| floats(fs);
    assert_eq!(
        keys,
        vec![
            (0, v(&[1.0, 2.0, 3.0])),
            (10, v(&[1.0, 2.0, 3.0])),
            (90, v(&[4.0, 5.0, 6.0])),
            (100, v(&[4.0, 5.0, 6.0])),
            (101, v(&[0.0, 0.0, 0.0])),
        ]
    );
    assert_eq!(ranges, vec![(0, 3), (4, 4)]);
}

#[test]
fn rotations_unpack_and_a_lone_set_serves_every_sequence() {
    let out = relayout_264(&model(), &skin(0), &AnimFiles::new()).unwrap();
    let b = &out.bytes;
    let bone = b.u32_at(h256::BONES + 4).unwrap() as usize;
    let (ranges, keys) = track_256(b, bone + 0x28, 16);
    let q = floats(&[1.0, -1.0, 0.0, 0.0]);
    let times: Vec<u32> = keys.iter().map(|k| k.0).collect();
    assert_eq!(times, vec![0, 100, 101, 151]);
    assert!(keys.iter().all(|k| k.1 == q));
    assert_eq!(ranges, vec![(0, 1), (2, 3)]);
    // A track with no keys stays empty: the reader's default, not a written one.
    let (ranges, keys) = track_256(b, bone + 0x44, 12);
    assert!(ranges.is_empty() && keys.is_empty());
}

#[test]
fn the_1_12_reader_takes_the_relayout() {
    let out = relayout_264(&model(), &skin(0), &AnimFiles::new()).unwrap();
    let m = parse_m2(&mut Cursor::new(&out.bytes[..])).unwrap();
    let m = m.model();
    assert_eq!(m.bones.len(), 1);
    assert_eq!((m.bones[0].pivot.x, m.bones[0].pivot.z), (7.0, 9.0));
    assert_eq!(m.bones[0].parent, -1);
    assert_eq!(m.textures[0].filename.string.to_str(), Ok("A.blp"));
    let view = m.parse_embedded_skin(&out.bytes, 0).unwrap();
    assert_eq!(view.triangles(), &vec![0, 1, 2]);
    assert_eq!(view.submeshes()[0].id, 1201);
    assert_eq!(view.submeshes()[0].triangle_count, 3);
    assert_eq!(view.batches()[0].texture_count, 1);
}

#[test]
fn what_1_12_cannot_hold_is_named_not_dropped_silently() {
    // The particle emitter is read from the version-264 record, so the relayout drops nothing.
    let out = relayout_264(&model(), &skin(0), &AnimFiles::new()).unwrap();
    assert!(out.dropped.is_empty(), "{:?}", out.dropped);

    let mut m = model();
    // Sequence 1's keys move to a `.anim` file.
    let ofs = m.u32_at(h264::SEQUENCES + 4).unwrap() as usize;
    m[ofs + 64 + 12..ofs + 64 + 16].copy_from_slice(&0u32.to_le_bytes());
    let out = relayout_264(&m, &skin(0), &AnimFiles::new()).unwrap();
    assert!(
        out.dropped.iter().any(|d| d.contains(".anim")),
        "{:?}",
        out.dropped
    );
}

/// Patch sequence `i`'s record: its id and variation, flags and `aliasNext`.
fn set_sequence(m: &mut [u8], i: usize, id: u16, sub: u16, flags: u32, alias_next: u16) {
    let rec = m.u32_at(h264::SEQUENCES + 4).unwrap() as usize + i * 64;
    m[rec..rec + 2].copy_from_slice(&id.to_le_bytes());
    m[rec + 2..rec + 4].copy_from_slice(&sub.to_le_bytes());
    m[rec + 12..rec + 16].copy_from_slice(&flags.to_le_bytes());
    m[rec + 0x3e..rec + 0x40].copy_from_slice(&alias_next.to_le_bytes());
}

/// The bone translation's version-256 keys after a relayout of `m` with `anims`.
fn translation(m: &[u8], anims: &AnimFiles) -> (Track256, Vec<String>) {
    let out = relayout_264(m, &skin(0), anims).unwrap();
    let b = &out.bytes;
    let bone = b.u32_at(h256::BONES + 4).unwrap() as usize;
    (track_256(b, bone + 0x0c, 12), out.dropped)
}

#[test]
fn a_sequence_keyed_in_a_dot_anim_reads_its_keys_there() {
    let mut m = model();
    set_sequence(&mut m, 1, 60, 2, 0, 1);
    assert_eq!(
        anim_paths(&m, "Character\\Human\\Male\\HumanMale.m2").unwrap(),
        vec![(
            1,
            "Character\\Human\\Male\\HumanMale0060-02.anim".to_string()
        )]
    );
    // Set 1 of the bone translation: two keys, times at offset 8 and values at 16 of the `.anim`
    // (offsets that in the `.m2` hold its header).
    let mut anim = vec![0u8; 8];
    anim.extend([5u32, 40].iter().flat_map(|t| t.to_le_bytes()));
    anim.extend(floats(&[1.0, 1.0, 1.0, 2.0, 2.0, 2.0]));
    let bone = m.u32_at(h264::BONES + 4).unwrap() as usize;
    for (heads, ofs) in [(bone + 0x18, 8u32), (bone + 0x20, 16)] {
        let set1 = m.u32_at(heads).unwrap() as usize + 8;
        m[set1..set1 + 4].copy_from_slice(&2u32.to_le_bytes());
        m[set1 + 4..set1 + 8].copy_from_slice(&ofs.to_le_bytes());
    }
    let anims = AnimFiles::from([(1, anim)]);
    let ((ranges, keys), dropped) = translation(&m, &anims);
    assert!(dropped.is_empty(), "{dropped:?}");
    let band1: Vec<(u32, Vec<u8>)> = keys[ranges[1].0 as usize..=ranges[1].1 as usize].to_vec();
    assert_eq!(
        band1,
        vec![
            (101, floats(&[1.0; 3])),
            (106, floats(&[1.0; 3])),
            (141, floats(&[2.0; 3])),
            (151, floats(&[2.0; 3])),
        ]
    );
    // Without the file the band keeps the default, and says so.
    let ((_, keys), dropped) = translation(&m, &AnimFiles::new());
    assert_eq!(keys.last(), Some(&(101, floats(&[0.0; 3]))));
    assert!(
        dropped.iter().any(|d| d.contains("0060-02.anim")),
        "{dropped:?}"
    );
}

#[test]
fn an_alias_plays_its_owners_keys() {
    let mut m = model();
    // Sequence 1, as long as sequence 0, aliases it; its own set is empty.
    let rec = m.u32_at(h264::SEQUENCES + 4).unwrap() as usize + 64;
    m[rec + 4..rec + 8].copy_from_slice(&100u32.to_le_bytes());
    set_sequence(&mut m, 1, 136, 0, SEQ_ALIAS, 0);
    assert!(anim_paths(&m, "a.m2").unwrap().is_empty());
    let ((ranges, keys), dropped) = translation(&m, &AnimFiles::new());
    assert!(dropped.is_empty(), "{dropped:?}");
    let band = |i: usize| -> Vec<(u32, Vec<u8>)> {
        keys[ranges[i].0 as usize..=ranges[i].1 as usize]
            .iter()
            .map(|(t, v)| (t - [0, 101][i], v.clone()))
            .collect()
    };
    assert_eq!(band(1), band(0));
    // A chain that never reaches a sequence without the flag keeps no keys.
    set_sequence(&mut m, 1, 136, 0, SEQ_ALIAS, 1);
    let ((_, keys), dropped) = translation(&m, &AnimFiles::new());
    assert_eq!(keys.last(), Some(&(101, floats(&[0.0; 3]))));
    assert!(
        dropped.iter().any(|d| d.contains("no owner")),
        "{dropped:?}"
    );
}

#[test]
fn tracks_264_reads_onto_the_relayouts_bands() {
    let m = model();
    let none = AnimFiles::new();
    let mut t = Tracks264::new(&m, &none).unwrap();
    assert_eq!(
        t.sequences(),
        vec![((0, 100), SEQ_IN_FILE), ((101, 151), SEQ_IN_FILE)]
    );
    let bone = m.u32_at(h264::BONES + 4).unwrap() as usize;
    let tr = t.vec3(bone + 0x10, [0.0; 3]).unwrap();
    // The same keys and windows the relayout writes for the bone's translation.
    let out = relayout_264(&m, &skin(0), &AnimFiles::new()).unwrap();
    let b = &out.bytes;
    let (ranges, keys) = track_256(b, b.u32_at(h256::BONES + 4).unwrap() as usize + 0x0c, 12);
    assert_eq!(tr.ranges, ranges);
    let as_bytes: Vec<(u32, Vec<u8>)> = tr.keys.iter().map(|&(ms, v)| (ms, floats(&v))).collect();
    assert_eq!(as_bytes, keys);
    assert!(matches!(
        Tracks264::new(&out.bytes, &none),
        Err(Error::UnsupportedVersion(256))
    ));
}

#[test]
fn a_section_past_u16_is_an_error() {
    assert!(matches!(
        relayout_264(&model(), &skin(1), &AnimFiles::new()),
        Err(Error::SkinSectionPastU16(0))
    ));
}

#[test]
fn only_version_264_relays_out() {
    let mut m = model();
    m[4..8].copy_from_slice(&256u32.to_le_bytes());
    assert!(matches!(
        relayout_264(&m, &skin(0), &AnimFiles::new()),
        Err(Error::UnsupportedVersion(256))
    ));
    assert!(matches!(
        relayout_264(&model(), b"NOPE", &AnimFiles::new()),
        Err(Error::NotSkin)
    ));
}

#[test]
fn the_skin_sits_beside_the_model() {
    assert_eq!(
        skin_path("interface/glues/models/ui_x/ui_x.m2"),
        "interface/glues/models/ui_x/ui_x00.skin"
    );
    assert_eq!(skin_path("A\\B.M2"), "A\\B00.skin");
}
