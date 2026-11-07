//! Dormant placements: a world-static M2 placement past its fade band draws, ticks and animates
//! nothing, as the reference admits a doodad to the frame's worklist only inside its fade distance.
//! Its static-pass and merged batches, its collision hull, its lights and an interior prop's probe
//! slot stay; its entity phase (parts, cards, anim host and anchors, emitters, ribbons) exists only
//! while the camera is within the band's far edge, so a city's thousands of out-of-range placements
//! are records rather than rows in every per-entity pass. `WOW_DORMANT=0` spawns every placement
//! whole, as before.
//!
//! Deviation: a woken placement's animation and particles start fresh, where the reference's
//! frozen ones resume; the restart falls at the far edge of its fade band, where it is faint.

use std::sync::Arc;

use benilla_assets::M2Model;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use super::assemble::PlacementHost;
use super::fx::{spawn_emitters_for, spawn_ribbons_for};
use crate::interact::WorldObject;
use crate::model_render::ShadeSel;
use crate::particles::EmitterFade;
use crate::wmo_portal::WmoGroupVis;

/// Yards past the band's far edge a placement wakes at, ahead of its first visible frame.
pub(super) const WAKE_MARGIN: f32 = 4.0;
/// Yards past the far edge it sleeps at: wider than [`WAKE_MARGIN`], so a camera parked on the
/// edge cannot churn it.
pub(super) const SLEEP_MARGIN: f32 = 12.0;

/// On unless `WOW_DORMANT=0`.
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_DORMANT").as_deref() != Ok("0"))
}

/// Whether `eye` is within `margin` of the owner's fade band; a band-less owner is always near.
pub(super) fn near(eye: Vec3, fade: &EmitterFade, margin: f32) -> bool {
    crate::model_fade::fade_band(fade.size)
        .is_none_or(|(_, far)| fade.center.distance(eye) <= far + margin)
}

/// Whether a placement with this gate can sleep: one with a fade band, under the switch.
pub(super) fn sleepy(fade: &EmitterFade) -> bool {
    enabled() && crate::model_fade::fade_band(fade.size).is_some()
}

/// Whether a model has an entity phase to put to sleep: an entity batch, an emitter or a ribbon.
/// One drawn wholly by the static pass has none, and needs no record.
pub(super) fn has_entity_phase(entity_batches: &[bool], m: &M2Model) -> bool {
    entity_batches.iter().any(|&b| b) || !m.emitters.is_empty() || !m.ribbons.is_empty()
}

/// A dormant-capable placement's wake record: the spawn inputs its entity phase re-runs from.
pub(crate) struct Sleeper {
    /// Per batch, whether the first spawn made it an entity (or a stored card), not a divert.
    pub(super) mask: Vec<bool>,
    /// Whether the entity phase is live.
    pub(super) awake: bool,
    /// The entity phase's live entities, also in the placement's despawn list; empty asleep.
    pub(super) live: Vec<Entity>,
    pub(super) fade: EmitterFade,
    pub(super) shade: ShadeSel,
    pub(super) object: Arc<WorldObject>,
    pub(super) radius: f32,
    pub(super) center: Vec3,
    pub(super) anim_bound: Option<Aabb>,
    pub(super) interior_slot: Option<u16>,
}

/// One sleeper's flip test, flat: the step reads every row, and a city holds thousands of them.
pub(crate) struct DormantRow {
    center: Vec3,
    /// Squared yards it wakes inside and sleeps past; infinite for a band-less owner.
    wake: f32,
    sleep: f32,
    /// As the record last stood; the record's own flag decides.
    pub(super) awake: bool,
    pub(super) id: u32,
    /// The WMO prop's index in its placement, `None` for the placement's own model.
    pub(super) prop: Option<u32>,
}

impl DormantRow {
    fn new(id: u32, prop: Option<u32>, s: &Sleeper) -> Self {
        let edge = |margin: f32| {
            crate::model_fade::fade_band(s.fade.size)
                .map_or(f32::INFINITY, |(_, far)| (far + margin) * (far + margin))
        };
        Self {
            center: s.fade.center,
            wake: edge(WAKE_MARGIN),
            sleep: edge(SLEEP_MARGIN),
            awake: s.awake,
            id,
            prop,
        }
    }

    /// Whether the eye crossed this row's edge: [`near`]'s test, squared.
    fn flips(&self, eye: Vec3) -> bool {
        let d = self.center.distance_squared(eye);
        if self.awake {
            d > self.sleep
        } else {
            d <= self.wake
        }
    }
}

/// Every sleeper's [`DormantRow`], rebuilt only when a sleeper arrives or leaves or the bands move,
/// so a step scans these instead of chasing the placement records (~2 ms a step at Dalaran).
#[derive(Default)]
pub(crate) struct DormantTable {
    /// The spawn epoch, sleeper epoch and fade generation [`Self::rows`] was built under.
    key: Option<(u64, u64, u32)>,
    pub(super) rows: Vec<DormantRow>,
    /// This step's flipping rows.
    pub(super) due: Vec<u32>,
}

impl DormantTable {
    pub(super) fn refresh(
        &mut self,
        key: (u64, u64, u32),
        by_id: &std::collections::HashMap<u32, super::super::Placement>,
    ) {
        if self.key.replace(key) == Some(key) {
            return;
        }
        self.rows.clear();
        for (&id, p) in by_id {
            if let Some(s) = &p.sleeper {
                self.rows.push(DormantRow::new(id, None, s));
            }
            for (i, d) in p.doodads.iter().enumerate() {
                if let Some(s) = &d.sleeper {
                    self.rows.push(DormantRow::new(id, Some(i as u32), s));
                }
            }
        }
    }
}

/// Collect into `due` the rows the eye's move flipped.
pub(super) fn judge(rows: &[DormantRow], eye: Vec3, due: &mut Vec<u32>) {
    due.clear();
    due.extend(
        rows.iter()
            .enumerate()
            .filter_map(|(i, r)| r.flips(eye).then_some(i as u32)),
    );
}

/// How a placement's drawn entities are tagged for the exterior and portal culls.
pub(super) enum SceneTag {
    /// An ADT doodad: exterior scene.
    Exterior,
    /// A WMO prop named by rooms: its building's PVS key, and exterior with its building.
    Room(Entity, Arc<[u16]>),
    /// A prop no room names: untagged.
    None,
}

/// The entity phase's tail after its submeshes: the cull tags (the anim root skipped, as it has
/// no geometry), the emitters and ribbons, and the pick identity. `ents` gains the emitters.
#[allow(clippy::too_many_arguments)]
pub(super) fn entity_tail(
    commands: &mut Commands,
    ents: &mut Vec<Entity>,
    host: Option<&PlacementHost>,
    m: &M2Model,
    transform: Transform,
    fade: &EmitterFade,
    tag: &SceneTag,
    object: &Arc<WorldObject>,
) {
    let anim_root = host.map(|h| h.root);
    for &e in ents.iter().filter(|e| Some(**e) != anim_root) {
        match tag {
            SceneTag::Exterior => {
                commands
                    .entity(e)
                    .insert(crate::exterior_cull::ExteriorScene);
            }
            SceneTag::Room(instance, groups) => {
                commands.entity(e).insert((
                    WmoGroupVis {
                        instance: *instance,
                        groups: Arc::clone(groups),
                    },
                    crate::exterior_cull::ExteriorScene,
                ));
            }
            SceneTag::None => {}
        }
    }
    spawn_emitters_for(commands, &m.emitters, transform, host, fade, ents);
    spawn_ribbons_for(
        commands,
        &m.ribbons,
        transform,
        host,
        ents.first().copied(),
        fade,
    );
    for &e in ents.iter() {
        commands.entity(e).insert((**object).clone());
    }
}

/// Despawn a sleeper's live entities and drop them from the placement's despawn list.
pub(super) fn sleep(commands: &mut Commands, s: &mut Sleeper, entities: &mut Vec<Entity>) {
    s.awake = false;
    let gone: bevy::ecs::entity::EntityHashSet = s.live.drain(..).collect();
    for &e in &gone {
        commands.entity(e).try_despawn();
    }
    entities.retain(|e| !gone.contains(e));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(size: f32) -> EmitterFade {
        EmitterFade {
            radius: 1.0,
            size,
            center: Vec3::ZERO,
            instance: None,
            room: None,
        }
    }

    /// The flat judge flips a row exactly where [`near`] does, with the sleep edge's hysteresis.
    #[test]
    fn a_row_flips_where_near_does() {
        let (_, far) = crate::model_fade::fade_band(0.5).unwrap();
        let row = |awake| DormantRow {
            center: Vec3::ZERO,
            wake: (far + WAKE_MARGIN) * (far + WAKE_MARGIN),
            sleep: (far + SLEEP_MARGIN) * (far + SLEEP_MARGIN),
            awake,
            id: 7,
            prop: None,
        };
        for (x, margin) in [(far + 1.0, WAKE_MARGIN), (far + 9.0, WAKE_MARGIN)] {
            let eye = Vec3::new(x, 0.0, 0.0);
            assert_eq!(row(false).flips(eye), near(eye, &gate(0.5), margin));
        }
        for (x, margin) in [(far + 9.0, SLEEP_MARGIN), (far + 20.0, SLEEP_MARGIN)] {
            let eye = Vec3::new(x, 0.0, 0.0);
            assert_eq!(row(true).flips(eye), !near(eye, &gate(0.5), margin));
        }
        let mut due = Vec::new();
        let eye = Vec3::new(far + 9.0, 0.0, 0.0);
        judge(&[row(true), row(false)], eye, &mut due);
        assert!(due.is_empty(), "between the edges neither flips");
        judge(&[row(true), row(false)], Vec3::X * (far + 1.0), &mut due);
        assert_eq!(due, [1], "the sleeper near the band wakes");
    }

    /// Sleep despawns the entity phase and leaves the hull and lights in the list.
    #[test]
    fn sleep_despawns_the_awake_and_keeps_the_rest() {
        let mut world = World::new();
        let (a, b, kept) = (
            world.spawn_empty().id(),
            world.spawn_empty().id(),
            world.spawn_empty().id(),
        );
        let mut s = Sleeper {
            mask: vec![true],
            awake: true,
            live: vec![a, b],
            fade: gate(0.5),
            shade: ShadeSel::Matte,
            object: Arc::new(WorldObject {
                kind: crate::model_render::ModelKind::Doodad,
                label: String::new(),
                id: 1,
                detail: String::new(),
            }),
            radius: 1.0,
            center: Vec3::ZERO,
            anim_bound: None,
            interior_slot: None,
        };
        let mut entities = vec![a, kept, b];
        let mut queue = bevy::ecs::world::CommandQueue::default();
        sleep(
            &mut Commands::new(&mut queue, &world),
            &mut s,
            &mut entities,
        );
        queue.apply(&mut world);
        assert_eq!(entities, vec![kept]);
        assert!(!s.awake && s.live.is_empty(), "asleep: nothing live");
        assert!(world.get_entity(a).is_err() && world.get_entity(b).is_err());
        assert!(world.get_entity(kept).is_ok());
    }

    /// A 0.5 yd doodad fades out at 30 yd: it wakes inside 34 and sleeps past 42.
    #[test]
    fn a_placement_wakes_and_sleeps_on_its_band_with_hysteresis() {
        let g = gate(0.5);
        let at = |d: f32| Vec3::new(d, 0.0, 0.0);
        assert!(near(at(33.9), &g, WAKE_MARGIN));
        assert!(!near(at(34.1), &g, WAKE_MARGIN));
        assert!(near(at(41.9), &g, SLEEP_MARGIN), "awake, it holds to 42");
        assert!(!near(at(42.1), &g, SLEEP_MARGIN));
        let never = gate(f32::INFINITY);
        assert!(
            near(at(1e6), &never, WAKE_MARGIN),
            "a band-less owner never sleeps"
        );
    }
}
