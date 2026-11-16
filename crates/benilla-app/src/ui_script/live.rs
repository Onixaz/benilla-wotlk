//! The VM's live view of the client and the events a packet handler fires. 12340's verbs read the
//! live object when called (`0x60f396`) and a handler fires its events once its changes apply
//! (`0x520fe0`). [`LiveView`] is the read-only [`LiveState`] over the `World` every call into Lua
//! is lent: a system holding the VM takes [`LendLive`], which lends it for the system's run, and
//! an exclusive one lends through [`with_live_world`] or [`lend_world`]. [`UiEvents`] is the queue
//! a handler fires into, emptied into the VM once that handler's commands apply.

use benilla_ui::script::{LiveLoan, LiveState, ScriptValue, UiScript};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::system::{
    ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError, SystemState,
};
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

/// What the verbs read live, a system parameter so a VM system lends it with its own access. The
/// index is optional, so an app without the net plugin still lends a view that holds nobody.
#[derive(SystemParam)]
pub(crate) struct LiveView<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    control: Option<Res<'w, crate::net::ControlFlag>>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
    far_sight_clear: Option<Res<'w, crate::player::view_subject::FarSightClear>>,
    _mark: Option<Res<'w, LendsLive>>,
}

impl LiveView<'_, '_> {
    /// A held unit's fields: the verbs' unit lookup (`0x60c1f0`) asks for type mask 8 (`0x60c21e`).
    fn unit(&self, guid: u64) -> Option<&benilla_protocol::ObjectFields> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        fields.is_unit().then_some(fields)
    }
}

/// Never inserted: read by every [`LiveView`], so a system's declared access shows it lends.
#[derive(Resource)]
pub(crate) struct LendsLive;

impl LiveState for LiveView<'_, '_> {
    fn unit_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_flags())
    }

    fn unit_health(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_health().unwrap_or(0))
    }

    fn active_player(&self) -> Option<u64> {
        self.self_guid.as_ref()?.0
    }

    /// An app without the net plugin boots in control.
    fn has_control(&self) -> bool {
        self.control.as_ref().is_none_or(|c| c.0)
    }

    /// Our `PLAYER_FARSIGHT` naming a held object, as the bit needs (`0x6e28c6`-`0x6e28ff`), and
    /// no immediate clear ending it (`0x6e2b00`).
    fn far_sight(&self) -> bool {
        let Some(index) = self.index.as_ref() else {
            return false;
        };
        let field = self
            .active_player()
            .and_then(|me| index.0.get(&me))
            .and_then(|&e| self.stores.get(e).ok())
            .and_then(|store| store.0.player_farsight());
        field.is_some_and(|subject| {
            index.0.contains_key(&subject)
                && self
                    .far_sight_clear
                    .as_ref()
                    .is_none_or(|c| !c.cuts(subject))
        })
    }
}

/// [`LiveView`] lent to every verb for the run of the system that takes it: the parameter each
/// system holding the VM takes beside it, so every call into Lua it makes reads the `World`.
pub(crate) struct LendLive<'w, 's> {
    _loan: LiveLoan<LiveView<'w, 's>>,
}

/// The state [`LendLive`] builds its view from.
type ViewState = <LiveView<'static, 'static> as SystemParam>::State;

// SAFETY: every access is `LiveView`'s, registered and fetched through it.
unsafe impl SystemParam for LendLive<'_, '_> {
    type State = ViewState;
    type Item<'w, 's> = LendLive<'w, 's>;

    fn init_state(world: &mut World) -> Self::State {
        <LiveView<'static, 'static> as SystemParam>::init_state(world)
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        <LiveView<'static, 'static> as SystemParam>::init_access(
            state,
            system_meta,
            component_access_set,
            world,
        );
    }

    fn apply(state: &mut Self::State, system_meta: &SystemMeta, world: &mut World) {
        <LiveView<'static, 'static> as SystemParam>::apply(state, system_meta, world);
    }

    fn queue(state: &mut Self::State, system_meta: &SystemMeta, world: DeferredWorld) {
        <LiveView<'static, 'static> as SystemParam>::queue(state, system_meta, world);
    }

    unsafe fn validate_param(
        state: &mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell,
    ) -> Result<(), SystemParamValidationError> {
        // SAFETY: the caller's guarantees are `LiveView`'s.
        unsafe {
            <LiveView<'static, 'static> as SystemParam>::validate_param(state, system_meta, world)
        }
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        // SAFETY: the caller's guarantees are `LiveView`'s.
        let view = unsafe {
            <LiveView<'static, 'static> as SystemParam>::get_param(
                state,
                system_meta,
                world,
                change_tick,
            )
        };
        // SAFETY: the system drops its parameters on the thread that ran it when its run ends,
        // after any loan made inside it, and never leaks one.
        LendLive {
            _loan: unsafe { LiveLoan::new(view) },
        }
    }
}

// SAFETY: `LiveView` reads only.
unsafe impl ReadOnlySystemParam for LendLive<'_, '_> {}

/// The events this frame's handlers fired, in order; the drain empties it after each handler.
#[derive(Resource, Default)]
pub(crate) struct UiEvents(Vec<(&'static str, Vec<ScriptValue>)>);

impl UiEvents {
    /// Fire `event` once the calling handler's changes have applied, before the next handler runs.
    pub(crate) fn fire(&mut self, event: &'static str, args: Vec<ScriptValue>) {
        self.0.push((event, args));
    }

    /// Whether nothing waits to fire.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The cached parameter state [`with_live_world`] lends from.
#[derive(Resource)]
pub(super) struct LiveViewState(SystemState<LiveView<'static, 'static>>);

/// The VM taken out of the world for a call, put back on return and on unwind.
struct HeldScript<'w> {
    world: &'w mut World,
    script: Option<UiScript>,
}

impl Drop for HeldScript<'_> {
    fn drop(&mut self) {
        if let Some(script) = self.script.take() {
            self.world.insert_non_send_resource(script);
        }
    }
}

/// The view's state taken out of the world for a call, put back on return and on unwind.
struct HeldState<'w> {
    world: &'w mut World,
    state: Option<LiveViewState>,
}

impl Drop for HeldState<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            self.world.insert_resource(state);
        }
    }
}

/// Run `f` on the VM with the `World` lent as its live view; `None` with no VM.
pub(crate) fn with_live_world<R>(
    world: &mut World,
    f: impl FnOnce(&mut UiScript) -> R,
) -> Option<R> {
    let script = world.remove_non_send_resource::<UiScript>()?;
    let mut held = HeldScript {
        world,
        script: Some(script),
    };
    let script = held.script.as_mut().expect("held until the drop");
    Some(lend_world(held.world, script, f))
}

/// Run `f` on `script`, a VM held outside the world, with the `World` lent as its live view.
pub(crate) fn lend_world<R>(
    world: &mut World,
    script: &mut UiScript,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    let state = match world.remove_resource::<LiveViewState>() {
        Some(state) => state,
        None => LiveViewState(SystemState::new(world)),
    };
    let mut held = HeldState {
        world,
        state: Some(state),
    };
    let state = held.state.as_mut().expect("held until the drop");
    let view = state.0.get(held.world);
    script.with_live(&view, f)
}

/// Fires what the handler that just ran queued, in its order, with the `World` lent; it runs after
/// each handler, not after each packet. With no VM they are dropped, as the reference has no
/// listener before its UI loads.
pub(crate) fn fire_handler_events(world: &mut World) {
    let Some(mut queued) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    if queued.0.is_empty() {
        return;
    }
    let events = std::mem::take(&mut queued.0);
    with_live_world(world, |script| {
        for (event, args) in events {
            script.fire_event(event, args);
        }
    });
}
