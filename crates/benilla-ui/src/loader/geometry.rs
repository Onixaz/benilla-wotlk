use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};

use super::{abs_dim, children_named, Loader};

impl Loader<'_> {
    /// A frame element's LoadXML attributes and simple children, and a model pane's own.
    pub(super) fn apply_attrs(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if el.attr_bool("hidden") {
            self.call(wrapper, "Hide", (), dbg);
        }
        // An unknown `frameStrata=` is logged and skipped, not raised as the Lua binding would:
        // `SetFrameStrata` runs only on a hit, so the frame keeps the strata it had.
        if let Some(strata) = el.attr("frameStrata") {
            if crate::script::object::strata_from_str(strata).is_some() {
                self.call(wrapper, "SetFrameStrata", strata.to_string(), dbg);
            } else {
                self.report
                    .warnings
                    .push(format!("{dbg}: Unknown frame strata: {strata}"));
            }
        }
        if let Some(level) = el.attr("frameLevel") {
            if let Ok(n) = level.parse::<i64>() {
                self.call(wrapper, "SetFrameLevel", n, dbg);
            }
        }
        if let Some(alpha) = el.attr("alpha") {
            if let Ok(a) = alpha.parse::<f32>() {
                self.call(wrapper, "SetAlpha", a, dbg);
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
        // of 0 or less is refused, never clamped. A plain frame's `scale=` is ignored; whether
        // reads one is untraced.
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
                self.call(wrapper, "SetID", n, dbg);
            }
        }
        // `clampedToScreen`: the layout's screen clamp, geometry flag bit 4.
        if el.attr_bool("clampedToScreen") {
            self.call(wrapper, "SetClampedToScreen", true, dbg);
        }
        if el.attr_bool("enableMouse") {
            self.call(wrapper, "EnableMouse", true, dbg);
        }
        // `<HitRectInsets>`: the mouse rect, inset from the frame's; a side it omits is 0.
        if let Some(ins) = children_named(el, "HitRectInsets").next() {
            let src = children_named(ins, "AbsInset").next().unwrap_or(ins);
            let side = |k: &str| {
                src.attr(k)
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            self.call(
                wrapper,
                "SetHitRectInsets",
                (side("left"), side("right"), side("top"), side("bottom")),
                dbg,
            );
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
        // `enableKeyboard` enables both key kinds.
        if el.attr_bool("enableKeyboard") {
            self.call(wrapper, "EnableKeyboard", true, dbg);
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
        for size in children_named(el, "Size") {
            let (x, y) = abs_dim(size);
            if let Some(w) = x {
                self.call(wrapper, "SetWidth", w, dbg);
            }
            if let Some(h) = y {
                self.call(wrapper, "SetHeight", h, dbg);
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
            self.call(wrapper, "SetAllPoints", (), dbg);
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
                // The `<Offset>` child, else the anchor's own `x`/`y` (`CLayoutFrame::LoadXML`,
                // `0x48b2d0`).
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
