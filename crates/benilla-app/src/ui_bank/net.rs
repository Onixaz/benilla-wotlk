//! The bank's packet handlers: `SMSG_SHOW_BANK` sets the banker and `SMSG_BUY_BANK_SLOT_RESULT`
//! shows its refusal, each in its handler's turn (`0x6e352a`, `0x6e355d`).

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::BankOpen;
use crate::net::NetHandlerApp;
use crate::ui_gossip::GossipState;
use crate::ui_quest::QuestGiver;

/// Register the bank handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ShowBank, on_show_bank)
        .net_handler(K::BuyBankSlotResult, on_buy_slot_result)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_show_bank(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    mut quest: NonSendMut<QuestGiver>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::ShowBank { banker } = ev {
        let quest_closes = gossip.npc.is_some() && quest.is_open();
        end_gossip(&mut gossip, &mut quest);
        // The quest window's close fires its event in the handler's turn (`0x58cb98`).
        if quest_closes {
            ui.fire("QUEST_FINISHED", vec![]);
        }
        ui.world(move |world| super::open_bank(world, banker));
    }
}

/// `SMSG_BUY_BANK_SLOT_RESULT` (`0x6e354c`): a code below 3 shows its line in the handler's turn; vmangos sends one only on failure.
fn on_buy_slot_result(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::BuyBankSlotResult { result } = ev {
        debug!("net: bank slot purchase refused (code {result})");
        if let Some(key) = super::bank_slot_error_key(result) {
            crate::ui_action::show_in_turn(&mut ui, "ui_bank", crate::ui_action::UiError::key(key));
        }
    }
}

/// The bank window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut bank: NonSendMut<BankOpen>) {
    bank.clear();
}

/// `SMSG_SHOW_BANK`, sent for `CMSG_BANKER_ACTIVATE` and unprompted for the gossip bank option:
/// the new interact target closes the gossip menu on it (`0x51fce0` -> `0x512ef6`).
fn end_gossip(gossip: &mut GossipState, quest: &mut QuestGiver) {
    if gossip.npc.is_some() {
        crate::ui_gossip::end_interaction(gossip, quest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_bank_ends_the_gossip_interaction() {
        let mut gossip = GossipState::default();
        let mut quest = QuestGiver::default();
        gossip.npc = Some(0x42);
        end_gossip(&mut gossip, &mut quest);
        assert_eq!(gossip.npc, None, "the gossip session ended with the menu");
    }
}
