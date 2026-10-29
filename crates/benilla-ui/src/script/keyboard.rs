//! Frame keyboard delivery. One dispatcher per [`Channel`] calls the keyboard-enabled frames in
//! [`walk_order`] until one consumes. Consumption is the existence of a script slot, not what the
//! handler does: a key-down consumes on `OnKeyDown` or `OnKeyUp` and fires only `OnKeyDown`, so a
//! frame with only `OnKeyUp` swallows key-downs and runs nothing; a char consumes on `OnChar`.
//! Membership is the keyboard-enabled flag, never a script: XML `enableKeyboard` and an XML
//! `<Scripts>` handler set it, Lua `SetScript` does not.
//!
//! The focused EditBox is a frame in the same walk, consuming through its own vtable, so a
//! keyboard frame in a higher stratum pre-empts it and one in a lower stratum does not. A key no
//! frame consumes falls through to the editbox routing, focus acquisition included.
//!
//! `OnKeyUp` is stored and gates key-downs but never fires: the host feeds no key-up, so the
//! reference's key-up gate (`OnKeyUp` alone) and its sticky per-code target (`[root+code*4+0x84]`,
//! the last frame to consume that code's down) are not built.

use mlua::Lua;

use super::{event, Model};
use crate::widget::{FrameHandle, FrameKind};

/// The two walks the reference registers separately.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// Kind-0 buckets, vtable `+0x5c`, fires `OnChar` with the literal character.
    Char,
    /// Kind-1 buckets, vtable `+0x60`, fires `OnKeyDown` with the key name.
    KeyDown,
}

/// The walk order: keyboard-enabled, effectively-visible frames, strata TOOLTIP down to WORLD, then
/// level descending, ties oldest registration first as the inserter keeps them.
fn walk_order(model: &Model) -> Vec<FrameHandle> {
    let mut candidates: Vec<(FrameHandle, u8, u16, u32)> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| f.effective_visible && f.keyboard_enabled)
        .map(|(h, f)| (h, f.strata as u8, f.level, f.insertion_seq))
        .collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    candidates.into_iter().map(|(h, ..)| h).collect()
}

/// The gate's existence test on slots `+0x180`/`+0x188`/`+0x190`, kept in the script registry.
fn has_script(lua: &Lua, id: u32, name: &str) -> bool {
    event::has_widget_handler(lua, id, name)
}

/// Whether `h` is a `CSimpleEditBox`, which the walk asks about focus, never a script slot: its
/// ctor puts it in both buckets, and its vtable replaces the input slots (`+0x5c` with, `+0x60`
/// with) without chaining to the base, consuming when focused and otherwise declining unless it
/// takes a free focus as `autoFocus`. So an unfocused box's `OnChar` (`+0x180`) never runs and
/// never eats a neighbour's keys.
fn is_editbox(lua: &Lua, h: FrameHandle) -> bool {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::EditBox)
}

/// One channel's walk, gate and fire. Returns whether a frame consumed the event, which is what
/// suppresses the keybinding; a handler's own return is discarded.
fn walk(lua: &Lua, channel: Channel, arg: &str) -> bool {
    let order = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        walk_order(&model)
    };
    for h in order {
        // The focused box answers through its own vtable, at its walk position.
        let is_focused_box = {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            model.focused_editbox == Some(h)
        };
        if is_focused_box {
            let consumed = match channel {
                Channel::Char => super::editbox::char_input(lua, arg),
                Channel::KeyDown => super::editbox::key_input(lua, arg),
            };
            if consumed {
                return true;
            }
            continue;
        }
        // An unfocused box declines here; the topmost `autoFocus` box takes a free focus in
        // `super::editbox::route` once the whole walk declines, where the reference's takes it at
        // its own walk position.
        if is_editbox(lua, h) {
            continue;
        }
        let id = {
            let mut model = crate::script::model_mut_of(lua);
            model.frame_id(h)
        };
        match channel {
            // Gates on the OnChar slot alone.
            Channel::Char => {
                if has_script(lua, id, "OnChar") {
                    fire(lua, id, "OnChar", arg);
                    return true;
                }
            }
            // Either key slot consumes, only `OnKeyDown` fires.
            Channel::KeyDown => {
                let down = has_script(lua, id, "OnKeyDown");
                if down || has_script(lua, id, "OnKeyUp") {
                    if down {
                        fire(lua, id, "OnKeyDown", arg);
                    }
                    return true;
                }
            }
        }
    }
    false
}

/// Fire one key handler with its one argument, a string on all three channels; errors are recorded
/// in [`Model::errors`], never raised into the dispatch.
fn fire(lua: &Lua, id: u32, script: &str, arg: &str) {
    let val = mlua::Value::String(match lua.create_string(arg) {
        Ok(s) => s,
        Err(e) => {
            crate::script::model_mut_of(lua).record_script_error(e.to_string());
            return;
        }
    });
    if let Err(e) = event::fire_widget_handler(lua, id, script, vec![val]) {
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
}

/// The char channel: `arg1` is the character in UTF-8, a multi-byte codepoint whole (encodes with
/// no range guard).
pub(super) fn char_input(lua: &Lua, text: &str) -> bool {
    walk(lua, Channel::Char, text)
}

/// The key-down channel: `arg1` is the key name without modifiers, from the same table as the
/// keybinding chord; the host already speaks those names.
pub(super) fn key_input(lua: &Lua, key: &str) -> bool {
    walk(lua, Channel::KeyDown, key)
}

/// The key-down walk for the editing keys a focused box takes as a [`crate::script::EditAction`]
/// (BACKSPACE, DELETE, the arrows, HOME, END): a frame above the box consumes them, and the walk
/// stops at the box with `false` for its chord path, so no frame below it takes them.
pub(super) fn frame_key_input(lua: &Lua, key: &str) -> bool {
    let order = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        walk_order(&model)
    };
    for h in order {
        {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if model.focused_editbox == Some(h) {
                return false; // the box's chord path owns this key
            }
        }
        // An unfocused box declines rather than gating on `+0x188`/`+0x190`.
        if is_editbox(lua, h) {
            continue;
        }
        let id = {
            let mut model = crate::script::model_mut_of(lua);
            model.frame_id(h)
        };
        let down = has_script(lua, id, "OnKeyDown");
        if down || has_script(lua, id, "OnKeyUp") {
            if down {
                fire(lua, id, "OnKeyDown", key);
            }
            return true;
        }
    }
    false
}
