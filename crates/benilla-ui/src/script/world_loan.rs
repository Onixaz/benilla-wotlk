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

/// One loan live on this thread: a host's state lent for one closure as a scoped userdata (once
/// its scope ends mlua has destructed it, so a handle left behind only ever fails to borrow), and
/// the view builder over it; `None` takes the host's own ([`set_live_access`]).
struct Loan(AnyUserData, Option<LiveAccess>);

/// The loans live on this thread, innermost last, and the first borrow that failed under them.
#[derive(Default)]
struct Loans {
    live: Vec<Loan>,
    /// A borrow held across a raise, failed inside a listener whose guard may swallow the panic.
    held_across: Option<String>,
}

thread_local! {
    static LOANS: RefCell<Loans> = RefCell::new(Loans::default());
    /// The host's view builder: turns the innermost loan into the verbs' [`LiveState`].
    static LIVE_ACCESS: RefCell<Option<LiveAccess>> = const { RefCell::new(None) };
    /// The host's faction-list access over its own loan ([`set_reputation_access`]).
    static REPUTATION_ACCESS: RefCell<Option<ReputationAccess>> = const { RefCell::new(None) };
}

/// A VM's turn, kept in its app data: the raises the open host step queued, and the quiet steps
/// open on it, innermost last.
#[derive(Default)]
struct TurnState {
    queue: Option<Vec<Raised>>,
    quiet: Vec<&'static std::panic::Location<'static>>,
    /// Whether the open step's queue was taken with raises in it.
    taken: bool,
}

/// `f` over `lua`'s turn state, made on first use.
fn with_turn<R>(lua: &Lua, f: impl FnOnce(&mut TurnState) -> R) -> R {
    if lua.app_data_ref::<TurnState>().is_none() {
        lua.set_app_data(TurnState::default());
    }
    let mut state = lua
        .app_data_mut::<TurnState>()
        .expect("the turn state is set");
    f(&mut state)
}

/// A host step that reaches the VM's model but must run no Lua (its raises queue in turn): while
/// one is open on the VM, running a chunk or a script handler on it fails a debug build.
pub struct Quiet(Lua);

impl Quiet {
    /// Open a quiet step on `lua`, named by its caller's place.
    #[track_caller]
    pub fn begin(lua: &Lua) -> Self {
        let at = std::panic::Location::caller();
        with_turn(lua, |t| t.quiet.push(at));
        Quiet(lua.clone())
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        with_turn(&self.0, |t| {
            t.quiet.pop();
        });
    }
}

/// Fail a debug build when Lua is about to run on `lua` inside a [`Quiet`] step.
pub(crate) fn assert_not_quiet(lua: &Lua) {
    if cfg!(debug_assertions) {
        let at = lua
            .app_data_ref::<TurnState>()
            .and_then(|t| t.quiet.last().copied());
        if let Some(at) = at {
            panic!(
                "Lua ran inside a system that holds the VM in turn ({at}): a system that runs Lua \
                 is exclusive and lends the `World`"
            );
        }
    }
}

/// One raise a step made in turn: an event, or a write into the VM that must keep its place
/// among the events (the combat edge's flag `0x524600`).
pub enum Raised {
    /// An event and its arguments.
    Event(String, Vec<super::ScriptValue>),
    /// A write into the VM, run in its turn.
    Op(Box<dyn FnOnce(&mut super::UiScript) + Send + Sync>),
}

impl Raised {
    /// Fire the event, or run the write, on `script`.
    pub fn run(self, script: &mut super::UiScript) {
        match self {
            Raised::Event(event, args) => script.fire_event(&event, args),
            Raised::Op(op) => op(script),
        }
    }

    /// The event's name; `None` for a write.
    pub fn event(&self) -> Option<&str> {
        match self {
            Raised::Event(event, _) => Some(event),
            Raised::Op(_) => None,
        }
    }
}

/// Whether a host step raises on `lua` in turn: an event raised now queues behind the step's.
pub fn in_turn(lua: &Lua) -> bool {
    with_turn(lua, |t| t.queue.is_some())
}

/// Whether the open step on `lua` has raised in turn: a raise or a write waits in its queue, or
/// one was taken from it.
pub fn raised_in_turn(lua: &Lua) -> bool {
    with_turn(lua, |t| {
        t.taken || t.queue.as_ref().is_some_and(|q| !q.is_empty())
    })
}

/// Run `op` on `script` in its turn: queued behind the step's raises while a step raises in turn
/// ([`raises_in_turn`]), at once otherwise.
pub fn in_turn_or_now(
    script: &mut super::UiScript,
    op: impl FnOnce(&mut super::UiScript) + Send + Sync + 'static,
) {
    let mut op = Some(op);
    with_turn(script.lua(), |t| {
        if let Some(q) = t.queue.as_mut() {
            q.extend(op.take().map(|op| Raised::Op(Box::new(op))));
        }
    });
    if let Some(op) = op {
        op(script);
    }
}

/// [`in_turn_or_now`] for a caller holding `&Lua`, such as a model write that fires a widget
/// handler: queued in turn while a host step raises, run at once otherwise.
pub(crate) fn lua_in_turn_or_now(lua: &Lua, op: impl FnOnce(&Lua) + Send + Sync + 'static) {
    let mut op = Some(op);
    with_turn(lua, |t| {
        if let Some(q) = t.queue.as_mut() {
            q.extend(
                op.take()
                    .map(|op| Raised::Op(Box::new(move |s: &mut super::UiScript| op(s.lua())))),
            );
        }
    });
    if let Some(op) = op {
        op(lua);
    }
}

/// A host step's turn on one VM: from [`Turn::begin`] every event raised on it queues in turn
/// instead of firing, until the turn drops and the enclosing step's queue is put back (also on
/// unwind). What is still queued at the drop is dropped: take it first ([`Turn::take`]).
pub struct Turn {
    lua: Lua,
    outer: Option<Vec<Raised>>,
    outer_taken: bool,
}

impl Turn {
    /// Begin queueing `lua`'s raises in turn.
    pub fn begin(lua: &Lua) -> Self {
        let (outer, outer_taken) = with_turn(lua, |t| {
            (t.queue.replace(Vec::new()), std::mem::take(&mut t.taken))
        });
        Turn {
            lua: lua.clone(),
            outer,
            outer_taken,
        }
    }

    /// The raises queued so far, in order; the turn keeps queueing.
    pub fn take(&mut self) -> Vec<Raised> {
        with_turn(&self.lua, |t| {
            let taken = t.queue.as_mut().map(std::mem::take).unwrap_or_default();
            t.taken |= !taken.is_empty();
            taken
        })
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let outer = self.outer.take();
        let outer_taken = self.outer_taken;
        with_turn(&self.lua, |t| {
            t.queue = outer;
            t.taken = outer_taken;
        });
    }
}

/// Run `f`, a host step that holds the lent state, with every event it raises on `lua` queued in
/// turn instead of fired; the events, in order, for the caller to fire once its borrows end.
pub fn raises_in_turn<R>(lua: &Lua, f: impl FnOnce() -> R) -> (R, Vec<Raised>) {
    let mut turn = Turn::begin(lua);
    let out = f();
    let raised = turn.take();
    (out, raised)
}

/// Queue `event` when a step raises on `lua` in turn ([`raises_in_turn`]); the arguments back
/// otherwise.
pub(crate) fn queue_in_turn(
    lua: &Lua,
    event: &str,
    args: Vec<super::ScriptValue>,
) -> Option<Vec<super::ScriptValue>> {
    with_turn(lua, |t| match t.queue.as_mut() {
        Some(q) => {
            q.push(Raised::Event(event.to_string(), args));
            None
        }
        None => Some(args),
    })
}

/// Builds the live view over the innermost loan and runs the closure on it; `false` when nothing
/// of the host's type is lent.
pub type LiveAccess = fn(&mut dyn FnMut(&mut dyn LiveState)) -> bool;

/// Name the host's view builder, read by [`super::read_live`] and [`super::write_live`] over a
/// loan made by [`lend`].
pub fn set_live_access(access: LiveAccess) {
    LIVE_ACCESS.with_borrow_mut(|a| *a = Some(access));
}

/// Runs the closure on the host's faction list and faction table over the innermost loan; `false`
/// when nothing of the host's type is lent.
pub type ReputationAccess =
    fn(&mut dyn FnMut(&mut super::FactionList, &mut dyn super::FactionTable)) -> bool;

/// Name the host's faction-list access: the list and the reputation store it writes are read by
/// the host's other views, so the host lends them apart from its view builder.
pub fn set_reputation_access(access: ReputationAccess) {
    REPUTATION_ACCESS.with_borrow_mut(|a| *a = Some(access));
}

/// `f` over the faction list and table of the innermost loan: the host's own access over a host
/// loan, the lent view's [`LiveState::reputation`] over a view; `false` with neither.
pub(crate) fn reputation(
    f: &mut dyn FnMut(&mut super::FactionList, &mut dyn super::FactionTable),
) -> bool {
    let Some(access) = LOANS.with_borrow(|l| l.live.last().map(|Loan(_, access)| *access)) else {
        return false;
    };
    if access.is_none() {
        if let Some(host) = REPUTATION_ACCESS.with_borrow(|a| *a) {
            return host(f);
        }
    }
    live(|l| l.reputation(f)).unwrap_or(false)
}

/// Pops the loan on return and on unwind.
struct Pop;

impl Drop for Pop {
    fn drop(&mut self) {
        LOANS.with_borrow_mut(|l| {
            l.live.pop();
            // A panic leaving the outermost loan carries the record itself; the next loan starts clean.
            if l.live.is_empty() && std::thread::panicking() {
                l.held_across = None;
            }
        });
    }
}

/// Run `f` with `host` lent to every [`with_lent`] it reaches; the loan ends before this returns.
pub fn lend<W: 'static, R>(lua: &Lua, host: &mut W, f: impl FnOnce() -> R) -> R {
    lend_with(lua, host, None, f)
}

/// Run `f` with `view` lent as the verbs' live view; the loan ends before this returns.
pub fn lend_live<V: LiveState + 'static, R>(lua: &Lua, view: &mut V, f: impl FnOnce() -> R) -> R {
    lend_with(lua, view, Some(view_access::<V>), f)
}

/// The view builder over a lent [`LiveState`] `V`: the view itself.
fn view_access<V: LiveState + 'static>(f: &mut dyn FnMut(&mut dyn LiveState)) -> bool {
    with_lent::<V, _>(|v| f(v)).is_some()
}

/// [`lend`], with `access` as the view builder over the loan.
fn lend_with<W: 'static, R>(
    lua: &Lua,
    host: &mut W,
    access: Option<LiveAccess>,
    f: impl FnOnce() -> R,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    lua.scope(|scope| {
        let ud = scope.create_any_userdata_ref_mut(host)?;
        LOANS.with_borrow_mut(|l| l.live.push(Loan(ud, access)));
        let _pop = Pop;
        out = f.take().map(|f| f());
        Ok(())
    })
    .expect("a scoped loan is made");
    if !lent() {
        // A handler guard may have swallowed the panic as a script error: a debug build fails here.
        if let Some(held) = LOANS.with_borrow_mut(|l| l.held_across.take()) {
            debug_assert!(false, "{held}");
        }
    }
    out.expect("the loan ran its closure")
}

/// `f` over the innermost loan of a `W`, `None` with none lent (or the innermost lends another
/// type). A borrow of a loan already borrowed panics: the caller holds it across a raise.
pub fn with_lent<W: 'static, R>(f: impl FnOnce(&mut W) -> R) -> Option<R> {
    let ud = LOANS.with_borrow(|l| l.live.last().map(|Loan(ud, _)| ud.clone()))?;
    match ud.borrow_mut_scoped::<W, R>(f) {
        Ok(r) => Some(r),
        Err(mlua::Error::UserDataBorrowMutError | mlua::Error::UserDataBorrowError) => {
            held_across(std::any::type_name::<W>())
        }
        Err(_) => None,
    }
}

/// A lent state borrowed while a borrow of it is held: recorded for the loan's end, then a panic.
fn held_across(name: &str) -> ! {
    let held = format!(
        "the lent {name} is borrowed across a raise: drop the borrow before calling into Lua, or \
         lend the reborrow again"
    );
    LOANS.with_borrow_mut(|l| l.held_across.get_or_insert_with(|| held.clone()).len());
    panic!("{held}")
}

/// Whether a loan lives on this thread.
pub fn lent() -> bool {
    depth() != 0
}

/// How many loans live on this thread, each lent inside the one before it.
pub fn depth() -> usize {
    LOANS.with_borrow(|l| l.live.len())
}

/// [`super::write_live`]'s way to the view through the innermost loan.
pub(crate) fn live<R>(f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    let access = LOANS.with_borrow(|l| l.live.last().map(|Loan(_, access)| *access))?;
    let access = access.or_else(|| LIVE_ACCESS.with_borrow(|a| *a))?;
    let mut f = Some(f);
    let mut out = None;
    access(&mut |l| out = f.take().map(|f| f(l)));
    out
}

/// [`super::read_live`]'s way to the view.
pub(crate) fn live_ref<R>(f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    live(|l| f(l))
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
                let swallowed = with_lent::<u32, _>(|_| lua.load("Bump()").exec().is_err());
                assert_eq!(
                    swallowed,
                    Some(true),
                    "the chunk returns the panic as its error"
                );
            })
        }));
        let message = held
            .err()
            .and_then(|e| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(message.contains("borrowed across a raise"), "{message}");
        assert!(!lent(), "the loan is popped on unwind");
        assert!(!lua.load("return Bump()").eval::<bool>().unwrap());
        // A held borrow whose panic propagates out of the loan leaves the next loan clean.
        let propagated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lend(&lua, &mut n, || {
                with_lent::<u32, _>(|_| with_lent::<u32, _>(|_| ()));
            })
        }));
        assert!(propagated.is_err());
        lend(&lua, &mut n, || lua.load("assert(Bump())").exec().unwrap());
    }

    /// A step's raises queue in turn, in order, nested steps each keeping their own, and nothing
    /// queues once the step returns; a turn is one VM's.
    #[test]
    fn a_steps_raises_queue_in_turn_and_nest() {
        let lua = Lua::new();
        let ((inner, ()), outer) = raises_in_turn(&lua, || {
            assert!(queue_in_turn(&lua, "A", Vec::new()).is_none());
            let inner = raises_in_turn(&lua, || {
                assert!(queue_in_turn(&lua, "B", Vec::new()).is_none());
            });
            assert!(queue_in_turn(&lua, "C", Vec::new()).is_none());
            (inner.1, ())
        });
        let names = |q: &[Raised]| {
            q.iter()
                .filter_map(|r| r.event().map(str::to_string))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&inner), ["B"]);
        assert_eq!(names(&outer), ["A", "C"]);
        assert!(queue_in_turn(&lua, "D", Vec::new()).is_some());
        let _turn = Turn::begin(&lua);
        assert!(queue_in_turn(&Lua::new(), "E", Vec::new()).is_some());
    }
}
