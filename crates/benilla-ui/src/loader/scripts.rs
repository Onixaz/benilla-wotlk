use mlua::{Function, ObjectLike, Table};

use crate::framexml::Element;

use super::{children_named, Loader};

impl Loader<'_> {
    /// `<Scripts>` (`0x769ef0`): `SetScript` for each handler. Returns the `OnLoad`, which the
    /// caller fires bottom-up.
    pub(super) fn apply_scripts(
        &mut self,
        el: &Element,
        wrapper: &Table,
        dbg: &str,
    ) -> Option<Function> {
        let mut onload = None;
        // A handler's chunk name is `"<GetName()>:<Handler>"` off the wrapper, `<unnamed>` for a
        // nameless frame (`0x7025fd`-`0x70263c`), never the loader's `dbg`.
        for scripts in children_named(el, "Scripts") {
            let owner = wrapper
                .call_method::<Option<String>>("GetName", ())
                .ok()
                .flatten()
                .unwrap_or_else(|| "<unnamed>".to_string());
            for handler in &scripts.children {
                let name = handler.tag.clone();
                // An empty body clears the handler to nil, but a whitespace one is an empty
                // function: `SetScript` (`0x7025c0`) tests only NULL and the first byte
                // (`0x7025f8`), and nothing trims the body (`0x6f29d0`). The stock blanks
                // (`BuffFrame.xml:101`) are whitespace, so their `GetScript` answers a function.
                let cleared = handler.body.is_empty();
                let func = if let Some(global) = self.function_attribute(handler) {
                    self.named_handler(&global, &name, &owner)
                } else if cleared {
                    None
                } else {
                    match self.compile_handler(handler, &name, &owner, dbg) {
                        Some(f) => Some(f),
                        None => continue,
                    }
                };
                if let Err(e) = wrapper.call_method::<()>("SetScript", (name.clone(), func.clone()))
                {
                    self.warn_once(
                        &format!("script:{name}"),
                        format!("{dbg}: SetScript(\"{name}\") unsupported in v1: {e}"),
                    );
                    continue;
                }
                // After each `SetScript` the XML walker enables the handler's input kind
                // (`0x769ef0` → `0x76af00`): these five arm the mouse (`OnDragStop` and
                // `OnReceiveDrag` do not), `OnMouseWheel` the wheel (kind 3). The Lua `SetScript`
                // never does (`0x7748d0`).
                const MOUSE_KIND: [&str; 5] = [
                    "OnEnter",
                    "OnLeave",
                    "OnMouseDown",
                    "OnMouseUp",
                    "OnDragStart",
                ];
                if MOUSE_KIND.iter().any(|k| name.eq_ignore_ascii_case(k)) {
                    self.call(wrapper, "EnableMouse", true, dbg);
                }
                if name.eq_ignore_ascii_case("OnMouseWheel") {
                    self.call(wrapper, "EnableMouseWheel", true, dbg);
                }
                // The key handlers, the same rule (`OnChar` kind 0, `OnKeyDown`/`OnKeyUp` kind 1);
                // here any of them sets the one flag both key walks read.
                const KEY_KIND: [&str; 3] = ["OnChar", "OnKeyDown", "OnKeyUp"];
                if KEY_KIND.iter().any(|k| name.eq_ignore_ascii_case(k)) {
                    self.call(wrapper, "EnableKeyboard", true, dbg);
                }
                if name.eq_ignore_ascii_case("OnLoad") {
                    // A cleared `OnLoad` also unsets a template's, or the caller would fire it.
                    onload = func;
                }
            }
        }
        onload
    }

    /// The non-empty `function=` of a handler element, on a target whose loader reads it.
    fn function_attribute(&self, handler: &Element) -> Option<String> {
        handler
            .attr("function")
            .filter(|f| !f.is_empty())
            .map(str::to_string)
    }

    /// 3.3.5a's `function="Name"` (`0x490009`): `_G[Name]`, read raw (`0x84e600`) once at load,
    /// is the handler and the body is unused. A nil logs `"Frame %s: Unknown function %s in
    /// element %s"` (`0x9eb7f0`) and clears the handler. Deviation: a non-function value, which
    /// the reference keeps until a call fails, is logged the same way and cleared.
    pub(super) fn named_handler(
        &mut self,
        global: &str,
        name: &str,
        owner: &str,
    ) -> Option<Function> {
        match self.lua().globals().raw_get::<mlua::Value>(global) {
            Ok(mlua::Value::Function(f)) => Some(f),
            _ => {
                self.report.warnings.push(format!(
                    "Frame {owner}: Unknown function {global} in element {name}"
                ));
                None
            }
        }
    }

    /// Compile a handler body as the client does: formatted into `return function(<params>) %s
    /// end` ([`crate::script::event::script_parameters`]), keeping the function the chunk returns,
    /// the body on the template's first line so its line numbers hold. A `function=` replaces the
    /// body before this is reached ([`Self::named_handler`]).
    pub(super) fn compile_handler(
        &mut self,
        handler: &Element,
        name: &str,
        owner: &str,
        dbg: &str,
    ) -> Option<Function> {
        // The raw body, never trimmed: a whitespace body is a real empty chunk (`apply_scripts`).
        let body = handler.body.as_str();
        if body.is_empty() {
            return None;
        }
        let params = crate::script::event::script_parameters(name);
        let source = format!("return function({params}) {body} end");
        let compiled = self
            .lua()
            .load(source.as_str())
            .set_name(format!("{owner}:{name}"))
            .set_mode(mlua::ChunkMode::Text)
            .into_function()
            .and_then(|chunk| chunk.call::<Function>(()));
        match compiled {
            Ok(f) => Some(f),
            Err(e) => {
                self.report
                    .errors
                    .push(format!("{dbg}: compiling <{name}>: {e}"));
                None
            }
        }
    }

    /// Fire a captured `OnLoad` under the event path's convention, `this` set and restored
    /// (`0x704d50`); an error is recorded, never propagated.
    pub(super) fn fire_onload(&mut self, wrapper: &Table, func: &Function, dbg: &str) {
        if let Err(e) = self.invoke_handler(wrapper, func) {
            self.report.errors.push(format!("{dbg}: OnLoad: {e}"));
        }
    }

    /// `PostLoadXML` (`0x490410`) runs `OnShow` after `OnLoad` for a frame visible at the end of
    /// its load, whose shows during the load were silent (`0x4902d0`).
    pub(super) fn fire_onshow_after_load(&mut self, h: crate::widget::FrameHandle, dbg: &str) {
        let id = {
            let mut model = self.model();
            if !model.arena.is_visible(h) {
                return;
            }
            model.frame_id(h)
        };
        if let Err(e) =
            crate::script::event::fire_widget_handler(self.lua(), id, "OnShow", Vec::new())
        {
            self.report.errors.push(format!("{dbg}: OnShow: {e}"));
        }
    }

    /// Call a frame-wrapper method, recording (not propagating) any error.
    pub(super) fn call(
        &mut self,
        wrapper: &Table,
        method: &str,
        args: impl mlua::IntoLuaMulti,
        dbg: &str,
    ) {
        if let Err(e) = wrapper.call_method::<()>(method, args) {
            self.report.errors.push(format!("{dbg}: {method}: {e}"));
        }
    }

    /// Call a region-wrapper method, recording (not propagating) any error.
    pub(super) fn call_region(
        &mut self,
        region: &Table,
        method: &str,
        args: impl mlua::IntoLuaMulti,
        dbg: &str,
    ) {
        if let Err(e) = region.call_method::<()>(method, args) {
            self.report
                .errors
                .push(format!("{dbg}: region {method}: {e}"));
        }
    }
}
