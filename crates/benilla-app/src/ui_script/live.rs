//! The VM's live view of the client and the events a packet handler fires. 12340's verbs read the
//! live object when called (`0x60f390`) and its handlers fire their events once their changes
//! apply (`0x520fe0`), packet by packet. [`LiveView`] is the read-only [`LiveState`] over the
//! `World` a Lua call is lent; [`UiEvents`] is the queue a handler fires into, which the drain
//! empties into the VM after that handler's commands apply and before the next packet.

use benilla_ui::script::{LiveState, ScriptValue, UiScript};
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

/// What the verbs read live, a system parameter so a VM system lends it with its own access. The
/// index is optional, so an app without the net plugin still lends a view that holds nobody.
#[derive(SystemParam)]
pub(crate) struct LiveView<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
}

impl LiveState for LiveView<'_, '_> {
    fn unit_flags(&self, guid: u64) -> Option<u32> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        // The object manager's unit lookup (`ecx = 8`): a unit or a player.
        fields.is_unit().then(|| fields.unit_flags())
    }
}

/// The events this frame's handlers fired, in order; the drain empties it after each handler.
#[derive(Resource, Default)]
pub(crate) struct UiEvents(Vec<(&'static str, Vec<ScriptValue>)>);

impl UiEvents {
    /// Fire `event` once the calling handler's changes have applied, before the next packet's.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no stock handler fires through it yet")
    )]
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
struct LiveViewState(SystemState<LiveView<'static, 'static>>);

/// Run `f` on the VM with the `World` lent as its live view; `None` with no VM.
pub(crate) fn with_live_world<R>(
    world: &mut World,
    f: impl FnOnce(&mut UiScript) -> R,
) -> Option<R> {
    let mut script = world.remove_non_send_resource::<UiScript>()?;
    if !world.contains_resource::<LiveViewState>() {
        let state = SystemState::new(world);
        world.insert_resource(LiveViewState(state));
    }
    let out = world.resource_scope(|world, mut state: Mut<LiveViewState>| {
        let view = state.0.get(world);
        script.with_live(&view, f)
    });
    world.insert_non_send_resource(script);
    Some(out)
}

/// Fires what the handler that just ran queued, in its order, with the `World` lent; with no VM
/// they are dropped, as the reference has no listener before its UI loads.
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
