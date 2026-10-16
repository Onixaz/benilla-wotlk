//! The glue AddOns screen's enable verbs (table `0xac3e00`): the enable state of every addon for
//! every character of the realm, read and set per character or, with `nil`, across them all, saved
//! by `SaveAddOns` and reloaded by `ResetAddOns`.
//!
//! The host seeds the state from the characters' enable files ([`super::UiScript::set_glue_addon_enable`])
//! and writes what `SaveAddOns` changed ([`super::GlueRequest::SaveAddOns`]). With no characters
//! the state is one column, the realm's default, which nothing can save.

use mlua::{Lua, MultiValue, Value};

use super::glue_core::{queue, GlueRequest};

/// One addon's enable bit for one character, as `SaveAddOns` hands it to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueAddonChange {
    pub character: String,
    pub addon: String,
    pub enabled: bool,
}

/// The enable state the verbs read and write, by registry row then character.
#[derive(Default)]
pub(crate) struct GlueAddonEnable {
    characters: Vec<String>,
    cells: Vec<Vec<bool>>,
    saved: Vec<Vec<bool>>,
}

impl GlueAddonEnable {
    /// The column a character string names, any case; `None` for a name off the roster.
    fn column(&self, character: &str) -> Option<usize> {
        self.characters
            .iter()
            .position(|c| c.eq_ignore_ascii_case(character))
    }

    /// `GetAddOnEnableState`: 2 enabled, 0 disabled for one character; across them all 2 when
    /// every character has it, 1 for some, 0 for none.
    fn state(&self, row: usize, character: Option<&str>) -> u8 {
        let Some(cells) = self.cells.get(row) else {
            return 0;
        };
        match character {
            Some(c) => match self.column(c).and_then(|i| cells.get(i)) {
                Some(true) => 2,
                _ => 0,
            },
            None => {
                let on = cells.iter().filter(|&&b| b).count();
                if on == 0 {
                    0
                } else if on == cells.len() {
                    2
                } else {
                    1
                }
            }
        }
    }

    /// `SetAddOnEnableState`: one character's bit, or every character's.
    fn set(&mut self, row: usize, character: Option<&str>, on: bool) {
        let column = character.map(|c| self.column(c));
        let Some(cells) = self.cells.get_mut(row) else {
            return;
        };
        match column {
            Some(Some(i)) => {
                if let Some(b) = cells.get_mut(i) {
                    *b = on;
                }
            }
            Some(None) => {}
            None => cells.iter_mut().for_each(|b| *b = on),
        }
    }
}

impl super::UiScript {
    /// Seed the AddOns screen's state: the realm's characters and, per registry row, each one's
    /// enable bit (`cells[row][character]`); with no characters, one column of defaults. The seed
    /// is also what `ResetAddOns` returns to.
    pub fn set_glue_addon_enable(&mut self, characters: Vec<String>, cells: Vec<Vec<bool>>) {
        let mut m = self.model_mut();
        m.glue.addons = GlueAddonEnable {
            characters,
            saved: cells.clone(),
            cells,
        };
    }
}

/// The optional character argument: a string, or nil for every character; anything else is the
/// usage error.
fn character_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<Option<String>> {
    match v {
        Value::Nil => Ok(None),
        v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
            Ok(lua.coerce_string(v)?.map(|s| s.to_string_lossy()))
        }
        _ => Err(mlua::Error::runtime(usage)),
    }
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

/// A Lua index into the registry row, or the reference's range error.
fn row(lua: &Lua, index: Value, usage: &'static str) -> mlua::Result<usize> {
    let model = crate::script::model_mut_of(lua);
    super::addon::glue_index(lua, &model, index, usage)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetAddOnEnableState(character, index)` (`0x4dc7c0`).
    g.set(
        "GetAddOnEnableState",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: GetAddOnEnableState(\"character\", index)";
            let character = character_arg(lua, arg(&args, 0), USAGE)?;
            if lua.coerce_number(arg(&args, 1))?.is_none() {
                return Err(mlua::Error::runtime(USAGE));
            }
            let row = row(lua, arg(&args, 1), USAGE)?;
            let m = crate::script::model_mut_of(lua);
            Ok(f64::from(m.glue.addons.state(row, character.as_deref())))
        })?,
    )?;

    // `EnableAddOn(character, index)` (`0x4dc8a0`) and `DisableAddOn` (`0x4dc9b0`).
    for (name, on, usage) in [
        (
            "EnableAddOn",
            true,
            "Usage: EnableAddOn(\"character\", index)",
        ),
        (
            "DisableAddOn",
            false,
            "Usage: DisableAddOn(\"character\", index)",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let character = character_arg(lua, arg(&args, 0), usage)?;
                if lua.coerce_number(arg(&args, 1))?.is_none() {
                    return Err(mlua::Error::runtime(usage));
                }
                let row = row(lua, arg(&args, 1), usage)?;
                crate::script::model_mut_of(lua)
                    .glue
                    .addons
                    .set(row, character.as_deref(), on);
                Ok(())
            })?,
        )?;
    }

    // `EnableAllAddOns(character)` (`0x4dc960`) and `DisableAllAddOns`: every addon of the Lua
    // index space.
    for (name, on) in [("EnableAllAddOns", true), ("DisableAllAddOns", false)] {
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let character = match arg(&args, 0) {
                    v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                        lua.coerce_string(v)?.map(|s| s.to_string_lossy())
                    }
                    _ => None,
                };
                let mut m = crate::script::model_mut_of(lua);
                let rows = m.addon_index.clone();
                for row in rows {
                    m.glue.addons.set(row, character.as_deref(), on);
                }
                Ok(())
            })?,
        )?;
    }

    // `SaveAddOns()` (`0x4dcac0`): every changed bit to the host's writer, the state now saved.
    g.set(
        "SaveAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let changes = {
                let mut m = crate::script::model_mut_of(lua);
                let names: Vec<String> = m.addons.iter().map(|a| a.name.clone()).collect();
                let e = &mut m.glue.addons;
                let mut changes = Vec::new();
                for (row, (now, was)) in e.cells.iter().zip(&e.saved).enumerate() {
                    for (column, (&on, &before)) in now.iter().zip(was).enumerate() {
                        if on != before {
                            if let (Some(character), Some(addon)) =
                                (e.characters.get(column), names.get(row))
                            {
                                changes.push(GlueAddonChange {
                                    character: character.clone(),
                                    addon: addon.clone(),
                                    enabled: on,
                                });
                            }
                        }
                    }
                }
                e.saved = e.cells.clone();
                changes
            };
            if !changes.is_empty() {
                queue(lua, GlueRequest::SaveAddOns(changes));
            }
            Ok(())
        })?,
    )?;

    // `ResetAddOns()` (`0x4dcad0`): the state as last loaded or saved.
    g.set(
        "ResetAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let mut m = crate::script::model_mut_of(lua);
            let e = &mut m.glue.addons;
            e.cells = e.saved.clone();
            Ok(())
        })?,
    )?;

    // `LaunchAddOnURL(index)` (`0x4dcd60`): the addon's url, from the server's reply, opened.
    g.set(
        "LaunchAddOnURL",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: LaunchAddOnURL(index)";
            if lua.coerce_number(arg(&args, 0))?.is_none() {
                return Err(mlua::Error::runtime(USAGE));
            }
            let row = row(lua, arg(&args, 0), USAGE)?;
            let url = crate::script::model_mut_of(lua).addons[row].url.clone();
            if let Some(url) = url {
                queue(lua, GlueRequest::LaunchUrl(url));
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{GlueRequest, UiScript};

    fn vm() -> UiScript {
        UiScript::new_glue().unwrap()
    }

    fn addon(name: &str) -> crate::script::addon::AddOnInfo {
        crate::script::addon::AddOnInfo {
            name: name.into(),
            enabled: true,
            ..Default::default()
        }
    }

    /// No registry: every index is out of range, and the error names the range.
    #[test]
    fn an_index_off_the_registry_raises_the_range() {
        let s = vm();
        let err = s
            .run("GetAddOnEnableState(nil, 1)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("AddOn index must be in the range of 1 to 0"),
            "{err}"
        );
        let err = s.run("EnableAddOn({}, 1)").unwrap_err().to_string();
        assert!(
            err.contains("Usage: EnableAddOn(\"character\", index)"),
            "{err}"
        );
    }

    /// The tri-state across characters, a per-character set, the All set, and Save handing the
    /// host only what changed; Reset returns to the saved state.
    #[test]
    fn the_state_is_per_character_and_saves_its_changes() {
        let mut s = vm();
        s.register_addons(vec![addon("Alpha"), addon("Beta")], None, None, None);
        // The index space opens with the server's reply.
        s.note_addon_info_reply(&[]);
        s.set_glue_addon_enable(
            vec!["Ann".into(), "Bob".into()],
            vec![vec![true, false], vec![true, true]],
        );
        let state = |s: &UiScript, call: &str| s.eval::<f64>(&format!("return {call}")).unwrap();
        assert_eq!(state(&s, "GetAddOnEnableState(nil, 1)"), 1.0);
        assert_eq!(state(&s, "GetAddOnEnableState(nil, 2)"), 2.0);
        assert_eq!(state(&s, "GetAddOnEnableState('ann', 1)"), 2.0);
        assert_eq!(state(&s, "GetAddOnEnableState('Bob', 1)"), 0.0);
        s.run("EnableAddOn('Bob', 1) DisableAllAddOns('Ann')")
            .unwrap();
        assert_eq!(state(&s, "GetAddOnEnableState(nil, 1)"), 1.0);
        s.run("ResetAddOns()").unwrap();
        assert_eq!(state(&s, "GetAddOnEnableState('Bob', 1)"), 0.0);
        s.run("EnableAddOn(nil, 1) SaveAddOns()").unwrap();
        let requests = s.take_glue_requests();
        match requests.as_slice() {
            [GlueRequest::SaveAddOns(changes)] => {
                assert_eq!(changes.len(), 1);
                assert_eq!(
                    (
                        changes[0].character.as_str(),
                        changes[0].addon.as_str(),
                        changes[0].enabled
                    ),
                    ("Bob", "Alpha", true)
                );
            }
            other => panic!("{other:?}"),
        }
        s.run("SaveAddOns()").unwrap();
        assert!(s.take_glue_requests().is_empty(), "nothing changed since");
    }
}
