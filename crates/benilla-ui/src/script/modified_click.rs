//! The modified-click table behind `IsModifiedClick`, `GetModifiedClick`, `SetModifiedClick`,
//! `GetNumModifiedClickActions` and `GetModifiedClickAction`: each action a `Bindings.xml`
//! `<ModifiedClick>` declares, bound to a modifier set and an optional mouse button (`SELFCAST`
//! is `ALT`, `CHATLINK` is `SHIFT-BUTTON1`). The stock action button asks
//! `IsModifiedClick("SELFCAST")` on every press (`SecureTemplates.lua:207`), and the stock
//! `CAMERAORSELECTORMOVE` release asks `STICKYCAMERA`. Like the reference's `m_data[4]`, a row
//! keeps a binding per set and rides the key-binding sets: `LoadBindings`/`SaveBindings` copy it
//! with the keys (`0x562b80`), and a saved set holds the rows set from Lua (`0x55f1b0`).
//!
//! Deviation: benilla reads one flag per modifier, so a binding naming a side (`LSHIFT`) holds on
//! either key; the side is kept and read back.

use mlua::{Lua, Value};

use super::binding_abi::flag;
use super::Model;

/// One binding as `MODIFIEDCLICK::SetBinding` (`0x55e180`) stores it: `AddModifiers`' side bits
/// (`0x55d860`: LSHIFT 1, RSHIFT 2, LCTRL 4, RCTRL 8, LALT 0x10, RALT 0x20) and a mouse button by
/// its binding index (`BUTTON1` is the left, `IndexToMouseButton` `0x5eec20`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClickBinding {
    modifiers: u8,
    button: Option<u8>,
}

/// `AddModifiers` (`0x55d860`): the modifier prefixes, each optionally `-`-joined, and the rest.
fn add_modifiers(text: &str) -> (u8, &str) {
    const PREFIXES: [(&str, u8); 9] = [
        ("LSHIFT", 0x1),
        ("RSHIFT", 0x2),
        ("SHIFT", 0x3),
        ("LCTRL", 0x4),
        ("RCTRL", 0x8),
        ("CTRL", 0xc),
        ("LALT", 0x10),
        ("RALT", 0x20),
        ("ALT", 0x30),
    ];
    let (mut m, mut rest) = (0, text);
    while let Some(&(p, bits)) = PREFIXES.iter().find(|(p, _)| {
        rest.as_bytes()
            .get(..p.len())
            .is_some_and(|b| b.eq_ignore_ascii_case(p.as_bytes()))
    }) {
        m |= bits;
        rest = &rest[p.len()..];
        rest = rest.strip_prefix('-').unwrap_or(rest);
    }
    (m, rest)
}

/// A mouse button's binding index: `LeftButton` 1, `RightButton` 2, `MiddleButton` 3, `ButtonN` N.
fn button_index(name: &str) -> Option<u8> {
    crate::widget::mouse_button_bit(name).map(|bit| match bit {
        0 => 1,
        1 => 3,
        2 => 2,
        b => b as u8 + 1,
    })
}

impl ClickBinding {
    /// `0x55e180`'s parse: empty, nil or `NONE` is the empty binding; otherwise the modifiers,
    /// then `BUTTON` and one digit (`0x55e1e6` steps one character), and nothing after. `None`
    /// for text it refuses.
    fn parse(text: Option<&str>) -> Option<Self> {
        let Some(text) = text.filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("NONE")) else {
            return Some(Self::default());
        };
        let (modifiers, rest) = add_modifiers(text);
        let mut button = None;
        let mut rest = rest;
        if rest
            .as_bytes()
            .get(..6)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"BUTTON"))
        {
            let digit = *rest.as_bytes().get(6).filter(|c| c.is_ascii_digit())?;
            button = (digit != b'0').then_some(digit - b'0');
            rest = &rest[7..];
        }
        rest.is_empty().then_some(Self { modifiers, button })
    }

    /// `MODIFIEDCLICK::GetBinding` (`0x55e230`): `AddMetaPrefix`'s ALT, CTRL then SHIFT parts
    /// (`0x55d990`), then `BUTTON<n>`, or `NONE` for neither.
    fn canonical(&self) -> String {
        let m = self.modifiers;
        if m == 0 && self.button.is_none() {
            return "NONE".into();
        }
        let mut out = String::new();
        for (both, left, right, name) in [
            (0x30, 0x10, 0x20, "ALT"),
            (0xc, 0x4, 0x8, "CTRL"),
            (0x3, 0x1, 0x2, "SHIFT"),
        ] {
            if m & both == both {
                out.push_str(name);
            } else if m & left != 0 {
                out.push('L');
                out.push_str(name);
            } else if m & right != 0 {
                out.push('R');
                out.push_str(name);
            } else {
                continue;
            }
            out.push('-');
        }
        match self.button {
            Some(n) => out.push_str(&format!("BUTTON{n}")),
            None => {
                out.pop();
            }
        }
        out
    }

    /// `MODIFIEDCLICK::IsActive` (`0x55d6b0`): any one of its modifiers held, then its button the
    /// running click's (`m_mouseButtonName`, set only around a click or mouse script) or no click
    /// running; with no modifier, only a button binding can hold.
    fn holds(&self, model: &Model) -> bool {
        let (shift, ctrl, alt) = model.modifiers;
        let m = self.modifiers;
        if m != 0 {
            let held = (m & 0x3 != 0 && shift) || (m & 0xc != 0 && ctrl) || (m & 0x30 != 0 && alt);
            if !held {
                return false;
            }
        }
        match (self.button, model.click_button.as_deref()) {
            (None, _) => m != 0,
            (Some(_), None) => true,
            (Some(want), Some(now)) => button_index(now) == Some(want),
        }
    }
}

/// One declared action: its default (set 0) and the live binding (set 3), with the reference's
/// flag bit 0 inverted: `explicit` once Lua or a saved set bound it (`0x55e200`/`0x55e20a`).
#[derive(Debug)]
struct Row {
    action: String,
    default: ClickBinding,
    live: ClickBinding,
    explicit: bool,
}

/// The table, in declaration order (`m_fulllist`), and the explicitly bound rows of stored sets
/// 1 (account) and 2 (character); lives in [`super::keybind::KeybindState`].
#[derive(Debug, Default)]
pub(crate) struct ModifiedClicks {
    rows: Vec<Row>,
    stored: [Option<Vec<(String, ClickBinding)>>; 2],
}

impl ModifiedClicks {
    fn row(&self, action: &str) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.action.eq_ignore_ascii_case(action))
    }

    /// The stored set `LoadBindings(set)` copies, with the key sets' fallback.
    fn chosen(&self, set: u32) -> Option<&Vec<(String, ClickBinding)>> {
        match set {
            1 => self.stored[0].as_ref(),
            2 => self.stored[1].as_ref().or(self.stored[0].as_ref()),
            _ => None,
        }
    }

    /// `LoadModifiedClick` (`0x562dd0`): a redefinition is skipped, so the first file keeps the
    /// action; a refused default leaves it unbound. A set already loaded binds it at once, since
    /// here the sets are read before the interface files.
    fn register(&mut self, action: &str, default: &str, current_set: u32) {
        if action.is_empty() || self.row(action).is_some() {
            return;
        }
        let default = ClickBinding::parse(Some(default)).unwrap_or_default();
        let stored = self
            .chosen(current_set)
            .and_then(|l| l.iter().find(|(a, _)| a.eq_ignore_ascii_case(action)))
            .map(|&(_, b)| b);
        self.rows.push(Row {
            action: action.to_string(),
            default,
            live: stored.unwrap_or(default),
            explicit: stored.is_some(),
        });
    }

    /// `CopyBindings(set, CURRENT)` (`0x562b80`): every row back to its default, then the set's.
    pub(super) fn load(&mut self, set: u32) {
        let chosen = self.chosen(set).cloned().unwrap_or_default();
        for r in &mut self.rows {
            match chosen
                .iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(&r.action))
            {
                Some(&(_, b)) => (r.live, r.explicit) = (b, true),
                None => (r.live, r.explicit) = (r.default, false),
            }
        }
    }

    /// `SaveBindings(which)`: the live explicit rows become set `which`; the account set drops the
    /// character set, as the key sets do.
    pub(super) fn save(&mut self, which: u32) {
        if which != 1 && which != 2 {
            return;
        }
        let rows = self
            .rows
            .iter()
            .filter(|r| r.explicit)
            .map(|r| (r.action.clone(), r.live))
            .collect();
        self.stored[(which - 1) as usize] = Some(rows);
        if which == 1 {
            self.stored[1] = None;
        }
    }

    /// Seed stored set 1 or 2 from the saved file, `(action, binding)` pairs; a binding
    /// `SetBinding` refuses is dropped, as the file reader ignores its failure (`0x5641c0`).
    pub(super) fn seed(&mut self, set: u32, entries: Option<Vec<(String, String)>>) {
        let parsed = entries.map(|e| {
            e.into_iter()
                .filter_map(|(a, b)| Some((a, ClickBinding::parse(Some(&b))?)))
                .collect()
        });
        match set {
            1 => self.stored[0] = parsed,
            2 => self.stored[1] = parsed,
            _ => {}
        }
    }

    /// `SetModifiedClick` on the live set: `false` for an undeclared action or a refused binding.
    fn set(&mut self, action: &str, binding: Option<&str>) -> bool {
        let Some(b) = ClickBinding::parse(binding) else {
            return false;
        };
        match self
            .rows
            .iter_mut()
            .find(|r| r.action.eq_ignore_ascii_case(action))
        {
            Some(r) => {
                (r.live, r.explicit) = (b, true);
                true
            }
            None => false,
        }
    }

    /// Stored set 1 or 2's rows as `(action, binding)`, the `modifiedclick` lines its file holds.
    fn stored_set(&self, set: u32) -> Option<Vec<(String, String)>> {
        let list = match set {
            1 => self.stored[0].as_ref(),
            2 => self.stored[1].as_ref(),
            _ => None,
        }?;
        Some(
            list.iter()
                .map(|(a, b)| (a.clone(), b.canonical()))
                .collect(),
        )
    }

    /// The live explicit rows as `(action, binding)`, the lines a save writes (`0x55f1b0`).
    fn overrides(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter(|r| r.explicit)
            .map(|r| (r.action.clone(), r.live.canonical()))
            .collect()
    }
}

impl super::UiScript {
    /// `IsModifiedClick(action)` asked from the engine, as `IsAutoLooting 0x513700` asks it.
    pub fn is_modified_click(&self, action: &str) -> bool {
        is_modified_click(&self.model_ref(), Some(action))
    }

    /// Register the core's `<ModifiedClick>` rows, each at its default.
    pub fn register_modified_clicks(&self, clicks: &[crate::bindings_xml::ModifiedClick]) {
        register(&mut self.model_mut(), clicks);
    }

    /// Seed stored set 1 (account) or 2 (character) of the table from the saved `(action,
    /// binding)` pairs; `None` clears it. The live rows move at [`Self::load_binding_set`].
    pub fn seed_modified_click_set(&mut self, set: u32, entries: Option<Vec<(String, String)>>) {
        self.model_mut().keybinds.clicks.seed(set, entries);
    }

    /// The rows bound from Lua or a saved set, `(action, binding)` in declaration order: what a
    /// save writes.
    pub fn modified_click_overrides(&self) -> Vec<(String, String)> {
        self.model_ref().keybinds.clicks.overrides()
    }

    /// Stored set 1 or 2's explicitly bound rows, `(action, binding)`.
    pub fn stored_modified_click_set(&self, set: u32) -> Option<Vec<(String, String)>> {
        self.model_ref().keybinds.clicks.stored_set(set)
    }
}

/// Register one file's `<ModifiedClick>` rows, core or addon, after the earlier files'.
pub(crate) fn register(model: &mut Model, clicks: &[crate::bindings_xml::ModifiedClick]) {
    let set = model.keybinds.current_set();
    for c in clicks {
        model.keybinds.clicks.register(&c.action, &c.default, set);
    }
}

/// `CGUIBindings::IsModifiedClick` (`0x55f940`): with no action, any modifier key down or any row
/// holding; an action that is itself a modifier set (`"SHIFT"`) needs each of its modifiers held;
/// otherwise the action's row holds by `0x55d6b0`, and an undeclared action never holds.
pub(super) fn is_modified_click(model: &Model, action: Option<&str>) -> bool {
    let (shift, ctrl, alt) = model.modifiers;
    let rows = &model.keybinds.clicks.rows;
    match action {
        None => shift || ctrl || alt || rows.iter().any(|r| r.live.holds(model)),
        Some(text) => match add_modifiers(text).0 {
            0 => model
                .keybinds
                .clicks
                .row(text)
                .is_some_and(|r| r.live.holds(model)),
            m => (m & 0x3 == 0 || shift) && (m & 0xc == 0 || ctrl) && (m & 0x30 == 0 || alt),
        },
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // IsModifiedClick([action]) (`0x55fcc0` → `0x55f940`): 1 or nil; a non-string is no action.
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

    // GetModifiedClick("action") (`0x55fc20`): the live binding as `GetBinding` spells it, nil
    // for an undeclared action; a non-string raises.
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
            match model.keybinds.clicks.row(&action) {
                Some(r) => Ok(Value::String(lua.create_string(r.live.canonical())?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // SetModifiedClick("action"[, "binding"]) (`0x55fb90`): rebinds a declared action in the live
    // set; a missing binding is NONE, and an undeclared action or a refused binding raises.
    g.set(
        "SetModifiedClick",
        lua.create_function(|lua, (action, binding): (Value, Option<Value>)| {
            let action = super::binding_abi::string_arg(
                lua,
                action,
                r#"Usage: SetModifiedClick("action", "binding")"#,
            )?;
            let binding = binding.and_then(|v| super::binding_abi::optional_string(lua, &v));
            let mut model = crate::script::model_mut_of(lua);
            if model.keybinds.clicks.set(&action, binding.as_deref()) {
                Ok(())
            } else {
                Err(mlua::Error::RuntimeError(format!(
                    "SetModifiedClick(): Unknown action ({action}) or binding ({})",
                    binding.as_deref().unwrap_or("(null)")
                )))
            }
        })?,
    )?;

    // GetNumModifiedClickActions() (`0x55dc60`): the declared count.
    g.set(
        "GetNumModifiedClickActions",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.keybinds.clicks.rows.len() as f64)
        })?,
    )?;

    // GetModifiedClickAction(index) (`0x55ea70`): the index-th declared action, nil past the end;
    // a non-number raises.
    g.set(
        "GetModifiedClickAction",
        lua.create_function(|lua, index: Value| {
            let Some(n) = lua.coerce_number(index)? else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetModifiedClickAction(index)".into(),
                ));
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let i = n as i64 - 1;
            match usize::try_from(i)
                .ok()
                .and_then(|i| model.keybinds.clicks.rows.get(i))
            {
                Some(r) => Ok(Value::String(lua.create_string(&r.action)?)),
                None => Ok(Value::Nil),
            }
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

    /// `GetNumModifiedClickActions`/`GetModifiedClickAction` walk the declaration order; a
    /// redefinition keeps the first (`0x562dd0`), and an index past the end is nil.
    #[test]
    fn the_actions_enumerate_in_declaration_order_and_the_first_wins() {
        let s = script();
        s.register_modified_clicks(&[ModifiedClick {
            action: "selfcast".into(),
            default: "CTRL".into(),
        }]);
        assert_eq!(
            s.eval::<i64>("return GetNumModifiedClickActions()")
                .unwrap(),
            4
        );
        assert_eq!(
            s.eval::<String>("return GetModifiedClickAction(1) .. GetModifiedClickAction('3')")
                .unwrap(),
            "SELFCASTCHATLINK"
        );
        assert!(s
            .eval::<bool>(
                "return GetModifiedClickAction(5) == nil and GetModifiedClickAction(0) == nil"
            )
            .unwrap());
        assert_eq!(
            s.eval::<String>(r#"return GetModifiedClick("SELFCAST")"#)
                .unwrap(),
            "ALT"
        );
        let err = s.run("GetModifiedClickAction()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: GetModifiedClickAction(index)"),
            "{err}"
        );
    }

    /// `SetModifiedClick` refuses an undeclared action or a binding `0x55e180` cannot read (one
    /// button digit, nothing after); a missing binding is NONE; `GetModifiedClick` spells the
    /// binding as `0x55e230` does.
    #[test]
    fn set_modified_click_validates_and_get_spells_canonically() {
        let s = script();
        for (set, get) in [
            ("shift-ctrl-button2", "CTRL-SHIFT-BUTTON2"),
            ("LALT-RCTRL", "LALT-RCTRL"),
            ("LSHIFT-RSHIFT", "SHIFT"),
            ("BUTTON3", "BUTTON3"),
            ("none", "NONE"),
        ] {
            s.run(&format!(r#"SetModifiedClick("SELFCAST", "{set}")"#))
                .unwrap();
            assert_eq!(
                s.eval::<String>(r#"return GetModifiedClick("SELFCAST")"#)
                    .unwrap(),
                get,
                "{set}"
            );
        }
        s.run(r#"SetModifiedClick("SELFCAST")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetModifiedClick("SELFCAST")"#)
                .unwrap(),
            "NONE"
        );
        for bad in [
            r#"SetModifiedClick("NOSUCHACTION", "ALT")"#,
            r#"SetModifiedClick("SELFCAST", "BUTTON12")"#,
            r#"SetModifiedClick("SELFCAST", "SHIFT-X")"#,
        ] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains("SetModifiedClick(): Unknown action"),
                "{bad}: {err}"
            );
        }
    }

    /// The table rides the key sets: `SaveBindings` keeps the rows Lua set, `LoadBindings(0)`
    /// restores every default, `LoadBindings(1)` the saved rows; a seeded set binds a row declared
    /// after it, as the saved file loads after the interface files in the reference.
    #[test]
    fn the_table_saves_and_loads_with_the_binding_sets() {
        let mut s = UiScript::new().unwrap();
        s.seed_modified_click_set(1, Some(vec![("CHATLINK".into(), "CTRL-BUTTON1".into())]));
        s.load_binding_set(1);
        s.register_modified_clicks(&[("SELFCAST", "ALT"), ("CHATLINK", "SHIFT-BUTTON1")].map(
            |(a, d)| ModifiedClick {
                action: a.into(),
                default: d.into(),
            },
        ));
        let get = |s: &UiScript, a: &str| {
            s.eval::<String>(&format!(r#"return GetModifiedClick("{a}")"#))
                .unwrap()
        };
        assert_eq!(
            get(&s, "CHATLINK"),
            "CTRL-BUTTON1",
            "the saved row, not the default"
        );
        s.run(r#"SetModifiedClick("SELFCAST", "SHIFT") SaveBindings(1)"#)
            .unwrap();
        assert_eq!(
            s.modified_click_overrides(),
            vec![
                ("SELFCAST".to_string(), "SHIFT".to_string()),
                ("CHATLINK".to_string(), "CTRL-BUTTON1".to_string()),
            ]
        );
        s.run("LoadBindings(0)").unwrap();
        assert_eq!(
            (get(&s, "SELFCAST"), get(&s, "CHATLINK")),
            ("ALT".into(), "SHIFT-BUTTON1".into())
        );
        assert!(s.modified_click_overrides().is_empty());
        s.run("LoadBindings(1)").unwrap();
        assert_eq!(
            (get(&s, "SELFCAST"), get(&s, "CHATLINK")),
            ("SHIFT".into(), "CTRL-BUTTON1".into())
        );
    }

    /// With no action, a row bound to a bare button holds outside a click (`0x55f940` walks the
    /// rows after the modifier keys); inside a click only its own button does.
    #[test]
    fn no_action_also_asks_the_rows() {
        let s = UiScript::new().unwrap();
        s.register_modified_clicks(&[ModifiedClick {
            action: "PROBE".into(),
            default: "BUTTON2".into(),
        }]);
        assert!(is(&s, "IsModifiedClick()"));
        assert!(is(&s, r#"IsModifiedClick("PROBE")"#));
        s.run(
            r#"local b = CreateFrame("Button", "NoActionProbe", UIParent)
               b:SetScript("OnClick", function(_, button)
                   SEEN = button .. ":" .. tostring(IsModifiedClick("PROBE"))
               end)
               b:Click("LeftButton")"#,
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "LeftButton:nil");
    }
}
