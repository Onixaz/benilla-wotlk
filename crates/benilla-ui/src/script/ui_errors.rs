//! The message display the engine's verbs call inline ([`show_now`]), and the Lua globals whose
//! whole body calls it. A line fires its event inside the verb's call; only its sound, and a chat
//! line whole, go to the app ([`Model::message_sounds`], [`Model::ui_errors`]).
//!
//! `NotWhileDeadError`: no argument, no dead check, no return, and a silent catalog row. FrameXML
//! decides when to call it (`UIParent.lua:663-666`, `ContainerFrame.lua:147` and `:190`).

use mlua::Lua;

/// Catalog id `0x7e`'s GlobalStrings key ("You can't do that when you're dead.").
const NOT_WHILE_DEAD_KEY: &str = "ERR_PLAYER_DEAD";

/// The message display called inline (`0x5216f0`): an error or info line's sound goes to the app first, whatever the text (`0x52172d`, `0x521765`), and its event fires inside the caller's call only with a non-empty text (`0x521870` -> `0x52188a`, `0x521849` -> `0x521863`); a chat line goes to the app whole.
pub(crate) fn show_now(lua: &Lua, key: &'static str) {
    show(lua, key, None);
}

/// [`show_now`] for a `%s` line, `arg` filling it.
pub(crate) fn show_now_s(lua: &Lua, key: &'static str, arg: String) {
    show(lua, key, Some(arg));
}

/// A refusal a helper hands back while it holds the model: its key and any `%s` argument. The
/// verb releases the model and passes it to [`show_line_now`] at that point.
pub(crate) type Line = (&'static str, Option<String>);

/// [`show_now`] for a [`Line`].
pub(crate) fn show_line_now(lua: &Lua, (key, arg): Line) {
    show(lua, key, arg);
}

impl super::UiScript {
    /// The message display from an app-side handler's write into the VM (`0x5216f0`), in that
    /// handler's turn.
    pub fn show_line(&mut self, key: &'static str) {
        show_now(self.lua(), key);
    }

    /// The message display (`0x5216f0`) for a line the app resolved, `key` naming its catalog
    /// record: an error or info line's sound goes to the app and its event fires now with a
    /// non-empty text; a chat line goes to the app whole ([`Self::take_chat_lines`]).
    pub fn show_text(&mut self, key: &'static str, text: String) {
        #[cfg(any(test, feature = "fixtures"))]
        crate::script::model_mut_of(self.lua())
            .shown_lines
            .push((key, None));
        let event = match crate::messages::kind_of(key) {
            crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
            crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
            crate::messages::MsgKind::Chat => {
                if !text.is_empty() {
                    crate::script::model_mut_of(self.lua())
                        .chat_lines
                        .push((key, text));
                }
                return;
            }
        };
        if crate::messages::by_key(key).is_some() {
            crate::script::model_mut_of(self.lua())
                .message_sounds
                .push(key);
        }
        if !text.is_empty() {
            self.fire_event(event, vec![super::ScriptValue::Str(text)]);
        }
    }

    /// The keys of every line the display showed, in order, taken: the app's tests' record.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn take_shown_keys(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut crate::script::model_mut_of(self.lua()).shown_lines)
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// The chat-kind lines [`Self::show_text`] handed over, taken.
    pub fn take_chat_lines(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut crate::script::model_mut_of(self.lua()).chat_lines)
    }
}

#[cfg(test)]
impl super::UiScript {
    /// The keys the display showed since the last take, without an argument.
    pub(crate) fn take_shown(&mut self) -> Vec<&'static str> {
        let mut model = self.model_mut();
        let (plain, rest) = std::mem::take(&mut model.shown_lines)
            .into_iter()
            .partition(|(_, arg)| arg.is_none());
        model.shown_lines = rest;
        plain
            .into_iter()
            .map(|(key, _): (&'static str, _)| key)
            .collect()
    }

    /// The `%s` lines the display showed since the last take, with their arguments.
    pub(crate) fn take_shown_s(&mut self) -> Vec<(&'static str, String)> {
        let mut model = self.model_mut();
        let (with_arg, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut model.shown_lines)
            .into_iter()
            .partition(|(_, arg)| arg.is_some());
        model.shown_lines = rest;
        with_arg
            .into_iter()
            .filter_map(|(key, arg)| Some((key, arg?)))
            .collect()
    }
}

fn show(lua: &Lua, key: &'static str, arg: Option<String>) {
    #[cfg(any(test, feature = "fixtures"))]
    crate::script::model_mut_of(lua)
        .shown_lines
        .push((key, arg.clone()));
    let event = match crate::messages::kind_of(key) {
        crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
        crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
        crate::messages::MsgKind::Chat => {
            let mut model = crate::script::model_mut_of(lua);
            match arg {
                Some(arg) => model.ui_errors_s.push((key, arg)),
                None => model.ui_errors.push(key),
            }
            return;
        }
    };
    if crate::messages::by_key(key).is_some() {
        crate::script::model_mut_of(lua).message_sounds.push(key);
    }
    let Some(text) = lua.globals().get::<Option<String>>(key).ok().flatten() else {
        return;
    };
    let args: Vec<crate::strings::Arg<'_>> = arg
        .as_deref()
        .map(crate::strings::Arg::S)
        .into_iter()
        .collect();
    let text = crate::strings::fill(&text, &args);
    if text.is_empty() {
        return;
    }
    super::tick::fire_event_into(lua, event, vec![super::ScriptValue::Str(text)]);
}

/// [`show_now`] for a line with one argument: a red or yellow line fills and fires inside the call; a chat line queues for the frame's sink (#1306).
pub(crate) fn show_now_arg(lua: &Lua, key: &'static str, arg: &super::PetitionArg) {
    use super::PetitionArg;
    let text_arg = match arg {
        PetitionArg::None => return show_now(lua, key),
        PetitionArg::S(s) => crate::strings::Arg::S(s),
        PetitionArg::D(d) => crate::strings::Arg::D(*d),
    };
    let event = match crate::messages::kind_of(key) {
        crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
        crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
        crate::messages::MsgKind::Chat => {
            let mut model = crate::script::model_mut_of(lua);
            match arg {
                PetitionArg::S(s) => model.ui_errors_s.push((key, s.clone())),
                _ => model.ui_errors_s.push((key, String::new())),
            }
            return;
        }
    };
    if crate::messages::by_key(key).is_some() {
        crate::script::model_mut_of(lua).message_sounds.push(key);
    }
    let Some(text) = lua.globals().get::<Option<String>>(key).ok().flatten() else {
        return;
    };
    let text = crate::strings::fill(&text, &[text_arg]);
    if text.is_empty() {
        return;
    }
    super::tick::fire_event_into(lua, event, vec![super::ScriptValue::Str(text)]);
}

/// Register the error-display globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "NotWhileDeadError",
        lua.create_function(|lua, ()| {
            show_now(lua, NOT_WHILE_DEAD_KEY);
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod show_now_tests {
    /// An empty text sounds and fires nothing (`0x521870`); a text fires `UI_ERROR_MESSAGE` in the call.
    #[test]
    fn an_empty_text_sounds_but_fires_nothing() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(
            "Seen = {} local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE')              f:SetScript('OnEvent', function(_, _, t) Seen[#Seen + 1] = t end)",
        )
        .unwrap();
        let key = "ERR_INV_FULL";
        s.lua().globals().set(key, "").unwrap();
        super::show_now(s.lua(), key);
        assert_eq!(s.eval::<i64>("return #Seen").unwrap(), 0);
        let sounds = s.model_ref().message_sounds.clone();
        assert_eq!(sounds, [key], "the sound goes first, whatever the text");
        s.lua().globals().set(key, "Inventory is full.").unwrap();
        super::show_now(s.lua(), key);
        assert_eq!(
            s.eval::<String>("return Seen[1]").unwrap(),
            "Inventory is full."
        );
    }

    /// Logs every `UI_ERROR_MESSAGE` and `UI_INFO_MESSAGE` with its text into `Log`, beside the
    /// markers a chunk appends.
    const LOGGER: &str = "Log = {} local f = CreateFrame('Frame') \
        f:RegisterEvent('UI_ERROR_MESSAGE') f:RegisterEvent('UI_INFO_MESSAGE') \
        f:SetScript('OnEvent', function(_, e, t) Log[#Log + 1] = e .. ':' .. t end)";

    fn log(s: &crate::script::UiScript) -> Vec<String> {
        s.eval("return Log").unwrap()
    }

    /// A refusing verb's line fires inside its call, ahead of what the same chunk does after it
    /// (`0x5216f0` from inside the verb): a line shown where it is raised (`NotWhileDeadError`), and
    /// one raised while the verb holds its state, shown at the refusal with the state let go
    /// (`DepositGuildBankMoney` past the purse).
    #[test]
    fn a_refusing_verbs_line_fires_inside_its_call() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(LOGGER).unwrap();
        let g = s.lua().globals();
        g.set("ERR_PLAYER_DEAD", "You can't do that when you're dead.")
            .unwrap();
        g.set("ERR_NOT_ENOUGH_MONEY", "You don't have enough money.")
            .unwrap();
        s.run("NotWhileDeadError() Log[#Log + 1] = 'after'")
            .unwrap();
        s.run("DepositGuildBankMoney(5) Log[#Log + 1] = 'after'")
            .unwrap();
        assert_eq!(
            log(&s),
            [
                "UI_ERROR_MESSAGE:You can't do that when you're dead.",
                "after",
                "UI_ERROR_MESSAGE:You don't have enough money.",
                "after",
            ]
        );
    }

    /// An info line fires `UI_INFO_MESSAGE` in the call; a chat line fires nothing in the VM and
    /// goes to the app whole, its argument with it.
    #[test]
    fn an_info_line_fires_its_event_and_a_chat_line_goes_to_the_app() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.run(LOGGER).unwrap();
        s.lua()
            .globals()
            .set("ERR_FISH_ESCAPED", "Your fish got away!")
            .unwrap();
        super::show_now(s.lua(), "ERR_FISH_ESCAPED");
        super::show_now_s(s.lua(), "ERR_PETITION_SIGNED_S", "Ann".into());
        assert_eq!(log(&s), ["UI_INFO_MESSAGE:Your fish got away!"]);
        assert_eq!(
            s.take_ui_errors_s(),
            [("ERR_PETITION_SIGNED_S", "Ann".to_string())]
        );
        assert!(s.take_ui_errors().is_empty());
    }
}
