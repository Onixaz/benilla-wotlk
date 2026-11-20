//! The bank's packet handlers: `SMSG_SHOW_BANK` sets the banker and `SMSG_BUY_BANK_SLOT_RESULT` shows its refusal, each in its handler's turn (`0x6e352a`, `0x6e355d`).

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::BankOpen;
use crate::net::{GuidIndex, NetHandlerApp, ObjectStore};

/// Register the bank handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ShowBank, on_show_bank)
        .net_handler(K::BuyBankSlotResult, on_buy_slot_result)
        .net_handler(K::DestroyedOnDeath, on_destroyed_on_death)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_show_bank(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::ShowBank { banker } = ev {
        ui.world(move |world| super::open_bank(world, banker));
    }
}

/// `SMSG_DESTROY_OBJECT`'s death byte on a unit above 0 health runs its death in the handler (`0x4d7661`-`0x4d7682`), whose interact-target clear with its guid (`0x7293bf`) closes the window on it.
fn on_destroyed_on_death(
    In(ev): In<SessionEvent>,
    index: Option<Res<GuidIndex>>,
    stores: Query<&ObjectStore>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    let SessionEvent::DestroyedOnDeath(guid) = ev else {
        return;
    };
    let alive = index
        .as_deref()
        .and_then(|i| i.0.get(&guid))
        .and_then(|&e| stores.get(e).ok())
        .is_some_and(|s| s.0.is_unit() && s.0.unit_health().unwrap_or(0) > 0);
    if alive {
        ui.world(move |world| crate::ui_session::clear_interact_target(world, guid));
    }
}

/// `SMSG_BUY_BANK_SLOT_RESULT` (`0x6e354c`): a code below 3 shows its line in the handler's turn.
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
