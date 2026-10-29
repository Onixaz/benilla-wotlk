//! The auction-house messages of the 12340 client: hello, the three list pages and the pending
//! sales, sell, bid, cancel, the command result and three notifications. Every guid is a plain
//! `u64`, and the auctioneer guid heads every CMSG.

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le, read_u8,
};

/// `SMSG_AUCTION_COMMAND_RESULT`'s `action`, as the reference's handler `0x59ffb0` branches on it.
pub mod auction_action {
    /// Answers [`super::auction_sell_item`].
    pub const STARTED: u32 = 0;
    /// Answers [`super::auction_remove_item`].
    pub const REMOVED: u32 = 1;
    /// Answers [`super::auction_place_bid`], buyouts too.
    pub const BID_PLACED: u32 = 2;
}

/// `SMSG_AUCTION_COMMAND_RESULT`'s `error`, the arms of `0x59ffb0`; any other code is the
/// database error.
pub mod auction_error {
    pub const OK: u32 = 0;
    /// Carries an `EQUIP_ERR_*` tail ([`super::AuctionCommandTail::Inventory`]).
    pub const INVENTORY: u32 = 1;
    pub const DATABASE: u32 = 2;
    pub const NOT_ENOUGH_MONEY: u32 = 3;
    pub const ITEM_NOT_FOUND: u32 = 4;
    /// Outbid in flight; carries the [`super::AuctionCommandTail::HigherBid`] tail.
    pub const HIGHER_BID: u32 = 5;
    pub const BID_INCREMENT: u32 = 7;
    pub const BID_OWN: u32 = 10;
    pub const RESTRICTED_ACCOUNT: u32 = 13;
}

/// The no-filter values of [`auction_list_items`]: `QueryAuctionItems` (`0x59bcf0`) sends `-1` for
/// an unset slot, class, subclass or quality, and `0` for an unset level or usability.
pub mod auction_filter {
    pub const ANY: u32 = 0xFFFF_FFFF;
    pub const ANY_LEVEL: u8 = 0;
    pub const ANY_USABILITY: u8 = 0;
}

/// The three `etime` values of `CMSG_AUCTION_SELL_ITEM` in minutes, the run-time table at
/// `0xa15248` that `StartAuction` indexes by the duration menu's 1..3.
pub mod auction_duration {
    pub const SHORT_MINUTES: u32 = 720;
    pub const MEDIUM_MINUTES: u32 = 1440;
    pub const LONG_MINUTES: u32 = 2880;
    /// In menu order: `StartAuction(…, runTime)` and `CalculateAuctionDeposit(runTime)` take 1..3.
    pub const BY_INDEX: [u32; 3] = [SHORT_MINUTES, MEDIUM_MINUTES, LONG_MINUTES];
}

/// The sort columns a criterion carries on `CMSG_AUCTION_LIST_ITEMS`, numbered as the reference's
/// sort-name table (`0x59a7c0`) names them.
pub mod auction_sort {
    pub const LEVEL: u8 = 0;
    pub const QUALITY: u8 = 1;
    pub const BUYOUT_THEN_BID: u8 = 2;
    pub const DURATION: u8 = 3;
    pub const STATUS: u8 = 4;
    pub const NAME: u8 = 5;
    pub const MIN_BID_BUYOUT: u8 = 6;
    pub const SELLER: u8 = 7;
    pub const BID: u8 = 8;
    pub const QUANTITY: u8 = 9;
    pub const BUYOUT: u8 = 10;
}

/// Records per list page; `list_from` steps by it (`QueryAuctionItems` sends `50 × page`).
pub const AUCTION_PAGE_SIZE: u32 = 50;

/// The enchant slots a list record carries, each `(id, duration, charges)`.
pub const AUCTION_ENCHANT_SLOTS: usize = 7;

/// A list record's width as `0x59e160` reads it: `2×u32, 7×3×u32, 5×u32, u64, 4×u32, u64, u32`.
pub const AUCTION_RECORD_BYTES: usize = 148;

/// One enchant slot of a list record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuctionEnchant {
    pub id: u32,
    pub duration: u32,
    pub charges: u32,
}

/// One row of any of the three list results; the item name is not on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuctionListEntry {
    pub auction_id: u32,
    pub item_entry: u32,
    /// The item's enchant slots in slot order; `[0]` is the permanent enchant the link carries.
    pub enchants: [AuctionEnchant; AUCTION_ENCHANT_SLOTS],
    /// Signed: negative is a random suffix id, positive a random property id.
    pub random_property_id: i32,
    pub suffix_factor: u32,
    /// The stack size.
    pub count: u32,
    pub spell_charges: i32,
    pub flags: u32,
    pub owner_guid: u64,
    /// The seller's opening price, what the first bid must meet.
    pub start_bid: u32,
    /// The next bid's step over [`Self::current_bid`]; 0 before any bid.
    pub min_increment: u32,
    /// 0 for none.
    pub buyout: u32,
    /// Milliseconds left; the reference stores `now + timeLeft` as the expiry.
    pub time_left_ms: u32,
    /// `0` = nobody has bid.
    pub bidder_guid: u64,
    /// `0` = no bid yet.
    pub current_bid: u32,
}

/// The body the three list results share (`0x59e160`, `0x59e480`, `0x59ecd0`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuctionListPage {
    pub auctions: Vec<AuctionListEntry>,
    /// The match count before the page cap.
    pub total_count: u32,
    /// The delay the next query of any list waits (`[0xacf628]`, 5000 ms until a page says
    /// otherwise); `None` when the body ends before it.
    pub query_delay_ms: Option<u32>,
}

/// One `SMSG_AUCTION_LIST_PENDING_SALES` row: an auction-house mail still in transit. The subject
/// and body are the server's colon-separated codes, which the app parses (`0x59a4a0`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuctionPendingSale {
    pub subject: String,
    pub body: String,
    pub mail_id: u32,
    pub mail_template: u32,
    /// Days until delivery.
    pub days_left: f32,
}

/// The browse filters and sort of [`auction_list_items`], already wire values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuctionListQuery {
    pub list_from: u32,
    pub searched_name: String,
    pub level_min: u8,
    pub level_max: u8,
    pub slot_id: u32,
    pub main_category: u32,
    pub sub_category: u32,
    /// A minimum quality, [`auction_filter::ANY`] for none.
    pub quality: u32,
    pub usable: u8,
    pub get_all: bool,
    /// The list's valid sort criteria in order: `(column, reversed)`.
    pub sort: Vec<(u8, bool)>,
}

/// The tail after `SMSG_AUCTION_COMMAND_RESULT`'s three dwords, read as `0x59ffb0` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionCommandTail {
    /// Every other pair: the three dwords are the whole packet.
    Empty,
    /// A placed bid: the step the next bid must clear.
    BidPlaced { new_min_outbid: u32 },
    /// [`auction_error::INVENTORY`]: an `InventoryResult` (`EQUIP_ERR_*`) code.
    Inventory { result: u32 },
    /// [`auction_error::HIGHER_BID`]: who outbid us, at what bid, and the next step to clear.
    HigherBid {
        new_bidder_guid: u64,
        new_bid: u32,
        new_min_outbid: u32,
    },
}

/// `SMSG_AUCTION_BIDDER_NOTIFICATION`, won or outbid (`0x5a0480`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionBidderNotification {
    /// `AuctionHouse.dbc` row, as in the hello reply.
    pub house_id: u32,
    pub auction_id: u32,
    pub bidder_guid: u64,
    /// 0 means won; otherwise the bid that beat us.
    pub bid_or_zero: u32,
    /// The step the next bid must clear.
    pub out_bid: u32,
    pub item_entry: u32,
    pub random_property_id: i32,
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION`, to the seller (`0x5a0790`): a nonzero bidder is a new bid,
/// a zero one a close, sold when the bid is nonzero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuctionOwnerNotification {
    pub auction_id: u32,
    pub bid: u32,
    pub out_bid: u32,
    pub bidder_guid: u64,
    pub item_entry: u32,
    pub random_property_id: i32,
    /// Seconds until the sale's money mail arrives.
    pub mail_delay: f32,
}

/// `MSG_AUCTION_HELLO`: the auctioneer guid; the reply (`0x59ff40`) opens the window.
pub fn auction_hello(auctioneer: u64) -> Vec<u8> {
    auctioneer.to_le_bytes().to_vec()
}

/// `CMSG_AUCTION_SELL_ITEM` (`0x59ba80`, `0x59f990`): the auctioneer, `u32` item count, then
/// `(u64 item guid, u32 stack count)` per item, the bid, buyout and run time in minutes.
pub fn auction_sell_item(
    auctioneer: u64,
    items: &[(u64, u32)],
    bid: u32,
    buyout: u32,
    etime_minutes: u32,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + 12 * items.len() + 12);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for (guid, count) in items {
        body.extend_from_slice(&guid.to_le_bytes());
        body.extend_from_slice(&count.to_le_bytes());
    }
    body.extend_from_slice(&bid.to_le_bytes());
    body.extend_from_slice(&buyout.to_le_bytes());
    body.extend_from_slice(&etime_minutes.to_le_bytes());
    body
}

/// `CMSG_AUCTION_REMOVE_ITEM` (`0x59ddf0`), the seller's cancel.
pub fn auction_remove_item(auctioneer: u64, auction_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&auction_id.to_le_bytes());
    body
}

/// `CMSG_AUCTION_LIST_ITEMS` (`0x59bcf0`): the filters, `u8 getAll`, then `u8` criteria count and
/// `(u8 column, u8 reversed)` per valid criterion of the Browse list.
pub fn auction_list_items(auctioneer: u64, q: &AuctionListQuery) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + q.searched_name.len() + 22 + 2 * q.sort.len());
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&q.list_from.to_le_bytes());
    body.extend_from_slice(q.searched_name.as_bytes());
    body.push(0);
    body.push(q.level_min);
    body.push(q.level_max);
    body.extend_from_slice(&q.slot_id.to_le_bytes());
    body.extend_from_slice(&q.main_category.to_le_bytes());
    body.extend_from_slice(&q.sub_category.to_le_bytes());
    body.extend_from_slice(&q.quality.to_le_bytes());
    body.push(q.usable);
    body.push(u8::from(q.get_all));
    body.push(q.sort.len() as u8);
    for &(column, reversed) in &q.sort {
        body.push(column);
        body.push(u8::from(reversed));
    }
    body
}

/// `CMSG_AUCTION_LIST_OWNER_ITEMS` (`0x59cc50`): the auctioneer and `u32` list start, always 0.
pub fn auction_list_owner_items(auctioneer: u64, list_from: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&list_from.to_le_bytes());
    body
}

/// `CMSG_AUCTION_LIST_PENDING_SALES` (`0x59cc50`, `0x59cd90`): the auctioneer guid alone.
pub fn auction_list_pending_sales(auctioneer: u64) -> Vec<u8> {
    auctioneer.to_le_bytes().to_vec()
}

/// `CMSG_AUCTION_PLACE_BID` (`0x59da40`), also the buyout: a `price` equal to the buyout is one.
pub fn auction_place_bid(auctioneer: u64, auction_id: u32, price: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + 4);
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&auction_id.to_le_bytes());
    body.extend_from_slice(&price.to_le_bytes());
    body
}

/// `CMSG_AUCTION_LIST_BIDDER_ITEMS` (`0x59ce50`): the auctioneer, `u32` list start, then the
/// outbid auction ids of this house, counted.
pub fn auction_list_bidder_items(auctioneer: u64, list_from: u32, auction_ids: &[u32]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 4 + 4 + 4 * auction_ids.len());
    body.extend_from_slice(&auctioneer.to_le_bytes());
    body.extend_from_slice(&list_from.to_le_bytes());
    body.extend_from_slice(&(auction_ids.len() as u32).to_le_bytes());
    for id in auction_ids {
        body.extend_from_slice(&id.to_le_bytes());
    }
    body
}

/// The `MSG_AUCTION_HELLO` reply (`0x59ff40`): the guid, the `AuctionHouse.dbc` house id and
/// `u8 enabled`; zero, or a body that ends before it, is a disabled house.
pub(super) fn read_auction_hello(r: &mut &[u8]) -> io::Result<(u64, u32, bool)> {
    let guid = read_u64_le(r)?;
    let house = read_u32_le(r)?;
    let enabled = if r.is_empty() { 0 } else { read_u8(r)? };
    Ok((guid, house, enabled != 0))
}

fn read_auction_list_entry(r: &mut &[u8]) -> io::Result<AuctionListEntry> {
    let auction_id = read_u32_le(r)?;
    let item_entry = read_u32_le(r)?;
    let mut enchants = [AuctionEnchant::default(); AUCTION_ENCHANT_SLOTS];
    for e in &mut enchants {
        *e = AuctionEnchant {
            id: read_u32_le(r)?,
            duration: read_u32_le(r)?,
            charges: read_u32_le(r)?,
        };
    }
    Ok(AuctionListEntry {
        auction_id,
        item_entry,
        enchants,
        random_property_id: read_i32_le(r)?,
        suffix_factor: read_u32_le(r)?,
        count: read_u32_le(r)?,
        spell_charges: read_i32_le(r)?,
        flags: read_u32_le(r)?,
        owner_guid: read_u64_le(r)?,
        start_bid: read_u32_le(r)?,
        min_increment: read_u32_le(r)?,
        buyout: read_u32_le(r)?,
        time_left_ms: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        current_bid: read_u32_le(r)?,
    })
}

/// The list body: `u32 count`, the records, `u32 totalCount`, then the `u32` query delay.
pub(super) fn read_auction_list_result(r: &mut &[u8]) -> io::Result<AuctionListPage> {
    let count = read_u32_le(r)?;
    let mut auctions = Vec::with_capacity(capacity_hint(count, r.len() / AUCTION_RECORD_BYTES));
    for _ in 0..count {
        // A count past the records is bounded by the buffer rather than read as garbage.
        if r.len() < AUCTION_RECORD_BYTES {
            break;
        }
        auctions.push(read_auction_list_entry(r)?);
    }
    let total_count = if r.len() >= 4 {
        read_u32_le(r)?
    } else {
        auctions.len() as u32
    };
    let query_delay_ms = if r.len() >= 4 {
        Some(read_u32_le(r)?)
    } else {
        None
    };
    Ok(AuctionListPage {
        auctions,
        total_count,
        query_delay_ms,
    })
}

/// `SMSG_AUCTION_LIST_PENDING_SALES` (`0x59e880`): `u32 count`, then
/// `(string subject, string body, u32 mailId, u32 template, f32 daysLeft)` per row.
pub(super) fn read_auction_pending_sales(r: &mut &[u8]) -> io::Result<Vec<AuctionPendingSale>> {
    let count = read_u32_le(r)?;
    let mut rows = Vec::with_capacity(capacity_hint(count, r.len() / 14));
    for _ in 0..count {
        rows.push(AuctionPendingSale {
            subject: read_cstring(r)?,
            body: read_cstring(r)?,
            mail_id: read_u32_le(r)?,
            mail_template: read_u32_le(r)?,
            days_left: read_f32_le(r)?,
        });
    }
    Ok(rows)
}

/// `SMSG_AUCTION_COMMAND_RESULT` (`0x59ffb0`): `u32` auction id, action, error, then the
/// [`AuctionCommandTail`] the error and action select.
pub(super) fn read_auction_command_result(
    r: &mut &[u8],
) -> io::Result<(u32, u32, u32, AuctionCommandTail)> {
    let auction_id = read_u32_le(r)?;
    let action = read_u32_le(r)?;
    let error = read_u32_le(r)?;
    let tail = match error {
        auction_error::OK if action == auction_action::BID_PLACED && !r.is_empty() => {
            AuctionCommandTail::BidPlaced {
                new_min_outbid: read_u32_le(r)?,
            }
        }
        auction_error::INVENTORY if !r.is_empty() => AuctionCommandTail::Inventory {
            result: read_u32_le(r)?,
        },
        auction_error::HIGHER_BID if !r.is_empty() => AuctionCommandTail::HigherBid {
            new_bidder_guid: read_u64_le(r)?,
            new_bid: read_u32_le(r)?,
            new_min_outbid: read_u32_le(r)?,
        },
        _ => AuctionCommandTail::Empty,
    };
    Ok((auction_id, action, error, tail))
}

pub(super) fn read_auction_bidder_notification(
    r: &mut &[u8],
) -> io::Result<AuctionBidderNotification> {
    Ok(AuctionBidderNotification {
        house_id: read_u32_le(r)?,
        auction_id: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        bid_or_zero: read_u32_le(r)?,
        out_bid: read_u32_le(r)?,
        item_entry: read_u32_le(r)?,
        random_property_id: read_i32_le(r)?,
    })
}

/// `SMSG_AUCTION_OWNER_NOTIFICATION` (`0x5a0790`): 3 `u32`, the bidder `u64`, item and property,
/// then the `f32` mail delay.
pub(super) fn read_auction_owner_notification(
    r: &mut &[u8],
) -> io::Result<AuctionOwnerNotification> {
    Ok(AuctionOwnerNotification {
        auction_id: read_u32_le(r)?,
        bid: read_u32_le(r)?,
        out_bid: read_u32_le(r)?,
        bidder_guid: read_u64_le(r)?,
        item_entry: read_u32_le(r)?,
        random_property_id: read_i32_le(r)?,
        mail_delay: if r.len() >= 4 { read_f32_le(r)? } else { 0.0 },
    })
}

/// `SMSG_AUCTION_REMOVED_NOTIFICATION` (`0x5a0ac0`), to a bidder whose auction was cancelled:
/// `(auction_id, item_entry, random_property_id)`.
pub(super) fn read_auction_removed_notification(r: &mut &[u8]) -> io::Result<(u32, u32, i32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?, read_i32_le(r)?))
}
