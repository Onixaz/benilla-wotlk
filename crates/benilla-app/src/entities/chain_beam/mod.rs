//! Spell chain beams: one bolt per hop, drawn by [`lightning`], spawned and placed per `0x7fc5f0`.

pub(crate) mod lightning;

use benilla_formats::{ChainEffect, EffectAttach, VisualStages, MAX_COMBO_ROWS};
use benilla_protocol::EntityKind;
use benilla_world::sky_order::Rung;
use benilla_world::view::WorldCamera;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::creature_anim::spell_visual::{AreaKitWait, DelayedAreaKit};
use crate::creature_anim::{
    ChainProcPlay, KitEffectClear, SpellEffectClear, SpellGoTargets, SpellVisuals,
};
use crate::net::{Guid, GuidIndex, NetEntity};
use lightning::{Lightning, Scratch};

use super::OverheadFallback;

/// The share of the model height the caster end and a bare hop end sit at (`0x7faa87`).
const HEIGHT_FACTOR: f32 = 0.75;

/// The attachment a hop end takes when the spell has no row (`0x7fac91`).
const NO_SPELL_ATTACH: u16 = 34;

/// The beams' render layers, drawn in order; a row outside them never draws (`0x9ab070`).
const RENDER_LAYERS: std::ops::RangeInclusive<i32> = 0..=3;

/// The caster's hop array in wire order (`0x724f50`), consumed by the next chain proc (`0x726b0d`).
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct ChainHops(pub(crate) Vec<Entity>);

/// Where a beam's ends sit, from the spell's visual row in model space (`0x7fc5f0`, `0x7fabf0`).
#[derive(Clone, Copy, Default)]
struct Ends {
    /// Column 16, the caster end's attachment.
    source_attach: Option<EffectAttach>,
    /// Columns 26-28, the caster end's offset.
    cast_offset: Vec3,
    /// Column 10, every hop end's attachment.
    dest_attach: Option<EffectAttach>,
    /// Columns 29-31, every hop end's offset.
    impact_offset: Vec3,
    /// The spell has a row; without one a hop end takes attachment 34 (`0x7fac79`).
    known_spell: bool,
}

impl Ends {
    fn from_visual(stages: Option<&VisualStages>, known_spell: bool) -> Self {
        let Some(s) = stages else {
            return Self {
                known_spell,
                ..Default::default()
            };
        };
        Self {
            source_attach: s.missile_source_attachment(),
            cast_offset: benilla_assets::coords::wow_to_bevy(s.cast_offset()),
            dest_attach: s.beam_impact_attachment(),
            impact_offset: benilla_assets::coords::wow_to_bevy(s.impact_offset()),
            known_spell,
        }
    }
}

/// What [`spawn_chain_beams`] keeps between plays.
#[derive(Default)]
pub(crate) struct SpawnState {
    /// The hop array's in-place zeroing within one frame (`0x726b0d`).
    consumed: bevy::ecs::entity::EntityHashSet,
    plays_seen: u32,
}

/// Each caster's saved ground point, cleared by its next beam row's GO (`0x715400`).
#[derive(Resource, Default)]
pub(crate) struct ChainLocations(bevy::ecs::entity::EntityHashMap<Vec3>);

impl ChainLocations {
    /// A beam row's GO saves its dest, unless flag 0x1 keeps it on the units it hit (`0x80ed7d`).
    fn save(&mut self, caster: Entity, row_flags: u32, hit_units: bool, dest: Option<Vec3>) {
        match dest.filter(|_| row_flags & 0x1 == 0 || !hit_units) {
            Some(dest) => self.0.insert(caster, dest),
            None => self.0.remove(&caster),
        };
    }
}

/// One hop, the reference's 20-byte bolt: the node indices it runs between and its window.
struct Bolt {
    /// 0, the caster, for every hop of a reversed beam, else the hop before (`0x7fa547`).
    source: usize,
    target: usize,
    /// `t0 + delay + i × stagger` in scene seconds (`0x7fa4e2`, `0x7fa569`).
    start: f32,
    /// `start + boltLife` (`0x7fa53f`).
    end: f32,
    /// The renderer's bolt, made on the first tick inside the window and dropped at its end.
    live: Option<Lightning>,
}

/// A live chain beam, the reference's lightning object: one row over one hop list.
#[derive(Component)]
pub(crate) struct ChainBeam {
    /// The reap key.
    spell_id: u32,
    /// The kit of the effect holding it, a kit clear's key (`0x744bd0`).
    kit_id: u32,
    /// The kit's owner, whose effect list holds the beam (`0x6f8040`); its loss ends the beam.
    caster: Entity,
    /// `[source, hop0, hop1, …]` as guids resolved every frame, a gone one hiding its bolts until
    /// it streams back (`0x7faeef`); a beam never re-targets.
    nodes: Vec<u64>,
    /// The ground point of a beam cast at no unit, its one bolt's target (`0x7fc878`).
    location: Option<Vec3>,
    bolts: Vec<Bolt>,
    effect: ChainEffect,
    texture: Handle<Image>,
    ends: Ends,
    /// The decoded `CharParamTwo`: a channel beam, which never expires by time.
    persistent: bool,
    /// The last hop's end, the latest any hop burns to (`0x7fa569`); ignored while persistent.
    expiry: f32,
    seed: u32,
}

/// The row's texture as an `mpq://` `.blp` URL; one no archive holds never draws (`0x9aade9`).
fn beam_texture_url(raw: &str) -> String {
    format!(
        "mpq://{}.blp",
        benilla_assets::sprite_key(raw).replace('\\', "/")
    )
}

/// The live channel's object while the hop array holds at most one, else the hops (`0x726a56`).
fn select_targets(
    spell_id: u32,
    hops: Option<&ChainHops>,
    store: &crate::net::ObjectStore,
    index: &GuidIndex,
) -> Vec<Entity> {
    let hops = hops.map(|h| h.0.as_slice()).unwrap_or_default();
    let channelling = spell_id != 0 && store.0.unit_channel_spell() == spell_id;
    if channelling && hops.len() <= 1 {
        if let Some(object) = store.0.unit_channel_object().filter(|g| *g != 0) {
            // An unstreamed channel object leaves nothing to run to, like a hidden hop.
            return index.0.get(&object).copied().into_iter().collect();
        }
    }
    hops.to_vec()
}

/// The dispatcher's beam case (`0x726a8d`): reaps persistent beams, then spawns one beam per row.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_chain_beams(
    mut commands: Commands,
    time: Res<Time>,
    mut plays: MessageReader<ChainProcPlay>,
    mut clears: MessageReader<SpellEffectClear>,
    mut kit_clears: MessageReader<KitEffectClear>,
    mut go_targets: MessageReader<SpellGoTargets>,
    mut area_kits: MessageWriter<DelayedAreaKit>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    asset_server: Res<AssetServer>,
    index: Res<GuidIndex>,
    stores: Query<(&crate::net::ObjectStore, Option<&ChainHops>, Option<&Guid>)>,
    beams: Query<(Entity, &ChainBeam)>,
    transforms: Query<&GlobalTransform, Without<WorldCamera>>,
    mut locations: ResMut<ChainLocations>,
    mut state: Local<SpawnState>,
) {
    let SpawnState {
        consumed,
        plays_seen,
    } = &mut *state;
    consumed.clear();
    let visual_of = |spell_id: u32| {
        let display = spells.as_deref()?.catalog.get(spell_id)?;
        let id = crate::creature_anim::spell_visual::spell_visual_id(display);
        visuals.as_deref()?.0.stages(id)
    };
    for go in go_targets.read() {
        let Some(row) = visual_of(go.spell_id) else {
            continue;
        };
        if !visuals
            .as_deref()
            .is_some_and(|v| v.0.has_chain_effect(row))
        {
            continue;
        }
        let hit_units = !(go.hits.is_empty() && go.misses.is_empty());
        locations.save(go.caster, row.flags, hit_units, go.dest);
    }
    locations.0.retain(|e, _| stores.contains(*e));
    // The spell clear releases the owner's effects and the persistent beams they hold; a
    // self-terminating beam keeps its own clock (`0x743b40`, `0x6f74f0`).
    let cleared: Vec<SpellEffectClear> = clears.read().copied().collect();
    let kits_cleared: Vec<KitEffectClear> = kit_clears.read().copied().collect();
    if !cleared.is_empty() || !kits_cleared.is_empty() {
        for (e, beam) in &beams {
            let spell = || {
                cleared
                    .iter()
                    .any(|c| c.entity == beam.caster && c.spell_id == beam.spell_id)
            };
            let kit = || {
                kits_cleared
                    .iter()
                    .any(|c| c.entity == beam.caster && c.kit_id == beam.kit_id)
            };
            if beam.persistent && (spell() || kit()) {
                commands.entity(e).despawn();
            }
        }
    }

    let now = time.elapsed_secs();
    for play in plays.read() {
        // The caster may be gone within the same wire drain that produced this play.
        let Ok((store, hops, _)) = stores.get(play.entity) else {
            continue;
        };
        // Another unit's play runs from it to this one, or ends here unconsumed (`0x7269d8`).
        let source = play.from.filter(|&from| from != play.entity);
        if source.is_some_and(|s| !stores.get(s).is_ok_and(|(o, ..)| o.is_unit())) {
            continue;
        }
        // Consume once on every other path: zeroed even when nothing is drawn (`0x726b0d`).
        let hops = hops.filter(|_| !consumed.contains(&play.entity));
        consumed.insert(play.entity);
        commands.entity(play.entity).try_remove::<ChainHops>();
        // A saved location overrides the hops (`0x726a3b`); no spell or target is a silent return.
        let location = locations.0.get(&play.entity).copied();
        let targets = match (source, location) {
            (Some(_), _) => vec![play.entity],
            (None, Some(_)) => Vec::new(),
            (None, None) => select_targets(play.spell_id, hops, store, &index),
        };
        if play.spell_id == 0 || (targets.is_empty() && location.is_none()) {
            continue;
        }
        let from = source.unwrap_or(play.entity);
        let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
            continue;
        };
        let Some(root) = visuals.0.chain_effect(play.proc.effect_id) else {
            continue; // an id naming no row is the client's own no-op
        };
        let stages = visual_of(play.spell_id);
        let ends = Ends::from_visual(stages, spells.catalog.get(play.spell_id).is_some());
        // Each play adds its own effect beside the spell's earlier ones (`0x7269ca`).
        *plays_seen = plays_seen.wrapping_add(1);
        let guid_of = |e: Entity| stores.get(e).ok().and_then(|(.., g)| g).map_or(0, |g| g.0);
        let nodes: Vec<u64> = std::iter::once(from).chain(targets).map(guid_of).collect();
        // A location aims the one bolt when no unit is hit, turned with the caster (`0x7fc878`).
        let turn = transforms
            .get(play.entity)
            .map_or(Quat::IDENTITY, |t| t.to_scale_rotation_translation().1);
        let ground = location
            .filter(|_| source.is_none())
            .map(|at| at + turn * ends.impact_offset);
        let hop_count = if ground.is_some() { 1 } else { nodes.len() - 1 };
        // The root, then its combo walk, each drawing the same hops (`0x7fc8ea`).
        let rows = std::iter::once(play.proc.effect_id)
            .chain(root.combo.iter().copied())
            .take(MAX_COMBO_ROWS);
        for (r, row) in rows.enumerate() {
            let Some(effect) = visuals.0.chain_effect(row) else {
                break;
            };
            let delay = effect.first_bolt_delay_ms as f32 / 1000.0;
            let stagger = effect.bolt_stagger_ms as f32 / 1000.0;
            let life = effect.bolt_life_ms as f32 / 1000.0;
            let bolts: Vec<Bolt> = (0..hop_count)
                .map(|i| {
                    let start = now + delay + i as f32 * stagger;
                    Bolt {
                        source: if play.proc.reverse { 0 } else { i },
                        target: i + 1,
                        start,
                        end: start + life,
                        live: None,
                    }
                })
                .collect();
            let expiry = bolts.iter().map(|b| b.end).fold(now + delay, f32::max);
            commands.spawn((
                Transform::IDENTITY,
                ChainBeam {
                    spell_id: play.spell_id,
                    kit_id: play.kit_id,
                    caster: play.entity,
                    nodes: nodes.clone(),
                    location: ground,
                    bolts,
                    texture: asset_server.load::<Image>(beam_texture_url(&effect.texture)),
                    effect: effect.clone(),
                    ends,
                    persistent: play.proc.flag,
                    expiry,
                    // Per-beam seed, so neighbouring beams never draw the same joints.
                    seed: 0x9E37_79B9
                        ^ play.spell_id.wrapping_mul(2_654_435_761)
                        ^ plays_seen.wrapping_mul(40_503)
                        ^ (r as u32).wrapping_mul(0x85EB_CA6B),
                },
            ));
        }
        // A location beam plays the area kit the GO skipped, unless this is that kit (`0x726b2c`).
        let area_kit = stages.map_or(0, |s| s.impact_area_kit);
        if let Some(pos) =
            location.filter(|_| area_kit != play.kit_id && visuals.0.kit(area_kit).is_some())
        {
            area_kits.write(DelayedAreaKit {
                kit_id: area_kit,
                pos,
                wait: AreaKitWait::Secs(0.0),
            });
        }
    }
}

/// The world points a beam's ends resolve to.
#[derive(SystemParam)]
pub(crate) struct EndPoints<'w, 's> {
    units: super::missile::AttachPosQuery<'w, 's>,
    joints: Query<'w, 's, &'static GlobalTransform>,
    heights: Query<'w, 's, &'static OverheadFallback>,
    kinds: Query<'w, 's, &'static NetEntity>,
    weapons: super::equipment::HeldWeapons<'w, 's>,
}

impl EndPoints<'_, '_> {
    /// A posed attachment or marker of `e` with `extra` added in its frame.
    fn posed(&self, e: Entity, at: Option<(u16, Vec3)>, extra: Vec3) -> Option<Vec3> {
        let (_, _, pose) = self.units.get(e).ok()?;
        let pose = pose?;
        let (bone, offset) = at?;
        pose.posed_point(
            self.joints.get(pose.joints_root).ok()?,
            bone,
            offset + extra,
        )
    }

    /// The unit's base, its rotation, and its height share; `None` when it is gone.
    fn base(&self, e: Entity) -> Option<(Vec3, Quat, Vec3, bool)> {
        let (gt, bones, pose) = self.units.get(e).ok()?;
        let (scale, rot, base) = gt.to_scale_rotation_translation();
        // The model height times the scale stands in for both `obj+0xac` and the stand height.
        let height = self.heights.get(e).map_or(0.0, |h| h.0);
        let lift = Vec3::Y * (scale.y * height * HEIGHT_FACTOR);
        Some((base, rot, lift, bones.is_some() && pose.is_some()))
    }

    /// Whether `e` is a unit; another object's end is its offset in its frame (`0x7fb008`).
    fn is_unit(&self, e: Entity) -> bool {
        self.kinds.get(e).map_or(true, |n| {
            matches!(n.kind, EntityKind::Player | EntityKind::Unit)
        })
    }

    /// The caster end: column 16's attachment, else `$CSL`, else 3/4 up the model (`0x7faa40`).
    fn caster(&self, e: Entity, ends: &Ends) -> Option<Vec3> {
        let (base, rot, lift, loaded) = self.base(e)?;
        if !loaded {
            return Some(base + lift);
        }
        let (_, bones, _) = self.units.get(e).ok()?;
        let bones = bones?;
        let attach = ends
            .source_attach
            .and_then(|t| self.weapons.effect_point(e, bones, t));
        if let Some(p) = self.posed(e, attach, ends.cast_offset) {
            return Some(p);
        }
        let marker = bones.markers.get(b"$CSL").copied();
        if let Some(p) = self.posed(e, marker, ends.cast_offset) {
            return Some(p);
        }
        Some(base + lift + rot * ends.cast_offset)
    }

    /// A hop end: column 10's attachment, else 3/4 up the model, plus the offset (`0x7fabf0`).
    fn impact(&self, e: Entity, ends: &Ends) -> Option<Vec3> {
        let (base, rot, lift, loaded) = self.base(e)?;
        let bones = self.units.get(e).ok()?.1.filter(|_| loaded);
        if !ends.known_spell {
            let attach = bones.and_then(|b| b.points.get(&NO_SPELL_ATTACH).copied());
            return Some(self.posed(e, attach, Vec3::ZERO).unwrap_or(base + lift));
        }
        let attach = bones.zip(ends.dest_attach);
        let attach = attach.and_then(|(b, t)| self.weapons.effect_point(e, b, t));
        if let Some(p) = self.posed(e, attach, ends.impact_offset) {
            return Some(p);
        }
        Some(base + lift + rot * ends.impact_offset)
    }

    /// A bolt's source: the caster end for node 0 (`0x7fafc8`), a hop end past it (`0x7faf9c`).
    fn source(&self, node: usize, e: Entity, ends: &Ends) -> Option<Vec3> {
        if !self.is_unit(e) {
            let (base, rot, _, _) = self.base(e)?;
            return Some(base + rot * ends.cast_offset);
        }
        if node == 0 {
            self.caster(e, ends)
        } else {
            self.impact(e, ends)
        }
    }

    /// A bolt's target end (`0x7fb045`).
    fn target(&self, e: Entity, ends: &Ends) -> Option<Vec3> {
        if !self.is_unit(e) {
            let (base, rot, _, _) = self.base(e)?;
            return Some(base + rot * ends.impact_offset);
        }
        self.impact(e, ends)
    }
}

/// Per frame, the object tick (`0x7fae90`), bolt update (`0x9ab3b0`) and draw (`0x9aa210`).
// Not built: the endpoint hand-off that shows a target's effects when a pulse arrives (`0x7fb173`).
pub(crate) fn simulate_chain_beams(
    time: Res<Time>,
    mut commands: Commands,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
    images: Res<Assets<Image>>,
    world_cam: Query<(Entity, &GlobalTransform), With<WorldCamera>>,
    ends: EndPoints,
    index: Res<GuidIndex>,
    mut beams: Query<(Entity, &mut ChainBeam)>,
    mut scratch: Local<Scratch>,
) {
    let Ok((cam, cam_xf)) = world_cam.single() else {
        return;
    };
    let eye = cam_xf.translation();
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    for (entity, mut beam) in &mut beams {
        // A one-shot ends with its last hop (`0x7fb205`); any beam ends with its caster.
        if (!beam.persistent && now >= beam.expiry) || !ends.units.contains(beam.caster) {
            commands.entity(entity).despawn();
            continue;
        }
        let ChainBeam {
            nodes,
            location,
            bolts,
            effect,
            texture,
            ends: spec,
            persistent,
            seed,
            ..
        } = &mut *beam;
        let drawn = RENDER_LAYERS.contains(&effect.render_layer) && images.contains(&*texture);
        let mut batch = drawn.then(|| draw.batch(cam, texture.id()));
        let (mut anchor_sum, mut anchor_n) = (Vec3::ZERO, 0.0f32);
        for (i, bolt) in bolts.iter_mut().enumerate() {
            if *persistent || (bolt.start <= now && now < bolt.end) {
                let resolve = |guid: u64| index.0.get(&guid).copied();
                // An end that does not resolve hides the bolt where it stands (`0x7fb13b`).
                let src =
                    resolve(nodes[bolt.source]).and_then(|n| ends.source(bolt.source, n, spec));
                let dst = nodes.get(bolt.target).map_or(*location, |&g| {
                    resolve(g).and_then(|n| ends.target(n, spec))
                });
                let live = bolt.live.get_or_insert_with(|| {
                    let s = (*seed).wrapping_add((i as u32).wrapping_mul(0x27D4_EB2F));
                    Lightning::new(effect, src.unwrap_or_default(), dst.unwrap_or_default(), s)
                });
                if let Some(src) = src {
                    live.src = src;
                }
                if let Some(dst) = dst {
                    live.dst = dst;
                }
                live.visible = src.is_some() && dst.is_some();
            }
            if !*persistent && now >= bolt.end {
                bolt.live = None;
            }
            let Some(live) = bolt.live.as_mut() else {
                continue;
            };
            live.update(effect, dt);
            if let Some(batch) = batch.as_mut() {
                if live.visible {
                    anchor_sum += (live.src + live.dst) * 0.5;
                    anchor_n += 1.0;
                }
                live.render(effect, eye, &mut scratch, batch.verts_mut());
            }
        }
        let Some(batch) = batch else {
            continue;
        };
        let layer = effect.render_layer as f32 * Rung::CHAIN_BEAM_LAYER;
        // Fog and culling off (`0x9ab17a`, `0x9ab149`): the lane's defaults.
        batch
            .gx_blend(effect.blend_mode)
            .rung(Rung::CHAIN_BEAM + layer, 0)
            .anchored(anchor_sum / anchor_n.max(1.0))
            .owner(entity)
            .quads();
    }
}

#[cfg(test)]
mod tests;
