//! The host's state lent into a call into Lua with no `unsafe`: [`lend`] wraps the host's `&mut`
//! in an mlua scoped userdata for the length of one closure, and a verb (or the host itself, between
//! its own calls) reaches it through [`with_lent`], a runtime-checked borrow that cannot outlive
//! the closure. A borrow held across a raise makes the listener's borrow fail, which panics: never
//! two `&mut` to one state. The VM's handler guard turns that panic into a script error, so a debug
//! build panics again where the outermost loan ends. A step that holds the loan and raises lends its
//! own reborrow again, so a listener's nested press runs inside the raise, as 12340 re-enters the
//! cast entry from a `UNIT_SPELLCAST_SENT` listener (`0x80b556` before the GCD arm `0x80b583`).

use std::cell::RefCell;

use mlua::{AnyUserData, Lua};

use super::LiveState;

thread_local! {
    /// The loans live on this thread, innermost last. Each is a scoped userdata: once its scope ends
    /// mlua has destructed it, so a handle left behind only ever fails to borrow.
    static LOANS: RefCell<Vec<AnyUserData>> = const { RefCell::new(Vec::new()) };
    /// The host's view builder: turns the innermost loan into the verbs' [`LiveState`].
    static LIVE_ACCESS: RefCell<Option<LiveAccess>> = const { RefCell::new(None) };
    /// The first borrow that failed under the loans live now: a borrow held across a raise.
    static HELD_ACROSS: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The events a host step raised while it held the lent state, queued in turn.
    static IN_TURN: RefCell<Option<Vec<Raised>>> = const { RefCell::new(None) };
}

/// One event a step raised in turn: its name and arguments.
pub type Raised = (String, Vec<super::ScriptValue>);

/// Puts the enclosing step's queue back on return and on unwind.
struct Restore(Option<Vec<Raised>>);

impl Drop for Restore {
    fn drop(&mut self) {
        let outer = self.0.take();
        IN_TURN.with_borrow_mut(|q| *q = outer);
    }
}

/// Run `f`, a host step that holds the lent state, with every event it raises queued in turn
/// instead of fired; the events, in order, for the caller to fire once its borrows end.
pub fn raises_in_turn<R>(f: impl FnOnce() -> R) -> (R, Vec<Raised>) {
    let restore = Restore(IN_TURN.with_borrow_mut(|q| q.replace(Vec::new())));
    let out = f();
    let raised = IN_TURN.with_borrow_mut(|q| q.take()).unwrap_or_default();
    drop(restore);
    (out, raised)
}

/// Queue `event` when a step raises in turn ([`raises_in_turn`]); the arguments back otherwise.
pub(crate) fn queue_in_turn(
    event: &str,
    args: Vec<super::ScriptValue>,
) -> Option<Vec<super::ScriptValue>> {
    IN_TURN.with_borrow_mut(|q| match q {
        Some(q) => {
            q.push((event.to_string(), args));
            None
        }
        None => Some(args),
    })
}

/// Builds the live view over the innermost loan and runs the closure on it; `false` when nothing
/// of the host's type is lent.
pub type LiveAccess = fn(&mut dyn FnMut(&mut dyn LiveState)) -> bool;

/// Name the host's view builder, read by [`super::read_live`] and [`super::write_live`] when no
/// [`super::LiveLoan`] is lent.
pub fn set_live_access(access: LiveAccess) {
    LIVE_ACCESS.with_borrow_mut(|a| *a = Some(access));
}

/// Pops the loan on return and on unwind.
struct Pop;

impl Drop for Pop {
    fn drop(&mut self) {
        LOANS.with_borrow_mut(|l| {
            l.pop();
        });
    }
}

/// Run `f` with `host` lent to every [`with_lent`] it reaches; the loan ends before this returns.
pub fn lend<W: 'static, R>(lua: &Lua, host: &mut W, f: impl FnOnce() -> R) -> R {
    let mut f = Some(f);
    let mut out = None;
    lua.scope(|scope| {
        let ud = scope.create_any_userdata_ref_mut(host)?;
        LOANS.with_borrow_mut(|l| l.push(ud));
        let _pop = Pop;
        out = f.take().map(|f| f());
        Ok(())
    })
    .expect("a scoped loan is made");
    if !lent() {
        // A handler guard may have swallowed the panic as a script error: a debug build fails here.
        if let Some(held) = HELD_ACROSS.with_borrow_mut(Option::take) {
            debug_assert!(false, "{held}");
        }
    }
    out.expect("the loan ran its closure")
}

/// `f` over the innermost loan of a `W`, `None` with none lent (or the innermost lends another
/// type). A borrow of a loan already borrowed panics: the caller holds it across a raise.
pub fn with_lent<W: 'static, R>(f: impl FnOnce(&mut W) -> R) -> Option<R> {
    let ud = LOANS.with_borrow(|l| l.last().cloned())?;
    match ud.borrow_mut_scoped::<W, R>(f) {
        Ok(r) => Some(r),
        Err(mlua::Error::UserDataBorrowMutError | mlua::Error::UserDataBorrowError) => {
            let held = format!(
                "the lent {} is borrowed across a raise: drop the borrow before calling into Lua, \
                 or lend the reborrow again",
                std::any::type_name::<W>()
            );
            HELD_ACROSS.with_borrow_mut(|h| h.get_or_insert_with(|| held.clone()).len());
            panic!("{held}")
        }
        Err(_) => None,
    }
}

/// Whether a loan lives on this thread.
pub fn lent() -> bool {
    depth() != 0
}

/// How many loans live on this thread, each lent inside the one before it.
pub fn depth() -> usize {
    LOANS.with_borrow(Vec::len)
}

/// [`super::read_live`]'s and [`super::write_live`]'s way to the host's view through the loan.
pub(crate) fn live<R>(f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    let access = LIVE_ACCESS.with_borrow(|a| *a)?;
    let mut f = Some(f);
    let mut out = None;
    access(&mut |l| out = f.take().map(|f| f(l)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loan reaches a Lua-called function, ends with its closure, and a borrow held across a
    /// call into Lua makes the inner borrow panic, where a re-lent reborrow reaches it.
    #[test]
    fn a_loan_is_scoped_runtime_checked_and_relendable() {
        let lua = Lua::new();
        let bump = lua
            .create_function(|_, ()| Ok(with_lent::<u32, _>(|n| *n += 1).is_some()))
            .unwrap();
        lua.globals().set("Bump", bump).unwrap();
        let mut n = 0u32;
        lend(&lua, &mut n, || {
            lua.load("assert(Bump())").exec().unwrap();
            // The host's own short borrow between calls.
            assert_eq!(with_lent::<u32, _>(|n| *n), Some(1));
            // Lent again from inside a borrow: the listener reaches the reborrow.
            with_lent::<u32, _>(|n| lend(&lua, n, || lua.load("Bump()").exec().unwrap()));
        });
        assert_eq!(n, 2);
        assert!(!lent());
        // The chunk's error is dropped, as a handler guard drops it: the loan's end still fails.
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lend(&lua, &mut n, || {
                with_lent::<u32, _>(|_| {
                    let _ = lua.load("Bump()").exec();
                });
            })
        }));
        let message = held
            .err()
            .and_then(|e| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(message.contains("borrowed across a raise"), "{message}");
        assert!(!lent(), "the loan is popped on unwind");
        assert!(!lua.load("return Bump()").eval::<bool>().unwrap());
    }

    /// A step's raises queue in turn, in order, nested steps each keeping their own, and nothing
    /// queues once the step returns.
    #[test]
    fn a_steps_raises_queue_in_turn_and_nest() {
        let ((inner, ()), outer) = raises_in_turn(|| {
            assert!(queue_in_turn("A", Vec::new()).is_none());
            let inner = raises_in_turn(|| {
                assert!(queue_in_turn("B", Vec::new()).is_none());
            });
            assert!(queue_in_turn("C", Vec::new()).is_none());
            (inner.1, ())
        });
        let names = |q: &[Raised]| q.iter().map(|(e, _)| e.clone()).collect::<Vec<_>>();
        assert_eq!(names(&inner), ["B"]);
        assert_eq!(names(&outer), ["A", "C"]);
        assert!(queue_in_turn("D", Vec::new()).is_some());
    }
}
