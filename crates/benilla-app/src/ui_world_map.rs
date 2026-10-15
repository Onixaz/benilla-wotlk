//! The world-map data feed behind the stock `WorldMapFrame.xml` and benilla-ui's map engine:
//!
//! - [`seed_world_map_catalog`], at world entry: the static catalog from `WorldMapArea`,
//!   `WorldMapContinent`, `WorldMapOverlay`, `DungeonMap`, `WorldMapTransforms`, `AreaPOI`,
//!   `AreaTable`, `Map` and the `.zmp` bitmaps, built as 12340's `0x547eb0` builds it.
//! - [`feed_world_map`], every frame: the world positions the engine projects when Lua asks, the
//!   discovery bits, world states and guard marker its landmarks gate on, and the first-entry
//!   selection (`0x5204f9` into `0x547170`).
//! - [`dev_map_jump`]: Alt+click the map to go there.

use std::collections::HashMap;

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{
    load_dungeon_map_catalog, load_world_map_area_catalog, load_world_map_continent_catalog,
    load_world_map_overlay_catalog, load_world_map_transform_catalog, load_zone_map, AreaPoi,
    AreaTableCatalog, WorldMapArea,
};
use benilla_ui::script::{
    UiScript, WorldMapCatalog, WorldMapContinentView, WorldMapDungeonView, WorldMapExploreGate,
    WorldMapFeed, WorldMapOverlayView, WorldMapPlayer, WorldMapPoiView, WorldMapPoint,
    WorldMapSheet, WorldMapTransformView, WorldMapZoneView,
};

use crate::net::{ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_script::UiInput;
use benilla_assets::MapCatalogRes;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::world_map::CurrentMap;

/// The map a row is drawn on (`+0x20`, else `+0x4`), which places it on a continent.
fn shown_map(a: &WorldMapArea) -> u32 {
    u32::try_from(a.display_map_id).unwrap_or(a.map_id)
}

fn loc_rect(a: &WorldMapArea) -> (f32, f32, f32, f32) {
    (a.loc_left, a.loc_right, a.loc_top, a.loc_bottom)
}

/// One `WorldMapArea` row as the engine sees it, its overlays revealed by their areas'
/// `AreaTable` exploration gates (`0x546310`).
fn zone_view(
    id: u32,
    a: &WorldMapArea,
    areas: &AreaTableCatalog,
    wma: &benilla_formats::WorldMapAreaCatalog,
    wmo: &benilla_formats::WorldMapOverlayCatalog,
) -> WorldMapZoneView {
    WorldMapZoneView {
        id,
        name: areas.name(a.area_id).unwrap_or(a.name.as_str()).to_string(),
        area_id: a.area_id,
        map_id: a.map_id,
        display_map_id: a.display_map_id,
        parent_world_map_id: a.parent_world_map_id,
        map_file: a.name.clone(),
        loc_rect: loc_rect(a),
        default_dungeon_floor: a.default_dungeon_floor,
        overlays: wmo
            .for_area(id)
            .iter()
            .map(|o| {
                let slots: Vec<_> = o
                    .area_id
                    .iter()
                    .filter(|&&aid| aid != 0)
                    .filter_map(|&aid| areas.get(aid))
                    .collect();
                // `0x545c80`: the art under its own row's folder, else `World`'s; none when empty.
                let texture = match (o.texture_name.is_empty(), wma.get(o.world_map_area_id)) {
                    (true, _) => String::new(),
                    (false, Some(row)) => {
                        format!("Interface\\WorldMap\\{}\\{}", row.name, o.texture_name)
                    }
                    (false, None) => format!("Interface\\WorldMap\\World\\{}", o.texture_name),
                };
                WorldMapOverlayView {
                    texture,
                    width: o.texture_width,
                    height: o.texture_height,
                    offset_x: o.offset_x,
                    offset_y: o.offset_y,
                    map_point: (o.map_point_x, o.map_point_y),
                    explore_bits: slots
                        .iter()
                        .filter(|r| r.exploration_level >= 0)
                        .map(|r| r.explore_flag)
                        .collect(),
                    always_revealed: slots.iter().any(|r| r.exploration_level < 0),
                    hit_rect: (
                        o.hit_rect_top,
                        o.hit_rect_left,
                        o.hit_rect_bottom,
                        o.hit_rect_right,
                    ),
                    area_name: areas.name(o.area_id[0]).map(str::to_string),
                }
            })
            .collect(),
    }
}

/// An `AreaPOI` row as a landmark source, its `AreaID` resolved to the exploration gate.
fn poi_view(poi: &AreaPoi, areas: &AreaTableCatalog) -> WorldMapPoiView {
    let explore = if (poi.area_id as i32) < 1 {
        WorldMapExploreGate::Open
    } else {
        match areas.get(poi.area_id) {
            None => WorldMapExploreGate::Never,
            Some(r) if r.exploration_level < 0 => WorldMapExploreGate::Free,
            Some(r) => WorldMapExploreGate::Bit(r.explore_flag),
        }
    };
    WorldMapPoiView {
        name: poi.name.clone(),
        description: poi.description.clone(),
        icons: poi.icons,
        pos: WorldMapPoint {
            map: poi.continent_id,
            x: poi.pos[0],
            y: poi.pos[1],
        },
        flags: poi.flags,
        area_id: poi.area_id,
        explore,
        world_state_id: poi.world_state_id,
        map_link: poi.map_link,
    }
}

/// The catalog off the patch chain, the pure half of [`seed_world_map_catalog`]; `None` when a
/// DBC fails to load, which disables the map window.
pub(crate) fn build_catalog(
    chain: &mut benilla_formats::Chain,
    areas: &AreaTableCatalog,
    maps: &MapCatalogRes,
    pois: Option<&benilla_formats::AreaPoiCatalog>,
) -> Option<WorldMapCatalog> {
    let loaded = load_world_map_area_catalog(&mut *chain).and_then(|wma| {
        let wmc = load_world_map_continent_catalog(&mut *chain)?;
        let wmo = load_world_map_overlay_catalog(&mut *chain)?;
        let dungeons = load_dungeon_map_catalog(&mut *chain)?;
        let transforms = load_world_map_transform_catalog(&mut *chain)?;
        Ok((wma, wmc, wmo, dungeons, transforms))
    });
    let (wma, wmc, wmo, dungeons, transforms) = match loaded {
        Ok(t) => t,
        Err(e) => {
            error!("world map: DBC load failed, map window disabled: {e:#}");
            return None;
        }
    };

    // Continents: the `AreaID` 0 rows in file order, the Lua continent index.
    let mut continents = Vec::new();
    for (cont_id, cont) in wma.iter().filter(|(_, a)| a.area_id == 0) {
        // Zones: the other rows drawn on this map, sorted by `AreaTable` name, case-insensitive
        // (`0x5437b0`).
        let mut zones: Vec<WorldMapZoneView> = wma
            .iter()
            .filter(|(_, a)| a.area_id != 0 && shown_map(a) == cont.map_id)
            .map(|(id, a)| zone_view(id, a, areas, &wma, &wmo))
            .collect();
        zones.sort_by_key(|z| z.name.to_lowercase());

        let sheet = wmc.get(cont.map_id).map(|c| WorldMapSheet {
            tiles: (
                c.left_boundary,
                c.right_boundary,
                c.top_boundary,
                c.bottom_boundary,
            ),
            offset: (c.offset_x, c.offset_y),
            scale: c.scale,
            clickable: c.world_map_id != 0,
        });

        // The `.zmp` bitmap: each cell's area rolled up one parent, then the row of that area
        // drawn on this map, as a 1-based zone index.
        let zone_grid: Vec<u16> = match load_zone_map(&mut *chain, &cont.name) {
            Ok(grid) => grid
                .iter()
                .map(|&raw| {
                    let mut area_id = raw;
                    if let Some(row) = areas.get(area_id) {
                        if row.zone_id != 0 {
                            area_id = row.zone_id;
                        }
                    }
                    if area_id == 0 {
                        return 0;
                    }
                    wma.iter()
                        .find(|(_, a)| shown_map(a) == cont.map_id && a.area_id == area_id)
                        .and_then(|(id, _)| zones.iter().position(|z| z.id == id))
                        .map_or(0, |i| i as u16 + 1)
                })
                .collect(),
            Err(e) => {
                warn!("world map: no zone bitmap for {}: {e:#}", cont.name);
                Vec::new()
            }
        };

        continents.push(WorldMapContinentView {
            id: cont_id,
            map_id: cont.map_id,
            name: maps
                .0
                .name(cont.map_id)
                .unwrap_or(cont.name.as_str())
                .to_string(),
            map_file: cont.name.clone(),
            loc_rect: loc_rect(cont),
            sheet,
            zone_grid,
            zones,
        });
    }

    // The orphans: rows below a continent whose own map is no continent's, or with no rect
    // (`0x547eb0`'s last passes), in file order.
    let continent_maps: Vec<u32> = continents.iter().map(|c| c.map_id).collect();
    let orphans = wma
        .iter()
        .filter(|(_, a)| {
            let has_rect = [a.loc_left, a.loc_right, a.loc_top, a.loc_bottom]
                .iter()
                .any(|v| v.abs() >= 2.384_185_8e-7);
            a.area_id != 0 && (!has_rect || !continent_maps.contains(&a.map_id))
        })
        .map(|(id, a)| zone_view(id, a, areas, &wma, &wmo))
        .collect();

    Some(WorldMapCatalog {
        continents,
        orphans,
        dungeon_maps: dungeons
            .rows()
            .iter()
            .map(|d| WorldMapDungeonView {
                id: d.id,
                map_id: d.map_id,
                floor_index: d.floor_index,
                y: (d.y_min, d.y_max),
                x: (d.x_min, d.x_max),
                parent_world_map_id: d.parent_world_map_id,
            })
            .collect(),
        transforms: transforms
            .rows()
            .iter()
            .map(|t| WorldMapTransformView {
                map_id: t.map_id,
                x: (t.x_min, t.x_max),
                y: (t.y_min, t.y_max),
                new_map_id: t.new_map_id,
                offset: (t.offset_x, t.offset_y),
                new_dungeon_map_id: t.new_dungeon_map_id,
            })
            .collect(),
        entrances: maps
            .0
            .entrances()
            .map(|(id, (map, x, y))| (id, WorldMapPoint { map, x, y }))
            .collect(),
        pois: pois
            .map(|p| p.rows().map(|(_, poi)| poi_view(poi, areas)).collect())
            .unwrap_or_default(),
    })
}

/// The built catalog, kept for the process: a later login re-seeds its VM from here.
#[derive(Resource)]
pub(crate) struct WorldMapCatalogRes(pub(crate) WorldMapCatalog);

/// Push the map catalog into the VM before any interface file runs, from
/// [`crate::ui_script::load_ingame_ui_on_world_entry`]: addons read `GetMapContinents` and
/// `GetMapZones` at file scope, and the reference holds this DBC data from load.
pub(crate) fn seed_world_map_catalog(world: &mut World, script: &mut UiScript) {
    if !world.contains_resource::<WorldMapCatalogRes>() {
        let Some(catalog) = build_catalog_from_world(world) else {
            return;
        };
        info!(
            "world map: catalog — {} continents, {} zones, {} orphans, {} floors, {} landmarks",
            catalog.continents.len(),
            catalog
                .continents
                .iter()
                .map(|c| c.zones.len())
                .sum::<usize>(),
            catalog.orphans.len(),
            catalog.dungeon_maps.len(),
            catalog.pois.len(),
        );
        world.insert_resource(WorldMapCatalogRes(catalog));
    }
    let Some(catalog) = world.get_resource::<WorldMapCatalogRes>() else {
        return;
    };
    script.set_world_map_catalog(catalog.0.clone());
}

/// [`build_catalog`] over the app's `Startup` resources; only a bare test world gets `None`.
fn build_catalog_from_world(world: &World) -> Option<WorldMapCatalog> {
    let assets = world.get_resource::<WorldAssets>()?;
    let maps = world.get_resource::<MapCatalogRes>()?;
    let areas = world.get_resource::<crate::area::AreaTableRes>()?;
    let pois = world.get_resource::<crate::area_poi::AreaPoiRes>();
    let mut chain = assets.chain.lock_recover();
    build_catalog(&mut chain, &areas.0, maps, pois.map(|p| &p.0))
}

/// [`feed_world_map`]'s memos under one [`crate::ui_script::VmMemo`], so a new VM resets them all.
#[derive(Default)]
struct FeedMemos {
    explored: Option<Vec<u32>>,
    states: Option<u64>,
    marker: Option<Option<WorldMapPoiView>>,
    /// The first-entry selection has run: the reference's cached-zone `0` gate.
    map_synced: bool,
}

/// The guard marker as a landmark source: ungated, its own icon in every slot.
fn marker_view(poi: &AreaPoi) -> WorldMapPoiView {
    WorldMapPoiView {
        name: poi.name.clone(),
        description: poi.description.clone(),
        icons: poi.icons,
        pos: WorldMapPoint {
            map: poi.continent_id,
            x: poi.pos[0],
            y: poi.pos[1],
        },
        flags: poi.flags,
        ..Default::default()
    }
}

fn feed_world_map(
    script: Option<NonSendMut<UiScript>>,
    catalog: Option<Res<WorldMapCatalogRes>>,
    player: Res<Player>,
    map: Option<Res<CurrentMap>>,
    world: benilla_world::world_point::WorldPoint,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    death_net: Res<crate::death::DeathNet>,
    poi_marker: Res<crate::poi_marker::PoiMarker>,
    group: Res<crate::ui_party::GroupState>,
    guids: Res<crate::net::GuidIndex>,
    unit_pos: Query<&GlobalTransform, With<crate::net::NetEntity>>,
    world_states: Res<crate::world_state::WorldStates>,
    mut memo: Local<crate::ui_script::VmMemo<FeedMemos>>,
) {
    let (Some(mut script), Some(catalog), Some(map), Some(areas)) = (script, catalog, map, areas)
    else {
        return;
    };
    let explored: Vec<u32> = self_q
        .iter()
        .next()
        .map(|store| {
            (0..benilla_protocol::messages::PLAYER_EXPLORED_ZONES_SLOTS)
                .map(|i| store.0.player_explored_zone_slot(i))
                .collect()
        })
        .unwrap_or_default();
    {
        let memo = memo.get(&script);
        if !explored.is_empty() && memo.explored.as_ref() != Some(&explored) {
            memo.explored = Some(explored.clone());
            script.set_world_map_explored(explored);
        }
    }
    {
        let generation = world_states.generation();
        let memo = memo.get(&script);
        if memo.states != Some(generation) {
            memo.states = Some(generation);
            script.set_world_map_world_states(world_states.pairs().collect::<HashMap<_, _>>());
        }
    }
    {
        let marker = poi_marker.poi.as_ref().map(marker_view);
        let memo = memo.get(&script);
        if memo.marker.as_ref() != Some(&marker) {
            memo.marker = Some(marker.clone());
            script.set_world_map_gossip_poi(marker);
        }
    }

    let wow = bevy_to_wow(player.pos);
    // CurrentArea is the MCNK leaf; its top zone is the zone the reference matches.
    let top_zone = world.area().and_then(|aid| areas.0.top_zone(aid));
    let point = |map: u32, x: f32, y: f32| WorldMapPoint { map, x, y };

    // The `party1..4` blips: a streamed member's transform, else the `i16` position from
    // `SMSG_PARTY_MEMBER_STATS`, whose zone names the continent and so the map.
    let member = |m: &benilla_protocol::messages::GroupMemberEntry| {
        let (px, py) = crate::minimap::party_member_pos(m, &group, &guids, &unit_pos)?;
        let member_map = group
            .stats
            .get(&m.guid)
            .and_then(|st| st.zone)
            .and_then(|zone| {
                catalog
                    .0
                    .continents
                    .iter()
                    .find(|c| c.zones.iter().any(|z| z.area_id == u32::from(zone)))
                    .map(|c| c.map_id)
            })
            .unwrap_or(map.0);
        Some(point(member_map, px, py))
    };
    let me = point(map.0, wow[0], wow[1]);
    // `raid1..40` in roster order, ourselves first so `raidN` matches; the stock map skips our
    // own slot by `UnitIsUnit`.
    let raid = if group.group_type == crate::ui_party::GROUPTYPE_RAID {
        std::iter::once(Some(me))
            .chain(group.members.iter().map(member))
            .collect()
    } else {
        Vec::new()
    };
    script.set_world_map_feed(WorldMapFeed {
        player: Some(WorldMapPlayer {
            pos: me,
            facing: player.facing(),
            zone: top_zone.unwrap_or(0),
        }),
        // The corpse at the query answer's display position, a dungeon corpse at its entrance.
        corpse: death_net.corpse.map(|cp| {
            point(
                u32::try_from(cp.display_map).unwrap_or(u32::MAX),
                cp.position[0],
                cp.position[1],
            )
        }),
        party: group.party_slots().map(member).collect(),
        raid,
    });

    // First world entry: the zone updater (`0x5204f9`), finding no cached zone, selects the
    // player's map itself before any Lua asks. Once per VM, each login being a first entry.
    let memo = memo.get(&script);
    if !memo.map_synced && top_zone.is_some() {
        memo.map_synced = true;
        script.sync_world_map_to_player_zone();
    }
}

/// Alt+click the world map to go there, a dev jump: the click's UV back through the displayed
/// rect to a world `(x, y)`, sent as `.go xy x y <mapid>` so the server finds the ground. It
/// declines on the Azeroth sheet, whose click law does not invert. The click is not consumed:
/// the stock `WorldMapButton_OnClick` still zooms into the clicked zone.
fn dev_map_jump(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    script: Option<NonSendMut<UiScript>>,
    net: Res<crate::net::NetCommands>,
) {
    // A dev affordance compiled into every build, so gated at run time.
    if !crate::run_mode::dev_affordances() {
        return;
    }
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !alt || !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let (Some(script), Ok(window)) = (script, windows.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    // Window px (y-down) to the VM's y-up 768-unit space, as the pointer feed converts them.
    let s = crate::ui_script::seam_scale(window.height(), ui_scale.0);
    let Some((u, v)) = script.world_map_uv_at(cursor.x / s, (window.height() - cursor.y) / s)
    else {
        return;
    };
    let Some((map_id, (left, right, top, bottom))) = script.world_map_displayed_rect() else {
        info!("map-jump: no click→world law at the world level — zoom into a continent first");
        return;
    };
    let wy = left - (left - right) * u;
    let wx = top - (top - bottom) * v;
    let text = format!(".go xy {wx:.2} {wy:.2} {map_id}");
    info!("map-jump: {text}");
    let _ = net.0.send(crate::net::ClientCommand::Chat {
        kind: crate::net::ChatKind::Say,
        target: None,
        text,
        language: None,
    });
}

/// The world-map data feed.
pub(crate) struct WorldMapUiPlugin;

impl Plugin for WorldMapUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                feed_world_map.after(UiInput),
                // After the script tick, so the jump hit-tests this tick's frame rects.
                dev_map_jump.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real catalog off the player's own chain, or a skip.
    fn real_catalog() -> Option<WorldMapCatalog> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let maps = MapCatalogRes(benilla_formats::load_map_catalog(&mut chain).expect("Map"));
        let pois = benilla_formats::load_area_poi_catalog(&mut chain).expect("AreaPOI");
        Some(build_catalog(&mut chain, &areas, &maps, Some(&pois)).expect("the real catalog"))
    }

    /// 12340's four continents in `WorldMapArea` file order, Outland the one the Azeroth sheet
    /// does not click, and each with its bitmap.
    #[test]
    fn the_real_catalog_has_the_four_continents_in_file_order() {
        let Some(cat) = real_catalog() else {
            return;
        };
        assert_eq!(
            cat.continents
                .iter()
                .map(|c| (c.map_id, c.map_file.as_str()))
                .collect::<Vec<_>>(),
            [
                (1, "Kalimdor"),
                (0, "Azeroth"),
                (530, "Expansion01"),
                (571, "Northrend")
            ]
        );
        assert_eq!(
            cat.continents
                .iter()
                .map(|c| c.sheet.map(|s| s.clickable))
                .collect::<Vec<_>>(),
            [Some(true), Some(true), Some(false), Some(true)]
        );
        assert!(cat
            .continents
            .iter()
            .all(|c| c.zone_grid.len() == 128 * 128));
    }

    /// A zone is placed by the map it is drawn on: map 530's Eversong Woods is an Eastern
    /// Kingdoms zone and Azuremyst Isle a Kalimdor one, neither Outland's.
    #[test]
    fn the_blood_elf_and_draenei_zones_sit_on_their_drawn_continents() {
        let Some(cat) = real_catalog() else {
            return;
        };
        let on = |map: u32, file: &str| {
            cat.continents
                .iter()
                .find(|c| c.map_id == map)
                .is_some_and(|c| c.zones.iter().any(|z| z.map_file == file))
        };
        assert!(on(0, "EversongWoods") && !on(530, "EversongWoods"));
        assert!(on(1, "AzuremystIsle") && !on(530, "AzuremystIsle"));
        assert!(cat
            .continents
            .iter()
            .find(|c| c.map_id == 530)
            .is_some_and(|c| !c.zones.is_empty()));
    }

    /// An instance map is an orphan, and so is a rectless continent row such as Dalaran, whose
    /// default floor opens it as its own row.
    #[test]
    fn the_orphans_are_the_instance_maps_and_the_rectless_rows() {
        let Some(cat) = real_catalog() else {
            return;
        };
        let continent_maps: Vec<u32> = cat.continents.iter().map(|c| c.map_id).collect();
        let dalaran = cat
            .orphans
            .iter()
            .find(|o| o.map_file == "Dalaran")
            .expect("Dalaran is an orphan");
        assert!(continent_maps.contains(&dalaran.map_id));
        assert!(dalaran.default_dungeon_floor > 0);
        assert!(cat
            .orphans
            .iter()
            .any(|o| o.map_file == "Ulduar" && !continent_maps.contains(&o.map_id)));
        assert!(!cat.dungeon_maps.is_empty() && !cat.transforms.is_empty());
    }
}
