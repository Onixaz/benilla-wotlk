//! [`WidgetArena`]'s mutators: propagation down the subtree, reparenting and the frame flags.

use crate::order::Strata;

use super::{FrameHandle, RegionHandle, WidgetArena, SCALE_EPS};

impl WidgetArena {
    // ── Visibility ───────────────────────────────────────────────────────────────────────────────

    /// Set `shown` and cascade effective visibility through the subtree (`Show` `0x48f660` → the
    /// per-frame show step `0x4918f0`, `Hide` `0x48f620` → the hide step `0x491830`), returning the
    /// frames that changed in the order the reference notifies them: each frame after its
    /// descendants. For a caller with no handlers to run in between; `script::event::set_shown`
    /// runs the same cascade live, firing each frame's `OnShow`/`OnHide` at its own step.
    pub fn set_shown(&mut self, h: FrameHandle, shown: bool) -> Vec<FrameHandle> {
        let mut changed = Vec::new();
        match self.frame_mut(h) {
            Some(f) if f.shown != shown => f.shown = shown,
            _ => return changed,
        }
        self.cascade_visible(h, shown, &mut changed);
        changed
    }

    /// The live gate and own step of the per-frame show (`0x4918f0`): a frame that is shown, under
    /// a visible parent or none, and not yet visible becomes visible and links at its bucket's
    /// tail; false, with nothing changed, otherwise. Its children are the caller's to step next.
    pub fn show_this(&mut self, h: FrameHandle) -> bool {
        let gate = self
            .frame(h)
            .is_some_and(|f| f.shown && !f.effective_visible);
        if !gate || !self.parent_visible(h) {
            return false;
        }
        self.set_effective_visible(h, true);
        self.resequence_to_tail(h);
        true
    }

    /// The own step of the per-frame hide (`0x491830`): a visible frame stops being visible; false,
    /// with nothing changed, for a frame already hidden.
    pub fn hide_this(&mut self, h: FrameHandle) -> bool {
        if !self.frame(h).is_some_and(|f| f.effective_visible) {
            return false;
        }
        self.set_effective_visible(h, false);
        true
    }

    fn set_effective_visible(&mut self, h: FrameHandle, visible: bool) {
        self.frame_mut(h).unwrap().effective_visible = visible;
        self.sync_glance(h);
        self.note_visibility_change();
        if !visible {
            // Unlinked from its level list; a show's link counts in `resequence_to_tail`.
            self.link_epoch = self.link_epoch.wrapping_add(1);
        }
    }

    /// [`Self::show_this`] or [`Self::hide_this`] down the subtree, pruned where a step changes
    /// nothing, each changed frame pushed after its descendants.
    fn cascade_visible(&mut self, h: FrameHandle, show: bool, changed: &mut Vec<FrameHandle>) {
        let stepped = if show {
            self.show_this(h)
        } else {
            self.hide_this(h)
        };
        if !stepped {
            return;
        }
        let children = self
            .frame(h)
            .map(|f| f.children.clone())
            .unwrap_or_default();
        for c in children {
            self.cascade_visible(c, show, changed);
        }
        changed.push(h);
    }

    // ── Strata ───────────────────────────────────────────────────────────────────────────────────

    /// Force `h` and its whole subtree to `strata`, moving each visible frame to the tail of its
    /// new bucket.
    pub fn set_frame_strata(&mut self, h: FrameHandle, strata: Strata) {
        let (visible, children) = match self.frame_mut(h) {
            Some(f) => {
                if f.strata == strata {
                    return;
                }
                f.strata = strata;
                (f.effective_visible, f.children.clone())
            }
            None => return,
        };
        self.sync_glance(h);
        if visible {
            self.resequence_to_tail(h);
        }
        for c in children {
            self.set_frame_strata(c, strata);
        }
    }

    // ── Level ────────────────────────────────────────────────────────────────────────────────────

    /// Set `h`'s level (the frame-level set, `0x4910a0`): the step up is capped at `0x80`
    /// (`0x4910d9`), so the level moves by `min(new - old, 128)`; with `propagate`, same-strata
    /// children move by the same step. A same-value call returns before relinking.
    pub fn set_frame_level(&mut self, h: FrameHandle, level: u16, propagate: bool) {
        let (delta, strata, children) = match self.frame_mut(h) {
            Some(f) => {
                if f.level == level {
                    return;
                }
                let delta = (i32::from(level) - i32::from(f.level)).min(0x80);
                f.level = (i32::from(f.level) + delta).clamp(0, i32::from(u16::MAX)) as u16;
                (delta, f.strata, f.children.clone())
            }
            None => return,
        };
        self.sync_glance(h);
        // Only a visible frame relinks; a hidden one is in no bucket.
        if self.frame(h).is_some_and(|f| f.effective_visible) {
            self.resequence_to_tail(h);
        }
        if !propagate {
            return;
        }
        for c in children {
            let child_new = match self.frame(c) {
                Some(cf) if cf.strata == strata => {
                    (i32::from(cf.level) + delta).clamp(0, i32::from(u16::MAX)) as u16
                }
                _ => continue, // cross-strata or stale: untouched
            };
            self.set_frame_level(c, child_new, true);
        }
    }

    /// The strata compression (`0x495060`), run by the raise before it lifts the frame to the
    /// returned bound: below the stratum's bound, a level is in use when any frame of the stratum
    /// sits there, hidden ones included. At each used level past a run of empty ones, every frame
    /// from there up to the bound drops by the run's length through the non-propagating level
    /// setter, which relinks a visible one, a level at a time in creation order. The scan resumes
    /// at the used level, so a second run inside the dropped span stays: `{0, 4, 6}` under a bound
    /// of 7 becomes `{0, 1, 3}` under 4. A frame at or past the bound keeps its level.
    pub fn compress_strata(&mut self, strata: Strata) -> u16 {
        let mut top = self.top_level(strata);
        let mut members: Vec<(u16, u32, FrameHandle)> = self
            .iter_frames()
            .filter(|(_, f)| f.strata == strata && f.level < top)
            .map(|(h, f)| (f.level, f.created_seq, h))
            .collect();
        members.sort_unstable_by_key(|&(level, created, _)| (level, created));
        let mut at_level: Vec<Vec<FrameHandle>> = vec![Vec::new(); usize::from(top)];
        for (level, _, h) in members {
            at_level[usize::from(level)].push(h);
        }
        let mut empty: Option<u16> = None;
        let mut level = 0u16;
        while level < top {
            if at_level[usize::from(level)].is_empty() {
                empty.get_or_insert(level);
                level += 1;
                continue;
            }
            let Some(first_empty) = empty.take() else {
                level += 1;
                continue;
            };
            let gap = level - first_empty;
            for from in level..top {
                for h in std::mem::take(&mut at_level[usize::from(from)]) {
                    self.set_frame_level(h, from - gap, false);
                    at_level[usize::from(from - gap)].push(h);
                }
            }
            top -= gap;
        }
        if let Some(first_empty) = empty {
            top = first_empty;
        }
        self.top_level[usize::from(strata.index())] = top;
        top
    }

    // ── Toplevel ─────────────────────────────────────────────────────────────────────────────────

    /// `SetToplevel` via the bit-setter: a flag write that raises nothing.
    pub fn set_toplevel(&mut self, h: FrameHandle, toplevel: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.toplevel = toplevel;
        }
    }

    pub fn is_toplevel(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.toplevel)
    }

    // ── Scale ────────────────────────────────────────────────────────────────────────────────────

    /// Set `h`'s own scale and recompute `effective_scale = parent's * own` down the subtree,
    /// stopping at a frame whose value moves less than [`SCALE_EPS`].
    pub fn set_scale(&mut self, h: FrameHandle, scale: f32) {
        if let Some(f) = self.frame_mut(h) {
            f.scale = scale;
        } else {
            return;
        }
        let parent_scale = self.parent_effective_scale(h);
        self.propagate_scale(h, parent_scale);
    }

    fn propagate_scale(&mut self, h: FrameHandle, parent_scale: f32) {
        let (own, ignore, old_eff, children) = {
            let f = self.frame(h).expect("live node in propagation");
            (
                f.scale,
                f.ignore_parent_scale,
                f.effective_scale,
                f.children.clone(),
            )
        };
        let new_eff = if ignore { own } else { parent_scale * own };
        if (f64::from(new_eff) - f64::from(old_eff)).abs() < SCALE_EPS {
            return;
        }
        self.frame_mut(h).unwrap().effective_scale = new_eff;
        for c in children {
            self.propagate_scale(c, new_eff);
        }
    }

    // ── Alpha ────────────────────────────────────────────────────────────────────────────────────

    /// `SetAlpha`: write the raw value to `h` and every descendant frame (`+0xc8`), with no
    /// draw-time product, so a child's own later `SetAlpha` holds until an ancestor's next. Every
    /// draw target `set_alpha` on `h` reaches: `h`, its descendants, and their regions.
    pub fn subtree_targets(&self, h: FrameHandle) -> Vec<crate::order::ZTarget> {
        let mut out = Vec::new();
        let mut stack = vec![h];
        while let Some(f) = stack.pop() {
            let Some(frame) = self.frame(f) else { continue };
            out.push(crate::order::ZTarget::Frame(f));
            out.extend(
                frame
                    .regions
                    .iter()
                    .map(|&r| crate::order::ZTarget::Region(r)),
            );
            stack.extend(frame.children.iter().copied());
        }
        out
    }

    pub fn set_alpha(&mut self, h: FrameHandle, alpha: f32) {
        let Some(f) = self.frame_mut(h) else { return };
        f.alpha = alpha;
        f.effective_alpha = alpha;
        let children = self.frame(h).expect("just wrote it").children.clone();
        for c in children {
            self.set_alpha(c, alpha);
        }
    }

    // ── Reparenting ──────────────────────────────────────────────────────────────────────────────

    /// Phase 1 of `SetParent` (`0x4911b0`): the guards and the hide half. `None` when the move
    /// changes nothing ([`Self::reparent_moves`]); otherwise the frames that lost effective
    /// visibility, each after its descendants, whose `OnHide` runs under the old parent.
    ///
    /// Deviation: the frame is still in the old parent's child list during `OnHide`, unlike the
    /// reference, because a handler that itself reparents must not find the lists half-spliced.
    pub fn reparent_begin(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Option<Vec<FrameHandle>> {
        if !self.reparent_moves(h, new_parent) {
            return None;
        }
        let mut hidden = Vec::new();
        self.cascade_visible(h, false, &mut hidden);
        Some(hidden)
    }

    /// Whether `SetParent(h, new_parent)` does anything: false for a dead frame, an unchanged
    /// parent (`0x4911b0` returns at once) or a move that would cycle (the Lua binding raises
    /// first).
    pub fn reparent_moves(&self, h: FrameHandle, new_parent: Option<FrameHandle>) -> bool {
        let Some(f) = self.frame(h) else {
            return false;
        };
        let new_parent = new_parent.filter(|&p| self.frame(p).is_some());
        if new_parent.is_some_and(|np| np == h || self.is_ancestor(h, np)) {
            return false;
        }
        f.parent != new_parent
    }

    /// Phase 2 of `SetParent` (`0x4911b0`): [`Self::reparent_relink`], then the show half,
    /// returning the frames that became visible, each after its descendants. The show half runs
    /// for a shown frame whose new parent is visible or absent, so a frame moved out of a hidden
    /// parent shows with its new one, as the stock quest log's detail pane relies on.
    pub fn reparent_finish(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Vec<FrameHandle> {
        let mut shown = Vec::new();
        if self.reparent_relink(h, new_parent) {
            self.cascade_visible(h, true, &mut shown);
        }
        shown
    }

    /// `SetParent`'s move (`0x4911b0`): relink, strata := the parent's (MEDIUM for none), level :=
    /// the parent's + 1 (0 for none) with its same-strata subtree shifted along (`0x4910a0` with
    /// propagate 1), scale re-inherited; alpha is untouched. False for a frame that died inside an
    /// `OnHide`, which has nothing to move.
    pub fn reparent_relink(&mut self, h: FrameHandle, new_parent: Option<FrameHandle>) -> bool {
        if self.frame(h).is_none() {
            return false;
        }
        let new_parent = new_parent.filter(|&p| self.frame(p).is_some());
        // Relink from the current parent: an `OnHide` may have reparented it, and the reference's
        // unconditional parent store lets the outer call win.
        let old_parent = self.frame(h).unwrap().parent;
        if let Some(op) = old_parent {
            if let Some(of) = self.frame_mut(op) {
                of.children.retain(|&c| c != h);
            }
        }
        self.frame_mut(h).unwrap().parent = new_parent;
        if let Some(np) = new_parent {
            self.frame_mut(np).unwrap().children.push(h);
        }

        let (pstrata, plevel) = match new_parent {
            Some(np) => {
                let pf = self.frame(np).expect("live new parent");
                (pf.strata, pf.level.saturating_add(1))
            }
            // `SetParent(nil)` resets to MEDIUM and level 0.
            None => (Strata::default(), 0),
        };
        self.set_frame_strata(h, pstrata);
        self.set_frame_level(h, plevel, true);
        let ps = self.parent_effective_scale(h);
        self.propagate_scale(h, ps);
        true
    }

    /// Both reparent phases in one call, hide changes first, for a caller with no `OnHide` to fire.
    pub fn set_parent(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Vec<FrameHandle> {
        let Some(hidden) = self.reparent_begin(h, new_parent) else {
            return Vec::new();
        };
        let shown = self.reparent_finish(h, new_parent);
        [hidden, shown].concat()
    }

    /// `Region:SetParent` for a texture or font string; `None` detaches. The client re-links in
    /// full: out of the old parent's layer and list, into the new parent's list and layer, its
    /// layer kept, so the region takes the tail `decl_seq`. Readers resolve through `owner`, so
    /// nothing propagates. A same-owner call is a no-op, as in the reference.
    pub fn set_region_owner(&mut self, rh: RegionHandle, new_owner: Option<FrameHandle>) -> bool {
        let Some(region) = self.region(rh) else {
            return false;
        };
        let (old_owner, was_detached) = (region.owner, region.detached);
        // No cycle check: the client's looks for the receiver in the new parent's frame chain, and
        // a region is never a frame.
        let new_owner = new_owner.filter(|&f| self.frame(f).is_some());
        match new_owner {
            Some(f) if f == old_owner && !was_detached => return false,
            None if was_detached => return false,
            _ => {}
        }
        let Some(new_owner) = new_owner else {
            if let Some(r) = self.region_mut(rh) {
                r.detached = true;
            }
            return true;
        };
        if let Some(of) = self.frame_mut(old_owner) {
            of.regions.retain(|&r| r != rh);
        }
        let decl_seq = {
            let f = self.frame_mut(new_owner).expect("live new owner");
            let d = f.next_decl;
            f.next_decl += 1;
            d
        };
        if let Some(r) = self.region_mut(rh) {
            r.owner = new_owner;
            r.decl_seq = decl_seq;
            r.detached = false;
        }
        self.frame_mut(new_owner)
            .expect("live new owner")
            .regions
            .push(rh);
        true
    }

    // ── Frame flags ──────────────────────────────────────────────────────────────────────────────

    /// `EnableMouse`.
    pub fn set_mouse_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_enabled = enabled;
        }
    }

    /// `EnableKeyboard`: the char and key bits the key walk filters on.
    pub fn set_keyboard_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.keyboard_enabled = enabled;
        }
    }

    /// `IsKeyboardEnabled`.
    pub fn is_keyboard_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.keyboard_enabled)
    }

    /// `EnableMouseWheel`, the wheel's own bit, separate from the mouse's.
    pub fn set_mouse_wheel_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_wheel_enabled = enabled;
        }
    }

    pub fn is_mouse_wheel_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.mouse_wheel_enabled)
    }

    pub fn is_mouse_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.mouse_enabled).unwrap_or(false)
    }

    /// `SetClampedToScreen`.
    pub fn set_clamped_to_screen(&mut self, h: FrameHandle, clamp: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.clamped_to_screen = clamp;
        }
    }

    /// `IsClampedToScreen`.
    pub fn is_clamped_to_screen(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.clamped_to_screen).unwrap_or(false)
    }

    /// `SetClampRectInsets` (`0x48eb00`), `[left, right, top, bottom]`.
    pub fn set_clamp_rect_insets(&mut self, h: FrameHandle, insets: [f32; 4]) {
        if let Some(f) = self.frame_mut(h) {
            f.clamp_rect_insets = insets;
        }
    }

    /// `GetClampRectInsets` (`0x48eb30`), `[left, right, top, bottom]`.
    pub fn clamp_rect_insets(&self, h: FrameHandle) -> [f32; 4] {
        self.frame(h)
            .map(|f| f.clamp_rect_insets)
            .unwrap_or([0.0; 4])
    }

    /// `SetHitRectInsets`, `[left, right, top, bottom]`.
    pub fn set_hit_rect_insets(&mut self, h: FrameHandle, insets: [f32; 4]) {
        if let Some(f) = self.frame_mut(h) {
            f.hit_rect_insets = insets;
        }
    }

    /// `GetHitRectInsets`, `[left, right, top, bottom]`.
    pub fn hit_rect_insets(&self, h: FrameHandle) -> [f32; 4] {
        self.frame(h).map(|f| f.hit_rect_insets).unwrap_or([0.0; 4])
    }

    // ── Small helpers ────────────────────────────────────────────────────────────────────────────

    /// Whether `maybe_ancestor` is on `node`'s parent chain (read the other way round).
    pub fn is_ancestor(&self, maybe_ancestor: FrameHandle, node: FrameHandle) -> bool {
        let mut cur = self.frame(node).and_then(|f| f.parent);
        let mut guard = 0usize;
        while let Some(c) = cur {
            if c == maybe_ancestor {
                return true;
            }
            cur = self.frame(c).and_then(|f| f.parent);
            guard += 1;
            if guard > self.frames.slots.len() {
                break; // a well-formed tree never loops
            }
        }
        false
    }

    fn parent_visible(&self, h: FrameHandle) -> bool {
        match self.frame(h).and_then(|f| f.parent) {
            Some(p) => self.frame(p).is_none_or(|pf| pf.effective_visible),
            None => true,
        }
    }

    fn parent_effective_scale(&self, h: FrameHandle) -> f32 {
        match self.frame(h).and_then(|f| f.parent) {
            Some(p) => self.frame(p).map_or(1.0, |pf| pf.effective_scale),
            None => 1.0,
        }
    }
}
