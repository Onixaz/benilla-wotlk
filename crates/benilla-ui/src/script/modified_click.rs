//! The modified-click table behind `IsModifiedClick`, `GetModifiedClick` and `SetModifiedClick`:
//! each action a `Bindings.xml` `<ModifiedClick>` declares, bound to a modifier set and an
//! optional mouse button (`SELFCAST` is `ALT`, `CHATLINK` is `SHIFT-BUTTON1`). The stock action
//! button asks `IsModifiedClick("SELFCAST")` on every press (`SecureTemplates.lua:207`).
//!
//! Deviation: benilla reads one flag per modifier, so a binding naming a side (`LSHIFT`) holds on
//! either key, and `SetModifiedClick` lasts the session, where the reference saves the table
//! with the key bindings.

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::Model;

/// One binding parsed as `0x55d860` reads it: the modifier prefixes, each `-`-joined, then the
/// rest, a mouse button such as `BUTTON1`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ClickBinding {
    shift: bool,
    ctrl: bool,
    alt: bool,
    button: Option<String>,
}

impl ClickBinding {
    fn parse(text: &str) -> Self {
        let mut b = Self::default();
        let mut rest = text;
        loop {
            let upper = rest.to_ascii_uppercase();
            let (len, which) = [
                ("LSHIFT", 0),
                ("RSHIFT", 0),
                ("SHIFT", 0),
                ("LCTRL", 1),
                ("RCTRL", 1),
                ("CTRL", 1),
                ("LALT", 2),
                ("RALT", 2),
                ("ALT", 2),
            ]
            .iter()
            .find(|(p, _)| upper.starts_with(p))
            .map_or((0, 3), |&(p, w)| (p.len(), w));
            if len == 0 {
                break;
            }
            match which {
                0 => b.shift = true,
                1 => b.ctrl = true,
                _ => b.alt = true,
            }
            rest = &rest[len..];
            rest = rest.strip_prefix('-').unwrap_or(rest);
        }
        if !rest.is_empty() && !rest.eq_ignore_ascii_case("NONE") {
            b.button = Some(rest.to_string());
        }
        b
    }

    fn has_modifiers(&self) -> bool {
        self.shift || self.ctrl || self.alt
    }
}

/// The click's button as a binding names it (`LeftButton` is `BUTTON1`).
fn binding_button(button: &str) -> &str {
    match button {
        "LeftButton" => "BUTTON1",
        "RightButton" => "BUTTON2",
        "MiddleButton" => "BUTTON3",
        "Button4" => "BUTTON4",
        "Button5" => "BUTTON5",
        other => other,
    }
}

/// `0x55d6b0`: any one of the binding's modifiers held, and its button the click's while a click
/// is running (`[0xb499a8]+0x1234`); with no modifier, only a button binding can hold.
fn binding_holds(b: &ClickBinding, model: &Model) -> bool {
    let (shift, ctrl, alt) = model.modifiers;
    let modifier = (b.shift && shift) || (b.ctrl && ctrl) || (b.alt && alt);
    if b.has_modifiers() && !modifier {
        return false;
    }
    match (&b.button, &model.click_button) {
        (None, _) if !b.has_modifiers() => false,
        (Some(want), Some(now)) => want.eq_ignore_ascii_case(binding_button(now)),
        _ => true,
    }
}

impl super::UiScript {
    /// `IsModifiedClick(action)` asked from the engine, as `IsAutoLooting 0x513700` asks it.
    pub fn is_modified_click(&self, action: &str) -> bool {
        is_modified_click(&self.model_ref(), Some(action))
    }

    /// Register the core's `<ModifiedClick>` rows, each at its default.
    pub fn register_modified_clicks(&self, clicks: &[crate::bindings_xml::ModifiedClick]) {
        let mut model = self.model_mut();
        for c in clicks {
            set_click(&mut model, &c.action, &c.default);
        }
    }
}

fn set_click(model: &mut Model, action: &str, binding: &str) {
    match model
        .modified_clicks
        .iter_mut()
        .find(|(a, _)| a.eq_ignore_ascii_case(action))
    {
        Some(row) => row.1 = binding.to_string(),
        None => model
            .modified_clicks
            .push((action.to_string(), binding.to_string())),
    }
}

/// The action's binding, matched case-insensitively.
fn click_of<'a>(model: &'a Model, action: &str) -> Option<&'a str> {
    model
        .modified_clicks
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(action))
        .map(|(_, b)| b.as_str())
}

/// `0x55f940`: with no action, any modifier held; an action that is itself a modifier set
/// (`"SHIFT"`) needs each of its modifiers held; otherwise the action'"'"'s binding holds by
/// `0x55d6b0`, and an action no `Bindings.xml` declared never holds. `[mod:...]` asks it too.
pub(super) fn is_modified_click(model: &Model, action: Option<&str>) -> bool {
    let (shift, ctrl, alt) = model.modifiers;
    match action {
        None | Some("") => shift || ctrl || alt,
        Some(text) => {
            let set = ClickBinding::parse(text);
            if set.has_modifiers() {
                (!set.shift || shift) && (!set.ctrl || ctrl) && (!set.alt || alt)
            } else {
                click_of(model, text).is_some_and(|b| binding_holds(&ClickBinding::parse(b), model))
            }
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // IsModifiedClick([action]) (`0x55fcc0` → `0x55f940`): 1 or nil. With no argument, any
    // modifier held (`0x55fa60`). An argument that is itself a modifier set (`"SHIFT"`) needs each
    // of its modifiers held (`0x55f977`-`0x55fa24`); otherwise it names an action, whose binding
    // holds by `0x55d6b0`, and an action no `Bindings.xml` declared never holds (`0x55fa49`).
    g.set(
        "IsModifiedClick",
        lua.create_function(|lua, action: Option<Value>| {
            let action = action.and_then(|v| super::binding_abi::optional_string(lua, &v));
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(is_modified_click(&model, action.as_deref())))
        })?,
    )?;

    // GetModifiedClick("action") (`0x55fc20`): the action's binding string, nil for an action no
    // `Bindings.xml` declared (`0x55fc90`); a non-string raises.
    g.set(
        "GetModifiedClick",
        lua.create_function(|lua, action: Value| {
            let action = super::binding_abi::string_arg(
                lua,
                action,
                r#"Usage: GetModifiedClick("action")"#,
            )?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            match click_of(&model, &action) {
                Some(b) => Ok(Value::String(lua.create_string(b)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // SetModifiedClick("action", "binding") (`0x55fb90`): rebinds an action, the Controls
    // panel's self-cast key.
    g.set(
        "SetModifiedClick",
        lua.create_function(|lua, (action, binding): (Value, Value)| {
            let usage = r#"Usage: SetModifiedClick("action", "binding")"#;
            let action = super::binding_abi::string_arg(lua, action, usage)?;
            let binding = super::binding_abi::string_arg(lua, binding, usage)?;
            let mut model = crate::script::model_mut_of(lua);
            set_click(&mut model, &action, &binding);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::bindings_xml::ModifiedClick;
    use crate::script::UiScript;

    fn script() -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_modified_clicks(
            &[
                ("SELFCAST", "ALT"),
                ("FOCUSCAST", "NONE"),
                ("CHATLINK", "SHIFT-BUTTON1"),
                ("SOCKETITEM", "SHIFT-BUTTON2"),
            ]
            .map(|(action, default)| ModifiedClick {
                action: action.into(),
                default: default.into(),
            }),
        );
        s
    }

    fn is(s: &UiScript, lua: &str) -> bool {
        s.eval::<Option<i64>>(&format!("return {lua}"))
            .unwrap()
            .is_some()
    }

    #[test]
    fn an_action_holds_on_its_binding() {
        let mut s = script();
        assert!(!is(&s, r#"IsModifiedClick("SELFCAST")"#));
        assert!(!is(&s, "IsModifiedClick()"), "no modifier held");
        s.set_modifiers(false, false, true);
        assert!(is(&s, r#"IsModifiedClick("SELFCAST")"#));
        assert!(is(&s, r#"IsModifiedClick("selfcast")"#), "case folds");
        assert!(is(&s, "IsModifiedClick()"));
        assert!(
            !is(&s, r#"IsModifiedClick("FOCUSCAST")"#),
            "NONE never holds"
        );
        assert!(!is(&s, r#"IsModifiedClick("NOSUCHACTION")"#));
        s.set_modifiers(true, false, false);
        assert!(!is(&s, r#"IsModifiedClick("SELFCAST")"#));
        assert!(
            is(&s, r#"IsModifiedClick("CHATLINK")"#) && is(&s, r#"IsModifiedClick("SOCKETITEM")"#),
            "outside a click the button part passes"
        );
    }

    /// Inside a click the button part must match: a shift-right-click is `SOCKETITEM`'s, not
    /// `CHATLINK`'s. The three click scripts run in order with `(button, down)`.
    #[test]
    fn a_click_runs_pre_on_post_and_its_button_qualifies_the_binding() {
        let mut s = script();
        s.set_modifiers(true, false, false);
        s.run(
            r#"SEEN = {}
               local b = CreateFrame("Button", "ClickProbe", UIParent)
               for _, k in ipairs({"PreClick", "OnClick", "PostClick"}) do
                   b:SetScript(k, function(self, button, down)
                       table.insert(SEEN, k .. ":" .. button .. ":" .. tostring(down))
                   end)
               end
               b:SetScript("OnClick", function(self, button)
                   table.insert(SEEN, "OnClick:" .. button
                       .. ":" .. tostring(IsModifiedClick("CHATLINK"))
                       .. ":" .. tostring(IsModifiedClick("SOCKETITEM")))
               end)
               b:Click("RightButton")"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return table.concat(SEEN, ' ')").unwrap(),
            "PreClick:RightButton:false OnClick:RightButton:nil:1 PostClick:RightButton:false"
        );
    }

    #[test]
    fn a_modifier_argument_needs_every_modifier() {
        let mut s = script();
        s.set_modifiers(true, false, false);
        assert!(is(&s, r#"IsModifiedClick("SHIFT")"#));
        assert!(!is(&s, r#"IsModifiedClick("CTRL-SHIFT")"#));
        s.set_modifiers(true, true, false);
        assert!(is(&s, r#"IsModifiedClick("CTRL-SHIFT")"#));
    }

    #[test]
    fn set_modified_click_rebinds_and_get_reads_back() {
        let mut s = script();
        assert_eq!(
            s.eval::<String>(r#"return GetModifiedClick("SELFCAST")"#)
                .unwrap(),
            "ALT"
        );
        assert!(s
            .eval::<bool>(r#"return GetModifiedClick("NOSUCHACTION") == nil"#)
            .unwrap());
        s.run(r#"SetModifiedClick("SELFCAST", "CTRL")"#).unwrap();
        s.set_modifiers(false, true, false);
        assert!(is(&s, r#"IsModifiedClick("SELFCAST")"#));
        let err = s.run("GetModifiedClick()").unwrap_err().to_string();
        assert!(
            err.contains(r#"Usage: GetModifiedClick("action")"#),
            "{err}"
        );
    }
}
