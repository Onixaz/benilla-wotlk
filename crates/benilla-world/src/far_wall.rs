//! The far-clip wall, compiled out where it cannot cut. `wow_model.wgsl` clips the detailed world
//! per pixel at `farclip` on planar eye-Z (the reference's far plane), and a reachable `discard`
//! costs a draw a tile GPU's hidden-surface removal even where it never fires. A part whose bound
//! lies wholly inside the wall draws through an opaque twin without it ([`NO_FAR_WALL_MARKER`]).
//!
//! The swap happens in the render world after material extraction, judged by the camera of the
//! frame being drawn, so no main-world handle writer sees it, and a part this does not decide
//! keeps its base material and the wall.

use std::any::TypeId;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibleEntities;
use bevy::ecs::system::SystemChangeTick;
use bevy::pbr::{
    late_sweep_material_instances, EntitySpecializationTickPair, EntitySpecializationTicks,
    MaterialExtractEntitiesNeedingSpecializationSystems, MaterialExtractionSystems,
    PreparedMaterial, RenderMaterialInstances,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::erased_render_asset::ErasedRenderAssets;
use bevy::render::sync_world::MainEntity;
use bevy::render::{Extract, ExtractSchedule, RenderApp};

use benilla_assets::materials::WowModelMaterial;

use crate::model_render::ModelPart;
use crate::view::{ViewDistance, WorldCamera};

/// The `clutter_fade.z` bit that compiles the wall out (`WowModelKey::no_far_wall`).
pub const NO_FAR_WALL_MARKER: u16 = 1 << 14;

/// The inside test's slack, a fraction of `farclip`: a skinned pose can reach past its bind-pose
/// bound, and a part this near the wall keeps it.
const WALL_MARGIN: f32 = 0.1;

/// The wall-free twin of each opaque base material a visible part binds, strongly held until the
/// base goes.
#[derive(Resource, Default)]
pub struct FarWallTwins {
    twins: HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
}

impl FarWallTwins {
    /// The live twin count, for the census.
    pub fn len(&self) -> usize {
        self.twins.len()
    }

    /// Whether no twin is live.
    pub fn is_empty(&self) -> bool {
        self.twins.is_empty()
    }
}

/// Whether a base material takes a twin: only an opaque draw gains, as a blend or cutout draw has
/// no hidden-surface removal to lose.
pub fn takes_twin(base: &WowModelMaterial) -> bool {
    base.base.alpha_mode == AlphaMode::Opaque
}

/// A base material's wall-free clone: the marker added, everything else kept.
pub fn twin_of(base: &WowModelMaterial) -> WowModelMaterial {
    let mut twin = base.clone();
    twin.extension.clutter_fade.z = twin_word(twin.extension.clutter_fade.z);
    twin
}

/// The twin's `clutter_fade.z` word: [`NO_FAR_WALL_MARKER`] added, every other marker kept.
fn twin_word(z: f32) -> f32 {
    f32::from(z as u16 | NO_FAR_WALL_MARKER)
}

/// One visible part's row for [`swap_far_wall_twins`].
type PartRow = (
    &'static GlobalTransform,
    Option<&'static Aabb>,
    &'static MeshMaterial3d<WowModelMaterial>,
);

/// Whether a bound lies wholly inside the wall with [`WALL_MARGIN`] to spare: its farthest point
/// along camera-forward, the coordinate the shader's wall reads.
pub fn inside_wall(farclip: f32, cam_pos: Vec3, cam_fwd: Vec3, center: Vec3, radius: f32) -> bool {
    (center - cam_pos).dot(cam_fwd) + radius <= farclip * (1.0 - WALL_MARGIN)
}

/// Mint a twin for each base material the world camera sees on a model part ([`takes_twin`]), and
/// keep each twin a copy of its base.
fn mint_far_wall_twins(
    cam: Query<&VisibleEntities, With<WorldCamera>>,
    parts: Query<&MeshMaterial3d<WowModelMaterial>, With<ModelPart>>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    mut edits: MessageReader<AssetEvent<WowModelMaterial>>,
    mut twins: ResMut<FarWallTwins>,
) {
    // A base edit re-clones its twin; a base going drops it. Twin writes touch only twin ids,
    // which are never keys, so this cannot feed back.
    for ev in edits.read() {
        match ev {
            AssetEvent::Modified { id } => {
                if let (Some(base), Some(twin)) = (materials.get(*id), twins.twins.get(id)) {
                    let (twin_id, value) = (twin.id(), twin_of(base));
                    // Cannot fail: the map holds the twin's strong handle.
                    materials
                        .insert(twin_id, value)
                        .expect("far-wall twin slot is strongly held");
                }
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                twins.twins.remove(id);
            }
            _ => {}
        }
    }
    for visible in &cam {
        for &entity in visible.iter(TypeId::of::<Mesh3d>()) {
            let Ok(mat) = parts.get(entity) else {
                continue;
            };
            let id = mat.id();
            if twins.twins.contains_key(&id) {
                continue;
            }
            let Some(base) = materials.get(id) else {
                continue;
            };
            if !takes_twin(base) {
                continue;
            }
            let value = twin_of(base);
            let twin = materials.add(value);
            twins.twins.insert(id, twin);
        }
    }
}

/// Point each visible model part inside the wall at its base's twin, and every other back at its
/// base, after the material extraction wrote this frame's bases. A twin not yet prepared waits a
/// frame on the base, so a part never goes undrawn.
#[allow(clippy::too_many_arguments)]
fn swap_far_wall_twins(
    view: Extract<Res<ViewDistance>>,
    cam: Extract<Query<(&GlobalTransform, &VisibleEntities), With<WorldCamera>>>,
    parts: Extract<Query<PartRow, With<ModelPart>>>,
    twins: Extract<Res<FarWallTwins>>,
    prepared: Res<ErasedRenderAssets<PreparedMaterial>>,
    mut instances: ResMut<RenderMaterialInstances>,
    mut spec_ticks: ResMut<EntitySpecializationTicks>,
    ticks: SystemChangeTick,
) {
    let tick = instances.current_change_tick;
    for (cam_t, visible) in &cam {
        let (cam_pos, cam_fwd) = (cam_t.translation(), Vec3::from(cam_t.forward()));
        for &entity in visible.iter(TypeId::of::<Mesh3d>()) {
            let Ok((xf, aabb, mat)) = parts.get(entity) else {
                continue;
            };
            let base = mat.id();
            // No bound yet (the frame after spawn) keeps the wall.
            let twin = twins.twins.get(&base).filter(|twin| {
                aabb.is_some_and(|a| {
                    let center = xf.transform_point(Vec3::from(a.center));
                    let radius =
                        Vec3::from(a.half_extents).length() * xf.affine().matrix3.x_axis.length();
                    inside_wall(view.farclip, cam_pos, cam_fwd, center, radius)
                }) && prepared.get(twin.id()).is_some()
            });
            let want = twin.map_or(base, Handle::id).untyped();
            let main = MainEntity::from(entity);
            let Some(instance) = instances.instances.get_mut(&main) else {
                continue;
            };
            if instance.asset_id == want {
                continue;
            }
            instance.asset_id = want;
            instance.last_change_tick = tick;
            // The pipeline keys on the material, so the entity re-specializes.
            spec_ticks.insert(
                main,
                EntitySpecializationTickPair {
                    system_tick: ticks.this_run(),
                    material_instances_tick: tick,
                },
            );
        }
    }
}

/// The far-wall twin lane's registrations.
pub fn plugin(app: &mut App) {
    app.init_resource::<FarWallTwins>().add_systems(
        PostUpdate,
        mint_far_wall_twins
            .after(bevy::camera::visibility::VisibilitySystems::CheckVisibility)
            .after(bevy::asset::AssetEventSystems)
            // It writes the handles read here.
            .after(crate::straddle::sync_straddle_twins),
    );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            ExtractSchedule,
            swap_far_wall_twins
                .after(MaterialExtractionSystems)
                .after(MaterialExtractEntitiesNeedingSpecializationSystems)
                .before(late_sweep_material_instances),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inside_test_reads_the_far_point_with_margin() {
        let (eye, fwd) = (Vec3::ZERO, Vec3::NEG_Z);
        // 777 yd wall, 10% slack: inside to 699.3 yd at the far point.
        assert!(inside_wall(
            777.0,
            eye,
            fwd,
            Vec3::new(0.0, 0.0, -690.0),
            9.0
        ));
        assert!(!inside_wall(
            777.0,
            eye,
            fwd,
            Vec3::new(0.0, 0.0, -690.0),
            10.0
        ));
        // Planar, not radial: far off to the side at shallow depth is inside.
        assert!(inside_wall(
            777.0,
            eye,
            fwd,
            Vec3::new(2000.0, 0.0, -10.0),
            5.0
        ));
        // Behind the eye is inside.
        assert!(inside_wall(777.0, eye, fwd, Vec3::new(0.0, 0.0, 50.0), 5.0));
    }

    #[test]
    fn a_twin_keeps_every_marker_and_adds_its_own() {
        let src = crate::model_render::SKY_DEPTH_MARKER | 0x3f;
        let z = twin_word(f32::from(src));
        assert_eq!(z as u16, src | NO_FAR_WALL_MARKER);
        // f32-exact with the top marker set.
        assert_eq!(f32::from(z as u16), z);
    }
}
