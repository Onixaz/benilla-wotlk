//! The vendor window's packet handlers: they fill [`MerchantOpen`] and [`MerchantErrors`].

use benilla_protocol::messages::VendorList;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{MerchantErrors, MerchantOpen, MerchantRefusal};
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

fn on_inventory(In(ev): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    if let SessionEvent::VendorInventory {
        vendor,
        items,
        error,
    } = ev
    {
        vendor_inventory(
            VendorList {
                vendor,
                items,
                error,
            },
            &mut merchant,
        );
    }
}

fn on_buy_result(In(ev): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    if let SessionEvent::VendorBuyResult {
        vendor,
        slot,
        new_count,
        ..
    } = ev
    {
        vendor_buy_result(vendor, slot, new_count, &mut merchant);
    }
}

fn on_buy_failed(
    In(ev): In<SessionEvent>,
    mut merchant: ResMut<MerchantOpen>,
    mut errors: ResMut<MerchantErrors>,
) {
    if let SessionEvent::VendorBuyFailed {
        vendor,
        item_entry,
        reason,
    } = ev
    {
        vendor_buy_failed(vendor, item_entry, reason, &mut merchant, &mut errors);
    }
}

fn on_sell_failed(In(ev): In<SessionEvent>, mut errors: ResMut<MerchantErrors>) {
    if let SessionEvent::VendorSellFailed { reason, .. } = ev {
        vendor_sell_failed(reason, &mut errors);
    }
}

/// The vendor window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    merchant.clear_session();
}

/// `SMSG_LIST_INVENTORY` opens the window on the vendor's rows; an empty list's refusal opens
/// nothing (`0x6d9ad0`) and only reaches the console.
fn vendor_inventory(list: VendorList, merchant: &mut MerchantOpen) {
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
    merchant.open(list.vendor, list.items);
}

/// `SMSG_BUY_ITEM`'s new stock count, applied only to the open vendor.
fn vendor_buy_result(vendor: u64, slot: u32, new_count: u32, merchant: &mut MerchantOpen) {
    if merchant.vendor == Some(vendor) {
        merchant.update_stock(slot, new_count);
    }
}

/// `SMSG_BUY_FAILED`: queue the message line, and for `ITEM_ALREADY_SOLD` zero the refused row of
/// the open vendor, as the reference does (`0x6cdba0`).
///
/// Deviation: the row is matched by item entry, where the reference matches its vendor slot word
/// (`+0x00`), because the server writes the item entry in that field.
fn vendor_buy_failed(
    vendor: u64,
    item_entry: u32,
    reason: u8,
    merchant: &mut MerchantOpen,
    errors: &mut MerchantErrors,
) {
    debug!("net: buy failed (entry {item_entry}, reason {reason})");
    if reason == benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD
        && merchant.vendor == Some(vendor)
    {
        merchant.sold_out(item_entry);
    }
    errors.0.push(MerchantRefusal::Buy(reason));
}

/// `SMSG_SELL_ITEM`'s error path: queue the message line.
fn vendor_sell_failed(reason: u8, errors: &mut MerchantErrors) {
    debug!("net: sell failed (reason {reason})");
    errors.0.push(MerchantRefusal::Sell(reason));
}
