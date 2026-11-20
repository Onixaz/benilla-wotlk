//! Destructible buildings (GameObject type 33): the Wintergrasp walls and towers, the Strand of the
//! Ancients and Isle of Conquest gates, the Ulduar towers and storm beacons. A building has four
//! damage states, intact, damaged, destroyed and rebuilding, picked from `GAMEOBJECT_FLAGS` bits
//! 9 and 10, each with its own WMO and doodad sets from `DestructibleModelData.dbc`.
//!
//! The reference builds all four models when the template lands (`0x710be0`) and shows the one
//! the flags pick (`0x70dfa0`), whatever `GAMEOBJECT_DISPLAYID` says: here the picked state's
//! display is the entity's live display, and a change swaps the model as a display move does.
//!
//! Each state model carries up to three doodad sets beside set 0: the destruction set a change
//! into the state plays once, the impact set a hit plays, and the ambient set that loops while
//! the state holds (fire, smoke). A change also carries the previous model's impact and ambient
//! sets over onto the new one, where they play Custom2 and settle. Each set's props run the
//! reference's sequence chain ([`SetRole::next`]).

use benilla_formats::{
    DestructibleModel, DestructibleModels, GameObjectCatalog, DESTRUCTIBLE_STATES,
};
use bevy::prelude::*;

use crate::go_templates::GoTemplate;
use crate::net::{Guid, NetEntity, ObjectStore};

use super::VisualAttached;

/// `AnimationData` Stand, Custom0, Custom1 and Custom2.
const STAND: u16 = 0;
const CUSTOM0: u16 = 153;
const CUSTOM1: u16 = 154;
const CUSTOM2: u16 = 155;

/// `GAMEOBJECT_FLAGS` `0x100`, whose every move plays the impact set (`0x70e6a0`).
const FLAG_HIT: u32 = 0x100;

/// `GAMEOBJECT_TYPE_DESTRUCTIBLE_BUILDING`.
pub(crate) const GO_TYPE_DESTRUCTIBLE_BUILDING: i32 = 33;

/// `DestructibleModelData.dbc`, read once at startup.
#[derive(Resource, Default)]
pub(crate) struct Destructibles(pub(crate) DestructibleModels);

/// The damage state `GAMEOBJECT_FLAGS` picks: DAMAGED (`0x200`) is 1, DESTROYED (`0x400`) 2,
/// both 3 (`0x70dfa0`).
pub(crate) fn damage_state(flags: u32) -> usize {
    (((flags >> 8) & 0xE) >> 1) as usize
}

/// The four states' displays (`0x710be0`): state 0 is the template's; with a row, each later
/// state takes the row's WMO when `GameObjectDisplayInfo` has it, else the state before it's.
/// Without a row the later states have no model.
pub(crate) fn state_displays(
    row: Option<&DestructibleModel>,
    template_display: u32,
    catalog: &GameObjectCatalog,
) -> [Option<u32>; DESTRUCTIBLE_STATES] {
    let mut out = [None; DESTRUCTIBLE_STATES];
    out[0] = Some(template_display).filter(|&d| d != 0);
    let Some(row) = row else {
        return out;
    };
    for state in 1..DESTRUCTIBLE_STATES {
        let own = row.wmo[state];
        out[state] = if own != 0 && catalog.model_path(own).is_some() {
            Some(own)
        } else {
            if own != 0 {
                warn_once!(
                    "destructible building: state {state} names display {own}, which \
                     GameObjectDisplayInfo lacks; it keeps the state before it"
                );
            }
            out[state - 1]
        };
    }
    out
}

/// A destructible building's live display: its template's state model for the flags' state.
/// `None` when the object is not one, its template has not landed, or the state has no model, and
/// the caller then keeps `GAMEOBJECT_DISPLAYID`. The reference draws nothing for a stateless
/// model; only AzerothCore's eight row-less test templates could reach that.
pub(crate) fn live_state_display(
    store: &ObjectStore,
    template: Option<&GoTemplate>,
    destructibles: Option<&Destructibles>,
    catalog: &GameObjectCatalog,
) -> Option<u32> {
    if store.0.gameobject_type_id() != GO_TYPE_DESTRUCTIBLE_BUILDING {
        return None;
    }
    let t = template?.destructible?;
    let row = destructibles.and_then(|d| d.0.get(t.model_data));
    state_displays(row, t.display_id, catalog)[damage_state(store.0.gameobject_flags())]
}

/// The mouseover test of a destructible building (`0x70df30`): only with a row whose
/// `DoNotHighlight` is clear. Its other term, refusing the building the player stands inside
/// (the player's model parented into one of its WMO groups), is not built: benilla does not
/// parent a mover into a GameObject's WMO groups.
pub(crate) fn mouseover_allowed(
    template: &GoTemplate,
    destructibles: Option<&Destructibles>,
) -> bool {
    let Some(t) = template.destructible else {
        return template.highlight_column;
    };
    destructibles
        .and_then(|d| d.0.get(t.model_data))
        .is_some_and(|row| !row.do_not_highlight)
}

/// Which of a state's doodad sets a prop belongs to, which picks its sequence chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetRole {
    /// The set that loops while the state holds.
    Ambient,
    /// The set a change into the state plays once.
    Destruction,
    /// The set a hit plays.
    Impact,
    /// The previous model's impact or ambient set, carried onto the new model.
    Moved,
}

impl SetRole {
    /// The sequence the set starts on: Custom2 for a carried set (`0x70dfa0`), else Custom0.
    fn first(self) -> u16 {
        match self {
            SetRole::Moved => CUSTOM2,
            _ => CUSTOM0,
        }
    }

    /// The sequence after `finished` plays out, from the reference's finish handlers; `None`
    /// leaves the last one looping.
    fn next(self, finished: u16) -> Option<u16> {
        match self {
            // `0x70b870`: anything but Custom1 moves on to Custom1.
            SetRole::Ambient => (finished != CUSTOM1).then_some(CUSTOM1),
            // `0x70b800`: anything but Stand settles to Stand.
            SetRole::Destruction => (finished != STAND).then_some(STAND),
            // `0x70b830`: Custom0 moves on to Custom1, the rest settle to Stand.
            SetRole::Impact => match finished {
                CUSTOM0 => Some(CUSTOM1),
                STAND => None,
                _ => Some(STAND),
            },
            // `0x70b8b0`: Custom0 and Custom1 move on to Custom2, which settles to Stand.
            SetRole::Moved => match finished {
                CUSTOM0 | CUSTOM1 => Some(CUSTOM2),
                CUSTOM2 => Some(STAND),
                _ => None,
            },
        }
    }
}

/// On a state-set prop's anim root: the requested sequence of its chain and when its first
/// play-window ends.
#[derive(Component, Debug)]
pub(crate) struct BuildingDoodad {
    role: SetRole,
    requested: u16,
    ends_at: Option<f32>,
    done: bool,
}

impl BuildingDoodad {
    pub(crate) fn new(role: SetRole) -> Self {
        BuildingDoodad {
            role,
            requested: role.first(),
            ends_at: None,
            done: false,
        }
    }

    pub(crate) fn role(&self) -> SetRole {
        self.role
    }
}

/// A building's damage state and flags as last seen, the diff key of [`track_building_states`].
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct BuildingState {
    state: usize,
    flags: u32,
    /// Whether its row was known when its visual was built: the template can land after it.
    has_row: bool,
}

/// A state change waiting on the rebuilt visual: the state left and the model it showed.
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct BuildingTransition {
    from: usize,
    from_display: Option<u32>,
}

/// A hit to play: the impact set of the state the flags name.
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct BuildingImpact {
    pub(crate) set: u16,
}

/// One state set to spawn: the doodad set index, its role, and the display of the model it
/// comes from when that is the previous state's (`None` is the current model).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StateSet {
    pub(crate) set: u16,
    pub(crate) role: SetRole,
    pub(crate) from_display: Option<u32>,
}

/// The state sets a building's current model shows: on a change the previous state's impact and
/// ambient sets carried over, then for a damaged or destroyed state its destruction set; always
/// its ambient set (`0x70dfa0`, and `0x70e750` on load). A set named twice on one model keeps the
/// later role, as the later call is the one that sets its sequence.
pub(crate) fn state_sets(
    row: &DestructibleModel,
    state: usize,
    transition: Option<&BuildingTransition>,
) -> Vec<StateSet> {
    let mut out = Vec::new();
    let own = |out: &mut Vec<StateSet>, set: u16, role: SetRole| {
        if set == 0 {
            return;
        }
        out.retain(|s: &StateSet| s.from_display.is_some() || s.set != set);
        out.push(StateSet {
            set,
            role,
            from_display: None,
        });
    };
    if let Some(t) = transition.filter(|t| t.from != state) {
        for set in [row.impact(t.from), row.ambient(t.from)] {
            if set != 0 {
                out.push(StateSet {
                    set,
                    role: SetRole::Moved,
                    from_display: t.from_display,
                });
            }
        }
        // Intact and rebuilding rise instead of playing a destruction set.
        if matches!(state, 1 | 2) {
            own(&mut out, row.destruction(state), SetRole::Destruction);
        }
    }
    own(&mut out, row.ambient(state), SetRole::Ambient);
    out
}

/// What a building's sets are read from.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BuildingSets<'w> {
    templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    destructibles: Option<Res<'w, Destructibles>>,
}

impl BuildingSets<'_> {
    /// The building's row, `None` for any other object or before its template lands.
    fn row(&self, guid: u64, store: &ObjectStore) -> Option<&DestructibleModel> {
        if store.0.gameobject_type_id() != GO_TYPE_DESTRUCTIBLE_BUILDING {
            return None;
        }
        let t = self.templates.as_deref()?.get(guid)?.destructible?;
        self.destructibles.as_deref()?.0.get(t.model_data)
    }

    /// [`state_sets`] for a live building; none without a row.
    pub(crate) fn state_sets(
        &self,
        guid: u64,
        store: &ObjectStore,
        transition: Option<&BuildingTransition>,
    ) -> Vec<StateSet> {
        self.row(guid, store).map_or_else(Vec::new, |row| {
            state_sets(row, damage_state(store.0.gameobject_flags()), transition)
        })
    }
}

/// Diff each building's flags against the last seen. A state change rebuilds the visual, even
/// when the new state shows the same display, as the reference swaps to the state's own model
/// object (`0x70dfa0`), and leaves a [`BuildingTransition`] for the props to play. A move of the
/// hit flag on a building with a row plays the impact set of the state the flags name
/// (`0x70e6a0`). Runs before the display swap, so the entity still names the model it shows.
#[allow(clippy::type_complexity)] // one query's filters
pub(super) fn track_building_states(
    mut commands: Commands,
    sets: BuildingSets,
    mut buildings: Query<
        (
            Entity,
            &Guid,
            &NetEntity,
            Ref<ObjectStore>,
            Option<&mut BuildingState>,
        ),
        With<VisualAttached>,
    >,
) {
    let templates_new = sets.templates.as_ref().is_some_and(|t| t.is_changed());
    for (entity, guid, net, store, seen) in &mut buildings {
        if !store.is_changed() && !templates_new && seen.is_some() {
            continue;
        }
        if store.0.gameobject_type_id() != GO_TYPE_DESTRUCTIBLE_BUILDING {
            continue;
        }
        let flags = store.0.gameobject_flags();
        let state = damage_state(flags);
        let has_row = sets.row(guid.0, &store).is_some();
        let Some(mut seen) = seen else {
            commands.entity(entity).insert(BuildingState {
                state,
                flags,
                has_row,
            });
            continue;
        };
        // The template landed after the visual was built: build it again with its sets.
        if has_row && !seen.has_row {
            seen.has_row = true;
            super::live_display::rebuild_visual(&mut commands, entity);
        }
        if seen.flags == flags {
            continue;
        }
        let changed = seen.flags ^ flags;
        if changed & FLAG_HIT != 0 {
            // The impact getter takes the four flag bits, so a stray `0x800` names no state.
            let set = sets
                .row(guid.0, &store)
                .map_or(0, |row| row.impact((((flags >> 8) & 0xF) >> 1) as usize));
            if set != 0 {
                commands.entity(entity).insert(BuildingImpact { set });
            }
        }
        if state != seen.state {
            info!(
                "destructible building: guid {:016x} state {} -> {state} (display {:?})",
                guid.0, seen.state, net.display_id
            );
            commands.entity(entity).insert(BuildingTransition {
                from: seen.state,
                from_display: net.display_id,
            });
            super::live_display::rebuild_visual(&mut commands, entity);
        }
        *seen = BuildingState {
            state,
            flags,
            has_row,
        };
    }
}

/// Step each state-set prop through its sequence chain: request the sequence (resolved on the
/// model through `AnimationData`'s fallbacks), and when its first play-window ends, move to the
/// next one, the reference's finish callbacks.
pub(super) fn drive_building_doodads(
    mut commands: Commands,
    time: Res<Time>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut props: Query<(
        Entity,
        &mut BuildingDoodad,
        &benilla_assets::ModelAnimations,
        &benilla_world::doodad_anim::DoodadAnimHost,
        Option<&mut benilla_world::doodad_anim::HostSequence>,
    )>,
) {
    let now = time.elapsed_secs();
    for (entity, mut doodad, anims, host, current) in &mut props {
        if doodad.done {
            continue;
        }
        let want = match anim_data.as_deref() {
            Some(data) => anims.resolve(doodad.requested, &data.0).id,
            None if anims.find(doodad.requested).is_some() => doodad.requested,
            None => STAND,
        };
        let request = benilla_world::doodad_anim::HostSequence(want);
        match current {
            Some(mut c) => {
                c.set_if_neq(request);
            }
            None => {
                commands.entity(entity).insert(request);
            }
        }
        match doodad.ends_at {
            None => {
                if let Some((id, window_end)) = host.armed() {
                    if id == want && window_end.is_finite() {
                        doodad.ends_at = Some(window_end);
                    }
                }
            }
            Some(end) if now >= end => match doodad.role.next(doodad.requested) {
                Some(next) => {
                    doodad.requested = next;
                    doodad.ends_at = None;
                }
                None => doodad.done = true,
            },
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::go_templates::DestructibleTemplate;

    fn catalog(ids: &[u32]) -> GameObjectCatalog {
        GameObjectCatalog::from_models(ids.iter().map(|&i| (i, format!("w{i}.wmo"))).collect())
    }

    #[test]
    fn the_flags_pick_the_state_from_bits_9_and_10() {
        assert_eq!(damage_state(0), 0);
        assert_eq!(damage_state(0x100), 0); // the hit bit is not a state
        assert_eq!(damage_state(0x200), 1);
        assert_eq!(damage_state(0x400), 2);
        assert_eq!(damage_state(0x600 | 0x20), 3);
    }

    /// The Wintergrasp fortress wall (row 31): damaged 7897, destroyed 7874, no rebuilding WMO.
    #[test]
    fn a_state_without_its_own_wmo_keeps_the_state_before_it() {
        let row = DestructibleModel {
            wmo: [0, 7897, 7874, 0],
            ..Default::default()
        };
        let shown = state_displays(Some(&row), 7877, &catalog(&[7877, 7897, 7874]));
        assert_eq!(shown, [Some(7877), Some(7897), Some(7874), Some(7874)]);
        // A display the catalog lacks falls back as well.
        let shown = state_displays(Some(&row), 7877, &catalog(&[7877, 7874]));
        assert_eq!(shown, [Some(7877), Some(7877), Some(7874), Some(7874)]);
        // No row: only the intact model.
        assert_eq!(
            state_displays(None, 7877, &catalog(&[7877])),
            [Some(7877), None, None, None]
        );
    }

    fn template(model_data: u32) -> GoTemplate {
        let mut t = crate::go_templates::GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[18] = model_data as i32;
        t.insert(190219, 33, 7877, "Wintergrasp Fortress Wall".into(), &data);
        let got = t.template_by_entry(190219).cloned().unwrap();
        assert_eq!(
            got.destructible,
            Some(DestructibleTemplate {
                model_data,
                display_id: 7877
            })
        );
        got
    }

    #[test]
    fn mouseover_needs_a_row_that_allows_it() {
        let rows = Destructibles(DestructibleModels::from_rows(HashMap::from([
            (31, DestructibleModel::default()),
            (
                26,
                DestructibleModel {
                    do_not_highlight: true,
                    ..Default::default()
                },
            ),
        ])));
        assert!(mouseover_allowed(&template(31), Some(&rows)));
        assert!(!mouseover_allowed(&template(26), Some(&rows)));
        assert!(!mouseover_allowed(&template(0), Some(&rows)));
        assert!(!mouseover_allowed(&template(31), None));
    }

    /// The mouseover slot reads the row.
    #[test]
    fn a_building_is_a_mouseover_when_its_row_allows() {
        use crate::target::cursor_mode::{mouseover_eligible, GoOverrides};
        let eligible =
            |column| mouseover_eligible(33, 0, 0, column, Some(3), GoOverrides::default());
        assert!(eligible(Some(true)));
        assert!(!eligible(Some(false)));
        // A template not yet answered reads eligible, as GENERIC's column does.
        assert!(eligible(None));
    }

    #[test]
    fn each_role_plays_the_reference_chain() {
        let chain = |role: SetRole| {
            let mut seq = vec![role.first()];
            while let Some(n) = role.next(*seq.last().unwrap()) {
                seq.push(n);
                assert!(seq.len() < 8, "{role:?} never settles: {seq:?}");
            }
            seq
        };
        assert_eq!(chain(SetRole::Ambient), [CUSTOM0, CUSTOM1]);
        assert_eq!(chain(SetRole::Destruction), [CUSTOM0, STAND]);
        assert_eq!(chain(SetRole::Impact), [CUSTOM0, CUSTOM1, STAND]);
        assert_eq!(chain(SetRole::Moved), [CUSTOM2, STAND]);
    }

    /// Row 36 (a Wintergrasp workshop wall): damaged destruction 1, impact 3, ambient 2; intact
    /// none; rebuilding ambient 7.
    fn row36() -> DestructibleModel {
        DestructibleModel {
            wmo: [0, 7913, 0, 0],
            destruction_set: [0, 1, 0, 0],
            impact_set: [0, 3, 0, 0],
            ambient_set: [0, 2, 0, 7],
            ..Default::default()
        }
    }

    #[test]
    fn a_loaded_building_shows_only_its_ambient_set() {
        let set = |set, role| StateSet {
            set,
            role,
            from_display: None,
        };
        assert_eq!(state_sets(&row36(), 1, None), [set(2, SetRole::Ambient)]);
        assert!(state_sets(&row36(), 0, None).is_empty());
    }

    #[test]
    fn a_change_plays_destruction_and_carries_the_old_sets_over() {
        let damaged = BuildingTransition {
            from: 0,
            from_display: Some(7913),
        };
        assert_eq!(
            state_sets(&row36(), 1, Some(&damaged)),
            [
                StateSet {
                    set: 1,
                    role: SetRole::Destruction,
                    from_display: None
                },
                StateSet {
                    set: 2,
                    role: SetRole::Ambient,
                    from_display: None
                },
            ]
        );
        // Damaged to rebuilding: the damaged model's impact and ambient sets ride over and the
        // rebuilt model rises, so no destruction set.
        let rebuilt = BuildingTransition {
            from: 1,
            from_display: Some(7913),
        };
        let carried = |set| StateSet {
            set,
            role: SetRole::Moved,
            from_display: Some(7913),
        };
        assert_eq!(
            state_sets(&row36(), 3, Some(&rebuilt)),
            [
                carried(3),
                carried(2),
                StateSet {
                    set: 7,
                    role: SetRole::Ambient,
                    from_display: None
                },
            ]
        );
    }

    #[test]
    fn a_set_named_twice_keeps_its_later_role() {
        let row = DestructibleModel {
            destruction_set: [0, 4, 0, 0],
            ambient_set: [0, 4, 0, 0],
            ..Default::default()
        };
        let t = BuildingTransition {
            from: 0,
            from_display: None,
        };
        assert_eq!(
            state_sets(&row, 1, Some(&t)),
            [StateSet {
                set: 4,
                role: SetRole::Ambient,
                from_display: None
            }]
        );
    }
}
