//! Stored billboard cards: a world-static placement's fading cards wait as records on one holder
//! entity and spawn as ordinary card entities only while the camera is inside their fade range, so
//! a city's thousands of out-of-range glows stay out of every per-entity pass. A live card is the
//! entity the spawn site would have made, so it faces, fades, sorts and draws exactly as one.
//! `WOW_STORED_CARDS=0` spawns every card at placement time, as before.

use std::sync::Arc;

use benilla_assets::materials::WowModelMaterial;
use benilla_formats::RenderSubmesh;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::billboard::BillboardCard;
use crate::model_fade::DoodadFade;
use crate::model_render::{M2BatchSort, ModelPart};

/// Yards past the band's far edge a card spawns at, so it exists before its first visible frame.
const SPAWN_MARGIN: f32 = 2.0;
/// Yards past the far edge a live card despawns at: wider than [`SPAWN_MARGIN`], so a camera
/// parked on the edge cannot churn it.
const DESPAWN_MARGIN: f32 = 6.0;

/// On unless `WOW_STORED_CARDS=0`.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_STORED_CARDS").as_deref() != Ok("0"))
}

/// One stored card: the bundle the spawn site makes for a world-static billboard batch.
#[derive(Clone)]
pub struct StoredCard {
    pub mesh: Handle<Mesh>,
    pub material: Handle<WowModelMaterial>,
    pub part: ModelPart,
    pub why: &'static str,
    pub pick: Arc<RenderSubmesh>,
    pub tag: MeshTag,
    pub card: BillboardCard,
    pub sort: Option<M2BatchSort>,
    pub aabb: Option<Aabb>,
    /// The card's world pivot, where it sits and where its fade is measured from.
    pub pivot: Vec3,
    pub fade: DoodadFade,
    /// The tag carries an interior prop's probe slot (`crate::mesh_tag::InteriorProbePayload`).
    pub probe: bool,
    /// `crate::doodad_anim::AnimMatPart`: a scrolling or tinting material the scan must visit.
    pub anim_mat: bool,
}

impl StoredCard {
    /// Spawn the card as the spawn site's own entity.
    pub fn spawn<'a>(&self, commands: &'a mut Commands) -> EntityCommands<'a> {
        let mut e = commands.spawn((
            Mesh3d(self.mesh.clone()),
            MeshMaterial3d(self.material.clone()),
            Transform::from_translation(self.pivot),
            self.part,
            crate::model_render::EntityPathWhy(self.why),
            crate::interact::PickMesh(self.pick.clone()),
            MeshTag(self.tag.0),
            self.card.clone(),
            self.fade.clone(),
        ));
        if let Some(sort) = self.sort {
            e.insert(sort);
        }
        // The build-time bound, as at the spawn site: `calculate_bounds` can race extraction.
        if let Some(aabb) = self.aabb {
            e.insert((aabb, NoAutoAabb));
        }
        if self.probe {
            e.insert(crate::mesh_tag::InteriorProbePayload);
        }
        if self.anim_mat {
            e.insert(crate::doodad_anim::AnimMatPart);
        }
        e
    }

    /// The fade band's far edge in yards at the live `environmentDetail`; `None` never fades out.
    fn far(&self) -> Option<f32> {
        crate::model_fade::fade_band(self.fade.size).map(|(_, far)| far)
    }
}

/// A placement's stored cards, on an entity in its spawn list, so they go with the placement; the
/// spawn site's tags on it (`ExteriorScene`, `WorldObject`, a prop's `WmoGroupVis`) are copied
/// onto each live card.
#[derive(Component)]
#[component(on_remove = despawn_live_cards)]
pub struct CardHolder {
    cards: Vec<StoredCard>,
    live: Vec<Option<Entity>>,
    /// The cards' pivots' centre and the farthest pivot-plus-reach from it, the holder's coarse
    /// test; refreshed when `environmentDetail` moves the bands.
    center: Vec3,
    reach: f32,
    generation: u32,
}

impl CardHolder {
    pub fn new(cards: Vec<StoredCard>) -> Self {
        let live = vec![None; cards.len()];
        let mut h = Self {
            cards,
            live,
            center: Vec3::ZERO,
            reach: 0.0,
            generation: u32::MAX,
        };
        h.refresh_reach();
        h
    }

    fn refresh_reach(&mut self) {
        let n = self.cards.len().max(1) as f32;
        self.center = self.cards.iter().map(|c| c.pivot).sum::<Vec3>() / n;
        self.reach = self
            .cards
            .iter()
            .map(|c| c.pivot.distance(self.center) + c.far().unwrap_or(f32::INFINITY))
            .fold(0.0, f32::max);
        self.generation = crate::model_fade::fade_generation();
    }
}

/// A placement's despawn takes its live cards with it.
fn despawn_live_cards(mut world: DeferredWorld, ctx: HookContext) {
    let live: Vec<Entity> = world
        .get::<CardHolder>(ctx.entity)
        .map(|h| h.live.iter().flatten().copied().collect())
        .unwrap_or_default();
    let mut commands = world.commands();
    for e in live {
        commands.entity(e).try_despawn();
    }
}

/// Spawn each stored card whose pivot the camera is within its fade range of, and despawn a live
/// one the camera has left, before the model-visibility authority judges this frame's set.
#[allow(clippy::type_complexity)]
pub fn admit_stored_cards(
    mut commands: Commands,
    cam: Query<Ref<GlobalTransform>, With<crate::view::WorldCamera>>,
    mut holders: Query<(
        &mut CardHolder,
        Option<&crate::wmo_portal::WmoGroupVis>,
        Has<crate::exterior_cull::ExteriorScene>,
        Option<&crate::interact::WorldObject>,
    )>,
    mut last_generation: Local<u32>,
) {
    let Ok(cam) = cam.single() else {
        return;
    };
    let generation = crate::model_fade::fade_generation();
    let bands_moved = *last_generation != generation;
    *last_generation = generation;
    let cam_moved = cam.is_changed();
    let eye = cam.translation();
    for (mut holder, room, exterior, object) in &mut holders {
        // A still camera over unchanged bands leaves every verdict standing, except a new holder's.
        if !cam_moved && !bands_moved && !holder.is_added() {
            continue;
        }
        if holder.generation != generation {
            holder.bypass_change_detection().refresh_reach();
        }
        let any_live = holder.live.iter().any(Option::is_some);
        if !any_live && eye.distance(holder.center) > holder.reach + SPAWN_MARGIN {
            continue;
        }
        let h = holder.bypass_change_detection();
        for (card, live) in h.cards.iter().zip(h.live.iter_mut()) {
            let d = eye.distance(card.pivot);
            let far = card.far().unwrap_or(f32::INFINITY);
            match *live {
                None if d <= far + SPAWN_MARGIN => {
                    let mut e = card.spawn(&mut commands);
                    if let Some(room) = room {
                        e.insert(room.clone());
                    }
                    if exterior {
                        e.insert(crate::exterior_cull::ExteriorScene);
                    }
                    if let Some(object) = object {
                        e.insert(object.clone());
                    }
                    *live = Some(e.id());
                }
                Some(e) if d > far + DESPAWN_MARGIN => {
                    commands.entity(e).try_despawn();
                    *live = None;
                }
                _ => {}
            }
        }
    }
}

/// Registers the admission pass on this frame's camera pose and ahead of the model-visibility
/// authority, so a card spawned this frame takes its fade alpha and verdict before it can draw.
pub(crate) fn plugin(app: &mut App) {
    if enabled() {
        app.add_systems(
            Update,
            admit_stored_cards
                .after(crate::view::publish_camera_pose)
                .before(crate::model_render::ModelVisSet),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(pivot: Vec3) -> StoredCard {
        StoredCard {
            mesh: Handle::default(),
            material: Handle::default(),
            part: ModelPart {
                kind: crate::model_render::ModelKind::Doodad,
                blend: benilla_formats::ModelBlend::Blend,
            },
            why: "test",
            pick: Arc::new(RenderSubmesh::default()),
            tag: MeshTag(0),
            card: BillboardCard::new(
                &benilla_assets::BillboardInfo {
                    pivot: Vec3::ZERO,
                    kind: benilla_formats::BillboardKind::Spherical,
                    bone: 0,
                    scale_anim: None,
                    seq_translations: Vec::new(),
                },
                Transform::from_translation(pivot),
            ),
            sort: None,
            aabb: None,
            pivot,
            // A 0.5 yd card: fade level 0, the 25..30 yd band.
            fade: DoodadFade {
                size: 0.5,
                local_center: Vec3::ZERO,
                cutout: Handle::default(),
                blend: Handle::default(),
            },
            probe: false,
            anim_mat: false,
        }
    }

    /// A world with the camera at `eye`, and the pass registered once, so its change ticks carry
    /// across runs as the schedule's do.
    fn world_at(eye: Vec3) -> (World, bevy::ecs::system::SystemId) {
        let mut world = World::new();
        world.spawn((
            crate::view::WorldCamera,
            GlobalTransform::from_translation(eye),
        ));
        let pass = world.register_system(admit_stored_cards);
        (world, pass)
    }

    fn move_camera(world: &mut World, eye: Vec3) {
        let mut q = world.query_filtered::<&mut GlobalTransform, With<crate::view::WorldCamera>>();
        *q.single_mut(world).unwrap() = GlobalTransform::from_translation(eye);
    }

    fn live_cards(world: &mut World) -> usize {
        world
            .query_filtered::<(), With<BillboardCard>>()
            .iter(world)
            .count()
    }

    /// A stored card exists only inside its fade range (with the spawn margin), carries its
    /// holder's tags, survives the hysteresis band, and goes when the camera leaves it.
    #[test]
    fn a_stored_card_lives_only_inside_its_fade_range() {
        let (mut world, pass) = world_at(Vec3::new(100.0, 0.0, 0.0));
        let holder = world
            .spawn((
                CardHolder::new(vec![stored(Vec3::ZERO)]),
                crate::exterior_cull::ExteriorScene,
            ))
            .id();
        world.run_system(pass).unwrap();
        assert_eq!(live_cards(&mut world), 0, "100 yd out: a record only");

        move_camera(&mut world, Vec3::new(31.0, 0.0, 0.0));
        world.run_system(pass).unwrap();
        assert_eq!(live_cards(&mut world), 1, "31 yd: inside 30 + margin");
        let tagged = world
            .query_filtered::<(), (
                With<BillboardCard>,
                With<crate::exterior_cull::ExteriorScene>,
            )>()
            .iter(&world)
            .count();
        assert_eq!(tagged, 1, "the holder's tags ride the live card");

        move_camera(&mut world, Vec3::new(34.0, 0.0, 0.0));
        world.run_system(pass).unwrap();
        assert_eq!(
            live_cards(&mut world),
            1,
            "34 yd: inside the despawn margin"
        );

        move_camera(&mut world, Vec3::new(40.0, 0.0, 0.0));
        world.run_system(pass).unwrap();
        assert_eq!(live_cards(&mut world), 0, "40 yd: gone");

        move_camera(&mut world, Vec3::new(10.0, 0.0, 0.0));
        world.run_system(pass).unwrap();
        assert_eq!(live_cards(&mut world), 1);
        world.despawn(holder);
        world.flush();
        assert_eq!(
            live_cards(&mut world),
            0,
            "the placement takes its live cards"
        );
    }

    /// A still camera skips judged holders, but a new holder is judged on its first frame.
    #[test]
    fn a_new_holder_is_judged_under_a_still_camera() {
        let (mut world, pass) = world_at(Vec3::new(5.0, 0.0, 0.0));
        world.run_system(pass).unwrap();
        world.run_system(pass).unwrap(); // the camera's spawn mark is now old
        world.spawn(CardHolder::new(vec![stored(Vec3::ZERO)]));
        world.run_system(pass).unwrap();
        assert_eq!(live_cards(&mut world), 1);
    }
}
