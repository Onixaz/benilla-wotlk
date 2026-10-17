//! The `Button` and `CheckButton` methods (`CSimpleButton` `0x6eeab0`, `CSimpleCheckbox`
//! `0x6eeb30`). The shown state texture latches at each state edge (`SetState` `0x779790`), not at
//! paint. `<PushedTextOffset>` is not built: a pushed label does not shift.
//! Lookup runs CheckButton's table, then Button's, then the frame table, the client's class chain,
//! so `SetChecked` is nil on a plain Button and `GetText` on a plain Frame.

use mlua::{Lua, MultiValue, ObjectLike, Table, Value};

use super::object::{as_f32, frame_handle_of};
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::justify::Justify;
use crate::order::DrawLayer;
use crate::widget::{
    ButtonFont, ButtonState, ButtonVisualState, FrameHandle, FrameKind, KindState, RegionHandle,
    RegionKind,
};

pub(super) const REG_BUTTON_METHODS: &str = "__benilla_button_methods";
pub(super) const REG_CHECKBUTTON_METHODS: &str = "__benilla_checkbutton_methods";

/// A Button's texture and label slots, each an arena region created on first set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Normal,
    Pushed,
    Disabled,
    Highlight,
    Checked,
    DisabledChecked,
    Text,
}

impl Slot {
    fn get(self, bs: &ButtonState) -> Option<crate::widget::RegionHandle> {
        match self {
            Slot::Normal => bs.normal,
            Slot::Pushed => bs.pushed,
            Slot::Disabled => bs.disabled,
            Slot::Highlight => bs.highlight,
            Slot::Checked => bs.checked_tex,
            Slot::DisabledChecked => bs.disabled_checked,
            Slot::Text => bs.text,
        }
    }

    /// The three state slots go through [`ButtonState::set_state_slot`] (`0x778fd0`), which also
    /// shows the texture when it is the current state's; the others are plain fields.
    fn set(self, bs: &mut ButtonState, rh: Option<crate::widget::RegionHandle>) {
        match self {
            Slot::Normal => bs.set_state_slot(ButtonVisualState::Normal, rh),
            Slot::Pushed => bs.set_state_slot(ButtonVisualState::Pushed, rh),
            Slot::Disabled => bs.set_state_slot(ButtonVisualState::Disabled, rh),
            Slot::Highlight => bs.highlight = rh,
            Slot::Checked => bs.checked_tex = rh,
            Slot::DisabledChecked => bs.disabled_checked = rh,
            Slot::Text => bs.text = rh,
        }
    }

    /// The region kind and default layer: state textures under the label, the highlight in its own
    /// layer (the draw-layer table `0x811a84`), the checked marks above the state.
    fn shape(self) -> (RegionKind, DrawLayer) {
        match self {
            Slot::Normal | Slot::Pushed | Slot::Disabled => {
                (RegionKind::Texture, DrawLayer::Artwork)
            }
            Slot::Checked | Slot::DisabledChecked => (RegionKind::Texture, DrawLayer::Overlay),
            Slot::Highlight => (RegionKind::Texture, DrawLayer::Highlight),
            Slot::Text => (RegionKind::FontString, DrawLayer::Overlay),
        }
    }
}

/// The normal font's justify, its font object's or CENTER: the slot holds the shared object
/// (`[button+0x2b0]`), whose style flags the label adopter reads (`SetFontString 0x96f7b0`).
fn normal_font_justify(model: &Model, bs: &ButtonState) -> Justify {
    let mut word = Justify::default();
    if let Some(j) = bs
        .normal_font
        .as_deref()
        .and_then(|n| model.font_object(n))
        .and_then(|fo| fo.justify_h)
    {
        word.set_h(j);
    }
    word
}

/// `SetFontString`'s tail (`0x96f7b0`), which `SetText` also creates its label through
/// (`0x96f860`): an unanchored label anchors LEFT, RIGHT or CENTER by the normal font's justify,
/// never its own, then takes the per-state font (`UpdateTextState 0x96f4a0`).
fn adopt_label(model: &mut Model, owner: FrameHandle, rh: RegionHandle) {
    let point = {
        let Some(frame) = model.arena.frame(owner) else {
            return;
        };
        let KindState::Button(bs) = &frame.kind_state else {
            return;
        };
        super::region::justify_anchor_point(normal_font_justify(model, bs).0)
    };
    super::region::anchor_unanchored_at(model, rh, point);
    apply_normal_font(model, owner);
}

/// Link the label to the normal font object (`UpdateTextState 0x96f4a0` → `SetFontObject
/// 0x485160`), so its `GetFont` answers what it paints; the stock `DropDownList1` reads it
/// (`UIDropDownMenu.xml:11`). Deviation: a hovered or disabled label still reports the normal
/// font, since the state fonts are overlaid at paint and a link made here would outlive the state.
/// `font::repaint` keeps what the label set itself (its severance mask, `CSimpleFontString+0xd4`).
fn apply_normal_font(model: &mut Model, owner: FrameHandle) {
    let (rh, name) = {
        let Some(frame) = model.arena.frame(owner) else {
            return;
        };
        let KindState::Button(bs) = &frame.kind_state else {
            return;
        };
        let Some(rh) = bs.text else {
            return;
        };
        (rh, bs.normal_font.clone())
    };
    // An unregistered name is no error here: the setter accepted it and the loader reports it.
    let fo = name.as_deref().and_then(|n| model.font_object(n).cloned());
    let d = model.region_data.entry(rh).or_default();
    match (&name, fo) {
        (None, _) => d.font_object = None,
        (Some(_), None) => {}
        (Some(name), Some(fo)) => {
            d.font_object = Some(name.clone());
            super::font::repaint(d, &fo);
        }
    }
    model.touch_measure(rh);
}

/// Reads one of the three per-state font slots.
type FontSlot = fn(&ButtonState) -> Option<String>;

/// Run `f` over a frame's Button state under one short write borrow. It moves no state, only an
/// edge does: re-registering a held button's clicks does not un-press it, as in the reference.
fn with_button<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ButtonState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Button(bs) => Ok(f(bs)),
        _ => Err(mlua::Error::runtime("not a Button")),
    }
}

/// `Enable()`/`Disable()`. `Disable` (`0x77ffd0`) reaches `SetEnabled` (`0x779160`), which also
/// turns draw layer 4, HIGHLIGHT, off (`0x7791bb`) in the array `DisableDrawLayer` writes, so every
/// region in that layer goes dark, not only the HighlightTexture. `Enable()` here turns it back
/// on; the reference restores it only on a hovered button, by re-running its `<OnEnter>`
/// (`0x779183`), and never while `LockHighlight` holds (`[frame+0xf8]`).
fn set_enabled(lua: &Lua, this: &Table, on: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let fire = {
        let mut model = crate::script::model_mut_of(lua);
        let frame = model
            .arena
            .frame_mut(h)
            .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
        let changed = match &mut frame.kind_state {
            KindState::Button(bs) => bs.set_enabled(on),
            _ => return Err(mlua::Error::runtime("not a Button")),
        };
        let bit = 1u8 << DrawLayer::Highlight.index();
        if on {
            frame.disabled_layers &= !bit;
        } else {
            frame.disabled_layers |= bit;
        }
        // A change runs `OnEnable`/`OnDisable` (`0x9704eb`), unless the frame is still in its XML
        // load (`+0x130`, `0x9704fe`).
        (changed && !model.xml_loading.contains(&h)).then(|| model.frame_id(h))
    };
    if let Some(id) = fire {
        let script = if on { "OnEnable" } else { "OnDisable" };
        if let Err(e) = event::fire_widget_handler(lua, id, script, Vec::new()) {
            crate::script::model_mut_of(lua).record_script_error(e.to_string());
        }
    }
    Ok(())
}

/// Run an engine state edge over frame `h` if it is a live Button. The reference has six, all of
/// `0x779790`'s call sites but Lua's: enable, disable, hide, press, release and drag start; the
/// cursor entering or leaving a button is not one.
pub(super) fn edge(model: &mut Model, h: FrameHandle, f: impl FnOnce(&mut ButtonState)) {
    if let Some(frame) = model.arena.frame_mut(h) {
        if let KindState::Button(bs) = &mut frame.kind_state {
            f(bs);
        }
    }
}

/// Whether a hover edge on `h` reaches the base notify, the HIGHLIGHT layer and the Lua
/// `<OnEnter>`/`<OnLeave>`: `OnEnter`/`OnLeave` (`0x779490`/`0x7794e0`) skip it on a DISABLED
/// Button, leaving only the hover sound (`0x7794b0`). Every hover path goes through that pair
/// (the hover walk `0x766218`, `SetMouseFocus` `0x764dc0`, the hide tail `0x764cce`), and the
/// Hyperlink and NamePlate overrides chain back (`0x7cb8a5`). The hover target still moves, so
/// `GetMouseFocus()` answers a disabled button.
pub(super) fn hover_notify_runs(model: &Model, h: Option<FrameHandle>) -> bool {
    let Some(h) = h else { return true };
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.enabled(),
        _ => true,
    }
}

/// Get-or-create the region behind `slot`; returns its id.
fn ensure_slot(lua: &Lua, this: &Table, slot: Slot) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::Button(bs) => slot.get(bs),
        _ => return Err(mlua::Error::runtime("not a Button")),
    };
    let rh = match existing {
        Some(rh) => rh,
        None => {
            let (kind, layer) = slot.shape();
            let rh = model
                .arena
                .create_region(h, kind, layer, 0)
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            // A new highlight blends ADD, `SetHighlightTexture`'s default (`0x781d92`). The
            // reference applies that mode, or a third argument's, on every call and to a handed-in
            // Texture too (`0x7703f0` from `0x779050` and `0x779110`); here only a new region
            // takes it.
            model.region_data.insert(
                rh,
                RegionData {
                    blend: if slot == Slot::Highlight {
                        crate::script::BlendMode::Add
                    } else {
                        crate::script::BlendMode::default()
                    },
                    ..Default::default()
                },
            );
            model.touch_layout(); // a region entered the layout gate's read set
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Button(bs) = &mut frame.kind_state {
                    slot.set(bs, Some(rh));
                }
            }
            // Only a new region gets an implicit anchor: a state texture fills the button
            // (`0x778f9d`/`0x7790db`), a label goes to the adopter (`0x96f860` → `0x96f7b0`).
            match slot {
                Slot::Text => adopt_label(&mut model, h, rh),
                _ => super::region::implicit_creation_anchor(&mut model, rh),
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

/// Point `slot` at `rh` or at nothing, returning the different region it held for the caller to
/// free, as the slot store `0x778fd0` destroys the outgoing object.
fn swap_slot(
    model: &mut Model,
    h: FrameHandle,
    slot: Slot,
    rh: Option<RegionHandle>,
) -> mlua::Result<Option<RegionHandle>> {
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    let KindState::Button(bs) = &mut frame.kind_state else {
        return Err(mlua::Error::runtime("not a Button"));
    };
    let outgoing = slot.get(bs).filter(|old| Some(*old) != rh);
    slot.set(bs, rh);
    model.touch_layout();
    Ok(outgoing)
}

/// Free the region a slot gave up, never the one that moved in.
fn free_outgoing(lua: &Lua, outgoing: Option<RegionHandle>, incoming: RegionHandle) {
    if let Some(old) = outgoing.filter(|old| *old != incoming) {
        super::region::free_region(&mut crate::script::model_mut_of(lua), old);
    }
}

/// `Set<State>Texture(texture | "path" | nil)`, forked on the argument's type (`0x781970`). The
/// object and nil legs never touch the slot's own region, so they run before [`ensure_slot`]
/// would create one. There is no colour form: a number takes the path leg through `lua_isstring`
/// (`0x781b23`) and loads as its own decimal name, so `(1, 1, 1, 0)` is the file `"1"`; anything
/// else, no argument included, raises `Usage: <name>:<usage>` (`0x87a1b4`).
fn set_slot_texture(
    lua: &Lua,
    this: &Table,
    slot: Slot,
    usage: &str,
    args: &MultiValue,
) -> mlua::Result<()> {
    match args.front() {
        // A Texture object is itself installed into the slot (`0x781b0b` → `0x778fd0`).
        Some(Value::Table(t)) => {
            let rh = super::region::region_handle_of(lua, t)?;
            let h = frame_handle_of(lua, this)?;
            let mut model = crate::script::model_mut_of(lua);
            // Any other widget raises (`0x781ab1`, message `0x87a1e0`).
            if model.arena.region(rh).map(|r| r.kind) != Some(RegionKind::Texture) {
                return Err(mlua::Error::runtime("Wrong object type, expected texture"));
            }
            let outgoing = swap_slot(&mut model, h, slot, Some(rh))?;
            drop(model);
            free_outgoing(lua, outgoing, rh);
            return Ok(());
        }
        // nil clears the slot and destroys the old region (`0x781b5a` → `0x778fd0`): unhooked but
        // alive, it would draw in every state. No argument is not nil (`0x781b4f`) and clears
        // nothing.
        Some(Value::Nil) => {
            let h = frame_handle_of(lua, this)?;
            let mut model = crate::script::model_mut_of(lua);
            let outgoing = swap_slot(&mut model, h, slot, None)?;
            drop(model);
            if let Some(old) = outgoing {
                super::region::free_region(&mut crate::script::model_mut_of(lua), old);
            }
            return Ok(());
        }
        _ => {}
    }

    // `lua_tostring` (`0x781b33`): a number converts as `%.14g`, as the stack's coercion does.
    let path = match args.front() {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Integer(_))) => {
            lua.coerce_string(v.clone())?.map(|s| s.to_string_lossy())
        }
        _ => None,
    };
    let Some(path) = path else {
        let who = {
            let h = frame_handle_of(lua, this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            model
                .arena
                .frame(h)
                .and_then(|f| f.name.clone())
                .unwrap_or_else(|| "<unnamed>".to_string())
        };
        return Err(mlua::Error::runtime(format!("Usage: {who}:{usage}")));
    };
    let id = ensure_slot(lua, this, slot)?;
    let mut model = crate::script::model_mut_of(lua);
    let rh = *model.id_to_region.get(&id).expect("slot region id");
    let data = model.region_data.entry(rh).or_default();
    // `""` clears the texture, which `QuestLogFrame.lua:165` relies on.
    data.texture = (!path.is_empty()).then_some(path);
    data.fill = None;
    Ok(())
}

/// `Get<State>Texture`: the region wrapper, or nil while unset.
fn get_slot_texture(lua: &Lua, this: &Table, slot: Slot) -> mlua::Result<Value> {
    let rh = with_button(lua, this, |bs| slot.get(bs))?;
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        rh.map(|rh| model.region_id(rh))
    };
    match id {
        Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
        None => Ok(Value::Nil),
    }
}

/// Register one `Set<X>Texture`/`Get<X>Texture` pair on `m`.
fn texture_pair(lua: &Lua, m: &Table, name: &str, slot: Slot) -> mlua::Result<()> {
    let method = format!("Set{name}Texture");
    // The highlight setter's own line names its blend argument (`0x87a240`), unclosed as in the
    // reference; the others share `0x87a1b4`.
    let usage = if slot == Slot::Highlight {
        format!("{method}(texture or \"texture\" or nil [, \"blendmode\")")
    } else {
        format!("{method}(texture or \"texture\" or nil)")
    };
    m.set(
        method.as_str(),
        lua.create_function(move |lua, (this, args): (Table, MultiValue)| {
            set_slot_texture(lua, &this, slot, &usage, &args)
        })?,
    )?;
    m.set(
        format!("Get{name}Texture"),
        lua.create_function(move |lua, this: Table| get_slot_texture(lua, &this, slot))?,
    )?;
    Ok(())
}

/// `text` into the button's label, created on first use, through the label's grammar pass
/// (`0x96f860` sets it with the process flag).
fn set_label(lua: &Lua, this: &Table, text: String) -> mlua::Result<()> {
    let id = ensure_slot(lua, this, Slot::Text)?;
    let mut model = crate::script::model_mut_of(lua);
    let rh = *model.id_to_region.get(&id).expect("text region id");
    model.set_region_text(rh, Some(crate::language::process_owned(text)));
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    texture_pair(lua, &m, "Normal", Slot::Normal)?;
    texture_pair(lua, &m, "Pushed", Slot::Pushed)?;
    texture_pair(lua, &m, "Disabled", Slot::Disabled)?;
    texture_pair(lua, &m, "Highlight", Slot::Highlight)?;

    // SetText/GetText go to the ButtonText fontstring, as the `text` attribute does (`0x96f860`).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<Value>)| {
            let text = super::binding_abi::text_arg(lua, text)?;
            // A nil returns at once (`0x778dcc`): no clear, no label created. The guard is the
            // button's own; `FontString:SetText(nil)` clears (`0x771d80`).
            let Some(text) = text else { return Ok(()) };
            set_label(lua, &this, text)
        })?,
    )?;
    // 3.3.5a's `Button:SetFormattedText(fmt, ...)` (`0x9779b0`). Its setter (`0x96f860`)
    // creates no label for an empty string.
    m.set(
        "SetFormattedText",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let text = super::formatted_text::format(lua, &args)?;
            if text.is_empty() && with_button(lua, &this, |bs| bs.text)?.is_none() {
                return Ok(());
            }
            set_label(lua, &this, text)
        })?,
    )?;
    // GetText (`0x780e10`): nil for no label, and for an empty one (`0x780ec5`).
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| {
            let rh = with_button(lua, &this, |bs| bs.text)?;
            let text = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                rh.and_then(|rh| model.region_data.get(&rh))
                    .and_then(|d| d.text.clone())
                    .filter(|t| !t.is_empty())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    m.set(
        "GetFontString",
        lua.create_function(|lua, this: Table| get_slot_texture(lua, &this, Slot::Text))?,
    )?;

    // SetFontString(fontString) (`0x9777c0` → `0x96f7b0`) raises, naming the button, on a
    // non-table (`0x87a100`), nil included, so Lua cannot clear the label; on a non-widget
    // (`0x87a160`); on a non-FontString (`0x87a124`). The same label is a no-op; otherwise the old
    // one is destroyed and the new one moves to the button at ARTWORK (`0x77fd10`) and is adopted.
    m.set(
        "SetFontString",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a button")),
            };
            let who = {
                let h = frame_handle_of(lua, &this)?;
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            let Some(Value::Table(fs)) = it.next() else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {who}:SetFontString(fontstring)"
                )));
            };
            // Two checks in the reference's order, a widget at all (`0x780b27`), then a FontString
            // (`0x780b90`), so a Frame or a Texture gets the second message.
            let rh = {
                let not_an_object = || {
                    mlua::Error::runtime(format!(
                        "{who}:SetFontString(): Couldn't find 'this' in fontstring"
                    ))
                };
                let id = super::object::decode_id(&fs).map_err(|_| not_an_object())?;
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let known_widget =
                    model.id_to_frame.contains_key(&id) || model.id_to_region.contains_key(&id);
                if !known_widget {
                    return Err(not_an_object());
                }
                match model.id_to_region.get(&id).copied() {
                    Some(rh)
                        if model.arena.region(rh).map(|r| r.kind)
                            == Some(RegionKind::FontString) =>
                    {
                        rh
                    }
                    _ => {
                        return Err(mlua::Error::runtime(format!(
                            "{who}:SetFontString(): Wrong object type, expected fontstring"
                        )))
                    }
                }
            };
            let old = with_button(lua, &this, |bs| bs.text)?;
            if old == Some(rh) {
                return Ok(());
            }
            let owner = frame_handle_of(lua, &this)?;
            with_button(lua, &this, |bs| bs.text = Some(rh))?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(dead) = old {
                // Destroyed, not orphaned (`0x778d3c`).
                super::region::free_region(&mut model, dead);
            }
            model.arena.set_region_owner(rh, Some(owner));
            if let Some(r) = model.arena.region_mut(rh) {
                r.draw_layer = DrawLayer::Artwork;
            }
            adopt_label(&mut model, owner, rh);
            // The layout's read set moved, and `GetTextWidth` reads the new label's extents.
            model.touch_layout();
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // GetTextWidth/GetTextHeight (`0x782290`/`0x782390`) forward to the label's extents (vtable
    // `0x1c`/`0x20`). The width is the natural, unwrapped one: the laid-out width would feed a
    // size-to-text caller (`MoneyFrame.lua:202`) its own output. 0 before measurement and with no
    // label, where the reference dereferences `+0x338` and what it does on null is untraced.
    for (name, region_getter) in [
        ("GetTextWidth", "GetStringWidth"),
        // 1.12 has no `GetStringHeight`; `0x782390` reads the label's height.
        ("GetTextHeight", "GetHeight"),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, this: Table| {
                let label = get_slot_texture(lua, &this, Slot::Text)?;
                match label {
                    Value::Table(t) => t.call_method::<f32>(region_getter, ()),
                    _ => Ok(0.0),
                }
            })?,
        )?;
    }

    // The per-state label fonts, and `<NormalFont>` and kin through the loader. Each takes a font
    // object, its name or nil, stored by name: the slots point at the shared object (`+0x2b0`,
    // `+0x2b4`, `+0x2b8`) and extract re-resolves the current state's every frame, so a state
    // change or a later edit to the object reaches the label.
    m.set(
        "SetNormalFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let font = super::font::resolve("SetNormalFontObject", &font)?;
            let owner = frame_handle_of(lua, &this)?;
            with_button(lua, &this, |bs| bs.normal_font = font)?;
            let mut model = crate::script::model_mut_of(lua);
            apply_normal_font(&mut model, owner);
            Ok(())
        })?,
    )?;
    // The slot's font object, or nothing at all when it is empty (`0x977450`, `0x977750`,
    // `0x9775d0`).
    let slots: [(&str, FontSlot); 3] = [
        ("GetNormalFontObject", |bs| bs.normal_font.clone()),
        ("GetHighlightFontObject", |bs| bs.highlight_font.clone()),
        ("GetDisabledFontObject", |bs| bs.disabled_font.clone()),
    ];
    for (name, slot) in slots {
        m.set(
            name,
            lua.create_function(move |lua, this: Table| {
                let font = with_button(lua, &this, |bs| slot(bs))?.filter(|n| {
                    lua.app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data")
                        .font_object(n)
                        .is_some()
                });
                match font {
                    Some(n) => Ok(MultiValue::from_iter([Value::Table(super::font::wrapper(
                        lua, &n,
                    )?)])),
                    None => Ok(MultiValue::new()),
                }
            })?,
        )?;
    }
    m.set(
        "SetHighlightFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetHighlightFontObject", &font)?;
            with_button(lua, &this, |bs| bs.highlight_font = name)
        })?,
    )?;
    m.set(
        "SetDisabledFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetDisabledFontObject", &font)?;
            with_button(lua, &this, |bs| bs.disabled_font = name)
        })?,
    )?;

    // SetFont(file, height [, flags]) (`0x780880`) sets all three embedded fonts and returns
    // nothing, discarding the shared `0x79f210`'s load result. It never touches the label
    // (`+0x338`), so creates none: extract applies the stored font to whatever label exists. Its
    // checks are 5.0's coercing `lua_isstring`/`lua_isnumber`, else a usage error (`0x87c69c`).
    m.set(
        "SetFont",
        lua.create_function(
            |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                let usage = || {
                    mlua::Error::runtime(
                        "Usage: <Button>:SetFont(\"font\", fontHeight [, flags])".to_string(),
                    )
                };
                let path = match &file {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Number(_) | Value::Integer(_) => as_f32(&file).to_string(),
                    _ => return Err(usage()),
                };
                let height = match &height {
                    Value::Number(_) | Value::Integer(_) => as_f32(&height),
                    Value::String(s) => s.to_str()?.parse::<f32>().map_err(|_| usage())?,
                    _ => return Err(usage()),
                };
                let flags = super::Outline::flags(flags.as_deref().unwrap_or(""))
                    .as_str()
                    .to_string();
                with_button(lua, &this, |bs| {
                    bs.font = Some(ButtonFont {
                        path,
                        height,
                        flags,
                    })
                })
            },
        )?,
    )?;
    // GetFont() (`0x7809a0` → `0x79f3b0`) → file, height, flags off the normal font, else the
    // object it inherits (`<NormalFont>`, `SetNormalFontObject`); always three values.
    m.set(
        "GetFont",
        lua.create_function(|lua, this: Table| {
            let (own, inherits) =
                with_button(lua, &this, |bs| (bs.font.clone(), bs.normal_font.clone()))?;
            let (path, height, flags) = match own {
                Some(f) => (Some(f.path), Some(f.height), f.flags),
                None => {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    let fo = inherits.and_then(|n| model.font_object(&n));
                    (
                        fo.and_then(|f| f.font.clone()),
                        fo.and_then(|f| f.height),
                        fo.map(|f| f.outline)
                            .unwrap_or_default()
                            .as_str()
                            .to_string(),
                    )
                }
            };
            let path = match path {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            Ok((path, height, flags))
        })?,
    )?;

    // The per-state label colours: a set one paints over the state font's colour at extract.
    m.set(
        "SetTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.normal_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;
    // GetTextColor() (`0x781100`) → r, g, b, a, four values, off the normal font: its own colour,
    // else the inherited object's (`<NormalFont>`, `SetNormalFontObject`), else white.
    m.set(
        "GetTextColor",
        lua.create_function(|lua, this: Table| {
            let (own, inherits) =
                with_button(lua, &this, |bs| (bs.normal_color, bs.normal_font.clone()))?;
            let c = own.unwrap_or_else(|| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                inherits
                    .and_then(|n| model.font_object(&n))
                    .and_then(|f| f.color)
                    .unwrap_or([1.0, 1.0, 1.0, 1.0])
            });
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    m.set(
        "SetHighlightTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.highlight_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;
    m.set(
        "SetDisabledTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.disabled_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;

    // LockHighlight (`0x782840`) and UnlockHighlight: the highlight shows regardless of hover.
    m.set(
        "LockHighlight",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| bs.locked_highlight = true)
        })?,
    )?;
    m.set(
        "UnlockHighlight",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| bs.locked_highlight = false)
        })?,
    )?;

    m.set(
        "Enable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, true))?,
    )?;
    m.set(
        "Disable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, false))?,
    )?;
    // IsEnabled() (`0x7800b0`) → the number 1 or 0 from the state at `+0x328`, never a boolean:
    // 0 is truthy in Lua, and the stock UI tests `== 0` (`FriendsFrame.lua:404`) and `== 1`
    // (`StaticPopup.lua:713`).
    m.set(
        "IsEnabled",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| i64::from(bs.enabled()))
        })?,
    )?;

    // RegisterForClicks(...) (`0x9781d0`) replaces the mask with the OR of its names' bits
    // (`0x96e9c0`), reading arguments while each is a string or number; [`wants_click`] reads it.
    m.set(
        "RegisterForClicks",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let mut mask = 0;
            for v in args.iter() {
                match v {
                    Value::String(s) => mask |= crate::widget::click_mask_of(&s.to_str()?),
                    Value::Integer(_) | Value::Number(_) => {}
                    _ => break,
                }
            }
            with_button(lua, &this, |bs| bs.click_mask = mask)
        })?,
    )?;

    // SetButtonState(state [, lock]) (`0x780270`): DISABLED, NORMAL or PUSHED in any case
    // (`0x780390`), else a usage error; `lock` is `GetBoolOrDefault` with default 0 (`0x78032c`),
    // so the `1` of `MainMenuBarMicroButtons.lua:22` sets it. While locked, presses and releases
    // leave the state alone; unlocked, the next release un-pushes a scripted push.
    m.set(
        "SetButtonState",
        lua.create_function(|lua, (this, state, lock): (Table, String, MultiValue)| {
            let new = if state.eq_ignore_ascii_case("PUSHED") {
                ButtonVisualState::Pushed
            } else if state.eq_ignore_ascii_case("NORMAL") {
                ButtonVisualState::Normal
            } else if state.eq_ignore_ascii_case("DISABLED") {
                ButtonVisualState::Disabled
            } else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: SetButtonState(\"state\", lock) — unknown state '{state}'"
                )));
            };
            let first = lock.into_iter().next();
            let locked = super::binding_abi::bool_or_default(first.as_ref(), false);
            with_button(lua, &this, |bs| bs.set_button_state(new, locked))
        })?,
    )?;
    m.set(
        "GetButtonState",
        lua.create_function(|lua, this: Table| {
            // The latched state (`0x780180` reads `+0x328`): a held button answers PUSHED because
            // its press wrote it, which the chat scroll repeat reads (`ChatFrame.lua:1588`).
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(KindState::Button(bs)) = model.arena.frame(h).map(|f| &f.kind_state) else {
                return Err(mlua::Error::runtime("not a Button"));
            };
            Ok(match bs.button_state() {
                ButtonVisualState::Disabled => "DISABLED",
                ButtonVisualState::Pushed => "PUSHED",
                ButtonVisualState::Normal => "NORMAL",
            })
        })?,
    )?;

    // Click([button]) (`0x7826c0`): the physical click's path, toggle and `OnClick` alike.
    m.set(
        "Click",
        lua.create_function(|lua, (this, button): (Table, Option<String>)| {
            let h = frame_handle_of(lua, &this)?;
            let id = {
                let mut model = crate::script::model_mut_of(lua);
                model.frame_id(h)
            };
            let btn = button.unwrap_or_else(|| "LeftButton".to_string());
            click_button(lua, id, &btn, false);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_BUTTON_METHODS, m)?;

    // CheckButton's own table, consulted before Button's.
    let c = lua.create_table()?;
    texture_pair(lua, &c, "Checked", Slot::Checked)?;
    texture_pair(lua, &c, "DisabledChecked", Slot::DisabledChecked)?;
    c.set(
        "SetChecked",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            // `SetChecked` (`0x799bf0`) reads its argument through `0x6f1c10` with default 1
            // (`0x799c77`), not Lua truthiness: a number truncates toward zero (`0x40a2b0`), so
            // `SetChecked(0)` unchecks, and the stock UI passes `"true"`/`"false"`
            // (`SpellBookFrame.lua:296-303`). Here a missing argument unchecks and a string checks
            // only as `"true"` or a nonzero number, where the reference checks for a missing one
            // and reads a string by its first byte, so `"yes"` checks there.
            let checked = match v {
                Value::Boolean(b) => b,
                Value::Integer(i) => i != 0,
                Value::Number(n) => n.trunc() != 0.0,
                Value::String(s) => s.to_str().ok().is_some_and(|s| {
                    let t = s.trim();
                    t.eq_ignore_ascii_case("true")
                        || t.parse::<f64>().is_ok_and(|n| n.trunc() != 0.0)
                }),
                _ => false,
            };
            with_button(lua, &this, |bs| bs.checked = checked)
        })?,
    )?;
    c.set(
        // GetChecked() → the number 1 or nil, never a boolean: `UIOptionsFrame.xml:310` saves
        // `tostring(this:GetChecked())` and `BuffFrame.lua:71` compares it with `"1"`.
        "GetChecked",
        lua.create_function(|lua, this: Table| {
            let checked = with_button(lua, &this, |bs| bs.checked)?;
            Ok(crate::script::binding_abi::flag(checked))
        })?,
    )?;
    lua.set_named_registry_value(REG_CHECKBUTTON_METHODS, c)?;

    Ok(())
}

/// Whether `h` fires `OnClick` for `button`'s release (`up`) or press: a Button tests its click
/// mask ([`ButtonState::wants_click`]); any other kind fires on release only.
pub(super) fn wants_click(model: &Model, h: FrameHandle, button: &str, up: bool) -> bool {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.wants_click(button, up),
        _ => up,
    }
}

/// Whether a press on `h` pushes the art: any Button, whatever its click mask, since
/// `OnLayerMouseDown` (`0x96fb34`) sets PUSHED after the mask test, not under it.
pub(super) fn wants_press_visual(model: &Model, h: FrameHandle) -> bool {
    matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(KindState::Button(_))
    )
}

/// The click shared by the input path and `Click()`: a disabled Button fires nothing, a CheckButton
/// toggles before `OnClick` runs (`0x785550`), then `OnClick` gets the button name alone as `arg1`
/// (`0x779540` fires it with the format `"%s"`); `down`, true only for a click a `…ButtonDown`
/// registration fired, reaches the nameplate's select alone.
pub(super) fn click_button(lua: &Lua, id: u32, button: &str, down: bool) {
    let fire = {
        let mut model = crate::script::model_mut_of(lua);
        let Some(&h) = model.id_to_frame.get(&id) else {
            return;
        };
        let Some(frame) = model.arena.frame_mut(h) else {
            return;
        };
        let is_check = frame.kind == FrameKind::CheckButton;
        match &mut frame.kind_state {
            // A Button fires only while enabled;
            KindState::Button(bs) => {
                if bs.enabled() {
                    if is_check {
                        bs.checked = !bs.checked;
                    }
                    true
                } else {
                    false
                }
            }
            // any other kind with an `OnClick` just fires.
            _ => true,
        }
    };
    if !fire {
        return;
    }
    let btn = match lua.create_string(button) {
        Ok(s) => Value::String(s),
        Err(_) => return,
    };
    // `PreClick`, `OnClick`, `PostClick`, each `(button, down)`, the secure bar's order: its
    // `OnClick` casts and its `PostClick` re-checks the button (`ActionBarFrame.xml:15-17`). The
    // click's button stands for the three, which `IsModifiedClick` reads.
    let outer = crate::script::model_mut_of(lua)
        .click_button
        .replace(button.to_string());
    for script in ["PreClick", "OnClick", "PostClick"] {
        if let Err(e) =
            event::fire_widget_handler(lua, id, script, vec![btn.clone(), Value::Boolean(down)])
        {
            crate::script::model_mut_of(lua).record_script_error(e.to_string());
        }
    }
    crate::script::model_mut_of(lua).click_button = outer;
    // A nameplate click selects its unit, from the pointer or `Click()` alike (the plate's
    // override `0x7cb910` chains the base), after the handler so an erroring hook cannot eat it. A
    // press selects nothing: the plate registers Up clicks only (`0x7cb637`).
    if !down {
        super::nameplate::note_click(lua, id, button);
    }
}
