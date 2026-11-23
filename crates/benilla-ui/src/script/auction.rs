//! The auction-house bindings, after 12340's auction-house engine: each verb reads the house at its
//! call and sends, sorts and raises inside it, through the host's live view (the house's lists, sort
//! criteria and query gates are the app's). Getters are keyed by `"list"` (Browse), `"bidder"` (Bids)
//! or `"owner"` (Auctions), with `index` 1-based in the current batch. A fixture's house, pushed
//! whole, stands in where no view lends one.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::container::Arm;
use super::cursor::{self, CursorItem, CursorPayload};
use super::{Model, ScriptValue};

/// The three list types, in the order the app pushes them (`0x59a9d0`).
pub const LIST: usize = 0;
pub const BIDDER: usize = 1;
pub const OWNER: usize = 2;

/// The sort column names by column number (`0x59a7c0`).
pub const SORT_COLUMNS: [&str; 11] = [
    "level",
    "quality",
    "buyoutthenbid",
    "duration",
    "status",
    "name",
    "minbidbuyout",
    "seller",
    "bid",
    "quantity",
    "buyout",
];

/// A list holds at most this many criteria (`m_listSortCriteria[12]`).
const MAX_CRITERIA: usize = 12;

/// The time-left buckets' upper bounds in ms, the table at `0xa15238`.
const TIME_LEFT_THRESHOLDS_MS: [f64; 3] = [1_800_000.0, 7_200_000.0, 43_200_000.0];

/// A live row this far past its expiry leaves its lists at its time-left read (`0x59fe6d`).
const EXPIRY_GRACE_MS: f64 = 10_000.0;

/// The run times `StartAuction` and `CalculateAuctionDeposit` index by 1..3 (`0xa15248`).
const RUN_TIMES_MINUTES: [u32; 3] = [720, 1440, 2880];

/// The run time any other index falls back to.
const DEFAULT_RUN_TIME_MINUTES: u32 = 1440;

/// The deposit's per-minute factor, the `f32` at `0xa394c8`.
const DEPOSIT_PER_MINUTE: f32 = 1.0 / 240.0;

/// The least deposit `CalculateAuctionDeposit` (`0x76dde0`) answers.
const MIN_DEPOSIT: u32 = 100;

/// The bid ceiling `PlaceAuctionBid` refuses past (`0x59da40`).
const MAXIMUM_BID: u32 = 2_000_000_000;

/// A list type by name (`0x59a9d0`): case-insensitive.
fn list_index(kind: &str) -> Option<usize> {
    if kind.eq_ignore_ascii_case("list") {
        Some(LIST)
    } else if kind.eq_ignore_ascii_case("bidder") {
        Some(BIDDER)
    } else if kind.eq_ignore_ascii_case("owner") {
        Some(OWNER)
    } else {
        None
    }
}

/// The sort's column lookup (`0x59a850`): a column name to its number, case-insensitive.
fn sort_column(name: &str) -> Option<u8> {
    SORT_COLUMNS
        .iter()
        .position(|c| c.eq_ignore_ascii_case(name))
        .map(|i| i as u8)
}

/// One auction row, resolved by the app from a wire record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionItemRow {
    /// The wire auction id a bid or a cancel addresses; never shown.
    pub auction_id: u32,
    pub item_id: u32,
    /// The wire's `randomPropertyId`, 0 when unrolled; [`Self::name`] already carries its suffix.
    pub random_property_id: u32,
    /// `None` while the item template query is in flight: the row then answers as a miss.
    pub name: Option<String>,
    /// The record's own name, no suffix: the name sort's key (`0x59b460`); `None` reads [`Self::name`].
    pub base_name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    pub quality: Option<u32>,
    /// The template's `RequiredLevel`; `GetAuctionItemInfo` answers at least 1.
    pub level: u32,
    /// The seller's opening price.
    pub min_bid: u32,
    /// The minimum step above the current bid; 0 before any bid.
    pub min_increment: u32,
    /// `0` = no buyout.
    pub buyout_price: u32,
    /// The current high bid; 0 before any bid.
    pub bid_amount: u32,
    /// `GetAuctionItemInfo`'s eleventh value, which the app picks per list and row.
    pub high_bidder: AuctionHighBidder,
    /// The seller's name; `None` while the name query is in flight.
    pub owner: Option<String>,
    /// The seller's guid, the seller sort's fallback while either name is unknown.
    pub owner_guid: u64,
    /// The status sort's key: 2 when the player holds the bid, 1 for another bidder, 0 for none.
    pub bidder_status: u8,
    /// Seconds to the expiry at the read: arrival plus the wire's time left, or for a sold row the
    /// money mail's delivery; negative once past.
    pub time_left: f64,
    /// A sold auction from the pending-sales list: `GetAuctionItemInfo`'s `saleStatus` 1.
    pub sold: bool,
    /// The full item link: entry, enchant, random property and suffix.
    pub link: Option<String>,
}

/// `GetAuctionItemInfo`'s `highBidder` (`0x59d5e0`): on an `"owner"` row the player owns, the
/// bidder's name from the name cache; on any other row, `1` when the player holds the bid.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AuctionHighBidder {
    /// No bid, another player's bid on a row the player does not own, or a name not arrived.
    #[default]
    Nil,
    /// The player holds the bid on a row it does not own: `1`.
    Player,
    /// The bidder on the player's own auction, by name.
    Name(String),
}

/// One of the three lists: the batch the server sent and the match count before the page cap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionListState {
    /// The current batch, sorted here by the list's criteria.
    pub rows: Vec<AuctionItemRow>,
    /// The pre-cap match count `GetNumAuctionItems` answers second.
    pub total: u32,
}

/// One class of the Browse tab's category tree, a row of the class table at `0xa14f70`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionCategory {
    /// The wire's `mainCategory`: an item class id, not the menu position.
    pub class_id: u32,
    /// The `ItemClass.dbc` name; empty when the player's table lacks the row, which
    /// `GetAuctionItemClasses` then skips while the index keeps its place.
    pub name: String,
    /// The table's second column: false empties `GetAuctionItemSubClasses` and
    /// `GetAuctionInvTypes`, but `QueryAuctionItems` never reads it.
    pub has_subclass_filter: bool,
    /// The class's `ItemSubClass.dbc` rows with `DisplayFlags & 2` clear, in file order.
    pub subclasses: Vec<AuctionSubCategory>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionSubCategory {
    /// The wire's `subCategory`.
    pub sub_id: u32,
    /// The verbose name, or the display name when that is empty.
    pub name: String,
    /// `ItemSubClass.Flags & 0x200`: the subclass offers the inventory-slot rows.
    pub has_inv_types: bool,
}

/// A fixture's auction house, pushed whole where no view lends the house.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionState {
    /// Browse / Bids / Auctions, indexed by [`LIST`], [`BIDDER`], [`OWNER`].
    pub lists: [AuctionListState; 3],
    /// `GetAuctionHouseDepositRate()`: the house's `AuctionHouse.dbc` deposit percentage.
    pub deposit_percent: u32,
}

/// `QueryAuctionItems`'s search, already wire values (`0x59bcf0`); the send adds the browse list's
/// criteria.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuctionQuery {
    pub name: String,
    pub min_level: u8,
    pub max_level: u8,
    /// `0xFFFFFFFF` = any, for these four.
    pub inv_type: u32,
    pub class: u32,
    pub sub_class: u32,
    pub quality: u32,
    /// The item offset, `50 × page`.
    pub list_from: u32,
    pub usable: u8,
    pub get_all: bool,
}

/// A send an auction verb makes inside its call, each behind the gate its send reads.
#[derive(Clone, Debug, PartialEq)]
pub enum AuctionPacket {
    /// `QueryAuctionItems` (`0x59bcf0`).
    Query(AuctionQuery),
    /// `GetOwnerAuctionItems`'s owner-list ask (`0x59cc50`).
    OwnerList,
    /// `GetBidderAuctionItems`'s bidder-list ask (`0x59ce50`) from this list offset.
    BidderList(u32),
    /// `PlaceAuctionBid`'s bid (`0x59dcca`), checked against the row as `0x59da40` checks it.
    Bid { auction_id: u32, amount: u32 },
    /// `CancelAuction`'s cancel (`0x59df3b`).
    Cancel { auction_id: u32 },
    /// `StartAuction`'s sale of the sell item alone (`0x59fc66`).
    Sell {
        item: u64,
        count: u32,
        bid: u32,
        buyout: u32,
        minutes: u32,
    },
}

/// `StartAuction`'s multi-sell (`0x59fc92`): the sellable stacks gathered, `(guid, count)` in scan
/// order, listed `num_stacks` auctions of `stack_size` each.
#[derive(Clone, Debug, PartialEq)]
pub struct AuctionMultiSell {
    pub items: Vec<(u64, u32)>,
    pub stack_size: u32,
    pub num_stacks: u32,
    pub bid: u32,
    /// `0` = no buyout; never below the bid.
    pub buyout: u32,
    /// Minutes: 720, 1440 or 2880.
    pub minutes: u32,
}

/// Why the carried-count check (`0x754dd0`) refuses: past the record's `MaxCount`, shown as
/// message `0x14`, or past its limit category's quantity, shown as `0x272` with the row's quantity
/// and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CarryRefusal {
    MaxCount,
    Category { quantity: u32, name: String },
}

impl CarryRefusal {
    /// The message key the check shows (`0x754e2a`, `0x754ea7`).
    pub fn key(&self) -> &'static str {
        match self {
            Self::MaxCount => "ERR_ITEM_MAX_COUNT",
            Self::Category { .. } => "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
        }
    }
}

/// What the next-item sell did (`0x59ba80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuctionSellStep {
    /// One stack went out.
    Sent,
    /// Every stack is out: the multi-sell ends quietly (`0x59bc37`).
    Done,
    /// The gathered items ran dry: the multi-sell failure (`0x59bc06`).
    Failed,
}

/// The default criteria the auction house's game initialization sets (`0x5a0ba0`), front first.
pub fn auction_default_criteria() -> [Vec<(u8, bool)>; 3] {
    let build = |sets: &[(u8, bool)]| {
        let mut c = Vec::new();
        for &(column, rev) in sets {
            set_criterion(&mut c, column, rev);
        }
        c
    };
    [
        build(&[
            (3, false),
            (8, false),
            (9, true),
            (6, false),
            (5, false),
            (0, true),
            (1, false),
        ]),
        build(&[
            (9, false),
            (5, false),
            (0, true),
            (1, false),
            (6, false),
            (4, false),
            (8, false),
            (3, false),
        ]),
        build(&[
            (9, true),
            (8, false),
            (5, false),
            (0, true),
            (1, false),
            (4, false),
            (3, false),
        ]),
    ]
}

/// The model's starting criteria, for [`Model`]'s constructor.
#[cfg(any(test, feature = "fixtures"))]
pub(super) fn initial_sort_criteria() -> [Vec<(u8, bool)>; 3] {
    auction_default_criteria()
}

/// The sort-criteria setter (`0x59a5e0`): the column moves to the front with `reversed`; a new
/// column on a full list is dropped.
fn set_criterion(criteria: &mut Vec<(u8, bool)>, column: u8, reversed: bool) {
    if let Some(pos) = criteria.iter().position(|&(c, _)| c == column) {
        criteria.remove(pos);
    } else if criteria.len() >= MAX_CRITERIA {
        return;
    }
    criteria.insert(0, (column, reversed));
}

/// The bid a row shows: the current bid, or the opening price before any.
fn shown_bid(r: &AuctionItemRow) -> u32 {
    if r.bid_amount != 0 {
        r.bid_amount
    } else {
        r.min_bid
    }
}

/// The sort compare (`0x59b460`), one column's order of two rows; a row whose template is in flight
/// ties on the template columns.
fn compare_column(column: u8, a: &AuctionItemRow, b: &AuctionItemRow) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let cached = a.name.is_some() && b.name.is_some();
    match column {
        0 if cached => a.level.max(1).cmp(&b.level.max(1)),
        1 if cached => b.quality.cmp(&a.quality),
        2 => {
            let buyout = |r: &AuctionItemRow| {
                if r.buyout_price != 0 {
                    r.buyout_price
                } else {
                    shown_bid(r)
                }
            };
            buyout(a)
                .cmp(&buyout(b))
                .then_with(|| shown_bid(a).cmp(&shown_bid(b)))
        }
        3 => b.sold.cmp(&a.sold).then_with(|| {
            a.time_left
                .partial_cmp(&b.time_left)
                .unwrap_or(Ordering::Equal)
        }),
        4 => b.bidder_status.cmp(&a.bidder_status),
        // The record's name without the random property's suffix (`0x59b4d2`).
        5 if cached => {
            let key = |r: &AuctionItemRow| {
                r.base_name
                    .as_deref()
                    .or(r.name.as_deref())
                    .unwrap_or("")
                    .to_lowercase()
            };
            key(a).cmp(&key(b))
        }
        6 => {
            let buyout = |r: &AuctionItemRow| {
                if r.buyout_price != 0 {
                    r.buyout_price
                } else {
                    shown_bid(r)
                }
            };
            buyout(a).cmp(&buyout(b))
        }
        7 => match (&a.owner, &b.owner) {
            (Some(x), Some(y)) => x.as_bytes().cmp(y.as_bytes()),
            _ => a.owner_guid.cmp(&b.owner_guid),
        },
        8 => shown_bid(a).cmp(&shown_bid(b)),
        9 => a.count.cmp(&b.count),
        10 => a.buyout_price.cmp(&b.buyout_price),
        _ => Ordering::Equal,
    }
}

/// The order of two rows by `criteria`, the first deciding column winning (`0x59b890`).
fn compare_rows(
    a: &AuctionItemRow,
    b: &AuctionItemRow,
    criteria: &[(u8, bool)],
) -> std::cmp::Ordering {
    for &(column, reversed) in criteria {
        let ord = compare_column(column, a, b);
        if ord != std::cmp::Ordering::Equal {
            return if reversed { ord.reverse() } else { ord };
        }
    }
    std::cmp::Ordering::Equal
}

/// The indices of `rows` in their order by `criteria`, for a host that sorts its own list in place.
pub fn auction_sort_order(rows: &[AuctionItemRow], criteria: &[(u8, bool)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| compare_rows(&rows[a], &rows[b], criteria));
    order
}

/// Sort `rows` by `criteria` in place.
#[cfg(any(test, feature = "fixtures"))]
fn sort_rows(rows: &mut [AuctionItemRow], criteria: &[(u8, bool)]) {
    rows.sort_by(|a, b| compare_rows(a, b, criteria));
}

/// The update event each list fires once sorted (`0x59b980`, `0x59b9e0`, `0x59cab0`).
fn list_event(list: usize) -> &'static str {
    match list {
        LIST => "AUCTION_ITEM_LIST_UPDATE",
        BIDDER => "AUCTION_BIDDER_LIST_UPDATE",
        _ => "AUCTION_OWNED_LIST_UPDATE",
    }
}

/// Whether a house is open at the call (`[0xc0f3e8]`).
fn house_open(model: &Model) -> bool {
    match super::read_live(|l| l.auction_auctioneer()).flatten() {
        Some(auctioneer) => auctioneer.is_some(),
        None => fixture_open(model),
    }
}

#[cfg(any(test, feature = "fixtures"))]
fn fixture_open(model: &Model) -> bool {
    model.auction.is_some()
}

#[cfg(not(any(test, feature = "fixtures")))]
fn fixture_open(_: &Model) -> bool {
    false
}

/// One list's rows and its total.
fn counts(model: &Model, list: usize) -> (usize, u32) {
    if let Some(counts) = super::read_live(|l| l.auction_counts(list)).flatten() {
        return counts;
    }
    #[cfg(any(test, feature = "fixtures"))]
    if let Some(s) = model.auction.as_ref() {
        return (s.lists[list].rows.len(), s.lists[list].total);
    }
    let _ = model;
    (0, 0)
}

/// Row `index` (1-based) of the list named `kind`, read at the call; `info` is
/// `GetAuctionItemInfo`'s read.
fn row(model: &Model, kind: &str, index: usize, info: bool) -> Option<AuctionItemRow> {
    let list = list_index(kind)?;
    let i = index.checked_sub(1)?;
    if let Some(row) = super::write_live(|l| l.auction_row(list, i, info)).flatten() {
        return row;
    }
    #[cfg(any(test, feature = "fixtures"))]
    if let Some(s) = model.auction.as_ref() {
        return s.lists[list].rows.get(i).cloned();
    }
    let _ = (model, info);
    None
}

/// Row `index` (1-based) of the list named `kind`, for the tooltip's hover.
pub(super) fn row_for_tooltip(model: &Model, kind: &str, index: usize) -> Option<AuctionItemRow> {
    row(model, kind, index, false)
}

/// The 1-based row of `auction_id` on `list`, 0 for none.
fn find(model: &Model, list: usize, auction_id: u32) -> usize {
    let found = match super::read_live(|l| l.auction_find(list, auction_id)).flatten() {
        Some(found) => found,
        None => fixture_find(model, list, auction_id),
    };
    found.map_or(0, |i| i + 1)
}

#[cfg(any(test, feature = "fixtures"))]
fn fixture_find(model: &Model, list: usize, auction_id: u32) -> Option<usize> {
    model.auction.as_ref()?.lists[list]
        .rows
        .iter()
        .position(|r| r.auction_id == auction_id)
}

#[cfg(not(any(test, feature = "fixtures")))]
fn fixture_find(_: &Model, _: usize, _: u32) -> Option<usize> {
    None
}

/// The house's deposit rate, `None` with no `AuctionHouse.dbc` row.
fn deposit_rate(model: &Model) -> Option<u32> {
    if let Some(rate) = super::read_live(|l| l.auction_deposit_rate()).flatten() {
        return rate;
    }
    #[cfg(any(test, feature = "fixtures"))]
    if let Some(s) = model.auction.as_ref() {
        return Some(s.deposit_percent);
    }
    let _ = model;
    None
}

/// The query gate of `list` and the get-all one.
fn gate(model: &Model, list: usize) -> (bool, bool) {
    if let Some(gate) = super::read_live(|l| l.auction_can_send(list)).flatten() {
        return gate;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let can = model.auction_can_query[list];
        (can, can && model.auction_can_get_all)
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        (false, false)
    }
}

/// `list`'s sort criteria, front first.
fn criteria(model: &Model, list: usize) -> Vec<(u8, bool)> {
    if let Some(c) = super::read_live(|l| l.auction_criteria(list)).flatten() {
        return c;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model.auction_sort[list].clone()
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        Vec::new()
    }
}

fn set_criteria(model: &mut Model, list: usize, criteria: Vec<(u8, bool)>) {
    if super::write_live(|l| l.set_auction_criteria(list, criteria.clone()))
        .flatten()
        .is_some()
    {
        return;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model.auction_sort[list] = criteria;
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    let _ = (model, criteria);
}

/// Re-sort one list in place and signal its update event, as the reference's list sorts do.
fn apply_sort(model: &mut crate::script::ModelGuard<'_>, list: usize) {
    if super::write_live(|l| l.auction_sort(list))
        .flatten()
        .is_none()
    {
        #[cfg(any(test, feature = "fixtures"))]
        {
            let criteria = model.auction_sort[list].clone();
            if let Some(state) = model.auction.as_mut() {
                sort_rows(&mut state.lists[list].rows, &criteria);
            }
        }
    }
    model.raise(list_event(list), Vec::new());
}

/// Send `packet` inside the call; a fixture records it.
fn send(model: &mut Model, packet: AuctionPacket) {
    // A view that does not lend the house declines it, and the fixture keeps the packet.
    if super::write_live(|l| l.auction_send(packet.clone()))
        .flatten()
        .is_some()
    {
        return;
    }
    #[cfg(any(test, feature = "fixtures"))]
    model.auction_sent.push(packet);
    #[cfg(not(any(test, feature = "fixtures")))]
    let _ = (model, packet);
}

/// The sell-item clear (`0x59a560`): a held item unlocks and leaves the slot, and
/// `NEW_AUCTION_UPDATE` fires.
fn clear_sell_item(model: &mut crate::script::ModelGuard<'_>) {
    let Some(it) = model.auction_sell_item.take() else {
        return;
    };
    cursor::set_item_lock(model, it.bag, it.slot, false);
    model.raise("NEW_AUCTION_UPDATE", Vec::new());
}

/// A multi-sell's end: the gathered items unlock (`0x59aae0`), the sell slot clears (`0x59a560`)
/// and, on a failure, `AUCTION_MULTISELL_FAILURE` fires (`0x59ab20`).
fn end_multisell(model: &mut crate::script::ModelGuard<'_>, failure: bool) {
    for (bag, slot) in std::mem::take(&mut model.auction_locked) {
        cursor::set_item_lock(model, bag, slot, false);
    }
    clear_sell_item(model);
    if failure {
        model.raise("AUCTION_MULTISELL_FAILURE", Vec::new());
    }
}

/// The next-item sell's outcome raised (`0x59ba80`): the last stack out ends the multi-sell, running
/// dry fails it.
fn sell_step(model: &mut crate::script::ModelGuard<'_>, step: AuctionSellStep) {
    match step {
        AuctionSellStep::Sent => {}
        AuctionSellStep::Done => end_multisell(model, false),
        AuctionSellStep::Failed => end_multisell(model, true),
    }
}

impl super::UiScript {
    /// Push the Browse tab's class tree, the twelve rows of the table at `0xa14f70` in order.
    /// Login-scoped: the stock `AuctionFrameBrowse_OnLoad` reads the classes when the addon loads.
    pub fn set_auction_item_classes(&mut self, classes: Vec<AuctionCategory>) {
        set_field!(self, auction_item_classes, classes);
    }

    /// Empty the sell slot (`0x59a560`), as the auctioneer set does (`0x59f10b`); `NEW_AUCTION_UPDATE`
    /// fires when it held an item.
    pub fn clear_auction_sell_item(&mut self) {
        if self.model_ref().auction_sell_item.is_none() {
            return;
        }
        clear_sell_item(&mut self.model_mut());
    }

    /// The world entry's part (`0x59dfb0`): the sell slot clears with `NEW_AUCTION_UPDATE` when it
    /// held an item, the Auctions tab reads hidden and no list keeps a selection.
    pub fn enter_auction_world(&mut self) {
        let mut model = self.model_mut();
        clear_sell_item(&mut model);
        model.auction_tab_showing = false;
        model.auction_selected = [0; 3];
    }

    /// A multi-sell's end in a handler's turn: its items unlock, the slot clears and, on a
    /// failure, `AUCTION_MULTISELL_FAILURE` fires (`0x59ab20`).
    pub fn end_auction_multisell(&mut self, failure: bool) {
        end_multisell(&mut self.model_mut(), failure);
    }

    /// The next-item sell's outcome in a handler's turn (`0x5a005d`).
    pub fn auction_sell_step(&mut self, step: AuctionSellStep) {
        sell_step(&mut self.model_mut(), step);
    }

    /// Push a fixture's house, rows sorted by its criteria as the reference sorts a page on
    /// arrival. `None` is a closed house.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_auction(&mut self, state: Option<AuctionState>) {
        let mut model = self.model_mut();
        let mut state = state;
        if let Some(s) = state.as_mut() {
            for (list, l) in s.lists.iter_mut().enumerate() {
                sort_rows(&mut l.rows, &model.auction_sort[list]);
            }
        }
        model.auction = state;
    }

    /// A fixture's query gate per list and its get-all gate.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_auction_can_query(&mut self, can: [bool; 3], get_all: bool) {
        set_field!(self, auction_can_query, can);
        set_field!(self, auction_can_get_all, get_all);
    }

    /// The sends a fixture's verbs made, taken.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn take_auction_sent(&mut self) -> Vec<AuctionPacket> {
        drain_field!(self, auction_sent)
    }

    /// A fixture's multi-sell, taken.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn take_auction_multisell(&mut self) -> Option<AuctionMultiSell> {
        drain_option!(self, auction_multisell)
    }
}

/// A Lua number, or a numeric string as `lua_isnumber` accepts it.
fn number(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Integer(n)) => Some(*n as f64),
        Some(Value::Number(n)) => Some(*n),
        Some(Value::String(s)) => s.to_str().ok()?.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// The x87 truncation the reference's number-to-integer casts use.
fn trunc_i64(n: f64) -> i64 {
    n.trunc() as i64
}

/// The sell-item validity test (`0x59d0b0`): the refusal for putting the item at `(bag, slot)` up
/// for sale.
fn sell_refusal(model: &Model, bag: i64, slot: u32, item_id: u32) -> Option<&'static str> {
    let Some(t) = super::item_stats::item_record(model, item_id) else {
        return Some("ERR_ITEM_NOT_FOUND");
    };
    let s = super::container::live_slot(model, bag, slot);
    let s = s.as_ref();
    if s.and_then(|s| s.durability)
        .is_some_and(|(cur, max)| cur < max)
    {
        return Some("ERR_AUCTION_REPAIR_ITEM");
    }
    if s.is_some_and(|s| s.already_bound) {
        return Some(if t.bonding == 4 {
            "ERR_AUCTION_QUEST_ITEM"
        } else {
            "ERR_AUCTION_BOUND_ITEM"
        });
    }
    // Static flags: conjured `0x2`, openable `0x4`.
    if t.flags & 0x2 != 0 {
        return Some("ERR_AUCTION_CONJURED_ITEM");
    }
    if s.is_some_and(|s| s.duration_ms.is_some()) {
        return Some("ERR_AUCTION_LIMITED_DURATION_ITEM");
    }
    // An equipped bag that still holds items.
    if t.container_slots > 0 && bag == cursor::EQUIPMENT_BAG {
        let equipped_bag = i64::from(slot) - 19;
        if super::container::live_bag(model, equipped_bag)
            .is_some_and(|c| c.slots.values().any(|s| s.item_id != 0))
        {
            return Some("ERR_AUCTION_BAG");
        }
    }
    if s.is_some_and(|s| s.flags & 0x8 != 0) {
        return Some("ERR_AUCTION_WRAPPED_ITEM");
    }
    if t.flags & 0x4 != 0 {
        return Some("ERR_AUCTION_LOOT_ITEM");
    }
    None
}

/// The sellable-item count (`0x59f6e0`): the stack total of the sellable items of `item_id` in the
/// backpack and the four bags.
fn sellable_total(model: &Model, item_id: u32) -> u32 {
    sellable_stacks(model, item_id).iter().map(|s| s.2).sum()
}

/// The backpack and bag slots holding a sellable `item_id`, `(bag, slot, count)` in scan order.
fn sellable_stacks(model: &Model, item_id: u32) -> Vec<(i64, u32, u32)> {
    let mut out = Vec::new();
    for bag in 0..=4i64 {
        let Some(c) = super::container::live_bag(model, bag) else {
            continue;
        };
        let mut slots: Vec<u32> = c.slots.keys().copied().collect();
        slots.sort_unstable();
        for slot in slots {
            let s = &c.slots[&slot];
            if s.item_id == item_id && sell_refusal(model, bag, slot, item_id).is_none() {
                out.push((bag, slot, s.count));
            }
        }
    }
    out
}

/// The auction stack gather (`0x59f640`): the sellable stacks until they hold `wanted`, or none
/// when the bags fall short.
fn gather_for_sale(model: &Model, item_id: u32, wanted: u32) -> Vec<(i64, u32, u32)> {
    let mut total = 0u32;
    let mut out = Vec::new();
    for stack in sellable_stacks(model, item_id) {
        total = total.saturating_add(stack.2);
        out.push(stack);
        if wanted != 0 && total >= wanted {
            return out;
        }
    }
    Vec::new()
}

/// The sell slot's stack size: the real count of its source slot.
fn sell_count(model: &Model, it: &CursorItem) -> u32 {
    cursor::held_count(
        model,
        &CursorItem {
            count: None,
            ..it.clone()
        },
    )
}

/// `CalculateAuctionDeposit` (`0x76dde0`): `rate × value / 100` in integers, times the run time's
/// minutes over 240, truncated, at least 100; an unknown run time is the minimum.
fn deposit_for(rate: u32, value: u32, minutes: u32) -> u32 {
    if !RUN_TIMES_MINUTES.contains(&minutes) {
        return MIN_DEPOSIT;
    }
    let base = f64::from(rate.wrapping_mul(value) / 100);
    let deposit = (base * f64::from(minutes) * f64::from(DEPOSIT_PER_MINUTE)) as i64 as u32;
    deposit.max(MIN_DEPOSIT)
}

/// The run time of a 1-based duration index (`0x59f990`): anything but 1..3 is 24 hours.
fn run_time(index: f64) -> u32 {
    let i = trunc_i64(index - 1.0);
    usize::try_from(i)
        .ok()
        .and_then(|i| RUN_TIMES_MINUTES.get(i).copied())
        .unwrap_or(DEFAULT_RUN_TIME_MINUTES)
}

/// The table at `0xa14fd0`: the wire id, the GlobalString token, then whether Armor's first
/// subclass offers the slot and whether every later Armor subclass hides it.
const AUCTION_INV_TYPES: [(u32, &str, bool, bool); 14] = [
    (1, "INVTYPE_HEAD", true, false),
    (2, "INVTYPE_NECK", true, true),
    (3, "INVTYPE_SHOULDER", false, false),
    (4, "INVTYPE_BODY", true, true),
    (5, "INVTYPE_CHEST", false, false),
    (6, "INVTYPE_WAIST", false, false),
    (7, "INVTYPE_LEGS", false, false),
    (8, "INVTYPE_FEET", false, false),
    (9, "INVTYPE_WRIST", false, false),
    (10, "INVTYPE_HAND", false, false),
    (11, "INVTYPE_FINGER", true, true),
    (12, "INVTYPE_TRINKET", true, true),
    (16, "INVTYPE_CLOAK", false, true),
    (23, "INVTYPE_HOLDABLE", true, true),
];

/// The Armor class id, the one class whose slot rows `GetAuctionInvTypes` filters.
const ARMOR_CLASS: u32 = 4;

/// The `INVTYPE_CLOAK` wire id, which Armor's second subclass (Cloth) offers anyway.
const CLOAK_INV_TYPE: u32 = 16;

fn usage(text: &str) -> mlua::Error {
    mlua::Error::RuntimeError(text.into())
}

fn string_arg(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => s.to_str().ok().map(|s| s.to_string()),
        Some(Value::Integer(n)) => Some(n.to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumAuctionItems(type) → the batch size, then the pre-cap total (`0x59c1a0`); the owner
    // list's total is its batch.
    g.set(
        "GetNumAuctionItems",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let kind = string_arg(a.first())
                .ok_or_else(|| usage("Usage: GetNumAuctionItems(\"type\")"))?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (batch, total) = list_index(&kind).map_or((0, 0), |i| {
                let (rows, total) = counts(&model, i);
                let batch = rows as i64;
                (batch, if i == OWNER { batch } else { i64::from(total) })
            });
            Ok((batch, total))
        })?,
    )?;

    // GetAuctionItemInfo(type, index) → thirteen values (`0x59d5e0`), the last `saleStatus`; a
    // miss or an uncached template answers the defaults with count 1 and quality -1.
    g.set(
        "GetAuctionItemInfo",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index)) = (string_arg(a.first()), number(a.get(1))) else {
                return Err(usage("Usage: GetAuctionItemInfo(\"type\", index)"));
            };
            let index = usize::try_from(trunc_i64(index)).unwrap_or(0);
            let (row, can_use) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let row = row(&model, &kind, index, true).filter(|r| r.name.is_some());
                let can_use = row
                    .as_ref()
                    .is_some_and(|r| super::item_stats::item_usable_by_id(&model, r.item_id));
                (row, can_use)
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(1),
                    Value::Integer(-1),
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            let opt_str = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(v) => Value::String(lua.create_string(v)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&r.name)?,
                opt_str(&r.texture)?,
                Value::Integer(i64::from(r.count)),
                Value::Integer(i64::from(r.quality.unwrap_or(0))),
                flag(can_use),
                Value::Integer(i64::from(r.level.max(1))),
                Value::Integer(i64::from(r.min_bid)),
                Value::Integer(i64::from(r.min_increment)),
                Value::Integer(i64::from(r.buyout_price)),
                Value::Integer(i64::from(r.bid_amount)),
                match &r.high_bidder {
                    AuctionHighBidder::Nil => Value::Nil,
                    AuctionHighBidder::Player => flag(true),
                    AuctionHighBidder::Name(n) => Value::String(lua.create_string(n)?),
                },
                opt_str(&r.owner)?,
                Value::Integer(i64::from(r.sold)),
            ]))
        })?,
    )?;

    // GetAuctionItemTimeLeft(type, index) (`0x59fd60`) → the 1..4 bucket of the time to the
    // expiry, for a sold row the seconds to delivery, 1 once past it, 0 for a miss. A sold row past
    // its delivery, or any row 10 s past its expiry, expires inside the call (`0x59fe7d`).
    g.set(
        "GetAuctionItemTimeLeft",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index)) = (string_arg(a.first()), number(a.get(1))) else {
                return Err(usage("Usage: GetAuctionItemTimeLeft(\"type\", index)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            let index = usize::try_from(trunc_i64(index)).unwrap_or(0);
            let Some(r) = row(&model, &kind, index, false) else {
                return Ok(0i64);
            };
            let left_ms = r.time_left * 1000.0;
            if left_ms > 0.0 {
                if r.sold {
                    return Ok((left_ms / 1000.0) as i64);
                }
                return Ok(TIME_LEFT_THRESHOLDS_MS
                    .iter()
                    .position(|&t| left_ms < t)
                    .map_or(4, |i| i as i64 + 1));
            }
            if r.sold || -left_ms > EXPIRY_GRACE_MS {
                let events = super::write_live(|l| l.auction_expired(r.auction_id))
                    .flatten()
                    .unwrap_or_default();
                for event in events {
                    model.raise(event, Vec::new());
                }
            }
            Ok(1)
        })?,
    )?;

    // GetAuctionItemLink(type, index) (`0x59c2d0`): no values on a miss or an uncached template.
    g.set(
        "GetAuctionItemLink",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index)) = (string_arg(a.first()), number(a.get(1))) else {
                return Err(usage("Usage: GetAuctionItemLink(\"type\", index)"));
            };
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let index = usize::try_from(trunc_i64(index)).unwrap_or(0);
                row(&model, &kind, index, false).and_then(|r| r.link)
            };
            Ok(match link {
                Some(l) => MultiValue::from_vec(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetSelectedAuctionItem(type) (`0x59c920`) → the selected row, 0 for none; an unknown type
    // answers 1. Stored as the auction id, so it follows its auction through a re-sort.
    g.set(
        "GetSelectedAuctionItem",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let kind = string_arg(a.first())
                .ok_or_else(|| usage("Usage: GetSelectedAuctionItem(\"type\")"))?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(i) = list_index(&kind) else {
                return Ok(1i64);
            };
            let id = model.auction_selected[i];
            if id == 0 {
                return Ok(0);
            }
            Ok(find(&model, i, id) as i64)
        })?,
    )?;

    // SetSelectedAuctionItem(type, index) (`0x59c830`): remembers the auction at that row; out of
    // range clears.
    g.set(
        "SetSelectedAuctionItem",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index)) = (string_arg(a.first()), number(a.get(1))) else {
                return Err(usage("Usage: SetSelectedAuctionItem(\"type\", index)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            let Some(i) = list_index(&kind) else {
                return Ok(());
            };
            let id = usize::try_from(trunc_i64(index))
                .ok()
                .and_then(|index| row(&model, &kind, index, false))
                .map_or(0, |r| r.auction_id);
            model.auction_selected[i] = id;
            Ok(())
        })?,
    )?;

    // SortAuctionItems(type, sort) (`0x59dd00`): the column flips if primary, else leads
    // unreversed, then the list re-sorts.
    g.set(
        "SortAuctionItems",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(sort)) = (string_arg(a.first()), string_arg(a.get(1))) else {
                return Err(usage("Usage: SortAuctionItems(\"type\", \"sort\")"));
            };
            let (Some(list), Some(column)) = (list_index(&kind), sort_column(&sort)) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            let mut c = criteria(&model, list);
            let reversed = match c.first() {
                Some(&(first, rev)) if first == column => !rev,
                _ => false,
            };
            set_criterion(&mut c, column, reversed);
            set_criteria(&mut model, list, c);
            apply_sort(&mut model, list);
            Ok(())
        })?,
    )?;

    // IsAuctionSortReversed(type, sort) (`0x59ab90`) → `reversed` (1/nil) and 1 for a column the
    // list sorts by, nil and nil otherwise; an unknown column answers nothing.
    g.set(
        "IsAuctionSortReversed",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(sort)) = (string_arg(a.first()), string_arg(a.get(1))) else {
                return Err(usage("Usage: IsAuctionSortReversed(\"type\", \"sort\")"));
            };
            let Some(column) = sort_column(&sort) else {
                return Ok(MultiValue::new());
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let found = list_index(&kind).and_then(|i| {
                criteria(&model, i)
                    .into_iter()
                    .find(|&(c, _)| c == column)
                    .map(|(_, rev)| rev)
            });
            Ok(MultiValue::from_vec(match found {
                Some(rev) => vec![flag(rev), Value::Integer(1)],
                None => vec![Value::Nil, Value::Nil],
            }))
        })?,
    )?;

    // GetAuctionSort(type, index) (`0x59ad40`) → the criterion's column name and `reversed`
    // (1/nil); nil and nil past the list's criteria.
    g.set(
        "GetAuctionSort",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index)) = (string_arg(a.first()), number(a.get(1))) else {
                return Err(usage("Usage: GetAuctionSort(\"type\", \"index\")"));
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let found = list_index(&kind).and_then(|i| {
                usize::try_from(trunc_i64(index) - 1)
                    .ok()
                    .and_then(|n| criteria(&model, i).get(n).copied())
            });
            Ok(MultiValue::from_vec(match found {
                Some((column, rev)) => vec![
                    Value::String(lua.create_string(SORT_COLUMNS[usize::from(column)])?),
                    flag(rev),
                ],
                None => vec![Value::Nil, Value::Nil],
            }))
        })?,
    )?;

    // SortAuctionClearSort(type) (`0x59ae70`): the list keeps no criteria.
    g.set(
        "SortAuctionClearSort",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let kind = string_arg(a.first())
                .ok_or_else(|| usage("Usage: SortAuctionClearSort(\"type\")"))?;
            if let Some(i) = list_index(&kind) {
                set_criteria(&mut crate::script::model_mut_of(lua), i, Vec::new());
            }
            Ok(())
        })?,
    )?;

    // SortAuctionSetSort(type, sort, reversed) (`0x59af10`): the column moves to the front;
    // `reversed` counts only as the boolean true.
    g.set(
        "SortAuctionSetSort",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(sort)) = (string_arg(a.first()), string_arg(a.get(1))) else {
                return Err(usage(
                    "Usage: SortAuctionSetSort(\"type\", \"sort\", reversed)",
                ));
            };
            let reversed = matches!(a.get(2), Some(Value::Boolean(true)));
            if let (Some(i), Some(column)) = (list_index(&kind), sort_column(&sort)) {
                let mut model = crate::script::model_mut_of(lua);
                let mut c = criteria(&model, i);
                set_criterion(&mut c, column, reversed);
                set_criteria(&mut model, i, c);
            }
            Ok(())
        })?,
    )?;

    // SortAuctionApplySort(type) (`0x59cab0`): re-sort the list and fire its update event.
    g.set(
        "SortAuctionApplySort",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let kind = string_arg(a.first())
                .ok_or_else(|| usage("Usage: SortAuctionApplySort(\"type\")"))?;
            if let Some(i) = list_index(&kind) {
                apply_sort(&mut crate::script::model_mut_of(lua), i);
            }
            Ok(())
        })?,
    )?;

    // SetAuctionsTabShowing(showing) (`0x59b0b0`): with it set, a bag right-click fills the sell
    // slot (`0x5d88e0`).
    g.set(
        "SetAuctionsTabShowing",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let Some(Value::Boolean(showing)) = a.first() else {
                return Err(usage("Usage: SetAuctionsTabShowing(bool)"));
            };
            crate::script::model_mut_of(lua).auction_tab_showing = *showing;
            Ok(())
        })?,
    )?;

    // CanSendAuctionQuery([type]) (`0x59c740`) → whether a query of that list may go out, then
    // whether a get-all one may; the type defaults to the Browse list.
    g.set(
        "CanSendAuctionQuery",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let list = string_arg(a.first())
                .and_then(|k| list_index(&k))
                .unwrap_or(LIST);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (can, get_all) = gate(&model, list);
            Ok((flag(can), flag(get_all)))
        })?,
    )?;

    // GetAuctionItemClasses() (`0x59c420`) → the class names of the twelve-row table that the
    // player's ItemClass.dbc holds; no session needed.
    g.set(
        "GetAuctionItemClasses",
        lua.create_function(|lua, ()| {
            let names: Vec<String> = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .auction_item_classes
                    .iter()
                    .filter(|c| !c.name.is_empty())
                    .map(|c| c.name.clone())
                    .collect()
            };
            let mut out = Vec::with_capacity(names.len());
            for n in &names {
                out.push(Value::String(lua.create_string(n)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetAuctionItemSubClasses(classIndex) (`0x59c480`) → the class's subclass names; nothing for
    // an index past the twelve or a class whose second column is 0.
    g.set(
        "GetAuctionItemSubClasses",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let index =
                number(a.first()).ok_or_else(|| usage("Usage: GetAuctionItemSubClasses(index)"))?;
            let names: Vec<String> = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                usize::try_from(trunc_i64(index) - 1)
                    .ok()
                    .and_then(|i| model.auction_item_classes.get(i))
                    .filter(|c| c.has_subclass_filter)
                    .map_or_else(Vec::new, |c| {
                        c.subclasses.iter().map(|s| s.name.clone()).collect()
                    })
            };
            let mut out = Vec::with_capacity(names.len());
            for n in &names {
                out.push(Value::String(lua.create_string(n)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetAuctionInvTypes(classIndex, subClassIndex) (`0x59c590`) → 28 values, a token and 1 or
    // nil per slot; nothing without a class, or for a found subclass lacking `Flags & 0x200`.
    // Armor shows its first subclass the slots flagged for it and hides the flagged ones from
    // the rest, Cloth keeping the cloak.
    g.set(
        "GetAuctionInvTypes",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(class_index), Some(sub_index)) = (number(a.first()), number(a.get(1))) else {
                return Err(usage(
                    "Usage: GetAuctionInvTypes(classIndex, subClassIndex)",
                ));
            };
            let sub = trunc_i64(sub_index) - 1;
            let class = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                usize::try_from(trunc_i64(class_index) - 1)
                    .ok()
                    .and_then(|i| model.auction_item_classes.get(i))
                    .filter(|c| c.has_subclass_filter)
                    .filter(|c| {
                        // An index the scan runs past skips the flag test.
                        usize::try_from(sub)
                            .ok()
                            .and_then(|i| c.subclasses.get(i))
                            .is_none_or(|s| s.has_inv_types)
                    })
                    .map(|c| c.class_id)
            };
            let Some(class_id) = class else {
                return Ok(MultiValue::new());
            };
            let mut out = Vec::with_capacity(28);
            for &(inv_type, token, first_shows, rest_hides) in &AUCTION_INV_TYPES {
                out.push(Value::String(lua.create_string(token)?));
                let shown = class_id != ARMOR_CLASS
                    || match sub {
                        0 => first_shows,
                        1 if inv_type == CLOAK_INV_TYPE => true,
                        _ => !rest_hides,
                    };
                out.push(if shown { Value::Integer(1) } else { Value::Nil });
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // QueryAuctionItems(name, minLevel, maxLevel, invTypeIndex, classIndex, subclassIndex, page,
    // usable, quality[, getAll]) (`0x59bcf0`): with a session and the throttle open, the search goes
    // out inside the call with the Browse list's criteria; each menu position maps to its wire id.
    g.set(
        "QueryAuctionItems",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let mut model = crate::script::model_mut_of(lua);
            if !house_open(&model) || !gate(&model, LIST).0 {
                return Ok(());
            }
            let get_all = matches!(a.get(9), Some(Value::Boolean(true)));
            if get_all && !gate(&model, LIST).1 {
                return Ok(());
            }
            let name = string_arg(a.first()).unwrap_or_default();
            let byte = |i: usize| number(a.get(i)).map_or(0, |n| trunc_i64(n) as u8);
            let position =
                |i: usize| number(a.get(i)).and_then(|n| usize::try_from(trunc_i64(n) - 1).ok());
            let inv_type = position(3)
                .and_then(|i| AUCTION_INV_TYPES.get(i))
                .map_or(u32::MAX, |t| t.0);
            let category = position(4).and_then(|i| model.auction_item_classes.get(i));
            let class = category.map_or(u32::MAX, |c| c.class_id);
            // The subclass scan never reads the class's filter flag.
            let sub_class = category
                .and_then(|c| position(5).and_then(|i| c.subclasses.get(i)))
                .map_or(u32::MAX, |s| s.sub_id);
            let page = number(a.get(6)).map_or(0, trunc_i64);
            let quality = number(a.get(8)).map_or(u32::MAX, |n| trunc_i64(n) as u32);
            let query = AuctionQuery {
                name,
                min_level: byte(1),
                max_level: byte(2),
                inv_type,
                class,
                sub_class,
                quality,
                list_from: (page * 50) as u32,
                usable: byte(7),
                get_all,
            };
            send(&mut model, AuctionPacket::Query(query));
            // A fixture's query is in flight from here (`[0xc0e734]`): its gate closes.
            #[cfg(any(test, feature = "fixtures"))]
            {
                model.auction_can_query[LIST] = false;
            }
            Ok(())
        })?,
    )?;

    // GetOwnerAuctionItems() (`0x59d4f0`): the owner list and the pending sales go out inside the
    // call while the owner gate and the query gate are open (`0x59cc50`).
    g.set(
        "GetOwnerAuctionItems",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            if house_open(&model) {
                send(&mut model, AuctionPacket::OwnerList);
            }
            Ok(())
        })?,
    )?;

    // GetBidderAuctionItems([page]) (`0x59d540`): `50 × page` goes out inside the call while the
    // bidder gate and the query gate are open (`0x59ce50`).
    g.set(
        "GetBidderAuctionItems",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let mut model = crate::script::model_mut_of(lua);
            if house_open(&model) {
                let page = number(a.first()).map_or(0, trunc_i64);
                send(&mut model, AuctionPacket::BidderList((page * 50) as u32));
            }
            Ok(())
        })?,
    )?;

    // PlaceAuctionBid(type, index, bid) (`0x59da40`): clamped to the buyout, then refused locally
    // under the opening price, at or under the current bid, short of the step, past the purse, or past
    // the carried count.
    g.set(
        "PlaceAuctionBid",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(kind), Some(index), Some(bid)) =
                (string_arg(a.first()), number(a.get(1)), number(a.get(2)))
            else {
                return Err(usage("Usage: PlaceAuctionBid(\"type\", index, bid)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            let Some(r) = usize::try_from(trunc_i64(index))
                .ok()
                .and_then(|i| row(&model, &kind, i, false))
            else {
                return Ok(());
            };
            if !house_open(&model) {
                return Ok(());
            }
            let mut bid = trunc_i64(bid) as u32;
            if r.buyout_price != 0 && bid > r.buyout_price {
                bid = r.buyout_price;
            }
            let refusal = if bid < r.min_bid {
                Some("ERR_AUCTION_MIN_BID")
            } else if bid <= r.bid_amount {
                Some("ERR_AUCTION_HIGHER_BID")
            } else if r.bid_amount != 0
                && bid < r.bid_amount.saturating_add(r.min_increment)
                && (r.buyout_price == 0 || bid < r.buyout_price)
            {
                Some("ERR_AUCTION_BID_INCREMENT")
            } else {
                None
            };
            if let Some(key) = refusal {
                drop(model);
                crate::script::ui_errors::show_now(lua, key);
                return Ok(());
            }
            if bid > MAXIMUM_BID && bid > r.bid_amount.saturating_add(r.min_increment) {
                return Ok(());
            }
            // Raising one's own bid costs only the difference.
            let cost = if r.bidder_status == 2 {
                bid - r.bid_amount
            } else {
                bid
            };
            if crate::script::live::money(&model) < u64::from(cost) {
                drop(model);
                crate::script::ui_errors::show_now(lua, "ERR_NOT_ENOUGH_MONEY");
                return Ok(());
            }
            // The carried-count check with its message on the row's item and count (`0x59dc7c`).
            if let Some(refusal) =
                crate::script::read_live(|l| l.carry_refusal(r.item_id, r.count)).flatten()
            {
                drop(model);
                crate::script::ui_errors::show_carry_refusal(lua, &refusal);
                return Ok(());
            }
            send(
                &mut model,
                AuctionPacket::Bid {
                    auction_id: r.auction_id,
                    amount: bid,
                },
            );
            Ok(())
        })?,
    )?;

    // StartAuction(minBid, buyoutPrice, runTime[, stackSize, numStacks]) (`0x59f990`): runTime is
    // the duration menu's 1..3. The sell item alone covering one stack goes out inside the call and
    // empties the slot (`0x59fc66`); anything more gathers the sellable stacks (`0x59f640`) and
    // lists one stack now (`0x59ba80`), a multi-sell of several firing its start and locking them.
    g.set(
        "StartAuction",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let (Some(min_bid), Some(buyout), Some(duration)) =
                (number(a.first()), number(a.get(1)), number(a.get(2)))
            else {
                return Err(usage(
                    "Usage: StartAuction(minBid, buyoutPrice, runTime, stackSize, numStacks)",
                ));
            };
            let mut model = crate::script::model_mut_of(lua);
            if !house_open(&model) {
                return Ok(());
            }
            let Some(it) = model.auction_sell_item.clone() else {
                return Ok(());
            };
            if !super::item_stats::item_record_cached(&model, it.item_id) {
                return Ok(());
            }
            let worn = super::container::live_slot(&model, it.bag, it.slot)
                .and_then(|s| s.durability)
                .is_some_and(|(cur, max)| cur < max);
            if worn {
                model.show("ERR_AUCTION_REPAIR_ITEM");
                return Ok(());
            }
            let min_bid = trunc_i64(min_bid) as u32;
            let mut buyout = trunc_i64(buyout) as u32;
            if buyout != 0 && buyout < min_bid {
                buyout = min_bid;
            }
            let minutes = run_time(duration);
            let stack_size = number(a.get(3)).map_or(0, |n| trunc_i64(n) as u32);
            let num_stacks = number(a.get(4)).map_or(1, |n| trunc_i64(n) as u32);
            let guid = |model: &Model, bag: i64, slot: u32| {
                super::container::live_slot(model, bag, slot).map_or(0, |s| s.guid)
            };
            if sell_count(&model, &it) >= stack_size && num_stacks <= 1 {
                let item = guid(&model, it.bag, it.slot);
                send(
                    &mut model,
                    AuctionPacket::Sell {
                        item,
                        count: stack_size,
                        bid: min_bid,
                        buyout,
                        minutes,
                    },
                );
                clear_sell_item(&mut model);
                return Ok(());
            }
            let stacks = gather_for_sale(&model, it.item_id, stack_size.saturating_mul(num_stacks));
            if stacks.is_empty() {
                model.show("ERR_AUCTION_ENOUGH_ITEMS");
                clear_sell_item(&mut model);
                return Ok(());
            }
            let sell = AuctionMultiSell {
                items: stacks
                    .iter()
                    .map(|&(bag, slot, count)| (guid(&model, bag, slot), count))
                    .collect(),
                stack_size,
                num_stacks,
                bid: min_bid,
                buyout,
                minutes,
            };
            let live = super::write_live(|l| l.auction_multisell_start(sell.clone()))
                .flatten()
                .is_some();
            #[cfg(any(test, feature = "fixtures"))]
            if !live {
                model.auction_multisell = Some(sell);
            }
            #[cfg(not(any(test, feature = "fixtures")))]
            let _ = sell;
            let next = |model: &mut crate::script::ModelGuard<'_>| {
                let step = if live {
                    super::write_live(|l| l.auction_sell_next()).flatten()
                } else {
                    None
                };
                sell_step(model, step.unwrap_or(AuctionSellStep::Sent));
            };
            if num_stacks <= 1 {
                next(&mut model);
                clear_sell_item(&mut model);
            } else {
                model.raise(
                    "AUCTION_MULTISELL_START",
                    vec![ScriptValue::Int(i64::from(num_stacks))],
                );
                for &(bag, slot, _) in &stacks {
                    model.auction_locked.push((bag, slot));
                    cursor::set_item_lock(&mut model, bag, slot, true);
                }
                next(&mut model);
            }
            Ok(())
        })?,
    )?;

    // CancelSell() (`0x59b040`): a pending multi-sell fails (`0x59ab20`).
    g.set(
        "CancelSell",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            if super::write_live(|l| l.auction_pending_sell(true))
                .flatten()
                .unwrap_or(false)
            {
                end_multisell(&mut model, true);
            }
            Ok(())
        })?,
    )?;

    // CancelAuction(index) (`0x59ddf0`), an "owner" row not sold; with a bid on it the purse must
    // hold the 5% cut.
    g.set(
        "CancelAuction",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let index = number(a.first()).ok_or_else(|| usage("Usage: CancelAuction(index)"))?;
            let mut model = crate::script::model_mut_of(lua);
            if !house_open(&model) {
                return Ok(());
            }
            let Some(r) = usize::try_from(trunc_i64(index))
                .ok()
                .and_then(|i| row(&model, "owner", i, false))
                .filter(|r| !r.sold)
            else {
                return Ok(());
            };
            if r.bid_amount != 0 {
                let cut = (r.bid_amount as f32 * 0.05) as u64;
                if crate::script::live::money(&model) < cut {
                    drop(model);
                    crate::script::ui_errors::show_now(lua, "ERR_NOT_ENOUGH_MONEY");
                    return Ok(());
                }
            }
            send(
                &mut model,
                AuctionPacket::Cancel {
                    auction_id: r.auction_id,
                },
            );
            Ok(())
        })?,
    )?;

    // CanCancelAuction(index) (`0x59c9f0`) → 1 for an unsold owner row, nil otherwise; nothing
    // without a session.
    g.set(
        "CanCancelAuction",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let index = number(a.first()).ok_or_else(|| usage("Usage: CanCancelAuction(index)"))?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !house_open(&model) {
                return Ok(MultiValue::new());
            }
            let can = usize::try_from(trunc_i64(index))
                .ok()
                .and_then(|i| row(&model, "owner", i, false))
                .is_some_and(|r| !r.sold);
            Ok(MultiValue::from_vec(vec![flag(can)]))
        })?,
    )?;

    g.set(
        "GetAuctionHouseDepositRate",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(deposit_rate(&model).map_or(0, i64::from))
        })?,
    )?;

    // CalculateAuctionDeposit(runTime[, count]) (`0x59d270`) → copper for `count` of the sell
    // item (its stack by default) over the run time; 0 with no house or sell item.
    g.set(
        "CalculateAuctionDeposit",
        lua.create_function(|lua, args: MultiValue| {
            let a: Vec<Value> = args.into_iter().collect();
            let duration = number(a.first())
                .ok_or_else(|| usage("Usage: CalculateAuctionDeposit(runTime)"))?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (Some(rate), Some(it)) = (deposit_rate(&model), model.auction_sell_item.as_ref())
            else {
                return Ok(0i64);
            };
            let Some(t) = super::item_stats::item_record(&model, it.item_id) else {
                return Ok(0);
            };
            let count =
                number(a.get(1)).map_or_else(|| sell_count(&model, it), |n| trunc_i64(n) as u32);
            Ok(i64::from(deposit_for(
                rate,
                count.wrapping_mul(t.sell_price),
                run_time(duration),
            )))
        })?,
    )?;

    // GetAuctionSellItemInfo() (`0x59f760`) → name, texture, count, quality, canUse, price,
    // pricePerUnit, stackCount, totalCount: nine values always, the empty slot's count 1 and
    // quality -1.
    g.set(
        "GetAuctionSellItemInfo",
        lua.create_function(|lua, ()| {
            let info = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.auction_sell_item.as_ref().and_then(|it| {
                    let t = super::item_stats::item_record(&model, it.item_id)?;
                    let count = sell_count(&model, it);
                    Some((
                        cursor::item_link_name(it.link.as_deref()),
                        it.texture.clone(),
                        count,
                        t.quality,
                        super::item_stats::item_usable_by_id(&model, it.item_id),
                        t.sell_price,
                        t.stackable,
                        sellable_total(&model, it.item_id),
                    ))
                })
            };
            let Some((name, texture, count, quality, can_use, price, stackable, total)) = info
            else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(1),
                    Value::Integer(-1),
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                match &texture {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(count)),
                Value::Integer(i64::from(quality)),
                flag(can_use),
                Value::Integer(i64::from(price.wrapping_mul(count))),
                Value::Integer(i64::from(price)),
                Value::Integer(i64::from(stackable)),
                Value::Integer(i64::from(total)),
            ]))
        })?,
    )?;

    // ClickAuctionSellItemButton() (`0x59d410`): swap the cursor's item with the sell slot's and
    // fire `NEW_AUCTION_UPDATE` (`0x59d4df`); an item the house refuses stays held with its reason.
    g.set(
        "ClickAuctionSellItemButton",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            match click_auction_sell_item(&mut model) {
                Click::Refused(key) => model.show(key),
                Click::Swapped => model.raise("NEW_AUCTION_UPDATE", Vec::new()),
                Click::Nothing => {}
            }
            Ok(())
        })?,
    )?;

    // CloseAuctionHouse() (`0x59f750` -> `0x59f150`): with a house open, `AUCTION_HOUSE_CLOSED`
    // fires, a pending multi-sell fails, and the interact target clears on the auctioneer
    // (`0x59f189`); when the auctioneer held it, its interaction close runs the house's close again
    // (`0x51304a`), a second `AUCTION_HOUSE_CLOSED`. Then the house empties and the sell slot
    // clears. Nothing goes on the wire.
    g.set(
        "CloseAuctionHouse",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if !house_open(&model) {
                return Ok(());
            }
            model.raise("AUCTION_HOUSE_CLOSED", Vec::new());
            if super::write_live(|l| l.auction_pending_sell(true))
                .flatten()
                .unwrap_or(false)
            {
                end_multisell(&mut model, true);
            }
            if super::write_live(|l| l.auction_clear_target())
                .flatten()
                .unwrap_or(false)
            {
                model.raise("AUCTION_HOUSE_CLOSED", Vec::new());
            }
            if super::write_live(|l| l.auction_end_session())
                .flatten()
                .is_none()
            {
                #[cfg(any(test, feature = "fixtures"))]
                {
                    model.auction = None;
                }
            }
            clear_sell_item(&mut model);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// `UseContainerItem`'s auction arm (`0x5d88e0`): with the Auctions tab showing, the clicked item
/// replaces the sell slot's.
pub(super) fn use_container_item(
    model: &mut crate::script::ModelGuard<'_>,
    bag: i64,
    slot: u32,
) -> Arm {
    if !model.auction_tab_showing || !house_open(model) {
        return Arm::Pass;
    }
    let Some(s) = super::container::index_slot(model, bag, slot).filter(|s| s.item_id != 0) else {
        return Arm::Pass;
    };
    if let Some(key) = sell_refusal(model, bag, slot, s.item_id) {
        return Arm::Refused(key);
    }
    if let Some(prev) = model.auction_sell_item.take() {
        cursor::set_item_lock(model, prev.bag, prev.slot, false);
    }
    // The lock signals only on its edge (`0x706fe0`).
    model.auction_sell_item = Some(CursorItem {
        bag,
        slot,
        item_id: s.item_id,
        texture: s.texture.clone(),
        link: s.link.clone(),
        count: None,
        quality: s.quality,
        equip_slots: s.equip_slots.clone(),
        bar_placeable: s.bar_placeable,
    });
    cursor::set_item_lock(model, bag, slot, true);
    model.raise("NEW_AUCTION_UPDATE", Vec::new());
    Arm::Took
}

/// What the sell slot's click did.
enum Click {
    /// The house refuses the held item: it stays held, with this reason.
    Refused(&'static str),
    /// The slot's item and the cursor's traded places.
    Swapped,
    Nothing,
}

/// The sell slot's click (`0x59d410`): a refused item stays held with its reason; the same item
/// goes back; otherwise the cursor's item and the slot's trade places, either side possibly empty.
fn click_auction_sell_item(model: &mut crate::script::ModelGuard<'_>) -> Click {
    let held = match &model.cursor {
        Some(CursorPayload::Item(item)) => Some(item.clone()),
        None => None,
        Some(_) => return Click::Nothing,
    };
    if let Some(item) = &held {
        if let Some(key) = sell_refusal(model, item.bag, item.slot, item.item_id) {
            return Click::Refused(key);
        }
        let same = model
            .auction_sell_item
            .as_ref()
            .is_some_and(|s| s.bag == item.bag && s.slot == item.slot);
        if same {
            cursor::clear_cursor(model);
            return Click::Nothing;
        }
    }
    let previous = model.auction_sell_item.take();
    model.auction_sell_item = held.map(|item| CursorItem {
        count: None,
        ..item
    });
    // The slot's item back through the item setter, neither item unlocking (`0x59d4d3`).
    cursor::swap_cursor_item(model, previous);
    Click::Swapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A session holding one browse page of the given auction ids, named and unbid.
    fn page(ids: &[u32]) -> AuctionState {
        let rows: Vec<AuctionItemRow> = ids
            .iter()
            .map(|&auction_id| AuctionItemRow {
                auction_id,
                item_id: 1000 + auction_id,
                name: Some(format!("Item {auction_id}")),
                count: 1,
                min_bid: 100 * auction_id,
                ..Default::default()
            })
            .collect();
        let mut state = AuctionState::default();
        state.lists[LIST] = AuctionListState {
            total: rows.len() as u32,
            rows,
        };
        state
    }

    /// The name sort reads the record's own name (`0x59b4d2`): two rolls of one item tie on it, and
    /// the next criterion decides.
    #[test]
    fn the_name_sort_ignores_the_suffix() {
        let row = |auction_id: u32, suffix: &str, count: u32| AuctionItemRow {
            auction_id,
            name: Some(format!("Bandit Cinch {suffix}")),
            base_name: Some("Bandit Cinch".into()),
            count,
            ..Default::default()
        };
        let rows = [row(1, "of the Wolf", 2), row(2, "of the Bear", 1)];
        assert_eq!(
            auction_sort_order(&rows, &[(5, false), (9, false)]),
            vec![1, 0],
            "the quantity decides, not the suffix"
        );
    }

    fn sorts(s: &UiScript, kind: &str) -> Vec<(String, bool)> {
        s.eval::<Vec<String>>(&format!(
            r#"local t = {{}}
            for i = 1, 12 do
                local c, r = GetAuctionSort("{kind}", i)
                if not c then break end
                t[i] = c .. (r and "+" or "")
            end
            return t"#
        ))
        .unwrap()
        .into_iter()
        .map(|c| match c.strip_suffix('+') {
            Some(name) => (name.to_string(), true),
            None => (c, false),
        })
        .collect()
    }

    #[test]
    fn the_default_criteria_are_initialize_games() {
        let s = UiScript::new().unwrap();
        let list = sorts(&s, "list");
        assert_eq!(list[0], ("quality".to_string(), false));
        assert_eq!(list[1], ("level".to_string(), true));
        assert_eq!(list.len(), 7);
        assert_eq!(sorts(&s, "owner")[0].0, "duration");
        assert_eq!(sorts(&s, "bidder").len(), 8);
    }

    #[test]
    fn set_sort_builds_the_stock_tables_front_first() {
        let s = UiScript::new().unwrap();
        // AuctionFrame_SetSort("list", "quality", false) over `AuctionSort["list_quality"]`.
        s.run(
            r#"SortAuctionClearSort("list")
            for _, c in ipairs({{"duration",false},{"bid",false},{"quantity",true},
                {"minbidbuyout",false},{"name",false},{"level",true},{"quality",false}}) do
                SortAuctionSetSort("list", c[1], c[2])
            end"#,
        )
        .unwrap();
        let list = sorts(&s, "list");
        assert_eq!(list.len(), 7);
        assert_eq!(list[0], ("quality".to_string(), false));
        assert_eq!(list[1], ("level".to_string(), true));
        assert_eq!(list[6], ("duration".to_string(), false));
        // A repeated column moves to the front rather than growing the list.
        s.run(r#"SortAuctionSetSort("list", "BID", true)"#).unwrap();
        let list = sorts(&s, "list");
        assert_eq!(list.len(), 7);
        assert_eq!(list[0], ("bid".to_string(), true));
        let (rev, found): (Value, Value) = s
            .eval(r#"return IsAuctionSortReversed("list", "bid")"#)
            .unwrap();
        assert_eq!((rev, found), (Value::Integer(1), Value::Integer(1)));
        let n = s.arity(r#"IsAuctionSortReversed("list", "nope")"#).unwrap();
        assert_eq!(n, 0, "an unknown column answers nothing");
        s.run(r#"SortAuctionClearSort("list")"#).unwrap();
        assert!(sorts(&s, "list").is_empty());
    }

    #[test]
    fn rows_sort_on_push_and_on_apply() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SortAuctionClearSort("list") SortAuctionSetSort("list", "bid", true)"#)
            .unwrap();
        s.set_auction(Some(page(&[1, 3, 2])));
        let first: i64 = s
            .eval(r#"local _,_,_,_,_,_,m = GetAuctionItemInfo("list", 1) return m"#)
            .unwrap();
        assert_eq!(first, 300, "highest bid first, reversed");

        crate::script::tick::fired::take();
        s.run(r#"SortAuctionSetSort("list", "bid", false) SortAuctionApplySort("list")"#)
            .unwrap();
        let first: i64 = s
            .eval(r#"local _,_,_,_,_,_,m = GetAuctionItemInfo("list", 1) return m"#)
            .unwrap();
        assert_eq!(first, 100);
        assert!(crate::script::tick::fired::names()
            .iter()
            .any(|e| e == "AUCTION_ITEM_LIST_UPDATE"));
    }

    #[test]
    fn the_selection_follows_its_auction_through_a_resort() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SortAuctionClearSort("list")"#).unwrap();
        s.set_auction(Some(page(&[11, 22])));
        s.run(r#"SetSelectedAuctionItem("list", 2)"#).unwrap();
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            2
        );
        s.set_auction(Some(page(&[22, 11])));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            1
        );
        s.set_auction(Some(page(&[33, 44])));
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            0
        );
        s.set_auction(None);
        assert_eq!(
            s.eval::<i64>(r#"return GetSelectedAuctionItem("list")"#)
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_missing_row_answers_thirteen_values() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity(r#"GetAuctionItemInfo("list", 99)"#).unwrap(), 13);
        let (count, quality): (i64, i64) = s
            .eval(r#"local _, _, c, q = GetAuctionItemInfo("list", 99) return c, q"#)
            .unwrap();
        assert_eq!((count, quality), (1, -1));
        assert_eq!(s.arity(r#"GetAuctionItemLink("list", 99)"#).unwrap(), 0);
        assert_eq!(s.arity("GetAuctionSellItemInfo()").unwrap(), 9);
    }

    #[test]
    fn the_deposit_is_the_clients_arithmetic() {
        // Under 100 copper answers the minimum.
        assert_eq!(deposit_for(5, 65, 720), 100);
        // 15% of 10000 is 1500 per unit, times 3 / 6 / 12 for 12 / 24 / 48 hours.
        assert_eq!(deposit_for(15, 10_000, 720), 4_500);
        assert_eq!(deposit_for(15, 10_000, 1440), 9_000);
        assert_eq!(deposit_for(15, 10_000, 2880), 18_000);
        // The integer division floors before the run time scales it.
        assert_eq!(deposit_for(15, 1_999, 2880), 3_588);
        assert_eq!(deposit_for(15, 10_000, 60), 100, "an unknown run time");
        assert_eq!(run_time(1.0), 720);
        assert_eq!(run_time(3.0), 2880);
        assert_eq!(run_time(7.0), 1440);
    }

    /// Weapon, Armor (Misc, Cloth, Shield) and Quest (no subclass rows).
    fn tree() -> Vec<AuctionCategory> {
        let sub = |sub_id: u32, name: &str, has_inv_types: bool| AuctionSubCategory {
            sub_id,
            name: name.into(),
            has_inv_types,
        };
        vec![
            AuctionCategory {
                class_id: 2,
                name: "Weapon".into(),
                has_subclass_filter: true,
                subclasses: vec![sub(0, "Axe", false), sub(1, "Axe2", false)],
            },
            AuctionCategory {
                class_id: 4,
                name: "Armor".into(),
                has_subclass_filter: true,
                subclasses: vec![
                    sub(0, "Miscellaneous", true),
                    sub(1, "Cloth", true),
                    sub(6, "Shield", false),
                ],
            },
            AuctionCategory {
                class_id: 12,
                name: "Quest".into(),
                has_subclass_filter: false,
                subclasses: vec![sub(0, "Quest", false)],
            },
        ]
    }

    fn open(s: &mut UiScript) {
        s.set_auction(Some(AuctionState::default()));
        s.set_auction_can_query([true; 3], true);
    }

    /// The query a fixture's verbs sent, if one.
    fn query(s: &mut UiScript) -> Option<AuctionQuery> {
        s.take_auction_sent().into_iter().find_map(|p| match p {
            AuctionPacket::Query(q) => Some(q),
            _ => None,
        })
    }

    #[test]
    fn query_sends_ids_and_the_browse_criteria() {
        let mut s = UiScript::new().unwrap();
        s.set_auction_item_classes(tree());
        open(&mut s);
        s.run(r#"QueryAuctionItems("lin", "10", "", 13, 1, 1, 2, 1, -1)"#)
            .unwrap();
        let q = query(&mut s).expect("sent in the call");
        assert_eq!(q.inv_type, 16, "INVTYPE_CLOAK is inventory type 16");
        assert_eq!((q.class, q.sub_class), (2, 0));
        assert_eq!((q.min_level, q.max_level), (10, 0));
        assert_eq!(q.list_from, 100);
        assert_eq!(q.usable, 1);
        assert_eq!(q.quality, u32::MAX, "-1 rides as any");
        assert!(!q.get_all);

        // The gate closed with the query; nothing more goes out until it reopens.
        s.run(r#"QueryAuctionItems("", "", "", nil, nil, nil, 0, nil, nil)"#)
            .unwrap();
        assert!(query(&mut s).is_none());
        let can: Value = s.eval("return CanSendAuctionQuery()").unwrap();
        assert_eq!(can, Value::Nil);

        // Armor's third subclass skips the id gap; a class out of range is any.
        s.set_auction_can_query([true; 3], true);
        s.run(r#"QueryAuctionItems("", "", "", 14, 2, 3, 0, nil, 3, true)"#)
            .unwrap();
        let q = query(&mut s).expect("sent in the call");
        assert_eq!((q.inv_type, q.class, q.sub_class, q.quality), (23, 4, 6, 3));
        assert!(q.get_all);
        s.set_auction_can_query([true; 3], true);
        s.run(r#"QueryAuctionItems("", "", "", 15, 13, 1, 0)"#)
            .unwrap();
        let q = query(&mut s).expect("sent in the call");
        assert_eq!(
            (q.inv_type, q.class, q.sub_class),
            (u32::MAX, u32::MAX, u32::MAX)
        );
    }

    #[test]
    fn inv_types_answer_token_pairs_and_the_armor_gates() {
        let mut s = UiScript::new().unwrap();
        s.set_auction_item_classes(tree());
        assert_eq!(s.arity("GetAuctionItemSubClasses(1)").unwrap(), 2);
        assert_eq!(s.arity("GetAuctionItemSubClasses(3)").unwrap(), 0);
        assert_eq!(s.arity("GetAuctionInvTypes(2, 1)").unwrap(), 28);
        assert_eq!(s.arity("GetAuctionInvTypes(2, 3)").unwrap(), 0, "Shield");
        assert_eq!(s.arity("GetAuctionInvTypes(3, 1)").unwrap(), 0, "no filter");
        assert_eq!(
            s.arity("GetAuctionInvTypes(1, 1)").unwrap(),
            0,
            "Axe lacks 0x200"
        );
        assert_eq!(
            s.arity("GetAuctionInvTypes(1, 9)").unwrap(),
            28,
            "past the scan"
        );
        let shown = |c: u32, sub: u32| -> Vec<String> {
            s.eval::<Vec<String>>(&format!(
                r#"local t, v = {{}}, {{GetAuctionInvTypes({c}, {sub})}}
                local n = select('#', GetAuctionInvTypes({c}, {sub}))
                for i = 1, n, 2 do if v[i + 1] then t[#t + 1] = v[i] end end
                return t"#
            ))
            .unwrap()
        };
        let misc = shown(2, 1);
        assert!(misc.contains(&"INVTYPE_HEAD".to_string()));
        assert!(!misc.contains(&"INVTYPE_CHEST".to_string()));
        let cloth = shown(2, 2);
        assert!(
            cloth.contains(&"INVTYPE_CLOAK".to_string()),
            "Cloth keeps the cloak"
        );
        assert!(cloth.contains(&"INVTYPE_CHEST".to_string()));
        assert!(!cloth.contains(&"INVTYPE_FINGER".to_string()));
        assert_eq!(shown(1, 9).len(), 14, "a weapon index shows every slot");
    }

    #[test]
    fn a_bid_is_checked_before_it_sends() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[5]);
        state.lists[LIST].rows[0].bid_amount = 1_000;
        state.lists[LIST].rows[0].min_increment = 50;
        state.lists[LIST].rows[0].buyout_price = 5_000;
        s.set_auction(Some(state));
        s.model_mut().money = 10_000;
        s.run(r#"PlaceAuctionBid("list", 1, 1020)"#).unwrap();
        assert_eq!(s.take_shown(), vec!["ERR_AUCTION_BID_INCREMENT"]);
        s.run(r#"PlaceAuctionBid("list", 1, 900)"#).unwrap();
        assert_eq!(s.take_shown(), vec!["ERR_AUCTION_HIGHER_BID"]);
        assert!(s.take_auction_sent().is_empty());
        // Past the buyout is the buyout.
        s.run(r#"PlaceAuctionBid("list", 1, 9000)"#).unwrap();
        assert_eq!(
            s.take_auction_sent(),
            vec![AuctionPacket::Bid {
                auction_id: 5,
                amount: 5_000
            }]
        );
        s.model_mut().money = 10;
        s.run(r#"PlaceAuctionBid("list", 1, 1100)"#).unwrap();
        assert_eq!(s.take_shown(), vec!["ERR_NOT_ENOUGH_MONEY"]);
    }

    /// `0x59dc7c`: past the purse test, the carried-count check on the row's item and count refuses
    /// with its line and sends nothing; a passing check bids.
    #[test]
    fn a_bid_past_the_carried_count_is_refused_before_it_sends() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[5]);
        state.lists[LIST].rows[0].count = 3;
        s.set_auction(Some(state));
        s.model_mut().money = 10_000;
        s.lua()
            .globals()
            .set(
                "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
                "You can only carry %d %s",
            )
            .unwrap();
        s.run(
            "Log = {} local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE')              f:SetScript('OnEvent', function(_, _, t) Log[#Log + 1] = t end)",
        )
        .unwrap();
        let mut view = crate::script::live::TestView {
            carry: Some(CarryRefusal::MaxCount),
            ..Default::default()
        };
        crate::script::live::lending(&mut view, || {
            s.run(r#"PlaceAuctionBid("list", 1, 600)"#).unwrap();
        });
        assert_eq!(s.take_shown(), vec!["ERR_ITEM_MAX_COUNT"]);
        assert!(
            s.take_auction_sent().is_empty(),
            "a refused bid sends nothing"
        );
        view.carry = Some(CarryRefusal::Category {
            quantity: 2,
            name: "Healthstone".into(),
        });
        crate::script::live::lending(&mut view, || {
            s.run(r#"PlaceAuctionBid("list", 1, 600)"#).unwrap();
        });
        assert_eq!(
            s.eval::<Vec<String>>("return Log").unwrap().last().unwrap(),
            "You can only carry 2 Healthstone"
        );
        assert!(s.take_auction_sent().is_empty());
        // The purse refuses first: the count is not asked (`0x59dc2d` before `0x59dc7c`).
        s.model_mut().money = 10;
        crate::script::live::lending(&mut view, || {
            s.run(r#"PlaceAuctionBid("list", 1, 600)"#).unwrap();
        });
        assert_eq!(s.take_shown(), vec!["ERR_NOT_ENOUGH_MONEY"]);
        assert_eq!(
            view.carry_asks.take(),
            vec![(1005, 3), (1005, 3)],
            "asked on the row's item and count"
        );
        s.model_mut().money = 10_000;
        view.carry = None;
        crate::script::live::lending(&mut view, || {
            s.run(r#"PlaceAuctionBid("list", 1, 600)"#).unwrap();
        });
        assert_eq!(
            s.take_auction_sent(),
            vec![AuctionPacket::Bid {
                auction_id: 5,
                amount: 600
            }]
        );
    }

    #[test]
    fn the_time_left_buckets_the_time_to_the_expiry() {
        let mut s = UiScript::new().unwrap();
        let mut state = page(&[1, 2, 3]);
        state.lists[LIST].rows[0].time_left = 60.0 * 60.0; // an hour
        state.lists[LIST].rows[1].time_left = 20.0 * 3600.0; // past 12 h: 4
        state.lists[LIST].rows[2].time_left = 0.0; // already gone
        s.run(r#"SortAuctionClearSort("list")"#).unwrap();
        s.set_auction(Some(state));
        let left = |i: u32| {
            s.eval::<i64>(&format!(r#"return GetAuctionItemTimeLeft("list", {i})"#))
                .unwrap()
        };
        assert_eq!((left(1), left(2), left(3)), (2, 4, 1));
        assert_eq!(left(9), 0);
    }
}
