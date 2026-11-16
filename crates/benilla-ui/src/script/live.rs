//! The client state the verbs read at call time. 12340's verbs read the live object when called
//! (`UnitOnTaxi` `0x60f350` reads the flags word at `0x60f396`); here the app lends a [`LiveState`]
//! over its own state for the length of every call into Lua, either around one call
//! ([`UiScript::with_live`]) or for as long as a guard lives ([`LiveLoan`]), and a verb reads it
//! through [`read_live`]. The loan is the calling thread's: the VM runs on one thread, and a test
//! thread's loan never reaches another's.

use std::cell::Cell;
use std::ptr::NonNull;

use super::UiScript;

/// What the verbs read live, implemented by the app over its state. Read-only.
pub trait LiveState {
    /// A held unit's flags word (`UNIT_FIELD_FLAGS`); `None` for a guid that is no held unit.
    fn unit_flags(&self, guid: u64) -> Option<u32>;

    /// A held unit's health (`UNIT_FIELD_HEALTH`); `None` for a guid that is no held unit.
    fn unit_health(&self, guid: u64) -> Option<u32>;

    /// The active player's guid (`0x4d3790`), `None` before our create names one.
    fn active_player(&self) -> Option<u64>;

    /// The UI's control flag (`0xbcfb8c`), which the control update for our own unit writes
    /// (`0x520fe0`) and a world entry sets (`0x513880`).
    fn has_control(&self) -> bool;

    /// Whether our view looks through a far-sight subject: the bit set as `CMSG_FAR_SIGHT` 1 goes
    /// out (`0x6e29b5`) and cleared ahead of the 0 (`0x6e2a65`), which `HasFullControl` refuses on.
    fn far_sight(&self) -> bool;
}

/// A lent view's address, its lifetime erased.
type Lent = Option<NonNull<dyn LiveState>>;

thread_local! {
    /// The view lent on this thread: set only by [`UiScript::with_live`] and [`LiveLoan`], each of
    /// which puts the previous one back before its borrow ends.
    static SLOT: Cell<Lent> = const { Cell::new(None) };
}

/// Lend `live` on this thread, returning what it replaces.
///
/// # Safety
/// The caller puts the returned value back with [`restore`] before `live`'s borrow ends.
unsafe fn lend(live: &dyn LiveState) -> Lent {
    let ptr: NonNull<dyn LiveState + '_> = NonNull::from(live);
    // SAFETY: only the lifetime is erased; the caller restores the slot before the borrow ends,
    // and `read_live` hands the view out only for the length of one closure.
    let ptr: NonNull<dyn LiveState + 'static> = unsafe { std::mem::transmute(ptr) };
    SLOT.with(|s| s.replace(Some(ptr)))
}

fn restore(previous: Lent) {
    SLOT.with(|s| s.set(previous));
}

/// Puts the slot back as it was, on return and on unwind.
struct Restore(Lent);

impl Drop for Restore {
    fn drop(&mut self) {
        restore(self.0);
    }
}

/// A view lent to every verb on this thread until the loan drops, which puts the previous one
/// back: the guard a host keeps for the length of a stretch of calls into Lua.
pub struct LiveLoan<V: LiveState> {
    /// Boxed, so the lent address holds while the loan moves.
    view: Box<V>,
    previous: Lent,
}

impl<V: LiveState> LiveLoan<V> {
    /// Lend `view` until the loan drops.
    ///
    /// # Safety
    /// The loan is dropped, never leaked, on the thread that made it, and loans on one thread
    /// drop in the reverse order of their making.
    pub unsafe fn new(view: V) -> Self {
        let view = Box::new(view);
        // SAFETY: `Drop` restores the slot before the box, and with it the view, is freed.
        let previous = unsafe { lend(&*view) };
        Self { view, previous }
    }

    /// The lent view.
    pub fn view(&self) -> &V {
        &self.view
    }
}

impl<V: LiveState> Drop for LiveLoan<V> {
    fn drop(&mut self) {
        restore(self.previous);
    }
}

impl UiScript {
    /// Run `f` with `live` lent to every verb it reaches; the loan ends before this returns, and a
    /// nested call lends its own and restores the outer one.
    pub fn with_live<R>(&mut self, live: &dyn LiveState, f: impl FnOnce(&mut Self) -> R) -> R {
        // SAFETY: `Restore` puts the slot back before `live`'s borrow ends, on unwind too.
        let _restore = Restore(unsafe { lend(live) });
        f(self)
    }

    /// Whether a view is lent right now.
    pub fn has_live(&self) -> bool {
        SLOT.with(|s| s.get().is_some())
    }
}

/// The UI's control flag through the lent view; with none lent, as a VM outside the app has, the
/// value a game start sets (`0x52b8a0`).
pub(crate) fn has_control() -> bool {
    read_live(|l| l.has_control()).unwrap_or(true)
}

/// `f` over the view lent right now, `None` with none lent.
pub fn read_live<R>(f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    let ptr = SLOT.with(Cell::get)?;
    // SAFETY: a pointer in the slot is a live borrow held by an enclosing `with_live` or
    // `LiveLoan`, which outlives this call; the reference cannot escape `f`.
    let live = unsafe { ptr.as_ref() };
    Some(f(live))
}

/// A view over fixed state, for the VM's own tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestView {
    /// The active player's guid.
    pub player: Option<u64>,
    /// Each held unit's `(flags, health)`.
    pub units: std::collections::HashMap<u64, (u32, u32)>,
    /// The control flag; the default view has lost control.
    pub control: bool,
    /// Whether the view looks through a far-sight subject.
    pub far_sight: bool,
}

#[cfg(test)]
impl LiveState for TestView {
    fn unit_flags(&self, guid: u64) -> Option<u32> {
        self.units.get(&guid).map(|u| u.0)
    }

    fn unit_health(&self, guid: u64) -> Option<u32> {
        self.units.get(&guid).map(|u| u.1)
    }

    fn active_player(&self) -> Option<u64> {
        self.player
    }

    fn has_control(&self) -> bool {
        self.control
    }

    fn far_sight(&self) -> bool {
        self.far_sight
    }
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

        fn unit_health(&self, guid: u64) -> Option<u32> {
            (guid == self.guid).then_some(1)
        }

        fn active_player(&self) -> Option<u64> {
            Some(self.guid)
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_view_is_lent_for_the_call_only_and_a_nested_loan_restores_the_outer() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.has_live());
        let outer = One { guid: 1, flags: 7 };
        let inner = One { guid: 1, flags: 9 };
        s.with_live(&outer, |s| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            s.with_live(&inner, |_| {
                assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(9)));
            });
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
        });
        assert!(!s.has_live());
        assert_eq!(read_live(|l| l.unit_flags(1)), None);
    }

    /// A Lua function reaches the view lent around the call that runs it, and nothing outside one.
    #[test]
    fn a_lua_function_reads_the_view_lent_around_its_call() {
        let mut s = UiScript::new().unwrap();
        let flags = s
            .lua()
            .create_function(|_, guid: u64| Ok(read_live(|l| l.unit_flags(guid)).flatten()))
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

    /// `UnitOnTaxi` tests bit 20 of the lent view's flags word for whichever unit the token names
    /// (`0x60f396`); nil for a unit not held, and with no view lent.
    #[test]
    fn unit_on_taxi_reads_the_lent_view_for_any_unit() {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            target: 2,
            held: [(1, 0), (2, 0)].into(),
            ..Default::default()
        });
        let ask = |s: &UiScript| {
            s.eval::<String>(
                r#"return tostring(UnitOnTaxi("player")) .. tostring(UnitOnTaxi("target"))"#,
            )
            .unwrap()
        };
        assert_eq!(ask(&s), "nilnil");
        let riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&riding, |s| ask(s)), "nil1");
        let grounded = One {
            guid: 1,
            flags: !(1 << 20),
        };
        assert_eq!(s.with_live(&grounded, |s| ask(s)), "nilnil");
    }

    /// `HasFullControl` needs the active player held, not looking through far sight, with health
    /// above zero and the control flag set (`0x611603`-`0x611637`); with no view lent it answers
    /// the boot flag.
    #[test]
    fn has_full_control_reads_the_flag_and_the_players_health() {
        let mut s = UiScript::new().unwrap();
        let ask = |s: &mut UiScript, view: &TestView| {
            s.with_live(view, |s| {
                s.eval::<String>("return tostring(HasFullControl())")
            })
            .unwrap()
        };
        assert_eq!(s.eval::<i64>("return HasFullControl()").unwrap(), 1);
        let alive = |control| TestView {
            player: Some(1),
            units: [(1, (0, 100))].into(),
            control,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &alive(true)), "1");
        assert_eq!(ask(&mut s, &alive(false)), "nil");
        let far = TestView {
            far_sight: true,
            ..alive(true)
        };
        assert_eq!(ask(&mut s, &far), "nil");
        let dead = TestView {
            player: Some(1),
            units: [(1, (0, 0))].into(),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &dead), "nil");
        let nobody = TestView {
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &nobody), "nil");
        let unheld = TestView {
            player: Some(1),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &unheld), "nil");
    }

    /// A loan lends until it drops, a call's loan inside it restores it, and dropping it restores
    /// what it replaced.
    #[test]
    fn a_loan_lends_until_it_drops() {
        let mut s = UiScript::new().unwrap();
        let outer = One { guid: 1, flags: 7 };
        // SAFETY: dropped below, on this thread, after the inner one.
        let loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        s.with_live(&outer, |_| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            // SAFETY: dropped at the end of this closure, before `loan`.
            let inner = unsafe { LiveLoan::new(One { guid: 1, flags: 9 }) };
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(9)));
            drop(inner);
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
        });
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        assert_eq!(loan.view().flags, 5);
        drop(loan);
        assert!(!s.has_live());
    }

    /// A loan is its thread's: another thread sees none.
    #[test]
    fn a_loan_is_the_lending_threads_alone() {
        // SAFETY: dropped at the end of this test, on this thread.
        let _loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        let elsewhere = std::thread::spawn(|| read_live(|l| l.unit_flags(1)))
            .join()
            .unwrap();
        assert_eq!(elsewhere, None);
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
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
