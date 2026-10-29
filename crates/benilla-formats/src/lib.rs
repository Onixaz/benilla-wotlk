//! `benilla-formats`: the schema layer over the 1.12.1 (build 5875) asset formats, the patch chain
//! ([`Chain`]) and the `benilla-extract` CLI. Data stays in raw WoW coordinates; the Bevy transform
//! lives in `benilla`.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use benilla_dbc::{DbcParser, Schema};

/// The BLP texel forms, re-exported for [`BlpMipChain::texels`].
pub use benilla_blp::BlpTexels;

mod chain;
pub use chain::{data_locale, Chain, ChainEntry};
/// Loose `.tga` art, as addon folders ship it.
mod tga;
pub use tga::tga_to_rgba;
/// Where the WoW install is; [`Chain`] opens it.
mod install;
pub use install::{
    addon_corpus, addon_corpus_candidates, candidates, drop_in_install, project_folder,
    set_drop_in_install, set_project_folder, skipped, wow_data,
};
mod characters;
pub use characters::{
    equip_blits, equip_column, equip_region_candidates, equip_tex_dir, equip_tile, forearm_dressed,
    BlitSource, CharCreateCatalog, CharSectionGrid, CharSections, CharacterGeosets, CompositePlan,
    CreateRaceRow, EmblemLayer, EquipBlit, EquipGeosets, GuildEmblem, NameGenRow, StartOutfitItem,
    VisibleGeosets,
};
mod camera_shakes;
mod cinematics;
mod creatures;
mod dbc;
mod dbc_layout;
mod dbc_tables;
pub use dbc_tables::{dbc_schema, dbc_width, READ_TABLES};
mod game_tables;
pub use game_tables::{load_game_tables, GameTableId, GameTables};
mod unit_blood;
pub use camera_shakes::{load_camera_shakes, CameraShake, CameraShakeCatalog, SpellShakeGroup};
pub use cinematics::{
    camera_model_path, load_cinematics, CinematicCameraRow, CinematicCatalog, CinematicPath,
    CinematicSequence, CinematicView, SEQUENCE_CAMERAS,
};
pub use creatures::{
    load_creature_catalog, CreatureCatalog, CreatureModel, FootprintParams, NpcAppearance,
};
pub use macro_icons::{load_macro_icons, load_macro_item_icons};
mod macro_icons;
pub use unit_blood::{load_blood_catalog, BloodCatalog};
mod itemsets;
pub use itemsets::{load_item_sets, ItemSetCatalog, ItemSetInfo};
mod gm_ticket_category;
pub use gm_ticket_category::{
    load_gm_ticket_categories, GmTicketCategory, GmTicketCategoryCatalog,
};
mod summon_properties;
mod vehicle;
pub use summon_properties::{
    load_summon_properties, SummonPropertiesCatalog, SUMMON_CONTROL_POSSESSED,
};
pub use vehicle::{
    load_vehicles, IndicatorSeat, OverrideSpellsRec, VehicleCatalog, VehicleRec, VehicleSeatRec,
    VehicleUiIndicator, OVERRIDE_SPELLS, VEHICLE_SEATS,
};
mod glyphs;
pub use glyphs::{load_glyph_catalog, GlyphCatalog, GlyphProperty, GlyphSlot};
mod spell_rune_cost;
mod totem_category;
pub use spell_rune_cost::{load_spell_rune_costs, SpellRuneCost, SpellRuneCostCatalog};
pub use totem_category::{load_totem_categories, TotemCategory, TotemCategoryCatalog};
mod cfg_categories;
pub use cfg_categories::{load_realm_categories, RealmCategory};
mod wow_ini;
pub use wow_ini::client_region;
mod itembagfamily;
pub use itembagfamily::{load_item_bag_families, ItemBagFamilyCatalog};
mod itemclass;
pub use itemclass::{load_item_classes, ItemClassCatalog};
mod auction_house;
pub use auction_house::{load_auction_houses, AuctionHouseCatalog, AuctionHouseInfo};
mod battlemaster_list;
pub use battlemaster_list::{
    load_battlemaster_catalog, BattlemasterCatalog, BattlemasterRec, PvpBracket,
};
mod itemsubclass;
pub use itemsubclass::{load_item_sub_classes, ItemSubClassCatalog, ItemSubClassInfo};
mod factions;
pub use factions::{
    faction_flags, load_faction_catalog, reputation_rank, FactionCatalog, FactionInfo,
    FactionTemplate, Reaction,
};
mod creature_types;
pub use creature_types::{load_creature_type_flags, CreatureTypeFlags};
/// The chat language scramble, the reference's `0x49b560`.
mod garble;
pub use garble::{garble, garble_chat, Garble, FLUENT_SKILL};
mod languages;
pub use languages::{
    load_default_languages, load_language_words, load_languages, DefaultLanguages, LanguagePool,
    LanguageWords, Languages,
};
mod creature_families;
pub use creature_families::{
    load_creature_families, load_pet_food_names, CreatureFamilies, CreatureFamily, PetFoodNames,
};
mod gameobjects;
pub use gameobjects::{
    go_sound_slot, load_gameobject_catalog, load_gameobject_sounds, GameObjectCatalog,
    GameObjectSounds,
};
mod durability;
pub use durability::{load_durability_tables, DurabilityTables};
mod exhaustion;
pub use exhaustion::{load_exhaustion, ExhaustionRow};
mod bank_bag_slot_prices;
mod currency;
pub use bank_bag_slot_prices::{load_bank_bag_slot_prices, BankBagSlotPrices};
pub use currency::{load_currency_tables, CurrencyCategory, CurrencyTables, CurrencyType};
mod item_extended_cost;
pub use item_extended_cost::{
    load_item_extended_costs, ExtendedCostRow, ItemExtendedCosts, EXTENDED_COST_ITEMS,
};
mod stable_slot_prices;
pub use stable_slot_prices::{load_stable_slot_prices, StableSlotPrices};
mod page_text_material;
pub use page_text_material::{load_page_text_material_catalog, PageTextMaterialCatalog};
mod packages;
pub use packages::{load_packages, PackageRow};
mod stationery;
pub use stationery::{
    load_stationery_catalog, StationeryCatalog, StationeryRow, STATIONERY_DEFAULT,
};
mod lock;
pub use lock::{
    load_lock_catalog, LockCatalog, LockSlot, GO_STATE_ACTIVE, GO_STATE_ACTIVE_ALTERNATIVE,
    GO_STATE_READY, LOCK_KEY_ITEM, LOCK_KEY_NONE, LOCK_KEY_SKILL, MAX_LOCK_SLOTS,
};
mod lock_type;
pub use lock_type::{load_lock_type_catalog, LockTypeCatalog};
mod chr_classes;
pub use chr_classes::{load_chr_classes, ChrClassNames, ChrClasses, PET_NAME_TOKEN_FALLBACK};
mod pet_stats;
pub use pet_stats::{
    load_pet_personalities, PetHappiness, PetPersonalities, PetPersonality, FALLBACK_PERSONALITY,
};
mod items;
pub use items::{load_item_display_catalog, ItemDisplay, ItemDisplayCatalog};
mod item_sounds;
pub use item_sounds::{load_item_group_sounds, ItemGesture, ItemGroupSoundsCatalog};
mod item_visuals;
pub use item_visuals::{
    load_enchant_catalog, load_item_visual_catalog, EnchantCatalog, ItemVisualCatalog,
    ITEM_VISUAL_SLOTS,
};
mod item_random_properties;
pub use item_random_properties::{
    load_random_property_catalog, RandomProperty, RandomPropertyCatalog,
    RANDOM_PROPERTY_FIRST_SLOT, RANDOM_PROPERTY_SLOTS,
};
mod ground_effects;
pub use ground_effects::{
    load_ground_effect_catalog, scatter_ground_doodads, GroundDoodadPlacement, GroundEffect,
    GroundEffectCatalog, FRILL_DENSITY, FRILL_DENSITY_MAX,
};
mod light;
pub use light::{Atmosphere, LightCatalog, Submersion, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};
mod loading_screen;
pub use loading_screen::{load_loading_screens, LoadingScreenCatalog};
mod liquid;
pub use liquid::{mclq_liquid_type, LiquidKind, LiquidMesh};
mod liquid_type;
pub use liquid_type::{load_liquid_types, DepthRamp, LiquidTypeRow, LiquidTypes, VertexFormat};
mod maps;
pub use maps::{load_map_catalog, MapCatalog};
mod anim_data;
pub use anim_data::{load_anim_data_catalog, AnimDataCatalog, AnimEntry};
mod quest_headers;
pub use quest_headers::{load_quest_header_names, QuestHeaderNames};
mod quest_info;
pub use quest_info::{load_quest_tag_names, QuestTagNames};
mod quest_rewards;
pub use quest_rewards::{load_quest_reward_tables, CharTitle, QuestRewardTables};
mod area_trigger;
pub use area_trigger::{load_area_trigger_catalog, AreaTriggerCatalog, AreaTriggerRow};
mod area_sound;
pub use area_sound::{
    load_area_sound_catalog, AreaAudio, AreaEntry, AreaSoundCatalog, SoundAmbienceEntry,
    ZoneIntroEntry, ZoneMusicEntry,
};
mod creature_sound;
pub use creature_sound::{load_creature_voice_catalog, CreatureVoice, CreatureVoiceCatalog};

mod npc_greeting;
pub use npc_greeting::{load_npc_greeting_catalog, NpcGreeting, NpcGreetingCatalog};
mod emotes;
pub use emotes::{load_emote_sound_catalog, EmoteSoundCatalog, EMOTE_FLAG_INTERACTION};
mod emote_text;
pub use emote_text::{load_emote_text_catalog, EmoteLine, EmoteTextCatalog};
mod environmental_damage;
pub use environmental_damage::{load_environmental_damage, EnvironmentalDamageTable};
mod footsteps;
pub use footsteps::{load_footprint_textures, load_footstep_catalog, FootstepCatalog};
mod death_thud;
pub use death_thud::{load_death_thud_catalog, DeathThudCatalog};
mod sound_entries;
pub use sound_entries::{load_sound_kit_catalog, sound_kit_flags, SoundKit, SoundKitCatalog};
mod sound_provider;
pub use sound_provider::{load_sound_provider_catalog, SoundProvider, SoundProviderCatalog};
mod weather;
pub use weather::{load_weather_catalog, WeatherCatalog, WeatherRow};
mod vocal_ui_sounds;
pub use vocal_ui_sounds::{
    load_vocal_ui_sounds, VocalUiSound, VocalUiSoundCatalog, VOCAL_UI_LINES,
};
mod weapon_impact;
pub use weapon_impact::{
    impact_slot, load_weapon_impact_catalog, WeaponImpactCatalog, WeaponImpactRow,
};
mod weapon_swing;
pub use weapon_swing::{load_weapon_swing_catalog, WeaponSwingCatalog};
mod material;
mod sound_filter;
pub use material::{load_material_catalog, MaterialCatalog};
pub use sound_filter::{
    filter_type, load_sound_filter_catalog, SoundFilterCatalog, SoundFilterElem,
};
mod wmo_area;
pub use wmo_area::{load_wmo_area_catalog, WmoArea, WmoAreaCatalog};
mod spells;
pub use spells::{
    cc_exemption, grants_immunity, load_shapeshift_forms, load_spell_cast_times,
    load_spell_catalog, load_spell_description_variables, load_spell_dispel_types,
    load_spell_durations, load_spell_radii, load_spell_ranges, min_max_range,
    min_max_range_reads_units, soft_modify, substitute, Caster, CcExemption, CompanionKind,
    FormRefusal, LearnAnnouncement, LearnEffect, OpenLock, PlayerValue, RangeTargets, RangeUnit,
    ShapeshiftForm, SpellCastTime, SpellCastTimeCatalog, SpellCatalog,
    SpellDescriptionVariablesCatalog, SpellDispelTypes, SpellDisplay, SpellDuration,
    SpellDurationCatalog, SpellMods, SpellRadius, SpellRadiusCatalog, SpellRange,
    SpellRangeCatalog, TokenContext, TokenNumber, UnitMotion, ATTR_CASTABLE_WHILE_DEAD,
    ATTR_NOT_IN_COMBAT, ATTR_ONLY_STEALTHED, COMBAT_REACH_ADD, MELEE_RANGE_FLOOR,
    MOVING_BONUS_FLAGS, MOVING_RANGE_BONUS, ON_NEXT_SWING_RANGE, SPELL_ATTR_IS_TRADESKILL,
    SPELL_AURA_MOUNTED, SPELL_EFFECT_APPLY_GLYPH, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_ENCHANT_ITEM, SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, SPELL_EFFECT_LEARN_PET_SPELL,
    SPELL_EFFECT_LEARN_SPELL, SPELL_EFFECT_PROSPECTING, SPELL_EFFECT_SKILL_STEP,
    SPELL_EFFECT_SKINNING, SPELL_EFFECT_TRADE_SKILL,
};
mod skill_lines;
pub use skill_lines::{
    load_skill_line_catalog, LinkAbility, SkillLineCatalog, SkillLineInfo, SkillRaceClass, SlaInfo,
};
mod spell_focus;
mod spell_mechanic;
pub use spell_focus::{load_spell_focus_catalog, SpellFocusCatalog};
pub use spell_mechanic::{load_spell_mechanic_catalog, SpellMechanicCatalog};
mod achievements;
pub use achievements::{
    load_achievement_catalog, Achievement, AchievementCatalog, AchievementCategory,
    AchievementCriteria,
};
mod talents;
pub use talents::{load_talent_catalog, Talent, TalentCatalog, TalentTabInfo, MAX_TALENT_RANK};
mod spell_visual;
pub use spell_visual::{
    char_proc_small_int, char_proc_type, load_spell_visual_catalog, ChainEffect, ChainProc,
    CharProc, SpellVisualCatalog, TrailProc, VisualKit, VisualStages, KIT_CHAR_PROCS,
    KIT_SLOT_TAGS, MISSILE_ATTACH_TABLE, WORLD_EFFECT_TAG,
};
mod emit_timing;
pub use emit_timing::{EmitParams, EmitTiming, ParamsNow};
mod particles;
pub use particles::{
    load_particle_colors, parse_m2_particle_emitters, LifeTrack, OverLife, OverLifeSample,
    ParticleBlend, ParticleColorTable, ParticleColors, ParticleEmitterDef, ParticleShape,
    SplineData,
};
mod ribbons;
pub use ribbons::{parse_m2_ribbon_emitters, RibbonEmitterDef, RibbonVisibility};
mod value_track;
pub use value_track::{TrackValue, ValueTrack};
mod models;
pub use models::{
    accumulate_wmo_group_camera_collision, accumulate_wmo_group_camera_only_collision,
    accumulate_wmo_group_collision, anim_paths, authored_half_height, batch_footprint,
    glue_art_extent, hand_grip_finger_poses, load_m2_animation_summary, load_m2_bone_spins,
    load_m2_bounds, load_m2_collision_hull, load_m2_mesh, load_m2_mesh_skinned, load_object_model,
    load_wmo, load_wmo_collision_tris, m2_bone_spins, m2_owner_reach, m2_ribbon_emitter_count,
    m2_sequence_visible_textures, m2_texture_transform_count, md20_version,
    non_separable_billboard_bones, owner_last_rung, owner_last_rung_bucket, parse_m2,
    parse_m2_animation_lookup, parse_m2_animation_summary, parse_m2_animations,
    parse_m2_attachments, parse_m2_bounds, parse_m2_camera, parse_m2_cch_marker,
    parse_m2_collision_hull, parse_m2_event_markers, parse_m2_global_sequence_bones,
    parse_m2_lights, parse_m2_pane_cameras, parse_m2_portrait_camera, parse_m2_render_submeshes,
    parse_m2_skeleton, parse_m2_string_anchors, parse_wmo_fogs, parse_wmo_lights,
    parse_wmo_portals, parse_wmo_root, read_m2, rotation_2x2, shipped_glue_art_extent, skin_path,
    uv_transform, wmo_group_doodad_refs, wmo_group_fixed_colors, wmo_group_footprint_tris,
    wmo_group_has_liquid_grid, wmo_group_header, wmo_group_light_refs, wmo_group_liquid_mesh,
    wmo_group_raw_colors, wmo_group_submeshes, wmo_root_id, AlphaAnim, AlphaMap, AlphaSeq,
    AnimEvent, AnimFiles, ArtExtent, BatchFootprint, BatchSort, Billboard, BillboardKind, BoneKeys,
    BoneScaleAnim, BoneSpin, CharSkinSlot, CollisionMesh, Combiner, CombinerOp, CombinerShader,
    Coverage, CoverageReader, EmitterBoneLink, EventMarker, FogPolicy, FootprintTris,
    GlobalSeqBone, GlobalSeqChannel, GroundQuad, KeyAnim, M2AnimSummary, M2Attachment, M2Bounds,
    M2CameraTracks, M2Light, M2Model, M2PaneCamera, M2PortraitCamera, ModelAnimation, ModelBlend,
    ParentArm, ParentBasis, PlayableAnim, RenderSubmesh, RgbAnim, ScalarAnim, SeqLoops,
    ShippedGlueScene, Skeleton, SkeletonBone, StageUv, StringAnchors, UvAnim, UvRotAnim,
    WmoBatchClass, WmoDoodad, WmoDoodadSet, WmoFog, WmoGroupHeader, WmoGroupInfo, WmoLight,
    WmoLightLaw, WmoLiquidRule, WmoPortalInfo, WmoPortalRef, WmoPortals, WmoRoot, ALPHA_KEY_REF,
    DEGENERATE_RING_FOOTPRINT, GLUE_AUTHORED_ASPECT, NO_GROUP_LIQUID, OWNER_RUNG_BUCKETS,
    SHIPPED_GLUE_SCENES, VERSION_264,
};
mod terrain;
pub use terrain::{
    adt_to_tile_mesh, area_id_at, find_tile_near, ground_effect_at, impassable_at, load_tile_mesh,
    load_tile_mesh_with, load_tiles_around, mcsh_shadowed_at, terrain_height_at, triangle_z_at,
    ChunkMesh, Doodad, MapTiles, TerrainLiquid, TileMesh, WmoInstance, ALPHA_MAP_SIZE, CHUNK_SIZE,
    SHADOW_MAP_SIZE, STORMWIND_XY, TERRAIN_LAYER_TILES, TILE_SIZE,
};
mod wdl;
/// World (x, y) to ADT tile `(col, row)`; minimap `map<X>_<Y>.blp` names use the same order.
pub use benilla_wdt::{
    tile_to_world, world_to_chunk, world_to_tile, GlobalWmo, WdtFile, WdtReader, WowVersion,
};
pub use wdl::{WdlFile, WdlTileMesh};

// The minimap tile hash catalog and the world-map DBCs.
mod minimap_translate;
pub use minimap_translate::{load_minimap_translate, MinimapTranslate};
mod world_map_area;
pub use world_map_area::{load_world_map_area_catalog, WorldMapArea, WorldMapAreaCatalog};
mod world_map_overlay;
pub use world_map_overlay::{
    load_world_map_overlay_catalog, WorldMapOverlay, WorldMapOverlayCatalog,
};
mod world_map_transforms;
pub use world_map_transforms::{
    load_world_map_transform_catalog, WorldMapTransform, WorldMapTransformCatalog,
};
mod dungeon_map;
pub use dungeon_map::{load_dungeon_map_catalog, DungeonMap, DungeonMapCatalog};
mod dungeon_encounter;
pub use dungeon_encounter::{load_dungeon_encounters, DungeonEncounter};
mod lfg_dungeons;
pub use lfg_dungeons::{
    load_lfg_catalog, LfgCatalog, LfgDungeon, LfgDungeonExpansion, LfgDungeonGroup,
};
mod holidays;
pub use holidays::{load_holidays, Holiday};
mod map_difficulty;
pub use map_difficulty::{find_map_difficulty, load_map_difficulties, MapDifficulty};
mod world_map_continent;
pub use world_map_continent::{
    load_world_map_continent_catalog, WorldMapContinent, WorldMapContinentCatalog,
};
mod area_poi;
pub use area_poi::{load_area_poi_catalog, AreaPoi, AreaPoiCatalog};
mod world_state_ui;
pub use world_state_ui::{load_world_state_ui_catalog, WorldStateUiCatalog, WorldStateUiRow};
mod area_table;
pub use area_table::{load_area_table_catalog, AreaTableCatalog, AreaTableRow};
mod chat_channels;
pub use chat_channels::{
    flags as chat_channel_flags, load_chat_channels_catalog, ChatChannelRow, ChatChannelsCatalog,
};
mod server_messages;
pub use server_messages::{load_server_messages_catalog, ServerMessagesCatalog};
mod game_tips;
pub use game_tips::{load_game_tips, GameTipsCatalog};
mod barber_shop_style;
pub use barber_shop_style::{load_barber_shop_styles, BarberShopStyle};
mod text_filter_lists;
pub use text_filter_lists::{load_chat_profanity, load_spam_messages, FilterPattern};
mod race_sound;
pub use race_sound::{load_exploration_sound_catalog, ExplorationSoundCatalog};

mod race_creature_type;
pub use race_creature_type::load_race_creature_types;
mod race_pvp_team;
pub use race_pvp_team::load_race_pvp_teams;
mod zone_map;
pub use zone_map::{load_zone_map, ZONE_MAP_EDGE};

// Taxi and transport paths, and the transport timetable.
mod taxi;
pub use taxi::{load_taxi_path_nodes, TaxiPathNode, TaxiPathNodes};
mod transport_period;
mod transports;
pub use transports::{TransportSample, TransportTimetable};
// Type-11 elevator and lift keyframe paths.
mod elevators;
pub use elevators::{
    elevator_period_ms, elevator_sample, load_elevator_paths, ElevatorKeyframe, ElevatorPaths,
};

// The flight-master catalogs: nodes (`TaxiNodes.dbc`) and direct-hop fares (`TaxiPath.dbc`).
// `taxi` above holds a path's waypoints.
mod taxi_nodes;
pub use taxi_nodes::{load_taxi_nodes, TaxiNode, TaxiNodes};
mod taxi_path;
pub use taxi_path::{load_taxi_paths, TaxiPath, TaxiPaths};

/// [`Chain::open`] on one `.MPQ` file or the active client's `Data` directory.
pub fn open_chain(path: &Path) -> Result<Chain> {
    Chain::open(path)
}

/// Write a BLP2 texture's level 0 (1.12 ships no BLP1) as a PNG at `out`; returns its size.
pub fn blp_to_png(blp_bytes: &[u8], out: &Path) -> Result<(u32, u32)> {
    let (w, h, rgba) = blp_to_rgba(blp_bytes)?;
    image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| anyhow::anyhow!("BLP RGBA buffer size mismatch"))?
        .save(out)
        .with_context(|| format!("writing PNG {}", out.display()))?;
    Ok((w, h))
}

/// One authored mip level's texel census, split by alpha: the multiply blends (Mod2x,
/// `2·src·dst`) read no alpha, so a transparent texel's colour still modulates the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct BlpMipStats {
    pub level: u32,
    pub width: u32,
    pub height: u32,
    /// Texels with alpha 0 (the authored "outside") and with alpha > 0 (the "inside").
    pub outside: usize,
    pub inside: usize,
    /// `(min, mean, max)` luma, `(r+g+b)/3`, over the outside and the inside texels.
    pub outside_luma: Option<(u8, f32, u8)>,
    pub inside_luma: Option<(u8, f32, u8)>,
    /// Texels with luma below 128, each of which darkens the scene under Mod2x.
    pub below_128: usize,
}

/// [`BlpMipStats`] for every authored level, decoded as CPU consumers decode: a neutral level 0
/// can hide dark tail levels, which are all a minified sprite samples.
pub fn blp_mip_stats(blp_bytes: &[u8]) -> Result<Vec<BlpMipStats>> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    Ok(decoded
        .mips
        .iter()
        .enumerate()
        .map(|(level, m)| {
            let mut outside = (0usize, u8::MAX, 0u64, u8::MIN);
            let mut inside = (0usize, u8::MAX, 0u64, u8::MIN);
            let mut below_128 = 0usize;
            for px in m.rgba.as_chunks::<4>().0 {
                let luma = ((px[0] as u32 + px[1] as u32 + px[2] as u32) / 3) as u8;
                if luma < 128 {
                    below_128 += 1;
                }
                let acc = if px[3] == 0 {
                    &mut outside
                } else {
                    &mut inside
                };
                acc.0 += 1;
                acc.1 = acc.1.min(luma);
                acc.2 += luma as u64;
                acc.3 = acc.3.max(luma);
            }
            let fold = |(n, lo, sum, hi): (usize, u8, u64, u8)| {
                (n > 0).then(|| (lo, sum as f32 / n as f32, hi))
            };
            BlpMipStats {
                level: level as u32,
                width: m.width,
                height: m.height,
                outside: outside.0,
                inside: inside.0,
                outside_luma: fold(outside),
                inside_luma: fold(inside),
                below_128,
            }
        })
        .collect())
}

/// Write every authored level as `<stem>.mip<N>.png` beside `out`, level 0 at `out` itself.
pub fn blp_mips_to_png(blp_bytes: &[u8], out: &Path) -> Result<Vec<BlpMipStats>> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    let stem = out.with_extension("");
    for (level, m) in decoded.mips.iter().enumerate() {
        let path = if level == 0 {
            out.to_path_buf()
        } else {
            let mut p = stem.as_os_str().to_owned();
            p.push(format!(".mip{level}.png"));
            std::path::PathBuf::from(p)
        };
        image::RgbaImage::from_raw(m.width, m.height, m.rgba.clone())
            .ok_or_else(|| anyhow::anyhow!("BLP RGBA buffer size mismatch at level {level}"))?
            .save(&path)
            .with_context(|| format!("writing PNG {}", path.display()))?;
    }
    blp_mip_stats(blp_bytes)
}

/// Decode a BLP's level 0 to RGBA8: `(width, height, pixels)`.
pub fn blp_to_rgba(blp_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    let level0 = decoded
        .mips
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("BLP has no level 0"))?;
    Ok((level0.width, level0.height, level0.rgba))
}

/// Read a texture from the chain by internal path (accepts `/` or `\`) and decode to RGBA8.
pub fn read_texture_rgba(chain: &mut Chain, path: &str) -> Result<(u32, u32, Vec<u8>)> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_to_rgba(&bytes)
}

/// A BLP's authored mip levels, level 0 first, never empty: a no-mipmaps BLP is one level.
/// [`texels`](Self::texels) says whether they hold pixels or DXTC blocks.
#[derive(Debug, Clone)]
pub struct BlpMipChain {
    /// Level 0's dimensions.
    pub width: u32,
    pub height: u32,
    /// What every level holds: decoded pixels or S3TC blocks.
    pub texels: BlpTexels,
    /// One buffer per authored level; the 1.12 client uploads the BLP's stored levels and never
    /// regenerates them.
    pub mips: Vec<Vec<u8>>,
}

impl BlpMipChain {
    /// Drop leading levels until level 0 is at most `side` on both axes, keeping one level.
    pub fn trim_to(&mut self, side: u32) {
        while self.mips.len() > 1 && (self.width > side || self.height > side) {
            self.mips.remove(0);
            self.width = (self.width >> 1).max(1);
            self.height = (self.height >> 1).max(1);
        }
    }

    /// Level `i`'s size, at least 1×1.
    pub fn mip_size(&self, i: u32) -> (u32, u32) {
        ((self.width >> i).max(1), (self.height >> i).max(1))
    }

    /// Whether the levels are `Rgba8Unorm` pixels a CPU reader can index, not blocks.
    pub fn is_rgba8(&self) -> bool {
        !self.texels.is_block_compressed()
    }

    /// Every level decoded to `Rgba8Unorm` without rereading the file; a no-op if already so.
    pub fn into_rgba8(self) -> Self {
        if self.is_rgba8() {
            return self;
        }
        let mips = self
            .mips
            .iter()
            .enumerate()
            .map(|(i, level)| {
                let (w, h) = self.mip_size(i as u32);
                benilla_blp::decode_level(self.texels, w, h, level)
            })
            .collect();
        Self {
            texels: BlpTexels::Rgba8Unorm,
            mips,
            ..self
        }
    }
}

/// Read a texture from the chain and decode every authored level to `Rgba8Unorm`.
pub fn read_texture_mip_chain(chain: &mut Chain, path: &str) -> Result<BlpMipChain> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_bytes_to_mip_chain(&bytes).with_context(|| format!("decoding texture '{name}'"))
}

/// Read a texture from the chain, keeping its DXTC blocks ([`blp_bytes_to_native_chain`]).
pub fn read_texture_native_chain(chain: &mut Chain, path: &str) -> Result<BlpMipChain> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_bytes_to_native_chain(&bytes).with_context(|| format!("decoding texture '{name}'"))
}

/// Decode every authored level of an in-memory BLP to `Rgba8Unorm`, as stored.
pub fn blp_bytes_to_mip_chain(bytes: &[u8]) -> Result<BlpMipChain> {
    let decoded = benilla_blp::decode(bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    // A no-mipmaps BLP counts 0 levels, yet `decode` always fills `mips[0]`: take at least one.
    let (width, height, count) = (
        decoded.width,
        decoded.height,
        decoded.mip_chain_count().max(1),
    );
    let mips: Vec<Vec<u8>> = decoded
        .mips
        .into_iter()
        .take(count)
        .map(|m| m.rgba)
        .collect();
    Ok(BlpMipChain {
        width,
        height,
        texels: BlpTexels::Rgba8Unorm,
        mips,
    })
}

/// Every authored level of an in-memory BLP with its DXTC blocks kept, as the reference uploads
/// them (`glCompressedTexImage2DARB`, `0x59f5b0`). Raw1 and Raw3 come back decoded; levels are
/// padded to whole blocks.
pub fn blp_bytes_to_native_chain(bytes: &[u8]) -> Result<BlpMipChain> {
    let decoded =
        benilla_blp::decode_native(bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    // At least one level, as in `blp_bytes_to_mip_chain`.
    let (width, height, count) = (
        decoded.width,
        decoded.height,
        decoded.mip_chain_count().max(1),
    );
    let texels = decoded.texels;
    let mips: Vec<Vec<u8>> = decoded
        .mips
        .into_iter()
        .take(count)
        .map(|m| m.bytes)
        .collect();
    Ok(BlpMipChain {
        width,
        height,
        texels,
        mips,
    })
}

/// Our schema for a DBC by file name, the loader's own ([`dbc_schema`]): a table's stem, matched
/// case-insensitively.
fn schema_for(dbc_name: &str) -> Option<Schema> {
    let base = dbc_name.rsplit(['/', '\\']).next().unwrap_or(dbc_name);
    let stem = base.strip_suffix(".dbc").unwrap_or(base);
    let table = READ_TABLES.iter().find(|t| t.eq_ignore_ascii_case(stem))?;
    dbc_schema(table)
}

/// Write a DBC as CSV through our schema for it, returning `(record_count, field_count)`; an error
/// when there is no schema or its field count is wrong.
pub fn dbc_to_csv(dbc_bytes: &[u8], dbc_name: &str, out: &Path) -> Result<(u32, u32)> {
    let mut cursor = Cursor::new(dbc_bytes);
    let parser =
        DbcParser::parse(&mut cursor).map_err(|e| anyhow::anyhow!("parsing DBC header: {e}"))?;
    let (records, fields) = (parser.header().record_count, parser.header().field_count);

    let schema = schema_for(dbc_name).ok_or_else(|| {
        anyhow::anyhow!("no schema for '{dbc_name}' ({fields} fields): benilla does not read it")
    })?;

    let parser = parser
        .with_schema(schema)
        .map_err(|e| anyhow::anyhow!("applying schema (field count mismatch?): {e}"))?;
    let record_set = parser
        .parse_records()
        .map_err(|e| anyhow::anyhow!("parsing DBC records: {e}"))?;

    let file = std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
    benilla_dbc::export_to_csv(&record_set, std::io::BufWriter::new(file))
        .with_context(|| format!("writing CSV {}", out.display()))?;

    Ok((records, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete 148-byte BLP2 header, magic through `mip_sizes[16]`.
    fn blp2_header(
        compression: u8,
        alpha_bits: u8,
        alpha_type: u8,
        has_mipmaps: u8,
        width: u32,
        height: u32,
        offsets: [u32; 16],
        sizes: [u32; 16],
    ) -> Vec<u8> {
        const HEADER_SIZE: usize = 148;
        let mut b = vec![0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(b"BLP2");
        b[4..8].copy_from_slice(&1u32.to_le_bytes()); // content: 1 = Direct
        b[8] = compression;
        b[9] = alpha_bits;
        b[10] = alpha_type;
        b[11] = has_mipmaps;
        b[12..16].copy_from_slice(&width.to_le_bytes());
        b[16..20].copy_from_slice(&height.to_le_bytes());
        for (i, o) in offsets.iter().enumerate() {
            b[20 + i * 4..24 + i * 4].copy_from_slice(&o.to_le_bytes());
        }
        for (i, s) in sizes.iter().enumerate() {
            b[84 + i * 4..88 + i * 4].copy_from_slice(&s.to_le_bytes());
        }
        b
    }

    /// `has_mipmaps = 0` counts 0 levels, and consumers index `mips[0]`.
    #[test]
    fn no_mipmap_blp_yields_a_one_level_chain_not_empty() {
        const HEADER_SIZE: usize = 148;
        const PALETTE_SIZE: usize = 256 * 4;
        let pixel_offset = (HEADER_SIZE + PALETTE_SIZE) as u32;
        let pixel_size = 2 * 2 * 4;
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = pixel_offset;
        sizes[0] = pixel_size;
        // compression 3 = Raw3 (uncompressed BGRA8); has_mipmaps 0.
        let mut b = blp2_header(3, 8, 0, 0, 2, 2, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0); // palette bytes: unused by Raw3, present anyway
        b.extend_from_slice(&[
            10, 20, 30, 40, // B G R A
            50, 60, 70, 80, //
            90, 100, 110, 120, //
            130, 140, 150, 160,
        ]);

        let chain = blp_bytes_to_mip_chain(&b).expect("valid no-mipmap BLP decodes");
        assert_eq!(chain.width, 2);
        assert_eq!(chain.height, 2);
        assert_eq!(
            chain.mips.len(),
            1,
            "no-mipmaps BLP must yield 1 level, not 0"
        );
        assert_eq!(chain.mips[0].len(), 2 * 2 * 4);
    }

    /// `RainDrop01.blp` (16×128 DXT3) stores 16 bytes for its 2×16 and 1×8 levels, which need 64
    /// and 32. The reference completes a short level from the bytes after it (`0x5a5780`); a zero
    /// fill would darken the Mod2x rain streak toward black.
    /// The Storm Peaks base snow is a single-unit file in `lichking.MPQ`: it reads at its block
    /// table size and decodes.
    #[test]
    fn a_single_unit_texture_reads_and_decodes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = open_chain(&data).expect("open chain");
        let path = "TILESET\\Expansion02\\StormPeaks\\SP_SnowA.blp";
        let bytes = chain
            .read_file(path)
            .unwrap_or_else(|e| panic!("{path}: {e:#}"));
        let native = blp_bytes_to_native_chain(&bytes).expect("decodes natively");
        assert!(native.width > 0 && native.height > 0 && !native.mips.is_empty());
    }

    #[test]
    fn the_rain_streak_texture_is_neutral_on_every_level_the_sampler_reaches() {
        let data = crate::wow_data_or_skip!();
        let mut chain = open_chain(&data).expect("open chain");
        let path = "textures\\Weather\\RainDrop01.blp";
        let bytes = chain
            .read_file(path)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let native = blp_bytes_to_native_chain(&bytes).expect("decodes natively");
        assert_eq!((native.width, native.height), (16, 128));
        assert_eq!(native.texels, benilla_blp::BlpTexels::Bc2);
        assert!(
            native.mips.len() >= 5,
            "the tail levels are what this is about"
        );
        // Level 3 (2×16) needs four blocks and stores one; its second is level 4's first.
        assert_eq!(native.mips[3].len(), 64);
        assert_eq!(
            &native.mips[3][16..32],
            &native.mips[4][..16],
            "level 3's second block must be the file's next 16 bytes — level 4's block"
        );
        assert!(
            native.mips[3][16..].iter().any(|&x| x != 0),
            "level 3's completion is never zero-filled"
        );
        // No texel on any level falls below luma 120; the authored band is 125 to 164.
        let stats = blp_mip_stats(&bytes).expect("census");
        for s in &stats {
            for (class, luma) in [("outside", s.outside_luma), ("inside", s.inside_luma)] {
                if let Some((lo, _, _)) = luma {
                    assert!(
                        lo >= 120,
                        "level {} ({}x{}) {class} texels reach luma {lo} — a Mod2x streak sampling \
                         this level darkens the scene",
                        s.level,
                        s.width,
                        s.height
                    );
                }
            }
        }
    }

    #[test]
    fn registered_dbc_schemas_dump_the_shipped_tables() {
        let data = crate::wow_data_or_skip!();
        let mut chain = open_chain(&data).expect("open chain");
        let out =
            std::env::temp_dir().join(format!("benilla-schema-reg-{}.csv", std::process::id()));
        for table in [
            "CreatureDisplayInfo",
            "CreatureDisplayInfoExtra",
            "CreatureModelData",
            "CharHairGeosets",
            "CharacterFacialHairStyles",
            "HelmetGeosetVisData",
            "CharSections",
            "ItemDisplayInfo",
            "ItemVisuals",
            "ItemVisualEffects",
            "SpellItemEnchantment",
            "ItemRandomProperties",
            "TaxiNodes",
            "AreaTable",
            "GameObjectDisplayInfo",
            "TaxiPath",
            "TaxiPathNode",
            "TransportAnimation",
        ] {
            let path = format!("DBFilesClient\\{table}.dbc");
            let bytes = chain
                .read_file(&path)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            let (rows, _) = dbc_to_csv(&bytes, &path, &out)
                .unwrap_or_else(|e| panic!("{table} schema does not fit the shipped file: {e}"));
            assert!(rows > 0, "{table} has rows");
        }
        let _ = std::fs::remove_file(&out);
    }
}
