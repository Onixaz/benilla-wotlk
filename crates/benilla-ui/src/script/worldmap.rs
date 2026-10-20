//! The world map, 12340's `CGWorldMap` over a catalog the app pushes once and a per-frame feed of
//! world positions. The selection is four cells, `[0xaccf04]` continent, `[0xaccf08]` zone,
//! `[0xaccf0c]` `DungeonMap` row and `[0xaccf10]` direct `WorldMapArea` row, with one writer, the
//! setter `0x546310`: it rebuilds the landmark list and fires `WORLD_MAP_UPDATE` before it returns,
//! so a handler reading the map inside a selection verb sees the new one.
//!
//! The cells are 0-based with -1 for none, as the reference keeps them; Lua reads them plus one.
//! Continent -1 is the Azeroth sheet and -2 a map selected by its row (an instance or a dungeon
//! floor), or with no row the cosmic sheet. Projection is the engine's (`0x544140`), so every
//! position verb answers for the selection of the moment it is called.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// A world position: the `Map.dbc` id and the world `(x, y)`, x north and y west.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldMapPoint {
    pub map: u32,
    pub x: f32,
    pub y: f32,
}

/// One `WorldMapArea.dbc` row below a continent: a zone, a city, or an instance map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapZoneView {
    /// The row id, which the selection and `SetMapByID` hold.
    pub id: u32,
    /// `AreaTable.dbc`'s localized name, "Elwynn Forest".
    pub name: String,
    pub area_id: u32,
    pub map_id: u32,
    /// The map the row is drawn on, -1 for its own (`+0x20`): map 530's blood elf zones are 0.
    pub display_map_id: i32,
    /// The row a zoom out returns to (`+0x28`), 0 for none.
    pub parent_world_map_id: u32,
    /// The `Interface\WorldMap\<file>\` art folder, `WorldMapArea`'s `AreaName`.
    pub map_file: String,
    /// The loc rect `(left, right, top, bottom)`: left and right are world y, top and bottom x.
    pub loc_rect: (f32, f32, f32, f32),
    pub overlays: Vec<WorldMapOverlayView>,
    /// `DefaultDungeonFloor` (`+0x24`): a `DungeonMap` row the setter opens the row on when
    /// positive, -1 on a map whose floor 0 is the terrain.
    pub default_dungeon_floor: i32,
}

impl WorldMapZoneView {
    /// The map the row is drawn on (`+0x20`, else `+0x4`), which places it on a continent.
    fn shown_map(&self) -> u32 {
        u32::try_from(self.display_map_id).unwrap_or(self.map_id)
    }

    fn has_rect(&self) -> bool {
        has_rect(self.loc_rect)
    }
}

/// A rect with any corner off zero, the builders' `2.384e-7` test (`0x547eb0`, `0x547170`).
fn has_rect((a, b, c, d): (f32, f32, f32, f32)) -> bool {
    const EPS: f32 = 2.384_185_8e-7;
    [a, b, c, d].iter().any(|v| v.abs() >= EPS)
}

/// A POI icon source: an `AreaPOI.dbc` row, or the guard-directions marker (`SMSG_GOSSIP_POI`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapPoiView {
    pub name: String,
    /// The second tooltip line; empty for none.
    pub description: String,
    /// `Icon[9]`, indexed by the row's world state (`0x545a30`).
    pub icons: [u32; 9],
    pub pos: WorldMapPoint,
    pub flags: u32,
    pub area_id: u32,
    /// The exploration gate its `AreaID` resolves to (`0x546310`).
    pub explore: WorldMapExploreGate,
    pub world_state_id: u32,
    /// `WorldMapLink`, the `DungeonMap` row `ClickLandmark` opens; 0 for none.
    pub map_link: u32,
}

/// An `AreaPOI` row's exploration gate, from its `AreaID`'s `AreaTable` row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WorldMapExploreGate {
    /// `AreaID < 1`: no gate.
    #[default]
    Open,
    /// An `AreaID` no row carries: the lookup fails and the row never shows.
    Never,
    /// A row with `ExplorationLevel < 0`: shown whenever there is a player.
    Free,
    /// Shown once this `PLAYER_EXPLORED_ZONES` bit is set.
    Bit(u32),
}

/// A sub-area's map art, drawn over the zone tiles once revealed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapOverlayView {
    /// `Interface\WorldMap\<Zone>\<TexName>`; the Lua appends the piece number.
    pub texture: String,
    pub width: u32,
    pub height: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    /// `MapPointX`, `MapPointY` (`+0x18`, `+0x1c`).
    pub map_point: (u32, u32),
    /// The explore bits of the area slots that need one; any one set reveals.
    pub explore_bits: Vec<u32>,
    /// A slot's area has `ExplorationLevel < 0`, which reveals with nothing explored.
    pub always_revealed: bool,
    /// `HitRect*` `(top, left, bottom, right)` in detail-frame pixels, edges inclusive.
    pub hit_rect: (u32, u32, u32, u32),
    /// The first area slot's `AreaTable` name, the hover label; `None` is walked past.
    pub area_name: Option<String>,
}

/// A continent's `WorldMapContinent.dbc` row: its tiles on the Azeroth sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldMapSheet {
    /// ADT tile bounds `(left, right, top, bottom)`.
    pub tiles: (u32, u32, u32, u32),
    pub offset: (f32, f32),
    pub scale: f32,
    /// The last column (`+0x34`): 0 on Outland, which the Azeroth sheet does not click.
    pub clickable: bool,
}

impl WorldMapSheet {
    /// The `(u0, v0, u1, v1)` click box on the sheet (`0x547eb0`).
    fn rect(&self) -> (f32, f32, f32, f32) {
        let (left, right, top, bottom) = self.tiles;
        let s = self.scale;
        let u = ((31.3125 - s * 32.0) + self.offset.0) * SHEET_U;
        let v = ((20.875 - s * 32.0) + self.offset.1) * SHEET_V;
        (
            left as f32 * s * SHEET_U + u,
            top as f32 * s * SHEET_V + v,
            (right + 1) as f32 * s * SHEET_U + u,
            (bottom + 1) as f32 * s * SHEET_V + v,
        )
    }
}

/// `1/62.625` and `1/41.75`, the Azeroth sheet's span in tiles (`0x547eb0`).
const SHEET_U: f32 = 0.015_968_064;
const SHEET_V: f32 = 0.023_952_097;

/// One continent: its `AreaID` 0 row, its zones and its sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapContinentView {
    /// The continent's `WorldMapArea` row id.
    pub id: u32,
    pub map_id: u32,
    /// `Map.dbc`'s localized name, the dropdown label.
    pub name: String,
    pub map_file: String,
    pub loc_rect: (f32, f32, f32, f32),
    pub sheet: Option<WorldMapSheet>,
    /// The 128×128 `.zmp` bitmap as 1-based indices into [`Self::zones`], 0 for none.
    pub zone_grid: Vec<u16>,
    /// The rows drawn on this map, sorted by name: the Lua zone index.
    pub zones: Vec<WorldMapZoneView>,
}

impl WorldMapContinentView {
    fn row(&self) -> WorldMapZoneView {
        WorldMapZoneView {
            id: self.id,
            name: self.name.clone(),
            map_id: self.map_id,
            display_map_id: -1,
            map_file: self.map_file.clone(),
            loc_rect: self.loc_rect,
            ..Default::default()
        }
    }
}

/// One `DungeonMap.dbc` row, a floor of a map.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldMapDungeonView {
    pub id: u32,
    pub map_id: u32,
    pub floor_index: i32,
    /// `(min, max)` of world y and of world x.
    pub y: (f32, f32),
    pub x: (f32, f32),
    pub parent_world_map_id: u32,
}

/// One `WorldMapTransforms.dbc` row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldMapTransformView {
    pub map_id: u32,
    pub x: (f32, f32),
    pub y: (f32, f32),
    pub new_map_id: u32,
    pub offset: (f32, f32),
    pub new_dungeon_map_id: u32,
}

/// The static catalog, built once from the DBCs (`0x547eb0`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapCatalog {
    pub continents: Vec<WorldMapContinentView>,
    /// The rows on no continent, or with no rect (`[0xbe8f3c]`), in file order.
    pub orphans: Vec<WorldMapZoneView>,
    pub dungeon_maps: Vec<WorldMapDungeonView>,
    pub transforms: Vec<WorldMapTransformView>,
    /// `Map.dbc`'s entrance (`+0x2c..+0x34`) by map, for maps with one.
    pub entrances: HashMap<u32, WorldMapPoint>,
    /// `AreaPOI.dbc`, in file order.
    pub pois: Vec<WorldMapPoiView>,
}

/// The player as the map reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldMapPlayer {
    pub pos: WorldMapPoint,
    /// Radians, 0 north, counterclockwise-positive.
    pub facing: f32,
    /// The zone `AreaTable` id the resolver matches (`[0xbd080c]`).
    pub zone: u32,
}

/// The per-frame feed: world positions the position verbs project when called.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldMapFeed {
    pub player: Option<WorldMapPlayer>,
    pub corpse: Option<WorldMapPoint>,
    /// `party1`..`party4` in party order.
    pub party: Vec<Option<WorldMapPoint>>,
    /// `raid1`..`raid40` in roster order.
    pub raid: Vec<Option<WorldMapPoint>>,
}

/// The four selection cells, as the reference keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldMapSelection {
    pub continent: i32,
    pub zone: i32,
    pub dungeon_map: i32,
    pub direct: i32,
}

impl Default for WorldMapSelection {
    fn default() -> Self {
        Self {
            continent: -1,
            zone: -1,
            dungeon_map: -1,
            direct: -1,
        }
    }
}

/// A landmark slot (`[0xbe8f1c]`, 20 bytes): its source and its place on the displayed map.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Landmark {
    Poi(usize, (f32, f32)),
    Marker((f32, f32)),
}

/// The engine-side world map.
#[derive(Default)]
pub struct WorldMapState {
    catalog: WorldMapCatalog,
    selection: WorldMapSelection,
    feed: WorldMapFeed,
    /// `PLAYER_EXPLORED_ZONES`, bit n being `AreaTable` `exploreFlag` n.
    explored: Vec<u32>,
    world_states: HashMap<u32, i32>,
    marker: Option<WorldMapPoiView>,
    landmarks: Vec<Landmark>,
    /// The arrow frames' wrapper ids, built once per session.
    pub(crate) arrow_world: Option<(u32, u32)>,
    pub(crate) arrow_mini: Option<(u32, u32)>,
}

fn explored_bit(explored: &[u32], n: u32) -> bool {
    explored
        .get((n / 32) as usize)
        .is_some_and(|w| w & (1 << (n % 32)) != 0)
}

/// `1/(64·533.33)`, yards to `.zmp` cells over 128 (`0x543e50`).
const GRID_K: f32 = 2.929_687_6e-5;

impl WorldMapState {
    /// The player's facing, for the arrow frames; `None` with no player.
    pub(crate) fn player_facing(&self) -> Option<f32> {
        self.feed.player.map(|p| p.facing)
    }

    fn continent(&self, c: i32) -> Option<&WorldMapContinentView> {
        usize::try_from(c)
            .ok()
            .and_then(|c| self.catalog.continents.get(c))
    }

    /// Any `WorldMapArea` row by id, the reference's `[0xad4edc]` lookup.
    fn row(&self, id: i32) -> Option<WorldMapZoneView> {
        let id = u32::try_from(id).ok()?;
        self.catalog
            .continents
            .iter()
            .find_map(|c| {
                if c.id == id {
                    return Some(c.row());
                }
                c.zones.iter().find(|z| z.id == id).cloned()
            })
            .or_else(|| self.catalog.orphans.iter().find(|o| o.id == id).cloned())
    }

    fn dungeon(&self, id: i32) -> Option<&WorldMapDungeonView> {
        let id = u32::try_from(id).ok().filter(|&id| id != 0)?;
        self.catalog.dungeon_maps.iter().find(|d| d.id == id)
    }

    /// The displayed row's id (`0x543770`): a continent's zone or own row, else the direct cell.
    fn current_id(&self) -> i32 {
        let s = self.selection;
        match self.continent(s.continent) {
            Some(c) => match usize::try_from(s.zone).ok().and_then(|z| c.zones.get(z)) {
                Some(z) => z.id as i32,
                None => c.id as i32,
            },
            None if s.continent >= 0 => -1,
            None => s.direct,
        }
    }

    /// The dungeon cell as its readers take it: none unless a row is selected directly.
    fn current_dungeon(&self) -> i32 {
        if self.selection.direct == -1 {
            -1
        } else {
            self.selection.dungeon_map
        }
    }

    /// `0x543920`: the first transform holding a position, applied; the position itself and no
    /// dungeon row otherwise.
    fn transform(&self, p: WorldMapPoint) -> (WorldMapPoint, u32) {
        self.catalog
            .transforms
            .iter()
            .find(|t| {
                t.map_id == p.map
                    && (t.x.0..=t.x.1).contains(&p.x)
                    && (t.y.0..=t.y.1).contains(&p.y)
            })
            .map_or((p, 0), |t| {
                (
                    WorldMapPoint {
                        map: t.new_map_id,
                        x: p.x + t.offset.0,
                        y: p.y + t.offset.1,
                    },
                    t.new_dungeon_map_id,
                )
            })
    }

    /// `0x544140`: a world position on the displayed map, `(0, 0)` off it. The second value is
    /// the Outland flag the Azeroth sheet raises for map 530 (`0x212`), which every caller but
    /// the corpse's honours by hiding.
    fn project(&self, p: WorldMapPoint) -> ((f32, f32), bool) {
        const OFF: ((f32, f32), bool) = ((0.0, 0.0), false);
        let s = self.selection;
        let (t, t_dungeon) = self.transform(p);
        if s.continent == -1 {
            if let Some(sheet) = self
                .catalog
                .continents
                .iter()
                .rev()
                .find(|c| c.map_id == t.map)
                .and_then(|c| c.sheet)
            {
                let u = (sheet.offset.0 * SHEET_U + 0.5) - t.y * 2.994_012e-5 * sheet.scale;
                let v = (sheet.offset.1 * SHEET_V + 0.5) - t.x * 4.491_018_3e-5 * sheet.scale;
                return ((u, v), t.map == 530);
            }
        }
        if s.continent >= 0
            && self
                .continent(s.continent)
                .is_none_or(|c| c.map_id != t.map)
        {
            return OFF;
        }
        let Some(row) = self.row(self.current_id()) else {
            return OFF;
        };
        if row.map_id != p.map && row.map_id != t.map {
            return OFF;
        }
        // A zone's rect is in its own map's coordinates; a continent's in the drawn map's.
        let pos = if s.zone < 0 { t } else { p };
        // `DungeonMapChunk` is not read, so the floor the position stands on is never known.
        let at_floor = -1;
        let shown = self.current_dungeon();
        let floor = if at_floor == shown {
            self.dungeon(at_floor)
        } else if t_dungeon as i32 == shown {
            self.dungeon(shown)
        } else if shown > 0 {
            return OFF;
        } else {
            None
        };
        let (y_lo, y_hi, x_lo, x_hi) = match floor {
            Some(d) => (d.y.0, d.y.1, d.x.0, d.x.1),
            None => (
                row.loc_rect.1,
                row.loc_rect.0,
                row.loc_rect.3,
                row.loc_rect.2,
            ),
        };
        let axis = |input: f32, lo: f32, hi: f32| {
            if input - lo == 0.0 || hi - lo == 0.0 {
                0.0
            } else {
                1.0 - (input - lo) / (hi - lo)
            }
        };
        let (u, v) = (axis(pos.y, y_lo, y_hi), axis(pos.x, x_lo, x_hi));
        if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
            ((u, v), false)
        } else {
            OFF
        }
    }

    /// A unit's blip (`0x5444f0`): the projection, hidden on the Outland flag.
    fn blip(&self, p: Option<WorldMapPoint>) -> (f32, f32) {
        match p.map(|p| self.project(p)) {
            Some((uv, false)) => uv,
            _ => (0.0, 0.0),
        }
    }

    /// `0x543e50`: the zone under map UV `(x, y)` through the displayed rect, its corners carried
    /// onto the drawn map, as a 1-based index into the continent's zones.
    fn grid_zone(&self, x: f32, y: f32) -> Option<u16> {
        let s = self.selection;
        let id = self.current_id();
        if id == -1 || s.continent == -2 {
            return None;
        }
        let cont = self.continent(s.continent)?;
        let row = self.row(id)?;
        let (left, right, top, bottom) = row.loc_rect;
        let (tl, _) = self.transform(WorldMapPoint {
            map: row.map_id,
            x: top,
            y: left,
        });
        let (br, _) = self.transform(WorldMapPoint {
            map: row.map_id,
            x: bottom,
            y: right,
        });
        let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        if tl.y - br.y == 0.0 || tl.x - br.x == 0.0 {
            return None;
        }
        let f2 = 0.5 - (br.y + (1.0 - x) * (tl.y - br.y)) * GRID_K;
        let f3 = 0.5 - ((1.0 - y) * (tl.x - br.x) + br.x) * GRID_K;
        if !(0.0..=1.0).contains(&f2) || !(0.0..=1.0).contains(&f3) {
            return None;
        }
        let row = (f3 * -128.0) as i32;
        let col = (f2 * 128.0) as i32;
        let cell = usize::try_from(col - row * 128).ok()?;
        match cont.zone_grid.get(cell).copied().unwrap_or(0) {
            0 => None,
            z => Some(z),
        }
    }

    /// The revealed overlays of the displayed zone (`0x546310`'s second half): only at zone
    /// level and with a player, any area slot revealing.
    fn overlays(&self) -> Vec<&WorldMapOverlayView> {
        let s = self.selection;
        if self.feed.player.is_none() || s.continent < 0 || s.zone < 0 {
            return Vec::new();
        }
        let Some(zone) = self
            .continent(s.continent)
            .and_then(|c| c.zones.get(s.zone as usize))
        else {
            return Vec::new();
        };
        zone.overlays
            .iter()
            .filter(|o| {
                o.always_revealed
                    || o.explore_bits
                        .iter()
                        .any(|&b| explored_bit(&self.explored, b))
            })
            .collect()
    }

    /// `0x5445f0`'s level gates on a POI's `Flags`.
    fn poi_level_passes(&self, poi: &WorldMapPoiView) -> bool {
        let s = self.selection;
        let f = poi.flags;
        let zone_or_direct = !((s.continent < 0 || s.zone < 0) && s.continent != -2);
        if zone_or_direct && f & 0x04 == 0 {
            return false;
        }
        if s.continent == -1 && f & 0x10 == 0 {
            return false;
        }
        if s.zone < 0 && s.direct < 0 && f & 0x08 == 0 {
            return false;
        }
        if f & 0x400 != 0 {
            let zone = self
                .continent(s.continent)
                .and_then(|c| usize::try_from(s.zone).ok().and_then(|z| c.zones.get(z)));
            if !zone.is_some_and(|z| z.area_id == poi.area_id && z.map_id == poi.pos.map) {
                return false;
            }
        }
        true
    }

    /// The landmark list (`0x546310`'s first half): each `AreaPOI` row past its level, map,
    /// exploration and world-state gates, then the guard marker, ungated.
    fn rebuild_landmarks(&mut self) {
        let degenerate = |(u, v): (f32, f32)| u.abs() < 2.384_185_8e-7 && v.abs() < 2.384_185_8e-7;
        let has_player = self.feed.player.is_some();
        let mut landmarks = Vec::new();
        for (i, poi) in self.catalog.pois.iter().enumerate() {
            let uv = if self.poi_level_passes(poi) {
                match self.project(poi.pos) {
                    (uv, false) => uv,
                    (_, true) => (0.0, 0.0),
                }
            } else {
                (0.0, 0.0)
            };
            if degenerate(uv) {
                continue;
            }
            let explored = match poi.explore {
                WorldMapExploreGate::Open => true,
                WorldMapExploreGate::Never => false,
                WorldMapExploreGate::Free => has_player,
                WorldMapExploreGate::Bit(b) => has_player && explored_bit(&self.explored, b),
            };
            let state = poi.world_state_id == 0
                || self
                    .world_states
                    .get(&poi.world_state_id)
                    .copied()
                    .unwrap_or(0)
                    != 0;
            if explored && state {
                landmarks.push(Landmark::Poi(i, uv));
            }
        }
        if let Some(marker) = &self.marker {
            if let (uv, false) = self.project(marker.pos) {
                if !degenerate(uv) {
                    landmarks.push(Landmark::Marker(uv));
                }
            }
        }
        self.landmarks = landmarks;
    }

    /// The setter `0x546310`: a zone with a default floor opens as its row; then the landmarks.
    /// The caller fires `WORLD_MAP_UPDATE`.
    fn set_map(&mut self, mut c: i32, mut z: i32, mut d: i32) {
        let s = &mut self.selection;
        if let Some(cont) = usize::try_from(c)
            .ok()
            .and_then(|i| self.catalog.continents.get(i))
        {
            let zone = usize::try_from(z).ok().and_then(|i| cont.zones.get(i));
            s.zone = if zone.is_some() { z } else { -1 };
            s.direct = -1;
            s.dungeon_map = -1;
            match zone.filter(|r| r.default_dungeon_floor >= 1) {
                None => {
                    s.continent = c;
                    self.rebuild_landmarks();
                    return;
                }
                Some(row) => {
                    d = row.default_dungeon_floor;
                    z = row.id as i32;
                    c = -2;
                }
            }
        }
        s.zone = -1;
        s.dungeon_map = d;
        s.direct = if c == -2 { z } else { -1 };
        s.continent = c;
        self.rebuild_landmarks();
    }

    /// `0x50f490`, the refresh the data edges run: the setter over the current cells.
    fn refresh(&mut self) {
        let s = self.selection;
        if s.direct == -1 {
            self.set_map(s.continent, s.zone, -1);
        } else {
            self.set_map(s.continent, s.direct, s.dungeon_map);
        }
    }

    /// `CGWorldMap::FindCurrentZone` (`0x547170`): the player's continent and zone, else the row
    /// of the instance map, else the zone holding the instance's entrance; `None` is the cosmic
    /// sheet. `DungeonMapChunk` is not read, so no position resolves to a floor.
    fn current_zone(&self) -> (i32, i32, i32) {
        let Some(player) = self.feed.player else {
            return (-1, -1, -1);
        };
        let (t, t_dungeon) = self.transform(player.pos);
        let found = self
            .catalog
            .continents
            .iter()
            .position(|c| c.map_id == t.map);
        let Some(ci) = found else {
            let dungeon = if t_dungeon != 0 { t_dungeon as i32 } else { -1 };
            if let Some(orphan) = self
                .catalog
                .orphans
                .iter()
                .find(|o| o.map_id == player.pos.map)
            {
                return (-2, orphan.id as i32, dungeon);
            }
            if let Some(&entrance) = self
                .catalog
                .entrances
                .get(&t.map)
                .filter(|e| has_rect((e.x, e.y, 0.0, 0.0)))
            {
                let (e, _) = self.transform(entrance);
                if let Some(ci) = self
                    .catalog
                    .continents
                    .iter()
                    .position(|c| c.map_id == e.map)
                {
                    let fx = 0.5 - e.x * GRID_K;
                    let fy = 0.5 - e.y * GRID_K;
                    if (0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy) {
                        let cell = (fy * 128.0) as i32 - (fx * -128.0) as i32 * 128;
                        let zi = usize::try_from(cell)
                            .ok()
                            .and_then(|i| self.catalog.continents[ci].zone_grid.get(i))
                            .copied()
                            .unwrap_or(0);
                        if zi != 0 {
                            return (ci as i32, i32::from(zi) - 1, -1);
                        }
                    }
                    return (ci as i32, self.zone_of(ci, player.zone), -1);
                }
            }
            return (-2, -1, -1);
        };
        (ci as i32, self.zone_of(ci, player.zone), -1)
    }

    /// `0x547170`'s zone walk: the row of the player's zone with a rect, else a rectless row's
    /// parent; -1 for the continent map.
    fn zone_of(&self, ci: usize, area: u32) -> i32 {
        let zones = &self.catalog.continents[ci].zones;
        let mut found = -1;
        for z in zones.iter().filter(|z| z.area_id == area) {
            if z.has_rect() {
                return zones
                    .iter()
                    .position(|r| r.id == z.id)
                    .map_or(-1, |i| i as i32);
            }
            if z.parent_world_map_id != 0 {
                if let Some(i) = zones.iter().position(|r| r.id == z.parent_world_map_id) {
                    found = i as i32;
                }
            }
        }
        found
    }

    /// The overlay pre-search (`0x545110`): a revealed overlay whose hit rect holds the cursor,
    /// edges inclusive, names its first area; a nameless one is walked past.
    fn overlay_hover(&self, x: f32, y: f32) -> Option<String> {
        self.overlays().into_iter().find_map(|o| {
            let (top, left, bottom, right) = o.hit_rect;
            let (xlo, xhi) = (left as f32 * 0.000_998_004, right as f32 * 0.000_998_004);
            let (ylo, yhi) = (top as f32 * 0.001_497_006, bottom as f32 * 0.001_497_006);
            let inside = xhi - xlo != 0.0
                && yhi - ylo != 0.0
                && (xlo..=xhi).contains(&x)
                && (ylo..=yhi).contains(&y);
            if inside {
                o.area_name.clone()
            } else {
                None
            }
        })
    }

    /// `0x544420`: the row under the cursor; a continent's on the Azeroth sheet, a zone's through
    /// the grid, never the displayed zone itself.
    fn hover_row(&self, x: f32, y: f32) -> Option<WorldMapZoneView> {
        let s = self.selection;
        match s.continent {
            -2 => None,
            -1 => self
                .catalog
                .continents
                .iter()
                .find(|c| {
                    c.sheet.is_some_and(|sh| {
                        let (u0, v0, u1, v1) = sh.rect();
                        sh.clickable && (u0..=u1).contains(&x) && (v0..=v1).contains(&y)
                    })
                })
                .map(WorldMapContinentView::row),
            c => {
                let cont = self.continent(c)?;
                let zi = self.grid_zone(x, y)?;
                if s.zone >= 0 && i32::from(zi) - 1 == s.zone {
                    return None;
                }
                cont.zones.get(zi as usize - 1).cloned()
            }
        }
    }

    /// `UpdateMapHighlight`'s answer (`0x545110`).
    fn highlight(&self, x: f32, y: f32) -> Hover {
        if self.selection.zone >= 0 {
            if let Some(name) = self.overlay_hover(x, y) {
                return Hover::Name(Some(name));
            }
        }
        let Some(row) = self.hover_row(x, y) else {
            return Hover::Miss;
        };
        if self.selection.zone >= 0 {
            return Hover::Name(Some(row.name).filter(|n| !n.is_empty()));
        }
        if !row.has_rect() {
            return Hover::Miss;
        }
        let name = Some(row.name.clone()).filter(|n| !n.is_empty());
        let file = row.map_file.clone();
        let continent_row = row.area_id == 0;
        let sheet = continent_row
            .then(|| {
                self.catalog
                    .continents
                    .iter()
                    .find(|c| c.map_id == row.map_id)
                    .and_then(|c| c.sheet)
            })
            .flatten();
        let (w, h, dim) = if continent_row {
            match sheet {
                Some(sh) => {
                    let w = (sh.tiles.1 as i64 - sh.tiles.0 as i64 + 1) as f32;
                    let h = (sh.tiles.3 as i64 - sh.tiles.2 as i64 + 1) as f32;
                    (w, h, ((h * 512.0) / w) as i64 as u32)
                }
                None => (64.0, 64.0, 0x80),
            }
        } else {
            let (l, r, t, b) = row.loc_rect;
            let (w, h) = (l - r, t - b);
            (w, h, ((h * 128.0) / w) as i64 as u32)
        };
        let pot = highlight_pot(dim);
        if continent_row {
            let (tpx, tpy) = if w <= h { (w / h, 1.0) } else { (1.0, h / w) };
            let geometry = sheet.map(|sh| {
                let s = sh.scale;
                let sx = (31.3125 - s * 32.0) + sh.tiles.0 as f32 * s + sh.offset.0;
                let sy = sh.tiles.2 as f32 * s + (20.875 - s * 32.0) + sh.offset.1;
                [
                    f64::from(s * w * SHEET_U),
                    f64::from(s * h * SHEET_V),
                    f64::from(sx * SHEET_U),
                    f64::from(sy * SHEET_V),
                ]
            });
            return Hover::Highlight {
                name,
                file,
                tex_pct: (f64::from(tpx), f64::from(tpy)),
                geometry: geometry.unwrap_or_default(),
            };
        }
        let tex_pct_y = f64::from(dim as f32 / pot as f32);
        let shown = row.shown_map();
        let geometry = self
            .catalog
            .continents
            .iter()
            .find(|c| c.map_id == shown)
            .map(|c| {
                let (cl, cr, ct, cb) = c.loc_rect;
                let (zl, _, zt, _) = row.loc_rect;
                let (zt, zl) = if row.display_map_id < 0 {
                    (zt, zl)
                } else {
                    let (p, _) = self.transform(WorldMapPoint {
                        map: row.map_id,
                        x: zt,
                        y: zl,
                    });
                    (p.x, p.y)
                };
                let rx = 1.0 / (cl - cr);
                let ry = 1.0 / (ct - cb);
                [
                    f64::from(rx * w),
                    f64::from(ry * h),
                    f64::from(rx * (cl - zl)),
                    f64::from(ry) * f64::from(ct - zt),
                ]
            })
            .unwrap_or_default();
        Hover::Highlight {
            name,
            file,
            tex_pct: (1.0, tex_pct_y),
            geometry,
        }
    }
}

/// `0x545110`'s texture-height round-up: the low byte to a power of two, the high bits kept past
/// 256 with one more page.
fn highlight_pot(dim: u32) -> u32 {
    let low = dim & 0xff;
    if low == 0 {
        return dim;
    }
    let mut pot = low;
    if low & (low - 1) != 0 {
        pot = 1 << (32 - low.leading_zeros());
    }
    if dim > 0x100 {
        pot += 0x100 + (dim & !0xff);
    }
    pot
}

/// `GetMapInfo`'s second and third values (`0x543cd0`, `0x544ca0`): the art's height at 1002
/// wide, and that with its low byte rounded up to a power of two.
fn texture_height(wm: &WorldMapState) -> (u32, u32) {
    let Some(row) = wm.row(wm.current_id()) else {
        return (0, 0);
    };
    let (w, h) = if let Some(d) = wm.dungeon(wm.current_dungeon()) {
        (d.y.1 - d.y.0, d.x.1 - d.x.0)
    } else if wm.selection.zone == -1 {
        match wm
            .catalog
            .continents
            .iter()
            .find(|c| c.map_id == row.map_id)
            .and_then(|c| c.sheet)
        {
            Some(sh) => (
                (sh.tiles.1 as i64 - sh.tiles.0 as i64 + 1) as f32,
                (sh.tiles.3 as i64 - sh.tiles.2 as i64 + 1) as f32,
            ),
            None => return (0, 0),
        }
    } else {
        let (l, r, t, b) = row.loc_rect;
        (l - r, t - b)
    };
    if w == 0.0 || h == 0.0 {
        return (0, 0);
    }
    let height = ((h * 1002.0) / w) as i64 as u32;
    let mut low = height & 0xff;
    if low & low.wrapping_sub(1) != 0 {
        low = 1 << (32 - low.leading_zeros());
    }
    (height, (height & !0xff) + low)
}

/// What `UpdateMapHighlight` answers.
enum Hover {
    Highlight {
        name: Option<String>,
        file: String,
        tex_pct: (f64, f64),
        geometry: [f64; 4],
    },
    Name(Option<String>),
    Miss,
}

impl super::UiScript {
    /// Pushes the catalog before any interface file runs; the selection is re-applied over it.
    pub fn set_world_map_catalog(&mut self, catalog: WorldMapCatalog) {
        let mut model = self.model_mut();
        model.worldmap.catalog = catalog;
        model.worldmap.refresh();
        queue_update(&mut model);
    }

    /// Pushes the discovery bitset; a change refreshes the map (`0x547120`'s field callback).
    pub fn set_world_map_explored(&mut self, explored: Vec<u32>) {
        let mut model = self.model_mut();
        if model.worldmap.explored != explored {
            model.worldmap.explored = explored;
            model.worldmap.refresh();
            queue_update(&mut model);
        }
    }

    /// Pushes the world-state values the POI gates and icons read; a change refreshes the map.
    pub fn set_world_map_world_states(&mut self, states: HashMap<u32, i32>) {
        let mut model = self.model_mut();
        if model.worldmap.world_states != states {
            model.worldmap.world_states = states;
            model.worldmap.refresh();
            queue_update(&mut model);
        }
    }

    /// Pushes the guard-directions marker; a change refreshes the map.
    pub fn set_world_map_gossip_poi(&mut self, marker: Option<WorldMapPoiView>) {
        let mut model = self.model_mut();
        if model.worldmap.marker != marker {
            model.worldmap.marker = marker;
            model.worldmap.refresh();
            queue_update(&mut model);
        }
    }

    /// Pushes the per-frame world positions; the player's arrival or departure re-runs the
    /// landmark gates, which need a player.
    pub fn set_world_map_feed(&mut self, feed: WorldMapFeed) {
        if self.model_ref().worldmap.feed == feed {
            return;
        }
        let mut model = self.model_mut();
        let had_player = model.worldmap.feed.player.is_some();
        model.worldmap.feed = feed;
        if had_player != model.worldmap.feed.player.is_some() {
            model.worldmap.rebuild_landmarks();
        }
    }

    /// The four selection cells.
    pub fn world_map_selection(&self) -> WorldMapSelection {
        self.model_ref().worldmap.selection
    }

    /// A world position on the displayed map, `(0, 0)` off it, as `GetPlayerMapPosition` answers.
    pub fn world_map_project(&self, p: WorldMapPoint) -> (f32, f32) {
        self.model_ref().worldmap.blip(Some(p))
    }

    /// The displayed row's world rect `(map, left, right, top, bottom)`, for a click's way back.
    pub fn world_map_displayed_rect(&self) -> Option<(u32, (f32, f32, f32, f32))> {
        let model = self.model_ref();
        let wm = &model.worldmap;
        if wm.selection.continent == -1 {
            return None;
        }
        wm.row(wm.current_id()).map(|r| (r.map_id, r.loc_rect))
    }

    /// The zone updater's first-entry call (`0x5204f9` into `0x547170`): the player's map before
    /// any Lua asks. Queued, as an engine edge.
    pub fn sync_world_map_to_player_zone(&mut self) {
        let mut model = self.model_mut();
        let (c, z, d) = model.worldmap.current_zone();
        model.worldmap.set_map(c, z, d);
        queue_update(&mut model);
    }

    /// The map UV at UI point `(x, y)`, or `None` off the art, normalized over `WorldMapButton`
    /// as `WorldMapButton_OnClick` does. A pure query for the app's dev map-jump.
    pub fn world_map_uv_at(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let id = self.hit_test(x, y)?;
        let model = self.model_ref();
        let hit = model.id_to_frame.get(&id).copied()?;
        let mut fh = hit;
        while model.arena.frame(fh)?.name.as_deref() != Some("WorldMapButton") {
            fh = model.arena.frame(fh)?.parent?;
        }
        let r = model.resolved.get(&fh)?;
        let (w, h) = (r.right - r.left, r.top - r.bottom);
        (w > 0.0 && h > 0.0).then(|| ((x - r.left) / w, (r.top - y) / h))
    }
}

fn queue_update(model: &mut Model) {
    model
        .pending_events
        .push(("WORLD_MAP_UPDATE".to_string(), Vec::new()));
}

/// Runs the setter from a Lua verb and fires `WORLD_MAP_UPDATE` before the verb returns, as
/// `0x546310` signals synchronously.
fn set_map_now(lua: &Lua, c: i32, z: i32, d: i32) {
    crate::script::model_mut_of(lua).worldmap.set_map(c, z, d);
    super::tick::fire_event_into(lua, "WORLD_MAP_UPDATE", Vec::new());
}

impl WorldMapState {
    /// A world state's value as last pushed, 0 for one never seen (`GetWorldState`).
    pub(crate) fn world_state(&self, id: u32) -> i32 {
        self.world_states.get(&id).copied().unwrap_or(0)
    }
}

fn with_map<R>(lua: &Lua, f: impl FnOnce(&WorldMapState) -> R) -> R {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    f(&model.worldmap)
}

fn pair(uv: (f32, f32)) -> (f64, f64) {
    (f64::from(uv.0), f64::from(uv.1))
}

/// The `fistp`-rounded index argument (`0x544b90`, `0x545a30`, `0x545c80`).
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(n.round_ties_even() as i64),
        None => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

fn string(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

/// Register the world-map globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetMapContinents() → each continent's `Map.dbc` name, in catalog order (`0x544b20`).
    g.set(
        "GetMapContinents",
        lua.create_function(|lua, ()| {
            let names = with_map(lua, |wm| {
                wm.catalog
                    .continents
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
            });
            names
                .iter()
                .map(|n| string(lua, n))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;

    // GetMapZones(continent) → the zone names; the bound is unsigned, so 0 or a negative answers
    // none (`0x544b90`).
    g.set(
        "GetMapZones",
        lua.create_function(|lua, c: Value| {
            let c = index_arg(lua, c, "Usage: GetMapZones(continentIndex)")?;
            let names = with_map(lua, |wm| {
                wm.continent((c - 1) as i32)
                    .map(|cont| cont.zones.iter().map(|z| z.name.clone()).collect())
                    .unwrap_or_else(Vec::new)
            });
            names
                .iter()
                .map(|n| string(lua, n))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;

    // GetCurrentMapContinent() (`0x544d40`): a selected floor's continent, else the cell plus one,
    // so the Azeroth sheet is 0 and a direct row or the cosmic sheet -1.
    g.set(
        "GetCurrentMapContinent",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                wm.dungeon(wm.current_dungeon())
                    .and_then(|d| {
                        wm.catalog
                            .continents
                            .iter()
                            .position(|c| c.map_id == d.map_id)
                    })
                    .map_or(i64::from(wm.selection.continent) + 1, |i| i as i64 + 1)
            }))
        })?,
    )?;

    // GetCurrentMapZone() (`0x544e80`): with no zone cell, the displayed row's place among the
    // zones of the continent on its map, else 0; otherwise the cell plus one.
    g.set(
        "GetCurrentMapZone",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                let s = wm.selection;
                if s.zone >= 0 {
                    return i64::from(s.zone) + 1;
                }
                let id = match wm.continent(s.continent) {
                    Some(c) => c.id as i32,
                    None => s.direct,
                };
                wm.row(id)
                    .and_then(|row| {
                        wm.catalog
                            .continents
                            .iter()
                            .filter(|c| c.map_id == row.map_id)
                            .find_map(|c| c.zones.iter().position(|z| z.id == row.id))
                    })
                    .map_or(0, |i| i as i64 + 1)
            }))
        })?,
    )?;

    // GetCurrentMapAreaID() (`0x544e10`): the displayed row's id plus one, 0 for none.
    g.set(
        "GetCurrentMapAreaID",
        lua.create_function(|lua, ()| Ok(with_map(lua, |wm| i64::from(wm.current_id()) + 1)))?,
    )?;

    // SetMapZoom(continent [, zone]) (`0x5469e0`): both truncated and less one; a continent past
    // the catalog or below the cosmic sheet raises.
    g.set(
        "SetMapZoom",
        lua.create_function(|lua, (c, z): (Value, Value)| {
            const USAGE: &str = "Usage: SetMapZoom(continentIndex [,zoneIndex])";
            let c = number_arg(lua, c, USAGE)?.wrapping_sub(1);
            let n = with_map(lua, |wm| wm.catalog.continents.len());
            if !(c < 1 || (c as u32 as usize) < n) || c < -2 {
                return Err(mlua::Error::RuntimeError(format!(
                    "{USAGE} .. requested continent out of bounds"
                )));
            }
            let z = match lua.coerce_number(z)? {
                Some(z) => (z as i64 as i32).wrapping_sub(1),
                None => -1,
            };
            set_map_now(lua, c, z, -1);
            Ok(())
        })?,
    )?;

    // SetMapByID(id) (`0x546c50`): a raw `WorldMapArea` id, the one `GetCurrentMapAreaID`
    // answers less one. A row on a continent's map selects through it; else its floor.
    g.set(
        "SetMapByID",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: SetMapByID(mapID)")?;
            let target = with_map(lua, |wm| {
                let row = wm.row(id)?;
                for (ci, c) in wm.catalog.continents.iter().enumerate() {
                    if c.map_id != row.shown_map() {
                        continue;
                    }
                    if row.default_dungeon_floor > 0 {
                        return Some((-2, row.id as i32, row.default_dungeon_floor));
                    }
                    if row.area_id == 0 {
                        return Some((ci as i32, -1, -1));
                    }
                    if let Some(zi) = c.zones.iter().position(|z| z.id == row.id) {
                        return Some((ci as i32, zi as i32, -1));
                    }
                }
                if row.default_dungeon_floor == -1 {
                    return Some((-2, id, -1));
                }
                let first_floor = wm
                    .catalog
                    .dungeon_maps
                    .iter()
                    .find(|d| d.map_id == row.map_id && d.floor_index == 1);
                Some((-2, id, first_floor.map_or(-1, |d| d.id as i32)))
            });
            match target {
                Some((c, z, d)) => {
                    set_map_now(lua, c, z, d);
                    Ok(Some(true))
                }
                None => Ok(None),
            }
        })?,
    )?;

    // ZoomOut() (`0x546a90`): from a floor or a direct row to its parent zone.
    g.set(
        "ZoomOut",
        lua.create_function(|lua, ()| {
            let target = with_map(lua, |wm| {
                let s = wm.selection;
                let parent = if s.direct == -1 || s.dungeon_map < 1 {
                    if s.continent != -2 {
                        return None;
                    }
                    let row = wm.row(wm.current_id())?;
                    let mut parent = row.parent_world_map_id;
                    if parent == 0 {
                        for d in &wm.catalog.dungeon_maps {
                            if d.map_id == row.map_id && d.floor_index == 1 {
                                parent = d.parent_world_map_id;
                            }
                        }
                    }
                    parent
                } else {
                    wm.dungeon(s.dungeon_map)?.parent_world_map_id
                };
                let prow = wm.row(parent as i32)?;
                wm.catalog
                    .continents
                    .iter()
                    .enumerate()
                    .find_map(|(ci, c)| {
                        (c.map_id == prow.map_id)
                            .then(|| c.zones.iter().position(|z| z.id == prow.id))
                            .flatten()
                            .map(|zi| (ci as i32, zi as i32))
                    })
            });
            if let Some((c, z)) = target {
                set_map_now(lua, c, z, -1);
            }
            Ok(())
        })?,
    )?;

    // IsZoomOutAvailable() (`0x545050`): false only on a direct row with no floor, no parent and
    // no terrain floor, the cosmic sheet included.
    g.set(
        "IsZoomOutAvailable",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                if wm.dungeon(wm.current_dungeon()).is_some() || wm.selection.continent != -2 {
                    return true;
                }
                match wm.row(wm.current_id()) {
                    None => false,
                    Some(r) if r.default_dungeon_floor == -1 => true,
                    Some(r) => wm.row(r.parent_world_map_id as i32).is_some(),
                }
            }))
        })?,
    )?;

    // SetMapToCurrentZone() (`0x547c10`): the resolver into the setter.
    g.set(
        "SetMapToCurrentZone",
        lua.create_function(|lua, ()| {
            let (c, z, d) = with_map(lua, WorldMapState::current_zone);
            set_map_now(lua, c, z, d);
            Ok(())
        })?,
    )?;

    // GetCurrentMapDungeonLevel() (`0x544fc0`): the floor's index, plus one on a row whose floor
    // 0 is the terrain.
    g.set(
        "GetCurrentMapDungeonLevel",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                let level = wm
                    .dungeon(wm.current_dungeon())
                    .map_or(0, |d| d.floor_index);
                let terrain = wm
                    .row(wm.selection.direct)
                    .is_some_and(|r| r.default_dungeon_floor == -1);
                i64::from(level) + i64::from(terrain)
            }))
        })?,
    )?;

    // GetNumDungeonMapLevels() (`0x546190`): the distinct floors on the direct row's map, plus
    // its terrain floor.
    g.set(
        "GetNumDungeonMapLevels",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                let Some(row) = wm.row(wm.selection.direct) else {
                    return 0;
                };
                if wm.selection.continent >= 0 && wm.current_id() != wm.selection.direct {
                    return 0;
                }
                let mut seen = 0u32;
                let mut n = 0i64;
                for d in wm
                    .catalog
                    .dungeon_maps
                    .iter()
                    .filter(|d| d.map_id == row.map_id)
                {
                    let bit = 1u32.wrapping_shl(d.floor_index as u32);
                    if seen & bit == 0 {
                        seen |= bit;
                        n += 1;
                    }
                }
                n + i64::from(row.default_dungeon_floor == -1)
            }))
        })?,
    )?;

    // SetDungeonMapLevel(level) (`0x547b80`, `0x547a70`): level 0 is the terrain floor where the
    // row has one; else the run of the map's rows holding that floor index.
    g.set(
        "SetDungeonMapLevel",
        lua.create_function(|lua, level: Value| {
            let level = number_arg(lua, level, "Usage: SetDungeonMapLevel(level)")?;
            let target = with_map(lua, |wm| {
                let s = wm.selection;
                let row = wm.row(s.direct)?;
                let level = if row.default_dungeon_floor == -1 {
                    level - 1
                } else {
                    level
                };
                if level < 0 || (row.default_dungeon_floor != -1 && level < 1) {
                    return None;
                }
                if s.continent >= 0 && wm.current_id() != s.direct {
                    return None;
                }
                if level == 0 {
                    return Some((-2, row.id as i32, -1));
                }
                let mut run = false;
                for d in &wm.catalog.dungeon_maps {
                    if d.map_id == row.map_id {
                        if d.floor_index == level {
                            return (d.id >= 1).then_some((s.continent, s.direct, d.id as i32));
                        }
                        run = true;
                    } else if run {
                        return None;
                    }
                }
                None
            });
            if let Some((c, z, d)) = target {
                set_map_now(lua, c, z, d);
            }
            Ok(())
        })?,
    )?;

    // ClickLandmark(mapLinkID) (`0x546ef0`): a `DungeonMap` row opens on the first orphan of its map.
    g.set(
        "ClickLandmark",
        lua.create_function(|lua, link: Value| {
            let link = index_arg(lua, link, "Usage: ClickLandmark(mapLinkID)")? as i32;
            let target = with_map(lua, |wm| {
                let d = wm.dungeon(link)?;
                wm.catalog
                    .orphans
                    .iter()
                    .find(|o| o.map_id == d.map_id)
                    .map(|o| o.id as i32)
            });
            if let Some(id) = target {
                set_map_now(lua, -2, id, link);
            }
            Ok(())
        })?,
    )?;

    // DungeonUsesTerrainMap() (`0x544c40`): 1 on a direct row whose floor 0 is the terrain.
    g.set(
        "DungeonUsesTerrainMap",
        lua.create_function(|lua, ()| {
            Ok(with_map(lua, |wm| {
                wm.row(wm.selection.direct)
                    .is_some_and(|r| r.default_dungeon_floor == -1)
                    .then_some(1)
            }))
        })?,
    )?;

    // GetMapInfo() → the art folder (nil on the Azeroth and cosmic sheets) and the art height
    // twice, the second rounded (`0x544ca0`).
    g.set(
        "GetMapInfo",
        lua.create_function(|lua, ()| {
            let (file, (h, hpot)) = with_map(lua, |wm| {
                (
                    wm.row(wm.current_id()).map(|r| r.map_file),
                    texture_height(wm),
                )
            });
            let file = match file {
                Some(f) => string(lua, &f)?,
                None => Value::Nil,
            };
            Ok((file, i64::from(h), i64::from(hpot)))
        })?,
    )?;

    // GetPlayerMapPosition(unit) (`0x545880`): the player or a group member, projected now.
    g.set(
        "GetPlayerMapPosition",
        lua.create_function(|lua, unit: Value| {
            let unit = super::binding_abi::string_arg(
                lua,
                unit,
                "Usage: GetPlayerMapPosition(\"player\")",
            )?;
            Ok(pair(with_map(lua, |wm| {
                let slot = |prefix: &str, list: &[Option<WorldMapPoint>]| {
                    unit.strip_prefix(prefix)
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|n| *n >= 1)
                        .and_then(|n| list.get(n - 1).copied().flatten())
                };
                let pos = match unit.to_ascii_lowercase().as_str() {
                    "player" => wm.feed.player.map(|p| p.pos),
                    u if u.starts_with("party") => slot("party", &wm.feed.party),
                    u if u.starts_with("raid") => slot("raid", &wm.feed.raid),
                    _ => None,
                };
                wm.blip(pos)
            })))
        })?,
    )?;

    // GetCorpseMapPosition() (`0x545950`): projected without the Outland hide.
    g.set(
        "GetCorpseMapPosition",
        lua.create_function(|lua, ()| {
            Ok(pair(with_map(lua, |wm| {
                wm.feed.corpse.map_or((0.0, 0.0), |p| wm.project(p).0)
            })))
        })?,
    )?;

    // GetDeathReleasePosition() (`0x5459c0`): the release graveyard `0x512ca0` keeps, which no
    // packet fills here yet, so the zeroed outputs.
    g.set(
        "GetDeathReleasePosition",
        lua.create_function(|_, ()| Ok((0.0f64, 0.0f64)))?,
    )?;

    // ProcessMapClick(x, y) (`0x5468f0`): a continent on the Azeroth sheet, a zone through the
    // grid, nothing on a direct row.
    g.set(
        "ProcessMapClick",
        lua.create_function(|lua, (x, y): (Value, Value)| {
            const USAGE: &str = "Usage: ProcessMapClick(x, y)";
            let (Some(x), Some(y)) = (lua.coerce_number(x)?, lua.coerce_number(y)?) else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let (x, y) = (x as f32, y as f32);
            let target = with_map(lua, |wm| match wm.selection.continent {
                -2 => None,
                -1 => wm
                    .catalog
                    .continents
                    .iter()
                    .position(|c| {
                        c.sheet.is_some_and(|sh| {
                            let (u0, v0, u1, v1) = sh.rect();
                            sh.clickable && (u0..=u1).contains(&x) && (v0..=v1).contains(&y)
                        })
                    })
                    .map(|i| (i as i32, -1)),
                c => wm.grid_zone(x, y).map(|zi| (c, i32::from(zi) - 1)),
            });
            if let Some((c, z)) = target {
                set_map_now(lua, c, z, -1);
            }
            Ok(())
        })?,
    )?;

    // UpdateMapHighlight(x, y) → name, fileName, texPctX, texPctY, textureX, textureY,
    // scrollChildX, scrollChildY (`0x545110`).
    g.set(
        "UpdateMapHighlight",
        lua.create_function(|lua, (x, y): (Value, Value)| {
            const USAGE: &str = "Usage: UpdateMapHighlight(x, y)";
            let (Some(x), Some(y)) = (lua.coerce_number(x)?, lua.coerce_number(y)?) else {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            };
            let hit = with_map(lua, |wm| wm.highlight(x as f32, y as f32));
            let opt = |s: Option<String>| match s {
                Some(s) => string(lua, &s),
                None => Ok(Value::Nil),
            };
            let n = |v: f64| Value::Number(v);
            let values = match hit {
                Hover::Highlight {
                    name,
                    file,
                    tex_pct,
                    geometry,
                } => vec![
                    opt(name)?,
                    string(lua, &file)?,
                    n(tex_pct.0),
                    n(tex_pct.1),
                    n(geometry[0]),
                    n(geometry[1]),
                    n(geometry[2]),
                    n(geometry[3]),
                ],
                Hover::Name(name) => {
                    let mut v = vec![opt(name)?, Value::Nil];
                    v.extend(std::iter::repeat_n(n(0.0), 6));
                    v
                }
                Hover::Miss => {
                    let mut v = vec![Value::Nil, Value::Nil];
                    v.extend(std::iter::repeat_n(n(0.0), 6));
                    v
                }
            };
            Ok(MultiValue::from_vec(values))
        })?,
    )?;

    g.set(
        "GetNumMapLandmarks",
        lua.create_function(|lua, ()| Ok(with_map(lua, |wm| wm.landmarks.len() as i64)))?,
    )?;

    // GetMapLandmarkInfo(i) → name, description, textureIndex, x, y, mapLinkID,
    // showInBattleMap (`0x545a30`). The icon is the row's `Icon[worldState - 1]` on a marker, a
    // `Flags & 0x80` row or the continent map, else 0.
    g.set(
        "GetMapLandmarkInfo",
        lua.create_function(|lua, i: Value| {
            let i = index_arg(lua, i, "Usage: GetMapLandmarkInfo(index)")?;
            let info = with_map(lua, |wm| {
                let slot = usize::try_from(i - 1).ok()?;
                let (poi, uv, marker) = match *wm.landmarks.get(slot)? {
                    Landmark::Poi(p, uv) => (wm.catalog.pois.get(p)?.clone(), uv, false),
                    Landmark::Marker(uv) => (wm.marker.clone()?, uv, true),
                };
                let s = wm.selection;
                let icon = if marker || poi.flags & 0x80 != 0 || (s.zone == -1 && s.continent != -2)
                {
                    let state = match poi.world_state_id {
                        0 => 0,
                        id => {
                            let v = wm.world_states.get(&id).copied().unwrap_or(0) - 1;
                            usize::try_from(v).ok().filter(|&v| v <= 8).unwrap_or(0)
                        }
                    };
                    poi.icons[state]
                } else {
                    0
                };
                Some((poi, uv, icon))
            });
            let Some((poi, uv, icon)) = info else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            let description = match poi.description.is_empty() {
                true => Value::Nil,
                false => string(lua, &poi.description)?,
            };
            Ok(MultiValue::from_vec(vec![
                string(lua, &poi.name)?,
                description,
                Value::Number(f64::from(icon)),
                Value::Number(f64::from(uv.0)),
                Value::Number(f64::from(uv.1)),
                match poi.map_link {
                    0 => Value::Nil,
                    l => Value::Number(f64::from(l)),
                },
                Value::Boolean(poi.flags & 0x200 != 0),
            ]))
        })?,
    )?;

    g.set(
        "GetNumMapOverlays",
        lua.create_function(|lua, ()| Ok(with_map(lua, |wm| wm.overlays().len() as i64)))?,
    )?;

    // GetMapOverlayInfo(i) → textureName, textureWidth, textureHeight, offsetX, offsetY,
    // mapPointX, mapPointY (`0x545c80`).
    g.set(
        "GetMapOverlayInfo",
        lua.create_function(|lua, i: Value| {
            let i = index_arg(lua, i, "Usage: GetMapOverlayInfo(index)")?;
            let overlay = with_map(lua, |wm| {
                usize::try_from(i - 1)
                    .ok()
                    .and_then(|i| wm.overlays().get(i).map(|o| (*o).clone()))
            });
            let n = |v: u32| Value::Number(f64::from(v));
            let Some(o) = overlay else {
                let mut v = vec![Value::Nil];
                v.extend(std::iter::repeat_n(Value::Number(0.0), 6));
                return Ok(MultiValue::from_vec(v));
            };
            Ok(MultiValue::from_vec(vec![
                string(lua, &o.texture)?,
                n(o.width),
                n(o.height),
                n(o.offset_x),
                n(o.offset_y),
                n(o.map_point.0),
                n(o.map_point.1),
            ]))
        })?,
    )?;

    // GetWintergraspWaitTime() (`0x5435a0`): with world state 3801 set, state 4354 less the UI
    // clock, never negative; nothing otherwise. The UI clock is `time()` plus the server offset
    // (`0x548700`), which stays 0 until `SMSG_WORLD_STATE_UI_TIMER_UPDATE` is read.
    g.set(
        "GetWintergraspWaitTime",
        lua.create_function(|lua, ()| {
            let (active, end) = with_map(lua, |wm| {
                let ws = |id| wm.world_states.get(&id).copied().unwrap_or(0);
                (ws(3801) != 0, ws(4354))
            });
            if !active {
                return Ok(MultiValue::new());
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i32);
            Ok(MultiValue::from_vec(vec![Value::Number(f64::from(
                end.wrapping_sub(now).max(0),
            ))]))
        })?,
    )?;

    // CanQueueForWintergrasp() (`0x543600`): world state 4375 set, 1 or nil.
    g.set(
        "CanQueueForWintergrasp",
        lua.create_function(|lua, ()| {
            Ok(super::binding_abi::flag(with_map(lua, |wm| {
                wm.world_state(4375) != 0
            })))
        })?,
    )?;

    // The debug map verbs: `GetNumMapDebugObjects` pushes 0 (`0x608560`); `HasDebugZoneMap`,
    // `GetDebugZoneMap` and `GetMapDebugObjectInfo` share `0x8e5250`, which returns nothing.
    g.set("GetNumMapDebugObjects", lua.create_function(|_, ()| Ok(0))?)?;
    for name in [
        "HasDebugZoneMap",
        "GetDebugZoneMap",
        "GetMapDebugObjectInfo",
    ] {
        g.set(name, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }

    Ok(())
}
