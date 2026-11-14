//! Handler firing: the FrameScript calling convention for events, ticks and show/hide. The
//! reference (`0x819ea0`) sets `this`, `event` and `arg1..argN` around the call, restoring each
//! after, and calls the handler with the same values as `(self, event, ...)`. Handler errors
//! return to the caller, which records them in [`super::Model::errors`].

use mlua::{Function, Lua, Table, Value};

use super::{ScriptValue, REG_SCRIPTS};
use crate::script::object::frame_wrapper;
use crate::widget::{ButtonState, FrameHandle};

/// `UiScript::fire_event` for engine code that holds only the Lua context.
pub(super) fn fire_global(lua: &Lua, event: &str, args: &[ScriptValue]) {
    super::tick::fire_event_into(lua, event, args.to_vec());
}

/// The `RegisterAllEvents()` half of a dispatch: the all-events frames, after the event's own
/// listeners and skipping any already visited. The next frame is re-found by position each step, so
/// a handler that unregisters the walk's successor ends the dispatch there.
pub(super) fn fire_all_event_listeners(lua: &Lua, event: &str, args: &[ScriptValue]) {
    let model_mut = || crate::script::model_mut_of(lua);
    let mut at = model_mut().all_event_frames.first().copied();
    while let Some(h) = at {
        let mut model = model_mut();
        let Some(pos) = model.all_event_frames.iter().position(|&x| x == h) else {
            break;
        };
        let next = model.all_event_frames.get(pos + 1).copied();
        let already = model
            .event_to_frames
            .get(event)
            .is_some_and(|l| l.contains(&h));
        let id = model.frame_id(h);
        drop(model);
        if !already {
            if let Err(e) = fire_event_handler(lua, id, event, args) {
                crate::script::model_mut_of(lua).record_lua_error(&e);
            }
        }
        at = next;
    }
}

/// Fire a frame's `OnEvent` (both conventions) with the given args.
pub(super) fn fire_event_handler(
    lua: &Lua,
    id: u32,
    event: &str,
    args: &[ScriptValue],
) -> mlua::Result<()> {
    let extra: Vec<Value> = args
        .iter()
        .cloned()
        .map(|a| a.into_lua(lua))
        .collect::<mlua::Result<_>>()?;
    fire(lua, id, "OnEvent", Some(event), extra)
}

/// The `OnUpdate` sweep's handles, fetched once per tick rather than once per frame: a city's
/// sweep fires a hundred handlers, and each registry and key lookup is a cold miss.
pub(super) struct UpdateSweep {
    scripts: Table,
    wrappers: Table,
    fire: Function,
    on_update: mlua::String,
}

impl UpdateSweep {
    pub(super) fn new(lua: &Lua) -> mlua::Result<UpdateSweep> {
        Ok(UpdateSweep {
            scripts: lua.named_registry_value(REG_SCRIPTS)?,
            wrappers: lua.named_registry_value(super::REG_WRAPPERS)?,
            fire: super::stdlib::update_fire(lua)?,
            on_update: lua.create_string("OnUpdate")?,
        })
    }

    /// Fire frame `id`'s `OnUpdate(self, elapsed)` with `this` and `arg1` set and restored after,
    /// as [`fire`] does; a no-op when the frame has none.
    pub(super) fn fire(&self, lua: &Lua, id: u32, elapsed: f32) -> mlua::Result<()> {
        let func: Function = match self.scripts.raw_get::<Value>(id)? {
            Value::Table(per) => match per.raw_get::<Value>(&self.on_update)? {
                Value::Function(f) => f,
                _ => return Ok(()),
            },
            _ => return Ok(()),
        };
        let wrapper = match self.wrappers.raw_get::<Value>(id)? {
            Value::Table(t) => t,
            _ => frame_wrapper(lua, id)?,
        };
        let _fire = super::handler_prof::armed()
            .then(|| super::handler_prof::Fire::open(lua, id, "OnUpdate"));
        self.fire
            .call((func, wrapper, f64::from(elapsed)))
            .and_then(super::stdlib::pcall_outcome)
    }
}

/// Fire a handler that carries no `event` name (`OnEnter`, `OnClick`, `OnValueChanged`, ...), with
/// `extra` as its `arg1..argN`; a no-op when the frame has no such script.
pub(crate) fn fire_widget_handler(
    lua: &Lua,
    id: u32,
    script: &str,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    fire(lua, id, script, None, extra)
}

/// Drain [`Model::pending_size_changed`], queued by the resolve pass, and fire `OnSizeChanged` for
/// each, recording errors. It drains once: a size a handler changes fires on the next resolve, as
/// the reference's rect apply does, so a handler that grows its own frame cannot spin forever.
pub(super) fn fire_size_changes(lua: &Lua) {
    let pending = std::mem::take(&mut crate::script::model_mut_of(lua).pending_size_changed);
    for (id, w, h) in pending {
        if let Err(e) = fire(
            lua,
            id,
            "OnSizeChanged",
            None,
            vec![Value::Number(f64::from(w)), Value::Number(f64::from(h))],
        ) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
    }
}

/// `Show`/`Hide` (`0x48f660`/`0x48f620`): set the frame's own flag, then run the visibility
/// cascade live from it ([`show_this`], [`hide_this`]), whose step is a virtual call, so a kind's
/// override runs even when the flag already held.
pub(super) fn set_shown(lua: &Lua, h: FrameHandle, shown: bool) {
    {
        let mut model = crate::script::model_mut_of(lua);
        let quiet = model.epoch();
        match model.arena.frame_mut(h) {
            Some(f) if f.shown != shown => f.shown = shown,
            Some(_) => model.set_epoch(quiet),
            None => {
                model.set_epoch(quiet);
                return;
            }
        }
    }
    if shown {
        show_this(lua, h);
    } else {
        hide_this(lua, h);
    }
}

/// The frame's show (`0x4918f0`), live: the frame's own step under its gate, then each child's,
/// then the raise of a toplevel frame (`0x4951a0`) and its `OnShow`, so a frame hears of its show
/// after its whole subtree has. Each step re-reads the gate, so a frame an earlier handler hid, or
/// whose parent it hid, is not shown.
pub(super) fn show_this(lua: &Lua, h: FrameHandle) {
    if is_tooltip(lua, h) && !super::tooltip::show_this_override(lua, h) {
        return;
    }
    let children = {
        let mut model = crate::script::model_mut_of(lua);
        let quiet = model.epoch();
        if !model.arena.show_this(h) {
            model.set_epoch(quiet);
            return;
        }
        // Joining the mouse index re-runs the hover walk at the saved cursor (`0x495880`).
        model.hover_repick |= model.arena.is_mouse_enabled(h);
        model
            .arena
            .frame(h)
            .map(|f| f.children.clone())
            .unwrap_or_default()
    };
    for c in children {
        show_this(lua, c);
    }
    {
        let mut model = crate::script::model_mut_of(lua);
        super::object::toplevel::raise_on_show(&mut model, h);
    }
    notify_visibility(lua, h, true);
}

/// The frame's hide (`0x491830`), live: the frame's own step, in which the hovered frame loses the
/// hover and hears `OnLeave` (the UI root's event unregister, `0x495900`), then each child's, then
/// its `OnHide`.
pub(super) fn hide_this(lua: &Lua, h: FrameHandle) {
    hide_step(lua, h, true);
}

/// `SetParent`'s body (`0x4911b0`), live: nothing for an unchanged parent, else the hide half
/// under the old parent, the relink, and the show half under the new one.
pub(super) fn set_parent(lua: &Lua, h: FrameHandle, new_parent: Option<FrameHandle>) {
    let moves = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        model.arena.reparent_moves(h, new_parent)
    };
    if !moves {
        return;
    }
    hide_this(lua, h);
    let relinked = {
        let mut model = crate::script::model_mut_of(lua);
        let relinked = model.arena.reparent_relink(h, new_parent);
        if relinked {
            // A reparent moves the subtree's effective scale, a layout input and part of every
            // descendant FontString's measure key.
            model.touch_layout_reparent(h);
            model.touch_measure_all();
        }
        relinked
    };
    if relinked {
        show_this(lua, h);
    }
}

/// The being-scrolled set's bracket (`0x490f60`): a visible frame is hidden and shown again, so
/// its subtree hears `OnHide` and `OnShow`. The flags it writes between the two (`0x2000`,
/// `0x4000`) route the client's own draw; benilla clips a scroll child at extract and keeps none.
pub(super) fn rescroll_bracket(lua: &Lua, h: FrameHandle) {
    let visible = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.effective_visible);
    if visible {
        hide_this(lua, h);
        show_this(lua, h);
    }
}

/// `Hide` on `h` without its kind's hide override; its descendants still take theirs.
pub(super) fn hide_without_override(lua: &Lua, h: FrameHandle) {
    {
        let mut model = crate::script::model_mut_of(lua);
        let quiet = model.epoch();
        match model.arena.frame_mut(h) {
            Some(f) if f.shown => f.shown = false,
            _ => model.set_epoch(quiet),
        }
    }
    hide_step(lua, h, false);
}

fn hide_step(lua: &Lua, h: FrameHandle, kind_override: bool) {
    if kind_override && is_tooltip(lua, h) {
        super::tooltip::hide_this_override(lua, h);
    }
    let (children, left) = {
        let mut model = crate::script::model_mut_of(lua);
        let quiet = model.epoch();
        if !model.arena.hide_this(h) {
            model.set_epoch(quiet);
            return;
        }
        drop_mouse_holds(&mut model, h);
        let hovered = (model.mouseover == Some(h)).then_some(h);
        // The leave is a virtual call: a disabled Button's own skips its `OnLeave`, while the
        // hover and the drag-arm still clear.
        let notified = super::button::hover_notify_runs(&model, hovered);
        if hovered.is_some() {
            model.mouseover = None;
            if model.drag.as_ref().is_some_and(|d| d.source == h) {
                model.drag = None;
            }
            model.hover_repick = true;
        }
        let left = hovered.filter(|_| notified).map(|m| model.frame_id(m));
        let children = model
            .arena
            .frame(h)
            .map(|f| f.children.clone())
            .unwrap_or_default();
        (children, left)
    };
    if let Some(id) = left {
        if let Err(e) = fire_widget_handler(lua, id, "OnLeave", vec![Value::Boolean(false)]) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
    }
    for c in children {
        hide_this(lua, c);
    }
    notify_visibility(lua, h, false);
}

/// `EnableMouse` (`0x48f6a0`, `0x48f6f0`): a change on a visible frame joins or leaves the mouse
/// index at once. Leaving drops the capture and the press and, for the hovered frame, the hover
/// and drag arm with `OnLeave(false)`; unlike a hide, a held Button keeps its PUSHED art.
pub(super) fn set_mouse_enabled(lua: &Lua, h: FrameHandle, enable: bool) {
    let left = {
        let mut model = crate::script::model_mut_of(lua);
        if model.arena.is_mouse_enabled(h) == enable {
            return;
        }
        model.arena.set_mouse_enabled(h, enable);
        let visible = model.arena.frame(h).is_some_and(|f| f.effective_visible);
        if !visible {
            return;
        }
        if enable {
            model.hover_repick = true;
            return;
        }
        drop_mouse_holds(&mut model, h);
        if model.mouseover != Some(h) {
            return;
        }
        let notified = super::button::hover_notify_runs(&model, Some(h));
        model.mouseover = None;
        if model.drag.as_ref().is_some_and(|d| d.source == h) {
            model.drag = None;
        }
        model.hover_repick = true;
        notified.then(|| model.frame_id(h))
    };
    if let Some(id) = left {
        if let Err(e) = fire_widget_handler(lua, id, "OnLeave", vec![Value::Boolean(false)]) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
    }
}

/// A frame leaving the mouse index (`0x495900`) loses the capture and every press it held. No
/// release follows, so nothing clicks and no `OnMouseUp` fires; a slider's grab ends with them.
pub(super) fn drop_mouse_holds(model: &mut crate::script::Model, h: FrameHandle) {
    if model.mouse_capture == Some(h) {
        model.mouse_capture = None;
    }
    model.mouse_down_on.retain(|_, held| *held != h);
    if model.slider_drag.as_ref().is_some_and(|d| d.slider == h) {
        model.slider_drag = None;
    }
}

/// Whether `h` is a live GameTooltip, whose show and hide overrides run before the base.
fn is_tooltip(lua: &Lua, h: FrameHandle) -> bool {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .arena
        .glance(h)
        .is_some_and(|g| g.kind == crate::widget::FrameKind::GameTooltip)
}

/// A frame's own show or hide notify (the layer show `0x4902d0`, the layer hide `0x490300`): its
/// `OnShow` or `OnHide` unless it is in its XML load, and the kind overrides around it.
fn notify_visibility(lua: &Lua, h: FrameHandle, visible: bool) {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        if model.arena.frame(h).is_none() {
            return;
        }
        if !visible {
            // The button's hide notify un-presses it, unless disabled or locked, before the base
            // notify, so a button hidden while held comes back unpushed.
            super::button::edge(&mut model, h, ButtonState::on_hide);
        }
        (!model.xml_loading.contains(&h)).then(|| model.frame_id(h))
    };
    if let Some(id) = id {
        let name = if visible { "OnShow" } else { "OnHide" };
        if let Err(e) = fire(lua, id, name, None, Vec::new()) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
    }
    // The EditBox show/hide overrides call the base notify first, so this runs after the `fire`:
    // an `autoFocus` box takes a free focus, a hidden box releases its own.
    super::editbox::visibility_focus(lua, h, visible);
}

/// Fire `OnShow`/`OnHide` for the frames an engine-side cascade changed, in its order (each frame
/// after its descendants), recording errors in [`Model::errors`]; a verb with handlers to run
/// between the steps uses [`set_shown`] instead. A shown `toplevel` frame raises before its
/// `OnShow` (`0x4918f0`), so a show that does not come through Lua `Show` raises too.
pub(super) fn fire_visibility_changes(lua: &Lua, changed: Vec<FrameHandle>) {
    if changed.is_empty() {
        return; // nothing to fire, and no borrow to count as a change
    }
    // Resolve (handle, id, now-visible?) under one short borrow, then fire with no borrow held.
    let items: Vec<(FrameHandle, u32, bool)> = {
        let mut model = crate::script::model_mut_of(lua);
        changed
            .into_iter()
            .filter_map(|h| {
                let vis = model.arena.frame(h)?.effective_visible;
                Some((h, model.frame_id(h), vis))
            })
            .collect()
    };
    // Hiding the hovered frame, directly or by an ancestor, fires its `OnLeave` inside the hide and
    // before its `OnHide`, clears the hover and the drag-arm (`+0x100`/`+0x104`) and arms the
    // re-pick; a show arms it too. This leave is how a tooltip closes with its window; the
    // reference has no other.
    let left: Option<(FrameHandle, u32)> = {
        let mut model = crate::script::model_mut_of(lua);
        for &(h, _, _) in items.iter().filter(|&&(_, _, vis)| !vis) {
            drop_mouse_holds(&mut model, h);
        }
        let hidden_hover = model
            .mouseover
            .filter(|&m| items.iter().any(|&(h, _, vis)| h == m && !vis));
        // The leave is a virtual call (`[vtable+0x50]`): a disabled Button's own skips its
        // `OnLeave`, while the hover and the drag-arm still clear.
        let notified = super::button::hover_notify_runs(&model, hidden_hover);
        if let Some(m) = hidden_hover {
            model.mouseover = None;
            if model.drag.as_ref().is_some_and(|d| d.source == m) {
                model.drag = None;
            }
        }
        let mouse_shown = items
            .iter()
            .any(|&(h, _, vis)| vis && model.arena.is_mouse_enabled(h));
        model.hover_repick |= hidden_hover.is_some() || mouse_shown;
        hidden_hover
            .filter(|_| notified)
            .map(|m| (m, model.frame_id(m)))
    };
    if let Some((_, oid)) = left {
        if let Err(e) = fire_widget_handler(lua, oid, "OnLeave", vec![Value::Boolean(false)]) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
    }
    for (h, id, visible) in items {
        if visible {
            let mut model = crate::script::model_mut_of(lua);
            super::object::toplevel::raise_on_show(&mut model, h);
        } else {
            // The button's hide notify (`+0x34`) un-presses it, unless disabled or locked, before
            // the base notify, so a button hidden while held comes back unpushed.
            let mut model = crate::script::model_mut_of(lua);
            super::button::edge(&mut model, h, ButtonState::on_hide);
        }
        let name = if visible { "OnShow" } else { "OnHide" };
        if let Err(e) = fire(lua, id, name, None, Vec::new()) {
            crate::script::model_mut_of(lua).record_lua_error(&e);
        }
        // The EditBox show/hide overrides (`+0x30`/`+0x34`) call the base notify first, so this
        // runs after the `fire`: an `autoFocus` box takes a free focus, a hidden box releases its
        // own.
        super::editbox::visibility_focus(lua, h, visible);
    }
}

/// Whether a handler is bound under `script`, without firing it: the keyboard walk consumes on the
/// slot's presence. A failed lookup reads absent, so the gate never raises.
pub(super) fn has_widget_handler(lua: &Lua, id: u32, script: &str) -> bool {
    let Ok(scripts) = lua.named_registry_value::<Table>(REG_SCRIPTS) else {
        return false;
    };
    match scripts.get::<Value>(id) {
        Ok(Value::Table(per)) => matches!(per.get::<Value>(script), Ok(Value::Function(_))),
        _ => false,
    }
}

/// The one firing path: `event_name` is `OnEvent`'s, `extra` the `arg1..argN`. Holds no model
/// borrow across the call.
fn fire(
    lua: &Lua,
    id: u32,
    script: &str,
    event_name: Option<&str>,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let func: Function = match scripts.get::<Value>(id)? {
        Value::Table(per) => match per.get::<Value>(script)? {
            Value::Function(f) => f,
            _ => return Ok(()),
        },
        _ => return Ok(()),
    };

    let wrapper = frame_wrapper(lua, id)?;
    // The profiler's fire opens after `frame_wrapper`, where no model borrow is held, and closes
    // when the guard drops, unwinds included.
    let _fire =
        super::handler_prof::armed().then(|| super::handler_prof::Fire::open(lua, id, script));
    invoke_with_globals(lua, wrapper, &func, event_name, extra)
}

/// The parameter list an XML handler body of `script` is compiled under: one of `Wow.exe`'s
/// `return function(…) %s end` templates (`0x9eb31c`…`0xaa2ae8`), picked by a name compare ahead
/// of the template's store; any other name takes the bare `self` (`0x9eb868`). Names are matched
/// as the XML schema spells them.
pub(crate) fn script_parameters(script: &str) -> &'static str {
    match script {
        "OnEvent" => "self,event,...",
        "OnUpdate" => "self,elapsed",
        "OnClick" | "PreClick" | "PostClick" => "self,button,down",
        "OnMouseDown" | "OnMouseUp" | "OnDragStart" | "OnDoubleClick" => "self,button",
        "OnEnter" | "OnLeave" => "self,motion",
        "OnMouseWheel" => "self,delta",
        "OnKeyDown" | "OnKeyUp" => "self,key",
        "OnChar" | "OnCharComposition" | "OnMovieShowSubtitle" => "self,text",
        "OnSizeChanged" => "self,w,h",
        "OnAttributeChanged" => "self,name,value",
        "OnValueChanged" => "self,value",
        "OnMinMaxChanged" => "self,min,max",
        "OnTextChanged" => "self, userInput",
        "OnInputLanguageChanged" => "self,language",
        "OnCursorChanged" => "self,x,y,w,h",
        "OnVerticalScroll" | "OnHorizontalScroll" => "self,offset",
        "OnScrollRangeChanged" => "self,xrange,yrange",
        "OnColorSelect" => "self,r,g,b",
        "OnHyperlinkClick" => "self,link,text,button",
        "OnHyperlinkEnter" | "OnHyperlinkLeave" => "self,link,text",
        "OnTooltipAddMoney" => "self,cost,maxcost",
        // Animation scripts.
        "OnStop" | "OnFinished" => "self,requested",
        "OnLoop" => "self,loopState",
        _ => "self",
    }
}

/// The calling convention, shared by [`fire`] and the loader's `OnLoad`: the handler is called
/// with `self`, then `event` for `OnEvent`, then the arguments. As `0x819ea0` does, `this`, `event`
/// and one `argN` global per argument after the event name are set around the call and restored
/// after it, even on error, so nested fires are safe; a higher `argN` is left as it was.
pub(crate) fn invoke_with_globals(
    lua: &Lua,
    wrapper: Table,
    func: &Function,
    event_name: Option<&str>,
    extra: Vec<Value>,
) -> mlua::Result<()> {
    let g = lua.globals();
    let saved_this: Value = g.get("this")?;
    let saved_event: Value = g.get("event")?;
    let saved_args: Vec<Value> = (1..=extra.len())
        .map(|n| g.get(arg_name(n)))
        .collect::<mlua::Result<_>>()?;

    g.set("this", wrapper.clone())?;
    let event_value = match event_name {
        Some(ev) => Some(Value::String(lua.create_string(ev)?)),
        None => None,
    };
    if let Some(ev) = &event_value {
        g.set("event", ev.clone())?;
    }
    for (n, value) in (1..).zip(&extra) {
        g.set(arg_name(n), value.clone())?;
    }

    let args: mlua::MultiValue = std::iter::once(Value::Table(wrapper))
        .chain(event_value)
        .chain(extra)
        .collect();
    // A protected call; the globals are restored before its outcome returns.
    let outcome = super::stdlib::protected_call(lua, func, args);

    g.set("this", saved_this)?;
    g.set("event", saved_event)?;
    for (n, value) in (1..).zip(saved_args) {
        g.set(arg_name(n), value)?;
    }

    outcome
}

/// The global the `n`th handler argument is set under (`0x819ea0` writes the number into its
/// `"arg0"` buffer, `0xaf57f4`), with no upper bound.
fn arg_name(n: usize) -> std::borrow::Cow<'static, str> {
    const NAMES: [&str; 20] = [
        "arg1", "arg2", "arg3", "arg4", "arg5", "arg6", "arg7", "arg8", "arg9", "arg10", "arg11",
        "arg12", "arg13", "arg14", "arg15", "arg16", "arg17", "arg18", "arg19", "arg20",
    ];
    match NAMES.get(n.wrapping_sub(1)) {
        Some(name) => (*name).into(),
        None => format!("arg{n}").into(),
    }
}

#[cfg(test)]
mod script_parameters_tests {
    use super::script_parameters as p;

    #[test]
    fn each_script_takes_its_template_and_the_rest_take_self() {
        assert_eq!(p("OnEvent"), "self,event,...");
        assert_eq!(p("OnUpdate"), "self,elapsed");
        assert_eq!(p("OnClick"), "self,button,down");
        assert_eq!(p("OnEnter"), "self,motion");
        assert_eq!(p("OnTextChanged"), "self, userInput");
        assert_eq!(p("OnLoad"), "self");
        assert_eq!(p("OnShow"), "self");
        assert_eq!(p("OnDragStop"), "self");
    }
}
