//! [`feed_actions`] drains the app's generic UI error queues, which show a frame late (#1354), and
//! the sound and chat halves of the lines the VM's verbs showed in their call.

use bevy::prelude::*;

use super::{show_messages, ui_error_text, MessageSink, Shown, UiError, UiErrorKeys, UiErrorTexts};

#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_actions(
    mut script: crate::ui_script::InTurn,
    mut ui_error_keys: ResMut<UiErrorKeys>,
    mut ui_error_texts: ResMut<UiErrorTexts>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script.turn() else {
        return;
    };

    // The app's generic refusal queues still show a frame late (#1354).
    // Client-local refusals by key ([`UiErrorKeys`]); the key also names the catalog record.
    let key_lines: Vec<Shown> = ui_error_keys
        .0
        .drain(..)
        .filter_map(|e| {
            ui_error_text(&e, &|key| script.lua().globals().get::<String>(key).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", key_lines);

    // Already-resolved lines ([`UiErrorTexts`]); the queued kind is the message sink's flag.
    let resolved: Vec<Shown> = ui_error_texts
        .0
        .drain(..)
        .map(|(text, kind)| Shown::unkeyed(kind, text))
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", resolved);

    // The chat-kind lines an app caller showed through the VM in hand.
    let chat_lines: Vec<Shown> = script
        .take_chat_lines()
        .into_iter()
        .map(|(key, text)| Shown::keyed(key, text))
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", chat_lines);

    // The sound half of the lines verbs displayed inline, their events already fired in the call.
    for key in script.take_message_sounds() {
        if let Some(record) = benilla_ui::messages::by_key(key) {
            sink.sounds.push(record);
        }
    }

    // The engine's chat-kind lines: its verbs fire their error and info lines inside the call
    // (`0x5216f0`) and hand a chat line whole to the app, as the display adds a system line.
    let engine_keys = script.take_ui_errors();
    let engine_lines: Vec<Shown> = engine_keys
        .into_iter()
        .filter_map(|key| {
            let e = UiError::key(key);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_lines);
    // The same with one `%s` (`ERR_PETITION_SIGNED_S`).
    let engine_s: Vec<Shown> = script
        .take_ui_errors_s()
        .into_iter()
        .filter_map(|(key, arg)| {
            let e = UiError::s(key, arg);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_s);

    // `SecureCmdOptionParse`'s unknown condition names (`0x5f011a`, message 574).
    let unknown = script.take_unknown_macro_options();
    let unknown_lines: Vec<Shown> = unknown
        .iter()
        .filter_map(|name| {
            super::errors::keyed_line_s(&script, "ERR_UNKNOWN_MACRO_OPTION_S", &[name])
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", unknown_lines);
}
