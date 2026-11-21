//! The host runtime loop: event fan-out ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]), which advances the `GetTime()` clock ([`UiScript::now`]).

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{event, ScriptValue, UiScript};

impl UiScript {
    /// Hold an event for the start of the next tick, behind everything held earlier and before that
    /// tick's `OnUpdate` pass: the door of a host lane whose drain runs a step behind its input (the
    /// sound device change). A verb's own event fires at its step instead
    /// (`ModelGuard::raise`).
    pub fn queue_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        self.model_mut().defer_event(event, args);
    }

    /// Fire an event to every frame registered for it, shown or not, setting both the
    /// `this`/`event`/`arg1..argN` globals and the `(self, event, ...)` arguments; handler errors
    /// go to [`UiScript::errors`]. Frames fire in registration order, as the event signal walks its
    /// tail-appended list, and consumers rely on it: both ZoneText frames write `PVPInfoTextString`
    /// on one event, and the last one wins.
    ///
    /// The walk steps by the next listener saved before each handler runs, so a handler that
    /// unregisters itself does not skip its successor. Deviation: a handler that unregisters the
    /// saved next ends the dispatch, because the reference then reads a freed node.
    pub fn fire_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        let before = super::epoch_trace::enabled().then(|| self.visual_epoch());
        fire_event_into(&self.lua, event, args);
        if let Some(before) = before {
            super::epoch_trace::note_event(event, self.visual_epoch().wrapping_sub(before));
        }
    }

    /// The combat edge (`0x524600`): protected actions allowed and then `PLAYER_REGEN_ENABLED`, or
    /// `PLAYER_REGEN_DISABLED` and then protected actions barred.
    pub fn set_regen_enabled(&mut self, enabled: bool) {
        if enabled {
            self.model_mut().protected_actions_allowed = true;
            self.fire_event("PLAYER_REGEN_ENABLED", Vec::new());
        } else {
            self.fire_event("PLAYER_REGEN_DISABLED", Vec::new());
            // After the event's listeners, also when a host step queues the event in turn.
            super::world_loan::in_turn_or_now(self, |s| {
                s.model_mut().protected_actions_allowed = false;
            });
        }
    }
}

/// Every event this thread's VMs signalled, in order, for a test to read.
#[cfg(test)]
pub(crate) mod fired {
    use crate::script::ScriptValue;

    thread_local! {
        static LOG: std::cell::RefCell<Vec<(String, Vec<ScriptValue>)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(super) fn note(event: &str, args: &[ScriptValue]) {
        LOG.with(|l| l.borrow_mut().push((event.to_string(), args.to_vec())));
    }

    /// The events signalled since the last take, with their arguments.
    pub(crate) fn take() -> Vec<(String, Vec<ScriptValue>)> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    /// The names of the events signalled since the last take.
    pub(crate) fn names() -> Vec<String> {
        take().into_iter().map(|(e, _)| e).collect()
    }
}

/// [`UiScript::fire_event`] for a caller holding `&Lua`, such as a Lua binding.
pub(crate) fn fire_event_into(lua: &Lua, event: &str, args: Vec<ScriptValue>) {
    // A host step that holds the lent state queues its raise in turn.
    let Some(args) = super::world_loan::queue_in_turn(lua, event, args) else {
        return;
    };
    let model_mut = || crate::script::model_mut_of(lua);
    #[cfg(test)]
    fired::note(event, &args);
    let vm = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data set");
        if model.vm.fires(event) {
            model
                .event_signals
                .set(model.event_signals.get().wrapping_add(1));
        }
        model.vm
    };
    if !vm.fires(event) {
        return;
    }
    {
        let mut at = {
            let model = model_mut();
            model
                .event_to_frames
                .get(event)
                .and_then(|l| l.first().copied())
        };
        while let Some(h) = at {
            let mut model = model_mut();
            // A saved next that the previous handler unregistered ends the walk.
            let Some(pos) = model
                .event_to_frames
                .get(event)
                .and_then(|l| l.iter().position(|&x| x == h))
            else {
                break;
            };
            let next = model
                .event_to_frames
                .get(event)
                .and_then(|l| l.get(pos + 1).copied());
            let id = model.frame_id(h);
            drop(model);
            if let Err(e) = event::fire_event_handler(lua, id, event, &args) {
                model_mut().record_lua_error(&e);
            }
            at = next;
        }
    }
    event::fire_all_event_listeners(lua, event, &args);
}

impl super::UiScript {
    /// Advance a frame as the reference's pump does (the UI root's layer update `0x495320`): the
    /// `GetTime()` clock and queued events, the layout drained (the pending resizes), the update
    /// walk ([`Self::update_walk`]), the layout drained again, then the hover re-pick; then the
    /// model panes' paint callbacks.
    pub fn tick(&mut self, elapsed: f32) {
        self.tick_anchored(elapsed, std::time::Instant::now());
    }

    /// [`Self::tick`], its `GetTime` clock anchored at `anchor`, the host's instant whose steps the
    /// clock's deltas are: every stored `Instant` then maps onto `GetTime` alike at every read, as
    /// the reference's stored starts do.
    pub fn tick_anchored(&mut self, elapsed: f32, anchor: std::time::Instant) {
        // The clock is no extract input and a drained queue is no change: the mutation count
        // stands ([`ModelCell`]); the events fired below bump it through their verbs.
        let quiet = self.visual_epoch();
        let quiet_at_entry = quiet;
        {
            let mut model = self.model_mut();
            model.now += f64::from(elapsed);
            // The layer update first closes every attribute-change allowance (`0x495320`).
            model.attribute_changes_allowed.clear();
        }
        self.model_ref().clock_wall.set(anchor);
        // The socketed item's changes queue first, the reference signalling them as they land.
        self.watch_socket_item();
        // Events queued since the last tick fire before this tick's walk.
        let pending = drain_field!(self, pending_events);
        self.restore_epoch(quiet);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        self.resolve();
        let tracing = super::epoch_trace::enabled();
        if tracing {
            let _ = self.lua.set_hook(
                mlua::HookTriggers {
                    on_calls: true,
                    on_returns: true,
                    ..mlua::HookTriggers::new()
                },
                |lua, debug| {
                    super::epoch_trace::hook(lua, debug);
                    Ok(mlua::VmState::Continue)
                },
            );
        }
        let sweep = match event::UpdateSweep::new(&self.lua) {
            Ok(sweep) => Some(sweep),
            Err(e) => {
                self.push_error(e);
                None
            }
        };
        let in_handlers = self.update_walk(elapsed, sweep.as_ref());
        if tracing {
            self.lua.remove_hook();
        }
        let mut leg_at = tracing.then(|| self.visual_epoch());
        let mut leg = |me: &Self, name: &'static str| {
            if let Some(at) = leg_at.as_mut() {
                let now = me.visual_epoch();
                super::epoch_trace::note_leg(name, now.wrapping_sub(*at));
                *at = now;
            }
        };
        self.resolve();
        leg(self, "layout drain");
        let quiet = self.visual_epoch();
        // A frame hid or showed under a still cursor: re-run the hover walk at the saved position,
        // as the pump's tail does on its focus check. The hidden frame's OnLeave already fired, so
        // only the new winner's `OnEnter` fires.
        let repick = {
            let mut model = self.model_mut();
            let due = model.hover_repick;
            model.hover_repick = false;
            due.then_some(model.cursor_pos)
        };
        if repick.is_none() {
            self.restore_epoch(quiet);
        }
        if let Some((x, y)) = repick {
            self.mouse_move(x, y);
        }
        leg(self, "hover repick");
        self.paint_model_panes();
        leg(self, "model panes");
        // `WOW_UI_HANDLERS=<secs>`: last, so its report covers everything this tick fired.
        self.report_handler_profile(elapsed);
        leg(self, "handler profile");
        if tracing {
            let total = self.visual_epoch().wrapping_sub(quiet_at_entry);
            super::epoch_trace::note_legs(total.saturating_sub(in_handlers));
            let now = self.now();
            super::epoch_trace::report(now, |id| {
                let model = self.model_ref();
                model
                    .id_to_frame
                    .get(&id)
                    .and_then(|h| model.arena.frame(*h))
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| format!("frame#{id}"))
            });
        }
    }
}

impl UiScript {
    /// The model panes' paint (`0x95fc30`), in draw order: a pane with a model, a resolved rect and
    /// a screen-clamped viewport with area fires `OnUpdateModel` (`0x95fba0`), then its scene
    /// animates, which fires `OnAnimFinished` (`0x95f370`) once per arm when the armed sequence
    /// completes naturally, a loop's first pass included.
    fn paint_model_panes(&mut self) {
        // The roster compaction is bookkeeping: the count stands.
        let quiet = self.visual_epoch();
        let panes: Vec<(FrameHandle, u32, bool)> = {
            let mut model = self.model_mut();
            // Kept by `SetScript`; a destroyed frame's handle compacts out on its first miss.
            if model
                .on_update_model_frames
                .iter()
                .any(|&h| model.arena.glance(h).is_none())
            {
                let arena = &model.arena;
                let live: Vec<FrameHandle> = model
                    .on_update_model_frames
                    .iter()
                    .copied()
                    .filter(|&h| arena.glance(h).is_some())
                    .collect();
                model.on_update_model_frames = live;
            }
            let screen = model.screen;
            let mut panes: Vec<((crate::order::Strata, u16, u32), FrameHandle, bool)> = model
                .arena
                .ticked_kinds()
                .iter()
                .copied()
                .filter_map(|h| {
                    let g = model.arena.glance(h)?;
                    if !(g.visible && g.kind.is_model()) {
                        return None;
                    }
                    let crate::widget::KindState::Model(m) = &model.arena.frame(h)?.kind_state
                    else {
                        return None;
                    };
                    m.path.as_ref()?;
                    let rect = model.resolved.get(&h)?;
                    let view = super::clip::intersect_rect(*rect, screen);
                    if view.width() <= 0.0 || view.height() <= 0.0 {
                        return None;
                    }
                    Some((g.walk_key(), h, model.on_update_model_frames.contains(&h)))
                })
                .collect();
            panes.sort_unstable_by_key(|&(key, ..)| key);
            panes
                .into_iter()
                .map(|(_, h, handler)| (h, model.frame_id(h), handler))
                .collect()
        };
        self.restore_epoch(quiet);
        for (h, id, handler) in panes {
            if handler {
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnUpdateModel", Vec::new())
                {
                    self.push_error(e);
                }
            }
            // Completion is read after `OnUpdateModel`, whose handler may re-arm.
            let finished = {
                let model = self.model_ref();
                let Some(crate::widget::KindState::Model(m)) =
                    model.arena.frame(h).map(|f| &f.kind_state)
                else {
                    continue;
                };
                let Some(path) = m.path.as_deref() else {
                    continue;
                };
                model
                    .model_facts
                    .get(&crate::widget::model_key(path))
                    .is_some_and(|facts| m.completion_due(facts))
            };
            if !finished {
                continue;
            }
            if let Some(crate::widget::KindState::Model(m)) = self
                .model_mut()
                .arena
                .frame_mut(h)
                .map(|f| &mut f.kind_state)
            {
                if let Some(a) = &mut m.armed {
                    a.finished = true;
                }
            }
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnAnimFinished", Vec::new())
            {
                self.push_error(e);
            }
        }
    }
}

/// The events that do not fire at their step: every push onto the deferred queue, by file. The
/// counts can only fall.
#[cfg(test)]
mod pending_census_tests {
    /// `(file under the workspace's crates, needle, sites, why it defers)`.
    const DEFERRED_PUSHES: &[(&str, &str, usize, &str)] = &[
        (
            "benilla-ui/src/script/model.rs",
            "pending_events.push(",
            1,
            "the one door, `Model::defer_event`",
        ),
        (
            "benilla-ui/src/script/live.rs",
            "defer_event(",
            1,
            "a unique signal with no queue lent: 12340 queues it (`0x615020`) and sends it after the frame's packets (`0x614760`)",
        ),
        (
            "benilla-ui/src/script/tick.rs",
            "defer_event(",
            1,
            "`UiScript::queue_event`, the host lanes' door",
        ),
        (
            "benilla-app/src/sound/ui.rs",
            ".queue_event(",
            1,
            "a host lane: the sound device change (#1368)",
        ),
    ];

    /// The needles the census counts: the model's queue and its door in this crate, the host's door in both.
    const NEEDLES: &[&str] = &["pending_events.push(", "defer_event(", ".queue_event("];

    /// A file's text above its test module, comments dropped.
    fn code(path: &std::path::Path) -> String {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .take_while(|l| {
                !l.starts_with("mod pending_census_tests") && !l.starts_with("mod tests")
            })
            .filter(|l| !l.trim_start().starts_with("//"))
            .map(|l| {
                l.replace("fn defer_event(", "")
                    .replace("fn queue_event(", "")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every deferred push in the two crates is a listed row, at no more than its count.
    #[test]
    fn every_deferred_push_is_listed_and_none_grows() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crates directory");
        let mut files = Vec::new();
        for root in ["benilla-ui/src", "benilla-app/src"] {
            walk(&crates.join(root), &mut files);
        }
        assert!(files.len() > 100, "the walk found the sources");
        let mut unlisted = Vec::new();
        for path in files {
            let rel = path
                .strip_prefix(crates)
                .expect("under crates")
                .to_string_lossy()
                .replace('\\', "/");
            // A test file's pushes are its own fixtures.
            if rel.contains("/tests/") || rel.ends_with("tests.rs") || rel.contains("_gate") {
                continue;
            }
            let text = code(&path);
            // The app reaches the model's queue only through the host's door.
            let needles = if rel.starts_with("benilla-app/") {
                &NEEDLES[2..]
            } else {
                NEEDLES
            };
            for needle in needles {
                let found = text.matches(needle).count();
                let allowed = DEFERRED_PUSHES
                    .iter()
                    .find(|(file, n, _, _)| *file == rel && n == needle)
                    .map_or(0, |(_, _, sites, _)| *sites);
                if found > allowed {
                    unlisted.push(format!("{rel}: {found} `{needle}`, {allowed} listed"));
                }
                assert!(
                    found >= allowed,
                    "{rel}: {found} `{needle}` where {allowed} are listed: lower the row"
                );
            }
        }
        assert!(
            unlisted.is_empty(),
            "an event fires at its step (`ModelGuard::raise`), not from the deferred queue: {unlisted:#?}"
        );
    }

    /// Each row says why it defers.
    #[test]
    fn every_row_names_its_reason() {
        for (file, _, sites, why) in DEFERRED_PUSHES {
            assert!(*sites > 0, "{file}: an empty row is struck");
            assert!(
                why.contains("0x") || why.contains("#1368") || why.contains("door"),
                "{file}: {why}"
            );
        }
    }
}
