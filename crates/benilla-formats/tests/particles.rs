//! The M2 particle-emitter parser against shipped models.

use benilla_formats::{open_chain, parse_m2_particle_emitters, ParticleBlend};

/// File `+0x188` and `+0x18c` are twinkleScale `{min, max}`, a per-frame size flicker skipped when
/// the range is degenerate, not a spawn-time multiplier: the kobold candle authors `{0, 0}` and
/// burns in the reference.
#[test]
fn twinkle_fields_gate_not_scale() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open vanilla patch chain");

    let kobold = parse_m2_particle_emitters(
        &benilla_formats::read_m2(&chain, "Creature\\Kobold\\Kobold.m2").expect("read Kobold.m2"),
    )
    .expect("parse kobold emitters");
    assert_eq!(kobold.len(), 1, "kobold has one candle emitter");
    let candle = &kobold[0];
    assert_eq!((candle.twinkle_min, candle.twinkle_max), (0.0, 0.0));
    assert_eq!(
        candle.twinkle(0.7),
        1.0,
        "degenerate {{0,0}} twinkle is identity — the candle burns at ramp size"
    );
    assert!(
        candle.over_life.sample(0.5).size[0] > 0.0,
        "the candle flame's over-life size ramp is nonzero"
    );

    let campfire = parse_m2_particle_emitters(
        &benilla_formats::read_m2(
            &chain,
            "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2",
        )
        .expect("read ElwynnCampfire.m2"),
    )
    .expect("parse campfire emitters");
    let glow = &campfire[0];
    assert_eq!((glow.twinkle_min, glow.twinkle_max), (0.0, 1.0));
    assert_eq!(glow.twinkle(0.25), 0.25, "active range lerps min..max");
    assert!(glow.twinkle_percent.is_finite() && glow.twinkle_speed.is_finite());
}

/// The file-to-runtime flag remap (`0x833cf8`-`0x833ddc`): file bit 0x10 (runtime 0x200) is model
/// space, file 0x20 (runtime 0x400) scales size by the placement. The chandelier's flames (0x11,
/// 0x15) ride its swing; the campfire (0x21, 0x29) scales.
#[test]
fn flag_remap_reads_the_file_bits() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open vanilla patch chain");

    let kobold = parse_m2_particle_emitters(
        &benilla_formats::read_m2(&chain, "Creature\\Kobold\\Kobold.m2").unwrap(),
    )
    .unwrap();
    assert!(!kobold[0].model_space() && !kobold[0].scale_size_by_instance());

    let chandelier = parse_m2_particle_emitters(
        &benilla_formats::read_m2(
            &chain,
            "World\\Dungeon\\GoldshireInn\\InnChandelier\\InnChandelier.m2",
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        chandelier.iter().take(6).all(|e| e.model_space()),
        "the swinging candle flames are model-space (file bit 0x10)"
    );

    let campfire = parse_m2_particle_emitters(
        &benilla_formats::read_m2(
            &chain,
            "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2",
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        campfire
            .iter()
            .all(|e| e.scale_size_by_instance() && !e.model_space()),
        "campfire: size-by-scale (0x20) set, model-space (0x10) clear"
    );
}

/// `G_BarrelExplode.m2` (GameObject 20737): all 7 emitters fire inside the one-shot clips (slot 0,
/// anim 157; slot 3, anim 150 Destroy) and are off in both idles (slot 1 Stand, slot 2 Closed).
#[test]
fn barrel_explode_emitters_are_off_at_rest_and_fire_in_their_clips() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open vanilla patch chain");
    let emitters = parse_m2_particle_emitters(
        &benilla_formats::read_m2(&chain, "World\\Goober\\G_BarrelExplode.m2")
            .expect("read G_BarrelExplode.m2"),
    )
    .expect("parse emitters");
    assert_eq!(emitters.len(), 7, "the barrel authors seven emitters");
    for (i, e) in emitters.iter().enumerate() {
        assert!(e.timing.peak_rate() > 0.0, "emitter {i} can emit");
        for slot in [1usize, 2] {
            for t in [0.0f32, 0.1, 0.3, 5.0] {
                assert!(
                    !e.timing.emitting(Some(slot), t, 0.0),
                    "emitter {i} must be OFF at rest (slot {slot}, t {t})"
                );
            }
        }
        // The explode clip, slot 0, runs 1 s and clamps.
        let fires = (0..100).any(|k| e.timing.emitting(Some(0), k as f32 * 0.01, 0.0));
        assert!(fires, "emitter {i} fires inside the explode clip");
        assert!(
            !e.timing.emitting(Some(0), 1.0, 0.0),
            "emitter {i} must be off at the clip end"
        );
        assert!(
            !e.timing.emitting(Some(0), 30.0, 0.0),
            "emitter {i} must stay off parked past the clip"
        );
    }
    let destroy_fires = emitters
        .iter()
        .any(|e| (0..100).any(|k| e.timing.emitting(Some(3), k as f32 * 0.01, 0.0)));
    assert!(destroy_fires, "the Destroy clip plays the explosion");
}

/// Particle `blendingType` 1 is AlphaKey, not Opaque: blending off under `glAlphaFunc(GEQUAL,
/// 224/255)`, which cuts `PARTROCK.BLP`'s silhouette. The earth elemental is the one creature that
/// authors it, and nothing in 1.12.1 authors mode 0.
#[test]
fn earth_elemental_debris_is_alphakey_not_opaque() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = benilla_formats::read_m2(&chain, "Creature\\ElementalEarth\\ElementalEarth.m2")
        .expect("read ElementalEarth.m2");

    let emitters = parse_m2_particle_emitters(&bytes).expect("parse emitters");
    assert_eq!(emitters.len(), 13, "the elemental authors 13 emitters");

    // Emitters 3 and 12 are the rock chips; 11 is the third PARTROCK emitter, authored mode 2.
    let rock = |e: &benilla_formats::ParticleEmitterDef| {
        e.texture
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("CREATURE\\ELEMENTALEARTH\\PARTROCK.BLP"))
    };
    assert!(rock(&emitters[3]) && rock(&emitters[11]) && rock(&emitters[12]));
    assert_eq!(emitters[3].blend, ParticleBlend::AlphaKey);
    assert_eq!(emitters[12].blend, ParticleBlend::AlphaKey);
    assert_eq!(emitters[11].blend, ParticleBlend::Alpha);

    assert!(
        !emitters.iter().any(|e| e.blend == ParticleBlend::Opaque),
        "no emitter here folds to Opaque"
    );
}
