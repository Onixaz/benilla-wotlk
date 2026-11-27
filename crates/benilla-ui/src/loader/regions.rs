use mlua::Table;

use crate::framexml::{self, Element};

use super::{abs_dim, abs_value, children_named, color_of, tex_coords_of, Loader};

impl Loader<'_> {
    /// One `<Layers>`: each `<Texture>` and `<FontString>`, created on its layer.
    pub(super) fn apply_layers(
        &mut self,
        layers: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        let owner = match crate::script::object::frame_handle_of(self.lua(), wrapper) {
            Ok(h) => h,
            Err(e) => {
                self.report_error(format!("{dbg}: apply_layers: {e}"), &e);
                return;
            }
        };
        for layer in children_named(layers, "Layer") {
            let level = layer.attr("level").unwrap_or("ARTWORK");
            for region in &layer.children {
                // `inherits=` may name a region template, in the frames' one registry; a font
                // object's name passes on to `apply_fontstring_font`.
                let region = &self.expand_region(region);
                let is_texture = region.tag.eq_ignore_ascii_case("Texture");
                let is_fontstring = region.tag.eq_ignore_ascii_case("FontString");
                if !is_texture && !is_fontstring {
                    self.report.warnings.push(format!(
                        "{dbg}: <Layer> child <{}> is not a Texture/FontString; skipped",
                        region.tag
                    ));
                    continue;
                }
                let rname: Option<String> = region
                    .name()
                    .map(|raw| framexml::resolve_name(raw, parent_name));
                let kind = if is_texture {
                    crate::widget::RegionKind::Texture
                } else {
                    crate::widget::RegionKind::FontString
                };
                let (region_wrapper, rh) =
                    match crate::script::object::events_regions::create_region_with_handle(
                        self.lua(),
                        owner,
                        kind,
                        rname.clone(),
                        Some(level),
                    ) {
                        Ok(pair) => pair,
                        Err(e) => {
                            let method = if is_texture {
                                "CreateTexture"
                            } else {
                                "CreateFontString"
                            };
                            self.report_error(format!("{dbg}: {method}: {e}"), &e);
                            continue;
                        }
                    };
                self.bind_parent_key(region, wrapper, &region_wrapper);
                let anim_base = rname.as_deref().unwrap_or(parent_name);
                self.load_region_with_handle(
                    region,
                    &region_wrapper,
                    rh,
                    anim_base,
                    parent_name,
                    dbg,
                );
            }
        }
    }

    /// A region's own XML load past its creation: its animations, `hidden` and `alpha`, layout,
    /// font and visual, then the implicit creation anchor, which leaves an anchored region alone.
    pub(super) fn load_region(
        &mut self,
        region: &Element,
        region_wrapper: &Table,
        anim_base: &str,
        parent_name: &str,
        dbg: &str,
    ) {
        let rh = match crate::script::region::region_handle_of(self.lua, region_wrapper) {
            Ok(rh) => rh,
            Err(e) => {
                self.report_error(format!("{dbg}: region handle: {e}"), &e);
                return;
            }
        };
        self.load_region_with_handle(region, region_wrapper, rh, anim_base, parent_name, dbg);
    }

    fn load_region_with_handle(
        &mut self,
        region: &Element,
        region_wrapper: &Table,
        rh: crate::widget::RegionHandle,
        anim_base: &str,
        parent_name: &str,
        dbg: &str,
    ) {
        let is_texture = region.tag.eq_ignore_ascii_case("Texture");
        self.apply_animations(region, region_wrapper, anim_base, dbg);
        if region.attr_bool("hidden") {
            crate::script::region::set_region_hidden(self.lua, rh, true);
        }
        if let Some(a) = region
            .attr("alpha")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            crate::script::region::paint::set_region_alpha_direct(&mut self.model(), rh, a);
        }
        self.apply_region_layout_handle(region, region_wrapper, rh, parent_name, dbg);
        if !is_texture {
            // Before the visual pass, so an explicit `<Color>` beats the font object's.
            self.apply_fontstring_font_handle(region, region_wrapper, rh, dbg);
            self.apply_fontstring_flags_handle(region, rh);
        }
        self.apply_region_visual_handle(region, region_wrapper, rh, is_texture, dbg);
        crate::script::region::implicit_creation_anchor(&mut self.model(), rh);
    }

    /// A frame's direct-child `<FontString>`, its "special" string (a message frame's line font, an
    /// EditBox's text), on the OVERLAY layer.
    pub(super) fn apply_special_fontstrings(
        &mut self,
        el: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        // A `<SimpleHTML>`'s direct `<FontString>` declares its `P` font, not a region;
        // `apply_simplehtml` owns it.
        if el.tag.eq_ignore_ascii_case("SimpleHTML") {
            return;
        }
        for region in &el.children {
            if !region.tag.eq_ignore_ascii_case("FontString") {
                continue;
            }
            let region = &self.expand_region(region);
            let rname: Option<String> = region
                .name()
                .map(|raw| framexml::resolve_name(raw, parent_name));
            // An EditBox's `<FontString>` declares the text string its ctor built, so it creates
            // nothing: a second region would also shift `GetRegions`.
            let existing_text = el
                .tag
                .eq_ignore_ascii_case("EditBox")
                .then(|| crate::script::editbox_text_region_wrapper(self.lua(), wrapper))
                .flatten();
            let (region_wrapper, rh) = match existing_text {
                Some(w) => {
                    if let Some(n) = rname {
                        crate::script::region::publish_region_name(self.lua(), &n, &w);
                        if let Err(e) = self.lua().globals().set(n.clone(), w.clone()) {
                            self.report
                                .warnings
                                .push(format!("{dbg}: embedded FontString global '{n}': {e}"));
                        }
                    }
                    let Ok(rh) = crate::script::region::region_handle_of(self.lua, &w) else {
                        continue;
                    };
                    (w, rh)
                }
                None => {
                    let owner = match crate::script::object::frame_handle_of(self.lua(), wrapper) {
                        Ok(h) => h,
                        Err(e) => {
                            self.report
                                .errors
                                .push(format!("{dbg}: CreateFontString (special): {e}"));
                            continue;
                        }
                    };
                    match crate::script::object::events_regions::create_region_with_handle(
                        self.lua(),
                        owner,
                        crate::widget::RegionKind::FontString,
                        rname,
                        Some("OVERLAY"),
                    ) {
                        Ok(pair) => pair,
                        Err(e) => {
                            self.report
                                .errors
                                .push(format!("{dbg}: CreateFontString (special): {e}"));
                            continue;
                        }
                    }
                }
            };
            self.apply_region_layout_handle(region, &region_wrapper, rh, parent_name, dbg);
            self.apply_fontstring_font_handle(region, &region_wrapper, rh, dbg);
            self.apply_fontstring_flags_handle(region, rh);
            // The element is the frame's own font too (`0x497070`), whose `indented` its
            // indented-word-wrap pair answers.
            if let Some(on) = xml_flag(region, "indented") {
                self.call(wrapper, "SetIndentedWordWrap", on, dbg);
            }
            self.apply_region_visual_handle(region, &region_wrapper, rh, false, dbg);
            // The implicit creation anchor, as in `apply_layers`; an EditBox's adopt replaces it.
            crate::script::region::implicit_creation_anchor(&mut self.model(), rh);
            // It is the EditBox's text region by slot, never found by a search, which could take a
            // `<Layers>` string; the box's insets anchor it.
            if el.tag.eq_ignore_ascii_case("EditBox") {
                if let Err(e) =
                    crate::script::adopt_text_region(self.lua(), wrapper, &region_wrapper)
                {
                    self.report
                        .errors
                        .push(format!("{dbg}: adopt special FontString: {e}"));
                }
            }
        }
    }

    /// A region's geometry: `<Size>`, a FontString's justify and spacing, `setAllPoints` and `<Anchors>`, whose `$parent` is the owner's name, or a nameless owner's
    /// nearest named ancestor's.
    pub(super) fn apply_region_layout(
        &mut self,
        region: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        let rh = match crate::script::region::region_handle_of(self.lua, wrapper) {
            Ok(rh) => rh,
            Err(e) => {
                self.report_error(format!("{dbg}: region layout: {e}"), &e);
                return;
            }
        };
        self.apply_region_layout_handle(region, wrapper, rh, parent_name, dbg);
    }

    fn apply_region_layout_handle(
        &mut self,
        region: &Element,
        wrapper: &Table,
        rh: crate::widget::RegionHandle,
        parent_name: &str,
        dbg: &str,
    ) {
        {
            let mut model = self.model();
            // Every `<Size>` in document order, as in `apply_size`.
            for size in children_named(region, "Size") {
                let (x, y) = abs_dim(size);
                if let Some(w) = x {
                    crate::script::region::layout::set_region_width_direct(&mut model, rh, w);
                }
                if let Some(h) = y {
                    crate::script::region::layout::set_region_height_direct(&mut model, rh, h);
                }
            }
            if let Some(j) = region.attr("justifyH") {
                if let Err(e) = crate::script::region::text::set_justify_h_direct(&mut model, rh, j)
                {
                    drop(model);
                    self.report_error(format!("{dbg}: region SetJustifyH: {e}"), &e);
                    model = self.model();
                }
            }
            if let Some(j) = region.attr("justifyV") {
                if let Err(e) = crate::script::region::text::set_justify_v_direct(&mut model, rh, j)
                {
                    drop(model);
                    self.report_error(format!("{dbg}: region SetJustifyV: {e}"), &e);
                    model = self.model();
                }
            }
            if let Some(sp) = region
                .attr("spacing")
                .and_then(|v| v.trim().parse::<f32>().ok())
            {
                crate::script::region::text::set_spacing_direct(&mut model, rh, sp);
            }
            if region.attr_bool("setAllPoints") {
                crate::script::region::layout::set_region_all_points_to_owner(&mut model, rh);
            }
        }
        for anchors in children_named(region, "Anchors") {
            for anchor in children_named(anchors, "Anchor") {
                let Some(point) = anchor.attr("point") else {
                    self.report
                        .warnings
                        .push(format!("{dbg}: region <Anchor> without a point; skipped"));
                    continue;
                };
                let rel_point = anchor.attr("relativePoint").unwrap_or(point).to_string();
                let rel_to: Option<String> = anchor
                    .attr("relativeTo")
                    .map(|r| framexml::resolve_name(r, parent_name));
                // The `<Offset>` child, else the anchor's own `x`/`y` (the layout frame's XML load,
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
                    region: true,
                    args,
                    dbg: dbg.to_string(),
                };
                // Deferred while the enclosing subtree has not built its target yet.
                self.apply_anchor(d, true);
            }
        }
    }

    /// The wrap of a texture slot the XML builds fresh (`0x815f40`), a button state, slider thumb
    /// or colour-select texture: its own wrap stands whenever its file loaded.
    pub(super) fn apply_slot_texture_wrap(&mut self, el: &Element, region: &Table, dbg: &str) {
        self.apply_texture_non_blocking(el, region, dbg);
        let Some(file) = el.attr("file") else {
            return;
        };
        if crate::script::region::texture_path_of(self.lua, region).as_deref() != Some(file) {
            return;
        }
        let wrap = xml_texture_wrap(el);
        if let Err(e) = crate::script::region::set_texture_wrap(self.lua, region, wrap) {
            self.report
                .errors
                .push(format!("{dbg}: region texture wrap: {e}"));
        }
    }

    fn apply_fontstring_flags_handle(&mut self, el: &Element, rh: crate::widget::RegionHandle) {
        let non_space = xml_flag(el, "nonspacewrap");
        let word = xml_flag(el, "wordwrap");
        let indented = xml_flag(el, "indented");
        if non_space.is_none() && word.is_none() && indented.is_none() {
            return;
        }
        let mut model = self.model();
        if let Some(on) = non_space {
            crate::script::region::text::set_non_space_wrap_direct(&mut model, rh, on);
        }
        if let Some(on) = word {
            crate::script::region::text::set_word_wrap_direct(&mut model, rh, on);
        }
        if let Some(on) = indented {
            crate::script::region::text::set_indented_word_wrap_direct(&mut model, rh, on);
        }
    }

    /// A texture element's `nonBlocking` (`0x485f40`), present and non-empty, through its setter.
    pub(super) fn apply_texture_non_blocking(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if let Some(on) = xml_flag(el, "nonBlocking") {
            match crate::script::region::region_handle_of(self.lua, wrapper) {
                Ok(rh) => crate::script::region::paint::set_texture_non_blocking_direct(
                    &mut self.model(),
                    rh,
                    on,
                ),
                Err(e) => self.report_error(format!("{dbg}: region SetNonBlocking: {e}"), &e),
            }
        }
    }

    fn apply_region_visual_handle(
        &mut self,
        region: &Element,
        region_wrapper: &Table,
        rh: crate::widget::RegionHandle,
        is_texture: bool,
        dbg: &str,
    ) {
        let color = children_named(region, "Color").next().map(color_of);
        if is_texture {
            if let Some(on) = xml_flag(region, "nonBlocking") {
                crate::script::region::paint::set_texture_non_blocking_direct(
                    &mut self.model(),
                    rh,
                    on,
                );
            }
            // `horizTile`/`vertTile`, present and non-empty, set their tile bit to the value read
            // as a boolean (`0x485f40`), before the file, which they also wrap.
            if let Some(on) = xml_flag(region, "horizTile") {
                crate::script::region::paint::set_texture_horiz_tile_direct(
                    &mut self.model(),
                    rh,
                    on,
                );
            }
            if let Some(on) = xml_flag(region, "vertTile") {
                crate::script::region::paint::set_texture_vert_tile_direct(
                    &mut self.model(),
                    rh,
                    on,
                );
            }
            if let Some(file) = region.attr("file") {
                // A `<Color>` beside `file=` is discarded, not a tint: the reference applies it in
                // its child loop, then the file, read after, overwrites the same slot.
                let wrap = xml_texture_wrap(region);
                crate::script::region::paint::set_texture_file_direct(self.lua, rh, file, wrap);
            } else if let Some(c) = color {
                // No file: a solid fill, an 8×8 texture in the reference, so its alpha multiplies
                // with `SetVertexColor` rather than being replaced by it.
                crate::script::region::paint::set_texture_color_direct(&mut self.model(), rh, c);
            }
            // `<Gradient>` writes the vertex colours (`+0xb8`), which `file=` leaves alone, so
            // unlike `<Color>` it tints the art. MinColor is the first stop and MaxColor the
            // second; the reference's stop-to-vertex order is untraced.
            if let Some(g) = children_named(region, "Gradient").next() {
                let stop = |tag: &str| children_named(g, tag).next().map(color_of);
                // A gradient missing a stop is skipped: an absent stop is not a black one.
                if let (Some(min), Some(max)) = (stop("MinColor"), stop("MaxColor")) {
                    let vertical = g
                        .attr("orientation")
                        .is_some_and(|o| o.eq_ignore_ascii_case("VERTICAL"));
                    crate::script::region::paint::set_region_gradient_direct(
                        &mut self.model(),
                        rh,
                        vertical,
                        min,
                        max,
                    );
                }
            }
            if let Some(tc) = tex_coords_of(region) {
                crate::script::region::paint::set_tex_coords_rect_direct(
                    &mut self.model(),
                    rh,
                    [tc.0, tc.1, tc.2, tc.3],
                );
            }
            if let Some(mode) = region.attr("alphaMode") {
                crate::script::region::paint::set_blend_mode_direct(&mut self.model(), rh, mode);
            }
        } else {
            if let Some(text) = region.attr("text") {
                // A global-string key, not a literal.
                let text = self.resolve_text(text, dbg);
                if let Err(e) = crate::script::store_text(self.lua, region_wrapper, Some(text)) {
                    self.report
                        .errors
                        .push(format!("{dbg}: region SetText: {e}"));
                }
            }
            if let Some(c) = color {
                crate::script::region::paint::set_region_vertex_color_direct(
                    &mut self.model(),
                    rh,
                    c[0],
                    c[1],
                    c[2],
                    c[3],
                );
            }
        }
    }

    /// The font object a FontString's `inherits=` names, directly or through virtual FontString
    /// templates; `None` when the chain ends without one. Bounded, as a registry can hold a cycle.
    pub(super) fn font_object_through_templates(&self, inherits: &str) -> Option<String> {
        const MAX_HOPS: usize = 8;
        let model = self.model();
        let fonts = model.framexml_fonts.borrow();
        let templates = model.framexml_templates.borrow();
        for entry in crate::framexml::inherit_names(inherits) {
            let mut name = entry.to_string();
            for _ in 0..MAX_HOPS {
                if fonts.contains_key(&name) || fonts.keys().any(|k| k.eq_ignore_ascii_case(&name))
                {
                    return Some(name);
                }
                let Some(next) = templates
                    .get(&name)
                    .and_then(|t| t.attr("inherits"))
                    .map(str::to_string)
                else {
                    break;
                };
                name = next;
            }
        }
        None
    }

    /// Whether `inherits=` names a registered element template, each name matched
    /// case-insensitively, as `framexml::expand` looks it up.
    pub(super) fn names_template(&self, inherits: &str) -> bool {
        let model = self.model();
        let templates = model.framexml_templates.borrow();
        crate::framexml::inherit_names(inherits)
            .into_iter()
            .any(|name| {
                templates.contains_key(name)
                    || templates.keys().any(|k| k.eq_ignore_ascii_case(name))
            })
    }

    /// Whether `name` is a registered font object: the reference's flat, case-insensitive lookup,
    /// with no template walk.
    pub(super) fn is_font_object(&self, name: &str) -> bool {
        let model = self.model();
        let fonts = model.framexml_fonts.borrow();
        fonts.contains_key(name) || fonts.keys().any(|k| k.eq_ignore_ascii_case(name))
    }

    /// A `<FontString>`'s font: `inherits=` through [`Self::font_object_through_templates`], then
    /// `font=`, a font object's name first and a file only on a miss.
    pub(super) fn apply_fontstring_font(&mut self, region: &Element, wrapper: &Table, dbg: &str) {
        let rh = match crate::script::region::region_handle_of(self.lua, wrapper) {
            Ok(rh) => rh,
            Err(e) => {
                self.report_error(format!("{dbg}: region font: {e}"), &e);
                return;
            }
        };
        self.apply_fontstring_font_handle(region, wrapper, rh, dbg);
    }

    fn apply_fontstring_font_handle(
        &mut self,
        region: &Element,
        wrapper: &Table,
        rh: crate::widget::RegionHandle,
        dbg: &str,
    ) {
        if let Some(name) = region.attr("inherits") {
            // The expanded element keeps the instance's `inherits=`, a template's name, so the
            // chain is walked to its font object. 12340 (`0x4873e0`) loads a name that is no font
            // object as the template's XML, so a template chain with no font object is already
            // in the expansion and its `font=` lands below. Any other name passes through
            // unchanged: `SetFontObject("")` would clear the font object where a bad name warns.
            let resolved = match self.font_object_through_templates(name) {
                Some(font) => Some(font),
                None if self.names_template(name) => None,
                None => Some(name.to_string()),
            };
            if let Some(font) = resolved {
                let res = crate::script::font_block::set_font_object_name_direct(
                    &mut self.model(),
                    rh,
                    &font,
                );
                if let Err(e) = res {
                    self.warn_once(
                        &format!("fontobj:{name}"),
                        format!("{dbg}: <FontString inherits=\"{name}\">: {e}"),
                    );
                }
            }
        }
        // `font=` gates `<FontHeight>` and `outline=`: a font object's name links it and skips
        // them, any other value is a file they qualify, and with no `font=` they are never read, so
        // `ZoneText.xml`'s `AutoFollowStatusText` draws at GameFontNormal's 12, not its own 20.
        match region.attr("font") {
            Some(name) if self.is_font_object(name) => {
                let res = crate::script::font_block::set_font_object_name_direct(
                    &mut self.model(),
                    rh,
                    name,
                );
                if let Err(e) = res {
                    self.warn_once(
                        &format!("fontobj:{name}"),
                        format!("{dbg}: <FontString font=\"{name}\">: {e}"),
                    );
                }
            }
            Some(path) => {
                // Applied directly, not through the Lua `SetFont`, which requires a height where
                // XML may omit it.
                let height = children_named(region, "FontHeight")
                    .next()
                    .and_then(abs_value);
                let outline = region.attr("outline").map(str::to_string);
                if let Err(e) = crate::script::apply_font_parts(
                    self.lua,
                    wrapper,
                    Some(path.to_string()),
                    height,
                    outline,
                ) {
                    self.report
                        .errors
                        .push(format!("{dbg}: region font attrs: {e}"));
                }
            }
            None => {}
        }
    }
}

/// A boolean attribute as the XML load reads one: present and non-empty, through the loader's
/// string-to-boolean with a false default; `None` otherwise.
pub(super) fn xml_flag(el: &Element, attr: &str) -> Option<bool> {
    el.attr(attr)
        .filter(|v| !v.is_empty())
        .map(|v| crate::script::binding_abi::str_bool_or_default(v.as_bytes(), false))
}

/// A `<Texture>`'s `(u, v)` wrap, as the XML load (`0x485f40`) builds it for the file's create:
/// `horizTile` or `vertTile` present and non-empty sets its axis whatever its value, and a
/// `<TexCoords>` edge outside `[0, 1]` sets the axis it lies on.
///
/// Deviation: only the `left`/`right`/`top`/`bottom` form is read, all four present, as
/// [`tex_coords_of`] parses it; the reference also reads a `<Rect>` child and defaults a
/// missing edge.
pub(super) fn xml_texture_wrap(region: &Element) -> (bool, bool) {
    let tiled = |attr: &str| region.attr(attr).is_some_and(|v| !v.is_empty());
    let out = |a: f32, b: f32| !(0.0..=1.0).contains(&a) || !(0.0..=1.0).contains(&b);
    let (u, v) =
        tex_coords_of(region).map_or((false, false), |(l, r, t, b)| (out(l, r), out(t, b)));
    (tiled("horizTile") || u, tiled("vertTile") || v)
}
