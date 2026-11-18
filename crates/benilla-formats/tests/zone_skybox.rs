//! The zone skies on the shipped data: Outland and Northrend lights name `LightSkybox.dbc` models,
//! the old continents none; the blend hands them out by weight and each loads as an animated sky.

use benilla_formats::{Chain, LightCatalog, Submersion, SKYBOX_FLAG_SYNC_TIME};

const MAP_EK: u32 = 0;
const MAP_OUTLAND: u32 = 530;
const MAP_NORTHREND: u32 = 571;
/// Goldshire: the Eastern Kingdoms name no zone sky at all.
const GOLDSHIRE: [f32; 3] = [-9464.0, 62.0, 56.0];
/// Light 377's centre in Hellfire Peninsula (falloff 1175-1284 yd): `HellfireSkyBox.mdx`.
const HELLFIRE: [f32; 3] = [-213.0, 3958.5, 29.0];
/// Light 1802's centre over Dalaran (falloff 405-477 yd): `DalaranSkyBox.mdx`.
const DALARAN: [f32; 3] = [5797.1, 666.3, 775.5];
/// Light 1818's centre in the Storm Peaks (falloff 10-14 yd): `StormPeaks_SkyA.mdx`, flags 1.
const STORM_PEAKS_A: [f32; 3] = [7708.7, -475.6, 848.5];
/// K3, deep inside the Storm Peaks zone outline: zone light 1777, `StormPeaks_SkyC_Ulduar.mdx`.
const K3: [f32; 3] = [6150.0, -1050.0, 410.0];
/// Star's Rest, inside Dragonblight's outline: zone light 825, `AuroraYellowGreen.mdx`.
const STARS_REST: [f32; 3] = [3500.0, 2000.0, 64.0];

fn chain() -> Option<Chain> {
    let data = benilla_formats::wow_data_or_skip!(None);
    Some(Chain::open(&data).expect("open the patch chain"))
}

fn names(cat: &LightCatalog, map: u32, pos: [f32; 3]) -> Vec<(String, f32, u32)> {
    cat.skybox_blend(map, pos, Submersion::Dry, &[])
        .into_iter()
        .map(|d| (d.path.to_ascii_lowercase(), d.weight, d.flags))
        .collect()
}

#[test]
fn the_old_continents_name_no_zone_sky_and_outland_and_northrend_do() {
    let Some(mut chain) = chain() else { return };
    let cat = LightCatalog::load(&mut chain).expect("load the light tables");

    assert!(
        names(&cat, MAP_EK, GOLDSHIRE).is_empty(),
        "Elwynn draws the gradient dome alone"
    );

    let hellfire = names(&cat, MAP_OUTLAND, HELLFIRE);
    assert!(
        hellfire
            .iter()
            .any(|(p, w, _)| p.contains("hellfireskybox") && *w > 0.99),
        "Hellfire's sky at full weight: {hellfire:?}"
    );

    let dalaran = names(&cat, MAP_NORTHREND, DALARAN);
    assert!(
        dalaran
            .iter()
            .any(|(p, w, _)| p.contains("dalaranskybox") && *w > 0.99),
        "Dalaran's sky at full weight: {dalaran:?}"
    );

    let peaks = names(&cat, MAP_NORTHREND, STORM_PEAKS_A);
    let a = peaks
        .iter()
        .find(|(p, _, _)| p.contains("stormpeaks_skya"))
        .unwrap_or_else(|| panic!("Storm Peaks sky A at its light: {peaks:?}"));
    assert!(
        a.2 & SKYBOX_FLAG_SYNC_TIME != 0,
        "StormPeaks_SkyA is pinned to the time of day"
    );
    assert!(peaks.len() <= 3, "at most three blend skies");

    // The zone lights (`0x77eed0`): no area sphere covers these spots.
    let k3 = names(&cat, MAP_NORTHREND, K3);
    assert!(
        k3.iter()
            .any(|(p, w, _)| p.contains("stormpeaks_skyc_ulduar") && *w > 0.99),
        "the Storm Peaks zone sky at K3: {k3:?}"
    );
    let rest = names(&cat, MAP_NORTHREND, STARS_REST);
    assert!(
        rest.iter()
            .any(|(p, w, f)| p.contains("aurorayellowgreen") && *w > 0.99 && f & 2 != 0),
        "Dragonblight's aurora at Star's Rest: {rest:?}"
    );
}

#[test]
fn a_zone_sky_loads_as_an_animated_model() {
    let Some(mut chain) = chain() else { return };
    let (subs, rig) =
        benilla_formats::load_sky_model(&mut chain, "ENVIRONMENTS\\Stars\\StormPeaks_SkyA.mdx")
            .expect("load the Storm Peaks sky");
    assert!(!subs.is_empty(), "it has batches");
    // One 160 s sequence, file slot 0, looping (flags 0x20).
    assert_eq!(rig.slot, Some(0));
    assert!((rig.length - 160.0).abs() < 1e-3, "length {}", rig.length);
    assert!(rig.looping);
    // Its cloud bones turn on a global sequence, and its layers swap by texture weight.
    assert!(rig.moving_bones().iter().any(|&m| m), "some bone moves");
    assert!(
        subs.iter().any(|s| s.alpha_anim.is_some()),
        "a batch's colour alpha or texture weight is animated"
    );
    assert!(
        subs.iter().any(|s| s.uv_anim.is_some()),
        "a batch's texture scrolls"
    );
    // The pose moves a vertex somewhere in the day.
    let moved = (0..8).any(|k| {
        let t = rig.length * k as f32 / 8.0;
        rig.pose(t, f64::from(t)) != rig.pose(0.0, 0.0)
    });
    assert!(moved, "the bones move over the sequence");
}
