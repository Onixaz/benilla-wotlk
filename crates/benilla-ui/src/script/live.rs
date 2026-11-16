//! The client state the verbs read at call time. 12340's verbs read the live object when called
//! (`UnitOnTaxi` `0x60f350` reads the flags word at `0x60f390`); here the app lends the VM a
//! [`LiveState`] over its own state for the length of one call into Lua ([`UiScript::with_live`]),
//! and a verb reads it through [`read_live`]. No stock verb reads it yet: a verb moves to it once
//! every call into Lua that can reach it lends one, so it never answers two ways in one frame.

use std::cell::Cell;
use std::ptr::NonNull;

use mlua::Lua;

use super::UiScript;

/// What the verbs read live, implemented by the app over its state. Read-only.
pub trait LiveState {
    /// A held unit's flags word (`UNIT_FIELD_FLAGS`); `None` for a guid that is no held unit.
    fn unit_flags(&self, guid: u64) -> Option<u32>;
}

/// The lent view, its lifetime erased: set only inside [`UiScript::with_live`], which puts the
/// previous one back before the borrow ends.
#[derive(Default)]
pub(crate) struct LiveSlot(Cell<Option<NonNull<dyn LiveState>>>);

/// Puts the slot back as it was, on return and on unwind.
struct Restore<'a> {
    lua: &'a Lua,
    previous: Option<NonNull<dyn LiveState>>,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.lua.app_data_ref::<LiveSlot>() {
            slot.0.set(self.previous);
        }
    }
}

impl UiScript {
    /// Run `f` with `live` lent to every verb it reaches; the loan ends before this returns, and a
    /// nested call lends its own and restores the outer one.
    pub fn with_live<R>(&mut self, live: &dyn LiveState, f: impl FnOnce(&mut Self) -> R) -> R {
        if self.lua.app_data_ref::<LiveSlot>().is_none() {
            self.lua.set_app_data(LiveSlot::default());
        }
        let ptr: NonNull<dyn LiveState + '_> = NonNull::from(live);
        // SAFETY: only the lifetime is erased; `Restore` takes the pointer out of the slot before
        // `live`'s borrow ends, and `read` hands it out only for the length of one closure.
        let ptr: NonNull<dyn LiveState + 'static> = unsafe { std::mem::transmute(ptr) };
        let previous = self
            .lua
            .app_data_ref::<LiveSlot>()
            .expect("slot set above")
            .0
            .replace(Some(ptr));
        let lua = self.lua.clone();
        let _restore = Restore {
            lua: &lua,
            previous,
        };
        f(self)
    }

    /// Whether a view is lent right now.
    pub fn has_live(&self) -> bool {
        is_lent(&self.lua)
    }
}

/// Whether a view is lent to `lua` right now.
pub(crate) fn is_lent(lua: &Lua) -> bool {
    lua.app_data_ref::<LiveSlot>()
        .is_some_and(|s| s.0.get().is_some())
}

/// `f` over the view lent to `lua`, `None` with none lent.
pub fn read_live<R>(lua: &Lua, f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    let ptr = lua.app_data_ref::<LiveSlot>()?.0.get()?;
    // SAFETY: a pointer in the slot is a live borrow held by an enclosing `with_live`, which
    // outlives this call; the reference cannot escape `f`.
    let live = unsafe { ptr.as_ref() };
    Some(f(live))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view whose one held unit carries `flags`.
    struct One {
        guid: u64,
        flags: u32,
    }

    impl LiveState for One {
        fn unit_flags(&self, guid: u64) -> Option<u32> {
            (guid == self.guid).then_some(self.flags)
        }
    }

    #[test]
    fn a_view_is_lent_for_the_call_only_and_a_nested_loan_restores_the_outer() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.has_live());
        let outer = One { guid: 1, flags: 7 };
        let inner = One { guid: 1, flags: 9 };
        s.with_live(&outer, |s| {
            assert_eq!(read_live(s.lua(), |l| l.unit_flags(1)), Some(Some(7)));
            s.with_live(&inner, |s| {
                assert_eq!(read_live(s.lua(), |l| l.unit_flags(1)), Some(Some(9)));
            });
            assert_eq!(read_live(s.lua(), |l| l.unit_flags(1)), Some(Some(7)));
        });
        assert!(!s.has_live());
        assert_eq!(read_live(s.lua(), |l| l.unit_flags(1)), None);
    }

    /// A Lua function reaches the view lent around the call that runs it, and nothing outside one.
    #[test]
    fn a_lua_function_reads_the_view_lent_around_its_call() {
        let mut s = UiScript::new().unwrap();
        let flags = s
            .lua()
            .create_function(|lua, guid: u64| Ok(read_live(lua, |l| l.unit_flags(guid)).flatten()))
            .unwrap();
        s.lua().globals().set("Flags", flags).unwrap();
        let ask = |s: &UiScript| s.eval::<Option<u32>>("return Flags(2)").unwrap();
        assert_eq!(ask(&s), None);
        let riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&riding, |s| ask(s)), Some(1 << 20));
        assert_eq!(ask(&s), None);
    }

    /// `UnitOnTaxi` does not read the view yet: lent or not, it answers the pushed ride flag, for
    /// the player only.
    #[test]
    fn unit_on_taxi_answers_the_push_with_a_view_lent() {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            target: 2,
            held: [(1, 0), (2, 0)].into(),
            ..Default::default()
        });
        s.set_on_taxi(true);
        let ask = |s: &UiScript| {
            s.eval::<String>(
                r#"return tostring(UnitOnTaxi("player")) .. tostring(UnitOnTaxi("target"))"#,
            )
            .unwrap()
        };
        assert_eq!(ask(&s), "1nil");
        let riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&riding, |s| ask(s)), "1nil");
    }

    #[test]
    fn an_unwinding_call_ends_the_loan() {
        let mut s = UiScript::new().unwrap();
        let view = One { guid: 1, flags: 7 };
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.with_live(&view, |_| panic!("a call that unwinds"));
        }));
        assert!(unwound.is_err());
        assert!(!s.has_live());
    }
}
