//! `GetText(token [, gender] [, ordinal])` (`0x81b720`): a global string by token, with the
//! plural and gender variants tried first (`FrameScript_GetText`, `0x819d40`).

use mlua::{Lua, Value};

/// `GENDER_FEMALE`, the one gender with a suffix of its own.
const FEMALE: i64 = 3;

/// `FrameScript_GetPluralIndex` (`0x817d10`) under rule 0, the one enUS sets: 1 for any count
/// but one, the -1 an absent ordinal passes included.
fn plural_index(count: i64) -> i64 {
    i64::from(count != 1)
}

/// The global `name` when it is a string or a number (`lua_isstring`, `0x818010`).
fn string_global(lua: &Lua, name: &str) -> Option<mlua::String> {
    match lua.globals().raw_get::<Value>(name).ok()? {
        Value::String(s) => Some(s),
        Value::Integer(n) => lua.create_string(n.to_string()).ok(),
        Value::Number(n) => lua.create_string(n.to_string()).ok(),
        _ => None,
    }
}

/// `FrameScript_GetText`: token, plural and gender suffixes, then token and plural, token and
/// gender, the bare token, and "" when none is a string.
pub(super) fn get_text(
    lua: &Lua,
    token: &str,
    ordinal: i64,
    gender: i64,
) -> mlua::Result<mlua::String> {
    let plural = match plural_index(ordinal) {
        p @ 1..=9 => format!("_P{p}"),
        _ => String::new(),
    };
    let gender = if gender == FEMALE { "_FEMALE" } else { "" };
    [
        format!("{token}{plural}{gender}"),
        format!("{token}{plural}"),
        format!("{token}{gender}"),
        token.to_string(),
    ]
    .iter()
    .find_map(|name| string_global(lua, name))
    .map_or_else(|| lua.create_string(""), Ok)
}

/// `FrameScript_GetText(token, -1, 0)` for a binding holding `&Lua`.
pub(super) fn global_text(lua: &Lua, token: &str) -> String {
    get_text(lua, token, -1, 0)
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
}

impl super::UiScript {
    /// `FrameScript_GetText(token, -1, 0)`, the lookup the engine makes for a line it prints
    /// itself: `token_P1` first, then the bare token, else "".
    pub fn engine_text(&self, token: &str) -> String {
        get_text(&self.lua, token, -1, 0)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetText",
        lua.create_function(|lua, (token, gender, ordinal): (Value, Value, Value)| {
            let token = match &token {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(n) => n.to_string(),
                Value::Number(n) => n.to_string(),
                _ => {
                    return Err(mlua::Error::runtime(
                        "Usage: GetText(\"token\" [,gender] [,ordinal])",
                    ))
                }
            };
            let int = |v: &Value| match v {
                Value::Integer(n) => Some(*n),
                Value::Number(n) => Some(*n as i64),
                Value::String(s) => s
                    .to_str()
                    .ok()?
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|n| n as i64),
                _ => None,
            };
            get_text(
                lua,
                &token,
                int(&ordinal).unwrap_or(-1),
                int(&gender).unwrap_or(0),
            )
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_variants_are_tried_plural_and_gender_first() {
        let s = UiScript::new().expect("VM");
        s.run(
            "TOKEN = 'plain' TOKEN_FEMALE = 'female' TOKEN_P1 = 'plural' \
             ONLY = 'only' ONLY_FEMALE = 'only female' NUM = 5",
        )
        .unwrap();
        let got = |e: &str| s.eval::<String>(&format!("return {e}")).unwrap();
        assert_eq!(
            got("GetText('TOKEN')"),
            "plural",
            "no ordinal counts as plural"
        );
        assert_eq!(got("GetText('TOKEN', 2, 1)"), "plain");
        assert_eq!(got("GetText('TOKEN', 3, 1)"), "female");
        assert_eq!(got("GetText('ONLY')"), "only");
        assert_eq!(got("GetText('ONLY', 3)"), "only female");
        assert_eq!(got("GetText('NUM')"), "5");
        assert_eq!(got("GetText('MISSING')"), "");
        assert!(s.run("GetText()").is_err(), "a token is required");
    }
}
