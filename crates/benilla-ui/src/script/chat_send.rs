//! `SendChatMessage(text [, chatType [, language [, target]]])`: the engine verb an addon speaks
//! through, with no FrameXML between it and the wire. `chatType` defaults to `"SAY"`; the fourth
//! argument is the whisper target or the channel's number.
//!
//! It hands a [`ChatSend`] to the host inside the call, never through the edit box's slash
//! grammar: an addon's `SendChatMessage("/dance")` says the characters. The reference splits the
//! same way, `ChatEdit_SendText` parsing and `SendChatMessage` not.
//!
//! `language` starts as the speaker's race base language. A string or number third argument is
//! matched by name, case-insensitively, against every `Languages.dbc` row, known to the character
//! or not, and a miss raises `Unknown language`: a number is its text, so `7` raises. A tongue the
//! character does not speak is the host's refusal, inside the call (`0x50d2b0`).
//!
//! The host's line passes cut the text at 255 bytes, its parse packing into 256 (`0x50d69f`,
//! `0x577d65`).

use mlua::{Lua, MultiValue, Value};

impl super::UiScript {
    /// Push the name `GetDefaultLanguage()` answers. `None` is the reference's no-player state,
    /// which returns zero values, not `nil`; the app resolves it per world entry from
    /// `ChrRaces.BaseLanguage` and `Languages.dbc`.
    pub fn set_default_language(&mut self, name: Option<String>) {
        set_field!(self, default_language, name);
    }
}

/// One queued `SendChatMessage` line, plain data: this crate holds no wire types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSend {
    /// The line's bytes, verbatim: a leading `/` is text, not a command, and the bytes need not be
    /// valid UTF-8 (the walk at `0x47bd40` passes overlong and cut forms).
    pub text: Vec<u8>,
    /// The chat type token (`"SAY"`, `"WHISPER"`, `"CHANNEL"`, …), uppercased, since the
    /// reference's compare ignores case.
    pub chat_type: String,
    /// The fourth argument as `lua_tostring` gives it: the whisper target, or the channel number
    /// the app resolves to a joined channel's name.
    pub target: Option<String>,
    /// The `Languages.dbc` id the third argument named; `None` is the speaker's default.
    pub language: Option<u32>,
}

impl ChatSend {
    /// The empty-line gate: a line whose first byte is NUL ends the call, before the language is
    /// read or anything is sent, unless its type is AFK (`0x17`) or DND (`0x18`). The chat-type
    /// lookup and its "Unknown chat type" come first.
    pub fn ends_at_empty_line(&self) -> bool {
        ends_at_empty_line(&self.text, &self.chat_type)
    }
}

/// The chat-type names `SendChatMessage` accepts, compared ignoring case (`0x4fbb20`): no
/// `RAID_LEADER` or `BATTLEGROUND_LEADER`, which the verb picks itself for a leader.
pub const CHAT_TYPES: [&str; 17] = [
    "SAY",
    "PARTY",
    "RAID",
    "GUILD",
    "OFFICER",
    "YELL",
    "WHISPER",
    "EMOTE",
    "CHANNEL",
    "AFK",
    "DND",
    "RAID_WARNING",
    "BATTLEGROUND",
    "BN",
    "BN_WHISPER",
    "BN_WHISPER_INFORM",
    "BN_CONVERSATION",
];

/// Whether an uppercased token names a [`CHAT_TYPES`] row.
fn chat_type_known(token: &str) -> bool {
    CHAT_TYPES.contains(&token)
}

/// [`ChatSend::ends_at_empty_line`] on the binding's arguments, before the line is built.
fn ends_at_empty_line(text: &[u8], chat_type: &str) -> bool {
    text.first().copied().unwrap_or(0) == 0 && !matches!(chat_type, "AFK" | "DND")
}

/// `SendChatMessage`'s escape walk (`0x50d6ff`-`0x50d7ae`): whether the line passes. After each
/// `|`: `||` skips both bytes; `|c` skips past the first later `|r` (a case-sensitive search),
/// leaving the run between unchecked; `|c` with no `|r` cuts `text` at the `|` and passes; anything
/// else, the NUL after a trailing `|` included, fails.
pub fn chat_escapes_valid(text: &mut Vec<u8>) -> bool {
    let mut i = 0usize;
    while let Some(off) = text[i..].iter().position(|&b| b == b'|') {
        let pipe = i + off;
        match text.get(pipe + 1) {
            Some(b'|') => i = pipe + 2,
            Some(b'c') => {
                // The search starts at the `|c`, which cannot itself match `|r`.
                match text[pipe..]
                    .windows(2)
                    .position(|w| w == b"|r")
                    .map(|p| pipe + p)
                {
                    // Past the closer: its `|` must not reach the loop head, where `|r` fails.
                    Some(closer) => i = closer + 2,
                    None => {
                        text.truncate(pipe);
                        return true;
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// The UTF-8 test (`0x7e0ec0`), walking the line with the decoder (`0x47bd40`): a lead byte
/// `10xxxxxx`, `0xFE` or `0xFF` (`0x47be35`), or a trail byte not `10xxxxxx` (`0x47be0b`), fails;
/// a NUL, one that cuts a sequence included (`0x47bdfa`), ends the walk valid. Forms of up to six
/// bytes and overlong ones pass.
fn utf8_valid(bytes: &[u8]) -> bool {
    let mut at = 0;
    loop {
        let lead = bytes.get(at).copied().unwrap_or(0);
        if lead == 0 {
            return true;
        }
        at += 1;
        let extra = if lead & 0xFE == 0xFC {
            5
        } else if lead & 0xFC == 0xF8 {
            4
        } else if lead & 0xF8 == 0xF0 {
            3
        } else if lead & 0xF0 == 0xE0 {
            2
        } else if lead & 0xE0 == 0xC0 {
            1
        } else if lead & 0x80 != 0 {
            return false;
        } else {
            0
        };
        for _ in 0..extra {
            let next = bytes.get(at).copied().unwrap_or(0);
            if next == 0 {
                return true;
            }
            at += 1;
            if next & 0xC0 != 0x80 {
                return false;
            }
        }
    }
}

impl super::UiScript {
    /// Push `Languages.dbc` as `(ID, Name_lang)` rows in file order, the table `SendChatMessage`
    /// matches its language name against.
    pub fn set_language_table(&mut self, rows: Vec<(u32, String)>) {
        set_field!(self, language_table, rows);
    }
}

/// The first row, in file order, whose name equals `name` ignoring case (a case-insensitive compare
/// of unbounded length), checked against every row, not only the known languages.
fn language_id(table: &[(u32, String)], name: &str) -> Option<u32> {
    table
        .iter()
        .find(|(_, row)| row.eq_ignore_ascii_case(name))
        .map(|&(id, _)| id)
}

impl super::UiScript {
    /// The languages this character knows, in `Languages.dbc` row order: a language counts when a
    /// known spell with `Effect_1 == 39` teaches it and its skill line is in the skill block. A
    /// change fires `LANGUAGE_LIST_CHANGED` (event 0x102).
    pub fn set_known_languages(&mut self, names: Vec<String>) {
        let changed = {
            let mut model = self.model_mut();
            if model.known_languages == names {
                false
            } else {
                model.known_languages = names;
                true
            }
        };
        if changed {
            self.fire_event("LANGUAGE_LIST_CHANGED", vec![]);
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetNumLanguages() (`0x500760`): one number, the known languages (`0x6e0640` over
    // `Languages.dbc`). 1.12.1 registered it misspelled, `GetNumLaguages`; 12340 has only this.
    lua.globals().set(
        "GetNumLanguages",
        lua.create_function(|lua, _ignored: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.known_languages.len() as i64)
        })?,
    )?;
    // GetLanguageByIndex(index) (`0x500810`): the `Name_lang` of the index-th known language,
    // positional, never a row number or a language id; past the end it pushes nothing. The index
    // is a number or `Usage:` (`0x500829`), truncated (`0x500889`).
    lua.globals().set(
        "GetLanguageByIndex",
        lua.create_function(|lua, index: Value| {
            let index = crate::script::binding_abi::number_arg(
                lua,
                index,
                "Usage: GetLanguageByIndex(index)",
            )?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let name = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.known_languages.get(i));
            Ok(match name {
                Some(n) => MultiValue::from_vec(vec![Value::String(lua.create_string(n)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;
    // GetDefaultLanguage(): one string, or zero values, never `nil`, on all four failure edges. It
    // reads no arguments, so an addon's `GetDefaultLanguage("player")` is ignored.
    lua.globals().set(
        "GetDefaultLanguage",
        lua.create_function(|lua, _ignored: MultiValue| {
            let name = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.default_language.clone()
            };
            Ok(match name {
                Some(n) => MultiValue::from_vec(vec![Value::String(lua.create_string(&n)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    lua.globals().set(
        "SendChatMessage",
        lua.create_function(
            |lua, (text, chat_type, language, target): (Value, Value, Value, Value)| {
                // The action gate with kind 10 passes every caller (`0x50d17b` -> `0x5191d9`, its
                // table row 3, `0x519247`); with no active player the call returns silently
                // (`0x50d18b`-`0x50d1ab`), before any argument is read.
                if super::calls::ask_host(lua, &super::HostAsk::ActivePlayer)
                    == Some(super::HostAnswer::Declined)
                {
                    return Ok(());
                }
                // A non-string line is the usage error (`0x50d889`).
                let Some(line) = lua.coerce_string(text)? else {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: SendChatMessage(text [,type] [,language] [,targetPlayer])".into(),
                    ));
                };
                let text = line.as_bytes().to_vec();
                // A string type is looked up (`0x50d203` -> `0x4fbb20`) and a miss is the error
                // (`0x50d20f`); any other value keeps SAY, all ahead of the language.
                let chat_type = super::binding_abi::optional_string(lua, &chat_type)
                    .map_or_else(|| "SAY".into(), |t| t.to_ascii_uppercase());
                if !chat_type_known(&chat_type) {
                    return Err(mlua::Error::RuntimeError(
                        "SendChatMessage(): Unknown chat type".into(),
                    ));
                }
                // An empty line of any type but AFK and DND ends the call (`0x50d229`-`0x50d231`).
                if ends_at_empty_line(&text, &chat_type) {
                    return Ok(());
                }
                // The line must walk as UTF-8 (`0x50d238` -> `0x7e0ec0`), else the error (`0x50d244`).
                if !utf8_valid(&text) {
                    return Err(mlua::Error::RuntimeError(
                        "SendChatMessage(): Chat message must be UTF-8 text".into(),
                    ));
                }
                // A string language is looked up and a miss is the error (`0x50d28b`, `0x50d297`).
                let language = match super::binding_abi::optional_string(lua, &language) {
                    Some(name) => {
                        let model = lua
                            .app_data_ref::<crate::script::ModelCell>()
                            .expect("model app_data");
                        let id = language_id(&model.language_table, &name);
                        Some(id.ok_or_else(|| {
                            mlua::Error::RuntimeError("SendChatMessage(): Unknown language".into())
                        })?)
                    }
                    None => None,
                };
                // `lua_isstring` then `lua_tostring`: a number is its text, so `2.7` reaches the
                // channel's string-to-int as "2.7", which reads 2.
                let target = super::binding_abi::optional_string(lua, &target);
                let send = ChatSend {
                    text,
                    chat_type,
                    target,
                    language,
                };
                // The rest of the verb runs on the host inside the call (`0x50d170`).
                match super::calls::ask_host(lua, &super::HostAsk::ChatSend(send.clone())) {
                    Some(super::HostAnswer::LuaError(msg)) => {
                        Err(mlua::Error::RuntimeError(msg.into()))
                    }
                    Some(_) => Ok(()),
                    // No host lent: the VM's own record, which only its tests read.
                    None => {
                        crate::script::model_mut_of(lua).chat_sends.push(send);
                        Ok(())
                    }
                }
            },
        )?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// Drain the lines `SendChatMessage` recorded with no host lent, which only the VM's tests make.
    pub fn take_chat_sends(&mut self) -> Vec<ChatSend> {
        drain_field!(self, chat_sends)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn send_chat_message_queues_a_line_without_parsing_it() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hello")"#).unwrap();
        s.run(r#"SendChatMessage("/dance", "say")"#).unwrap();
        assert_eq!(
            s.take_chat_sends(),
            vec![
                ChatSend {
                    text: "hello".into(),
                    chat_type: "SAY".into(),
                    target: None,
                    language: None,
                },
                ChatSend {
                    text: "/dance".into(),
                    chat_type: "SAY".into(),
                    target: None,
                    language: None,
                },
            ]
        );
        assert!(s.take_chat_sends().is_empty());
    }

    #[test]
    fn the_target_argument_is_its_lua_tostring_text() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hi", "WHISPER", nil, "Bob")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 1)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, "General")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 2.7)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, {})"#)
            .unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent[0].target.as_deref(), Some("Bob"));
        assert_eq!(sent[1].target.as_deref(), Some("1"));
        assert_eq!(sent[2].target.as_deref(), Some("General"));
        assert_eq!(
            sent[3].target.as_deref(),
            Some("2.7"),
            "not rounded: the app's string-to-int reads 2"
        );
        assert_eq!(sent[4].target, None, "a table is no string");
        assert_eq!(sent[1].chat_type, "CHANNEL");
    }

    /// Three `Languages.dbc` rows, in the shipped file's order.
    fn with_languages(s: &mut UiScript) {
        s.set_language_table(vec![
            (1, "Orcish".into()),
            (2, "Darnassian".into()),
            (7, "Common".into()),
        ]);
    }

    #[test]
    fn a_language_name_sends_its_languages_dbc_id() {
        let mut s = UiScript::new().unwrap();
        with_languages(&mut s);
        s.run(r#"SendChatMessage("hi", "SAY", "Darnassian")"#)
            .unwrap();
        s.run(r#"SendChatMessage("hi", "YELL", "dARNASSIAN")"#)
            .unwrap();
        s.run(r#"SendChatMessage("hi", "WHISPER", "orcish", "Bob")"#)
            .unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent[0].language, Some(2), "Darnassian's row ID");
        assert_eq!(sent[1].language, Some(2), "the compare ignores case");
        assert_eq!(
            sent[2].language,
            Some(1),
            "a language the character may not know is sent: the server refuses it"
        );
        assert_eq!(sent[2].target.as_deref(), Some("Bob"));
    }

    #[test]
    fn no_language_argument_speaks_the_default() {
        let mut s = UiScript::new().unwrap();
        with_languages(&mut s);
        s.run(r#"SendChatMessage("hi")"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", nil)"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", {})"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", true)"#).unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent.len(), 4);
        assert!(
            sent.iter().all(|c| c.language.is_none()),
            "not a string, so `lua_isstring` keeps the default: {sent:?}"
        );
    }

    #[test]
    fn an_unknown_language_raises_and_sends_nothing() {
        let mut s = UiScript::new().unwrap();
        with_languages(&mut s);
        for arg in [r#""Klingon""#, r#""""#, "7", r#""Darnassian ""#] {
            let err = s
                .run(&format!(r#"SendChatMessage("hi", "SAY", {arg})"#))
                .expect_err(&format!("{arg} names no row"));
            assert!(err.to_string().contains("Unknown language"), "{arg}: {err}");
        }
        assert!(s.take_chat_sends().is_empty());
        // AFK and DND pass the empty-line gate, so their language is read.
        assert!(s.run(r#"SendChatMessage("", "AFK", "Klingon")"#).is_err());
        // An empty line of any other type ends before the language is read.
        s.run(r#"SendChatMessage("", "SAY", "Klingon")"#).unwrap();
    }

    /// The count matters: an addon passes the result straight to `SendChatMessage` as `language`.
    #[test]
    fn get_default_language_is_one_string_or_zero_values() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetDefaultLanguage()").unwrap(),
            0,
            "no player object → ZERO values, not nil"
        );

        s.set_default_language(Some("Common".into()));
        assert_eq!(
            s.arity("GetDefaultLanguage()").unwrap(),
            1,
            "one value — not (name, id)"
        );
        assert_eq!(
            s.eval::<String>("return GetDefaultLanguage()").unwrap(),
            "Common"
        );
        // The binding reads no arguments; addons pass `"player"` anyway.
        assert_eq!(
            s.eval::<String>(r#"return GetDefaultLanguage("player")"#)
                .unwrap(),
            "Common"
        );
    }
}

#[cfg(test)]
mod language_tests {
    use crate::script::UiScript;

    #[test]
    fn the_language_walk_is_positional_over_the_known_rows() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumLanguages()").unwrap(), 0);
        assert_eq!(
            s.arity("GetLanguageByIndex(1)").unwrap(),
            0,
            "past the end pushes nothing, not nil"
        );
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('LANGUAGE_LIST_CHANGED') \
             f:SetScript('OnEvent', function() FIRED = (FIRED or 0) + 1 end)",
        )
        .unwrap();
        s.set_known_languages(vec!["Orcish".into(), "Common".into()]);
        s.set_known_languages(vec!["Orcish".into(), "Common".into()]);
        assert_eq!(s.eval::<i64>("return GetNumLanguages()").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return GetLanguageByIndex(2)").unwrap(),
            "Common"
        );
        assert_eq!(
            s.eval::<String>("return GetLanguageByIndex(1.9)").unwrap(),
            "Orcish",
            "truncated (0x500889)"
        );
        let err = s.run("GetLanguageByIndex()").unwrap_err().to_string();
        assert!(err.contains("Usage: GetLanguageByIndex(index)"), "{err}");
        assert_eq!(
            s.eval::<i64>("return FIRED").unwrap(),
            1,
            "the list changing fires once; the same list again is silent"
        );
    }

    /// The escape walk (`0x50d6ff`-`0x50d7ae`): colour runs, links inside them and even pipe runs
    /// pass; any other escape, a bare closer and a trailing pipe fail; an unclosed `|c` cuts.
    #[test]
    fn the_escape_walk_passes_runs_and_cuts_an_unclosed_colour() {
        for ok in [
            "|cffff0000red|r",
            "before |cffff0000red|r after",
            "|cffAA0000 a |cff00BB00 b |r",
            "|cffa335ee|Hitem:12345:0:0:0|h[Thunderfury]|h|r",
            "a || b",
            "||||",
        ] {
            let mut line = ok.as_bytes().to_vec();
            assert!(crate::script::chat_escapes_valid(&mut line), "{ok:?}");
            assert_eq!(line, ok.as_bytes(), "{ok:?} verbatim");
        }
        for bad in [
            "|r",
            "|Hitem:1:0:0:0|h[x]|h",
            "|h",
            "|T",
            "|n",
            "|C",
            "trailing |",
            "|||",
        ] {
            assert!(
                !crate::script::chat_escapes_valid(&mut bad.as_bytes().to_vec()),
                "{bad:?}"
            );
        }
        for (line, cut) in [
            ("keep|cff00ff00dropped", "keep"),
            ("keep|cff00ff00x|Ry", "keep"),
        ] {
            let mut bytes = line.as_bytes().to_vec();
            assert!(crate::script::chat_escapes_valid(&mut bytes), "{line:?}");
            assert_eq!(bytes, cut.as_bytes(), "{line:?}");
        }
    }
}
