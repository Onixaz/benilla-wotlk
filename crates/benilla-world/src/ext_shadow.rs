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
//! Here one `DirectionalLight` carries the maps as cascades: cascade 0 the dynamic map, cascades
//! 1 to 3 the bands. Bevy fits cascades to the camera frustum; [`place_cascades`] replaces them with
//! the reference's viewer-centred boxes. The receivers (`wow_shadow.wgsl`) read the cascades
//! themselves, so nothing of Bevy's PBR shadow lookup runs. At mode 0 no light exists, and Bevy's
//! light passes stay gated off (`world_plugins`).

use bevy::camera::primitives::CascadesFrusta;
use bevy::light::cascade::{build_directional_light_cascades, Cascade};
use bevy::light::{
    CascadeShadowConfig, Cascades, DirectionalLightShadowMap, NotShadowCaster,
    SimulationLightSystems,
};
use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

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

/// The light carrying the maps, while a mode past 0 is set.
#[derive(Component)]
struct ShadowLight;

/// The bands' held centres at modes 3 and 4.
#[derive(Resource, Default)]
struct HeldBands([Option<Vec3>; 3]);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ExtShadowQuality>()
        .init_resource::<HeldBands>()
        .add_systems(
            PostUpdate,
            (
                sync_shadow_light.run_if(resource_changed::<ExtShadowQuality>),
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
        );
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

/// Replace Bevy's frustum-fitted cascades with the reference's maps about the viewer.
fn place_cascades(
    quality: Res<ExtShadowQuality>,
    lighting: Res<crate::lighting::WowLighting>,
    viewer: Res<Viewer>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut held: ResMut<HeldBands>,
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
    let mut maps = Vec::with_capacity(4);
    maps.push(map_cascade(dyn_center(at, size), DYN_RADIUS, dir, size));
    if has_bands(mode) {
        for (band, &radius) in BAND_RADIUS.iter().enumerate() {
            let center = if mode >= 5 {
                snapped_band_center(at, band)
            } else {
                let c = held_band_center(held.0[band], at, band);
                held.0[band] = Some(c);
                c
            };
            maps.push(map_cascade(center, radius, dir, size));
        }
    }
    for mut cascades in &mut lights {
        for view in cascades.cascades.values_mut() {
            view.clone_from(&maps);
        }
    }
}

/// Texels of padding around a narrowed cull rect, for the receivers' filter taps.
const CULL_PAD_TEXELS: f32 = 4.0;

/// Cull each map's casters with its frustum narrowed to the camera's clip frustum, as the
/// reference does under `shadowCull` (`0x7bafd0`): the map renders with its full matrix, but only
/// the rect of it the camera's frustum covers along the light gathers casters, since a receiver on
/// screen samples only there. Bevy culls with `CascadesFrusta` and renders with `Cascades`, so this
/// rewrites the former after Bevy builds it.
fn narrow_cascade_frusta(
    quality: Res<ExtShadowQuality>,
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
            for (map, frustum) in maps.iter().zip(view_frusta.iter_mut()) {
                if let Some(narrowed) = narrowed_cull(&map.clip_from_world, &corners, pad) {
                    *frustum = narrowed;
                }
            }
        }
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
    added: Query<Entity, (Added<Mesh3d>, Without<MutedCaster>)>,
    unmuted: Query<Entity, (With<Mesh3d>, Without<MutedCaster>)>,
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
    fn the_dynamic_map_centre_rounds_to_a_texel_of_a_yard() {
        let at = wow_to_bevy([10.0004, -3.0006, 2.5]);
        let c = bevy_to_wow(dyn_center(at, 1024));
        assert_eq!(c[2], 2.5);
        assert!((c[0] * 1024.0 - (c[0] * 1024.0).round()).abs() < 1e-3);
        assert!((c[0] - 10.0004).abs() <= 0.5 / 1024.0 + 1e-6);
    }
}
