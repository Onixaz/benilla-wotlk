//! The auction house's packet handlers, after the 12340 handlers they name: each writes
//! [`AuctionOpen`] and raises its events and lines at its step, the list updates once every row's
//! item record is in (`0x59cb70`, `0x59cbb0`, `0x59cbe0`, `0x59cc10`).

use benilla_protocol::messages::{
    auction_action, auction_error, AuctionBidderNotification, AuctionCommandTail, AuctionListPage,
    AuctionOwnerNotification, AuctionPendingSale,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, BIDDER, LIST, OWNER};
use bevy::prelude::*;

use super::{
    bidder_list, expire, list_event, owner_list, sell_next, AuctionEntry, AuctionLive, AuctionOpen,
    BID_EMOTE, BID_EXTENSION_SECS,
};
use crate::items::{RecordWait, RecordWaits};
use crate::net::{ClientCommand, NetHandlerApp};
use crate::ui_action::signals::SignalSink;
use crate::ui_action::UiError;
use crate::ui_mail::MailArrival;
use crate::ui_script::live::UiEvents;

/// One handler per `SMSG_AUCTION_*` kind, the name answers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::AuctionHello, on_hello)
        .net_handler(K::AuctionCommandResult, on_command_result)
        .net_handler(K::AuctionListResult, on_list_result)
        .net_handler(K::AuctionOwnerListResult, on_owner_list_result)
        .net_handler(K::AuctionBidderListResult, on_bidder_list_result)
        .net_handler(K::AuctionPendingSales, on_pending_sales)
        .net_handler(K::AuctionBidderNotification, on_bidder_notification)
        .net_handler(K::AuctionOwnerNotification, on_owner_notification)
        .net_handler(K::AuctionRemovedNotification, on_removed_notification)
        .net_handler(K::PlayerName, on_name_record)
        .net_handler(K::PlayerNameMiss, on_name_record)
        .net_handler(K::Disconnected, on_session_end);
}

/// A handler's reach: the house, the event queue, the record callbacks and the mail arrival.
#[derive(bevy::ecs::system::SystemParam)]
struct Step<'w, 's> {
    live: AuctionLive<'w, 's>,
    ui: ResMut<'w, UiEvents>,
    waits: Option<NonSendMut<'w, RecordWaits>>,
}

/// `MSG_AUCTION_HELLO`'s reply (`0x59ff40`): an open house sets the auctioneer (`0x59f000`); a
/// closed one fires `AUCTION_HOUSE_DISABLED`.
fn on_hello(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    let SessionEvent::AuctionHello {
        auctioneer,
        house_id,
        enabled,
    } = ev
    else {
        return;
    };
    if enabled {
        ui.world(move |w| super::set_auctioneer(w, auctioneer, house_id));
    } else {
        ui.fire("AUCTION_HOUSE_DISABLED", Vec::new());
    }
}

fn on_list_result(In(ev): In<SessionEvent>, mut step: Step) {
    if let SessionEvent::AuctionListResult(page) = ev {
        list_result(LIST, page, &mut step);
    }
}

fn on_owner_list_result(In(ev): In<SessionEvent>, mut step: Step) {
    if let SessionEvent::AuctionOwnerListResult(page) = ev {
        list_result(OWNER, page, &mut step);
    }
}

fn on_bidder_list_result(In(ev): In<SessionEvent>, mut step: Step) {
    if let SessionEvent::AuctionBidderListResult(page) = ev {
        list_result(BIDDER, page, &mut step);
    }
}

/// Sort `list` and raise its update (`0x59b980`, `0x59b9e0`, `0x59e86a`).
fn sort_and_signal(live: &mut AuctionLive, ui: &mut UiEvents, list: usize) {
    live.sort(list);
    ui.fire(list_event(list), Vec::new());
}

/// Any of the three list results (`0x59e160`, `0x59e480`, `0x59ecd0`): the query is answered and
/// its delay paces the next; the callbacks the old rows left are dropped and the page replaces them,
/// the owner list keeping its sold rows; each row whose record is in flight links the list's
/// callback, and with none in flight the list sorts and signals.
fn list_result(which: usize, page: AuctionListPage, step: &mut Step) {
    let Step { live, ui, waits } = step;
    let look = &live.look;
    let a = &mut *live.auction;
    debug!(
        "ui_auction: list {which} result: {} row(s) {:?}, total {}, delay {:?}",
        page.auctions.len(),
        page.auctions
            .iter()
            .map(|r| r.auction_id)
            .collect::<Vec<_>>(),
        page.total_count,
        page.query_delay_ms
    );
    let now = look.now();
    a.wire.list_results[which] += 1;
    a.answered(which, now, page.query_delay_ms);
    let callback = RecordWait::Auction(Callback::List(which));
    let held = a.pending_items[which] != 0;
    let slot = &mut a.lists[which];
    // The owner list keeps its sold rows (`0x59e4d2`); the others drop every row.
    let (kept, dropped): (Vec<AuctionEntry>, Vec<AuctionEntry>) = std::mem::take(&mut slot.entries)
        .into_iter()
        .partition(|e| which == OWNER && e.sold);
    if held {
        if let Some(w) = waits.as_deref_mut() {
            for e in &dropped {
                w.cancel_on(e.wire.item_entry, |x| x == callback);
            }
        }
    }
    a.pending_items[which] = 0;
    let count = page.auctions.len() as u32;
    let mut entries = kept;
    for wire in page.auctions {
        if !look.record(wire.item_entry) {
            a.pending_items[which] += 1;
            if let Some(w) = waits.as_deref_mut() {
                w.wait(wire.item_entry, callback, false);
            }
        }
        entries.push(AuctionEntry {
            expires_at: now + f64::from(wire.time_left_ms) / 1000.0,
            wire,
            sold: false,
        });
    }
    a.lists[which].entries = entries;
    a.lists[which].total = page.total_count;
    if which == BIDDER && count != page.total_count {
        a.can_get_bidder = true;
    }
    let owed = if which == OWNER {
        a.owner_settled()
    } else {
        a.pending_items[which] == 0
    };
    if owed {
        sort_and_signal(live, ui, which);
    }
}

fn on_pending_sales(In(ev): In<SessionEvent>, mut step: Step) {
    if let SessionEvent::AuctionPendingSales { sales } = ev {
        pending_sales(&sales, &mut step);
    }
}

/// `SMSG_AUCTION_LIST_PENDING_SALES` (`0x59e880`): the owner list's sold rows are replaced by the
/// auction mails in transit whose subject reads `item:property:2:auction`, each record in flight
/// linking the sold rows' callback; with nothing outstanding the list sorts and signals.
fn pending_sales(sales: &[AuctionPendingSale], step: &mut Step) {
    let Step { live, ui, waits } = step;
    let look = &live.look;
    let a = &mut *live.auction;
    debug!("ui_auction: pending sales: {} row(s)", sales.len());
    let now = look.now();
    a.answered(OWNER, now, None);
    let callback = RecordWait::Auction(Callback::Sold);
    let held = a.pending_sold != 0;
    let (sold, kept): (Vec<AuctionEntry>, Vec<AuctionEntry>) =
        std::mem::take(&mut a.lists[OWNER].entries)
            .into_iter()
            .partition(|e| e.sold);
    if held {
        if let Some(w) = waits.as_deref_mut() {
            for e in &sold {
                w.cancel_on(e.wire.item_entry, |x| x == callback);
            }
        }
    }
    a.pending_sold = 0;
    let mut entries = kept;
    for entry in sales.iter().filter_map(|s| sold_entry(s, now)) {
        if !look.record(entry.wire.item_entry) {
            a.pending_sold += 1;
            if let Some(w) = waits.as_deref_mut() {
                w.wait(entry.wire.item_entry, callback, false);
            }
        }
        entries.push(entry);
    }
    a.lists[OWNER].entries = entries;
    if a.owner_settled() {
        sort_and_signal(live, ui, OWNER);
    }
}

/// One pending sale as a sold owner row (`0x59a4a0` parses the subject, the body is
/// `buyer:bid:buyout:deposit:cut` with the buyer in hex), or `None` for any other mail; it expires
/// when its money is delivered (`0x59ebc3`).
fn sold_entry(sale: &AuctionPendingSale, now: f64) -> Option<AuctionEntry> {
    // The parse reads the first four fields and ignores the rest, so `item:0:2:id:count` is a sale.
    let fields: Vec<u32> = sale.subject.split(':').take(4).map(leading_int).collect();
    let [item, property, action, auction_id] = fields[..] else {
        return None;
    };
    if action != 2 {
        return None;
    }
    let mut body = sale.body.split(':');
    let buyer = body
        .next()
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
        .unwrap_or(0);
    let mut num = || {
        body.next()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0) as u32
    };
    let bid = num();
    let buyout = num();
    let wire = benilla_protocol::messages::AuctionListEntry {
        auction_id,
        item_entry: item,
        random_property_id: property as i32,
        bidder_guid: buyer,
        current_bid: bid,
        buyout,
        ..Default::default()
    };
    Some(AuctionEntry {
        wire,
        expires_at: now + f64::from(sale.days_left) * 86_400.0,
        sold: true,
    })
}

/// A subject field's number as the subject parse reads it (`0x59a4a0`): an optional `-`, then
/// the digits up to the first other character.
fn leading_int(field: &str) -> u32 {
    let (neg, digits) = match field.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, field),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u32, |n, d| {
            n.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
        });
    if neg {
        n.wrapping_neg()
    } else {
        n
    }
}

/// Which list callback a record or name read left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Callback {
    /// A row of the list's (`0x59cb70`, `0x59cbb0`, `0x59cc10`).
    List(usize),
    /// A sold owner row's (`0x59cbe0`).
    Sold,
    /// A notice's line (`0x59a400`): its message and the item's random property.
    Notice(&'static str, i32),
}

/// A record a list or notice waited on answered, in its turn.
pub(crate) fn record_callback(world: &mut World, entry: u32, landed: bool, callback: Callback) {
    if let Err(e) = world.run_system_cached_with(on_record, (entry, landed, callback)) {
        warn!("ui_auction: record callback: {e}");
    }
}

/// The record callbacks: a list's counts its rows down and sorts and signals on the last
/// (`0x59cb70`, `0x59cc10`), the owner list's with the sales and its replies in too (`0x59cbb0`,
/// `0x59cbe0`); a notice's shows its line once found (`0x59a400`).
fn on_record(In((entry, landed, callback)): In<(u32, bool, Callback)>, mut step: Step) {
    let Step { live, ui, .. } = &mut step;
    let a = &mut *live.auction;
    match callback {
        Callback::List(list) => {
            if a.pending_items[list] > 0 {
                a.pending_items[list] -= 1;
                if a.pending_items[list] > 0 {
                    return;
                }
            }
            if list != OWNER || a.owner_settled() {
                sort_and_signal(live, ui, list);
            }
        }
        Callback::Sold => {
            a.pending_sold = a.pending_sold.saturating_sub(1);
            if a.owner_settled() {
                sort_and_signal(live, ui, OWNER);
            }
        }
        Callback::Notice(key, property) => {
            if landed {
                if let Some(name) = live.look.item_name(entry, property) {
                    ui.line(UiError::s(key, name));
                }
            }
        }
    }
}

/// A bid notice's patch of `auction_id` on `list`: the bid, its step and bidder, and with `extend`
/// the expiry held at least the bid extension out; whether the row was there.
fn patch(
    a: &mut AuctionOpen,
    list: usize,
    auction_id: u32,
    (bid, min_increment, bidder): (u32, u32, u64),
    extend: Option<f64>,
) -> bool {
    let Some(e) = a.lists[list]
        .entries
        .iter_mut()
        .find(|e| e.wire.auction_id == auction_id)
    else {
        return false;
    };
    e.wire.current_bid = bid;
    e.wire.min_increment = min_increment;
    e.wire.bidder_guid = bidder;
    if let Some(now) = extend {
        e.expires_at = e.expires_at.max(now + BID_EXTENSION_SECS);
    }
    true
}

/// A patched row's list update, unless its records are in flight.
fn patched(a: &AuctionOpen, ui: &mut UiEvents, list: usize) {
    let quiet = match list {
        OWNER => a.pending_items[OWNER] != 0 || a.pending_sold != 0,
        l => a.pending_items[l] != 0,
    };
    if !quiet {
        ui.fire(list_event(list), Vec::new());
    }
}

/// The expiry's events (`0x59f2f0`), raised in turn.
fn expire_and_signal(live: &mut AuctionLive, ui: &mut UiEvents, auction_id: u32) {
    for event in expire(&mut live.auction, &live.look, auction_id) {
        ui.fire(event, Vec::new());
    }
}

/// The notices' mail arrival with its delay (`0x5704d0`), which signals when it lights the icon.
/// A handler's writes after its line: the line's listeners read the house as it was before them.
enum Tail {
    /// The owner list after a listing (`0x5a006f`, `0x5a0075`).
    Started,
    /// A placed bid: the outbid clear (`0x5a00a8`), the bidder list (`0x5a00b9`), the browse row's
    /// bid (`0x5a0112`, `0x5a0121`) and its event (`0x5a0195`), then the bid emote (`0x5a01d5`).
    BidPlaced { auction_id: u32, min_increment: u32 },
    /// The mail, when one arrives, then the expiry (`0x5a0208` -> `0x5a0211`, `0x5a0aa2` ->
    /// `0x5a0aab`, `0x5a0b7f`).
    Expired {
        auction_id: u32,
        mail: Option<(f32, &'static str)>,
    },
    /// A bidder notice: won expires the auction (`0x5a076c`); outbid keeps the id (`0x5a0598`) and
    /// patches this house's bidder and browse rows (`0x5a0674`, `0x5a073a`); either mails
    /// (`0x5a077a`).
    Bidder(AuctionBidderNotification),
}

/// Show `line` at the handler's step and run `tail` after its listeners (`0x5216f0` returns
/// before the handler goes on).
fn line_then(ui: &mut UiEvents, line: UiError, tail: Tail) {
    crate::ui_action::show_in_turn_then(ui, "ui_lines", line, move |world| {
        if let Err(e) = world.run_system_cached_with(run_tail, tail) {
            warn!("ui_auction: a handler's tail did not run: {e}");
        }
    });
}

/// A notice's line, then `tail`; with the item's record not cached the line waits on it
/// (`0x59a400`) and `tail` runs at once.
fn notice_then(
    step: &mut Step,
    mail: &mut MailArrival,
    (key, item, property): (&'static str, u32, i32),
    tail: Tail,
) {
    match notice_line(step, key, item, property) {
        Some(line) => line_then(&mut step.ui, line, tail),
        None => apply_tail(tail, step, mail),
    }
}

fn run_tail(In(tail): In<Tail>, mut step: Step, mut mail: MailArrival) {
    apply_tail(tail, &mut step, &mut mail);
}

/// Run a handler's [`Tail`], its events raised in turn.
fn apply_tail(tail: Tail, step: &mut Step, mail: &mut MailArrival) {
    let Step { live, ui, .. } = step;
    match tail {
        Tail::Started => {
            live.auction.can_get_owner = true;
            owner_list(&mut live.auction, &live.look);
        }
        Tail::BidPlaced {
            auction_id,
            min_increment,
        } => {
            let look = &live.look;
            let a = &mut *live.auction;
            // The outbid clear drops the first entry of the id alone (`0x59ba10`).
            if let Some(i) = a.outbid.iter().position(|&(_, id)| id == auction_id) {
                a.outbid.remove(i);
            }
            a.can_get_bidder = true;
            let page = a.last_bidder_page;
            bidder_list(a, look, page);
            if a.auctioneer.is_some() {
                let me = look.player().unwrap_or(0);
                let bid = (a.last_bid_amount, min_increment, me);
                if patch(a, LIST, auction_id, bid, Some(look.now())) {
                    patched(a, ui, LIST);
                }
            }
            look.send(ClientCommand::Emote {
                emote_id: BID_EMOTE,
            });
        }
        Tail::Expired {
            auction_id,
            mail: arrival,
        } => {
            if let Some((delay, source)) = arrival {
                mail_arrived(mail, ui, delay, source);
            }
            expire_and_signal(live, ui, auction_id);
        }
        Tail::Bidder(n) => {
            if n.bid_or_zero == 0 {
                expire_and_signal(live, ui, n.auction_id);
            } else {
                let now = live.look.now();
                let a = &mut *live.auction;
                a.outbid.push((n.house_id, n.auction_id));
                if a.house_id == n.house_id {
                    for list in [BIDDER, LIST] {
                        let bid = (n.bid_or_zero, n.out_bid, n.bidder_guid);
                        if patch(a, list, n.auction_id, bid, Some(now)) {
                            patched(a, ui, list);
                        }
                    }
                }
            }
            mail_arrived(mail, ui, 0.0, "SMSG_AUCTION_BIDDER_NOTIFICATION");
        }
    }
}

fn mail_arrived(mail: &mut MailArrival, ui: &mut UiEvents, delay: f32, source: &str) {
    if mail.arrive(delay, source) {
        ui.fire("UPDATE_PENDING_MAIL", Vec::new());
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut step: Step) {
    if let SessionEvent::AuctionCommandResult {
        auction_id,
        action,
        error,
        tail,
    } = ev
    {
        command_result(auction_id, action, error, &tail, &mut step);
    }
}

/// `SMSG_AUCTION_COMMAND_RESULT` (`0x59ffb0`), the verdict on a sell, a cancel or a bid, each step
/// raised in turn.
fn command_result(
    auction_id: u32,
    action: u32,
    error: u32,
    tail: &AuctionCommandTail,
    step: &mut Step,
) {
    let Step { live, ui, .. } = step;
    let look = &live.look;
    let a = &mut *live.auction;
    a.wire.last_command = Some((auction_id, action, error));
    a.wire.results.push((auction_id, action, error));
    debug!("ui_auction: command result id {auction_id} action {action} error {error} {tail:?}");
    // A failed listing ends a pending multi-sell (`0x5a0014`).
    if action == auction_action::STARTED && error != auction_error::OK && a.pending_sell() {
        a.multisell = None;
        ui.write(|s| s.end_auction_multisell(true));
    }
    match error {
        auction_error::OK => match action {
            auction_action::STARTED => {
                if let Some((done, total)) = a
                    .multisell
                    .as_ref()
                    .filter(|m| m.pending)
                    .map(|m| (m.total - m.remaining, m.total))
                {
                    ui.fire(
                        "AUCTION_MULTISELL_UPDATE",
                        vec![
                            ScriptValue::Int(i64::from(done)),
                            ScriptValue::Int(i64::from(total)),
                        ],
                    );
                    let next = sell_next(a, look);
                    ui.write(move |s| s.auction_sell_step(next));
                }
                // The line (`0x5a0067`), then the owner list (`0x5a006f`, `0x5a0075`).
                line_then(ui, UiError::key("ERR_AUCTION_STARTED"), Tail::Started);
            }
            auction_action::BID_PLACED => {
                // The line (`0x5a009f`), then the house's writes and its event after its listeners.
                let min_increment = match tail {
                    AuctionCommandTail::BidPlaced { new_min_outbid } => *new_min_outbid,
                    _ => 0,
                };
                line_then(
                    ui,
                    UiError::key("ERR_AUCTION_BID_PLACED"),
                    Tail::BidPlaced {
                        auction_id,
                        min_increment,
                    },
                );
            }
            auction_action::REMOVED => {
                // The line (`0x5a01fe`), then the mail (`0x5a0208`) and the expiry (`0x5a0211`).
                line_then(
                    ui,
                    UiError::key("ERR_AUCTION_REMOVED"),
                    Tail::Expired {
                        auction_id,
                        mail: Some((0.0, "SMSG_AUCTION_COMMAND_RESULT cancel")),
                    },
                );
            }
            _ => {}
        },
        auction_error::INVENTORY => {
            if let AuctionCommandTail::Inventory { result } = tail {
                ui.line(UiError::key(crate::ui_items::equip_error_key(*result)));
            }
        }
        auction_error::NOT_ENOUGH_MONEY => ui.line(UiError::key("ERR_NOT_ENOUGH_MONEY")),
        auction_error::ITEM_NOT_FOUND => ui.line(UiError::key("ERR_ITEM_NOT_FOUND")),
        auction_error::HIGHER_BID => {
            if let AuctionCommandTail::HigherBid {
                new_bidder_guid,
                new_bid,
                new_min_outbid,
            } = *tail
            {
                if a.auctioneer.is_some() {
                    for list in [LIST, BIDDER] {
                        let bid = (new_bid, new_min_outbid, new_bidder_guid);
                        if patch(a, list, auction_id, bid, None) {
                            patched(a, ui, list);
                        }
                    }
                }
            }
            ui.line(UiError::key("ERR_AUCTION_HIGHER_BID"));
        }
        auction_error::BID_INCREMENT => ui.line(UiError::key("ERR_AUCTION_BID_INCREMENT")),
        auction_error::BID_OWN => ui.line(UiError::key("ERR_AUCTION_BID_OWN")),
        auction_error::RESTRICTED_ACCOUNT => ui.line(UiError::key("ERR_RESTRICTED_ACCOUNT")),
        _ => ui.line(UiError::key("ERR_AUCTION_DATABASE_ERROR")),
    }
}

/// A notice's `_S` line to show now with the item's record cached, else `None` and the line left
/// to its record callback (`0x59a400`).
fn notice_line(step: &mut Step, key: &'static str, item: u32, property: i32) -> Option<UiError> {
    let Step { live, waits, .. } = step;
    let name = live.look.item_name(item, property);
    if name.is_none() {
        if let Some(w) = waits.as_deref_mut() {
            w.wait(
                item,
                RecordWait::Auction(Callback::Notice(key, property)),
                false,
            );
        }
    }
    name.map(|name| UiError::s(key, name))
}

fn on_bidder_notification(In(ev): In<SessionEvent>, mut step: Step, mut mail: MailArrival) {
    if let SessionEvent::AuctionBidderNotification(n) = ev {
        bidder_notification(&n, &mut step, &mut mail);
    }
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION` (`0x5a0480`): outbid shows its line, remembers the id for the
/// next bidder query and patches this house's bidder and browse rows; won shows its line and
/// expires the auction; either mails at once.
fn bidder_notification(n: &AuctionBidderNotification, step: &mut Step, mail: &mut MailArrival) {
    let won = n.bid_or_zero == 0;
    let key = if won {
        "ERR_AUCTION_WON_S"
    } else {
        "ERR_AUCTION_OUTBID_S"
    };
    // The line (`0x5a057c`, `0x5a0754`), then the house's writes after its listeners.
    let line = (key, n.item_entry, n.random_property_id);
    notice_then(step, mail, line, Tail::Bidder(*n));
}

fn on_owner_notification(In(ev): In<SessionEvent>, mut step: Step, mut mail: MailArrival) {
    if let SessionEvent::AuctionOwnerNotification(n) = ev {
        owner_notification(&n, &mut step, &mut mail);
    }
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION` (`0x5a0790`): a bidder patches our owner and browse rows with
/// the house open; a zero bidder closes the auction, sold with a bid and expired without: its line,
/// the mail (a sale's after its delay), then the expiry.
fn owner_notification(n: &AuctionOwnerNotification, step: &mut Step, mail: &mut MailArrival) {
    if n.bidder_guid != 0 {
        let Step { live, ui, .. } = step;
        let now = live.look.now();
        let a = &mut *live.auction;
        if a.auctioneer.is_none() {
            return;
        }
        for list in [OWNER, LIST] {
            let bid = (n.bid, n.out_bid, n.bidder_guid);
            if patch(a, list, n.auction_id, bid, Some(now)) {
                patched(a, ui, list);
            }
        }
        return;
    }
    let key = if n.bid != 0 {
        "ERR_AUCTION_SOLD_S"
    } else {
        "ERR_AUCTION_EXPIRED_S"
    };
    // The line (`0x5a0a5e`, `0x5a0a7d`), then the mail (`0x5a0aa2`) and the expiry (`0x5a0aab`).
    let delay = if n.bid != 0 { n.mail_delay } else { 0.0 };
    notice_then(
        step,
        mail,
        (key, n.item_entry, n.random_property_id),
        Tail::Expired {
            auction_id: n.auction_id,
            mail: Some((delay, "SMSG_AUCTION_OWNER_NOTIFICATION")),
        },
    );
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION` (`0x5a0ac0`): its line, then the expiry.
fn on_removed_notification(In(ev): In<SessionEvent>, mut step: Step, mut mail: MailArrival) {
    if let SessionEvent::AuctionRemovedNotification {
        auction_id,
        item_entry,
        random_property_id,
    } = ev
    {
        // The line (`0x5a0b73`), then the expiry (`0x5a0b7f`).
        notice_then(
            &mut step,
            &mut mail,
            ("ERR_AUCTION_REMOVED_S", item_entry, random_property_id),
            Tail::Expired {
                auction_id,
                mail: None,
            },
        );
    }
}

/// A player's name answered: each list callback a row read left on it signals its list once found
/// (`0x59a380`-`0x59a3c0`); a placeholder answer counts as found (`0x635894`).
fn on_name_record(
    In(ev): In<SessionEvent>,
    mut auction: ResMut<AuctionOpen>,
    ui: Option<ResMut<UiEvents>>,
) {
    use benilla_protocol::messages::NameQueryMiss;
    let (guid, found) = match ev {
        SessionEvent::PlayerName { guid, .. } => (guid, true),
        SessionEvent::PlayerNameMiss { guid, miss } => (guid, miss == NameQueryMiss::Placeholder),
        _ => return,
    };
    let mut lists = Vec::new();
    auction.name_waits.retain(|&(g, list)| {
        let hit = g == guid;
        if hit {
            lists.push(list);
        }
        !hit
    });
    if let (true, Some(mut ui)) = (found, ui) {
        for list in lists {
            ui.fire(list_event(list), Vec::new());
        }
    }
}

/// The house dies with the socket, the outbid ids with the game's shutdown (`0x59e090`).
fn on_session_end(In(_): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    *auction = AuctionOpen::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::AuctionListEntry;
    use bevy::ecs::system::RunSystemOnce;

    const AUCTIONEER: u64 = 0xF130_0000_0000_0010;
    const LINEN: u32 = 2589;
    const LATE: u32 = 9999;

    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AuctionOpen>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<UiEvents>()
            .init_resource::<crate::ui_mail::MailOpen>()
            .init_resource::<crate::ui_mail::MailPending>()
            .insert_resource(crate::net::NetCommands(tx));
        app.world_mut()
            .insert_non_send_resource(RecordWaits::default());
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
        register(&mut app);
        (app, rx)
    }

    fn open(app: &mut App) {
        app.world_mut()
            .resource_mut::<AuctionOpen>()
            .open(AUCTIONEER, 1);
    }

    /// The events and lines the handlers queued since the last read, the lines by key, each
    /// line's step run where it stands so the writes after it show in order.
    fn raised(app: &mut App) -> Vec<&'static str> {
        let queued = UiEvents::walk_for_test(app.world_mut());
        let mut lines = crate::ui_action::take_lines_in_turn().into_iter();
        queued
            .into_iter()
            .map(|n| match n {
                "<world>" => lines.next().map_or(n, |l| l.key),
                _ => n,
            })
            .collect()
    }

    fn sends(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .map(|c| format!("{c:?}"))
            .map(|c| c.split([' ', '{']).next().unwrap_or_default().to_string())
            .collect()
    }

    fn row(auction_id: u32, item_entry: u32) -> AuctionListEntry {
        AuctionListEntry {
            auction_id,
            item_entry,
            count: 5,
            start_bid: 100,
            time_left_ms: 3_600_000,
            ..Default::default()
        }
    }

    fn page(rows: Vec<AuctionListEntry>) -> AuctionListPage {
        AuctionListPage {
            total_count: rows.len() as u32,
            auctions: rows,
            query_delay_ms: Some(300),
        }
    }

    fn list(app: &mut App, rows: Vec<AuctionListEntry>) {
        app.world_mut()
            .run_system_once_with(on_list_result, SessionEvent::AuctionListResult(page(rows)))
            .unwrap();
    }

    fn command(app: &mut App, auction_id: u32, action: u32, error: u32, tail: AuctionCommandTail) {
        app.world_mut()
            .run_system_once_with(
                on_command_result,
                SessionEvent::AuctionCommandResult {
                    auction_id,
                    action,
                    error,
                    tail,
                },
            )
            .unwrap();
    }

    fn ids(app: &App, list: usize) -> Vec<u32> {
        app.world().resource::<AuctionOpen>().lists[list]
            .entries
            .iter()
            .map(|e| e.wire.auction_id)
            .collect()
    }

    #[test]
    fn the_table_routes_the_hello_and_the_session_end_to_the_house() {
        let (mut app, _rx) = app();
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionHello {
                auctioneer: 0x10,
                house_id: 1,
                enabled: true,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, Some(0x10));
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, None);
    }

    /// A closed house opens nothing and fires `AUCTION_HOUSE_DISABLED` from its handler
    /// (`0x59ff9a`).
    #[test]
    fn a_closed_house_opens_nothing() {
        let (mut app, _rx) = app();
        app.world_mut()
            .run_system_once_with(
                on_hello,
                SessionEvent::AuctionHello {
                    auctioneer: 0x10,
                    house_id: 1,
                    enabled: false,
                },
            )
            .unwrap();
        assert_eq!(app.world().resource::<AuctionOpen>().auctioneer, None);
        assert_eq!(raised(&mut app), vec!["AUCTION_HOUSE_DISABLED"]);
    }

    /// A page with a row's record in flight signals nothing (`0x59e432`); the record's callback
    /// counts it down, sorts and signals (`0x59cb70`). A page landing first drops the callbacks the
    /// old rows left (`0x59e1ba`).
    #[test]
    fn a_page_signals_once_every_records_in() {
        let (mut app, _rx) = app();
        open(&mut app);
        list(&mut app, vec![row(1, LINEN), row(2, LATE)]);
        assert!(
            raised(&mut app).is_empty(),
            "a record in flight holds the update"
        );
        let callback = RecordWait::Auction(Callback::List(LIST));
        assert!(app
            .world()
            .non_send_resource::<RecordWaits>()
            .waiting()
            .contains(&(LATE, callback)));
        record_callback(app.world_mut(), LATE, true, Callback::List(LIST));
        assert_eq!(raised(&mut app), vec!["AUCTION_ITEM_LIST_UPDATE"]);

        list(&mut app, vec![row(3, LATE)]);
        list(&mut app, vec![row(4, LINEN)]);
        assert!(
            !app.world()
                .non_send_resource::<RecordWaits>()
                .waiting()
                .contains(&(LATE, callback)),
            "the next page dropped the callback"
        );
        assert_eq!(raised(&mut app), vec!["AUCTION_ITEM_LIST_UPDATE"]);
        assert_eq!(ids(&app, LIST), vec![4]);
    }

    /// The owner list signals with both of its replies in and its records landed (`0x59e86a`); it
    /// keeps its sold rows, and the sales keep the live ones.
    #[test]
    fn the_owner_list_signals_with_both_replies_in() {
        let (mut app, _rx) = app();
        open(&mut app);
        app.world_mut().resource_mut::<AuctionOpen>().pending[OWNER] = 2;
        app.world_mut()
            .run_system_once_with(
                on_owner_list_result,
                SessionEvent::AuctionOwnerListResult(page(vec![row(1, LINEN), row(2, LINEN)])),
            )
            .unwrap();
        assert!(raised(&mut app).is_empty(), "the sales are still owed");
        let sale = AuctionPendingSale {
            subject: format!("{LINEN}:0:2:9"),
            body: "0000000000000505:150:400:20:5".into(),
            mail_id: 1,
            mail_template: 0,
            days_left: 0.5,
        };
        // A non-sale mail is not a row.
        let other = AuctionPendingSale {
            subject: format!("{LINEN}:0:1:8"),
            ..sale.clone()
        };
        app.world_mut()
            .run_system_once_with(
                on_pending_sales,
                SessionEvent::AuctionPendingSales {
                    sales: vec![sale, other],
                },
            )
            .unwrap();
        assert_eq!(raised(&mut app), vec!["AUCTION_OWNED_LIST_UPDATE"]);
        let a = app.world().resource::<AuctionOpen>();
        let sold = a.lists[OWNER]
            .entries
            .iter()
            .find(|e| e.sold)
            .expect("the sale");
        assert_eq!(
            (
                sold.wire.auction_id,
                sold.wire.bidder_guid,
                sold.wire.current_bid,
                sold.wire.buyout
            ),
            (9, 0x505, 150, 400)
        );
        assert_eq!(a.lists[OWNER].entries.len(), 3);
        app.world_mut()
            .run_system_once_with(
                on_owner_list_result,
                SessionEvent::AuctionOwnerListResult(page(vec![row(3, LINEN)])),
            )
            .unwrap();
        let mut kept = ids(&app, OWNER);
        kept.sort_unstable();
        assert_eq!(
            kept,
            vec![3, 9],
            "the sold row stays, the live ones are replaced"
        );
    }

    /// The subject parse reads four fields and ignores the rest (`0x59a4a0`): the servers' own
    /// `item:0:2:auction:count` is a sale, and each field is its leading number.
    #[test]
    fn a_sale_subject_reads_its_first_four_fields() {
        let sale = |subject: &str| AuctionPendingSale {
            subject: subject.into(),
            body: "               5:150:400:20:5:0:0".into(),
            mail_id: 1,
            mail_template: 0,
            days_left: 0.5,
        };
        let e = sold_entry(&sale(&format!("{LINEN}:0:2:9:5")), 0.0).expect("a sale");
        assert_eq!(
            (e.wire.item_entry, e.wire.auction_id, e.wire.bidder_guid),
            (LINEN, 9, 5)
        );
        assert_eq!(
            sold_entry(&sale(&format!("{LINEN}:0:2:9x")), 0.0).map(|e| e.wire.auction_id),
            Some(9)
        );
        assert!(sold_entry(&sale(&format!("{LINEN}:0:2")), 0.0).is_none());
        assert!(sold_entry(&sale(&format!("{LINEN}:0:3:9:5")), 0.0).is_none());
    }

    /// A placed bid clears the first outbid entry of its id alone (`0x59ba10`).
    #[test]
    fn a_placed_bid_clears_one_outbid_entry() {
        let (mut app, _rx) = app();
        open(&mut app);
        app.world_mut().resource_mut::<AuctionOpen>().outbid = vec![(1, 4), (1, 5), (1, 4)];
        command(
            &mut app,
            4,
            auction_action::BID_PLACED,
            auction_error::OK,
            AuctionCommandTail::BidPlaced { new_min_outbid: 1 },
        );
        let _ = raised(&mut app);
        assert_eq!(
            app.world().resource::<AuctionOpen>().outbid,
            vec![(1, 5), (1, 4)]
        );
    }

    /// The world leave closes an open house (`0x528d55`): `AUCTION_HOUSE_CLOSED`, then the house
    /// empties; a closed house raises nothing.
    #[test]
    fn the_world_leave_closes_the_house() {
        let (mut app, _rx) = app();
        super::super::leave_world(app.world_mut());
        assert!(raised(&mut app).is_empty(), "no house, no event");
        open(&mut app);
        list(&mut app, vec![row(1, LINEN)]);
        let _ = raised(&mut app);
        super::super::leave_world(app.world_mut());
        assert_eq!(
            app.world().resource::<UiEvents>().names(),
            vec!["AUCTION_HOUSE_CLOSED", "<world>"],
            "the event fires with the house still open"
        );
        assert_eq!(
            app.world().resource::<AuctionOpen>().auctioneer,
            Some(AUCTIONEER)
        );
        crate::ui_script::live::fire_handler_events(app.world_mut());
        let a = app.world().resource::<AuctionOpen>();
        assert_eq!(a.auctioneer, None);
        assert!(a.lists[LIST].entries.is_empty());
    }

    /// The game start sets the criteria back to their defaults and forgets the outbid ids
    /// (`0x5a0ba0`); the world entry zeroes the gates and pending counts (`0x59dfb0`).
    #[test]
    fn the_game_start_resets_the_criteria_and_outbids() {
        let mut a = AuctionOpen::default();
        a.criteria.0[LIST] = vec![(5, true)];
        a.outbid.push((1, 4));
        a.last_get_all = Some(1.0);
        a.initialize_game();
        assert_eq!(
            a.criteria.0[LIST],
            benilla_ui::script::auction_default_criteria()[LIST]
        );
        assert!(a.outbid.is_empty());
        assert_eq!(a.last_get_all, None);
        a.open(AUCTIONEER, 1);
        a.pending_items[LIST] = 2;
        a.pending[OWNER] = 1;
        a.enter_world();
        assert_eq!(
            (a.auctioneer, a.pending_items, a.pending),
            (None, [0; 3], [0; 3])
        );
    }

    /// A placed bid (`0x59ffb0`): its line first, then the bidder list asked again, the browse row
    /// patched and signalled, and the emote.
    #[test]
    fn a_placed_bid_shows_its_line_then_patches_the_row() {
        let (mut app, rx) = app();
        open(&mut app);
        list(&mut app, vec![row(4, LINEN)]);
        let _ = raised(&mut app);
        {
            let mut a = app.world_mut().resource_mut::<AuctionOpen>();
            a.outbid.push((1, 4));
            a.last_bid_amount = 250;
        }
        command(
            &mut app,
            4,
            auction_action::BID_PLACED,
            auction_error::OK,
            AuctionCommandTail::BidPlaced { new_min_outbid: 13 },
        );
        assert_eq!(
            raised(&mut app),
            vec!["ERR_AUCTION_BID_PLACED", "AUCTION_ITEM_LIST_UPDATE"]
        );
        assert_eq!(sends(&rx), vec!["AuctionListBidderItems", "Emote"]);
        let a = app.world().resource::<AuctionOpen>();
        let e = &a.lists[LIST].entries[0];
        assert_eq!((e.wire.current_bid, e.wire.min_increment), (250, 13));
        assert!(a.outbid.is_empty());
    }

    #[test]
    fn outcomes_and_refusals_show_their_lines() {
        let tail = AuctionCommandTail::Empty;
        for (action, error, key) in [
            (
                auction_action::STARTED,
                auction_error::OK,
                "ERR_AUCTION_STARTED",
            ),
            (
                auction_action::REMOVED,
                auction_error::OK,
                "ERR_AUCTION_REMOVED",
            ),
            (auction_action::BID_PLACED, 10, "ERR_AUCTION_BID_OWN"),
            (auction_action::BID_PLACED, 5, "ERR_AUCTION_HIGHER_BID"),
            (auction_action::STARTED, 9, "ERR_AUCTION_DATABASE_ERROR"),
        ] {
            let (mut app, _rx) = app();
            command(&mut app, 7, action, error, tail);
            let lines: Vec<&str> = crate::ui_action::take_lines_in_turn()
                .into_iter()
                .map(|l| l.key)
                .collect();
            assert_eq!(lines, vec![key], "action {action} error {error}");
        }
    }

    /// A sale (`0x5a0790`): its line, then the expiry removes the auction from every list, the
    /// owner list asking the sales again (`0x59cd90`) and the browse list signalling.
    #[test]
    fn a_sale_removes_the_auction_everywhere() {
        let (mut app, rx) = app();
        open(&mut app);
        app.world_mut()
            .run_system_once_with(
                on_owner_list_result,
                SessionEvent::AuctionOwnerListResult(page(vec![row(7, LINEN), row(8, LINEN)])),
            )
            .unwrap();
        list(&mut app, vec![row(7, LINEN)]);
        let _ = raised(&mut app);
        let _ = sends(&rx);
        // The owner reply's delay past, the sales may be asked again (`0x59cd99`).
        app.world_mut().resource_mut::<AuctionOpen>().delay_secs = 0.0;
        app.world_mut()
            .run_system_once_with(
                on_owner_notification,
                SessionEvent::AuctionOwnerNotification(AuctionOwnerNotification {
                    auction_id: 7,
                    bid: 500,
                    out_bid: 0,
                    bidder_guid: 0,
                    item_entry: LINEN,
                    random_property_id: 0,
                    mail_delay: 3600.0,
                }),
            )
            .unwrap();
        assert_eq!(
            raised(&mut app),
            vec!["ERR_AUCTION_SOLD_S", "AUCTION_ITEM_LIST_UPDATE"]
        );
        assert_eq!(ids(&app, OWNER), vec![8]);
        assert!(ids(&app, LIST).is_empty());
        assert!(
            sends(&rx).contains(&"AuctionListPendingSales".to_string()),
            "the owner list asks the sales again"
        );
    }

    #[test]
    fn an_outbid_patches_this_house_and_remembers_the_id() {
        let (mut app, _rx) = app();
        app.world_mut()
            .resource_mut::<AuctionOpen>()
            .open(AUCTIONEER, 2);
        app.world_mut()
            .run_system_once_with(
                on_bidder_list_result,
                SessionEvent::AuctionBidderListResult(page(vec![row(3, LINEN)])),
            )
            .unwrap();
        let _ = raised(&mut app);
        app.world_mut()
            .run_system_once_with(
                on_bidder_notification,
                SessionEvent::AuctionBidderNotification(AuctionBidderNotification {
                    house_id: 2,
                    auction_id: 3,
                    bidder_guid: 0x66,
                    bid_or_zero: 900,
                    out_bid: 45,
                    item_entry: LINEN,
                    random_property_id: 0,
                }),
            )
            .unwrap();
        // The refund is mailed at once (`0x5a077a`).
        assert_eq!(
            raised(&mut app),
            vec![
                "ERR_AUCTION_OUTBID_S",
                "AUCTION_BIDDER_LIST_UPDATE",
                "UPDATE_PENDING_MAIL"
            ]
        );
        let a = app.world().resource::<AuctionOpen>();
        let e = &a.lists[BIDDER].entries[0];
        assert_eq!((e.wire.current_bid, e.wire.bidder_guid), (900, 0x66));
        assert_eq!(a.outbid, vec![(2, 3)]);
    }

    /// A notice whose item record is in flight shows its line from the record's callback
    /// (`0x59a400`).
    #[test]
    fn a_notice_line_waits_on_its_item() {
        let (mut app, _rx) = app();
        app.world_mut()
            .run_system_once_with(
                on_removed_notification,
                SessionEvent::AuctionRemovedNotification {
                    auction_id: 5,
                    item_entry: LATE,
                    random_property_id: 0,
                },
            )
            .unwrap();
        assert!(raised(&mut app).is_empty());
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(LATE, Some(crate::items::test_template("Charz Late")));
        record_callback(
            app.world_mut(),
            LATE,
            true,
            Callback::Notice("ERR_AUCTION_REMOVED_S", 0),
        );
        let lines = crate::ui_action::take_lines_in_turn();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].key, "ERR_AUCTION_REMOVED_S");
    }

    /// The notices that mail something run the mail arrival themselves (#449): a closed sale with
    /// its delay, a won or outbid notice and a cancel at once; a bid on our auction mails nothing.
    #[test]
    fn auction_notices_that_mail_light_the_mail_icon() {
        let (mut app, rx) = app();
        let has_new_mail = |app: &App| {
            app.world()
                .resource::<crate::ui_mail::MailPending>()
                .has_new_mail()
        };
        let owner = |bidder_guid: u64, bid: u32, mail_delay: f32| {
            SessionEvent::AuctionOwnerNotification(AuctionOwnerNotification {
                auction_id: 7,
                bid,
                out_bid: 0,
                bidder_guid,
                item_entry: LINEN,
                random_property_id: 0,
                mail_delay,
            })
        };

        crate::net::handlers::dispatch(app.world_mut(), vec![owner(0x99, 500, 0.0)]);
        assert!(!has_new_mail(&app), "a bid mails nothing");
        assert!(rx.try_recv().is_err());

        crate::net::handlers::dispatch(app.world_mut(), vec![owner(0, 500, 0.0)]);
        assert!(has_new_mail(&app), "a sale with no delay lights at once");
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::QueryNextMailTime)));

        // A sale whose money waits: the icon lights when the delay runs out, not before.
        *app.world_mut()
            .resource_mut::<crate::ui_mail::MailPending>() = Default::default();
        crate::net::handlers::dispatch(app.world_mut(), vec![owner(0, 500, 3600.0)]);
        assert!(!has_new_mail(&app));

        *app.world_mut()
            .resource_mut::<crate::ui_mail::MailPending>() = Default::default();
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionBidderNotification(
                AuctionBidderNotification {
                    house_id: 1,
                    auction_id: 8,
                    bidder_guid: 0x99,
                    bid_or_zero: 0,
                    out_bid: 0,
                    item_entry: LINEN,
                    random_property_id: 0,
                },
            )],
        );
        assert!(has_new_mail(&app), "a won auction lights at once");

        *app.world_mut()
            .resource_mut::<crate::ui_mail::MailPending>() = Default::default();
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionCommandResult {
                auction_id: 9,
                action: auction_action::REMOVED,
                error: auction_error::OK,
                tail: AuctionCommandTail::Empty,
            }],
        );
        assert!(has_new_mail(&app), "a cancel's item comes back at once");
    }
}
