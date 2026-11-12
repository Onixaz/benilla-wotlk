//! The beam's spawn rules against the reference, and the hop array's consumption.

use std::collections::HashMap;

use benilla_formats::{
    char_proc_type, ChainProc, CharProc, SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualKit,
    VisualStages,
};

use super::*;
use crate::net::ObjectStore;

/// Chain Lightning (`benilla-extract <Data> spellvis 421`): visual 36.
const CHAIN_SPELL: u32 = 421;
const CHAIN_VISUAL: u32 = 36;
/// The fixture visual's cast kit, with its beam slot, and its impact area kit.
const BEAM_KIT: u32 = 900;
const AREA_KIT: u32 = 901;
/// A row that staggers its hops: 300 ms delay, 40 ms between hops.
const STAGGERED: u32 = 61;
/// A row whose combo names two further rows.
const COMBO: u32 = 70;
/// The combo's rows.
const COMBO_A: u32 = 71;
const COMBO_B: u32 = 72;

/// A synthetic row: 300 ms delay, 1 s per hop, no stagger.
fn lightning() -> ChainEffect {
    ChainEffect {
        avg_seg_len: 2.78,
        half_width: 0.5,
        noise_scale: 0.04,
        tex_scroll_speed: 1.0,
        bolt_life_ms: 1000,
        first_bolt_delay_ms: 300,
        bolt_stagger_ms: 0,
        texture: "Textures\\SpellChainEffects\\Lightning.blp".into(),
        rgba: [255; 4],
        blend_mode: 3,
        ..Default::default()
    }
}

fn beam_app() -> App {
    beam_app_area_flags(0)
}

/// [`beam_app`] with the area kit's flags.
fn beam_app_area_flags(area_flags: u32) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
    app.init_asset::<Image>();
    app.init_resource::<GuidIndex>();
    app.init_resource::<ChainLocations>();
    app.add_message::<ChainProcPlay>()
        .add_message::<SpellEffectClear>()
        .add_message::<KitEffectClear>()
        .add_message::<SpellGoTargets>()
        .add_message::<DelayedAreaKit>();
    app.insert_resource(SpellVisuals(
        SpellVisualCatalog::from_tables(
            HashMap::from([(
                CHAIN_VISUAL,
                VisualStages {
                    // Column 10's ordinal: missile-table index 1 is attachment 34.
                    missile_attach: 1,
                    cast: BEAM_KIT,
                    impact_area_kit: AREA_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([
                (
                    BEAM_KIT,
                    VisualKit {
                        id: BEAM_KIT,
                        char_proc_slots: [
                            Some(CharProc {
                                ty: char_proc_type::CHAIN_CAST,
                                params: [1.0, 0.0, 0.0, 0.0],
                            }),
                            None,
                            None,
                            None,
                        ],
                        ..Default::default()
                    },
                ),
                (
                    AREA_KIT,
                    VisualKit {
                        id: AREA_KIT,
                        flags: area_flags,
                        ..Default::default()
                    },
                ),
            ]),
        )
        .with_chain_effect(1, lightning())
        .with_chain_effect(
            STAGGERED,
            ChainEffect {
                bolt_life_ms: 203,
                bolt_stagger_ms: 40,
                ..lightning()
            },
        )
        .with_chain_effect(
            COMBO,
            ChainEffect {
                combo: vec![COMBO_A, COMBO_B],
                ..lightning()
            },
        )
        .with_chain_effect(
            COMBO_A,
            ChainEffect {
                bolt_life_ms: 500,
                ..lightning()
            },
        )
        .with_chain_effect(COMBO_B, lightning()),
    ));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            CHAIN_SPELL,
            SpellDisplay {
                visual: CHAIN_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, spawn_chain_beams);
    app
}

/// Index a test object under a guid of its own, as the object manager does.
fn streamed(app: &mut App, store: ObjectStore) -> Entity {
    let e = app.world_mut().spawn(store).id();
    let guid = 0xF130_0000_0000_0000 | u64::from(e.index_u32());
    app.world_mut().entity_mut(e).insert(Guid(guid));
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(guid, e);
    e
}

/// A streamed unit: an empty descriptor store is all the spawner reads off a target.
fn unit(app: &mut App) -> Entity {
    streamed(
        app,
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
    )
}

/// A beam's nodes as the entities their guids name now.
fn node_entities(app: &App, nodes: &[u64]) -> Vec<Entity> {
    let index = app.world().resource::<GuidIndex>();
    nodes
        .iter()
        .map(|g| index.0.get(g).copied().unwrap_or(Entity::PLACEHOLDER))
        .collect()
}

/// A cast-stage play of chain id 1, flag clear.
fn play(entity: Entity, spell_id: u32) -> ChainProcPlay {
    ChainProcPlay {
        entity,
        spell_id,
        proc: ChainProc {
            effect_id: 1,
            flag: false,
            reverse: false,
            ty: char_proc_type::CHAIN_CAST,
        },
        kit_id: BEAM_KIT,
        from: None,
    }
}

/// A streamed unit whose store says so (`OBJECT_FIELD_TYPE` 0x9), which the guid arm resolves.
fn typed_unit(app: &mut App) -> Entity {
    streamed(
        app,
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)])),
    )
}

/// Every live beam as `(nodes, [(hop start, hop end)], expiry, persistent)`.
#[allow(clippy::type_complexity)]
fn beams(app: &mut App) -> Vec<(Vec<Entity>, Vec<(f32, f32)>, f32, bool)> {
    app.world_mut()
        .query::<&ChainBeam>()
        .iter(app.world())
        .map(|b| {
            (
                node_entities(app, &b.nodes),
                b.bolts.iter().map(|x| (x.start, x.end)).collect(),
                b.expiry,
                b.persistent,
            )
        })
        .collect()
}

/// Every live beam's bolts as `(source node, target node)` pairs.
fn hops(app: &mut App) -> Vec<Vec<(usize, usize)>> {
    app.world_mut()
        .query::<&ChainBeam>()
        .iter(app.world())
        .map(|b| b.bolts.iter().map(|x| (x.source, x.target)).collect())
        .collect()
}

/// The single-target path takes the channel object only with at most one hop.
#[test]
fn target_selection_prefers_the_channel_object_only_on_its_own_narrow_path() {
    use benilla_protocol::ObjectFields;
    const CHANNEL_SPELL: u32 = 689; // Drain Life
    const OBJECT_GUID: u64 = 0xABC;
    let victim = Entity::from_raw_u32(7).expect("a test entity id");
    let hop = Entity::from_raw_u32(8).expect("a test entity id");
    let mut index = GuidIndex::default();
    index.0.insert(OBJECT_GUID, victim);
    // Field 20 = UNIT_FIELD_CHANNEL_OBJECT (a guid pair), 144 = UNIT_CHANNEL_SPELL.
    let channelling = ObjectStore(ObjectFields::from_pairs(&[
        (20, OBJECT_GUID as u32),
        (21, (OBJECT_GUID >> 32) as u32),
        (144, CHANNEL_SPELL),
    ]));
    let idle = ObjectStore(ObjectFields::from_pairs(&[]));

    assert_eq!(
        select_targets(CHANNEL_SPELL, None, &channelling, &index),
        vec![victim],
        "no hops + a live channel object ⇒ the channel object"
    );
    assert_eq!(
        select_targets(
            CHANNEL_SPELL,
            Some(&ChainHops(vec![hop])),
            &channelling,
            &index
        ),
        vec![victim],
        "…and `<= 1` is unsigned `jbe`: ONE hop still takes the channel path"
    );
    assert_eq!(
        select_targets(
            CHANNEL_SPELL,
            Some(&ChainHops(vec![hop, victim])),
            &channelling,
            &index
        ),
        vec![hop, victim],
        "two hops outgrow the single-target path and the array wins"
    );
    assert_eq!(
        select_targets(421, Some(&ChainHops(vec![hop])), &channelling, &index),
        vec![hop],
        "a DIFFERENT spell's kit never takes the channel object"
    );
    assert!(
        select_targets(CHANNEL_SPELL, None, &idle, &index).is_empty(),
        "no channel, no hops ⇒ nothing is drawn"
    );
}

#[test]
fn a_play_over_a_filled_hop_array_builds_the_staggered_polyline_once() {
    let mut app = beam_app();
    let (caster, t1, t2) = (unit(&mut app), unit(&mut app), unit(&mut app));
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1, t2]));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();

    let live = beams(&mut app);
    assert_eq!(live.len(), 1, "one beam, not one per hop");
    let (nodes, bolts, expiry, persistent) = &live[0];
    assert_eq!(
        nodes,
        &vec![caster, t1, t2],
        "caster → t1 → t2, in wire order"
    );
    assert_eq!(bolts.len(), 2, "two hops for two targets");
    // id 1's row: 1000 ms of life per hop, both lit together after the delay.
    assert_eq!(bolts[0].0, bolts[1].0, "no stagger between the hops");
    assert!(
        (bolts[0].1 - bolts[0].0 - 1.0).abs() < 1e-5,
        "one second per hop"
    );
    assert_eq!(*expiry, bolts[1].1, "the beam ends with its last hop");
    assert!(!persistent, "a cast-stage proc (flag 0) self-terminates");

    assert!(app.world().entity(caster).get::<ChainHops>().is_none());
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();
    assert_eq!(
        beams(&mut app).len(),
        1,
        "the second play has nothing to run"
    );
}

/// Row 61: hop `i` lights `delay + i × 40 ms` after the play and burns 203 ms.
#[test]
fn a_staggered_row_lights_its_hops_in_turn() {
    let mut app = beam_app();
    let (caster, t1, t2) = (unit(&mut app), unit(&mut app), unit(&mut app));
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1, t2]));
    let mut p = play(caster, CHAIN_SPELL);
    p.proc.effect_id = STAGGERED;
    app.world_mut().write_message(p);
    app.update();
    let now = app.world().resource::<Time>().elapsed_secs();
    let (_, bolts, expiry, _) = beams(&mut app).remove(0);
    assert!((bolts[0].0 - now - 0.3).abs() < 1e-5, "the first-hop delay");
    assert!((bolts[1].0 - bolts[0].0 - 0.04).abs() < 1e-5, "the stagger");
    assert!(
        (bolts[1].1 - bolts[1].0 - 0.203).abs() < 1e-5,
        "203 ms a hop"
    );
    assert_eq!(expiry, bolts[1].1);
}

#[test]
fn two_plays_in_one_frame_share_no_hops() {
    let mut app = beam_app();
    let (caster, t1) = (unit(&mut app), unit(&mut app));
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1]));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();
    assert_eq!(beams(&mut app).len(), 1, "only the first play consumes");
}

#[test]
fn only_a_persistent_beam_answers_the_spell_clear() {
    for persistent in [false, true] {
        let mut app = beam_app();
        let (caster, t1) = (unit(&mut app), unit(&mut app));
        app.world_mut()
            .entity_mut(caster)
            .insert(ChainHops(vec![t1]));
        let mut p = play(caster, CHAIN_SPELL);
        p.proc.flag = persistent;
        app.world_mut().write_message(p);
        app.update();
        assert_eq!(beams(&mut app).len(), 1);
        assert_eq!(beams(&mut app)[0].3, persistent);

        app.world_mut().write_message(SpellEffectClear {
            entity: caster,
            spell_id: CHAIN_SPELL,
        });
        app.update();
        assert_eq!(
            beams(&mut app).len(),
            usize::from(!persistent),
            "persistent={persistent}: the clear takes the channel beam and spares the cast one"
        );
    }
}

#[test]
fn the_constructors_guards_build_nothing_but_still_consume() {
    for (spell_id, effect_id) in [(0u32, 1u32), (CHAIN_SPELL, 99)] {
        let mut app = beam_app();
        let (caster, t1) = (unit(&mut app), unit(&mut app));
        app.world_mut()
            .entity_mut(caster)
            .insert(ChainHops(vec![t1]));
        let mut p = play(caster, spell_id);
        p.proc.effect_id = effect_id;
        app.world_mut().write_message(p);
        app.update();
        assert!(
            beams(&mut app).is_empty(),
            "spell {spell_id} / chain {effect_id} draws nothing"
        );
        assert!(
            app.world().entity(caster).get::<ChainHops>().is_none(),
            "…and the array is consumed anyway"
        );
    }
}

#[test]
fn the_texture_path_becomes_an_mpq_url() {
    assert_eq!(
        beam_texture_url("Textures\\SpellChainEffects\\Lightning.blp"),
        "mpq://textures/spellchaineffects/lightning.blp"
    );
    assert_eq!(
        beam_texture_url("Textures\\SpellChainEffects\\SummonGhoulsLightning.tga"),
        "mpq://textures/spellchaineffects/summonghoulslightning.blp",
        "the `.blp` the loader falls back to"
    );
}

/// Param 3 sends every hop from the caster; clear, each hop leaves the one before (`0x7fa547`).
#[test]
fn the_reverse_flag_fans_every_hop_out_of_the_caster() {
    for (reverse, want) in [
        (false, vec![(0, 1), (1, 2), (2, 3)]),
        (true, vec![(0, 1), (0, 2), (0, 3)]),
    ] {
        let mut app = beam_app();
        let caster = unit(&mut app);
        let targets = vec![unit(&mut app), unit(&mut app), unit(&mut app)];
        app.world_mut()
            .entity_mut(caster)
            .insert(ChainHops(targets));
        let mut p = play(caster, CHAIN_SPELL);
        p.proc.reverse = reverse;
        app.world_mut().write_message(p);
        app.update();
        assert_eq!(hops(&mut app), vec![want], "reverse={reverse}");
    }
}

/// A combo row draws itself and each row its combo names, over the same hops (`0x7fc8ea`).
#[test]
fn a_combo_row_draws_every_row_it_names() {
    let mut app = beam_app();
    let (caster, t1) = (unit(&mut app), unit(&mut app));
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1]));
    let mut p = play(caster, CHAIN_SPELL);
    p.proc.effect_id = COMBO;
    app.world_mut().write_message(p);
    app.update();
    let mut lives: Vec<f32> = beams(&mut app)
        .into_iter()
        .map(|(nodes, bolts, _, _)| {
            assert_eq!(nodes, vec![caster, t1], "one hop list for every row");
            bolts[0].1 - bolts[0].0
        })
        .collect();
    lives.sort_by(f32::total_cmp);
    assert_eq!(lives.len(), 3, "the root and its two combo rows");
    assert!((lives[0] - 0.5).abs() < 1e-5, "row {COMBO_A}'s own life");
}

/// A saved location aims the next play at the ground, past any hops (`0x726a3b`).
#[test]
fn a_saved_location_draws_one_bolt_to_the_ground_and_plays_the_area_kit() {
    let mut app = beam_app();
    let (caster, t1) = (unit(&mut app), unit(&mut app));
    let dest = Vec3::new(5.0, 1.0, -7.0);
    app.world_mut()
        .resource_mut::<ChainLocations>()
        .save(caster, 0, false, Some(dest));
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1]));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();
    let beam = app
        .world_mut()
        .query::<&ChainBeam>()
        .single(app.world())
        .expect("one beam");
    assert_eq!(
        node_entities(&app, &beam.nodes),
        vec![caster],
        "no unit target"
    );
    assert_eq!(beam.location, Some(dest));
    assert_eq!(beam.bolts.len(), 1);
    let kits: Vec<u32> = app
        .world_mut()
        .resource_mut::<Messages<DelayedAreaKit>>()
        .drain()
        .map(|k| k.kit_id)
        .collect();
    assert_eq!(kits, vec![AREA_KIT]);
}

/// A flag-0x1 area kit waits on the caster under the beam's sequence, the first bolt's attachment;
/// the caster's next beam takes the sequence 2 up (`0x726b8e`, `0x726b96`, `0x7fa59b`).
#[test]
fn a_location_beams_flag_1_area_kit_waits_under_the_beams_sequence() {
    let mut app = beam_app_area_flags(benilla_formats::kit_flag::BEAM_WAIT);
    app.init_resource::<BeamKitWaits>();
    let caster = unit(&mut app);
    let t1 = unit(&mut app);
    let dest = Vec3::new(5.0, 1.0, -7.0);
    let attachments = |app: &mut App| -> Vec<Vec<i32>> {
        app.world_mut()
            .query::<&ChainBeam>()
            .iter(app.world())
            .map(|b| b.bolts.iter().map(|x| x.attachment).collect())
            .collect()
    };
    // A unit beam first: its first bolt carries sequence 0, the rest none.
    app.world_mut()
        .entity_mut(caster)
        .insert(ChainHops(vec![t1, caster]));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();
    assert_eq!(attachments(&mut app), vec![vec![0, NO_BEAM_KEY]]);
    assert!(
        app.world().resource::<BeamKitWaits>().is_empty(),
        "no location, no area kit"
    );
    for (e, _) in beams_with_ids(&mut app) {
        app.world_mut().despawn(e);
    }

    // Then a ground beam: sequence 2, its area kit held, not played.
    app.world_mut()
        .resource_mut::<ChainLocations>()
        .save(caster, 0, false, Some(dest));
    app.world_mut().write_message(play(caster, CHAIN_SPELL));
    app.update();
    assert_eq!(attachments(&mut app), vec![vec![2]]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<Messages<DelayedAreaKit>>()
            .drain()
            .count(),
        0,
        "the area kit waits for the ground bolt"
    );
    assert!(!app.world().resource::<BeamKitWaits>().is_empty());
}

/// Every live beam with its entity.
fn beams_with_ids(app: &mut App) -> Vec<(Entity, u32)> {
    app.world_mut()
        .query::<(Entity, &ChainBeam)>()
        .iter(app.world())
        .map(|(e, b)| (e, b.spell_id))
        .collect()
}

/// The GO's save: the dest unless flag 0x1 hit units, and a clear otherwise (`0x80ed7d`).
#[test]
fn a_beam_rows_go_saves_or_clears_the_location() {
    let caster = Entity::from_raw_u32(9).expect("a test entity id");
    let mut locations = ChainLocations::default();
    let at = |l: &ChainLocations| l.0.get(&caster).copied();
    locations.save(caster, 0, true, Some(Vec3::X));
    assert_eq!(at(&locations), Some(Vec3::X), "flag clear: units or not");
    locations.save(caster, 0x1, true, Some(Vec3::Y));
    assert_eq!(at(&locations), None, "flag 0x1 with units hit clears it");
    locations.save(caster, 0x1, false, Some(Vec3::Z));
    assert_eq!(at(&locations), Some(Vec3::Z), "flag 0x1 with none saves");
    locations.save(caster, 0, false, None);
    assert_eq!(at(&locations), None, "no dest clears it");
}

/// Under `0x1000` another unit's guid becomes the source and the owner the one hop, past its hop
/// array and its saved location, whose area kit still plays (`0x7269d8`, `0x726b12`).
#[test]
fn a_play_from_another_unit_runs_from_it_to_the_owner() {
    let mut app = beam_app();
    let (owner, hop, other) = (unit(&mut app), unit(&mut app), typed_unit(&mut app));
    app.world_mut()
        .entity_mut(owner)
        .insert(ChainHops(vec![hop]));
    app.world_mut()
        .resource_mut::<ChainLocations>()
        .save(owner, 0, false, Some(Vec3::X));
    let mut p = play(owner, CHAIN_SPELL);
    p.from = Some(other);
    app.world_mut().write_message(p);
    app.update();
    let beam = app
        .world_mut()
        .query::<&ChainBeam>()
        .single(app.world())
        .expect("one beam");
    assert_eq!(
        node_entities(&app, &beam.nodes),
        vec![other, owner],
        "the other unit to the owner"
    );
    assert_eq!(beam.location, None, "a unit target, never the ground");
    assert_eq!(beam.caster, owner, "the owner's kit keeps it");
    assert_eq!(hops(&mut app), vec![vec![(0, 1)]]);
    assert!(
        app.world().entity(owner).get::<ChainHops>().is_none(),
        "the hop array is consumed"
    );
    let kits: Vec<u32> = app
        .world_mut()
        .resource_mut::<Messages<DelayedAreaKit>>()
        .drain()
        .map(|k| k.kit_id)
        .collect();
    assert_eq!(
        kits,
        vec![AREA_KIT],
        "the owner's location still plays its area kit"
    );
}

/// The owner's own guid fails the test and takes the ordinary path (`0x7269fd`).
#[test]
fn a_play_naming_its_owner_takes_the_ordinary_path() {
    let mut app = beam_app();
    let (owner, hop) = (typed_unit(&mut app), unit(&mut app));
    app.world_mut()
        .entity_mut(owner)
        .insert(ChainHops(vec![hop]));
    let mut p = play(owner, CHAIN_SPELL);
    p.from = Some(owner);
    app.world_mut().write_message(p);
    app.update();
    assert_eq!(beams(&mut app)[0].0, vec![owner, hop]);
}

/// A guid naming no unit ends the arm before the hop array is touched (`0x726a1d`).
#[test]
fn an_unresolved_guid_draws_nothing_and_keeps_the_hops() {
    let mut app = beam_app();
    let gone = typed_unit(&mut app);
    app.world_mut().despawn(gone);
    let object = unit(&mut app); // a store with no unit type
    for from in [gone, object, Entity::PLACEHOLDER] {
        let (owner, hop) = (unit(&mut app), unit(&mut app));
        app.world_mut()
            .entity_mut(owner)
            .insert(ChainHops(vec![hop]));
        let mut p = play(owner, CHAIN_SPELL);
        p.from = Some(from);
        app.world_mut().write_message(p);
        app.update();
        assert!(beams(&mut app).is_empty(), "{from} draws nothing");
        assert!(
            app.world().entity(owner).get::<ChainHops>().is_some(),
            "{from} leaves the hops for the next play"
        );
    }
}

/// Each play links its own effect on the owner, so another unit's aura beam and the owner's own
/// channel beam of one spell coexist (`0x7269ca`), and one spell clear takes both (`0x743b40`).
#[test]
fn two_persistent_beams_of_one_spell_coexist_until_the_spell_clear() {
    let mut app = beam_app();
    let (owner, other) = (unit(&mut app), typed_unit(&mut app));
    let mut drawn_on_owner = play(owner, CHAIN_SPELL);
    drawn_on_owner.from = Some(other);
    drawn_on_owner.proc.flag = true;
    app.world_mut().write_message(drawn_on_owner);
    app.update();
    app.world_mut()
        .entity_mut(owner)
        .insert(ChainHops(vec![other]));
    let mut own = play(owner, CHAIN_SPELL);
    own.proc.flag = true;
    app.world_mut().write_message(own);
    app.update();
    let mut nodes: Vec<Vec<Entity>> = beams(&mut app).into_iter().map(|b| b.0).collect();
    nodes.sort();
    let mut want = vec![vec![other, owner], vec![owner, other]];
    want.sort();
    assert_eq!(nodes, want, "both beams live");
    // Another owner's clear of the spell leaves them.
    app.world_mut().write_message(SpellEffectClear {
        entity: other,
        spell_id: CHAIN_SPELL,
    });
    app.update();
    assert_eq!(beams(&mut app).len(), 2);
    app.world_mut().write_message(SpellEffectClear {
        entity: owner,
        spell_id: CHAIN_SPELL,
    });
    app.update();
    assert!(beams(&mut app).is_empty(), "the owner's clear takes both");
}

/// A beam keeps its ends as guids (`0x7faeef`): a source that streams out and back under a new
/// entity is the same end again, where an entity key would stay hidden forever.
#[test]
fn a_beam_end_follows_its_guid_across_a_restream() {
    let mut app = beam_app();
    let (owner, other) = (unit(&mut app), typed_unit(&mut app));
    let mut p = play(owner, CHAIN_SPELL);
    p.from = Some(other);
    p.proc.flag = true;
    app.world_mut().write_message(p);
    app.update();
    let guid = app.world().entity(other).get::<Guid>().expect("a guid").0;
    app.world_mut().despawn(other);
    app.world_mut().resource_mut::<GuidIndex>().0.remove(&guid);
    assert_eq!(
        beams(&mut app)[0].0,
        vec![Entity::PLACEHOLDER, owner],
        "gone: the bolt hides, the beam lives"
    );
    let back = app.world_mut().spawn(Guid(guid)).id();
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(guid, back);
    assert_eq!(
        beams(&mut app)[0].0,
        vec![back, owner],
        "back: the same end"
    );
}

/// A kit clear takes the persistent beams of that kit on the owner, whatever the spell: the
/// precast clear (`0x744bd0`, `0x7fec2f`).
#[test]
fn a_kit_clear_takes_only_its_kit_s_persistent_beams() {
    let mut app = beam_app();
    let (owner, other) = (unit(&mut app), typed_unit(&mut app));
    let mut p = play(owner, CHAIN_SPELL);
    p.from = Some(other);
    p.proc.flag = true;
    app.world_mut().write_message(p);
    app.update();
    assert_eq!(beams(&mut app).len(), 1);
    for miss in [
        KitEffectClear {
            entity: owner,
            kit_id: BEAM_KIT + 1,
        },
        KitEffectClear {
            entity: other,
            kit_id: BEAM_KIT,
        },
    ] {
        app.world_mut().write_message(miss);
        app.update();
        assert_eq!(beams(&mut app).len(), 1, "{miss:?} leaves it");
    }
    app.world_mut().write_message(KitEffectClear {
        entity: owner,
        kit_id: BEAM_KIT,
    });
    app.update();
    assert!(beams(&mut app).is_empty(), "its kit's clear takes it");
}

/// The tick's hand-off (`0x7fb15a`): first vertex to the source, last to the target object under
/// no key, and with no target object to the source under the bolt's attachment.
#[test]
fn a_bolts_drawn_ends_connect_its_source_and_target() {
    let id = |n| Entity::from_raw_u32(n).expect("a test entity id");
    let (src, dst) = (id(1), id(2));
    const SEQ: i32 = 4;
    assert_eq!(
        reached_ends((true, false), src, Some(dst), SEQ),
        [Some((src, NO_BEAM_KEY)), None],
        "a pulse leaving the caster"
    );
    assert_eq!(
        reached_ends((false, true), src, Some(dst), SEQ),
        [None, Some((dst, NO_BEAM_KEY))],
        "a pulse arriving at the target, unit or game object"
    );
    assert_eq!(
        reached_ends((true, true), src, Some(dst), NO_BEAM_KEY),
        [Some((src, NO_BEAM_KEY)), Some((dst, NO_BEAM_KEY))],
        "no pulse: both ends every drawn frame"
    );
    assert_eq!(
        reached_ends((false, true), src, None, SEQ),
        [None, Some((src, SEQ))],
        "a ground bolt's end connects its caster under the beam's sequence"
    );
    assert_eq!(
        reached_ends((false, false), src, Some(dst), SEQ),
        [None, None]
    );
}

/// A ground beam whose chain id names no row draws nothing, yet its area kit still plays: the
/// beam builder returns nothing to wait on (`0x726afd`).
#[test]
fn a_ground_beam_with_no_chain_row_still_plays_its_area_kit() {
    let mut app = beam_app();
    let caster = unit(&mut app);
    app.world_mut()
        .resource_mut::<ChainLocations>()
        .save(caster, 0, false, Some(Vec3::ONE));
    let mut p = play(caster, CHAIN_SPELL);
    p.proc.effect_id = 999;
    app.world_mut().write_message(p);
    app.update();
    assert!(beams(&mut app).is_empty(), "no row, no beam");
    let kits: Vec<u32> = app
        .world_mut()
        .resource_mut::<Messages<DelayedAreaKit>>()
        .drain()
        .map(|k| k.kit_id)
        .collect();
    assert_eq!(kits, vec![AREA_KIT]);
}

/// Only a unit caster saves its GO's location; a GameObject's GO saves nothing (`0x80ed20`).
#[test]
fn only_a_unit_casters_go_saves_the_location() {
    let mut app = beam_app();
    let unit = typed_unit(&mut app);
    let trap = streamed(
        &mut app,
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21)])),
    );
    for caster in [unit, trap] {
        app.world_mut().write_message(SpellGoTargets {
            caster,
            spell_id: CHAIN_SPELL,
            hits: Vec::new(),
            misses: Vec::new(),
            list: Vec::new(),
            target_mask: 0x40,
            src: None,
            src_deck: None,
            dest: Some(Vec3::X),
            dest_deck: None,
            points: Vec::new(),
            ammo: None,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
    }
    app.update();
    let saved = &app.world().resource::<ChainLocations>().0;
    assert_eq!(saved.get(&unit), Some(&Vec3::X), "the unit's");
    assert_eq!(saved.get(&trap), None, "not the GameObject's");
}
