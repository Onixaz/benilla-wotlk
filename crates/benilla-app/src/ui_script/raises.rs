//! The raise queue: a system that only raises reaches the VM's model through [`InTurn`] and calls
//! no Lua. Every event it raises, and every VM write it puts in turn
//! (`world_loan::in_turn_or_now`), queues on [`UiEvents`] in its order, and the exclusive step after
//! its set fires them with the `World` lent ([`fire_raises`]), so a listener's script call applies
//! inside the listener.

use std::ops::{Deref, DerefMut};

use benilla_ui::script::world_loan::{Quiet, Raised, Turn};
use benilla_ui::script::UiScript;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::live::UiEvents;

/// The VM's model for a system that raises without calling into Lua: [`InTurn::turn`] opens the
/// model with every raise queued in turn on [`UiEvents`].
#[derive(SystemParam)]
pub(crate) struct InTurn<'w> {
    vm: Option<NonSendMut<'w, UiScript>>,
    queue: Option<ResMut<'w, UiEvents>>,
    _mark: Option<Res<'w, RaisesInTurn>>,
}

/// Never inserted: read by every [`InTurn`], so a system's declared access shows it raises in turn.
#[derive(Resource)]
pub(crate) struct RaisesInTurn;

impl InTurn<'_> {
    /// The VM's model with this system's raises queued in turn; `None` with no VM.
    #[track_caller]
    pub(crate) fn turn(&mut self) -> Option<VmTurn<'_>> {
        let script = self.vm.as_deref_mut()?;
        let turn = Some((Turn::begin(script.lua()), Quiet::begin(script.lua())));
        Some(VmTurn {
            script,
            queue: self.queue.as_deref_mut(),
            turn,
        })
    }

    /// Whether a VM is loaded.
    #[cfg(test)]
    pub(crate) fn is_some(&self) -> bool {
        self.vm.is_some()
    }

    /// Whether the VM was inserted since this system last ran: a new session's.
    pub(crate) fn is_added(&self) -> bool {
        self.vm.as_ref().is_some_and(|vm| vm.is_added())
    }
}

/// The VM's model while a system raises in turn: it derefs to the VM, every event raised through
/// it, or anywhere on this thread, queues on [`UiEvents`] when it drops, and running Lua through it
/// fails a debug build ([`Quiet`]).
pub(crate) struct VmTurn<'a> {
    script: &'a mut UiScript,
    queue: Option<&'a mut UiEvents>,
    turn: Option<(Turn, Quiet)>,
}

impl VmTurn<'_> {
    /// The handler queue behind the raises made so far, so a `ui.fire` keeps its place among them.
    #[cfg(test)]
    pub(crate) fn ui(&mut self) -> &mut UiEvents {
        self.flush();
        self.queue
            .as_deref_mut()
            .expect("a system that queues on `UiEvents` runs where the queue is initialized")
    }

    /// Move the raises queued so far onto the handler queue, in order.
    fn flush(&mut self) {
        let Some((turn, _)) = self.turn.as_mut() else {
            return;
        };
        let raised = turn.take();
        if raised.is_empty() {
            return;
        }
        if let Some(queue) = self.queue.as_deref_mut() {
            queue.write(move |script| run(script, raised));
        } else {
            // A harness with no handler queue: the raises fire as the system ends.
            self.turn = None;
            run(self.script, raised);
            self.turn = Some((
                Turn::begin(self.script.lua()),
                Quiet::begin(self.script.lua()),
            ));
        }
    }
}

/// Fire each raise in order.
fn run(script: &mut UiScript, raised: Vec<Raised>) {
    for raise in raised {
        raise.run(script);
    }
}

impl Deref for VmTurn<'_> {
    type Target = UiScript;
    fn deref(&self) -> &UiScript {
        self.script
    }
}

impl DerefMut for VmTurn<'_> {
    fn deref_mut(&mut self) -> &mut UiScript {
        self.script
    }
}

impl Drop for VmTurn<'_> {
    fn drop(&mut self) {
        self.flush();
        self.turn = None;
    }
}

/// The raise step after a set of [`InTurn`] systems: what they queued fires in order with the
/// `World` lent, before the next set runs.
pub(crate) fn fire_raises(world: &mut World) {
    super::live::fire_handler_events(world);
}

/// `holders`, [`InTurn`] systems outside the UI phases, with a raise step chained right after
/// them, run while raises wait: what they queued fires before any later system runs Lua.
pub(crate) fn then_fire<M>(
    holders: impl IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
) -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (holders, fire_raises.run_if(waiting)).chain()
}

/// Whether raises wait on the handler queue: the raise step's trigger.
pub(crate) fn waiting(queue: Option<Res<UiEvents>>) -> bool {
    queue.is_some_and(|q| !q.is_empty())
}

/// The UI phases a raise step follows: an [`InTurn`] system outside them chains its own step.
#[cfg(test)]
pub(crate) fn phases() -> [bevy::ecs::intern::Interned<dyn bevy::ecs::schedule::SystemSet>; 5] {
    use super::{UiDrain, UiFeed, UiPaint, UiPress, UiSignals};
    [
        UiPress.intern(),
        UiFeed.intern(),
        UiSignals.intern(),
        UiDrain.intern(),
        UiPaint.intern(),
    ]
}

/// The raise steps between the UI phases: what a phase's [`InTurn`] systems queued fires before
/// the next phase runs. Each step reads the queue itself: one shared condition is read once a
/// frame, at the first step, and would skip the later ones.
pub(crate) fn schedule(app: &mut App) {
    use super::{UiDrain, UiFeed, UiInput, UiPaint, UiPress, UiSignals};
    app.init_resource::<UiEvents>().add_systems(
        Update,
        (
            fire_raises.after(UiPress).before(UiFeed),
            fire_raises.after(UiFeed).before(UiSignals),
            fire_raises.after(UiSignals).before(UiInput),
            fire_raises.after(UiDrain).before(UiPaint),
            fire_raises.after(UiPaint),
        )
            .distributive_run_if(waiting),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::ScriptValue;
    use bevy::ecs::system::RunSystemOnce;

    fn vm(world: &mut World) {
        let script = UiScript::new().expect("a VM");
        script
            .run(
                r#"Seen = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("PLAYER_TARGET_CHANGED")
                   f:RegisterEvent("PLAYER_REGEN_DISABLED")
                   f:SetScript("OnEvent", function(_, e) table.insert(Seen, e) end)"#,
            )
            .expect("the chunk runs");
        world.insert_non_send_resource(script);
    }

    fn seen(world: &World) -> String {
        world
            .non_send_resource::<UiScript>()
            .eval::<String>("return table.concat(Seen, ',')")
            .unwrap()
    }

    /// A system's raises queue on the handler queue in their order, between its own `ui.fire`s, and
    /// fire at the raise step, not inside the system.
    #[test]
    fn a_systems_raises_queue_in_order_and_fire_at_the_raise_step() {
        let mut world = World::new();
        vm(&mut world);
        world.init_resource::<UiEvents>();
        world
            .run_system_once(|mut vm: InTurn| {
                let mut script = vm.turn().expect("a VM");
                script.fire_event("PLAYER_TARGET_CHANGED", Vec::new());
                script.ui().fire("PLAYER_REGEN_DISABLED", Vec::new());
                script.fire_event("PLAYER_TARGET_CHANGED", vec![ScriptValue::Nil]);
            })
            .unwrap();
        assert_eq!(seen(&world), "", "nothing fires inside the system");
        assert_eq!(
            world.resource::<UiEvents>().names(),
            ["<write>", "PLAYER_REGEN_DISABLED", "<write>"]
        );
        world.run_system_once(fire_raises).unwrap();
        assert_eq!(
            seen(&world),
            "PLAYER_TARGET_CHANGED,PLAYER_REGEN_DISABLED,PLAYER_TARGET_CHANGED"
        );
        assert!(world.resource::<UiEvents>().is_empty());
    }

    /// In a scheduled frame each phase's raises fire at the step after it: a feed's event reaches
    /// its listener before the same frame's tick, and a drain's before the paint, although the
    /// queue was empty at the step after the press.
    #[test]
    fn a_feeds_raise_reaches_its_listener_before_the_same_frames_tick() {
        use super::super::{UiDrain, UiFeed, UiInput, UiPaint, UiPress, UiSignals};
        #[derive(Resource, Default)]
        struct AtTick(Vec<String>);
        let mut app = App::new();
        app.configure_sets(
            Update,
            (UiPress, UiFeed, UiSignals, UiInput, UiDrain, UiPaint).chain(),
        )
        .add_plugins(schedule)
        .init_resource::<AtTick>();
        vm(app.world_mut());
        app.add_systems(
            Update,
            (
                (|mut vm: InTurn| {
                    if let Some(mut s) = vm.turn() {
                        s.fire_event("PLAYER_TARGET_CHANGED", Vec::new());
                    }
                })
                .in_set(UiFeed),
                (|world: &mut World| {
                    let seen = seen(world);
                    world.resource_mut::<AtTick>().0.push(seen);
                })
                .in_set(UiInput),
                (|mut vm: InTurn| {
                    if let Some(mut s) = vm.turn() {
                        s.fire_event("PLAYER_REGEN_DISABLED", Vec::new());
                    }
                })
                .in_set(UiDrain),
                (|world: &mut World| {
                    let seen = seen(world);
                    world.resource_mut::<AtTick>().0.push(seen);
                })
                .in_set(UiPaint),
            ),
        );
        app.update();
        assert_eq!(
            app.world().resource::<AtTick>().0,
            [
                "PLAYER_TARGET_CHANGED",
                "PLAYER_TARGET_CHANGED,PLAYER_REGEN_DISABLED"
            ]
        );
    }

    /// The combat edge's barring write waits behind its event's listeners in turn (`0x524600`).
    #[test]
    fn the_combat_edges_bar_queues_behind_its_event() {
        let mut world = World::new();
        vm(&mut world);
        world.init_resource::<UiEvents>();
        world
            .run_system_once(|mut vm: InTurn| {
                vm.turn().expect("a VM").set_regen_enabled(false);
            })
            .unwrap();
        let lockdown = |world: &World| {
            world
                .non_send_resource::<UiScript>()
                .eval::<bool>("return InCombatLockdown() and true or false")
                .unwrap()
        };
        assert!(!lockdown(&world));
        world.run_system_once(fire_raises).unwrap();
        assert_eq!(seen(&world), "PLAYER_REGEN_DISABLED");
        assert!(lockdown(&world));
    }
}
