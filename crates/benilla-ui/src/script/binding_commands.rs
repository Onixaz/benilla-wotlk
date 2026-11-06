//! The override bindings and the commands a binding carries past the `Bindings.xml` names: `SPELL`,
//! `ITEM`, `MACRO` and `CLICK` (`0x563150`), and the macro runner they share with `RunMacro`.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{bool_or_default, flag, optional_string, string_arg};
use super::calls::{CastAt, ScriptCall};
use super::item_search::{find_item, ItemQuery};
use super::model::Model;

/// The longest line and body the runner keeps (`0x564db0`, `0x566400`).
const MACRO_LINE: usize = 1023;
const MACRO_BODY: usize = 1023;

/// One key's override entries by priority, newest first (`0x5603d0`).
#[derive(Debug, Clone, Default)]
struct OverrideKey {
    lists: [Vec<(u32, String)>; 2],
}

impl OverrideKey {
    /// The owner's entry goes; a command files a new one at the head of its priority's list.
    fn set(&mut self, owner: u32, priority: bool, command: Option<&str>) {
        for list in &mut self.lists {
            list.retain(|(o, _)| *o != owner);
        }
        if let Some(command) = command.filter(|c| !c.is_empty()) {
            self.lists[usize::from(priority)].insert(0, (owner, command.to_string()));
        }
    }

    /// The newest priority entry, else the newest plain one (`0x55e5e0`).
    fn active(&self) -> Option<&str> {
        self.lists
            .iter()
            .rev()
            .find_map(|l| l.first())
            .map(|(_, c)| c.as_str())
    }
}

/// The override table; never saved, and no `UPDATE_BINDINGS` fires for it.
#[derive(Debug, Clone, Default)]
pub(crate) struct OverrideBindings {
    keys: HashMap<String, OverrideKey>,
    generation: u64,
}

impl OverrideBindings {
    /// The command an override puts on `key`.
    pub(crate) fn active(&self, key: &str) -> Option<&str> {
        self.keys.get(key)?.active()
    }
}

impl super::UiScript {
    /// Every key an override holds, with its active command.
    pub fn override_bindings(&self) -> Vec<(String, String)> {
        let model = self.model_ref();
        let keys = model.override_bindings.keys.iter();
        keys.filter_map(|(k, o)| Some((k.clone(), o.active()?.to_string())))
            .collect()
    }

    /// Moves with every override write.
    pub fn override_generation(&self) -> u64 {
        self.model_ref().override_bindings.generation
    }

    /// Run a macro body as `UseAction` does, under the click's button.
    pub fn run_macro_body(&self, body: &str) {
        let button = self.model_ref().click_button.clone();
        run_macro(&self.lua, body, button);
    }
}

/// The macro runner (`0x564db0`): each line through `EXECUTE_CHAT_LINE`, a comment or `#` line
/// skipped, until `StopMacro`; the outer macro's state comes back after.
fn run_macro(lua: &Lua, body: &str, button: Option<String>) {
    let saved = {
        let mut model = crate::script::model_mut_of(lua);
        let saved = (model.macro_running, model.click_button.take());
        model.macro_running = true;
        model.click_button = button;
        saved
    };
    for line in body.split(['\r', '\n']) {
        if !crate::script::model_mut_of(lua).macro_running {
            break;
        }
        let line = super::chat_record::clipped(line, MACRO_LINE + 1);
        if !line.is_empty() && !line.starts_with(['-', '#']) {
            super::tick::fire_event_into(
                lua,
                "EXECUTE_CHAT_LINE",
                vec![super::ScriptValue::Str(line)],
            );
        }
    }
    let mut model = crate::script::model_mut_of(lua);
    model.macro_running = saved.0;
    model.click_button = saved.1;
}

/// A macro by its 1-based id or its name (`0x566df0`).
fn macro_body(model: &Model, id: &Value, name: Option<String>) -> Option<String> {
    let index = match (id, name) {
        (Value::Integer(i), _) => usize::try_from(*i).ok()?,
        (Value::Number(n), _) => *n as usize,
        (_, Some(name)) => model.macros.index_by_name(&name),
        _ => 0,
    };
    model.macros.get(index).map(|m| m.body.clone())
}

/// The frame a binding clicks (`0x562990`): the global's frame, carrying that exact name.
fn click_frame(lua: &Lua, name: &str) -> Option<Table> {
    let Value::Table(t) = lua.globals().raw_get::<Value>(name).ok()? else {
        return None;
    };
    let handle = super::object::frame_handle_of(lua, &t).ok()?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let frame = model.arena.frame(handle)?;
    (frame.name.as_deref() == Some(name)).then_some(t)
}

/// A binding's cast target: the player or the focus under the modified clicks (`0x563150`).
fn modified_target(model: &Model) -> CastAt {
    if super::modified_click::is_modified_click(model, Some("SELFCAST")) {
        CastAt::Player
    } else if super::modified_click::is_modified_click(model, Some("FOCUSCAST")) {
        CastAt::Unit("focus".to_string())
    } else {
        CastAt::Selection
    }
}

/// Run a `SPELL `, `ITEM `, `MACRO ` or `CLICK ` command, `None` for any other: the first three on
/// the release, a click on both halves (`0x563150`).
pub(super) fn run_special(lua: &Lua, command: &str, down: bool) -> Option<mlua::Result<bool>> {
    let (kind, argument) = command.split_once(' ')?;
    match kind.to_ascii_uppercase().as_str() {
        "SPELL" => {
            if !down {
                let mut model = crate::script::model_mut_of(lua);
                let spell = super::spellbook::resolve_spell_by_name(&model.spellbook, argument);
                if let Some(spell_id) = spell.map(|s| s.spell_id) {
                    let at = modified_target(&model);
                    model.script_calls.push(ScriptCall::CastSpell {
                        spell_id,
                        at,
                        via_book: true,
                    });
                }
            }
        }
        "ITEM" => {
            if !down {
                let name = Value::String(lua.create_string(argument).ok()?);
                let query = ItemQuery::of(lua, &name, ItemQuery::Prefix).ok()??;
                let mut model = crate::script::model_mut_of(lua);
                if let Some(held) = find_item(&model, &query, 0) {
                    let at = modified_target(&model);
                    model.script_calls.push(ScriptCall::UseItemAt {
                        bag: held.bag,
                        slot: held.slot,
                        at,
                    });
                }
            }
        }
        "MACRO" => {
            if !down {
                let body = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    let digits =
                        !argument.is_empty() && argument.bytes().all(|b| b.is_ascii_digit());
                    if digits {
                        let index = argument.parse().unwrap_or(0);
                        macro_body(&model, &Value::Integer(index), None)
                    } else {
                        macro_body(&model, &Value::Nil, Some(argument.to_string()))
                    }
                };
                if let Some(body) = body {
                    run_macro(lua, &body, Some("LeftButton".to_string()));
                }
            }
        }
        "CLICK" => {
            let (name, button) = argument.split_once(':')?;
            let Some(frame) = click_frame(lua, name) else {
                return Some(Ok(false));
            };
            if let Err(e) = super::button::binding_click(lua, &frame, button, down) {
                return Some(Err(e));
            }
        }
        _ => return None,
    }
    Some(Ok(true))
}

/// `T[0]`'s frame id, the owner an override is filed under.
fn owner_id(lua: &Lua, owner: &Value, usage: &'static str) -> mlua::Result<u32> {
    let handle = match owner {
        Value::Table(t) => super::object::frame_handle_of(lua, t).ok(),
        _ => None,
    };
    let Some(handle) = handle else {
        return Err(mlua::Error::runtime(usage));
    };
    Ok(crate::script::model_mut_of(lua).frame_id(handle))
}

/// Write one override; the key folds as `SetBinding`'s does.
fn file_override(lua: &Lua, owner: u32, priority: bool, key: &str, command: Option<&str>) {
    let mut model = crate::script::model_mut_of(lua);
    let table = &mut model.override_bindings;
    table
        .keys
        .entry(key.to_ascii_uppercase())
        .or_default()
        .set(owner, priority, command);
    table.generation = table.generation.wrapping_add(1);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SetOverrideBinding(owner, isPriority, key [, command]) (`0x563a90`).
    g.set(
        "SetOverrideBinding",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str =
                "Usage: SetOverrideBinding(owner, isPriority, \"KEY\"[, \"COMMAND\"])";
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let owner = owner_id(lua, &arg(0), USAGE)?;
            let Some(key) = optional_string(lua, &arg(2)) else {
                return Err(mlua::Error::runtime(USAGE));
            };
            let command = optional_string(lua, &arg(3));
            let priority = bool_or_default(args.get(1), false);
            file_override(lua, owner, priority, &key, command.as_deref());
            Ok(())
        })?,
    )?;

    // SetOverrideBindingSpell, Item and Macro (`0x563b80`, `0x563cb0`, `0x563de0`).
    for (verb, usage, prefix) in [
        (
            "SetOverrideBindingSpell",
            "Usage: SetOverrideBindingSpell(owner, isPriority, \"KEY\", \"spellname\")",
            "SPELL",
        ),
        (
            "SetOverrideBindingItem",
            "Usage: SetOverrideBindingItem(owner, isPriority, \"KEY\", \"itemname\")",
            "ITEM",
        ),
        (
            "SetOverrideBindingMacro",
            "Usage: SetOverrideBindingMacro(owner, isPriority, \"KEY\", \"macroname\"|macroid)",
            "MACRO",
        ),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, args: MultiValue| {
                let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
                let owner = owner_id(lua, &arg(0), usage)?;
                let key = optional_string(lua, &arg(2));
                let target = optional_string(lua, &arg(3));
                let (Some(key), Some(target)) = (key, target) else {
                    return Err(mlua::Error::runtime(usage));
                };
                let priority = bool_or_default(args.get(1), false);
                let command = format!("{prefix} {target}");
                file_override(lua, owner, priority, &key, Some(&command));
                Ok(())
            })?,
        )?;
    }

    // SetOverrideBindingClick(owner, isPriority, key, button [, mouseButton]) (`0x563f10`).
    g.set(
        "SetOverrideBindingClick",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: SetOverrideBindingClick(owner, isPriority, \"KEY\", \"buttonName\"[, \"mouseButton\"])";
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let owner = owner_id(lua, &arg(0), USAGE)?;
            let key = optional_string(lua, &arg(2));
            let button = optional_string(lua, &arg(3));
            let (Some(key), Some(button)) = (key, button) else {
                return Err(mlua::Error::runtime(USAGE));
            };
            let mouse = optional_string(lua, &arg(4)).unwrap_or_else(|| "LeftButton".to_string());
            let priority = bool_or_default(args.get(1), false);
            let command = format!("CLICK {button}:{mouse}");
            file_override(lua, owner, priority, &key, Some(&command));
            Ok(())
        })?,
    )?;

    // ClearOverrideBindings(owner) (`0x560560`).
    g.set(
        "ClearOverrideBindings",
        lua.create_function(|lua, owner: Value| {
            let owner = owner_id(lua, &owner, "Usage: ClearOverrideBindings(owner)")?;
            let mut model = crate::script::model_mut_of(lua);
            let table = &mut model.override_bindings;
            for key in table.keys.values_mut() {
                key.set(owner, false, None);
            }
            table.generation = table.generation.wrapping_add(1);
            Ok(())
        })?,
    )?;

    // GetClickFrame(name) (`0x564130`): the frame a `CLICK` command would press, else nil.
    g.set(
        "GetClickFrame",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: GetClickFrame(\"name\")")?;
            Ok(click_frame(lua, &name))
        })?,
    )?;

    // IsModifierKeyDown() (`0x514430`): any of shift, control and alt, 1 or nil.
    g.set(
        "IsModifierKeyDown",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (shift, ctrl, alt) = model.modifiers;
            Ok(flag(shift || ctrl || alt))
        })?,
    )?;

    // RunMacro(macro [, button]) (`0x566e80`): by 1-based id or name.
    g.set(
        "RunMacro",
        lua.create_function(|lua, (id, button): (Value, Value)| {
            let name = optional_string(lua, &id);
            let body = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                macro_body(&model, &id, name)
            };
            if let Some(body) = body {
                run_macro(lua, &body, optional_string(lua, &button));
            }
            Ok(())
        })?,
    )?;

    // RunMacroText(text [, button]) (`0x566400`).
    g.set(
        "RunMacroText",
        lua.create_function(|lua, (text, button): (Value, Value)| {
            let text = string_arg(lua, text, "Usage: RunMacroText(\"text\" [, \"button\"])")?;
            let text = super::chat_record::clipped(&text, MACRO_BODY + 1);
            run_macro(lua, &text, optional_string(lua, &button));
            Ok(())
        })?,
    )?;

    // StopMacro() (`0x564b90`): the running macro's next line does not come.
    g.set(
        "StopMacro",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.macro_running = false;
            model.click_button = None;
            Ok(())
        })?,
    )?;

    // SpellTargetItem(item) (`0x8007e0`): only with a spell waiting on an item, where the bag walk
    // picks what a click on the item would.
    g.set(
        "SpellTargetItem",
        lua.create_function(|lua, item: Value| {
            let mut model = crate::script::model_mut_of(lua);
            if !model.item_pick_armed {
                return Ok(());
            }
            let query = match item {
                Value::Integer(_) | Value::Number(_) | Value::String(_) => {
                    ItemQuery::of(lua, &item, ItemQuery::Prefix)?
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "Usage: SpellTargetItem(itemID|\"name\"|\"itemlink\")",
                    ))
                }
            };
            if let Some(held) = query.and_then(|q| find_item(&model, &q, 0)) {
                model.item_picks.push((held.bag, held.slot));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// `0x5603d0`, `0x55e5e0`: a priority entry beats a plain one, the newest of a list wins, and
    /// an owner holds one entry on a key.
    #[test]
    fn the_newest_priority_override_wins() {
        let mut k = OverrideKey::default();
        k.set(1, false, Some("A"));
        k.set(2, false, Some("B"));
        assert_eq!(k.active(), Some("B"));
        k.set(1, true, Some("C"));
        assert_eq!(k.active(), Some("C"));
        assert_eq!(k.lists[0].len(), 1, "the owner's plain entry moved");
        k.set(1, false, None);
        assert_eq!(k.active(), Some("B"));
        k.set(2, false, Some(""));
        assert_eq!(k.active(), None, "an empty command clears");
    }

    /// The command forms each verb files, and the owner check.
    #[test]
    fn the_verbs_file_their_commands() {
        let s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('Frame', 'Owner')
             SetOverrideBindingSpell(f, false, 'shift-q', 'Fireball')
             SetOverrideBindingItem(f, false, 'W', 'Hearthstone')
             SetOverrideBindingMacro(f, false, 'E', 3)
             SetOverrideBindingClick(f, true, 'R', 'MyButton')
             SetOverrideBinding(f, false, 'T', 'JUMP')",
        )
        .unwrap();
        let mut got = s.override_bindings();
        got.sort();
        assert_eq!(
            got,
            [
                ("E", "MACRO 3"),
                ("R", "CLICK MyButton:LeftButton"),
                ("SHIFT-Q", "SPELL Fireball"),
                ("T", "JUMP"),
                ("W", "ITEM Hearthstone"),
            ]
            .map(|(k, c)| (k.to_string(), c.to_string()))
        );
        s.run("ClearOverrideBindings(Owner)").unwrap();
        assert!(s.override_bindings().is_empty());
        assert!(s.run("SetOverrideBinding({}, false, 'T')").is_err());
    }

    /// `0x564db0`: comment and `#` lines are skipped, a line keeps its spaces, and `StopMacro` ends
    /// the run.
    #[test]
    fn the_runner_skips_comments_and_stops() {
        let s = UiScript::new().unwrap();
        s.run(
            "lines = {}
             local f = CreateFrame('Frame')
             f:RegisterEvent('EXECUTE_CHAT_LINE')
             f:SetScript('OnEvent', function(_, _, line)
                 lines[#lines + 1] = line
                 if line == 'stop' then StopMacro() end
             end)
             RunMacroText('#show\\n-- note\\n /say a\\n\\nstop\\nnever')",
        )
        .unwrap();
        let joined: String = s.eval("return table.concat(lines, '|')").unwrap();
        assert_eq!(joined, " /say a|stop");
        assert!(s.run("RunMacroText()").is_err());
    }
}
