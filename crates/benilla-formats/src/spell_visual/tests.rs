//! Tests for the spell visual tables, on synthetic WDBCs and on the installed data.

use super::*;

/// A WDBC of u32 records with an empty string block.
fn build_wdbc(record_count: u32, field_count: u32, records: &[u8]) -> Vec<u8> {
    let record_size = field_count * 4;
    assert_eq!(records.len(), (record_count * record_size) as usize);
    let mut b = Vec::new();
    b.extend_from_slice(b"WDBC");
    b.extend_from_slice(&record_count.to_le_bytes());
    b.extend_from_slice(&field_count.to_le_bytes());
    b.extend_from_slice(&record_size.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // empty string block
    b.extend_from_slice(records);
    b
}

/// A row of `fields` u32 columns, zero but for the `(column, value)` pairs.
fn row(fields: usize, set: &[(usize, u32)]) -> Vec<u8> {
    let mut cols = vec![0u32; fields];
    for &(c, v) in set {
        cols[c] = v;
    }
    cols.into_iter().flat_map(u32::to_le_bytes).collect()
}

/// Fireball's `SpellVisual` row 67 as 3.3.5a ships its first fourteen columns.
fn fireball_visual_row() -> Vec<u8> {
    row(
        SPELL_VISUAL_FIELDS,
        &[
            (0, 67),
            (1, 30),
            (2, 38),
            (3, 286),
            (7, 1),
            (8, 365),
            (10, 1),
            (11, 3011),
            (13, 1),
            (16, u32::MAX),
        ],
    )
}

#[test]
fn spell_visual_columns_read_the_3_3_5a_layout() {
    let mut records = fireball_visual_row();
    // Blizzard's row 9490: channel kit 717 in column 6, impact and persistent area kits.
    records.extend(row(
        SPELL_VISUAL_FIELDS,
        &[
            (0, 9490),
            (1, 197),
            (2, 5390),
            (6, 717),
            (10, 1),
            (24, 13152),
            (25, 9350),
        ],
    ));
    records.extend(row(SPELL_VISUAL_FIELDS, &[(0, 1)])); // an all-silent row
    let visuals = parse_visuals(&build_wdbc(3, SPELL_VISUAL_FIELDS as u32, &records)).unwrap();

    assert_eq!(
        visuals[&67],
        VisualStages {
            precast: 30,
            cast: 38,
            impact: 286,
            missile_gate: 1,
            missile_model: 365,
            missile_attach: 1,
            missile_sound: Some(3011),
            flags: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        visuals[&9490],
        VisualStages {
            precast: 197,
            cast: 5390,
            channel: 717,
            missile_attach: 1,
            impact_area_kit: 13152,
            persistent_area_kit: 9350,
            ..Default::default()
        }
    );
    assert_eq!(
        visuals[&1],
        VisualStages::default(),
        "an all-zero row is all-silent"
    );
}

/// Fireball's cast kit 38 as 3.3.5a ships it: anim 53, both hands, sound 1484.
#[test]
fn kit_columns_read_the_3_3_5a_layout() {
    let kit_row = |id, set: &[(usize, u32)]| {
        let mut cols = vec![(0, id)];
        cols.extend_from_slice(set);
        // Every CharProc slot empty, as the data writes it.
        cols.extend((17..21).map(|c| (c, u32::MAX)));
        row(SPELL_VISUAL_KIT_FIELDS, &cols)
    };
    let mut records = kit_row(
        38,
        &[(1, u32::MAX), (2, 53), (6, 288), (7, 288), (15, 1484)],
    );
    // Frost Nova's state kit 285: head sparkle, the world-planted ice, a tint proc.
    records.extend(kit_row(
        285,
        &[(2, u32::MAX), (3, 54), (14, 284), (15, 1502)],
    ));
    records.extend(kit_row(1, &[])); // plain-zero "none"
    records.extend(kit_row(2, &[(2, u32::MAX), (15, u32::MAX)])); // the -1 "none" form
                                                                  // The weapon columns 9 and 10 are no slot the walk plays.
    records.extend(kit_row(3, &[(9, 77), (10, 78)]));
    let mut bytes = build_wdbc(5, SPELL_VISUAL_KIT_FIELDS as u32, &records);
    // Kit 285's first CharProc: type 1 (tint), `CharParamZero` 6711039.0.
    let base = 20 + SPELL_VISUAL_KIT_FIELDS * 4;
    bytes[base + 17 * 4..base + 18 * 4].copy_from_slice(&1u32.to_le_bytes());
    bytes[base + 21 * 4..base + 22 * 4].copy_from_slice(&6_711_039.0f32.to_le_bytes());
    let kits = parse_kits(&bytes).unwrap();

    assert_eq!(kits[&38].anim_id, Some(53));
    assert_eq!(kits[&38].sound, Some(1484));
    assert_eq!(
        kits[&38].effects().collect::<Vec<_>>(),
        vec![(0x15, 288), (0x16, 288)],
        "LeftHand then RightHand"
    );
    assert_eq!(kits[&38].char_procs().count(), 0);

    assert_eq!(
        kits[&285].effects().collect::<Vec<_>>(),
        vec![(0x14, 54), (WORLD_EFFECT_TAG, 284)],
        "the nine first, then the world-plant slot"
    );
    assert_eq!(kits[&285].sound, Some(1502));
    assert_eq!(
        kits[&285].char_proc_param(char_proc_type::TINT),
        Some(6_711_039.0)
    );

    assert_eq!(kits[&1], VisualKit::default(), "plain 0 = none");
    assert_eq!(kits[&2], VisualKit::default(), "0xFFFFFFFF = none too");
    assert_eq!(kits[&3].effects().count(), 0, "no weapon-column slot");
}

#[test]
fn the_dest_attachment_reads_the_table_or_the_column_itself() {
    let indexed = VisualStages {
        missile_attach: 1,
        ..Default::default()
    };
    assert_eq!(indexed.missile_dest_attachment(), Some(0x22), "chest");
    let past = VisualStages {
        missile_attach: 16,
        ..Default::default()
    };
    assert_eq!(past.missile_dest_attachment(), None);
    let raw = VisualStages {
        missile_attach: 0x22,
        flags: FLAG_DEST_ATTACH_IS_ID,
        ..Default::default()
    };
    assert_eq!(raw.missile_dest_attachment(), Some(0x22), "the id itself");
}

#[test]
fn kit_effect_slots_pair_with_their_attach_tags() {
    let kit = VisualKit {
        // Fireball's precast: left hand (slot 3) and right hand (slot 4).
        effect_slots: [
            None,
            None,
            None,
            Some(287),
            Some(287),
            None,
            None,
            None,
            None,
        ],
        ..Default::default()
    };
    assert_eq!(
        kit.effects().collect::<Vec<_>>(),
        vec![(0x15, 287), (0x16, 287)],
        "LeftHand then RightHand, slot order"
    );
    assert_eq!(VisualKit::default().effects().count(), 0);
}

#[test]
fn real_spell_visual_chain_resolves_fireball() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load SpellVisual/SpellVisualKit");
    assert_eq!(cat.len(), 9406, "all 12340 SpellVisual rows load");
    assert_eq!(cat.kit_len(), 8663, "all 12340 SpellVisualKit rows load");

    // Fireball, spell 133, is visual 67.
    let stages = cat.stages(67).expect("Fireball's SpellVisual row");
    assert_eq!(
        *stages,
        VisualStages {
            precast: 30,
            cast: 38,
            impact: 286,
            state: 0,
            state_done: 0,
            channel: 0,
            missile_model: 365,
            missile_attach: 1,
            // SoundEntries 3011 is FireMissileLoop.wav.
            missile_sound: Some(3011),
            strike_sound: None,
            // Set: the missile owns the landing.
            missile_gate: 1,
            flags: 1,
            caster_impact: 0,
            target_impact: 0,
            instant_area_kit: 0,
            impact_area_kit: 0,
            persistent_area_kit: 0,
        }
    );
    assert_eq!(
        cat.stages(93).and_then(|s| s.strike_sound),
        Some(1143),
        "Mining's visual carries the column-12 strike clang"
    );
    assert_eq!(
        cat.stages(91).and_then(|s| s.strike_sound),
        Some(1142),
        "Herb Gathering's visual carries the column-12 rustle"
    );

    // Visual 98 is the thrown weapon's substitute; 3318 is WeaponLoop.wav.
    let thrown = cat
        .stages(98)
        .expect("the thrown-weapon substitute SpellVisual");
    assert_eq!(
        thrown.missile_model, 0,
        "a thrown weapon flies its own model, not an effect"
    );
    assert_eq!(
        thrown.missile_sound,
        Some(3318),
        "the thrown weapon's flight loop"
    );
    assert_eq!(
        cat.effect_path(stages.missile_model),
        Some("Spells\\Fireball_Missile_Low.mdx"),
        "the flying fireball's model"
    );
    assert_eq!(
        stages.missile_dest_attachment(),
        Some(0x22),
        "Fireball homes onto the target's chest"
    );

    // The ding resolves by the client's baked name.
    assert_eq!(
        cat.hardcoded_effect("HARDCODED Unit Level Up"),
        Some((21, "Spells\\LevelUp\\LevelUp.mdl")),
        "the level-up pillar resolves by name"
    );
    assert!(
        cat.hardcoded_effect("HARDCODED Loot Art").is_some(),
        "the sibling hardcoded rows ride the same map"
    );

    let cast_kit = cat.kit(stages.cast).expect("cast kit");
    assert_eq!(
        cast_kit.anim_id,
        Some(53),
        "AnimationData id 53 = SpellCastDirected"
    );
    assert_eq!(cast_kit.sound, Some(1484));

    let impact_kit = cat.kit(stages.impact).expect("impact kit");
    assert_eq!(
        impact_kit.anim_id,
        Some(9),
        "AnimationData id 9 = CombatWound, the target's hit reaction"
    );
    assert_eq!(impact_kit.sound, Some(1507));

    let precast_kit = cat.kit(stages.precast).expect("precast kit");
    assert_eq!(
        precast_kit.effects().collect::<Vec<_>>(),
        vec![(0x15, 287), (0x16, 287)],
        "Fire_Precast_Hand on LeftHand + RightHand"
    );
    assert_eq!(
        cat.effect_path(287),
        Some("Spells\\Fire_Precast_Hand.mdx"),
        "SpellVisualEffectName column 2 = the effect model path"
    );

    let frost_nova_state = cat
        .kit(cat.stages(17).expect("Frost Nova's visual").state)
        .expect("Frost Nova's state kit");
    assert_eq!(frost_nova_state.world_effect, Some(284));
    assert_eq!(
        cat.effect_path(284),
        Some("Spells\\Frost_Nova_state.mdx"),
        "the frozen-feet model rides column 14"
    );
    assert_eq!(
        cat.kit(stages.impact)
            .unwrap()
            .effects()
            .collect::<Vec<_>>(),
        vec![(0x22, 321)],
        "MoltenBlast_Impact_Chest on the Chest attach"
    );
}

/// The lootable-corpse sparkle's row names a model that ships.
#[test]
fn real_effect_name_table_resolves_the_loot_art_row() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load the visual catalog");
    assert_eq!(cat.loot_art_effect(), Some((14, "Particles\\LootFX.mdl")));
    // Consumers load an `.mdl` path as `.m2`.
    assert!(
        chain.read_file("Particles\\LootFX.m2").is_ok(),
        "LootFX.m2 must ship in the chain"
    );
}

/// The ground spells' area kits: the dynobject's model and shard emitter come from the persistent
/// kit (`0x704b30`, `0x704f60`), Blizzard's spawn from the impact kit.
#[test]
fn ground_aoe_chain_reads_the_area_kits() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let catalog = crate::load_spell_catalog(&mut chain).expect("spells");
    let visuals = load_spell_visual_catalog(&mut chain).expect("visuals");

    // Blizzard (10, visual 9490): no own model, a type-9 shard emitter, index 0, 5 per second.
    let v = visuals.stages(catalog.get(10).unwrap().visual).unwrap();
    assert_eq!(
        (
            v.missile_gate,
            v.instant_area_kit,
            v.impact_area_kit,
            v.persistent_area_kit
        ),
        (0, 0, 13152, 9350)
    );
    assert_eq!(visuals.persistent_area_model(v), None);
    let k = visuals.kit(9350).unwrap();
    assert_eq!(k.sound, Some(7));
    let proc = k.char_procs().find(|p| p.ty == 9).unwrap();
    assert_eq!((proc.params[0], proc.params[1]), (0.0, 5.0));
    assert_eq!(
        visuals
            .kit(v.impact_area_kit)
            .and_then(|k| k.world_effect)
            .and_then(|e| visuals.effect_path(e)),
        Some("Spells\\Blizzard_Spawn.mdx")
    );

    // Flamestrike (2120): its own model from the kit's base slot, and its sound.
    let v = visuals.stages(catalog.get(2120).unwrap().visual).unwrap();
    assert_eq!(v.persistent_area_kit, 9357);
    assert_eq!(
        visuals.persistent_area_model(v),
        Some((420, "Spells\\Flamestrike_Impact_Base.mdx"))
    );
    let k = visuals.kit(9357).unwrap();
    assert_eq!(k.sound, Some(3077));
    assert!(k.char_procs().all(|p| p.ty != 9));

    // Rain of Fire (5740): shard index 1, 5 per second.
    let v = visuals.stages(catalog.get(5740).unwrap().visual).unwrap();
    let proc = visuals
        .kit(v.persistent_area_kit)
        .unwrap()
        .char_procs()
        .find(|p| p.ty == 9)
        .unwrap();
    assert_eq!((proc.params[0], proc.params[1]), (1.0, 5.0));
}

// ── The chain/beam table ──────────────────────────────────────────────────────

/// Pinned against hand-computed IEEE bits, not against itself.
#[test]
fn char_proc_small_int_recovers_the_integer() {
    // 1.0 + 512.0 = 513.0 = 0x44004000; >>14 = 0x11001; &0xff = 1.
    assert_eq!(char_proc_small_int(1.0), 1);
    assert_eq!(char_proc_small_int(0.0), 0);
    for n in 0..=255u32 {
        assert_eq!(char_proc_small_int(n as f32), n, "round-trips every byte");
    }
    // It truncates, reading the integer part out of the mantissa.
    assert_eq!(char_proc_small_int(3.9), 3);
}

/// The chain case truncates the id and reads only params 0 and 2 for what benilla draws.
#[test]
fn a_chain_proc_takes_its_id_by_truncation() {
    let proc = CharProc {
        ty: char_proc_type::CHAIN_CHANNEL,
        params: [719.0, 0.0, 1.0, 1.0],
    };
    assert_eq!(
        proc.as_chain(),
        Some(ChainProc {
            effect_id: 719,
            flag: true,
            ty: char_proc_type::CHAIN_CHANNEL,
        }),
        "an id past 255, which the small-int decode would mangle"
    );
    let cast = CharProc {
        ty: char_proc_type::CHAIN_CAST,
        params: [743.9, 1.0, 0.0, 0.0],
    };
    assert_eq!(
        cast.as_chain().map(|c| (c.effect_id, c.flag)),
        Some((743, false))
    );
    let padding = CharProc {
        ty: char_proc_type::CHAIN_CHANNEL,
        params: [0.0; 4],
    };
    assert_eq!(padding.as_chain(), None);
}

/// Rows 1 (Lightning), 61 (a staggered row) and 8 (a drain) as the file ships them, through the
/// 177-byte record with its byte columns.
#[test]
fn real_chain_effects_table() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    assert_eq!(
        cat.chain_effect_len(),
        935,
        "935 SpellChainEffects rows in 12340"
    );

    let lightning = cat.chain_effect(1).expect("chain effect 1");
    assert_eq!(
        lightning.texture,
        "Textures\\SpellChainEffects\\Lightning.blp"
    );
    assert_eq!(lightning.avg_seg_len, 2.78);
    assert_eq!(lightning.half_width, 0.5);
    assert_eq!(lightning.noise_scale, 0.04);
    assert_eq!(lightning.scroll_period_s, 1.0);
    assert_eq!(lightning.bolt_life_ms, 1000);
    assert_eq!(lightning.first_bolt_delay_ms, 300);
    assert_eq!(lightning.bolt_stagger_ms, 0, "its hops light together");

    let staggered = cat.chain_effect(61).expect("chain effect 61");
    assert_eq!(
        (staggered.first_bolt_delay_ms, staggered.bolt_stagger_ms),
        (300, 40),
        "column 31, a float, past the byte columns' shift"
    );

    let drain = cat.chain_effect(8).expect("chain effect 8");
    assert_eq!(drain.texture, "Textures\\SpellChainEffects\\SoulBeam.blp");
    assert_eq!(
        drain.scroll_period_s, -2.0,
        "a NEGATIVE scroll period flows the texture back toward the caster"
    );

    // Ids are a lookup, never an index.
    assert!(cat.chain_effect(0).is_none(), "id 0 is the client's no-op");
    assert_eq!(cat.chain_effect_ids().last(), Some(1178));
}

/// Both chain keys reach their beams; folding type 0 to empty would lose every channel beam.
#[test]
fn real_chain_procs_resolve_to_their_beams() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");

    // Chain Lightning (spell 421, visual 36): cast kit 321, a type-0 slot, flag clear.
    let cl = cat
        .kit(cat.stages(36).expect("visual 36").cast)
        .expect("kit 321")
        .chain_proc()
        .expect("a beam");
    assert_eq!((cl.ty, cl.effect_id, cl.flag), (0, 743, false));
    assert_eq!(
        cat.chain_effect(cl.effect_id).map(|c| c.texture.as_str()),
        Some("Textures\\SpellChainEffects\\ShockLightning.blp")
    );

    // Drain Life (spell 689, visual 12655): channel kit 11762, flag set.
    let dl = cat
        .kit(cat.stages(12655).expect("visual 12655").channel)
        .expect("kit 11762")
        .chain_proc()
        .expect("a beam");
    assert_eq!((dl.effect_id, dl.flag), (719, true));

    // Chain Heal (spell 1064, visual 3659): cast kit 3169.
    let ch = cat
        .kit(3169)
        .expect("kit 3169")
        .chain_proc()
        .expect("a beam");
    assert_eq!(
        cat.chain_effect(ch.effect_id).map(|c| c.texture.as_str()),
        Some("Textures\\SpellChainEffects\\HealBeam.blp")
    );

    // The whole-table census, as `chaincensus` prints it.
    let (mut slots, mut live, mut padding, mut dangling) = (0, 0, 0, Vec::new());
    for id in cat.kit_ids() {
        let kit = cat.kit(id).expect("kit");
        for proc in kit.char_procs().filter(|p| char_proc_type::is_chain(p.ty)) {
            slots += 1;
            match proc.as_chain() {
                Some(c) => {
                    live += 1;
                    if cat.chain_effect(c.effect_id).is_none() {
                        dangling.push((id, c.effect_id));
                    }
                }
                None => padding += 1,
            }
        }
    }
    assert_eq!(
        (slots, live, padding),
        (806, 782, 24),
        "806 chain CharProc slots ship: 782 live beams, 24 zero-param padding"
    );
    assert_eq!(
        dangling,
        [(13491, 938)],
        "every live slot names a real row but kit 13491's, which the data leaves dangling"
    );
}

/// A texture path that resolves to nothing draws an invisible beam.
#[test]
fn real_chain_effect_textures_resolve_on_the_patch_chain() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    // Only the rows a shipped kit reaches.
    let mut reached: Vec<u32> = cat
        .kit_ids()
        .filter_map(|id| Some(cat.kit(id)?.chain_proc()?.effect_id))
        .filter(|id| cat.chain_effect(*id).is_some())
        .collect();
    reached.sort_unstable();
    reached.dedup();
    assert!(!reached.is_empty(), "some kit must reach a beam");
    let mut missing = Vec::new();
    for id in reached {
        let path = &cat.chain_effect(id).expect("its row").texture;
        // The texture loader falls back to the `.blp`, the only kind the chain ships; row 257
        // stores a `.tga`.
        let stem = path
            .rsplit_once('.')
            .map_or(path.as_str(), |(stem, _)| stem);
        if chain.read_file(&format!("{stem}.blp")).is_err() {
            missing.push((id, path.clone()));
        }
    }
    assert_eq!(
        missing,
        [
            (321, "Beam_VineGreen.tga".to_string()),
            (566, "SPELLS\\TEXTURES\\Beam_Pestilence.blp".to_string()),
            (963, "SPELLS\\TEXTURES\\Beam_Pestilence.blp".to_string()),
        ],
        "every reached texture ships but row 321's bare file name and the Pestilence beam, \
         which no archive holds"
    );
}

/// A type-0 slot whose `CharParamZero` is 0 is no beam; kit 2090 ships four.
#[test]
fn real_zero_param_chain_slots_are_padding() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    let kit = cat.kit(2090).expect("kit 2090");
    assert_eq!(
        kit.char_procs()
            .filter(|p| p.ty == char_proc_type::CHAIN_CHANNEL)
            .count(),
        4,
        "four type-0 slots survive the parser"
    );
    assert!(kit.chain_proc().is_none(), "…but none of them is a beam");
}

/// The client's fill (`0x7210c3`-`0x721181`), column by column.
#[test]
fn the_weapon_merge_fills_only_the_empty_slots() {
    let weapon = VisualStages {
        precast: 7,
        cast: 164,
        impact: 1947,
        state: 999, // never merged
        state_done: 997,
        channel: 998, // never merged
        missile_gate: 1,
        missile_model: 42,
        missile_attach: 3,
        missile_sound: Some(4222),
        strike_sound: Some(1143),
        flags: 0x200,
        caster_impact: 11,
        target_impact: 12,
        instant_area_kit: 5,
        impact_area_kit: 7,
        persistent_area_kit: 9,
    };
    // Multi-Shot's real shape: its own impact + missile, both body kits empty.
    let own = VisualStages {
        impact: 658,
        caster_impact: 21,
        missile_gate: 1,
        missile_model: 528,
        missile_attach: 1,
        flags: 1,
        ..Default::default()
    };
    let merged = own.merged_over_weapon(&weapon);
    assert_eq!((merged.precast, merged.cast), (7, 164), "empty slots fill");
    assert_eq!(merged.impact, 658, "a populated slot is kept");
    assert_eq!(
        (merged.caster_impact, merged.target_impact),
        (21, 12),
        "the impact-kit columns fill the same way (`0x7210e7`, `0x7210f3`)"
    );
    assert_eq!(
        (merged.missile_model, merged.missile_attach),
        (528, 1),
        "its own missile block survives — the gate was already nonzero"
    );
    assert_eq!(
        merged.missile_sound,
        Some(4222),
        "the flight loop fills from the bow (Serpent Sting / Multi-Shot carry none)"
    );
    assert_eq!(
        (merged.state, merged.channel),
        (0, 0),
        "state/channel are NOT in the client's fill list"
    );
    assert_eq!(merged.flags, 1, "nor the flags");
    assert_eq!(
        (
            merged.instant_area_kit,
            merged.impact_area_kit,
            merged.persistent_area_kit
        ),
        (0, 0, 0),
        "nor the area kits"
    );

    // A row with no missile takes the weapon's, the gate as a literal 1 (`0x72110b`).
    let no_missile = VisualStages::default().merged_over_weapon(&VisualStages {
        missile_gate: 5,
        missile_model: 42,
        ..Default::default()
    });
    assert_eq!((no_missile.missile_gate, no_missile.missile_model), (1, 42));
    // A weapon with no missile gives none.
    let neither = VisualStages::default().merged_over_weapon(&VisualStages::default());
    assert_eq!((neither.missile_gate, neither.missile_model), (0, 0));
}

/// Every live hunter shot leaves both body kits empty, so its draw and release clips come from
/// the bow's visual.
#[test]
fn real_hunter_shots_take_the_bows_load_and_release_clips() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_visual_catalog(&mut chain).expect("load spell visuals");
    // Visual 5, the bow's substitute visual.
    let bow = *cat.stages(5).expect("the bow's substitute visual 5");
    assert_eq!(
        (bow.precast, bow.cast),
        (7, 164),
        "visual 5 = LoadBow kit 7 → AttackBow kit 164"
    );
    for (spell_visual, own_impact) in [
        (3179, 276),  // Serpent Sting
        (567, 658),   // Multi-Shot
        (3299, 419),  // Arcane Shot
        (7955, 6808), // Aimed Shot
        (3180, 7610), // Concussive Shot
        (3222, 2894), // Black Arrow
    ] {
        let own = *cat.stages(spell_visual).expect("a live hunter-shot row");
        assert_eq!(
            (own.precast, own.cast),
            (0, 0),
            "visual {spell_visual} authors no body kit of its own"
        );
        let merged = own.merged_over_weapon(&bow);
        assert_eq!(
            (merged.precast, merged.cast),
            (7, 164),
            "visual {spell_visual} takes the bow's Load/Attack pair"
        );
        assert_eq!(
            merged.impact, own_impact,
            "visual {spell_visual} keeps its own impact kit"
        );
        assert_eq!(
            merged.missile_model, own.missile_model,
            "visual {spell_visual} keeps its own projectile"
        );
    }
    assert_eq!(cat.kit(7).and_then(|k| k.anim_id), Some(105), "LoadBow");
    assert_eq!(cat.kit(164).and_then(|k| k.anim_id), Some(46), "AttackBow");
}

/// The type-8 arm truncates (`_ftol`); the small-int decode would give no trail.
#[test]
fn a_weapon_trail_proc_decodes_by_truncation() {
    // Kit 324's shipped row: Zero = 16263465, One = 20, Two = 600, Three = 100.
    let proc = CharProc {
        ty: char_proc_type::WEAPON_TRAIL,
        params: [16_263_465.0, 20.0, 600.0, 100.0],
    };
    let trail = proc.as_weapon_trail().expect("a live trail proc");
    assert_eq!(trail.rgb(), [0xf8, 0x29, 0x29], "R248 G41 B41");
    assert_eq!(trail.alpha(), 100);
    assert_eq!(trail.duration_ms, 600);
    assert_eq!(
        trail.packed, 0x64f8_2929,
        "0xAARRGGBB, as `unit+0xd1c` holds it"
    );
    assert_ne!(
        proc.small_int(0),
        u32::from(trail.rgb()[0]),
        "the small-int decode is the WRONG idiom here and must not be reused"
    );
}

/// A zero duration fires nothing.
#[test]
fn a_zero_duration_trail_proc_is_no_trail() {
    let proc = CharProc {
        ty: char_proc_type::WEAPON_TRAIL,
        params: [16_263_465.0, 20.0, 0.0, 100.0],
    };
    assert!(proc.as_weapon_trail().is_none());
}

#[test]
fn only_type_eight_arms_a_trail() {
    for ty in [
        char_proc_type::TINT,
        char_proc_type::ALPHA,
        char_proc_type::ANIM_RATE,
        char_proc_type::CHAIN_CAST,
    ] {
        let proc = CharProc {
            ty,
            params: [16_263_465.0, 20.0, 600.0, 100.0],
        };
        assert!(proc.as_weapon_trail().is_none(), "type {ty}");
    }
}
