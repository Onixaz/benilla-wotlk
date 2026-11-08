//! Dest-anchored spell effects: what a ground cast shows at its point. A DynamicObject anchors a
//! persistent area effect with two visuals from `SpellVisual` column 25's kit, its own looping
//! model (`0x704b30`) and a shard emitter (`0x704f60`), outside the unit kit pipeline, and the area
//! kits play their models once at a point ([`GroundBurst`]).

use bevy::prelude::*;

use crate::creature_anim::{SpellKitSound, SpellVisuals};
use crate::net::{NetEntity, ObjectStore};
use benilla_protocol::EntityKind;

use super::spell_fx::{attach_effect_visuals, ensure_model, FxMaterials, SpellFx};

/// The `CharProcType` whose first kit slot the emitter takes (`0x705000`).
const PROC_TYPE_SHARD_EMITTER: i32 = 9;

/// The longest frame the emitter steps, 1/15 s (`0x7fc22f`).
const SHARD_STEP_CAP: f32 = 1.0 / 15.0;

/// How far above the centre a shard's ground search reaches, 16.67 yd (`0x7fc3b4`).
const SHARD_PROBE_RISE: f32 = 50.0 / 3.0;

/// The longest delay before a shard starts, `2 × trunc(255 × u)` ms (`0x7fc51a`).
const SHARD_DELAY_MAX_MS: f32 = 255.0;

/// The shard emitter's rate factor from the `spellEffectLevel` record's integer, read once at the
/// emitter's init (`0x7fb5e9`-`0x7fb5fe`), so a change reaches only
/// emitters created after it: `level × [0xa1ea9c] (1/9) × [0xa02a34] (0.9) + [0x9e3004] (0.1)`,
/// unclamped, and 1.0 with no record.
fn shard_rate_scale(spell_effect_level: Option<i32>) -> f32 {
    spell_effect_level.map_or(1.0, |level| level as f32 * 0.111_111_11 * 0.9 + 0.1)
}

/// A dest-anchored effect model: visual A, a shard or a GO burst.
#[derive(Component)]
pub(super) struct GroundFx {
    /// The [`SpellFx`] model-cache key.
    path: String,
    /// Loop for life (visual A), or one pass of sequence 0 then despawn (a shard, a burst).
    looping: bool,
    /// Nothing draws before this `time.elapsed_secs()`, a shard's start delay (`0x7fb33b`).
    starts: f32,
    /// Parts attached (the model was ready).
    spawned: bool,
    loop_armed: bool,
    /// One-shot self-termination deadline (`time.elapsed_secs()`), set at attach.
    expires: Option<f32>,
}

impl GroundFx {
    fn new(path: String, looping: bool) -> Self {
        Self {
            path,
            looping,
            starts: 0.0,
            spawned: false,
            loop_armed: false,
            expires: None,
        }
    }
}

/// Visual B, a DynamicObject's shard emitter. It dies with the anchor; its shards are free entities
/// that run out their own lifetimes.
#[derive(Component)]
pub(super) struct ShardEmitter {
    /// The `SpellVisualKitAreaModel` row the density picks (`0x7fbd70`).
    path: String,
    /// `DYNAMICOBJECT_RADIUS`: the wire radius is the spread.
    radius: f32,
    /// Shards per second: `CharParamOne` times the `spellEffectLevel` factor
    /// ([`shard_rate_scale`]), 1.0 at its default "9".
    rate: f32,
    /// Fractional emissions carried between frames.
    accum: f32,
    /// xorshift* state for the spawn offsets, seeded per emitter: no rand dependency.
    rng: u64,
}

/// One effect model of an area kit played at a point: `SMSG_SPELL_GO`'s instant area kit at the
/// dest point, a missile's impact area kit where it lands.
#[derive(Message, Clone)]
pub(crate) struct GroundBurst {
    /// The `SpellVisualEffectName` model path.
    pub(crate) path: String,
    /// The dest point, in Bevy coordinates.
    pub(crate) pos: Vec3,
    /// A `SpellVisualKitModelAttach` row's offset and turn at the point.
    pub(crate) attach: Option<benilla_formats::ModelAttach>,
}

/// Arm each new DynamicObject off its create's fields: visual A, the shard emitter and the area
/// sound. A re-cast is a new guid, never a re-create in place.
pub(super) fn arm_ground_effects(
    mut commands: Commands,
    created: Query<(Entity, &NetEntity, &ObjectStore), Added<ObjectStore>>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    mut sounds: MessageWriter<SpellKitSound>,
    spell_effect_level: Option<Res<crate::video::SpellEffectLevel>>,
) {
    let (Some(visuals), Some(spells), Some(mut fx)) = (visuals, spells, fx) else {
        return;
    };
    let level_scale = shard_rate_scale(spell_effect_level.map(|l| l.0));
    for (anchor, net, store) in &created {
        if net.kind != EntityKind::DynamicObject {
            continue;
        }
        let Some(spell_id) = store.0.dynamicobject_spell_id() else {
            continue;
        };
        let Some(stages) = spells.catalog.get(spell_id).and_then(|d| {
            visuals
                .0
                .stages(crate::creature_anim::spell_visual::spell_visual_id(d))
        }) else {
            debug!("dest_fx: dynobj spell {spell_id} has no visual row — invisible area");
            continue;
        };
        let facing = store
            .0
            .dynamicobject_position()
            .map(|(_, f)| f)
            .unwrap_or(0.0);
        // Visual A (`0x704b30`): the persistent kit's world effect, else its base slot, at the
        // object's position with no terrain projection, turned by `DYNAMICOBJECT_FACING`; as a
        // child it goes with the anchor.
        if let Some((_, path)) = visuals.0.persistent_area_model(stages) {
            let path = path.to_string();
            ensure_model(&mut fx, &asset_server, &path);
            let child = commands
                .spawn((
                    GroundFx::new(path, true),
                    Transform::from_rotation(Quat::from_rotation_y(facing)),
                    Visibility::default(),
                ))
                .id();
            // The dynamic object's model-loaded hook forces its model's projected batches by
            // attribute.
            if super::spell_fx::spell_forces_projection(Some(&*spells), spell_id) {
                commands
                    .entity(child)
                    .insert(benilla_world::model_render::ForcedProjection);
            }
            commands.entity(anchor).add_child(child);
        }
        // Visual B from the same kit, and the kit sound looped with or without it (`0x705059`).
        if let Some(kit) = visuals.0.kit(stages.persistent_area_kit) {
            if let Some(proc) = kit.char_procs().find(|p| p.ty == PROC_TYPE_SHARD_EMITTER) {
                let density = benilla_formats::shard_density(proc.params[0]);
                let path = visuals.0.area_model(density).to_string();
                ensure_model(&mut fx, &asset_server, &path);
                commands.entity(anchor).insert(ShardEmitter {
                    path,
                    radius: store.0.dynamicobject_radius().unwrap_or(0.0),
                    rate: proc.params[1] * level_scale,
                    accum: 0.0,
                    rng: 0x9e3779b97f4a7c15 ^ anchor.to_bits(),
                });
            }
            if let Some(kit_sound) = kit.sound {
                sounds.write(SpellKitSound::Play {
                    entity: anchor,
                    kit_sound,
                });
            }
        }
        debug!(
            "dest_fx: dynobj armed — spell {spell_id}, kit={} model={:?} radius {:?}",
            stages.persistent_area_kit,
            visuals.0.persistent_area_model(stages).map(|(id, _)| id),
            store.0.dynamicobject_radius(),
        );
    }
}

/// Spawn the GO dest one-shots at once, never waiting on the DynamicObject create that follows.
pub(super) fn spawn_ground_bursts(
    mut commands: Commands,
    mut bursts: MessageReader<GroundBurst>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
) {
    let Some(mut fx) = fx else { return };
    for burst in bursts.read() {
        ensure_model(&mut fx, &asset_server, &burst.path);
        // Deviation: the reference turns a row's offset by the caster's facing (`0x6f8c31`); a
        // point play carries no facing, so it lands unturned.
        let (shift, turn) = burst.attach.as_ref().map_or(
            (Vec3::ZERO, Quat::IDENTITY),
            super::spell_fx::model_attach_local,
        );
        commands.spawn((
            GroundFx::new(burst.path.clone(), false),
            Transform::from_translation(burst.pos + shift).with_rotation(turn),
            Visibility::default(),
        ));
    }
}

/// Emit `rate` shards per second (`0x7fc220`): a random direction at a uniform fraction of the
/// radius, so they crowd the centre, set on the ground and started up to 510 ms late.
pub(super) fn tick_shard_emitters(
    mut commands: Commands,
    time: Res<Time>,
    mut emitters: Query<(&mut ShardEmitter, &GlobalTransform)>,
    ground: benilla_world::surface::TerrainHeight,
) {
    let now = time.elapsed_secs();
    for (mut em, tf) in &mut emitters {
        em.accum += em.rate * time.delta_secs().min(SHARD_STEP_CAP);
        if em.accum < 1.0 {
            continue;
        }
        let center = tf.translation();
        let height = |p: Vec3| ground.at(p);
        // Deviation: the reference rays the world up then down (`0x7fc3d8`, `0x7fc46c`); the
        // heightfield alone is cheap, so only an emitter standing on the terrain follows it.
        let on_terrain = height(center).is_some_and(|h| (h - center.y).abs() < 0.5);
        while em.accum >= 1.0 {
            em.accum -= 1.0;
            let mut next = || {
                em.rng ^= em.rng >> 12;
                em.rng ^= em.rng << 25;
                em.rng ^= em.rng >> 27;
                (em.rng.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32
            };
            let (scale, angle, delay) = (next(), next() * std::f32::consts::TAU, next());
            let r = em.radius * scale;
            let mut at = center + Vec3::new(r * angle.cos(), 0.0, r * angle.sin());
            // Off terrain a flat disc; a probe miss takes the centre itself (`0x7fc4a3`).
            if on_terrain {
                match height(at).filter(|h| (h - center.y).abs() <= SHARD_PROBE_RISE) {
                    Some(h) => at.y = h,
                    None => at = center,
                }
            }
            let mut fx = GroundFx::new(em.path.clone(), false);
            fx.starts = now + 2.0 * (delay * SHARD_DELAY_MAX_MS).trunc() / 1000.0;
            commands.spawn((fx, Transform::from_translation(at), Visibility::default()));
        }
    }
}

/// Attach each pending instance's parts once its M2 builds, start the one-shot clocks, arm the
/// loops and despawn expired one-shots.
pub(super) fn attach_ground_fx_models(
    mut commands: Commands,
    time: Res<Time>,
    mut instances: Query<(Entity, &mut GroundFx, Option<&mut AnimationPlayer>)>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    mut wow_materials: ResMut<Assets<benilla_assets::materials::WowModelMaterial>>,
    mut tint_reg: ResMut<super::spell_fx::FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    for (entity, mut inst, player) in &mut instances {
        if !inst.spawned {
            if now < inst.starts {
                continue;
            }
            ensure_model(&mut fx, &asset_server, &inst.path);
            let Some(dm) = fx.models.get(&inst.path) else {
                continue; // unreachable: just inserted
            };
            if !attach_effect_visuals(
                &mut commands,
                entity,
                dm,
                now,
                true, // a dest-anchored model's flat quads are ground decals
                // Chained to nothing: its trail stays world-frozen and its pool finishes in place.
                super::spell_fx::EffectHost::default(),
                // Not a kit effect on a unit: its own span clock, the plain single-clip arm.
                None,
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None,
            ) {
                continue; // model still building
            }
            inst.spawned = true;
            if !inst.looping {
                // One pass of the first sequence, as `spell_fx`'s span clock times a kit.
                let span = dm.first_seq_span.unwrap_or(super::spell_fx::FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            continue; // the AnimationPlayer lands next frame
        }
        // The reference re-fires on completion, the hardcoded sequence `0x9e` when `obj+0x190` bit
        // 1 is set (its source untraced); this repeats the first clip.
        if inst.looping && !inst.loop_armed {
            if let Some(mut player) = player {
                for (_, anim) in player.playing_animations_mut() {
                    anim.repeat();
                }
                inst.loop_armed = true;
            }
        }
        if let Some(expires) = inst.expires {
            if now >= expires {
                commands.entity(entity).despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shard_rate_follows_the_spell_effect_level_record() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(
            near(shard_rate_scale(Some(9)), 1.0),
            "the registered 9 is full rate"
        );
        assert!(near(shard_rate_scale(Some(0)), 0.1));
        assert!(near(shard_rate_scale(Some(3)), 0.4));
        // Unclamped, as the record's integer reaches the multiply.
        assert!(near(shard_rate_scale(Some(18)), 1.9));
        assert_eq!(shard_rate_scale(None), 1.0);
    }
}
