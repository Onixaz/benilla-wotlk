//! The minimap blip layer: AreaPOI landmarks, quest and tracking dots, rim arrows and the hover
//! tooltip.
//!
//! Landmarks:
//! - Candidates: `ContinentID` is the current map and `Flags & 1`, no faction or importance gate.
//! - In range (`d / viewRadius ≤ 0.8`): the `POIIcons.blp` cell of the `Icon` column, only for
//!   rows with `Flags & 2` (the world-PvP towers in 1.12 data); other rows draw nothing.
//! - Out of range within 694.444 yd: the first 3 by (`Importance` signed ascending, distance),
//!   on the 0.8 rim, each rotated to point at its POI in its source's art ([`RimArrow`]).
//!
//! The object blips are [`dots`]': the town tracking type, quest status, an unlearned flight
//! master and the tracking masks each give a cell of `ObjectIcons.blp`, culled at 0.8 of the view
//! radius and drawn last.
//!
//! Blip sizes are frozen once by the minimap frame's constructor against a 140.8-px basis.
//!
//! Not built: the landmark `WorldStateID` gate; the per-source rim-arrow z-order (bottom to top
//! landmark, party, quest, gossip, corpse, from `+0x48` as a frame-level offset), which conflicts
//! with the reference drawing the dots last and is unsettled; and static blip slot 0,
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
use benilla_formats::{AreaPoi, AreaPoiCatalog};
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
    /// Your own corpse: sequence `0xa7`, `+0x48 == 2`.
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

    /// Any art at all: the landmark pass runs without `AreaPOI.dbc`, for the guard marker.
    pub(super) fn any(&self) -> bool {
        self.0.iter().any(Option::is_some)
    }
}
/// An in-range POI icon's quad, 16 px (frozen by the constructor), the corpse's too (`0x5827e2`).
pub(super) const POI_ICON_PX: f32 = 16.0;
/// The corpse's static POI icon (`0x7f4990`).
pub(super) const CORPSE_POI_ICON: u32 = 8;

/// The blip under the cursor this frame, for [`drive_blip_tooltip`].
#[derive(Resource, Default)]
pub(crate) enum MinimapBlipHover {
    #[default]
    None,
    /// A landmark's name and the cursor's UI-space point, the tooltip's seat.
    Landmark(String, Vec2),
    /// The object blips under the cursor in draw order, and the cursor point (`0x57c720`).
    Objects(Vec<ObjectHover>, Vec2),
}

/// One hovered object blip: its name, whether it drew grey, and a tracked trainer's title.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectHover {
    pub(crate) name: HoverName,
    pub(crate) grey: bool,
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
    Res<'w, crate::ui_quest::QuestGiver>,
    Res<'w, GuidIndex>,
    Query<'w, 's, &'static GlobalTransform, With<NetEntity>>,
    Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    ResMut<'w, MinimapBlipHover>,
    Res<'w, crate::ui_script::UiScaleCvar>,
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
    Option<Res<'w, crate::area_poi::AreaPoiRes>>,
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
    /// The cursor in y-up UI space; the reference seats the blip tooltip at the cursor, its exact
    /// offset untraced.
    pub(super) cursor_ui: Option<Vec2>,
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
    /// In-range rows with [`FLAG_IN_RANGE_ICON`], drawn at their position.
    pub(super) icons: Vec<&'a AreaPoi>,
    /// The rim arrows `(dist, poi, art)` in rank order.
    pub(super) arrows: Vec<(f32, &'a AreaPoi, RimArrow)>,
}

/// The landmark selection (module doc). `marker`, the guard-directions POI, is appended after the
/// DBC scan as the reference appends its static blip slot: it skips the candidacy gate but not the
/// 694.444-yd cut, which the reference spares only for the corpse slot.
pub(super) fn select_landmarks<'a>(
    pois: impl Iterator<Item = &'a AreaPoi>,
    marker: Option<&'a AreaPoi>,
    map_id: u32,
    wx: f32,
    wy: f32,
    radius_yd: f32,
) -> LandmarkSelection<'a> {
    let mut icons = Vec::new();
    let mut arrows: Vec<(f32, &AreaPoi, RimArrow)> = Vec::new();
    let candidates = pois
        .filter(|p| p.continent_id == map_id && p.flags & FLAG_CANDIDATE != 0)
        .map(|p| (p, RimArrow::Landmark))
        .chain(marker.map(|m| (m, RimArrow::Guide)));
    for (p, art) in candidates {
        let d = ((p.pos[0] - wx).powi(2) + (p.pos[1] - wy).powi(2)).sqrt();
        if d / radius_yd <= BLIP_EDGE_RATIO {
            if p.flags & FLAG_IN_RANGE_ICON != 0 {
                icons.push(p);
            }
        } else if d <= LANDMARK_RANK_YD {
            arrows.push((d, p, art));
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

/// Draw the in-range POI icons, then the rim arrows; the later-drawn hover hit wins.
pub(super) fn emit_landmarks(
    ctx: &BlipCtx,
    cat: Option<&AreaPoiCatalog>,
    marker: Option<&AreaPoi>,
    map_id: u32,
    arrows: &RimArrowArt,
    poi_icons: Option<&PoiAtlas>,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    let sel = select_landmarks(
        cat.into_iter().flat_map(|c| c.rows().map(|(_, p)| p)),
        marker,
        map_id,
        ctx.wx,
        ctx.wy,
        ctx.radius_yd,
    );
    if let Some(atlas) = poi_icons {
        for poi in &sel.icons {
            let Some(cell) = atlas.cell(poi.icon) else {
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
            if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
                if rect.contains(c) {
                    *hover = MinimapBlipHover::Landmark(poi.name.clone(), ui);
                }
            }
        }
    }
    for (_d, poi, art) in &sel.arrows {
        let Some(tex) = arrows.get(*art) else {
            continue;
        };
        let rect = push_rim_arrow(ctx, poi.pos, tex, ARROW_QUAD_PX, quads);
        if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
            if rect.contains(c) {
                *hover = MinimapBlipHover::Landmark(poi.name.clone(), ui);
            }
        }
    }
}

/// One rim arrow of `px` on the 0.8 rim, pointing at its target, drawn in one pass as only one of
/// the model's layers is ever opaque. Returns its rect for the hover test.
fn push_rim_arrow(
    ctx: &BlipCtx,
    target: [f32; 3],
    tex: &Handle<Image>,
    px: f32,
    quads: &mut UiQuads,
) -> Rect {
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
    rect
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

/// The party, corpse and raid rim arrows, the out-of-range half of the group placement, drawn
/// before the player arrow. A member a tracked vehicle stands for draws none (`0x7f3f40`).
///
/// This differs from the reference, not built yet: its slot 4 is the pet (`UNIT_FIELD_CHARM`, else
/// `UNIT_FIELD_SUMMON`), and the corpse is static blip slot 2 on the POI path, at `Importance` -1
/// and exempt from the 694.444-yd cut, taking one of the three rim slots. So there is no pet blip
/// here, and four rim arrows can draw where the reference draws three.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_group_arrows(
    ctx: &BlipCtx,
    group: &crate::ui_party::GroupState,
    possessors: &[u64],
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
    corpse: Option<[f32; 3]>,
    arrows: &RimArrowArt,
    quads: &mut UiQuads,
) {
    let member = |m: &benilla_protocol::messages::GroupMemberEntry| {
        (!possessors.contains(&m.guid))
            .then(|| party_member_pos(m, group, guids, unit_pos))
            .flatten()
            .map(|p| (p, RimArrow::Group, GROUP_ARROW_PX))
    };
    let party = group.party_slots().filter_map(member);
    let corpse = corpse.map(|c| ((c[0], c[1]), RimArrow::Corpse, ARROW_QUAD_PX));
    // The raid's arrows draw after the party's (`0x580ec0` after `0x580ae0`).
    let raid = raid_rows(group).filter_map(member);
    for ((x, y), art, px) in party.chain(corpse).chain(raid) {
        let d = ((x - ctx.wx).powi(2) + (y - ctx.wy).powi(2)).sqrt();
        if d / ctx.radius_yd <= BLIP_EDGE_RATIO {
            continue; // in range: the dot pass draws it
        }
        let Some(tex) = arrows.get(art) else {
            continue;
        };
        push_rim_arrow(ctx, [x, y, 0.0], tex, px, quads);
    }
}

/// Show the hovered blip's name; a unit's resolves through the name cache. Hover loss fades the
/// tooltip, never hides it at once. Runs after `UnitFeed`, so a same-frame move from a world
/// hover to a blip ends on the blip's tooltip.
pub(super) fn drive_blip_tooltip(
    script: Option<NonSendMut<UiScript>>,
    hover: Res<MinimapBlipHover>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<(String, Vec2)>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let show = match &*hover {
        MinimapBlipHover::None => None,
        MinimapBlipHover::Landmark(name, at) => Some((name.clone(), *at, false)),
        MinimapBlipHover::Objects(hits, at) => object_tooltip_text(hits, &names, &commands)
            .map(|t| (t, *at, hits.iter().all(|h| h.grey))),
    };
    match (&show, &*last) {
        // Same blip: the reference's blip tooltip follows the cursor.
        (Some((t, at, _)), Some((lt, lat))) if t == lt => {
            if at != lat {
                script.world_tooltip_move(at.x, at.y);
                *last = Some((t.clone(), *at));
            }
        }
        (Some((t, at, grey)), _) => {
            script.minimap_tooltip(t, at.x, at.y, *grey);
            *last = Some((t.clone(), *at));
        }
        (None, Some(_)) => {
            script.world_tooltip_fade();
            *last = None;
        }
        (None, None) => {}
    }
}

/// The object blips' tooltip text (`0x57c720`): one name a line, a tracked trainer's title on
/// the next as ` <title>`; a name not yet cached is skipped.
fn object_tooltip_text(
    hits: &[ObjectHover],
    names: &NameCache,
    commands: &NetCommands,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for h in hits {
        let name = match &h.name {
            HoverName::Known(n) => n.clone(),
            HoverName::Guid(g) => match names.resolve(*g, commands) {
                Some(n) => n.to_string(),
                None => continue,
            },
        };
        lines.push(name);
        if let Some(t) = &h.title {
            lines.push(format!(" <{t}>"));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 133.0);
        assert!(sel.icons.is_empty(), "no Flags&2 row is in range");
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
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
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 100.0);
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
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
        let sel = select_landmarks(cities.iter(), Some(&marker), 0, 0.0, 0.0, 133.0);
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Stormwind Warrior Trainer", "Stormwind", "Goldshire"],
            "Importance 0 ranks the marker first; the third city is crowded out"
        );
        let arts: Vec<RimArrow> = sel.arrows.iter().map(|&(_, _, a)| a).collect();
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
        let sel = select_landmarks(std::iter::empty(), Some(&marker), 0, 0.0, 0.0, 133.0);
        assert!(sel.arrows.is_empty(), "in range — no rim arrow");
        assert_eq!(sel.icons.len(), 1);
        assert_eq!(sel.icons[0].icon, 6);
    }

    /// The caller ([`crate::poi_marker::PoiMarker`]'s `on_map`) decides the marker's map.
    #[test]
    fn the_guard_marker_skips_the_candidacy_filter() {
        let mut marker = poi(0, 0, 300.0, "The Inn"); // Flags bit 0 clear: a DBC row would drop
        marker.continent_id = 571; // and a continent that isn't the displayed one
        let sel = select_landmarks(std::iter::empty(), Some(&marker), 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 1, "appended unconditionally");
    }

    #[test]
    fn in_range_icons_gate_on_flag_bit1_and_the_atlas_bound() {
        let mut tower = poi(0, 0x87, 50.0, "Crown Guard Tower");
        tower.icon = 9;
        let plain = poi(3, 0x5, 50.0, "Northshire Abbey");
        let rows = [tower, plain];
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 0);
        assert_eq!(sel.icons.len(), 1, "only the Flags&2 tower draws in range");
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
