//! The per-batch keys the transparent order and the second texture stage read off 3.3.5a models:
//! the priority plane, the section sort centre and global flag `0x10` (#435, #336), and a
//! combiner's stage-1 texture transform (#433).

use benilla_formats::{open_chain, parse_m2_render_submeshes, read_m2};

const NORTHREND: &str = "Interface\\Glues\\Models\\UI_MainMenu_Northrend\\UI_MainMenu_Northrend.m2";
const DEATH_KNIGHT: &str = "Interface\\Glues\\Models\\UI_DeathKnight\\UI_DeathKnight.m2";
const NEBULA_SKY: &str = "Environments\\Stars\\NexusRaid_NebulaSky.M2";

/// The glue scenes share one sort depth (global flag `0x10`), so their planes decide; the login
/// scene's sky bowl sits on plane 6, over its clouds on planes 1-4.
#[test]
fn glue_scenes_rank_by_plane_under_one_model_depth() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    for path in [NORTHREND, DEATH_KNIGHT] {
        let model = read_m2(&chain, path).unwrap_or_else(|e| panic!("{path}: {e:#}"));
        assert_ne!(
            model.global_flags & 0x10,
            0,
            "{path} sorts on the model's depth"
        );
        let subs = parse_m2_render_submeshes(&model, "", &[]).expect("submeshes");
        let keys: Vec<_> = subs.iter().filter_map(|s| s.sort).collect();
        assert_eq!(
            keys.len(),
            subs.len(),
            "{path}: every M2 batch carries its key"
        );
        assert!(keys.iter().all(|k| k.model_depth), "{path}");
        let planes: std::collections::BTreeSet<i8> =
            keys.iter().map(|k| k.priority_plane).collect();
        assert!(
            planes.len() > 1,
            "{path} spreads its batches over planes: {planes:?}"
        );
    }
    let model = read_m2(&chain, NORTHREND).expect("the login scene");
    let planes: Vec<i8> = parse_m2_render_submeshes(&model, "", &[])
        .expect("submeshes")
        .iter()
        .filter_map(|s| s.sort.map(|k| k.priority_plane))
        .collect();
    assert!(planes.contains(&6), "the sky bowl's plane: {planes:?}");
    assert!(
        planes.iter().any(|p| (1..=4).contains(p)),
        "the clouds': {planes:?}"
    );
}

/// Every batch's key names its section's sort centre and radius from the skin profile.
#[test]
fn a_batch_key_carries_its_sections_sort_centre() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let model = read_m2(&chain, NORTHREND).expect("the login scene");
    let sections = model.skin.submeshes();
    for s in parse_m2_render_submeshes(&model, "", &[]).expect("submeshes") {
        let (Some(key), Some(section)) = (s.sort, s.section) else {
            panic!("an M2 batch has a key and a section");
        };
        let sec = &sections[usize::from(section)];
        assert_eq!(key.center, sec.sort_center);
        assert_eq!(key.radius, sec.sort_radius);
    }
}

/// The Eye of Eternity's nebula sky scrolls its second texture on its own transform; the login
/// scene's stage-1 transforms are all `0xffff`, so none of its batches carries one.
#[test]
fn a_second_stage_carries_its_own_texture_transform() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let sky = read_m2(&chain, NEBULA_SKY).expect("the nebula sky");
    let subs = parse_m2_render_submeshes(&sky, "", &[]).expect("submeshes");
    assert!(
        subs.iter()
            .filter_map(|s| s.combiner.as_ref())
            .any(|c| c.uv1.is_some() && !c.stage1_env),
        "a two-stage batch animates its second stage"
    );
    let login = read_m2(&chain, NORTHREND).expect("the login scene");
    assert!(parse_m2_render_submeshes(&login, "", &[])
        .expect("submeshes")
        .iter()
        .filter_map(|s| s.combiner.as_ref())
        .all(|c| c.uv1.is_none()));
}
