//! The mouseover picks, each frame: the unit under the cursor into [`super::Hovered`] and the
//! GameObject into [`super::HoveredObject`], as the reference's pick finds them.

use std::collections::HashSet;

use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::creature_anim::AnimDriver;
use crate::entities::HeldAttached;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::CameraControl;
use crate::ui_script::PointerOverUi;
use benilla_world::billboard::BillboardCard;
use benilla_world::collision::PickOccluder;
use benilla_world::interact::{
    ray_mesh_bounds, ray_posed_mesh, CreaturePickPart, DoodadOccluders, GoPickPart, PickParts,
};
use benilla_world::view::WorldCamera;

use super::{Hovered, HoveredObject, PickOcclusion};

/// Trace this frame's world hit, the world leg of the world frame's hit test (flags `0x1000124`):
/// the [`PickOccluder`] colliders (terrain, WMO faces), then the drawn doodads nearer than that
/// ([`DoodadOccluders`]). Net entities are the object trace, not occluders, so a chest never
/// occludes itself. The picks run unbounded and compare against it after. Aimed as the picks are:
/// the OS pointer, else the hover probe's aim. While a location is wanted, the ground leg also
/// traces GameObject hulls ([`super::ObjectGround`]).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(super) fn update_pick_occlusion(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    spatial: avian3d::prelude::SpatialQuery,
    occluders: Query<(), With<PickOccluder>>,
    doodads: DoodadOccluders,
    targeting: Option<Res<crate::spell::SpellTargeting>>,
    parents: Query<&ChildOf>,
    objects: PickObjects,
    mut occlusion: ResMut<PickOcclusion>,
) {
    *occlusion = PickOcclusion::default();
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    // The probe's aim as the GameObject pick published it last frame: steady for a fixed point
    // and a held lock.
    let Some(cursor) = window.cursor_position().or_else(|| {
        super::hover_probe::armed()
            .then(super::hover_probe::now)
            .flatten()
    }) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    let wants_location = targeting.is_some_and(|t| {
        t.spell_for(crate::spell::targeting::TargetingWants::Location)
            .is_some()
    });
    *occlusion = trace_occlusion(
        ray,
        &spatial,
        &occluders,
        &doodads,
        wants_location,
        &parents,
        &objects,
    );
}

/// The GameObjects a hull's collider resolves to, for the ground leg.
type PickObjects<'w, 's> = Query<
    'w,
    's,
    (
        &'static Guid,
        &'static NetEntity,
        &'static GlobalTransform,
        Has<crate::transport::Transport>,
    ),
>;

/// The world hit along `ray`, and while a location is wanted the GameObject hull nearer than it.
fn trace_occlusion(
    ray: Ray3d,
    spatial: &avian3d::prelude::SpatialQuery,
    occluders: &Query<(), With<PickOccluder>>,
    doodads: &DoodadOccluders,
    wants_location: bool,
    parents: &Query<&ChildOf>,
    objects: &PickObjects,
) -> PickOcclusion {
    let mut occlusion = PickOcclusion {
        ray: Some(ray.direction),
        ..default()
    };
    let world = spatial
        .cast_ray_predicate(
            ray.origin,
            ray.direction,
            f32::MAX,
            true,
            // The mask that reaches every `PickOccluder` collider; the marker picks the set within.
            &benilla_world::collision::WorldCollision::body_filter(),
            &|e| occluders.contains(e),
        )
        .map_or(f32::INFINITY, |hit| hit.distance);
    let distance = doodads.cast(ray, world).unwrap_or(world);
    if distance.is_finite() {
        occlusion.distance = distance;
        occlusion.point = Some(ray.origin + *ray.direction * distance);
    }
    if !wants_location {
        return occlusion;
    }
    // A hull's owning GameObject: the collider's own entity or a parent of it (a deck prop).
    let owner = |mut e: Entity| {
        for _ in 0..4 {
            if let Ok((guid, net, tf, transport)) = objects.get(e) {
                return matches!(net.kind, EntityKind::GameObject)
                    .then_some((guid.0, tf, transport));
            }
            e = parents.get(e).ok()?.parent();
        }
        None
    };
    let Some(hit) = spatial.cast_ray_predicate(
        ray.origin,
        ray.direction,
        distance.min(f32::MAX),
        true,
        &benilla_world::collision::WorldCollision::body_filter(),
        &|e| !occluders.contains(e) && owner(e).is_some(),
    ) else {
        return occlusion;
    };
    let Some((guid, deck, transport)) = owner(hit.entity) else {
        return occlusion;
    };
    let point = ray.origin + *ray.direction * hit.distance;
    occlusion.object_ground = Some(super::ObjectGround {
        point,
        deck: (transport && super::is_ship_guid(guid))
            .then(|| (guid, crate::entities::deck_local(point, deck))),
    });
    occlusion
}

/// The model instances whose parts are `unit`'s pick geometry: the body, everything it wears, and
/// its mount. The reference walks the model attachment tree (`[model+0x1dc]` head, `[+0x1e4]`
/// sibling) and registers each model under one candidate, so a hit on a held axe is a hit on its
/// wielder. `pub(crate)` for the capture census, which counts the same.
pub(crate) fn pick_model_roots(
    unit: Entity,
    worn: &[Option<Entity>],
    mount: Option<Entity>,
) -> impl Iterator<Item = Entity> + use<'_> {
    std::iter::once(unit)
        .chain(worn.iter().flatten().copied())
        .chain(mount)
}

/// Every model's current skeleton pose, as one [`SystemParam`] (the picker is at Bevy's 16-param
/// limit): the palette rows and the per-part rig link that indexes them.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PickPose<'w, 's> {
    /// The world-space matrices the vertex stage skins with, read from the CPU rows.
    palettes: Res<'w, benilla_world::rig_palette::RigPalettes>,
    rigs: Query<'w, 's, &'static benilla_world::rig_palette::RigSkin>,
    /// Worn gear drawn through its wearer's slot: its palette is the wearer's rider rows.
    riders: Query<'w, 's, &'static benilla_world::rig_rider::RigRider>,
    /// A unit's model frame, where its body is drawn: a seat anchor for a passenger.
    poses: Query<'w, 's, &'static benilla_world::rig_anim::RigPose>,
    frames: Query<'w, 's, &'static GlobalTransform>,
}

impl PickPose<'_, '_> {
    /// The frame `unit`'s model is drawn in, its rig's `joints_root`, else the unit's own.
    fn model_frame(&self, unit: Entity) -> Option<&GlobalTransform> {
        let frame = self.poses.get(unit).map_or(unit, |p| p.joints_root);
        self.frames.get(frame).ok()
    }

    /// The skinning matrices a part linked to `rig` draws with.
    fn palette(&self, rig: Entity) -> Option<Vec<Mat4>> {
        if let Ok(skin) = self.rigs.get(rig) {
            return self
                .palettes
                .world_palette(skin.slot, skin.bones() as usize);
        }
        let rider = self.riders.get(rig).ok().filter(|r| r.row.is_some())?;
        self.palettes.rider_world_palette(rider.slot)
    }
}

/// The local identity and what of it and of the corpses the pick takes, one [`SystemParam`] under
/// the picker's param limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct TargetHoverState<'w, 's> {
    /// For the selectable test's `UNIT_FIELD_CREATEDBY` clause and the pick filter's possession
    /// clause.
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// The reference's pick flag `0x20`, as the frame began.
    picks_self: Res<'w, crate::spell::PicksSelf>,
    /// The corpse leg's word, as the frame began.
    corpse_pick: Res<'w, crate::spell::CorpsePick>,
    /// The corpse reaction gate's inputs.
    factions: Option<Res<'w, super::Factions>>,
    self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    /// The unit and player filter while a word stands.
    units: crate::spell::targeting::PickChecks<'w, 's>,
}

impl TargetHoverState<'_, '_> {
    /// The reference's two exclusions, which come first and which a pick whose flags carry `0x20`
    /// skips ([`crate::spell::PicksSelf`]; it is set only for a word that takes the player, so
    /// outside targeting they always run): the active player, and a unit they possess, an Eye of
    /// Kilrogg or a Mind Controlled target ([`super::is_possessed_by`]), so the cursor and the
    /// click pass through it to what lies behind.
    fn excludes(&self, is_self: bool, store: Option<&ObjectStore>) -> bool {
        !self.picks_self.0 && (is_self || super::is_possessed_by(store, self.self_guid.0))
    }

    /// Whether an object of this kind is a candidate of this pick, the posed pass and the box
    /// fallback alike: every unit and player outside targeting but the two [`Self::excludes`]
    /// drops, and while targeting the ones the unit filter passes for the word
    /// ([`crate::spell::targeting::PickChecks::admits_unit`]); a corpse always outside targeting,
    /// whose pick flags carry `0x40`, and while targeting only one the corpse filter passes. A
    /// corpse whose store has not streamed is refused then. `is_self` is the active player's own
    /// body. The hover and the press, which latches this pick, share it.
    fn is_candidate(
        &self,
        entity: Entity,
        kind: EntityKind,
        is_self: bool,
        store: Option<&ObjectStore>,
    ) -> bool {
        match kind {
            EntityKind::Unit | EntityKind::Player => {
                !self.excludes(is_self, store) && self.units.admits_unit(entity, store)
            }
            EntityKind::Corpse => {
                let Some(word) = self.corpse_pick.0 else {
                    return true;
                };
                store.is_some_and(|store| {
                    crate::spell::targeting::corpse_pick_admits(
                        word,
                        store,
                        self.factions.as_deref(),
                        self.self_store.iter().next(),
                    )
                })
            }
            _ => false,
        }
    }
}

/// The posed pick's geometry under one model root: its skinned parts. Ours are dropped while
/// hidden: first person hides them ([`crate::player::apply_self_model_fade`] at alpha 0), where the
/// reference's camera fade clears its player-visible flag and the render test refuses the active
/// player, so it is never picked.
#[allow(clippy::type_complexity)] // the picker's own part query, borrowed as-is
fn posed_parts<'a>(
    kids: &Children,
    parts: &'a Query<(
        &Mesh3d,
        &benilla_world::rig_palette::RigPart,
        Option<&InheritedVisibility>,
    )>,
    is_self: bool,
) -> Vec<(&'a Mesh3d, &'a benilla_world::rig_palette::RigPart)> {
    kids.iter()
        .filter_map(|c| parts.get(c).ok())
        .filter(|(_, _, drawn)| !is_self || drawn.is_none_or(|v| v.get()))
        .map(|(mesh, part, _)| (mesh, part))
        .collect()
}

/// The unit under the cursor, as the reference's pick finds it. Broad phase: the ray against the
/// playing sequence's bounds sphere. Pass 1: the ray against the posed render mesh, nearest wins.
/// Pass 2, the mouse pick's retry when pass 1 hit nothing anywhere: every vertex pushed 1 model
/// unit along its skinned normal, a halo of about a yard, won by last frame's pick, else the higher
/// priority (alive 3, dead 2), else the nearer. Every model chained to a unit is its geometry
/// ([`pick_model_roots`]); a unit with no skinned parts takes a box test at pass-1 level. Inert
/// while mouse-looking or over the UI.
#[allow(clippy::type_complexity)]
pub(super) fn update_hover(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit whose plate holds the pointer, as of last frame's plate layout.
    plate_hover: Res<crate::vplates::PlateHover>,
    mut hovered: ResMut<Hovered>,
    mesh_assets: Res<Assets<Mesh>>,
    pose: PickPose,
    // Our own body joins the pick only while a targeting word takes it.
    target_state: TargetHoverState,
    // Last frame's pick, which outranks everything in pass 2 (the reference's anti-flicker cache).
    mut last_pick: Local<Option<Entity>>,
    // Unit roots. An undrawn body is not in the reference's draw list, so it takes no mouseover,
    // cursor or click.
    roots: Query<
        (
            Entity,
            Has<SelfPlayer>,
            &GlobalTransform,
            &NetEntity,
            Option<&ModelAnimations>,
            Option<&AnimDriver>,
            Option<&ObjectStore>,
            Option<&Children>,
            Option<&crate::entities::mount::MountChild>,
            Option<&InheritedVisibility>,
            Option<&HeldAttached>,
        ),
        With<Guid>,
    >,
    // The part children of every pick model root: body, worn items and mount.
    child_sets: Query<&Children>,
    parts: Query<(
        &Mesh3d,
        &benilla_world::rig_palette::RigPart,
        Option<&InheritedVisibility>,
    )>,
    // The box fallback: every `CreaturePickPart`, the fallback cube included.
    meshes: Query<
        (
            &ChildOf,
            &Aabb,
            &GlobalTransform,
            Option<&InheritedVisibility>,
        ),
        With<CreaturePickPart>,
    >,
    // The pick's guid and kind; the kind decides which slot of `Hovered` a hit lands in.
    units: Query<(&Guid, &NetEntity)>,
) {
    hovered.target = None;
    hovered.guid = None;
    hovered.corpse = None;
    hovered.corpse_guid = None;
    hovered.distance = f32::MAX;
    hovered.refused = false;
    // A plate under the pointer is the mouseover, read before the UI gate below, which the plate
    // itself trips: the reference's plate OnEnter publishes ungraded, with no world pick behind it.
    // Freelook wins, as the plates hand the mouse back.
    if !rig.is_looking() {
        if let Some(entity) = plate_hover.0 {
            hovered.target = Some(entity);
            hovered.guid = units.get(entity).ok().map(|(g, _)| g.0);
            hovered.distance = 0.0; // topmost UI: it beats any world GameObject at a tie
            *last_pick = Some(entity);
            return;
        }
    }
    if rig.is_looking() || pointer_over_ui.0 {
        *last_pick = None;
        return;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position().or_else(|| {
        super::hover_probe::armed()
            .then(super::hover_probe::now)
            .flatten()
    }) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    let (origin, dir) = (ray.origin, *ray.direction);
    let limit = occlusion.distance;

    // ── Broad phase, then pass 1 and pass 2 ──
    // A candidate: the unit, its pass-2 priority, its parts' mesh ids and its posed joint palette.
    let mut candidates: Vec<(Entity, u8, Vec<AssetId<Mesh>>, Vec<Mat4>)> = Vec::new();
    // Units with skinned parts: out of the box fallback even when the broad phase rejects them.
    let mut faithful: HashSet<Entity> = HashSet::new();
    for (entity, is_self, gt, net, anims, drv, store, children, mount_child, drawn, held) in &roots
    {
        // Units, players and corpses: the reference picks every object in one trace and switches on
        // type at the end, below.
        if !target_state.is_candidate(entity, net.kind, is_self, store) {
            continue;
        }
        // Not drawn, not clickable. Checked before `faithful.insert`, so an undrawn unit is out by
        // this rule, not by its exclusion from the box fallback.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let Some(children) = children else { continue };
        let worn = held.map_or(&[][..], |h| h.spawned_slots().as_slice());
        // Membership in `faithful` is decided before the broad phase rejects; the parts are
        // collected below, for the survivors only.
        if !children.iter().any(|c| parts.contains(c)) && worn.iter().all(Option::is_none) {
            continue; // static/cube unit → the AABB fallback below
        }
        faithful.insert(entity);
        // Broad phase: the playing sequence's bounds sphere in the model's drawn frame, as
        // `0x81cff0` takes each model's own matrix, so a passenger's sphere rides its seat. An
        // unauthored radius passes (the reference uses the header sphere), as does a mounted unit
        // (the reference sizes it from the mount's box).
        let clip = match (anims, drv) {
            (Some(a), Some(d)) => d.active_anim().and_then(|id| a.find(id)),
            _ => None,
        };
        let frame = pose.model_frame(entity).unwrap_or(gt);
        if let Some(c) = clip.filter(|c| c.bounds_radius > 0.0 && mount_child.is_none()) {
            let centre = frame.transform_point(c.bounds_center);
            let radius = c.bounds_radius * frame.scale().max_element();
            // Reject when the ray's closest approach exceeds the radius; a centre behind the
            // origin clamps to t = 0, so a unit we stand inside stays clickable.
            let along = (centre - origin).dot(dir).max(0.0);
            if (centre - origin - dir * along).length_squared() > radius * radius {
                continue;
            }
        }
        // The skinning matrices of one skeleton's parts, as of last frame's propagated pose.
        let palette_of =
            |sk: &[(&Mesh3d, &benilla_world::rig_palette::RigPart)]| -> Option<Vec<Mat4>> {
                sk.first().and_then(|(_, part)| pose.palette(part.0))
            };
        // The pass-2 ladder: alive 3, dead 2, a corpse 2 when `CORPSE_DYNFLAG_LOOTABLE`, else 0. A
        // corpse has no UNIT block, so `unit_is_dead()` would answer alive for it.
        let priority = if net.kind == EntityKind::Corpse {
            u8::from(store.is_some_and(|s| s.0.corpse_lootable())) * 2
        } else if store.is_some_and(|s| s.0.unit_is_dead()) {
            2
        } else {
            3
        };
        // One candidate per model instance, all resolving to this unit. A partless body is no
        // reason to stop: a trigger creature's whole visible self can be what it wears.
        for root in pick_model_roots(entity, worn, mount_child.map(|mc| mc.0)) {
            let Ok(kids) = child_sets.get(root) else {
                continue;
            };
            let sk = posed_parts(kids, &parts, is_self);
            let Some(palette) = palette_of(&sk) else {
                continue;
            };
            candidates.push((
                entity,
                priority,
                sk.iter().map(|(m, _)| m.id()).collect(),
                palette,
            ));
        }
    }

    // Pass 1: the exact posed mesh, nearest wins, unbounded as in the reference; occlusion is one
    // compare on the result, below.
    let mut best: Option<(f32, Entity)> = None;
    for (entity, _, mesh_ids, palette) in &candidates {
        for id in mesh_ids {
            let Some(t) = ray_posed_mesh(&mesh_assets, *id, palette, origin, dir, false) else {
                continue;
            };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, *entity));
            }
        }
    }

    // ── The box fallback, for skinless units only ──
    // The nearest part per parent is the union's hit; `dir` is normalized, so `t` is a world
    // distance like the hits above.
    for (child_of, aabb, gt, drawn) in &meshes {
        // Not drawn, not clickable: a part inherits its root's visibility.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let parent = child_of.parent();
        if faithful.contains(&parent) {
            continue; // posed-mesh-tested above
        }
        // The same candidates as the posed pick. A bone pile lands here: its corpse model has no
        // skeleton, so only this box test can pick it.
        let Ok((_, parent_net)) = units.get(parent) else {
            continue;
        };
        let root = roots.get(parent).ok();
        let is_self = root.as_ref().is_some_and(|root| root.1);
        let store = root.and_then(|root| root.6);
        if !target_state.is_candidate(parent, parent_net.kind, is_self, store) {
            continue;
        }
        if let Some(t) = ray_mesh_bounds(origin, dir, aabb, gt) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, parent));
            }
        }
    }

    // Pass 2, when nothing hit exactly: the halo, won by the priority ladder, not distance.
    if best.is_none() {
        let mut best2: Option<(f32, u32, Entity)> = None;
        for (entity, priority, mesh_ids, palette) in &candidates {
            let hit = mesh_ids
                .iter()
                .filter_map(|id| ray_posed_mesh(&mesh_assets, *id, palette, origin, dir, true))
                .min_by(f32::total_cmp);
            let Some(t) = hit else { continue };
            let prio = if *last_pick == Some(*entity) {
                u32::MAX // the sticky-hover cache outranks everything
            } else {
                *priority as u32
            };
            let wins = best2.is_none_or(|(bt, bp, _)| prio > bp || (prio == bp && t < bt));
            if wins {
                best2 = Some((t, prio, *entity));
            }
        }
        best = best2.map(|(t, _, e)| (t, e));
    }

    // Occlusion: a strictly nearer world hit drops the pick, a tie keeps it.
    if best.is_some_and(|(t, _)| limit < t) {
        best = None;
    }
    // Switch on type at the end, as the reference does: the one pick lands in the unit slot or the
    // corpse slot, and a unit the selectable grader refuses in neither.
    if let Some((t, entity)) = best {
        if let Ok((guid, net)) = units.get(entity) {
            if net.kind == EntityKind::Corpse {
                hovered.corpse = Some(entity);
                hovered.corpse_guid = Some(guid.0);
            } else if selectable_pick(entity, &roots, target_state.self_guid.0) {
                hovered.target = Some(entity);
                hovered.guid = Some(guid.0);
            } else {
                hovered.refused = true;
            }
        } else if selectable_pick(entity, &roots, target_state.self_guid.0) {
            hovered.target = Some(entity);
        } else {
            hovered.refused = true;
        }
        // Set on every arm, the refusal included (`Hovered::refused`).
        hovered.distance = t;
    }
    *last_pick = best.map(|(_, e)| e);
}

/// The hover grader's selectable test: the reference calls vtable slot 21 (the predicate the
/// selection set runs), and a refusal jumps to the mouseover clear and `ResetCursor`, past the
/// publisher: no tooltip, cursor, brighten or click.
///
/// It grades the winner and filters no candidate: the pick reads no unit field, so a refused unit
/// in front of a chest still takes the pick ([`Hovered::refused`]). A corpse's slot 21 is the
/// corpse object's, so only the unit arms run this. A plate hover skips the grader, but the plate
/// gate already refuses the same units. Graded at the publish, so [`Hovered`] is never readable
/// ungraded.
#[allow(clippy::type_complexity)] // the picker's own root query, borrowed as-is
fn selectable_pick(
    entity: Entity,
    roots: &Query<
        (
            Entity,
            Has<SelfPlayer>,
            &GlobalTransform,
            &NetEntity,
            Option<&ModelAnimations>,
            Option<&AnimDriver>,
            Option<&ObjectStore>,
            Option<&Children>,
            Option<&crate::entities::mount::MountChild>,
            Option<&InheritedVisibility>,
            Option<&HeldAttached>,
        ),
        With<Guid>,
    >,
    self_guid: Option<u64>,
) -> bool {
    let store = roots.get(entity).ok().and_then(|r| r.6);
    super::relations::is_selectable(store, self_guid)
}

/// The GameObject picker's state as one [`SystemParam`] (the picker is at Bevy's 16-param limit):
/// the pick-set cache and the stream edges that rebuild it, the sticky pick and the probe's locals.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct GoPickSet<'w, 's> {
    added: Query<'w, 's, (), Added<GoPickPart>>,
    removed: RemovedComponents<'w, 's, GoPickPart>,
    cache: Local<'s, bevy::platform::collections::HashSet<Entity>>,
    /// Last frame's pick by net entity, which outranks everything in pass 2.
    last_pick: Local<'s, Option<Entity>>,
    /// Every billboard card, for [`net_entity_of`]'s first hop.
    cards: Query<'w, 's, &'static BillboardCard>,
    /// The headless probe's frame counter and say-once report ([`super::hover_probe`]), idle in
    /// a player run.
    frame: Local<'s, u64>,
    report: Local<'s, super::hover_probe::ProbeReport>,
    /// `WOW_HOVER_PROBE=lock`'s held target, where the probe rests its aim for the rest of the run.
    lock: Local<'s, super::hover_probe::LockedAim>,
}

/// The most `ChildOf` hops [`net_entity_of`] climbs: a malformed-data guard, as real joint chains
/// are single digits deep.
const NET_WALK_HOPS: usize = 64;

/// A picked GameObject part's net entity, the one carrying its `Guid`. A mesh part is its child; a
/// billboard card is a world root linked by [`BillboardCard::follows`] (a joint on a rigged host,
/// else the mirror anchor), and the climb goes on from there. The reference's narrow phase walks
/// the one model's batches, billboards included, so a card hit is a GameObject hit.
fn net_entity_of(
    e: Entity,
    cards: &Query<&BillboardCard>,
    guids: &Query<&Guid>,
    child_of: &Query<&ChildOf>,
) -> Entity {
    let mut cur = cards
        .get(e)
        .ok()
        .and_then(BillboardCard::follows)
        .unwrap_or(e);
    for _ in 0..NET_WALK_HOPS {
        if guids.contains(cur) {
            return cur;
        }
        let Ok(parent) = child_of.get(cur) else { break };
        cur = parent.parent();
    }
    cur
}

/// What the GameObject gates read beside the object's store: the template cache (GENERIC's
/// eligibility is its `data[1]`) and the faction catalog.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GoGateInputs<'w> {
    pub(crate) templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    pub(crate) factions: Option<Res<'w, super::ring::Factions>>,
}

pub(super) fn update_hovered_object(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit pick, earlier in the chain: a unit hover keeps the GameObject halo shut.
    unit_hovered: Res<Hovered>,
    mut hovered: ResMut<HoveredObject>,
    go_parts: Query<Entity, With<GoPickPart>>,
    child_of: Query<&ChildOf>,
    guids: Query<&Guid>,
    stores: Query<&ObjectStore>,
    go_gate: GoGateInputs,
    self_q: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    parts: PickParts,
    mut cache: GoPickSet,
) {
    *cache.frame = cache.frame.wrapping_add(1);
    let added_parts = &cache.added;
    let removed_parts = &mut cache.removed;
    let pickable = &mut *cache.cache;
    let last_pick = &mut *cache.last_pick;
    let cards = &cache.cards;
    let resolve_net = |e: Entity| net_entity_of(e, cards, &guids, &child_of);
    // The pick set, rebuilt only on GameObject-part stream edges (a type id never changes after
    // create). Maintained before any early return: an `Added` edge shows only on its own frame, so
    // a rebuild behind the cursor gates would drop parts streamed in while the pointer sat on UI.
    // A part despawned between edges misses at the caster.
    //
    // Transports (TRANSPORT 11, MAP_OBJECT 14, MO_TRANSPORT 15) never join: the reference's model
    // resolver gives them no pick geometry and their interaction predicates are constant false, so
    // a ship's hull never swallows the pick of an NPC on deck.
    if removed_parts.read().next().is_some() || !added_parts.is_empty() {
        pickable.clear();
        pickable.extend(go_parts.iter().filter(|&e| {
            // Through the hit's own resolver, so a card and its mesh siblings agree.
            !stores
                .get(resolve_net(e))
                .is_ok_and(|s| matches!(s.0.gameobject_type_id(), 11 | 14 | 15))
        }));
    }
    hovered.target = None;
    hovered.guid = None;
    hovered.distance = f32::MAX;
    // The sticky pick drops the moment the pointer is not ours.
    let yielded = rig.is_looking() || pointer_over_ui.0;
    if yielded {
        *last_pick = None;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    // The aim: the OS pointer, else the probe's for a cursorless window. Published before the
    // yield, as the UI mouse feed reads it as its pointer: skipping it would drop `PointerOverUi`
    // and flicker the UI the probe rests on.
    let probing = super::hover_probe::armed();
    let probe_aim = cache
        .lock
        .point(camera, cam_tf, |e| parts.get(e).ok().map(|(_, gt, ..)| *gt))
        .or_else(|| super::hover_probe::point(window, *cache.frame));
    super::hover_probe::publish(probe_aim);
    if yielded {
        return;
    }
    let Some(cursor) = window.cursor_position().or(probe_aim) else {
        return;
    };
    if pickable.is_empty() {
        *last_pick = None;
        return;
    }
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    let self_store = self_q.single().ok();

    // The probe's census, once a second: where the pickable GameObjects are on screen, so a
    // headless miss tells a pick fault from a camera looking elsewhere.
    if probing && (*cache.frame).is_multiple_of(60) {
        let rows: Vec<String> = pickable
            .iter()
            .filter_map(|&e| {
                let (_, gt, vis, ..) = parts.get(e).ok()?;
                let world = gt.translation();
                let screen = camera.world_to_viewport(cam_tf, world).ok();
                Some(format!(
                    "{:?}{} → {}",
                    resolve_net(e),
                    if vis.get() { "" } else { " (unseen)" },
                    screen.map_or_else(
                        || "off-camera".to_string(),
                        |p| format!("{:.0},{:.0}", p.x, p.y)
                    ),
                ))
            })
            .take(8)
            .collect();
        info!(
            "hover probe census: window {:.0}x{:.0}, {} pickable — {}",
            window.width(),
            window.height(),
            pickable.len(),
            rows.join(" · ")
        );
    }

    // The hit list's admission (`0x4f96c4`-`0x4f96e0`): an object its highlight test refuses
    // (`vtbl+0xa4`, the type's `CanHighlight`) never enters either pass, so it neither takes the
    // pick nor hides the one behind it. A store not yet streamed is admitted.
    let admitted = |net: Entity| {
        let Ok(store) = stores.get(net) else {
            return true;
        };
        let guid = guids.get(net).ok().map(|g| g.0);
        crate::target::cursor_mode::mouseover_eligible(
            store.0.gameobject_type_id(),
            store.0.gameobject_flags(),
            store.0.gameobject_dynamic_flags(),
            guid.and_then(|g| go_gate.templates.get(g))
                .map(|t| t.highlight_column),
            crate::target::cursor_mode::go_reaction(
                go_gate.factions.as_deref(),
                store.0.gameobject_faction(),
                self_store,
            ),
            crate::target::cursor_mode::GoOverrides {
                channel_owned: crate::target::cursor_mode::fishing_channel_owned(self_store, guid),
            },
        )
    };

    // Pass 1: the exact resident geometry, the nearest admitted object wins. The front hit alone
    // first; the whole list only when the front object is refused.
    let nearest = |all_hits| {
        benilla_world::interact::cast_pick_ray(ray, pickable, &parts, all_hits)
            .into_iter()
            .find(|&(e, _)| admitted(resolve_net(e)))
            .map(|(e, h)| (h.distance, e))
    };
    let front = benilla_world::interact::cast_pick_ray(ray, pickable, &parts, false);
    let mut best: Option<(f32, Entity)> = match front.first() {
        Some((e, h)) if admitted(resolve_net(*e)) => Some((h.distance, *e)),
        Some(_) => nearest(true),
        None => None,
    };

    // Pass 2, only when nothing hit anywhere, units included (one resolve in the reference): the
    // geometry pushed 1 model unit along its normals, a halo of about a yard that makes a wispy
    // herb clickable around its leaves. Every unit outranks every GameObject on that ladder, so a
    // unit hover keeps it shut. The reference's sticky pick is cross-type and can hold a
    // GameObject over a unit's halo; here the unit wins that frame.
    if best.is_none() {
        if unit_hovered.target.is_some() {
            *last_pick = None;
            return;
        }
        let mut best2: Option<(f32, u32, Entity)> = None;
        for (part, hit) in benilla_world::interact::cast_pick_ray_inflated(ray, pickable, &parts) {
            let net = resolve_net(part);
            if !admitted(net) {
                continue;
            }
            let prio = if *last_pick == Some(net) {
                u32::MAX // the sticky-hover cache outranks everything
            } else {
                // Highlightable 1, else 0. A store not yet streamed reads highlightable, the
                // eligibility gate's own default.
                stores.get(net).map_or(1, |s| {
                    let reaction = crate::target::cursor_mode::go_reaction(
                        go_gate.factions.as_deref(),
                        s.0.gameobject_faction(),
                        self_store,
                    );
                    let go_guid = guids.get(net).ok().map(|g| g.0);
                    let overrides = crate::target::cursor_mode::GoOverrides {
                        channel_owned: crate::target::cursor_mode::fishing_channel_owned(
                            self_store, go_guid,
                        ),
                    };
                    u32::from(crate::target::cursor_mode::go_highlightable(
                        s, reaction, overrides,
                    ))
                })
            };
            let wins =
                best2.is_none_or(|(bt, bp, _)| prio > bp || (prio == bp && hit.distance < bt));
            if wins {
                best2 = Some((hit.distance, prio, part));
            }
        }
        best = best2.map(|(t, _, e)| (t, e));
    }

    // Occlusion: a strictly nearer world hit drops the pick, a tie keeps it.
    if best.is_some_and(|(t, _)| occlusion.distance < t) {
        best = None;
    }
    let picked = best.map(|(t, e)| (t, resolve_net(e)));
    *last_pick = picked.map(|(_, net)| net);
    let Some((distance, net_entity)) = picked else {
        if probing {
            cache.report.say(format!(
                "aim {cursor:?} — no GameObject hit (pickable {})",
                pickable.len()
            ));
        }
        return;
    };
    let Ok(guid) = guids.get(net_entity) else {
        return;
    };
    // The mouseover-eligibility gate (vtable `+0x54`): an ineligible object publishes the null
    // mouseover, so no tooltip, brighten or cursor; the reference reaches the tooltip, the brighten
    // and the cursor only past it. Its null mouseover also hides a unit behind the object, which
    // our split picks leave hoverable.
    if let Ok(store) = stores.get(net_entity) {
        let tmpl = go_gate.templates.get(guid.0);
        let reaction = crate::target::cursor_mode::go_reaction(
            go_gate.factions.as_deref(),
            store.0.gameobject_faction(),
            self_store,
        );
        if !crate::target::cursor_mode::mouseover_eligible(
            store.0.gameobject_type_id(),
            store.0.gameobject_flags(),
            store.0.gameobject_dynamic_flags(),
            tmpl.map(|t| t.highlight_column),
            reaction,
            crate::target::cursor_mode::GoOverrides {
                channel_owned: crate::target::cursor_mode::fishing_channel_owned(
                    self_store,
                    Some(guid.0),
                ),
            },
        ) {
            if probing {
                cache.report.say(format!(
                    "guid {:#x} type {} at {distance:.1} yd — hover ✗ (tmpl {:?}, highlight {:?}): \
                     the eligibility gate refused it, so there is no tooltip BY DESIGN",
                    guid.0,
                    store.0.gameobject_type_id(),
                    tmpl.map(|t| t.name.as_str()),
                    tmpl.map(|t| t.highlight_column),
                ));
            }
            return;
        }
        if probing {
            cache.report.say(format!(
                "guid {:#x} type {} at {distance:.1} yd — hover ✓, published (tmpl {:?}, highlight \
                 {:?}, occlusion {:.1})",
                guid.0,
                store.0.gameobject_type_id(),
                tmpl.map(|t| t.name.as_str()),
                tmpl.map(|t| t.highlight_column),
                occlusion.distance,
            ));
            // `lock` holds only an object past the eligibility gate, the hit kept in the part's
            // frame so the aim survives the camera and the object moving.
            if super::hover_probe::locks() {
                if let Some((t, part)) = best {
                    if let Ok((_, gt, ..)) = parts.get(part) {
                        cache.lock.hold(part, gt, ray.origin + *ray.direction * t);
                    }
                }
            }
        }
    }
    hovered.target = Some(net_entity);
    hovered.guid = Some(guid.0);
    hovered.distance = distance;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::targeting::corpse_fixture as fx;
    use bevy::ecs::system::RunSystemOnce;

    /// The ground leg stops on a GameObject hull above the terrain, and a zeppelin's hull gives its
    /// guid and the point in its frame (`0x4f9b5c`, `0x4f9d19`); a plain hull names no guid, and
    /// with no location wanted the trace is the world's alone.
    #[test]
    fn the_ground_leg_stops_on_a_hull_above_the_terrain() {
        use avian3d::prelude::{Collider, RigidBody};
        #[derive(Resource)]
        struct Aim(bool);
        #[derive(Resource, Default)]
        struct Out(PickOcclusion);
        fn trace(
            aim: Res<Aim>,
            spatial: avian3d::prelude::SpatialQuery,
            occluders: Query<(), With<PickOccluder>>,
            doodads: DoodadOccluders,
            parents: Query<&ChildOf>,
            objects: PickObjects,
            mut out: ResMut<Out>,
        ) {
            let ray = Ray3d::new(Vec3::new(1.0, 50.0, 2.0), Dir3::NEG_Y);
            out.0 = trace_occlusion(
                ray, &spatial, &occluders, &doodads, aim.0, &parents, &objects,
            );
        }
        const ZEPPELIN: u64 = 0x1FC0_0000_0000_0010;
        let run = |wants_location: bool, guid: u64, transport: bool| {
            let mut app = App::new();
            app.add_plugins((
                MinimalPlugins,
                bevy::transform::TransformPlugin,
                bevy::asset::AssetPlugin::default(),
                bevy::scene::ScenePlugin,
                avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
            ));
            app.init_asset::<Mesh>();
            app.finish();
            app.cleanup();
            app.insert_resource(Aim(wants_location))
                .init_resource::<Out>()
                .add_systems(Update, trace);
            // The terrain, a world occluder at y 0.
            app.world_mut().spawn((
                PickOccluder,
                RigidBody::Static,
                Collider::cuboid(40.0, 1.0, 40.0),
                Transform::from_xyz(0.0, -0.5, 0.0),
            ));
            // A deck turned a quarter, its hull a child collider 10 above the terrain.
            let deck = Transform::from_xyz(0.0, 10.0, 0.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
            let mut object = app.world_mut().spawn((
                Guid(guid),
                NetEntity {
                    kind: EntityKind::GameObject,
                    display_id: None,
                    scale: 1.0,
                },
                deck,
                RigidBody::Kinematic,
            ));
            if transport {
                object.insert(crate::transport::Transport::parked());
            }
            let object = object.id();
            app.world_mut().spawn((
                Collider::cuboid(8.0, 1.0, 8.0),
                Transform::from_xyz(0.0, -0.5, 0.0),
                ChildOf(object),
            ));
            for _ in 0..3 {
                app.update();
            }
            (app.world().resource::<Out>().0, deck)
        };
        let (on_deck, deck) = run(true, ZEPPELIN, true);
        assert_eq!(
            on_deck.point,
            Some(Vec3::new(1.0, 0.0, 2.0)),
            "the world hit"
        );
        let ground = on_deck
            .object_ground
            .expect("the hull stops the ground leg");
        assert!(ground.point.distance(Vec3::new(1.0, 10.0, 2.0)) < 1e-3);
        let (guid, local) = ground.deck.expect("a zeppelin names itself");
        assert_eq!(guid, ZEPPELIN);
        let want = deck.rotation.inverse() * (ground.point - deck.translation);
        assert!(local.distance(want) < 1e-3, "{local} vs {want}");
        assert!(
            on_deck.ground_point().unwrap().distance(ground.point) < 1e-3,
            "the reticle rides the deck"
        );
        let (plain, _) = run(true, 0xF110_0000_0000_0020, false);
        assert_eq!(
            plain.object_ground.and_then(|g| g.deck),
            None,
            "no guid for a plain hull"
        );
        assert!(
            plain.object_ground.is_some(),
            "a plain hull still stops the ground leg"
        );
        let (hover, _) = run(false, ZEPPELIN, true);
        assert_eq!(hover.object_ground, None, "the world leg passes the hull");
        assert_eq!(hover.point, Some(Vec3::new(1.0, 0.0, 2.0)));
    }

    fn card(bone: u16, owner: Entity) -> BillboardCard {
        let info = benilla_assets::BillboardInfo {
            pivot: Vec3::ZERO,
            bone,
            kind: benilla_formats::BillboardKind::LockZ,
            scale_anim: None,
            seq_translations: vec![],
        };
        BillboardCard::following(&info, owner)
    }

    /// Both card shapes `spawn_billboard_part` makes: following the mirror anchor (a rigless host)
    /// and following a nested joint (a rigged one).
    #[test]
    fn a_billboard_cards_hit_resolves_to_its_gameobject() {
        let mut world = World::new();
        let net = world.spawn(Guid(0xdead_beef)).id();
        // An ordinary mesh part, a direct child.
        let mesh_part = world.spawn(ChildOf(net)).id();
        // The rigless shape: the card follows the mirror anchor under the net entity.
        let anchor = world.spawn(ChildOf(net)).id();
        let anchor_card = world.spawn(card(0, anchor)).id();
        // The rigged shape: net → model root → root-bone joint → child joint, card on the deepest.
        let root = world.spawn(ChildOf(net)).id();
        let joint0 = world.spawn(ChildOf(root)).id();
        let joint1 = world.spawn(ChildOf(joint0)).id();
        let joint_card = world.spawn(card(1, joint1)).id();
        // A card whose owner is gone (despawned mid-frame) must not climb into someone else's tree.
        let orphan_card = world
            .spawn(card(2, Entity::from_raw_u32(9999).unwrap()))
            .id();

        let resolved = world
            .run_system_once(
                move |cards: Query<&BillboardCard>,
                      guids: Query<&Guid>,
                      child_of: Query<&ChildOf>|
                      -> Vec<Entity> {
                    [mesh_part, anchor_card, joint_card, orphan_card, net]
                        .into_iter()
                        .map(|e| net_entity_of(e, &cards, &guids, &child_of))
                        .collect()
                },
            )
            .unwrap();

        assert_eq!(
            resolved[0], net,
            "mesh part: the one-hop case still resolves"
        );
        assert_eq!(
            resolved[1], net,
            "anchor-following card: the world root hops through `follows()`, not `ChildOf`"
        );
        assert_eq!(
            resolved[2], net,
            "joint-following card: the climb continues up the joint hierarchy, not one hop"
        );
        assert_ne!(
            resolved[3], net,
            "a card whose owner is gone resolves to nothing that carries a guid"
        );
        assert_eq!(resolved[4], net, "the net entity resolves to itself");
    }

    /// First person hides our own parts (`Visibility::Hidden`), and the posed pick drops them, as
    /// the render test refuses the active player then; anyone else's stay the root's call.
    #[test]
    fn our_hidden_parts_leave_the_posed_pick() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let rig = world.spawn_empty().id();
        let part = |world: &mut World, drawn| {
            world
                .spawn((
                    ChildOf(root),
                    Mesh3d(Handle::default()),
                    benilla_world::rig_palette::RigPart(rig),
                    drawn,
                ))
                .id()
        };
        let shown = part(&mut world, InheritedVisibility::VISIBLE);
        part(&mut world, InheritedVisibility::HIDDEN);
        let count = move |is_self: bool| {
            move |kids: Query<&Children>,
                  parts: Query<(
                &Mesh3d,
                &benilla_world::rig_palette::RigPart,
                Option<&InheritedVisibility>,
            )>| {
                posed_parts(kids.get(root).expect("the parts"), &parts, is_self).len()
            }
        };
        assert_eq!(world.run_system_once(count(false)).unwrap(), 2);
        assert_eq!(
            world.run_system_once(count(true)).unwrap(),
            1,
            "ours: only the drawn part"
        );
        world.entity_mut(shown).insert(InheritedVisibility::HIDDEN);
        assert_eq!(
            world.run_system_once(count(true)).unwrap(),
            0,
            "first person: nothing of ours to pick"
        );
    }

    /// The picker's inputs with a camera ten yards up the Z axis, looking down it through a
    /// 100-pixel square with the cursor at its centre, over whatever stands at the origin.
    fn pick_world() -> World {
        let mut world = World::new();
        pick_world_in(&mut world);
        world
    }

    /// [`pick_world`]'s inputs, in a world that already exists.
    fn pick_world_in(world: &mut World) {
        use bevy::camera::RenderTargetInfo;
        use bevy::window::WindowResolution;

        world.init_resource::<CameraControl>();
        world.init_resource::<PointerOverUi>();
        world.insert_resource(PickOcclusion {
            distance: f32::INFINITY,
            point: None,
            ..Default::default()
        });
        world.init_resource::<crate::vplates::PlateHover>();
        world.init_resource::<Hovered>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<benilla_world::rig_palette::RigPalettes>();
        world.insert_resource(crate::net::SelfGuid(Some(1)));
        world.init_resource::<crate::spell::PicksSelf>();
        world.init_resource::<crate::spell::CorpsePick>();
        world.init_resource::<crate::spell::targeting::UnitPick>();
        world.insert_resource(crate::net::Reputations(Vec::new(), Vec::new()));
        let mut camera = Camera::default();
        camera.computed.clip_from_view = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::splat(100),
            scale_factor: 1.0,
        });
        world.spawn((
            camera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
            WorldCamera,
        ));
        let mut window = Window {
            resolution: WindowResolution::new(100, 100),
            ..Default::default()
        };
        window.set_cursor_position(Some(Vec2::splat(50.0)));
        world.spawn((window, PrimaryWindow));
    }

    /// What the cursor's ray down the Z axis picks of one skinned unit whose root stands at `root`
    /// while its rig is drawn at `seat`, as [`crate::vehicle`] reseats a passenger: a one-bone body
    /// whose single triangle spans a yard around its model origin, playing a sequence whose bounds
    /// sphere is a 1.5-yard ball there.
    fn pick_seated(root: Vec3, seat: Vec3) -> Option<Entity> {
        use benilla_assets::{
            AnimClip, ModelAnimations, ModelJoint, ModelSkeleton, ATTRIBUTE_WOW_JOINT_INDEX,
            ATTRIBUTE_WOW_JOINT_WEIGHT,
        };
        use bevy::asset::RenderAssetUsages;
        use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
        use bevy::mesh::{Indices, PrimitiveTopology};

        let mut app = App::new();
        app.init_resource::<Assets<SkinnedMeshInverseBindposes>>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<benilla_world::lighting::PropProbes>();
        benilla_world::rig_palette::plugin(&mut app);
        pick_world_in(app.world_mut());
        let world = app.world_mut();
        world.insert_resource(fx::factions());
        world.init_resource::<crate::spell::SpellTargeting>();
        world.spawn((
            SelfPlayer,
            Guid(1),
            GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
            fx::caster(1.5),
        ));

        let skeleton = ModelSkeleton {
            joints: vec![ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            }],
            ..Default::default()
        };
        let ibp = world
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY]));
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
        );
        mesh.insert_attribute(
            ATTRIBUTE_WOW_JOINT_INDEX,
            bevy::mesh::VertexAttributeValues::Uint16x4(vec![[0; 4]; 3]),
        );
        mesh.insert_attribute(ATTRIBUTE_WOW_JOINT_WEIGHT, vec![[1.0f32, 0.0, 0.0, 0.0]; 3]);
        mesh.insert_indices(Indices::U16(vec![0, 1, 2]));
        let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
        let sequence = AnimClip {
            anim_id: 0,
            seq_index: 0,
            node: Default::default(),
            looping: true,
            duration: 1.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: Vec3::ZERO,
            bounds_radius: 1.5,
            bounds_min: Vec3::splat(-1.0),
            bounds_max: Vec3::splat(1.0),
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        };
        let anims = ModelAnimations {
            graph: Handle::default(),
            clips: vec![sequence],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        };

        // The seat the rig hangs from, apart from the unit's own frame.
        let anchor = world
            .spawn((
                Transform::default(),
                GlobalTransform::from_translation(seat),
            ))
            .id();
        let unit = world
            .spawn((
                Guid(9),
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                GlobalTransform::from_translation(root),
                InheritedVisibility::VISIBLE,
                foe(),
                anims,
                AnimDriver::playing_gait(0),
                benilla_world::rig_anim::RigPose::new(anchor, &skeleton),
            ))
            .id();
        let joints = {
            let mut commands = world.commands();
            benilla_world::rig_palette::spawn_joints(&mut commands, anchor, unit, &skeleton)
        };
        world.flush();
        world
            .entity_mut(joints[0])
            .insert(GlobalTransform::from_translation(seat));
        let skin = benilla_world::rig_palette::RigSkin::allocate(
            &mut world.resource_mut::<benilla_world::rig_palette::RigPalettes>(),
            joints,
            ibp,
        )
        .expect("a fresh palette has room");
        world.entity_mut(unit).insert(skin);
        world.spawn((
            ChildOf(unit),
            Mesh3d(mesh),
            benilla_world::rig_palette::RigPart(unit),
            InheritedVisibility::VISIBLE,
        ));
        // The palette is written from the joints' globals, as each frame's compose leaves them.
        app.update();
        pick_under(app.world_mut(), None).target
    }

    /// The broad phase centres the playing sequence's sphere in the frame the model is drawn in, as
    /// `0x81cff0` takes each model's own matrix: a seated passenger is picked through its drawn body
    /// with its root out of the ray, and not through its empty root.
    #[test]
    fn a_seated_unit_is_picked_where_its_rig_is_drawn() {
        let on_ray = Vec3::new(0.0, 0.0, 2.0);
        let aside = Vec3::new(6.0, 0.0, 0.0);
        assert!(
            pick_seated(on_ray, on_ray).is_some(),
            "an unseated unit: the root is the drawn frame"
        );
        assert!(
            pick_seated(aside, on_ray).is_some(),
            "the ray through the drawn body picks it, root or not"
        );
        assert_eq!(
            pick_seated(on_ray, aside),
            None,
            "the ray through the empty root picks nothing"
        );
    }

    /// One drawn pick box around the origin, the box fallback's geometry for a partless body.
    fn pick_box(world: &mut World, body: Entity) {
        world.spawn((
            ChildOf(body),
            CreaturePickPart,
            Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            GlobalTransform::default(),
            InheritedVisibility::VISIBLE,
        ));
    }

    /// Our own body joins the unit pick only while [`crate::spell::PicksSelf`] says the word takes
    /// it, here through the box fallback of a body with no skinned parts.
    #[test]
    fn our_body_joins_the_pick_only_while_the_word_takes_it() {
        let mut world = pick_world();
        // Us, at the origin under the cursor, with one drawn pick box.
        let me = world
            .spawn((
                SelfPlayer,
                Guid(1),
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: None,
                    scale: 1.0,
                },
                GlobalTransform::default(),
                InheritedVisibility::VISIBLE,
            ))
            .id();
        pick_box(&mut world, me);
        let hover = |world: &mut World, picks: bool| {
            world.resource_mut::<crate::spell::PicksSelf>().0 = picks;
            world
                .run_system_cached(update_hover)
                .expect("the pick runs");
            world.resource::<Hovered>().target
        };

        assert_eq!(hover(&mut world, false), None, "not ours to pick");
        assert_eq!(hover(&mut world, true), Some(me), "a word that takes us");
        assert_eq!(hover(&mut world, false), None, "and not once it ends");
    }

    /// A corpse is a candidate outside targeting (`0x5c` carries flag `0x40`); while targeting,
    /// only under a word in `0x8600` and only one the corpse filter passes, as
    /// [`crate::spell::CorpsePick`] carries the standing word to the picker.
    #[test]
    fn a_corpse_joins_the_pick_as_the_word_admits_it() {
        use crate::spell::targeting::corpse_fixture as fx;

        let hover = |corpse: ObjectStore, word: Option<u32>| {
            let mut world = pick_world();
            world.insert_resource(fx::factions());
            world.init_resource::<crate::spell::SpellTargeting>();
            // Us, a human, away from the ray: the reaction gate's other side.
            world.spawn((
                SelfPlayer,
                Guid(1),
                GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
                fx::caster(1.5),
            ));
            let body = world
                .spawn((
                    Guid(fx::CORPSE),
                    NetEntity {
                        kind: EntityKind::Corpse,
                        display_id: None,
                        scale: 1.0,
                    },
                    GlobalTransform::default(),
                    InheritedVisibility::VISIBLE,
                    corpse,
                ))
                .id();
            pick_box(&mut world, body);
            if let Some(word) = word {
                world.resource_mut::<crate::spell::SpellTargeting>().enter(
                    fx::RESURRECTION,
                    crate::spell::CastCommit::Spell,
                    word,
                );
            }
            world
                .run_system_cached(crate::spell::targeting::publish_corpse_pick)
                .expect("the word publishes");
            world
                .run_system_cached(update_hover)
                .expect("the pick runs");
            let hovered = *world.resource::<Hovered>();
            assert_eq!(hovered.target, None, "a corpse is never the unit slot");
            hovered.corpse.inspect(|&e| assert_eq!(e, body)).is_some()
        };
        let friend = || fx::corpse(fx::HUMAN, false);
        let foe = || fx::corpse(fx::ORC, false);
        let bones = || fx::corpse(fx::HUMAN, true);

        // Not targeting: every corpse, bones and foes included.
        for (corpse, what) in [(friend(), "friend"), (foe(), "foe"), (bones(), "bones")] {
            assert!(hover(corpse, None), "{what} outside targeting");
        }
        // Resurrection's word: a friend's corpse, never a foe's or bones.
        assert!(hover(friend(), Some(0x8000)));
        assert!(!hover(foe(), Some(0x8000)), "a hostile corpse");
        assert!(!hover(bones(), Some(0x8000)), "bones");
        // The enemy bit takes the other side.
        assert!(hover(foe(), Some(0x0200)));
        assert!(!hover(friend(), Some(0x0200)));
        // A heal word sets no flag `0x40`: the corpse is out of the pick, so a unit behind it
        // is reachable.
        assert!(!hover(friend(), Some(0x0002)));
        assert!(!hover(friend(), Some(0x0040)), "a ground word");
    }

    /// A unit or player at depth `z` on the cursor's ray with one drawn pick box, of this store.
    fn unit_at(world: &mut World, guid: u64, z: f32, store: ObjectStore) -> Entity {
        let at = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, z));
        let body = world
            .spawn((
                Guid(guid),
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                at,
                InheritedVisibility::VISIBLE,
                store,
            ))
            .id();
        world.spawn((
            ChildOf(body),
            CreaturePickPart,
            Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            at,
            InheritedVisibility::VISIBLE,
        ));
        body
    }

    /// A store of `UNIT_FIELD_FACTIONTEMPLATE` (35), `UNIT_FIELD_FLAGS` (46) and
    /// `UNIT_FIELD_HEALTH` (22), on [`fx::factions`]' rows: template 1 the caster's Human, 2 an Orc.
    fn unit_store(template: u32, flags: u32, health: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (35, template),
            (46, flags),
            (22, health),
        ]))
    }

    /// A living player of the caster's faction: assistable, not attackable.
    fn friend() -> ObjectStore {
        unit_store(1, 0x8, 100)
    }

    /// A living Orc creature: attackable, not assistable.
    fn foe() -> ObjectStore {
        unit_store(2, 0, 100)
    }

    /// The pick over `word`, the way a frame runs it: the word published, its flags `0x20` and the
    /// unit legs both, then the pick.
    fn pick_under(world: &mut World, word: Option<u32>) -> Hovered {
        let mut targeting = world.resource_mut::<crate::spell::SpellTargeting>();
        match word {
            Some(word) => targeting.enter(fx::RESURRECTION, crate::spell::CastCommit::Spell, word),
            None => targeting.clear(),
        }
        world
            .run_system_cached(crate::spell::targeting::publish_unit_pick)
            .expect("the word publishes");
        world
            .run_system_cached(crate::spell::targeting::publish_picks_self)
            .expect("the self bit publishes");
        world
            .run_system_cached(update_hover)
            .expect("the pick runs");
        *world.resource::<Hovered>()
    }

    /// Us, a human away from the ray, the faction tables, and the units a case puts on it.
    fn unit_pick_world() -> World {
        let mut world = pick_world();
        world.insert_resource(fx::factions());
        world.init_resource::<crate::spell::SpellTargeting>();
        world.spawn((
            SelfPlayer,
            Guid(1),
            GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
            fx::caster(1.5),
        ));
        world
    }

    /// The unit filter while a word stands: a unit the word cannot take is no candidate, so the one
    /// behind it takes the pick, where outside targeting the nearer wins whatever it is. The front
    /// unit is 5 yards nearer the camera than the rear one.
    #[test]
    fn a_unit_the_word_cannot_take_lets_the_pick_through_to_the_one_behind() {
        const HEAL: u32 = 0x0100;
        const ENEMY: u32 = 0x0080;
        const RESURRECTION: u32 = 0x8000;
        const SKINNING: u32 = 0x0402;
        // (word, the front unit it refuses, the rear unit it takes, what stands behind it)
        let cases: [(&str, u32, ObjectStore, ObjectStore); 6] = [
            ("a foe before a friend, under a heal", HEAL, foe(), friend()),
            (
                "a corpse before a friend, under a heal",
                HEAL,
                unit_store(1, 0x8, 0),
                friend(),
            ),
            (
                "a friend before a foe, under an enemy word",
                ENEMY,
                friend(),
                foe(),
            ),
            (
                "the living before a corpse, under Resurrection",
                RESURRECTION,
                friend(),
                unit_store(1, 0x8, 0),
            ),
            (
                "the living before a corpse, under Skinning",
                SKINNING,
                foe(),
                unit_store(2, 0, 0),
            ),
            (
                "a unit immune to players before a friend, under a heal",
                HEAL,
                unit_store(1, 0x108, 100),
                friend(),
            ),
        ];
        for (what, word, front_store, rear_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear = unit_at(&mut world, 11, 0.0, rear_store);
            // Outside targeting the nearer unit wins, whoever it is.
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(front),
                "{what}: outside targeting"
            );
            let hovered = pick_under(&mut world, Some(word));
            assert_eq!(
                hovered.target,
                Some(rear),
                "{what}: the word takes the rear"
            );
            assert_eq!(hovered.guid, Some(11), "{what}");
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(front),
                "{what}: and back outside targeting"
            );
        }
    }

    /// The control of the table above: the same front unit is the pick when the word takes it.
    #[test]
    fn a_unit_the_word_takes_keeps_the_pick_in_front_of_the_one_behind() {
        let cases: [(&str, u32, ObjectStore); 4] = [
            ("a friend under a heal", 0x0100, friend()),
            ("a foe under an enemy word", 0x0080, foe()),
            ("a corpse under Resurrection", 0x8000, unit_store(1, 0x8, 0)),
            ("a foe under a plain unit word", 0x0002, foe()),
        ];
        for (what, word, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear_store = if word == 0x8000 {
                unit_store(1, 0x8, 0)
            } else {
                friend()
            };
            unit_at(&mut world, 11, 0.0, rear_store);
            assert_eq!(
                pick_under(&mut world, Some(word)).target,
                Some(front),
                "{what}"
            );
        }
    }

    /// With nothing behind it, a unit the word cannot take leaves the pick empty, which is no
    /// refusal: the selectable grader grades the winner only, and there is none.
    #[test]
    fn a_lone_unit_the_word_cannot_take_is_no_pick_at_all() {
        let mut world = unit_pick_world();
        unit_at(&mut world, 10, 0.0, foe());
        let hovered = pick_under(&mut world, Some(0x0100));
        assert_eq!((hovered.target, hovered.guid), (None, None));
        assert!(!hovered.refused);
        assert_eq!(hovered.distance, f32::MAX);
    }

    /// A living Orc creature whose `UNIT_FIELD_FLAGS` are `flags`, with its `UNIT_FIELD_CHARMEDBY`
    /// (10) and `UNIT_FIELD_CREATEDBY` (14) the guids `charmed_by` and `created_by`, 0 for none.
    fn owned(flags: u32, charmed_by: u32, created_by: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (35, 2),
            (46, flags),
            (22, 100),
            (10, charmed_by),
            (11, 0),
            (14, created_by),
            (15, 0),
        ]))
    }

    /// The word's plain unit bit (`0x0002`), which the reference takes the player for (`0x850e`),
    /// and the enemy bit (`0x0080`), which it does not.
    const SELF_WORD: u32 = 0x0002;
    const ENEMY_WORD: u32 = 0x0080;

    /// A unit flagged `UNIT_FLAG_POSSESSED` whose charmer, else creator, is the active player is no
    /// candidate outside targeting, or while a word without `0x20` stands, so the pick passes
    /// through it to the unit behind. Us is guid 1.
    #[test]
    fn a_unit_you_possess_lets_the_pick_through_to_the_one_behind() {
        let possessed = crate::target::UNIT_FLAG_POSSESSED;
        let cases: [(&str, ObjectStore); 3] = [
            (
                "a Mind Controlled unit, charmed by us",
                owned(possessed, 1, 0),
            ),
            ("an Eye of Kilrogg, created by us", owned(possessed, 0, 1)),
            (
                "charmed by us, created by another: the charmer comes first",
                owned(possessed, 1, 7),
            ),
        ];
        for (what, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear = unit_at(&mut world, 11, 0.0, foe());
            for word in [None, Some(ENEMY_WORD)] {
                let hovered = pick_under(&mut world, word);
                assert_eq!(
                    hovered.target,
                    Some(rear),
                    "{what}: the rear, word {word:?}"
                );
                assert_eq!(hovered.guid, Some(11), "{what}: word {word:?}");
            }
            // The word that takes us takes it too: the exclusions are skipped.
            assert_eq!(
                pick_under(&mut world, Some(SELF_WORD)).target,
                Some(front),
                "{what}: while flag 0x20 stands"
            );
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(rear),
                "{what}: and excluded again once the word ends"
            );
        }
    }

    /// With nothing behind it, the unit you possess leaves the pick empty, which is no refusal.
    #[test]
    fn a_lone_unit_you_possess_is_no_pick_at_all() {
        let mut world = unit_pick_world();
        unit_at(
            &mut world,
            10,
            0.0,
            owned(crate::target::UNIT_FLAG_POSSESSED, 1, 0),
        );
        let hovered = pick_under(&mut world, None);
        assert_eq!((hovered.target, hovered.guid), (None, None));
        assert!(!hovered.refused);
    }

    /// The exclusion asks for the flag, then for us: an owned unit that is not possessed, a unit
    /// someone else possesses, and one whose charmer is another even where its creator is us, all
    /// stay candidates.
    #[test]
    fn a_unit_you_do_not_possess_keeps_the_pick() {
        let possessed = crate::target::UNIT_FLAG_POSSESSED;
        let cases: [(&str, ObjectStore); 5] = [
            ("possessed, charmed by another", owned(possessed, 2, 0)),
            ("possessed, created by another", owned(possessed, 0, 2)),
            (
                "possessed, charmed by another, created by us: the charmer comes first",
                owned(possessed, 2, 1),
            ),
            (
                "our own pet, created by us but not possessed",
                owned(0, 0, 1),
            ),
            ("possessed by no one", owned(possessed, 0, 0)),
        ];
        for (what, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            unit_at(&mut world, 11, 0.0, foe());
            for word in [None, Some(ENEMY_WORD), Some(SELF_WORD)] {
                assert_eq!(
                    pick_under(&mut world, word).target,
                    Some(front),
                    "{what}: word {word:?}"
                );
            }
        }
    }

    /// Only a word with unit bits (`0x878e`) puts units and players in the pick. A ground word, a
    /// lock word and a bag-item word pick none, so a unit in front of the ground or the chest is no
    /// obstacle, and a unit word does pick them.
    #[test]
    fn only_a_word_with_unit_bits_picks_units() {
        let mut world = unit_pick_world();
        let front = unit_at(&mut world, 10, 5.0, friend());
        for word in [0x0040, 0x0020, 0x4800, 0x0800, 0x0010] {
            let hovered = pick_under(&mut world, Some(word));
            assert_eq!(hovered.target, None, "{word:#06x}");
            assert!(!hovered.refused, "{word:#06x}");
        }
        for word in [0x0002, 0x0100] {
            assert_eq!(
                pick_under(&mut world, Some(word)).target,
                Some(front),
                "{word:#06x}"
            );
        }
    }

    /// The press latches the hover's pick, so the click reaches what the cursor showed: the unit
    /// behind, not the one in front the word refuses.
    #[test]
    fn the_press_latches_the_pick_the_word_left() {
        use crate::target::{latch_press_pick, PressPick};

        let mut world = unit_pick_world();
        unit_at(&mut world, 10, 5.0, foe());
        let rear = unit_at(&mut world, 11, 0.0, friend());
        world.init_resource::<HoveredObject>();
        world.init_resource::<PressPick>();
        world.init_resource::<crate::target::WorldCursor>();
        world.init_resource::<crate::target::cursor_mode::AttackFork>();
        world.init_resource::<ButtonInput<MouseButton>>();

        pick_under(&mut world, Some(0x0100));
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        world
            .run_system_cached(latch_press_pick)
            .expect("the press latches");
        assert_eq!(world.resource::<PressPick>().hovered.target, Some(rear));
    }

    /// Naxxramas's "Unholy Axe" is an `InvisibleStalker` body with no vertices holding a real axe,
    /// which is its whole pick surface.
    #[test]
    fn a_units_pick_geometry_is_every_model_chained_to_it() {
        let unit = Entity::from_raw_u32(1).unwrap();
        let main_hand = Entity::from_raw_u32(2).unwrap();
        let helm = Entity::from_raw_u32(3).unwrap();
        let mount = Entity::from_raw_u32(4).unwrap();

        let bare: Vec<_> = pick_model_roots(unit, &[], None).collect();
        assert_eq!(bare, vec![unit], "a unit wearing nothing is just its body");

        // Empty slots, as in the real `HeldAttached` (main, off, ranged, helm, both shoulders,
        // ammo, quiver), are not candidates.
        let worn = [
            Some(main_hand),
            None,
            None,
            Some(helm),
            None,
            None,
            None,
            None,
        ];
        let dressed: Vec<_> = pick_model_roots(unit, &worn, Some(mount)).collect();
        assert_eq!(
            dressed,
            vec![unit, main_hand, helm, mount],
            "body, then every worn model, then the mount — all one unit's geometry"
        );

        // A body that draws nothing: the axe is the whole hit surface.
        let trigger: Vec<_> = pick_model_roots(unit, &worn[..1], None).collect();
        assert!(
            trigger.contains(&main_hand),
            "a body with no geometry of its own still offers what it holds"
        );
    }
}
