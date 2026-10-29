//! The retained pass's per-frame CPU scene walk: admission per cell, WMO group and prop set, the
//! dead-region reap, the fader exile scan and the `WOW_GX_CENSUS` line.

use bevy::prelude::*;

use super::render::GxDoodadVis;
use super::{FaderState, GxFader, StaticGx};

/// Exile hysteresis in yards: entry into the feather is exact, but both exits need the camera this
/// far past the edge, so a camera parked on one cannot flap.
const FADE_HYST: f32 = 1.0;

/// [`FaderState`] without its entity list.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FadeClass {
    Steady,
    Feather,
    Gone,
}

impl GxFader {
    fn class(&self) -> FadeClass {
        match self.state {
            FaderState::Steady => FadeClass::Steady,
            FaderState::Exiled { .. } => FadeClass::Feather,
            FaderState::Gone => FadeClass::Gone,
        }
    }
}

/// Where a fader at distance `d` moves from `current`, by the entity path's own
/// [`crate::model_fade::doodad_fade_alpha`]; the exits shift the sample by [`FADE_HYST`].
fn fade_step(size: f32, d: f32, current: FadeClass) -> FadeClass {
    use crate::model_fade::doodad_fade_alpha as alpha;
    match current {
        FadeClass::Steady => {
            let a = alpha(size, d);
            if a >= 1.0 {
                FadeClass::Steady
            } else if a > 0.0 {
                FadeClass::Feather
            } else {
                FadeClass::Gone
            }
        }
        FadeClass::Feather => {
            if alpha(size, d + FADE_HYST) >= 1.0 {
                FadeClass::Steady
            } else if alpha(size, d - FADE_HYST) <= 0.0 {
                FadeClass::Gone
            } else {
                FadeClass::Feather
            }
        }
        FadeClass::Gone => {
            if alpha(size, d + FADE_HYST) >= 1.0 {
                FadeClass::Steady
            } else if alpha(size, d) > 0.0 {
                FadeClass::Feather
            } else {
                FadeClass::Gone
            }
        }
    }
}

/// How far a fader `d` yd away may move before [`fade_step`] can answer differently: its
/// distance to the nearer alpha threshold of its band (`doodad_fade_alpha_at`'s 0.99 and 0.01),
/// less the hysteresis either exit shifts it by. A fader with no band never changes.
fn class_slack(near: f32, far: f32, d: f32) -> f32 {
    if near == f32::MAX {
        return f32::MAX;
    }
    let span = far - near;
    let (steady_edge, gone_edge) = (near + 0.01 * span, far - 0.01 * span);
    ((d - steady_edge).abs().min((d - gone_edge).abs()) - FADE_HYST).max(0.0)
}

/// A referrer set's PVS verdict, `WmoGroupVis::drawn_by`'s rule: any named room visible, a missing
/// bit fail-open. An empty set, an unnamed prop, is never portal-culled.
fn set_admitted(visible: &[bool], rooms: &[u16]) -> bool {
    rooms.is_empty()
        || rooms
            .iter()
            .any(|&g| visible.get(usize::from(g)).copied().unwrap_or(true))
}

/// `WOW_GX_FADE_TRACE=1`: one line per exile transition.
fn fade_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_GX_FADE_TRACE").is_some())
}

/// Spawn an exiled placement as `assemble.rs`'s own doodad bundle (a prop's with its rooms),
/// seeded with the blend material and live fade alpha the fade authority owns from the next
/// `Update`.
fn spawn_exile(commands: &mut Commands, f: &GxFader, alpha: f32, admitted: bool) -> Vec<Entity> {
    let mut ents = Vec::with_capacity(f.batches.len());
    for b in &f.batches {
        let mut e = commands.spawn((
            Mesh3d(b.stat_mesh.clone()),
            MeshMaterial3d(if alpha < 1.0 {
                b.blend.clone()
            } else {
                b.cutout.clone()
            }),
            f.transform,
            // A placement is a world root: its Transform is its global.
            GlobalTransform::from(f.transform),
            crate::model_render::ModelPart {
                kind: crate::model_render::ModelKind::Doodad,
                blend: b.blend_mode,
            },
            crate::model_render::EntityPathWhy("exile"),
            crate::interact::PickMesh(b.geometry.clone()),
            bevy::mesh::MeshTag(crate::mesh_tag::alpha_bits(alpha)),
            crate::model_fade::DoodadFade {
                size: f.size,
                local_center: f.local_center,
                sphere: Some((f.center, f.radius)),
                cutout: b.cutout.clone(),
                blend: b.blend.clone(),
            },
            crate::exterior_cull::ExteriorScene,
            // The placement's own identity: the pick names it alike whichever lane draws it.
            (*f.object).clone(),
        ));
        if admitted {
            e.insert(bevy::camera::visibility::InheritedVisibility::VISIBLE);
        } else {
            e.insert((
                Visibility::Hidden,
                bevy::camera::visibility::InheritedVisibility::HIDDEN,
            ));
        }
        if let Some(aabb) = b.aabb {
            e.insert((aabb, bevy::camera::visibility::NoAutoAabb));
        }
        // A prop rides its rooms, as at its spawn site: the model-visibility authority culls it.
        if let Some((instance, groups)) = &f.room {
            e.insert(crate::wmo_portal::WmoGroupVis {
                instance: *instance,
                groups: std::sync::Arc::clone(groups),
            });
        }
        ents.push(e.id());
    }
    if fade_trace() {
        println!("GX_FADE_SPAWN uid={} ents={ents:?}", f.uid);
    }
    ents
}

/// One region's exile scan: arm last frame's spawns, step each fader across its band, and rebuild
/// the kill bitmap of `draw` from the states. `admitted` is an exile's spawn-frame verdict; the
/// entity path's own test runs from the next `Update`. Returns whether an exile waits to arm.
fn scan_faders(
    commands: &mut Commands,
    cell: &mut super::GxCell,
    draw: Option<&mut std::sync::Arc<super::render::GxCellDraw>>,
    cam_pos: Vec3,
    fade_events: &mut [u32; 3],
    label: &dyn std::fmt::Display,
    admitted: impl Fn(&GxFader) -> bool,
) -> bool {
    let Some((mn, mx)) = cell.fader_bounds else {
        return false;
    };
    // The camera's distance range to the fader centres against the band union, with the
    // hysteresis margin, so a wholesale verdict holds for every placement.
    let dmin = cam_pos.distance(cam_pos.clamp(mn, mx));
    let far = |c: f32, lo: f32, hi: f32| {
        if (c - lo).abs() > (c - hi).abs() {
            lo
        } else {
            hi
        }
    };
    let corner = Vec3::new(
        far(cam_pos.x, mn.x, mx.x),
        far(cam_pos.y, mn.y, mx.y),
        far(cam_pos.z, mn.z, mx.z),
    );
    let dmax = cam_pos.distance(corner);
    let (near_min, far_max) = cell.ring;
    let verdict = if dmax < near_min - FADE_HYST {
        Some(true) // every placement steady, with margin
    } else if dmin > far_max + FADE_HYST {
        Some(false) // every placement gone, with margin
    } else {
        None // the ring straddles the cell: walk it
    };
    if verdict.is_some() && verdict == cell.settled && !cell.bits_stale {
        return false;
    }
    // A fader changes class only when its distance crosses a band edge, and the distance moves no
    // more than the camera does: until the camera has travelled the cell's slack, nothing can.
    if !cell.bits_stale
        && cell
            .slack
            .is_some_and(|(at, margin)| cam_pos.distance(at) < margin)
    {
        return false;
    }
    let mut bits_changed = cell.bits_stale;
    let mut unarmed_left = false;
    let mut slack = f32::MAX;
    for f in cell.faders.values_mut() {
        // Last frame's exile spawns draw from this frame: arm their kill bits now.
        if let FaderState::Exiled { armed, .. } = &mut f.state {
            if !*armed {
                *armed = true;
                bits_changed = true;
            }
        }
        let d = f.center.distance(cam_pos);
        slack = slack.min(class_slack(f.near, f.far, d));
        let old = f.class();
        let new = fade_step(f.size, d, old);
        if new == old {
            continue;
        }
        let was_killed = matches!(
            f.state,
            FaderState::Exiled { armed: true, .. } | FaderState::Gone
        );
        if let FaderState::Exiled { ents, .. } = std::mem::replace(&mut f.state, FaderState::Steady)
        {
            for e in ents {
                commands.entity(e).try_despawn();
            }
        }
        f.state = match new {
            FadeClass::Steady => {
                fade_events[1] += 1;
                FaderState::Steady
            }
            FadeClass::Gone => {
                fade_events[2] += 1;
                FaderState::Gone
            }
            FadeClass::Feather => {
                fade_events[0] += 1;
                let alpha = crate::model_fade::doodad_fade_alpha(f.size, d);
                // Unarmed until the entities draw, unless it was Gone and is already killed.
                let armed = old == FadeClass::Gone;
                unarmed_left |= !armed;
                FaderState::Exiled {
                    ents: spawn_exile(commands, f, alpha, admitted(f)),
                    armed,
                }
            }
        };
        let now_killed = matches!(
            f.state,
            FaderState::Exiled { armed: true, .. } | FaderState::Gone
        );
        if was_killed != now_killed {
            bits_changed = true;
        }
        if fade_trace() {
            println!(
                "GX_FADE {label} uid={} {:?}->{:?} d={:.1} r={:.2}",
                f.uid, old, new, d, f.size
            );
        }
    }
    // An unarmed exile must be revisited next frame whatever the ring says.
    cell.settled = if unarmed_left { None } else { verdict };
    cell.slack = (!unarmed_left).then_some((cam_pos, slack));
    if bits_changed {
        if let Some(draw) = draw {
            // Copy-on-write: the region is shared with the render world's extracted clone.
            let draw = std::sync::Arc::make_mut(draw);
            // Rebuilt whole from the states, so a re-bake's new indices cannot drift.
            draw.killed.iter_mut().for_each(|w| *w = 0);
            for f in cell.faders.values() {
                let killed = match f.state {
                    FaderState::Steady | FaderState::Exiled { armed: false, .. } => false,
                    FaderState::Exiled { armed: true, .. } | FaderState::Gone => true,
                };
                if !killed {
                    continue;
                }
                for &i in &f.items {
                    let i = usize::from(i);
                    if let Some(w) = draw.killed.get_mut(i / 64) {
                        *w |= 1u64 << (i % 64);
                    }
                }
            }
            draw.killed_rev = draw.killed_rev.wrapping_add(1);
        }
        cell.bits_stale = false;
    }
    unarmed_left
}

/// Whether the last full cull's lists still answer: it read this draw epoch and fade generation,
/// and left no exile to arm.
fn memo_holds(memo: Option<(u64, u32, bool)>, epoch: u64, generation: u32) -> bool {
    memo.is_some_and(|(e, g, revisit)| e == epoch && g == generation && !revisit)
}

/// The CPU scene walk into this frame's visible lists: frustum, farclip and the exterior window
/// gate per cell (conservative: overdraw, never a hole), plus the portal PVS per WMO group, the
/// camera's own building exempt from the gate. A re-admitted exile despawns and clears its kill
/// bit in the same frame.
#[allow(clippy::type_complexity)]
pub(super) fn cull_cells(
    mut commands: Commands,
    mut gx: ResMut<StaticGx>,
    debug: Res<crate::dev_state::DebugState>,
    view: Res<crate::view::ViewDistance>,
    cam: Query<
        (
            Ref<GlobalTransform>,
            Ref<Projection>,
            &bevy::camera::primitives::Frustum,
        ),
        With<crate::view::WorldCamera>,
    >,
    windows: Res<crate::wmo_portal::ExteriorWindows>,
    claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    // A placement's PVS or fog lane moving, or a placement leaving, re-runs the cull.
    (moved_instances, mut gone_instances): (
        Query<(), Changed<crate::wmo_portal::WmoPortalInstance>>,
        RemovedComponents<crate::wmo_portal::WmoPortalInstance>,
    ),
) {
    let _t = super::gx_perf_guard(1);
    for e in gx.pending_despawn.drain(..) {
        commands.entity(e).try_despawn();
    }
    let cam_view = cam.iter().next();
    // A still frame over an unchanged draw set: every verdict below is last frame's, so the
    // published lists stand. Fader bands move only with the camera or `environmentDetail`.
    let gone = gone_instances.read().count();
    let scene_still = cam_view
        .as_ref()
        .is_some_and(|(t, p, _)| !t.is_changed() && !p.is_changed())
        && !debug.is_changed()
        && !view.is_changed()
        && !windows.is_changed()
        && !claim.is_changed()
        && moved_instances.is_empty()
        && gone == 0;
    if scene_still
        && memo_holds(
            gx.cull_memo,
            gx.draw_epoch,
            crate::model_fade::fade_generation(),
        )
    {
        return;
    }
    let StaticGx {
        cells,
        wmos,
        props,
        world,
        fade_events,
        fade_generation,
        draw_epoch,
        cull_memo,
        ..
    } = &mut *gx;
    // Whether this run leaves an exile to arm next frame, which a still frame must not skip.
    let mut revisit = false;
    // Reap regions whose instance died with its placement; a WMO placement never changes owner
    // tile (the straddler handoff is M2-only).
    wmos.retain(|e, _| instances.contains(*e));
    world.wmos.retain(|e, _| instances.contains(*e));
    // A dead building's prop exiles are this lane's own: they go with it.
    props.retain(|e, region| {
        let live = instances.contains(*e);
        if !live {
            let mut ents = Vec::new();
            region.queue_exiles(&mut ents);
            for e in ents {
                commands.entity(e).try_despawn();
            }
        }
        live
    });
    world.props.retain(|e, _| instances.contains(*e));
    world.visible.clear();
    world.visible_wmos.clear();
    let Some((cam_t, proj, frustum)) = cam_view else {
        return;
    };
    let (cam_t, proj) = (&*cam_t, &*proj);
    let cam_pos = cam_t.translation();
    let cam_fwd = Vec3::from(cam_t.forward());
    let gate = crate::exterior_cull::ExteriorGate::build(&windows, Some((cam_t, proj)));
    let m = &debug.models;
    let doodads_on =
        m.kind_visible[crate::model_render::kind_index(crate::model_render::ModelKind::Doodad)];

    // ---- The fader exile scan ----
    // An `environmentDetail` write moved every band: re-derive them before the scan reads them.
    let generation = crate::model_fade::fade_generation();
    if *fade_generation != generation {
        *fade_generation = generation;
        for cell in cells.values_mut().chain(props.values_mut()) {
            cell.refresh_bands();
        }
    }
    for (&key, cell) in cells.iter_mut() {
        if cell.faders.is_empty() {
            continue;
        }
        // Cell-grain admission for the spawn frame; the entity path's own test runs next Update.
        let admitted = doodads_on
            && world.cells.get(&key).is_none_or(|d| {
                gate.admits(&GlobalTransform::from_translation(d.origin), Some(&d.aabb))
            });
        revisit |= scan_faders(
            &mut commands,
            cell,
            world.cells.get_mut(&key),
            cam_pos,
            fade_events,
            &format_args!("cell=({},{})", key.0, key.1),
            |_| admitted,
        );
    }
    // Prop regions by the same law, a fader admitted on the spawn frame by its rooms' PVS.
    for (&entity, cell) in props.iter_mut() {
        if cell.faders.is_empty() {
            continue;
        }
        let inst = instances.get(entity).ok();
        revisit |= scan_faders(
            &mut commands,
            cell,
            world.props.get_mut(&entity),
            cam_pos,
            fade_events,
            &format_args!("props={entity}"),
            |f| {
                doodads_on
                    && f.room.as_ref().is_none_or(|(_, rooms)| {
                        inst.is_none_or(|i| set_admitted(&i.visible, rooms))
                    })
            },
        );
    }

    // The building the camera is in is not exterior to itself, for WMO groups and prop sets.
    let own = claim.0.map(|c| c.room.instance);
    // The dev doodad kind toggle hides every cell at once; the blend-layer toggles do not reach
    // this lane. Cells and prop regions, both the M2 scene, sort near-first as one list: the 1.12
    // client's front-to-back band walk (32 bands of 33⅓ yd, `0xc7bd40`) at cell grain.
    if doodads_on {
        let mut admitted: Vec<(u32, GxDoodadVis)> = Vec::new();
        for (&cell, draw) in &world.cells {
            let center = draw.origin + Vec3::from(draw.aabb.center);
            let radius = Vec3::from(draw.aabb.half_extents).length();
            if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                continue;
            }
            let sphere = bevy::camera::primitives::Sphere {
                center: center.into(),
                radius,
            };
            if !frustum.intersects_sphere(&sphere, false) {
                continue;
            }
            // ADT doodads are exterior scene: from inside a WMO they draw only through a window.
            // Deviation: the whole baked cell on its box, where the reference tests each
            // placement's sphere; the admitted cell holds every placement the window reaches.
            if !gate.admits(
                &GlobalTransform::from_translation(draw.origin),
                Some(&draw.aabb),
            ) {
                continue;
            }
            // Distance² is non-negative, so its IEEE bits sort exactly like the value.
            admitted.push((
                cam_pos.distance_squared(center).to_bits(),
                GxDoodadVis::Cell(cell),
            ));
        }
        // Prop regions join the list, admitted per referrer set: PVS (`set_admitted`), farclip,
        // frustum, and the exterior gate for named sets only; an unnamed prop is never gated.
        for (&entity, draw) in &world.props {
            // An instance not queryable yet (spawn-command latency) skips this frame.
            let Ok(inst) = instances.get(entity) else {
                continue;
            };
            let mut sel = crate::static_gx::render::GxSel {
                drawn: vec![false; draw.sets.len()],
                fog: vec![false; draw.sets.len()],
            };
            let mut any = false;
            for (set, aabb) in &draw.groups {
                let Some(rooms) = draw.sets.get(usize::from(*set)) else {
                    continue;
                };
                // The fog lane is written for every set, drawn or not: the record sync reads the
                // whole vector. A missing bit reads false (`interior_fogged_by`), not fail-open.
                sel.fog[usize::from(*set)] = rooms.iter().any(|&g| {
                    inst.interior_fog
                        .get(usize::from(g))
                        .copied()
                        .unwrap_or(false)
                });
                if m.portal_cull && !set_admitted(&inst.visible, rooms) {
                    continue;
                }
                let center = draw.origin + Vec3::from(aabb.center);
                let radius = Vec3::from(aabb.half_extents).length();
                if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                    continue;
                }
                let sphere = bevy::camera::primitives::Sphere {
                    center: center.into(),
                    radius,
                };
                if !frustum.intersects_sphere(&sphere, false) {
                    continue;
                }
                if !rooms.is_empty()
                    && Some(entity) != own
                    && !gate.admits(&GlobalTransform::from_translation(draw.origin), Some(aabb))
                {
                    continue;
                }
                sel.drawn[usize::from(*set)] = true;
                any = true;
            }
            if any {
                let center = draw.origin + Vec3::from(draw.aabb.center);
                admitted.push((
                    cam_pos.distance_squared(center).to_bits(),
                    GxDoodadVis::Prop(entity, sel),
                ));
            }
        }
        admitted.sort_unstable_by_key(|(d, _)| *d);
        world.visible.extend(admitted.into_iter().map(|(_, v)| v));
    }
    // WMO regions: the dev WMO toggle, then per-group admission, near-first.
    if m.kind_visible[crate::model_render::kind_index(crate::model_render::ModelKind::Wmo)] {
        let mut admitted: Vec<(u32, (Entity, crate::static_gx::render::GxSel))> = Vec::new();
        for (&entity, draw) in &world.wmos {
            let Ok(inst) = instances.get(entity) else {
                continue;
            };
            let max_group = draw.groups.iter().map(|(g, _)| *g).max().unwrap_or(0);
            let mut sel = crate::static_gx::render::GxSel {
                drawn: vec![false; usize::from(max_group) + 1],
                fog: vec![false; usize::from(max_group) + 1],
            };
            let mut any = false;
            for (group, aabb) in &draw.groups {
                // The fog lane for every group, drawn or not, as for prop sets.
                sel.fog[usize::from(*group)] = inst
                    .interior_fog
                    .get(usize::from(*group))
                    .copied()
                    .unwrap_or(false);
                // The portal PVS bit, fail-open like `WmoGroupVis::drawn_by`.
                if m.portal_cull
                    && !inst
                        .visible
                        .get(usize::from(*group))
                        .copied()
                        .unwrap_or(true)
                {
                    continue;
                }
                let center = draw.origin + Vec3::from(aabb.center);
                let radius = Vec3::from(aabb.half_extents).length();
                if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                    continue;
                }
                let sphere = bevy::camera::primitives::Sphere {
                    center: center.into(),
                    radius,
                };
                if !frustum.intersects_sphere(&sphere, false) {
                    continue;
                }
                // Another building is exterior scene; the camera's own is exempt.
                if Some(entity) != own
                    && !gate.admits(&GlobalTransform::from_translation(draw.origin), Some(aabb))
                {
                    continue;
                }
                sel.drawn[usize::from(*group)] = true;
                any = true;
            }
            if any {
                let center = draw.origin + Vec3::from(draw.aabb.center);
                admitted.push((cam_pos.distance_squared(center).to_bits(), (entity, sel)));
            }
        }
        admitted.sort_unstable_by_key(|(d, _)| *d);
        world
            .visible_wmos
            .extend(admitted.into_iter().map(|(_, v)| v));
    }
    *cull_memo = Some((*draw_epoch, generation, revisit));
    // `WOW_GX_CENSUS=1`: a selected item is one would-be entity-path submesh, so the counts read
    // against the entity path's `VIS_CENSUS`; at or above it is coarser grain, far above a leak.
    static CENSUS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *CENSUS.get_or_init(|| std::env::var_os("WOW_GX_CENSUS").is_some())
        && gx.frame.is_multiple_of(64)
    {
        let world = &gx.world;
        let mut cell_items = 0usize;
        let (mut prop_regions, mut prop_items) = (0usize, 0usize);
        for vis in &world.visible {
            match vis {
                GxDoodadVis::Cell(c) => {
                    cell_items += world.cells.get(c).map_or(0, |d| d.draws.len());
                }
                GxDoodadVis::Prop(e, sel) => {
                    prop_regions += 1;
                    if let Some(d) = world.props.get(e) {
                        prop_items += d
                            .draws
                            .iter()
                            .filter(|i| {
                                i.group.is_some_and(|s| {
                                    sel.drawn.get(usize::from(s)).copied().unwrap_or(false)
                                })
                            })
                            .count();
                    }
                }
            }
        }
        let (mut wmo_items, mut wmo_groups) = (0usize, 0usize);
        for (e, sel) in &world.visible_wmos {
            if let Some(d) = world.wmos.get(e) {
                wmo_items += d
                    .draws
                    .iter()
                    .filter(|i| {
                        i.group.is_some_and(|g| {
                            sel.drawn.get(usize::from(g)).copied().unwrap_or(false)
                        })
                    })
                    .count();
            }
            wmo_groups += sel.drawn.iter().filter(|b| **b).count();
        }
        let (mut fs, mut fx, mut fg) = (0usize, 0usize, 0usize);
        for cell in gx.cells.values() {
            for f in cell.faders.values() {
                match f.state {
                    FaderState::Steady => fs += 1,
                    FaderState::Exiled { .. } => fx += 1,
                    FaderState::Gone => fg += 1,
                }
            }
        }
        let ev = gx.fade_events;
        println!(
            "GX_CENSUS cells={}/{} cell_items={cell_items} wmo_regions={}/{} \
             wmo_groups={wmo_groups} wmo_items={wmo_items} \
             prop_regions={prop_regions}/{} prop_items={prop_items} \
             faders={fs}s/{fx}x/{fg}g fade_events={}x/{}s/{}g",
            world.visible.len() - prop_regions,
            world.cells.len(),
            world.visible_wmos.len(),
            world.wmos.len(),
            world.props.len(),
            ev[0],
            ev[1],
            ev[2],
        );
    }
    if super::gx_perf_enabled() && gx.frame.is_multiple_of(64) {
        use std::sync::atomic::Ordering::Relaxed;
        let ms: Vec<f64> = super::GX_PERF
            .iter()
            .map(|a| a.swap(0, Relaxed) as f64 / 64.0 / 1.0e6)
            .collect();
        println!(
            "GX_PERF ms/frame flush={:.3} cull={:.3} publish={:.3} prepare={:.3} node={:.3} \
             arrays_mb={}",
            ms[0],
            ms[1],
            ms[2],
            ms[3],
            ms[4],
            super::GX_VRAM.load(Relaxed) / (1024 * 1024)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No camera move shorter than a fader's slack changes what `fade_step` answers, from any
    /// class, so a cell can skip its walk until the camera has travelled the cell's minimum.
    #[test]
    fn a_move_inside_the_slack_never_changes_a_class() {
        for size in [0.5f32, 2.0, 6.0, 15.0, 40.0, 120.0] {
            let Some((near, far)) = crate::model_fade::fade_band(size) else {
                continue;
            };
            let mut d = 0.0f32;
            while d < far + 40.0 {
                let slack = class_slack(near, far, d);
                for class in [FadeClass::Steady, FadeClass::Feather, FadeClass::Gone] {
                    let here = fade_step(size, d, class);
                    for frac in [-0.999f32, -0.5, 0.5, 0.999] {
                        let moved = (d + slack.min(500.0) * frac).max(0.0);
                        assert_eq!(
                            fade_step(size, moved, class),
                            here,
                            "size {size} d {d} moved {moved} slack {slack} from {class:?}"
                        );
                    }
                }
                d += 0.37;
            }
        }
    }

    /// A still frame reuses the last full cull only over the same draw set and fade bands, and
    /// never while an exile waits to be armed.
    #[test]
    fn the_cull_memo_holds_only_over_the_same_draw_set() {
        assert!(!memo_holds(None, 0, 0), "no full run yet");
        assert!(memo_holds(Some((3, 1, false)), 3, 1));
        assert!(
            !memo_holds(Some((3, 1, false)), 4, 1),
            "a bake or a drop since"
        );
        assert!(
            !memo_holds(Some((3, 1, false)), 3, 2),
            "environmentDetail moved the bands"
        );
        assert!(!memo_holds(Some((3, 1, true)), 3, 1), "an exile to arm");
    }

    #[test]
    fn set_admission_is_any_of_named_rooms_fail_open() {
        let vis = [false, true, false];
        assert!(set_admitted(&vis, &[1]));
        assert!(set_admitted(&vis, &[0, 1]));
        assert!(!set_admitted(&vis, &[0, 2]));
        assert!(set_admitted(&vis, &[7]), "a missing bit fails open");
        assert!(set_admitted(&vis, &[]), "an unnamed prop is never culled");
    }

    /// A prop fader past its band near edge exiles as an entity on its rooms, its retained item
    /// killed once the exile has drawn a frame, and comes back when the camera returns.
    #[test]
    fn a_prop_fader_exiles_with_its_rooms_and_returns() {
        use super::super::testkit::{batch_of, object, tri};
        use super::super::{GxFadeSeed, GxPropBatch};
        use bevy::ecs::system::RunSystemOnce;
        use std::sync::Arc;
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        let instance = world.spawn_empty().id();
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let building = object(500);
        let mut b = batch_of(
            &building,
            &g,
            Vec3::ZERO,
            None,
            benilla_formats::ModelBlend::Opaque,
        );
        b.shade = crate::model_render::ShadeSel::Matte;
        b.prop = Some(GxPropBatch {
            instance,
            groups: Arc::from([3u16].as_slice()),
            slot: None,
            key: 7,
        });
        b.fade = Some(GxFadeSeed {
            size: 0.5, // fade level 0: the 25..30 yd band
            local_center: Vec3::ZERO,
            radius: 1.0,
            stat_mesh: Handle::default(),
            aabb: None,
            cutout: Handle::default(),
            blend: Handle::default(),
        });
        assert!(gx.divert(b));
        gx.flush_now = true;
        world.insert_resource(gx);
        world
            .run_system_once(super::super::bake::flush_static_gx)
            .unwrap();
        let scan = move |cam: Vec3| {
            move |mut commands: Commands, mut gx: ResMut<StaticGx>| {
                let StaticGx {
                    props,
                    world,
                    fade_events,
                    ..
                } = &mut *gx;
                scan_faders(
                    &mut commands,
                    props.get_mut(&instance).unwrap(),
                    world.props.get_mut(&instance),
                    cam,
                    fade_events,
                    &"test",
                    |_| true,
                )
            }
        };
        let killed =
            |world: &World| world.resource::<StaticGx>().world.props[&instance].killed[0] & 1;
        let band = Vec3::new(27.0, 0.0, 0.0);
        assert!(
            world.run_system_once(scan(band)).unwrap(),
            "an exile waits to arm"
        );
        let mut exiles = world.query_filtered::<&crate::wmo_portal::WmoGroupVis, With<crate::model_render::ModelPart>>();
        let rooms: Vec<Vec<u16>> = exiles.iter(&world).map(|v| v.groups.to_vec()).collect();
        assert_eq!(
            rooms,
            vec![vec![3u16]],
            "one exile, culled by the prop's rooms"
        );
        assert_eq!(
            killed(&world),
            0,
            "the retained item draws until the exile has"
        );
        assert!(!world.run_system_once(scan(band)).unwrap());
        assert_eq!(killed(&world), 1, "armed: the retained item is killed");

        assert!(!world.run_system_once(scan(Vec3::ZERO)).unwrap());
        world.flush();
        assert_eq!(
            exiles.iter(&world).count(),
            0,
            "back in: the exile despawned"
        );
        assert_eq!(killed(&world), 0, "and the retained item draws again");
    }

    #[test]
    fn the_exile_step_enters_exact_and_exits_sticky() {
        // A 0.5 yd prop is fade level 0, the 25..30 yd band at any `environmentDetail`.
        let r = 0.5;
        assert_eq!(fade_step(r, 24.0, FadeClass::Steady), FadeClass::Steady);
        assert_eq!(fade_step(r, 25.0, FadeClass::Steady), FadeClass::Steady);
        assert_eq!(fade_step(r, 25.5, FadeClass::Steady), FadeClass::Feather);
        // A teleport past the band end goes straight to Gone.
        assert_eq!(fade_step(r, 35.0, FadeClass::Steady), FadeClass::Gone);
        // Exits need the 1 yd margin: at 25.5 the probe at 26.5 still feathers.
        assert_eq!(fade_step(r, 25.5, FadeClass::Feather), FadeClass::Feather);
        assert_eq!(fade_step(r, 24.0, FadeClass::Feather), FadeClass::Steady);
        assert_eq!(fade_step(r, 29.5, FadeClass::Feather), FadeClass::Feather);
        assert_eq!(fade_step(r, 31.0, FadeClass::Feather), FadeClass::Gone);
        // Gone feathers again as soon as alpha is nonzero, and needs the margin for Steady.
        assert_eq!(fade_step(r, 29.9, FadeClass::Gone), FadeClass::Feather);
        assert_eq!(fade_step(r, 30.0, FadeClass::Gone), FadeClass::Gone);
        assert_eq!(fade_step(r, 23.9, FadeClass::Gone), FadeClass::Steady);
    }
}
