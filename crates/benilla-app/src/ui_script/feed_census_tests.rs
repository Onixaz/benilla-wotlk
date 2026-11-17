//! The tracker of #1245: every feed still pushing the UI model or firing its events by frame.
//! 12340's verbs read live state and its events fire from the packet handlers, so each feed
//! migrated deletes itself and its line in [`FEEDS`]; the list only shrinks.

use super::UiScript;

/// The feeds left: each non-exclusive system in [`super::UiFeed`] (or a set inside it) that holds
/// the VM, by its system name. Sorted.
const FEEDS: &[&str] = &[
    "benilla_app::combat_log::feed_combat_log",
    "benilla_app::combat_log::log_cast_failures",
    "benilla_app::combat_log::log_deaths",
    "benilla_app::combat_log::log_missile_misses",
    "benilla_app::minimap::blips::drive_blip_tooltip",
    "benilla_app::minimap::feed_game_time",
    "benilla_app::minimap::feed_minimap_compass",
    "benilla_app::minimap::feed_minimap_inside",
    "benilla_app::minimap::feed_minimap_player_facing",
    "benilla_app::minimap::ping::drive_minimap_ping",
    "benilla_app::screenshot::report_captures",
    "benilla_app::spell::targeting::feed_targeting_to_vm",
    "benilla_app::tutorial::feed_tutorials",
    "benilla_app::ui_achievement::feed_achievements",
    "benilla_app::ui_action::feed::feed_actions",
    "benilla_app::ui_action::state::feed_action_state",
    "benilla_app::ui_action::state::fire_combat_edges",
    "benilla_app::ui_auction::feed_auction",
    "benilla_app::ui_aura::feed_auras",
    "benilla_app::ui_bank::feed_bank",
    "benilla_app::ui_barber::feed_barber",
    "benilla_app::ui_binder::feed_binder",
    "benilla_app::ui_calendar::feed_calendar",
    "benilla_app::ui_cast::feed_cast_bar",
    "benilla_app::ui_char::feed_char",
    "benilla_app::ui_char::feed_skills",
    "benilla_app::ui_char::watch_skill_ups",
    "benilla_app::ui_chat::broadcast::feed_broadcasts",
    "benilla_app::ui_chat::channels::seed_channels",
    "benilla_app::ui_chat::feed::feed_chat",
    "benilla_app::ui_chat::feed::played_time_bridge",
    "benilla_app::ui_chat::net::feed_channel_roster_names",
    "benilla_app::ui_companion::feed_companions",
    "benilla_app::ui_currency::feed_currencies",
    "benilla_app::ui_dialog_verbs::feed_dialog_verbs",
    "benilla_app::ui_dressup::feed_dressup",
    "benilla_app::ui_focus::feed_focus",
    "benilla_app::ui_follow::feed_follow",
    "benilla_app::ui_glyph::feed_glyphs",
    "benilla_app::ui_gm_ticket::feed_gm_ticket",
    "benilla_app::ui_gossip::feed_gossip",
    "benilla_app::ui_guild::event_log::feed_guild_event_log",
    "benilla_app::ui_guild::feed::feed_guild",
    "benilla_app::ui_guild_bank::feed_guild_bank",
    "benilla_app::ui_inspect::feed_inspect",
    "benilla_app::ui_instance::feed_instance",
    "benilla_app::ui_item_text::feed_item_text",
    "benilla_app::ui_items::feed::feed_containers",
    "benilla_app::ui_items::feed::feed_item_sets",
    "benilla_app::ui_items::feed::feed_item_stats",
    "benilla_app::ui_items::feed::feed_item_tables",
    "benilla_app::ui_items::feed::feed_player_req",
    "benilla_app::ui_layout::load_layout",
    "benilla_app::ui_lfg::feed_lfg",
    "benilla_app::ui_logout::feed_logout",
    "benilla_app::ui_loot::feed_loot",
    "benilla_app::ui_loot_roll::feed_loot_rolls",
    "benilla_app::ui_macro::load_icon_catalog",
    "benilla_app::ui_macro::load_macros",
    "benilla_app::ui_macro::rebind_macro_spells",
    "benilla_app::ui_mail::feed_mail",
    "benilla_app::ui_merchant::feed_merchant",
    "benilla_app::ui_merchant::feed_repair_costs",
    "benilla_app::ui_mirror::feed_mirror_timers",
    "benilla_app::ui_models::forget_dead_vm_tiles",
    "benilla_app::ui_movie::feed_movie",
    "benilla_app::ui_net::feed_net_stats",
    "benilla_app::ui_notices::feed_notices",
    "benilla_app::ui_party::difficulty::feed_menu_gates",
    "benilla_app::ui_party::feed::feed_marks",
    "benilla_app::ui_party::feed::feed_party",
    "benilla_app::ui_pet::bar::feed_pet_bar",
    "benilla_app::ui_pet::bar::fire_pet_cooldown_events",
    "benilla_app::ui_pet::menu::feed_pet_menu",
    "benilla_app::ui_pet::possess::feed_possess_spell",
    "benilla_app::ui_pet::temp_bar::update_pet_temp_bar",
    "benilla_app::ui_pet::unit::feed_pet_unit",
    "benilla_app::ui_pet_book::feed_pet_book",
    "benilla_app::ui_pet_doll::feed_pet_doll",
    "benilla_app::ui_pet_stats::feed_pet_stats",
    "benilla_app::ui_petition::feed::feed_petition",
    "benilla_app::ui_quest::feed_quest",
    "benilla_app::ui_quest::lines::feed_quest_lines",
    "benilla_app::ui_quest_log::feed_quest_log",
    "benilla_app::ui_quest_log::feed_server_clock",
    "benilla_app::ui_quest_log::progress::announce_quest_progress",
    "benilla_app::ui_quest_log::tooltip::feed_quest_tooltips",
    "benilla_app::ui_quest_share::feed_quest_share",
    "benilla_app::ui_quest_watch::feed_quest_poi_lines",
    "benilla_app::ui_quest_watch::feed_quest_pois",
    "benilla_app::ui_quest_watch::feed_special_items",
    "benilla_app::ui_reputation::feed_reputation",
    "benilla_app::ui_rune::feed_runes",
    "benilla_app::ui_script::demo_unit_feed",
    "benilla_app::ui_script::feed_framerate",
    "benilla_app::ui_shapeshift::feed_shapeshift_bar",
    "benilla_app::ui_social::feed::feed_social",
    "benilla_app::ui_socket::feed_refunds",
    "benilla_app::ui_spell_info::feed_spell_info",
    "benilla_app::ui_spellbook::feed_spellbook",
    "benilla_app::ui_stable::feed_stable",
    "benilla_app::ui_tabard::feed_tabard",
    "benilla_app::ui_talent::feed_inspect_talents",
    "benilla_app::ui_talent::feed_talents",
    "benilla_app::ui_talent_wipe::feed_talent_wipe",
    "benilla_app::ui_taxi::feed_taxi",
    "benilla_app::ui_threat::feed_threat",
    "benilla_app::ui_title::feed_titles",
    "benilla_app::ui_tooltip::drive_mouseover_tooltip",
    "benilla_app::ui_tooltip::spell_feed::feed_spell_tooltips",
    "benilla_app::ui_totem::feed_multicast",
    "benilla_app::ui_totem::feed_totems",
    "benilla_app::ui_trade::feed_trade",
    "benilla_app::ui_tradeskill::feed_trade_skill",
    "benilla_app::ui_trainer::feed_trainer",
    "benilla_app::ui_transfer::feed_difficulty_notices",
    "benilla_app::ui_transfer::feed_transfer_aborts",
    "benilla_app::ui_unit::feed_combo_points",
    "benilla_app::ui_unit::feed_default_language",
    "benilla_app::ui_unit::feed_farsight_focus",
    "benilla_app::ui_unit::feed_known_languages",
    "benilla_app::ui_unit::feed_name_records",
    "benilla_app::ui_unit::feed_option_state",
    "benilla_app::ui_unit::feed_unit_reach",
    "benilla_app::ui_unit::feed_units",
    "benilla_app::ui_unit::fire_combat_text",
    "benilla_app::ui_unit::fire_leaving_world_on_worldport",
    "benilla_app::ui_unit::fire_unit_combat",
    "benilla_app::ui_unit::held::feed_chain_units",
    "benilla_app::ui_unit::load_exhaustion_rows",
    "benilla_app::ui_unit::taxi_benchmark",
    "benilla_app::vehicle::control::on_mover_change",
    "benilla_app::vehicle::drive_vehicle_aim",
    "benilla_app::vehicle::feed_vehicle_signals",
    "benilla_app::vehicle::feed_vehicle_view",
    "benilla_app::vehicle::send_vehicle_requests",
    "benilla_app::video::publish_display_modes",
];

/// Systems in [`super::UiFeed`] that hold the VM and stay, each with why: state 12340 does not
/// read live, or no state at all. A piece of #1245 moves a feed here only with its reason.
const KEPT: &[(&str, &str)] = &[(
    "benilla_app::death::update_corpse_distance",
    "12340 tests the corpse distance on every world-frame update (0x4fa040 -> 0x51f5c0) and fires the corpse range events from that test (0x512c40), not from a packet",
)];

/// Every system in [`super::UiFeed`] that holds the VM, off the built `Update` schedule.
fn scheduled_feeds() -> Vec<String> {
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::NodeId;
    use std::any::TypeId;
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let mut feeds = Vec::new();
    app.world_mut()
        .schedule_scope(bevy::app::Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<UiScript>())
                .expect("the VM is registered");
            let feed_set = format!("{:?}", super::UiFeed);
            let graph = schedule.graph();
            for (key, system, _) in graph.systems.iter() {
                if system.is_exclusive() {
                    continue;
                }
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                if !(access.has_resource_read(vm) || access.has_resource_write(vm)) {
                    continue;
                }
                // Its sets, walked up the hierarchy to the outermost.
                let mut in_feed = false;
                let mut stack = vec![NodeId::System(key)];
                while let Some(node) = stack.pop() {
                    for parent in graph
                        .hierarchy()
                        .graph()
                        .neighbors_directed(node, Direction::Incoming)
                    {
                        if let NodeId::Set(set) = parent {
                            if graph
                                .system_sets
                                .get(set)
                                .is_some_and(|s| format!("{s:?}") == feed_set)
                            {
                                in_feed = true;
                            }
                            stack.push(parent);
                        }
                    }
                }
                if in_feed {
                    feeds.push(system.name().to_string());
                }
            }
        });
    feeds.sort();
    feeds.dedup();
    feeds
}

/// The scheduled feeds are exactly [`FEEDS`]: a new feed fails, and so does a migrated one left on
/// the list, so each piece of #1245 strikes the feeds it removed.
#[test]
fn the_remaining_feeds_are_the_listed_ones() {
    let found = scheduled_feeds();
    let added: Vec<_> = found
        .iter()
        .filter(|f| !FEEDS.contains(&f.as_str()) && !KEPT.iter().any(|(k, _)| k == f))
        .collect();
    let gone: Vec<_> = FEEDS
        .iter()
        .chain(KEPT.iter().map(|(k, _)| k))
        .filter(|f| !found.iter().any(|g| g == *f))
        .collect();
    println!("feeds left: {}", found.len());
    assert!(
        added.is_empty() && gone.is_empty(),
        "#1245 moves the UI to live reads and handler-fired events, so no new feed may appear and \
         a removed one leaves FEEDS.\nnew (read live or fire from the handler instead):\n{}\n\
         removed (strike from FEEDS):\n{}",
        added
            .iter()
            .map(|f| format!("    \"{f}\","))
            .collect::<Vec<_>>()
            .join("\n"),
        gone.iter()
            .map(|f| format!("    \"{f}\","))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn the_feed_list_is_sorted_and_unique() {
    let mut sorted = FEEDS.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, FEEDS);
    let kept: Vec<_> = KEPT.iter().map(|(k, _)| *k).collect();
    let mut sorted = kept.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, kept);
    assert!(
        kept.iter().all(|k| !FEEDS.contains(k)),
        "a system is either left to migrate or kept, not both"
    );
}
