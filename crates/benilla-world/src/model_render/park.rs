//! Parking hidden world submeshes: a model part hidden for [`PARK_AFTER_FRAMES`] puts its `Mesh3d`
//! down, leaving every Bevy per-`Mesh3d` sweep, and picks it up once its root is shown again.
//! This runs in `Update` on last frame's verdict, so a re-shown part draws a frame after its
//! election but its `Mesh3d` is back before specialization and the visibility check: inserted
//! after the visibility check on a visible entity, it trips Bevy's `specialize_material_meshes`
//! tick unwrap. Parts on a render layer of their own (a portrait booth's, whose bake window is
//! four frames) never park.

use std::any::TypeId;

use bevy::camera::visibility::VisibilityClass;
use bevy::ecs::world::EntityWorldMut;
use bevy::prelude::*;

use super::ModelPart;

/// Consecutive hidden frames before a part parks, half a second at 60 Hz, so a glance costs
/// nothing.
const PARK_AFTER_FRAMES: u16 = 30;

/// The mesh a parked part put down, restored verbatim on unpark.
#[derive(Component)]
// Sparse: parking toggles it, so it splits no table.
#[component(storage = "SparseSet")]
pub struct ParkedMesh(pub Handle<Mesh>);

/// The run on which this part last went hidden (its own or an ancestor's verdict).
#[derive(Component)]
pub struct HiddenSince(pub u32);

/// Parks the long-hidden and unparks the shown, by last `PostUpdate`'s propagated
/// `InheritedVisibility`. Only flips are read (a city holds ~25k parts): a hide stamps its run
/// and queues the deadline, which parks the part if it is still hidden under that stamp.
#[allow(clippy::type_complexity)]
pub(super) fn park_hidden_parts(
    mut commands: Commands,
    mut run: Local<u32>,
    mut due: Local<std::collections::VecDeque<(Entity, u32)>>,
    mut parts: ParamSet<(
        Query<
            (
                Entity,
                &InheritedVisibility,
                Option<&mut HiddenSince>,
                Option<&ParkedMesh>,
            ),
            (
                With<ModelPart>,
                Without<bevy::camera::visibility::RenderLayers>,
                Or<(Changed<InheritedVisibility>, Added<ModelPart>)>,
            ),
        >,
        Query<
            (&InheritedVisibility, &HiddenSince, Option<&Mesh3d>),
            (
                With<ModelPart>,
                Without<bevy::camera::visibility::RenderLayers>,
            ),
        >,
    )>,
) {
    *run += 1;
    let now = *run;
    for (entity, inherited, since, parked) in &mut parts.p0() {
        if inherited.get() {
            if let Some(parked) = parked {
                commands
                    .entity(entity)
                    .insert(Mesh3d(parked.0.clone()))
                    .remove::<ParkedMesh>()
                    // Bevy's `Mesh3d` add hook pushes a `VisibilityClass` entry on every re-add
                    // and `check_visibility` queues an entity once per entry, so each cycle would
                    // draw the part once more; dedup after the hook has run.
                    .queue(dedup_visibility_class);
            }
            continue;
        }
        // In place where it exists, so a stale deadline below already reads the new stamp.
        match since {
            Some(mut s) => s.0 = now,
            None => {
                commands.entity(entity).insert(HiddenSince(now));
            }
        }
        due.push_back((entity, now + u32::from(PARK_AFTER_FRAMES) - 1));
    }
    let waiting = parts.p1();
    while let Some(&(entity, at)) = due.front() {
        if at > now {
            break;
        }
        due.pop_front();
        let Ok((inherited, since, mesh)) = waiting.get(entity) else {
            continue; // despawned
        };
        if inherited.get() || since.0 + u32::from(PARK_AFTER_FRAMES) - 1 != at {
            continue; // shown since, or re-hidden under a later stamp
        }
        if let Some(mesh) = mesh {
            commands
                .entity(entity)
                .insert(ParkedMesh(mesh.0.clone()))
                .remove::<Mesh3d>();
        }
    }
}

/// Leaves one entry per class in an entity's [`VisibilityClass`].
fn dedup_visibility_class(mut e: EntityWorldMut) {
    if let Some(mut class) = e.get_mut::<VisibilityClass>() {
        let mut seen: Vec<TypeId> = Vec::with_capacity(class.len());
        class.retain(|t| {
            if seen.contains(t) {
                false
            } else {
                seen.push(*t);
                true
            }
        });
    }
}

/// `WOW_NO_MESH_PARK=1` turns parking off.
pub(super) fn enabled() -> bool {
    std::env::var_os("WOW_NO_MESH_PARK").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (World, Entity) {
        let mut w = World::new();
        let e = w
            .spawn((
                ModelPart {
                    kind: super::super::ModelKind::Doodad,
                    blend: benilla_formats::ModelBlend::Opaque,
                },
                Mesh3d(Handle::default()),
                InheritedVisibility::HIDDEN,
            ))
            .id();
        (w, e)
    }

    /// One system across a test's runs: its counter and deadline queue are `Local`s.
    struct Parker(Box<dyn System<In = (), Out = ()>>);

    impl Parker {
        fn new(w: &mut World) -> Self {
            let mut sys: Box<dyn System<In = (), Out = ()>> =
                Box::new(IntoSystem::into_system(park_hidden_parts));
            sys.initialize(w);
            Self(sys)
        }

        fn run(&mut self, w: &mut World, n: usize) {
            for _ in 0..n {
                self.0.run((), w).unwrap();
                self.0.apply_deferred(w);
            }
        }
    }

    #[test]
    fn a_part_parks_after_the_hysteresis_and_unparks_when_shown() {
        let (mut w, e) = world();
        let mut p = Parker::new(&mut w);
        p.run(&mut w, PARK_AFTER_FRAMES as usize - 1);
        assert!(w.entity(e).contains::<Mesh3d>(), "not yet");
        p.run(&mut w, 1);
        assert!(!w.entity(e).contains::<Mesh3d>(), "parked");
        assert!(w.entity(e).contains::<ParkedMesh>());
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        p.run(&mut w, 1);
        assert!(
            w.entity(e).contains::<Mesh3d>(),
            "unparked on the shown frame"
        );
        assert!(!w.entity(e).contains::<ParkedMesh>());
    }

    /// Only flips are written: a part hidden for good is stamped once, however long it stays.
    #[test]
    fn a_still_hidden_part_is_written_once() {
        let (mut w, e) = world();
        let mut p = Parker::new(&mut w);
        p.run(&mut w, 1);
        let stamped = w.entity(e).get_ref::<HiddenSince>().unwrap().last_changed();
        p.run(&mut w, PARK_AFTER_FRAMES as usize * 2);
        assert!(w.entity(e).contains::<ParkedMesh>(), "parked");
        assert_eq!(
            w.entity(e).get_ref::<HiddenSince>().unwrap().last_changed(),
            stamped,
            "a still hidden part was rewritten"
        );
    }

    /// The world registers the `Mesh3d` hook and required component as Bevy's plugin does.
    #[test]
    fn an_unparked_part_keeps_one_visibility_class_entry() {
        let mut w = World::new();
        w.register_required_components::<Mesh3d, VisibilityClass>();
        w.register_component_hooks::<Mesh3d>()
            .on_add(bevy::camera::visibility::add_visibility_class::<Mesh3d>);
        let e = w
            .spawn((
                ModelPart {
                    kind: super::super::ModelKind::Doodad,
                    blend: benilla_formats::ModelBlend::Blend,
                },
                Mesh3d(Handle::default()),
                InheritedVisibility::HIDDEN,
            ))
            .id();
        assert_eq!(w.entity(e).get::<VisibilityClass>().unwrap().len(), 1);
        let mut p = Parker::new(&mut w);
        p.run(&mut w, PARK_AFTER_FRAMES as usize);
        assert!(!w.entity(e).contains::<Mesh3d>(), "parked");
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        p.run(&mut w, 1);
        assert!(w.entity(e).contains::<Mesh3d>(), "unparked");
        assert_eq!(
            w.entity(e).get::<VisibilityClass>().unwrap().len(),
            1,
            "the re-added Mesh3d's hook pushed a second class entry — the part draws twice"
        );
    }

    #[test]
    fn a_glance_never_parks() {
        let (mut w, e) = world();
        let mut p = Parker::new(&mut w);
        p.run(&mut w, 10);
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        p.run(&mut w, 1);
        w.entity_mut(e).insert(InheritedVisibility::HIDDEN);
        p.run(&mut w, 10);
        assert!(w.entity(e).contains::<Mesh3d>(), "the count restarted");
    }
}
