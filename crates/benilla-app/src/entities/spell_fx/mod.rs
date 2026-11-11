//! Spell-visual effect models: the attach-point `.mdx` a kit hangs on a unit, and the
//! lootable-corpse sparkle, which the client hangs on the same effect node type.
//!
//! Each instance root is parented once under the unit's attach joint and rides the bone through the
//! transform tree, as the reference parents the effect model to the host once and its transform
//! refresh carries it. Every instance takes the missile's attach cascade (`ATTACH_FALLBACKS`); the
//! reference's kit effect has none, since its effect add tests no attachment, so on a model lacking
//! the tag it draws nothing and never retries.
//!
//! A persistent instance lives until its spell's [`SpellKitFx::Reap`]; a self-terminating one
//! despawns after one pass of its model's sequence 0, a span clock standing in for the client's
//! completion callback.

mod lifecycle;

use std::collections::HashMap;

use benilla_assets::bone_target_id;
use bevy::animation::AnimatedBy;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::spell_visual::FxSlot;
use crate::creature_anim::{scan_events, AnimSoundEvent, FxClass, FxStage, SpellKitFx};
use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_render::{ModelKind, ModelPart};
use benilla_world::particles;
use benilla_world::vis_chain::VisChainOnly;

use super::{BoneAttach, DisplayModel, EntityPart, ModelHandle};
pub(super) use lifecycle::advance_fx_anim;
use lifecycle::{decay_span, FxAnimLife, FxDecay};

/// The missile's attach cascade for a model lacking the requested point: `0xf`, then `0x13`, then
/// the unit root.
const ATTACH_FALLBACKS: [u16; 2] = [0xf, 0x13];

/// The self-termination span of a model with no sequence table; [`super::dest_fx`] shares it.
pub(crate) const FALLBACK_SPAN: f32 = 1.0;

/// How long a self-terminating instance may wait for a model that never loads.
const PENDING_TIMEOUT: f32 = 10.0;

/// The effect-model cache by path; `super::update_display_models` builds each entry's parts.
#[derive(Resource, Default)]
pub(crate) struct SpellFx {
    pub(crate) models: HashMap<String, DisplayModel>,
}

/// Get-or-insert this path's cache entry and start its load: the only way to reach
/// [`SpellFx::models`], since a map change clears it and a standing aura never re-sends `Begin`.
pub(super) fn ensure_model(fx: &mut SpellFx, asset_server: &AssetServer, path: &str) {
    fx.models
        .entry(path.to_string())
        .or_insert_with(|| DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(path))),
            ..super::empty_shell()
        });
}

/// Per-instance tint clones, material id → `(RGB loop, attach time)`: one cast is one phase, so an
/// animated M2Color cannot share the doodad lane's global clock. An entry drops with its material.
#[derive(Resource, Default)]
pub(crate) struct FxTintAnims(
    HashMap<
        bevy::asset::AssetId<WowModelMaterial>,
        (std::sync::Arc<benilla_formats::RgbAnim>, f32),
    >,
);

/// Re-sample every live clone's tint on its instance clock. Not capture-gated: `fxview` ages an
/// effect inside a capture, and no golden scenario spawns one.
pub(crate) fn tick_fx_tint(
    time: Res<Time>,
    mut reg: ResMut<FxTintAnims>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
) {
    if reg.0.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    reg.0.retain(|id, (anim, origin)| {
        let Some(mat) = materials.get_mut(*id) else {
            return false; // the instance despawned, and its clone with it
        };
        let rgb = anim.sample(now - *origin);
        mat.extension.tint = Vec4::new(rgb[0], rgb[1], rgb[2], 1.0);
        true
    });
}

/// The material store and the per-instance animation registries a part's clone joins.
pub(crate) struct FxMaterials<'a> {
    pub(crate) store: &'a mut Assets<WowModelMaterial>,
    pub(crate) tint: &'a mut FxTintAnims,
    pub(crate) uv: &'a mut benilla_world::doodad_anim::UvAnimMaterials,
    pub(crate) table: &'a mut benilla_world::mat_anim_table::MatAnimTable,
}

/// One part's material for a new instance: the shared handle, or a per-instance clone when its RGB
/// or texture transform animates. The UV lane reads `host`'s player.
fn fx_part_material(
    part: &EntityPart,
    now: f32,
    host: Entity,
    mats: &mut FxMaterials,
) -> Handle<WowModelMaterial> {
    let loops = part.uv_loops();
    if part.rgb_anim.is_none() && !loops.any() {
        return part.material.clone();
    }
    // The shared material may still be parked (`model_render::lazy`).
    benilla_world::model_render::lazy::realize(mats.store, part.material.id());
    let Some(mut mat) = mats.store.get(part.material.id()).cloned() else {
        return part.material.clone(); // not built yet, though the parts were checked ready
    };
    if let Some(anim) = &part.rgb_anim {
        let t0 = anim.sample(0.0);
        mat.extension.tint = Vec4::new(t0[0], t0[1], t0[2], 1.0);
    }
    // Off the shared mat-anim table, or its delta would add to the clone's own rows.
    mat.extension.anim_slots = Vec4::ZERO;
    if loops.any() {
        // Seed at t = 0: the delta table measures from `sun_scale.zw` at registration.
        let t0 = loops.open_offset();
        mat.extension.sun_scale.z = t0[0];
        mat.extension.sun_scale.w = t0[1];
    }
    let handle = mats.store.add(mat);
    if let Some(anim) = &part.rgb_anim {
        mats.tint.0.insert(handle.id(), (anim.clone(), now));
    }
    if loops.any() {
        benilla_world::doodad_anim::register_fx_uv(
            mats.uv,
            mats.table,
            mats.store,
            handle.id(),
            loops,
            host,
            now,
        );
    }
    handle
}

/// Where an effect-model instance sits in the client's model graph.
#[derive(Clone, Copy, Default)]
pub(crate) struct EffectHost {
    /// The model this instance is chained to ([`benilla_world::model_fade::ParentModel`]), if any.
    pub parent: Option<Entity>,
}

/// Attach one effect model's visuals to `root`, for every effect consumer. Part meshes go unculled,
/// since a bind-pose bound is wrong for a billboarded subtree; a `ground_anchor` instance's flat
/// ground quads become projected decals ([`crate::ground_fx`]). Returns `false` while the model's
/// parts still build.
pub(crate) fn attach_effect_visuals(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    now: f32,
    ground_anchor: bool,
    host: EffectHost,
    stage: Option<FxStage>,
    mats: &mut FxMaterials,
    ibps: &Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    preferred_anim: Option<u16>,
) -> bool {
    let Some(parts) = dm.parts.as_ref() else {
        return false; // still loading
    };
    // Chained onto its host model, its alpha composes through the parent's, as the reference does.
    if let Some(parent) = host.parent {
        commands
            .entity(root)
            .insert(benilla_world::model_fade::ParentModel(parent));
    }
    // Freed with the host model (the reference's destructor); a free-standing missile drains.
    let on_owner_loss = if host.parent.is_some() {
        benilla_world::particles::OwnerLoss::Free
    } else {
        benilla_world::particles::OwnerLoss::Drain
    };
    let is_ground_decal = |part: &EntityPart| ground_anchor && part.ground_quad.is_some();
    let part_materials: Vec<Handle<WowModelMaterial>> = parts
        .iter()
        .map(|p| {
            if is_ground_decal(p) {
                p.material.clone()
            } else {
                fx_part_material(p, now, root, mats)
            }
        })
        .collect();
    let (joints, armed) = arm_effect_rig(commands, root, dm, preferred_anim, stage);
    // The bodies' reserve is not an effect's to take: past it the effect draws its static mesh.
    let rig_slot = match (&dm.inverse_bindposes, joints.is_empty()) {
        (Some(ibp), false) if palettes.room_beyond_bodies() => {
            benilla_world::rig_palette::RigSkin::allocate(palettes, joints.clone(), ibp.clone())
                .map_or(0, |rig| {
                    let slot = rig.slot;
                    commands.entity(root).insert(rig);
                    slot
                })
        }
        _ => 0,
    };
    let rigged = rig_slot != 0;
    // The opening clip's file sequence slot keys each batch's per-sequence alpha loops.
    let played = dm
        .animations
        .as_ref()
        .and_then(|a| a.preferred_clip(preferred_anim));
    let played_seq = played.map(|c| c.seq_index);
    // A kit effect's riders follow its player's live sequence; other lanes keep the opening one.
    let seq_host = (armed && stage.is_some()).then_some(root);
    commands.entity(root).with_children(|children| {
        for (part, material) in parts.iter().zip(&part_materials) {
            if part.billboard.is_some() {
                continue; // spawned as a following card below
            }
            if is_ground_decal(part) {
                continue; // spawned as a projected surface decal below
            }
            let mesh = match (rigged, &part.skinned_mesh) {
                (true, Some(sm)) => sm.clone(),
                _ => part.mesh.clone(),
            };
            let mut child = children.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                bevy::camera::visibility::NoFrustumCulling,
                ModelPart {
                    kind: ModelKind::Creature,
                    blend: part.blend,
                },
                // The picker's triangles; the render meshes are `RENDER_WORLD`-only.
                benilla_world::interact::PickMesh(part.geometry.clone()),
            ));
            // The model's batches rank inside one transparent group.
            if let Some(sort) =
                benilla_world::model_render::M2BatchSort::of(&part.geometry, root.to_bits())
            {
                child.insert(sort);
            }
            if let (true, Some(_)) = (rigged, &part.skinned_mesh) {
                child.insert((
                    benilla_world::rig_palette::RigPart(root),
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(rig_slot, 1.0)),
                ));
            }
            // The alpha loops on this instance's clock; the sampler is the tag's only writer.
            if let Some(anim) = &part.alpha_anim {
                let mat_anim =
                    benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                        .following_host(seq_host);
                let tag_slot = if rigged && part.skinned_mesh.is_some() {
                    rig_slot
                } else {
                    0
                };
                child.insert((
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                        tag_slot,
                        mat_anim.current,
                    )),
                    mat_anim,
                ));
            }
        }
    });
    for (part, material) in parts.iter().zip(&part_materials) {
        let Some(info) = &part.billboard else {
            continue;
        };
        let card = match joints.get(info.bone as usize) {
            Some(&j) => benilla_world::billboard::BillboardCard::following_joint(info, j),
            None => benilla_world::billboard::BillboardCard::following(info, root),
        };
        let mut spawned = commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            ModelPart {
                kind: ModelKind::Creature,
                blend: part.blend,
            },
            // The picker's triangles, pivot-centred by the caster like the bake.
            benilla_world::interact::PickMesh(part.geometry.clone()),
            card,
        ));
        // The model's batches rank inside one transparent group.
        if let Some(sort) = benilla_world::model_render::M2BatchSort::card(
            &part.geometry,
            root.to_bits(),
            benilla_world::model_render::SortOrigin::Entity(root),
        ) {
            spawned.insert(sort);
        }
        // The card's build-time bound: `calculate_bounds` cannot read the `RENDER_WORLD`-only mesh.
        if let Some(aabb) = part.aabb {
            spawned.insert(aabb);
        }
        // A card shares its batch's material-alpha loops (the billboard split copies them).
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            spawned.insert((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(0, mat_anim.current)),
                mat_anim,
            ));
        }
    }
    // Ground quads as projected decals on their joints: texture, blend, fog policy and RGB loop
    // ride the record. No UV scroll: no effect model with a texture transform has a ground quad
    // (`benilla-extract fxuvscan`).
    let binds = dm.inverse_bindposes.as_ref().and_then(|h| ibps.get(h));
    for part in parts.iter() {
        let Some(quad) = part.ground_quad.filter(|_| is_ground_decal(part)) else {
            continue;
        };
        let (joint, ibp) = match joints.get(quad.bone as usize) {
            Some(&j) => (
                j,
                binds
                    .and_then(|b| b.get(quad.bone as usize))
                    .copied()
                    .unwrap_or(Mat4::IDENTITY),
            ),
            None => (root, Mat4::IDENTITY), // boneless model: the quad rides the instance root
        };
        // The material's packed fog bits, raw; Scene (7) with no material, the packer's default.
        benilla_world::model_render::lazy::realize(mats.store, part.material.id());
        let (texture, fog_bits) = match mats.store.get(part.material.id()) {
            Some(mat) => (
                mat.base.base_color_texture.clone(),
                (mat.extension.clutter_fade.z as u32 >> 4) & 7,
            ),
            None => (None, u32::from(benilla_formats::FogPolicy::Scene as u8)),
        };
        let Some(texture) = texture else {
            continue; // no texture, nothing to draw
        };
        let decal = benilla_world::ground_fx::spawn_ground_fx_decal(
            commands,
            texture,
            part.blend,
            part.additive,
            fog_bits,
            part.rgb_anim.as_ref().map(|a| (a.clone(), now)),
            &quad,
            joint,
            ibp,
        );
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            commands.entity(decal).insert(mat_anim);
        }
    }
    for em in &dm.emitters {
        let owner = joints
            .get(em.def.bone as usize)
            .map_or((root, [0.0; 3]), |&j| (j, em.bone_pivot));
        particles::spawn_emitter(
            commands,
            em,
            Transform::IDENTITY,
            particles::EmitterFrames {
                owner: Some(owner),
                // The cloud sorts at the instance root; the bone only places births.
                anchor: Some(root),
                on_owner_loss,
                // The particles' model is this instance, whose chain carries the host's fade.
                alpha: Some(root),
                // Every `Spells\` emitter sets the unlit bit; a lit one would take the scene light.
                light_node: None,
            },
            // Rate and enable windows follow the current sequence, as the animate kernel samples
            // it; global sequences run on the instance's own age.
            match seq_host {
                Some(h) => particles::EmitClock::Host(h),
                None => particles::EmitClock::Effect(played_seq),
            },
        );
    }
    for rb in &dm.ribbons {
        let (owner, use_pivot) = joints
            .get(rb.def.bone as usize)
            .map_or((root, false), |&j| (j, true));
        benilla_world::ribbons::spawn_ribbon(
            commands,
            rb,
            owner,
            use_pivot,
            // Unscaled, like the emitters' identity placement, so both land on one draw-order rung.
            1.0,
            // The visibility gate reads the root's live sequence, so it follows the lifecycle.
            benilla_world::ribbons::RibbonSeq::Host(root),
            // This instance's model alpha, chained to its host.
            Some(root),
            // No fade sphere: an effect instance is not a placed model.
            None,
        );
    }
    true
}

/// Arm an effect instance's rig under `root`, returning the joints and whether a player was armed.
/// The clip plays even with constant keys, unlike the doodad gate (`ModelAnimations::first_seq`),
/// because emitters read the posed joints.
pub(super) fn arm_effect_rig(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    preferred_anim: Option<u16>,
    stage: Option<FxStage>,
) -> (Vec<Entity>, bool) {
    if dm.skeleton.joints.is_empty() {
        return (Vec::new(), false);
    }
    let joints = benilla_world::rig_palette::spawn_joints(commands, root, root, &dm.skeleton);
    // Billboard bones face the camera at the palette level, their children inheriting.
    if let Some(bb) = benilla_world::billboard::BillboardJointRig::new(&dm.skeleton, &joints, root)
    {
        commands.entity(root).insert(bb);
    }
    let mut armed = false;
    if let Some(anims) = dm.animations.as_ref() {
        // The caller's sequence (a thrown weapon's InFlight), else the bootstrap's `Stand`.
        if let Some(clip) = anims.preferred_clip(preferred_anim) {
            let mut player = AnimationPlayer::default();
            // The stage owns the repeat policy (the re-arm stages repeat a clamping precast).
            let life = FxAnimLife::arm(&mut player, clip, stage.unwrap_or(FxStage::OneShot));
            commands
                .entity(root)
                .insert((player, AnimationGraphHandle(anims.graph.clone())));
            // The lifecycle watcher, on a kit effect only.
            if stage.is_some() {
                commands.entity(root).insert((life, anims.clone()));
            }
            for (i, &j) in joints.iter().enumerate() {
                commands
                    .entity(j)
                    .insert((bone_target_id(i as u16), AnimatedBy(root)));
            }
            armed = true;
        }
        if let Some(drive) =
            benilla_world::rig_anim::GlobalSeqDrive::new(&anims.global_bones, &joints)
        {
            // Fresh per play: the reference makes a model per cast, so global sequences open at 0.
            commands.entity(root).insert(drive);
        }
    }
    (joints, armed)
}

/// One live (or pending) effect-model instance on a unit.
struct FxInstance {
    /// The reap key (the effect node's `+0x18`); the lootable-corpse sparkle uses `u32::MAX`.
    spell_id: u32,
    /// Reaped by spell id (precast, channel) rather than self-terminating (cast release).
    persistent: bool,
    /// Which reap can end a persistent instance, the client's discriminator beside the spell id.
    class: FxClass,
    /// The lifecycle, apart from `class`: one class covers a stage-4 precast and a stage-2 channel.
    stage: FxStage,
    /// The M2 attachment id to hang from ([`benilla_formats::KIT_SLOT_TAGS`]), or
    /// [`benilla_formats::WORLD_EFFECT_TAG`] for the field-12 world-plant slot.
    tag: u16,
    /// The `SpellVisualEffectName` id; with `tag`, the same-slot replace key.
    effect: u32,
    path: String,
    /// The root's uniform local scale ([`FxSlot::scale`]).
    scale: f32,
    /// The `SpellVisualKitModelAttach` row placing the model ([`FxSlot::attach`]).
    attach: Option<benilla_formats::ModelAttach>,
    root: Option<Entity>,
    /// The self-termination deadline on `time.elapsed_secs()`, or a reaped instance's `Decay` end.
    expires: Option<f32>,
    /// Reaped and playing its `Decay` out. A reap or replace cannot reach it, as the reference has
    /// moved it to its pending-destroy list.
    decaying: bool,
    /// The held missile model's key, on a [`FxClass::Precast`] instance alone.
    precast: Option<PrecastKey>,
}

/// What a held missile model is matched by: its cast, its edge's stamp and whether a missile took
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PrecastKey {
    cast_count: u8,
    seq: u64,
    /// A missile created since took it, the unit flag `0x1000000` (`0x733058`).
    handed: bool,
}

/// A unit's effect instances (the client's per-unit `+0xb4` effect list).
#[derive(Component, Default)]
#[component(storage = "SparseSet")]
pub(super) struct FxAttached {
    instances: Vec<FxInstance>,
}

/// Which held missile model a release drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrecastDrop {
    /// Any, as the launch's tail does (`0x72e1ed`).
    Any,
    /// One begun at or before this stamp: a release that came earlier on the wire leaves a later
    /// hold.
    UpTo(u64),
    /// One begun at or before this stamp, of this cast count (`0x71bff0`).
    Cast(u64, u8),
}

impl FxAttached {
    /// Drop the held missile model `which` names, returning its spawned root for the caller to end.
    pub(super) fn release_precast(&mut self, which: PrecastDrop) -> Option<Entity> {
        let mut root = None;
        self.instances.retain(|i| {
            let Some(key) = i.precast else {
                return true;
            };
            let drops = match which {
                PrecastDrop::Any => true,
                PrecastDrop::UpTo(seq) => key.seq <= seq,
                PrecastDrop::Cast(seq, count) => key.seq <= seq && key.cast_count == count,
            };
            if drops {
                root = root.or(i.root);
            }
            !drops
        });
        root
    }

    /// Whether a missile model is held, spawned or still loading.
    pub(super) fn holds_precast(&self) -> bool {
        self.instances.iter().any(|i| i.precast.is_some())
    }

    /// Whether a missile created at stamp `seq` takes the held model: the first one created after
    /// the hold began (`0x73304d`).
    pub(super) fn takes_precast(&self, seq: u64) -> bool {
        self.instances
            .iter()
            .any(|i| i.precast.is_some_and(|k| !k.handed && k.seq <= seq))
    }

    /// Mark the held model taken (`0x733058`).
    pub(super) fn hand_precast(&mut self) {
        for k in self.instances.iter_mut().filter_map(|i| i.precast.as_mut()) {
            k.handed = true;
        }
    }

    /// The held model's spawned root while a missile has taken it.
    pub(super) fn handed_precast_root(&self) -> Option<Entity> {
        self.instances
            .iter()
            .find(|i| i.precast.is_some_and(|k| k.handed))
            .and_then(|i| i.root)
    }

    /// A unit holding one missile model of `spell_id` at the right spell hand, spawned as `root`.
    #[cfg(test)]
    pub(super) fn holding_for_tests(spell_id: u32, root: Option<Entity>) -> Self {
        Self {
            instances: vec![FxInstance {
                spell_id,
                persistent: true,
                class: FxClass::Precast,
                stage: FxStage::Relive,
                tag: 0x16,
                effect: 4704,
                path: "Spells\\Demolisher_Missile.mdx".into(),
                scale: 1.0,
                attach: None,
                root,
                expires: None,
                decaying: false,
                precast: Some(PrecastKey {
                    cast_count: 0,
                    seq: 0,
                    handed: false,
                }),
            }],
        }
    }

    /// The test hold re-keyed to a cast count and a stamp.
    #[cfg(test)]
    pub(super) fn keyed_for_tests(mut self, cast_count: u8, seq: u64) -> Self {
        for k in self.instances.iter_mut().filter_map(|i| i.precast.as_mut()) {
            (k.cast_count, k.seq) = (cast_count, seq);
        }
        self
    }
}

/// End an instance root at once: its emitters drain and the model goes.
pub(super) fn end_root(
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
    root: Entity,
) {
    drain_instance_emitters(emitters, root);
    commands.entity(root).despawn();
}

/// A world-planted kit instance root (kit column 14): a free entity planted once at its owner, and
/// re-planted on displacement for a root-aura spell.
#[derive(Component)]
pub(super) struct WorldPlantFx {
    owner: Entity,
    /// Re-plant on owner displacement (the client's flag `0x4000`): root-aura persistents only.
    follow: bool,
    /// The effect row's scale over the owner's.
    name_scale: Option<benilla_formats::EffectScale>,
    /// A `SpellVisualKitModelAttach` row's offset and turn on top of the plant.
    attach: Option<benilla_formats::ModelAttach>,
}

/// The client's re-plant gate, distance² > 1e-3 in world units.
const REPLANT_EPS_SQ: f32 = 1e-3;

/// The plant transform: the owner's position, yaw only and scale, then the row's offset turned by
/// the yaw but not scaled, and its turn inside the yaw (`0x6f8c31`).
fn world_plant_transform(
    owner: &GlobalTransform,
    attach: Option<&benilla_formats::ModelAttach>,
    name_scale: Option<benilla_formats::EffectScale>,
) -> Transform {
    let (scale, rotation, translation) = owner.to_scale_rotation_translation();
    let (yaw, _, _) = rotation.to_euler(EulerRot::YXZ);
    let yaw = Quat::from_rotation_y(yaw);
    let (offset, turn) = attach.map_or((Vec3::ZERO, Quat::IDENTITY), model_attach_local);
    Transform {
        translation: translation + yaw * offset,
        rotation: yaw * turn,
        scale: name_scale.map_or(scale, |s| Vec3::splat(s.planted(scale.x))),
    }
}

/// A `SpellVisualKitModelAttach` row as a Bevy offset and turn in its attachment's frame: WoW
/// `Rz(-yaw)·Ry(pitch)·Rx(roll)` and the y-negated offset (`0x6f84f0`).
pub(super) fn model_attach_local(a: &benilla_formats::ModelAttach) -> (Vec3, Quat) {
    // WoW z, y and x map to Bevy +Y, -X and -Z, so each turn keeps its sense about its image.
    let turn = Quat::from_rotation_y(-a.yaw)
        * Quat::from_rotation_x(-a.pitch)
        * Quat::from_rotation_z(-a.roll);
    (
        benilla_assets::coords::wow_to_bevy(a.applied_offset()),
        turn,
    )
}

/// Whether an `EffectApplyAuraName` slot is `SPELL_AURA_MOD_ROOT` (26): the client's
/// `spellRec+0x16c[0..2] == 0x1a` scan that arms the re-plant flag `0x4000`.
fn spell_has_root_aura(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    const SPELL_AURA_MOD_ROOT: u32 = 26;
    spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.effect_apply_aura.contains(&SPELL_AURA_MOD_ROOT))
}

/// The spell attribute (`AttributesEx5`) that forces an effect's projected batches on.
const SPELL_ATTR_EX5_FORCE_PROJECTION: u32 = 0x4000_0000;

/// Whether `spell_id`'s effects draw their projected batches whatever `projectedTextures` says
/// (attribute ExE `0x40000000`, [`benilla_world::model_render::ForcedProjection`]).
pub(super) fn spell_forces_projection(
    spells: Option<&crate::ui_action::Spells>,
    spell_id: u32,
) -> bool {
    spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.attributes_ex5 & SPELL_ATTR_EX5_FORCE_PROJECTION != 0)
}

/// Despawn a plant whose owner is gone, and re-plant a `follow` one whose owner moved past
/// [`REPLANT_EPS_SQ`] (per frame).
pub(super) fn tend_world_plants(
    mut commands: Commands,
    mut plants: Query<(Entity, &WorldPlantFx, &mut Transform)>,
    owners: Query<&GlobalTransform>,
) {
    for (root, plant, mut t) in &mut plants {
        let Ok(owner) = owners.get(plant.owner) else {
            commands.entity(root).despawn();
            continue;
        };
        let at = world_plant_transform(owner, plant.attach.as_ref(), plant.name_scale);
        if plant.follow && at.translation.distance_squared(t.translation) > REPLANT_EPS_SQ {
            *t = at;
        }
    }
}

/// Apply the router's [`SpellKitFx`] edges per unit in emission order: `Begin` records instances,
/// `Reap` decays the spell's persistent ones, so a GO's precast dies before its release flash.
pub(super) fn resolve_spell_fx(
    mut commands: Commands,
    mut events: MessageReader<SpellKitFx>,
    mut units: Query<&mut FxAttached>,
    fx: Option<ResMut<SpellFx>>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
    bodies: Query<(Has<crate::entities::VisualAttached>, Option<&BoneAttach>)>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    let mut ops: EntityHashMap<Vec<&SpellKitFx>> = EntityHashMap::default();
    for ev in events.read() {
        let entity = match ev {
            SpellKitFx::Begin { entity, .. }
            | SpellKitFx::Reap { entity, .. }
            | SpellKitFx::Hold { entity, .. } => *entity,
        };
        ops.entry(entity).or_default().push(ev);
    }
    for (entity, edges) in ops {
        if commands.get_entity(entity).is_err() {
            continue;
        }
        let existing = units.get_mut(entity).ok();
        let mut instances = match existing {
            Some(mut att) => std::mem::take(&mut att.instances),
            None => Vec::new(),
        };
        for edge in edges {
            match edge {
                SpellKitFx::Begin {
                    spell_id,
                    persistent,
                    class,
                    stage,
                    effects,
                    ..
                } => {
                    // A persistent Begin first reaps the same (spell, class): a re-applied aura's
                    // old node decays out while the new one is born, as in the reference.
                    if *persistent {
                        reap_matching(
                            &mut instances,
                            *spell_id,
                            *class,
                            &fx,
                            now,
                            &mut commands,
                            &mut emitters,
                        );
                    }
                    for FxSlot {
                        tag,
                        effect,
                        path,
                        scale,
                        attach,
                    } in effects
                    {
                        // Per slot, as the reference's effect add opens with the same-slot replace.
                        replace_same_slot(
                            &mut instances,
                            (*effect, *tag, attach.map(|a| a.id)),
                            &mut commands,
                            &mut emitters,
                        );
                        ensure_model(&mut fx, &asset_server, path);
                        instances.push(FxInstance {
                            spell_id: *spell_id,
                            persistent: *persistent,
                            class: *class,
                            stage: *stage,
                            tag: *tag,
                            effect: *effect,
                            path: path.clone(),
                            scale: *scale,
                            attach: *attach,
                            root: None,
                            expires: None,
                            decaying: false,
                            precast: None,
                        });
                    }
                }
                SpellKitFx::Reap {
                    spell_id, class, ..
                } => {
                    reap_matching(
                        &mut instances,
                        *spell_id,
                        *class,
                        &fx,
                        now,
                        &mut commands,
                        &mut emitters,
                    );
                }
                SpellKitFx::Hold {
                    spell_id,
                    cast_count,
                    seq,
                    slot,
                    ..
                } => {
                    // A new hold first drops the unit's last, whatever its spell (`0x722950`).
                    let mut att = FxAttached {
                        instances: std::mem::take(&mut instances),
                    };
                    if let Some(root) = att.release_precast(PrecastDrop::Any) {
                        end_root(&mut commands, &mut emitters, root);
                    }
                    instances = att.instances;
                    // The body must be loaded and have the attachment now, with no cascade
                    // (`0x7229da`, `0x7229ee`); the held model itself loads after (`0x722a3f`).
                    let (built, bones) = bodies.get(entity).unwrap_or((false, None));
                    let Some(slot) = slot.as_ref().filter(|sl| {
                        built && bones.is_some_and(|b| b.points.contains_key(&sl.tag))
                    }) else {
                        continue;
                    };
                    ensure_model(&mut fx, &asset_server, &slot.path);
                    instances.push(FxInstance {
                        spell_id: *spell_id,
                        persistent: true,
                        class: FxClass::Precast,
                        stage: FxStage::Relive,
                        tag: slot.tag,
                        effect: slot.effect,
                        path: slot.path.clone(),
                        scale: slot.scale,
                        attach: None,
                        root: None,
                        expires: None,
                        decaying: false,
                        precast: Some(PrecastKey {
                            cast_count: *cast_count,
                            seq: *seq,
                            handed: false,
                        }),
                    });
                }
            }
        }
        match units.get_mut(entity) {
            Ok(mut att) => att.instances = instances,
            Err(_) => {
                commands.entity(entity).insert(FxAttached { instances });
            }
        }
    }
}

/// Hand an ending instance's emitters to the drain, the half of the teardown that is not a despawn:
/// emission stops and live particles age out, since the reference gates the particle draw on the
/// live count while the meshes leave the draw list at once.
fn drain_instance_emitters(
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
    root: Entity,
) {
    for mut e in emitters.iter_mut() {
        if e.anchor() == Some(root) {
            e.drain_on_owner_loss();
        }
    }
}

/// The same-slot replace: destroy each live node with the same `SpellVisualEffectName` record at
/// the same tag, without a decay, which keeps the effect count flat in the number of attackers. The
/// world-plant tag (-1) is exempt; a decaying instance is out of reach. A node placed by a
/// `SpellVisualKitModelAttach` row matches only that row (`0x6f8a60`).
fn replace_same_slot(
    instances: &mut Vec<FxInstance>,
    (effect, tag, attach_row): (u32, u16, Option<u32>),
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    if tag == benilla_formats::WORLD_EFFECT_TAG {
        return;
    }
    instances.retain(|i| {
        let row_matches = i.attach.is_none_or(|a| Some(a.id) == attach_row);
        if i.decaying || i.effect != effect || i.tag != tag || !row_matches {
            return true;
        }
        if let Some(root) = i.root {
            // Through the shared teardown, so its particles finish.
            drain_instance_emitters(emitters, root);
            commands.entity(root).despawn();
        }
        false
    });
}

/// The spell-id reap: each persistent instance of `(spell_id, class)` plays `Decay` out, or goes at
/// once without a model, a ready model or a `Decay`.
fn reap_matching(
    instances: &mut Vec<FxInstance>,
    spell_id: u32,
    class: FxClass,
    fx: &SpellFx,
    now: f32,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    instances.retain_mut(|i| {
        if i.decaying || !i.persistent || i.spell_id != spell_id || !class.reaps(i.class) {
            return true;
        }
        debug!(
            "spell fx: {} reaped for spell {spell_id} ({class:?})",
            i.path
        );
        let span = i
            .root
            .and_then(|_| fx.models.get(&i.path))
            .and_then(|dm| decay_span(dm.animations.as_ref()));
        let (Some(root), Some(span)) = (i.root, span) else {
            if let Some(root) = i.root {
                // Destroyed at once, through the shared teardown like any other, so its particles
                // drain.
                drain_instance_emitters(emitters, root);
                commands.entity(root).despawn();
            }
            return false;
        };
        commands.entity(root).try_insert(FxDecay);
        i.decaying = true;
        i.expires = Some(now + span);
        lifecycle::trace_leg("decay", root, lifecycle::ANIM_DECAY);
        true
    });
}

/// Spawn pending instances whose model has built, and run the self-termination clock. A root hangs
/// under its attach joint and rides the bone; a world plant ([`WorldPlantFx`]) stands free.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn attach_spell_fx(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &mut FxAttached,
        Option<&BoneAttach>,
        Option<&mut benilla_world::rig_anim::RigPose>,
        &GlobalTransform,
        Has<crate::entities::VisualAttached>,
        Option<&crate::net::NetEntity>,
    )>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    spells: Option<Res<crate::ui_action::Spells>>,
    creatures: Option<Res<crate::entities::Creatures>>,
    visuals: Option<Res<crate::creature_anim::SpellVisuals>>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    let Some(mut fx) = fx else {
        return;
    };
    let now = time.elapsed_secs();
    for (unit, mut att, bones, mut pose, unit_gt, body_built, net) in &mut units {
        // A model flagged to hide them drops the hand-attachment models (`0x6f8d6e`).
        let hides_hands = net
            .and_then(|n| n.display_id)
            .zip(creatures.as_deref())
            .is_some_and(|(d, c)| c.hides_hand_effects(d));
        att.instances.retain_mut(|inst| {
            if hides_hands && matches!(inst.tag, 1 | 2) {
                if let Some(root) = inst.root {
                    commands.entity(root).despawn();
                }
                return false;
            }
            // Self-termination (the cast-release flash ran its span).
            if let Some(expires) = inst.expires {
                if now >= expires {
                    if let Some(root) = inst.root {
                        // The teardown: the particles outlive the flash.
                        drain_instance_emitters(&mut emitters, root);
                        commands.entity(root).despawn();
                    }
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fx",
                            &format!("kit expire unit={unit} path={}", inst.path),
                        );
                    }
                    return false;
                }
            }
            // A root lost with the unit's visual: the reference's rebuild drains the effect list
            // and re-creates what persists, so a persistent instance re-arms here and a one-shot or
            // decaying one goes.
            if let Some(root) = inst.root {
                if commands.get_entity(root).is_ok() {
                    return true;
                }
                if !inst.persistent || inst.decaying {
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fx",
                            &format!(
                                "kit collateral drain unit={unit} path={} decaying={}",
                                inst.path, inst.decaying
                            ),
                        );
                    }
                    return false;
                }
                inst.root = None;
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fx",
                        &format!("kit collateral re-create unit={unit} path={}", inst.path),
                    );
                }
            }
            // A held missile sits at its own attachment, with no cascade (`0x7229ee`): a rebuilt
            // body without it drops the hold.
            let held = inst.precast.is_some();
            if held && body_built && !bones.is_some_and(|b| b.points.contains_key(&inst.tag)) {
                return false;
            }
            // Pending: unspawnable flashes time out; a spawn sets the real span.
            if !inst.persistent && inst.expires.is_none() {
                inst.expires = Some(now + PENDING_TIMEOUT);
            }
            ensure_model(&mut fx, &asset_server, &inst.path);
            let Some(dm) = fx.models.get(&inst.path) else {
                return true; // unreachable: just inserted
            };
            if dm.parts.is_none() {
                return true; // model still loading
            }
            // The world-plant slot (kit column 14): no attach tag, planted once in world space,
            // riding no bone.
            let planted = inst.tag == benilla_formats::WORLD_EFFECT_TAG;
            // A bone-riding effect waits for the body: before it is built there are no attach
            // points, and a live root is never re-seated. The reference re-arms after the rebuild
            // sets the model; the cube fallback counts as built.
            if !planted && !body_built {
                return true;
            }
            let name_scale = visuals
                .as_deref()
                .and_then(|v| v.0.effect_scale(inst.effect));
            // Ground-anchored: a world plant, or an instance on the base point (`0x13`) or the unit
            // root. A chest-anchored one keeps its flat quads as geometry (`ProtectionFrom*`).
            let (root, ground_anchor) = if planted {
                let follow =
                    inst.persistent && spell_has_root_aura(spells.as_deref(), inst.spell_id);
                let root = commands
                    .spawn((
                        world_plant_transform(unit_gt, inst.attach.as_ref(), name_scale),
                        Visibility::default(),
                        WorldPlantFx {
                            owner: unit,
                            follow,
                            name_scale,
                            attach: inst.attach,
                        },
                    ))
                    // Chain-only visibility: the wrapper draws nothing, its children do.
                    .vis_chain_only()
                    .id();
                (root, true)
            } else {
                // The missile's cascade: the slot's tag, `0xf`, `0x13`, else the unit root.
                let point = bones.and_then(|b| {
                    std::iter::once(inst.tag)
                        .chain(ATTACH_FALLBACKS)
                        .find_map(|tag| b.points.get(&tag).copied().map(|p| (tag, p)))
                        .and_then(|(tag, (bone, offset))| {
                            pose.as_mut()
                                .and_then(|p| p.anchor_for(&mut commands, unit, bone))
                                .map(|joint| (tag, joint, offset))
                        })
                });
                let ground_anchor = point.is_none_or(|(tag, ..)| tag == 0x13);
                let (parent, offset) = point.map_or((unit, Vec3::ZERO), |(_, j, o)| (j, o));
                // A row's offset and turn sit in the attachment's frame (`0x6f8e41`).
                let (shift, turn) = inst
                    .attach
                    .as_ref()
                    .map_or((Vec3::ZERO, Quat::IDENTITY), model_attach_local);
                // The attachment's world scale is the unit's; a bone's own scale is not read.
                let unit_scale = unit_gt.to_scale_rotation_translation().0.x;
                let scale = if held {
                    // The name's scale in the world, turned a quarter right (`0x722a8f`).
                    name_scale.map_or(1.0, |s| s.scale) / unit_scale.max(f32::EPSILON)
                } else {
                    name_scale.map_or(inst.scale, |s| s.attached(inst.scale, unit_scale))
                };
                let turn = if held {
                    Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)
                } else {
                    turn
                };
                let root = commands
                    .spawn((
                        Transform::from_translation(offset + shift)
                            .with_rotation(turn)
                            .with_scale(Vec3::splat(scale)),
                        Visibility::default(),
                    ))
                    // Chain-only, as above.
                    .vis_chain_only()
                    .id();
                commands.entity(parent).add_child(root);
                (root, ground_anchor)
            };
            if spell_forces_projection(spells.as_deref(), inst.spell_id) {
                commands
                    .entity(root)
                    .insert(benilla_world::model_render::ForcedProjection);
            }
            // Parts were checked ready above, so this always attaches.
            attach_effect_visuals(
                &mut commands,
                root,
                dm,
                now,
                ground_anchor,
                // Chained to the unit: it fades and is freed with the body.
                EffectHost { parent: Some(unit) },
                // A kit effect runs the stage's lifecycle.
                Some(inst.stage),
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None, // an attach-point kit effect opens on its model's own `Stand`
            );
            inst.root = Some(root);
            debug!(
                "spell fx: {} spawned for spell {} on {unit} ({:?})",
                inst.path, inst.spell_id, inst.class
            );
            if !inst.persistent {
                // One pass of sequence 0, from the raw sequence table: a sequence with no bone
                // keys builds no clip (the eat/drink tankard). A looping one counts as one pass;
                // the reference's loop boundary is untraced, but its tankard ends with the drink.
                let span = dm.first_seq_span.unwrap_or(FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fx",
                    &format!(
                        "kit spawn unit={unit} path={} persistent={} span={:?}",
                        inst.path,
                        inst.persistent,
                        inst.expires.map(|e| e - now)
                    ),
                );
            }
            true
        });
    }
}

/// Fire the event keyframes each instance's clip crossed since last frame ([`scan_events`]), at the
/// host unit, so a sound authored in the effect's own M2 plays. An instance is born at t = 0, so
/// first sight fires `[0, cur]`: the level-up pillar's `$SND(888)` sits at 0.033 s.
pub(super) fn fire_fx_anim_events(
    units: Query<(Entity, &FxAttached)>,
    players: Query<&AnimationPlayer>,
    // The instance root's world frame and rig, the frame its event records are authored in.
    globals: Query<&GlobalTransform>,
    poses: Query<&benilla_world::rig_anim::RigPose>,
    fx: Option<Res<SpellFx>>,
    mut last: Local<EntityHashMap<f32>>,
    mut seen: Local<Vec<Entity>>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    let Some(fx) = fx else { return };
    seen.clear();
    for (unit, att) in &units {
        for inst in &att.instances {
            let Some(root) = inst.root else { continue };
            let Some(clip) = fx
                .models
                .get(&inst.path)
                .and_then(|dm| dm.animations.as_ref())
                .and_then(|a| a.clips.first())
                .filter(|c| !c.events.is_empty())
            else {
                continue;
            };
            let Ok(player) = players.get(root) else {
                continue;
            };
            let Some(active) = player.animation(clip.node) else {
                continue;
            };
            let cur = active.seek_time();
            seen.push(root);
            let prev = last.insert(root, cur).unwrap_or(-1.0);
            // Fired at the unit, but the point composes through the root's frame, the space it is
            // authored in (`ArcaneShot_Area.m2` puts a `$SND` 30.5 yd out).
            let Ok(root_world) = globals.get(root) else {
                continue;
            };
            let frame = crate::creature_anim::EventFrame {
                world: root_world,
                rig: poses
                    .get(root)
                    .ok()
                    .and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
            };
            scan_events(clip, unit, prev, cur, &frame, &mut out);
        }
    }
    if last.len() > seen.len() {
        let live: bevy::ecs::entity::EntityHashSet = seen.iter().copied().collect();
        last.retain(|e, _| live.contains(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// One spawned instance of Ice Barrier's state model.
    fn instance(root: Entity, persistent: bool) -> FxInstance {
        FxInstance {
            spell_id: 13033, // Ice Barrier
            persistent,
            class: FxClass::AuraState,
            stage: FxStage::State,
            tag: 0x13,
            effect: 1499, // Ice Barrier's state model
            path: "Spells\\IceShield_State.mdx".into(),
            scale: 1.0,
            attach: None,
            root: Some(root),
            expires: None,
            decaying: false,
            precast: None,
        }
    }

    /// An app running `attach_spell_fx`, and a unit with one spawned instance per `persistent`.
    fn standing(persistent: &[bool]) -> (App, Entity, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_asset::<SkinnedMeshInverseBindposes>()
            .init_resource::<SpellFx>()
            .init_resource::<FxTintAnims>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .add_systems(Update, attach_spell_fx);
        let roots: Vec<Entity> = persistent
            .iter()
            .map(|_| app.world_mut().spawn_empty().id())
            .collect();
        let instances = roots
            .iter()
            .zip(persistent)
            .map(|(&root, &p)| instance(root, p))
            .collect();
        let unit = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                FxAttached { instances },
            ))
            .id();
        (app, unit, roots)
    }

    fn instances_of(app: &App, unit: Entity) -> Vec<(bool, Option<Entity>)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .unwrap()
            .instances
            .iter()
            .map(|i| (i.persistent, i.root))
            .collect()
    }

    /// The reference's rebuild drains every effect node and re-creates what persists.
    #[test]
    fn a_persistent_instance_whose_root_died_as_collateral_is_re_armed() {
        let (mut app, unit, roots) = standing(&[true, false]);
        // A display swap despawns the unit's whole visual.
        for root in roots {
            app.world_mut().entity_mut(root).despawn();
        }
        app.update();
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "the aura's instance re-arms for the spawn pass; the one-shot drains",
        );
    }

    /// With the new body still loading, a re-armed instance waits rather than spawn at the root.
    #[test]
    fn a_re_armed_instance_waits_for_the_body_before_it_spawns() {
        let (mut app, unit, roots) = standing(&[true]);
        app.world_mut().entity_mut(roots[0]).despawn();
        app.world_mut().resource_mut::<SpellFx>().models.insert(
            "Spells\\IceShield_State.mdx".into(),
            DisplayModel {
                parts: Some(Vec::new()),
                ..crate::entities::display::empty_shell()
            },
        );
        app.update();
        app.update();
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "no body yet: the instance holds, it does not spawn at the root"
        );
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        assert!(
            matches!(instances_of(&app, unit)[..], [(true, Some(_))]),
            "the body is built: the instance spawns"
        );
    }

    /// A decaying instance was already ending, so the drain takes it too.
    #[test]
    fn a_decaying_instance_whose_root_died_as_collateral_is_dropped() {
        let (mut app, unit, roots) = standing(&[true]);
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[0]
            .decaying = true;
        app.world_mut().entity_mut(roots[0]).despawn();
        app.update();
        assert_eq!(instances_of(&app, unit), vec![]);
    }

    #[test]
    fn a_live_root_is_never_disturbed() {
        let (mut app, unit, roots) = standing(&[true]);
        app.update();
        assert_eq!(instances_of(&app, unit), vec![(true, Some(roots[0]))]);
    }

    // ---- the same-slot replace ----

    /// The blood spurt's record and the two flank tags.
    const SPURT: u32 = 63;
    const FRONT: u16 = 0xf;
    const BACK: u16 = 0x10;

    /// An app running `resolve_spell_fx` alone, plus one bare unit.
    fn resolving() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_resource::<SpellFx>()
            .add_message::<SpellKitFx>()
            .add_systems(Update, resolve_spell_fx);
        let unit = app.world_mut().spawn_empty().id();
        (app, unit)
    }

    /// One self-terminating `Begin` carrying one slot.
    fn begin(entity: Entity, effect: u32, tag: u16, path: &str) -> SpellKitFx {
        SpellKitFx::Begin {
            entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag,
                effect,
                path: path.into(),
                scale: 1.0,
                attach: None,
            }],
        }
    }

    fn send(app: &mut App, msg: SpellKitFx) {
        app.world_mut()
            .resource_mut::<Messages<SpellKitFx>>()
            .write(msg);
        app.update();
    }

    fn slots_of(app: &App, unit: Entity) -> Vec<(u32, u16)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .map(|a| a.instances.iter().map(|i| (i.effect, i.tag)).collect())
            .unwrap_or_default()
    }

    /// A second play of the same record at the same tag destroys the first.
    #[test]
    fn a_replay_of_the_same_record_at_the_same_tag_replaces_rather_than_stacks() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        // The first instance has been attached: give it the root the spawn pass would.
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[0]
            .root = Some(root);

        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT)],
            "one instance, not two"
        );
        assert!(
            app.world().get_entity(root).is_err(),
            "the replaced node is DESTROYED the same frame, not left to decay",
        );
    }

    /// A unit with a built body that has `tags`, for the hold's start gate.
    fn bodied(app: &mut App, tags: &[u16], built: bool) -> Entity {
        let mut e = app.world_mut().spawn(BoneAttach {
            points: tags.iter().map(|&t| (t, (0u16, Vec3::ZERO))).collect(),
            markers: HashMap::new(),
        });
        if built {
            e.insert(crate::entities::VisualAttached);
        }
        e.id()
    }

    fn held_slot(effect: u32) -> FxSlot {
        FxSlot {
            tag: 0x15,
            effect,
            path: "Spells\\Frostbolt.mdx".into(),
            scale: 1.0,
            attach: None,
        }
    }

    fn hold(entity: Entity, spell_id: u32, seq: u64, slot: Option<FxSlot>) -> SpellKitFx {
        SpellKitFx::Hold {
            entity,
            spell_id,
            cast_count: 3,
            seq,
            slot,
        }
    }

    /// A held missile model: a new hold of any spell drops the last at once, with no decay, and a
    /// hold whose own gate fails still drops it (`0x722950`).
    #[test]
    fn a_new_precast_hold_drops_the_last_even_when_its_own_gate_fails() {
        let (mut app, _) = resolving();
        let unit = bodied(&mut app, &[0x15], true);
        let seat = |app: &mut App| {
            let root = app.world_mut().spawn_empty().id();
            app.world_mut()
                .entity_mut(unit)
                .get_mut::<FxAttached>()
                .unwrap()
                .instances[0]
                .root = Some(root);
            root
        };
        send(&mut app, hold(unit, 47937, 1, Some(held_slot(3839))));
        let first = seat(&mut app);
        send(&mut app, hold(unit, 47801, 2, Some(held_slot(4016))));
        assert_eq!(
            slots_of(&app, unit),
            vec![(4016, 0x15)],
            "one hold per unit"
        );
        assert!(app.world().get_entity(first).is_err(), "dropped at once");
        let second = seat(&mut app);
        send(&mut app, hold(unit, 47937, 3, None));
        assert!(
            slots_of(&app, unit).is_empty(),
            "a hold whose gate fails still drops the last"
        );
        assert!(app.world().get_entity(second).is_err());
    }

    /// The hold's start gate: a loaded body with the slot's own attachment, no cascade
    /// (`0x7229da`, `0x7229ee`); the held model itself may still be loading.
    #[test]
    fn a_precast_hold_needs_a_built_body_with_its_attachment_at_start() {
        let (mut app, _) = resolving();
        let held = |app: &App, e: Entity| {
            app.world()
                .get::<FxAttached>(e)
                .is_some_and(FxAttached::holds_precast)
        };
        let ready = bodied(&mut app, &[0x15], true);
        let lacking = bodied(&mut app, &[0xf, 0x13], true);
        let unbuilt = bodied(&mut app, &[0x15], false);
        for unit in [ready, lacking, unbuilt] {
            send(&mut app, hold(unit, 1, 1, Some(held_slot(3839))));
        }
        assert!(held(&app, ready), "held, its model still loading");
        assert!(!held(&app, lacking), "no fall back to another attachment");
        assert!(!held(&app, unbuilt), "no body, no hold");
    }

    /// The hold's key: a missile made after it takes it once, a release stamped before it leaves
    /// it, and the end-cast clear matches its cast count (`0x73304d`, `0x71bff0`).
    #[test]
    fn a_held_model_is_taken_once_and_dropped_by_stamp_and_count() {
        let mut att = FxAttached::default();
        att.instances.push(FxInstance {
            precast: Some(PrecastKey {
                cast_count: 3,
                seq: 5,
                handed: false,
            }),
            ..instance(Entity::PLACEHOLDER, true)
        });
        assert!(!att.takes_precast(4), "a missile made before the hold");
        assert!(att.takes_precast(6));
        att.hand_precast();
        assert!(!att.takes_precast(7), "taken once");
        assert_eq!(att.handed_precast_root(), Some(Entity::PLACEHOLDER));
        assert_eq!(att.release_precast(PrecastDrop::UpTo(4)), None);
        assert_eq!(att.release_precast(PrecastDrop::Cast(9, 2)), None);
        assert!(att.holds_precast());
        assert_eq!(
            att.release_precast(PrecastDrop::Cast(9, 3)),
            Some(Entity::PLACEHOLDER)
        );
        assert!(!att.holds_precast());
    }

    /// A built body that lost the attachment drops the hold rather than cascade (`0x7229ee`).
    #[test]
    fn a_rebuilt_body_without_the_attachment_drops_the_hold() {
        let (mut app, unit, _) = standing(&[]);
        let held = |app: &App, e: Entity| {
            app.world()
                .entity(e)
                .get::<FxAttached>()
                .is_some_and(FxAttached::holds_precast)
        };
        let bones = |tag: u16| BoneAttach {
            points: HashMap::from([(tag, (0u16, Vec3::ZERO))]),
            markers: HashMap::new(),
        };
        app.world_mut().entity_mut(unit).insert((
            FxAttached::holding_for_tests(1, None),
            bones(0x16),
            crate::entities::VisualAttached,
        ));
        let lacking = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                FxAttached::holding_for_tests(1, None),
                bones(0xf),
                crate::entities::VisualAttached,
            ))
            .id();
        let rebuilding = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                FxAttached::holding_for_tests(1, None),
                bones(0xf),
            ))
            .id();
        app.update();
        assert!(held(&app, unit), "kept, waiting on its model");
        assert!(!held(&app, lacking), "no fall back to another attachment");
        assert!(held(&app, rebuilding), "waits for the body like any effect");
    }

    /// Execute's cast kit hangs one record on both hands (`0x15` and `0x16`), and both survive.
    #[test]
    fn the_same_record_at_a_different_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(&mut app, begin(unit, SPURT, BACK, "Particles\\Spurt.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, FRONT), (SPURT, BACK)]);
    }

    /// The key is the record, not the model path: two rows may name one `.mdx`.
    #[test]
    fn a_different_record_at_the_same_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(
            &mut app,
            begin(unit, SPURT + 1, FRONT, "Particles\\Spurt.mdx"),
        );
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT), (SPURT + 1, FRONT)]
        );
    }

    /// The walk skips tag -1: a second Thunder Clap ring does not eat the first.
    #[test]
    fn the_world_plant_slot_is_excluded_from_the_replace() {
        let (mut app, unit) = resolving();
        let tag = benilla_formats::WORLD_EFFECT_TAG;
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, tag), (SPURT, tag)]);
    }

    /// A map change clears the cache: the spawn pass rebuilds the entry and keeps the instance.
    #[test]
    fn an_evicted_cache_entry_is_rebuilt_by_the_spawn_pass() {
        const PATH: &str = "Spells\\StunSwirl_State_Head.mdx";

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
        ))
        .init_asset::<WowModelMaterial>()
        .init_asset::<benilla_assets::M2Model>()
        .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
        .init_resource::<Time>()
        .init_resource::<SpellFx>()
        .init_resource::<FxTintAnims>()
        .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
        .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .add_systems(Update, attach_spell_fx);

        let unit = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                FxAttached {
                    instances: vec![FxInstance {
                        spell_id: 9032,
                        persistent: true,
                        class: FxClass::AuraState,
                        stage: FxStage::State,
                        tag: 0x14,
                        effect: 50,
                        path: PATH.to_string(),
                        scale: 1.0,
                        attach: None,
                        root: None,
                        expires: None,
                        decaying: false,
                        precast: None,
                    }],
                },
            ))
            .id();
        assert!(
            app.world().resource::<SpellFx>().models.is_empty(),
            "the fixture is a cache the eviction already cleared"
        );

        app.update();

        assert!(
            app.world().resource::<SpellFx>().models.contains_key(PATH),
            "the spawn pass must get-or-insert its entry, not read and give up"
        );
        assert_eq!(
            app.world()
                .entity(unit)
                .get::<FxAttached>()
                .expect("the unit keeps its instance list")
                .instances
                .len(),
            1,
            "the instance must survive to use the rebuilt entry — a persistent one is never reaped \
             for being pending"
        );
    }

    /// A row's turns keep their sense in Bevy space: yaw +90° carries WoW +x to -y, pitch +90°
    /// carries it to -z, and the offset's y is negated (`0x6f84f0`).
    #[test]
    fn a_model_attach_row_turns_and_shifts_as_the_effect_matrix_does() {
        use benilla_assets::coords::wow_to_bevy;
        let near = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
        let row = |yaw, pitch| benilla_formats::ModelAttach {
            offset: [0.0, 2.0, 1.0],
            yaw,
            pitch,
            ..Default::default()
        };
        let (shift, turn) = model_attach_local(&row(std::f32::consts::FRAC_PI_2, 0.0));
        assert!(near(shift, wow_to_bevy([0.0, -2.0, 1.0])));
        assert!(near(
            turn * wow_to_bevy([1.0, 0.0, 0.0]),
            wow_to_bevy([0.0, -1.0, 0.0])
        ));
        let (_, turn) = model_attach_local(&row(0.0, std::f32::consts::FRAC_PI_2));
        assert!(near(
            turn * wow_to_bevy([1.0, 0.0, 0.0]),
            wow_to_bevy([0.0, 0.0, -1.0])
        ));
    }

    /// A node a row placed is replaced only by that row (`0x6f8a60`).
    #[test]
    fn a_model_attach_node_is_replaced_only_by_its_own_row() {
        let (mut app, unit) = resolving();
        let at = |id| benilla_formats::ModelAttach {
            id,
            ..Default::default()
        };
        let begin_row = |row| SpellKitFx::Begin {
            entity: unit,
            spell_id: 1,
            persistent: false,
            class: FxClass::Hold,
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: 0x13,
                effect: SPURT,
                path: "Spells\\Spurt.mdx".to_string(),
                scale: 1.0,
                attach: Some(at(row)),
            }],
        };
        send(&mut app, begin_row(1));
        send(&mut app, begin_row(2));
        assert_eq!(slots_of(&app, unit).len(), 2, "two rows coexist");
        send(&mut app, begin_row(2));
        assert_eq!(slots_of(&app, unit).len(), 2, "the same row replaces");
    }
}
