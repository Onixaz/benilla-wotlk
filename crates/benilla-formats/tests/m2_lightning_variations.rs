//! The New Agamand apothecary's lightning (`Apothecary_FX.m2`, a doodad of
//! `ND_Forsaken_Apothecary.wmo`) flashes by colour alpha alone: five variations of animation id
//! 0, none of which moves a bone, each with its own alpha keys. A placement that arms no player
//! reads slot 0 forever, a 167 ms window, and the strike never changes.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_render_submeshes};

const LIGHTNING: &str = "World\\Expansion02\\Doodads\\HowlingFjord\\Forsaken\\Apothecary_FX.m2";

#[test]
fn the_apothecary_lightning_is_five_boneless_alpha_variations() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open the patch chain");
    let bytes =
        benilla_formats::read_m2(&chain, LIGHTNING).expect("the lightning M2 is in the chain");

    let anims = parse_m2_animations(&bytes);
    assert_eq!(anims.len(), 5, "five sequences");
    assert!(
        anims.iter().all(|a| a.anim_id == 0),
        "one Stand variation chain"
    );
    assert!(
        anims.iter().all(|a| a.frequency > 0),
        "every variation is rollable: {:?}",
        anims.iter().map(|a| a.frequency).collect::<Vec<_>>()
    );
    assert!(
        anims.iter().all(|a| a.bones.is_empty()),
        "no variation poses a bone, so the bone gate alone never arms a player"
    );

    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    let differing = subs
        .iter()
        .filter_map(|s| s.alpha_anim.as_ref())
        .filter(|a| a.slots().windows(2).any(|w| w[0] != w[1]))
        .count();
    assert!(
        differing > 0,
        "a batch's alpha differs between variations: the flash is the armed sequence's"
    );
}
