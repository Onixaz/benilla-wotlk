use super::player::player_slot_of_field;
use super::*;

/// Each block's length and a field per run, against the offsets in the reference's descriptor
/// tables (`0xac5ce8` item, `0xac5cac` container, `0xac5fe0` unit, `0xac66d8` player, `0xac77a8`
/// gameobject, `0xac7848` dynamic object, `0xac78b0` corpse), each added to the end of the block
/// before it.
#[test]
fn indices_are_the_12340_descriptor_offsets() {
    assert_eq!(
        [
            OBJECT_END,
            ITEM_END,
            CONTAINER_END,
            UNIT_END,
            PLAYER_END,
            GAMEOBJECT_END,
            DYNAMICOBJECT_END,
            CORPSE_END
        ],
        [6, 64, 138, 148, 1326, 18, 12, 36]
    );
    // Item: `ITEM_FIELD_ENCHANTMENT_1_1` at 16, `_12_3` at 51, `CREATE_PLAYED_TIME` at 56.
    assert_eq!(FIELD_ITEM_ENCHANTMENT_1_1, 6 + 16);
    assert_eq!(FIELD_ITEM_ENCHANTMENT_1_1 + 3 * 11 + 2, 6 + 51);
    assert_eq!(FIELD_ITEM_CREATE_PLAYED_TIME, 6 + 56);
    assert_eq!(FIELD_CONTAINER_SLOT_1, 64 + 2);
    // Unit: health 18, the seven powers 19..=25, maxima 27..=33, level 48, bytes 1 68, bytes 2 116,
    // the cost multiplier 132.
    assert_eq!(FIELD_UNIT_HEALTH, 6 + 18);
    assert_eq!(FIELD_UNIT_POWER1 + 6, 6 + 25);
    assert_eq!(FIELD_UNIT_MAXPOWER1 + 6, 6 + 33);
    assert_eq!(FIELD_UNIT_LEVEL, 6 + 48);
    assert_eq!(FIELD_UNIT_BYTES_1, 6 + 68);
    assert_eq!(FIELD_UNIT_BYTES_2, 6 + 116);
    assert_eq!(FIELD_UNIT_POWER_COST_MULTIPLIER, 6 + 132);
    assert_eq!(FIELD_UNIT_HOVERHEIGHT, 6 + 140);
    // Player: quest log 10, visible items 135, title 173, inventory 176, far sight 476, skills
    // 488, coinage 1022, field bytes 1049, bytes 2 1081, daily quests 1132, pet spell power 1177.
    assert_eq!(FIELD_PLAYER_QUEST_LOG_1_1, 148 + 10);
    assert_eq!(player_visible_item_entry_field(0), 148 + 135);
    assert_eq!(player_visible_item_entry_field(18) + 1, 148 + 172);
    assert_eq!(FIELD_PLAYER_CHOSEN_TITLE, 148 + 173);
    assert_eq!(FIELD_PLAYER_INV_SLOT_HEAD, 148 + 176);
    assert_eq!(FIELD_PLAYER_BANK_BAG_SLOT_1, 148 + 310);
    assert_eq!(FIELD_PLAYER_FARSIGHT, 148 + 476);
    assert_eq!(FIELD_PLAYER_SKILL_INFO_1_1, 148 + 488);
    assert_eq!(FIELD_PLAYER_EXPERTISE, 148 + 879);
    assert_eq!(FIELD_PLAYER_SHIELD_BLOCK, 148 + 891);
    assert_eq!(FIELD_PLAYER_EXPLORED_ZONES_1, 148 + 893);
    assert_eq!(FIELD_PLAYER_FIELD_COINAGE, 148 + 1022);
    assert_eq!(FIELD_PLAYER_FIELD_BYTES, 148 + 1049);
    assert_eq!(FIELD_PLAYER_FIELD_KILLS, 148 + 1077);
    assert_eq!(FIELD_PLAYER_FIELD_BYTES2, 148 + 1081);
    assert_eq!(FIELD_PLAYER_FIELD_ARENA_TEAM_INFO_1_1, 148 + 1108);
    assert_eq!(FIELD_PLAYER_DAILY_QUESTS_1, 148 + 1132);
    assert_eq!(FIELD_PLAYER_GLYPHS_ENABLED, 148 + 1176);
    assert_eq!(FIELD_PLAYER_PET_SPELL_POWER + 1, PLAYER_END);
    // GameObject, dynamic object, corpse.
    assert_eq!(FIELD_GAMEOBJECT_DYNAMIC, 6 + 8);
    assert_eq!(FIELD_GAMEOBJECT_BYTES_1 + 1, GAMEOBJECT_END);
    assert_eq!(FIELD_DYNAMICOBJECT_CASTTIME + 1, DYNAMICOBJECT_END);
    assert_eq!(FIELD_CORPSE_ITEM, 6 + 5);
    assert_eq!(FIELD_CORPSE_DYNAMIC_FLAGS, 6 + 28);
}

#[test]
fn unit_stats_and_resistances_read_indexed_and_gate_range() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_STAT0, 25),                            // strength
        (FIELD_UNIT_STAT0 + 4, 31),                        // spirit
        (FIELD_UNIT_RESISTANCES, 120),                     // armor
        (FIELD_UNIT_RESISTANCES + 2, 15),                  // fire
        (FIELD_UNIT_RESISTANCES + 6, 8u32.wrapping_neg()), // arcane, cursed to −8
    ]);
    assert_eq!(f.unit_stat(0), Some(25));
    assert_eq!(f.unit_stat(4), Some(31));
    assert_eq!(f.unit_stat(1), None, "never streamed");
    assert_eq!(f.unit_stat(5), None, "out of range");
    assert_eq!(f.unit_resistance(0), Some(120));
    assert_eq!(f.unit_resistance(2), Some(15));
    assert_eq!(f.unit_resistance(6), Some(-8), "resistances are signed");
    assert_eq!(f.unit_resistance(7), None, "out of range");
}

#[test]
fn attack_power_mods_split_signed_halves() {
    // The positive mod in the low half, the negative in the high: pos 30, neg -10.
    let packed = u32::from(30u16) | (u32::from((-10i16) as u16) << 16);
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_ATTACK_POWER, 78),
        (FIELD_UNIT_ATTACK_POWER_MODS, packed),
        // The multiplier less 1.0.
        (FIELD_UNIT_ATTACK_POWER_MULTIPLIER, 0.1f32.to_bits()),
        (FIELD_UNIT_RANGED_ATTACK_POWER, 52),
        (
            FIELD_UNIT_RANGED_ATTACK_POWER_MODS,
            u32::from((-3i16) as u16) << 16,
        ),
        (FIELD_UNIT_RANGED_ATTACK_POWER_MULTIPLIER, 0.0f32.to_bits()),
    ]);
    assert_eq!(f.unit_attack_power(), Some(78));
    assert_eq!(f.unit_attack_power_mods(), (30, -10));
    assert_eq!(f.unit_attack_power_multiplier(), Some(0.1));
    assert_eq!(f.unit_ranged_attack_power(), Some(52));
    assert_eq!(f.unit_ranged_attack_power_mods(), (0, -3));
    assert_eq!(f.unit_ranged_attack_power_multiplier(), Some(0.0));
    assert_eq!(ObjectFields::default().unit_attack_power_mods(), (0, 0));
}

#[test]
fn damage_and_attack_time_fields_read_their_slots() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_BASEATTACKTIME, 2900),
        (FIELD_UNIT_BASEATTACKTIME + 1, 1500),
        (FIELD_UNIT_RANGEDATTACKTIME, 2800),
        (FIELD_UNIT_MINDAMAGE, 12.5f32.to_bits()),
        (FIELD_UNIT_MAXDAMAGE, 19.5f32.to_bits()),
        (FIELD_UNIT_MINOFFHANDDAMAGE, 5.0f32.to_bits()),
        (FIELD_UNIT_MAXOFFHANDDAMAGE, 9.0f32.to_bits()),
        (FIELD_UNIT_MINRANGEDDAMAGE, 31.0f32.to_bits()),
        (FIELD_UNIT_MAXRANGEDDAMAGE, 47.0f32.to_bits()),
    ]);
    assert_eq!(f.unit_base_attack_time(0), Some(2900));
    assert_eq!(f.unit_base_attack_time(1), Some(1500));
    assert_eq!(f.unit_base_attack_time(2), None, "out of range");
    assert_eq!(f.unit_ranged_attack_time(), Some(2800));
    assert_eq!(f.unit_min_damage(), Some(12.5));
    assert_eq!(f.unit_max_damage(), Some(19.5));
    assert_eq!(f.unit_min_offhand_damage(), Some(5.0));
    assert_eq!(f.unit_max_offhand_damage(), Some(9.0));
    assert_eq!(f.unit_min_ranged_damage(), Some(31.0));
    assert_eq!(f.unit_max_ranged_damage(), Some(47.0));
}

/// The buff-split arrays are ints on the wire, narrowed from the server's floats; `-4` is an x86
/// server's word, where arm64 would send 0.
#[test]
fn stat_buff_arrays_read_signed_ints() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POSSTAT0, 10),
        (FIELD_UNIT_NEGSTAT0, (-4i32) as u32),
        (FIELD_UNIT_RESISTANCEBUFFMODSPOSITIVE, 30),
        (FIELD_UNIT_RESISTANCEBUFFMODSNEGATIVE + 2, (-20i32) as u32),
    ]);
    assert_eq!(f.player_posstat(0), Some(10));
    assert_eq!(f.player_negstat(0), Some(-4));
    assert_eq!(f.player_posstat(5), None, "out of range");
    assert_eq!(f.player_negstat(5), None, "out of range");
    assert_eq!(f.player_resistance_buff_pos(0), Some(30));
    assert_eq!(f.player_resistance_buff_neg(2), Some(-20));
    assert_eq!(f.player_resistance_buff_pos(7), None, "out of range");
    assert_eq!(f.player_resistance_buff_neg(7), None, "out of range");
}

#[test]
fn player_mod_damage_done_reads_int_pos_neg_and_float_pct() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_PLAYER_MOD_DAMAGE_DONE_POS, 25),
        (FIELD_PLAYER_MOD_DAMAGE_DONE_POS + 2, 40),
        (FIELD_PLAYER_MOD_DAMAGE_DONE_NEG, 15u32.wrapping_neg()),
        (FIELD_PLAYER_MOD_DAMAGE_DONE_PCT, 1.1f32.to_bits()),
    ]);
    assert_eq!(f.player_mod_damage_done_pos(0), Some(25));
    assert_eq!(f.player_mod_damage_done_pos(2), Some(40));
    assert_eq!(f.player_mod_damage_done_neg(0), Some(-15));
    assert_eq!(f.player_mod_damage_done_pct(0), Some(1.1));
    assert_eq!(f.player_mod_damage_done_pct(1), None, "never streamed");
    assert_eq!(f.player_mod_damage_done_pos(7), None, "out of range");
    assert_eq!(f.player_mod_damage_done_neg(7), None, "out of range");
    assert_eq!(f.player_mod_damage_done_pct(7), None, "out of range");
}

#[test]
fn player_skill_unpacks_the_three_dword_triplet() {
    // Slot 1: Swords (43) step 0, 25/300, temp -3, perm +5; lo|hi pairs, bonuses signed.
    let base = FIELD_PLAYER_SKILL_INFO_1_1 + 3;
    let f = ObjectFields::from_pairs(&[
        (base, 43),
        (base + 1, 25 | (300 << 16)),
        (base + 2, u32::from((-3i16) as u16) | (5 << 16)),
    ]);
    assert_eq!(
        f.player_skill(1),
        Some(PlayerSkillSlot {
            skill_id: 43,
            step: 0,
            value: 25,
            max: 300,
            temp_bonus: -3,
            perm_bonus: 5,
        })
    );
    assert_eq!(f.player_skill(0), None);
    assert_eq!(
        ObjectFields::from_pairs(&[(FIELD_PLAYER_SKILL_INFO_1_1, 0)])
            .player_skill(0)
            .map(|s| s.skill_id),
        Some(0)
    );
    assert_eq!(f.player_skill(PLAYER_SKILL_SLOTS), None);
}

#[test]
fn player_self_res_spell_collapses_absent_and_zero() {
    // Its neighbours are set, so a wrong index reads one. 3026 is the rank-1 soulstone's spell.
    let f = ObjectFields::from_pairs(&[
        (FIELD_PLAYER_FIELD_BYTES, 0),
        (FIELD_PLAYER_AMMO_ID, 2512),
        (FIELD_PLAYER_SELF_RES_SPELL, 3026),
    ]);
    assert_eq!(f.player_self_res_spell(), Some(3026));
    assert_eq!(
        f.player_ammo_id(),
        Some(2512),
        "the neighbour is not disturbed"
    );
    assert_eq!(ObjectFields::default().player_self_res_spell(), None);
    assert_eq!(
        ObjectFields::from_pairs(&[(FIELD_PLAYER_SELF_RES_SPELL, 0)]).player_self_res_spell(),
        None
    );
}

/// `PLAYER_FIELD_BYTES`: flags, Refer-a-Friend levels, the action bar toggles, the lifetime
/// highest honor rank.
#[test]
fn player_field_bytes_splits_into_flags_raf_toggles_and_honor_rank() {
    let f = ObjectFields::from_pairs(&[(FIELD_PLAYER_FIELD_BYTES, 0x05_03_02_18)]);
    assert!(f.player_release_timer_running() && f.player_no_release_window());
    assert_eq!(f.player_raf_grantable_level(), 2);
    assert_eq!(f.player_action_bar_toggles(), Some(3));
    assert_eq!(f.player_honor_rank(), Some(5));
    // The accessor returns the raw byte, high nibble included; only the binding masks to 4 bits.
    let high = ObjectFields::from_pairs(&[(FIELD_PLAYER_FIELD_BYTES, 0x00_f5_00_00)]);
    assert_eq!(high.player_action_bar_toggles(), Some(0xf5));
    assert_eq!(ObjectFields::default().player_honor_rank(), None);
    assert_eq!(ObjectFields::default().player_action_bar_toggles(), None);
}

/// `PLAYER_BYTES_3`: gender, drunkenness, PvP title, arena faction.
#[test]
fn player_bytes_3_reads_gender_drunkenness_and_arena_faction() {
    let f = ObjectFields::from_pairs(&[(FIELD_PLAYER_BYTES_3, 0x01_04_A0_01)]);
    assert_eq!(f.player_gender_byte(), Some(1));
    assert_eq!(f.player_drunk_byte(), Some(0xA0));
    assert_eq!(f.player_arena_faction(), Some(1));
}

/// `PLAYER_FIELD_BYTES2`: the override-spells id in the low `i16`, the aura vision in byte 3.
#[test]
fn player_field_bytes2_reads_the_override_spells_and_the_aura_vision() {
    let f = ObjectFields::from_pairs(&[(FIELD_PLAYER_FIELD_BYTES2, 0x6056_1234)]);
    assert_eq!(f.player_override_spells_id(), Some(0x1234));
    assert_eq!(f.player_aura_vision(), Some(0x60));
}

/// `PLAYER_FIELD_KILLS` is `TWO_SHORT`, today's then yesterday's honorable kills.
#[test]
fn honor_fields_read_their_own_words() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_PLAYER_FIELD_KILLS, 0x0029_0011),
        (FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION, 1_250),
        (FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION, 640),
        (FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS, 3_907),
        (FIELD_PLAYER_FIELD_HONOR_CURRENCY, 75_000),
    ]);
    assert_eq!(f.player_kills_today(), Some(17));
    assert_eq!(f.player_kills_yesterday(), Some(41));
    assert_eq!(f.player_today_contribution(), Some(1_250));
    assert_eq!(f.player_yesterday_contribution(), Some(640));
    assert_eq!(f.player_lifetime_honorable_kills(), Some(3_907));
    assert_eq!(f.player_honor_currency(), Some(75_000));
    assert_eq!(ObjectFields::default().player_kills_today(), None);
}

/// `UNIT_FIELD_BYTES_1`: stand state, pet talents, the visibility flags (`0x1` ghost, `0x2`
/// creep, `0x4` untrackable) and the anim tier.
#[test]
fn unit_bytes_1_reads_stand_state_visibility_and_anim_tier() {
    let f = ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_1, 0x03_02_05_01)]);
    assert_eq!(f.unit_stand_state(), 1);
    assert!(f.unit_is_stealthed());
    assert!(!f.unit_is_ghost_visual());
    assert!(!f.unit_is_untrackable());
    assert_eq!(f.unit_anim_tier(), 3);
    let ghost = ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_1, 0x00_05_00_00)]);
    assert!(ghost.unit_is_ghost_visual() && ghost.unit_is_untrackable());
    assert_eq!(ghost.unit_anim_tier(), 0);
}

/// `UNIT_FIELD_BYTES_2`: sheath byte 0, flags byte 1, pet flags byte 2, form byte 3 (`0x71af70`).
#[test]
fn unit_bytes_2_reads_sheath_flags_and_form() {
    let f = ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_2, 0x1003_0901)]);
    assert_eq!(f.unit_sheath_state(), Some(1));
    assert_eq!(f.unit_byte2_flags(), 0x09);
    assert_eq!(f.unit_pet_flags(), 0x03);
    assert_eq!(f.unit_shapeshift_form(), 0x10);
}

/// A death knight's runes and runic power ride `POWER6`/`POWER7` and `MAXPOWER6`/`MAXPOWER7`, the
/// rune regen four floats.
#[test]
fn a_death_knight_s_seven_powers_and_rune_regen_all_read() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER1, 100),
        (FIELD_UNIT_POWER1 + 5, 8),
        (FIELD_UNIT_POWER1 + 6, 450),
        (FIELD_UNIT_MAXPOWER1, 200),
        (FIELD_UNIT_MAXPOWER1 + 5, 8),
        (FIELD_UNIT_MAXPOWER1 + 6, 1000),
        (FIELD_PLAYER_RUNE_REGEN_1, 0.1f32.to_bits()),
        (FIELD_PLAYER_RUNE_REGEN_1 + 3, 0.2f32.to_bits()),
    ]);
    assert_eq!(f.unit_power(0), Some(100));
    assert_eq!(f.unit_max_power(0), Some(200));
    assert_eq!((f.unit_power(5), f.unit_max_power(5)), (Some(8), Some(8)));
    assert_eq!(
        (f.unit_shown_power(6), f.unit_shown_max_power(6)),
        (Some(45), Some(100))
    );
    assert_eq!(f.unit_power(7), None);
    assert_eq!(f.player_rune_regen(0), 0.1);
    assert_eq!(f.player_rune_regen(3), 0.2);
    assert_eq!(f.player_rune_regen(1), 0.0);
    assert_eq!(f.player_rune_regen(4), 0.0);
}

/// A visible-item slot is the entry, then the permanent and temporary enchant as two `u16`.
#[test]
fn a_visible_item_slot_reads_its_entry_and_both_enchant_halves() {
    let chest = player_visible_item_entry_field(4);
    let f = ObjectFields::from_pairs(&[(chest, 6_096), (chest + 1, 1_897 | 25 << 16)]);
    assert_eq!(f.player_visible_item_entry(4), Some(6_096));
    assert_eq!(f.player_visible_item_enchant(4, 0), Some(1_897));
    assert_eq!(f.player_visible_item_enchant(4, 1), Some(25));
    assert_eq!(f.player_visible_item_enchant(4, 2), None, "two halves only");
    assert_eq!(f.player_visible_item_entry(3), None);
    assert_eq!(f.player_visible_item_entry(19), None);
}

#[test]
fn the_character_sheet_fields_read_where_the_12340_stat_verbs_read_them() {
    let f = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER_REGEN_FLAT_MODIFIER, 12.5f32.to_bits()),
        (
            FIELD_UNIT_POWER_REGEN_INTERRUPTED_FLAT_MODIFIER,
            3.25f32.to_bits(),
        ),
        (FIELD_UNIT_POWER_COST_MODIFIER + 2, 12),
        (FIELD_UNIT_POWER_COST_MODIFIER + 3, (-30i32) as u32),
        (FIELD_UNIT_POWER_COST_MODIFIER + 6, 7),
        (FIELD_UNIT_POWER_COST_MULTIPLIER + 2, (-0.25f32).to_bits()),
        (FIELD_UNIT_POWER_COST_MULTIPLIER + 3, 0.25f32.to_bits()),
        (FIELD_UNIT_POWER_COST_MULTIPLIER + 6, 0.5f32.to_bits()),
        (FIELD_UNIT_MAXHEALTHMODIFIER, 0.1f32.to_bits()),
        (FIELD_PLAYER_EXPERTISE, 12),
        (FIELD_PLAYER_EXPERTISE + 1, 7),
        (FIELD_PLAYER_RANGED_CRIT_PERCENTAGE, 4.5f32.to_bits()),
        (FIELD_PLAYER_SPELL_CRIT_PERCENTAGE1 + 1, 6.75f32.to_bits()),
        (FIELD_PLAYER_SHIELD_BLOCK, 95),
        (FIELD_PLAYER_MOD_HEALING_DONE_POS, 410),
        (FIELD_PLAYER_MOD_TARGET_RESISTANCE, (-35i32) as u32),
        (FIELD_PLAYER_COMBAT_RATING_1, 18),
        (FIELD_PLAYER_COMBAT_RATING_1 + 24, 40),
    ]);
    assert_eq!(f.unit_power_regen(0, false), Some(12.5));
    assert_eq!(f.unit_power_regen(0, true), Some(3.25));
    assert_eq!(f.unit_max_health_modifier(), Some(0.1));
    assert_eq!(f.unit_power_cost_modifier(1 << 2 | 1 << 3), -30);
    assert_eq!(f.unit_power_cost_modifier(1 << 2), 12);
    assert_eq!(f.unit_power_cost_modifier(1 << 6), 7);
    assert_eq!(f.unit_power_cost_modifier(0), 0);
    assert_eq!(f.unit_power_cost_multiplier(1 << 2 | 1 << 3), 0.75);
    assert_eq!(f.unit_power_cost_multiplier(1 << 6), 1.5);
    assert_eq!(f.unit_power_cost_multiplier(0), 1.0);
    assert_eq!(f.player_expertise(0), Some(12));
    assert_eq!(f.player_expertise(1), Some(7));
    assert_eq!(f.player_ranged_crit_percentage(), Some(4.5));
    assert_eq!(f.player_spell_crit_percentage(1), Some(6.75));
    assert_eq!(f.player_shield_block(), Some(95));
    assert_eq!(f.player_mod_healing_done_pos(), Some(410));
    assert_eq!(f.player_mod_target_resistance(), Some(-35));
    assert_eq!(f.player_combat_rating(0), Some(18));
    assert_eq!(f.player_combat_rating(24), Some(40));
    assert_eq!(f.player_combat_rating(25), None);
}

/// `GAMEOBJECT_BYTES_1` byte 2 is the art kit, a signed byte; with no field it is 0.
#[test]
fn a_gameobject_reads_its_art_kit_as_a_signed_byte() {
    let kit = |bytes: u32| {
        ObjectFields::from_pairs(&[(FIELD_GAMEOBJECT_BYTES_1, bytes)]).gameobject_art_kit()
    };
    assert_eq!(kit(0x0001_0001), 1);
    assert_eq!(kit(0x0051_0601), 81);
    assert_eq!(kit(0xFFFF_0601), -1, "0xff names no row");
    assert_eq!(ObjectFields::from_pairs(&[]).gameobject_art_kit(), 0);
}

/// `GAMEOBJECT_BYTES_1` byte 3 is the anim progress, a building's health out of 255.
#[test]
fn a_gameobject_reads_its_anim_progress_from_byte_3() {
    let progress = |bytes: u32| {
        ObjectFields::from_pairs(&[(FIELD_GAMEOBJECT_BYTES_1, bytes)]).gameobject_anim_progress()
    };
    // A full-health Wintergrasp wall: state 1, type 33, no art kit, progress 255.
    assert_eq!(progress(0xFF00_2101), 255);
    assert_eq!(progress(0x7F00_2101), 127);
    assert_eq!(ObjectFields::from_pairs(&[]).gameobject_anim_progress(), 0);
}

/// `GAMEOBJECT_BYTES_1` holds the state and type; `GAMEOBJECT_DYNAMIC`'s low half the dynamic
/// flags under a path progress. The parent rotation is not the spawn's own.
#[test]
fn a_gameobject_reads_its_packed_bytes_and_its_rotation_from_movement() {
    let mut f = ObjectFields::from_pairs(&[
        (FIELD_GAMEOBJECT_BYTES_1, 0xFF00_1301),
        (FIELD_GAMEOBJECT_DYNAMIC, 0x1234_0004),
        (FIELD_GAMEOBJECT_DISPLAYID, 5_000),
        (FIELD_GAMEOBJECT_FACTION, 35),
        (FIELD_GAMEOBJECT_PARENTROTATION + 3, 1.0f32.to_bits()),
    ]);
    assert_eq!(
        f.gameobject_rotation(),
        None,
        "the parent rotation is not the spawn's"
    );
    f.place_movement_values(ObjectType::GameObject, None, Some([0.0, 0.0, 0.6, 0.8]));
    let f = f.into_created(ObjectType::GameObject);
    assert_eq!(f.gameobject_state(), Some(1));
    assert_eq!(f.gameobject_type_id(), 19);
    assert_eq!(
        f.gameobject_art_kit(),
        0,
        "byte 3 is the progress, not the art kit"
    );
    assert_eq!(f.gameobject_dynamic_flags(), 0x4);
    assert_eq!(f.gameobject_displayid(), Some(5_000));
    assert_eq!(f.gameobject_faction(), 35);
    assert_eq!(f.gameobject_rotation(), Some([0.0, 0.0, 0.6, 0.8]));
    assert_eq!(f.gameobject_parent_rotation(), [0.0, 0.0, 0.0, 1.0]);
    // A values delta keeps the create's rotation.
    let mut live = f.clone();
    live.merge(ObjectFields::from_pairs(&[(
        FIELD_GAMEOBJECT_BYTES_1,
        0xFF00_1300,
    )]));
    assert_eq!(live.gameobject_state(), Some(0));
    assert_eq!(live.gameobject_rotation(), Some([0.0, 0.0, 0.6, 0.8]));
}

/// A dynamic object's position rides its create's movement block; `DYNAMICOBJECT_CASTTIME` is a
/// field of its own.
#[test]
fn a_dynamic_object_s_position_comes_from_its_movement_block() {
    let mut f = ObjectFields::from_pairs(&[
        (FIELD_DYNAMICOBJECT_CASTER, 26),
        (FIELD_DYNAMICOBJECT_BYTES, 1),
        (FIELD_DYNAMICOBJECT_SPELLID, 10),
        (FIELD_DYNAMICOBJECT_RADIUS, 8.0f32.to_bits()),
        (FIELD_DYNAMICOBJECT_CASTTIME, 123_456),
    ]);
    assert_eq!(f.dynamicobject_position(), None);
    f.place_movement_values(
        ObjectType::DynamicObject,
        Some((
            Vector3d {
                x: -8844.14,
                y: 668.83,
                z: 97.81,
            },
            0.5,
        )),
        None,
    );
    let f = f.into_created(ObjectType::DynamicObject);
    assert_eq!(f.dynamicobject_caster(), Some(26));
    assert_eq!(f.dynamicobject_bytes(), Some(1));
    assert_eq!(f.dynamicobject_spell_id(), Some(10));
    assert_eq!(f.dynamicobject_radius(), Some(8.0));
    assert_eq!(f.dynamicobject_cast_time(), Some(123_456));
    assert_eq!(
        f.dynamicobject_position(),
        Some(([-8844.14, 668.83, 97.81], 0.5))
    );
}

/// All twelve enchant triples sit in a row: permanent, a socket gem, the socket bonus, the last
/// random property.
#[test]
fn all_twelve_item_enchant_slots_read() {
    let e = FIELD_ITEM_ENCHANTMENT_1_1;
    let item = ObjectFields::from_pairs(&[
        (e, 3_000),
        (e + 3 * 2, 3_100),
        (e + 3 * 2 + 2, 2),
        (e + 3 * 5, 3_301),
        (e + 3 * 11, 3_400),
    ]);
    assert_eq!(item.item_enchant(0), Some(3_000));
    assert_eq!(item.item_enchant(2), Some(3_100));
    assert_eq!(item.item_enchant_charges(2), 2);
    assert_eq!(item.item_enchant(5), Some(3_301));
    assert_eq!(item.item_enchant(11), Some(3_400));
    assert_eq!(item.item_enchant(12), None);
}

/// `PLAYER_EXPLORED_ZONES_1` is 128 words: the later Northrend zones reveal through `exploreFlag`
/// 2048 and up, in words 64 and past, and rest XP follows.
#[test]
fn every_word_of_the_discovery_bitset_reads() {
    let z = FIELD_PLAYER_EXPLORED_ZONES_1;
    let f = ObjectFields::from_pairs(&[
        (z, 0x1),
        (z + 64, 0x2),
        (z + 127, 0x8000_0000),
        (FIELD_PLAYER_REST_STATE_EXPERIENCE, 4_200),
    ]);
    assert_eq!(f.player_explored_zone_slot(0), 0x1);
    assert_eq!(f.player_explored_zone_slot(64), 0x2);
    assert_eq!(f.player_explored_zone_slot(127), 0x8000_0000);
    assert_eq!(f.player_explored_zone_slot(128), 0);
    assert_eq!(f.player_rest_state_experience(), Some(4_200));
    assert_eq!(z + 128, FIELD_PLAYER_REST_STATE_EXPERIENCE);
}

/// A create omits zero fields, so a created store reads absent as 0,
/// like the client's zeroed descriptor; a bare delta reads it as untouched.
#[test]
fn created_store_reads_absent_fields_as_zero_a_delta_as_none() {
    // A broken item's create: MAXDURABILITY present (40), DURABILITY omitted (it is 0).
    let create =
        ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 2264), (FIELD_ITEM_MAXDURABILITY, 40)])
            .into_created(ObjectType::Item);
    assert_eq!(create.item_durability(), Some(0), "created absent = 0");
    assert_eq!(create.item_max_durability(), Some(40));

    // The same mask as a bare delta: absent means "not changed", never 0.
    let delta = ObjectFields::from_pairs(&[(FIELD_ITEM_MAXDURABILITY, 40)]);
    assert_eq!(delta.item_durability(), None, "delta absent = untouched");

    // A guid slot is present only if its low half was sent, even on a created store.
    assert_eq!(create.corpse_owner(), None);
}

#[test]
fn corpse_descriptor_indices_and_packing() {
    let corpse = ObjectFields::from_pairs(&[
        (FIELD_CORPSE_DISPLAY_ID, 49),
        // Slot 4 (chest): DisplayInfoID 902 | InventoryType 5 << 24.
        (FIELD_CORPSE_ITEM + 4, 902 | (5 << 24)),
        // BYTES_1: (0) | race<<8 | gender<<16 | skin<<24.
        (FIELD_CORPSE_BYTES_1, (6 << 8) | (1 << 16) | (3 << 24)),
        // BYTES_2: face | hairstyle<<8 | haircolor<<16 | facialhair<<24.
        (FIELD_CORPSE_BYTES_2, 7 | (2 << 8) | (4 << 16) | (5 << 24)),
        (FIELD_CORPSE_FLAGS, 0x04 | 0x08),
        (FIELD_CORPSE_DYNAMIC_FLAGS, 0x01),
    ])
    .into_created(ObjectType::Corpse);

    assert_eq!(corpse.corpse_display_id(), Some(49));
    assert_eq!(
        corpse.corpse_item(4),
        Some((902, 5)),
        "display | invType<<24"
    );
    assert_eq!(
        corpse.corpse_item(3),
        None,
        "an empty slot is absent, not (0, 0)"
    );

    let look = corpse.corpse_look().expect("both BYTES words present");
    assert_eq!(
        (
            look.race,
            look.sex,
            look.skin,
            look.face,
            look.hair_style,
            look.hair_color,
            look.facial_hair
        ),
        (6, 1, 3, 7, 2, 4, 5),
        "the seven bytes the reference's dress loads, in that order"
    );

    // Three independent bits; BONES picks the skeleton model.
    assert!(!corpse.corpse_is_bones(), "0x01 clear");
    assert!(corpse.corpse_hides_helm(), "0x08 set");
    assert!(!corpse.corpse_hides_cloak(), "0x10 clear");
    assert!(
        corpse.corpse_lootable(),
        "DYNAMIC_FLAGS bit 0 — a different field"
    );

    // A created corpse reads 0, not None, for every field below `CORPSE_END`.
    let bare =
        ObjectFields::from_pairs(&[(FIELD_CORPSE_DISPLAY_ID, 49)]).into_created(ObjectType::Corpse);
    assert_eq!(bare.corpse_flags(), 0);
    assert!(
        bare.corpse_look().is_some(),
        "absent BYTES read as the zero default"
    );
    assert!(!bare.corpse_lootable());
}

/// Absent reads 0 only inside the object's own descriptor: a creature has no player block, so a
/// player-block read off one is `None`.
#[test]
fn a_creature_has_no_player_block_to_be_absent_from() {
    // PLAYER_FIELD_MOD_DAMAGE_DONE_PCT[0] lies past a unit's descriptor.
    const MOD_DAMAGE_DONE_PCT: u16 = FIELD_PLAYER_MOD_DAMAGE_DONE_PCT;
    // UNIT_FIELD_BASEATTACKTIME, well inside it.
    const BASEATTACKTIME: u16 = FIELD_UNIT_BASEATTACKTIME;

    let pet = ObjectFields::from_pairs(&[(FIELD_OBJECT_TYPE, 0x09), (FIELD_UNIT_STAT0, 33)])
        .into_created(ObjectType::Unit);
    assert_eq!(
        pet.player_mod_damage_done_pct(0),
        None,
        "a creature cannot answer a PLAYER-block field"
    );
    assert_eq!(
        pet.unit_base_attack_time(0),
        Some(0),
        "…while a field it DOES have still reads the create's zero"
    );
    assert_eq!(pet.unit_stat(0), Some(33), "and a present field is itself");

    // On a player the same index is inside the descriptor, so absent reads 0.
    let player = ObjectFields::from_pairs(&[(FIELD_OBJECT_TYPE, 0x19), (BASEATTACKTIME, 1800)])
        .into_created(ObjectType::Player);
    assert_eq!(player.player_mod_damage_done_pct(0), Some(0.0));
    assert_eq!(player.unit_base_attack_time(0), Some(1800));

    // A bare delta reads absent as None either way.
    let delta = ObjectFields::from_pairs(&[(MOD_DAMAGE_DONE_PCT, 1.0f32.to_bits())]);
    assert_eq!(delta.player_mod_damage_done_pct(0), Some(1.0));
    assert_eq!(delta.unit_base_attack_time(0), None);
}

/// A re-create replaces the store, so a field that dropped to 0 out of view does not survive.
#[test]
fn merge_overlays_deltas_and_replaces_on_recreate() {
    const ENTRY: u16 = FIELD_OBJECT_ENTRY;
    const DUR: u16 = FIELD_ITEM_DURABILITY;
    const MAXDUR: u16 = FIELD_ITEM_MAXDURABILITY;
    let mut store = ObjectFields::from_pairs(&[(ENTRY, 2264), (DUR, 40), (MAXDUR, 40)])
        .into_created(ObjectType::Item);

    // A durability delta overlays; the store stays created.
    store.merge(ObjectFields::from_pairs(&[(DUR, 30)]));
    assert_eq!(store.item_durability(), Some(30));
    assert_eq!(
        store.item_max_durability(),
        Some(40),
        "untouched field kept"
    );

    // Re-create after the item broke out of view: the fresh snapshot omits DURABILITY (0).
    store.merge(
        ObjectFields::from_pairs(&[(ENTRY, 2264), (MAXDUR, 40)]).into_created(ObjectType::Item),
    );
    assert_eq!(
        store.item_durability(),
        Some(0),
        "a re-create replaces — the stale 30 must not survive an overlay"
    );
}

/// Like the reference's per-field notifier, which fires on a memcmp difference with the old
/// value: moved dwords only, ascending, absent as 0, an unchanged resend silent.
#[test]
fn merge_diff_reports_each_moved_dword_once_with_its_old_value() {
    const HP: u16 = FIELD_UNIT_HEALTH;
    const MAXHP: u16 = FIELD_UNIT_MAXHEALTH;
    const LEVEL: u16 = FIELD_UNIT_LEVEL;
    const FLAGS: u16 = FIELD_UNIT_FLAGS;
    let mut store = ObjectFields::from_pairs(&[(HP, 100), (MAXHP, 100), (LEVEL, 9)])
        .into_created(ObjectType::Unit);
    let mut edges = Vec::new();
    // Health drops, level rises, max-health re-sent as it was, a never-seen field appears.
    store.merge_diff(
        ObjectFields::from_pairs(&[(LEVEL, 10), (HP, 40), (MAXHP, 100), (FLAGS, 8)]),
        |i, old, new| edges.push((i, old, new)),
    );
    assert_eq!(
        edges,
        vec![(HP, 100, 40), (LEVEL, 9, 10), (FLAGS, 0, 8)],
        "moved dwords only, ascending; absent-before reads 0; an unchanged resend is silent"
    );
    assert_eq!(store.unit_health(), Some(40), "the delta still lands");
    assert_eq!(store.unit_flags(), 8);
}

/// A re-create refreshes a live guid in place and notifies over both masks: an omitted field is
/// an edge to 0.
#[test]
fn merge_diff_on_a_recreate_reports_the_replace_on_both_masks() {
    const ENTRY: u16 = FIELD_OBJECT_ENTRY;
    const GIFT: u16 = FIELD_ITEM_GIFTCREATOR;
    const DUR: u16 = FIELD_ITEM_DURABILITY;
    const MAXDUR: u16 = FIELD_ITEM_MAXDURABILITY;
    let mut store = ObjectFields::from_pairs(&[(ENTRY, 2264), (DUR, 30), (MAXDUR, 40)])
        .into_created(ObjectType::Item);
    let mut edges = Vec::new();
    store.merge_diff(
        ObjectFields::from_pairs(&[(ENTRY, 2264), (MAXDUR, 40), (GIFT, 7)])
            .into_created(ObjectType::Item),
        |i, old, new| edges.push((i, old, new)),
    );
    assert_eq!(
        edges,
        vec![(GIFT, 0, 7), (DUR, 30, 0)],
        "the omitted durability is a 30 → 0 edge; the identical entry and max are silent"
    );
    assert_eq!(store.item_durability(), Some(0));
}

#[test]
fn created_as_reads_back_off_the_created_length() {
    for t in [
        ObjectType::Object,
        ObjectType::Item,
        ObjectType::Container,
        ObjectType::Unit,
        ObjectType::Player,
        ObjectType::GameObject,
        ObjectType::DynamicObject,
        ObjectType::Corpse,
    ] {
        assert_eq!(
            ObjectFields::default().into_created(t).created_as(),
            Some(t)
        );
    }
    assert_eq!(
        ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 1)]).created_as(),
        None
    );
}

/// The dead-or-ghost test: health 0, or `PLAYER_FLAGS & 0x10`; a released ghost has health 1, so
/// only the flag catches it, and feign death is neither.
#[test]
fn dead_or_ghost_is_health_zero_or_the_ghost_flag() {
    let unit = |pairs: &[(u16, u32)]| ObjectFields::from_pairs(pairs);
    let alive = unit(&[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)]);
    let corpse = unit(&[(FIELD_UNIT_HEALTH, 0), (FIELD_UNIT_MAXHEALTH, 100)]);
    let ghost = unit(&[
        (FIELD_UNIT_HEALTH, 1),
        (FIELD_UNIT_MAXHEALTH, 100),
        (FIELD_PLAYER_FLAGS, 0x10),
    ]);
    let feigning = unit(&[
        (FIELD_UNIT_HEALTH, 100),
        (FIELD_UNIT_MAXHEALTH, 100),
        (FIELD_UNIT_DYNAMIC_FLAGS, 0x20),
    ]);
    assert!(!alive.is_dead_or_ghost());
    assert!(corpse.is_dead_or_ghost());
    assert!(!ghost.unit_is_dead(), "a ghost has health");
    assert!(ghost.is_dead_or_ghost(), "the ghost-flag leg");
    assert!(
        !feigning.is_dead_or_ghost(),
        "feign death is the reference's reads-dead test, not this"
    );
}

/// `Unit::SetFeignDeath` sets only `UNIT_DYNFLAG_DEAD`: the raw predicates stay false, the
/// reads-dead ones (the reference's reads-dead test, `UnitHealth`, `UnitMana`) flip, and the maxima
/// stay.
#[test]
fn feign_death_reads_dead_without_touching_the_raw_health_field() {
    const MANA: u8 = 0;
    let unit = |pairs: &[(u16, u32)]| ObjectFields::from_pairs(pairs);
    let alive = unit(&[
        (FIELD_UNIT_HEALTH, 1200),
        (FIELD_UNIT_MAXHEALTH, 1500),
        (FIELD_UNIT_POWER1, 300),
        (FIELD_UNIT_MAXPOWER1, 900),
    ]);
    let feigning = unit(&[
        (FIELD_UNIT_HEALTH, 1200),
        (FIELD_UNIT_MAXHEALTH, 1500),
        (FIELD_UNIT_POWER1, 300),
        (FIELD_UNIT_MAXPOWER1, 900),
        (FIELD_UNIT_DYNAMIC_FLAGS, 0x20),
    ]);
    let corpse = unit(&[(FIELD_UNIT_HEALTH, 0), (FIELD_UNIT_MAXHEALTH, 1500)]);

    // The raw predicate ignores the flag.
    assert!(!alive.unit_is_dead());
    assert!(!feigning.unit_is_dead(), "feign leaves HEALTH alone");
    assert!(corpse.unit_is_dead());
    assert_eq!(feigning.unit_health(), Some(1200), "the raw field survives");

    // The reads-dead predicate flips for both, plus the stand-state leg.
    assert!(!alive.unit_reads_dead());
    assert!(feigning.unit_reads_dead(), "the dynflag leg");
    assert!(corpse.unit_reads_dead());
    assert!(
        unit(&[
            (FIELD_UNIT_HEALTH, 1200),
            (FIELD_UNIT_MAXHEALTH, 1500),
            (FIELD_UNIT_BYTES_1, 7),
        ])
        .unit_reads_dead(),
        "the stand-state leg: stand state 7, as the reference has it"
    );

    // Current values go to 0 and the maxima stay, so the bar shows empty.
    assert_eq!(alive.unit_shown_health(), Some(1200));
    assert_eq!(feigning.unit_shown_health(), Some(0));
    assert_eq!(feigning.unit_max_health(), Some(1500));
    assert_eq!(alive.unit_shown_power(MANA), Some(300));
    assert_eq!(feigning.unit_shown_power(MANA), Some(0));
    assert_eq!(feigning.unit_max_power(MANA), Some(900));
    // The gate covers all seven types (`0x60ee13`); past them, None, as the ungated reader has it.
    assert_eq!(feigning.unit_shown_power(6), Some(0));
    assert_eq!(feigning.unit_shown_power(7), None);
    // An empty store has no flag and no health.
    assert!(!ObjectFields::default().unit_reads_dead());
    assert_eq!(ObjectFields::default().unit_shown_health(), None);
}

/// Rage max 1000 and happiness max 1050000 on the wire; the table divides them to the 100 and 1050
/// the reference shows.
#[test]
fn rage_and_happiness_divide_for_display_but_not_for_the_raw_readers() {
    const RAGE: u8 = 1;
    const HAPPINESS: u8 = 4;
    const MANA: u8 = 0;
    let warrior = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER1 + u16::from(RAGE), 350),
        (FIELD_UNIT_MAXPOWER1 + u16::from(RAGE), 1000),
    ]);
    assert_eq!(
        warrior.unit_power(RAGE),
        Some(350),
        "the wire value, untouched"
    );
    assert_eq!(warrior.unit_shown_power(RAGE), Some(35));
    assert_eq!(warrior.unit_shown_max_power(RAGE), Some(100), "not 1000");

    let pet = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER1 + u16::from(HAPPINESS), 1_020_000),
        (FIELD_UNIT_MAXPOWER1 + u16::from(HAPPINESS), 1_050_000),
    ]);
    assert_eq!(
        pet.unit_power(HAPPINESS),
        Some(1_020_000),
        "raw — the happiness bucket thresholds are on this scale"
    );
    assert_eq!(pet.unit_shown_power(HAPPINESS), Some(1020));
    assert_eq!(pet.unit_shown_max_power(HAPPINESS), Some(1050));

    // Mana, focus and energy divide by 1.
    let caster = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER1 + u16::from(MANA), 4200),
        (FIELD_UNIT_MAXPOWER1 + u16::from(MANA), 8000),
    ]);
    assert_eq!(caster.unit_shown_power(MANA), caster.unit_power(MANA));
    assert_eq!(
        caster.unit_shown_max_power(MANA),
        caster.unit_max_power(MANA)
    );
    assert_eq!(caster.unit_shown_power(RAGE), None);
    assert_eq!(caster.unit_shown_max_power(RAGE), None);
    // The dead gate still wins over the divide, and only for the current value.
    let feigning_warrior = ObjectFields::from_pairs(&[
        (FIELD_UNIT_POWER1 + u16::from(RAGE), 350),
        (FIELD_UNIT_MAXPOWER1 + u16::from(RAGE), 1000),
        (FIELD_UNIT_DYNAMIC_FLAGS, 0x20),
    ]);
    assert_eq!(feigning_warrior.unit_shown_power(RAGE), Some(0));
    assert_eq!(feigning_warrior.unit_shown_max_power(RAGE), Some(100));
}

#[test]
fn base_mana_and_base_health_read_their_fields() {
    let unit =
        ObjectFields::from_pairs(&[(FIELD_UNIT_BASE_MANA, 1500), (FIELD_UNIT_BASE_HEALTH, 1689)]);
    assert_eq!(unit.unit_base_mana(), Some(1500));
    assert_eq!(unit.unit_base_health(), Some(1689));
}

/// A predicted power reads until its field moves, capped at the maximum (`0x722c50`); a resend of
/// the field's old value moves nothing, and a maximum health change is not a power slot.
#[test]
fn a_predicted_power_reads_until_its_field_moves() {
    const ENERGY: u8 = 3;
    let power = FIELD_UNIT_POWER1 + u16::from(ENERGY);
    let max = FIELD_UNIT_MAXPOWER1 + u16::from(ENERGY);
    let mut f = ObjectFields::from_pairs(&[(power, 40), (max, 100)]).into_created(ObjectType::Unit);
    assert!(f.set_predicted_power(ENERGY, 55));
    assert_eq!(f.unit_power(ENERGY), Some(55));
    assert!(
        !f.set_predicted_power(ENERGY, 55),
        "the same value moves nothing"
    );
    assert!(f.set_predicted_power(ENERGY, 500));
    assert_eq!(f.unit_power(ENERGY), Some(100), "capped at the maximum");

    f.merge(ObjectFields::from_pairs(&[(power, 40)]));
    assert_eq!(
        f.unit_power(ENERGY),
        Some(100),
        "an unchanged field keeps it"
    );
    f.merge(ObjectFields::from_pairs(&[(FIELD_UNIT_MAXHEALTH, 900)]));
    assert_eq!(f.unit_power(ENERGY), Some(100));
    f.merge(ObjectFields::from_pairs(&[(power, 90)]));
    assert_eq!(
        f.unit_power(ENERGY),
        Some(90),
        "the field moved: it reads again"
    );

    f.set_predicted_power(ENERGY, 95);
    f.merge(ObjectFields::from_pairs(&[(max, 120)]));
    assert_eq!(
        f.unit_power(ENERGY),
        Some(90),
        "a maximum change re-seeds too"
    );
    assert!(!f.set_predicted_power(7, 1), "seven slots");
}

/// A predicted health reads through `UnitHealth`'s getter until the health field moves
/// (`0x73f35c`); the raw field and the maximum are left alone.
#[test]
fn a_predicted_health_reads_until_the_health_field_moves() {
    let mut f = ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 400), (FIELD_UNIT_MAXHEALTH, 500)])
        .into_created(ObjectType::Unit);
    assert!(f.set_predicted_health(350));
    assert_eq!(f.unit_shown_health(), Some(350));
    assert_eq!(f.unit_health(), Some(400), "the field itself is untouched");
    assert!(!f.set_predicted_health(350), "the same value moves nothing");

    f.merge(ObjectFields::from_pairs(&[(FIELD_UNIT_MAXHEALTH, 600)]));
    assert_eq!(
        f.unit_shown_health(),
        Some(350),
        "a maximum change keeps it"
    );
    f.merge(ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 420)]));
    assert_eq!(
        f.unit_shown_health(),
        Some(420),
        "the field moved: it reads again"
    );
}

/// A dropped mount reads 0 until the mount field moves, a re-create included (`0x741a40`).
#[test]
fn a_dropped_mount_reads_zero_until_its_field_moves() {
    let mount = |id: u32| ObjectFields::from_pairs(&[(FIELD_UNIT_MOUNTDISPLAYID, id)]);
    let mut f = mount(14_376).into_created(ObjectType::Unit);
    assert!(f.drop_mount());
    assert_eq!(f.unit_mount_display_id(), 0);
    f.merge(mount(14_376).into_created(ObjectType::Unit));
    assert_eq!(
        f.unit_mount_display_id(),
        0,
        "a re-create of the same mount stays dropped"
    );
    f.merge(mount(0));
    assert!(!f.drop_mount(), "nothing left to drop");
    f.merge(mount(2_410));
    assert_eq!(f.unit_mount_display_id(), 2_410, "a new mount reads");
}

/// An early form cancel reads no form (`0x71af70`) until the form byte moves (`0x6e538a`, `0x6da174`): a move of another byte of `UNIT_FIELD_BYTES_2` keeps it.
#[test]
fn an_early_form_cancel_reads_no_form_until_the_form_byte_moves() {
    let bytes2 = |v: u32| ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_2, v)]);
    let mut f = bytes2(1 << 24).into_created(ObjectType::Player);
    assert_eq!(f.unit_shapeshift_form(), 1);
    f.suppress_form();
    assert_eq!(f.unit_shapeshift_form(), 0);
    f.merge(bytes2((1 << 24) | 0x08));
    assert_eq!(
        f.unit_shapeshift_form(),
        0,
        "the form byte held: still none"
    );
    f.merge(bytes2(0x08));
    assert!(
        !f.unit_form_suppressed(),
        "the form moved: the cancel is over"
    );
    f.merge(bytes2(5 << 24));
    assert_eq!(f.unit_shapeshift_form(), 5);
}

/// A held dismount reads unmounted while the copy stays, through a move back to the copy, and a
/// move to another display clears it (`0x51a230`, `0x740450`, `0x741a7f`).
#[test]
fn a_held_mount_reads_unmounted_until_its_field_names_another() {
    let mount = |id: u32| ObjectFields::from_pairs(&[(FIELD_UNIT_MOUNTDISPLAYID, id)]);
    let mut f = mount(14_376).into_created(ObjectType::Unit);
    assert!(f.unit_is_mounted());
    f.hold_mount(f.unit_mount_copy());
    assert!(!f.unit_is_mounted(), "held while passengers ride");
    assert_eq!(f.unit_mount_copy(), 14_376, "the copy stays");
    f.merge(mount(0));
    assert_eq!(
        f.unit_mount_copy(),
        0,
        "a zero with no hold renewed clears it"
    );
    f.hold_mount(14_376);
    f.merge(mount(14_376));
    assert!(!f.unit_is_mounted(), "the copy it holds is a no-op");
    f.merge(mount(2_410));
    assert!(f.unit_is_mounted(), "another display clears the hold");
    f.hold_mount(0);
    assert!(f.unit_is_mounted(), "no mount, nothing held");
}

/// Each slot array's guid words map to their player slot and back, one past each run to none.
#[test]
fn a_slot_field_names_its_player_slot_and_word() {
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_INV_SLOT_HEAD),
        Some((0, false))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_PACK_SLOT_1 + 1),
        Some((23, true))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_BANK_BAG_SLOT_1 + 13),
        Some((73, true))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_KEYRING_SLOT_1 + 63),
        Some((117, true))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1),
        Some((118, false))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 63),
        Some((149, true))
    );
    assert_eq!(
        player_slot_of_field(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 64),
        None
    );
    assert_eq!(player_slot_of_field(FIELD_PLAYER_FARSIGHT), None);
    let mut pairs = Vec::new();
    for slot in 0..150u8 {
        let lo = (0..u16::MAX)
            .find(|&i| player_slot_of_field(i) == Some((slot, false)))
            .unwrap();
        pairs.push((lo, u32::from(slot) + 1));
    }
    let f = ObjectFields::from_pairs(&pairs);
    for slot in 0..150u8 {
        assert_eq!(
            f.player_slot(slot),
            Some(u64::from(slot) + 1),
            "slot {slot}"
        );
    }
    assert_eq!(f.player_slot(150), None);
}
