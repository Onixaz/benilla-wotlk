use super::*;
use crate::fixture::{self, floats, header as h, sequence, Model, Section};

const IN_FILE: u32 = 0x20;
const ALIAS: u32 = 0x40;

fn empty() -> Model {
    Model::new()
}

fn parse(m: &Model) -> Result<M2Model> {
    m.parse(&fixture::empty_skin())
}

#[test]
fn minimal_header_parses_to_an_empty_model() {
    let m = parse(&empty()).expect("an all-zero-array header parses");
    assert!(m.vertices.is_empty());
    assert!(m.textures.is_empty());
    assert!(m.materials.is_empty());
    assert!(m.bones.is_empty());
    assert!(m.sequences.is_empty());
    assert!(m.color_alpha_tracks.is_empty());
    assert!(m.transparency_tracks.is_empty());
    assert!(m.raw_data.texture_lookup_table.is_empty());
    assert!(m.raw_data.bounding_triangles.is_empty());
    assert!(m.attachments.is_empty());
    assert!(m.events.is_empty());
    assert!(m.lights.is_empty());
    assert!(m.cameras.is_empty());
    assert!(m.texture_transforms.is_empty());
    assert!(m.skin.batches().is_empty());
    assert!(m.dropped.is_empty());
}

/// Two in-file sequences (100 and 50 ms) and one bone: a translation keyed in the first only, a
/// one-set packed rotation and no scale.
fn two_sequence_model() -> Model {
    let mut m = empty();
    m.array(
        h::SEQUENCES,
        2,
        &[sequence(0, 100, IN_FILE), sequence(4, 50, IN_FILE)].concat(),
    );
    let t = m.track(
        1,
        0xffff,
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
    let r = m.track(1, 0xffff, &[&[(0, packed)]]);
    let s = m.no_track();
    let bone = fixture::bone(-1, 0, [7.0, 8.0, 9.0], [&t, &r, &s]);
    m.array(h::BONES, 1, &bone);
    m
}

#[test]
fn sequences_take_consecutive_bands() {
    let m = parse(&two_sequence_model()).unwrap();
    let bands: Vec<(u32, u32)> = m.sequences.iter().map(|s| (s.start, s.end)).collect();
    assert_eq!(bands, vec![(0, 100), (101, 151)]);
    assert_eq!(m.sequences[1].id, 4);
    assert_eq!(m.sequences[0].variation_next, -1);
}

#[test]
fn a_band_holds_its_end_keys_and_an_empty_one_takes_the_default() {
    let m = parse(&two_sequence_model()).unwrap();
    let tr = &m.bones[0].translation;
    assert_eq!(
        tr.keys,
        vec![
            (0, [1.0, 2.0, 3.0]),
            (10, [1.0, 2.0, 3.0]),
            (90, [4.0, 5.0, 6.0]),
            (100, [4.0, 5.0, 6.0]),
            (101, [0.0, 0.0, 0.0]),
        ]
    );
    assert_eq!(tr.ranges, vec![(0, 3), (4, 4)]);
}

#[test]
fn rotations_unpack_and_a_lone_set_serves_every_sequence() {
    let m = parse(&two_sequence_model()).unwrap();
    let rot = &m.bones[0].rotation;
    let times: Vec<u32> = rot.keys.iter().map(|k| k.0).collect();
    assert_eq!(times, vec![0, 100, 101, 151]);
    assert!(rot.keys.iter().all(|k| k.1 == [1.0, -1.0, 0.0, 0.0]));
    assert_eq!(rot.ranges, vec![(0, 1), (2, 3)]);
    // A track with no keys stays empty: the reader's default, not a written one.
    let sc = &m.bones[0].scale;
    assert!(sc.ranges.is_empty() && sc.keys.is_empty());
    assert_eq!((m.bones[0].pivot.x, m.bones[0].pivot.z), (7.0, 9.0));
    assert_eq!(m.bones[0].parent, -1);
}

/// Patch sequence `i`'s record: its id and variation, flags and `aliasNext`.
fn set_sequence(m: &mut Model, i: usize, id: u16, sub: u16, flags: u32, alias_next: u16) {
    let rec = m.b.u32_at(h::SEQUENCES + 4).unwrap() as usize + i * 64;
    m.b[rec..rec + 2].copy_from_slice(&id.to_le_bytes());
    m.b[rec + 2..rec + 4].copy_from_slice(&sub.to_le_bytes());
    m.b[rec + 12..rec + 16].copy_from_slice(&flags.to_le_bytes());
    m.b[rec + 0x3e..rec + 0x40].copy_from_slice(&alias_next.to_le_bytes());
}

#[test]
fn a_sequence_keyed_in_a_dot_anim_reads_its_keys_there() {
    let mut m = two_sequence_model();
    set_sequence(&mut m, 1, 60, 2, 0, 1);
    assert_eq!(
        anim_paths(&m.b, "Character\\Human\\Male\\HumanMale.m2").unwrap(),
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
    let bone = m.b.u32_at(h::BONES + 4).unwrap() as usize;
    for (heads, ofs) in [(bone + 0x18, 8u32), (bone + 0x20, 16)] {
        let set1 = m.b.u32_at(heads).unwrap() as usize + 8;
        m.b[set1..set1 + 4].copy_from_slice(&2u32.to_le_bytes());
        m.b[set1 + 4..set1 + 8].copy_from_slice(&ofs.to_le_bytes());
    }
    let skin = fixture::empty_skin();
    let model = parse_m2(m.b.clone(), &skin, AnimFiles::from([(1, anim)])).unwrap();
    assert!(model.dropped.is_empty(), "{:?}", model.dropped);
    let tr = &model.bones[0].translation;
    let (lo, hi) = tr.ranges[1];
    assert_eq!(
        tr.keys[lo as usize..=hi as usize].to_vec(),
        vec![
            (101, [1.0; 3]),
            (106, [1.0; 3]),
            (141, [2.0; 3]),
            (151, [2.0; 3]),
        ]
    );
    // Without the file the band keeps the default, and says so.
    let model = parse(&m).unwrap();
    assert_eq!(
        model.bones[0].translation.keys.last(),
        Some(&(101, [0.0; 3]))
    );
    assert!(
        model.dropped.iter().any(|d| d.contains("0060-02.anim")),
        "{:?}",
        model.dropped
    );
}

#[test]
fn an_alias_plays_its_owners_keys() {
    let mut m = two_sequence_model();
    // Sequence 1, as long as sequence 0, aliases it; its own set is empty.
    let rec = m.b.u32_at(h::SEQUENCES + 4).unwrap() as usize + 64;
    m.b[rec + 4..rec + 8].copy_from_slice(&100u32.to_le_bytes());
    set_sequence(&mut m, 1, 136, 0, ALIAS, 0);
    assert!(anim_paths(&m.b, "a.m2").unwrap().is_empty());
    let model = parse(&m).unwrap();
    assert!(model.dropped.is_empty(), "{:?}", model.dropped);
    let tr = &model.bones[0].translation;
    let band = |i: usize| -> Vec<(u32, [f32; 3])> {
        let (lo, hi) = tr.ranges[i];
        tr.keys[lo as usize..=hi as usize]
            .iter()
            .map(|&(t, v)| (t - [0, 101][i], v))
            .collect()
    };
    assert_eq!(band(1), band(0));
    // A chain that never reaches a sequence without the flag keeps no keys.
    set_sequence(&mut m, 1, 136, 0, ALIAS, 1);
    let model = parse(&m).unwrap();
    assert_eq!(
        model.bones[0].translation.keys.last(),
        Some(&(101, [0.0; 3]))
    );
    assert!(
        model.dropped.iter().any(|d| d.contains("no owner")),
        "{:?}",
        model.dropped
    );
}

#[test]
fn tracks_264_reads_the_parse_bands() {
    let m = two_sequence_model();
    let none = AnimFiles::new();
    let mut t = Tracks264::new(&m.b, &none).unwrap();
    assert_eq!(
        t.sequences(),
        vec![((0, 100), IN_FILE), ((101, 151), IN_FILE)]
    );
    let bone = m.b.u32_at(h::BONES + 4).unwrap() as usize;
    let tr = t.vec3(bone + 0x10, [0.0; 3]).unwrap();
    let model = parse(&m).unwrap();
    assert_eq!(tr.ranges, model.bones[0].translation.ranges);
    assert_eq!(tr.keys, model.bones[0].translation.keys);
    let mut v256 = m.b.clone();
    v256[4..8].copy_from_slice(&256u32.to_le_bytes());
    assert!(matches!(
        Tracks264::new(&v256, &none),
        Err(Error::UnsupportedVersion(256))
    ));
}

/// `TanarisTrollGate.m2` swaps its intact and burnt gates with ±1 keys: both fixed16 records,
/// the M2Color alpha and the M2TextureWeight, decode signed.
#[test]
fn a_negative_fixed16_key_decodes_signed() {
    let mut m = empty();
    m.array(h::SEQUENCES, 1, &sequence(0, 1000, IN_FILE));
    let keys = || {
        vec![
            (0u32, 0x7fffu16.to_le_bytes().to_vec()), // +1.0, drawn
            (333, 0x8001u16.to_le_bytes().to_vec()),  // −1.0, hidden
        ]
    };
    let rgb = m.no_track();
    let alpha = m.track(1, 0xffff, &[&keys()]);
    m.array(h::COLORS, 1, &[rgb, alpha].concat());
    let weight = m.track(1, 0xffff, &[&keys()]);
    m.array(h::TEXTURE_WEIGHTS, 1, &weight);
    let model = parse(&m).unwrap();
    let values = |t: &M2ScalarTrack| t.keys.iter().map(|&(_, v)| v).collect::<Vec<_>>();
    let alpha = values(&model.color_alpha_tracks[0]);
    assert!((alpha[0] - 1.0).abs() < 1e-4, "0x7fff is +1.0: {alpha:?}");
    assert!((alpha[1] + 1.0).abs() < 1e-4, "0x8001 is −1.0: {alpha:?}");
    let weight = values(&model.transparency_tracks[0]);
    assert!((weight[1] + 1.0).abs() < 1e-4, "{weight:?}");
}

#[test]
fn texture_transform_tracks_and_lookups_decode() {
    let mut m = empty();
    m.array(h::SEQUENCES, 1, &sequence(0, 1000, IN_FILE));
    let tr = m.track(
        1,
        0xffff,
        &[&[(0, floats(&[0.0; 3])), (1000, floats(&[0.5, 1.0, 0.0]))]],
    );
    let (r, s) = (m.no_track(), m.no_track());
    m.array(h::TEXTURE_TRANSFORMS, 1, &[tr, r, s].concat());
    m.array(h::TEXTURE_TRANSFORM_LOOKUP, 1, &fixture::shorts(&[0]));
    m.array(h::TEXTURE_UNIT_LOOKUP, 1, &fixture::shorts(&[0]));
    m.array(h::TEXTURE_WEIGHT_LOOKUP, 4, &fixture::shorts(&[0, 1, 2, 3]));
    let model = parse(&m).unwrap();
    assert_eq!(
        model.texture_transforms[0].translation.keys,
        vec![(0, [0.0; 3]), (1000, [0.5, 1.0, 0.0])]
    );
    assert_eq!(model.texture_transform_lookup, vec![0]);
    assert_eq!(model.texture_unit_lookup, vec![0]);
    assert_eq!(model.transparency_lookup, vec![0, 1, 2, 3]);
}

fn attachment_record(id: u32, bone: u16, position: [f32; 3]) -> Vec<u8> {
    let mut a = id.to_le_bytes().to_vec();
    a.extend_from_slice(&bone.to_le_bytes());
    a.extend_from_slice(&0u16.to_le_bytes());
    a.extend(floats(&position));
    a.resize(0x28, 0); // the animate track, which nothing reads
    a
}

fn two_bones(m: &mut Model) {
    let (t, r, s) = (m.no_track(), m.no_track(), m.no_track());
    let b0 = fixture::bone(-1, 0, [0.0; 3], [&t, &r, &s]);
    let b1 = fixture::bone(0, 0, [1.0; 3], [&t, &r, &s]);
    m.array(h::BONES, 2, &[b0, b1].concat());
}

#[test]
fn attachments_parse_skip_out_of_range_records_and_resolve_by_lookup() {
    let mut m = empty();
    two_bones(&mut m);
    let recs = [
        attachment_record(1, 0, [1.0, 2.0, 3.0]),  // valid
        attachment_record(70_000, 0, [0.0; 3]),    // id overflows u16: skipped
        attachment_record(2, 5, [9.0; 3]),         // bone out of range: skipped
        attachment_record(17, 1, [0.0, 0.0, 4.5]), // the camera pivot
    ];
    m.array(h::ATTACHMENTS, 4, &recs.concat());
    let mut lookup = vec![0xffffu16; 18];
    lookup[1] = 0;
    lookup[2] = 2;
    lookup[17] = 3;
    m.array(h::ATTACHMENT_LOOKUP, 18, &fixture::shorts(&lookup));
    let model = parse(&m).unwrap();
    assert_eq!(
        model.attachments.len(),
        2,
        "only the in-range records survive"
    );
    assert_eq!(
        model.attachment(1).map(|a| a.position),
        Some([1.0, 2.0, 3.0])
    );
    assert!(
        model.attachment(2).is_none(),
        "a dropped record hangs nothing"
    );
    assert_eq!(model.attachment(17).map(|a| a.bone), Some(1));
    assert_eq!(model.pivot_attach_z, Some(4.5));
}

#[test]
fn events_keep_every_record_with_its_times_and_markers_only_posable_ones() {
    let mut m = empty();
    m.array(
        h::SEQUENCES,
        2,
        &[sequence(0, 100, IN_FILE), sequence(4, 50, IN_FILE)].concat(),
    );
    two_bones(&mut m);
    let event = |m: &mut Model, ident: &[u8; 4], bone: u16, sets: &[&[u32]]| {
        let mut e = ident.to_vec();
        e.extend_from_slice(&7u32.to_le_bytes());
        e.extend_from_slice(&bone.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend(floats(&[1.0, 2.0, 3.0]));
        e.extend(m.track_base(0xffff, sets));
        e
    };
    let a = event(&mut m, b"$CSL", 0, &[&[20], &[10, 60]]);
    let b = event(&mut m, b"$CSR", 5, &[]);
    m.array(h::EVENTS, 2, &[a, b].concat());
    let model = parse(&m).unwrap();
    assert_eq!(model.events.len(), 2);
    let e = &model.events[0];
    assert_eq!((e.data, e.position), (7, [1.0, 2.0, 3.0]));
    // Sequence 1's 60 ms is past its 50 ms length and is dropped.
    assert_eq!(e.times, vec![20, 111]);
    assert_eq!(e.ranges, vec![(0, 0), (1, 1)]);
    assert_eq!(model.event_markers.len(), 1);
    assert_eq!(&model.event_markers[0].ident, b"$CSL");
}

#[test]
fn one_vertex_decodes_all_fields() {
    let mut m = empty();
    let mut vert = floats(&[1.0, 2.0, 3.0]);
    vert.extend_from_slice(&[10, 20, 30, 40, 1, 2, 3, 4]);
    vert.extend(floats(&[4.0, 5.0, 6.0, 0.5, 0.25, 0.75, 0.125]));
    m.array(h::VERTICES, 1, &vert);
    let model = parse(&m).unwrap();
    let v = &model.vertices[0];
    assert_eq!((v.position.x, v.position.y, v.position.z), (1.0, 2.0, 3.0));
    assert_eq!(v.bone_weights, [10, 20, 30, 40]);
    assert_eq!(v.bone_indices, [1, 2, 3, 4]);
    assert_eq!((v.normal.x, v.normal.y, v.normal.z), (4.0, 5.0, 6.0));
    assert_eq!((v.tex_coords.x, v.tex_coords.y), (0.5, 0.25));
    assert_eq!((v.tex_coords2.x, v.tex_coords2.y), (0.75, 0.125));
}

#[test]
fn hostile_counts_err_cleanly_not_oom() {
    for at in [
        h::VERTICES,
        h::ATTACHMENTS,
        h::COLORS,
        h::BONES,
        h::SEQUENCES,
    ] {
        let mut m = empty();
        let end = m.b.len() as u32;
        m.put_arr(at, u32::MAX as usize, end);
        assert!(matches!(parse(&m), Err(Error::Truncated)), "header {at:#x}");
    }
    let mut m = empty();
    m.array(h::VERTICES, 1, &[0u8; 40]); // one vertex declared, 40 of its 48 bytes present
    assert!(matches!(parse(&m), Err(Error::Truncated)));
}

#[test]
fn hostile_shapes_do_not_panic() {
    let skin = fixture::empty_skin();
    assert!(matches!(
        parse_m2(Vec::new(), &skin, AnimFiles::new()),
        Err(Error::NotMd20)
    ));
    let mut m = empty();
    m.b[4..8].copy_from_slice(&256u32.to_le_bytes());
    assert!(matches!(parse(&m), Err(Error::UnsupportedVersion(256))));
    let mut m = empty();
    m.b.truncate(0x30);
    assert!(matches!(parse(&m), Err(Error::Truncated)));
    assert!(matches!(empty().parse(b"NOPE"), Err(Error::NotSkin)));
}

#[test]
fn the_skin_decodes_geometry_batches_and_the_sort_centre() {
    let skin = fixture::skin(
        &[1, 2, 3],
        &[0, 1, 2],
        &[Section {
            id: 1201,
            triangle_start: 0,
            triangle_count: 3,
            sort_center: [1.0, 2.0, 3.0],
            sort_radius: 9.0,
        }],
        &[fixture::batch(&[
            (0x00, 0x0610), // flags 0x10, priority plane 6
            (0x08, 11),
            (0x0a, 2),
            (0x0e, 1),
            (0x10, 5),
            (0x14, 9),
        ])],
    );
    let model = empty().parse(&skin).unwrap();
    let s = &model.skin;
    assert_eq!(*s.indices(), vec![1, 2, 3]);
    assert_eq!(*s.triangles(), vec![0, 1, 2]);
    let sec = &s.submeshes()[0];
    assert_eq!((sec.id, sec.triangle_count), (1201, 3));
    assert_eq!((sec.sort_center, sec.sort_radius), ([1.0, 2.0, 3.0], 9.0));
    let b = &s.batches()[0];
    assert_eq!(b.priority_plane(), 6);
    assert_eq!(
        (b.color_index, b.material_index, b.texture_count),
        (11, 2, 1)
    );
    assert_eq!((b.texture_combo_index, b.weight_combo_index), (5, 9));
}

#[test]
fn a_hostile_skin_section_count_errs_cleanly() {
    let mut skin = fixture::empty_skin();
    let end = skin.len() as u32;
    skin[0x1c..0x20].copy_from_slice(&u32::MAX.to_le_bytes());
    skin[0x20..0x24].copy_from_slice(&end.to_le_bytes());
    assert!(matches!(Skin::parse(&skin), Err(Error::Truncated)));
}

/// A one-camera model on one 1000 ms sequence whose position runs X from 0 to 3, with
/// `outTan[k0] = 1` and `inTan[k1] = 2`.
fn model_with_one_camera(interp: u16) -> Model {
    let mut m = empty();
    m.array(h::SEQUENCES, 1, &sequence(0, 1000, IN_FILE));
    let key = |v: [f32; 3], i: [f32; 3], o: [f32; 3]| [floats(&v), floats(&i), floats(&o)].concat();
    let positions = m.track(
        interp,
        0xffff,
        &[&[
            (0, key([0.0; 3], [0.0; 3], [1.0, 0.0, 0.0])),
            (1000, key([3.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0; 3])),
        ]],
    );
    let (target, roll) = (m.no_track(), m.no_track());
    let mut rec = vec![0u8; 0x10];
    rec[0..4].copy_from_slice(&(-1i32).to_le_bytes());
    rec[4..8].copy_from_slice(&std::f32::consts::FRAC_PI_4.to_le_bytes());
    rec[8..12].copy_from_slice(&27.777779f32.to_le_bytes());
    rec[12..16].copy_from_slice(&0.22222222f32.to_le_bytes());
    rec.extend(positions);
    rec.extend(floats(&[10.0, 20.0, 30.0]));
    rec.extend(target);
    rec.extend(floats(&[40.0, 50.0, 60.0]));
    rec.extend(roll);
    assert_eq!(rec.len(), 0x64);
    m.array(h::CAMERAS, 1, &rec);
    m.array(h::CAMERA_LOOKUP, 1, &fixture::shorts(&[0]));
    m
}

#[test]
fn camera_record_fields_and_lookup_parse() {
    let model = parse(&model_with_one_camera(2)).unwrap();
    assert_eq!(model.cameras.len(), 1);
    let c = &model.cameras[0];
    assert_eq!(c.camera_type, -1);
    assert!((c.fov - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    assert!((c.far_clip - 27.777779).abs() < 1e-4);
    assert!((c.near_clip - 0.22222222).abs() < 1e-7);
    assert_eq!(c.position_base, [10.0, 20.0, 30.0]);
    assert_eq!(c.target_base, [40.0, 50.0, 60.0]);
    assert_eq!(c.positions.keys.len(), 2);
    assert!(c.target.keys.is_empty());
    assert_eq!(model.camera_lookup, vec![0]);
}

#[test]
fn cubic_sampler_takes_all_four_interp_legs() {
    // At the midpoint, hand-computed from the bases in `M2Track::sample_ms`:
    //   step   → value[k0]                                              = 0
    //   linear → 0 + (3−0)·0.5                                          = 1.5
    //   Bézier → 0.125·0 + 0.375·1 + 0.375·2 + 0.125·3                  = 1.5
    //   Hermite→ 0.5·0 + 0.125·1 + 0.5·3 + (−0.125)·2                   = 1.375
    for (interp, want) in [(0u16, 0.0f32), (1, 1.5), (2, 1.5), (3, 1.375)] {
        let model = parse(&model_with_one_camera(interp)).unwrap();
        let cam = &model.cameras[0];
        let got = cam.positions.sample_ms(500).expect("keyed track samples")[0];
        assert!(
            (got - want).abs() < 1e-5,
            "interp {interp}: got {got}, want {want}"
        );
        // End-clamped at both ends, on every leg.
        assert_eq!(cam.positions.sample_ms(0).unwrap()[0], 0.0);
        assert_eq!(cam.positions.sample_ms(9_999).unwrap()[0], 3.0);
    }
}

#[test]
fn lights_read_their_seven_tracks() {
    let mut m = empty();
    m.array(h::SEQUENCES, 1, &sequence(0, 100, IN_FILE));
    let mut rec = 1u16.to_le_bytes().to_vec();
    rec.extend_from_slice(&(-1i16).to_le_bytes());
    rec.extend(floats(&[1.0, 2.0, 3.0]));
    let colour = m.track(1, 0xffff, &[&[(0, floats(&[0.5, 0.25, 1.0]))]]);
    let intensity = m.track(1, 0xffff, &[&[(0, floats(&[2.0]))]]);
    let visible = m.track(0, 0xffff, &[&[(0, vec![0])]]);
    rec.extend(m.no_track());
    rec.extend(m.no_track());
    rec.extend(colour);
    rec.extend(intensity);
    rec.extend(m.no_track());
    rec.extend(m.no_track());
    rec.extend(visible);
    m.array(h::LIGHTS, 1, &rec);
    let model = parse(&m).unwrap();
    let l = &model.lights[0];
    assert_eq!((l.light_type, l.bone, l.position), (1, -1, [1.0, 2.0, 3.0]));
    assert_eq!(l.diffuse_color.keys.first(), Some(&(0, [0.5, 0.25, 1.0])));
    assert_eq!(l.diffuse_intensity.keys.first(), Some(&(0, 2.0)));
    assert_eq!(l.visibility.keys.first(), Some(&(0, 0.0)));
    assert!(l.ambient_color.keys.is_empty());
}

#[test]
fn combiner_ops_are_read_under_flag_8_alone() {
    let mut m = empty();
    m.array(
        h::TEXTURE_COMBINER_COMBOS,
        2,
        &fixture::shorts(&[0x4014, 0x0010]),
    );
    assert!(parse(&m).unwrap().texture_combiner_combos.is_empty());
    m.flags(8);
    assert_eq!(
        parse(&m).unwrap().texture_combiner_combos,
        vec![0x4014, 0x0010]
    );
}

#[test]
fn the_skin_sits_beside_the_model() {
    assert_eq!(
        skin_path("interface/glues/models/ui_x/ui_x.m2"),
        "interface/glues/models/ui_x/ui_x00.skin"
    );
    assert_eq!(skin_path("A\\B.M2"), "A\\B00.skin");
}
