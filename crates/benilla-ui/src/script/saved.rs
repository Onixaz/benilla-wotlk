//! Saved variables: the Lua settings that survive a restart, in 12340's files and grammar. FrameXML's
//! `RegisterForSave` globals go to the account's `SavedVariables.lua`, its
//! `RegisterForSavePerCharacter` ones to the character's (`CGGameUI::RegisterSavedVariable`
//! `0x52aed0`), and an addon's `## SavedVariables` / `## SavedVariablesPerCharacter` to its own two
//! files; each runs as a chunk at load. The app owns the files (`ui_saved`).
//!
//! The grammar is `FrameScript_PrintVariable`'s (`0x818b50`, `PrintVariable` `0x8183a0`): CRLF, each
//! variable written as `\r\n` + `NAME = ` + value and one `\r\n` after the last, so a file opens on a
//! blank line and holds just `\r\n` when nothing is registered; numbers as `%.16g`; strings with
//! `\000`, `\n`, `\r`, `\"` and `\\` escaped and every other byte raw; tables as `{`, the array part
//! `1..#t` as `value, -- [i]` (a hole `nil`), then every other key bracketed, `[key] = value,`, in
//! the table's own walk order, tab-indented, and `}`; a value with no literal written as a
//! `nil --[[ skipped … ]]` comment.

use std::collections::HashSet;
use std::ffi::c_void;

use mlua::{Lua, Table, Value};

/// Deviation: nesting stops at 32 (the table writes `nil`) where the reference recurses uncapped
/// (its indent saturates at `0x80`), so a hostile structure cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

/// `%.16g` (`0xa44840`) as 12340's statically linked VS2005 CRT prints it (`__vsnprintf`
/// `0x88bc83`): 16 significant digits, trailing zeros dropped, and an exponent of at least three
/// digits when the exponent form is taken (below -4 or from 16 up).
pub fn format_g16(n: f64) -> String {
    if n == 0.0 {
        return if n.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let e = format!("{n:.15e}");
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..16).contains(&exp) {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{m}e{sign}{:03}", exp.abs());
    }
    let fixed = format!("{:.*}", (15 - exp).max(0) as usize, n);
    if fixed.contains('.') {
        fixed
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        fixed
    }
}

impl super::UiScript {
    /// Hold a saved-variables file that failed to load, so the shutdown write leaves it alone.
    /// Deviation: the reference rewrites it from the live defaults, which would cost the player the
    /// whole file for one typo in a hand edit.
    pub fn hold_saved_file(&self, path: &std::path::Path) {
        let mut model = self.model_mut();
        if !model.held_saved_files.iter().any(|p| p == path) {
            model.held_saved_files.push(path.to_path_buf());
        }
    }

    /// Whether this session's load failed on `path`.
    pub fn saved_file_held(&self, path: &std::path::Path) -> bool {
        self.model_ref().held_saved_files.iter().any(|p| p == path)
    }

    /// The `RegisterForSave` names in registration order, the reference's own write order.
    pub fn saved_variable_names(&self) -> Vec<String> {
        self.model_mut().saved_names.clone()
    }

    /// The `RegisterForSavePerCharacter` names in registration order.
    pub fn saved_variable_names_per_character(&self) -> Vec<String> {
        self.model_mut().saved_names_per_character.clone()
    }

    /// The account's flat file, from the `RegisterForSave` globals.
    pub fn saved_variables_bytes(&self) -> Vec<u8> {
        self.saved_variables_bytes_for(&self.saved_variable_names())
    }

    /// One saved-variables file over an explicit name list, in bytes, since a Lua string is bytes
    /// and the reference writes them raw.
    pub fn saved_variables_bytes_for(&self, names: &[String]) -> Vec<u8> {
        let mut out = Vec::new();
        for name in names {
            let value: Value = self
                .lua()
                .globals()
                .raw_get(name.as_str())
                .unwrap_or(Value::Nil);
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b" = ");
            let mut seen = HashSet::new();
            print_value(&mut out, &value, 1, &mut seen);
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

/// `PrintVariable`: one value at table depth `depth` (1 at top level).
fn print_value(out: &mut Vec<u8>, v: &Value, depth: usize, seen: &mut HashSet<*const c_void>) {
    match v {
        Value::Nil => out.extend_from_slice(b"nil"),
        Value::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Integer(i) => out.extend_from_slice(format_g16(*i as f64).as_bytes()),
        Value::Number(n) => print_number(out, *n),
        Value::String(s) => quote(out, &s.as_bytes()),
        Value::Table(t) => print_table(out, t, depth, seen),
        Value::Function(_) => out.extend_from_slice(b"nil --[[ skipped inline function ]]"),
        Value::Thread(_) => out.extend_from_slice(b"nil --[[ skipped thread ]]"),
        _ => out.extend_from_slice(b"nil --[[ skipped userdata ]]"),
    }
}

fn print_number(out: &mut Vec<u8>, n: f64) {
    if n.is_nan() {
        out.extend_from_slice(b"nil --[[ nan ]]");
    } else if n.is_infinite() {
        out.extend_from_slice(b"nil --[[ inf ]]");
    } else {
        out.extend_from_slice(format_g16(n).as_bytes());
    }
}

/// A quoted string: `\0`, `\n`, `\r`, `"` and `\` escaped, every other byte raw.
fn quote(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for &b in s {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0 => out.extend_from_slice(b"\\000"),
            b => out.push(b),
        }
    }
    out.push(b'"');
}

/// A table: an ancestor of itself is skipped, the array part `1..#t` comes first, then every
/// other key in the walk's order (`lua_next`, as the reference walks it).
fn print_table(out: &mut Vec<u8>, t: &Table, depth: usize, seen: &mut HashSet<*const c_void>) {
    if depth > MAX_DEPTH {
        out.extend_from_slice(b"nil");
        return;
    }
    if !seen.insert(t.to_pointer()) {
        out.extend_from_slice(b"nil --[[ skipped recursive table ]]");
        return;
    }
    let indent = "\t".repeat(depth);
    out.extend_from_slice(b"{\r\n");
    let n = t.raw_len();
    for i in 1..=n {
        out.extend_from_slice(indent.as_bytes());
        let v: Value = t.raw_get(i).unwrap_or(Value::Nil);
        print_value(out, &v, depth + 1, seen);
        out.extend_from_slice(format!(", -- [{i}]\r\n").as_bytes());
    }
    for pair in t.pairs::<Value, Value>() {
        let Ok((k, v)) = pair else { continue };
        let number = match &k {
            Value::Integer(i) => Some(*i as f64),
            Value::Number(x) => Some(*x),
            _ => None,
        };
        if let Some(x) = number {
            if x.is_nan() || x.is_infinite() {
                let which = if x.is_nan() { "nan" } else { "inf" };
                out.extend_from_slice(
                    format!("{indent}--[[ skipped entry with {which} key ]]\r\n").as_bytes(),
                );
                continue;
            }
            if n > 0 && x.fract() == 0.0 && x >= 1.0 && x <= n as f64 {
                continue; // written in the array part
            }
        }
        if !matches!(
            k,
            Value::Integer(_) | Value::Number(_) | Value::String(_) | Value::Boolean(_)
        ) {
            let kind = match k {
                Value::Table(_) => "table",
                Value::Function(_) => "function",
                Value::Thread(_) => "thread",
                _ => "userdata",
            };
            out.extend_from_slice(
                format!("{indent}--[[ skipped entry with {kind} key ]]\r\n").as_bytes(),
            );
            continue;
        }
        out.extend_from_slice(indent.as_bytes());
        out.push(b'[');
        print_value(out, &k, depth + 1, seen);
        out.extend_from_slice(b"] = ");
        print_value(out, &v, depth + 1, seen);
        out.extend_from_slice(b",\r\n");
    }
    seen.remove(&t.to_pointer());
    out.extend_from_slice("\t".repeat(depth - 1).as_bytes());
    out.push(b'}');
}

/// Register `RegisterForSave` and `RegisterForSavePerCharacter` (`0x52b470`, `0x52b4e0`), with no
/// taint gate: the reference refuses them outside Blizzard code, and benilla has no taint model.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "RegisterForSave",
        lua.create_function(|lua, name: String| {
            let mut model = crate::script::model_mut_of(lua);
            if !model.saved_names.contains(&name) {
                model.saved_names.push(name);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "RegisterForSavePerCharacter",
        lua.create_function(|lua, name: String| {
            let mut model = crate::script::model_mut_of(lua);
            if !model.saved_names_per_character.contains(&name) {
                model.saved_names_per_character.push(name);
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::format_g16;
    use crate::script::UiScript;

    /// Bagnon's shape: a saved bag list sorted with the keyring (-2) last, laid out by `pairs`.
    #[test]
    fn a_saved_list_still_walks_in_index_order_after_a_restart() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            BAGS = { -2, 0, 1, 2, 3, 4 }
            table.sort(BAGS, function(a, b)
                if a == -2 then return false
                elseif b == -2 then return true
                else return a < b end
            end)
            RegisterForSave("BAGS")
        "#,
        )
        .unwrap();
        let walk = |vm: &UiScript| {
            vm.eval::<String>(
                "local out = '' for _, v in pairs(BAGS) do out = out .. v .. ',' end return out",
            )
            .unwrap()
        };
        assert_eq!(walk(&s), "0,1,2,3,4,-2,", "the live table, before any save");

        let text = s.saved_variables_bytes();
        let fresh = UiScript::new().unwrap();
        fresh.run_chunk(&text).unwrap();
        assert_eq!(
            walk(&fresh),
            "0,1,2,3,4,-2,",
            "the restart must walk the same order the live session did; wrote:\n{}",
            String::from_utf8_lossy(&text)
        );
    }

    /// The array part is `1..#t`; every other key is bracketed, in the table's walk order.
    #[test]
    fn the_array_part_is_written_as_list_items_and_the_rest_bracketed() {
        let s = UiScript::new().unwrap();
        s.run(r#"T = { "a", "b", [4] = "d", [0] = "z", [-1] = "m" } RegisterForSave("T")"#)
            .unwrap();
        let text = String::from_utf8(s.saved_variables_bytes()).unwrap();
        assert!(
            text.starts_with("\r\nT = {\r\n\t\"a\", -- [1]\r\n\t\"b\", -- [2]\r\n"),
            "{text:?}"
        );
        for line in [
            "\t[4] = \"d\",\r\n",
            "\t[0] = \"z\",\r\n",
            "\t[-1] = \"m\",\r\n",
        ] {
            assert!(text.contains(line), "{line:?} in {text:?}");
        }
        assert!(text.ends_with(",\r\n}\r\n"), "{text:?}");
    }

    #[test]
    fn the_declared_globals_round_trip_through_a_fresh_vm() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            TRAINER_FILTER_AVAILABLE = 1
            TRAINER_FILTER_UNAVAILABLE = 0
            NEVER_TOUCHED = nil
            CHAT_LABEL = "say \"hi\"\nnow"
            OPACITY = 0.5
            SPELLBOOK_PAGENUMBERS = { [2] = 3, [1] = 1, ["odd key"] = { [1] = true } }
            RegisterForSave("TRAINER_FILTER_AVAILABLE")
            RegisterForSave("TRAINER_FILTER_UNAVAILABLE")
            RegisterForSave("NEVER_TOUCHED")
            RegisterForSave("CHAT_LABEL")
            RegisterForSave("OPACITY")
            RegisterForSave("SPELLBOOK_PAGENUMBERS")
            RegisterForSave("TRAINER_FILTER_AVAILABLE")
        "#,
        )
        .unwrap();
        assert_eq!(
            s.saved_variable_names(),
            vec![
                "TRAINER_FILTER_AVAILABLE",
                "TRAINER_FILTER_UNAVAILABLE",
                "NEVER_TOUCHED",
                "CHAT_LABEL",
                "OPACITY",
                "SPELLBOOK_PAGENUMBERS",
            ],
            "registration order, and a re-register is not a second entry"
        );
        let text = String::from_utf8(s.saved_variables_bytes()).expect("every value is UTF-8");
        assert_eq!(
            text,
            "\r\nTRAINER_FILTER_AVAILABLE = 1\
             \r\nTRAINER_FILTER_UNAVAILABLE = 0\
             \r\nNEVER_TOUCHED = nil\
             \r\nCHAT_LABEL = \"say \\\"hi\\\"\\nnow\"\
             \r\nOPACITY = 0.5\
             \r\nSPELLBOOK_PAGENUMBERS = {\r\n\
             \t1, -- [1]\r\n\
             \t3, -- [2]\r\n\
             \t[\"odd key\"] = {\r\n\
             \t\ttrue, -- [1]\r\n\
             \t},\r\n\
             }\r\n",
            "got:\n{text}"
        );

        let fresh = UiScript::new().unwrap();
        fresh.run(&text).unwrap();
        assert_eq!(
            fresh
                .eval::<i64>("return TRAINER_FILTER_AVAILABLE + TRAINER_FILTER_UNAVAILABLE")
                .unwrap(),
            1
        );
        assert!(fresh.eval::<bool>("return NEVER_TOUCHED == nil").unwrap());
        assert_eq!(
            fresh.eval::<String>("return CHAT_LABEL").unwrap(),
            "say \"hi\"\nnow"
        );
        assert_eq!(fresh.eval::<f64>("return OPACITY").unwrap(), 0.5);
        assert_eq!(
            fresh
                .eval::<i64>("return SPELLBOOK_PAGENUMBERS[2]")
                .unwrap(),
            3
        );
        assert!(fresh
            .eval::<bool>("return SPELLBOOK_PAGENUMBERS['odd key'][1]")
            .unwrap());
    }

    #[test]
    fn nothing_registered_is_one_line_end() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.saved_variables_bytes(), b"\r\n");
    }

    #[test]
    fn a_value_with_no_literal_is_written_as_a_skip_comment() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            A_FUNCTION = function() end
            INF = 1/0
            NAN = 0/0
            KEPT = 7
            CYCLE = {}
            CYCLE.self = CYCLE
            BAD_KEY = { [{}] = 1 }
            RegisterForSave("A_FUNCTION")
            RegisterForSave("INF")
            RegisterForSave("NAN")
            RegisterForSave("KEPT")
            RegisterForSave("CYCLE")
            RegisterForSave("BAD_KEY")
        "#,
        )
        .unwrap();
        let text = s.saved_variables_bytes();
        assert_eq!(
            String::from_utf8_lossy(&text),
            "\r\nA_FUNCTION = nil --[[ skipped inline function ]]\
             \r\nINF = nil --[[ inf ]]\
             \r\nNAN = nil --[[ nan ]]\
             \r\nKEPT = 7\
             \r\nCYCLE = {\r\n\t[\"self\"] = nil --[[ skipped recursive table ]],\r\n}\
             \r\nBAD_KEY = {\r\n\t--[[ skipped entry with table key ]]\r\n}\
             \r\n"
        );
        UiScript::new().unwrap().run_chunk(&text).unwrap();
    }

    #[test]
    fn a_shared_table_is_written_at_each_place() {
        let s = UiScript::new().unwrap();
        s.run(r#"local t = { 1 } PAIR = { t, t } RegisterForSave("PAIR")"#)
            .unwrap();
        assert_eq!(
            s.saved_variables_bytes(),
            b"\r\nPAIR = {\r\n\t{\r\n\t\t1, -- [1]\r\n\t}, -- [1]\r\n\t{\r\n\t\t1, -- [1]\r\n\t}, -- [2]\r\n}\r\n"
        );
    }

    #[test]
    fn the_per_character_names_are_their_own_list() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            A = 1 C = 2
            RegisterForSave("A")
            RegisterForSavePerCharacter("C")
            RegisterForSavePerCharacter("C")
        "#,
        )
        .unwrap();
        assert_eq!(s.saved_variable_names(), ["A"]);
        assert_eq!(s.saved_variable_names_per_character(), ["C"]);
        assert_eq!(
            s.saved_variables_bytes_for(&s.saved_variable_names_per_character()),
            b"\r\nC = 2\r\n"
        );
    }

    #[test]
    fn numbers_print_as_the_reference_crts_percent_16g() {
        assert_eq!(format_g16(0.7f32 as f64), "0.699999988079071");
        assert_eq!(format_g16(0.1), "0.1");
        assert_eq!(format_g16(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(format_g16(100.0), "100");
        assert_eq!(format_g16(-0.0), "-0");
        assert_eq!(format_g16(0.0001), "0.0001");
        assert_eq!(format_g16(0.00001), "1e-005");
        assert_eq!(format_g16(1e15), "1000000000000000");
        assert_eq!(format_g16(1e16), "1e+016");
        assert_eq!(format_g16(123456789012345678.0), "1.234567890123457e+017");
        assert_eq!(format_g16(-2.5e-300), "-2.5e-300");
    }

    #[test]
    fn a_byte_string_that_is_not_utf8_round_trips_unchanged() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            PACKED = string.char(65, 200, 255, 0, 66, 13)
            KEYS = { [string.char(200)] = 1, [string.char(201)] = 2 }
            RegisterForSave("PACKED")
            RegisterForSave("KEYS")
        "#,
        )
        .unwrap();
        let fresh = UiScript::new().unwrap();
        fresh.run_chunk(&s.saved_variables_bytes()).unwrap();
        assert!(fresh
            .eval::<bool>(
                "return string.len(PACKED) == 6 and string.byte(PACKED, 2) == 200 \
                 and string.byte(PACKED, 3) == 255 and string.byte(PACKED, 4) == 0 \
                 and string.byte(PACKED, 6) == 13"
            )
            .unwrap());
        assert!(fresh
            .eval::<bool>("return KEYS[string.char(200)] == 1 and KEYS[string.char(201)] == 2")
            .unwrap());
    }
}
