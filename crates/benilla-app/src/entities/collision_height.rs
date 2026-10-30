//! The per-unit collision height ([`CollisionHeight`]), the movement object's `+0xb4`, derived
//! from the unit's display id and stamped on every streamed entity.

use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};

use super::model_scale::ModelScale;
use super::Creatures;

/// A unit's collision height in yards, the movement object's `+0xb4`: the `h` whose fractions
/// are the depth lines (swim `0.75·h`, splash `0.4·h`, foam gate `max(2·h, 1.0)`). It is
/// `CreatureModelData.collisionHeight`, the MD20 collision box's Z extent in model units, times the
/// native display's model scale and `max(SCALE_X, 1)` (`0x726061`, a player's `0x6cf350`). An NPC
/// whose template lacks the model-collision type flag takes the default box (`0x725fb8`).
///
/// The display is the native one (`UNIT_FIELD_NATIVEDISPLAYID`), so a transform keeps the unit's
/// own depth lines. Not the movement capsule, [`crate::player::CAPSULE_HEIGHT`].
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollisionHeight(pub(crate) f32);

impl Default for CollisionHeight {
    /// The movement object's constructor default, never `0.0`, at which every depth line collapses
    /// and the unit swims on dry land.
    fn default() -> Self {
        Self(crate::player::DEFAULT_COLLISION_HEIGHT)
    }
}

/// The display the collision height derives from: `UNIT_FIELD_NATIVEDISPLAYID`, else the rendered
/// display when that is absent or `0` (no descriptor block yet, a synthetic mount child).
pub(crate) fn prism_display_id(store: Option<&ObjectStore>, rendered: Option<u32>) -> Option<u32> {
    store
        .and_then(|s| s.0.unit_native_displayid())
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0)
        .or(rendered)
}

/// [`CollisionHeight`]'s one derivation, shared by the first stamp and the live restamp
/// ([`super::live_display::refresh_live_display`]) so the two cannot disagree.
/// An unstamped entity (a mount child, which has no descriptor) keeps the model box at scale 1.
pub(crate) fn collision_height_for(
    creatures: Option<&Creatures>,
    display_id: Option<u32>,
    scale: f32,
    model: Option<&ModelScale>,
) -> CollisionHeight {
    let Some(model_scale) = model.map_or(Some(1.0), |m| m.collision) else {
        return CollisionHeight::default();
    };
    let raw = creatures
        .zip(display_id)
        .and_then(|(c, id)| c.collision_height(id))
        // A zero column (67 rows: invisible triggers and the like) means no authored box, so it
        // falls back like a missing row.
        .filter(|h| *h > 0.0)
        .unwrap_or(crate::player::DEFAULT_COLLISION_HEIGHT);
    CollisionHeight(raw * prism_scale(scale, model_scale))
}

/// `model scale × max(SCALE_X, 1)`, the box's scale (`0x726030`). Never 0, at which every depth
/// line collapses.
fn prism_scale(scale: f32, model_scale: f32) -> f32 {
    (model_scale * scale.max(1.0)).max(f32::MIN_POSITIVE)
}

/// Stamp each streamed unit's first [`CollisionHeight`], at the ctor default when its display
/// misses the DBCs; only the unstamped are queried, so it heals if `Creatures` loads late.
#[allow(clippy::type_complexity)] // one query: the unstamped units and their two optional inputs
pub(super) fn stamp_collision_heights(
    mut commands: Commands,
    creatures: Option<Res<Creatures>>,
    units: Query<
        (
            Entity,
            &NetEntity,
            Option<&ObjectStore>,
            Option<&ModelScale>,
        ),
        Without<CollisionHeight>,
    >,
) {
    for (entity, net, store, model) in &units {
        let display = prism_display_id(store, net.display_id);
        let h = collision_height_for(creatures.as_deref(), display, net.scale, model);
        commands.entity(entity).insert(h);
    }
}

#[cfg(test)]
mod native_display {
    use super::prism_display_id;
    use crate::net::ObjectStore;
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_NATIVEDISPLAYID`, between DISPLAYID (131) and MOUNTDISPLAYID (133).
    const NATIVE: u16 = 132;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(pairs))
    }

    /// A druid in bear form: rendered display 2281, native still the night elf's 55.
    #[test]
    fn a_shapeshifted_unit_keeps_its_native_display() {
        let shifted = store(&[(NATIVE, 55)]);
        assert_eq!(prism_display_id(Some(&shifted), Some(2281)), Some(55));
    }

    #[test]
    fn an_absent_or_zero_native_falls_back_to_the_rendered_display() {
        assert_eq!(prism_display_id(None, Some(2281)), Some(2281));
        let empty = store(&[]);
        assert_eq!(prism_display_id(Some(&empty), Some(2281)), Some(2281));
        let zeroed = store(&[(NATIVE, 0)]);
        assert_eq!(prism_display_id(Some(&zeroed), Some(2281)), Some(2281));
        assert_eq!(prism_display_id(Some(&empty), None), None);
    }

    #[test]
    fn an_unshifted_unit_is_unaffected() {
        let plain = store(&[(NATIVE, 4945)]);
        assert_eq!(prism_display_id(Some(&plain), Some(4945)), Some(4945));
    }
}

#[cfg(test)]
mod prism {
    use super::prism_scale;

    /// `0x726030`: the model scale times `SCALE_X` floored at 1.
    #[test]
    fn the_box_scale_is_the_model_scale_times_scale_x_floored_at_one() {
        // The Shore Strider (display 4945): model scale 1.75 at `SCALE_X` 1.
        assert_eq!(prism_scale(1.0, 1.75), 1.75);
        // A shrink under 1 is floored: the box keeps the model's size.
        assert_eq!(prism_scale(0.5, 1.75), 1.75);
        // A growth above 1 multiplies.
        assert_eq!(prism_scale(2.0, 1.75), 3.5);
        assert!(prism_scale(0.0, 0.0) > 0.0, "never collapses");
    }

    /// An NPC without the model-collision flag takes the default box whatever its model.
    #[test]
    fn an_npc_without_the_model_box_takes_the_default() {
        let npc = super::ModelScale {
            render: 3.0,
            collision: None,
        };
        let h = super::collision_height_for(None, Some(4945), 2.0, Some(&npc));
        assert_eq!(h, super::CollisionHeight::default());
        let player = super::ModelScale {
            render: 1.0,
            collision: Some(1.0),
        };
        let h = super::collision_height_for(None, None, 1.0, Some(&player));
        assert_eq!(h.0, crate::player::DEFAULT_COLLISION_HEIGHT);
    }
}
