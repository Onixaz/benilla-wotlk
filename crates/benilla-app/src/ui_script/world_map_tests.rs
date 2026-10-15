//! The world map, the stock 3.3.5a `WorldMapFrame.xml` on the production load over the real
//! catalog: it loads, opens on the player's zone, and the stock buttons walk zone, continent,
//! the Azeroth sheet and the cosmic one.

use benilla_ui::script::{UiScript, WorldMapFeed, WorldMapPlayer, WorldMapPoint};

/// Errors from the map's own code; the production load carries unrelated 3.3.5a gaps.
fn map_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("WorldMap") || e.contains("ArrowFrame"))
        .collect()
}

/// The production load with the real catalog and the player at Goldshire, Elwynn Forest.
fn session() -> (UiScript, Vec<String>) {
    let (mut s, failures) = super::layer_tests::production_load_with("worldmap", false, "", |_| {});
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let areas = benilla_formats::load_area_table_catalog(&mut chain).unwrap();
    let maps =
        benilla_assets::MapCatalogRes(benilla_formats::load_map_catalog(&mut chain).unwrap());
    let pois = benilla_formats::load_area_poi_catalog(&mut chain).unwrap();
    let catalog =
        crate::ui_world_map::build_catalog(&mut chain, &areas, &maps, Some(&pois)).unwrap();
    s.set_world_map_catalog(catalog);
    s.set_world_map_feed(WorldMapFeed {
        player: Some(WorldMapPlayer {
            pos: WorldMapPoint {
                map: 0,
                x: -9464.0,
                y: 62.0,
            },
            facing: 0.0,
            zone: 12,
        }),
        ..Default::default()
    });
    s.sync_world_map_to_player_zone();
    s.tick(0.01);
    (s, failures)
}

/// The reported symptom: the stock `OnLoad` ran to its end (it stopped at its first statement,
/// `SetClampRectInsets`), and M opens the map on the player's zone with its tiles and arrow.
#[test]
fn the_stock_map_loads_and_opens_on_the_players_zone() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = session();
    let failures = map_errors(failures);
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let _ = s.take_errors();

    s.run(
        r#"
        assert(WorldMapFrame.numQuests == 0, "OnLoad ran to its last line")
        assert(math.abs(WorldMapDetailFrame:GetScale() - WORLDMAP_QUESTLIST_SIZE) < 1e-6)
        assert(PlayerArrowFrame and math.abs(PlayerArrowEffectFrame:GetAlpha() - 0.65) < 1e-3)
        assert(WorldMapBlobFrame:GetObjectType() == "QuestPOIFrame")
        -- `WorldMapFrame_ResetFrameLevels` lifts the window to 87; its buttons ride along above
        -- its art, as `SetFrameLevel` carries the children (`0x49ea50`).
        local top = WorldMapFrame:GetFrameLevel()
        assert(top == WORLDMAP_POI_FRAMELEVEL - 13, "window level " .. top)
        for _, f in ipairs({ WorldMapZoomOutButton, WorldMapContinentDropDown, WorldMapZoneDropDown }) do
            assert(f:GetFrameLevel() > top, f:GetName() .. " sits above the window's art")
        end
        "#,
    )
    .unwrap();

    // The M binding (`Bindings.xml`: `ToggleFrame(WorldMapFrame)`).
    s.run("ToggleFrame(WorldMapFrame)").unwrap();
    s.run("WorldMapFrame_UpdateMap()").unwrap();
    s.run("this = WorldMapButton WorldMapButton_OnUpdate(WorldMapButton, 0.1) this = nil")
        .unwrap();
    s.run("WorldMapFrame_OnUpdate(WorldMapFrame)").unwrap();
    let errors = map_errors(s.take_errors());
    assert!(errors.is_empty(), "map errors: {errors:#?}");
    assert!(s.eval::<bool>("return WorldMapFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<(String, i64)>("return GetMapInfo(), GetCurrentMapContinent()")
            .unwrap(),
        ("Elwynn".into(), 2)
    );
    assert_eq!(
        s.eval::<String>("return WorldMapDetailTile1:GetTexture()")
            .unwrap()
            .to_ascii_lowercase(),
        "interface\\worldmap\\elwynn\\elwynn1"
    );
    let (u, v) = s
        .eval::<(f64, f64)>(r#"return GetPlayerMapPosition("player")"#)
        .unwrap();
    assert!(
        u > 0.0 && u < 1.0 && v > 0.0 && v < 1.0,
        "Goldshire on Elwynn: {u} {v}"
    );
    assert!(
        s.eval::<bool>("return PlayerArrowFrame:IsShown()").unwrap(),
        "the arrow is up"
    );
    assert!(!s
        .eval::<bool>("return WorldMapLevelDropDown:IsShown()")
        .unwrap());

    s.run("ToggleFrame(WorldMapFrame)").unwrap();
    let errors = map_errors(s.take_errors());
    assert!(errors.is_empty(), "close errors: {errors:#?}");
    assert!(!s.eval::<bool>("return WorldMapFrame:IsShown()").unwrap());
}

/// The stock zoom-out button walks Elwynn, the Eastern Kingdoms, the Azeroth sheet and the cosmic
/// one, where the zoom out disables and the Outland and Azeroth buttons show; those and the
/// dropdowns come back down.
#[test]
fn the_stock_buttons_walk_every_level() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = session();
    s.run("ToggleFrame(WorldMapFrame)").unwrap();
    let _ = s.take_errors();
    let at = |s: &mut UiScript| -> (Option<String>, i64, i64) {
        s.eval("return GetMapInfo(), GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap()
    };
    let zoom_out = |s: &mut UiScript| s.run("WorldMapZoomOutButton_OnClick()").unwrap();

    assert_eq!(at(&mut s).0.as_deref(), Some("Elwynn"));
    zoom_out(&mut s);
    assert_eq!(at(&mut s), (Some("Azeroth".into()), 2, 0));
    zoom_out(&mut s);
    assert_eq!(at(&mut s), (None, 0, 0));
    assert_eq!(
        s.eval::<String>("return WorldMapDetailTile1:GetTexture()")
            .unwrap()
            .to_ascii_lowercase(),
        "interface\\worldmap\\world\\world1"
    );
    zoom_out(&mut s);
    assert_eq!(at(&mut s), (None, -1, 0));
    assert!(s
        .eval::<bool>(
            "return OutlandButton:IsShown() and AzerothButton:IsShown() \
             and WorldMapZoomOutButton:IsEnabled() == 0"
        )
        .unwrap());

    s.run("OutlandButton:Click()").unwrap();
    assert_eq!(at(&mut s), (Some("Expansion01".into()), 3, 0));
    s.run("SetMapZoom(-1) AzerothButton:Click()").unwrap();
    assert_eq!(at(&mut s), (None, 0, 0));
    // The continent dropdown's fourth row, then its zone dropdown's first.
    s.run(
        "WorldMapContinentButton_OnClick({ GetID = function() return 4 end }) \
         WorldMapZoneButton_OnClick({ GetID = function() return 1 end })",
    )
    .unwrap();
    let (file, c, z) = at(&mut s);
    assert_eq!((c, z), (4, 1), "{file:?}");
    let errors = map_errors(s.take_errors());
    assert!(errors.is_empty(), "map errors: {errors:#?}");
}
