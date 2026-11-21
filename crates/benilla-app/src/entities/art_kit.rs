//! GameObject art kits: `GAMEOBJECT_BYTES_1` byte 2 names a `GameObjectArtKit` row whose attach
//! models hang on the object's model, model `i` at attachment id `i`: an Eastern Plaguelands
//! tower's faction banner, a Hellfire fort's flags, a Midsummer bonfire's flame.
//!
//! The reference applies the kit as the model loads (`0x713f50`) and again on every change of the
//! byte once it has (`0x713ed0`). Each application first takes the children on attachment ids 0 to
//! 3 off the model, then attaches the row's models; a model lacking the attachment takes nothing
//! there, as its attach returns on the lookup's miss (`0x831630`). Here the first application waits
//! for the built visual, and a rebuild clears [`AppliedArtKit`], so the new body takes the kit again.
//!
//! Deviations: only the models the previous kit hung come off, not every child on ids 0 to 3 (a
//! spell visual hung there stays); and a row's models hang together once each has loaded or
//! failed to, where the reference attaches each as it creates it, skipping one that fails.
//!
//! A row's texture variations, which the reference binds to texture types 11 to 13 of the object's
//! model, are not applied: no 12340 row names one, and they would need a display per art kit.

use std::collections::HashMap;

use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;
use benilla_formats::GameObjectArtKits;
use benilla_protocol::EntityKind;
use benilla_world::vis_chain::VisChainOnly;
use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};

use super::spell_fx::{attach_effect_visuals, EffectHost, FxMaterials, FxTintAnims};
use super::{BoneAttach, DisplayModel, GameObjects, ModelHandle, VisualAttached};

/// The `GameObjectArtKit` table and a path-keyed [`DisplayModel`] per attach model. Without it no
/// GameObject wears its kit.
#[derive(Resource)]
pub(crate) struct ArtKits {
    pub(super) table: GameObjectArtKits,
    pub(super) models: HashMap<String, DisplayModel>,
}

impl ArtKits {
    pub(super) fn new(table: GameObjectArtKits) -> Self {
        ArtKits {
            table,
            models: HashMap::new(),
        }
    }

    /// Create the cache entries of a row's models, so `super::update_display_models` builds them.
    fn ensure_models(&mut self, art_kit: i8, asset_server: &AssetServer) {
        let Some(row) = self.table.get(art_kit) else {
            return;
        };
        for (_, path) in row.attached() {
            if !self.models.contains_key(path) {
                self.models.insert(
                    path.to_string(),
                    DisplayModel {
                        handle: ModelHandle::M2(asset_server.load(m2_url(path))),
                        ..super::empty_shell()
                    },
                );
            }
        }
    }
}

/// The art kit this GameObject's model wears and the instance roots it hung for it. Removed with
/// the visual, so a rebuilt body applies the kit again.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(super) struct AppliedArtKit {
    kit: i8,
    roots: Vec<Entity>,
}

/// The kit's models are still loading: revisit until they hang.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(super) struct ArtKitPending;

/// Where one attach model hangs: under the rig's joint for the attachment's bone, or, on a model
/// with no rig, at the attachment's bind-pose point under the object.
fn attach_seat(
    commands: &mut Commands,
    object: Entity,
    host: &DisplayModel,
    bones: Option<&BoneAttach>,
    pose: Option<&mut benilla_world::rig_anim::RigPose>,
    id: u16,
) -> Option<(Entity, Vec3)> {
    match (bones, pose) {
        (Some(bones), Some(pose)) => {
            let &(bone, offset) = bones.points.get(&id)?;
            let joint = pose.anchor_for(commands, object, bone)?;
            Some((joint, offset))
        }
        _ => crate::portrait::attachment_point(&host.skeleton, &host.attachments, id)
            .map(|at| (object, at)),
    }
}

/// Put each built GameObject's art kit on its model: on its first visit, and when the byte moves.
#[allow(clippy::type_complexity)] // the visit gate: new body, changed fields or a pending load
pub(super) fn apply_art_kits(
    mut commands: Commands,
    mut objects: Query<
        (
            Entity,
            &NetEntity,
            &ObjectStore,
            Option<&AppliedArtKit>,
            Has<ArtKitPending>,
            Option<&BoneAttach>,
            Option<&mut benilla_world::rig_anim::RigPose>,
        ),
        // A new or rebuilt body (a rebuild removes and re-adds `VisualAttached`), a pending one,
        // or moved fields: never every unit every frame.
        (
            With<VisualAttached>,
            Or<(
                Added<VisualAttached>,
                With<ArtKitPending>,
                Changed<ObjectStore>,
            )>,
        ),
    >,
    kits: Option<ResMut<ArtKits>>,
    gameobjects: Option<Res<GameObjects>>,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    // The session's first kit to hang is logged once at info level.
    mut logged: Local<bool>,
) {
    let (Some(mut kits), Some(gameobjects)) = (kits, gameobjects) else {
        return;
    };
    let now = time.elapsed_secs();
    for (object, net, store, applied, pending, bones, mut pose) in &mut objects {
        if net.kind != EntityKind::GameObject {
            continue;
        }
        let kit = store.0.gameobject_art_kit();
        if applied.is_some_and(|a| a.kit == kit) && !pending {
            continue; // another field moved
        }
        // The old kit comes off before the new one goes on (the module's first deviation).
        for &root in applied.map_or(&[][..], |a| &a.roots[..]) {
            commands.entity(root).try_despawn();
        }
        let pend = |commands: &mut Commands| {
            commands.entity(object).insert((
                AppliedArtKit {
                    kit,
                    roots: Vec::new(),
                },
                ArtKitPending,
            ));
        };
        let settle = |commands: &mut Commands, roots: Vec<Entity>| {
            commands
                .entity(object)
                .insert(AppliedArtKit { kit, roots })
                .remove::<ArtKitPending>();
        };
        let Some(row) = kits.table.get(kit) else {
            settle(&mut commands, Vec::new()); // kit 0, or no row: nothing to hang
            continue;
        };
        if row.textures.iter().any(Option::is_some) {
            debug!("art kit {kit} names texture variations, which are not applied");
        }
        let Some(host) = net.display_id.and_then(|d| gameobjects.models.get(&d)) else {
            // No display entry (evicted, or none): no points to hang on. A rebuilt body, which a
            // map change's eviction brings, takes the kit again.
            settle(&mut commands, Vec::new());
            continue;
        };
        kits.ensure_models(kit, &asset_server);
        let Some(row) = kits.table.get(kit) else {
            continue; // unreachable: looked up above
        };
        // A row's models hang together once each is built or has failed to load (the module's
        // second deviation); an emitter-only model's empty list is `Some`.
        let ready = row.attached().all(|(_, p)| {
            kits.models.get(p).is_some_and(|dm| {
                dm.parts.is_some()
                    || matches!(&dm.handle, ModelHandle::M2(h)
                        if asset_server.load_state(h.id()).is_failed())
            })
        });
        if !ready {
            pend(&mut commands);
            continue;
        }
        let mut roots = Vec::new();
        for (id, path) in row.attached() {
            // A model that failed to load hangs nothing, as the reference skips it.
            let Some(dm) = kits.models.get(path).filter(|dm| dm.parts.is_some()) else {
                continue;
            };
            let Some((parent, offset)) =
                attach_seat(&mut commands, object, host, bones, pose.as_deref_mut(), id)
            else {
                debug!(
                    "art kit {kit}: display {:?} has no attachment {id} for {path}",
                    net.display_id
                );
                continue;
            };
            let root = commands
                .spawn((Transform::from_translation(offset), Visibility::default()))
                // Chain-only visibility: the root draws nothing, its children do.
                .vis_chain_only()
                .id();
            commands.entity(parent).add_child(root);
            attach_effect_visuals(
                &mut commands,
                root,
                dm,
                now,
                false, // hung on the object's model, never ground-anchored
                // Chained to the object's model: it fades with it and goes with it.
                EffectHost {
                    parent: Some(object),
                },
                // Not armed by the kit player: the plain single-clip arm.
                None,
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None, // the model's idle, as a newly created model plays
            );
            roots.push(root);
        }
        let authored = row.attached().count();
        if !*logged && !roots.is_empty() {
            *logged = true;
            info!(
                "art kit: display {:?} kit {kit} → {} of {authored} model(s) attached (the first \
                 this session)",
                net.display_id,
                roots.len(),
            );
        }
        debug!(
            "art kit: display {:?} kit {kit} → {} of {authored} model(s)",
            net.display_id,
            roots.len(),
        );
        settle(&mut commands, roots);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_formats::ArtKit;

    fn kits() -> ArtKits {
        let mut rows = HashMap::new();
        rows.insert(
            1,
            ArtKit {
                models: [Some("Flag.mdx".into()), None, None, None],
                ..Default::default()
            },
        );
        ArtKits::new(GameObjectArtKits::from_rows(rows))
    }

    fn world() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
            .init_resource::<FxTintAnims>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .insert_resource(kits())
            .insert_resource(GameObjects {
                catalog: benilla_formats::GameObjectCatalog::default(),
                models: HashMap::from([(
                    7,
                    DisplayModel {
                        attachments: vec![benilla_assets::ModelAttachment {
                            id: 0,
                            bone: 0,
                            offset: Vec3::new(0.0, 2.0, 0.0),
                        }],
                        skeleton: one_bone(),
                        ..super::super::empty_display()
                    },
                )]),
            })
            .add_systems(Update, apply_art_kits);
        app
    }

    fn one_bone() -> benilla_assets::ModelSkeleton {
        benilla_assets::ModelSkeleton {
            joints: vec![benilla_assets::ModelJoint {
                parent: -1,
                local_translation: Vec3::new(0.0, 1.0, 0.0),
                billboard: None,
                parent_arm: None,
            }],
            ..Default::default()
        }
    }

    fn object(app: &mut App, bytes_1: u32) -> Entity {
        app.world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::GameObject,
                    display_id: Some(7),
                    scale: 1.0,
                },
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_GAMEOBJECT_BYTES_1,
                    bytes_1,
                )])),
                VisualAttached,
                Transform::default(),
                Visibility::default(),
            ))
            .id()
    }

    /// Mark the row's model built, as `update_display_models` does once its M2 lands.
    fn build_flag(app: &mut App) {
        let mut kits = app.world_mut().resource_mut::<ArtKits>();
        let dm = kits
            .models
            .get_mut("Flag.mdx")
            .expect("the entry was created");
        dm.parts = Some(Vec::new());
    }

    fn applied(app: &App, e: Entity) -> Option<(i8, usize, bool)> {
        let world = app.world();
        let a = world.get::<AppliedArtKit>(e)?;
        Some((
            a.kit,
            a.roots.len(),
            world.get::<ArtKitPending>(e).is_some(),
        ))
    }

    #[test]
    fn a_kit_waits_for_its_model_then_hangs_at_its_attachment_point() {
        let mut app = world();
        let e = object(&mut app, 0x0001_0601);
        app.update();
        assert_eq!(applied(&app, e), Some((1, 0, true)), "pending on the load");
        build_flag(&mut app);
        app.update();
        assert_eq!(applied(&app, e), Some((1, 1, false)));
        let root = app.world().get::<AppliedArtKit>(e).unwrap().roots[0];
        assert_eq!(
            app.world().get::<ChildOf>(root).map(|c| c.parent()),
            Some(e)
        );
        assert_eq!(
            app.world().get::<Transform>(root).unwrap().translation,
            Vec3::new(0.0, 3.0, 0.0),
            "the bone's bind pivot plus the attachment's offset"
        );
    }

    #[test]
    fn a_kit_change_takes_the_old_models_off_and_kit_zero_hangs_none() {
        let mut app = world();
        let e = object(&mut app, 0x0001_0601);
        app.update();
        build_flag(&mut app);
        app.update();
        let root = app.world().get::<AppliedArtKit>(e).unwrap().roots[0];
        app.world_mut().get_mut::<ObjectStore>(e).unwrap().0 =
            benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_GAMEOBJECT_BYTES_1,
                0x0000_0601,
            )]);
        app.update();
        assert_eq!(applied(&app, e), Some((0, 0, false)));
        assert!(
            app.world().get_entity(root).is_err(),
            "the old kit's model is gone"
        );
    }

    #[test]
    fn a_missing_attachment_or_a_negative_kit_hangs_nothing() {
        let mut app = world();
        app.world_mut()
            .resource_mut::<GameObjects>()
            .models
            .get_mut(&7)
            .unwrap()
            .attachments
            .clear();
        let e = object(&mut app, 0x0001_0601);
        app.update();
        build_flag(&mut app);
        app.update();
        assert_eq!(applied(&app, e), Some((1, 0, false)));
        let f = object(&mut app, 0x00FF_0601);
        app.update();
        assert_eq!(applied(&app, f), Some((-1, 0, false)));
    }
}
