//! The message display the engine's verbs call inline ([`show_now`]), and the Lua globals whose
//! whole body calls it. A line fires its event, or adds its chat line through the lent view, inside
//! the verb's call; only its sound goes to the app ([`Model::message_sounds`]).
//!
//! `NotWhileDeadError`: no argument, no dead check, no return, and a silent catalog row. FrameXML
//! decides when to call it (`UIParent.lua:663-666`, `ContainerFrame.lua:147` and `:190`).

use mlua::Lua;

/// Catalog id `0x7e`'s GlobalStrings key ("You can't do that when you're dead.").
const NOT_WHILE_DEAD_KEY: &str = "ERR_PLAYER_DEAD";

/// The message display called inline (`0x5216f0`): an error or info line's sound goes to the app first, whatever the text (`0x52172d`, `0x521765`), and its event fires inside the caller's call only with a non-empty text (`0x521870` -> `0x52188a`, `0x521849` -> `0x521863`); a chat line is added to the chat log inside the call (`0x52183c`).
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
    /// non-empty text; a chat line is added in the caller's turn ([`Self::add_chat_text`]).
    pub fn show_text(&mut self, key: &'static str, text: String) {
        #[cfg(any(test, feature = "fixtures"))]
        crate::script::model_mut_of(self.lua())
            .shown_lines
            .push((key, None));
        // Every row's sound goes first, a chat row's too (`0x52172d`, `0x521765`).
        if crate::messages::by_key(key).is_some() {
            crate::script::model_mut_of(self.lua())
                .message_sounds
                .push(key);
        }
        let event = match crate::messages::kind_of(key) {
            crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
            crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
            crate::messages::MsgKind::Chat => {
                if !text.is_empty() {
                    let chat_type = crate::messages::by_key(key).map_or(0, |r| r.chat_type);
                    self.add_chat_text(chat_type, text);
                }
                return;
            }
        };
        if !text.is_empty() {
            self.fire_event(event, vec![super::ScriptValue::Str(text)]);
        }
    }

    /// The display's chat arm for a resolved line (`0x521814` -> `0x509dd0`): added through the chat add on `chat_type` in the caller's turn, inside a host step's turn at that step's raise.
    pub fn add_chat_text(&mut self, chat_type: u8, text: String) {
        if super::world_loan::in_turn(self.lua()) {
            super::world_loan::in_turn_or_now(self, move |s| s.add_chat_text(chat_type, text));
            return;
        }
        add_chat_text(self.lua(), chat_type, &text);
    }

    /// The keys of every line the display showed, in order, taken: the app's tests' record.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn take_shown_keys(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut crate::script::model_mut_of(self.lua()).shown_lines)
            .into_iter()
            .map(|(key, _)| key)
            .collect()
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
    // Every row's sound goes first, a chat row's too (`0x52172d`, `0x521765`).
    if crate::messages::by_key(key).is_some() {
        crate::script::model_mut_of(lua).message_sounds.push(key);
    }
    let event = match crate::messages::kind_of(key) {
        crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
        crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
        crate::messages::MsgKind::Chat => {
            let args: Vec<crate::strings::Arg<'_>> = arg
                .as_deref()
                .map(crate::strings::Arg::S)
                .into_iter()
                .collect();
            return add_chat_line(lua, key, &args);
        }
    };
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

/// The display's chat arm (`0x521814` -> `0x52183c`): the filled text goes to the chat log on the
/// row's chat type inside the call, where the add records it and fires its event (`0x509dd0`:
/// `0x50a8a5`, `0x50a8d5`); the event fires here once the host lets go. A missing text adds nothing.
fn add_chat_line(lua: &Lua, key: &'static str, args: &[crate::strings::Arg<'_>]) {
    let Some(text) = lua.globals().get::<Option<String>>(key).ok().flatten() else {
        return;
    };
    let text = crate::strings::fill(&text, args);
    let chat_type = crate::messages::by_key(key).map_or(0, |r| r.chat_type);
    add_chat_text(lua, chat_type, &text);
}

/// Add `text` through the host's chat add on `chat_type` inside the call (`0x509dd0`), which
/// records it and raises its event before returning (`0x50a8a5`, `0x50a8d5`); a lent view alone
/// records it and the event fires here.
pub(crate) fn add_chat_text(lua: &Lua, chat_type: u8, text: &str) {
    let ask = super::calls::HostAsk::ChatLine {
        chat_type,
        text: text.to_string(),
    };
    if super::calls::ask_host(lua, &ask).is_some() {
        return;
    }
    match super::write_live(|l| l.add_chat_line(chat_type, text)) {
        Some(Some((event, args))) => super::tick::fire_event_into(lua, event, args),
        Some(None) => {}
        // Every caller adds in its own turn with the host lent (`0x509dd0` runs inside the caller),
        // so a line with nothing lent has no add to reach: a hard failure in a debug build.
        None if cfg!(debug_assertions) => {
            panic!("a chat line was added with no host lent, outside the add's caller: {text:?}")
        }
        None => crate::script::model_mut_of(lua).warn_host_only(format!(
            "a chat line was added with no host lent and is lost: {text:?}"
        )),
    }
}

/// The carried-count check's message (`0x754e2a`, `0x754ea7`): the limit category's line takes the
/// row's quantity then its name.
pub(crate) fn show_carry_refusal(lua: &Lua, refusal: &super::CarryRefusal) {
    match refusal {
        super::CarryRefusal::MaxCount => show_now(lua, refusal.key()),
        super::CarryRefusal::Category { quantity, name } => show_filled(
            lua,
            refusal.key(),
            &[
                crate::strings::Arg::D(i64::from(*quantity)),
                crate::strings::Arg::S(name),
            ],
        ),
    }
}

/// [`show_now`] for a line with one argument: a red or yellow line fills and fires inside the call, a chat line is added to the chat log inside it (#1306).
pub(crate) fn show_now_arg(lua: &Lua, key: &'static str, arg: &super::PetitionArg) {
    use super::PetitionArg;
    let text_arg = match arg {
        PetitionArg::None => return show_now(lua, key),
        PetitionArg::S(s) => crate::strings::Arg::S(s),
        PetitionArg::D(d) => crate::strings::Arg::D(*d),
    };
    show_filled(lua, key, &[text_arg]);
}

/// [`show_now`] for a line its `args` fill: a red or yellow line fires inside the call, a chat line
/// is added to the chat log inside it.
fn show_filled(lua: &Lua, key: &'static str, args: &[crate::strings::Arg<'_>]) {
    // Every row's sound goes first, a chat row's too (`0x52172d`, `0x521765`).
    if crate::messages::by_key(key).is_some() {
        crate::script::model_mut_of(lua).message_sounds.push(key);
    }
    let event = match crate::messages::kind_of(key) {
        crate::messages::MsgKind::Error => "UI_ERROR_MESSAGE",
        crate::messages::MsgKind::Info => "UI_INFO_MESSAGE",
        crate::messages::MsgKind::Chat => return add_chat_line(lua, key, args),
    };
    let Some(text) = lua.globals().get::<Option<String>>(key).ok().flatten() else {
        return;
    };
    let text = crate::strings::fill(&text, args);
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

    /// Logs the chat lines the display adds beside [`LOGGER`]'s.
    const CHAT_LOGGER: &str = "local c = CreateFrame('Frame') \
        c:RegisterEvent('CHAT_MSG_SYSTEM') c:RegisterEvent('CHAT_MSG_SKILL') \
        c:SetScript('OnEvent', function(_, e, t) Log[#Log + 1] = e .. ':' .. t end)";

    /// An info line fires `UI_INFO_MESSAGE` in the call; a chat line is added to the lent chat log
    /// inside the call, its argument filled, and its event fires there (`0x52183c`).
    #[test]
    fn an_info_line_fires_its_event_and_a_chat_line_is_added_in_the_call() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(LOGGER).unwrap();
        s.run(CHAT_LOGGER).unwrap();
        let g = s.lua().globals();
        g.set("ERR_FISH_ESCAPED", "Your fish got away!").unwrap();
        g.set("ERR_PETITION_SIGNED_S", "%s has signed your charter.")
            .unwrap();
        let mut view = crate::script::live::TestView::default();
        crate::script::live::lending(&mut view, || {
            super::show_now(s.lua(), "ERR_FISH_ESCAPED");
            super::show_now_s(s.lua(), "ERR_PETITION_SIGNED_S", "Ann".into());
            s.run("Log[#Log + 1] = 'after'").unwrap();
        });
        assert_eq!(
            log(&s),
            [
                "UI_INFO_MESSAGE:Your fish got away!",
                "CHAT_MSG_SYSTEM:Ann has signed your charter.",
                "after",
            ]
        );
        assert_eq!(
            view.chat_lines,
            [(0, "Ann has signed your charter.".into())]
        );
    }

    /// A chat row sounds as a red or yellow one does, its sound queued before its line is added (`0x52172d`, `0x521765` ahead of `0x521814`), from a verb and from an app handler alike.
    #[test]
    fn a_chat_rows_sound_goes_first() {
        let mut s = crate::script::UiScript::new().unwrap();
        s.lua()
            .globals()
            .set("ERR_RECEIVE_ITEM_S", "You receive item: %s.")
            .unwrap();
        let mut view = crate::script::live::TestView::default();
        crate::script::live::lending(&mut view, || {
            super::show_now_s(s.lua(), "ERR_RECEIVE_ITEM_S", "Linen".into());
            s.show_text("ERR_RECEIVE_ITEM_S", "You receive item: Linen.".into());
        });
        assert_eq!(
            s.take_message_sounds(),
            ["ERR_RECEIVE_ITEM_S", "ERR_RECEIVE_ITEM_S"]
        );
    }

    /// Every row of the catalog (`0xac8af0`) shows inside the call: a red or yellow line fires its
    /// event, a chat line is added to the lent chat log on the row's chat type and fires its event
    /// there (#1306).
    #[test]
    fn every_catalog_row_shows_inside_the_call() {
        use crate::messages::{MsgKind, CATALOG};
        let s = crate::script::UiScript::new().unwrap();
        s.run(LOGGER).unwrap();
        s.run(CHAT_LOGGER).unwrap();
        let mut view = crate::script::live::TestView::default();
        for r in CATALOG.iter().filter(|r| !r.key.is_empty()) {
            s.lua().globals().set(r.key, "text").unwrap();
            s.run("Log = {}").unwrap();
            view.chat_lines.clear();
            crate::script::live::lending(&mut view, || super::show_now(s.lua(), r.key));
            let fired = log(&s);
            match r.kind {
                MsgKind::Error => {
                    assert_eq!(fired, ["UI_ERROR_MESSAGE:text"], "{}", r.key);
                    assert!(view.chat_lines.is_empty(), "{}", r.key);
                }
                MsgKind::Info => {
                    assert_eq!(fired, ["UI_INFO_MESSAGE:text"], "{}", r.key);
                    assert!(view.chat_lines.is_empty(), "{}", r.key);
                }
                MsgKind::Chat => {
                    let event = if r.chat_type == 26 {
                        "CHAT_MSG_SKILL:text"
                    } else {
                        "CHAT_MSG_SYSTEM:text"
                    };
                    assert_eq!(fired, [event], "{}", r.key);
                    assert_eq!(view.chat_lines, [(r.chat_type, "text".into())], "{}", r.key);
                }
            }
        }
    }

    /// The add runs inside its caller (`0x509dd0`), so a chat line shown with nothing lent has no
    /// add to reach and fails a debug build instead of waiting for a later frame.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a chat line was added with no host lent")]
    fn a_chat_line_with_nothing_lent_fails() {
        let s = crate::script::UiScript::new().unwrap();
        s.lua()
            .globals()
            .set("ERR_RECEIVE_ITEM_S", "You receive item: %s.")
            .unwrap();
        super::show_now_s(s.lua(), "ERR_RECEIVE_ITEM_S", "Linen".into());
    }
}
