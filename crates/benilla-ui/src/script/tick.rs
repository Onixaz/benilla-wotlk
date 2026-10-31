//! The host runtime loop: event fan-out ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]), which advances the `GetTime()` clock ([`UiScript::now`]).

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{event, ScriptValue, UiScript};

impl UiScript {
    /// Queue an event for the start of the next tick, behind everything queued earlier and before
    /// that tick's `OnUpdate` pass. A drain runs a step behind its input, so firing at once would
    /// overtake events that input already queued: the soulbind confirm for an item placed into a
    /// worn slot must follow that placement's `CURSOR_UPDATE`, whose handler hides it
    /// (`UIParent.lua:357-361`).
    pub fn queue_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        self.model_mut()
            .pending_events
            .push((event.to_string(), args));
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
}

/// [`UiScript::fire_event`] for a caller holding `&Lua`, such as a Lua binding.
pub(crate) fn fire_event_into(lua: &Lua, event: &str, args: Vec<ScriptValue>) {
    let model_mut = || crate::script::model_mut_of(lua);
    let vm = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data set")
        .vm;
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
                model_mut().record_script_error(e.to_string());
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
        // The clock is no extract input and a drained queue is no change: the mutation count
        // stands ([`ModelCell`]); the events fired below bump it through their verbs.
        let quiet = self.visual_epoch();
        let quiet_at_entry = quiet;
        self.model_mut().now += f64::from(elapsed);
        // The socketed item's changes queue first, the reference signalling them as they land.
        self.watch_socket_item();
        // Events queued since the last tick fire before this tick's walk.
        let pending = drain_field!(self, pending_events);
        self.restore_epoch(quiet);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        self.expire_quest_watches();
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
