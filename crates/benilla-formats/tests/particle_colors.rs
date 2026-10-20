//! A display's `ParticleColorID` recolours the model's emitters whose `particleColorIndex` is 11,
//! 12 or 13 (#119): their colour keys become that set's start, mid and end.

use benilla_formats::{load_particle_colors, open_chain, parse_m2_particle_emitters, read_m2};

#[test]
fn the_wisps_emitters_take_their_sets_colours() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open chain");
    let table = load_particle_colors(&mut chain).expect("ParticleColor.dbc");
    let colors = (1..4096)
        .filter_map(|id| table.resolve(id))
        .find(|c| *c != benilla_formats::ParticleColors::MISSING)
        .expect("a ParticleColor row");
    assert_eq!(table.resolve(0), None, "id 0 overrides nothing");

    let wisp = read_m2(&chain, "Creature\\WISP\\Wisp.M2").expect("the wisp");
    let emitters = parse_m2_particle_emitters(&wisp).expect("its emitters");
    let mut seen = [false; 3];
    for e in &emitters {
        match e.recolored(&colors) {
            Some(r) => {
                let set = usize::from(e.color_index - 11);
                seen[set] = true;
                assert_eq!(r.over_life.color.values, colors.0[set].to_vec());
                assert_eq!(r.over_life.color.times, e.over_life.color.times);
            }
            None => assert!(!(11..=13).contains(&e.color_index)),
        }
    }
    assert!(
        seen[0] && seen[1] && seen[2],
        "the wisp uses all three sets: {seen:?}"
    );
}
