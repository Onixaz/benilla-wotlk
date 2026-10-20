//! Every chain loader reads a 3.3.5a (version 264) M2 through `read_m2`: Elwynn's ground-clutter
//! tufts, the night star dome and a doodad's bounds and hull all decode.

use benilla_formats::{
    load_m2_bone_spins, load_m2_bounds, load_m2_collision_hull, load_m2_mesh, md20_version,
    open_chain, VERSION_264,
};

/// Northshire's ground-effect detail models (grass, flowers) and the night sky's star dome.
const MODELS: [&str; 3] = [
    "World\\NoDXT\\Detail\\ElwGra01.m2",
    "World\\NoDXT\\Detail\\ElwFlo01.m2",
    "Environments\\Stars\\Stars.m2",
];

#[test]
fn version_264_models_decode_through_the_chain_loaders() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open the patch chain");

    for path in MODELS {
        let raw = chain
            .read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e:#}"));
        assert_eq!(
            md20_version(&raw),
            Some(VERSION_264),
            "{path} is a 3.3.5a model"
        );

        let subs = load_m2_mesh(&mut chain, path).unwrap_or_else(|e| panic!("mesh {path}: {e:#}"));
        assert!(
            subs.iter().any(|s| s.indices.len() >= 3),
            "{path} must build at least one triangle"
        );
        load_m2_bone_spins(&mut chain, path).unwrap_or_else(|e| panic!("spins {path}: {e:#}"));
        load_m2_bounds(&mut chain, path).unwrap_or_else(|e| panic!("bounds {path}: {e:#}"));
        load_m2_collision_hull(&mut chain, path).unwrap_or_else(|e| panic!("hull {path}: {e:#}"));
    }
}
