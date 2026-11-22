//! A bar slot as the verbs read it at the call: the app's bar through the lent view (the slot word,
//! `[0xc1e358 + slot * 4]`, its icon `0x5a97f0`, its count `0x5a7d10`), or with none lent the
//! stand-in table a test or fixture pushed. A macro's and an equipment set's icon are the VM's own:
//! it holds the macros and the sets.

use super::action::{ACTION_KIND_EQUIPMENT_SET, ACTION_KIND_MACRO};
use super::live::{read_live, write_live};
use super::model::Model;

/// A slot's word, split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Word {
    /// The kind byte, bits 24-31.
    pub(crate) kind: u8,
    /// The spell, item, macro, set or pet-bar index, bits 0-23.
    pub(crate) action: u32,
}

impl Word {
    fn of(packed: u32) -> Option<Self> {
        (packed != 0).then_some(Word {
            kind: (packed >> 24) as u8,
            action: packed & 0x00FF_FFFF,
        })
    }

    /// The word as the wire and the store pack it.
    pub(crate) fn packed(self) -> u32 {
        (u32::from(self.kind) << 24) | (self.action & 0x00FF_FFFF)
    }
}

/// The 0-based slot of the 1-based Lua `action`.
fn slot(action: u32) -> u32 {
    action.wrapping_sub(1)
}

/// The stand-in slot a test or fixture pushed, with no view lent.
#[cfg(any(test, feature = "fixtures"))]
fn stand_in<R>(model: &Model, action: u32, f: impl FnOnce(&super::ActionSlot) -> R) -> Option<R> {
    model.actions.get(&action).map(f)
}

#[cfg(not(any(test, feature = "fixtures")))]
fn stand_in<R>(_: &Model, _: u32, _: impl FnOnce(&super::ActionSlot) -> R) -> Option<R> {
    None
}

/// The 1-based `action`'s word, `None` for an empty slot.
pub(crate) fn word(model: &Model, action: u32) -> Option<Word> {
    match read_live(|l| l.action_word(slot(action))).flatten() {
        Some(packed) => Word::of(packed),
        None => stand_in(model, action, |s| Word {
            kind: s.kind,
            action: s.action,
        }),
    }
}

/// `GetActionTexture` (`0x5a97f0`): a macro's and a set's icon off the VM's own tables, any other
/// slot's resolved by the app at the call, a pet mode's or order's through `global`.
pub(crate) fn texture(
    model: &Model,
    action: u32,
    global: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let w = word(model, action)?;
    match w.kind & 0xf0 {
        ACTION_KIND_MACRO => model
            .macros
            .get(w.action as usize)
            .and_then(|m| m.texture.clone()),
        // `Interface\Icons\` and the set's icon; nothing for a set the list lacks or an empty icon
        // (`0x5ae6b0`).
        ACTION_KIND_EQUIPMENT_SET => model
            .equipment_manager
            .sets
            .iter()
            .find(|s| s.id == w.action)
            .filter(|s| !s.icon.is_empty())
            .map(|s| format!("Interface\\Icons\\{}", s.icon)),
        _ => match read_live(|l| l.action_texture(slot(action))).flatten() {
            Some(Some(super::ActionIcon::Path(path))) => Some(path),
            Some(Some(super::ActionIcon::Global(key))) => global(key),
            Some(None) => None,
            None => stand_in(model, action, |s| s.texture.clone()).flatten(),
        },
    }
}

/// `GetActionCount` (`0x5a7d10`).
pub(crate) fn count(model: &Model, action: u32) -> u32 {
    read_live(|l| l.action_count(slot(action)))
        .flatten()
        .or_else(|| stand_in(model, action, |s| s.count))
        .unwrap_or(0)
}

/// `IsConsumableAction` (`0x5a9360`).
pub(crate) fn consumable(model: &Model, action: u32) -> bool {
    read_live(|l| l.action_consumable(slot(action)))
        .flatten()
        .or_else(|| stand_in(model, action, |s| s.consumable))
        .unwrap_or(false)
}

/// `IsStackableAction` (`0x5a8470`).
pub(crate) fn stackable(model: &Model, action: u32) -> bool {
    read_live(|l| l.action_stackable(slot(action)))
        .flatten()
        .or_else(|| stand_in(model, action, |s| s.stackable))
        .unwrap_or(false)
}

/// A pet-action slot's spell (`0x5a80e0`), `None` for a mode, an order or an empty action.
pub(crate) fn pet_spell(model: &Model, action: u32) -> Option<u32> {
    match read_live(|l| l.action_pet_spell(slot(action))).flatten() {
        Some(spell) => spell,
        None => stand_in(model, action, |s| s.pet_spell).flatten(),
    }
}

/// The slot write every bar verb shares (`0x5aae80`, `0x5aaa90` for `None`): the app's bar stores
/// it, recomputes its usable bytes and sends it; with no view lent the stand-in takes it, the icon
/// `texture` for the reads after. Whether it was written: a locked bar refuses (`0x5aaec1`,
/// `0x5aaac2`). The caller signals `ACTIONBAR_SLOT_CHANGED`.
pub(crate) fn write(
    model: &mut Model,
    action: u32,
    to: Option<Word>,
    texture: Option<String>,
) -> bool {
    let packed = to.map_or(0, Word::packed);
    if let Some(written) = write_live(|l| l.set_action_word(slot(action), packed)).flatten() {
        return written;
    }
    write_stand_in(model, action, to, texture);
    true
}

#[cfg(any(test, feature = "fixtures"))]
fn write_stand_in(model: &mut Model, action: u32, to: Option<Word>, texture: Option<String>) {
    model.action_sets.push((action, to.map_or(0, Word::packed)));
    match to {
        Some(w) => {
            model.actions.insert(
                action,
                super::ActionSlot {
                    texture,
                    kind: w.kind,
                    action: w.action,
                    ..Default::default()
                },
            );
        }
        None => {
            model.actions.remove(&action);
        }
    }
}

#[cfg(not(any(test, feature = "fixtures")))]
fn write_stand_in(_: &mut Model, _: u32, _: Option<Word>, _: Option<String>) {}

/// A bar verb's slot signals inside its call, one for each slot written.
pub(crate) fn signal_all(lua: &mlua::Lua, changed: Vec<u32>) {
    for action in changed {
        signal(lua, action);
    }
}

/// `ACTIONBAR_SLOT_CHANGED` for the 1-based `action`, inside the call that changed it
/// (`0x5aa458`).
pub(crate) fn signal(lua: &mlua::Lua, action: u32) {
    super::tick::fire_event_into(
        lua,
        "ACTIONBAR_SLOT_CHANGED",
        vec![super::ScriptValue::Int(i64::from(action))],
    );
}
