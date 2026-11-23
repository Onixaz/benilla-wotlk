//! The `World` lent into every call into Lua through a scoped loan: an exclusive system lends its
//! `&mut World` through [`lend`] for the length of its calls into Lua, and a verb, or the system
//! itself between its calls, borrows it through [`with_world`] at the moment it reads or writes. A
//! raise holds no borrow, so a listener's nested press takes the `World` itself, as 12340 re-enters
//! the cast entry from a `UNIT_SPELLCAST_SENT` listener before the outer GCD arm (`0x80b556`,
//! `0x80b583`); a borrow held across one panics in the listener, never aliases.

use benilla_ui::script::{world_loan, LiveState, ScriptValue, UiScript};
use bevy::prelude::*;

/// Run `f` on the VM with the `World` lent to every verb it reaches; `None` with no VM. The VM stays
/// in the `World`: `f` gets another handle on it, so a nested step reads it there too.
pub(crate) fn lend<R>(world: &mut World, f: impl FnOnce(&mut UiScript) -> R) -> Option<R> {
    let mut script = world.get_non_send_resource::<UiScript>()?.share();
    Some(lend_to(world, &mut script, f))
}

/// Run `f` on `script`, a handle on the VM held by the caller, with the `World` lent.
pub(crate) fn lend_to<R>(
    world: &mut World,
    script: &mut UiScript,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    let lua = script.lua().clone();
    world_loan::set_live_access(live_access);
    world_loan::set_reputation_access(crate::ui_reputation::live_access);
    benilla_ui::script::set_apply(&lua, crate::script_calls::apply_in_call);
    benilla_ui::script::set_answer(&lua, crate::script_calls::answer_in_call);
    world_loan::lend(&lua, world, || f(script))
}

/// One call into Lua from an exclusive system that holds the parameters `$p` of its
/// `SystemState` `$state`: the parameters are dropped, the `World` is lent for `$body` (run on the
/// VM as `$s`), and the parameters are fetched again, so no borrow of the `World` is held across
/// the call and what a verb wrote is what the system reads after it. `None` with no VM.
macro_rules! lua {
    ($world:expr, $state:expr, $p:ident, |$s:ident| $body:expr) => {{
        $crate::ui_script::safe_world::release($p);
        let out = $crate::ui_script::safe_world::lend($world, |$s| $body);
        $p = $state.get_mut($world);
        out
    }};
}
pub(crate) use lua;

/// End a system's borrow of the `World` by taking its parameters, before a call into Lua.
pub(crate) fn release<P>(_params: P) {}

/// `f` over the lent `World`, `None` with none lent; panics while the loan is already borrowed.
pub(crate) fn with_world<R>(f: impl FnOnce(&mut World) -> R) -> Option<R> {
    world_loan::with_lent::<World, R>(f)
}

/// Fire `event` with the `World` lent: the step's raise, holding no borrow.
pub(crate) fn raise(world: &mut World, event: &'static str, args: Vec<ScriptValue>) {
    lend(world, |script| script.fire_event(event, args));
}

/// The verbs' view over the lent `World`, built for the one read.
fn live_access(f: &mut dyn FnMut(&mut dyn LiveState)) -> bool {
    with_world(|w| super::live::with_world_view(w, |l| f(l))).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(world: &mut World, src: &str) {
        let script = UiScript::new().expect("a VM");
        script.run(src).expect("the chunk runs");
        world.insert_non_send_resource(script);
    }

    fn eval<T: mlua::FromLua>(world: &World, src: &str) -> T {
        world
            .non_send_resource::<UiScript>()
            .eval::<T>(src)
            .unwrap()
    }

    /// A verb reaches the lent `World` through the view (the control flag through `read_live`),
    /// and the loan ends with the call.
    #[test]
    fn a_verb_reads_the_lent_world_through_the_view() {
        let mut world = World::new();
        vm(&mut world, "");
        let lua = world.non_send_resource::<UiScript>().lua().clone();
        let f = lua
            .create_function(|_, ()| Ok(benilla_ui::script::read_live(|l| l.has_control())))
            .unwrap();
        lua.globals().set("LentHasControl", f).unwrap();
        world.insert_resource(crate::net::ControlFlag(false));
        let seen = lend(&mut world, |s| {
            s.eval::<Option<bool>>("return LentHasControl()").unwrap()
        });
        assert_eq!(seen.flatten(), Some(false));
        assert_eq!(
            eval::<Option<bool>>(&world, "return LentHasControl()"),
            None
        );
    }

    /// A `World` borrow held across a raise fails the debug build where the loan ends, although
    /// the VM's handler guard turned the listener's panic into a script error.
    #[test]
    #[cfg(debug_assertions)]
    fn a_world_borrow_held_across_a_raise_is_a_hard_failure() {
        let mut world = World::new();
        vm(
            &mut world,
            r#"local f = CreateFrame("Frame")
               f:RegisterEvent("PLAYER_ENTERING_WORLD")
               f:SetScript("OnEvent", function() LentHasControl() end)"#,
        );
        let lua = world.non_send_resource::<UiScript>().lua().clone();
        let f = lua
            .create_function(|_, ()| Ok(benilla_ui::script::read_live(|l| l.has_control())))
            .unwrap();
        lua.globals().set("LentHasControl", f).unwrap();
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lend(&mut world, |script| {
                with_world(|_held| script.fire_event("PLAYER_ENTERING_WORLD", Vec::new()));
            })
        }));
        let message = held
            .err()
            .and_then(|e| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(message.contains("borrowed across a raise"), "{message}");
    }
}
