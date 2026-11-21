//! Party quest sharing: each member's verdict on a quest we shared, and the escort confirm a
//! member gets when someone starts a party-accept quest, with one server latch
//! (`Player::SetQuestShareInfo`) behind both. The receiving half needs nothing here: the receiver
//! gets a plain `SMSG_QUESTGIVER_QUEST_DETAILS` whose giver is the sharer, which [`crate::ui_quest`] tells from an NPC offer by the guid's type.
//!
//! `QUEST_ACCEPT_CONFIRM(member, title)` raises the stock popup (`UIParent.lua:353`); Yes calls
//! `ConfirmAcceptQuest()` and No sends nothing. Both handlers read the member's name from the name cache with no callback (`0x6e3a61`, `0x58bc85`): a miss asks for it and shows nothing.

use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::{show_in_turn, UiError};
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;

/// The share state: the quest the last escort confirm named.
#[derive(Resource, Default)]
pub(crate) struct QuestShare {
    /// The quest `ConfirmAcceptQuest()` answers, latched by the confirm's handler before its name lookup (`0x58bc5d`): the verb takes no argument (`StaticPopup.lua:731-733`).
    confirm_quest: Option<u32>,
}

impl QuestShare {
    /// On disconnect: the server's latch went with the session.
    pub(crate) fn clear_session(&mut self) {
        self.confirm_quest = None;
    }
}

/// The `ERR_QUEST_PUSH_*` key a verdict shows. 12340's `0x276` arm maps 0..=11 onto messages
/// `0x19b`-`0x1a6` in this order (`0x6e3a72`), each passed the member's name, all `kind 0`:
/// `CHAT_MSG_SYSTEM` lines, never `UIErrorsFrame`. An unmapped byte shows nothing.
fn verdict_message(msg: QuestShareMsg) -> Option<&'static str> {
    let key = match msg {
        QuestShareMsg::SHARING_QUEST => "ERR_QUEST_PUSH_SUCCESS_S",
        QuestShareMsg::CANT_TAKE_QUEST => "ERR_QUEST_PUSH_INVALID_S",
        QuestShareMsg::ACCEPT_QUEST => "ERR_QUEST_PUSH_ACCEPTED_S",
        QuestShareMsg::DECLINE_QUEST => "ERR_QUEST_PUSH_DECLINED_S",
        QuestShareMsg::BUSY => "ERR_QUEST_PUSH_BUSY_S",
        QuestShareMsg::LOG_FULL => "ERR_QUEST_PUSH_LOG_FULL_S",
        QuestShareMsg::HAVE_QUEST => "ERR_QUEST_PUSH_ONQUEST_S",
        QuestShareMsg::FINISH_QUEST => "ERR_QUEST_PUSH_ALREADY_DONE_S",
        QuestShareMsg::CANT_BE_SHARED_TODAY => "ERR_QUEST_PUSH_NOT_DAILY_S",
        QuestShareMsg::SHARING_TIMER_EXPIRED => "ERR_QUEST_PUSH_TIMER_EXPIRED_S",
        QuestShareMsg::NOT_IN_PARTY => "ERR_QUEST_PUSH_NOT_IN_PARTY_S",
        QuestShareMsg::NOT_ELIGIBLE_TODAY => "ERR_QUEST_PUSH_DIFFERENT_SERVER_DAILY_S",
        _ => return None,
    };
    Some(key)
}

/// The quest-share packet handlers, each showing its line or firing its event in its turn.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use super::*;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestPushResult, on_push_result)
            .net_handler(K::QuestConfirmAccept, on_confirm_accept)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// `MSG_QUEST_PUSH_RESULT` (`0x6e3a31`): the member's name from the cache, then the verdict's line (`0x6e3a72`); a name not cached shows nothing.
    fn on_push_result(
        In(ev): In<SessionEvent>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::QuestPushResult { member, msg } = ev else {
            return;
        };
        debug!(
            "net: quest push result — member {member:#x} verdict {}",
            msg.0
        );
        let Some(name) = names.resolve(member, &commands).map(str::to_string) else {
            debug!(
                "ui_quest_share: {member:#x} not named — verdict {} dropped",
                msg.0
            );
            return;
        };
        let Some(key) = verdict_message(msg) else {
            debug!("ui_quest_share: unmapped push verdict {} — no line", msg.0);
            return;
        };
        show_in_turn(&mut ui, "ui_quest_share", UiError::s(key, name));
    }

    /// `SMSG_QUEST_CONFIRM_ACCEPT` (`0x58bc50`): the quest is latched for the Yes, then `QUEST_ACCEPT_CONFIRM(member, title)` fires when the member's name is cached.
    fn on_confirm_accept(
        In(ev): In<SessionEvent>,
        mut share: ResMut<QuestShare>,
        names: Res<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
            quest_id,
            title,
            sender,
        }) = ev
        else {
            return;
        };
        debug!("net: quest confirm accept — quest {quest_id} ({title:?}) from {sender:#x}");
        share.confirm_quest = Some(quest_id);
        if let Some(name) = names.resolve(sender, &commands).map(str::to_string) {
            ui.fire(
                "QUEST_ACCEPT_CONFIRM",
                vec![ScriptValue::Str(name), ScriptValue::Str(title)],
            );
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        share.clear_session();
    }
}

/// Owns [`QuestShare`] and its drain; nothing here is bound to the questgiver window.
pub(crate) struct QuestSharePlugin;

impl Plugin for QuestSharePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // After the input pass, so Yes goes out the frame it is clicked.
        app.init_resource::<QuestShare>()
            .add_systems(Update, drain_quest_share.in_set(UiDrain));
    }
}

/// `ConfirmAcceptQuest()`, the popup's Yes, answered with the latched quest id.
fn drain_quest_share(
    script: Option<NonSendMut<UiScript>>,
    share: Res<QuestShare>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_quest_confirms() {
        let Some(quest) = share.confirm_quest else {
            debug!("ui_quest_share: ConfirmAcceptQuest with no confirm held — ignored");
            continue;
        };
        let _ = commands.0.send(ClientCommand::QuestConfirmAccept { quest });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_action::ui_error_text;

    #[test]
    fn every_wire_verdict_has_a_key_and_an_unknown_has_none() {
        for v in 0u8..=11 {
            let key =
                verdict_message(QuestShareMsg(v)).unwrap_or_else(|| panic!("verdict {v} unmapped"));
            // Byte `v` is message `0x19b + v` (`0x6e43f8`).
            assert_eq!(
                benilla_ui::messages::by_key(key).map(|r| r.id),
                Some(0x19b + u16::from(v)),
                "verdict {v} shows {key}"
            );
        }
        assert!(verdict_message(QuestShareMsg(12)).is_none());
        assert!(verdict_message(QuestShareMsg(0xFF)).is_none());
    }

    #[test]
    fn verdict_keys_are_distinct() {
        let mut keys: Vec<&str> = (0u8..=11)
            .filter_map(|v| verdict_message(QuestShareMsg(v)))
            .collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate ERR_QUEST_PUSH_* key");
    }

    #[test]
    fn every_verdict_key_resolves() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        // Three of the twelve have no `%s` in enUS, though the arm passes the name to all.
        for raw in 0u8..=11 {
            let key = verdict_message(QuestShareMsg(raw)).expect("mapped");
            let text = g(key).unwrap_or_default();
            assert!(!text.is_empty(), "{key} (verdict {raw}) missing");
        }

        // The opener the sharer sees on the click, and a decline's answer.
        let line = |raw: u8| {
            let key = verdict_message(QuestShareMsg(raw)).unwrap();
            ui_error_text(&UiError::s(key, "Mate"), &g)
        };
        assert_eq!(
            line(QuestShareMsg::SHARING_QUEST.0).as_deref(),
            Some("Sharing quest with Mate...")
        );
        assert_eq!(
            line(QuestShareMsg::DECLINE_QUEST.0).as_deref(),
            Some("Mate has declined your quest")
        );
    }
}
