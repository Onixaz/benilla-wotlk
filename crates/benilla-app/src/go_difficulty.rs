//! A difficulty portal's two objects (GameObject type 31): each draws only at its own difficulty
//! (`0x710a50`, run from the object's display update), at the type's base alpha (`0x70dd50`).

use benilla_assets::MapCatalogRes;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::net::Guid;
use crate::ui_party::GroupState;
use crate::ui_transfer::MapDifficulties;
use benilla_world::schedule::WorldStage;
use benilla_world::world_map::CurrentMap;
use benilla_world::world_unit::ObjectHidden;

/// `GAMEOBJECT_TYPE_DUNGEON_DIFFICULTY`.
pub(crate) const GO_TYPE_DIFFICULTY: i32 = 31;

/// `GAMEOBJECT_DYNAMIC` bit that makes the type opaque (`0x70dd59`).
const GO_DYNFLAG_OPAQUE: u32 = 0x2;

/// `Map.dbc` `InstanceType` of a raid (`[map+0x8]`, `0x710aec`).
const INSTANCE_TYPE_RAID: u32 = 2;

/// The type's base alpha (`0x70dd50`): 1.0 with the opaque bit, else 0.5 (`0x9e2ec4`).
pub(crate) fn base_alpha(dynamic_flags: u32) -> f32 {
    if dynamic_flags & GO_DYNFLAG_OPAQUE != 0 {
        1.0
    } else {
        0.5
    }
}

/// A type 31 object, stepped each frame like the reference's display update.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct DifficultyObject;

/// The template's slots, `data[0]` (property `0x57`) and `data[1]` (property `0x58`); a template
/// not cached yet reads zero in both, as `0x710a7c` and `0x710b88` do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DifficultySlots {
    pub(crate) map: u32,
    pub(crate) difficulty: u32,
}

/// The `Map.dbc` row the object names, as the branch reads it.
#[derive(Clone, Copy, Debug, Default)]
struct PortalMap {
    instance_type: Option<u32>,
    /// `[map+0xc] & 0x100` (`0x710af2`).
    dynamic: bool,
}

/// The difficulties the branches compare with.
#[derive(Clone, Copy, Debug, Default)]
struct Current {
    /// `0x5138d0`.
    dungeon: u32,
    /// `0x513900`.
    raid: u32,
    /// `0x519680`.
    effective_raid: u32,
}

/// The show test of `0x710a50`; `has_current_raid_row` is the map's row at the current raid
/// difficulty (`0x634950`).
fn shown(
    slots: DifficultySlots,
    map: PortalMap,
    current: Current,
    has_current_raid_row: impl FnOnce() -> bool,
) -> bool {
    if slots.map == 0 || map.instance_type != Some(INSTANCE_TYPE_RAID) {
        return current.dungeon == slots.difficulty;
    }
    if map.dynamic {
        return current.effective_raid == slots.difficulty;
    }
    current.raid == slots.difficulty
        || (current.raid >= 2 && current.raid - 2 == slots.difficulty && !has_current_raid_row())
}

/// The reference's display update for every type 31 object: the hidden bit from the current
/// difficulty (`0x77f2f0`); [`ObjectHidden`] also stops its sound (`0x710ba8`).
pub(crate) fn step_difficulty_objects(
    group: Res<GroupState>,
    templates: Res<GameObjectTemplates>,
    maps: Option<Res<MapCatalogRes>>,
    map_difficulties: Res<MapDifficulties>,
    here: Option<Res<CurrentMap>>,
    objects: Query<(Entity, &Guid, Has<ObjectHidden>), With<DifficultyObject>>,
    mut commands: Commands,
) {
    if objects.is_empty() {
        return;
    }
    let catalog = maps.as_deref().map(|m| &m.0);
    let here_dynamic = here.is_some_and(|m| catalog.is_some_and(|c| c.has_dynamic_difficulty(m.0)));
    let current = Current {
        dungeon: group.current_dungeon_difficulty(),
        raid: group.current_raid_difficulty(),
        effective_raid: group.effective_raid_difficulty(here_dynamic),
    };
    for (entity, guid, hidden) in &objects {
        let slots = templates
            .get(guid.0)
            .and_then(|t| t.difficulty)
            .unwrap_or_default();
        let map = PortalMap {
            instance_type: catalog.and_then(|c| c.instance_type(slots.map)),
            dynamic: catalog.is_some_and(|c| c.has_dynamic_difficulty(slots.map)),
        };
        let show = shown(slots, map, current, || {
            benilla_formats::find_map_difficulty(
                &map_difficulties.0,
                slots.map as i32,
                current.raid as i32,
            )
            .is_some()
        });
        // While hidden the object is out of the exterior cull, which owns the root's `Visibility`
        // otherwise; showing hands it back.
        if show == hidden {
            debug!(
                "go difficulty: guid {:#x} {} (map {}, difficulty {}, current {current:?})",
                guid.0,
                if show { "shown" } else { "hidden" },
                slots.map,
                slots.difficulty,
            );
            if show {
                commands
                    .entity(entity)
                    .try_insert(Visibility::Inherited)
                    .try_remove::<ObjectHidden>();
            } else {
                commands
                    .entity(entity)
                    .try_insert((Visibility::Hidden, ObjectHidden));
            }
        }
    }
}

/// Marks a type 31 object on spawn and gives it the type's base alpha.
pub(crate) fn tag(entity: &mut EntityCommands, kind: EntityKind, type_id: i32, dynamic_flags: u32) {
    if kind == EntityKind::GameObject && type_id == GO_TYPE_DIFFICULTY {
        entity.insert((
            DifficultyObject,
            crate::aura_visual::AuraNodes::steady(base_alpha(dynamic_flags)),
        ));
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        step_difficulty_objects
            .after(crate::ui_party::drain_party)
            .in_set(WorldStage::Present),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAID: PortalMap = PortalMap {
        instance_type: Some(2),
        dynamic: false,
    };

    fn slots(map: u32, difficulty: u32) -> DifficultySlots {
        DifficultySlots { map, difficulty }
    }

    fn current(dungeon: u32, raid: u32, effective_raid: u32) -> Current {
        Current {
            dungeon,
            raid,
            effective_raid,
        }
    }

    #[test]
    fn a_map_id_of_zero_compares_with_the_dungeon_difficulty() {
        assert!(shown(slots(0, 1), RAID, current(1, 0, 0), || true));
        assert!(!shown(slots(0, 1), RAID, current(0, 1, 1), || true));
    }

    #[test]
    fn a_missing_map_row_or_a_party_map_compares_with_the_dungeon_difficulty() {
        let party = PortalMap {
            instance_type: Some(1),
            dynamic: true,
        };
        for map in [PortalMap::default(), party] {
            assert!(shown(slots(599, 1), map, current(1, 0, 0), || true));
            assert!(!shown(slots(599, 0), map, current(1, 0, 0), || true));
        }
    }

    #[test]
    fn a_dynamic_raid_compares_with_the_effective_raid_difficulty() {
        let dynamic = PortalMap {
            instance_type: Some(2),
            dynamic: true,
        };
        assert!(shown(slots(631, 3), dynamic, current(0, 1, 3), || true));
        assert!(!shown(slots(631, 1), dynamic, current(0, 1, 3), || true));
    }

    #[test]
    fn a_static_raid_compares_with_the_raid_difficulty() {
        assert!(shown(slots(533, 1), RAID, current(0, 1, 9), || true));
        assert!(!shown(slots(533, 0), RAID, current(1, 1, 0), || true));
    }

    #[test]
    fn a_heroic_raid_falls_back_two_only_without_its_own_row() {
        assert!(shown(slots(533, 0), RAID, current(0, 2, 2), || false));
        assert!(!shown(slots(533, 0), RAID, current(0, 2, 2), || true));
        assert!(shown(slots(533, 1), RAID, current(0, 3, 3), || false));
        assert!(
            !shown(slots(533, 0), RAID, current(0, 1, 1), || false),
            "below 2 there is no fallback"
        );
    }

    #[test]
    fn the_base_alpha_is_opaque_only_with_bit_two() {
        assert_eq!(base_alpha(0), 0.5);
        assert_eq!(base_alpha(0x8), 0.5);
        assert_eq!(base_alpha(0x2), 1.0);
    }
}
