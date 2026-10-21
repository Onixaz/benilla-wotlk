//! The reference's transparent order for M2 batches. `CM2Scene::Animate` (`0x821a20`) gives each
//! transparent element two depths in view space and `CM2Scene::SortTransparent` (`0x81ef30`)
//! orders by sort depth farther first, then the priority plane, then the element depth farther
//! first, then the material layer and the batch index:
//!
//! - Under global flag `0x10` the sort depth is the model's (`m_scaleSq`, its origin's squared
//!   distance) and the element depth the section's sort centre's, so the plane decides inside the
//!   model.
//! - Otherwise both are the section's sort centre, pushed along the view ray by its sort radius
//!   (toward the eye for batch flag `0x1`, away for `0x2`), negated behind the eye.
//!
//! Bevy sorts `Transparent3d` by each draw's own bound centre, so after the queue this pass gives
//! every M2 batch's item the distance its key ranks at: the model's or the batch's view depth,
//! plus its rank inside the model in [`RANK_STEP`]s.

use std::sync::atomic::{AtomicU64, Ordering};

use benilla_formats::{BatchSort, RenderSubmesh};
use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_phase::{sort_phase_system, ViewSortedRenderPhases};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

/// Yards between two ranks inside one model: [`super::BATCH_ORDER_SORT_EPS`], over f32 noise at
/// 500 yd and under the owner-last rung's 1-yd floor for the model's effects.
const RANK_STEP: f32 = super::BATCH_ORDER_SORT_EPS;

/// An M2 batch's transparent key and the model instance it ranks within.
#[derive(Component, Clone, Copy, Debug)]
pub struct M2BatchSort {
    pub key: BatchSort,
    /// The model instance, shared by its batches: [`Self::group`], or its root's
    /// [`Entity::to_bits`].
    pub group: u64,
    /// Where the model origin is: a billboard card sits at its pivot, not at the model's.
    pub origin: SortOrigin,
}

/// Where an [`M2BatchSort`] part's model origin is.
#[derive(Clone, Copy, Debug)]
pub enum SortOrigin {
    /// The part's own transform, its mesh in model space.
    Part,
    /// A fixed world point (a static placement's).
    Fixed(Vec3),
    /// This entity's transform (the unit or booth root a card follows).
    Entity(Entity),
}

impl M2BatchSort {
    /// A fresh model-instance id for one spawn's batches, apart from every entity's bits.
    pub fn group() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed) | 1 << 63
    }

    /// The key of an M2 batch in model instance `group`, `None` for one with no key (WMO).
    pub fn of(sub: &RenderSubmesh, group: u64) -> Option<Self> {
        sub.sort.map(|key| Self {
            key,
            group,
            origin: SortOrigin::Part,
        })
    }

    /// [`Self::of`] for a billboard card, its model origin at `origin`.
    pub fn card(sub: &RenderSubmesh, group: u64, origin: SortOrigin) -> Option<Self> {
        Self::of(sub, group).map(|s| Self { origin, ..s })
    }
}

/// [`M2BatchSort`] in the render world, its points in world space.
#[derive(Clone, Copy, Debug)]
struct ExtractedM2Sort {
    key: BatchSort,
    group: u64,
    origin: Vec3,
    center: Vec3,
    /// The sort radius times the part's scale.
    radius: f32,
}

/// The visible M2 batches' keys by main-world entity: a mesh has no render-world entity of its
/// own, so a `Transparent3d` item names its batch by [`MainEntity`].
#[derive(Resource, Default)]
struct M2Sorts(HashMap<MainEntity, ExtractedM2Sort>);

fn extract_m2_sort(
    mut sorts: ResMut<M2Sorts>,
    parts: Extract<Query<(Entity, &ViewVisibility, &M2BatchSort, &GlobalTransform)>>,
    anchors: Extract<Query<&GlobalTransform>>,
) {
    sorts.0.clear();
    for (entity, visibility, sort, transform) in &parts {
        if !visibility.get() {
            continue;
        }
        let (origin, center) = match sort.origin {
            SortOrigin::Part => (
                transform.translation(),
                transform.transform_point(benilla_assets::coords::wow_to_bevy(sort.key.center)),
            ),
            // A card's mesh is centred on its pivot: its centre is the part's position.
            SortOrigin::Fixed(origin) => (origin, transform.translation()),
            SortOrigin::Entity(e) => (
                anchors
                    .get(e)
                    .map_or(transform.translation(), GlobalTransform::translation),
                transform.translation(),
            ),
        };
        let scale = transform.affine().matrix3.x_axis.length();
        sorts.0.insert(
            MainEntity::from(entity),
            ExtractedM2Sort {
                key: sort.key,
                group: sort.group,
                origin,
                center,
                radius: sort.key.radius * scale,
            },
        );
    }
}

/// One M2 item's place: its index in the phase, its model, and its keys.
struct Ranked {
    item: usize,
    group: u64,
    sort_depth: f32,
    plane: i8,
    depth: f32,
    layer: u16,
    index: u16,
}

/// Rewrite the M2 batches' distances in every view's transparent phase before it sorts.
fn order_m2_transparent(
    views: Query<&ExtractedView>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    sorts: Res<M2Sorts>,
    mut ranked: Local<Vec<Ranked>>,
) {
    if sorts.0.is_empty() {
        return;
    }
    for view in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let rangefinder = view.rangefinder3d();
        let eye = view.world_from_view.translation();
        ranked.clear();
        for (i, item) in phase.items.iter().enumerate() {
            let Some(s) = sorts.0.get(&item.entity.1) else {
                continue;
            };
            let (sort_depth, depth) = if s.key.model_depth {
                (
                    rangefinder.distance(&s.origin),
                    s.center.distance_squared(eye),
                )
            } else {
                // The push runs along the eye ray through the centre.
                let ray = (s.center - eye).normalize_or_zero();
                let pushed = match s.key.push {
                    1 => s.center - ray * s.radius,
                    2 => s.center + ray * s.radius,
                    _ => s.center,
                };
                let d = rangefinder.distance(&pushed);
                (d, -d)
            };
            ranked.push(Ranked {
                item: i,
                group: s.group,
                sort_depth,
                plane: s.key.priority_plane,
                depth,
                layer: s.key.material_layer,
                index: s.key.index,
            });
        }
        assign(&mut ranked, |item, distance| {
            phase.items[item].distance = distance
        });
    }
}

/// Order `ranked` and hand each item its distance: the reference's order inside one sort depth of
/// one model is plane, element depth farther first, layer, index, and each run of one model at one
/// sort depth draws in it a [`RANK_STEP`] apart. Distances grow toward the eye, so ascending is
/// back to front.
fn assign(ranked: &mut [Ranked], mut set: impl FnMut(usize, f32)) {
    ranked.sort_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then(a.sort_depth.total_cmp(&b.sort_depth))
            .then(a.plane.cmp(&b.plane))
            .then(b.depth.total_cmp(&a.depth))
            .then(a.layer.cmp(&b.layer))
            .then(a.index.cmp(&b.index))
    });
    let mut rank = 0.0;
    for k in 0..ranked.len() {
        let r = &ranked[k];
        let same_run =
            k > 0 && ranked[k - 1].group == r.group && ranked[k - 1].sort_depth == r.sort_depth;
        rank = if same_run { rank + RANK_STEP } else { 0.0 };
        set(r.item, r.sort_depth + rank);
    }
}

pub(super) fn plugin(app: &mut App) {
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .init_resource::<M2Sorts>()
            .add_systems(ExtractSchedule, extract_m2_sort)
            .add_systems(
                Render,
                order_m2_transparent
                    .in_set(RenderSystems::PhaseSort)
                    .before(sort_phase_system::<Transparent3d>),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mesh has no render-world entity, so the keys must reach the pass by [`MainEntity`], the
    /// name a `Transparent3d` item carries; keyed by render entity, nothing ever matched.
    #[test]
    fn extraction_keys_a_visible_batch_by_its_main_entity() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::render::MainWorld;
        let key = BatchSort {
            model_depth: true,
            priority_plane: 6,
            center: [0.0; 3],
            radius: 1.0,
            push: 0,
            material_layer: 0,
            index: 3,
        };
        let mut main = MainWorld::default();
        let shown = main
            .spawn((
                M2BatchSort {
                    key,
                    group: 7,
                    origin: SortOrigin::Part,
                },
                ViewVisibility::default(),
                GlobalTransform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            ))
            .id();
        {
            use bevy::camera::visibility::SetViewVisibility;
            let mut vis = main.get_mut::<ViewVisibility>(shown).expect("spawned");
            vis.set_visible();
        }
        main.spawn((
            M2BatchSort {
                key,
                group: 7,
                origin: SortOrigin::Part,
            },
            ViewVisibility::default(),
            GlobalTransform::IDENTITY,
        ));
        let mut render = World::new();
        render.init_resource::<M2Sorts>();
        render.insert_resource(main);
        render
            .run_system_once(extract_m2_sort)
            .expect("extract runs");
        let sorts = &render.resource::<M2Sorts>().0;
        assert_eq!(sorts.len(), 1, "only the visible batch");
        let s = sorts
            .get(&MainEntity::from(shown))
            .expect("keyed by its main entity");
        assert_eq!((s.group, s.origin), (7, Vec3::new(1.0, 2.0, 3.0)));
    }

    fn item(item: usize, group: u64, sort_depth: f32, plane: i8, depth: f32, index: u16) -> Ranked {
        Ranked {
            item,
            group,
            sort_depth,
            plane,
            depth,
            layer: 0,
            index,
        }
    }

    fn order(mut ranked: Vec<Ranked>) -> Vec<f32> {
        let mut out = vec![0.0; ranked.len()];
        assign(&mut ranked, |i, d| out[i] = d);
        out
    }

    /// The Northrend login scene's sky bowl (plane 6) draws over its clouds (planes 1-4) though
    /// it comes first in the file: one sort depth, so the plane decides.
    #[test]
    fn one_sort_depth_draws_by_plane_before_file_order() {
        let d = order(vec![
            item(0, 1, -50.0, 6, 0.0, 0),
            item(1, 1, -50.0, 1, 0.0, 1),
            item(2, 1, -50.0, 4, 0.0, 2),
        ]);
        assert!(d[1] < d[2] && d[2] < d[0], "{d:?}");
        assert!(d.iter().all(|&x| (x + 50.0).abs() < 0.01), "{d:?}");
    }

    /// Inside one plane the farther section draws first, then the file order breaks a tie.
    #[test]
    fn inside_a_plane_the_farther_section_draws_first() {
        let d = order(vec![
            item(0, 1, -50.0, 2, 4.0, 0),
            item(1, 1, -50.0, 2, 9.0, 1),
            item(2, 1, -50.0, 2, 9.0, 2),
        ]);
        assert!(d[1] < d[2] && d[2] < d[0], "{d:?}");
    }

    /// Two models never interleave their ranks, and a batch on its own depth keeps it.
    #[test]
    fn models_keep_their_own_depths() {
        let d = order(vec![
            item(0, 1, -50.0, 3, 0.0, 0),
            item(1, 2, -20.0, 0, 0.0, 0),
            item(2, 1, -50.0, 0, 0.0, 1),
            item(3, 3, -35.5, 0, 0.0, 0),
        ]);
        assert_eq!((d[1], d[3]), (-20.0, -35.5));
        assert!(d[2] < d[0] && d[0] < -49.9, "{d:?}");
    }
}
