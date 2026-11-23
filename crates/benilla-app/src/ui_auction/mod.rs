//! The auction house's app side, after 12340's: [`AuctionOpen`] is the house's state (the
//! auctioneer, the three lists and their sort criteria, the query gates and the multi-sell), [`net`]'s
//! handlers fill it and raise each event at their step, and the verbs read and write it inside their
//! call through [`AuctionLive`].
//!
//! The hello reply opens the window, not the click (the auctioneer set, `0x59f000`, fires
//! `AUCTION_HOUSE_SHOW`). Browse, Bids and Auctions are independent lists, each with its own
//! query gate (`0x59a2f0`). Walking out of range closes the window with no packet.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::{AuctionListEntry, AuctionListQuery};
use benilla_ui::script::{
    AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionMultiSell, AuctionPacket,
    AuctionSellStep, AuctionSubCategory, UiScript, BIDDER, LIST, OWNER,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;

mod net;

pub(crate) use net::{record_callback, Callback};

/// The query delay until a list result says otherwise (`[0xacf628]`, 5000 ms).
const DEFAULT_QUERY_DELAY_SECS: f64 = 5.0;

/// The get-all query's own cooldown, 15 minutes (`0x59bcf0`, `0x59c740`).
const GET_ALL_COOLDOWN_SECS: f64 = 900.0;

/// A bid holds the auction open at least this long (`0x59ffb0`, `0x5a0480`).
const BID_EXTENSION_SECS: f64 = 90.0;

/// `ItemSubClass.dbc` `DisplayFlags` bit 1: left out of the auction category filter (`0x59c480`).
const SUBCLASS_HIDDEN_FROM_AUCTIONS: u32 = 0x2;

/// `ItemSubClass.dbc` `Flags` bit `0x200`: the subclass offers the inventory-slot rows.
const SUBCLASS_OFFERS_INV_TYPES: u32 = 0x200;

/// The class table at `0xa14f70`, `{classId, hasSubClasses}` in menu order.
const AUCTION_CLASSES: [(u32, bool); 12] = [
    (2, true),   // Weapon
    (4, true),   // Armor
    (1, true),   // Container
    (0, true),   // Consumable
    (16, true),  // Glyph
    (7, true),   // Trade Goods
    (6, true),   // Projectile
    (11, true),  // Quiver
    (9, true),   // Recipe
    (3, true),   // Gem
    (15, true),  // Miscellaneous
    (12, false), // Quest
];

/// The emote a placed bid plays (`0x5a01d5`).
const BID_EMOTE: u32 = 3;

/// The `AuctionHouse.dbc` catalog; without it the sell pane shows no deposit.
#[derive(Resource)]
pub(crate) struct AuctionHouses(pub(crate) benilla_formats::AuctionHouseCatalog);

/// One row of a list as the reference keeps it: the wire record, its expiry and whether it is a
/// sold auction from the pending sales.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AuctionEntry {
    pub(crate) wire: AuctionListEntry,
    /// The expiry on `Time::elapsed_secs_f64`: arrival plus the time left (`0x59e22e`).
    expires_at: f64,
    pub(crate) sold: bool,
}

/// One of the three lists.
#[derive(Debug, Default, Clone)]
pub(crate) struct AuctionListSlot {
    pub(crate) entries: Vec<AuctionEntry>,
    /// The match count before the page cap (`[0xc0f408]`, `[0xc0f410]`; `[0xc0f40c]` the owner's).
    pub(crate) total: u32,
}

/// A multi-sell in progress (`0xc0e740`-`0xc0e768`): the items it draws from, consumed from the
/// end, and the stacks still to list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MultiSell {
    auctioneer: u64,
    /// `(guid, count)` left to draw from.
    items: Vec<(u64, u32)>,
    stack_size: u32,
    pub(crate) remaining: u32,
    pub(crate) total: u32,
    bid: u32,
    buyout: u32,
    minutes: u32,
    /// More than one stack: each `STARTED` answer lists the next (`[0xc0f434]`).
    pending: bool,
}

/// What the live probe reads to tell an empty answer from none; the client never reads it.
#[derive(Default, Clone)]
pub(crate) struct AuctionWireLog {
    /// List results landed, per list, in [`LIST`]/[`BIDDER`]/[`OWNER`] order.
    pub(crate) list_results: [u32; 3],
    /// Browse queries that went out.
    pub(crate) browse_sent: u32,
    /// The latest `SMSG_AUCTION_COMMAND_RESULT` as `(auction_id, action, error)`.
    pub(crate) last_command: Option<(u32, u32, u32)>,
    /// Every command result, in arrival order.
    pub(crate) results: Vec<(u32, u32, u32)>,
    /// The latest `CMSG_AUCTION_LIST_ITEMS` `(slot, class, subclass)` filters.
    pub(crate) last_filter: Option<(u32, u32, u32)>,
    /// The latest `CMSG_AUCTION_LIST_ITEMS` sort criteria.
    pub(crate) last_sort: Vec<(u8, bool)>,
    /// Every `CMSG_AUCTION_SELL_ITEM` sent: its `(count per item)` list.
    pub(crate) sells: Vec<Vec<u32>>,
}

/// Each list's sort criteria, front first: set at each game start (`0x52b832` -> `0x5a0ba0`), kept
/// across the houses of one game.
#[derive(Clone)]
pub(crate) struct Criteria(pub(crate) [Vec<(u8, bool)>; 3]);

impl Default for Criteria {
    fn default() -> Self {
        Self(benilla_ui::script::auction_default_criteria())
    }
}

/// The auction house (`0xc0e728`-`0xc0f470`); the auctioneer cleared on a client-side close, on
/// walking away and on disconnect.
#[derive(Resource, Default, Clone)]
pub(crate) struct AuctionOpen {
    /// `[0xc0f3e8]`.
    pub(crate) auctioneer: Option<u64>,
    /// The `AuctionHouse.dbc` row, which keys the deposit rate (`[0xc0f3f0]`).
    pub(crate) house_id: u32,
    /// Browse, Bids and Auctions, sorted in place as the reference sorts them.
    pub(crate) lists: [AuctionListSlot; 3],
    pub(crate) criteria: Criteria,
    /// The item records each list's rows wait on (`[0xc0f424]`, `[0xc0f430]`, `[0xc0f428]`), and
    /// the owner list's sold rows' (`[0xc0f42c]`).
    pending_items: [u32; 3],
    pending_sold: u32,
    /// The names a row read left its list's callback on, `(guid, list)` (`0x59d5e0`'s once asks).
    name_waits: Vec<(u64, usize)>,
    /// The last query's time per list, on `Time::elapsed_secs_f64` (`[0xc0e728]`).
    last_query: [Option<f64>; 3],
    /// Sends not yet answered (`[0xc0e734]`).
    pending: [i32; 3],
    /// The query delay, seconds (`[0xacf628]`).
    delay_secs: f64,
    /// `[0xc0f418]`.
    last_get_all: Option<f64>,
    /// Whether the owner list and the bidder list may be asked for (`[0xc0f41c]`, `[0xc0f420]`).
    can_get_owner: bool,
    can_get_bidder: bool,
    /// `[0xc0f438]`.
    last_bidder_page: u32,
    /// The bid in flight, which a `BID_PLACED` result writes into the browse row (`[0xc0f43c]`).
    last_bid_amount: u32,
    /// The outbid auctions, `(house, auction)`: kept across the houses of one game, sent with each
    /// bidder query.
    pub(crate) outbid: Vec<(u32, u32)>,
    /// `[0xc0f434]` with its state.
    pub(crate) multisell: Option<MultiSell>,
    pub(crate) wire: AuctionWireLog,
}

impl AuctionOpen {
    /// The auctioneer set's state (`0x59f000`): the gates open and the lists empty.
    fn open(&mut self, auctioneer: u64, house_id: u32) {
        self.auctioneer = Some(auctioneer);
        self.house_id = house_id;
        self.last_query = [None; 3];
        self.pending = [0; 3];
        self.can_get_owner = true;
        self.can_get_bidder = true;
        self.delay_secs = DEFAULT_QUERY_DELAY_SECS;
        for slot in &mut self.lists {
            *slot = AuctionListSlot::default();
        }
        self.last_bidder_page = 0;
    }

    /// The game start (`0x5a0ba0`): the criteria at their defaults, no outbid ids, no get-all yet.
    fn initialize_game(&mut self) {
        self.criteria = Criteria::default();
        self.outbid.clear();
        self.last_get_all = None;
        self.last_query = [None; 3];
        self.pending = [0; 3];
    }

    /// The world entry (`0x59dfb0`): no auctioneer, the gates and pending counts zeroed, the
    /// browse list and every total empty, the query delay back at its default.
    fn enter_world(&mut self) {
        self.auctioneer = None;
        self.house_id = 0;
        self.last_query = [None; 3];
        self.pending = [0; 3];
        self.lists[LIST].entries.clear();
        for slot in &mut self.lists {
            slot.total = 0;
        }
        self.can_get_owner = false;
        self.can_get_bidder = false;
        self.pending_items = [0; 3];
        self.pending_sold = 0;
        self.delay_secs = DEFAULT_QUERY_DELAY_SECS;
    }

    /// The close's state (`0x59f198`-`0x59f22b`): no auctioneer, no owner or bidder asks, the lists
    /// empty.
    fn end_session(&mut self) {
        self.auctioneer = None;
        self.house_id = 0;
        self.can_get_owner = false;
        self.can_get_bidder = false;
        for slot in &mut self.lists {
            *slot = AuctionListSlot::default();
        }
    }

    /// The query gate (`0x59a2f0`): nothing in flight, and the delay past the last send or answer.
    pub(crate) fn can_send(&self, list: usize, now: f64) -> bool {
        self.pending[list] <= 0 && self.last_query[list].is_none_or(|t| now - t >= self.delay_secs)
    }

    /// `CanSendAuctionQuery`'s second answer (`0x59c7ce`): the get-all cooldown past, too.
    fn can_get_all(&self, list: usize, now: f64) -> bool {
        self.can_send(list, now)
            && self
                .last_get_all
                .is_none_or(|t| now - t >= GET_ALL_COOLDOWN_SECS)
    }

    /// A query of `list` went out.
    fn sent(&mut self, list: usize, now: f64) {
        self.pending[list] = (self.pending[list] + 1).max(0);
        self.last_query[list] = Some(now);
    }

    /// A result of `list` landed: one query answered, and its delay paces the next.
    pub(crate) fn answered(&mut self, list: usize, now: f64, delay_ms: Option<u32>) {
        self.pending[list] = (self.pending[list] - 1).max(0);
        self.last_query[list] = Some(now);
        if let Some(ms) = delay_ms {
            self.delay_secs = f64::from(ms) / 1000.0;
        }
    }

    /// Whether a multi-sell is pending (`[0xc0f434]`).
    pub(crate) fn pending_sell(&self) -> bool {
        self.multisell.as_ref().is_some_and(|m| m.pending)
    }

    /// Whether the owner list's sort and update are owed: no record and no reply outstanding
    /// (`0x59e86a`).
    fn owner_settled(&self) -> bool {
        self.pending_items[OWNER] == 0 && self.pending_sold == 0 && self.pending[OWNER] <= 0
    }
}

impl NpcSession for AuctionOpen {
    fn npc(&self) -> Option<u64> {
        self.auctioneer
    }

    fn close(&mut self) {
        self.end_session();
    }
}

/// The update event each list fires once sorted (`0x59b980`, `0x59b9e0`, `0x59cab0`). Literal
/// names: `reference_ui`'s producer test finds fire sites by literal.
pub(crate) fn list_event(list: usize) -> &'static str {
    match list {
        LIST => "AUCTION_ITEM_LIST_UPDATE",
        BIDDER => "AUCTION_BIDDER_LIST_UPDATE",
        _ => "AUCTION_OWNED_LIST_UPDATE",
    }
}

/// The next-item sell's draw (`0x59ba80`): one stack out of the end of the gathered items,
/// `(guid, count)` per item; `None` once every stack is out, an error on running dry.
fn next_stack(ms: &mut MultiSell) -> Result<Option<Vec<(u64, u32)>>, ()> {
    if ms.remaining == 0 {
        return Ok(None);
    }
    ms.remaining -= 1;
    let mut rest = ms.stack_size;
    let Some(mut last) = ms.items.len().checked_sub(1) else {
        return Err(());
    };
    while rest > ms.items[last].1 {
        rest -= ms.items[last].1;
        last = last.checked_sub(1).ok_or(())?;
    }
    let mut send: Vec<(u64, u32)> = ms.items[last + 1..].iter().rev().copied().collect();
    send.push((ms.items[last].0, rest));
    let emptied = ms.items[last].1 == rest;
    ms.items[last].1 -= rest;
    ms.items.truncate(if emptied { last } else { last + 1 });
    Ok(Some(send))
}

/// What a row resolves through and what the house's sends go out on.
#[derive(SystemParam)]
pub(crate) struct AuctionLookups<'w, 's> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    time: Res<'w, Time>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    houses: Option<Res<'w, AuctionHouses>>,
    self_q: Query<'w, 's, &'static crate::net::Guid, With<SelfPlayer>>,
}

impl AuctionLookups<'_, '_> {
    fn now(&self) -> f64 {
        self.time.elapsed_secs_f64()
    }

    fn player(&self) -> Option<u64> {
        self.self_q.iter().next().map(|g| g.0)
    }

    fn rolls(&self) -> crate::items::RollCatalogs<'_> {
        crate::items::RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        }
    }

    fn send(&self, command: ClientCommand) {
        debug!("ui_auction: sending {command:?}");
        let _ = self.commands.0.send(command);
    }

    /// Whether `entry`'s record is cached; a miss asks once.
    fn record(&self, entry: u32) -> bool {
        self.items.template(entry, 0, &self.commands).is_some()
    }

    /// An item's name with its random property's suffix (`0x706d70`), `None` until its record lands.
    fn item_name(&self, entry: u32, property: i32) -> Option<String> {
        self.items
            .template(entry, 0, &self.commands)
            .map(|t| self.rolls().name(&t.name, property as u32))
    }

    /// `GetAuctionItemInfo`'s `highBidder` for a row of `list` (`0x59d5e0`): on an `"owner"` row
    /// the player owns, the bidder's name through the name cache (nil with no bid or until it lands);
    /// any other row, `1` if the player holds the bid, else nil.
    fn high_bidder(&self, list: usize, entry: &AuctionEntry, owner: u64) -> AuctionHighBidder {
        let me = self.player();
        if list == OWNER && me == Some(owner) {
            if entry.wire.bidder_guid == 0 {
                return AuctionHighBidder::Nil;
            }
            return self
                .names
                .resolve(entry.wire.bidder_guid, &self.commands)
                .map_or(AuctionHighBidder::Nil, |n| {
                    AuctionHighBidder::Name(n.to_string())
                });
        }
        if me.is_some_and(|g| g == entry.wire.bidder_guid) {
            AuctionHighBidder::Player
        } else {
            AuctionHighBidder::Nil
        }
    }

    /// One entry of `list` as a VM row, read now through the ask-once template and name caches.
    fn resolve(&self, list: usize, entry: &AuctionEntry) -> AuctionItemRow {
        let w = &entry.wire;
        let template = self.items.template(w.item_entry, 0, &self.commands);
        let roll = w.random_property_id as u32;
        let name = template.map(|t| self.rolls().name(&t.name, roll));
        let quality = template.map(|t| t.quality);
        let level = template.map_or(0, |t| t.required_level);
        let texture = crate::ui_items::item_icon(
            self.icons.as_deref(),
            template.map_or(0, |t| t.display_info_id),
        );
        let link = name.as_ref().map(|n| {
            crate::ui_items::item_link_full(
                w.item_entry,
                &benilla_ui::script::ItemLinkFields {
                    enchant: w.enchants[0].id as i32,
                    gems: std::array::from_fn(|i| w.enchants[2 + i].id as i32),
                    ..benilla_ui::script::ItemLinkFields::roll(
                        w.random_property_id,
                        w.suffix_factor as i32,
                    )
                },
                n,
                quality.unwrap_or(0),
            )
        });
        // A sold row's owner is the player (`0x59eb78`).
        let owner_guid = if entry.sold {
            self.player().unwrap_or(0)
        } else {
            w.owner_guid
        };
        let bidder_status = if w.bidder_guid == 0 {
            0
        } else if self.player() == Some(w.bidder_guid) {
            2
        } else {
            1
        };
        AuctionItemRow {
            auction_id: w.auction_id,
            item_id: w.item_entry,
            random_property_id: roll,
            name,
            base_name: template.map(|t| t.name.clone()),
            texture,
            count: w.count,
            quality,
            level,
            min_bid: w.start_bid,
            min_increment: w.min_increment,
            buyout_price: w.buyout,
            bid_amount: w.current_bid,
            high_bidder: self.high_bidder(list, entry, owner_guid),
            owner: self
                .names
                .resolve(owner_guid, &self.commands)
                .map(str::to_string),
            owner_guid,
            bidder_status,
            time_left: entry.expires_at - self.now(),
            sold: entry.sold,
            link,
        }
    }
}

/// The house as the verbs and handlers read and write it.
#[derive(SystemParam)]
pub(crate) struct AuctionLive<'w, 's> {
    pub(crate) auction: ResMut<'w, AuctionOpen>,
    look: AuctionLookups<'w, 's>,
}

impl AuctionLive<'_, '_> {
    pub(crate) fn auctioneer(&self) -> Option<u64> {
        self.auction.auctioneer
    }

    /// One list's rows and its total.
    pub(crate) fn counts(&self, list: usize) -> (usize, u32) {
        let slot = &self.auction.lists[list];
        (slot.entries.len(), slot.total)
    }

    /// Row `index` (0-based) of `list` resolved now; `info` is `GetAuctionItemInfo`'s read, whose
    /// name misses leave the list's callback once (`0x59d8dd`, `0x59d946`).
    pub(crate) fn row(&mut self, list: usize, index: usize, info: bool) -> Option<AuctionItemRow> {
        let entry = self.auction.lists[list].entries.get(index)?;
        let row = self.look.resolve(list, entry);
        if info && row.name.is_some() {
            let mut misses = Vec::new();
            if row.owner_guid != 0 && row.owner.is_none() {
                misses.push(row.owner_guid);
            }
            let owned = list == OWNER && self.look.player() == Some(row.owner_guid);
            let bidder = entry.wire.bidder_guid;
            if owned && bidder != 0 && row.high_bidder == AuctionHighBidder::Nil {
                misses.push(bidder);
            }
            for guid in misses {
                if !self.auction.name_waits.contains(&(guid, list)) {
                    self.auction.name_waits.push((guid, list));
                }
            }
        }
        Some(row)
    }

    /// The 0-based index of `auction_id` on `list` (`0x59b3a0`).
    pub(crate) fn find(&self, list: usize, auction_id: u32) -> Option<usize> {
        self.auction.lists[list]
            .entries
            .iter()
            .position(|e| e.wire.auction_id == auction_id)
    }

    /// The deposit rate of the house's `AuctionHouse.dbc` row (`0x59bc90`), `None` with no row.
    pub(crate) fn deposit_rate(&self) -> Option<u32> {
        self.look
            .houses
            .as_deref()
            .and_then(|h| h.0.deposit_percent(self.auction.house_id))
    }

    /// The query gate (`0x59a2f0`) and the get-all one (`0x59c7ce`).
    pub(crate) fn can_send(&self, list: usize) -> (bool, bool) {
        let now = self.look.now();
        (
            self.auction.can_send(list, now),
            self.auction.can_get_all(list, now),
        )
    }

    /// Re-sort `list` in place by its criteria (`0x59b980`, `0x59b9e0`, `0x59cab0`); the caller
    /// raises its update.
    pub(crate) fn sort(&mut self, list: usize) {
        let rows: Vec<AuctionItemRow> = self.auction.lists[list]
            .entries
            .iter()
            .map(|e| self.look.resolve(list, e))
            .collect();
        let order = benilla_ui::script::auction_sort_order(&rows, &self.auction.criteria.0[list]);
        let entries = std::mem::take(&mut self.auction.lists[list].entries);
        let mut slots: Vec<Option<AuctionEntry>> = entries.into_iter().map(Some).collect();
        self.auction.lists[list].entries =
            order.into_iter().filter_map(|i| slots[i].take()).collect();
    }

    /// Send `packet` inside the verb's call, each behind the gate its send reads; nothing with no
    /// auctioneer.
    pub(crate) fn send(&mut self, packet: AuctionPacket) {
        let Some(auctioneer) = self.auction.auctioneer else {
            return;
        };
        let now = self.look.now();
        match packet {
            // `0x59bcf0`: the gate, then the get-all's own, then the send with the browse list's
            // criteria; one more in flight.
            AuctionPacket::Query(q) => {
                let (can, get_all) = self.can_send(LIST);
                if !can || (q.get_all && !get_all) {
                    return;
                }
                let sort = self.auction.criteria.0[LIST].clone();
                self.auction.wire.browse_sent += 1;
                self.auction.wire.last_sort = sort.clone();
                self.auction.wire.last_filter = Some((q.inv_type, q.class, q.sub_class));
                self.look.send(ClientCommand::AuctionListItems {
                    auctioneer,
                    query: AuctionListQuery {
                        list_from: q.list_from,
                        searched_name: q.name,
                        level_min: q.min_level,
                        level_max: q.max_level,
                        slot_id: q.inv_type,
                        main_category: q.class,
                        sub_category: q.sub_class,
                        quality: q.quality,
                        usable: q.usable,
                        get_all: q.get_all,
                        sort,
                    },
                });
                self.auction.sent(LIST, now);
                if q.get_all {
                    self.auction.last_get_all = Some(now);
                }
            }
            AuctionPacket::OwnerList => owner_list(&mut self.auction, &self.look),
            // `0x59d540`: the bidder gate and the query gate, then the ask.
            AuctionPacket::BidderList(list_from) => {
                if self.auction.can_get_bidder && self.auction.can_send(BIDDER, now) {
                    bidder_list(&mut self.auction, &self.look, list_from);
                }
            }
            // `0x59dcca`: the bid, remembered for its result (`0x59dcd5`).
            AuctionPacket::Bid { auction_id, amount } => {
                self.look.send(ClientCommand::AuctionPlaceBid {
                    auctioneer,
                    auction_id,
                    price: amount,
                });
                self.auction.last_bid_amount = amount;
            }
            AuctionPacket::Cancel { auction_id } => {
                self.look.send(ClientCommand::AuctionRemoveItem {
                    auctioneer,
                    auction_id,
                });
            }
            // `0x59fc66`: the sell item alone, as is.
            AuctionPacket::Sell {
                item,
                count,
                bid,
                buyout,
                minutes,
            } => {
                self.auction.wire.sells.push(vec![count]);
                self.look.send(ClientCommand::AuctionSellItem {
                    auctioneer,
                    items: vec![(item, count)],
                    bid,
                    buyout,
                    etime_minutes: minutes,
                });
            }
        }
    }

    /// `StartAuction`'s multi-sell state (`0x59fc92`-`0x59fcf3`), `pending` (`[0xc0f434]`) with more
    /// than one stack.
    pub(crate) fn multisell_start(&mut self, sell: AuctionMultiSell) {
        let Some(auctioneer) = self.auction.auctioneer else {
            return;
        };
        self.auction.multisell = Some(MultiSell {
            auctioneer,
            items: sell.items,
            stack_size: sell.stack_size,
            remaining: sell.num_stacks,
            total: sell.num_stacks,
            bid: sell.bid,
            buyout: sell.buyout,
            minutes: sell.minutes,
            pending: sell.num_stacks > 1,
        });
    }

    /// The next-item sell (`0x59ba80`).
    pub(crate) fn sell_next(&mut self) -> AuctionSellStep {
        sell_next(&mut self.auction, &self.look)
    }

    /// Whether a multi-sell is pending; ending it (`0x59ab26`) when `end`.
    pub(crate) fn pending_sell(&mut self, end: bool) -> bool {
        let pending = self.auction.pending_sell();
        if pending && end {
            self.auction.multisell = None;
        }
        pending
    }

    /// The close's state (`0x59f198`), the target's clear and window leg the caller's.
    pub(crate) fn end_session(&mut self) {
        self.auction.end_session();
    }

    /// The auction expiry (`0x59f2f0`): the auction leaves each list; leaving the owner list asks
    /// the pending sales (`0x59cd90`). The events it raises, in order.
    pub(crate) fn expired(&mut self, auction_id: u32) -> Vec<&'static str> {
        expire(&mut self.auction, &self.look, auction_id)
    }

    /// One list's criteria, front first.
    pub(crate) fn criteria(&self, list: usize) -> Vec<(u8, bool)> {
        self.auction.criteria.0[list].clone()
    }

    pub(crate) fn set_criteria(&mut self, list: usize, criteria: Vec<(u8, bool)>) {
        self.auction.criteria.0[list] = criteria;
    }
}

/// The owner-list ask (`0x59cc50`): the owner gate and the query gate, then the owner list and the
/// pending sales, two answers owed.
fn owner_list(auction: &mut AuctionOpen, look: &AuctionLookups) {
    let now = look.now();
    let Some(auctioneer) = auction.auctioneer else {
        return;
    };
    if !auction.can_get_owner || !auction.can_send(OWNER, now) {
        return;
    }
    look.send(ClientCommand::AuctionListOwnerItems {
        auctioneer,
        list_from: 0,
    });
    look.send(ClientCommand::AuctionListPendingSales { auctioneer });
    auction.pending[OWNER] = (auction.pending[OWNER] + 2).max(0);
    auction.last_query[OWNER] = Some(now);
    auction.can_get_owner = false;
}

/// The bidder-list ask (`0x59ce50`) with this house's outbid ids, behind the bidder gate alone.
fn bidder_list(auction: &mut AuctionOpen, look: &AuctionLookups, list_from: u32) {
    if !auction.can_get_bidder {
        return;
    }
    let house = auction.house_id;
    let auction_ids = auction
        .outbid
        .iter()
        .filter(|&&(h, _)| h == house)
        .map(|&(_, id)| id)
        .collect();
    look.send(ClientCommand::AuctionListBidderItems {
        auctioneer: auction.auctioneer.unwrap_or(0),
        list_from,
        auction_ids,
    });
    auction.sent(BIDDER, look.now());
    auction.can_get_bidder = false;
    auction.last_bidder_page = list_from;
}

/// The next-item sell (`0x59ba80`): the last stack out ends the multi-sell, running dry fails it,
/// otherwise one stack goes out.
fn sell_next(auction: &mut AuctionOpen, look: &AuctionLookups) -> AuctionSellStep {
    let Some(ms) = auction.multisell.as_mut() else {
        return AuctionSellStep::Done;
    };
    match next_stack(ms) {
        Ok(Some(items)) => {
            let (auctioneer, bid, buyout, minutes) = (ms.auctioneer, ms.bid, ms.buyout, ms.minutes);
            // A single stack is one send; nothing waits on its answer.
            if !ms.pending {
                auction.multisell = None;
            }
            auction.wire.sells.push(items.iter().map(|i| i.1).collect());
            look.send(ClientCommand::AuctionSellItem {
                auctioneer,
                items,
                bid,
                buyout,
                etime_minutes: minutes,
            });
            AuctionSellStep::Sent
        }
        Ok(None) => {
            auction.multisell = None;
            AuctionSellStep::Done
        }
        Err(()) => {
            auction.multisell = None;
            AuctionSellStep::Failed
        }
    }
}

/// The auction expiry (`0x59f2f0`) over each list, the events it raises returned in order.
fn expire(auction: &mut AuctionOpen, look: &AuctionLookups, auction_id: u32) -> Vec<&'static str> {
    let mut events = Vec::new();
    for list in [LIST, OWNER, BIDDER] {
        let slot = &mut auction.lists[list];
        let Some(pos) = slot
            .entries
            .iter()
            .position(|e| e.wire.auction_id == auction_id)
        else {
            continue;
        };
        slot.entries.remove(pos);
        slot.total = slot.total.saturating_sub(1);
        match list {
            LIST if auction.pending_items[LIST] == 0 => events.push(list_event(LIST)),
            BIDDER if auction.pending_items[BIDDER] == 0 => events.push(list_event(BIDDER)),
            OWNER => pending_sales_ask(auction, look),
            _ => {}
        }
    }
    events
}

/// The pending-sales ask (`0x59cd90`), behind the owner query gate.
fn pending_sales_ask(auction: &mut AuctionOpen, look: &AuctionLookups) {
    let now = look.now();
    if !auction.can_send(OWNER, now) {
        return;
    }
    look.send(ClientCommand::AuctionListPendingSales {
        auctioneer: auction.auctioneer.unwrap_or(0),
    });
    auction.sent(OWNER, now);
}

/// The auctioneer set (`0x59f000`): the interact target set first (`0x59f008`), whose interaction
/// close takes the window bound to the old one; then in its turn the house opens, the sell slot
/// clears (`0x59f10b`) and `AUCTION_HOUSE_SHOW` fires (`0x59f142`).
pub(crate) fn set_auctioneer(world: &mut World, auctioneer: u64, house_id: u32) {
    crate::ui_session::set_interact_target(world, auctioneer);
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut auction) = w.get_resource_mut::<AuctionOpen>() {
            auction.open(auctioneer, house_id);
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.write(UiScript::clear_auction_sell_item);
            ui.fire("AUCTION_HOUSE_SHOW", Vec::new());
        }
    });
}

/// The interaction close's auction leg (`0x51304a` -> `0x59f150`), its target already zeroed:
/// `AUCTION_HOUSE_CLOSED` with the house still open, so a listener's `CloseAuctionHouse` runs the
/// close again inside it (`0x59f16a`); then the close's own legs in turn.
pub(crate) fn close_by_interaction(world: &mut World) {
    if world
        .get_resource::<AuctionOpen>()
        .and_then(|a| a.auctioneer)
        .is_none()
    {
        return;
    }
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => {
            ui.fire("AUCTION_HOUSE_CLOSED", Vec::new());
            ui.world(close_legs);
        }
        None => close_legs(world),
    }
}

/// The close's legs after its event: a pending multi-sell fails (`0x59f17b`), then the house
/// empties (`0x59f198`) and the sell slot clears (`0x59f22b`); a close its listeners ran already
/// did them.
fn close_legs(world: &mut World) {
    let pending = match world.get_resource_mut::<AuctionOpen>() {
        Some(mut auction) => {
            let pending = auction.pending_sell();
            if pending {
                auction.multisell = None;
            }
            pending
        }
        None => return,
    };
    let empty = |w: &mut World| {
        if let Some(mut auction) = w.get_resource_mut::<AuctionOpen>() {
            auction.end_session();
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.write(UiScript::clear_auction_sell_item);
        }
    };
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => {
            if pending {
                ui.write(|s| s.end_auction_multisell(true));
            }
            ui.world(empty);
        }
        None => empty(world),
    }
}

/// The world leave's auction part (`0x528d55` -> `0x59f150`): with a house open,
/// `AUCTION_HOUSE_CLOSED` fires while it is still open; then a pending multi-sell fails and the
/// interact target clears on the auctioneer (`0x59f189`), whose interaction close runs the close
/// again when it held the target (`0x51304a`); then the house empties and the sell slot clears.
pub(crate) fn leave_world(world: &mut World) {
    let Some(auctioneer) = world
        .get_resource::<AuctionOpen>()
        .and_then(|a| a.auctioneer)
    else {
        return;
    };
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => {
            ui.fire("AUCTION_HOUSE_CLOSED", Vec::new());
            ui.world(move |w| close_after_event(w, auctioneer));
        }
        None => close_after_event(world, auctioneer),
    }
}

/// The close after its event, on a house a listener did not close already.
fn close_after_event(world: &mut World, auctioneer: u64) {
    let open = world
        .get_resource::<AuctionOpen>()
        .is_some_and(|a| a.auctioneer == Some(auctioneer));
    if !open {
        return;
    }
    let pending = world
        .get_resource_mut::<AuctionOpen>()
        .is_some_and(|mut a| a.pending_sell() && a.multisell.take().is_some());
    if pending {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.write(|s| s.end_auction_multisell(true));
        }
    }
    crate::ui_session::clear_interact_target(world, auctioneer);
    let empty = |w: &mut World| {
        if let Some(mut auction) = w.get_resource_mut::<AuctionOpen>() {
            auction.end_session();
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.write(UiScript::clear_auction_sell_item);
        }
    };
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => ui.world(empty),
        None => empty(world),
    }
}

/// The world entry's auction part (`0x52823f` -> `0x59dfb0`): the sell slot clears, the house's
/// gates and counts reset, and the Auctions tab and each list's selection are forgotten.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut auction) = world.get_resource_mut::<AuctionOpen>() {
        auction.enter_world();
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.write(UiScript::enter_auction_world);
    }
}

pub(crate) struct UiAuctionPlugin;

impl Plugin for UiAuctionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<AuctionOpen>().add_systems(
            OnEnter(crate::char_select::ClientState::InWorld),
            // A game start runs the house's init (`0x52b832` -> `0x5a0ba0`).
            |mut auction: ResMut<AuctionOpen>| auction.initialize_game(),
        );
    }
}

/// The Browse tab's category tree from the player's own DBCs, the twelve rows of the class
/// table; the addon-corpus survey reuses it.
pub(crate) fn categories(
    classes: Option<&crate::ui_items::ItemClasses>,
    subclasses: Option<&crate::ui_items::ItemSubClasses>,
) -> Vec<AuctionCategory> {
    let (Some(classes), Some(subclasses)) = (classes, subclasses) else {
        return Vec::new();
    };
    AUCTION_CLASSES
        .iter()
        .map(|&(class_id, has_subclass_filter)| {
            let name = classes.0.name(class_id).unwrap_or("").to_string();
            let subs = subclasses
                .0
                .subclasses_of(class_id)
                .into_iter()
                .filter(|&sub| {
                    subclasses.0.display_flags(class_id, sub) & SUBCLASS_HIDDEN_FROM_AUCTIONS == 0
                })
                .map(|sub| AuctionSubCategory {
                    sub_id: sub,
                    name: subclasses.0.name(class_id, sub).unwrap_or("").to_string(),
                    has_inv_types: subclasses.0.flags(class_id, sub) & SUBCLASS_OFFERS_INV_TYPES
                        != 0,
                })
                .collect();
            AuctionCategory {
                class_id,
                name,
                has_subclass_filter,
                subclasses: subs,
            }
        })
        .collect()
}

/// Seed the static auction filter tree before the VM loads interface files: 12340's category
/// verbs read DBC data even at addon file scope, without an auction session.
pub(crate) fn seed_auction_categories_for_vm(world: &World, script: &mut UiScript) {
    script.set_auction_item_classes(categories(
        world.get_resource::<crate::ui_items::ItemClasses>(),
        world.get_resource::<crate::ui_items::ItemSubClasses>(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multisell(items: Vec<(u64, u32)>, stack: u32, stacks: u32) -> MultiSell {
        MultiSell {
            auctioneer: 0x1234,
            items,
            stack_size: stack,
            remaining: stacks,
            total: stacks,
            bid: 100,
            buyout: 0,
            minutes: 720,
            pending: stacks > 1,
        }
    }

    #[test]
    fn sell_next_draws_each_stack_from_the_end() {
        // Three stacks of 4, 3 and 5; two auctions of 5 each.
        let mut ms = multisell(vec![(1, 4), (2, 3), (3, 5)], 5, 2);
        assert_eq!(
            next_stack(&mut ms),
            Ok(Some(vec![(3, 5)])),
            "the last stack covers the first auction"
        );
        assert_eq!(
            next_stack(&mut ms),
            Ok(Some(vec![(2, 3), (1, 2)])),
            "the next takes the 3, then 2 of the 4"
        );
        assert_eq!(ms.items, vec![(1, 2)]);
        assert_eq!(next_stack(&mut ms), Ok(None), "two of two: done");
    }

    #[test]
    fn running_dry_is_a_multisell_failure() {
        let mut ms = multisell(vec![(1, 2)], 5, 2);
        assert_eq!(next_stack(&mut ms), Err(()));
    }

    #[test]
    fn categories_follow_the_twelve_row_table() {
        let tree = categories(None, None);
        assert!(tree.is_empty(), "no DBCs, no tree");
        assert_eq!(AUCTION_CLASSES.len(), 12);
        assert_eq!(AUCTION_CLASSES[4], (16, true), "Glyph is the fifth class");
        assert_eq!(
            AUCTION_CLASSES[11],
            (12, false),
            "Quest offers no subclass rows"
        );
    }
}
