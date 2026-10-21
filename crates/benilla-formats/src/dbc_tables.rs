//! Every DBC table benilla reads, with the width its loader holds the file to: the loader's own
//! schema, or the packed width a hand parser checks. `reference/3.3.5a-dbc.tsv` holds the widths
//! 12340's loaders check; `tests/dbc_coverage.rs` measures this registry against it.

use benilla_dbc::Schema;

use crate::dbc_layout::LAYOUTS;
use crate::*;

macro_rules! tables {
    (
        schemas { $($table:literal => $schema:expr,)* }
        packed { $($packed:literal => $width:expr,)* }
    ) => {
        /// Every DBC table benilla reads, by file stem.
        pub const READ_TABLES: &[&str] = &[$($table,)* $($packed,)*];

        /// Our schema for `table` (a file stem), the one its loader applies; `None` for a table
        /// read by hand off its packed record, or not read at all.
        pub fn dbc_schema(table: &str) -> Option<Schema> {
            match table {
                $($table => Some($schema),)*
                _ => None,
            }
        }

        /// `(columns, row bytes)` a loader requires of `table`'s file header.
        pub fn dbc_width(table: &str) -> Option<(usize, usize)> {
            if let Some(schema) = dbc_schema(table) {
                return Some((schema.expanded_len(), schema.record_width()));
            }
            match table {
                $($packed => Some($width),)*
                _ => None,
            }
        }
    };
}

tables! {
    schemas {
        "Achievement" => achievements::achievement_schema(),
        "Achievement_Category" => achievements::category_schema(),
        "Achievement_Criteria" => achievements::criteria_schema(),
        "AnimationData" => anim_data::schema(),
        "AreaPOI" => area_poi::schema(),
        "AreaTable" => area_table::area_table_schema(&LAYOUTS.area_table),
        "AreaTrigger" => area_trigger::area_trigger_schema(),
        "AuctionHouse" => auction_house::auction_house_schema(),
        "BattlemasterList" => battlemaster_list::battlemaster_list_schema(),
        "BankBagSlotPrices" => bank_bag_slot_prices::schema(),
        "CameraShakes" => camera_shakes::camera_shakes_schema(),
        "Cfg_Categories" => cfg_categories::cfg_categories_schema(),
        "CharacterFacialHairStyles" => {
            characters::char_facial_hair_schema(&LAYOUTS.character_facial_hair_styles)
        },
        "CharHairGeosets" => characters::char_hair_geosets_schema(),
        "CharSections" => characters::char_sections_schema(&LAYOUTS.char_sections),
        "CharTitles" => quest_rewards::titles_schema(),
        "ChatChannels" => chat_channels::schema(&LAYOUTS.chat_channels),
        "ChatProfanity" => text_filter_lists::chat_profanity_schema(),
        "ChrClasses" => chr_classes::schema(),
        "ChrRaces" => characters::chr_races_schema(&LAYOUTS.chr_races),
        "CinematicCamera" => cinematics::cameras_schema(),
        "CinematicSequences" => cinematics::sequences_schema(),
        "CreatureDisplayInfo" => creatures::creature_display_info_schema(&LAYOUTS.creature_display_info),
        "CreatureDisplayInfoExtra" => {
            creatures::creature_display_info_extra_schema(&LAYOUTS.creature_display_info_extra)
        },
        "CreatureFamily" => creature_families::family_schema(),
        "CreatureModelData" => creatures::creature_model_data_schema(&LAYOUTS.creature_model_data),
        "CreatureSoundData" => creature_sound::csd_schema(),
        "CreatureType" => creature_types::creature_type_schema(),
        "DeathThudLookups" => death_thud::death_thud_lookups_schema(),
        "DungeonEncounter" => dungeon_encounter::schema(),
        "DungeonMap" => dungeon_map::schema(),
        "DurabilityCosts" => durability::costs_schema(),
        "DurabilityQuality" => durability::qualities_schema(),
        "Emotes" => emotes::emotes_schema(),
        "EmotesText" => emote_text::emotes_text_schema(),
        "EmotesTextData" => emote_text::emotes_text_data_schema(&LAYOUTS.emotes_text_data),
        "EmotesTextSound" => emotes::emotes_text_sound_schema(),
        "EnvironmentalDamage" => environmental_damage::schema(),
        "Exhaustion" => exhaustion::exhaustion_schema(),
        "Faction" => factions::faction_schema(),
        "FactionGroup" => factions::faction_group_schema(),
        "FactionTemplate" => factions::faction_template_schema(),
        "FootprintTextures" => footsteps::footprint_textures_schema(),
        "FootstepTerrainLookup" => footsteps::footstep_terrain_lookup_schema(),
        "GameObjectDisplayInfo" => gameobjects::gameobject_display_info_schema(),
        "GameTables" => game_tables::game_tables_schema(),
        "GameTips" => game_tips::schema(&LAYOUTS.game_tips),
        "GMTicketCategory" => gm_ticket_category::gm_ticket_category_schema(),
        "GroundEffectDoodad" => ground_effects::doodad_schema(),
        "GroundEffectTexture" => ground_effects::ground_effect_texture_schema(),
        "gtChanceToMeleeCrit" => game_tables::gt_value_schema("gtChanceToMeleeCrit"),
        "gtChanceToMeleeCritBase" => game_tables::gt_value_schema("gtChanceToMeleeCritBase"),
        "gtChanceToSpellCrit" => game_tables::gt_value_schema("gtChanceToSpellCrit"),
        "gtChanceToSpellCritBase" => game_tables::gt_value_schema("gtChanceToSpellCritBase"),
        "gtCombatRatings" => game_tables::gt_value_schema("gtCombatRatings"),
        "gtOCTClassCombatRatingScalar" => game_tables::gt_keyed_value_schema(),
        "gtOCTRegenHP" => game_tables::gt_value_schema("gtOCTRegenHP"),
        "gtRegenHPPerSpt" => game_tables::gt_value_schema("gtRegenHPPerSpt"),
        "gtRegenMPPerSpt" => game_tables::gt_value_schema("gtRegenMPPerSpt"),
        "HelmetGeosetVisData" => characters::helmet_vis_schema(&LAYOUTS.helmet_geoset_vis_data),
        "ItemBagFamily" => itembagfamily::item_bag_family_schema(),
        "ItemClass" => itemclass::item_class_schema(),
        "ItemDisplayInfo" => items::item_display_info_schema(&LAYOUTS.item_display_info),
        "ItemExtendedCost" => item_extended_cost::schema(),
        "ItemGroupSounds" => item_sounds::item_group_sounds_schema(),
        "ItemPetFood" => creature_families::food_schema(),
        "ItemRandomProperties" => item_random_properties::item_random_properties_schema(),
        "ItemSet" => itemsets::item_set_schema(),
        "ItemSubClass" => itemsubclass::item_sub_class_schema(),
        "ItemSubClassMask" => itemsubclass::item_sub_class_mask_schema(),
        "ItemVisualEffects" => item_visuals::item_visual_effects_schema(),
        "ItemVisuals" => item_visuals::item_visuals_schema(),
        "Languages" => languages::languages_schema(),
        "LanguageWords" => languages::language_words_schema(),
        "LfgDungeonExpansion" => lfg_dungeons::expansions_schema(),
        "LfgDungeonGroup" => lfg_dungeons::groups_schema(),
        "LfgDungeons" => lfg_dungeons::dungeons_schema(),
        "Light" => light::light_schema(),
        "LightFloatBand" => light::light_float_band_schema(),
        "LightIntBand" => light::light_int_band_schema(),
        "LightParams" => light::light_params_schema(),
        "LightSkybox" => light::light_skybox_schema(),
        "LiquidMaterial" => liquid_type::material_schema(),
        "LiquidType" => liquid_type::type_schema(),
        "LoadingScreens" => loading_screen::schema(&LAYOUTS.loading_screens),
        "Lock" => lock::schema(),
        "LockType" => lock_type::schema(),
        "Map" => maps::map_schema(&LAYOUTS.map),
        "MapDifficulty" => map_difficulty::schema(),
        "Material" => material::schema(),
        "NPCSounds" => npc_greeting::npcsounds_schema(),
        "NameGen" => characters::name_gen_schema(),
        "Package" => packages::schema(),
        "PageTextMaterial" => page_text_material::schema(),
        "ParticleColor" => particles::particle_color_schema(),
        "PetPersonality" => pet_stats::personality_schema(),
        "PvpDifficulty" => battlemaster_list::pvp_difficulty_schema(),
        "QuestFactionReward" => quest_rewards::quest_faction_reward_schema(),
        "QuestInfo" => quest_info::quest_info_schema(),
        "QuestSort" => quest_headers::quest_sort_schema(),
        "QuestXP" => quest_rewards::quest_xp_schema(),
        "ServerMessages" => server_messages::schema(&LAYOUTS.server_messages),
        "SkillLine" => skill_lines::skill_line_schema(),
        "SkillLineAbility" => skill_lines::skill_line_ability_schema(),
        "SkillLineCategory" => skill_lines::skill_line_category_schema(),
        "SkillRaceClassInfo" => skill_lines::skill_race_class_info_schema(),
        "SoundAmbience" => area_sound::ambience_schema(),
        "SoundEntries" => sound_entries::sound_entries_schema(),
        "SoundProviderPreferences" => sound_provider::schema(),
        "SpamMessages" => text_filter_lists::spam_messages_schema(),
        "Spell" => spells::spell_schema(),
        "SpellCastTimes" => spells::cast_times::schema(),
        "SpellCategory" => spells::spell_category_schema(),
        "SpellChainEffects" => spell_visual::chain_effects::chain_effects_schema(),
        "SpellDescriptionVariables" => spells::desc_vars::schema(),
        "SpellDispelType" => spells::dispel_types::schema(),
        "SpellDuration" => spells::duration::schema(),
        "SpellEffectCameraShakes" => camera_shakes::spell_effect_camera_shakes_schema(),
        "SpellFocusObject" => spell_focus::schema(),
        "SpellIcon" => dbc::spell_icon_schema(),
        "SpellItemEnchantment" => item_visuals::spell_item_enchantment_schema(),
        "SpellMechanic" => spell_mechanic::schema(),
        "SpellRadius" => spells::radius::schema(),
        "SpellRange" => spells::ranges::schema(),
        "SpellShapeshiftForm" => spells::forms::schema(),
        "SpellVisual" => spell_visual::spell_visual_schema(),
        "SpellVisualEffectName" => spell_visual::effect_name_schema(),
        "SpellVisualKit" => spell_visual::kit_schema(),
        "StableSlotPrices" => stable_slot_prices::schema(),
        "Stationery" => stationery::schema(),
        "SummonProperties" => summon_properties::summon_properties_schema(),
        "Talent" => talents::talent_schema(),
        "TalentTab" => talents::talent_tab_schema(),
        "TaxiNodes" => taxi_nodes::taxi_nodes_schema(),
        "TaxiPath" => taxi_path::taxi_path_schema(),
        "TaxiPathNode" => taxi::taxi_path_node_schema(),
        "TeamContributionPoints" => quest_rewards::honor_schema(),
        "TerrainType" => footsteps::terrain_type_schema(),
        "TerrainTypeSounds" => death_thud::terrain_type_sounds_schema(),
        "SpellRuneCost" => spell_rune_cost::spell_rune_cost_schema(),
        "TotemCategory" => totem_category::totem_category_schema(),
        "TransportAnimation" => elevators::transport_animation_schema(),
        "UnitBlood" => unit_blood::unit_blood_schema(),
        "UnitBloodLevels" => unit_blood::unit_blood_levels_schema(),
        "VocalUISounds" => vocal_ui_sounds::schema(),
        "WeaponImpactSounds" => weapon_impact::schema(),
        "WeaponSwingSounds2" => weapon_swing::schema(),
        "Weather" => weather::schema(),
        "WMOAreaTable" => wmo_area::schema(),
        "WorldMapArea" => world_map_area::schema(),
        "WorldMapContinent" => world_map_continent::schema(),
        "WorldMapOverlay" => world_map_overlay::schema(),
        "WorldMapTransforms" => world_map_transforms::schema(),
        "WorldStateUI" => world_state_ui::schema(),
        "ZoneIntroMusicTable" => area_sound::intro_schema(),
        "ZoneMusic" => area_sound::zone_music_schema(),
    }
    packed {
        "CharBaseInfo" => characters::CHAR_BASE_INFO_WIDTH,
        "CharStartOutfit" => (LAYOUTS.char_start_outfit.fields, LAYOUTS.char_start_outfit.record_size),
    }
}
