//! Player macros. The reference keeps them client-side with no opcode (`macros-cache.txt`), and
//! `CreateMacro` must return the new index at once, so the table lives here: the app seeds it
//! ([`UiScript::set_macros`]), saves it when [`UiScript::take_macros_dirty`] says, and pushes the
//! icon list ([`UiScript::set_macro_icons`]).
//!
//! Indices 1..=36 are the account macros and 37..=54 the character macros, each list dense from its
//! base (`MacroFrame.macroBase`): 12340 sorts each set into 36 slots (`m_sortedMacros`, read at
//! `36 * set + i`), the account set holding 36 and the character set 18 (`s_maxMacros`). A macro's
//! bound spell (the reference's macro arm returns `[rec+0x564]`) is the app's to derive: it needs
//! the spell catalog and the player's book.

use mlua::{Lua, MultiValue, Value};

use super::cursor::{queue_cursor_update, CursorMacro, CursorPayload};
use super::Model;

/// Account macros (`MAX_ACCOUNT_MACROS`, `s_maxMacros[0]`), and so the character set's base.
pub const MAX_ACCOUNT_MACROS: usize = 36;

/// Character macros (`MAX_CHARACTER_MACROS`, `s_maxMacros[1]`).
pub const MAX_CHARACTER_MACROS: usize = 18;

/// The highest macro index, the character set's last.
pub const MAX_MACRO_INDEX: usize = MAX_ACCOUNT_MACROS + MAX_CHARACTER_MACROS;

/// The name cap, `MacroPopupEditBox`'s `letters="16"`.
pub const MAX_MACRO_NAME: usize = 16;

/// The body cap, `MacroFrameText`'s `letters="255"`.
pub const MAX_MACRO_BODY: usize = 255;

/// One macro, as `GetMacroInfo` reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroView {
    pub name: String,
    /// The icon's texture path, kept by name because the reference saves the icon by name
    /// (`MACRO %d "%s" %s`).
    pub texture: Option<String>,
    /// The macro's lines, `\n`-separated.
    pub body: String,
}

/// A macro record's cached cast, the reference's `([rec+0x564], [rec+0x568])` pair: the bound spell
/// and the book it resolved from, which the app derives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacroBinding {
    /// The bound spell id; 0 for none, -1 for a `/cast` that did not resolve.
    pub spell: i32,
    /// The spell came from the pet's book, not the player's.
    pub pet_book: bool,
}

/// The macro table: the account and character lists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacroState {
    /// Indices 1..=36.
    pub account: Vec<MacroView>,
    /// Indices 37..=54.
    pub character: Vec<MacroView>,
}

/// A 1-based Lua macro index split into its list and 0-based position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacroIndex {
    /// True for 37..=54.
    per_character: bool,
    pos: usize,
}

impl MacroIndex {
    /// `None` outside 1..=54; not checked against the lists, which [`MacroState::get`] does.
    fn split(index: usize) -> Option<Self> {
        let zero = index.checked_sub(1)?;
        match zero {
            0..MAX_ACCOUNT_MACROS => Some(Self {
                per_character: false,
                pos: zero,
            }),
            _ if zero < MAX_MACRO_INDEX => Some(Self {
                per_character: true,
                pos: zero - MAX_ACCOUNT_MACROS,
            }),
            _ => None,
        }
    }

    fn lua_index(self) -> usize {
        self.pos
            + 1
            + if self.per_character {
                MAX_ACCOUNT_MACROS
            } else {
                0
            }
    }
}

impl MacroState {
    /// The macro at a 1-based Lua index.
    pub fn get(&self, index: usize) -> Option<&MacroView> {
        let at = MacroIndex::split(index)?;
        self.list(at.per_character).get(at.pos)
    }

    fn list(&self, per_character: bool) -> &Vec<MacroView> {
        if per_character {
            &self.character
        } else {
            &self.account
        }
    }

    fn list_mut(&mut self, per_character: bool) -> &mut Vec<MacroView> {
        if per_character {
            &mut self.character
        } else {
            &mut self.account
        }
    }

    /// `GetMacroIndexByName`: case-insensitive, account list first; 0 on a miss, as in 1.12.
    pub fn index_by_name(&self, name: &str) -> usize {
        for per_character in [false, true] {
            for (i, m) in self.list(per_character).iter().enumerate() {
                if m.name.eq_ignore_ascii_case(name) {
                    return MacroIndex {
                        per_character,
                        pos: i,
                    }
                    .lua_index();
                }
            }
        }
        0
    }
}

/// Clamp to a count of characters, not bytes, as the edit boxes do, so a script cannot store more.
fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl super::UiScript {
    /// Seed the whole table from the app's load. It does not mark the table dirty, or the load
    /// would trigger a save.
    pub fn set_macros(&mut self, state: MacroState) {
        let mut model = self.model_mut();
        model.macros = state;
        model.macros_dirty = false;
        model.macros_generation += 1;
    }

    /// The live table, for the app's save.
    pub fn macros(&self) -> MacroState {
        self.model_mut().macros.clone()
    }

    /// Push every macro's cached cast by 1-based index, replacing the last push.
    pub fn set_macro_bindings(&mut self, bindings: std::collections::HashMap<u32, MacroBinding>) {
        set_field!(self, macro_bindings, bindings);
    }

    /// Whether a script changed the table since the last call; the app then saves it and fires
    /// `UPDATE_MACROS`.
    pub fn take_macros_dirty(&mut self) -> bool {
        drain_field!(self, macros_dirty)
    }

    /// Push the icon chooser's texture paths in grid order.
    pub fn set_macro_icons(&mut self, icons: Vec<String>) {
        set_field!(self, macro_icons, icons);
    }

    /// Install the host's builder for the chooser's list. Like 12340's `GetNumMacroIcons`
    /// (`0x566490`, which runs `0x565840` while the count at `[0xbeaf94]` is 0), the first
    /// `GetNumMacroIcons` builds it; `GetMacroIconInfo` (`0x564e90`) only reads what is built.
    pub fn set_macro_icon_source(&mut self, source: crate::script::MacroIconSource) {
        self.model_mut().macro_icon_source = Some(source);
    }

    /// Bumped by every seed and every change, so a per-frame reader such as the action bar compares
    /// a `u64` instead of diffing the table. Not [`Self::take_macros_dirty`]: that drain has one
    /// owner, the save, and a second reader would steal its edge.
    pub fn macros_generation(&self) -> u64 {
        self.model_ref().macros_generation
    }
}

/// `CreateMacro(name, iconIndex, body, local, perCharacter)`: the new 1-based index, or `None` on
/// the two failures the client logs, an empty name and a full tab.
fn create_macro(
    model: &mut Model,
    name: &str,
    texture: Option<String>,
    body: &str,
    per_character: bool,
) -> Option<usize> {
    let name = clamp_chars(name.trim(), MAX_MACRO_NAME);
    if name.is_empty() {
        model.record_warning("CreateMacro() failed, no name specified");
        return None;
    }
    let list = model.macros.list_mut(per_character);
    let cap = if per_character {
        MAX_CHARACTER_MACROS
    } else {
        MAX_ACCOUNT_MACROS
    };
    if list.len() >= cap {
        // The message names 36 for either set (`Script_CreateMacro`).
        model.record_warning(format!(
            "CreateMacro() failed, already have {MAX_ACCOUNT_MACROS} macros"
        ));
        return None;
    }
    list.push(MacroView {
        name,
        texture,
        body: clamp_chars(body, MAX_MACRO_BODY),
    });
    let pos = list.len() - 1;
    model.macros_dirty = true;
    model.macros_generation += 1;
    Some(MacroIndex { per_character, pos }.lua_index())
}

/// `EditMacro(index, name, icon, body)` (`0x566fa0`): an omitted argument leaves its field alone, since
/// the stock UI calls it with disjoint halves, `(sel, name, icon)` from the rename popup and
/// `(sel, nil, nil, text)` from `MacroFrame_SaveMacro`.
fn edit_macro(
    model: &mut Model,
    index: usize,
    name: Option<String>,
    texture: Option<Option<String>>,
    body: Option<String>,
) -> Option<usize> {
    let at = MacroIndex::split(index)?;
    let entry = model.macros.list_mut(at.per_character).get_mut(at.pos)?;
    if let Some(name) = name {
        let name = clamp_chars(name.trim(), MAX_MACRO_NAME);
        // A blank name is ignored; the stock popup cannot send one (`MacroPopupOkayButton_Update`).
        if !name.is_empty() {
            entry.name = name;
        }
    }
    if let Some(texture) = texture {
        entry.texture = texture;
    }
    if let Some(body) = body {
        entry.body = clamp_chars(&body, MAX_MACRO_BODY);
    }
    model.macros_dirty = true;
    model.macros_generation += 1;
    Some(index)
}

/// `DeleteMacro(index)` closes the gap, as the reference's dense list does, so an action-bar
/// button holding a higher index then shows the next macro.
fn delete_macro(model: &mut Model, index: usize) -> bool {
    let Some(at) = MacroIndex::split(index) else {
        return false;
    };
    let list = model.macros.list_mut(at.per_character);
    if at.pos >= list.len() {
        return false;
    }
    list.remove(at.pos);
    model.macros_dirty = true;
    model.macros_generation += 1;
    true
}

/// `PickupMacro(index)`: the macro payload (cursor mode 8), which `PlaceAction` packs as
/// `macroId | 0x40000000`. Refused while the cursor holds anything; the reference's setter clears
/// the cursor first instead.
fn pickup_macro(model: &mut Model, index: usize) -> bool {
    if model.cursor.is_some() {
        return false;
    }
    let Some(entry) = model.macros.get(index) else {
        return false;
    };
    let payload = CursorMacro {
        index: index as u32,
        texture: entry.texture.clone(),
    };
    model.cursor = Some(CursorPayload::Macro(payload));
    queue_cursor_update(model);
    true
}

fn opt_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The `local` and `perCharacter` flags: `nil`, `false` and `0` are false, as in `UseAction`. The
/// reference reads them with `GetBoolOrDefault`, which also takes `"0"` and any number that
/// truncates to 0 as false; the stock popup passes a Lua boolean and never `local`.
fn flag(v: Option<&Value>) -> bool {
    v.is_some_and(super::action::truthy_nonzero)
}

/// Register the macro globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumMacros",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok((
                model.macros.account.len() as i64,
                model.macros.character.len() as i64,
            ))
        })?,
    )?;

    g.set(
        "GetMacroInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // `0x566eb0`: name, texture and body, or nothing for no macro.
            let Some(m) = model.macros.get(index) else {
                return Ok(MultiValue::new());
            };
            let texture = match &m.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&m.name)?),
                texture,
                Value::String(lua.create_string(&m.body)?),
            ]))
        })?,
    )?;

    g.set(
        "GetMacroIndexByName",
        lua.create_function(|lua, name: String| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.macros.index_by_name(&name) as i64)
        })?,
    )?;

    g.set(
        "GetNumMacroIcons",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if let Some(source) = model.macro_icon_source.take() {
                model.macro_icons = source();
            }
            Ok(model.macro_icons.len() as i64)
        })?,
    )?;

    g.set(
        "GetMacroIconInfo",
        lua.create_function(|lua, index: usize| {
            let path = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                index
                    .checked_sub(1)
                    .and_then(|i| model.macro_icons.get(i))
                    .cloned()
            };
            match path {
                Some(p) => Ok(Value::String(lua.create_string(&p)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // CreateMacro(name, iconIndex, body, perCharacter) (`0x568160`) stores `iconIndex`, an index
    // into the chooser list, as its path.
    g.set(
        "CreateMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let name = opt_string(a.first()).unwrap_or_default();
            let mut model = crate::script::model_mut_of(lua);
            let texture = icon_path(&model, a.get(1));
            let body = opt_string(a.get(2)).unwrap_or_default();
            let per_character = flag(a.get(3));
            Ok(
                match create_macro(&mut model, &name, texture, &body, per_character) {
                    Some(i) => Value::Integer(i as i64),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    g.set(
        "EditMacro",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let Some(index) = a.first().and_then(|v| v.as_integer()).map(|i| i as usize) else {
                return Ok(Value::Nil);
            };
            let mut model = crate::script::model_mut_of(lua);
            // A nil icon leaves the icon alone: `MacroFrame_SaveMacro` passes nil on every save.
            let texture = match a.get(2) {
                None | Some(Value::Nil) => None,
                other => Some(icon_path(&model, other)),
            };
            let name = opt_string(a.get(1));
            let body = opt_string(a.get(3));
            Ok(match edit_macro(&mut model, index, name, texture, body) {
                Some(i) => Value::Integer(i as i64),
                None => Value::Nil,
            })
        })?,
    )?;

    g.set(
        "DeleteMacro",
        lua.create_function(|lua, index: usize| {
            let mut model = crate::script::model_mut_of(lua);
            Ok(delete_macro(&mut model, index))
        })?,
    )?;

    g.set(
        "PickupMacro",
        lua.create_function(|lua, index: usize| {
            let mut model = crate::script::model_mut_of(lua);
            Ok(pickup_macro(&mut model, index))
        })?,
    )?;

    Ok(())
}

/// Resolve a chooser-list index to its texture path; a string is taken as a path already.
fn icon_path(model: &Model, v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        Some(v) => {
            let i = v.as_integer()?;
            let i = usize::try_from(i).ok()?.checked_sub(1)?;
            model.macro_icons.get(i).cloned()
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn seeded() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_macro_icons(vec![
            "Interface\\Icons\\Ability_Ambush".into(),
            "Interface\\Icons\\Ability_BackStab".into(),
            "Interface\\Icons\\Spell_Fire_FlameBolt".into(),
        ]);
        s
    }

    #[test]
    fn the_macro_api_round_trip_over_the_two_tabs() {
        let s = seeded();
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 0)
        );

        let i = s
            .eval::<i64>(r#"return CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();
        assert_eq!(i, 1);
        let j = s
            .eval::<i64>(r#"return CreateMacro("Bolt", 3, "/cast Fireball", true)"#)
            .unwrap();
        assert_eq!(j, 37, "the character tab starts at MAX_ACCOUNT_MACROS + 1");
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (1, 1)
        );

        let (name, tex, body, count) = s
            .eval::<(String, String, String, i64)>(
                "local n, t, b = GetMacroInfo(1) return n, t, b, select('#', GetMacroInfo(1))",
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), tex.as_str(), body.as_str()),
            ("Ambush", "Interface\\Icons\\Ability_Ambush", "/cast Ambush")
        );
        assert_eq!(count, 3, "name, texture and body (`0x566eb0`), no isLocal");

        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("ambush")"#)
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("bolt")"#)
                .unwrap(),
            37
        );
        assert_eq!(
            s.eval::<i64>(r#"return GetMacroIndexByName("nope")"#)
                .unwrap(),
            0
        );

        // The character macro keeps its own base after the delete.
        assert!(s.eval::<bool>("return DeleteMacro(1)").unwrap());
        assert_eq!(
            s.eval::<(i64, i64)>("local a, c = GetNumMacros() return a, c")
                .unwrap(),
            (0, 1)
        );
        assert!(s.eval::<bool>("return GetMacroInfo(1) == nil").unwrap());
        assert_eq!(s.eval::<String>("return GetMacroInfo(37)").unwrap(), "Bolt");
    }

    #[test]
    fn edit_macro_leaves_omitted_fields_alone() {
        let s = seeded();
        s.run(r#"CreateMacro("Old", 1, "/cast Ambush")"#).unwrap();

        // MacroPopupOkayButton_OnClick's form: name and icon.
        s.run(r#"EditMacro(1, "New", 2)"#).unwrap();
        let (name, tex, body) = s
            .eval::<(String, String, String)>("local n, t, b = GetMacroInfo(1) return n, t, b")
            .unwrap();
        assert_eq!(name, "New");
        assert_eq!(tex, "Interface\\Icons\\Ability_BackStab");
        assert_eq!(body, "/cast Ambush", "the body survives a rename");

        // MacroFrame_SaveMacro's form: the body alone.
        s.run(r#"EditMacro(1, nil, nil, "/cast Backstab")"#)
            .unwrap();
        let (name, tex, body) = s
            .eval::<(String, String, String)>("local n, t, b = GetMacroInfo(1) return n, t, b")
            .unwrap();
        assert_eq!(
            (name.as_str(), tex.as_str(), body.as_str()),
            (
                "New",
                "Interface\\Icons\\Ability_BackStab",
                "/cast Backstab"
            ),
            "a body save touches neither the name nor the icon"
        );
    }

    #[test]
    fn create_refuses_a_blank_name_and_a_full_tab_and_clamps_the_caps() {
        let s = seeded();
        assert!(s
            .eval::<bool>(r#"return CreateMacro("", 1, "x") == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return CreateMacro("   ", 1, "x") == nil"#)
            .unwrap());

        for i in 1..=MAX_ACCOUNT_MACROS {
            let made = s
                .eval::<i64>(&format!(r#"return CreateMacro("m{i}", 1, "")"#))
                .unwrap();
            assert_eq!(made as usize, i);
        }
        assert!(
            s.eval::<bool>(r#"return CreateMacro("one too many", 1, "") == nil"#)
                .unwrap(),
            "the 37th account macro is refused"
        );
        // The character set is a separate 18, from index 37.
        assert_eq!(
            s.eval::<i64>(r#"return CreateMacro("c", 1, "", 1)"#)
                .unwrap(),
            37
        );

        let s = seeded();
        s.run(&format!(
            r#"CreateMacro("{}", 1, "{}")"#,
            "N".repeat(40),
            "b".repeat(400)
        ))
        .unwrap();
        let (name, body) = s
            .eval::<(String, String)>("local n, _, b = GetMacroInfo(1) return n, b")
            .unwrap();
        assert_eq!(name.chars().count(), MAX_MACRO_NAME);
        assert_eq!(body.chars().count(), MAX_MACRO_BODY);
    }

    #[test]
    fn mutations_raise_the_dirty_flag_and_a_seed_does_not() {
        let mut s = seeded();
        assert!(!s.take_macros_dirty());

        s.run(r#"CreateMacro("a", 1, "")"#).unwrap();
        assert!(s.take_macros_dirty());
        assert!(!s.take_macros_dirty(), "the drain clears it");

        s.run(r#"EditMacro(1, nil, nil, "/say hi")"#).unwrap();
        assert!(s.take_macros_dirty());

        s.run("DeleteMacro(1)").unwrap();
        assert!(s.take_macros_dirty());

        s.run("DeleteMacro(7)").unwrap();
        assert!(!s.take_macros_dirty());

        s.set_macros(MacroState {
            account: vec![MacroView {
                name: "loaded".into(),
                ..Default::default()
            }],
            character: Vec::new(),
        });
        assert!(!s.take_macros_dirty());
        assert_eq!(s.macros().account.len(), 1);
    }

    #[test]
    fn pickup_macro_loads_the_cursor_and_refuses_while_holding() {
        use crate::script::cursor::CursorPayload;

        let s = seeded();
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();

        assert!(
            !s.eval::<bool>("return PickupMacro(5)").unwrap(),
            "empty slot"
        );
        assert!(s.eval::<bool>("return PickupMacro(1)").unwrap());
        assert_eq!(
            s.cursor_payload(),
            Some(CursorPayload::Macro(CursorMacro {
                index: 1,
                texture: Some("Interface\\Icons\\Ability_Ambush".into()),
            }))
        );
        assert!(
            !s.eval::<bool>("return PickupMacro(1)").unwrap(),
            "a macro button is a source, never a drop target"
        );
    }

    #[test]
    fn the_icon_chooser_list_is_one_based_with_a_nil_tail() {
        let s = seeded();
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 3);
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(2)").unwrap(),
            "Interface\\Icons\\Ability_BackStab"
        );
        assert!(s.eval::<bool>("return GetMacroIconInfo(4) == nil").unwrap());
        assert!(s.eval::<bool>("return GetMacroIconInfo(0) == nil").unwrap());
    }

    /// 12340 builds the list on the first `GetNumMacroIcons` (`0x566490`) and never in
    /// `GetMacroIconInfo` (`0x564e90`), so a VM that never opens the chooser never lists.
    #[test]
    fn the_icon_chooser_list_builds_on_the_first_count_and_only_once() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut s = UiScript::new().unwrap();
        let builds = Rc::new(Cell::new(0));
        let counter = Rc::clone(&builds);
        s.set_macro_icon_source(Box::new(move || {
            counter.set(counter.get() + 1);
            vec!["Interface\\Icons\\Ability_Ambush".into()]
        }));
        assert!(s.eval::<bool>("return GetMacroIconInfo(1) == nil").unwrap());
        assert_eq!(builds.get(), 0, "an info read must not build the list");
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetNumMacroIcons()").unwrap(), 1);
        assert_eq!(builds.get(), 1, "the list builds once");
        assert_eq!(
            s.eval::<String>("return GetMacroIconInfo(1)").unwrap(),
            "Interface\\Icons\\Ability_Ambush"
        );
    }
}
