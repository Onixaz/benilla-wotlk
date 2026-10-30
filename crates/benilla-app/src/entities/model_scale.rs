//! A unit's model scale, the DBC factor of its true scale (`0x71c0e0`): model scale × the `+0x9c`
//! modifier × `OBJECT_FIELD_SCALE_X`, times the mount display's column while mounted. The modifier
//! is 1.0 on every unit (only an effect or a dynamic object moves it), and the mount column
//! composes through the mount child's own transform ([`super::mount`]).
//!
//! The model scale is set from `UNIT_FIELD_DISPLAYID` at create (`0x73fce9`) and again, eased, on a
//! display change (`0x73e56e`), a level change (`0x72e415`), a pet number change (`0x72d025`) and
//! the creature record's arrival (`0x72ce4b`), which [`stamp_model_scales`] mirrors by recomputing
//! on any of those inputs. A corpse's is its display's `modelScale × creatureModelScale`
//! (`0x705ac0`); a game object has none (`0x4d5f90`).

use benilla_formats::CreatureFamilies;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::names::{CreatureRecord, NameCache};
use crate::net::{NetEntity, ObjectStore};
use crate::ui_pet_stats::PetFamilyTables;

use super::Creatures;

/// `CREATURE_TYPE_FLAG_USE_MODEL_COLLISION_SIZE`: an NPC sizes its collision box off its model;
/// any other takes the default box (`0x725f82`).
const USE_MODEL_COLLISION: u32 = 0x2_0000;

/// A unit's model scales: the render factor of its true scale and its collision box's factor.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModelScale {
    /// `[unit+0xb3c]`, of the rendered display (`0x722ae0`).
    pub(crate) render: f32,
    /// The native display's model scale, the collision box's factor (`0x726053`); `None` for an
    /// NPC whose template lacks [`USE_MODEL_COLLISION`] (or is not yet queried), which takes the
    /// default box.
    pub(crate) collision: Option<f32>,
}

/// The true scale's factors short of the mount: model scale × `SCALE_X` (`0x71c0e0`).
pub(crate) fn true_scale(net: &NetEntity, model: Option<&ModelScale>) -> f32 {
    model.map_or(1.0, |m| m.render) * net.scale
}

/// A unit's family term (`0x71c171`): its template's `CreatureFamily` ramp at its level; `None`
/// without a record, family 0 or a missing row.
fn family_scale(
    record: Option<&CreatureRecord>,
    families: Option<&CreatureFamilies>,
    level: u32,
) -> Option<f32> {
    families?.unit_scale(record?.pet_family, level)
}

/// One entity's [`ModelScale`]; `None` for a kind that has none.
fn model_scale_of(
    creatures: &Creatures,
    families: Option<&CreatureFamilies>,
    names: Option<&NameCache>,
    net: &NetEntity,
    store: &ObjectStore,
) -> Option<ModelScale> {
    let s = &store.0;
    match net.kind {
        EntityKind::Corpse => {
            let render = net
                .display_id
                .and_then(|d| creatures.model_scale(d))
                .unwrap_or(1.0);
            Some(ModelScale {
                render,
                collision: None,
            })
        }
        EntityKind::Unit | EntityKind::Player => {
            let display = s
                .unit_displayid()
                .and_then(|d| u32::try_from(d).ok())
                .filter(|&d| d != 0)
                .or(net.display_id);
            let native = s
                .unit_native_displayid()
                .and_then(|d| u32::try_from(d).ok())
                .filter(|&d| d != 0)
                .or(display);
            // A player has no creature record (`[unit+0x964]`), so no family term and no gate.
            let record = (net.kind == EntityKind::Unit)
                .then(|| s.object_entry().zip(names))
                .flatten()
                .and_then(|(entry, n)| n.creature_record(entry));
            let family = family_scale(record, families, s.unit_level().unwrap_or(0));
            let is_pet = s.unit_pet_number() != 0;
            let scale_of = |d: Option<u32>| {
                d.map_or(1.0, |d| {
                    creatures.catalog.unit_model_scale(d, family, is_pet)
                })
            };
            let model_box = net.kind == EntityKind::Player
                || record.is_some_and(|r| r.type_flags & USE_MODEL_COLLISION != 0);
            Some(ModelScale {
                render: scale_of(display),
                collision: model_box.then(|| scale_of(native)),
            })
        }
        _ => None,
    }
}

/// Stamp and restamp each unit's [`ModelScale`] when an input moves: its descriptor (display,
/// native display, level, pet number), its [`NetEntity`], the creature records or the tables.
#[allow(clippy::type_complexity)]
pub(super) fn stamp_model_scales(
    mut commands: Commands,
    creatures: Option<Res<Creatures>>,
    families: Option<Res<PetFamilyTables>>,
    names: Option<Res<NameCache>>,
    mut seen_names: Local<u64>,
    mut units: Query<(
        Entity,
        Ref<NetEntity>,
        Ref<ObjectStore>,
        Option<&mut ModelScale>,
    )>,
) {
    let Some(creatures) = creatures else {
        return;
    };
    let names_moved = names
        .as_ref()
        .is_some_and(|n| std::mem::replace(&mut *seen_names, n.generation()) != n.generation());
    let all =
        names_moved || creatures.is_added() || families.as_ref().is_some_and(|f| f.is_added());
    let families = families.as_deref().map(|f| &f.families);
    for (entity, net, store, current) in &mut units {
        if !all && current.is_some() && !net.is_changed() && !store.is_changed() {
            continue;
        }
        let Some(scale) = model_scale_of(&creatures, families, names.as_deref(), &net, &store)
        else {
            continue;
        };
        match current {
            Some(mut c) => {
                c.set_if_neq(scale);
            }
            None => {
                commands.entity(entity).insert(scale);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(scale: f32) -> NetEntity {
        NetEntity {
            kind: EntityKind::Unit,
            display_id: Some(1),
            scale,
        }
    }

    /// `0x71c0e0`: the model scale multiplies `SCALE_X`; without a stamp the unit is `SCALE_X`.
    #[test]
    fn the_true_scale_is_the_model_scale_times_scale_x() {
        let m = ModelScale {
            render: 0.1,
            collision: None,
        };
        assert!((true_scale(&net(9.86), Some(&m)) - 0.986).abs() < 1e-6);
        assert_eq!(true_scale(&net(1.0), None), 1.0);
        let lich = ModelScale {
            render: 2.5,
            collision: Some(2.5),
        };
        assert_eq!(true_scale(&net(1.0), Some(&lich)), 2.5);
    }

    /// The family term needs a record naming a family; a missing table or family 0 has none.
    #[test]
    fn the_family_term_needs_a_record_and_a_family() {
        let rec = CreatureRecord {
            pet_family: 1,
            ..Default::default()
        };
        assert_eq!(family_scale(Some(&rec), None, 80), None);
        assert_eq!(
            family_scale(None, Some(&CreatureFamilies::default()), 80),
            None
        );
        let none = CreatureRecord::default();
        assert_eq!(
            family_scale(Some(&none), Some(&CreatureFamilies::default()), 80),
            None
        );
    }
}
