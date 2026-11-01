//! A server light override (`SMSG_OVERRIDE_LIGHT`) blends its row's params inside the base light's
//! own sphere, capped at its fade (`0x7f1360`, `0x7ee5d0`); a zero fade, an unknown row or a point
//! outside the sphere changes nothing.

use benilla_formats::{Chain, LightCatalog, LightOverride, Submersion};

const MAP_EK: u32 = 0;
const NOON: u32 = 1440;
/// Inside light 28's sphere (falloff 805-1818 yd), 1351 yd out at the Tirisfal-Silverpine
/// border, where it weighs 0.461.
const IN_LIGHT_28: [f32; 3] = [1401.03, 628.90, 35.42];
/// Light 44's params, a grey ambient light 28 does not have.
const OTHER: u32 = 44;
/// Goldshire, far outside it.
const GOLDSHIRE: [f32; 3] = [-9464.0, 62.0, 56.0];

fn catalog() -> Option<LightCatalog> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = Chain::open(&data).expect("open the patch chain");
    Some(LightCatalog::load(&mut chain).expect("load the light tables"))
}

fn at(cat: &LightCatalog, pos: [f32; 3], overrides: &[LightOverride]) -> [f32; 3] {
    cat.sample_blended_with(MAP_EK, pos, NOON, false, Submersion::Dry, false, overrides)
        .ambient
}

#[test]
fn an_override_blends_only_inside_its_light_and_by_its_fade() {
    let Some(cat) = catalog() else { return };
    let o = |light, override_id, fade| LightOverride {
        light,
        override_id,
        fade,
    };
    let base = at(&cat, IN_LIGHT_28, &[]);
    assert_eq!(at(&cat, IN_LIGHT_28, &[o(28, OTHER, 0.0)]), base, "no fade");
    assert_eq!(
        at(&cat, IN_LIGHT_28, &[o(28, 999_999, 1.0)]),
        base,
        "no such row"
    );
    assert_eq!(
        at(&cat, GOLDSHIRE, &[o(28, OTHER, 1.0)]),
        at(&cat, GOLDSHIRE, &[]),
        "outside the base light's sphere"
    );
    let full = at(&cat, IN_LIGHT_28, &[o(28, OTHER, 1.0)]);
    assert_ne!(full, base, "inside, the override's row blends in");
    let half = at(&cat, IN_LIGHT_28, &[o(28, OTHER, 0.000_1)]);
    let moved = |a: [f32; 3]| (0..3).map(|i| (a[i] - base[i]).abs()).sum::<f32>();
    assert!(
        moved(half) <= moved(full),
        "a smaller fade moves it no further"
    );
}
