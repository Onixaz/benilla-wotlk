//! 3.3.5a's frame attributes: `SetAttribute(name, value)` and `GetAttribute(name)` on every frame.
//!
//! The store is a hash table at frame `+0x1cc` keyed through `SStrHash` (`0x76f640`), so a name is
//! matched case-insensitively (`0x55f4d0`); the value is a Lua registry reference. A store calls
//! the notifier `0x48ec10`, which, when the frame has an `OnAttributeChanged` handler, copies the
//! caller's name, lowercases it (`0x76f6e0`) and calls the handler with `(self, name, value)`. No
//! 1.12.1 method table binds either verb, so they exist only where the target has them.

use mlua::{Lua, Table, Value};

use super::{decode_id, frame_handle_of};
use crate::script::{event, Model};

/// Registry table of per-frame attribute tables, `[frame id][lowercased name] = value`.
const REG_ATTRIBUTES: &str = "__benilla_attributes";

/// Add `SetAttribute` and `GetAttribute` to the shared frame method table `m`, on a target that
/// has them.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    lua.set_named_registry_value(REG_ATTRIBUTES, lua.create_table()?)?;
    // `0x49f610`: a non-string name raises its usage line; any value, nil included, is stored.
    m.set(
        "SetAttribute",
        lua.create_function(|lua, (this, name, value): (Table, Value, Value)| {
            let Some(name) = attribute_name(lua, &name)? else {
                return Err(usage(&this, "SetAttribute(\"name\", value)"));
            };
            set_attribute(lua, &this, &name, value)
        })?,
    )?;
    // `0x49f2d0`: one name answers its value, nil when unset. With exactly three arguments and a
    // string name it is the modified-attribute lookup the secure templates use
    // (`SecureTemplates.lua:160`): `prefix..name..suffix`, `"*"..name..suffix`,
    // `prefix..name.."*"`, `"*"..name.."*"`, then `name`, the first set one answering. A
    // non-string prefix or suffix reads as empty. Deviation: no candidate is cut to the
    // reference's 255-byte buffer.
    m.set(
        "GetAttribute",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let args: Vec<Value> = args.into_iter().collect();
            let id = decode_id(&this)?;
            let store = attributes_of(lua, id, false)?;
            let get = |key: &str| match &store {
                Some(t) => t.get::<Value>(key.to_lowercase()),
                None => Ok(Value::Nil),
            };
            if args.len() == 3 && matches!(args[1], Value::String(_)) {
                let text = |v: &Value| match v {
                    Value::String(s) => s.to_string_lossy(),
                    _ => String::new(),
                };
                let (prefix, name, suffix) = (text(&args[0]), text(&args[1]), text(&args[2]));
                for key in [
                    format!("{prefix}{name}{suffix}"),
                    format!("*{name}{suffix}"),
                    format!("{prefix}{name}*"),
                    format!("*{name}*"),
                    name,
                ] {
                    let v = get(&key)?;
                    if !v.is_nil() {
                        return Ok(v);
                    }
                }
                return Ok(Value::Nil);
            }
            let first = args.into_iter().next().unwrap_or(Value::Nil);
            let Some(name) = attribute_name(lua, &first)? else {
                return Err(usage(&this, "GetAttribute(\"name\")"));
            };
            get(&name)
        })?,
    )?;
    Ok(())
}

/// The usage error, `Usage: %s:<call>` with the frame's name or `<unnamed>` (`0x9ece20`,
/// `0x9ece00`, `0x9ea280`).
fn usage(this: &Table, call: &str) -> mlua::Error {
    use mlua::ObjectLike;
    let name = this
        .call_method::<Option<String>>("GetName", ())
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".to_string());
    mlua::Error::runtime(format!("Usage: {name}:{call}"))
}

/// The attribute name a string or number argument gives (`lua_isstring`), else `None`.
fn attribute_name(lua: &Lua, v: &Value) -> mlua::Result<Option<String>> {
    Ok(match v {
        Value::String(_) | Value::Integer(_) | Value::Number(_) => {
            lua.coerce_string(v.clone())?.map(|s| s.to_string_lossy())
        }
        _ => None,
    })
}

/// Frame `id`'s attribute table, created when `create`.
fn attributes_of(lua: &Lua, id: u32, create: bool) -> mlua::Result<Option<Table>> {
    let all: Table = lua.named_registry_value(REG_ATTRIBUTES)?;
    match all.get::<Value>(id)? {
        Value::Table(t) => Ok(Some(t)),
        _ if create => {
            let t = lua.create_table()?;
            all.set(id, t.clone())?;
            Ok(Some(t))
        }
        _ => Ok(None),
    }
}

/// Store `value` under `name` without the notifier, as an XML `<Attribute>` does: 12340's
/// `CSimpleFrame::LoadXML_Attributes` (`0x492f70`) writes the hash table directly, so no
/// `OnAttributeChanged` fires.
pub(crate) fn store_from_xml(
    lua: &Lua,
    this: &Table,
    name: &str,
    value: Value,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let id = lua.app_data_mut::<Model>().expect("model").frame_id(h);
    if let Some(t) = attributes_of(lua, id, true)? {
        t.set(name.to_lowercase(), value)?;
    }
    Ok(())
}

/// Store `value` under `name` and fire `OnAttributeChanged(self, lowercased name, value)`. The
/// handler runs as every engine-fired script does: its error is reported and `SetAttribute`
/// returns as usual, so `ActionButton_OnLoad` (`ActionButton.lua:83-88`) sets each attribute even
/// when the update its first store triggers fails.
fn set_attribute(lua: &Lua, this: &Table, name: &str, value: Value) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let id = lua.app_data_mut::<Model>().expect("model").frame_id(h);
    let lower = name.to_lowercase();
    if let Some(t) = attributes_of(lua, id, true)? {
        t.set(lower.as_str(), value.clone())?;
    }
    let name = Value::String(lua.create_string(&lower)?);
    if let Err(e) = event::fire_widget_handler(lua, id, "OnAttributeChanged", vec![name, value]) {
        lua.app_data_mut::<Model>()
            .expect("model")
            .record_script_error(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    fn ui(src: &str) -> UiScript {
        let ui = UiScript::new().unwrap();
        ui.run(src).unwrap();
        ui
    }

    #[test]
    fn a_name_matches_case_insensitively_and_nil_clears() {
        let ui = ui("local f = CreateFrame('Frame')
             f:SetAttribute('chatType', 'SAY')
             A = f:GetAttribute('CHATTYPE')
             f:SetAttribute('ChatType', nil)
             B = f:GetAttribute('chatType') == nil
             C = CreateFrame('Frame'):GetAttribute('chatType') == nil");
        assert_eq!(ui.eval::<String>("return A").unwrap(), "SAY");
        assert!(ui.eval::<bool>("return B").unwrap());
        assert!(
            ui.eval::<bool>("return C").unwrap(),
            "each frame has its own store"
        );
    }

    #[test]
    fn the_three_name_form_tries_the_wildcards_in_the_references_order() {
        let ui = ui("F = CreateFrame('Frame')
             F:SetAttribute('type', 'plain')");
        let get = |src: &str| ui.eval::<Option<String>>(src).unwrap();
        let modified = "return F:GetAttribute('shift-', 'type', '1')";
        assert_eq!(get(modified).as_deref(), Some("plain"));
        ui.run("F:SetAttribute('*type*', 'any')").unwrap();
        assert_eq!(get(modified).as_deref(), Some("any"));
        ui.run("F:SetAttribute('shift-type*', 'shift-any')")
            .unwrap();
        assert_eq!(get(modified).as_deref(), Some("shift-any"));
        ui.run("F:SetAttribute('*type1', 'any-1')").unwrap();
        assert_eq!(get(modified).as_deref(), Some("any-1"));
        ui.run("F:SetAttribute('shift-type1', 'exact')").unwrap();
        assert_eq!(get(modified).as_deref(), Some("exact"));
        assert_eq!(get("return F:GetAttribute(nil, 'unit', nil)"), None);
    }

    #[test]
    fn a_store_fires_on_attribute_changed_with_the_name_lowercased() {
        let ui = ui("local f = CreateFrame('Frame')
             f:SetScript('OnAttributeChanged', function(self, name, value)
                 SELF = self == f; NAME = name; VALUE = value
             end)
             f:SetAttribute('Panel-Show', 7)");
        assert!(ui.eval::<bool>("return SELF").unwrap());
        assert_eq!(ui.eval::<String>("return NAME").unwrap(), "panel-show");
        assert_eq!(ui.eval::<f64>("return VALUE").unwrap(), 7.0);
    }

    /// A failing handler is reported, and the caller runs on to its next statement.
    #[test]
    fn a_handler_error_does_not_abort_the_store_that_fired_it() {
        let ui = ui("local f = CreateFrame('Frame')
             f:SetScript('OnAttributeChanged', function() error('boom') end)
             f:SetAttribute('a', 1)
             f:SetAttribute('b', 2)
             B = f:GetAttribute('b')");
        assert_eq!(ui.eval::<i64>("return B").unwrap(), 2);
        assert_eq!(ui.errors().len(), 2, "{:?}", ui.errors());
    }

    #[test]
    fn the_extra_script_kinds_take_a_handler() {
        let ui = ui("local e = CreateFrame('EditBox')
             e:SetScript('OnInputLanguageChanged', function() end)
             R = e:GetScript('OnInputLanguageChanged') ~= nil");
        assert!(ui.eval::<bool>("return R").unwrap());
    }
}
