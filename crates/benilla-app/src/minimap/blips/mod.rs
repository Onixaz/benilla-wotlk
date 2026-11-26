//! The minimap blip layer: AreaPOI landmarks, quest and tracking dots, rim arrows and the hover
//! tooltip.
//!
//! Landmarks:
//! - Candidates: `ContinentID` is the current map and `Flags & 1`, no faction or importance gate.
//!   A row at (0, 0), or with a `WorldStateID` whose state reads 0, is neither drawn nor ranked
//!   (`0x7f44a0`).
//! - In range (`d / viewRadius ≤ 0.8`): the `POIIcons.blp` cell of `Icon[state − 1]` (the first
//!   without a world state or outside 1 to 9), only for rows with `Flags & 2`; other rows draw
//!   nothing.
//! - Out of range within 694.444 yd: the first 3 by (`Importance` signed ascending, distance),
//!   on the 0.8 rim, each rotated to point at its POI in its source's art ([`RimArrow`]).
//!   Indoors only the gossip POI's, the corpse's and `Flags & 0x100` rows' arrows draw; the
//!   others keep their ranked slots hidden (`0x57d860`, `0x7f4b60`).
//!
//! The object blips are [`dots`]': the town tracking type, quest status, an unlearned flight
//! master and the tracking masks each give a cell of `ObjectIcons.blp`, culled at 0.8 of the view
//! radius and drawn after the group dots and vehicles; the rim arrows follow, and the player
//! arrow tops them all (`0x581e80`, `0x5832c0`).
//!
//! Blip sizes are frozen once by the minimap frame's constructor against a 140.8-px basis.
//!
//! Not built: the per-source rim-arrow z-order (bottom to top landmark, party, quest, gossip,
//! corpse, from `+0x48` as a frame-level offset), which is unsettled; and static blip slot 0,
//! `SelectQuestLogEntry()`'s gold guide arrow at the quest's POI.
//!
//! Deviation: blips re-project every frame, where the reference rebuilds them about once a second
//! (`0x581e80`), because that staleness is a throttle quirk.
//!
//! The cross-interior grey (`0xffb0b0b0`) comes from an indoor-containment mismatch; the
//! reference's compare is untraced. The tooltip sits at the cursor; its exact seat is untraced,
//! since the reference's tooltip builder gives only its content.

mod dots;
mod vehicles;

pub(super) use dots::{emit_group_dots, emit_object_blips, BlipWorld, ClassMemo, SelfTracking};
pub(super) use vehicles::{
    emit_vehicle_blips, select_vehicles, tracked_vehicles, TrackedVehicle, VehicleSelection,
    RIDING_ARROW_ALPHA, VEHICLE_ICONS,
};

use bevy::ecs::system::NonSendMut;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::AreaPoi;
use benilla_ui::script::UiScript;

use crate::go_templates::GameObjectTemplates;
use crate::names::NameCache;
use crate::net::{Embodied, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::ui_pass::{UiQuad, UiQuads, UvRect};

/// The landmark rank radius in yards.
const LANDMARK_RANK_YD: f32 = 694.444;
/// The in-range split and the rim radius as a fraction of the half-disc.
const BLIP_EDGE_RATIO: f32 = 0.8;
/// AreaPOI `Flags` bit 0, the candidacy gate.
const FLAG_CANDIDATE: u32 = 0x1;
/// AreaPOI `Flags` bit 1, draw the in-range icon.
const FLAG_IN_RANGE_ICON: u32 = 0x2;
/// AreaPOI `Flags` bit 8, keep the rim arrow indoors (`0x7f4b60`).
const FLAG_ALWAYS_SHOWN: u32 = 0x100;
/// The basis every blip size is frozen against: the stock 140-unit widget at 140.8 px on the
/// 1024×768 screen, computed once by the minimap frame's constructor.
pub(super) const BLIP_BASIS_PX: f32 = 140.8;
/// The player arrow's art, on the minimap and the world map, whose stock `WorldMapFrame.lua`
/// `SetModel`s a pane to the same M2.
pub(crate) const PLAYER_ARROW_TEXTURE: &str = "Interface\\Minimap\\MinimapArrow";
/// The rim arrow's quad: `Rotating-MinimapArrow.m2`'s 0.0500-unit full-texture quads at 768 px
/// per model unit (modelScale 0.6 × 1280), so the sprite needs no uv crop.
const ARROW_QUAD_PX: f32 = 38.4;
/// A group member's rim arrow: `0x580ae0` and `0x580ec0` draw the flat group-arrow texture on a
/// quad of `0.045` of the screen height × 1.6667 (`0x57ded7`), 57.6 px on the 140.8-px basis, its
/// uv square turned about the centre with corners at radius 1, so the art fills `1/√2` of it.
const GROUP_ARROW_PX: f32 = 57.6 * std::f32::consts::FRAC_1_SQRT_2;
/// The raid roster's capacity (`0x7f4390`).
const RAID_ROWS_MAX: usize = 40;

/// Which rim-arrow art a blip source draws. The minimap's one arrow model,
/// `Rotating-MinimapArrow.m2`, picks among four arrow layers by sequence: in each looping sequence
/// exactly one layer's stepped alpha is 1.0, untinted, so one flat sprite per source is exact.
/// `AnimationData.dbc` names them 165 `GroupArrow`, 166 `Arrow`, 167 `CorpseArrow`, 168
/// `GuideArrow`.
///
/// The selector is `0xa7` when the output record's `+0x48 == 2`, else `0xa6 + 2·(v != -2)`, so
/// everything but a DBC landmark and the corpse draws the guide arrow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RimArrow {
    /// An `AreaPOI.dbc` landmark: sequence `0xa6`, `+0x48 == -2`, the white one.
    Landmark,
    /// A party or raid member, or your pet: sequence `0xa5`; the group passes draw its texture
    /// flat ([`GROUP_ARROW_PX`]).
    Group,
    /// Your own corpse and the release graveyard: sequence `0xa7`, `+0x48 == 2`.
    Corpse,
    /// The guard's directions ([`crate::poi_marker`]): sequence `0xa8`, the default arm, gold.
    Guide,
}

impl RimArrow {
    /// The `AnimationData.dbc` sequence the reference plays for this art; the test checks the
    /// model against it.
    #[cfg(test)]
    pub(crate) const fn anim_id(self) -> u16 {
        match self {
            Self::Group => 0xa5,
            Self::Landmark => 0xa6,
            Self::Corpse => 0xa7,
            Self::Guide => 0xa8,
        }
    }

    /// The `.blp` that sequence lights.
    pub(crate) const fn texture(self) -> &'static str {
        match self {
            Self::Landmark => "Interface\\Minimap\\Rotating-MinimapArrow",
            Self::Group => "Interface\\Minimap\\Rotating-MinimapGroupArrow",
            Self::Corpse => "Interface\\Minimap\\Rotating-MinimapCorpseArrow",
            Self::Guide => "Interface\\Minimap\\Rotating-MinimapGuideArrow",
        }
    }

    /// The four, in [`RimArrowArt`]'s slot order.
    pub(crate) const ALL: [Self; 4] = [Self::Landmark, Self::Group, Self::Corpse, Self::Guide];
}

/// The four rim-arrow textures; a missing one draws no arrow for its source, never another's.
#[derive(Default)]
pub(crate) struct RimArrowArt([Option<Handle<Image>>; 4]);

impl RimArrowArt {
    pub(crate) fn set(&mut self, kind: RimArrow, tex: Option<Handle<Image>>) {
        self.0[kind as usize] = tex;
    }

    pub(super) fn get(&self, kind: RimArrow) -> Option<&Handle<Image>> {
        self.0[kind as usize].as_ref()
    }
}
/// An in-range POI icon's quad, 16 px (frozen by the constructor), the corpse's too (`0x5827e2`).
const POI_ICON_PX: f32 = 16.0;
/// A rim arrow's hover reach on each axis from its centre: `0.005` of the screen height in device
/// units at the frame's scale (`0x57c720`), 6.4 px on the 140.8-px basis.
const ARROW_HOVER_PX: f32 = 6.4;

/// Every blip under the cursor this frame, for [`drive_blip_tooltip`], in each list's draw order;
/// the tooltip lists them all (`0x57c720`).
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub(crate) struct MinimapBlipHover {
    /// The landmark rim arrows' names.
    pub(crate) directions: Vec<String>,
    /// The party rim arrows' member names.
    pub(crate) party: Vec<String>,
    /// The raid rim arrows' member names.
    pub(crate) raid: Vec<String>,
    /// The in-range landmark icons' names.
    pub(crate) landmarks: Vec<String>,
    /// The object blips.
    pub(crate) objects: Vec<ObjectHover>,
}

/// One hovered object blip: its name, whether it drew grey, whether it is an unknown flight
/// master's, and a tracked trainer's title.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectHover {
    pub(crate) name: HoverName,
    pub(crate) grey: bool,
    pub(crate) flight_master: bool,
    pub(crate) title: Option<String>,
}

/// How a hovered blip names itself: a unit through the name cache, a GameObject by its template.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HoverName {
    Guid(u64),
    Known(String),
}

/// The blip layer's system inputs, tupled to keep `emit_minimap` under Bevy's 16-parameter cap.
pub(super) type BlipInputs<'w, 's> = (
    Res<'w, crate::ui_quest::QuestStatuses>,
    Res<'w, GuidIndex>,
    Query<'w, 's, &'static GlobalTransform, With<NetEntity>>,
    Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    ResMut<'w, MinimapBlipHover>,
    Res<'w, crate::ui_party::GroupState>,
    TrackedCandidates<'w, 's>,
    Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    (
        Res<'w, NameCache>,
        Res<'w, GameObjectTemplates>,
        Option<Res<'w, crate::go_templates::Locks>>,
        Option<Res<'w, crate::target::Factions>>,
        Res<'w, crate::net::Reputations>,
    ),
    Res<'w, crate::poi_marker::PoiMarker>,
    // The landmark rows and the world states that gate them.
    (
        Option<Res<'w, crate::area_poi::AreaPoiRes>>,
        Option<Res<'w, crate::world_state::WorldStates>>,
    ),
    ResMut<'w, super::MinimapPing>,
    Option<NonSendMut<'w, UiScript>>,
    VehicleInputs<'w, 's>,
);

/// The vehicle pass's inputs: the streamed vehicles, their tables, our own seat, and a scratch
/// list kept across frames.
pub(super) type VehicleInputs<'w, 's> = (
    Query<
        'w,
        's,
        (
            &'static crate::vehicle::Vehicle,
            &'static GlobalTransform,
            &'static ObjectStore,
        ),
    >,
    Res<'w, crate::vehicle::VehicleTables>,
    Query<'w, 's, &'static crate::vehicle::Passenger, With<SelfPlayer>>,
    Local<'s, Vec<TrackedVehicle>>,
);

/// Every streamed object the classifier considers; our own avatar is the arrow, never a dot.
pub(super) type TrackedCandidates<'w, 's> = Query<
    'w,
    's,
    (
        &'static Guid,
        &'static NetEntity,
        &'static GlobalTransform,
        // A `Ref`, so a still object's classification can be kept ([`ClassMemo`]).
        Option<Ref<'static, ObjectStore>>,
        Option<&'static crate::ui_taxi::FlightMasterStatus>,
    ),
    Without<Embodied>,
>;

/// The frame geometry the blip emitters draw in; `wx`, `wy` are the player's position.
pub(super) struct BlipCtx {
    pub(super) center: Vec2,
    pub(super) side: f32,
    pub(super) px_per_yd: f32,
    /// The live view radius in yards: the in-range split's denominator and the dot cull.
    pub(super) radius_yd: f32,
    pub(super) z: u64,
    pub(super) alpha: f32,
    pub(super) wx: f32,
    pub(super) wy: f32,
    /// The cursor in the quads' y-down logical px.
    pub(super) cursor: Option<Vec2>,
    /// Window px per UI unit: everything else here is window px, while Lua
    /// ([`Minimap:PingLocation`](super::ping)) speaks UI units.
    pub(super) seam: f32,
    /// The map's clockwise turn, the facing under `rotateMinimap` and 0 north up.
    pub(super) turn: f32,
}

impl BlipCtx {
    /// A WoW world point's screen offset from the widget centre: north up (+X up, +Y left), then
    /// turned with the map (`point *= rotation`, `0x5803de`).
    pub(super) fn offset(&self, w: [f32; 3]) -> Vec2 {
        super::turn_offset(
            Vec2::new(
                (self.wy - w[1]) * self.px_per_yd,
                -(w[0] - self.wx) * self.px_per_yd,
            ),
            self.turn,
        )
    }
}

/// The landmark selection's two draw lists.
pub(super) struct LandmarkSelection<'a> {
    /// In-range rows with [`FLAG_IN_RANGE_ICON`] and the `Icon` cell their world state picks,
    /// drawn at their position.
    pub(super) icons: Vec<(&'a AreaPoi, u32)>,
    /// The rim arrows `(dist, poi, art, always_shown)` in rank order. An always-shown arrow
    /// still draws indoors: the gossip POI's and the corpse's (dynamic slots 1 and 2; slot 0's
    /// quest point is not built), or a row with [`FLAG_ALWAYS_SHOWN`] (`0x7f4b60`).
    pub(super) arrows: Vec<(f32, &'a AreaPoi, RimArrow, bool)>,
}

/// The static POI slots appended after the DBC scan, each placed on the shown map by its caller.
#[derive(Clone, Copy, Default)]
pub(super) struct StaticPois<'a> {
    /// The guard-directions POI, the reference's gossip slot 1 (`0x7f4870`).
    pub(super) marker: Option<&'a AreaPoi>,
    /// The corpse's POI (`0x7f4990`), slot `0xd39540`.
    pub(super) corpse: Option<&'a AreaPoi>,
    /// The release graveyard's spirit-healer POI (`0x7f4a00`), the slot after the corpse's: slot
    /// 3, the one dynamic slot hidden indoors.
    pub(super) release: Option<&'a AreaPoi>,
}

/// The landmark selection (module doc). The static slots skip the candidacy gate; all but the
/// corpse's also take the 694.444-yd cut (`0x7f46b1`). `world_state` reads the live table.
pub(super) fn select_landmarks<'a>(
    pois: impl Iterator<Item = &'a AreaPoi>,
    statics: StaticPois<'a>,
    map_id: u32,
    wx: f32,
    wy: f32,
    radius_yd: f32,
    world_state: impl Fn(u32) -> i32,
) -> LandmarkSelection<'a> {
    let mut icons = Vec::new();
    let mut arrows: Vec<(f32, &AreaPoi, RimArrow, bool)> = Vec::new();
    // `(poi, art, uncut, slot always shown)`.
    let candidates = pois
        .filter(|p| p.continent_id == map_id && p.flags & FLAG_CANDIDATE != 0)
        .map(|p| (p, RimArrow::Landmark, false, false))
        .chain(statics.marker.map(|m| (m, RimArrow::Guide, false, true)))
        .chain(statics.corpse.map(|c| (c, RimArrow::Corpse, true, true)))
        .chain(statics.release.map(|r| (r, RimArrow::Corpse, false, false)));
    for (p, art, uncut, slot_shown) in candidates {
        // Neither drawn nor on the rim: a row at (0, 0), or one whose world state reads 0
        // (`0x7f44a0`); Isle of Conquest keeps one row per node state.
        let unplaced = p.pos[0] == 0.0 && p.pos[1] == 0.0;
        if unplaced || (p.world_state_id != 0 && world_state(p.world_state_id) == 0) {
            continue;
        }
        let d = ((p.pos[0] - wx).powi(2) + (p.pos[1] - wy).powi(2)).sqrt();
        if d / radius_yd <= BLIP_EDGE_RATIO {
            if p.flags & FLAG_IN_RANGE_ICON != 0 {
                icons.push((p, landmark_icon(p, &world_state)));
            }
        } else if uncut || d <= LANDMARK_RANK_YD {
            arrows.push((d, p, art, slot_shown || p.flags & FLAG_ALWAYS_SHOWN != 0));
        }
    }
    arrows.sort_by(|a, b| {
        (a.1.importance as i32)
            .cmp(&(b.1.importance as i32))
            .then(a.0.total_cmp(&b.0))
    });
    arrows.truncate(3);
    LandmarkSelection { icons, arrows }
}

/// The drawn `Icon` column: `Icon[state − 1]` for a row with a world state, the first outside
/// 1 to 9 or without one (`0x581e80`).
fn landmark_icon(p: &AreaPoi, world_state: impl Fn(u32) -> i32) -> u32 {
    match p.world_state_id {
        // `icon` is `Icon[0]` on a DBC row, and the only icon a static slot sets.
        0 => p.icon,
        id => usize::try_from(world_state(id) - 1)
            .ok()
            .and_then(|s| p.icons.get(s))
            .copied()
            .unwrap_or(p.icons[0]),
    }
}

/// The `POIIcons.blp` atlas and its width in texels, which its cells are cut from.
pub(super) struct PoiAtlas {
    pub(super) tex: Handle<Image>,
    pub(super) width: u32,
}

impl PoiAtlas {
    /// `Icon`'s cell (`0x5832f0`): 196 cells, 14 a row, each 18 texels of the texture's width on
    /// both axes with a one-texel inset; the draw gates `Icon < 196` (`0x5826b9`).
    pub(super) fn cell(&self, icon: u32) -> Option<[f32; 4]> {
        poi_icon_cell(icon, self.width)
    }
}

/// The POI atlas's cell count, 14 × 14 (`0x5832f0`).
const POI_CELLS: u32 = 196;
/// The POI atlas's columns, `sqrt(196)`.
const POI_COLUMNS: u32 = 14;
/// A POI cell's pitch in texels.
const POI_CELL_TEXELS: f32 = 18.0;

fn poi_icon_cell(icon: u32, width: u32) -> Option<[f32; 4]> {
    if icon >= POI_CELLS || width == 0 {
        return None;
    }
    let texel = 1.0 / width as f32;
    let pitch = POI_CELL_TEXELS * texel;
    let u0 = (icon % POI_COLUMNS) as f32 * pitch + texel;
    let v0 = (icon / POI_COLUMNS) as f32 * pitch + texel;
    Some([u0, u0 + pitch - texel, v0, v0 + pitch - texel])
}

/// Draw the in-range POI icons, the first thing on the map after the tiles (`0x581e80`).
pub(super) fn emit_landmark_icons(
    ctx: &BlipCtx,
    sel: &LandmarkSelection,
    poi_icons: Option<&PoiAtlas>,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    let Some(atlas) = poi_icons else {
        return;
    };
    for &(poi, icon) in &sel.icons {
        let Some(cell) = atlas.cell(icon) else {
            continue;
        };
        let rect = Rect::from_center_size(
            ctx.center + ctx.offset(poi.pos),
            Vec2::splat(ctx.side * (POI_ICON_PX / BLIP_BASIS_PX)),
        );
        quads.overlays.push(UiQuad {
            rect,
            z_key: ctx.z,
            texture: Some(atlas.tex.clone()),
            uv: UvRect::from_tex_coords(cell),
            color: [1.0, 1.0, 1.0, ctx.alpha],
            ..default()
        });
        if ctx.cursor.is_some_and(|c| rect.contains(c)) {
            hover.landmarks.push(poi.name.clone());
        }
    }
}

/// Draw the landmark rim arrows, after the object blips and before the group's (`0x57d860`).
/// Indoors only the always-shown ones draw, and only a drawn one hovers (`0x57c720`); the three
/// ranked slots stay taken by the hidden ones.
pub(super) fn emit_landmark_arrows(
    ctx: &BlipCtx,
    sel: &LandmarkSelection,
    arrows: &RimArrowArt,
    indoors: bool,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    for &(_d, poi, art, always_shown) in &sel.arrows {
        if indoors && !always_shown {
            continue;
        }
        let Some(tex) = arrows.get(art) else {
            continue;
        };
        let at = push_rim_arrow(ctx, poi.pos, tex, ARROW_QUAD_PX, quads);
        if arrow_hovered(ctx, at) {
            hover.directions.push(poi.name.clone());
        }
    }
}

/// Whether the cursor is within [`ARROW_HOVER_PX`] of a rim arrow's centre on both axes
/// (`0x57c720`), a box much smaller than the arrow's art.
fn arrow_hovered(ctx: &BlipCtx, at: Vec2) -> bool {
    let reach = ctx.side * (ARROW_HOVER_PX / BLIP_BASIS_PX);
    ctx.cursor
        .is_some_and(|c| (c.x - at.x).abs() < reach && (c.y - at.y).abs() < reach)
}

/// One rim arrow of `px` on the 0.8 rim, pointing at its target, drawn in one pass as only one of
/// the model's layers is ever opaque. Returns its centre for the hover test.
fn push_rim_arrow(
    ctx: &BlipCtx,
    target: [f32; 3],
    tex: &Handle<Image>,
    px: f32,
    quads: &mut UiQuads,
) -> Vec2 {
    let dir = ctx.offset(target).normalize_or_zero();
    let pos = ctx.center + dir * (ctx.side * 0.5 * BLIP_EDGE_RATIO);
    // Quad rotation is clockwise on a y-down screen and the art points up at zero.
    let rotation = dir.x.atan2(-dir.y);
    let rect = Rect::from_center_size(pos, Vec2::splat(ctx.side * (px / BLIP_BASIS_PX)));
    quads.overlays.push(UiQuad {
        rect,
        z_key: ctx.z,
        texture: Some(tex.clone()),
        color: [1.0, 1.0, 1.0, ctx.alpha],
        rotation,
        ..default()
    });
    pos
}

/// A party member's position: the streamed transform, else the `PARTY_MEMBER_STATS` position
/// (whole yards on the wire).
pub(crate) fn party_member_pos(
    m: &benilla_protocol::messages::GroupMemberEntry,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
) -> Option<(f32, f32)> {
    if let Some(tf) = guids.0.get(&m.guid).and_then(|e| unit_pos.get(*e).ok()) {
        let w = bevy_to_wow(tf.translation());
        return Some((w[0], w[1]));
    }
    group
        .stats
        .get(&m.guid)
        .and_then(|s| s.position)
        .map(|(x, y)| (f32::from(x), f32::from(y)))
}

/// The raid roster's rows the raid list draws (`0x7f4390`): those outside our own subgroup, whom
/// the party slots do not carry.
pub(super) fn raid_rows(
    group: &crate::ui_party::GroupState,
) -> impl Iterator<Item = &benilla_protocol::messages::GroupMemberEntry> {
    use crate::ui_party::GROUP_MEMBER_SUBGROUP;
    let own = group.own_flags & GROUP_MEMBER_SUBGROUP;
    group
        .members
        .iter()
        .filter(move |m| m.flags & GROUP_MEMBER_SUBGROUP != own)
        .take(RAID_ROWS_MAX)
}

/// The party and raid rim arrows, the out-of-range half of the group placement, drawn before the
/// player arrow; a hovered one names its member (`0x57c720`). A member a tracked vehicle stands
/// for draws none (`0x7f3f40`). The corpse's is a landmark arrow ([`StaticPois::corpse`]).
///
/// This differs from the reference, not built yet: its slot 4 is the pet (`UNIT_FIELD_CHARM`, else
/// `UNIT_FIELD_SUMMON`), so there is no pet blip here.
pub(super) fn emit_group_arrows(
    ctx: &BlipCtx,
    group: &crate::ui_party::GroupState,
    possessors: &[u64],
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
    arrows: &RimArrowArt,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    let member = |raid: bool| {
        move |m: &benilla_protocol::messages::GroupMemberEntry| {
            (!possessors.contains(&m.guid))
                .then(|| party_member_pos(m, group, guids, unit_pos))
                .flatten()
                .map(|p| (p, raid, m.name.clone()))
        }
    };
    let party = group.party_slots().filter_map(member(false));
    // The raid's arrows draw after the party's (`0x580ec0` after `0x580ae0`).
    let raid = raid_rows(group).filter_map(member(true));
    for ((x, y), raid, name) in party.chain(raid) {
        let d = ((x - ctx.wx).powi(2) + (y - ctx.wy).powi(2)).sqrt();
        if d / ctx.radius_yd <= BLIP_EDGE_RATIO {
            continue; // in range: the dot pass draws it
        }
        let Some(tex) = arrows.get(RimArrow::Group) else {
            continue;
        };
        let at = push_rim_arrow(ctx, [x, y, 0.0], tex, GROUP_ARROW_PX, quads);
        if arrow_hovered(ctx, at) {
            let names = if raid {
                &mut hover.raid
            } else {
                &mut hover.party
            };
            names.push(name);
        }
    }
}

/// Show every hovered blip's name; a unit's resolves through the name cache. The plate follows
/// the cursor by itself, and hover loss hides it at once (`0x57d15e`). Runs after `UnitFeed`, so a
/// same-frame move from a world hover to a blip ends on the blip's tooltip.
pub(super) fn drive_blip_tooltip(
    mut script: crate::ui_script::InTurn,
    hover: Res<MinimapBlipHover>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    let last = last.get(&script);
    let show = if *hover == MinimapBlipHover::default() {
        None
    } else {
        // `colorblindMode` names an unknown flight master in words (`0x57c720`).
        let flight_master = script
            .cvar("colorblindMode")
            .is_some_and(|v| v.trim().parse::<i64>().unwrap_or(0) != 0)
            .then(|| {
                crate::ui_chat::combat::global_string(&script, "MINIMAP_TRACKING_FLIGHTMASTER")
            })
            .flatten();
        hover_tooltip_text(&hover, flight_master.as_deref(), |g| {
            names.resolve(g, &commands).map(str::to_string)
        })
    };
    match (&show, &*last) {
        // Same blips: the cursor mode's layer update keeps the plate on the pointer.
        (Some(t), Some(lt)) if t == lt => {}
        // The tooltip runs its handlers: at the raise step after the feed, with the `World` lent,
        // behind a world hover's plate queued this frame.
        (Some(t), _) => {
            let text = t.clone();
            benilla_ui::script::world_loan::in_turn_or_now(&mut script, move |s| {
                s.minimap_tooltip(&text);
            });
            *last = Some(t.clone());
        }
        (None, Some(_)) => {
            benilla_ui::script::world_loan::in_turn_or_now(&mut script, |s| {
                s.minimap_tooltip_hide();
            });
            *last = None;
        }
        (None, None) => {}
    }
}

/// The colour an other-floor object's entry draws in: the reference wraps it in `|cffb0b0b0`, which
/// its tooltip line multiplies with the gold; benilla's markup replaces the colour instead, so the
/// entry carries that product, gold × 0xb0.
const OTHER_FLOOR_COLOR: &str = "|cffb09000";

/// The minimap tooltip's one text (`0x57c720`), a line per entry: the landmark rim arrows, the
/// party's and the raid's rim arrows, the landmark icons, then the object blips. An object adds
/// ` <flight_master>` for an unknown flight master when given (colour-blind mode) and its tracked
/// trainer's ` <title>`, all inside the grey of an other-floor blip. An object whose name is not
/// yet known is left out; `None` when nothing is left.
fn hover_tooltip_text(
    hover: &MinimapBlipHover,
    flight_master: Option<&str>,
    resolve: impl Fn(u64) -> Option<String>,
) -> Option<String> {
    let mut entries: Vec<String> = hover
        .directions
        .iter()
        .chain(&hover.party)
        .chain(&hover.raid)
        .chain(&hover.landmarks)
        .cloned()
        .collect();
    for h in &hover.objects {
        let Some(name) = (match &h.name {
            HoverName::Known(n) => Some(n.clone()),
            HoverName::Guid(g) => resolve(*g),
        }) else {
            continue;
        };
        let mut entry = String::new();
        if h.grey {
            entry.push_str(OTHER_FLOOR_COLOR);
        }
        entry.push_str(&name);
        if let Some(label) = flight_master.filter(|_| h.flight_master) {
            entry.push_str(&format!("\n <{label}>"));
        }
        if let Some(t) = &h.title {
            entry.push_str(&format!("\n <{t}>"));
        }
        if h.grey {
            entry.push_str("|r");
        }
        entries.push(entry);
    }
    (!entries.is_empty()).then(|| entries.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blip driver is a feed that holds the VM in turn: the tooltip's show and its hover-loss
    /// hide run their handlers at the raise step after it, never inside the step.
    #[test]
    fn the_blip_tooltip_runs_its_handlers_at_the_raise() {
        use benilla_ui::script::UiScript;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut w = World::new();
        w.init_resource::<NameCache>();
        w.init_resource::<crate::ui_script::live::UiEvents>();
        w.insert_resource(NetCommands(tx));
        w.insert_resource(MinimapBlipHover {
            landmarks: vec!["Stormwind".into()],
            ..Default::default()
        });
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                Seen = {}
                UIParent = CreateFrame("Frame", "UIParent"); UIParent:SetAllPoints()
                CreateFrame("Frame", "Minimap", UIParent)
                local tt = CreateFrame("GameTooltip", "GameTooltip", UIParent); tt:Hide()
                tt:SetScript("OnShow", function() Seen[#Seen + 1] = "show" end)
                tt:SetScript("OnHide", function() Seen[#Seen + 1] = "hide" end)
            "#,
            )
            .unwrap();
        w.insert_non_send_resource(script);
        let seen = |w: &World| {
            w.non_send_resource::<UiScript>()
                .eval::<String>("return table.concat(Seen, ',')")
                .unwrap()
        };
        // One system across both runs: its memory of the last blip arms the hide.
        let drive = w.register_system(drive_blip_tooltip);
        w.run_system(drive).unwrap();
        assert_eq!(seen(&w), "", "nothing runs inside the feed");
        crate::ui_script::raises::fire_raises(&mut w);
        assert_eq!(seen(&w), "show");
        *w.resource_mut::<MinimapBlipHover>() = MinimapBlipHover::default();
        w.run_system(drive).unwrap();
        crate::ui_script::raises::fire_raises(&mut w);
        assert_eq!(seen(&w), "show,hide");
    }

    /// The raid list is the roster outside our own subgroup; the party slots carry the rest.
    #[test]
    fn raid_rows_are_the_members_outside_our_subgroup() {
        use benilla_protocol::messages::GroupMemberEntry;
        let row = |guid: u64, flags: u8| GroupMemberEntry {
            name: String::new(),
            guid,
            status: 1,
            flags,
            roles: 0,
        };
        let group = crate::ui_party::GroupState {
            own_flags: 0x81, // subgroup 1, assistant
            members: vec![row(2, 0x01), row(3, 0x00), row(4, 0x82), row(5, 0x81)],
            ..Default::default()
        };
        let raid: Vec<u64> = raid_rows(&group).map(|m| m.guid).collect();
        assert_eq!(
            raid,
            [3, 4],
            "subgroup bits alone decide, the assistant bit ignored"
        );
        let party: Vec<u64> = group.party_slots().map(|m| m.guid).collect();
        assert_eq!(party, [2, 5]);
    }

    /// The call site gives only the sequence id; the model says what it draws.
    #[test]
    fn each_rim_arrow_sequence_lights_exactly_its_own_layer() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::Chain::open(&data).expect("the patch chain");
        let bytes =
            benilla_formats::read_m2(&chain, "Interface\\Minimap\\Rotating-MinimapArrow.m2")
                .expect("the one rim-arrow model");

        for kind in RimArrow::ALL {
            let shown = benilla_formats::m2_sequence_visible_textures(&bytes, kind.anim_id())
                .unwrap_or_else(|| {
                    panic!(
                        "{kind:?}: the model authors no sequence {:#x} — the id \
                    is wrong, or this is not the model the reference animates",
                        kind.anim_id()
                    )
                });
            let want = format!("{}.BLP", kind.texture().to_uppercase());
            assert_eq!(
                shown,
                vec![want],
                "{kind:?} (sequence {:#x}) must light its layer and ONLY its layer",
                kind.anim_id()
            );
        }
    }

    fn marker_only(marker: &AreaPoi) -> StaticPois<'_> {
        StaticPois {
            marker: Some(marker),
            ..Default::default()
        }
    }

    /// A static slot's shape (`0x7f4990`, `0x7f4a00`): icon 8, importance -1, flags 2.
    fn static_slot(x: f32, name: &str) -> AreaPoi {
        AreaPoi {
            icon: 8,
            ..poi(u32::MAX, 0x2, x, name)
        }
    }

    /// The corpse alone is spared the 694.444-yd cut (`0x7f46b1`); importance -1 ranks it first.
    #[test]
    fn the_corpse_slot_skips_the_rank_cut_and_ranks_first() {
        let rows = [
            poi(0, 1, 300.0, "minor-near"),
            poi(0, 1, 400.0, "minor-mid"),
        ];
        let corpse = static_slot(2000.0, "Corpse");
        let release = static_slot(2000.0, "Spirit Healer");
        let statics = StaticPois {
            corpse: Some(&corpse),
            release: Some(&release),
            ..Default::default()
        };
        let sel = select_landmarks(rows.iter(), statics, 0, 0.0, 0.0, 133.0, |_| 0);
        let names: Vec<&str> = sel
            .arrows
            .iter()
            .map(|(_, p, _, _)| p.name.as_str())
            .collect();
        assert_eq!(names, ["Corpse", "minor-near", "minor-mid"]);
        assert_eq!(sel.arrows[0].2, RimArrow::Corpse);
    }

    fn hover_ctx(cursor: Vec2) -> BlipCtx {
        BlipCtx {
            center: Vec2::new(100.0, 100.0),
            side: BLIP_BASIS_PX,
            px_per_yd: 0.5,
            radius_yd: 140.8,
            z: 0,
            alpha: 1.0,
            wx: 0.0,
            wy: 0.0,
            cursor: Some(cursor),
            seam: 1.0,
            turn: 0.0,
        }
    }

    fn hover_at(cursor: Vec2, statics: StaticPois) -> MinimapBlipHover {
        let atlas = PoiAtlas {
            tex: Handle::default(),
            width: 256,
        };
        let mut arrows = RimArrowArt::default();
        arrows.set(RimArrow::Corpse, Some(Handle::default()));
        let mut quads = UiQuads::default();
        let mut hover = MinimapBlipHover::default();
        let ctx = hover_ctx(cursor);
        let sel = select_landmarks(
            std::iter::empty(),
            statics,
            0,
            ctx.wx,
            ctx.wy,
            ctx.radius_yd,
            |_| 0,
        );
        emit_landmark_icons(&ctx, &sel, Some(&atlas), &mut quads, &mut hover);
        emit_landmark_arrows(&ctx, &sel, &arrows, false, &mut quads, &mut hover);
        hover
    }

    /// The in-range corpse icon names itself on hover, as every POI in the tooltip walk does
    /// (`0x57c720`, the POI's name at `+0x3c`); off its 16-px quad nothing is hovered.
    #[test]
    fn the_in_range_corpse_icon_hovers_as_its_name() {
        let corpse = static_slot(40.0, "Corpse"); // 20 px north of the centre
        let statics = StaticPois {
            corpse: Some(&corpse),
            ..Default::default()
        };
        let on = hover_at(Vec2::new(103.0, 84.0), statics);
        assert_eq!(on.landmarks, ["Corpse"]);
        assert!(on.directions.is_empty());
        let off = hover_at(Vec2::new(100.0, 100.0), statics);
        assert_eq!(off, MinimapBlipHover::default());
    }

    /// Out of range the corpse's rim arrow carries the same name.
    #[test]
    fn the_corpse_rim_arrow_hovers_as_its_name() {
        let corpse = static_slot(1000.0, "Corpse"); // north, past the 0.8 rim
        let statics = StaticPois {
            corpse: Some(&corpse),
            ..Default::default()
        };
        let rim = 100.0 - BLIP_BASIS_PX * 0.5 * BLIP_EDGE_RATIO;
        let on = hover_at(Vec2::new(100.0, rim), statics);
        assert_eq!(on.directions, ["Corpse"]);
        assert!(on.landmarks.is_empty());
    }

    /// A rim arrow hovers within 6.4 px of its centre on each axis (`0x57c720`), not over its
    /// whole 38.4-px art.
    #[test]
    fn a_rim_arrow_hovers_only_near_its_centre() {
        let corpse = static_slot(1000.0, "Corpse");
        let statics = StaticPois {
            corpse: Some(&corpse),
            ..Default::default()
        };
        let rim = 100.0 - BLIP_BASIS_PX * 0.5 * BLIP_EDGE_RATIO;
        assert_eq!(
            hover_at(Vec2::new(106.0, rim), statics).directions,
            ["Corpse"]
        );
        assert!(hover_at(Vec2::new(107.0, rim), statics)
            .directions
            .is_empty());
        assert!(hover_at(Vec2::new(100.0, rim + 15.0), statics)
            .directions
            .is_empty());
    }

    /// One text for everything hovered, in the reference's order (`0x57c720`): the landmark rim
    /// arrows, the party's, the raid's, the landmark icons, then the objects; an other-floor
    /// object's whole entry in the faint colour, an unknown flight master named in colour-blind
    /// mode, a tracked trainer's title, and an object whose name is unknown left out.
    #[test]
    fn the_tooltip_lists_every_hovered_blip_in_the_reference_order() {
        let hover = MinimapBlipHover {
            directions: vec!["Stormwind".into()],
            party: vec!["Anna".into()],
            raid: vec!["Bert".into()],
            landmarks: vec!["Crown Guard Tower".into()],
            objects: vec![
                ObjectHover {
                    name: HoverName::Guid(7),
                    grey: false,
                    flight_master: false,
                    title: Some("Weapon Master".into()),
                },
                ObjectHover {
                    name: HoverName::Guid(8),
                    grey: false,
                    flight_master: false,
                    title: None,
                },
                ObjectHover {
                    name: HoverName::Known("Thor".into()),
                    grey: true,
                    flight_master: true,
                    title: None,
                },
            ],
        };
        let resolve = |g: u64| (g == 7).then(|| "Woo Ping".to_string());
        let text = hover_tooltip_text(&hover, Some("Flight Master"), resolve).unwrap();
        assert_eq!(
            text,
            "Stormwind\nAnna\nBert\nCrown Guard Tower\nWoo Ping\n <Weapon Master>\n\
             |cffb09000Thor\n <Flight Master>|r"
        );
        let plain = hover_tooltip_text(&hover, None, resolve).unwrap();
        assert!(
            plain.ends_with("|cffb09000Thor|r"),
            "no colour-blind line: {plain:?}"
        );
        let unknown = MinimapBlipHover {
            objects: vec![hover.objects[1].clone()],
            ..Default::default()
        };
        assert_eq!(hover_tooltip_text(&unknown, None, resolve), None);
    }

    /// A party member past the rim hovers its arrow by name, and a raid row lands on the raid's
    /// list; a member inside the rim draws a dot, not an arrow.
    #[test]
    fn a_group_rim_arrow_hovers_as_its_member() {
        use benilla_protocol::messages::{GroupMemberEntry, PartyMemberStatsInfo};
        use bevy::ecs::system::RunSystemOnce;
        let row = |name: &str, guid: u64, flags: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 1,
            flags,
            roles: 0,
        };
        let at = |x: i16| PartyMemberStatsInfo {
            position: Some((x, 0)),
            ..Default::default()
        };
        let group = crate::ui_party::GroupState {
            own_flags: 0x00,
            members: vec![
                row("Anna", 2, 0x00),
                row("Bert", 3, 0x01),
                row("Cleo", 4, 0x00),
            ],
            // North past the rim, north past the rim, and in range.
            stats: [(2, at(1000)), (3, at(1000)), (4, at(10))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let mut arrows = RimArrowArt::default();
        arrows.set(RimArrow::Group, Some(Handle::default()));
        let rim = 100.0 - BLIP_BASIS_PX * 0.5 * BLIP_EDGE_RATIO;
        let mut world = World::new();
        let hover = world
            .run_system_once(move |unit_pos: Query<&GlobalTransform, With<NetEntity>>| {
                let mut quads = UiQuads::default();
                let mut hover = MinimapBlipHover::default();
                emit_group_arrows(
                    &hover_ctx(Vec2::new(100.0, rim)),
                    &group,
                    &[],
                    &GuidIndex::default(),
                    &unit_pos,
                    &arrows,
                    &mut quads,
                    &mut hover,
                );
                hover
            })
            .unwrap();
        assert_eq!(hover.party, ["Anna"]);
        assert_eq!(hover.raid, ["Bert"]);
    }

    /// Indoors a landmark's rim arrow neither draws nor hovers; the corpse's and a
    /// `Flags & 0x100` row's still do (`0x57d860`, `0x57c720`).
    #[test]
    fn indoors_only_the_always_shown_rim_arrows_draw() {
        let corpse = static_slot(1000.0, "Corpse");
        let shown = poi(0, 0x101, 600.0, "Shown");
        let rows = [poi(0, 0x1, 600.0, "Hidden"), shown];
        let statics = StaticPois {
            corpse: Some(&corpse),
            ..Default::default()
        };
        let sel = select_landmarks(rows.iter(), statics, 0, 0.0, 0.0, 133.0, |_| 1);
        let mut arrows = RimArrowArt::default();
        for art in RimArrow::ALL {
            arrows.set(art, Some(Handle::default()));
        }
        let drawn = |indoors: bool| {
            let mut quads = UiQuads::default();
            let mut hover = MinimapBlipHover::default();
            emit_landmark_arrows(
                &hover_ctx(Vec2::ZERO),
                &sel,
                &arrows,
                indoors,
                &mut quads,
                &mut hover,
            );
            quads.overlays.len()
        };
        assert_eq!(sel.arrows.len(), 3);
        let flags: Vec<(&str, bool)> = sel
            .arrows
            .iter()
            .map(|(_, p, _, a)| (p.name.as_str(), *a))
            .collect();
        assert!(
            flags.contains(&("Corpse", true)) && flags.contains(&("Shown", true)),
            "{flags:?}"
        );
        assert_eq!(drawn(false), 3);
        assert_eq!(drawn(true), 2);
    }

    /// An in-range group dot names its member on the object half of the hover, the raid's list
    /// before the party's, even with no class to draw (`0x57c720` walks the lists, not the quads).
    #[test]
    fn an_in_range_group_dot_hovers_as_its_member() {
        use benilla_protocol::messages::{GroupMemberEntry, PartyMemberStatsInfo};
        use bevy::ecs::system::RunSystemOnce;
        let row = |name: &str, guid: u64, flags: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 1,
            flags,
            roles: 0,
        };
        let at = |x: i16| PartyMemberStatsInfo {
            position: Some((x, 0)),
            ..Default::default()
        };
        let group = crate::ui_party::GroupState {
            own_flags: 0x00,
            members: vec![
                row("Anna", 2, 0x00),
                row("Bert", 3, 0x01),
                row("Cleo", 4, 0x00),
            ],
            // Anna and Bert a yard north of us, Cleo past the rim.
            stats: [(2, at(1)), (3, at(1)), (4, at(1000))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let mut world = World::new();
        let hits = world
            .run_system_once(move |unit_pos: Query<&GlobalTransform, With<NetEntity>>| {
                let mut quads = UiQuads::default();
                let mut hits = Vec::new();
                dots::emit_group_dots(
                    &hover_ctx(Vec2::new(100.0, 100.0)),
                    &group,
                    &[],
                    &GuidIndex::default(),
                    &unit_pos,
                    |_| None,
                    &NameCache::default(),
                    None,
                    &mut quads,
                    &mut hits,
                );
                hits
            })
            .unwrap();
        let names: Vec<_> = hits.iter().map(|h| h.name.clone()).collect();
        assert_eq!(
            names,
            [
                HoverName::Known("Bert".into()),
                HoverName::Known("Anna".into())
            ]
        );
    }

    fn poi(importance: u32, flags: u32, x: f32, name: &str) -> AreaPoi {
        AreaPoi {
            importance,
            icon: 6,
            faction_id: 0,
            pos: [x, 0.0, 0.0],
            continent_id: 0,
            flags,
            area_id: 0,
            name: name.into(),
            description: String::new(),
            world_state_id: 0,
            ..Default::default()
        }
    }

    /// Real 1.12 AreaPOI rows near Northshire Abbey at the default zoom's 133-yd radius.
    #[test]
    fn northshire_shape_under_the_byte_law() {
        let rows = [
            poi(3, 0x5, 46.0, "Northshire Abbey"),
            poi(0, 0x4, 237.0, "Echo Ridge Mine"),
            poi(3, 0x1d, 556.0, "Stormwind"),
            poi(3, 0x5, 601.0, "Goldshire"),
        ];
        let sel = select_landmarks(
            rows.iter(),
            StaticPois::default(),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert!(sel.icons.is_empty(), "no Flags&2 row is in range");
        let names: Vec<&str> = sel
            .arrows
            .iter()
            .map(|(_, p, _, _)| p.name.as_str())
            .collect();
        assert_eq!(names, ["Stormwind", "Goldshire"]);
    }

    /// The rank key is `Importance` (DBC column 1), not AreaID; distance only breaks ties.
    #[test]
    fn arrow_rank_is_importance_then_distance_capped_at_three() {
        let rows = [
            poi(3, 1, 200.0, "city-near"),
            poi(0, 1, 650.0, "minor-far"),
            poi(0, 1, 300.0, "minor-near"),
            poi(3, 1, 400.0, "city-mid"),
            poi(0, 1, 695.0, "beyond-rank"),
        ];
        let sel = select_landmarks(
            rows.iter(),
            StaticPois::default(),
            0,
            0.0,
            0.0,
            100.0,
            |_| 0,
        );
        let names: Vec<&str> = sel
            .arrows
            .iter()
            .map(|(_, p, _, _)| p.name.as_str())
            .collect();
        assert_eq!(names, ["minor-near", "minor-far", "city-near"]);
    }

    /// The marker carries `Importance` 0, so it outranks the cities though farthest.
    #[test]
    fn the_guard_marker_competes_for_a_rim_slot_and_outranks_the_cities() {
        let cities = [
            poi(3, 0x1d, 200.0, "Stormwind"),
            poi(3, 0x5, 300.0, "Goldshire"),
            poi(3, 0x5, 400.0, "Northshire Abbey"),
        ];
        let marker = poi(0, 0x63, 500.0, "Stormwind Warrior Trainer");
        let sel = select_landmarks(
            cities.iter(),
            marker_only(&marker),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        let names: Vec<&str> = sel
            .arrows
            .iter()
            .map(|(_, p, _, _)| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Stormwind Warrior Trainer", "Stormwind", "Goldshire"],
            "Importance 0 ranks the marker first; the third city is crowded out"
        );
        let arts: Vec<RimArrow> = sel.arrows.iter().map(|&(_, _, a, _)| a).collect();
        assert_eq!(
            arts,
            [RimArrow::Guide, RimArrow::Landmark, RimArrow::Landmark],
            "the guard's directions draw the guide arrow; DBC rows draw the landmark arrow"
        );
    }

    /// Every 1.12 `points_of_interest` row carries flags 99 (0x63), so `Flags & 2` is set.
    #[test]
    fn the_guard_marker_draws_its_icon_in_range() {
        let marker = poi(0, 0x63, 50.0, "The Bank");
        let sel = select_landmarks(
            std::iter::empty(),
            marker_only(&marker),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert!(sel.arrows.is_empty(), "in range — no rim arrow");
        assert_eq!(sel.icons.len(), 1);
        assert_eq!(sel.icons[0].1, 6);
    }

    /// The caller ([`crate::poi_marker::PoiMarker`]'s `on_map`) decides the marker's map.
    #[test]
    fn the_guard_marker_skips_the_candidacy_filter() {
        let mut marker = poi(0, 0, 300.0, "The Inn"); // Flags bit 0 clear: a DBC row would drop
        marker.continent_id = 571; // and a continent that isn't the displayed one
        let sel = select_landmarks(
            std::iter::empty(),
            marker_only(&marker),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert_eq!(sel.arrows.len(), 1, "appended unconditionally");
    }

    #[test]
    fn in_range_icons_gate_on_flag_bit1_and_the_atlas_bound() {
        let mut tower = poi(0, 0x87, 50.0, "Crown Guard Tower");
        tower.icon = 9;
        let plain = poi(3, 0x5, 50.0, "Northshire Abbey");
        let rows = [tower, plain];
        let sel = select_landmarks(
            rows.iter(),
            StaticPois::default(),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert_eq!(sel.arrows.len(), 0);
        assert_eq!(sel.icons.len(), 1, "only the Flags&2 tower draws in range");
    }

    /// Isle of Conquest's Alliance Keep as `AreaPOI.dbc` 2383-2387 carry it: one row per node
    /// state, each with its own world state. Only the row whose state is set draws (`0x7f44a0`).
    fn ioc_alliance_keep() -> Vec<AreaPoi> {
        [(6, 4341), (11, 4339), (9, 4342), (12, 4343), (10, 4340)]
            .into_iter()
            .map(|(icon, ws)| {
                let mut row = poi(0, 0x3af, 50.0, "Alliance Keep");
                row.icons[0] = icon;
                row.icon = icon;
                row.world_state_id = ws;
                row
            })
            .collect()
    }

    #[test]
    fn a_landmark_whose_world_state_reads_zero_is_neither_drawn_nor_on_the_rim() {
        let rows = ioc_alliance_keep();
        // 4339, the keep held by the Alliance, alone is set.
        let ws = |id: u32| i32::from(id == 4339);
        let sel = select_landmarks(rows.iter(), StaticPois::default(), 0, 0.0, 0.0, 133.0, ws);
        let icons: Vec<u32> = sel.icons.iter().map(|&(_, i)| i).collect();
        assert_eq!(
            icons,
            [11],
            "only the held row draws, not the last of the five"
        );
        let far = select_landmarks(rows.iter(), StaticPois::default(), 0, 0.0, 0.0, 10.0, ws);
        assert_eq!(
            far.arrows.len(),
            1,
            "the unset rows take no rim slot either"
        );
        let none = select_landmarks(
            rows.iter(),
            StaticPois::default(),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert!(none.icons.is_empty() && none.arrows.is_empty());
    }

    #[test]
    fn a_landmark_draws_the_icon_its_world_state_picks() {
        let mut row = poi(0, 0x3, 50.0, "Tower");
        row.world_state_id = 7;
        row.icons = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        let pick = |v: i32| {
            let rows = [row.clone()];
            let sel = select_landmarks(
                rows.iter(),
                StaticPois::default(),
                0,
                0.0,
                0.0,
                133.0,
                |_| v,
            );
            sel.icons.first().map(|&(_, i)| i)
        };
        assert_eq!(pick(1), Some(1), "state 1 is the first column");
        assert_eq!(pick(9), Some(9), "state 9 is the last");
        assert_eq!(pick(10), Some(1), "past the nine falls back to the first");
        assert_eq!(
            pick(-3),
            Some(1),
            "a negative state falls back to the first"
        );
        assert_eq!(pick(0), None, "a zero state hides the row");
    }

    #[test]
    fn a_landmark_at_the_origin_is_skipped() {
        let rows = [poi(0, 0x3, 0.0, "Unplaced")];
        let sel = select_landmarks(
            rows.iter(),
            StaticPois::default(),
            0,
            0.0,
            0.0,
            133.0,
            |_| 0,
        );
        assert!(sel.icons.is_empty() && sel.arrows.is_empty());
    }

    /// 14 cells a row of 18 texels, inset one texel, rows cut from the width too.
    #[test]
    fn poi_cells_are_fourteen_a_row_of_eighteen_texels_inset_one() {
        let w = 256.0;
        assert_eq!(
            poi_icon_cell(0, 256),
            Some([1.0 / w, 18.0 / w, 1.0 / w, 18.0 / w])
        );
        // 15 = col 1 row 1.
        assert_eq!(
            poi_icon_cell(15, 256),
            Some([19.0 / w, 36.0 / w, 19.0 / w, 36.0 / w])
        );
        // The corpse's icon 8 sits on row 0, not the 8x8 grid's row 1.
        assert_eq!(
            poi_icon_cell(8, 256),
            Some([145.0 / w, 162.0 / w, 1.0 / w, 18.0 / w])
        );
        // The last cell, col 13 row 13, ends at texel 252.
        let last = poi_icon_cell(195, 256).unwrap();
        assert!((last[1] - 252.0 / w).abs() < 1e-6 && (last[3] - 252.0 / w).abs() < 1e-6);
        assert_eq!(poi_icon_cell(196, 256), None, "the draw gates Icon < 196");
        // A wider texture narrows the cells; rows follow the width, not the height.
        assert_eq!(
            poi_icon_cell(14, 512),
            Some([1.0 / 512.0, 18.0 / 512.0, 19.0 / 512.0, 36.0 / 512.0])
        );
    }
}
