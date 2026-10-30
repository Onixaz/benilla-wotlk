//! The world-map engine over a catalog shaped like 12340's: the four selection cells and their
//! one setter, which fires `WORLD_MAP_UPDATE` inside the verb that moved it.

use std::collections::HashMap;

use super::common::script;
use crate::script::*;

/// Kalimdor, the Eastern Kingdoms, Outland and Northrend in `WorldMapArea` file order, with the
/// real row ids. Elwynn, Stormwind and map 530's Eversong are Eastern Kingdoms zones; Northrend
/// holds Crystalsong Forest and the rectless Dalaran, which opens on its first floor.
fn catalog() -> WorldMapCatalog {
    let zone = |id, name: &str, area_id, map_id, file: &str, rect| WorldMapZoneView {
        id,
        name: name.into(),
        area_id,
        map_id,
        display_map_id: -1,
        map_file: file.into(),
        loc_rect: rect,
        ..Default::default()
    };
    let sheet = |tiles, offset, clickable| {
        Some(WorldMapSheet {
            tiles,
            offset,
            scale: 0.7,
            clickable,
        })
    };
    // Cell 12735 of `Azeroth.zmp` is Goldshire, Elwynn; 8801 is where (0.5, 0.5) of Elwynn's own
    // rect lands, Stormwind.
    let mut ek_grid = vec![0u16; 128 * 128];
    ek_grid[12735] = 1;
    ek_grid[8801] = 3;
    let mut elwynn = zone(
        30,
        "Elwynn Forest",
        12,
        0,
        "Elwynn",
        (-8000.0, -10000.0, -400.0, -1900.0),
    );
    elwynn.overlays = vec![
        WorldMapOverlayView {
            texture: "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY".into(),
            width: 200,
            height: 176,
            offset_x: 490,
            offset_y: 200,
            map_point: (3, 4),
            explore_bits: vec![125],
            hit_rect: (200, 490, 376, 690),
            area_name: Some("Northshire Valley".into()),
            ..Default::default()
        },
        WorldMapOverlayView {
            texture: "Interface\\WorldMap\\Elwynn\\GOLDSHIRE".into(),
            always_revealed: true,
            hit_rect: (400, 300, 520, 460),
            area_name: Some("Goldshire".into()),
            ..Default::default()
        },
    ];
    let mut eversong = zone(
        462,
        "Eversong Woods",
        3430,
        530,
        "EversongWoods",
        (-4487.5, -9412.5, 11041.7, 7758.3),
    );
    eversong.display_map_id = 0;
    let mut dalaran = zone(504, "Dalaran", 4395, 571, "Dalaran", (0.0, 0.0, 0.0, 0.0));
    dalaran.default_dungeon_floor = 27;
    dalaran.parent_world_map_id = 510;
    let mut ulduar = zone(
        529,
        "Ulduar",
        4273,
        603,
        "Ulduar",
        (1583.3, -1704.2, 1168.8, -1022.9),
    );
    ulduar.default_dungeon_floor = -1;
    WorldMapCatalog {
        continents: vec![
            WorldMapContinentView {
                id: 13,
                map_id: 1,
                name: "Kalimdor".into(),
                map_file: "Kalimdor".into(),
                loc_rect: (17066.6, -19733.2, 12799.9, -11733.3),
                sheet: sheet((16, 46, 9, 52), (-20.188, 3.312), true),
                ..Default::default()
            },
            WorldMapContinentView {
                id: 14,
                map_id: 0,
                name: "Eastern Kingdoms".into(),
                map_file: "Azeroth".into(),
                loc_rect: (16000.0, -19199.9, 7466.6, -16000.0),
                sheet: sheet((26, 44, 8, 60), (16.875, -1.5), true),
                zone_grid: ek_grid,
                zones: vec![
                    elwynn,
                    eversong,
                    zone(
                        301,
                        "Stormwind City",
                        1519,
                        0,
                        "StormwindCity",
                        (-8200.0, -8900.0, -400.0, -1100.0),
                    ),
                ],
            },
            WorldMapContinentView {
                id: 466,
                map_id: 530,
                name: "Outland".into(),
                map_file: "Expansion01".into(),
                loc_rect: (12996.0, -4468.0, 5821.4, -5821.4),
                sheet: Some(WorldMapSheet {
                    tiles: (23, 47, 15, 61),
                    offset: (14.5, -7.0),
                    scale: 0.75,
                    clickable: false,
                }),
                ..Default::default()
            },
            WorldMapContinentView {
                id: 485,
                map_id: 571,
                name: "Northrend".into(),
                map_file: "Northrend".into(),
                loc_rect: (9217.2, -8534.2, 10593.4, -1240.9),
                sheet: sheet((16, 45, 12, 33), (1.812, -6.375), true),
                zones: vec![
                    zone(
                        510,
                        "Crystalsong Forest",
                        2817,
                        571,
                        "CrystalsongForest",
                        (1443.75, -1279.17, 6929.17, 5112.5),
                    ),
                    dalaran.clone(),
                ],
                ..Default::default()
            },
        ],
        orphans: vec![dalaran, ulduar],
        dungeon_maps: vec![
            WorldMapDungeonView {
                id: 27,
                map_id: 571,
                floor_index: 1,
                y: (222.5, 1052.5),
                x: (5513.3, 6066.7),
                parent_world_map_id: 510,
            },
            WorldMapDungeonView {
                id: 26,
                map_id: 571,
                floor_index: 2,
                y: (352.6, 915.9),
                x: (5599.9, 5975.3),
                parent_world_map_id: 510,
            },
            WorldMapDungeonView {
                id: 70,
                map_id: 603,
                floor_index: 1,
                y: (-1000.0, 1000.0),
                x: (-1000.0, 1000.0),
                parent_world_map_id: 0,
            },
        ],
        transforms: vec![WorldMapTransformView {
            map_id: 530,
            x: (4800.0, 16000.0),
            y: (-10133.3, -2666.7),
            new_map_id: 0,
            offset: (-2400.0, 2400.0),
            new_dungeon_map_id: 0,
        }],
        entrances: HashMap::from([(
            36,
            WorldMapPoint {
                map: 0,
                x: -11208.0,
                y: 1672.0,
            },
        )]),
        pois: vec![
            // A town: zone and continent levels.
            WorldMapPoiView {
                name: "Goldshire".into(),
                icons: [7; 9],
                pos: WorldMapPoint {
                    map: 0,
                    x: -1500.0,
                    y: -9500.0,
                },
                flags: 0x0d,
                area_id: 87,
                explore: WorldMapExploreGate::Bit(124),
                ..Default::default()
            },
            // A capital: every level, no gate.
            WorldMapPoiView {
                name: "Stormwind".into(),
                description: "Capital".into(),
                icons: [5; 9],
                pos: WorldMapPoint {
                    map: 0,
                    x: -800.0,
                    y: -8500.0,
                },
                flags: 0x1d,
                map_link: 31,
                ..Default::default()
            },
            // A world-state row, zone level only, its icon picked by the state.
            WorldMapPoiView {
                name: "Tower".into(),
                icons: [10, 11, 12, 13, 14, 15, 16, 17, 18],
                pos: WorldMapPoint {
                    map: 0,
                    x: -1000.0,
                    y: -9000.0,
                },
                flags: 0x284,
                world_state_id: 2372,
                ..Default::default()
            },
        ],
    }
}

fn player_at(map: u32, x: f32, y: f32, zone: u32) -> WorldMapFeed {
    WorldMapFeed {
        player: Some(WorldMapPlayer {
            pos: WorldMapPoint { map, x, y },
            facing: 0.0,
            zone,
        }),
        ..Default::default()
    }
}

fn map(s: &mut UiScript) -> (Option<String>, i64, i64) {
    s.eval("return GetMapInfo(), GetCurrentMapContinent(), GetCurrentMapZone()")
        .unwrap()
}

fn vm() -> UiScript {
    let mut s = script();
    s.set_world_map_catalog(catalog());
    s.set_world_map_feed(player_at(0, -1500.0, -9500.0, 12));
    s
}

/// The continent and zone lists, `SetMapZoom`'s levels and its bounds: 0 is the Azeroth sheet,
/// -1 the cosmic one, and past the catalog it raises.
#[test]
fn set_map_zoom_walks_the_levels_and_raises_past_the_catalog() {
    let mut s = vm();
    assert_eq!(
        s.eval::<Vec<String>>("return { GetMapContinents() }")
            .unwrap(),
        ["Kalimdor", "Eastern Kingdoms", "Outland", "Northrend"]
    );
    assert_eq!(
        s.eval::<Vec<String>>("return { GetMapZones(2) }").unwrap(),
        ["Elwynn Forest", "Eversong Woods", "Stormwind City"],
        "map 530's Eversong sits on the continent it is drawn on"
    );
    assert!(s
        .eval::<bool>(
            "return select('#', GetMapZones(0)) == 0 and select('#', GetMapZones(-1)) == 0"
        )
        .unwrap());

    s.run("SetMapZoom(2)").unwrap();
    assert_eq!(map(&mut s), (Some("Azeroth".into()), 2, 0));
    s.run("SetMapZoom(2, 1)").unwrap();
    assert_eq!(map(&mut s), (Some("Elwynn".into()), 2, 1));
    s.run("SetMapZoom(2, 9)").unwrap();
    assert_eq!(
        map(&mut s),
        (Some("Azeroth".into()), 2, 0),
        "a zone past the list is none"
    );
    s.run("SetMapZoom(0)").unwrap();
    assert_eq!(map(&mut s), (None, 0, 0), "the Azeroth sheet");
    assert!(s.eval::<bool>("return IsZoomOutAvailable()").unwrap());
    s.run("SetMapZoom(-1)").unwrap();
    assert_eq!(map(&mut s), (None, -1, 0), "the cosmic sheet");
    assert!(!s.eval::<bool>("return IsZoomOutAvailable()").unwrap());
    assert_eq!(s.eval::<i64>("return GetCurrentMapAreaID()").unwrap(), 0);

    for call in ["SetMapZoom(5)", "SetMapZoom(-2)"] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(
            err.contains("requested continent out of bounds"),
            "{call}: {err}"
        );
    }
    let err = s.run("SetMapZoom()").unwrap_err().to_string();
    assert!(err.contains("Usage: SetMapZoom(continentIndex [,zoneIndex])"));
}

/// `GetCurrentMapAreaID` answers the row id plus one, which `SetMapByID` takes less one, the
/// round trip the stock window-size toggle makes; a continent row selects the continent and an
/// instance row its own map.
#[test]
fn set_map_by_id_takes_the_area_id_less_one() {
    let mut s = vm();
    s.run("SetMapZoom(2, 3)").unwrap();
    assert_eq!(s.eval::<i64>("return GetCurrentMapAreaID()").unwrap(), 302);
    s.run("SetMapZoom(1) SetMapByID(GetCurrentMapAreaID() - 1)")
        .unwrap();
    assert_eq!(map(&mut s), (Some("Kalimdor".into()), 1, 0));
    s.run("SetMapByID(301)").unwrap();
    assert_eq!(map(&mut s), (Some("StormwindCity".into()), 2, 3));
    s.run("SetMapByID(14)").unwrap();
    assert_eq!(map(&mut s), (Some("Azeroth".into()), 2, 0));

    // Ulduar: no continent draws map 603, and floor 0 is its terrain.
    assert!(s.eval::<bool>("return SetMapByID(529)").unwrap());
    assert_eq!(map(&mut s), (Some("Ulduar".into()), -1, 0));
    assert_eq!(
        s.eval::<(Option<i64>, i64, i64)>(
            "return DungeonUsesTerrainMap(), GetCurrentMapDungeonLevel(), GetNumDungeonMapLevels()"
        )
        .unwrap(),
        (Some(1), 1, 2),
        "the terrain counts as a floor"
    );
    s.run("SetDungeonMapLevel(2)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetCurrentMapDungeonLevel()").unwrap(),
        2
    );
    assert!(s
        .eval::<Option<bool>>("return SetMapByID(9999)")
        .unwrap()
        .is_none());
}

/// A zone with a default floor opens as its own row on that floor (`0x546310`): Dalaran reads
/// back on Northrend at its zone index, floor 1 of 2; a level change and a zoom out follow the
/// `DungeonMap` rows.
#[test]
fn a_zone_with_a_default_floor_opens_on_it() {
    let mut s = vm();
    s.run("SetMapZoom(4, 2)").unwrap();
    assert_eq!(map(&mut s), (Some("Dalaran".into()), 4, 2));
    assert_eq!(
        s.eval::<(i64, i64, Option<i64>)>(
            "return GetCurrentMapDungeonLevel(), GetNumDungeonMapLevels(), DungeonUsesTerrainMap()"
        )
        .unwrap(),
        (1, 2, None)
    );
    assert_eq!(
        s.world_map_selection(),
        WorldMapSelection {
            continent: -2,
            zone: -1,
            dungeon_map: 27,
            direct: 504
        }
    );
    s.run("SetDungeonMapLevel(2)").unwrap();
    assert_eq!(s.world_map_selection().dungeon_map, 26);
    s.run("SetDungeonMapLevel(0)").unwrap();
    assert_eq!(
        s.world_map_selection().dungeon_map,
        26,
        "floor 0 is no level here"
    );
    assert!(s.eval::<bool>("return IsZoomOutAvailable()").unwrap());
    s.run("ZoomOut()").unwrap();
    assert_eq!(map(&mut s), (Some("CrystalsongForest".into()), 4, 1));
}

/// #82: each selection verb runs the setter and fires `WORLD_MAP_UPDATE` before it returns, so
/// the handler already reads the new map and its landmarks.
#[test]
fn world_map_update_fires_inside_each_selection_verb() {
    let mut s = vm();
    s.tick(0.01); // the catalog push's queued event
    s.run(
        r#"
        seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("WORLD_MAP_UPDATE")
        f:SetScript("OnEvent", function()
            table.insert(seen, (GetMapInfo() or "nil") .. ":" .. GetNumMapLandmarks())
        end)
        "#,
    )
    .unwrap();
    let after = |s: &mut UiScript, call: &str| -> Vec<String> {
        s.run(&format!("seen = {{}} {call} heard = seen")).unwrap();
        s.eval("return heard").unwrap()
    };
    assert_eq!(
        after(&mut s, "SetMapZoom(2)"),
        ["Azeroth:1"],
        "the capital; the town is not explored yet"
    );
    assert_eq!(
        after(&mut s, "ProcessMapClick(0.452843, 0.720880)"),
        ["Elwynn:1"],
        "the grid click drills into Elwynn"
    );
    s.run("SetMapZoom(0)").unwrap();
    assert_eq!(
        after(&mut s, "SetMapToCurrentZone()"),
        ["Elwynn:1"],
        "the player's zone"
    );
    assert_eq!(
        after(&mut s, "ZoomOut()"),
        Vec::<String>::new(),
        "no parent: no setter, no event"
    );
    s.tick(0.01);
    assert_eq!(
        s.eval::<Vec<String>>("return seen").unwrap(),
        Vec::<String>::new()
    );
}

/// `FindCurrentZone`: the player's zone on the continent the transform carries the position to,
/// an instance row, an instance's entrance, else the cosmic sheet; with no player the Azeroth one.
#[test]
fn set_map_to_current_zone_resolves_the_players_map() {
    let mut s = vm();
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(map(&mut s), (Some("Elwynn".into()), 2, 1));

    // Silvermoon's neighbourhood on map 530 draws on the Eastern Kingdoms.
    s.set_world_map_feed(player_at(530, 9400.0, -6800.0, 3430));
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(map(&mut s), (Some("EversongWoods".into()), 2, 2));

    s.set_world_map_feed(player_at(603, 0.0, 0.0, 4273));
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(map(&mut s), (Some("Ulduar".into()), -1, 0));

    // An instance with no row, its entrance in the Eastern Kingdoms off any zone's cell.
    s.set_world_map_feed(player_at(36, 0.0, 0.0, 1581));
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(map(&mut s), (Some("Azeroth".into()), 2, 0));

    s.set_world_map_feed(player_at(999, 0.0, 0.0, 1));
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(
        map(&mut s),
        (None, -1, 0),
        "a true orphan: the cosmic sheet"
    );

    s.set_world_map_feed(WorldMapFeed::default());
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(map(&mut s), (None, 0, 0));
}

/// A position verb projects at call time, through the displayed map: the zone's own rect, the
/// continent's after the transform, the sheet's constants with Outland hidden.
#[test]
fn positions_project_onto_the_map_of_the_moment() {
    let mut s = vm();
    let pos = |s: &mut UiScript| -> (f64, f64) {
        s.eval(r#"return GetPlayerMapPosition("player")"#).unwrap()
    };
    s.run("SetMapZoom(2, 1)").unwrap();
    let (u, v) = pos(&mut s);
    assert!(
        (u - 0.75).abs() < 1e-5 && (v - (1.0 - 400.0 / 1500.0)).abs() < 1e-5,
        "{u} {v}"
    );
    s.run("SetMapZoom(1)").unwrap();
    assert_eq!(pos(&mut s), (0.0, 0.0), "Kalimdor does not hold map 0");
    s.run("SetMapZoom(0)").unwrap();
    let (u, v) = pos(&mut s);
    assert!(u > 0.5 && v > 0.0, "on the sheet's eastern half: {u} {v}");

    s.set_world_map_feed(player_at(530, 0.0, 0.0, 3483));
    assert_eq!(
        pos(&mut s),
        (0.0, 0.0),
        "Outland is hidden on the Azeroth sheet"
    );
    s.set_world_map_feed(player_at(530, 9400.0, -6800.0, 3430));
    s.run("SetMapZoom(2)").unwrap();
    let (u, v) = pos(&mut s);
    assert!(
        u > 0.0 && v > 0.0,
        "Eversong lands on the Eastern Kingdoms: {u} {v}"
    );
    s.run("SetMapZoom(2, 2)").unwrap();
    let (u, v) = pos(&mut s);
    assert!(
        u > 0.0 && v > 0.0,
        "and on its own zone map, untransformed: {u} {v}"
    );
}

/// The landmark gates and `GetMapLandmarkInfo`'s seven values: a town at zone and continent
/// level once explored, the capital everywhere, the tower at zone level while its state is set,
/// its icon picked by the state; the zone level shows icon 0 unless `Flags & 0x80`.
#[test]
fn landmarks_pass_their_gates_and_answer_seven_values() {
    let mut s = vm();
    let names = |s: &mut UiScript| -> Vec<String> {
        s.eval("local t = {} for i = 1, GetNumMapLandmarks() do t[i] = GetMapLandmarkInfo(i) end return t")
            .unwrap()
    };
    s.run("SetMapZoom(0)").unwrap();
    assert_eq!(names(&mut s), ["Stormwind"]);
    s.run("SetMapZoom(2)").unwrap();
    assert_eq!(names(&mut s), ["Stormwind"], "the town is not explored");
    let mut explored = vec![0u32; 64];
    explored[3] = 1 << 28;
    s.set_world_map_explored(explored);
    assert_eq!(names(&mut s), ["Goldshire", "Stormwind"]);
    assert_eq!(
        s.eval::<(String, String, i64, f64, f64, i64, bool)>("return GetMapLandmarkInfo(2)")
            .unwrap()
            .2,
        5,
        "the continent map shows the row's icon"
    );

    s.run("SetMapZoom(2, 1)").unwrap();
    assert_eq!(
        names(&mut s),
        ["Goldshire", "Stormwind"],
        "no state, no tower"
    );
    s.set_world_map_world_states(HashMap::from([(2372, 3)]));
    assert_eq!(names(&mut s), ["Goldshire", "Stormwind", "Tower"]);
    let (name, desc, icon, _, _, link, battle) = s
        .eval::<(String, Option<String>, i64, f64, f64, Option<i64>, bool)>(
            "return GetMapLandmarkInfo(3)",
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), desc, icon, link, battle),
        ("Tower", None, 12, None, true)
    );
    let (icon, link) = s
        .eval::<(i64, Option<i64>)>("local _, _, i, _, _, l = GetMapLandmarkInfo(2) return i, l")
        .unwrap();
    assert_eq!(
        (icon, link),
        (0, Some(31)),
        "zone level, no 0x80: icon 0; the map link"
    );
    assert_eq!(
        s.arity("GetMapLandmarkInfo(9)").unwrap(),
        7,
        "out of range: still seven values"
    );

    // The guard's marker comes last, ungated.
    s.set_world_map_gossip_poi(Some(WorldMapPoiView {
        name: "Trainer".into(),
        icons: [6; 9],
        pos: WorldMapPoint {
            map: 0,
            x: -1200.0,
            y: -9200.0,
        },
        ..Default::default()
    }));
    assert_eq!(names(&mut s).last().map(String::as_str), Some("Trainer"));
}

/// Overlays exist at zone level only, revealed by an explored bit or a free area; each answers
/// its map point.
#[test]
fn overlays_reveal_at_zone_level_only() {
    let mut s = vm();
    s.run("SetMapZoom(2, 1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetNumMapOverlays()").unwrap(),
        1,
        "the free one"
    );
    let mut explored = vec![0u32; 64];
    explored[3] = 1 << 29;
    s.set_world_map_explored(explored);
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 2);
    assert_eq!(
        s.eval::<(String, i64, i64, i64, i64, i64, i64)>("return GetMapOverlayInfo(1)")
            .unwrap(),
        (
            "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY".into(),
            200,
            176,
            490,
            200,
            3,
            4
        )
    );
    assert_eq!(s.arity("GetMapOverlayInfo(3)").unwrap(), 7);
    s.run("SetMapZoom(2)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 0);
}

/// Clicks: a continent box on the Azeroth sheet (Outland's never), a zone through the grid, and
/// nothing on a direct row or the cosmic sheet.
#[test]
fn process_map_click_drills_down() {
    let mut s = vm();
    s.run("SetMapZoom(0) ProcessMapClick(0.80, 0.50)").unwrap();
    assert_eq!(map(&mut s), (Some("Azeroth".into()), 2, 0));
    s.run("ProcessMapClick(0.5, 0.5)").unwrap();
    assert_eq!(map(&mut s), (Some("Azeroth".into()), 2, 0), "open sea");
    s.run("ProcessMapClick(0.452843, 0.720880)").unwrap();
    assert_eq!(map(&mut s), (Some("Elwynn".into()), 2, 1));
    s.run("ProcessMapClick(0.5, 0.5)").unwrap();
    assert_eq!(
        map(&mut s),
        (Some("StormwindCity".into()), 2, 3),
        "a city inside the zone"
    );
    s.run("SetMapZoom(-1) ProcessMapClick(0.80, 0.50)").unwrap();
    assert_eq!(map(&mut s), (None, -1, 0));
    let err = s.run("ProcessMapClick(1)").unwrap_err().to_string();
    assert!(err.contains("Usage: ProcessMapClick(x, y)"));
}

/// `UpdateMapHighlight`: a zone lights up on its continent, a continent on the Azeroth sheet; on
/// a zone map a neighbour is named without a highlight, and a revealed overlay names its area.
#[test]
fn the_highlight_follows_the_level() {
    let mut s = vm();
    type Answer = (Option<String>, Option<String>, f64, f64, f64, f64, f64, f64);
    let hover = |s: &mut UiScript, x: f32, y: f32| -> Answer {
        s.eval(&format!("return UpdateMapHighlight({x}, {y})"))
            .unwrap()
    };
    s.run("SetMapZoom(2)").unwrap();
    let (name, file, tpx, tpy, tx, ty, _, _) = hover(&mut s, 0.452843, 0.720880);
    assert_eq!(
        (name.as_deref(), file.as_deref(), tpx),
        (Some("Elwynn Forest"), Some("Elwynn"), 1.0)
    );
    assert!((tpy - 0.75).abs() < 1e-6, "96 of 128: {tpy}");
    assert!((tx - 2000.0 / 35199.9).abs() < 1e-4 && (ty - 1500.0 / 23466.6).abs() < 1e-4);

    s.run("SetMapZoom(0)").unwrap();
    let (name, file, ..) = hover(&mut s, 0.80, 0.50);
    assert_eq!(
        (name.as_deref(), file.as_deref()),
        (Some("Eastern Kingdoms"), Some("Azeroth"))
    );

    s.run("SetMapZoom(2, 1)").unwrap();
    let miss: Answer = (None, None, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let name = |n: &str| -> Answer { (Some(n.into()), None, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0) };
    assert_eq!(hover(&mut s, 0.5, 0.5), name("Stormwind City"));
    assert_eq!(
        hover(&mut s, 0.42, 0.7),
        name("Goldshire"),
        "the free overlay"
    );
    assert_eq!(hover(&mut s, 0.1, 0.1), miss);
}

/// `GetMapInfo`'s heights: a zone's art at 1002 wide, the continent's from its tiles, the low
/// byte of the second rounded to a power of two; none on the sheets.
#[test]
fn get_map_info_answers_the_art_height() {
    let s = vm();
    s.run("SetMapZoom(2, 1)").unwrap();
    let (_, h, pot) = s.eval::<(String, i64, i64)>("return GetMapInfo()").unwrap();
    assert_eq!(h, (1500.0f32 * 1002.0 / 2000.0) as i64);
    assert_eq!(
        (h, pot),
        (751, 768),
        "0x2ef: the low byte 0xef rounds up to 0x100"
    );
    s.run("SetMapZoom(2)").unwrap();
    let (_, h, _) = s.eval::<(String, i64, i64)>("return GetMapInfo()").unwrap();
    assert_eq!(h, (53.0f32 * 1002.0 / 19.0) as i64);
    s.run("SetMapZoom(0)").unwrap();
    assert_eq!(
        s.eval::<(Option<String>, i64, i64)>("return GetMapInfo()")
            .unwrap(),
        (None, 0, 0)
    );
}

/// The refreshes the data edges run keep the selection, a direct row included, and queue the
/// event for the next tick.
#[test]
fn a_data_push_refreshes_in_place_and_queues_the_event() {
    let mut s = vm();
    s.run("SetMapByID(529)").unwrap();
    s.tick(0.01);
    s.run(
        r#"heard = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("WORLD_MAP_UPDATE")
        f:SetScript("OnEvent", function() heard = heard + 1 end)"#,
    )
    .unwrap();
    s.set_world_map_explored(vec![1; 64]);
    assert_eq!(s.eval::<i64>("return heard").unwrap(), 0);
    s.tick(0.01);
    assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
    assert_eq!(map(&mut s), (Some("Ulduar".into()), -1, 0));
}

/// The engine's first-entry selection, with no Lua in the loop.
#[test]
fn the_engine_selects_the_players_zone_on_first_entry() {
    let mut s = vm();
    assert_eq!(map(&mut s), (None, 0, 0));
    s.sync_world_map_to_player_zone();
    assert_eq!(map(&mut s), (Some("Elwynn".into()), 2, 1));
}

/// The debug verbs are the reference's: no objects, nothing returned.
#[test]
fn the_debug_map_verbs_answer_nothing() {
    let mut s = vm();
    assert_eq!(s.eval::<i64>("return GetNumMapDebugObjects()").unwrap(), 0);
    for call in [
        "HasDebugZoneMap()",
        "GetDebugZoneMap(1, 1)",
        "GetMapDebugObjectInfo(1)",
    ] {
        assert_eq!(s.arity(call).unwrap(), 0, "{call}");
    }
    assert_eq!(
        s.arity("GetWintergraspWaitTime()").unwrap(),
        0,
        "no battle scheduled"
    );
    s.set_world_map_world_states(HashMap::from([(3801, 1), (4354, 0)]));
    assert_eq!(
        s.eval::<i64>("return GetWintergraspWaitTime()").unwrap(),
        0,
        "a past start reads 0, never negative"
    );
    assert!(s
        .eval::<Option<i64>>("return CanQueueForWintergrasp()")
        .unwrap()
        .is_none());
    s.set_world_map_world_states(HashMap::from([(4375, 1)]));
    assert_eq!(
        s.eval::<i64>("return CanQueueForWintergrasp()").unwrap(),
        1,
        "world state 4375 opens the queue"
    );
}

/// A moving player's feed is no extract input: the map's Lua reads it through its verbs, so a new
/// position leaves the mutation count standing, while the verbs still answer the new place.
#[test]
fn a_moving_player_leaves_the_mutation_count_standing() {
    let mut s = vm();
    s.run("SetMapZoom(2, 1)").unwrap();
    let pos = |s: &mut UiScript| -> (f64, f64) {
        s.eval(r#"return GetPlayerMapPosition("player")"#).unwrap()
    };
    let at = pos(&mut s);
    let before = s.visual_epoch();
    s.set_world_map_feed(player_at(0, -1510.0, -9490.0, 12));
    assert_eq!(s.visual_epoch(), before, "a feed write moved the count");
    assert_ne!(pos(&mut s), at, "the verb answers the new place");
}
