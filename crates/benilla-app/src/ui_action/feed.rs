//! [`feed_actions`] drains the sound half of the lines the VM's verbs showed in their call and the
//! chat lines an app caller showed through the VM in hand.

use bevy::prelude::*;

use super::{show_messages, MessageSink, Shown};

#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_actions(mut script: crate::ui_script::InTurn, mut sink: MessageSink) {
    let Some(mut script) = script.turn() else {
        return;
    };

    // The chat-kind lines an app caller showed through the VM with no chat log lent.
    for (chat_type, text) in script.take_chat_lines() {
        sink.chat.push_event(crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::kind_of_type(chat_type),
            text,
        ));
    }

    // The sound half of the lines verbs displayed inline, their events already fired in the call.
    for key in script.take_message_sounds() {
        if let Some(record) = benilla_ui::messages::by_key(key) {
            sink.sounds.push(record);
        }
    }

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
