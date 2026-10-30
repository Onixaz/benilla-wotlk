//! The frame update walk, the UI root's layer update (`0x495320`): each stratum low to high, each
//! level low to high, each level's shown frames oldest link first (the stratum node's walk,
//! `0x494a10`), every frame's layer update in its slot (`0x490770`): its `OnUpdate`, its
//! animations, then its kind's own update.
//!
//! The level list walk saves the next link before each frame runs, so a frame an earlier handler
//! hid is skipped, and a frame linked into the level being walked waits in its pending list for
//! the next pass; a link into a later level runs there. The walk here visits only the frames whose
//! slot does something, and keys each on its glance: a frame whose key moved since it was gathered
//! has relinked and waits, and a level that ends with the link or roster epoch moved re-gathers
//! the levels after it.

use mlua::Lua;

use super::{editbox, event, UiScript};
use crate::order::Strata;
use crate::widget::{FrameHandle, FrameKind, KindState};

type WalkKey = (Strata, u16, u32);

/// A frame the walk visits, as gathered.
#[derive(Clone, Copy)]
struct Entry {
    key: WalkKey,
    h: FrameHandle,
    /// The frame's script id, when it has an `OnUpdate`.
    on_update: Option<u32>,
    /// It or one of its regions holds a playing or paused animation group.
    animated: bool,
}

/// What can change the walk under a handler: a link or unlink, a gained or lost `OnUpdate`, a
/// started animation group.
fn walk_epoch(lua: &Lua) -> (u64, u64) {
    let m = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    (m.arena.link_epoch(), m.walk_roster)
}

/// The shown frames whose slot does something, in walk order, past the level `after` when given.
fn gather(lua: &Lua, after: Option<(Strata, u16)>) -> Vec<Entry> {
    let m = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let arena = &m.arena;
    let mut out: Vec<Entry> = Vec::new();
    let mut push = |h: FrameHandle, on_update: Option<u32>, animated: bool| {
        let Some(g) = arena.glance(h).filter(|g| g.visible) else {
            return;
        };
        let key = g.walk_key();
        if after.is_some_and(|level| (key.0, key.1) <= level) {
            return;
        }
        out.push(Entry {
            key,
            h,
            on_update,
            animated,
        });
    };
    for &(h, id) in &m.on_update_frames {
        push(h, Some(id), false);
    }
    // The kinds whose own update does work: every EditBox flushes, the fades, clocks and sweeps.
    for &h in arena
        .editbox_kinds()
        .iter()
        .chain(arena.ticked_kinds())
        .chain(arena.tooltip_kinds())
    {
        push(h, None, false);
    }
    for h in super::anim::run::animated_frames(&m) {
        push(h, None, true);
    }
    // A frame listed twice merges into one slot.
    out.sort_unstable_by_key(|e| e.key);
    out.dedup_by(|later, earlier| {
        let same = later.h == earlier.h;
        if same {
            earlier.on_update = earlier.on_update.or(later.on_update);
            earlier.animated |= later.animated;
        }
        same
    });
    out
}

impl UiScript {
    /// Run the walk; `sweep` fires the `OnUpdate`s, absent when its tables failed to build. With
    /// the epoch trace on, the mutation count its `OnUpdate` handlers moved.
    pub(super) fn update_walk(&mut self, elapsed: f32, sweep: Option<&event::UpdateSweep>) -> u64 {
        let lua = &self.lua;
        let mut in_handlers = 0u64;
        super::anim::run::begin_pass(lua);
        let tracing = super::epoch_trace::enabled();
        let mut list = gather(lua, None);
        let mut epoch = walk_epoch(lua);
        let mut i = 0;
        while i < list.len() {
            let level = (list[i].key.0, list[i].key.1);
            while i < list.len() && (list[i].key.0, list[i].key.1) == level {
                let entry = list[i];
                i += 1;
                // Hidden, or relinked since it was gathered: skipped, or waiting for its new spot.
                let current = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data")
                    .arena
                    .glance(entry.h)
                    .filter(|g| g.visible)
                    .map(|g| g.walk_key());
                if current != Some(entry.key) {
                    continue;
                }
                in_handlers += self.update_slot(entry, elapsed, sweep, tracing);
            }
            let now = walk_epoch(lua);
            if now != epoch {
                list = gather(lua, Some(level));
                i = 0;
                epoch = now;
            }
        }
        in_handlers
    }

    /// One frame's layer update: `OnUpdate` (`0x490770`), its animations, then the kind override's
    /// own work after the base call. With the epoch trace on, what its `OnUpdate` moved.
    fn update_slot(
        &self,
        entry: Entry,
        elapsed: f32,
        sweep: Option<&event::UpdateSweep>,
        tracing: bool,
    ) -> u64 {
        let lua = &self.lua;
        let roster = walk_epoch(lua).1;
        let mut in_handler = 0;
        if let (Some(id), Some(sweep)) = (entry.on_update, sweep) {
            let before = tracing.then(|| self.visual_epoch());
            if let Err(e) = sweep.fire(lua, id, elapsed) {
                self.push_error(e);
            }
            if let Some(before) = before {
                in_handler = self.visual_epoch().wrapping_sub(before);
                super::epoch_trace::note_handler(id, in_handler);
            }
        }
        // A group the handler started animates in this slot too.
        if entry.animated || walk_epoch(lua).1 != roster {
            super::anim::run::update_frame(lua, entry.h, elapsed);
        }
        let kind = {
            let m = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            match m.arena.glance(entry.h) {
                Some(g) => g.kind,
                None => return in_handler,
            }
        };
        let before = tracing.then(|| self.visual_epoch());
        match kind {
            // The box's flush follows its `OnUpdate` (`0x967670`), so the events an edit raised
            // fire in its place in the walk.
            FrameKind::EditBox => editbox::update(lua, entry.h, elapsed),
            FrameKind::Cooldown => super::cooldown_widget::update(lua, entry.h),
            FrameKind::GameTooltip => super::tooltip::update(lua, entry.h),
            FrameKind::MessageFrame | FrameKind::ScrollingMessageFrame => {
                self.update_message_lines(entry.h, kind, elapsed)
            }
            k if k.is_model() => self.advance_model_clock(entry.h, elapsed),
            _ => {}
        }
        if let Some(before) = before {
            super::epoch_trace::note_leg("kind update", self.visual_epoch().wrapping_sub(before));
        }
        in_handler
    }

    /// The message kinds' line fades: MessageFrame's (`0x968a60`), which also keeps only the lines
    /// that fit its height, read from the resolved rect, and ScrollingMessageFrame's (`0x969e60`).
    /// A step that moved no line's alpha and freed no line changed nothing drawn; one that did is a
    /// paint write of this frame, whose lines the extract re-reads alone (a freed line changes the
    /// frame's entry count, which sends the extract back to its full walk).
    fn update_message_lines(&self, h: FrameHandle, kind: FrameKind, elapsed: f32) {
        let mut model = self.model_mut();
        let quiet = model.epoch();
        let rows = if kind == FrameKind::MessageFrame {
            Self::message_viewport_rows(&model, h)
        } else {
            0
        };
        let faded = match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            Some(KindState::Message(mf)) => mf.tick(elapsed) | mf.trim_to_viewport(rows),
            Some(KindState::ScrollingMessage(smf)) => smf.tick(elapsed),
            _ => false,
        };
        model.set_epoch(quiet);
        if faded {
            model.paint_mut(&[crate::order::ZTarget::Frame(h)]);
        }
    }

    /// A model pane's scene clock, advanced by `trunc(elapsed · 1000)` ms in its slot (`0x960280`);
    /// a hidden pane's stands still. The clock is no extract input: the count stands.
    fn advance_model_clock(&self, h: FrameHandle, elapsed: f32) {
        let dt_ms = (elapsed * 1000.0).trunc().max(0.0) as u64;
        let mut model = self.model_mut();
        let quiet = model.epoch();
        if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.clock_ms += dt_ms;
        }
        model.set_epoch(quiet);
    }
}
