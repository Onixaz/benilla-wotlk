//! The skyboxes: authored sky M2s drawn over or instead of the `Light.dbc` gradient dome, camera
//! anchored at the far depth ([`crate::sky_order`]) through the ordinary M2 material lane
//! ([`M2BatchMaterials::skybox`]).
//!
//! The reference (`WoW/Source/WorldClient/DayNight.cpp`) keeps two kinds of sky in `DNInfo`:
//!
//! - **Three blend skies** (`m_blendSkybox[3]`, with a weight and a flag each). `SetColors`
//!   (`0x7f3230`) fills them every frame from the area-light blend's `LightSkybox.dbc` pairs
//!   ([`benilla_formats::LightCatalog::skybox_blend`]): the zone skies of Outland and Northrend and
//!   their instances. When the camera sees a WMO group flagged `0x40000`, the scene render
//!   (`CWorldSceneRender`) overwrites slot 0 with the root's MOSB sky at the interior crossfade
//!   and clears slot 1 (`DayNight::SetBlendSky`, `0x7f31c0`).
//! - **The override sky** (`m_skybox`, weight 1.0): the override light params' skybox, which here
//!   is the ghost sky ([`benilla_formats::LightCatalog::ghost_skybox`]).
//!
//! `RenderSky` (`0x7f09b0`) draws the blend skies in slot order unless the override sky is at full
//! weight, then the override sky, each at its weight. A sky above 0.99 hides the stars, sun disc,
//! moons, gradient and cloud dome unless its record combines (`LightSkybox` flag `0x2`, the
//! auroras); below it they draw under it. The sun and moon glares fade by `1 − w` for the strongest
//! sky (`DNGlare::Update`, `0x7ef6e0`).
//!
//! Each sky is a whole animated `CM2Model` in a scene of its own (`s_skyScene`): bones, colour
//! alpha, texture weights, texture transforms and tint all run on that scene's clock, which
//! advances only while the sky renders. A sky whose `LightSkybox` flags carry bit 1 has its
//! sequence pinned to the time of day (`SyncSkyAnimWithTime`, `0x7ecf20`). This lane evaluates the
//! model on the CPU each frame ([`benilla_formats::SkyRig`]) and writes the skinned positions, both
//! stages' coordinates, the tint and the batch alpha into its own meshes, so no shared animation
//! table or rig palette has to run on a clock that is not the sky's.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;

use crate::model_render::{M2BatchMaterials, SkyboxBatch};
use crate::view::WorldCamera;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::WmoModel;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{RenderSubmesh, SkyRig, SkyboxDraw};

/// The MOGP/MOGI group flag asking for the root's MOSB sky; the loader keeps the MOGP copy in
/// `WmoGroupNav::flags`.
const SHOW_SKYBOX: u32 = 0x40000;

/// The draw rungs: blend slots 0 to 2, then the override sky. Each rung is its own pipeline-key
/// integer in the sky band ([`crate::model_render`]'s `skybox_sort_bias`), so the skies stack in
/// `RenderSky`'s order whatever their batch counts.
pub const SKY_RUNGS: usize = 4;
/// The override sky's rung, drawn last.
const OVERRIDE_RUNG: usize = 3;

/// A sky above this weight hides the celestial pass and restarts the draw list (`0.99000001f`).
const FULL_WEIGHT: f32 = 0.99;

/// One sky the frame asks for: a `DNOverrideSky` at a weight.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyboxSlot {
    /// The model's chain path; empty for the null sky an unnamed `LightSkybox` record resolves to,
    /// which holds its slot but draws nothing.
    pub path: String,
    pub weight: f32,
    /// `m_blendSkyboxFlags[i]`: the record's combine bit ([`benilla_formats::SKYBOX_FLAG_COMBINE`]).
    /// Always clear for the WMO sky and the override sky.
    pub combine: bool,
    /// The record's `Flags`, which `GetOverrideSky` stores on the model when it first creates it
    /// (bit 1, the time-of-day sync, is read off that copy).
    pub flags: u32,
}

impl SkyboxSlot {
    fn from_draw(d: &SkyboxDraw) -> Self {
        Self {
            path: d.path.clone(),
            weight: d.weight,
            combine: d.combines(),
            flags: d.flags,
        }
    }

    /// `DNOverrideSky*` non-null: a named model.
    fn is_sky(&self) -> bool {
        !self.path.is_empty()
    }
}

/// The skies this frame asks for, `DNInfo`'s sky fields after `SetColors` and the scene render's
/// WMO overwrite.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct CameraSkybox {
    /// `m_blendSkybox[0..3]`, `None` for a cleared slot.
    pub blend: [Option<SkyboxSlot>; 3],
    /// `m_skybox`, the override (ghost) sky.
    pub over: Option<SkyboxSlot>,
}

impl CameraSkybox {
    /// `RenderSky`'s draws, in order, with each one's rung: the blend skies unless the override
    /// sky is at full weight (`!m_skybox || m_skyboxWeight < 1.0`), then the override sky. A weight
    /// of 0 or a null sky draws nothing (`DrawSky`'s guard).
    pub fn draws(&self) -> Vec<(usize, &SkyboxSlot)> {
        let mut out = Vec::with_capacity(SKY_RUNGS);
        let over_full = self
            .over
            .as_ref()
            .is_some_and(|o| o.is_sky() && o.weight >= 1.0);
        if !over_full {
            for (rung, slot) in self.blend.iter().enumerate() {
                if let Some(s) = slot.as_ref().filter(|s| s.is_sky() && s.weight > 0.0) {
                    out.push((rung, s));
                }
            }
        }
        if let Some(o) = self.over.as_ref().filter(|o| o.is_sky() && o.weight > 0.0) {
            out.push((OVERRIDE_RUNG, o));
        }
        out
    }

    /// Every named sky the frame holds a pointer to, drawn or not: the ones `RenderSky` syncs.
    fn held(&self) -> impl Iterator<Item = &SkyboxSlot> {
        self.blend
            .iter()
            .flatten()
            .chain(self.over.iter())
            .filter(|s| s.is_sky())
    }

    /// The model leaves of what draws, `+`-joined, for the debug panel and the census; `None` for
    /// the gradient dome alone.
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<String> = self
            .draws()
            .iter()
            .map(|(_, s)| {
                let leaf = s.path.rsplit('\\').next().unwrap_or(&s.path);
                format!("{leaf} w {:.2}", s.weight)
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" + "))
    }
}

/// What the skies do to the celestial pass this frame.
#[derive(Resource, Default, PartialEq)]
pub struct SkyboxWeight {
    /// `RenderSky`'s `drawObjects = false`: a drawable override sky above 0.99, or a drawable
    /// blend sky above 0.99 that does not combine.
    hides: bool,
    /// `DNGlare::Update`'s skybox weight: the override sky's while it is set, else the strongest
    /// blend sky's.
    glare: f32,
}

impl SkyboxWeight {
    /// Whether a sky replaces the whole celestial pass: stars, sun disc, both moons, gradient band
    /// and cloud dome. Below it they draw under the sky.
    pub fn replaces_celestial(&self) -> bool {
        self.hides
    }

    /// The glares' skybox fade: their target takes `(1 − w)`.
    pub fn glare_weight(&self) -> f32 {
        self.glare
    }
}

/// The sky scene's clock in seconds (`s_skySceneTimeMs`): `RenderSky` advances it, so it stops
/// while the sky does not render (under liquid). Frozen at 0 for a deterministic capture.
#[derive(Resource, Default)]
struct SkyScene {
    now: f64,
}

/// One built sky model (`DNOverrideSky`), shared by every slot that names its path.
struct SkyModel {
    rig: SkyRig,
    /// The `LightSkybox` flags of the request that first created it (`sky->m_flags`): the cache
    /// returns the existing model with them, whoever asks next.
    flags: u32,
    /// The scene clock when created: the global sequences' attach time.
    born: f64,
    /// The armed sequence's clock: `base_ms` at scene time `base_at`, advancing at `speed`.
    /// Created playing from 0 at speed 1; a time-of-day sync reseeks and slows it.
    base_ms: f64,
    base_at: f64,
    speed: f64,
    /// `m_lastTimeOfDay`, 0 at creation.
    last_minutes: u32,
    parts: Vec<Entity>,
}

impl SkyModel {
    /// The armed sequence's band time and the global-sequence cursor, in seconds, at `now`.
    fn clocks(&self, now: f64) -> (f32, f64) {
        let anim_ms = self.base_ms + (now - self.base_at) * 1000.0 * self.speed;
        (
            benilla_formats::sky_band_time(&self.rig, anim_ms / 1000.0),
            now - self.born,
        )
    }
}

/// Built skies by path; `None` is a failed or batchless load, never retried.
#[derive(Resource, Default)]
struct SkyModels(HashMap<String, Option<SkyModel>>);

impl SkyModels {
    /// `IsDrawable`: loaded with something to draw.
    fn drawable(&self, path: &str) -> bool {
        self.0.get(path).is_some_and(Option::is_some)
    }
}

/// One batch of a built sky model.
#[derive(Component)]
struct SkyboxPart {
    path: String,
    sub: Arc<RenderSubmesh>,
    /// The authored batch order + 1; the rung is added at material build.
    order: u16,
    mesh: Handle<Mesh>,
    texture: Option<Handle<Image>>,
    texture2: Option<Handle<Image>>,
    /// The batch's combiner with stage 1's transform taken out: this lane writes stage 1's
    /// coordinates itself, so the material must not fold a seed in again.
    combiner: Option<benilla_formats::Combiner>,
    /// The material pair per rung, built on first draw there.
    mats: [Option<SkyboxBatch>; SKY_RUNGS],
    /// Whether any vertex rides a bone that moves.
    skinned: bool,
}

/// [`CameraSkybox`] is settled after this set; the dome's gate runs after it, so the two backdrops
/// agree within a frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SkyboxResolve;

/// Resolves the wanted skies, builds them on first need, animates and shows them, and pins them to
/// the camera.
pub(crate) struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSkybox>()
            .init_resource::<SkyboxWeight>()
            .init_resource::<SkyModels>()
            .init_resource::<SkyScene>()
            .add_systems(
                Update,
                (
                    resolve_camera_skybox,
                    build_skyboxes,
                    resolve_skybox_weight,
                    animate_skyboxes,
                )
                    .chain()
                    // After the PVS pass, whose flood this reads the same frame.
                    .after(crate::wmo_portal::WmoPvsSet)
                    // After the lighting resolve: this frame's crossfade, clock and overrides.
                    .after(crate::lighting::LightingResolveSet)
                    .in_set(SkyboxResolve),
            )
            // Camera-anchored placement runs after propagation, off this frame's camera pose.
            .add_systems(
                PostUpdate,
                follow_camera.in_set(crate::billboard::BillboardPlace),
            );
    }
}

/// `SetColors`' sky half, then the scene render's WMO overwrite, then the override sky.
#[allow(clippy::too_many_arguments)]
fn resolve_camera_skybox(
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    sampler: Option<Res<crate::lighting::LightSampler>>,
    overrides: Res<crate::lighting::LightOverrides>,
    eye_liquid: crate::liquid::EyeLiquid,
    viewer: Res<crate::view::Viewer>,
    current_map: Option<Res<crate::world_map::CurrentMap>>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    crossfade: Res<crate::lighting::WmoCrossfade>,
    mut want: ResMut<CameraSkybox>,
) {
    let map = current_map.as_ref().map_or(0, |m| m.0);
    // The camera eye, as the area-light blend samples it.
    let pos = cam.single().ok().map(|t| bevy_to_wow(t.translation()));
    let mut next = CameraSkybox::default();

    // The zone skies: the blend's pairs, compacted.
    if let (Some(s), Some(pos)) = (sampler.as_ref(), pos) {
        let draws =
            s.0.skybox_blend(map, pos, eye_liquid.submersion(), &overrides.current());
        for (slot, d) in next.blend.iter_mut().zip(&draws) {
            *slot = Some(SkyboxSlot::from_draw(d));
        }
    }

    // The WMO sky: a group of the placement's flood PVS, not the camera's own group, carries
    // [`SHOW_SKYBOX`] (`ebx` the group visited) and its root names a MOSB. `min()`, not the first
    // match: query order is unstable across frames, and two overlapping Caverns of Time shells
    // both qualify.
    let interior = instances
        .iter()
        .filter_map(|inst| {
            let model = wmos.get(&inst.handle)?;
            // The MOSB test first: 810 of the game's 815 WMO roots name no skybox.
            let sky = model.skybox.as_deref()?;
            model
                .group_nav
                .iter()
                .enumerate()
                .any(|(i, nav)| {
                    // Fail closed, unlike the cull: a lookup miss here would paint a sky over the
                    // whole world.
                    nav.flags & SHOW_SKYBOX != 0 && inst.visible.get(i).copied().unwrap_or(false)
                })
                .then(|| sky.to_owned())
        })
        .min();
    if let Some(sky) = interior {
        // `SetBlendSky(0, s_interiorSkybox, 0, m_interiorFogBlend)` then `SetBlendSky(1, null)`:
        // slot 0 takes the WMO sky at the crossfade, even at weight 0 (seen through a doorway it
        // holds the slot and draws nothing), slot 1 empties and slot 2 stays.
        next.blend[0] = Some(SkyboxSlot {
            path: sky,
            weight: crossfade.t(),
            combine: false,
            flags: 0,
        });
        next.blend[1] = None;
    }

    // The override sky: the ghost's, at weight 1.0, off the same ghost flag (`PLAYER_FLAGS` 0x10)
    // the atmosphere switches on, so the two switch together.
    if viewer.ghost {
        if let (Some(s), Some(pos)) = (sampler.as_ref(), pos) {
            if let Some(sky) = s.0.ghost_skybox(map, pos) {
                next.over = Some(SkyboxSlot {
                    path: sky.to_owned(),
                    weight: 1.0,
                    combine: false,
                    flags: s.0.ghost_skybox_flags(map, pos),
                });
            }
        }
    }

    if *want != next {
        *want = next;
    }
}

/// Build each named sky on first request (`GetOverrideSky` creates the model the first time a path
/// is asked for); the models are few, so they stay built.
#[allow(clippy::too_many_arguments)]
fn build_skyboxes(
    mut commands: Commands,
    want: Res<CameraSkybox>,
    scene: Res<SkyScene>,
    mut models: ResMut<SkyModels>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut world_assets) = world_assets else {
        return; // assetless run: the gradient dome stays the backdrop
    };
    for slot in want.held() {
        let path = slot.path.as_str();
        if models.0.contains_key(path) {
            continue;
        }
        let loaded = benilla_formats::load_sky_model(&mut world_assets.chain.lock_recover(), path);
        let (subs, rig) = match loaded {
            Ok((subs, rig)) if !subs.is_empty() => (subs, rig),
            Ok(_) => {
                warn!("skybox '{path}' has no render batches — keeping the gradient dome");
                models.0.insert(path.to_owned(), None);
                continue;
            }
            Err(e) => {
                warn!("skybox '{path}' failed to load, keeping the gradient dome: {e:#}");
                models.0.insert(path.to_owned(), None);
                continue;
            }
        };
        let moving = rig.moving_bones();
        let mut parts = Vec::with_capacity(subs.len());
        for (i, sub) in subs.into_iter().enumerate() {
            let sub = Arc::new(sub);
            let mesh = meshes.add(sky_mesh(&sub));
            // The batch's authored address mode: Caverns of Time's belts wrap their UVs.
            let texture = sub
                .texture
                .as_deref()
                .and_then(|t| world_assets.texture(t, (sub.wrap_x, sub.wrap_y), &mut images));
            let combiner = sub.combiner.clone().map(|mut c| {
                c.uv1 = None;
                c
            });
            let texture2 = combiner
                .as_ref()
                .and_then(|c| c.texture2.as_deref().map(|t| (t, c.wrap2)))
                .and_then(|(t, wrap)| world_assets.texture(t, wrap, &mut images));
            let skinned = sub.joints.iter().zip(&sub.weights).any(|(j, w)| {
                (0..4)
                    .any(|k| w[k] > 0.0 && moving.get(usize::from(j[k])).copied().unwrap_or(false))
            });
            let part = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    Transform::default(),
                    Visibility::Hidden, // `animate_skyboxes` turns on exactly the drawn ones
                    // The bones move the vertices on the CPU, which the bind-pose AABB does not
                    // follow; a sky surrounds the eye, so there is nothing to cull.
                    bevy::camera::visibility::NoFrustumCulling,
                    // The batch alpha (bits 0..=5), written only by `animate_skyboxes`.
                    MeshTag(crate::mesh_tag::spawn_tag(0, 1.0)),
                    SkyboxPart {
                        path: path.to_owned(),
                        sub,
                        // `i + 1`, the authored batch order (0 is unordered).
                        order: u16::try_from(i + 1).unwrap_or(u16::MAX),
                        mesh,
                        texture,
                        texture2,
                        combiner,
                        mats: Default::default(),
                        skinned,
                    },
                ))
                .id();
            parts.push(part);
        }
        models.0.insert(
            path.to_owned(),
            Some(SkyModel {
                rig,
                flags: slot.flags,
                born: scene.now,
                base_ms: 0.0,
                base_at: scene.now,
                speed: 1.0,
                last_minutes: 0,
                parts,
            }),
        );
    }
}

/// A batch's mesh in the sky's one layout: position, normal, both UV sets and colour, so every sky
/// batch shares one vertex layout (a pipeline key) whatever it authors. The second set copies the
/// first where the batch has none, and the colour is the static M2Color tint, else white.
fn sky_mesh(sub: &RenderSubmesh) -> Mesh {
    let n = sub.positions.len();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        sub.positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect::<Vec<_>>(),
    );
    // Unread by the unlit sky, but the shared shader's vertex layout needs it.
    let normals: Vec<[f32; 3]> = if sub.normals.len() == n {
        sub.normals
            .iter()
            .map(|v| wow_to_bevy(*v).to_array())
            .collect()
    } else {
        vec![[0.0, 0.0, 1.0]; n]
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, second_uvs(sub));
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, static_colors(sub));
    mesh.insert_indices(Indices::U32(sub.indices.clone()));
    mesh
}

/// A one-triangle mesh in the sky lane's layout, for the pipeline warm pass.
pub fn warm_mesh() -> Mesh {
    sky_mesh(&RenderSubmesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        indices: vec![0, 1, 2],
        ..RenderSubmesh::default()
    })
}

fn second_uvs(sub: &RenderSubmesh) -> Vec<[f32; 2]> {
    if sub.uvs2.len() == sub.positions.len() {
        sub.uvs2.clone()
    } else {
        sub.uvs.clone()
    }
}

fn static_colors(sub: &RenderSubmesh) -> Vec<[f32; 4]> {
    if sub.vertex_colors.len() == sub.positions.len() {
        sub.vertex_colors.clone()
    } else {
        vec![[1.0; 4]; sub.positions.len()]
    }
}

/// `RenderSky`'s celestial gate and `DNGlare::Update`'s skybox weight, off this frame's skies.
fn resolve_skybox_weight(
    want: Res<CameraSkybox>,
    models: Res<SkyModels>,
    mut weight: ResMut<SkyboxWeight>,
) {
    let drawable = |s: &SkyboxSlot| s.is_sky() && models.drawable(&s.path);
    let over_hides = want
        .over
        .as_ref()
        .is_some_and(|o| drawable(o) && o.weight > FULL_WEIGHT);
    let blend_hides = want
        .blend
        .iter()
        .flatten()
        .any(|s| drawable(s) && s.weight > FULL_WEIGHT && !s.combine);
    // The glare reads the pointers, not `IsDrawable`: a sky still loading dims it too.
    let glare = match want.over.as_ref().filter(|o| o.is_sky() && o.weight > 0.0) {
        Some(o) => o.weight,
        None => want
            .blend
            .iter()
            .flatten()
            .filter(|s| s.is_sky())
            .fold(0.0f32, |m, s| m.max(s.weight)),
    };
    weight.set_if_neq(SkyboxWeight {
        hides: over_hides || blend_hides,
        glare,
    });
}

/// `RenderSky`'s model half: advance the sky scene, sync each held sky to the time of day, then
/// animate and show the drawn skies at their weights and hide every other batch. The sole writer of
/// these entities' `Visibility`, material, `MeshTag` and mesh attributes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn animate_skyboxes(
    time: Res<Time>,
    clock: Res<crate::lighting::GameClock>,
    underwater: Res<crate::liquid::Underwater>,
    want: Res<CameraSkybox>,
    mut scene: ResMut<SkyScene>,
    mut models: ResMut<SkyModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: M2BatchMaterials,
    mut parts: Query<(
        Entity,
        &mut SkyboxPart,
        &mut Visibility,
        Option<&mut MeshMaterial3d<WowModelMaterial>>,
        &mut MeshTag,
    )>,
    mut commands: Commands,
) {
    // The sky render is skipped under liquid (`!s_liquidTypeId`), and its scene clock with it.
    let rendering = !underwater.0.any();
    if rendering && !crate::dev_state::deterministic_run() {
        scene.now += f64::from(time.delta_secs());
    }
    let now = scene.now;

    // `SyncSkyAnimWithTime` on each held sky, before any draws (`GetHourAndMinutes`).
    if rendering {
        for slot in want.held() {
            let Some(Some(model)) = models.0.get_mut(&slot.path) else {
                continue;
            };
            let (flags, rig) = (model.flags, &model.rig);
            if let Some(sync) =
                benilla_formats::sync_sky_anim(rig, flags, clock.minute, &mut model.last_minutes)
            {
                model.base_ms = f64::from(sync.seek_ms);
                model.base_at = now;
                model.speed = f64::from(sync.speed);
            }
        }
    }

    // Each drawn part's rung and alpha: the slot weight is the model's fade alpha (`[+0x180]`).
    let mut drawn: HashMap<Entity, (usize, f32, f32, f64, Arc<Vec<benilla_formats::BoneMatrix>>)> =
        HashMap::new();
    if rendering {
        for (rung, slot) in want.draws() {
            let Some(Some(model)) = models.0.get(&slot.path) else {
                continue;
            };
            let (band_t, gseq_now) = model.clocks(now);
            let pose = Arc::new(if model.rig.is_static() {
                Vec::new()
            } else {
                model.rig.pose(band_t, gseq_now)
            });
            for &e in &model.parts {
                // A sky named in two slots draws once, in the earlier: the reference's one model
                // draws in each, at the later one's state.
                drawn
                    .entry(e)
                    .or_insert((rung, slot.weight, band_t, gseq_now, pose.clone()));
            }
        }
    }

    for (entity, mut part, mut vis, mat, mut tag) in &mut parts {
        let Some((rung, weight, band_t, gseq_now, pose)) = drawn.get(&entity) else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let slot = models
            .0
            .get(&part.path)
            .and_then(Option::as_ref)
            .and_then(|m| m.rig.slot);
        let sample = SkySample::of(&part.sub, part.combiner.is_some(), slot, *band_t, *gseq_now);
        // `A = fadeAlpha · colourAlpha · weight`; `A ≤ 0` skips the batch before its blend mode is
        // read, and an opaque batch with `0 < A < 1` draws blended.
        let alpha = weight * sample.alpha;
        if alpha <= 0.0 {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        }
        // The rung's materials, built on first draw there.
        if part.mats[*rung].is_none() {
            let order = crate::model_render::skybox_order(*rung, part.order);
            let built = mats.skybox(
                &part.sub,
                part.texture.clone(),
                part.combiner.as_ref().map(|c| (c, part.texture2.as_ref())),
                order,
            );
            let Some(built) = built else {
                continue; // the light buffer is not resident yet; retry next frame
            };
            part.mats[*rung] = Some(built);
        }
        let Some(pair) = part.mats[*rung].as_ref() else {
            continue;
        };
        let handle = if alpha < 1.0 {
            &pair.fade_blend
        } else {
            &pair.steady
        };
        match mat {
            Some(mut m) => {
                if m.0 != *handle {
                    m.0 = handle.clone();
                }
            }
            None => {
                commands
                    .entity(entity)
                    .insert(MeshMaterial3d(handle.clone()));
            }
        }
        let bits = crate::mesh_tag::with_alpha(tag.0, alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
        vis.set_if_neq(Visibility::Visible);
        write_animated_attributes(&mut meshes, &part, &sample, pose);
    }
}

/// One batch's material channels at the sky's clocks.
struct SkySample {
    alpha: f32,
    /// Stage 0's texture transform `(t, q, s)`, `None` while it holds the identity.
    uv0: Option<([f32; 2], [f32; 4], [f32; 2])>,
    /// Stage 1's, for a two-stage combiner off the sphere map.
    uv1: Option<([f32; 2], [f32; 4], [f32; 2])>,
    rgb: Option<[f32; 3]>,
}

impl SkySample {
    fn of(
        sub: &RenderSubmesh,
        has_combiner: bool,
        slot: Option<usize>,
        band_t: f32,
        gseq_now: f64,
    ) -> Self {
        let alpha = sub
            .alpha_anim
            .as_ref()
            .map_or(1.0, |a| a.sample(slot, band_t, gseq_now));
        let at2 = |l: &benilla_formats::KeyAnim<[f32; 2]>| l.sample(l.clock(band_t, gseq_now));
        let at3 = |l: &benilla_formats::KeyAnim<[f32; 3]>| l.sample(l.clock(band_t, gseq_now));
        let at4 = |l: &benilla_formats::KeyAnim<[f32; 4]>| l.sample(l.clock(band_t, gseq_now));
        let stage = |t: Option<[f32; 2]>, q: Option<[f32; 4]>, s: Option<[f32; 2]>| {
            (t.is_some() || q.is_some() || s.is_some()).then(|| {
                (
                    t.unwrap_or([0.0, 0.0]),
                    q.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                    s.unwrap_or([1.0, 1.0]),
                )
            })
        };
        // An env-mapped stage 0 takes no texture transform (the shader's sphere-map branch).
        let uv0 = (!sub.env_map)
            .then(|| {
                stage(
                    sub.uv_seq
                        .as_ref()
                        .and_then(|s| s.seq(slot))
                        .or(sub.uv_anim.as_ref())
                        .map(at2),
                    sub.uv_rot_seq.as_ref().and_then(|s| s.seq(slot)).map(at4),
                    sub.uv_scale_seq.as_ref().and_then(|s| s.seq(slot)).map(at2),
                )
            })
            .flatten();
        let uv1 = has_combiner
            .then(|| sub.combiner.as_ref()?.uv1.as_ref())
            .flatten()
            .and_then(|u| {
                stage(
                    u.seq
                        .as_ref()
                        .and_then(|s| s.seq(slot))
                        .or(u.anim.as_deref())
                        .map(at2),
                    u.rot_seq.as_ref().and_then(|s| s.seq(slot)).map(at4),
                    u.scale_seq.as_ref().and_then(|s| s.seq(slot)).map(at2),
                )
            });
        let rgb = sub
            .rgb_seq
            .as_ref()
            .and_then(|s| s.seq(slot))
            .or(sub.rgb_anim.as_ref())
            .map(at3);
        Self {
            alpha,
            uv0,
            uv1,
            rgb,
        }
    }
}

/// Write what moves this frame into the part's mesh: skinned positions, both stages' transformed
/// coordinates and the animated tint. A part with none of them is left untouched, never re-uploaded.
fn write_animated_attributes(
    meshes: &mut Assets<Mesh>,
    part: &SkyboxPart,
    sample: &SkySample,
    pose: &[benilla_formats::BoneMatrix],
) {
    let skin = part.skinned && !pose.is_empty();
    if !skin && sample.uv0.is_none() && sample.uv1.is_none() && sample.rgb.is_none() {
        return;
    }
    let Some(mesh) = meshes.get_mut(&part.mesh) else {
        return;
    };
    let sub = &part.sub;
    if skin {
        let positions: Vec<[f32; 3]> = sub
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (j, w) = (
                    sub.joints.get(i).copied().unwrap_or([0; 4]),
                    sub.weights.get(i).copied().unwrap_or([0.0; 4]),
                );
                wow_to_bevy(benilla_formats::skin_point(pose, j, w, *p)).to_array()
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    }
    let transform = |uvs: &[[f32; 2]], (t, q, s): ([f32; 2], [f32; 4], [f32; 2])| {
        uvs.iter()
            .map(|uv| benilla_formats::uv_transform(*uv, t, q, s))
            .collect::<Vec<_>>()
    };
    if let Some(st) = sample.uv0 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, transform(&sub.uvs, st));
    }
    if let Some(st) = sample.uv1 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, transform(&second_uvs(sub), st));
    }
    if let Some(rgb) = sample.rgb {
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_COLOR,
            vec![[rgb[0], rgb[1], rgb[2], 1.0]; sub.positions.len()],
        );
    }
}

/// Pin every sky to the camera, world-aligned and at authored scale, the model's origin at the eye
/// as the reference places it (`s_skyScene->Animate(C3Vector(0, 0, 0))` with the view's translation
/// removed): `StratholmeSkybox` is authored off-centre, so recentring it would be wrong. The bones
/// are already in the mesh, so every batch shares this one translation.
fn follow_camera(
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    mut parts: Query<
        (&mut Transform, &mut GlobalTransform),
        (With<SkyboxPart>, Without<WorldCamera>),
    >,
) {
    let Some(cam_gt) = cam.iter().next() else {
        return;
    };
    for (mut tf, mut gt) in &mut parts {
        tf.translation = cam_gt.translation();
        tf.rotation = Quat::IDENTITY;
        tf.scale = Vec3::ONE;
        // Propagation already ran this frame: the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(path: &str, weight: f32, combine: bool) -> Option<SkyboxSlot> {
        Some(SkyboxSlot {
            path: path.into(),
            weight,
            combine,
            flags: 0,
        })
    }

    #[test]
    fn the_blend_skies_draw_in_slot_order_then_the_override() {
        let sky = CameraSkybox {
            blend: [slot("A", 1.0, false), slot("B", 0.5, false), None],
            over: slot("Ghost", 0.5, false),
        };
        let d: Vec<(usize, &str)> = sky
            .draws()
            .iter()
            .map(|(r, s)| (*r, s.path.as_str()))
            .collect();
        assert_eq!(d, [(0, "A"), (1, "B"), (OVERRIDE_RUNG, "Ghost")]);
    }

    #[test]
    fn a_full_weight_override_alone_draws() {
        let sky = CameraSkybox {
            blend: [slot("A", 1.0, false), None, None],
            over: slot("Ghost", 1.0, false),
        };
        let d: Vec<&str> = sky.draws().iter().map(|(_, s)| s.path.as_str()).collect();
        assert_eq!(d, ["Ghost"]);
    }

    #[test]
    fn a_weightless_or_null_sky_draws_nothing_but_holds_its_slot() {
        let sky = CameraSkybox {
            blend: [
                slot("Wmo", 0.0, false),
                slot("", 1.0, false),
                slot("C", 0.3, false),
            ],
            over: None,
        };
        let d: Vec<(usize, &str)> = sky
            .draws()
            .iter()
            .map(|(r, s)| (*r, s.path.as_str()))
            .collect();
        assert_eq!(d, [(2, "C")]);
        assert_eq!(
            sky.held().count(),
            2,
            "the WMO sky at weight 0 is still held (and synced)"
        );
        assert_eq!(sky.summary().as_deref(), Some("C w 0.30"));
        assert_eq!(
            CameraSkybox::default().summary(),
            None,
            "the gradient dome alone"
        );
    }

    #[test]
    fn the_sky_mesh_has_one_layout_whatever_the_batch_authors() {
        let sub = RenderSubmesh {
            positions: vec![[0.0, 0.0, 0.0]; 3],
            uvs: vec![[0.25, 0.5]; 3],
            indices: vec![0, 1, 2],
            ..RenderSubmesh::default()
        };
        let mesh = sky_mesh(&sub);
        for attr in [
            Mesh::ATTRIBUTE_POSITION,
            Mesh::ATTRIBUTE_NORMAL,
            Mesh::ATTRIBUTE_UV_0,
            Mesh::ATTRIBUTE_UV_1,
            Mesh::ATTRIBUTE_COLOR,
        ] {
            assert!(mesh.attribute(attr).is_some(), "{} missing", attr.name);
        }
        assert_eq!(
            second_uvs(&sub),
            sub.uvs,
            "no second set: the first stands in"
        );
        assert_eq!(static_colors(&sub), vec![[1.0; 4]; 3], "no tint: white");
    }

    #[test]
    fn a_batch_samples_its_scroll_and_its_alpha_on_the_sky_clock() {
        use benilla_formats::{AlphaAnim, AlphaSeq, KeyAnim};
        let ramp = KeyAnim {
            period: 10.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, 0.0), (10.0, 1.0)],
        };
        let scroll = KeyAnim {
            period: 4.0,
            step: false,
            wrap: true,
            gseq: true,
            keys: vec![(0.0, [0.0, 0.0]), (4.0, [1.0, 0.0])],
        };
        let sub = RenderSubmesh {
            alpha_anim: AlphaAnim::new(vec![AlphaSeq {
                color: None,
                weight: Some(ramp),
            }]),
            uv_anim: Some(scroll),
            ..RenderSubmesh::default()
        };
        // Band time 5 s: half the weight ramp. Global cursor 1 s: a quarter of the gseq scroll.
        let s = SkySample::of(&sub, false, Some(0), 5.0, 1.0);
        assert!((s.alpha - 0.5).abs() < 1e-6);
        let (t, q, sc) = s.uv0.expect("the scroll runs");
        assert!((t[0] - 0.25).abs() < 1e-6 && t[1] == 0.0);
        assert_eq!((q, sc), ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0]));
        assert!(s.uv1.is_none() && s.rgb.is_none());
    }
}
