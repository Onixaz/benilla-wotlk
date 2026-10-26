//! The exterior scene draws only through the doorways the camera can see.
//!
//! The reference culls the outdoor scene once per frame in `CWorldScene::CullSortTable`
//! (`0x79a790`), inside the frustum `CWorldScene::FrustumSet` (`0x790af0`) narrows to one screen
//! rect. Outdoors the rect is the full screen ([`ExteriorWindows::Unrestricted`]). From inside a
//! building it is the union of every portal view whose far side is outdoors
//! (`CWorldScene::MergeIntoFrustumRect` `0x790ad0`, `CPortalView::Merge`); with none, the sort table
//! is cleared and no exterior draws: a sealed room. The rect takes no size gate.
//!
//! In that frustum, map chunks, WMO groups, liquid and the far band are tested on their boxes
//! (`CWorldScene::FrustumCull(CAaBox*)` `0x78fb20`), and doodads and entities on their bounding
//! spheres, once each: `CWorldScene::CullDoodadsExterior` (`0x799980`) through
//! `CWorldScene::FrustumCull(center, radius)` (`0x791120`), `CWorldScene::CullDoodads`
//! (`0x7987a0`) and `CWorldScene::CullEntitys` (`0x793060`). A body in a WMO room rides its
//! building's group walk instead (`CWorldScene::CullEntitysInterior` `0x793270`), drawn when its
//! room, or one a portal hop away (our doorway-straddle guard), is in the PVS and its sphere is
//! in view. `WOW_NO_DRAW_ELECTION=1` keeps only the window leg. The occlusion buffer and
//! volumes (`CWorldOcclusion`) are not built.
//!
//! The rect is a scale and offset on clip space, and [`Frustum::from_clip_from_world`] extracts
//! the same planes the reference bilerps from its corner rays.
//!
//! Terrain is tagged per 33.333 yd MCNK cell, the reference's chunk grain. The far band keeps its
//! whole-tile box.
//!
//! A doodad placement has no root entity here, so each of its parts takes the placement's sphere
//! ([`crate::model_fade::DoodadFade::sphere`]) and the placement draws whole. Deviation: the
//! retained static pass gates a whole 133 yd cell on its box (`static_gx::cull`), as one baked
//! draw cannot drop single placements; its admitted set contains the reference's.

use bevy::camera::primitives::{Aabb, Frustum};
use bevy::camera::visibility::VisibilitySystems;
use bevy::prelude::*;

use crate::view::WorldCamera;
use crate::wmo_portal::{ExteriorWindows, Rect, WmoPvsSet};

/// Tag for exterior scene content, gated by the window worklist: ADT terrain cells, ADT doodad
/// placements, the WDL far band, world WMO placements and open-world liquid, each on the drawn
/// object, as a container's box admits everything in it. Never on anything parented to a WMO
/// group, which the portal PVS already culls (a second gate would blank its interior), nor on a net
/// body, which is elected once at its root off `WorldUnit::bound`.
#[derive(Component)]
pub struct ExteriorScene;

/// Ordering handle: [`ExteriorWindows`] is consumed and `Visibility` written after this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExteriorCullSet;

/// What the cull did on its last run: `windows` is the worklist it read (`None` outdoors),
/// `frusta` whether their merge built a volume (0 or 1), `tested`/`hidden` how many tagged objects it
/// reached and rejected. An object drawn through a wall while `hidden == tested` is one the cull
/// never reached.
#[derive(Resource, Default, Clone, Copy)]
pub struct ExteriorCullVerdict {
    pub(crate) windows: Option<usize>,
    pub(crate) frusta: usize,
    pub(crate) tested: usize,
    pub(crate) hidden: usize,
    /// Tagged objects with no `Aabb`, admitted: the share of the scene the cull is not deciding.
    pub(crate) unbounded: usize,
    /// Net bodies the election reached, and how many it hid; kept apart from `tested`/`hidden`,
    /// whose tens of thousands would hide a body leg that reached nothing.
    pub(crate) bodies: usize,
    pub(crate) bodies_hidden: usize,
    /// Open-world liquid surfaces reached and hid, a subset of `tested`/`hidden` kept apart as the
    /// bodies are: a tile's few liquid layers vanish among its ~250 terrain cells.
    pub(crate) liquid: usize,
    pub(crate) liquid_hidden: usize,
}

pub(crate) struct ExteriorCullPlugin;

impl Plugin for ExteriorCullPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("WOW_CULL_TRACE").is_some() {
            app.insert_resource(CullTrace);
        }
        app.init_resource::<ExteriorCullVerdict>().add_systems(
            PostUpdate,
            apply_exterior_cull
                .in_set(ExteriorCullSet)
                // The flood writes the windows in `Update`. Running before visibility propagation
                // lets a root verdict reach `InheritedVisibility` the same frame.
                .after(WmoPvsSet)
                .after(bevy::transform::TransformSystems::Propagate)
                .before(VisibilitySystems::VisibilityPropagate)
                .before(VisibilitySystems::CheckVisibility),
        );
    }
}

/// The sub-frustum for one NDC window rect: a scale and offset on clip space with depth
/// untouched, so the extracted planes bound the window at the camera's own near and far. The
/// exterior walk and the building's own pass build theirs alike.
pub(crate) fn window_frustum(rect: Rect, clip_from_world: &Mat4) -> Frustum {
    let [x0, y0, x1, y1] = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    // Column-major: `Mat4::from_cols` takes columns, so the `w`-column carries the offsets.
    let rect_to_ndc = Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(-(x0 + x1) / w, -(y0 + y1) / h, 0.0, 1.0),
    );
    Frustum::from_clip_from_world(&(rect_to_ndc * *clip_from_world))
}

/// The exterior rect from inside a building: the union of the outdoor windows, as
/// `CPortalView::Merge` folds each into `frustumPortalView`. `None` with no window, or one that
/// collapsed to no area, which would extract a frustum of NaN planes.
fn merged_window(rects: &[Rect]) -> Option<Rect> {
    let merged = rects
        .iter()
        .copied()
        .reduce(|[a0, b0, a1, b1], [c0, d0, c1, d1]| {
            [a0.min(c0), b0.min(d0), a1.max(c1), b1.max(d1)]
        })?;
    let [x0, y0, x1, y1] = merged;
    (x1 > x0 && y1 > y0).then_some(merged)
}

/// This frame's exterior gate: the merged window as a clip volume, built once and asked per
/// object by [`apply_exterior_cull`] and by the owners that fold it into their own verdict
/// (`model_render::visibility`, the particle and static-geometry culls).
pub enum ExteriorGate {
    /// Outdoors: the ordinary frustum is the window, so the gate admits everything and leaves
    /// each object to its other owners.
    Open,
    /// Indoors: an object draws only where the merged window admits it. `None` admits nothing,
    /// the sealed room, and must never fail open.
    Window(Option<Frustum>),
}

impl ExteriorGate {
    /// From the worklist and the world camera. No camera yet is [`Self::Open`]: nothing is on
    /// screen, and a verdict without a view matrix would be arbitrary.
    pub(crate) fn build(
        windows: &ExteriorWindows,
        cam: Option<(&GlobalTransform, &Projection)>,
    ) -> Self {
        let ExteriorWindows::Windows(rects) = windows else {
            return Self::Open;
        };
        let Some((cam_t, proj)) = cam else {
            return Self::Open;
        };
        let clip_from_world = proj.get_clip_from_view() * cam_t.to_matrix().inverse();
        Self::Window(merged_window(rects).map(|r| window_frustum(r, &clip_from_world)))
    }

    /// The box test, `CWorldScene::FrustumCull(CAaBox*)` (`0x78fb20`): terrain cells, WMO groups,
    /// liquid, the far band. No `Aabb` (a mesh still loading, or nothing drawn) is admitted: a
    /// missing bound is a timing gap, and blanking on it would flicker the world as tiles stream
    /// in.
    pub(crate) fn admits(&self, gt: &GlobalTransform, aabb: Option<&Aabb>) -> bool {
        match (self, aabb) {
            (Self::Open, _) | (_, None) => true,
            (Self::Window(frustum), Some(aabb)) => frustum
                .as_ref()
                .is_some_and(|f| f.intersects_obb(aabb, &gt.affine(), true, true)),
        }
    }

    /// The sphere test, `CWorldScene::FrustumCull(center, radius)` (`0x791120`): a doodad
    /// placement or a body, once each, on its world bounding sphere.
    pub(crate) fn admits_sphere(&self, center: Vec3, radius: f32) -> bool {
        match self {
            Self::Open => true,
            Self::Window(frustum) => frustum.as_ref().is_some_and(|f| {
                f.intersects_sphere(
                    &bevy::camera::primitives::Sphere {
                        center: center.into(),
                        radius,
                    },
                    true,
                )
            }),
        }
    }

    fn frusta(&self) -> usize {
        match self {
            Self::Window(Some(_)) => 1,
            Self::Open | Self::Window(None) => 0,
        }
    }
}

/// A body's world bounding sphere: the circumsphere of its model-space box, scaled with it.
/// Deviation: the reference's entity sphere is the model header's (`CM2Model::GetBoundingSphere`
/// `0x4f5e80`); the game hands the world the idle animation's box instead.
fn body_sphere(gt: &GlobalTransform, bound: &Aabb) -> (Vec3, f32) {
    let scale = gt.affine().matrix3.x_axis.length();
    (
        gt.transform_point(Vec3::from(bound.center)),
        Vec3::from(bound.half_extents).length() * scale,
    )
}

/// What [`apply_exterior_cull`] reads per object.
type UnownedScene = (
    &'static GlobalTransform,
    Option<&'static Aabb>,
    &'static mut Visibility,
    // Instrument only: liquid takes the terrain cell's test, and this lets the counters say so.
    Has<crate::liquid::LiquidSurface>,
);

/// The objects [`apply_exterior_cull`] may write: exterior scene no one else owns. A `ModelPart`
/// or `WmoGroupVis` is the model-visibility authority's.
type UnownedSceneFilter = (
    With<ExteriorScene>,
    Without<crate::model_render::ModelPart>,
    Without<crate::wmo_portal::WmoGroupVis>,
    // Disjoint from the body leg, which also writes `Visibility`: the two never overlap, but Bevy
    // panics on the conflicting access unless the filter proves it.
    Without<crate::world_unit::WorldUnit>,
);

/// `WOW_CULL_TRACE=1`: one line per body per frame saying why it drew. A body escapes the cull in
/// three ways the counters cannot tell apart: never elected (no `WorldUnit::bound`), a WMO room
/// claim, or a window admitting its box. Unthrottled: the audience is dozens, and sampling would
/// lose which body on which frame.
fn trace_body(
    trace: &Option<Res<CullTrace>>,
    e: Entity,
    class: &str,
    gt: &GlobalTransform,
    room: Option<&crate::wmo_portal::UnitWmoRoom>,
    bound: Option<&Aabb>,
    admitted: bool,
) {
    if trace.is_none() {
        return;
    }
    let p = gt.translation();
    println!(
        "CULL_BODY {e} {class} admitted={admitted} claim={claim} bound={bound} \
         pos=[{:.1},{:.1},{:.1}]",
        p.x,
        p.y,
        p.z,
        claim = match room.map(|r| r.room()) {
            None => "absent".to_string(),
            Some(None) => "outdoors".to_string(),
            Some(Some(r)) => format!("g{}", r.group),
        },
        bound = match bound {
            None => "none".to_string(),
            Some(b) => format!("r{:.1}", Vec3::from(b.half_extents).length()),
        },
    );
}

/// Present when `WOW_CULL_TRACE` is set ([`trace_body`]).
#[derive(Resource)]
pub(crate) struct CullTrace;

/// What the body leg reads per net body: one sphere test on its root ([`body_sphere`]), which
/// takes the whole visual with it (gear, mount, rider, billboard cards). A room claim
/// ([`crate::wmo_portal::UnitWmoRoom`]) rides `CullEntitysInterior` with its building, no claim
/// rides `CullEntitys` in the exterior walk; a body not yet rayed has none and reads as outdoors.
type ElectedBody = (
    Entity,
    &'static GlobalTransform,
    &'static crate::world_unit::WorldUnit,
    Option<&'static crate::wmo_portal::UnitWmoRoom>,
    &'static mut Visibility,
);

/// Hides every [`ExteriorScene`] object no window admits, of those with no other `Visibility`
/// owner (terrain cells, the far band, open-world liquid), and elects each net body
/// ([`ElectedBody`]), whose root nothing else writes. A `ModelPart` or `WmoGroupVis` belongs to
/// `model_render::visibility`, which folds [`ExteriorGate`] into its own verdict: a write here
/// would overwrite it every frame.
fn apply_exterior_cull(
    windows: Res<ExteriorWindows>,
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut scene: Query<UnownedScene, UnownedSceneFilter>,
    mut bodies: Query<ElectedBody>,
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    wmos: Res<Assets<benilla_assets::WmoModel>>,
    mut verdict: ResMut<ExteriorCullVerdict>,
    trace: Option<Res<CullTrace>>,
    mut no_election: Local<Option<bool>>,
) {
    // `WOW_NO_DRAW_ELECTION=1` disables the frustum and room legs; the window leg stays.
    let no_election =
        *no_election.get_or_insert_with(|| std::env::var_os("WOW_NO_DRAW_ELECTION").is_some());
    let gate = ExteriorGate::build(&windows, cam.iter().next());
    // The view volume as the outdoor rect `{0,0,1,1}`: an out-of-frustum body is culled. Built
    // from this frame's camera transform, not the `Frustum` component, which may not be refreshed
    // yet; no camera admits.
    let full_view = cam.iter().next().map(|(cam_t, proj)| {
        let clip_from_world = proj.get_clip_from_view() * cam_t.to_matrix().inverse();
        window_frustum([-1.0, -1.0, 1.0, 1.0], &clip_from_world)
    });
    let in_view = |(center, radius): (Vec3, f32)| {
        full_view.as_ref().is_none_or(|f| {
            f.intersects_sphere(
                &bevy::camera::primitives::Sphere {
                    center: center.into(),
                    radius,
                },
                true,
            )
        })
    };
    let (mut tested, mut hidden, mut unbounded) = (0usize, 0usize, 0usize);
    let (mut liquid, mut liquid_hidden) = (0usize, 0usize);
    for (gt, aabb, mut vis, is_liquid) in &mut scene {
        tested += 1;
        unbounded += usize::from(aabb.is_none());
        let admitted = gate.admits(gt, aabb);
        hidden += usize::from(!admitted);
        liquid += usize::from(is_liquid);
        liquid_hidden += usize::from(is_liquid && !admitted);
        // `set_if_neq`, never a `&mut Visibility` reborrow: the reborrow alone marks the row
        // changed, and the visibility propagation re-walks every marked row.
        vis.set_if_neq(if admitted {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    // The body leg: one whole-object test per net body.
    let (mut body_n, mut body_hidden) = (0usize, 0usize);
    for (e, gt, body, room, mut vis) in &mut bodies {
        let Some(bound) = body.bound.as_ref() else {
            // No bound: not this cull's to decide. Traced, as no counter below sees it.
            trace_body(&trace, e, "unelected", gt, room, None, true);
            continue;
        };
        body_n += 1;
        // In a WMO room, its building's walk submits it. No claim yet reads as outdoors, as
        // `UnitWmoRoom::default()` does: the claim lands a frame or two after spawn, and admitting
        // in the gap would draw every streaming mob through a sealed ceiling.
        let in_room = room.is_some_and(|r| r.room().is_some());
        let (center, radius) = body_sphere(gt, bound);
        let admitted = if no_election {
            // The lever: the window leg alone, in-room bodies exempt.
            in_room || gate.admits_sphere(center, radius)
        } else if in_room {
            // `CullEntitysInterior`: drawn iff its room or a one-hop neighbour is in the PVS and
            // its sphere is in view; `room_pvs_visible` fails open at every resolve seam.
            crate::wmo_portal::room_pvs_visible(room, &instances, &wmos)
                && in_view((center, radius))
        } else {
            // `CullEntitys`: outdoors the full-screen rect, indoors the merged window, already a
            // sub-frustum of the view.
            match &gate {
                ExteriorGate::Open => in_view((center, radius)),
                ExteriorGate::Window(_) => gate.admits_sphere(center, radius),
            }
        };
        trace_body(
            &trace,
            e,
            if in_room { "in-room" } else { "outdoor" },
            gt,
            room,
            Some(bound),
            admitted,
        );
        body_hidden += usize::from(!admitted);
        // `set_if_neq`, never a `&mut Visibility` reborrow: the reborrow alone marks the row
        // changed, and the visibility propagation re-walks every marked row.
        vis.set_if_neq(if admitted {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    *verdict = ExteriorCullVerdict {
        windows: match &*windows {
            ExteriorWindows::Unrestricted => None,
            ExteriorWindows::Windows(rects) => Some(rects.len()),
        },
        frusta: gate.frusta(),
        tested,
        hidden,
        unbounded,
        bodies: body_n,
        bodies_hidden: body_hidden,
        liquid,
        liquid_hidden,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Affine3A;

    /// A camera at the origin looking down −Z (Bevy's convention), 90° fov, square aspect.
    fn clip_from_world() -> Mat4 {
        Projection::Perspective(PerspectiveProjection {
            fov: std::f32::consts::FRAC_PI_2,
            aspect_ratio: 1.0,
            near: 0.1,
            far: 1000.0,
            ..default()
        })
        .get_clip_from_view()
    }

    /// A point admitted by a frustum? (A degenerate AABB at `p`, identity placement.)
    fn admits(f: &Frustum, p: Vec3) -> bool {
        let aabb = Aabb::from_min_max(p - Vec3::splat(0.01), p + Vec3::splat(0.01));
        f.intersects_obb(&aabb, &Affine3A::IDENTITY, true, true)
    }

    /// The outside leg's `{0,0,1,1}` rect is exactly the ordinary view frustum.
    #[test]
    fn the_full_screen_window_is_the_whole_frustum() {
        let f = window_frustum([-1.0, -1.0, 1.0, 1.0], &clip_from_world());
        assert!(admits(&f, Vec3::new(0.0, 0.0, -10.0)), "straight ahead");
        // At 90° fov the frustum edge is |x| = |z|; well inside it must pass, and behind must not.
        assert!(admits(&f, Vec3::new(5.0, 0.0, -10.0)), "right of centre");
        assert!(admits(&f, Vec3::new(-5.0, 0.0, -10.0)), "left of centre");
        assert!(!admits(&f, Vec3::new(0.0, 0.0, 10.0)), "behind the eye");
    }

    #[test]
    fn a_half_screen_window_rejects_the_other_half() {
        let f = window_frustum([0.1, -1.0, 1.0, 1.0], &clip_from_world());
        assert!(admits(&f, Vec3::new(5.0, 0.0, -10.0)), "right: admitted");
        assert!(
            !admits(&f, Vec3::new(-5.0, 0.0, -10.0)),
            "left: must be rejected — the window is on the right"
        );
    }

    /// Why terrain is tagged per MCNK cell: a tile around the camera reaches the doorway's side of
    /// the view whichever side its ground is on, so a per-tile mesh would draw through the walls.
    #[test]
    fn a_narrow_window_rejects_a_chunk_of_ground_but_never_a_whole_tile_of_it() {
        // A doorway on the right of the view; the ground of interest is 200 yd to the left.
        let f = window_frustum([0.6, -1.0, 1.0, 1.0], &clip_from_world());
        let ground = Vec3::new(-200.0, -2.0, -200.0);

        const CELL: f32 = 33.333 / 2.0;
        let cell = Aabb::from_min_max(
            ground - Vec3::new(CELL, 0.5, CELL),
            ground + Vec3::new(CELL, 0.5, CELL),
        );
        assert!(
            !f.intersects_obb(&cell, &Affine3A::IDENTITY, true, true),
            "an MCNK cell to the left must not come in through a doorway on the right"
        );

        // The ADT tile containing that cell: the camera stands on it, so it spans the view.
        const TILE: f32 = 533.333 / 2.0;
        let tile = Aabb::from_min_max(Vec3::new(-TILE, -2.5, -TILE), Vec3::new(TILE, -1.5, TILE));
        assert!(
            f.intersects_obb(&tile, &Affine3A::IDENTITY, true, true),
            "a tile-sized box reaches the doorway's side of the view — it can never be culled, and \
             that is exactly why terrain is spawned per chunk"
        );
    }

    /// `CPortalView::Merge` folds the windows into one rect; a window collapsed to no area, or
    /// none at all, admits nothing rather than building a frustum of NaN planes.
    #[test]
    fn the_windows_merge_into_one_rect() {
        assert_eq!(
            merged_window(&[[0.1, -0.5, 0.3, 0.2], [-0.8, -0.2, -0.6, 0.9]]),
            Some([-0.8, -0.5, 0.3, 0.9])
        );
        assert_eq!(merged_window(&[]), None);
        assert_eq!(merged_window(&[[0.5, -1.0, 0.5, 1.0]]), None);
        // No size gate: a sliver still bounds a real volume.
        assert!(merged_window(&[[0.0, -1.0, 0.01, 1.0]]).is_some());
    }

    /// The gap between two doorways is inside the merged window: the reference culls one rect.
    #[test]
    fn two_doorways_admit_the_wall_between_them() {
        let gate = ExteriorGate::Window(
            merged_window(&[[-0.9, -1.0, -0.7, 1.0], [0.7, -1.0, 0.9, 1.0]])
                .map(|r| window_frustum(r, &clip_from_world())),
        );
        assert!(gate.admits_sphere(Vec3::new(0.0, 0.0, -10.0), 0.1));
        assert!(
            !gate.admits_sphere(Vec3::new(0.0, 0.0, 10.0), 0.1),
            "behind the eye"
        );
        assert!(!ExteriorGate::Window(None).admits_sphere(Vec3::new(0.0, 0.0, -10.0), 0.1));
    }

    /// A placement straddling the window's edge: its sphere reaches in while its box does not,
    /// and the sphere decides.
    #[test]
    fn a_doodad_straddling_the_window_edge_is_kept_on_its_sphere() {
        // The window's left edge is NDC x = 0.1: at z = -10 that is world x = 1.0.
        let gate = ExteriorGate::Window(Some(window_frustum(
            [0.1, -1.0, 1.0, 1.0],
            &clip_from_world(),
        )));
        let center = Vec3::new(0.0, 0.0, -10.0);
        let box_half = Vec3::splat(0.6);
        assert!(!gate.admits(
            &GlobalTransform::IDENTITY,
            Some(&Aabb::from_min_max(center - box_half, center + box_half))
        ));
        // The model's sphere radius, larger than the box's half side, as an authored one is.
        assert!(gate.admits_sphere(center, 1.2));
    }

    /// One `apply_exterior_cull` run: each body's verdict in spawn order, and the counters.
    fn run_bodies(
        windows: ExteriorWindows,
        bodies: &[(Vec3, Option<crate::wmo_portal::UnitWmoRoom>, Aabb)],
    ) -> (Vec<Visibility>, ExteriorCullVerdict) {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(windows)
            .add_systems(Update, apply_exterior_cull);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        let ids: Vec<Entity> = bodies
            .iter()
            .map(|(at, room, bound)| {
                let mut e = app.world_mut().spawn((
                    GlobalTransform::from_translation(*at),
                    crate::world_unit::WorldUnit {
                        wades: true,
                        scale: 1.0,
                        height: 2.0,
                        bound: Some(*bound),
                    },
                    Visibility::Inherited,
                ));
                if let Some(room) = *room {
                    e.insert(room);
                }
                e.id()
            })
            .collect();
        app.update();
        let vis = ids
            .iter()
            .map(|e| *app.world().entity(*e).get::<Visibility>().unwrap())
            .collect();
        (vis, *app.world().resource::<ExteriorCullVerdict>())
    }

    fn outdoors() -> Option<crate::wmo_portal::UnitWmoRoom> {
        Some(crate::wmo_portal::UnitWmoRoom::default()) // rayed, hit nothing
    }
    fn in_a_room() -> Option<crate::wmo_portal::UnitWmoRoom> {
        // Only whether there is a room matters here, not which.
        Some(crate::wmo_portal::UnitWmoRoom::claimed(
            crate::wmo_portal::WmoRoom {
                instance: Entity::from_raw_u32(1).expect("a valid entity id"),
                group: 0,
            },
        ))
    }
    const NOT_RAYED_YET: Option<crate::wmo_portal::UnitWmoRoom> = None;

    /// A yard box on the body's origin, a creature's authored idle `CAaBox`.
    fn creature_box() -> Aabb {
        Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5))
    }
    /// What the game writes before a body's model resolves: a point at its origin.
    fn unresolved() -> Aabb {
        Aabb::from_min_max(Vec3::ZERO, Vec3::ZERO)
    }

    /// Two bodies dead ahead in a sealed room: only the room claim separates the one the exterior
    /// walk never submits from the one `CullEntitysInterior` submits with its building.
    #[test]
    fn a_sealed_room_hides_the_outdoor_body_and_keeps_the_one_in_a_room() {
        let ahead = Vec3::new(0.0, 0.0, -50.0);
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[
                (ahead, outdoors(), creature_box()),
                (ahead, in_a_room(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden, Visibility::Inherited],
            "outdoors is elected to pass 2; a body in a room is submitted with its building"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// A body whose model has not resolved is tested on its origin, the server's exact position,
    /// not drawn for the frame until its extent arrives.
    #[test]
    fn a_body_whose_model_has_not_resolved_is_elected_on_its_origin() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[(Vec3::new(0.0, 0.0, -50.0), outdoors(), unresolved())],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "a point at the origin is a testable bound — the sealed room must reject it"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (1, 1));
    }

    /// A missing claim is not a missing bound: `UnitWmoRoom::default()` is a no-room claim, and a
    /// body wrongly outdoors appears a frame late where one wrongly indoors shows through rock.
    #[test]
    fn an_unrayed_body_reads_as_outdoors() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[(Vec3::new(0.0, 0.0, -50.0), NOT_RAYED_YET, creature_box())],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "no claim is not a licence to draw through a sealed room"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (1, 1));
    }

    /// Outdoors the `{0,0,1,1}` leg frustum-tests bodies: one behind the camera goes to pass 2.
    #[test]
    fn outdoors_the_full_screen_window_elects_the_bodies() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Unrestricted,
            &[
                (Vec3::new(0.0, 0.0, -50.0), outdoors(), creature_box()),
                // Behind the camera: pass 2. Its parts carry `NoFrustumCulling`, so this root
                // verdict alone keeps an off-view crowd out of the draw queue.
                (Vec3::new(0.0, 0.0, 50.0), outdoors(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "ahead is drawn; behind the camera is elected out"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// The in-room fork through the real resolve chain; the truth table is `room_pvs_visible`'s.
    #[test]
    fn a_body_in_a_pvs_dark_room_is_not_drawn_until_the_flood_reaches_it() {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(ExteriorWindows::Unrestricted)
            .add_systems(Update, apply_exterior_cull);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        // Two groups sharing no portal, with a real nav table: an absent entry fails open as
        // visible, which would void the dark-room verdict.
        let sealed = |_g: u16| benilla_assets::WmoGroupNav {
            flags: 0,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            ref_start: 0,
            ref_count: 0,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        let model = benilla_assets::WmoModel {
            wmo_id: 1,
            group_nav: vec![sealed(0), sealed(1)],
            portal_refs: Vec::new(),
            ..Default::default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<benilla_assets::WmoModel>>()
            .add(model);
        let inst = app
            .world_mut()
            .spawn(crate::wmo_portal::WmoPortalInstance {
                handle,
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![false, false],
                interior_fog: vec![false, false],
                liquid_visited: vec![false, false],
                flooded: vec![None, None],
            })
            .id();
        // Dead ahead of the camera: only the room term can hide it.
        let body = app
            .world_mut()
            .spawn((
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, -50.0)),
                crate::world_unit::WorldUnit {
                    wades: true,
                    scale: 1.0,
                    height: 2.0,
                    bound: Some(creature_box()),
                },
                crate::wmo_portal::UnitWmoRoom::claimed(crate::wmo_portal::WmoRoom {
                    instance: inst,
                    group: 0,
                }),
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "a PVS-dark room's body is not submitted"
        );
        app.world_mut()
            .entity_mut(inst)
            .get_mut::<crate::wmo_portal::WmoPortalInstance>()
            .unwrap()
            .visible[0] = true;
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "PVS reach ⇒ drawn"
        );
        // A lit room does not exempt a member from the view: behind the camera, hidden again.
        *app.world_mut()
            .entity_mut(body)
            .get_mut::<GlobalTransform>()
            .unwrap() = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 50.0));
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "a visible room's behind-camera member is still pass 2"
        );
    }

    #[test]
    fn a_doorway_admits_only_its_own_side() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(vec![[0.1, -1.0, 1.0, 1.0]]),
            &[
                (Vec3::new(40.0, 0.0, -50.0), outdoors(), creature_box()),
                (Vec3::new(-40.0, 0.0, -50.0), outdoors(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "the window is on the right"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// One `apply_exterior_cull` run over liquid surfaces shaped as the ADT spawn makes them: world
    /// roots at `IDENTITY` (MCLQ positions are absolute) with a world-space `Aabb`.
    fn run_liquid(
        windows: ExteriorWindows,
        surfaces: &[Aabb],
    ) -> (Vec<Visibility>, ExteriorCullVerdict) {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(windows)
            .add_systems(Update, apply_exterior_cull);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        let ids: Vec<Entity> = surfaces
            .iter()
            .map(|bound| {
                app.world_mut()
                    .spawn((
                        GlobalTransform::IDENTITY,
                        *bound,
                        ExteriorScene,
                        crate::liquid::LiquidSurface,
                        Visibility::Inherited,
                    ))
                    .id()
            })
            .collect();
        app.update();
        let vis = ids
            .iter()
            .map(|e| *app.world().entity(*e).get::<Visibility>().unwrap())
            .collect();
        (vis, *app.world().resource::<ExteriorCullVerdict>())
    }

    /// One MCNK liquid layer's world box at `at`: a flat 33.333 yd sheet, the granularity
    /// `spawn_liquids` produces (one entity per `LiquidMesh`).
    fn lake(at: Vec3) -> Aabb {
        const CELL: f32 = 33.333 / 2.0;
        Aabb::from_min_max(
            at - Vec3::new(CELL, 0.05, CELL),
            at + Vec3::new(CELL, 0.05, CELL),
        )
    }

    /// A sealed room never runs the exterior walk, the only path to the ADT liquid producer
    /// `0x683ab0`, so the lake overhead is not drawn; the counters prove the cull reached it.
    #[test]
    fn a_sealed_room_hides_the_lake_overhead() {
        let overhead = Vec3::new(0.0, 30.0, -60.0);
        let (vis, verdict) = run_liquid(ExteriorWindows::Windows(Vec::new()), &[lake(overhead)]);
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "a sealed cavern draws no exterior liquid — the ceiling is not a depth test"
        );
        assert_eq!(
            (verdict.liquid, verdict.liquid_hidden),
            (1, 1),
            "and the cull must have REACHED it — (0, 0) here is a cull that skipped it, which \
             leaves the surface `Inherited` and looks identical on screen to being admitted"
        );
    }

    /// Outdoors the gate is `Open` (`0x6811ca`) and liquid has no other owner, so no lake hides.
    #[test]
    fn outdoors_every_lake_is_admitted() {
        let (vis, verdict) = run_liquid(
            ExteriorWindows::Unrestricted,
            &[
                lake(Vec3::new(0.0, 30.0, -60.0)),
                lake(Vec3::new(0.0, -2.0, 400.0)), // behind the eye: still not this system's call
            ],
        );
        assert_eq!(vis, vec![Visibility::Inherited; 2]);
        assert_eq!((verdict.liquid, verdict.liquid_hidden), (2, 0));
    }

    #[test]
    fn a_doorway_admits_only_the_water_on_its_own_side() {
        let (vis, verdict) = run_liquid(
            ExteriorWindows::Windows(vec![[0.1, -1.0, 1.0, 1.0]]),
            &[
                lake(Vec3::new(60.0, 10.0, -60.0)),
                lake(Vec3::new(-60.0, 10.0, -60.0)),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "the window is on the right"
        );
        assert_eq!((verdict.liquid, verdict.liquid_hidden), (2, 1));
    }

    /// A WMO pool (`WmoGroupVis`) belongs to the model-visibility authority, which exempts the
    /// camera's own building: this walk must skip it, tag or not, even in a sealed room.
    #[test]
    fn a_wmo_pool_is_never_written_by_this_system() {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            // The sealed room: the one case that would blank it.
            .insert_resource(ExteriorWindows::Windows(Vec::new()))
            .add_systems(Update, apply_exterior_cull);
        let instance = app.world_mut().spawn(()).id();
        let pool = app
            .world_mut()
            .spawn((
                GlobalTransform::IDENTITY,
                lake(Vec3::new(0.0, 0.0, -10.0)),
                ExteriorScene,
                crate::liquid::LiquidSurface,
                crate::wmo_portal::WmoGroupVis {
                    instance,
                    groups: std::sync::Arc::from([0u16].as_slice()),
                },
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(pool).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "the pool in the room you are standing in belongs to the model-visibility authority"
        );
        let verdict = *app.world().resource::<ExteriorCullVerdict>();
        assert_eq!(
            (verdict.tested, verdict.liquid),
            (0, 0),
            "`Without<WmoGroupVis>` must exclude it from the walk, not just from the verdict"
        );
    }
}
