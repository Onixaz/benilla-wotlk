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
    // Ahead of each window's own handler: the setter runs at the start of its open (`0x54970f` before `0x54972c`).
    app.net_handler_first(K::GossipMenu, on_interact_target_taken)
        .net_handler_first(K::QuestGreeting, on_interact_target_taken)
        .net_handler_first(K::QuestDetail, on_interact_target_taken)
        .net_handler_first(K::QuestProgress, on_interact_target_taken)
        .net_handler_first(K::QuestOffer, on_interact_target_taken)
        .net_handler_first(K::VendorInventory, on_interact_target_taken)
        .net_handler_first(K::TaxiNodesShown, on_interact_target_taken)
        .net_handler_first(K::TrainerList, on_interact_target_taken)
        .net_handler_first(K::TabardVendorActivate, on_interact_target_taken)
        .net_handler_first(K::PetitionShowList, on_interact_target_taken)
        .net_handler_first(K::AuctionHello, on_interact_target_taken)
        .net_handler_first(K::ListStabledPets, on_interact_target_taken)
        .net_handler_first(K::ShowMailbox, on_interact_target_taken)
        .net_handler_first(K::BattlefieldList, on_interact_target_taken)
        .net_handler_first(K::TradeStatus, on_interact_target_taken);
}

/// A window's open takes the interact target (`0x51fce0`): gossip (`0x58b1b0`), the quest frame (`0x6d0240`, `0x6d0460`, `0x6d08f0`, `0x6d0ab0`), the vendor (`0x6d9960`), the taxi map (`0x6d6050`), the trainer (`0x5940e0`), the tabard designer (`0x598e60`), the registrar and charter vendor (`0x599e00`, `0x5a22a0`), the auctioneer (`0x59f000`), the stable (`0x5a11a0`), the mailbox (`0x56dbc0`), the battlemaster (`0x54e57d`) and the trade window (`0x5873e0`); the setter's interaction close takes the bank's leg while the banker holds it (`0x51fd1c` -> `0x512fa7`), ahead of that window's own open.
fn on_interact_target_taken(
    In(ev): In<SessionEvent>,
    bank: NonSend<BankOpen>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    use benilla_protocol::messages::TradeStatus;
    let opens = match &ev {
        // An empty list's refusal opens nothing (`0x6d9ad2`-`0x6d9af8`).
        SessionEvent::VendorInventory { items, error, .. } => {
            !items.is_empty() || *error == Some(0)
        }
        SessionEvent::AuctionHello { enabled, .. } => *enabled,
        // A list for the PvP window sets no battlemaster (`0x54e55d`); a zero one closes without the setter (`0x5496d9`).
        SessionEvent::BattlefieldList(list) => !list.from_anywhere && list.battlemaster != 0,
        SessionEvent::TradeStatus { status } => matches!(status, TradeStatus::OpenWindow { .. }),
        SessionEvent::GossipMenu(_)
        | SessionEvent::QuestGreeting(_)
        | SessionEvent::QuestDetail(_)
        | SessionEvent::QuestProgress(_)
        | SessionEvent::QuestOffer(_)
        | SessionEvent::TaxiNodesShown { .. }
        | SessionEvent::TrainerList { .. }
        | SessionEvent::TabardVendorActivate(_)
        | SessionEvent::PetitionShowList(_)
        | SessionEvent::ListStabledPets { .. }
        | SessionEvent::ShowMailbox { .. } => true,
        _ => false,
    };
    if opens && bank.is_open() {
        ui.world(super::close_by_interaction);
    }
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
