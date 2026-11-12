//! Dest-anchored spell effects: what a ground cast shows at its point. A DynamicObject anchors a
//! persistent area effect with two visuals from `SpellVisual` column 25's kit, its own looping
//! model (`0x704b30`) and a shard emitter (`0x704f60`), outside the unit kit pipeline, and the area
//! kits play their models once at a point ([`GroundBurst`]).

use bevy::prelude::*;

use crate::creature_anim::{SpellGoTargets, SpellKitSound, SpellVisuals};
use crate::net::{NetEntity, ObjectStore};
use benilla_protocol::messages::ObjectType;
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
    /// The unit whose effect list holds it: it loops until that unit is gone (`0x744ac0`).
    owner: Option<Entity>,
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
            owner: None,
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
    /// The model's uniform scale.
    pub(crate) scale: f32,
    /// The turn it stands at, as a resist burst takes the missile's facing (`0x7031ea`).
    pub(crate) turn: Quat,
    /// The unit a looping point play hangs on, or `None` for a one-pass burst.
    pub(crate) held_by: Option<Entity>,
    /// The transport the point is on, which the model rides while its unit lasts.
    pub(crate) ride: Option<crate::transport::DeckRide>,
}

/// One cast flying at a point or a unit, until it would land (`0x804b20`).
struct InTransit {
    caster: u64,
    spell_id: u32,
    server_time: u32,
    arrives: f32,
}

/// The casts in flight, which a DynamicObject of the same cast waits on.
#[derive(Resource, Default)]
pub(super) struct SpellsInTransit(Vec<InTransit>);

impl SpellsInTransit {
    /// The lookup of `0x804cc0`, keyed as the object's fields name its cast.
    fn holds(&self, caster: u64, spell_id: u32, server_time: u32) -> bool {
        self.0
            .iter()
            .any(|s| s.caster == caster && s.spell_id == spell_id && s.server_time == server_time)
    }
}

/// A DynamicObject whose spell is still flying: nothing of it shows yet (`0x7049d0`, `0x704f60`).
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(super) struct AwaitingTransit;

/// Note each GO of a travelling spell by the time it lands, and drop those landed (`0x804c10`).
pub(super) fn record_spells_in_transit(
    mut gos: MessageReader<SpellGoTargets>,
    spells: Option<Res<crate::ui_action::Spells>>,
    units: Query<&GlobalTransform>,
    stores: Query<&crate::net::ObjectStore>,
    time: Res<Time>,
    mut list: ResMut<SpellsInTransit>,
) {
    let now = time.elapsed_secs();
    list.0.retain(|s| s.arrives > now);
    let Some(spells) = spells else {
        gos.clear();
        return;
    };
    for go in gos.read() {
        let speed = spells.catalog.get(go.spell_id).map_or(0.0, |d| d.speed);
        // A unit caster's alone (`0x80e780`).
        let object = stores.get(go.caster).is_ok_and(|s| {
            s.0.object_type()
                .is_some_and(|t| !matches!(t, ObjectType::Unit | ObjectType::Player))
        });
        let Some(from) = units.get(go.caster).ok().filter(|_| speed > 0.0 && !object) else {
            continue;
        };
        let first = go.hits.first().or(go.misses.first().map(|(e, _)| e));
        let to = go
            .dest
            .or_else(|| Some(units.get(*first?).ok()?.translation()));
        if let Some(to) = to {
            list.0.push(InTransit {
                caster: go.caster_guid,
                spell_id: go.spell_id,
                server_time: go.server_time,
                arrives: now + from.translation().distance(to) / speed,
            });
        }
    }
}

/// A model held at a point until its owner despawns it, a kit a flight rides on the ground.
pub(super) fn spawn_held_point_fx(
    commands: &mut Commands,
    path: &str,
    pos: Vec3,
    ride: Option<crate::transport::DeckRide>,
    attach: Option<benilla_formats::ModelAttach>,
    scale: f32,
) -> Entity {
    let (shift, turn) = attach.as_ref().map_or(
        (Vec3::ZERO, Quat::IDENTITY),
        super::spell_fx::model_attach_local,
    );
    let mut e = commands.spawn((
        GroundFx::new(path.to_string(), true),
        Transform::from_translation(pos + shift)
            .with_rotation(turn)
            .with_scale(Vec3::splat(scale)),
        Visibility::default(),
    ));
    if let Some(ride) = ride {
        e.insert(crate::transport::DeckEffect { ride, shift });
    }
    e.id()
}

/// Arm each new DynamicObject off its create's fields: visual A, the shard emitter and the area
/// sound. A re-cast is a new guid, never a re-create in place.
pub(super) fn arm_ground_effects(
    mut commands: Commands,
    created: Query<(Entity, &NetEntity, &ObjectStore), Added<ObjectStore>>,
    waiting: Query<(Entity, &NetEntity, &ObjectStore), With<AwaitingTransit>>,
    transit: Res<SpellsInTransit>,
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
    let flying = |store: &ObjectStore| {
        transit.holds(
            store.0.dynamicobject_caster().unwrap_or(0),
            store.0.dynamicobject_spell_id().unwrap_or(0),
            store.0.dynamicobject_cast_time().unwrap_or(0),
        )
    };
    // A waiting object arms the frame its spell lands (`0x7051b0`).
    let landed = waiting.iter().filter(|(_, _, store)| !flying(store));
    for (anchor, net, store) in created.iter().chain(landed) {
        if net.kind != EntityKind::DynamicObject {
            continue;
        }
        if flying(store) {
            commands.entity(anchor).insert(AwaitingTransit);
            continue;
        }
        commands.entity(anchor).remove::<AwaitingTransit>();
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
        if let Some((effect, path)) = visuals.0.persistent_area_model(stages) {
            let path = path.to_string();
            ensure_model(&mut fx, &asset_server, &path);
            // The row's scale when above 0 (`0x704d90`); a portal's radius fit is not built.
            let scale = visuals.0.effect_scale(effect).map_or(1.0, |s| {
                if s.scale > 0.0 {
                    s.scale
                } else {
                    1.0
                }
            });
            let child = commands
                .spawn((
                    GroundFx::new(path, true),
                    Transform::from_rotation(Quat::from_rotation_y(facing))
                        .with_scale(Vec3::splat(scale)),
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
        let mut ground = GroundFx::new(burst.path.clone(), burst.held_by.is_some());
        ground.owner = burst.held_by;
        let mut e = commands.spawn((
            ground,
            Transform::from_translation(burst.pos + shift)
                .with_rotation(burst.turn * turn)
                .with_scale(Vec3::splat(burst.scale)),
            Visibility::default(),
        ));
        if let Some(ride) = burst.ride {
            e.insert(crate::transport::DeckEffect { ride, shift });
        }
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
        if inst.owner.is_some_and(|o| commands.get_entity(o).is_err()) {
            commands.entity(entity).despawn();
            continue;
        }
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
    fn a_cast_is_in_transit_only_under_its_own_three_keys() {
        let list = SpellsInTransit(vec![InTransit {
            caster: 7,
            spell_id: 133,
            server_time: 900,
            arrives: 1.0,
        }]);
        assert!(list.holds(7, 133, 900));
        assert!(!list.holds(8, 133, 900));
        assert!(!list.holds(7, 116, 900));
        assert!(!list.holds(7, 133, 901));
    }

    /// A unit's travelling GO is noted, a GameObject's never (`0x80e780`).
    #[test]
    fn only_a_unit_casters_go_is_in_transit() {
        const SPELL: u32 = 133;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SpellsInTransit>()
            .add_message::<SpellGoTargets>()
            .add_systems(Update, record_spells_in_transit);
        app.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from(
                [(
                    SPELL,
                    benilla_formats::SpellDisplay {
                        speed: 20.0,
                        ..Default::default()
                    },
                )],
            )),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let mut caster = |mask: u32| {
            app.world_mut()
                .spawn((
                    GlobalTransform::default(),
                    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, mask)])),
                ))
                .id()
        };
        let (unit, trap) = (caster(0x9), caster(0x21));
        for (guid, caster) in [(1u64, unit), (2, trap)] {
            app.world_mut().write_message(SpellGoTargets {
                caster,
                spell_id: SPELL,
                hits: Vec::new(),
                misses: Vec::new(),
                list: Vec::new(),
                target_mask: 0x40,
                src: None,
                src_deck: None,
                dest: Some(Vec3::new(40.0, 0.0, 0.0)),
                dest_deck: None,
                points: Vec::new(),
                ammo: None,
                seq: 1,
                trajectory: None,
                reflected: 0,
                reflects: Vec::new(),
                caster_guid: guid,
                server_time: 900,
            });
        }
        app.update();
        let list = app.world().resource::<SpellsInTransit>();
        assert!(list.holds(1, SPELL, 900), "the unit's");
        assert!(!list.holds(2, SPELL, 900), "not the GameObject's");
    }

    /// A point play held by a unit loops while the unit stands and goes with it (`0x744ac0`); a
    /// plain burst is a one-pass model with no owner.
    #[test]
    fn a_held_point_play_lives_until_its_unit_is_gone() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<benilla_assets::M2Model>()
            .init_asset::<benilla_assets::materials::WowModelMaterial>()
            .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
            .init_resource::<SpellFx>()
            .init_resource::<super::super::spell_fx::FxTintAnims>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .add_message::<GroundBurst>()
            .add_systems(
                Update,
                (spawn_ground_bursts, attach_ground_fx_models).chain(),
            );
        let caster = app.world_mut().spawn_empty().id();
        let burst = |held_by| GroundBurst {
            path: "Spells\\Base.m2".to_string(),
            pos: Vec3::new(24.0, 0.0, 0.0),
            attach: None,
            scale: 1.0,
            turn: Quat::IDENTITY,
            held_by,
            ride: None,
        };
        app.world_mut().write_message(burst(Some(caster)));
        app.world_mut().write_message(burst(None));
        app.update();
        let live = |app: &mut App| -> Vec<(bool, Option<Entity>)> {
            let mut q = app.world_mut().query::<&GroundFx>();
            q.iter(app.world()).map(|g| (g.looping, g.owner)).collect()
        };
        let mut now = live(&mut app);
        now.sort_by_key(|(looping, _)| *looping);
        assert_eq!(now, vec![(false, None), (true, Some(caster))]);
        app.update();
        assert_eq!(live(&mut app).len(), 2, "held while the caster stands");
        app.world_mut().despawn(caster);
        app.update();
        assert_eq!(live(&mut app), vec![(false, None)], "gone with the caster");
    }

    /// A held point model (a ground targeting kit's) on a transport spawns riding it.
    #[test]
    fn a_held_point_model_on_a_transport_spawns_riding_it() {
        let mut world = World::new();
        let (deck, caster) = (world.spawn_empty().id(), world.spawn_empty().id());
        let ride = crate::transport::DeckRide {
            deck,
            local: Vec3::X,
            source: caster,
        };
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let riding = spawn_held_point_fx(&mut commands, "a.m2", Vec3::ONE, Some(ride), None, 1.0);
        let plain = spawn_held_point_fx(&mut commands, "a.m2", Vec3::ONE, None, None, 1.0);
        queue.apply(&mut world);
        let fx = world.get::<crate::transport::DeckEffect>(riding).copied();
        assert_eq!(
            fx,
            Some(crate::transport::DeckEffect {
                ride,
                shift: Vec3::ZERO
            })
        );
        assert!(world.get::<crate::transport::DeckEffect>(plain).is_none());
    }

    /// A burst on a transport spawns riding it, with its attach row's offset kept beside the
    /// point; one off any transport spawns with no ride.
    #[test]
    fn a_burst_on_a_transport_spawns_riding_it() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<benilla_assets::M2Model>()
            .init_resource::<SpellFx>()
            .add_message::<GroundBurst>()
            .add_systems(Update, spawn_ground_bursts);
        let deck = app.world_mut().spawn_empty().id();
        let caster = app.world_mut().spawn_empty().id();
        let ride = crate::transport::DeckRide {
            deck,
            local: Vec3::new(1.0, 2.0, 3.0),
            source: caster,
        };
        let attach = benilla_formats::ModelAttach {
            offset: [0.0, 0.0, 2.0],
            ..Default::default()
        };
        for ride in [Some(ride), None] {
            app.world_mut().write_message(GroundBurst {
                path: "Spells\\Base.m2".to_string(),
                pos: Vec3::new(24.0, 0.0, 0.0),
                attach: Some(attach),
                scale: 1.0,
                turn: Quat::IDENTITY,
                held_by: None,
                ride,
            });
        }
        app.update();
        let mut q = app
            .world_mut()
            .query::<(&GroundFx, Option<&crate::transport::DeckEffect>)>();
        let rides: Vec<_> = q.iter(app.world()).map(|(_, d)| d.copied()).collect();
        assert_eq!(rides.len(), 2);
        let (shift, _) = super::super::spell_fx::model_attach_local(&attach);
        assert!(rides.contains(&Some(crate::transport::DeckEffect { ride, shift })));
        assert!(rides.contains(&None));
    }

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
