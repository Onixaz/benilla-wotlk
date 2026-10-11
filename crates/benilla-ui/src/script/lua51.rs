//! 3.3.5a's library surface over stock Lua 5.1: the base-library string helpers and the bare
//! aliases its built-in compat chunk publishes (`Wow.exe` `0x643180`).
//!
//! The helpers are C bindings in the FrameScript base library: `strtrim` (`0x816960`),
//! `strsplit` (`0x816a60`), `strjoin` (`0x816b60`) and `strconcat` (`0x816d80`), and the table
//! library's `wipe` (`0x852180`). The compat chunk binds `foreach`, `foreachi`, `getn`, `tinsert`,
//! `tremove`, `sort`, `wipe`, the degree-based trigonometry and the math names, `format`,
//! `gmatch`, `strmatch`, `strrev` and the 1.12.1 string names, with `mod` as `math.fmod`; it
//! leaves `randomseed` out, and adds `trim`, `split` and `join` to the string table. Only what
//! [`super::stdlib`]'s shared layer does not already bind is set here. `strreplace` is not built.

use mlua::{Lua, MultiValue, Table, Value, Variadic};

/// Install the layer; runs after [`super::stdlib::install`].
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `strtrim(str [, chars])`: `chars` defaults to `" \t\r\n"` (`0xa16860`); each end loses
    // every byte in the set.
    g.set(
        "strtrim",
        lua.create_function(|lua, (s, chars): (mlua::String, Option<mlua::String>)| {
            let bytes = s.as_bytes();
            let set: Vec<u8> = match &chars {
                Some(c) => c.as_bytes().to_vec(),
                None => b" \t\r\n".to_vec(),
            };
            let mut start = 0;
            let mut end = bytes.len();
            while start < end && set.contains(&bytes[start]) {
                start += 1;
            }
            while end > start && set.contains(&bytes[end - 1]) {
                end -= 1;
            }
            lua.create_string(&bytes[start..end])
        })?,
    )?;

    // `strsplit(delimiters, str [, pieces])`: split at every byte in `delimiters`, empty pieces
    // kept; with `pieces` > 0 the last piece is the unsplit remainder. `pieces` of 1 answers the
    // whole string.
    g.set(
        "strsplit",
        lua.create_function(
            |lua, (delims, s, pieces): (mlua::String, mlua::String, Option<i64>)| {
                let delims = delims.as_bytes().to_vec();
                let bytes = s.as_bytes();
                let pieces = pieces.unwrap_or(0);
                let mut out = Vec::new();
                let mut start = 0;
                if pieces == 0 || pieces > 1 {
                    for (i, b) in bytes.iter().enumerate() {
                        if delims.contains(b) {
                            out.push(Value::String(lua.create_string(&bytes[start..i])?));
                            start = i + 1;
                            if pieces > 0 && out.len() as i64 == pieces - 1 {
                                break;
                            }
                        }
                    }
                }
                out.push(Value::String(lua.create_string(&bytes[start..])?));
                Ok(MultiValue::from_vec(out))
            },
        )?,
    )?;

    // `strjoin(separator, ...)`: the pieces with `separator` between them, through `lua_concat`,
    // so a non-string, non-number piece raises; no pieces is `""`.
    g.set(
        "strjoin",
        lua.create_function(|lua, (sep, pieces): (mlua::String, Variadic<Value>)| {
            let mut out = Vec::new();
            for (i, piece) in pieces.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(&sep.as_bytes());
                }
                out.extend_from_slice(&concat_piece(lua, piece)?);
            }
            lua.create_string(&out)
        })?,
    )?;

    // `strconcat(...)`: `lua_concat` of every argument.
    g.set(
        "strconcat",
        lua.create_function(|lua, pieces: Variadic<Value>| {
            let mut out = Vec::new();
            for piece in pieces.iter() {
                out.extend_from_slice(&concat_piece(lua, piece)?);
            }
            lua.create_string(&out)
        })?,
    )?;

    // `table.wipe(t)`: every key cleared, the table returned.
    let table: Table = g.get("table")?;
    let wipe = lua.create_function(|_, t: Table| {
        let keys: Vec<Value> = t
            .pairs::<Value, Value>()
            .map(|kv| kv.map(|(k, _)| k))
            .collect::<mlua::Result<_>>()?;
        for k in keys {
            t.raw_set(k, Value::Nil)?;
        }
        Ok(t)
    })?;
    table.set("wipe", wipe.clone())?;
    g.set("wipe", wipe)?;

    // The compat chunk's names the shared layer leaves out.
    for (name, lib, member) in [
        ("foreach", "table", "foreach"),
        ("foreachi", "table", "foreachi"),
        ("getn", "table", "getn"),
        ("sort", "table", "sort"),
        ("mod", "math", "fmod"),
        ("format", "string", "format"),
        ("gmatch", "string", "gmatch"),
        ("strmatch", "string", "match"),
        ("strrev", "string", "reverse"),
    ] {
        let lib: Table = g.get(lib)?;
        g.set(name, lib.get::<Value>(member)?)?;
    }
    g.set("randomseed", Value::Nil)?;
    let string: Table = g.get("string")?;
    for (member, global) in [
        ("trim", "strtrim"),
        ("split", "strsplit"),
        ("join", "strjoin"),
    ] {
        string.set(member, g.get::<Value>(global)?)?;
    }
    Ok(())
}

/// One `lua_concat` operand: a string as is, a number in Lua's `%.14g`, else the raise.
fn concat_piece(lua: &Lua, v: &Value) -> mlua::Result<Vec<u8>> {
    match v {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Integer(_) | Value::Number(_) => Ok(lua
            .coerce_string(v.clone())?
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default()),
        other => Err(mlua::Error::runtime(format!(
            "attempt to concatenate a {} value",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

    fn wotlk() -> UiScript {
        UiScript::new().unwrap()
    }

    #[test]
    fn strtrim_strips_the_default_set_or_the_given_one() {
        let ui = wotlk();
        assert_eq!(
            ui.eval::<String>("return strtrim(' \\t hi \\r\\n')")
                .unwrap(),
            "hi"
        );
        assert_eq!(
            ui.eval::<String>("return strtrim('xxhixx', 'x')").unwrap(),
            "hi"
        );
        assert_eq!(
            ui.eval::<String>("return ('  a b  '):trim()").unwrap(),
            "a b"
        );
    }

    #[test]
    fn strsplit_keeps_empty_pieces_and_honours_the_limit() {
        let ui = wotlk();
        assert_eq!(
            ui.eval::<(String, String, String)>("return strsplit(',', 'a,,b')")
                .unwrap(),
            ("a".into(), String::new(), "b".into())
        );
        assert_eq!(
            ui.eval::<(String, String)>("return strsplit(' ', '/join General now', 2)")
                .unwrap(),
            ("/join".into(), "General now".into())
        );
        assert_eq!(
            ui.eval::<i64>("return select('#', strsplit(',', ''))")
                .unwrap(),
            1
        );
        assert_eq!(
            ui.eval::<String>("return strsplit(',', 'a,b', 1)").unwrap(),
            "a,b"
        );
    }

    #[test]
    fn strjoin_and_strconcat_concatenate() {
        let ui = wotlk();
        assert_eq!(
            ui.eval::<String>("return strjoin(', ', 'a', 1, 'b')")
                .unwrap(),
            "a, 1, b"
        );
        assert_eq!(ui.eval::<String>("return strjoin('-')").unwrap(), "");
        assert_eq!(
            ui.eval::<String>("return strconcat('a', 2, 'c')").unwrap(),
            "a2c"
        );
        assert!(ui.run("strjoin(',', {})").is_err());
    }

    #[test]
    fn the_compat_names_are_bound_and_randomseed_is_not() {
        let ui = wotlk();
        assert!(ui
            .eval::<bool>(
                "return strmatch == string.match and gmatch == string.gmatch \
                 and strrev == string.reverse and mod == math.fmod and getn == table.getn \
                 and wipe == table.wipe and randomseed == nil"
            )
            .unwrap());
        assert!(ui
            .eval::<bool>("local t = wipe({1, 2, x = 3}) return next(t) == nil")
            .unwrap());
    }
}
