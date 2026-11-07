//! The exterior shadow maps, `extShadowQuality` 1 to 5, on Bevy's directional-light cascades.
//!
//! The reference's shadow cache (`0x875d30` init, `0x875f80` update, `0x874890` bands):
//! - A dynamic map of radius 20 yd about the viewer, every mode past 0. At modes 1 and 2 only
//!   PC/NPC bodies cast into it (query flags 3, `0x7bb9d0`); mode 1 is 1024², mode 2 2048².
//! - Past mode 2, three bands of radius 40, 160 and 640 yd (`0xb1d520`) of environment casters
//!   (terrain, WMOs, doodads). Modes 3 and 4 re-centre a band once the viewer moves more than 2, 4
//!   or 32 yd (squared distances `0xb1d52c`); mode 5 re-centres them every frame on a 2, 4 and
//!   16 yd world grid, the cascaded mode. Mode 3 is 1024², modes 4 and 5 2048².
//! - Each map is an orthographic box `[-r, r]²` deep along the shadow sun, its eye 2000 yd back and
//!   its depth range [1, 4000] (`0x7bac10`). The shadow sun is the map light's direction with its
//!   z scaled by 5 and clamped at −1.2, then normalised (`0x7bb570`).
//!
//! - What casts (`0x7bb9d0`, `0x7bc890`, `0x7bcf20`; `0x7bbc50` draws map-object groups and M2
//!   batches alone, so terrain never casts): a map entity (a unit or GameObject) whose bounding
//!   sphere reaches the band's floor of 0.25, 2 or 10 yd; a WMO group flagged exterior; doodads.
//!   Modes 3 and 4 keep units out of the bands (query flags 8); mode 5 lets each band skip what
//!   lies wholly inside the next-inner map's box, which holds it already.
//! - Modes 3 and 4 keep each band's map until its viewer moves past the update distance, double
//!   buffered and redrawn over 9 or 25 frames; mode 5 redraws every band every frame.
//!
//! Here one `DirectionalLight` carries the maps as cascades: cascade 0 the dynamic map, cascades
//! 1 to 3 the bands. Bevy fits cascades to the camera frustum; [`place_cascades`] replaces them with
//! the reference's viewer-centred boxes. The receivers (`wow_shadow.wgsl`) read the cascades
//! themselves, so nothing of Bevy's PBR shadow lookup runs. At mode 0 no light exists, and Bevy's
//! light passes stay gated off (`world_plugins`).
//!
//! A held band ([`ShadowPlan`]) takes no casters and its layer is loaded, not cleared, so it keeps
//! the map it last drew under the matrix it drew it with. One held band redraws per frame at most,
//! whole, in place of the reference's tiles into a back buffer. The reference also clears every
//! band on a map preload and whenever the loaded chunk rect changes (`s_needsRebuild`), but samples
//! a held band under the current sun (`0x8750b0`), so a still viewer's shadows slide off their
//! casters as the sun turns. Here a held band is sampled under the sun it was drawn with, and
//! redraws once the sun turns past [`BAND_SUN_COS`]. Deviation: a still viewer's band also redraws
//! every [`BAND_REFRESH_SECS`], so casters that stream in after it was drawn reach it, where the
//! reference waits for its chunk rect to move.

use bevy::camera::primitives::{Aabb, CascadesFrusta, Frustum, HalfSpace};
use bevy::camera::visibility::CascadesVisibleEntities;
use bevy::light::cascade::{build_directional_light_cascades, Cascade};
use bevy::light::{
    CascadeShadowConfig, Cascades, DirectionalLightShadowMap, NotShadowCaster,
    SimulationLightSystems,
};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_formats::ModelBlend;

use crate::model_render::{ModelKind, ModelPart};
use crate::view::{Viewer, WorldCamera};
use crate::world_unit::WorldUnit;

/// The live `extShadowQuality` mode, 0 to [`SHADOW_MODE_MAX`]; 0 is the blob shadow alone.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExtShadowQuality(pub u8);

impl Default for ExtShadowQuality {
    /// The registered "0"; `$WOW_EXT_SHADOW` overrides it for the session.
    fn default() -> Self {
        let mode = std::env::var("WOW_EXT_SHADOW")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .map_or(0, |m| m.min(SHADOW_MODE_MAX));
        Self(mode)
    }
}

/// The highest mode, the mode count less one (`0x873f50`).
pub const SHADOW_MODE_MAX: u8 = 5;

/// The dynamic map's radius (`0x875d30`).
pub const DYN_RADIUS: f32 = 20.0;
/// The bands' radii (`0xb1d520`).
pub const BAND_RADIUS: [f32; 3] = [40.0, 160.0, 640.0];
/// The squared distance a held band's viewer moves before it re-centres, modes 3 and 4
/// (`0xb1d52c`).
pub const BAND_UPDATE_DIST_SQ: [f32; 3] = [4.0, 16.0, 1024.0];
/// The world grid each band snaps to at mode 5 (`0x874890`).
pub const BAND_CELL: [f32; 3] = [2.0, 4.0, 16.0];
/// The smallest bounding-sphere radius a map entity casts with into the dynamic map and each band
/// (`0x7bb9d0`, `0x7ba8f0`); doodads and map objects take no floor.
pub(crate) const DYN_MIN_RADIUS: f32 = 0.25;
pub(crate) const BAND_MIN_RADIUS: [f32; 3] = [0.25, 2.0, 10.0];
/// The longest a held band goes undrawn at modes 3 and 4 (module note, "Deviation").
pub(crate) const BAND_REFRESH_SECS: [f32; 3] = [0.25, 0.5, 1.0];
/// A held band redraws once the shadow sun turns more than about half a degree from the one it was
/// drawn under.
pub(crate) const BAND_SUN_COS: f32 = 0.99996;
/// The map eye's distance back along the shadow sun, and the depth range (`0x7bac10`).
const EYE_BACK: f32 = 2000.0;
const DEPTH_NEAR: f32 = 1.0;
const DEPTH_FAR: f32 = 4000.0;

/// A map's side in texels: 2048 at mode 2 and from mode 4, else 1024 (`0x875d30`).
pub fn shadow_map_size(mode: u8) -> usize {
    if mode == 2 || mode >= 4 {
        2048
    } else {
        1024
    }
}

/// Whether the mode draws the environment bands, past mode 2 (`0x875d30`).
pub fn has_bands(mode: u8) -> bool {
    mode > 2
}

/// Whether only PC/NPC bodies cast, modes 1 and 2 (query flags 3, `0x875f80`).
pub fn units_only(mode: u8) -> bool {
    matches!(mode, 1 | 2)
}

/// The program group a mode's receivers load, 0 to 3 (`0xb1d554`).
pub fn shadow_value(mode: u8) -> u8 {
    [0, 1, 1, 2, 2, 3][usize::from(mode.min(SHADOW_MODE_MAX))]
}

/// The cascade count the mode's light carries: the dynamic map, then the three bands.
fn cascade_count(mode: u8) -> usize {
    if has_bands(mode) {
        4
    } else {
        1
    }
}

/// The shadow sun, the direction the light travels: the map light's with WoW z scaled by 5 and
/// clamped at −1.2, then normalised (`0x7bb570`). Bevy y is WoW z.
pub fn shadow_light_dir(sun_travel: Vec3) -> Vec3 {
    let y = (sun_travel.y * 5.0).max(-1.2);
    Vec3::new(sun_travel.x, y, sun_travel.z).normalize_or(Vec3::NEG_Y)
}

/// The dynamic map's centre: the viewer with its WoW x and y rounded to `1/size` yd
/// (`0x875f80`).
pub fn dyn_center(viewer: Vec3, size: usize) -> Vec3 {
    let s = size as f32;
    let snap = |v: f32| (s * v + 0.5).floor() / s;
    let [x, y, z] = bevy_to_wow(viewer);
    wow_to_bevy([snap(x), snap(y), z])
}

/// A mode-5 band's centre: the viewer floored per WoW axis to the band's grid (`0x874890`).
pub fn snapped_band_center(viewer: Vec3, band: usize) -> Vec3 {
    let cell = BAND_CELL[band];
    let [x, y, z] = bevy_to_wow(viewer).map(|v| (v / cell).floor() * cell);
    wow_to_bevy([x, y, z])
}

/// A mode-3/4 band's centre: held until the viewer is more than the band's update distance from
/// it, then the viewer (`0x874890`).
pub fn held_band_center(held: Option<Vec3>, viewer: Vec3, band: usize) -> Vec3 {
    match held {
        Some(c) if c.distance_squared(viewer) <= BAND_UPDATE_DIST_SQ[band] => c,
        _ => viewer,
    }
}

/// One map's cascade: an orthographic box of half-side `radius` about `center`, its eye
/// `EYE_BACK` back along `dir`, depth [`DEPTH_NEAR`, `DEPTH_FAR`] in Bevy's reverse Z, up the
/// WoW +x axis as the reference's look-at takes it (`0x7bac10`).
pub fn map_cascade(center: Vec3, radius: f32, dir: Vec3, size: usize) -> Cascade {
    // WoW +x is Bevy −z; a sun along it would leave the basis degenerate.
    let up = if dir.cross(Vec3::NEG_Z).length_squared() > 1e-6 {
        Vec3::NEG_Z
    } else {
        Vec3::X
    };
    let eye = center - dir * EYE_BACK;
    let cascade_from_world = Mat4::look_to_rh(eye, dir, up);
    let world_from_cascade = cascade_from_world.inverse();
    // Near and far swapped: reverse Z, near → 1, as Bevy's shadow passes compare.
    let clip_from_cascade =
        Mat4::orthographic_rh(-radius, radius, -radius, radius, DEPTH_FAR, DEPTH_NEAR);
    Cascade {
        world_from_cascade,
        clip_from_cascade,
        clip_from_world: clip_from_cascade * cascade_from_world,
        texel_size: 2.0 * radius / size as f32,
    }
}

/// Whether a world-space box (centre, half extents) lies wholly inside a map's box across the
/// light: the band out from it leaves such a caster to this map (`CullInv`, `0x874890`).
pub(crate) fn inside_map(clip_from_world: &Mat4, center: Vec3, half: Vec3) -> bool {
    let c = clip_from_world.project_point3(center);
    // The map is orthographic: clip xy is affine in world, so the box's reach is |M₃ₓ₃|·h.
    let m = clip_from_world.to_cols_array_2d();
    let reach =
        |row: usize| m[0][row].abs() * half.x + m[1][row].abs() * half.y + m[2][row].abs() * half.z;
    c.x.abs() + reach(0) <= 1.0 && c.y.abs() + reach(1) <= 1.0
}

/// The light carrying the maps, while a mode past 0 is set.
#[derive(Component)]
struct ShadowLight;

/// A band as last drawn at modes 3 and 4: its centre, the shadow sun and when.
#[derive(Clone, Copy)]
struct HeldBand {
    center: Vec3,
    dir: Vec3,
    at: f32,
}

/// The bands' held maps at modes 3 and 4; `None` until drawn.
#[derive(Resource, Default)]
struct HeldBands([Option<HeldBand>; 3]);

/// What the maps take this frame, set with the cascades and read by every caster pass.
#[derive(Resource, Clone, Copy, PartialEq, ExtractResource)]
pub(crate) struct ShadowPlan {
    /// Per cascade, drawn this frame; a held band keeps the map it drew last.
    draw: [bool; 4],
    /// Mode 5: a band skips a caster wholly inside the next-inner map's box (`0x874890`).
    pub(crate) exclude_inner: bool,
    /// Modes 3 and 4: the bands take no map entity (query flags 8).
    bands_static: bool,
}

impl Default for ShadowPlan {
    fn default() -> Self {
        Self {
            draw: [true; 4],
            exclude_inner: false,
            bands_static: false,
        }
    }
}

impl ShadowPlan {
    /// Whether cascade `i` is drawn this frame.
    pub(crate) fn draws(&self, i: usize) -> bool {
        self.draw.get(i).copied().unwrap_or(true)
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ExtShadowQuality>()
        .init_resource::<HeldBands>()
        .init_resource::<ShadowPlan>()
        .add_plugins(ExtractResourcePlugin::<ShadowPlan>::default())
        .add_systems(
            PostUpdate,
            (
                sync_shadow_light.run_if(resource_changed::<ExtShadowQuality>),
                mute_blended_parts,
                mute_world_casters.run_if(caster_set_due),
                aim_shadow_light.run_if(light_exists),
            )
                .chain()
                .before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            place_cascades
                .run_if(light_exists)
                .in_set(SimulationLightSystems::UpdateDirectionalLightCascades)
                .after(build_directional_light_cascades),
        )
        .add_systems(
            PostUpdate,
            narrow_cascade_frusta
                .run_if(light_exists)
                .after(SimulationLightSystems::UpdateLightFrusta)
                .before(SimulationLightSystems::CheckLightVisibility),
        )
        .add_systems(
            PostUpdate,
            trim_band_casters
                .run_if(light_exists)
                .after(SimulationLightSystems::CheckLightVisibility),
        );
    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render_app.add_systems(
            bevy::render::Render,
            keep_held_maps.in_set(bevy::render::RenderSystems::PrepareResources),
        );
    }
}

/// A held band's layer loads instead of clearing, so the map it drew last survives the frame.
/// Bevy hands the light the same cached array texture each frame while its shape holds; a new
/// shape comes with a mode change, which redraws every band.
fn keep_held_maps(
    plan: Option<Res<ShadowPlan>>,
    mut views: Query<(&mut bevy::pbr::ShadowView, &bevy::pbr::LightEntity)>,
) {
    let Some(plan) = plan else {
        return;
    };
    for (mut view, light) in &mut views {
        if let bevy::pbr::LightEntity::Directional { cascade_index, .. } = light {
            if !plan.draws(*cascade_index) {
                let target = view.depth_attachment.view.clone();
                view.depth_attachment = bevy::render::texture::DepthAttachment::new(target, None);
            }
        }
    }
}

fn light_exists(lights: Query<(), With<ShadowLight>>) -> bool {
    !lights.is_empty()
}

/// Spawn, reshape or drop the light on a mode change; a fresh mode re-centres every band.
fn sync_shadow_light(
    mut commands: Commands,
    quality: Res<ExtShadowQuality>,
    mut lights: Query<(Entity, &mut CascadeShadowConfig), With<ShadowLight>>,
    mut held: ResMut<HeldBands>,
) {
    let mode = quality.0.min(SHADOW_MODE_MAX);
    held.0 = [None; 3];
    if mode == 0 {
        for (light, _) in &lights {
            commands.entity(light).despawn();
        }
        return;
    }
    commands.insert_resource(DirectionalLightShadowMap {
        size: shadow_map_size(mode),
    });
    // The bounds only size Bevy's cascade list; their values feed nothing of ours.
    let config = CascadeShadowConfig {
        bounds: [DYN_RADIUS, BAND_RADIUS[0], BAND_RADIUS[1], BAND_RADIUS[2]][..cascade_count(mode)]
            .to_vec(),
        overlap_proportion: 0.0,
        minimum_distance: 0.1,
    };
    if let Ok((_, mut existing)) = lights.single_mut() {
        *existing = config;
        return;
    }
    commands.spawn((
        ShadowLight,
        // No illuminance: the light only carries the maps, it lights nothing.
        DirectionalLight {
            illuminance: 0.0,
            shadows_enabled: true,
            ..default()
        },
        config,
        Transform::default(),
    ));
}

/// Turn the light down the shadow sun each frame the lighting moves.
fn aim_shadow_light(
    lighting: Res<crate::lighting::WowLighting>,
    fresh: Query<(), Added<ShadowLight>>,
    mut lights: Query<&mut Transform, With<ShadowLight>>,
) {
    if !lighting.is_changed() && fresh.is_empty() {
        return;
    }
    let aimed_at = Transform::IDENTITY.looking_to(shadow_light_dir(lighting.sun_dir), Vec3::NEG_Z);
    for mut t in &mut lights {
        t.set_if_neq(aimed_at);
    }
}

/// Replace Bevy's frustum-fitted cascades with the reference's maps about the viewer, and decide
/// which maps draw this frame.
#[allow(clippy::too_many_arguments)]
fn place_cascades(
    quality: Res<ExtShadowQuality>,
    lighting: Res<crate::lighting::WowLighting>,
    viewer: Res<Viewer>,
    time: Res<Time<Real>>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut held: ResMut<HeldBands>,
    mut plan: ResMut<ShadowPlan>,
    mut lights: Query<&mut Cascades, With<ShadowLight>>,
) {
    let mode = quality.0.min(SHADOW_MODE_MAX);
    let Some(at) = viewer
        .at
        .or_else(|| camera.iter().next().map(GlobalTransform::translation))
    else {
        return;
    };
    let size = shadow_map_size(mode);
    let dir = shadow_light_dir(lighting.sun_dir);
    let now = time.elapsed_secs();
    let mut next = ShadowPlan {
        draw: [true; 4],
        exclude_inner: mode >= 5,
        bands_static: has_bands(mode) && mode < 5,
    };
    let mut maps = Vec::with_capacity(4);
    maps.push(map_cascade(dyn_center(at, size), DYN_RADIUS, dir, size));
    if has_bands(mode) {
        let redraw = if mode >= 5 {
            [true; 3]
        } else {
            bands_to_draw(&held.0, at, dir, now)
        };
        for (band, &radius) in BAND_RADIUS.iter().enumerate() {
            let (center, band_dir) = if mode >= 5 {
                (snapped_band_center(at, band), dir)
            } else {
                if redraw[band] {
                    let center = held_band_center(held.0[band].map(|h| h.center), at, band);
                    held.0[band] = Some(HeldBand {
                        center,
                        dir,
                        at: now,
                    });
                }
                let h = held.0[band].expect("a band is drawn before it is held");
                (h.center, h.dir)
            };
            next.draw[band + 1] = redraw[band];
            maps.push(map_cascade(center, radius, band_dir, size));
        }
    }
    plan.set_if_neq(next);
    for mut cascades in &mut lights {
        for view in cascades.cascades.values_mut() {
            view.clone_from(&maps);
        }
    }
}

/// Which held bands draw this frame at modes 3 and 4: every band not yet drawn, and at most one
/// more, the innermost whose viewer moved past its update distance (`0x874890`) or whose sun
/// turned, else the innermost due its refresh.
fn bands_to_draw(held: &[Option<HeldBand>; 3], at: Vec3, dir: Vec3, now: f32) -> [bool; 3] {
    let mut draw = held.map(|h| h.is_none());
    let moved = |b: usize| {
        held[b].is_some_and(|h| {
            h.center.distance_squared(at) > BAND_UPDATE_DIST_SQ[b] || h.dir.dot(dir) < BAND_SUN_COS
        })
    };
    let stale = |b: usize| held[b].is_some_and(|h| now - h.at >= BAND_REFRESH_SECS[b]);
    if let Some(b) = (0..3)
        .find(|&b| moved(b))
        .or_else(|| (0..3).find(|&b| stale(b)))
    {
        draw[b] = true;
    }
    draw
}

/// Texels of padding around a narrowed cull rect, for the receivers' filter taps.
const CULL_PAD_TEXELS: f32 = 4.0;

/// Cull each map's casters with its frustum narrowed to the camera's clip frustum, as the
/// reference does under `shadowCull` (`0x7bafd0`): the map renders with its full matrix, but only
/// the rect of it the camera's frustum covers along the light gathers casters, since a receiver on
/// screen samples only there. Bevy culls with `CascadesFrusta` and renders with `Cascades`, so this
/// rewrites the former after Bevy builds it.
///
/// A held band takes nothing: its frustum admits no caster. A band drawn at modes 3 and 4 is kept
/// for later frames, so it gathers its whole box, as the reference's flag-8 queries do.
fn narrow_cascade_frusta(
    quality: Res<ExtShadowQuality>,
    plan: Res<ShadowPlan>,
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut lights: Query<(&Cascades, &mut CascadesFrusta), With<ShadowLight>>,
) {
    let Some((cam_t, Projection::Perspective(p))) = cam.iter().next() else {
        return;
    };
    let corners = Projection::Perspective(p.clone())
        .get_frustum_corners(-p.near, -p.far)
        .map(|c| cam_t.affine().transform_point3a(c));
    let pad = CULL_PAD_TEXELS * 2.0 / shadow_map_size(quality.0.min(SHADOW_MODE_MAX)) as f32;
    for (cascades, mut frusta) in &mut lights {
        for (view, maps) in &cascades.cascades {
            let Some(view_frusta) = frusta.frusta.get_mut(view) else {
                continue;
            };
            for (i, (map, frustum)) in maps.iter().zip(view_frusta.iter_mut()).enumerate() {
                if !plan.draws(i) {
                    *frustum = empty_frustum();
                } else if i > 0 && plan.bands_static {
                    continue;
                } else if let Some(narrowed) = narrowed_cull(&map.clip_from_world, &corners, pad) {
                    *frustum = narrowed;
                }
            }
        }
    }
}

/// A frustum no box meets: every half-space lies beyond any finite box, up a unit normal (a zero
/// normal would normalise to NaN and admit everything).
fn empty_frustum() -> Frustum {
    Frustum {
        half_spaces: [HalfSpace::new(Vec4::new(0.0, 1.0, 0.0, f32::MIN)); 6],
    }
}

/// A map's cull frustum shrunk to the clip-space XY rect of the camera frustum's corners, padded
/// and clamped to the map; `None` when the camera's rect misses the map.
fn narrowed_cull(
    clip_from_world: &Mat4,
    corners: &[bevy::math::Vec3A; 8],
    pad: f32,
) -> Option<bevy::camera::primitives::Frustum> {
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for c in corners {
        let p = clip_from_world.project_point3(Vec3::from(*c)).truncate();
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let lo = (lo - pad).max(Vec2::NEG_ONE);
    let hi = (hi + pad).min(Vec2::ONE);
    if hi.x <= lo.x || hi.y <= lo.y {
        return None;
    }
    // Clip XY [lo, hi] onto [-1, 1]; depth and w untouched.
    let scale = 2.0 / (hi - lo);
    let offset = -(hi + lo) / (hi - lo);
    let remap = Mat4::from_cols(
        Vec4::new(scale.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, scale.y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(offset.x, offset.y, 0.0, 1.0),
    );
    Some(bevy::camera::primitives::Frustum::from_clip_from_world(
        &(remap * *clip_from_world),
    ))
}

/// Drop from each band the casters the reference's gather leaves out of it (`0x7bb9d0`): every one
/// in a held band; a map entity's part under the band's radius floor (the dynamic map's too) or,
/// at modes 3 and 4, any; at mode 5 one wholly inside the next-inner map's box.
#[allow(clippy::type_complexity)]
fn trim_band_casters(
    plan: Res<ShadowPlan>,
    mut lights: Query<(&Cascades, &mut CascadesVisibleEntities), With<ShadowLight>>,
    parts: Query<(&Aabb, &GlobalTransform, Option<&ModelPart>)>,
    parents: Query<&ChildOf>,
    units: Query<(&WorldUnit, &GlobalTransform)>,
) {
    // The map entity owning a part, and its bounding-sphere radius when its box is known.
    let owner = |mut e: Entity, part: Option<&ModelPart>| -> Option<Option<f32>> {
        if part.is_some_and(|p| matches!(p.kind, ModelKind::Doodad | ModelKind::Wmo)) {
            return None;
        }
        loop {
            if let Ok((unit, t)) = units.get(e) {
                let scale = t.affine().matrix3.col(0).length();
                return Some(
                    unit.bound
                        .map(|b| Vec3::from(b.half_extents).length() * unit.scale * scale),
                );
            }
            e = parents.get(e).ok()?.parent();
        }
    };
    for (cascades, mut visible) in &mut lights {
        for (view, per_map) in visible.entities.iter_mut() {
            let Some(maps) = cascades.cascades.get(view) else {
                continue;
            };
            for (i, list) in per_map.iter_mut().enumerate() {
                if !plan.draws(i) {
                    list.entities.clear();
                    continue;
                }
                let floor = i
                    .checked_sub(1)
                    .map_or(DYN_MIN_RADIUS, |b| BAND_MIN_RADIUS[b]);
                let inner = (plan.exclude_inner && i > 0).then(|| maps[i - 1].clip_from_world);
                let units_out = plan.bands_static && i > 0;
                list.entities.retain(|&e| {
                    let Ok((aabb, t, part)) = parts.get(e) else {
                        return true;
                    };
                    if let Some(radius) = owner(e, part) {
                        if units_out || radius.is_some_and(|r| r < floor) {
                            return false;
                        }
                    }
                    let Some(inner) = inner else {
                        return true;
                    };
                    let m = t.affine().matrix3;
                    let h = Vec3::from(aabb.half_extents);
                    let half = Vec3::new(
                        m.row(0).abs().dot(h.into()),
                        m.row(1).abs().dot(h.into()),
                        m.row(2).abs().dot(h.into()),
                    );
                    !inside_map(&inner, t.transform_point(aabb.center.into()), half)
                });
            }
        }
    }
}

/// A blended, Mod or Mod2x model part never casts: the reference gathers opaque and alpha-key
/// batches alone (`0x834660`), which the caster shader also holds.
#[allow(clippy::type_complexity)]
fn mute_blended_parts(
    mut commands: Commands,
    parts: Query<(Entity, &ModelPart), (Added<ModelPart>, Without<NotShadowCaster>)>,
) {
    for (e, part) in &parts {
        if matches!(
            part.blend,
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
        ) {
            commands.entity(e).insert(NotShadowCaster);
        }
    }
}

/// The caster set needs work: the mode moved, or modes 1 and 2 are keeping new meshes out.
fn caster_set_due(quality: Res<ExtShadowQuality>) -> bool {
    quality.is_changed() || units_only(quality.0)
}

/// A mesh kept out of the maps at modes 1 and 2 because no PC/NPC body owns it.
#[derive(Component)]
struct MutedCaster;

/// At modes 1 and 2 only PC/NPC bodies cast (`0x7bb9d0`, query flags 3): every other mesh takes
/// [`NotShadowCaster`]; leaving those modes gives them back.
#[allow(clippy::type_complexity)]
fn mute_world_casters(
    mut commands: Commands,
    quality: Res<ExtShadowQuality>,
    mut was_units_only: Local<bool>,
    added: Query<
        Entity,
        (
            Added<Mesh3d>,
            Without<MutedCaster>,
            Without<NotShadowCaster>,
        ),
    >,
    unmuted: Query<Entity, (With<Mesh3d>, Without<MutedCaster>, Without<NotShadowCaster>)>,
    muted: Query<Entity, With<MutedCaster>>,
    parents: Query<&ChildOf>,
    units: Query<&WorldUnit>,
) {
    let now = units_only(quality.0);
    let entering = now && !*was_units_only;
    let leaving = !now && *was_units_only;
    *was_units_only = now;
    if leaving {
        for e in &muted {
            commands
                .entity(e)
                .remove::<(NotShadowCaster, MutedCaster)>();
        }
        return;
    }
    if !now {
        return;
    }
    let mut mute = |e: Entity| {
        if !owned_by_body(e, &parents, &units) {
            commands.entity(e).insert((NotShadowCaster, MutedCaster));
        }
    };
    if entering {
        unmuted.iter().for_each(&mut mute);
    } else {
        added.iter().for_each(&mut mute);
    }
}

/// Whether a PC/NPC body (a wading [`WorldUnit`]) is the mesh or one of its ancestors: its parts,
/// attachments and mount.
fn owned_by_body(mut e: Entity, parents: &Query<&ChildOf>, units: &Query<&WorldUnit>) -> bool {
    loop {
        if let Ok(unit) = units.get(e) {
            return unit.wades;
        }
        match parents.get(e) {
            Ok(child_of) => e = child_of.parent(),
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_sizes_follow_the_mode() {
        let sizes: Vec<usize> = (1..=5).map(shadow_map_size).collect();
        assert_eq!(sizes, [1024, 2048, 1024, 2048, 2048]);
        assert!(!has_bands(2) && has_bands(3) && has_bands(5));
        assert!(units_only(1) && units_only(2) && !units_only(0) && !units_only(3));
    }

    #[test]
    fn the_shadow_sun_steepens_and_clamps() {
        // A low sun: WoW z −0.2 becomes −1.0 before normalising.
        let d = shadow_light_dir(Vec3::new(0.6, -0.2, 0.0));
        let want = Vec3::new(0.6, -1.0, 0.0).normalize();
        assert!(d.abs_diff_eq(want, 1e-6), "{d} vs {want}");
        // A high sun clamps at −1.2.
        let d = shadow_light_dir(Vec3::new(0.3, -0.9, 0.1));
        let want = Vec3::new(0.3, -1.2, 0.1).normalize();
        assert!(d.abs_diff_eq(want, 1e-6), "{d} vs {want}");
    }

    #[test]
    fn held_bands_jump_past_their_update_distance() {
        let c = Vec3::new(100.0, 5.0, 100.0);
        // Within 2 yd of band 0's centre it holds; past it the band jumps to the viewer.
        assert_eq!(held_band_center(Some(c), c + Vec3::X * 1.9, 0), c);
        let far = c + Vec3::X * 2.1;
        assert_eq!(held_band_center(Some(c), far, 0), far);
        // Band 2 holds out to 32 yd.
        assert_eq!(held_band_center(Some(c), c + Vec3::Z * 31.0, 2), c);
        assert_eq!(held_band_center(None, far, 1), far);
    }

    #[test]
    fn mode_five_bands_floor_on_the_world_grid() {
        // WoW (5.5, −3.5, 7.9) on band 1's 4 yd grid is (4, −4, 4).
        let at = wow_to_bevy([5.5, -3.5, 7.9]);
        let c = bevy_to_wow(snapped_band_center(at, 1));
        assert_eq!(c, [4.0, -4.0, 4.0]);
    }

    #[test]
    fn a_map_box_spans_its_radius_and_depth_range() {
        let dir = shadow_light_dir(Vec3::new(0.4, -0.5, 0.3));
        let center = Vec3::new(-1200.0, 40.0, 3500.0);
        let c = map_cascade(center, DYN_RADIUS, dir, 1024);
        let ndc = |p: Vec3| c.clip_from_world.project_point3(p);
        // The centre lands mid-box, 2000 yd from the eye: reverse-Z depth (4000 − 2000)/3999.
        let mid = ndc(center);
        assert!(mid.x.abs() < 1e-4 && mid.y.abs() < 1e-4, "{mid}");
        assert!((mid.z - (DEPTH_FAR - EYE_BACK) / (DEPTH_FAR - DEPTH_NEAR)).abs() < 1e-4);
        // A point the radius off the centre, across the light, sits a box half-side out.
        let across = dir.cross(Vec3::Y).normalize() * DYN_RADIUS;
        let edge = ndc(center + across);
        assert!((edge.truncate().length() - 1.0).abs() < 1e-3, "{edge}");
        // Toward the light is nearer, so deeper in reverse Z.
        assert!(ndc(center - dir * 10.0).z > mid.z);
        assert!((c.texel_size - 40.0 / 1024.0).abs() < 1e-6);
    }

    #[test]
    fn held_bands_redraw_one_at_a_time() {
        let dir = Vec3::NEG_Y;
        let at = Vec3::new(10.0, 0.0, 10.0);
        let held = |center: Vec3, t: f32| Some(HeldBand { center, dir, at: t });
        // Nothing drawn yet: every band draws at once.
        assert_eq!(bands_to_draw(&[None; 3], at, dir, 0.0), [true; 3]);
        // All held and fresh: none draws.
        let still = [held(at, 0.0); 3];
        assert_eq!(bands_to_draw(&still, at, dir, 0.1), [false; 3]);
        // Moved 5 yd: bands 0 and 1 are past their 2 and 4 yd, the innermost goes first.
        let moved = at + Vec3::X * 5.0;
        assert_eq!(bands_to_draw(&still, moved, dir, 0.1), [true, false, false]);
        // A turned sun redraws too.
        let turned = Vec3::new(0.1, -1.0, 0.0).normalize();
        assert_eq!(bands_to_draw(&still, at, turned, 0.1), [true, false, false]);
        // Still, past band 1's refresh but not band 0's (it drew later): band 1.
        let aged = [held(at, 0.5), held(at, 0.0), held(at, 0.5)];
        assert_eq!(bands_to_draw(&aged, at, dir, 0.6), [false, true, false]);
    }

    #[test]
    fn inside_a_map_means_wholly_inside_its_box() {
        let dir = shadow_light_dir(Vec3::new(0.3, -0.8, 0.2));
        let center = Vec3::new(100.0, 20.0, -50.0);
        let m = map_cascade(center, BAND_RADIUS[0], dir, 2048).clip_from_world;
        assert!(inside_map(&m, center, Vec3::splat(2.0)));
        // Any depth along the light stays inside: the box is a prism.
        assert!(inside_map(&m, center - dir * 300.0, Vec3::splat(2.0)));
        // Straddling the box's edge is not inside.
        let side = map_cascade(center, BAND_RADIUS[0], dir, 2048)
            .world_from_cascade
            .x_axis
            .truncate();
        assert!(inside_map(&m, center + side * 30.0, Vec3::splat(2.0)));
        assert!(!inside_map(&m, center + side * 39.0, Vec3::splat(2.0)));
        assert!(!inside_map(&m, center, Vec3::splat(45.0)));
    }

    #[test]
    fn the_empty_frustum_admits_nothing() {
        let f = empty_frustum();
        let aabb = Aabb::from_min_max(Vec3::splat(-1e4), Vec3::splat(1e4));
        assert!(!f.intersects_obb(&aabb, &bevy::math::Affine3A::IDENTITY, false, true));
    }

    #[test]
    fn the_dynamic_map_centre_rounds_to_a_texel_of_a_yard() {
        let at = wow_to_bevy([10.0004, -3.0006, 2.5]);
        let c = bevy_to_wow(dyn_center(at, 1024));
        assert_eq!(c[2], 2.5);
        assert!((c[0] * 1024.0 - (c[0] * 1024.0).round()).abs() < 1e-3);
        assert!((c[0] - 10.0004).abs() <= 0.5 / 1024.0 + 1e-6);
    }

    #[test]
    fn the_modes_load_three_program_groups() {
        assert_eq!(
            (0..=5).map(shadow_value).collect::<Vec<_>>(),
            [0, 1, 1, 2, 2, 3]
        );
    }

    /// The programs of one `.bls`: a 12-byte head, then a 12-byte record, a size and padded code.
    fn bls_programs(bytes: &[u8]) -> Vec<&[u8]> {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let mut at = 12;
        (0..word(8))
            .map(|_| {
                let size = word(at + 12);
                let code = &bytes[at + 16..at + 16 + size];
                at += 16 + size.next_multiple_of(4);
                code
            })
            .collect()
    }

    /// Per sampler, how many texture reads one ps_3_0 program holds.
    fn sampler_reads(code: &[u8]) -> [u32; 16] {
        let words: Vec<u32> = code
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect();
        let mut reads = [0; 16];
        let mut at = 1;
        while let Some(&token) = words.get(at) {
            match token & 0xffff {
                0xffff => break,
                0xfffe => at += 1 + (token >> 16 & 0x7fff) as usize,
                op => {
                    let args = (token >> 24 & 0xf) as usize;
                    // D3DSIO_TEX and D3DSIO_TEXLDL close on the sampler register.
                    if op == 0x42 || op == 0x5f {
                        reads[(words[at + args] & 0x7ff) as usize] += 1;
                    }
                    at += 1 + args;
                }
            }
        }
        reads
    }

    /// The map taps of `0x79e5c0`'s and `0x81f1d0`'s programs per value: wow_shadow.wgsl's arms.
    #[test]
    fn the_receive_arms_are_the_programs_each_value_loads() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::Chain::open(&data).expect("open the patch chain");
        let mut read = |name: &str| {
            chain
                .read_file(&format!(r"Shaders\Pixel\ps_3_0\{name}.bls"))
                .expect("a program file")
        };
        let terrain3 = read("Terrain3_pcf");
        let terrain3 = bls_programs(&terrain3);
        assert_eq!(terrain3.len(), 96);
        for (group, maps) in [[5, 0, 0, 0], [5, 5, 5, 5], [9, 5, 5, 5]]
            .iter()
            .enumerate()
        {
            for layer_pair in 0..8 {
                let reads = sampler_reads(terrain3[8 * group + layer_pair]);
                assert_eq!(reads[5..9], *maps, "group {group} program {layer_pair}");
            }
            assert_eq!(sampler_reads(terrain3[8 * group])[1] > 0, group == 0);
        }
        let terrain2 = read("Terrain2_pcf");
        let terrain2 = bls_programs(&terrain2);
        assert_eq!(terrain2.len(), 32);
        assert_eq!(sampler_reads(terrain2[0])[5..9], [5, 0, 0, 0]);
        let model = read("Combiners_Opaque");
        let model = bls_programs(&model);
        for (value, maps) in [(1, [14, 0, 0, 0]), (2, [14, 5, 5, 5]), (3, [10, 5, 5, 5])] {
            assert_eq!(sampler_reads(model[4 + value])[4..8], maps, "value {value}");
        }
    }
}
