use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};

use super::{abs_dim, children_named, Loader};

impl Loader<'_> {
    /// A frame element's XML attributes and simple children, as the frame's XML load reads them,
    /// and a model pane's own.
    pub(super) fn apply_attrs(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let Ok(h) = crate::script::object::frame_handle_of(self.lua(), wrapper) else {
            return;
        };
        // SecureFrameTemplate marks its frame explicitly protected before OnLoad (12340).
        if el.attr_bool("protected") {
            self.model().protected_frames.insert(h);
        }
        if el.attr_bool("hidden") {
            match crate::script::taint::gate(self.lua(), wrapper) {
                Ok(true) => crate::script::event::set_shown(self.lua(), h, false),
                Ok(false) => {}
                Err(e) => self.report_error(format!("{dbg}: Hide: {e}"), &e),
            }
        }
        // An unknown `frameStrata=` is logged and skipped, not raised as the Lua binding would:
        // `SetFrameStrata` runs only on a hit, so the frame keeps the strata it had.
        if let Some(strata) = el.attr("frameStrata") {
            if let Some(s) = crate::script::object::strata_from_str(strata) {
                match crate::script::taint::gate(self.lua(), wrapper) {
                    Ok(true) => self.model().arena.set_frame_strata(h, s),
                    Ok(false) => {}
                    Err(e) => self.report_error(format!("{dbg}: SetFrameStrata: {e}"), &e),
                }
            } else {
                self.report
                    .warnings
                    .push(format!("{dbg}: Unknown frame strata: {strata}"));
            }
        }
        if let Some(level) = el.attr("frameLevel") {
            if let Ok(n) = level.parse::<i64>() {
                match crate::script::taint::gate(self.lua(), wrapper) {
                    Ok(true) => {
                        let lvl = n.clamp(0, i64::from(u16::MAX)) as u16;
                        self.model().arena.set_frame_level(h, lvl, true);
                    }
                    Ok(false) => {}
                    Err(e) => self.report_error(format!("{dbg}: SetFrameLevel: {e}"), &e),
                }
            }
        }
        if let Some(alpha) = el.attr("alpha") {
            if let Ok(a) = alpha.parse::<f32>() {
                let a = a.clamp(0.0, 1.0);
                let mut model = self.model();
                let targets = model.arena.subtree_targets(h);
                model.paint_mut(&targets).arena.set_alpha(h, a);
            }
        }

        // A model pane's `file=` is `SetModel`.
        let model_kind = super::model_kind_tag(&el.tag);
        if model_kind {
            if let Some(file) = el.attr("file") {
                let text = self.resolve_text(file, dbg);
                self.call(wrapper, "SetModel", text, dbg);
            }
        }
        // A model pane's `scale=` is the model's scale, the field `SetModelScale` writes; a value
        // of 0 or less is refused, never clamped. A plain frame's `scale=` is ignored; whether the
        // frame's XML load reads one is untraced.
        if let Some(scale) = el.attr("scale") {
            if model_kind {
                match scale.trim().parse::<f32>() {
                    Ok(s) if s > 0.0 => self.call(wrapper, "SetModelScale", s, dbg),
                    _ => self.warn_once(
                        &format!("model-scale:{dbg}"),
                        format!("Frame {dbg}: Invalid model scale: {scale}"),
                    ),
                }
            }
        }
        // A model pane's fog: `fogNear`/`fogFar` clamp at 0 here only, where the Lua setters store
        // raw; `<FogColor>` is `SetFogColor`, fog bit included.
        if model_kind {
            for (attr, verb) in [("fogNear", "SetFogNear"), ("fogFar", "SetFogFar")] {
                if let Some(v) = el.attr(attr).and_then(|v| v.trim().parse::<f32>().ok()) {
                    self.call(wrapper, verb, v.max(0.0), dbg);
                }
            }
            if let Some(fc) = children_named(el, "FogColor").next() {
                let ch = |k: &str, d: f32| {
                    fc.attr(k)
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(d)
                };
                self.call(
                    wrapper,
                    "SetFogColor",
                    (ch("r", 0.0), ch("g", 0.0), ch("b", 0.0), ch("a", 1.0)),
                    dbg,
                );
            }
        }
        if let Some(id) = el.attr("id") {
            if let Ok(n) = id.parse::<i64>() {
                match crate::script::taint::gate(self.lua(), wrapper) {
                    Ok(true) => {
                        if let Some(f) = self.model().arena.frame_mut(h) {
                            f.wow_id = n;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => self.report_error(format!("{dbg}: SetID: {e}"), &e),
                }
            }
        }
        // `clampedToScreen`: the layout's screen clamp, geometry flag bit 4.
        if el.attr_bool("clampedToScreen") {
            let mut model = self.model();
            let changed = !model.arena.is_clamped_to_screen(h);
            model.arena.set_clamped_to_screen(h, true);
            if changed {
                model.sync_layout_input(h);
                model.touch_layout_frame(h);
            }
        }
        if el.attr_bool("enableMouse") {
            match crate::script::taint::gate(self.lua(), wrapper) {
                Ok(true) => crate::script::event::set_mouse_enabled(self.lua(), h, true),
                Ok(false) => {}
                Err(e) => self.report_error(format!("{dbg}: EnableMouse: {e}"), &e),
            }
        }
        // `<HitRectInsets>`: the mouse rect, inset from the frame's; a side it omits is 0.
        if let Some(ins) = children_named(el, "HitRectInsets").next() {
            let src = children_named(ins, "AbsInset").next().unwrap_or(ins);
            let side = |k: &str| {
                src.attr(k)
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            match crate::script::taint::gate(self.lua(), wrapper) {
                Ok(true) => self.model().arena.set_hit_rect_insets(
                    h,
                    [side("left"), side("right"), side("top"), side("bottom")],
                ),
                Ok(false) => {}
                Err(e) => self.report_error(format!("{dbg}: SetHitRectInsets: {e}"), &e),
            }
        }
        // `<ResizeBounds>`: both pairs are always written, an absent one as 0, so a lone
        // `<minResize>` resets the max to unbounded. A `<Size>` outside them is not clamped.
        if let Some(rb) = children_named(el, "ResizeBounds").next() {
            for (tag, verb) in [("minResize", "SetMinResize"), ("maxResize", "SetMaxResize")] {
                let (x, y) = children_named(rb, tag).next().map_or((None, None), abs_dim);
                self.call(wrapper, verb, (x.unwrap_or(0.0), y.unwrap_or(0.0)), dbg);
            }
        }
        // `movable`/`resizable`: flag bits 0x100/0x200, via the setter Lua uses.
        if el.attr_bool("movable") {
            self.call(wrapper, "SetMovable", true, dbg);
        }
        if el.attr_bool("resizable") {
            self.call(wrapper, "SetResizable", true, dbg);
        }
        // `toplevel`: bit 0x1 of the same flag word.
        if el.attr_bool("toplevel") {
            self.call(wrapper, "SetToplevel", true, dbg);
        }
        // `dontSavePosition`: bit 0x80000, the flag `SetDontSavePosition` writes.
        if el.attr_bool("dontSavePosition") {
            if let Some(f) = self.model().arena.frame_mut(h) {
                f.dont_save_position = true;
            }
        }
        // `enableKeyboard` enables both key kinds.
        if el.attr_bool("enableKeyboard") {
            match crate::script::taint::gate(self.lua(), wrapper) {
                Ok(true) => self.model().arena.set_keyboard_enabled(h, true),
                Ok(false) => {}
                Err(e) => self.report_error(format!("{dbg}: EnableKeyboard: {e}"), &e),
            }
        }
    }

    /// `<TitleRegion>`: the drag handle, built through the idempotent `CreateTitleRegion` so the
    /// element and a later Lua call name one object.
    pub(super) fn apply_title_region(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let Some(tr) = children_named(el, "TitleRegion").next() else {
            return;
        };
        let region: Table = match wrapper.call_method("CreateTitleRegion", ()) {
            Ok(r) => r,
            Err(e) => {
                self.report
                    .errors
                    .push(format!("{dbg}: CreateTitleRegion: {e}"));
                return;
            }
        };
        self.apply_region_layout(tr, &region, self_name, dbg);
    }

    /// `<Size>`. Every one applies in document order, and expansion puts an instance's children
    /// after its template's, so the instance's own `<Size>` wins.
    pub(super) fn apply_size(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let mut sizes = children_named(el, "Size").peekable();
        if sizes.peek().is_none() {
            return;
        }
        let h = match crate::script::object::frame_handle_of(self.lua(), wrapper) {
            Ok(h) => h,
            Err(e) => {
                self.report_error(format!("{dbg}: SetSize: {e}"), &e);
                return;
            }
        };
        let mut model = self.model();
        for size in sizes {
            let (x, y) = abs_dim(size);
            if let Some(w) = x {
                crate::script::object::layout_methods::set_frame_width_direct(&mut model, h, w);
            }
            if let Some(ht) = y {
                crate::script::object::layout_methods::set_frame_height_direct(&mut model, h, ht);
            }
        }
    }

    /// `<Anchors>`, after the `setAllPoints` shorthand: `relativePoint` defaults to `point`, and
    /// `relativeTo` expands `$parent` to the nearest named ancestor's name, else `Top`.
    pub(super) fn apply_anchors(
        &mut self,
        el: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        if el.attr_bool("setAllPoints") {
            match crate::script::object::frame_handle_of(self.lua(), wrapper) {
                Ok(h) => {
                    let mut model = self.model();
                    let rel_id =
                        crate::script::object::layout_methods::default_parent_id(&mut model, h);
                    crate::script::object::layout_methods::set_frame_all_points_direct(
                        &mut model, h, rel_id,
                    );
                }
                Err(e) => self.report_error(format!("{dbg}: SetAllPoints: {e}"), &e),
            }
        }
        for anchors in children_named(el, "Anchors") {
            for anchor in children_named(anchors, "Anchor") {
                let Some(point) = anchor.attr("point") else {
                    self.report
                        .warnings
                        .push(format!("{dbg}: <Anchor> without a point; skipped"));
                    continue;
                };
                let rel_point = anchor.attr("relativePoint").unwrap_or(point).to_string();
                let rel_to: Option<String> = anchor
                    .attr("relativeTo")
                    .map(|r| framexml::resolve_name(r, parent_name));
                // The `<Offset>` child, else the anchor's own `x`/`y` (`0x48b2d0`).
                let (x, y) = children_named(anchor, "Offset")
                    .next()
                    .map_or_else(|| abs_dim(anchor), abs_dim);
                let args = (
                    point.to_string(),
                    rel_to,
                    rel_point,
                    x.unwrap_or(0.0),
                    y.unwrap_or(0.0),
                );
                let d = super::DeferredAnchor {
                    wrapper: wrapper.clone(),
                    region: false,
                    args,
                    dbg: dbg.to_string(),
                };
                // Deferred while the enclosing subtree has not built its target yet.
                self.apply_anchor(d, true);
            }
        }
    }
}
