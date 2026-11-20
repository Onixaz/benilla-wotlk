//! The vendor window's packet handlers: each writes the merchant globals and fires its events in its turn.

use benilla_protocol::messages::VendorList;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::NetHandlerApp;

/// Register the vendor handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::VendorInventory, on_inventory)
        .net_handler(K::VendorBuyResult, on_buy_result)
        .net_handler(K::VendorBuyFailed, on_buy_failed)
        .net_handler(K::VendorSellFailed, on_sell_failed)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_LIST_INVENTORY` (`0x6d92d0`): a list sets the merchant (`0x6d9af8`); an empty list's refusal opens nothing (`0x6d9ad2`) and only reaches the console.
fn on_inventory(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::VendorInventory {
        vendor,
        items,
        error,
    } = ev
    else {
        return;
    };
    let list = VendorList {
        vendor,
        items,
        error,
    };
    if !list.opens() {
        debug!(
            "net: vendor {:#x} refused its list (reason {:?})",
            list.vendor, list.error
        );
        return;
    }
    debug!(
        "net: vendor {:#x} listed {} items",
        list.vendor,
        list.items.len()
    );
    super::set_merchant(world, list.vendor, list.items);
}

/// `SMSG_BUY_ITEM` (`0x6cdd10`): the row's new count through `0x5840e0`; the bought item itself arrives as an object create.
fn on_buy_result(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::VendorBuyResult {
        vendor,
        slot,
        new_count,
        ..
    } = ev
    {
        super::update_item_quantity(world, vendor, slot, new_count);
    }
}

/// `SMSG_BUY_FAILED` (`0x6cdba0`): `ITEM_ALREADY_SOLD` zeroes each matching row through `0x5840e0`, its `MERCHANT_UPDATE` with each (`0x6cdc06`), then the code's line shows (`0x6cdc23`).
///
/// Deviation: the row is matched by item entry, where the reference matches its vendor slot word
/// (`+0x00`), because the server writes the item entry in that field.
fn on_buy_failed(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::VendorBuyFailed {
        vendor,
        item_entry,
        reason,
    } = ev
    else {
        return;
    };
    debug!("net: buy failed (entry {item_entry}, reason {reason})");
    if reason == benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD {
        let slots: Vec<u32> = world
            .get_resource::<super::MerchantOpen>()
            .filter(|m| m.vendor == Some(vendor))
            .map(|m| {
                m.items
                    .iter()
                    .filter(|r| r.entry == item_entry)
                    .map(|r| r.slot)
                    .collect()
            })
            .unwrap_or_default();
        for slot in slots {
            super::update_item_quantity(world, vendor, slot, 0);
        }
    }
    if let (Some(key), Some(mut ui)) = (
        super::buy_error_key(reason),
        world.get_resource_mut::<crate::ui_script::live::UiEvents>(),
    ) {
        crate::ui_action::show_in_turn(&mut ui, "ui_merchant", crate::ui_action::UiError::key(key));
    }
}

/// `SMSG_SELL_ITEM`'s error path (`0x6d1540`): the code's line shows (`0x6d15f7`).
fn on_sell_failed(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::VendorSellFailed { reason, .. } = ev {
        debug!("net: sell failed (reason {reason})");
        if let Some(key) = super::sell_error_key(reason) {
            crate::ui_action::show_in_turn(
                &mut ui,
                "ui_merchant",
                crate::ui_action::UiError::key(key),
            );
        }
    }
}

/// The vendor is dropped with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut merchant: ResMut<super::MerchantOpen>) {
    merchant.vendor = None;
    merchant.items.clear();
}
