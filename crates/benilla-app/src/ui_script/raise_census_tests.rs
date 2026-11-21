//! The raise queue's census: every system that raises in turn ([`super::raises::InTurn`]) and every
//! raise step that fires what they queued. The lists only fall; at zero the queue, its steps and
//! the VM's turn state are deleted, and only 12340's unique signals (`0x615020`/`0x614760`) remain.

use crate::test_support::{fn_items, rel_path, rust_files, src_dir};

/// The holders that raise in turn: (file, item). A new one fails; one gone is struck from here.
const ON_THE_QUEUE: &[(&str, &str)] = &[
    ("area.rs", "feed_zone_events"),
    ("bindings.rs", "drain_binding_requests"),
    ("bindings.rs", "sync_dispatch"),
    ("capture/fixtures.rs", "seed_ui_fixture"),
    ("cinematic.rs", "feed_ui"),
    ("cinematic.rs", "send_opening_cinematic"),
    ("combat_log/mod.rs", "feed_combat_log"),
    ("cvars/mod.rs", "sync_cvars"),
    ("death/mod.rs", "update_corpse_distance"),
    ("hover_log.rs", "record"),
    ("minimap/blips/mod.rs", "drive_blip_tooltip"),
    ("minimap/mod.rs", "feed_game_time"),
    ("minimap/mod.rs", "feed_minimap_compass"),
    ("minimap/mod.rs", "feed_minimap_inside"),
    ("minimap/mod.rs", "feed_minimap_mask"),
    ("minimap/mod.rs", "feed_minimap_player_facing"),
    ("minimap/ping.rs", "drive_minimap_ping"),
    ("player/camera.rs", "latch_world_mouse"),
    ("player/camera_view.rs", "drain_view_requests"),
    ("probe_shield.rs", "drive_shield"),
    ("screenshot.rs", "ask_for_captures"),
    ("screenshot.rs", "report_captures"),
    ("sound/ui.rs", "drain_restart_request"),
    ("sound/ui.rs", "drain_ui_sounds"),
    ("sound/ui.rs", "feed_drivers"),
    ("sound/ui.rs", "play_item_gesture_sounds"),
    ("sound/zone.rs", "lua_music"),
    ("target/click.rs", "world_right_click_payload"),
    ("target/script.rs", "assist_requests"),
    ("target/script.rs", "target_by_name_requests"),
    ("tutorial.rs", "drain_tutorials"),
    ("tutorial.rs", "feed_tutorials"),
    ("ui_achievement.rs", "feed_achievements"),
    ("ui_action/cast_refusal.rs", "show_refusal_now"),
    ("ui_action/equipment_sets.rs", "drain_equipment_sets"),
    ("ui_action/equipment_sets.rs", "on_equipment_set_list"),
    ("ui_action/equipment_sets.rs", "on_equipment_set_saved"),
    ("ui_action/equipment_sets.rs", "on_equipment_set_use_result"),
    ("ui_action/errors.rs", "show_line_now"),
    ("ui_action/feed.rs", "feed_actions"),
    ("ui_arena_team.rs", "drain_arena_teams"),
    ("ui_auction/mod.rs", "drain_auction"),
    ("ui_auction/mod.rs", "feed_auction"),
    ("ui_aura.rs", "drain_aura_cancels"),
    ("ui_aura.rs", "end_session_aura_state"),
    ("ui_aura.rs", "feed_auras"),
    ("ui_battlefield.rs", "drain_battlefield"),
    ("ui_calendar.rs", "feed_calendar"),
    ("ui_char.rs", "drain_skill_abandons"),
    ("ui_char.rs", "feed_char"),
    ("ui_char.rs", "feed_skills"),
    ("ui_char.rs", "watch_skill_ups"),
    ("ui_chat/away.rs", "movement_clears_afk"),
    ("ui_chat/broadcast.rs", "feed_broadcasts"),
    ("ui_chat/channels.rs", "auto_join_zone_channels"),
    ("ui_chat/channels.rs", "end_session_channels"),
    ("ui_chat/channels.rs", "end_session_channels_on_disconnect"),
    ("ui_chat/channels.rs", "seed_channels"),
    ("ui_chat/feed.rs", "feed_chat"),
    ("ui_chat/feed.rs", "played_time_bridge"),
    ("ui_chat/idle.rs", "idle_handler"),
    ("ui_chat/input/mod.rs", "chat_arm"),
    ("ui_chat/input/mod.rs", "drain_addon_chat_sends"),
    ("ui_chat/input/mod.rs", "drain_addon_message_sends"),
    ("ui_chat/input/mod.rs", "take_chat_lines"),
    ("ui_chat/logging.rs", "sync_chat_logging"),
    ("ui_chat/net.rs", "feed_channel_roster_names"),
    ("ui_chat/net.rs", "on_channel_list"),
    ("ui_chat/net.rs", "on_channel_user_list"),
    ("ui_chat/recruitment.rs", "guild_recruitment_cascade"),
    ("ui_companion.rs", "drain_companions"),
    ("ui_companion.rs", "feed_companion_booth"),
    ("ui_duel.rs", "drain_duel"),
    ("ui_glyph.rs", "drain_glyph_removes"),
    ("ui_glyph.rs", "feed_glyphs"),
    ("ui_gm_ticket.rs", "drain_gm_ticket"),
    ("ui_gm_ticket.rs", "feed_gm_ticket"),
    ("ui_guild/feed.rs", "drain_guild"),
    ("ui_guild/feed.rs", "feed_guild"),
    ("ui_guild_bank/mod.rs", "drain_guild_bank"),
    ("ui_honor.rs", "drain_inspect_honor_requests"),
    ("ui_honor.rs", "feed_pvp_name_strings"),
    ("ui_inspect.rs", "feed_inspect"),
    ("ui_instance.rs", "drain_instance"),
    ("ui_instance.rs", "feed_instance"),
    ("ui_items/drain.rs", "bind_answer_step"),
    ("ui_items/drain.rs", "drain_bag_autostores"),
    ("ui_items/drain.rs", "drain_bind_on_use_confirms"),
    ("ui_items/drain.rs", "drain_container_autoequips"),
    ("ui_items/drain.rs", "drain_container_repairs"),
    ("ui_items/drain.rs", "move_step"),
    ("ui_items/drain.rs", "send_container_wraps"),
    ("ui_lfg.rs", "feed_lfg"),
    ("ui_logout.rs", "drain_logout"),
    ("ui_logout.rs", "feed_logout"),
    ("ui_loot/mod.rs", "drain_loot_asks"),
    ("ui_loot/mod.rs", "feed_loot"),
    ("ui_loot_roll.rs", "drain_loot_rolls"),
    ("ui_loot_roll.rs", "feed_loot_rolls"),
    ("ui_macro/mod.rs", "load_icon_catalog"),
    ("ui_macro/mod.rs", "load_macros"),
    ("ui_macro/mod.rs", "note_dirty_macros"),
    ("ui_mail/mod.rs", "drain_mail"),
    ("ui_mail/mod.rs", "feed_mail"),
    ("ui_models/mod.rs", "forget_dead_vm_tiles"),
    ("ui_models/mod.rs", "sync_tiles"),
    ("ui_notices.rs", "drain_level_grant"),
    ("ui_notices.rs", "feed_notices"),
    ("ui_notices.rs", "on_notice"),
    ("ui_party/feed.rs", "drain_party"),
    ("ui_party/feed.rs", "feed_marks"),
    ("ui_party/feed.rs", "feed_party"),
    ("ui_pet/menu.rs", "drain_pet_menu"),
    ("ui_pet/unit.rs", "feed_pet_unit"),
    ("ui_pet/unit.rs", "push_pet_token_system"),
    ("ui_pet_doll.rs", "feed_pet_doll"),
    ("ui_pet_doll.rs", "sync_pet_booth"),
    ("ui_quest/lines.rs", "accept_line"),
    ("ui_quest/lines.rs", "turn_in_lines"),
    ("ui_quest/mod.rs", "read_trivial_tracking"),
    ("ui_quest_log.rs", "drain_quest_log_abandons"),
    ("ui_quest_log.rs", "drain_quest_log_pushes"),
    ("ui_quest_log/progress.rs", "announce_notices"),
    ("ui_quest_log/progress.rs", "drain_quest_progress_sends"),
    ("ui_reputation.rs", "drain_reputation_sends"),
    ("ui_reputation.rs", "feed_reputation"),
    ("ui_script/glue_mgr.rs", "drive_glue_mgr"),
    ("ui_script/glue_scene.rs", "feed_stock_glue_preview"),
    ("ui_script/mod.rs", "feed_framerate"),
    ("ui_social/feed.rs", "drain_social"),
    ("ui_social/feed.rs", "feed_social"),
    ("ui_talent.rs", "drain_talent_learns"),
    ("ui_talent.rs", "feed_inspect_talents"),
    ("ui_talent.rs", "feed_talents"),
    ("ui_title.rs", "drain_title_sets"),
    ("ui_title.rs", "feed_titles"),
    ("ui_title.rs", "on_title_earned"),
    ("ui_tooltip/mod.rs", "drive_mouseover_tooltip"),
    ("ui_trade/mod.rs", "drain_trade"),
    ("ui_trade/mod.rs", "feed_trade"),
    ("ui_tradeskill.rs", "feed_trade_skill"),
    ("ui_transfer.rs", "feed_difficulty_notices"),
    ("ui_transfer.rs", "feed_transfer_aborts"),
    ("ui_unit.rs", "drain_action_bar_toggles"),
    ("ui_unit.rs", "drain_pvp_toggles"),
    ("ui_unit.rs", "drain_worn_display_toggles"),
    ("ui_unit.rs", "feed_combo_points"),
    ("ui_unit.rs", "feed_default_language"),
    ("ui_unit.rs", "feed_farsight_focus"),
    ("ui_unit.rs", "feed_known_languages"),
    ("ui_unit.rs", "feed_name_records"),
    ("ui_unit.rs", "feed_option_state"),
    ("ui_unit.rs", "feed_unit_reach"),
    ("ui_unit.rs", "feed_units"),
    ("ui_unit.rs", "fire_combat_text"),
    ("ui_unit.rs", "fire_leaving_world_on_worldport"),
    ("ui_unit.rs", "fire_unit_combat"),
    ("ui_unit.rs", "load_exhaustion_rows"),
    ("ui_unit.rs", "push_arena_units"),
    ("ui_unit.rs", "push_npc_unit"),
    ("ui_unit.rs", "taxi_benchmark"),
    ("ui_unit/held.rs", "feed_chain_units"),
    ("ui_world_map.rs", "dev_map_jump"),
    ("ui_world_map.rs", "feed_world_map"),
    ("vehicle.rs", "drive_vehicle_aim"),
    ("vehicle.rs", "feed_vehicle_signals"),
    ("vehicle.rs", "feed_vehicle_view"),
    ("vehicle.rs", "send_vehicle_requests"),
    ("vehicle/control.rs", "on_mover_change"),
    ("video.rs", "drain_restart_gx"),
    ("video.rs", "publish_display_modes"),
    ("vplates.rs", "drive_vplates"),
];

/// The raise steps: the five between the UI phases and each `then_fire` chain.
const RAISE_STEPS: usize = 70;

/// `source` with every inline test module cut out, to its closing brace.
fn production(source: &str) -> String {
    let test_module =
        regex::Regex::new(r"\r?\n#\[cfg\(test\)\]\r?\n(?:#\[[^\n]*\]\r?\n)*mod \w+ \{")
            .expect("a valid pattern");
    let mut text = String::new();
    let mut from = 0;
    while let Some(m) = test_module.find_at(source, from) {
        text.push_str(&source[from..m.start()]);
        let mut depth = 0usize;
        from = source.len();
        for (i, c) in source[m.end() - 1..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => continue,
            }
            if depth == 0 {
                from = m.end() + i;
                break;
            }
        }
    }
    text.push_str(&source[from..]);
    text
}

/// Whether `params` hold the raise queue's [`super::raises::InTurn`] by value: a reference is a
/// helper's, and the script appliers' sink of the same name is another type.
fn takes_in_turn(params: &str) -> bool {
    let flat: String = params.split_whitespace().collect();
    let bytes = flat.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    flat.match_indices("InTurn").any(|(at, _)| {
        let tail = &bytes[at + "InTurn".len()..];
        if (at > 0 && word(bytes[at - 1])) || tail.first().is_some_and(|&b| word(b) || b == b'(') {
            return false;
        }
        // Back over the type's path, `a::b::`, to what precedes it.
        let mut start = at;
        while start >= 2 && &flat[start - 2..start] == "::" {
            start -= 2;
            while start > 0 && word(bytes[start - 1]) {
                start -= 1;
            }
        }
        let before = if start == 0 { b',' } else { bytes[start - 1] };
        !flat[start..at].contains("script_calls") && matches!(before, b':' | b',' | b'(' | b'<')
    })
}

/// Every system that raises in turn and every raise step is listed, and nothing listed is gone.
#[test]
fn the_raise_queue_only_shrinks() {
    let mut found = std::collections::BTreeSet::new();
    let mut steps = 0;
    for file in rust_files(&src_dir()) {
        let rel = rel_path(&file);
        if rel.ends_with("tests.rs") {
            continue;
        }
        let text = production(&std::fs::read_to_string(&file).expect("readable source"));
        if rel == "ui_script/raises.rs" {
            steps += text.matches("fire_raises.after(").count();
            continue;
        }
        steps += text.matches("raises::then_fire(").count();
        if !text.contains("InTurn") {
            continue;
        }
        for f in fn_items(&text) {
            if takes_in_turn(f.params) {
                found.insert((rel.clone(), f.name.to_string()));
            }
        }
        for (at, _) in text.match_indices("#[derive(SystemParam)]") {
            let rest = &text[at..];
            let (Some(open), Some(close)) = (rest.find('{'), rest.find("\n}")) else {
                continue;
            };
            let name = rest[..open].trim();
            let name = name.rsplit(' ').next().unwrap_or(name);
            let name = name.split('<').next().unwrap_or(name);
            if takes_in_turn(&rest[open..close]) {
                found.insert((rel.clone(), name.to_string()));
            }
        }
    }
    let listed: std::collections::BTreeSet<(String, String)> = ON_THE_QUEUE
        .iter()
        .map(|&(f, i)| (f.to_string(), i.to_string()))
        .collect();
    let rows: String = found
        .iter()
        .map(|(f, i)| format!("    (\"{f}\", \"{i}\"),\n"))
        .collect();
    let new: Vec<_> = found.difference(&listed).collect();
    assert!(
        new.is_empty(),
        "new systems raise in turn; a system that raises is exclusive and fires at its step \
         (#1245): {new:?}. The tree's rows ({}):\n{rows}",
        found.len()
    );
    assert!(
        found == listed,
        "systems left the raise queue: strike them. The tree's rows ({}):\n{rows}",
        found.len()
    );
    assert!(
        steps <= RAISE_STEPS,
        "a new raise step ({steps} > {RAISE_STEPS}): a system that raises is exclusive"
    );
    assert_eq!(steps, RAISE_STEPS, "raise steps went away: lower the count");
}
