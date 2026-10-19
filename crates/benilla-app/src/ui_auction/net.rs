//! The auction house's packet handlers, after the 12340 handlers they name: every
//! `SMSG_AUCTION_*` becomes state on [`AuctionOpen`], which [`super::feed_auction`] turns into
//! the window's events and [`super::drain_auction`] into the follow-up sends.

use benilla_protocol::messages::{
    auction_action, auction_error, AuctionBidderNotification, AuctionCommandTail, AuctionListPage,
    AuctionOwnerNotification, AuctionPendingSale,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{BIDDER, LIST, OWNER};
use bevy::prelude::*;

use super::{AuctionEntry, AuctionMessage, AuctionOpen, Patch};
use crate::net::NetHandlerApp;

/// One handler per `SMSG_AUCTION_*` kind, plus the session-end listener.
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
        .net_handler(K::Disconnected, on_session_end);
}

fn now(time: &Time) -> f64 {
    time.elapsed_secs_f64()
}

/// `MSG_AUCTION_HELLO`'s reply (`0x59ff40`): an open house sets the auctioneer and shows the
/// window (`SetAuctioneer` `0x59f000`); a closed one fires `AUCTION_HOUSE_DISABLED`.
fn on_hello(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionHello {
        auctioneer,
        house_id,
        enabled,
    } = ev
    {
        if enabled {
            auction.open(auctioneer, house_id);
        } else {
            auction.disabled_requested = true;
        }
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionCommandResult {
        auction_id,
        action,
        error,
        tail,
    } = ev
    {
        auction_command_result(auction_id, action, error, &tail, &mut auction);
    }
}

fn on_list_result(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>, time: Res<Time>) {
    if let SessionEvent::AuctionListResult(page) = ev {
        list_result(LIST, page, now(&time), &mut auction);
    }
}

fn on_owner_list_result(
    In(ev): In<SessionEvent>,
    mut auction: ResMut<AuctionOpen>,
    time: Res<Time>,
) {
    if let SessionEvent::AuctionOwnerListResult(page) = ev {
        list_result(OWNER, page, now(&time), &mut auction);
    }
}

fn on_bidder_list_result(
    In(ev): In<SessionEvent>,
    mut auction: ResMut<AuctionOpen>,
    time: Res<Time>,
) {
    if let SessionEvent::AuctionBidderListResult(page) = ev {
        list_result(BIDDER, page, now(&time), &mut auction);
    }
}

fn on_pending_sales(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>, time: Res<Time>) {
    if let SessionEvent::AuctionPendingSales { sales } = ev {
        pending_sales(&sales, now(&time), &mut auction);
    }
}

fn on_bidder_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionBidderNotification(n) = ev {
        auction_bidder_notification(&n, &mut auction);
    }
}

fn on_owner_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionOwnerNotification(n) = ev {
        auction_owner_notification(&n, &mut auction);
    }
}

fn on_removed_notification(In(ev): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    if let SessionEvent::AuctionRemovedNotification {
        auction_id,
        item_entry,
        random_property_id,
    } = ev
    {
        auction.messages.push(AuctionMessage::item(
            "ERR_AUCTION_REMOVED_S",
            item_entry,
            random_property_id,
        ));
        auction.expire(auction_id);
    }
}

/// The window dies with the socket.
fn on_session_end(In(_): In<SessionEvent>, mut auction: ResMut<AuctionOpen>) {
    auction.clear();
}

/// Any of the three list results (`0x59e160`, `0x59e480`, `0x59ecd0`): the query is answered,
/// the page replaces the list's, and its delay paces the next query.
fn list_result(which: usize, page: AuctionListPage, now: f64, auction: &mut AuctionOpen) {
    debug!(
        "ui_auction: list {which} result: {} row(s) {:?}, total {}, delay {:?}",
        page.auctions.len(),
        page.auctions
            .iter()
            .map(|a| a.auction_id)
            .collect::<Vec<_>>(),
        page.total_count,
        page.query_delay_ms
    );
    auction.wire.list_results[which] += 1;
    auction.answered(which, now, page.query_delay_ms);
    // A page in flight when the window closed has nowhere to go.
    if auction.auctioneer.is_none() {
        return;
    }
    let count = page.auctions.len() as u32;
    let entries = page.auctions.into_iter().map(AuctionEntry::live).collect();
    if which == BIDDER && count != page.total_count {
        auction.can_get_bidder = true;
    }
    auction.set_list(which, entries, page.total_count);
}

/// `SMSG_AUCTION_LIST_PENDING_SALES` (`0x59e880`): the owner list's sold rows are replaced by
/// the auction mails in transit whose subject reads `item:property:2:auction`.
fn pending_sales(sales: &[AuctionPendingSale], now: f64, auction: &mut AuctionOpen) {
    debug!("ui_auction: pending sales: {} row(s)", sales.len());
    auction.answered(OWNER, now, None);
    if auction.auctioneer.is_none() {
        return;
    }
    let sold: Vec<AuctionEntry> = sales.iter().filter_map(sold_entry).collect();
    auction.set_sold(sold);
}

/// One pending sale as a sold owner row (`0x59a4a0` parses the subject, the body is
/// `buyer:bid:buyout:deposit:cut` with the buyer in hex), or `None` for any other mail.
fn sold_entry(sale: &AuctionPendingSale) -> Option<AuctionEntry> {
    let fields: Vec<u32> = sale
        .subject
        .split(':')
        .map(|f| f.trim().parse::<i64>().unwrap_or(0) as u32)
        .collect();
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
        left_secs: f64::from(sale.days_left) * 86_400.0,
        expires_at: None,
        extend: false,
        sold: true,
    })
}

/// `SMSG_AUCTION_COMMAND_RESULT` (`0x59ffb0`), the verdict on a sell, a cancel or a bid.
fn auction_command_result(
    auction_id: u32,
    action: u32,
    error: u32,
    tail: &AuctionCommandTail,
    auction: &mut AuctionOpen,
) {
    auction.wire.last_command = Some((auction_id, action, error));
    auction.wire.results.push((auction_id, action, error));
    debug!("ui_auction: command result id {auction_id} action {action} error {error} {tail:?}");
    // Any failed listing ends a multi-sell.
    if action == auction_action::STARTED && error != auction_error::OK {
        auction.multisell_failure();
    }
    match error {
        auction_error::OK => match action {
            auction_action::STARTED => {
                if let Some(ms) = auction.multisell.as_mut() {
                    let done = ms.total - ms.remaining;
                    let total = ms.total;
                    ms.send_next = true;
                    auction.queue_multisell_event("AUCTION_MULTISELL_UPDATE", &[done, total]);
                }
                auction
                    .messages
                    .push(AuctionMessage::key("ERR_AUCTION_STARTED"));
                auction.can_get_owner = true;
                auction.owner_list_wanted = true;
            }
            auction_action::BID_PLACED => {
                auction
                    .messages
                    .push(AuctionMessage::key("ERR_AUCTION_BID_PLACED"));
                auction.outbid.retain(|&(_, id)| id != auction_id);
                auction.can_get_bidder = true;
                auction.bidder_list_wanted = Some(auction.last_bidder_page);
                let min_increment = match tail {
                    AuctionCommandTail::BidPlaced { new_min_outbid } => *new_min_outbid,
                    _ => 0,
                };
                if auction.auctioneer.is_some() {
                    let bid = auction.last_bid_amount;
                    auction.patch(
                        LIST,
                        auction_id,
                        Patch {
                            bid,
                            min_increment,
                            bidder: None,
                            extend: true,
                        },
                    );
                }
                // The reference plays emote 3 for a placed bid.
                auction.emote_wanted = true;
            }
            auction_action::REMOVED => {
                auction
                    .messages
                    .push(AuctionMessage::key("ERR_AUCTION_REMOVED"));
                auction.expire(auction_id);
            }
            _ => {}
        },
        auction_error::INVENTORY => {
            if let AuctionCommandTail::Inventory { result } = tail {
                let key = crate::ui_items::equip_error_key(*result as u8);
                auction.messages.push(AuctionMessage::key(key));
            }
        }
        auction_error::NOT_ENOUGH_MONEY => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_NOT_ENOUGH_MONEY"));
        }
        auction_error::ITEM_NOT_FOUND => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_ITEM_NOT_FOUND"));
        }
        auction_error::HIGHER_BID => {
            if let AuctionCommandTail::HigherBid {
                new_bidder_guid,
                new_bid,
                new_min_outbid,
            } = *tail
            {
                if auction.auctioneer.is_some() {
                    for list in [LIST, BIDDER] {
                        auction.patch(
                            list,
                            auction_id,
                            Patch {
                                bid: new_bid,
                                min_increment: new_min_outbid,
                                bidder: Some(new_bidder_guid),
                                extend: false,
                            },
                        );
                    }
                }
            }
            auction
                .messages
                .push(AuctionMessage::key("ERR_AUCTION_HIGHER_BID"));
        }
        auction_error::BID_INCREMENT => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_AUCTION_BID_INCREMENT"));
        }
        auction_error::BID_OWN => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_AUCTION_BID_OWN"));
        }
        auction_error::RESTRICTED_ACCOUNT => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_RESTRICTED_ACCOUNT"));
        }
        _ => {
            auction
                .messages
                .push(AuctionMessage::key("ERR_AUCTION_DATABASE_ERROR"));
        }
    }
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION` (`0x5a0480`): outbid patches the bidder and browse rows of
/// this house and remembers the id for the next bidder query; won removes the auction.
fn auction_bidder_notification(notice: &AuctionBidderNotification, auction: &mut AuctionOpen) {
    let won = notice.bid_or_zero == 0;
    auction.messages.push(AuctionMessage::item(
        if won {
            "ERR_AUCTION_WON_S"
        } else {
            "ERR_AUCTION_OUTBID_S"
        },
        notice.item_entry,
        notice.random_property_id,
    ));
    if won {
        auction.expire(notice.auction_id);
        return;
    }
    auction.outbid.push((notice.house_id, notice.auction_id));
    if auction.house_id == notice.house_id {
        for list in [BIDDER, LIST] {
            auction.patch(
                list,
                notice.auction_id,
                Patch {
                    bid: notice.bid_or_zero,
                    min_increment: notice.out_bid,
                    bidder: Some(notice.bidder_guid),
                    extend: true,
                },
            );
        }
    }
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION` (`0x5a0790`): a bidder patches our rows silently; a zero
/// bidder closes the auction, sold with a bid and expired without.
fn auction_owner_notification(notice: &AuctionOwnerNotification, auction: &mut AuctionOpen) {
    if notice.bidder_guid != 0 {
        if auction.auctioneer.is_none() {
            return;
        }
        for list in [OWNER, LIST] {
            auction.patch(
                list,
                notice.auction_id,
                Patch {
                    bid: notice.bid,
                    min_increment: notice.out_bid,
                    bidder: Some(notice.bidder_guid),
                    extend: true,
                },
            );
        }
        return;
    }
    auction.messages.push(AuctionMessage::item(
        if notice.bid != 0 {
            "ERR_AUCTION_SOLD_S"
        } else {
            "ERR_AUCTION_EXPIRED_S"
        },
        notice.item_entry,
        notice.random_property_id,
    ));
    auction.expire(notice.auction_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::AuctionListEntry;
    use benilla_ui::messages::MsgKind;

    fn keys(auction: &AuctionOpen) -> Vec<(&'static str, MsgKind)> {
        auction
            .messages
            .iter()
            .map(|m| (m.key, benilla_ui::messages::kind_of(m.key)))
            .collect()
    }

    fn row(auction_id: u32) -> AuctionListEntry {
        AuctionListEntry {
            auction_id,
            item_entry: 2589,
            count: 5,
            start_bid: 100,
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

    #[test]
    fn the_table_routes_the_hello_and_the_session_end_to_the_house() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AuctionOpen>();
        register(&mut app);
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

    #[test]
    fn a_closed_house_opens_nothing() {
        let mut a = AuctionOpen::default();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AuctionOpen>();
        register(&mut app);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::AuctionHello {
                auctioneer: 0x10,
                house_id: 1,
                enabled: false,
            }],
        );
        let open = app.world().resource::<AuctionOpen>();
        assert_eq!(open.auctioneer, None);
        assert!(open.disabled_requested);
        a.open(0x10, 1);
        assert!(!a.disabled_requested);
    }

    #[test]
    fn a_result_answers_its_query_and_paces_the_next() {
        let mut a = AuctionOpen::default();
        a.open(0x10, 1);
        a.sent(LIST, 10.0);
        assert!(!a.can_send(LIST, 50.0), "one query in flight");
        list_result(LIST, page(vec![row(1)]), 20.0, &mut a);
        assert!(!a.can_send(LIST, 20.1), "the page's 300 ms");
        assert!(a.can_send(LIST, 20.31));
        assert!(a.can_send(OWNER, 0.0), "each list paces itself");
    }

    #[test]
    fn the_owner_list_keeps_its_sold_rows_and_the_sales_keep_the_live_ones() {
        let mut a = AuctionOpen::default();
        a.open(0x10, 1);
        list_result(OWNER, page(vec![row(1), row(2)]), 0.0, &mut a);
        let sale = AuctionPendingSale {
            subject: "2589:0:2:9".into(),
            body: "0000000000000505:150:400:20:5".into(),
            mail_id: 1,
            mail_template: 0,
            days_left: 0.5,
        };
        // A non-sale mail is not a row.
        let other = AuctionPendingSale {
            subject: "2589:0:1:8".into(),
            ..sale.clone()
        };
        pending_sales(&[sale, other], 0.0, &mut a);
        let ids: Vec<(u32, bool)> = a.lists[OWNER]
            .entries
            .iter()
            .map(|e| (e.wire.auction_id, e.sold))
            .collect();
        assert_eq!(ids, vec![(1, false), (2, false), (9, true)]);
        let sold = &a.lists[OWNER].entries[2];
        assert_eq!(
            (
                sold.wire.bidder_guid,
                sold.wire.current_bid,
                sold.wire.buyout
            ),
            (0x505, 150, 400)
        );
        list_result(OWNER, page(vec![row(3)]), 0.0, &mut a);
        let ids: Vec<u32> = a.lists[OWNER]
            .entries
            .iter()
            .map(|e| e.wire.auction_id)
            .collect();
        assert_eq!(
            ids,
            vec![9, 3],
            "the sold row stays, the live ones are replaced"
        );
    }

    #[test]
    fn outcomes_go_to_chat_and_refusals_to_the_error_frame() {
        let tail = AuctionCommandTail::Empty;
        for (action, key) in [
            (auction_action::STARTED, "ERR_AUCTION_STARTED"),
            (auction_action::REMOVED, "ERR_AUCTION_REMOVED"),
        ] {
            let mut a = AuctionOpen::default();
            auction_command_result(7, action, auction_error::OK, &tail, &mut a);
            assert_eq!(keys(&a), vec![(key, MsgKind::Chat)]);
        }
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::BID_PLACED, 10, &tail, &mut a);
        assert_eq!(keys(&a), vec![("ERR_AUCTION_BID_OWN", MsgKind::Error)]);
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::BID_PLACED, 5, &tail, &mut a);
        assert_eq!(keys(&a), vec![("ERR_AUCTION_HIGHER_BID", MsgKind::Error)]);
        let mut a = AuctionOpen::default();
        auction_command_result(0, auction_action::STARTED, 9, &tail, &mut a);
        assert_eq!(
            keys(&a),
            vec![("ERR_AUCTION_DATABASE_ERROR", MsgKind::Error)]
        );
    }

    #[test]
    fn a_placed_bid_patches_the_browse_row_and_asks_the_bids_again() {
        let mut a = AuctionOpen::default();
        a.open(0x10, 1);
        list_result(LIST, page(vec![row(4)]), 0.0, &mut a);
        a.outbid.push((1, 4));
        a.last_bid_amount = 250;
        auction_command_result(
            4,
            auction_action::BID_PLACED,
            auction_error::OK,
            &AuctionCommandTail::BidPlaced { new_min_outbid: 13 },
            &mut a,
        );
        let e = &a.lists[LIST].entries[0];
        assert_eq!((e.wire.current_bid, e.wire.min_increment), (250, 13));
        assert!(e.extend, "the bid holds the auction open at least 90 s");
        assert!(a.outbid.is_empty());
        assert_eq!(a.bidder_list_wanted, Some(0));
        assert!(a.emote_wanted);
    }

    #[test]
    fn a_sale_removes_the_auction_everywhere() {
        let mut a = AuctionOpen::default();
        a.open(0x10, 1);
        list_result(OWNER, page(vec![row(7), row(8)]), 0.0, &mut a);
        list_result(LIST, page(vec![row(7)]), 0.0, &mut a);
        auction_owner_notification(
            &AuctionOwnerNotification {
                auction_id: 7,
                bid: 500,
                out_bid: 0,
                bidder_guid: 0,
                item_entry: 2589,
                random_property_id: 0,
                mail_delay: 3600.0,
            },
            &mut a,
        );
        assert_eq!(keys(&a), vec![("ERR_AUCTION_SOLD_S", MsgKind::Chat)]);
        assert_eq!(a.lists[OWNER].entries.len(), 1);
        assert!(a.lists[LIST].entries.is_empty());
        assert!(
            a.pending_sales_wanted,
            "the owner list asks the sales again"
        );
    }

    #[test]
    fn an_outbid_patches_this_house_and_remembers_the_id() {
        let mut a = AuctionOpen::default();
        a.open(0x10, 2);
        list_result(BIDDER, page(vec![row(3)]), 0.0, &mut a);
        let notice = AuctionBidderNotification {
            house_id: 2,
            auction_id: 3,
            bidder_guid: 0x66,
            bid_or_zero: 900,
            out_bid: 45,
            item_entry: 2589,
            random_property_id: 0,
        };
        auction_bidder_notification(&notice, &mut a);
        let e = &a.lists[BIDDER].entries[0];
        assert_eq!((e.wire.current_bid, e.wire.bidder_guid), (900, 0x66));
        assert_eq!(a.outbid, vec![(2, 3)]);
        assert_eq!(keys(&a), vec![("ERR_AUCTION_OUTBID_S", MsgKind::Chat)]);
    }
}
