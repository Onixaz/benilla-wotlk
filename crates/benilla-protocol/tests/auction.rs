//! Golden tests for the 12340 auction house wire: every client body byte-exact, and hand-built
//! server bodies through `parse_server_with_tail` (no byte left unread) and `decode`.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, auction_action, auction_duration, auction_error, auction_filter, auction_sort,
    AuctionBidderNotification, AuctionCommandTail, AuctionEnchant, AuctionListEntry,
    AuctionListPage, AuctionListQuery, AuctionOwnerNotification, AuctionPendingSale,
    AUCTION_RECORD_BYTES,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Parse a server body and require that the decoder read every byte of it.
fn parse_whole(opcode: u16, body: &[u8]) -> ServerPacket {
    let (packet, tail) = messages::parse_server_with_tail(opcode, body).unwrap();
    assert_eq!(tail, 0, "{} left {tail} byte(s) unread", packet.name());
    packet
}

const AUCTIONEER: u64 = 0x00F1_3000_0000_0055;

#[test]
fn auction_send_bodies_golden() {
    // MSG_AUCTION_HELLO: the auctioneer's full guid and nothing else.
    assert_eq!(
        messages::auction_hello(AUCTIONEER),
        hx("550000000030f100"),
        "MSG_AUCTION_HELLO body"
    );

    // CMSG_AUCTION_REMOVE_ITEM: u64 auctioneer, u32 auctionId.
    assert_eq!(
        messages::auction_remove_item(AUCTIONEER, 4242),
        hx("550000000030f10092100000"),
        "CMSG_AUCTION_REMOVE_ITEM body"
    );

    // CMSG_AUCTION_PLACE_BID: u64 auctioneer, u32 auctionId, u32 price.
    assert_eq!(
        messages::auction_place_bid(AUCTIONEER, 4242, 12_345),
        hx("550000000030f1009210000039300000"),
        "CMSG_AUCTION_PLACE_BID body"
    );

    // CMSG_AUCTION_LIST_OWNER_ITEMS: u64 auctioneer, u32 listfrom.
    assert_eq!(
        messages::auction_list_owner_items(AUCTIONEER, 0),
        hx("550000000030f10000000000"),
        "CMSG_AUCTION_LIST_OWNER_ITEMS body"
    );

    // CMSG_AUCTION_LIST_PENDING_SALES: the auctioneer guid alone.
    assert_eq!(
        messages::auction_list_pending_sales(AUCTIONEER),
        hx("550000000030f100"),
        "CMSG_AUCTION_LIST_PENDING_SALES body"
    );
}

#[test]
fn auction_sell_item_body_golden() {
    // One stack: count 1, then (guid, stack size), bid, buyout, run time in minutes.
    assert_eq!(
        messages::auction_sell_item(
            AUCTIONEER,
            &[(0x4000_0000_0100_00AB, 20)],
            10_000,
            50_000,
            auction_duration::MEDIUM_MINUTES,
        ),
        hx(concat!(
            "550000000030f100", // auctioneer
            "01000000",         // one item
            "ab00000100000040", // item guid
            "14000000",         // 20 of it
            "10270000",         // bid 10000
            "50c30000",         // buyout 50000
            "a0050000",         // etime 1440 minutes
        )),
        "CMSG_AUCTION_SELL_ITEM body (one stack)"
    );

    // A stack assembled from two partial stacks, as SellNextItem builds it.
    let body = messages::auction_sell_item(
        AUCTIONEER,
        &[(0x4000_0000_0000_0001, 3), (0x4000_0000_0000_0002, 2)],
        100,
        0,
        auction_duration::SHORT_MINUTES,
    );
    assert_eq!(body.len(), 8 + 4 + 2 * 12 + 12);
    assert_eq!(
        &body[8..12],
        &2u32.to_le_bytes(),
        "the item count leads the items"
    );
    assert_eq!(&body[body.len() - 4..], &720u32.to_le_bytes());
}

#[test]
fn auction_list_items_body_golden() {
    // The filters, getAll, then the sort count and (column, reversed) per criterion.
    let q = AuctionListQuery {
        list_from: 50,
        searched_name: "Copper".into(),
        level_min: 10,
        level_max: 20,
        slot_id: 1,
        main_category: 2,
        sub_category: 3,
        quality: 4,
        usable: 1,
        get_all: false,
        sort: vec![(auction_sort::QUALITY, false), (auction_sort::LEVEL, true)],
    };
    assert_eq!(
        messages::auction_list_items(AUCTIONEER, &q),
        hx(concat!(
            "550000000030f100", // auctioneer
            "32000000",         // listfrom 50
            "436f7070657200",   // "Copper\0"
            "0a",               // levelmin 10
            "14",               // levelmax 20
            "01000000",         // slotId
            "02000000",         // mainCategory
            "03000000",         // subCategory
            "04000000",         // quality
            "01",               // usable
            "00",               // getAll
            "02",               // two criteria
            "0100",             // quality, not reversed
            "0001",             // level, reversed
        )),
        "CMSG_AUCTION_LIST_ITEMS body (filters and sort)"
    );

    // Every filter at its sentinel, getAll, no criteria.
    let body = messages::auction_list_items(
        AUCTIONEER,
        &AuctionListQuery {
            level_min: auction_filter::ANY_LEVEL,
            level_max: auction_filter::ANY_LEVEL,
            slot_id: auction_filter::ANY,
            main_category: auction_filter::ANY,
            sub_category: auction_filter::ANY,
            quality: auction_filter::ANY,
            usable: auction_filter::ANY_USABILITY,
            get_all: true,
            ..Default::default()
        },
    );
    assert_eq!(
        body,
        hx(concat!(
            "550000000030f100",
            "00000000",
            "00",
            "00",
            "00",
            "ffffffff",
            "ffffffff",
            "ffffffff",
            "ffffffff",
            "00",
            "01", // getAll
            "00", // no criteria
        )),
        "CMSG_AUCTION_LIST_ITEMS body (no filters, getAll)"
    );
}

#[test]
fn auction_list_bidder_items_body_golden() {
    // u64 auctioneer, u32 listfrom, u32 id count: the count is present even with no ids.
    assert_eq!(
        messages::auction_list_bidder_items(AUCTIONEER, 0, &[]),
        hx("550000000030f1000000000000000000"),
        "CMSG_AUCTION_LIST_BIDDER_ITEMS body (no ids)"
    );
    assert_eq!(
        messages::auction_list_bidder_items(AUCTIONEER, 50, &[7, 4242]),
        hx(concat!(
            "550000000030f100",
            "32000000",
            "02000000",
            "07000000",
            "92100000",
        )),
        "CMSG_AUCTION_LIST_BIDDER_ITEMS body (2 ids)"
    );
}

#[test]
fn auction_hello_reply_wire() {
    // u64 auctioneer, u32 houseId, u8 enabled.
    let mut body = AUCTIONEER.to_le_bytes().to_vec();
    body.extend_from_slice(&6u32.to_le_bytes());
    body.push(1);
    match parse_whole(messages::opcode::MSG_AUCTION_HELLO, &body) {
        ServerPacket::AuctionHello {
            auctioneer,
            house_id,
            enabled,
        } => assert_eq!((auctioneer, house_id, enabled), (AUCTIONEER, 6, true)),
        other => panic!("auction hello, got {}", other.name()),
    }
    let packet = parse_whole(messages::opcode::MSG_AUCTION_HELLO, &body);
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionHello { enabled: true, .. }
    ));

    // Zero is a closed house, and so is a body that ends before the byte.
    body[12] = 0;
    assert!(matches!(
        parse_whole(messages::opcode::MSG_AUCTION_HELLO, &body),
        ServerPacket::AuctionHello { enabled: false, .. }
    ));
    body.pop();
    assert!(matches!(
        parse_whole(messages::opcode::MSG_AUCTION_HELLO, &body),
        ServerPacket::AuctionHello { enabled: false, .. }
    ));
}

/// Build one `SMSG_AUCTION_COMMAND_RESULT` head: `u32 auctionId, u32 action, u32 error`.
fn command_head(auction_id: u32, action: u32, error: u32) -> Vec<u8> {
    let mut body = auction_id.to_le_bytes().to_vec();
    body.extend_from_slice(&action.to_le_bytes());
    body.extend_from_slice(&error.to_le_bytes());
    body
}

fn parse_command(body: &[u8]) -> (u32, u32, u32, AuctionCommandTail) {
    match parse_whole(messages::opcode::SMSG_AUCTION_COMMAND_RESULT, body) {
        ServerPacket::AuctionCommandResult {
            auction_id,
            action,
            error,
            tail,
        } => (auction_id, action, error, tail),
        other => panic!("auction command result, got {}", other.name()),
    }
}

#[test]
fn auction_command_result_tails_wire() {
    // A listing is bare.
    assert_eq!(
        parse_command(&command_head(
            4242,
            auction_action::STARTED,
            auction_error::OK
        )),
        (
            4242,
            auction_action::STARTED,
            auction_error::OK,
            AuctionCommandTail::Empty
        )
    );

    // A placed bid carries the next step.
    let mut body = command_head(4242, auction_action::BID_PLACED, auction_error::OK);
    body.extend_from_slice(&617u32.to_le_bytes());
    assert_eq!(
        parse_command(&body).3,
        AuctionCommandTail::BidPlaced {
            new_min_outbid: 617
        }
    );

    // The inventory tail is keyed on the error alone.
    let mut body = command_head(0, auction_action::STARTED, auction_error::INVENTORY);
    body.extend_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        parse_command(&body).3,
        AuctionCommandTail::Inventory { result: 2 }
    );

    // Outbid in flight: the new bidder, bid and step.
    let mut body = command_head(4242, auction_action::BID_PLACED, auction_error::HIGHER_BID);
    body.extend_from_slice(&0x07D1u64.to_le_bytes());
    body.extend_from_slice(&25_000u32.to_le_bytes());
    body.extend_from_slice(&1_250u32.to_le_bytes());
    assert_eq!(
        parse_command(&body).3,
        AuctionCommandTail::HigherBid {
            new_bidder_guid: 0x07D1,
            new_bid: 25_000,
            new_min_outbid: 1_250,
        }
    );

    let packet = parse_whole(
        messages::opcode::SMSG_AUCTION_COMMAND_RESULT,
        &command_head(9, auction_action::REMOVED, auction_error::OK),
    );
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionCommandResult { auction_id: 9, .. }
    ));
}

/// Append one 148-byte list record in the order `0x59e160` reads it.
fn push_record(body: &mut Vec<u8>, e: &AuctionListEntry) {
    let before = body.len();
    body.extend_from_slice(&e.auction_id.to_le_bytes());
    body.extend_from_slice(&e.item_entry.to_le_bytes());
    for en in &e.enchants {
        body.extend_from_slice(&en.id.to_le_bytes());
        body.extend_from_slice(&en.duration.to_le_bytes());
        body.extend_from_slice(&en.charges.to_le_bytes());
    }
    body.extend_from_slice(&e.random_property_id.to_le_bytes());
    body.extend_from_slice(&e.suffix_factor.to_le_bytes());
    body.extend_from_slice(&e.count.to_le_bytes());
    body.extend_from_slice(&e.spell_charges.to_le_bytes());
    body.extend_from_slice(&e.flags.to_le_bytes());
    body.extend_from_slice(&e.owner_guid.to_le_bytes());
    body.extend_from_slice(&e.start_bid.to_le_bytes());
    body.extend_from_slice(&e.min_increment.to_le_bytes());
    body.extend_from_slice(&e.buyout.to_le_bytes());
    body.extend_from_slice(&e.time_left_ms.to_le_bytes());
    body.extend_from_slice(&e.bidder_guid.to_le_bytes());
    body.extend_from_slice(&e.current_bid.to_le_bytes());
    assert_eq!(body.len() - before, AUCTION_RECORD_BYTES);
    assert_eq!(AUCTION_RECORD_BYTES, 148);
}

/// Every field distinct, so a misread slot cannot pass for the right one.
fn distinct_row(auction_id: u32) -> AuctionListEntry {
    let mut enchants = [AuctionEnchant::default(); 7];
    for (i, e) in enchants.iter_mut().enumerate() {
        let base = 100 + 10 * i as u32;
        *e = AuctionEnchant {
            id: base,
            duration: base + 1,
            charges: base + 2,
        };
    }
    AuctionListEntry {
        auction_id,
        item_entry: 12_640,
        enchants,
        random_property_id: -19,
        suffix_factor: 143,
        count: 5,
        spell_charges: -3,
        flags: 0x8,
        owner_guid: 0x0202,
        start_bid: 1_000,
        min_increment: 60,
        buyout: 20_000,
        time_left_ms: 43_200_000,
        bidder_guid: 0x0303,
        current_bid: 1_200,
    }
}

fn parse_list(opcode: u16, body: &[u8]) -> AuctionListPage {
    match parse_whole(opcode, body) {
        ServerPacket::AuctionListResult(page)
        | ServerPacket::AuctionOwnerListResult(page)
        | ServerPacket::AuctionBidderListResult(page) => page,
        other => panic!("auction list result, got {}", other.name()),
    }
}

#[test]
fn auction_list_result_reads_the_148_byte_record_then_total_and_delay() {
    let rows = [distinct_row(1), distinct_row(2)];
    let mut body = (rows.len() as u32).to_le_bytes().to_vec();
    for row in &rows {
        push_record(&mut body, row);
    }
    body.extend_from_slice(&129u32.to_le_bytes()); // totalCount
    body.extend_from_slice(&300u32.to_le_bytes()); // query delay ms

    for opcode in [
        messages::opcode::SMSG_AUCTION_LIST_RESULT,
        messages::opcode::SMSG_AUCTION_OWNER_LIST_RESULT,
        messages::opcode::SMSG_AUCTION_BIDDER_LIST_RESULT,
    ] {
        let page = parse_list(opcode, &body);
        assert_eq!(page.auctions, rows, "every record intact, the second too");
        assert_eq!(page.total_count, 129);
        assert_eq!(page.query_delay_ms, Some(300));
    }

    let packet = parse_whole(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionListResult(AuctionListPage {
            total_count: 129,
            ..
        })
    ));
    let packet = parse_whole(messages::opcode::SMSG_AUCTION_OWNER_LIST_RESULT, &body);
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionOwnerListResult(_)
    ));
    let packet = parse_whole(messages::opcode::SMSG_AUCTION_BIDDER_LIST_RESULT, &body);
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::AuctionBidderListResult(_)
    ));
}

#[test]
fn auction_list_result_survives_a_short_body() {
    // An empty page with only its total: no delay rides.
    let mut body = 0u32.to_le_bytes().to_vec();
    body.extend_from_slice(&37u32.to_le_bytes());
    let page = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert!(page.auctions.is_empty());
    assert_eq!((page.total_count, page.query_delay_ms), (37, None));

    // A count past the records yields the records that arrived.
    let mut body = 3u32.to_le_bytes().to_vec();
    push_record(&mut body, &distinct_row(1));
    let page = parse_list(messages::opcode::SMSG_AUCTION_LIST_RESULT, &body);
    assert_eq!(page.auctions, [distinct_row(1)]);
    assert_eq!(page.total_count, 1, "falls back to the records read");
}

#[test]
fn auction_pending_sales_wire() {
    // u32 count, then (subject, body, u32 mail id, u32 template, f32 days) per row.
    let mut body = 1u32.to_le_bytes().to_vec();
    body.extend_from_slice(b"2589:0:2:77\0");
    body.extend_from_slice(b"0000000000000505:150:0:20:5\0");
    body.extend_from_slice(&900u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0.04f32.to_le_bytes());
    let expected = AuctionPendingSale {
        subject: "2589:0:2:77".into(),
        body: "0000000000000505:150:0:20:5".into(),
        mail_id: 900,
        mail_template: 0,
        days_left: 0.04,
    };
    match parse_whole(messages::opcode::SMSG_AUCTION_LIST_PENDING_SALES, &body) {
        ServerPacket::AuctionPendingSales { sales } => {
            assert_eq!(sales, std::slice::from_ref(&expected))
        }
        other => panic!("pending sales, got {}", other.name()),
    }
    let packet = parse_whole(messages::opcode::SMSG_AUCTION_LIST_PENDING_SALES, &body);
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionPendingSales { sales } => assert_eq!(sales, [expected]),
        other => panic!("pending sales event, got {other:?}"),
    }

    // AzerothCore's reply: a zero count and nothing else.
    match parse_whole(
        messages::opcode::SMSG_AUCTION_LIST_PENDING_SALES,
        &0u32.to_le_bytes(),
    ) {
        ServerPacket::AuctionPendingSales { sales } => assert!(sales.is_empty()),
        other => panic!("pending sales, got {}", other.name()),
    }
}

#[test]
fn auction_bidder_notification_wire() {
    // House id first and the guid third.
    let mut body = 6u32.to_le_bytes().to_vec();
    body.extend_from_slice(&4242u32.to_le_bytes());
    body.extend_from_slice(&0x0505u64.to_le_bytes());
    body.extend_from_slice(&15_000u32.to_le_bytes()); // nonzero: outbid
    body.extend_from_slice(&750u32.to_le_bytes());
    body.extend_from_slice(&12_640u32.to_le_bytes());
    body.extend_from_slice(&(-19i32).to_le_bytes());
    let expected = AuctionBidderNotification {
        house_id: 6,
        auction_id: 4242,
        bidder_guid: 0x0505,
        bid_or_zero: 15_000,
        out_bid: 750,
        item_entry: 12_640,
        random_property_id: -19,
    };
    match parse_whole(messages::opcode::SMSG_AUCTION_BIDDER_NOTIFICATION, &body) {
        ServerPacket::AuctionBidderNotification(n) => assert_eq!(n, expected),
        other => panic!("auction bidder notification, got {}", other.name()),
    }
    let packet = parse_whole(messages::opcode::SMSG_AUCTION_BIDDER_NOTIFICATION, &body);
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionBidderNotification(n) => assert_eq!(n, expected),
        other => panic!("auction bidder notification event, got {other:?}"),
    }
}

#[test]
fn auction_owner_notification_wire() {
    // 3 u32, the bidder u64, item, property, then the f32 mail delay.
    let mut body = 4242u32.to_le_bytes().to_vec();
    body.extend_from_slice(&15_000u32.to_le_bytes());
    body.extend_from_slice(&750u32.to_le_bytes());
    body.extend_from_slice(&0x0505u64.to_le_bytes());
    body.extend_from_slice(&12_640u32.to_le_bytes());
    body.extend_from_slice(&(-19i32).to_le_bytes());
    body.extend_from_slice(&3600.0f32.to_le_bytes());
    assert_eq!(body.len(), 32);
    let expected = AuctionOwnerNotification {
        auction_id: 4242,
        bid: 15_000,
        out_bid: 750,
        bidder_guid: 0x0505,
        item_entry: 12_640,
        random_property_id: -19,
        mail_delay: 3600.0,
    };
    match parse_whole(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &body) {
        ServerPacket::AuctionOwnerNotification(n) => assert_eq!(n, expected),
        other => panic!("auction owner notification, got {}", other.name()),
    }
    let packet = parse_whole(messages::opcode::SMSG_AUCTION_OWNER_NOTIFICATION, &body);
    match decode(packet).pop().unwrap() {
        SessionEvent::AuctionOwnerNotification(n) => assert_eq!(n, expected),
        other => panic!("auction owner notification event, got {other:?}"),
    }
}

#[test]
fn auction_removed_notification_wire() {
    // u32 auctionId, u32 itemEntry, i32 randomPropertyId.
    let mut body = 4242u32.to_le_bytes().to_vec();
    body.extend_from_slice(&2589u32.to_le_bytes());
    body.extend_from_slice(&(-7i32).to_le_bytes());
    match parse_whole(messages::opcode::SMSG_AUCTION_REMOVED_NOTIFICATION, &body) {
        ServerPacket::AuctionRemovedNotification {
            auction_id,
            item_entry,
            random_property_id,
        } => assert_eq!(
            (auction_id, item_entry, random_property_id),
            (4242, 2589, -7)
        ),
        other => panic!("auction removed notification, got {}", other.name()),
    }
}
