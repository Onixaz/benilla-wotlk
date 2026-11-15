//! The glue AddOns screen's enable verbs (table `0xac3e00`): the enable state of every addon for
//! every character of the realm, read and set per character or, with `nil`, across them all, saved
//! by `SaveAddOns` and reloaded by `ResetAddOns`.
//!
//! The host seeds the per-character lists from the characters' enable files
//! ([`super::UiScript::set_glue_addon_enable`]) and writes the nodes `SaveAddOns` hands it
//! ([`super::GlueRequest::SaveAddOns`]). Every answer is computed from the explicit rows as the
//! query does; with no characters there is no list, so each addon reads its default state and
//! nothing can set or save it.

use mlua::{Lua, MultiValue, Value};

use super::addon_enable::EnableLists;
use super::glue_core::{queue, GlueRequest};
use super::Model;

/// One dirty character node as `SaveAddOns` hands it to the host: every row, written as the
/// character's whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueAddonList {
    pub character: String,
    pub rows: Vec<(String, bool)>,
}

/// The lists the verbs read and write, what `ResetAddOns` reloads, and each registry row's
/// `## DefaultState`.
#[derive(Default)]
pub(crate) struct GlueAddonEnable {
    lists: EnableLists,
    loaded: EnableLists,
    defaults: Vec<bool>,
}

impl GlueAddonEnable {
    /// `GetAddOnEnableState` for a registry row: the query with the compute flag.
    fn state(&self, name: &str, row: usize, character: Option<&str>) -> u8 {
        let default = self.defaults.get(row).copied().unwrap_or(true);
        self.lists.state(name, default, character)
    }
}

/// The registry bit the glue's `GetAddOnInfo` gate reads for a row: the loadable test asks the
/// query across every character (`0x5f7ecc` with no character), disabled only when none has it.
fn refresh_enabled(m: &mut Model, row: usize) {
    let Some(name) = m.addons.get(row).map(|a| a.name.clone()) else {
        return;
    };
    let on = m.glue.addons.state(&name, row, None) != 0;
    m.addons[row].enabled = on;
}

/// The setter for one registry row, then its gate bit.
fn set_row(m: &mut Model, row: usize, character: Option<&str>, on: bool) {
    let Some(name) = m.addons.get(row).map(|a| a.name.clone()) else {
        return;
    };
    m.glue.addons.lists.set(&name, character, on);
    refresh_enabled(m, row);
}

impl super::UiScript {
    /// Seed the AddOns screen's state: the realm's per-character lists as loaded, and each
    /// registry row's `## DefaultState`. The lists are also what `ResetAddOns` returns to.
    pub fn set_glue_addon_enable(&mut self, lists: EnableLists, defaults: Vec<bool>) {
        let mut m = self.model_mut();
        m.glue.addons = GlueAddonEnable {
            loaded: lists.clone(),
            lists,
            defaults,
        };
        for row in 0..m.addons.len() {
            refresh_enabled(&mut m, row);
            m.addons[row].saved_enabled = m.addons[row].enabled;
        }
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
            let name = &m.addons[row].name;
            Ok(f64::from(m.glue.addons.state(
                name,
                row,
                character.as_deref(),
            )))
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
                let mut m = crate::script::model_mut_of(lua);
                set_row(&mut m, row, character.as_deref(), on);
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
                    set_row(&mut m, row, character.as_deref(), on);
                }
                Ok(())
            })?,
        )?;
    }

    // `SaveAddOns()` (`0x4dcac0` → `0x5f76a0`): each dirty node to the host's writer, clean after;
    // what was written is what a reload now reads.
    g.set(
        "SaveAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let lists = {
                let mut m = crate::script::model_mut_of(lua);
                let e = &mut m.glue.addons;
                let dirty = e.lists.take_dirty();
                e.loaded = e.lists.clone();
                dirty
            };
            if !lists.is_empty() {
                let lists = lists
                    .into_iter()
                    .map(|(character, rows)| GlueAddonList { character, rows })
                    .collect();
                queue(lua, GlueRequest::SaveAddOns(lists));
            }
            Ok(())
        })?,
    )?;

    // `ResetAddOns()` (`0x4dcad0` → `0x5f91e0` with no character): every node reloaded, the
    // lists as last loaded or saved.
    g.set(
        "ResetAddOns",
        lua.create_function(|lua, _: MultiValue| {
            let mut m = crate::script::model_mut_of(lua);
            m.glue.addons.lists = m.glue.addons.loaded.clone();
            for row in 0..m.addons.len() {
                refresh_enabled(&mut m, row);
            }
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
    use crate::script::{EnableHash, EnableLists, GlueAddonList, GlueRequest, UiScript};

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

    /// A glue VM over `names` (default state each), the server's reply seated, and one list per
    /// `(character, rows)`.
    fn seated(names: &[(&str, bool)], lists: &[(&str, &[(&str, bool)])]) -> UiScript {
        let mut s = vm();
        s.register_addons(
            names.iter().map(|(n, _)| addon(n)).collect(),
            None,
            None,
            None,
        );
        // The index space opens with the server's reply.
        s.note_addon_info_reply(&[]);
        let nodes = lists
            .iter()
            .map(|(c, rows)| {
                let rows = rows.iter().map(|(n, on)| (n.to_string(), *on));
                (c.to_string(), EnableHash::from_rows(rows))
            })
            .collect();
        s.set_glue_addon_enable(
            EnableLists::new(nodes),
            names.iter().map(|(_, d)| *d).collect(),
        );
        s
    }

    fn state(s: &UiScript, character: &str, index: u32) -> f64 {
        s.eval::<f64>(&format!("return GetAddOnEnableState({character}, {index})"))
            .unwrap()
    }

    fn saved(s: &mut UiScript) -> Vec<GlueAddonList> {
        s.run("SaveAddOns()").unwrap();
        match s.take_glue_requests().as_slice() {
            [] => Vec::new(),
            [GlueRequest::SaveAddOns(lists)] => lists.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn list(character: &str, rows: &[(&str, bool)]) -> GlueAddonList {
        GlueAddonList {
            character: character.into(),
            rows: rows.iter().map(|(n, on)| (n.to_string(), *on)).collect(),
        }
    }

    const ROSTER: [(&str, &[(&str, bool)]); 3] = [("Ann", &[]), ("Bob", &[]), ("Cid", &[])];

    /// One character's disable is the only explicit row, so the others take its aggregate 0 at
    /// once: All reads 0, not "some", and the glue's `GetAddOnInfo` gate reads it disabled.
    #[test]
    fn a_per_character_disable_with_no_other_rows_turns_the_others_off() {
        let mut s = seated(&[("Alpha", true)], &ROSTER);
        assert_eq!(state(&s, "nil", 1), 2.0);
        s.run("DisableAddOn('bob', 1)").unwrap();
        assert_eq!(state(&s, "nil", 1), 0.0);
        assert_eq!(state(&s, "'Ann'", 1), 0.0);
        let reason: String = s.eval("return select(6, GetAddOnInfo(1))").unwrap();
        assert_eq!(reason, "DISABLED");
        assert_eq!(saved(&mut s), vec![list("Bob", &[("Alpha", false)])]);
    }

    /// Mixed explicit rows aggregate to 1, so a list without a row takes the default state.
    #[test]
    fn mixed_explicit_rows_give_some_and_the_rest_take_the_default() {
        let lists: [(&str, &[(&str, bool)]); 3] = [
            ("Ann", &[("Alpha", true), ("Beta", true)]),
            ("Bob", &[("Alpha", false), ("Beta", false)]),
            ("Cid", &[]),
        ];
        let s = seated(&[("Alpha", true), ("Beta", false)], &lists);
        assert_eq!(state(&s, "'Cid'", 1), 2.0, "default enabled");
        assert_eq!(state(&s, "'Cid'", 2), 0.0, "DefaultState: disabled");
        assert_eq!(state(&s, "nil", 1), 1.0);
        assert_eq!(state(&s, "nil", 2), 1.0);
    }

    /// No explicit rows anywhere, or no list at all, is the default state's 2/0.
    #[test]
    fn no_rows_fall_back_to_the_default_state() {
        let mut s = seated(&[("Alpha", true), ("Beta", false)], &ROSTER);
        assert_eq!(state(&s, "nil", 1), 2.0);
        assert_eq!(state(&s, "nil", 2), 0.0);
        assert_eq!(state(&s, "'Nobody'", 2), 0.0, "a name with no list");
        let mut none = seated(&[("Alpha", true), ("Beta", false)], &[]);
        assert_eq!(state(&none, "nil", 1), 2.0);
        none.run("DisableAddOn(nil, 1)").unwrap();
        assert_eq!(state(&none, "nil", 1), 2.0, "no list to set");
        assert!(saved(&mut none).is_empty());
        assert!(saved(&mut s).is_empty(), "nothing set, nothing written");
    }

    /// The setter creates a missing row even when it matches what the query computed, and the
    /// new row is saved and decides the aggregate.
    #[test]
    fn enabling_an_already_enabled_addon_creates_and_saves_a_row() {
        let s = seated(&[("Alpha", false)], &ROSTER);
        s.run("EnableAddOn('Ann', 1) DisableAddOn('Ann', 1) EnableAddOn('Ann', 1)")
            .unwrap();
        assert_eq!(
            state(&s, "'Cid'", 1),
            2.0,
            "the one row, on, is the aggregate"
        );
        let mut fresh = seated(&[("Alpha", true)], &ROSTER);
        fresh.run("EnableAddOn('Ann', 1)").unwrap();
        assert_eq!(saved(&mut fresh), vec![list("Ann", &[("Alpha", true)])]);
        assert!(saved(&mut fresh).is_empty(), "clean after the write");
    }

    /// `nil` writes a row in every list; Reset returns to the lists as last loaded or saved.
    #[test]
    fn nil_writes_a_row_in_every_list_and_reset_restores_the_loaded_rows() {
        let lists: [(&str, &[(&str, bool)]); 2] = [("Ann", &[("Alpha", false)]), ("Bob", &[])];
        let mut s = seated(&[("Alpha", true), ("Beta", true)], &lists);
        assert_eq!(state(&s, "nil", 1), 0.0, "Bob takes Ann's lone off");
        s.run("EnableAddOn(nil, 1)").unwrap();
        assert_eq!(state(&s, "nil", 1), 2.0);
        s.run("ResetAddOns()").unwrap();
        assert_eq!(state(&s, "nil", 1), 0.0);
        let reason: String = s.eval("return select(6, GetAddOnInfo(1))").unwrap();
        assert_eq!(reason, "DISABLED", "the gate bit follows the reset");
        s.run("DisableAllAddOns(nil)").unwrap();
        assert_eq!(
            saved(&mut s),
            vec![
                list("Ann", &[("Alpha", false), ("Beta", false)]),
                list("Bob", &[("Alpha", false), ("Beta", false)]),
            ]
        );
        s.run("EnableAddOn('Bob', 2) ResetAddOns()").unwrap();
        assert_eq!(
            state(&s, "'Bob'", 2),
            0.0,
            "reset returns to the saved lists"
        );
    }
}
