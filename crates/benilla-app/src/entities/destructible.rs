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

use benilla_world::terrain_stream::PendingCollider;

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

/// Marks a destructible building from its create, the gate of the building systems.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct DestructibleBuilding;

/// Mark a type-33 GameObject as it streams in.
pub(crate) fn tag(entity: &mut EntityCommands, kind: benilla_protocol::EntityKind, type_id: i32) {
    if kind == benilla_protocol::EntityKind::GameObject && type_id == GO_TYPE_DESTRUCTIBLE_BUILDING
    {
        entity.insert(DestructibleBuilding);
    }
}

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
    gameobjects: Option<Res<'w, super::GameObjects>>,
}

impl BuildingSets<'_> {
    /// The display the building shows now: its state's model once the template has landed,
    /// `GAMEOBJECT_DISPLAYID` before that, as the display swap reads it.
    fn shown_display(&self, guid: u64, store: &ObjectStore) -> Option<u32> {
        self.gameobjects
            .as_deref()
            .and_then(|g| {
                live_state_display(
                    store,
                    self.templates.as_deref()?.get(guid),
                    self.destructibles.as_deref(),
                    &g.catalog,
                )
            })
            .or_else(|| {
                store
                    .0
                    .gameobject_displayid()
                    .filter(|&d| d > 0)
                    .map(|d| d as u32)
            })
    }

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
    parts: Query<(&Children, &GlobalTransform)>,
    solid: Query<(), Or<(With<avian3d::prelude::Collider>, With<PendingCollider>)>>,
    swaps: Query<&BuildingSwap>,
    mut buildings: Query<
        (
            Entity,
            &Guid,
            &mut NetEntity,
            Ref<ObjectStore>,
            Option<&mut BuildingState>,
        ),
        (With<VisualAttached>, With<DestructibleBuilding>),
    >,
) {
    let templates_new = sets.templates.as_ref().is_some_and(|t| t.is_changed());
    for (entity, guid, mut net, store, seen) in &mut buildings {
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
        // The template landed after the visual was built: build it again with its state's model
        // and sets, in one rebuild, the display swap having nothing left to do.
        if has_row && !seen.has_row {
            seen.has_row = true;
            net.display_id = sets.shown_display(guid.0, &store).or(net.display_id);
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
                "destructible building: guid {:016x} state {} -> {state} (display {:?} -> {:?})",
                guid.0,
                seen.state,
                net.display_id,
                sets.shown_display(guid.0, &store)
            );
            commands.entity(entity).insert(BuildingTransition {
                from: seen.state,
                from_display: net.display_id,
            });
            // The rebuild takes the new state's model at once: rebuilt on the old one, it would
            // spend the transition there and leave the swap a second, bare rebuild.
            net.display_id = sets.shown_display(guid.0, &store).or(net.display_id);
            // The old model stays up beside the new one: until it is built, or through the rise.
            if let Some(old) = swaps.get(entity).ok().and_then(|s| s.ghost) {
                commands.entity(old).despawn();
            }
            let ghost = parts.get(entity).ok().map(|(children, gt)| {
                let shown: Vec<Entity> = children.iter().filter(|&c| !solid.contains(c)).collect();
                let ghost = commands
                    .spawn((
                        gt.compute_transform(),
                        Visibility::default(),
                        BuildingGhostOf(entity),
                    ))
                    .add_children(&shown)
                    .id();
                ghost
            });
            let rise = sets
                .row(guid.0, &store)
                .and_then(|row| Rise::plan(row, state, net.display_id));
            match ghost {
                Some(ghost) => {
                    commands.entity(entity).insert(BuildingSwap {
                        ghost: Some(ghost),
                        rise,
                        started: None,
                    });
                }
                None => {
                    commands.entity(entity).remove::<BuildingSwap>();
                }
            }
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

/// The rise's default speed when the row's is 0 or less (`0x70dfa0`).
const DEFAULT_HEAL_SPEED: f32 = 12.0;

/// A rebuild's rise: a change into the intact or rebuilding state, on a row whose heal effect is
/// not 4 (`0x70dfa0`). The new model climbs out of the ground over its own height at a third of
/// the heal speed per second; effect 0 waits half the time, then eases up while the old model
/// sinks over that first half; effects 1 to 3 ease up from the start, effect 2 sinking the old
/// model as it goes, effect 1 keeping it standing, effect 3 dropping it at once (`0x70e750`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rise {
    effect: i32,
    speed: f32,
    /// The display the old model showed, whose height it sinks by.
    from_display: Option<u32>,
}

impl Rise {
    fn plan(row: &DestructibleModel, state: usize, from_display: Option<u32>) -> Option<Rise> {
        if !matches!(state, 0 | 3) || row.heal_effect == 4 {
            return None;
        }
        let speed = row.heal_effect_speed as f32;
        Some(Rise {
            effect: row.heal_effect,
            speed: if speed > 0.0 {
                speed
            } else {
                DEFAULT_HEAL_SPEED
            },
            from_display,
        })
    }

    /// The rise's length for a model this tall.
    fn duration(&self, height: f32) -> f32 {
        height / (self.speed / 3.0)
    }

    /// At progress `p` in `[0, 1)`, how far below its rest the new model is and how far the old
    /// one has sunk, each as a fraction of its own height; `None` for an old model left where it
    /// stands.
    fn depths(&self, p: f32) -> (f32, Option<f32>) {
        if self.effect == 0 {
            let up = if p > 0.5 {
                let t = 1.0 - 2.0 * (p - 0.5);
                1.0 - t * t * t
            } else {
                0.0
            };
            let sunk = if p >= 0.5 { 1.0 } else { 8.0 * p * p * p };
            (1.0 - up, Some(sunk))
        } else {
            let r = 1.0 - p;
            (r * r * r, (self.effect == 2).then_some(p * p * p))
        }
    }

    /// Whether the old model stays up through the rise.
    fn keeps_old(&self) -> bool {
        self.effect != 3
    }
}

/// A building's state swap in flight: the old model's render parts, moved onto a ghost so they
/// outlive the teardown, and the rise, if the new state rises.
#[derive(Component, Debug)]
pub(crate) struct BuildingSwap {
    /// `None` once the ghost is gone.
    ghost: Option<Entity>,
    rise: Option<Rise>,
    /// When the new visual landed and the two models' heights.
    started: Option<(f32, f32, f32)>,
}

/// A ghost of an old building model, freed with its building.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct BuildingGhostOf(Entity);

/// A render part's rest placement under a rising building.
#[derive(Component, Clone, Copy, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct RiseRest(Vec3);

/// A building display's height, the vertical extent of its WMO's root box at the object's scale.
fn display_height(
    gameobjects: Option<&super::GameObjects>,
    wmos: &Assets<benilla_assets::WmoModel>,
    display: Option<u32>,
    scale: f32,
) -> f32 {
    display
        .and_then(|d| gameobjects?.models.get(&d))
        .and_then(|dm| match &dm.handle {
            super::ModelHandle::Wmo(h) => wmos.get(h),
            _ => None,
        })
        .and_then(|w| w.root_bbox)
        .map_or(0.0, |(min, max)| (max[2] - min[2]).max(0.0) * scale)
}

/// Run each building swap once the new model is built: without a rise the ghost goes; with one,
/// the new model's render parts climb and the ghost sinks or stands until the rise ends, then
/// the parts settle at rest. The colliders stay at rest throughout, the base collision the
/// reference keeps on while the model is in motion.
#[allow(clippy::type_complexity)] // the swap's part queries
pub(super) fn tend_building_swaps(
    mut commands: Commands,
    time: Res<Time>,
    gameobjects: Option<Res<super::GameObjects>>,
    wmos: Res<Assets<benilla_assets::WmoModel>>,
    mut swaps: Query<
        (Entity, &mut BuildingSwap, &NetEntity, Option<&Children>),
        With<VisualAttached>,
    >,
    solid: Query<(), Or<(With<avian3d::prelude::Collider>, With<PendingCollider>)>>,
    mut parts: Query<(&mut Transform, Option<&RiseRest>), Without<BuildingGhostOf>>,
    mut ghosts: Query<&mut Transform, (With<BuildingGhostOf>, Without<RiseRest>)>,
    orphans: Query<(Entity, &BuildingGhostOf)>,
    alive: Query<(), With<BuildingSwap>>,
) {
    // A ghost whose building left or settled goes with it.
    for (ghost, of) in &orphans {
        if !alive.contains(of.0) {
            commands.entity(ghost).despawn();
        }
    }
    let now = time.elapsed_secs();
    for (entity, mut swap, net, children) in &mut swaps {
        let Some(rise) = swap.rise else {
            if let Some(ghost) = swap.ghost.take() {
                commands.entity(ghost).despawn();
            }
            commands.entity(entity).remove::<BuildingSwap>();
            continue;
        };
        if swap.started.is_none() {
            swap.started = Some((
                now,
                display_height(gameobjects.as_deref(), &wmos, net.display_id, net.scale),
                display_height(gameobjects.as_deref(), &wmos, rise.from_display, net.scale),
            ));
            if !rise.keeps_old() {
                if let Some(ghost) = swap.ghost.take() {
                    commands.entity(ghost).despawn();
                }
            }
        }
        let Some((start, height, old_height)) = swap.started else {
            continue;
        };
        let duration = rise.duration(height);
        let p = if duration > 0.0 {
            (now - start) / duration
        } else {
            1.0
        };
        // The building's own placement; the parts query covers it, being no ghost.
        let Ok(root) = parts.get(entity).map(|(tf, _)| *tf) else {
            continue;
        };
        let root = &root;
        // World up in the building's own frame, so a tilted or scaled placement sinks straight.
        let down =
            |root: &Transform, d: f32| root.rotation.inverse() * Vec3::NEG_Y * d / root.scale;
        let done = p >= 1.0;
        let (new_depth, old_depth) = if done { (0.0, None) } else { rise.depths(p) };
        for c in children.into_iter().flatten() {
            if solid.contains(*c) {
                continue;
            }
            let Ok((mut tf, rest)) = parts.get_mut(*c) else {
                continue;
            };
            let rest = match rest {
                Some(r) => r.0,
                None => {
                    commands.entity(*c).insert(RiseRest(tf.translation));
                    tf.translation
                }
            };
            tf.translation = rest + down(root, new_depth * height);
            if done {
                commands.entity(*c).remove::<RiseRest>();
            }
        }
        if let (Some(sunk), Some(mut g)) =
            (old_depth, swap.ghost.and_then(|g| ghosts.get_mut(g).ok()))
        {
            g.translation = root.translation + Vec3::NEG_Y * sunk * old_height;
        }
        if done {
            if let Some(ghost) = swap.ghost.take() {
                commands.entity(ghost).despawn();
            }
            commands.entity(entity).remove::<BuildingSwap>();
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

    fn rise(effect: i32) -> Rise {
        Rise {
            effect,
            speed: 12.0,
            from_display: None,
        }
    }

    #[test]
    fn only_intact_and_rebuilding_rise_and_effect_4_never() {
        let row = |heal_effect, heal_effect_speed| DestructibleModel {
            heal_effect,
            heal_effect_speed,
            ..Default::default()
        };
        assert!(Rise::plan(&row(0, 0), 1, None).is_none());
        assert!(Rise::plan(&row(0, 0), 2, None).is_none());
        assert!(Rise::plan(&row(4, 0), 0, None).is_none());
        assert_eq!(Rise::plan(&row(0, 0), 0, None).unwrap().speed, 12.0);
        // Row 58: effect 1 at speed 25.
        let r = Rise::plan(&row(1, 25), 3, Some(7541)).unwrap();
        assert_eq!((r.effect, r.speed, r.from_display), (1, 25.0, Some(7541)));
        // A third of the speed per second: a 30-yard wall at 12 takes 7.5 s.
        assert_eq!(rise(0).duration(30.0), 7.5);
    }

    #[test]
    fn effect_0_waits_half_the_rise_while_the_old_model_sinks() {
        let r = rise(0);
        assert_eq!(r.depths(0.0), (1.0, Some(0.0)));
        assert_eq!(r.depths(0.25), (1.0, Some(0.125)));
        assert_eq!(r.depths(0.5), (1.0, Some(1.0)));
        let (new, old) = r.depths(0.75);
        assert!((new - 0.125).abs() < 1e-6, "{new}");
        assert_eq!(old, Some(1.0));
    }

    #[test]
    fn effects_1_to_3_ease_up_from_the_start() {
        assert_eq!(rise(1).depths(0.5), (0.125, None));
        assert_eq!(rise(2).depths(0.5), (0.125, Some(0.125)));
        assert_eq!(rise(3).depths(0.5), (0.125, None));
        assert!(rise(1).keeps_old() && rise(2).keeps_old() && !rise(3).keeps_old());
    }
}
