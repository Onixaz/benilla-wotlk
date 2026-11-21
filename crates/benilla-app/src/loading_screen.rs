//! The loading screen: the full-screen splash and progress bar over world entry and cross-map
//! teleports. The art resolves `Map.dbc` → `LoadingScreens.dbc` → BLP for every map kind; the bar
//! is two layers, `Loading-BarBorder` and `Loading-BarFill` (Background, Glow and Glass are never
//! drawn in 1.12).
//!
//! Event-raised and readiness-cleared, as in the reference: it rises at the character pick's
//! `Connected` edge and at `SMSG_TRANSFER_PENDING` (the reference's two entries into the raise's
//! tail) and clears on a per-frame readiness poll. The reference's one blocking stretch
//! (`SMSG_NEW_WORLD`'s handler defers its work onto the deadline heap) also suppresses input, which
//! this client builds explicitly ([`input`]).
//!
//! A destination snap only ends a raise's wait ([`LoadingScreen::snap_landed`]). The screen clears
//! when the destination is scene-presentable ([`WorldLoadProgress`]), never on the body reaching
//! the ground.

use bevy::prelude::*;
use std::collections::HashMap;

use benilla_formats::{
    lit_dots, load_loading_screens, load_taxi_path_nodes, load_taxi_splines,
    load_world_map_continent_catalog, ship_route, DockLegs, LoadingScreenCatalog, ShipRoute,
    TaxiSplines, WorldMapContinentCatalog,
};

use benilla_assets::LockRecover;
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::terrain_stream::WorldLoadProgress;
use benilla_world::world_map::CurrentMap;

/// The cover's input half: while it is up the client takes no input (a source cut in `PreUpdate`).
mod input;
pub(crate) use input::CoverInput;

// The bar, from the loading screen's descriptor table: entry {cx, cy, halfW, halfH}, rect [cx ±
// halfW·0.5] × [cy ± halfH·0.5], fill right edge left + progress·halfW; Border {0.5, 0.075, 0.600,
// 0.050}, Fill {0.5, 0.075, 0.525, 0.025}. Viewport fractions, `cy` measured up from the bottom
// edge (Bevy UI `bottom:`).
const BORDER_LEFT: f32 = 0.200; // 0.5 − 0.600·0.5
const BORDER_WIDTH: f32 = 0.600;
const BORDER_BOTTOM: f32 = 0.050; // 0.075 − 0.050·0.5
const BORDER_HEIGHT: f32 = 0.050;
const FILL_LEFT: f32 = 0.2375; // 0.5 − 0.525·0.5
const FILL_BOTTOM: f32 = 0.0625; // 0.075 − 0.025·0.5
const FILL_HEIGHT: f32 = 0.025;
const FILL_MAX_WIDTH: f32 = 0.525; // halfW; fill width = progress · FILL_MAX_WIDTH

/// The loading art is authored 4:3; a row with the wide flag swaps in a 16:10 companion on a display
/// at least 4:3 + 0.001 wide (`0x409f69`). The art's area keeps its aspect, pillarboxed on a wider
/// window and letterboxed on a narrower one, and the bar and tip sit inside it.
const BACKDROP_ASPECT: f32 = 4.0 / 3.0;
const WIDE_BACKDROP_ASPECT: f32 = 1.6;
const WIDE_WINDOW_THRESHOLD: f32 = BACKDROP_ASPECT + 0.001;

/// Fit the art's area to the window at `aspect`, centred by the root.
fn fit_area(node: &mut Node, aspect: f32) {
    node.width = Val::Vh(100.0 * aspect);
    node.height = Val::Vh(100.0);
    node.max_width = Val::Vw(100.0);
    node.max_height = Val::Vw(100.0 / aspect);
}

/// The wide art's path: `Wide` before the file name's first `.`, or at the end with none
/// (`0x409fb0`).
fn wide_art_path(path: &str) -> String {
    match path.find('.') {
        Some(dot) => format!("{}Wide{}", &path[..dot], &path[dot..]),
        None => format!("{path}Wide"),
    }
}
/// The ship screen's world map: 4×3 tiles of 256 px over a 1002×668 sheet, the last column and row
/// cropped (`0x407c50`), each `Interface\WorldMap\World\World<n>` (`0x40a160`).
const SHEET_W: f32 = 1002.0;
const SHEET_H: f32 = 668.0;
const SHEET_TILE: f32 = 256.0;
/// The route's sprite sheet: dots from its bottom-left quarter, the departure marker from the
/// top-right, the arrival marker from the top-left (`0x408690`, `0x4089c0`).
const SHIP_ELEMENTS: &str = "Interface\\Glues\\LoadingScreens\\DynamicElements";
/// Half extents in the screen's unit square: a dot ±0.01 × ±0.015, a dock marker ±0.02 × ±0.0265.
const DOT_HALF: Vec2 = Vec2::new(0.01, 0.015);
const DOCK_HALF: Vec2 = Vec2::new(0.02, 0.0265);

/// Consecutive fully resident frames before the clear: debounces the post-teleport frame where
/// `ready` reads 0 against a nonzero `total`.
const CLEAR_AFTER_READY_FRAMES: u32 = 3;
/// Once the screen has been up this long, log which term still blocks the clear, every
/// [`WAIT_LOG_EVERY`] seconds.
const WAIT_LOG_AFTER: f32 = 3.0;
const WAIT_LOG_EVERY: f32 = 2.0;
/// How far (yd) a same-map snap must move the body to count as a load. It clears every combat
/// relocation (charge and intercept 25 yd, blink 20, knockbacks under 30), which also ends in a
/// server teleport and must never black out the screen.
const SNAP_LOAD_MIN_YD: f32 = 100.0;

/// Consecutive covered, in-world frames before the cover is on the glass: renders are serial, so
/// at 3 the two frames before have presented.
const COVER_PRESENT_FRAMES: u32 = 3;

/// Whether the entry cover has reached the glass. The first covered frame is the first whose
/// render can draw the cover, so synchronous work on it freezes character select on screen.
/// Counted in `First` so `PreUpdate` and `Update` readers see the same frame's answer.
#[derive(Resource, Default)]
pub(crate) struct EntryCover {
    /// Consecutive covered+in-world frames; reset the moment either goes false.
    frames: u32,
}

impl EntryCover {
    /// Whether the cover has had enough frames to reach the glass; true when no cover is up, since
    /// a consumer would otherwise wait forever (a capture that boots straight in world).
    pub(crate) fn presented(&self) -> bool {
        self.frames == 0 || self.frames >= COVER_PRESENT_FRAMES
    }

    /// A cover is up and not yet presented: a renderer draws nothing but the cover this frame.
    pub(crate) fn owes_a_present(&self) -> bool {
        self.frames > 0 && self.frames < COVER_PRESENT_FRAMES
    }

    /// A cover is up and the client is in world.
    pub(crate) fn covering(&self) -> bool {
        self.frames > 0
    }

    /// Covered frames so far.
    #[cfg(test)]
    pub(crate) fn frames(&self) -> u32 {
        self.frames
    }
}

impl EntryCover {
    /// One frame's count; the system and the tests both go through this.
    pub(crate) fn tick(&mut self, covered: bool) {
        self.frames = if covered {
            self.frames.saturating_add(1)
        } else {
            0
        };
    }
}

/// `First`: advance or reset the covered-frame count.
fn count_entry_cover(
    screen: Res<LoadingScreen>,
    state: Res<State<crate::char_select::ClientState>>,
    mut cover: ResMut<EntryCover>,
) {
    let in_world = *state.get() == crate::char_select::ClientState::InWorld;
    cover.tick(screen.covering() && in_world);
}

/// The `LoadingScreenID` → BLP path table; [`MapCatalogRes`] maps a map to its `LoadingScreenID`.
#[derive(Resource)]
struct LoadingScreenCatalogRes(LoadingScreenCatalog);

/// A texture and its size in pixels.
type SizedImage = (Handle<Image>, UVec2);

/// What the ship screen is built from; absent when a table or a texture is missing, and every
/// transport raise then draws the plain bar.
#[derive(Resource)]
struct ShipRouteData {
    splines: TaxiSplines,
    legs: DockLegs,
    continents: WorldMapContinentCatalog,
    /// The route's sprite sheet and its size in pixels.
    elements: SizedImage,
}

/// A transport raise's outcome, settled by [`pick_ship_route`] before the raise applies it.
#[derive(Debug)]
enum ShipPick {
    /// The route drew: it replaces the art, tip and bar, and the screen keeps no map.
    Route(ShipRoute),
    /// We ride the named transport but its route did not build: any route on the screen goes, and
    /// the plain screen shows the destination's art.
    Failed,
    /// We ride no transport, or another: the plain screen, any route on it kept.
    NotAboard,
}

/// What a raise means for the tip of the day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TipEdge {
    /// The glue→world entry: pick the next row and advance the cursor.
    Pick,
    /// Every other raise: this screen carries no tip.
    Clear,
}

/// Loading-screen state machine (event-raised, readiness-cleared).
#[derive(Resource, Default)]
pub(crate) struct LoadingScreen {
    active: bool,
    /// A raise from an entry edge holds until the destination snap lands, so the old location's
    /// residency cannot clear a screen raised for the new one (at world entry the login vista is
    /// resident well before `SMSG_LOGIN_VERIFY_WORLD`).
    awaiting_snap: bool,
    /// The map this screen's art resolves from, as the reference keeps one, set by every raise;
    /// `None` is its `-1`, no backdrop. Not `CurrentMap`: vmangos can relocate a character inside
    /// `Player::LoadFromDB` (`Player.cpp:15020`), under a live screen. Its writers: the two raises
    /// store a map id, three other sites store `-1`.
    map: Option<u32>,
    /// The reference's texture handle, which latches the art: the draw reads the map id only to
    /// reject `-1`, and skips the resolver while the handle is set. Cleared by [`Self::dismiss`],
    /// never by a raise.
    art_resolved: bool,
    /// Plain black cover with no art or bar, for the world→glue cut; dropped once the state leaves
    /// `InWorld`.
    blackout: bool,
    /// Consecutive presentable frames while active ([`CLEAR_AFTER_READY_FRAMES`]).
    ready_frames: u32,
    /// Bar fill, 0..1, monotonic within a load: the raw residency ratio dips as the view moves.
    displayed: f32,
    /// Decoded backdrop art by BLP path.
    art_cache: HashMap<String, Handle<Image>>,
    /// Which tip the next raise carries, taken by [`crate::game_tip::drive_game_tip`].
    pub(crate) tip_edge: Option<TipEdge>,
    /// Capture only: the clear is skipped while set ([`Self::hold_for_capture`]).
    held: bool,

    /// `Time::elapsed_secs` at the last raise and at the last wait log line.
    active_since: f32,
    last_wait_log: f32,
    /// The avatar's position last frame, to measure a snap (applied in `Input`, a stage earlier).
    last_pos: Option<Vec3>,

    /// The ship route drawn in place of the art, tip and bar (`0x409010`). Only the dismiss and a
    /// failed route clear it: a plain raise onto a live screen keeps it, as the reference's does.
    ship: Option<ShipRoute>,
    /// A transport raise's outcome, from [`pick_ship_route`], taken by the raise.
    ship_pick: Option<ShipPick>,
    /// Bumped whenever `ship` changes, so its dots are rebuilt once.
    ship_revision: u32,
}

impl LoadingScreen {
    /// Whether the opaque cover is up; the world camera renders under it, never behind the glue.
    pub(crate) fn covering(&self) -> bool {
        self.active
    }

    /// An active cover, for tests.
    #[cfg(test)]
    pub(crate) fn test_covering() -> Self {
        Self {
            active: true,
            ..Self::default()
        }
    }

    /// Take the pending raise's tip edge; read once, by [`crate::game_tip::drive_game_tip`].
    pub(crate) fn take_tip_edge(&mut self) -> Option<TipEdge> {
        self.tip_edge.take()
    }

    /// The capture instrument (`WOW_CAPTURE=loading-tip`): raise as the glue→world entry does,
    /// with art, bar and a `Pick` tip, and never clear. `map` latches the art as a raise's does.
    pub(crate) fn hold_for_capture(&mut self, map: Option<u32>) {
        self.active = true;
        self.blackout = false;
        self.awaiting_snap = false;
        self.map = map;
        self.held = true;
        self.tip_edge = Some(TipEdge::Pick);
    }

    /// Raise the screen for a fresh load, its art latched to `map` for its life (a caller with no
    /// announced destination passes the current map). Only an edge that reaches the reference's
    /// transition entries calls this, never a snap ([`Self::snap_landed`]).
    fn raise(&mut self, reason: &str, awaiting_snap: bool, map: Option<u32>, now: f32) {
        self.active = true;
        self.blackout = false;
        self.awaiting_snap = awaiting_snap;
        self.map = map;
        self.ready_frames = 0;
        self.displayed = 0.0; // restart the fill for the new load
        self.active_since = now;
        self.last_wait_log = now;
        info!("loading screen: up ({reason}, map {map:?})");
    }

    /// `SMSG_CHANGEPLAYER_DIFFICULTY_RESULT` 6 (`0x52c57b`): up over the current map, held until
    /// [`Self::release_difficulty_change`].
    pub(crate) fn raise_for_difficulty_change(&mut self, map: Option<u32>, now: f32) {
        self.raise("difficulty change", true, map, now);
    }

    /// Result 9's hide (`0x52c58d`): the hold ends, and the screen clears once the world under the
    /// body is ready, as every clear here does.
    pub(crate) fn release_difficulty_change(&mut self) {
        self.snap_landed(None);
    }

    /// The destination snap (`SMSG_NEW_WORLD` or `SMSG_LOGIN_VERIFY_WORLD`) landed: it ends the
    /// raise's wait and nothing else, not the art, the tip or the bar. The reference's handlers
    /// never touch the screen: the raise has three call sites, all inside the loading screen's own
    /// code. `map` is `None` for a same-map teleport ack.
    fn snap_landed(&mut self, map: Option<u32>) {
        if let (Some(raised), Some(dest)) = (self.map, map) {
            if raised != dest {
                info!("loading screen: snap landed on map {dest} — raised for map {raised}, art holds");
            }
        }
        self.awaiting_snap = false;
    }

    /// A far snap: a screen already up is this snap's own, so the snap only ends its wait. Returns
    /// `true` when no screen is up and the caller should raise one, our backstop for a port with no
    /// announcing edge (the reference's blocking world load needs none).
    #[must_use]
    fn far_snap(&mut self, map: u32) -> bool {
        if !self.active {
            return true;
        }
        self.snap_landed(Some(map));
        false
    }

    /// The dismiss, the only place a tip clears: the tip's only writers in the reference are the
    /// world entry's setter and this.
    fn dismiss(&mut self) {
        self.active = false;
        self.blackout = false;
        self.tip_edge = Some(TipEdge::Clear);
        // The reference clears the texture handle here: the next screen resolves its own art.
        self.art_resolved = false;
        // And the route (`0x409550` → `0x4094e0`).
        self.set_ship(None);
    }

    fn set_ship(&mut self, ship: Option<ShipRoute>) {
        if self.ship.is_some() || ship.is_some() {
            self.ship_revision = self.ship_revision.wrapping_add(1);
        }
        self.ship = ship;
    }

    /// A ridden transport's leg change on this map (`0x40ae30`): up with no snap to wait for,
    /// showing the leg just left. A failed route clears any route and keeps the screen's last map,
    /// as the reference leaves its map id alone here.
    fn raise_for_ship_leg(&mut self, route: Option<ShipRoute>, now: f32) {
        let map = if route.is_some() { None } else { self.map };
        self.raise("ship leg", false, map, now);
        self.set_ship(route);
    }

    /// `SMSG_TRANSFER_PENDING`'s raise (`0x401480`): plain when no transport rides along
    /// (`0x40ab70`), else the transport raise (`0x40ad50`), which keeps no map once its route draws.
    fn raise_for_transfer(&mut self, map: u32, now: f32) {
        match self.ship_pick.take() {
            Some(ShipPick::Route(route)) => {
                // No art resolves, so the area keeps the last screen's aspect, as the reference
                // keeps its wide flag.
                self.raise("transfer pending, ship route", true, None, now);
                self.set_ship(Some(route));
            }
            Some(ShipPick::Failed) => {
                self.raise("transfer pending", true, Some(map), now);
                self.set_ship(None);
            }
            Some(ShipPick::NotAboard) | None => {
                self.raise("transfer pending", true, Some(map), now)
            }
        }
    }
}

// UI entity markers.
#[derive(Component)]
struct LoadingRoot;

#[derive(Component)]
struct LoadingArea;
/// The plain screen's parts (art, tip, bar), hidden as one while a ship route draws.
#[derive(Component)]
struct LoadingPlain;
/// The ship screen: the world map tiles, then the route.
#[derive(Component)]
struct LoadingShip;
/// Disjoint from the root, backdrop and bar queries that share the system.
type NotCoverPart = (
    Without<LoadingRoot>,
    Without<LoadingBackdrop>,
    Without<LoadingBarFill>,
    Without<LoadingBarBorder>,
);
/// One of the route's sprites, the `i`th dot; the dock markers carry `None`.
#[derive(Component)]
struct ShipDot(Option<usize>);
#[derive(Component)]
struct LoadingBackdrop;
#[derive(Component)]
struct LoadingBarFill;
#[derive(Component)]
struct LoadingBarBorder;
/// The tip-of-the-day `Text` root; its coloured runs are children, rebuilt when the tip changes.
#[derive(Component)]
pub(crate) struct LoadingTip;

pub(crate) struct LoadingScreenPlugin;

impl Plugin for LoadingScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadingScreen>()
            .init_resource::<EntryCover>()
            // In `First`, ahead of every consumer in `PreUpdate` and `Update`.
            .add_systems(First, count_entry_cover)
            .add_systems(Startup, setup_loading_screen.after(AssetSet::Open))
            // In `WorldStage::Present`, after Input and Stream: a teleport snaps in Input and the
            // streamer refocuses in Stream, so the cover lands on the same frame, never a flash.
            .add_systems(
                Update,
                (
                    pick_ship_route,
                    follow_ship_leg,
                    drive_loading_screen,
                    draw_ship_route,
                )
                    .chain()
                    .in_set(WorldStage::Present),
            );
        // The cover's input rule, registered here so the two cannot be registered apart.
        input::build(app);
    }
}

/// Startup: load the LoadingScreens catalog and the bar textures, and spawn the hidden UI tree.
fn setup_loading_screen(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    // The black pillarbox root spawns unconditionally: `covering()` consumers act as covered while
    // the screen is active, so missing art must still leave a black cover.
    let root = commands
        .spawn((
            LoadingRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            // UI already paints after the 3D scene; this orders within UI.
            GlobalZIndex(1000),
            Visibility::Hidden,
        ))
        .id();

    let Some(mut assets) = world_assets else {
        return;
    };

    // Catalog (LoadingScreenID → BLP path).
    match load_loading_screens(&mut assets.chain.lock_recover()) {
        Ok(c) => {
            info!("LoadingScreens.dbc: {} screens catalogued", c.len());
            commands.insert_resource(LoadingScreenCatalogRes(c));
        }
        Err(e) => {
            error!("LoadingScreens.dbc unavailable — plain black cover, no art: {e:#}");
            return;
        }
    }

    // The two layers 1.12 draws, as sRGB clamp sprites.
    let mut tex = |path: &str| assets.sprite_texture(path, &mut images);
    let Some(border) = tex("Interface\\Glues\\LoadingBar\\Loading-BarBorder.blp") else {
        error!("loading bar textures missing — plain black cover, no art");
        return;
    };
    let fill = tex("Interface\\Glues\\LoadingBar\\Loading-BarFill.blp").unwrap_or_default();
    let ship = load_ship_route_data(&mut assets, &mut images);
    let ship_tiles = ship.as_ref().map(|(_, tiles)| tiles.clone());
    if let Some((data, _)) = ship {
        commands.insert_resource(data);
    }

    commands.entity(root).with_children(|root| {
        // One area at the art's aspect, fit to the window and centred; the bar's fractions are
        // relative to it. Paint order is spawn order: backdrop, tip, fill, border.
        let mut area = Node {
            position_type: PositionType::Relative,
            ..default()
        };
        fit_area(&mut area, BACKDROP_ASPECT);
        root.spawn((LoadingArea, area)).with_children(|area| {
            area.spawn((LoadingPlain, full_area_node()))
                .with_children(|area| spawn_plain_parts(area, fill, border));
            let mut ship = area.spawn((LoadingShip, full_area_node(), Visibility::Hidden));
            if let Some(tiles) = ship_tiles {
                ship.with_children(|ship| spawn_sheet_tiles(ship, tiles));
            }
        });
    });
}

/// A node filling the loading area.
fn full_area_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// The plain screen: backdrop, tip, fill and border, in paint order.
fn spawn_plain_parts(area: &mut ChildSpawnerCommands, fill: Handle<Image>, border: Handle<Image>) {
    // Tinted black until the art resolves.
    area.spawn((
        LoadingBackdrop,
        ImageNode {
            color: Color::BLACK,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
    ));
    // The tip, between the backdrop and the bar as in the reference, placed in percent of
    // this area, the space of `crate::game_tip`.
    area.spawn((LoadingTip, crate::game_tip::tip_bundle()));
    // The fill art is a horizontally uniform gradient, so scaling its width is a reveal.
    area.spawn((
        LoadingBarFill,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(FILL_LEFT * 100.0),
            bottom: Val::Percent(FILL_BOTTOM * 100.0),
            width: Val::Percent(0.0), // set each frame
            height: Val::Percent(FILL_HEIGHT * 100.0),
            ..default()
        },
        ImageNode {
            image: fill,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
    ));
    // Border frame, on top.
    area.spawn((
        LoadingBarBorder,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(BORDER_LEFT * 100.0),
            bottom: Val::Percent(BORDER_BOTTOM * 100.0),
            width: Val::Percent(BORDER_WIDTH * 100.0),
            height: Val::Percent(BORDER_HEIGHT * 100.0),
            ..default()
        },
        ImageNode {
            image: border,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
    ));
}

/// The ship screen's tables and textures, with the world map tiles and their pixel sizes. The
/// reference loads the textures each time a route draws (`0x40a160`); they are the same files, so
/// they load once here. Any miss leaves no ship screen: the reference then draws its plain bar.
fn load_ship_route_data(
    assets: &mut WorldAssets,
    images: &mut Assets<Image>,
) -> Option<(ShipRouteData, Vec<SizedImage>)> {
    let tables = {
        let mut chain = assets.chain.lock_recover();
        let splines = load_taxi_splines(&mut chain);
        let nodes = load_taxi_path_nodes(&mut chain);
        let continents = load_world_map_continent_catalog(&mut chain);
        (splines, nodes, continents)
    };
    let (splines, legs, continents) = match tables {
        (Ok(s), Ok(n), Ok(c)) => (s, DockLegs::build(&n), c),
        (s, n, c) => {
            let why = [s.err(), n.err(), c.err()]
                .into_iter()
                .flatten()
                .map(|e| format!("{e:#}"))
                .collect::<Vec<_>>()
                .join("; ");
            warn!("ship loading screen unavailable, transports show the plain bar: {why}");
            return None;
        }
    };
    let mut sized = |path: &str| {
        let handle = assets.sprite_texture(path, images)?;
        let size = images.get(&handle)?.size();
        Some((handle, size))
    };
    let Some(elements) = sized(SHIP_ELEMENTS) else {
        warn!("ship loading screen unavailable: {SHIP_ELEMENTS} missing");
        return None;
    };
    let mut tiles = Vec::with_capacity(12);
    for i in 1..=12 {
        let path = format!("Interface\\WorldMap\\World\\World{i}");
        let Some(tile) = sized(&path) else {
            warn!("ship loading screen unavailable: {path} missing");
            return None;
        };
        tiles.push(tile);
    }
    info!(
        "ship loading screen: {} route rows, world map sheet loaded",
        splines.len()
    );
    Some((
        ShipRouteData {
            splines,
            legs,
            continents,
            elements,
        },
        tiles,
    ))
}

/// The world map sheet, 4 tiles across and 3 down from the top-left, stretched over the area
/// (`0x407c50`): a tile spans 256 px of the 1002×668 sheet, cut at its right and bottom edges,
/// and shows the matching corner of its texture.
fn spawn_sheet_tiles(ship: &mut ChildSpawnerCommands, tiles: Vec<SizedImage>) {
    for (i, (image, size)) in tiles.into_iter().enumerate() {
        let (row, col) = ((i / 4) as f32, (i % 4) as f32);
        let (left, top) = (col * SHEET_TILE, row * SHEET_TILE);
        let width = (left + SHEET_TILE).min(SHEET_W) - left;
        let height = (top + SHEET_TILE).min(SHEET_H) - top;
        let size = size.as_vec2();
        ship.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(100.0 * left / SHEET_W),
                top: Val::Percent(100.0 * top / SHEET_H),
                width: Val::Percent(100.0 * width / SHEET_W),
                height: Val::Percent(100.0 * height / SHEET_H),
                ..default()
            },
            ImageNode {
                image,
                image_mode: NodeImageMode::Stretch,
                rect: Some(Rect::new(
                    0.0,
                    0.0,
                    size.x * width / SHEET_TILE,
                    size.y * height / SHEET_TILE,
                )),
                ..default()
            },
        ));
    }
}

/// Settle a transport raise before [`drive_loading_screen`] applies it (`0x40ad50`): the raise
/// draws the route when we ride the transport the packet names and the route builds, from the leg
/// that departs the map the packet says we leave.
#[allow(clippy::too_many_arguments)]
fn pick_ship_route(
    mut transfers: MessageReader<crate::net::TransferPendingMessage>,
    data: Option<Res<ShipRouteData>>,
    player: Option<Res<crate::player::Player>>,
    objects: Query<(&crate::net::Guid, Option<&crate::net::ObjectStore>)>,
    templates: Res<crate::go_templates::GameObjectTemplates>,
    mut screen: ResMut<LoadingScreen>,
) {
    let Some(&crate::net::TransferPendingMessage(info)) = transfers.read().last() else {
        return;
    };
    // No entry, or no map after it, is the plain raise (`0x401480`).
    let (Some(entry), Some(from_map)) = (info.transport_entry, info.transport_map) else {
        return;
    };
    if entry == 0 || from_map == u32::MAX {
        return;
    }
    let aboard = player
        .as_ref()
        .and_then(|p| p.ride_entity())
        .and_then(|deck| objects.get(deck).ok())
        .filter(|(_, store)| store.and_then(|s| s.0.object_entry()) == Some(entry));
    let pick = match aboard {
        None => ShipPick::NotAboard,
        Some((guid, _)) => {
            // A type-15 template's path is its `data0` (property 35); any other type has none.
            let path = templates
                .get(guid.0)
                .and_then(|t| t.mo_transport)
                .map(|mo| mo.taxi_path_id);
            let route = path.zip(data.as_ref()).and_then(|(path, d)| {
                let leg = d.legs.leg_departing(path, from_map).unwrap_or(u32::MAX);
                ship_route(&d.splines, &d.legs, &d.continents, path, leg)
            });
            match route {
                Some(route) => ShipPick::Route(route),
                None => ShipPick::Failed,
            }
        }
    };
    info!("loading screen: transport {entry} leaving map {from_map} → {pick:?}");
    screen.ship_pick = Some(pick);
}

/// The camera target's transport's leg, last seen on this map, and the transport.
#[derive(Default)]
struct SeenShipLeg(Option<(Entity, u32)>);

/// A type-15 that changes leg on the current map with the camera's target aboard raises the screen
/// over the leg it left (`0x7134a0` → `0x40ae30`; the passenger test is `0x7132e0`'s, against the
/// camera's `+0x88` target, our [`crate::net::Embodied`]): Undercity to Grom'gol's mid-route
/// teleport and its wrap. The leg only updates while the transport samples this map.
///
/// Deviation: the reference stays silent until the transport's model has loaded (its first
/// sequence set); here the first sight of the transport with the target aboard, after boarding or a
/// worldport (whose re-create is a new object there), only records the leg.
#[allow(clippy::type_complexity)]
fn follow_ship_leg(
    player: Option<Res<crate::player::Player>>,
    embodied: Query<
        (
            Has<crate::net::SelfPlayer>,
            Option<&crate::transport::TransportRider>,
        ),
        With<crate::net::Embodied>,
    >,
    index: Res<crate::net::GuidIndex>,
    current_map: Option<Res<CurrentMap>>,
    transports: Query<(
        &crate::net::Guid,
        &crate::transport::Transport,
        &crate::transport::TransportAnchor,
    )>,
    mut worldports: MessageReader<crate::net::WorldportMessage>,
    templates: Res<crate::go_templates::GameObjectTemplates>,
    data: Option<Res<ShipRouteData>>,
    time: Res<Time<Real>>,
    mut seen: Local<SeenShipLeg>,
    mut screen: ResMut<LoadingScreen>,
) {
    if worldports.read().count() > 0 {
        seen.0 = None;
    }
    let (Some(player), Some(current_map)) = (player, current_map) else {
        return;
    };
    // Our own body's ride is the player's; a possessed unit's is its rider tail.
    let ride = match embodied.single() {
        Ok((true, _)) => player.ride_entity(),
        Ok((false, Some(rider))) => index.0.get(&rider.transport_guid).copied(),
        _ => None,
    };
    let Some((deck, (guid, transport, anchor))) =
        ride.and_then(|deck| transports.get(deck).ok().map(|t| (deck, t)))
    else {
        seen.0 = None;
        return;
    };
    let Some(leg) = transport.ship_leg_at(anchor) else {
        return;
    };
    if transport.sample_at(anchor, current_map.0).map != current_map.0 {
        return;
    }
    let previous = match seen.0 {
        Some((entity, previous)) if entity == deck => previous,
        _ => {
            seen.0 = Some((deck, leg));
            return;
        }
    };
    if previous == leg {
        return;
    }
    seen.0 = Some((deck, leg));
    let route = templates
        .get(guid.0)
        .and_then(|t| t.mo_transport)
        .zip(data.as_ref())
        .and_then(|(mo, d)| {
            ship_route(
                &d.splines,
                &d.legs,
                &d.continents,
                mo.taxi_path_id,
                previous,
            )
        });
    info!(
        "loading screen: transport {:#x} left leg {previous} for {leg} → route {}",
        guid.0,
        route.is_some()
    );
    screen.raise_for_ship_leg(route, time.elapsed_secs());
}

/// The route over the sheet (`0x408e10`): the dots up to the load's progress, then the two dock
/// markers on top. Rebuilt when the route changes; each frame only the lit count moves.
#[allow(clippy::type_complexity)]
fn draw_ship_route(
    mut commands: Commands,
    screen: Res<LoadingScreen>,
    data: Option<Res<ShipRouteData>>,
    ship_root: Query<Entity, With<LoadingShip>>,
    mut dots: Query<(Entity, &ShipDot, &mut Visibility)>,
    mut built: Local<u32>,
) {
    let Ok(root) = ship_root.single() else {
        return;
    };
    if *built != screen.ship_revision {
        *built = screen.ship_revision;
        for (entity, _, _) in &dots {
            commands.entity(entity).despawn();
        }
        let (Some(route), Some(data)) = (screen.ship.as_ref(), data.as_ref()) else {
            return;
        };
        let (sheet, size) = (&data.elements.0, data.elements.1.as_vec2());
        // Quarters of the sheet as pixel rects, (u0, v0, u1, v1) with v down the image.
        let quarter = |u0: f32, v0: f32| {
            Rect::new(
                u0 * size.x,
                v0 * size.y,
                (u0 + 0.5) * size.x,
                (v0 + 0.5) * size.y,
            )
        };
        commands.entity(root).with_children(|ship| {
            for (i, &at) in route.dots.iter().enumerate() {
                ship.spawn(route_sprite(
                    ShipDot(Some(i)),
                    at,
                    DOT_HALF,
                    sheet,
                    quarter(0.0, 0.5),
                ));
            }
            ship.spawn(route_sprite(
                ShipDot(None),
                route.departure,
                DOCK_HALF,
                sheet,
                quarter(0.5, 0.0),
            ));
            ship.spawn(route_sprite(
                ShipDot(None),
                route.arrival,
                DOCK_HALF,
                sheet,
                quarter(0.0, 0.0),
            ));
        });
        // The new sprites appear next frame, lit by then.
        return;
    }
    let Some(route) = screen.ship.as_ref() else {
        return;
    };
    let lit = lit_dots(route.dots.len(), screen.displayed);
    for (_, dot, mut vis) in &mut dots {
        let shown = dot.0.is_none_or(|i| i < lit);
        vis.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// One route sprite centred at `at` in the area's unit square, y up.
fn route_sprite(
    dot: ShipDot,
    at: [f32; 2],
    half: Vec2,
    sheet: &Handle<Image>,
    rect: Rect,
) -> impl Bundle {
    (
        dot,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(100.0 * (at[0] - half.x)),
            bottom: Val::Percent(100.0 * (at[1] - half.y)),
            width: Val::Percent(200.0 * half.x),
            height: Val::Percent(200.0 * half.y),
            ..default()
        },
        ImageNode {
            image: sheet.clone(),
            image_mode: NodeImageMode::Stretch,
            rect: Some(rect),
            ..default()
        },
        Visibility::Hidden,
    )
}

/// Per frame: observe the lifecycle edges, run the raise and clear rules, resolve the backdrop and
/// set the bar.
#[allow(clippy::type_complexity)]
fn drive_loading_screen(
    mut screen: ResMut<LoadingScreen>,
    // Bundled to stay under Bevy's 16-parameter ceiling.
    stream: (
        Res<WorldLoadProgress>,
        Res<benilla_world::terrain_stream::ViewFocus>,
    ),
    context: (
        Option<Res<CurrentMap>>,
        Query<&Window, With<bevy::window::PrimaryWindow>>,
    ),
    maps: Option<Res<MapCatalogRes>>,
    screens: Option<Res<LoadingScreenCatalogRes>>,
    mut assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    // The root, then the plain and ship containers.
    mut covers: (
        Query<&mut Visibility, (With<LoadingRoot>, Without<LoadingBackdrop>)>,
        Query<&mut Visibility, (With<LoadingPlain>, Without<LoadingShip>, NotCoverPart)>,
        Query<&mut Visibility, (With<LoadingShip>, Without<LoadingPlain>, NotCoverPart)>,
    ),
    mut backdrop: Query<(&mut ImageNode, &mut Visibility), With<LoadingBackdrop>>,
    mut nodes: ParamSet<(
        Query<&mut Node, With<LoadingArea>>,
        Query<&mut Node, With<LoadingBarFill>>,
    )>,
    mut bar_vis: Query<
        &mut Visibility,
        (
            Or<(With<LoadingBarFill>, With<LoadingBarBorder>)>,
            Without<LoadingRoot>,
            Without<LoadingBackdrop>,
        ),
    >,
    player: Option<Res<crate::player::Player>>,
    // Real time: a load's long frames count in full, whatever the paced clock does.
    time: Res<Time<Real>>,
    // The cover holds until the warm pass has compiled its pipelines.
    warm: Res<crate::pipe_warm::WarmPass>,
    // The world-entry UI load runs behind this cover; the reference's reveal always has the UI up.
    entry_ui_pending: Option<Res<crate::ui_script::PendingEntryUiLoad>>,
    edges: (
        Res<State<crate::char_select::ClientState>>,
        MessageReader<crate::net::EnteredWorldMessage>,
        MessageReader<crate::net::WorldportMessage>,
        MessageReader<crate::net::TeleportMessage>,
        MessageReader<crate::net::LoggedOutMessage>,
        MessageReader<crate::net::DisconnectedMessage>,
        MessageReader<crate::net::CharacterLoginFailedMessage>,
        Option<Res<crate::net::PendingTransfer>>,
        Option<Res<crate::char_select::Roster>>,
        MessageReader<crate::net::TransferPendingMessage>,
    ),
) {
    let (
        state,
        mut entered,
        mut worldports,
        mut teleports,
        mut logouts,
        mut lost,
        mut refusals,
        transfer,
        roster,
        mut transfer_packets,
    ) = edges;
    let (progress, focus) = (&stream.0, &stream.1);
    let now = time.elapsed_secs();
    let (current_map, window) = context;
    let map_id = current_map.as_ref().map(|m| m.0);
    // The physics hold releases on the same residency signal, so the body's world is live first.
    let player_settling = player.as_ref().is_some_and(|p| p.settling);

    // --- The edges. ---
    // A glue entry (not the seamless in-world reconnect), raised on the frame the glue tears down
    // and held across the server's character load; the art comes from the roster's map.
    if entered.read().next().is_some() && *state.get() != crate::char_select::ClientState::InWorld {
        let map = roster.as_ref().and_then(|r| r.pending_map());
        screen.raise("world entry", true, map, now);
        // The tip rides this edge alone: the reference's setter has one caller, in the glue's world
        // entry, which neither `SMSG_TRANSFER_PENDING` arm reaches.
        screen.tip_edge = Some(TipEdge::Pick);
    }
    // `SMSG_TRANSFER_PENDING`: cover now, before the server unloads us, as both arms of the
    // reference do (`0x4014ef`, `0x401509`); a transport's arm draws its route if it can. Raised
    // on the packet's message: the worldport that follows in the same drain has already taken
    // the latch.
    if let Some(crate::net::TransferPendingMessage(info)) = transfer_packets.read().last() {
        screen.raise_for_transfer(info.map_id, now);
    }
    // The latch cleared with no snap in flight is `SMSG_TRANSFER_ABORTED` (a worldport lands below
    // this frame): stop waiting, and the resident old world clears the screen.
    if transfer
        .as_ref()
        .is_some_and(|t| t.is_changed() && t.0.is_none())
    {
        screen.awaiting_snap = false;
    }
    // A snap never re-raises a live screen ([`LoadingScreen::far_snap`]).
    for w in worldports.read() {
        if screen.far_snap(w.map_id) {
            screen.raise("worldport", false, Some(w.map_id), now);
        }
    }
    // A same-map teleport ends any awaited snap; whether it needs a screen is decided below.
    let teleported = teleports.read().next().is_some();
    if teleported {
        screen.snap_landed(None);
    }
    // Logout, a lost session or a refused login: `CharSelect` applies next frame, so cover the torn
    // down world with black (the reference's world→glue cut), and disarm a snap that will not come.
    let session_over = lost.read().any(|m| m.session_over);
    let refused = refusals.read().next().is_some();
    if logouts.read().next().is_some() || session_over || refused {
        screen.active = true;
        screen.blackout = true;
        screen.awaiting_snap = false;
        screen.ready_frames = 0;
        info!(
            "loading screen: blackout ({})",
            if session_over {
                "session lost"
            } else if refused {
                "character login refused"
            } else {
                "logout"
            }
        );
    }
    if screen.blackout && *state.get() != crate::char_select::ClientState::InWorld {
        // The glue is up (it renders above this root); the cover's job is done.
        screen.dismiss();
    }

    // --- Backstop: the ground under the focus is not resident and nothing raised us. In world
    // only, since behind the glue nothing loads; and only while the focus is the body, since in a
    // cinematic or free-fly residency describes the camera's tile. ---
    if !screen.active
        && !progress.focus_resident
        && focus.follows_body()
        && *state.get() == crate::char_select::ClientState::InWorld
    {
        // Same map by construction: the ground under the body.
        screen.raise("focus not resident", false, map_id, now);
    }

    // --- At a snap, the same test against the clear's own predicate: a teleport inside the keep
    // band lands on resident terrain while the destination's buildings may still be arriving. ---
    let body = player.as_ref().map(|p| p.pos);
    let relocated = match (teleported, screen.last_pos, body) {
        (true, Some(was), Some(now_pos)) => was.distance(now_pos) >= SNAP_LOAD_MIN_YD,
        // No previous position (the entry frame): treat it as a relocation.
        (true, _, _) => true,
        _ => false,
    };
    screen.last_pos = body;
    if relocated
        && !screen.active
        && *state.get() == crate::char_select::ClientState::InWorld
        && !(progress.is_ready() && progress.presentable())
    {
        screen.raise("teleport, destination not presentable", false, map_id, now);
    }

    // --- Clear, sustained a few frames. ---
    if screen.active && !screen.held {
        if progress.is_ready()
            && progress.presentable()
            && !screen.awaiting_snap
            && !player_settling
            && warm.satisfied()
            && entry_ui_pending.is_none()
        {
            screen.ready_frames += 1;
            if screen.ready_frames >= CLEAR_AFTER_READY_FRAMES {
                screen.dismiss();
                info!(
                    "loading screen: cleared ({}/{} resident, {:.1}s)",
                    progress.ready,
                    progress.total,
                    now - screen.active_since
                );
            }
        } else {
            screen.ready_frames = 0;
            if now - screen.active_since > WAIT_LOG_AFTER
                && now - screen.last_wait_log >= WAIT_LOG_EVERY
            {
                screen.last_wait_log = now;
                info!(
                    "loading screen: waiting {:.1}s — {}/{} resident, {} placements pending, \
                     {} colliders pending, {} merges pending, awaiting_snap={}, settling={}, \
                     warm={}, ui_pending={}",
                    now - screen.active_since,
                    progress.ready,
                    progress.total,
                    progress.placements_pending,
                    progress.colliders_pending,
                    progress.merge_pending,
                    screen.awaiting_snap,
                    player_settling,
                    warm.satisfied(),
                    entry_ui_pending.is_some(),
                );
            }
        }
    }

    // --- Visibility: under blackout only the root's black shows. ---
    if let Ok(mut vis) = covers.0.single_mut() {
        *vis = if screen.active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    let content_vis = if screen.blackout {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    if let Ok((_, mut vis)) = backdrop.single_mut() {
        if *vis != content_vis {
            *vis = content_vis;
        }
    }
    for mut vis in &mut bar_vis {
        if *vis != content_vis {
            *vis = content_vis;
        }
    }
    // The route, when one drew, replaces the art, tip and bar (`0x40a270` → `0x409010`).
    let ship_shown = screen.active && !screen.blackout && screen.ship.is_some();
    let (plain_vis, ship_vis) = if ship_shown {
        (Visibility::Hidden, Visibility::Inherited)
    } else {
        (Visibility::Inherited, Visibility::Hidden)
    };
    if let Ok(mut vis) = covers.1.single_mut() {
        vis.set_if_neq(plain_vis);
    }
    if let Ok(mut vis) = covers.2.single_mut() {
        vis.set_if_neq(ship_vis);
    }
    if !screen.active || screen.blackout {
        return;
    }

    // --- Backdrop art, the reference's gate: the map id is read once per screen to resolve the
    // texture, so a raise onto a live screen repaints nothing. ---
    if !screen.art_resolved {
        // Black, never the last screen's texture (the reference holds a null handle, no quad).
        if let Ok((mut img, _)) = backdrop.single_mut() {
            if img.color != Color::BLACK {
                img.color = Color::BLACK;
            }
        }
        if let (Some(map_id), Some(maps), Some(screens), Some(assets)) =
            (screen.map, maps.as_ref(), screens.as_ref(), assets.as_mut())
        {
            let row = maps
                .0
                .loading_screen_id(map_id)
                .and_then(|id| screens.0.row(id));
            let wide_window = window
                .single()
                .is_ok_and(|w| w.width() / w.height() >= WIDE_WINDOW_THRESHOLD);
            // The wide art first when the row and the window ask for it, then the normal art.
            let mut candidates = Vec::new();
            if let Some((base, has_wide)) = row {
                if has_wide && wide_window {
                    candidates.push((wide_art_path(base), true));
                }
                candidates.push((base.to_string(), false));
            }
            let mut selected_wide = false;
            let handle = candidates.into_iter().find_map(|(path, is_wide)| {
                let handle = match screen.art_cache.get(&path) {
                    Some(h) => Some(h.clone()),
                    None => assets.sprite_texture(&path, &mut images).inspect(|h| {
                        screen.art_cache.insert(path.clone(), h.clone());
                    }),
                }?;
                selected_wide = is_wide;
                Some(handle)
            });
            // Each load starts at 4:3 (`0x409f5c` clears the wide flag before the pick).
            if let Ok(mut node) = nodes.p0().single_mut() {
                let aspect = if selected_wide {
                    WIDE_BACKDROP_ASPECT
                } else {
                    BACKDROP_ASPECT
                };
                fit_area(&mut node, aspect);
            }
            match (handle, backdrop.single_mut()) {
                (Some(handle), Ok((mut img, _))) => {
                    img.image = handle;
                    img.color = Color::WHITE; // untint to reveal the art
                    screen.art_resolved = true;
                    info!(
                        "loading screen: backdrop for map {map_id} resolved (wide={selected_wide})"
                    );
                }
                // No retry: the reference writes `-1` into the map id and the draw returns on it.
                _ => {
                    warn!("loading screen: no backdrop for map {map_id} — plain black this load");
                    screen.map = None;
                }
            }
        }
    }

    // --- Progress bar: zero until the snap lands (residency is still the old place's). ---
    if !screen.awaiting_snap {
        screen.displayed = screen.displayed.max(progress.fraction());
    }
    let frac = screen.displayed;
    if let Ok(mut node) = nodes.p1().single_mut() {
        node.width = Val::Percent(frac * FILL_MAX_WIDTH * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_art_goes_before_the_first_dot_keeping_the_extension() {
        assert_eq!(
            wide_art_path("Interface\\Glues\\LoadingScreens\\LoadScreenNorthrend.blp"),
            "Interface\\Glues\\LoadingScreens\\LoadScreenNorthrendWide.blp"
        );
        assert_eq!(wide_art_path("A\\B.BLP"), "A\\BWide.BLP");
        assert_eq!(wide_art_path("A\\B"), "A\\BWide");
    }

    #[test]
    fn the_area_keeps_its_aspect_inside_the_window() {
        let mut node = Node::default();
        fit_area(&mut node, WIDE_BACKDROP_ASPECT);
        assert_eq!(node.width, Val::Vh(160.0));
        assert_eq!(node.height, Val::Vh(100.0));
        // A window narrower than the art caps the width at the window and the height to match.
        assert_eq!(node.max_width, Val::Vw(100.0));
        assert_eq!(node.max_height, Val::Vw(62.5));
    }

    #[test]
    fn the_cover_counts_frames_to_the_glass_and_resets_the_moment_it_drops() {
        let mut cover = EntryCover::default();
        assert!(
            cover.presented(),
            "no cover up: nothing is watching the previous present, so nothing waits"
        );
        assert!(!cover.owes_a_present(), "no cover owes no present");

        cover.tick(true);
        assert_eq!(cover.frames(), 1);
        assert!(
            !cover.presented(),
            "the cover's own render has not committed yet"
        );
        assert!(cover.owes_a_present());

        cover.tick(true);
        assert!(!cover.presented(), "one render committed, not two");

        cover.tick(true);
        assert!(
            cover.presented(),
            "at COVER_PRESENT_FRAMES the cover is provably on the glass"
        );
        assert!(!cover.owes_a_present());

        // A reveal drops the count to zero in one frame; the next raise earns its own.
        cover.tick(false);
        assert_eq!(cover.frames(), 0);
        assert!(cover.presented());
        cover.tick(true);
        assert!(!cover.presented(), "the next raise starts its own count");
    }
    /// vmangos can relocate a character out of an expired instance inside `Player::LoadFromDB`, so
    /// `SMSG_LOGIN_VERIFY_WORLD` names another map than the roster did.
    #[test]
    fn a_relocating_snap_cannot_move_the_art_the_screen_was_raised_with() {
        let mut screen = LoadingScreen::default();
        // The pick edge: art from the roster row, Shadowfang Keep, snap still to come.
        screen.raise("world entry", true, Some(33), 0.0);
        screen.tip_edge = Some(TipEdge::Pick);
        screen.displayed = 0.4;

        // `SMSG_LOGIN_VERIFY_WORLD`: the server seats us on map 0, at the instance's exit.
        assert!(
            !screen.far_snap(0),
            "a screen is already up — its own snap must not raise a second one"
        );
        assert_eq!(
            screen.map,
            Some(33),
            "the backdrop is the raise's and holds the whole way — the report"
        );
        assert!(
            !screen.awaiting_snap,
            "the snap the raise was waiting for has landed"
        );
        assert_eq!(
            screen.tip_edge,
            Some(TipEdge::Pick),
            "and the tip just picked survives it — a raise is the only thing that could \
             have cleared it, and no raise happened"
        );
        assert!(
            (screen.displayed - 0.4).abs() < f32::EPSILON,
            "nor does the bar restart mid-load"
        );
        assert!(screen.active, "the screen stays up across its own snap");
    }

    /// A cross-map port with no announcing edge still gets a cover: our backstop, which the
    /// reference's blocking world load does not need.
    #[test]
    fn a_snap_with_nothing_covering_it_asks_for_a_raise() {
        let mut screen = LoadingScreen::default();
        assert!(screen.far_snap(1), "nothing is covering this load");
        assert!(
            !screen.active,
            "far_snap answers the question; the caller does the raising, with its own reason"
        );
    }

    #[test]
    fn only_the_dismiss_clears_the_tip() {
        let mut screen = LoadingScreen::default();
        screen.raise("world entry", true, Some(0), 0.0);
        screen.tip_edge = Some(TipEdge::Pick);

        screen.raise("transfer pending", true, Some(389), 0.0);
        assert_eq!(
            screen.tip_edge,
            Some(TipEdge::Pick),
            "no raise clears a tip"
        );

        screen.dismiss();
        assert_eq!(screen.tip_edge, Some(TipEdge::Clear), "the dismiss does");
        assert!(!screen.active);
    }

    fn route() -> ShipRoute {
        ShipRoute {
            departure: [0.8, 0.4],
            arrival: [0.3, 0.3],
            dots: vec![[0.8, 0.4], [0.5, 0.5], [0.3, 0.3]],
        }
    }

    /// `0x40ad50`: a drawn route leaves the screen no map; a failed one clears any route and shows
    /// the destination's art; riding nothing keeps whatever is up.
    #[test]
    fn a_transport_raise_draws_its_route_or_falls_back_to_the_destinations_art() {
        let mut screen = LoadingScreen {
            ship_pick: Some(ShipPick::Route(route())),
            ..LoadingScreen::default()
        };
        screen.raise_for_transfer(0, 0.0);
        assert!(screen.active && screen.awaiting_snap);
        assert_eq!(screen.map, None, "the route replaces the art");
        assert_eq!(screen.ship, Some(route()));
        assert!(screen.ship_pick.is_none(), "the raise takes its pick");

        // A plain raise onto the live screen keeps the route but points the map.
        screen.raise_for_transfer(1, 0.0);
        assert_eq!(screen.ship, Some(route()));
        assert_eq!(screen.map, Some(1));
        screen.ship_pick = Some(ShipPick::NotAboard);
        screen.raise_for_transfer(1, 0.0);
        assert_eq!(screen.ship, Some(route()), "riding nothing keeps it too");

        let revision = screen.ship_revision;
        screen.ship_pick = Some(ShipPick::Failed);
        screen.raise_for_transfer(0, 0.0);
        assert_eq!(screen.ship, None, "a failed route clears the one up");
        assert_eq!(screen.map, Some(0));
        assert_ne!(screen.ship_revision, revision, "and its dots are rebuilt");

        screen.ship_pick = Some(ShipPick::Route(route()));
        screen.raise_for_transfer(0, 0.0);
        screen.dismiss();
        assert_eq!(screen.ship, None, "the dismiss clears the route");
    }

    /// `0x40ae30`: no snap to wait for; a failed route keeps the screen's last map.
    #[test]
    fn a_ship_leg_raise_waits_for_no_snap_and_keeps_the_last_map_when_its_route_fails() {
        let mut screen = LoadingScreen::default();
        screen.raise("world entry", true, Some(0), 0.0);
        screen.dismiss();
        screen.raise_for_ship_leg(None, 1.0);
        assert!(screen.active && !screen.awaiting_snap);
        assert_eq!(screen.map, Some(0), "the map id the last raise left");
        assert_eq!(screen.ship, None);
        screen.dismiss();
        screen.raise_for_ship_leg(Some(route()), 2.0);
        assert_eq!((screen.map, screen.ship.clone()), (None, Some(route())));
    }

    /// The reference's raises store the map id with no guard.
    #[test]
    fn a_raise_onto_a_live_screen_repoints_the_map_but_never_the_picture() {
        let mut screen = LoadingScreen::default();
        screen.raise("world entry", true, Some(33), 0.0);
        screen.art_resolved = true; // the first plain-draw frame resolved Shadowfang Keep's art

        screen.raise("transfer pending", true, Some(389), 0.0);
        assert_eq!(screen.map, Some(389), "the map id is re-pointed, unguarded");
        assert!(
            screen.art_resolved,
            "…and the picture is not: the texture is still the one this screen resolved"
        );

        // The dismiss is the only thing that lets the next screen resolve its own.
        screen.dismiss();
        assert!(!screen.art_resolved);
    }
}
