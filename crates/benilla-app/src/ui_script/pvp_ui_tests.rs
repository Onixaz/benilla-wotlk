//! The stock 3.3.5a PvP windows on the production load, over the model the app pushes: the PvP
//! tab's honor and arena teams, the Battlegrounds tab and the queue's minimap icon and popup, the
//! arena battlemaster's window and the arena registrar.

use std::collections::HashMap;

use benilla_ui::script::{
    ArenaTeamMemberView, ArenaTeamRecordView, ArenaTeamRequest, ArenaTeamSlotView,
    BattlefieldFrame, BattlefieldJoin, BattlefieldListPush, BattlefieldQueueSlot, BattlemasterRow,
    HonorState, PetitionRequest, PetitionVendorItem, PetitionVendorState, ScriptValue, UiScript,
    UnitState,
};

/// The current arena season's world state (`GetCurrentArenaSeason`, `0x5a2a40`).
const ARENA_SEASON: u32 = 3191;

/// Errors from the PvP windows' own code; the production load carries unrelated gaps.
fn pvp_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            ["PVP", "Battle", "Arena", "Honor"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The production load with a level-80 Alliance player in season 8.
fn load() -> UiScript {
    let (mut s, _failures) = super::layer_tests::production_load_with("pvp", false, "", |_| {});
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "the PvP files' load: {errors:#?}");
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Probesix".into()),
            level: 80,
            faction_group: Some("Alliance".into()),
            race: Some("Dwarf".into()),
            race_file: Some("Dwarf".into()),
            class: Some("Hunter".into()),
            class_file: Some("HUNTER".into()),
            ..Default::default()
        }),
    );
    s.set_world_map_world_states(HashMap::from([(ARENA_SEASON, 8)]));
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.tick(0.0);
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "world entry: {errors:#?}");
    s
}

fn text(s: &UiScript, frame: &str) -> Option<String> {
    s.eval::<Option<String>>(&format!("return {frame}:GetText()"))
        .unwrap_or_else(|e| panic!("{frame}:GetText(): {e}"))
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible() and true or false"))
        .unwrap()
}

/// A 2v2 team in slot 1: two members, the player its captain.
fn two_v_two() -> ArenaTeamSlotView {
    let member = |guid, name: &str, rank, rating| ArenaTeamMemberView {
        guid,
        name: name.into(),
        rank,
        level: 80,
        class: Some("Hunter".into()),
        online: true,
        week_games: 4,
        week_wins: 3,
        season_games: 10,
        season_wins: 6,
        personal_rating: rating,
        matchmaking: None,
    };
    ArenaTeamSlotView {
        info: [77, 2, 0, 4, 10, 6, 1512],
        record: Some(ArenaTeamRecordView {
            name: "Probe Squad".into(),
            team_type: 2,
            background_color: 0xFF20_40C0,
            emblem_style: 7,
            emblem_color: 0xFFFF_FFFF,
            border_style: 3,
            border_color: 0xFF33_3333,
        }),
        stats: [1530, 4, 3, 10, 6, 112],
        members: vec![
            member(0x10, "Probesix", 0, 1512),
            member(0x11, "Probeseven", 1, 1490),
        ],
    }
}

/// The PvP tab paints the honor pane from the 3.3.5a block and the team banner from the slot.
#[test]
fn the_pvp_tab_paints_honor_and_a_two_v_two_team() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = load();
    s.set_honor(Some(HonorState {
        today_hk: 3,
        today_honor: 120,
        yesterday_hk: 7,
        yesterday_honor: 450,
        lifetime_hk: 900,
        honor_currency: 12_345,
        arena_currency: 678,
        ..Default::default()
    }));
    s.set_arena_teams([two_v_two(), Default::default(), Default::default()]);
    s.run("TogglePVPFrame()").unwrap();
    s.tick(0.0);
    assert!(shown(&s, "PVPFrame"), "the PvP tab opens first");
    for (frame, want) in [
        ("PVPHonorTodayKills", "3"),
        ("PVPHonorTodayHonor", "120"),
        ("PVPHonorYesterdayKills", "7"),
        ("PVPHonorYesterdayHonor", "450"),
        ("PVPHonorLifetimeKills", "900"),
        ("PVPFrameHonorPoints", "12345"),
        ("PVPFrameArenaPoints", "678"),
        ("PVPTeam1DataName", "Probe Squad"),
        ("PVPTeam1DataRating", "1530"),
        ("PVPTeam1DataGames", "4"),
        ("PVPTeam1DataWins", "3"),
        ("PVPTeam1DataLoss", "1"),
        // The player's games this week over the team's, 4 of 4.
        ("PVPTeam1DataPlayed", "4 (100%)"),
        ("PVPTeam2TeamType", "(3v3)"),
    ] {
        assert_eq!(text(&s, frame).as_deref(), Some(want), "{frame}");
    }
    assert!(!shown(&s, "PVPFrameOffSeason"), "season 8 is in season");

    s.run("PVPTeam1:Click()").unwrap();
    assert!(
        shown(&s, "PVPTeamDetails"),
        "a team click opens its details"
    );
    assert!(s
        .take_arena_team_requests()
        .contains(&ArenaTeamRequest::Roster { team_id: 77 }));
    for (frame, want) in [
        ("PVPTeamDetailsName", "Probe Squad"),
        ("PVPTeamDetailsSize", "(2v2)"),
        ("PVPTeamDetailsRank", "112"),
        ("PVPTeamDetailsRating", "1530"),
        // The roster sorts by name first (`0x5a2bd0`).
        ("PVPTeamDetailsButton1NameText", "Probeseven"),
        ("PVPTeamDetailsButton1RatingText", "1490"),
        ("PVPTeamDetailsButton2NameText", "Probesix"),
    ] {
        assert_eq!(text(&s, frame).as_deref(), Some(want), "{frame}");
    }
    assert!(!shown(&s, "PVPTeamDetailsButton3"), "two members, two rows");
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}

/// No season shows the off-season card over the team buttons.
#[test]
fn the_off_season_covers_the_teams() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = load();
    s.set_world_map_world_states(HashMap::from([(3901, 7)]));
    s.run("TogglePVPFrame()").unwrap();
    assert!(shown(&s, "PVPFrameOffSeason"));
    assert!(!shown(&s, "PVPTeam1"));
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}

fn row(id: u32, name: &str, instance_type: u32) -> BattlemasterRow {
    BattlemasterRow {
        id,
        name: name.into(),
        instance_type,
        max_group_size: 10,
        groups_allowed: true,
        min_level: 10,
        can_enter: true,
        holiday: false,
        description: Some(format!("{name} lore.")),
        has_map: id != 32,
    }
}

fn battlemaster_rows() -> Vec<BattlemasterRow> {
    vec![
        row(1, "Alterac Valley", 3),
        row(2, "Warsong Gulch", 3),
        row(3, "Arathi Basin", 3),
        row(6, "All Arenas", 4),
        row(32, "Random Battleground", 3),
    ]
}

fn queued(status: u32) -> BattlefieldFrame {
    let mut frame = BattlefieldFrame::default();
    frame.slots[0] = BattlefieldQueueSlot {
        status,
        name: Some("Warsong Gulch".into()),
        bg_type: 2,
        instance_id: if status == 2 { 5 } else { 0 },
        min_level: 80,
        max_level: 80,
        port_expiration_ms: 80_000,
        estimated_wait_ms: 90_000,
        time_waited_ms: 12_000,
        ..Default::default()
    };
    frame
}

fn tabs_shown(s: &UiScript) -> [bool; 2] {
    [1, 2].map(|i| {
        s.eval::<bool>(&format!(
            "return PVPParentFrameTab{i}:IsShown() and true or false"
        ))
        .unwrap()
    })
}

/// Stock `PVPBattlegroundFrame_UpdateVisible` reads the type list at `PLAYER_ENTERING_WORLD` and
/// hides both PvP window tabs when no type reads `canEnter`, so the rows are in before that event;
/// the level watch's `PVPQUEUE_ANYWHERE_UPDATE_AVAILABLE` (`0x6d66e0`) reads them again.
#[test]
fn the_pvp_tabs_follow_the_types_the_player_can_enter() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let (mut s, _failures) = super::layer_tests::production_load_with("pvp", false, "", |_| {});
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 80,
            faction_group: Some("Alliance".into()),
            ..Default::default()
        }),
    );
    s.set_battlemaster_rows(battlemaster_rows());
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.tick(0.0);
    assert_eq!(tabs_shown(&s), [true, true], "rows in at world entry");

    let (mut s, _failures) = super::layer_tests::production_load_with("pvp", false, "", |_| {});
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 9,
            faction_group: Some("Alliance".into()),
            ..Default::default()
        }),
    );
    let low: Vec<BattlemasterRow> = battlemaster_rows()
        .into_iter()
        .map(|r| BattlemasterRow {
            can_enter: false,
            ..r
        })
        .collect();
    s.set_battlemaster_rows(low);
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.tick(0.0);
    assert_eq!(
        tabs_shown(&s),
        [false, false],
        "no type the player can enter"
    );
    s.set_battlemaster_rows(battlemaster_rows());
    s.fire_event(
        "PVPQUEUE_ANYWHERE_UPDATE_AVAILABLE",
        Vec::<ScriptValue>::new(),
    );
    s.tick(0.0);
    assert_eq!(tabs_shown(&s), [true, true], "a level that enters one");
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The Battlegrounds tab lists the rows the player can enter, joins the selection, and the queue
/// that answers shows on the minimap icon, the row and, at status 2, the entry popup.
#[test]
fn the_battlegrounds_tab_queues_and_the_popup_takes_the_port() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = load();
    s.set_battlemaster_rows(battlemaster_rows());
    s.run("TogglePVPFrame() PVPParentFrameTab2:Click()")
        .unwrap();
    s.tick(0.0);
    assert!(shown(&s, "PVPBattlegroundFrame"));
    let names: Vec<Option<String>> = (1..=5)
        .map(|i| text(&s, &format!("BattlegroundType{i}.title")))
        .collect();
    assert!(
        names.iter().flatten().any(|n| n == "Warsong Gulch"),
        "the battleground rows: {names:?}"
    );
    assert!(
        !names.iter().flatten().any(|n| n == "All Arenas"),
        "an arena row is not a battleground: {names:?}"
    );
    assert!(
        !s.take_battlefield_list_requests().is_empty(),
        "the tab asks the selection's instances"
    );

    // The list answers from anywhere, and the join names its type.
    s.apply_battlefield_list(BattlefieldListPush {
        from_anywhere: true,
        bg_type: 2,
        ..Default::default()
    });
    s.tick(0.0);
    s.run("PVPBattlegroundFrameJoinButton:Click()").unwrap();
    assert_eq!(
        s.take_battlefield_joins(),
        vec![BattlefieldJoin::Battleground {
            battlemaster: 0,
            bg_type: 2,
            instance_id: 0,
            as_group: false,
        }]
    );

    s.set_battlefield_frame(queued(1));
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![ScriptValue::Int(1)]);
    s.tick(0.0);
    assert!(
        shown(&s, "MiniMapBattlefieldFrame"),
        "queued shows the icon"
    );
    let tooltip = s
        .eval::<String>("return MiniMapBattlefieldFrame.tooltip")
        .unwrap();
    assert!(
        tooltip.contains("Warsong Gulch"),
        "the icon's tooltip: {tooltip}"
    );
    assert!(!shown(&s, "StaticPopup1"), "queued is not yet a question");

    // The event names the slot that changed; the entry popup shows only for it.
    s.set_battlefield_frame(queued(2));
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![ScriptValue::Int(1)]);
    s.tick(0.0);
    assert!(shown(&s, "StaticPopup1"), "confirm raises the entry popup");
    assert_eq!(
        text(&s, "StaticPopup1Text").as_deref(),
        Some("You are now eligible to enter Warsong Gulch 5, choose an action:")
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(s.take_battlefield_port_requests(), vec![(0, true)]);

    s.set_battlefield_frame(BattlefieldFrame::default());
    s.fire_event("UPDATE_BATTLEFIELD_STATUS", vec![ScriptValue::Int(1)]);
    s.tick(0.0);
    assert!(!shown(&s, "MiniMapBattlefieldFrame"), "no queue hides it");
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}

/// An arena battlemaster's list opens the arena window, whose fourth row is the 2v2 skirmish.
#[test]
fn the_arena_battlemaster_joins_a_two_v_two_skirmish() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = load();
    s.set_battlemaster_rows(battlemaster_rows());
    s.apply_battlefield_list(BattlefieldListPush {
        battlemaster: 0xF130_0000_4D2A_0001,
        bg_type: 6,
        ..Default::default()
    });
    s.tick(0.0);
    assert!(
        shown(&s, "ArenaFrame"),
        "an arena list opens the arena window"
    );
    assert!(!shown(&s, "BattlefieldFrame"));
    assert_eq!(text(&s, "ArenaZone4").as_deref(), Some("2v2 Skirmish"));
    s.run("ArenaZone4:Click() ArenaFrameJoinButton:Click()")
        .unwrap();
    assert_eq!(
        s.take_battlefield_joins(),
        vec![BattlefieldJoin::Arena {
            battlemaster: 0xF130_0000_4D2A_0001,
            slot: 0,
            as_group: false,
            rated: false,
        }]
    );
    assert!(!shown(&s, "ArenaFrame"), "joining hides it");
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The arena registrar's greeting lists the three charters, buys one by row, and turns in a
/// filled one with the banner's eleven numbers.
#[test]
fn the_arena_registrar_buys_and_turns_in_a_charter() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = load();
    let item = |name: &str, cost| PetitionVendorItem {
        name: Some(name.into()),
        texture: Some("Interface\\Icons\\INV_Misc_Note_02".into()),
        cost,
    };
    let mut vendor = PetitionVendorState {
        open: true,
        items: vec![
            item("2v2 Arena Charter", 800_000),
            item("3v3 Arena Charter", 1_200_000),
            item("5v5 Arena Charter", 2_000_000),
        ],
        has_filled_petition: false,
    };
    s.set_petition_vendor(vendor.clone());
    s.fire_event("PETITION_VENDOR_UPDATE", Vec::<ScriptValue>::new());
    s.fire_event("PETITION_VENDOR_SHOW", Vec::<ScriptValue>::new());
    s.tick(0.0);
    assert!(shown(&s, "ArenaRegistrarFrame"));
    assert!(!shown(&s, "ArenaRegistrarButton4"), "no filled charter");

    s.run("ArenaRegistrarButton1:Click()").unwrap();
    assert!(shown(&s, "ArenaRegistrarPurchaseFrame"), "a row's price");
    s.run(
        r#"ArenaRegistrarFrameEditBox:SetText("Probe Squad")
           ArenaRegistrarFramePurchaseButton:Click()"#,
    )
    .unwrap();
    // The stock purchase button hides the window, whose `OnHide` closes the vendor.
    assert_eq!(
        s.take_petition_requests(),
        vec![
            PetitionRequest::VendorBuy {
                index: 0,
                name: "Probe Squad".into()
            },
            PetitionRequest::CloseVendor
        ]
    );
    assert!(!shown(&s, "ArenaRegistrarFrame"));
    // The close it queued fires on the next tick.
    s.tick(0.0);

    // Back with the signed charter: the bags now hold a filled one.
    vendor.has_filled_petition = true;
    s.set_petition_vendor(vendor);
    s.fire_event("PETITION_VENDOR_UPDATE", Vec::<ScriptValue>::new());
    s.fire_event("PETITION_VENDOR_SHOW", Vec::<ScriptValue>::new());
    s.tick(0.0);
    assert!(
        shown(&s, "ArenaRegistrarButton4"),
        "a filled charter offers the turn-in"
    );
    s.run("ArenaRegistrarButton4:Click()").unwrap();
    assert!(shown(&s, "PVPBannerFrame"), "the turn-in designs a banner");
    s.run("PVPBannerFrameAcceptButton:Click()").unwrap();
    let requests = s.take_petition_requests();
    assert!(
        matches!(
            requests.as_slice(),
            // The banner frame's hide closes the vendor after the turn-in.
            [PetitionRequest::VendorTurnInArena(b), PetitionRequest::CloseVendor] if b.team_size == 2
                && b.background_color >> 24 == 0xFF
                && b.emblem_style != u32::MAX
        ),
        "{requests:?}"
    );
    let errors = pvp_errors(s.take_errors());
    assert!(errors.is_empty(), "{errors:#?}");
}
