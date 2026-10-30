//! The auction window's app side, after the 12340 auction house: [`net`] fills [`AuctionOpen`] from
//! the wire, [`feed_auction`] pushes it into the VM and fires its events, and [`drain_auction`]
//! sends the Lua intents through the house's query throttle.
//!
//! The hello reply opens the window, not the click (the auctioneer set, `0x59f000`, fires
//! `AUCTION_HOUSE_SHOW`). Browse, Bids and Auctions are independent lists, each with its own
//! query gate (`0x59a2f0`). Walking out of range closes the window with no packet.

use bevy::prelude::*;

use benilla_protocol::messages::{AuctionListEntry, AuctionListQuery};
use benilla_ui::script::{
    AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionState,
    AuctionSubCategory, ScriptValue, UiScript, BIDDER, LIST, OWNER,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, UiError};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod net;

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

/// The emote a placed bid plays (`0x5a019d`).
const BID_EMOTE: u32 = 3;

/// The `AuctionHouse.dbc` catalog; without it the sell pane shows no deposit.
#[derive(Resource)]
pub(crate) struct AuctionHouses(pub(crate) benilla_formats::AuctionHouseCatalog);

/// One row of a list as the reference keeps it: the wire record, its expiry and whether it is a
/// sold auction from the pending sales.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AuctionEntry {
    pub(crate) wire: AuctionListEntry,
    /// The time left at arrival, seconds; the feed stamps [`Self::expires_at`] from it.
    left_secs: f64,
    /// The expiry on the VM's `GetTime` clock, stamped by the first feed after arrival.
    expires_at: Option<f64>,
    /// A bid landed: the feed holds the expiry at least [`BID_EXTENSION_SECS`] out.
    extend: bool,
    pub(crate) sold: bool,
}

impl AuctionEntry {
    pub(crate) fn live(wire: AuctionListEntry) -> Self {
        Self {
            left_secs: f64::from(wire.time_left_ms) / 1000.0,
            wire,
            expires_at: None,
            extend: false,
            sold: false,
        }
    }
}

/// A row patch from a bid notice: the new bid and step, and the bidder (`None` = the player).
pub(crate) struct Patch {
    pub(crate) bid: u32,
    pub(crate) min_increment: u32,
    pub(crate) bidder: Option<u64>,
    pub(crate) extend: bool,
}

/// One of the three lists.
#[derive(Debug, Default)]
pub(crate) struct AuctionListSlot {
    pub(crate) entries: Vec<AuctionEntry>,
    /// The match count before the page cap.
    pub(crate) total: u32,
    /// A result for this list landed since the window opened.
    received: bool,
}

/// Stacks to sell from, `(bag, slot, count)`.
type Stacks = Vec<(i64, u32, u32)>;

/// A multi-sell in progress: the stacks it draws from, consumed from the end, and the stacks still
/// to list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MultiSell {
    auctioneer: u64,
    /// `(bag, slot, count)` left to draw from.
    items: Vec<(i64, u32, u32)>,
    stack_size: u32,
    pub(crate) remaining: u32,
    pub(crate) total: u32,
    bid: u32,
    buyout: u32,
    minutes: u32,
    /// The listing's events fire: more than one stack.
    pending: bool,
    /// Send the next stack in the drain.
    send_next: bool,
}

/// What the live probe reads to tell an empty answer from none; the client never reads it.
#[derive(Default)]
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

/// The open auctioneer session; cleared on a client-side close, on walking away and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct AuctionOpen {
    pub(crate) auctioneer: Option<u64>,
    /// The `AuctionHouse.dbc` row, which keys the deposit rate.
    pub(crate) house_id: u32,
    pub(crate) lists: [AuctionListSlot; 3],
    /// The player's guid, which the feed keeps current for row patches.
    player: u64,
    show_requested: bool,
    /// A closed house answered the hello: fire `AUCTION_HOUSE_DISABLED`.
    disabled_requested: bool,
    /// Per list, a result landed, so its event is owed even for an identical page.
    list_result_landed: [bool; 3],
    /// Messages queued for [`feed_auction`]; kept across a close, as a sale notice can land after.
    pub(crate) messages: Vec<AuctionMessage>,
    /// The last query's time, on `Time::elapsed_secs_f64`.
    last_query: [Option<f64>; 3],
    /// Sends not yet answered.
    pending: [i32; 3],
    /// The query delay, seconds.
    delay_secs: f64,
    last_get_all: Option<f64>,
    /// Whether the owner list and the bidder list may be asked for.
    can_get_owner: bool,
    can_get_bidder: bool,
    last_bidder_page: u32,
    /// The bid in flight, which a `BID_PLACED` result writes into the browse row.
    last_bid_amount: u32,
    /// The outbid auctions, `(house, auction)`: kept across sessions, sent with each bidder query.
    pub(crate) outbid: Vec<(u32, u32)>,
    /// Sends the drain owes: the owner list after a listing, the sales after a removal, the
    /// bidder list after a bid, the bid emote.
    owner_list_wanted: bool,
    pending_sales_wanted: bool,
    bidder_list_wanted: Option<u32>,
    emote_wanted: bool,
    pub(crate) multisell: Option<MultiSell>,
    /// `AUCTION_MULTISELL_*` events for the feed, in order.
    multisell_events: Vec<(&'static str, Vec<ScriptValue>)>,
    /// Empty the VM's sell slot next feed, as the reference's sell-slot clear does.
    sell_slot_clear: bool,
    /// The drain took a query: the feed re-pushes the gate even if it reads unchanged.
    gate_dirty: bool,
    pub(crate) wire: AuctionWireLog,
}

/// One of the reference's auction message rows, with the item whose name fills a `%s`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuctionMessage {
    pub(crate) key: &'static str,
    /// The entry and random property whose suffixed item name fills the `%s`.
    pub(crate) item: Option<(u32, i32)>,
}

impl AuctionMessage {
    pub(crate) fn key(key: &'static str) -> Self {
        Self { key, item: None }
    }

    pub(crate) fn item(key: &'static str, entry: u32, property: i32) -> Self {
        Self {
            key,
            item: Some((entry, property)),
        }
    }
}

impl AuctionOpen {
    /// The auctioneer set (`0x59f000`): the gates open, the lists empty and the window shows.
    pub(crate) fn open(&mut self, auctioneer: u64, house_id: u32) {
        self.auctioneer = Some(auctioneer);
        self.house_id = house_id;
        self.lists = Default::default();
        self.last_query = [None; 3];
        self.pending = [0; 3];
        self.delay_secs = DEFAULT_QUERY_DELAY_SECS;
        self.can_get_owner = true;
        self.can_get_bidder = true;
        self.last_bidder_page = 0;
        self.sell_slot_clear = true;
        self.show_requested = true;
        self.disabled_requested = false;
        self.gate_dirty = true;
    }

    /// The query gate (`0x59a2f0`): nothing in flight, and the delay past the last send or answer.
    pub(crate) fn can_send(&self, list: usize, now: f64) -> bool {
        self.pending[list] <= 0 && self.last_query[list].is_none_or(|t| now - t >= self.delay_secs)
    }

    fn can_get_all(&self, now: f64) -> bool {
        self.last_get_all
            .is_none_or(|t| now - t >= GET_ALL_COOLDOWN_SECS)
    }

    /// A query of `list` went out.
    pub(crate) fn sent(&mut self, list: usize, now: f64) {
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

    /// Replace a list's page. The owner list keeps its sold rows (`0x59e480`).
    pub(crate) fn set_list(&mut self, which: usize, entries: Vec<AuctionEntry>, total: u32) {
        self.list_result_landed[which] = true;
        let slot = &mut self.lists[which];
        slot.received = true;
        if which == OWNER {
            slot.entries.retain(|e| e.sold);
            slot.entries.extend(entries);
        } else {
            slot.entries = entries;
        }
        slot.total = total;
    }

    /// Replace the owner list's sold rows, keeping the live ones (`0x59e880`).
    pub(crate) fn set_sold(&mut self, sold: Vec<AuctionEntry>) {
        self.list_result_landed[OWNER] = true;
        let slot = &mut self.lists[OWNER];
        slot.received = true;
        slot.entries.retain(|e| !e.sold);
        slot.entries.extend(sold);
    }

    /// Patch one auction's bid on `list`.
    pub(crate) fn patch(&mut self, list: usize, auction_id: u32, p: Patch) {
        let player = self.player;
        if let Some(e) = self.lists[list]
            .entries
            .iter_mut()
            .find(|e| e.wire.auction_id == auction_id)
        {
            e.wire.current_bid = p.bid;
            e.wire.min_increment = p.min_increment;
            e.wire.bidder_guid = p.bidder.unwrap_or(player);
            e.extend |= p.extend;
        }
    }

    /// The auction expiry (`0x59f2f0`): the auction leaves every list; leaving the owner list asks
    /// the pending sales again.
    pub(crate) fn expire(&mut self, auction_id: u32) {
        for list in [LIST, OWNER, BIDDER] {
            let slot = &mut self.lists[list];
            if let Some(pos) = slot
                .entries
                .iter()
                .position(|e| e.wire.auction_id == auction_id)
            {
                slot.entries.remove(pos);
                slot.total = slot.total.saturating_sub(1);
                if list == OWNER {
                    self.pending_sales_wanted = true;
                }
            }
        }
    }

    fn queue_multisell_event(&mut self, event: &'static str, args: &[u32]) {
        self.multisell_events.push((
            event,
            args.iter()
                .map(|&n| ScriptValue::Int(i64::from(n)))
                .collect(),
        ));
    }

    /// The multi-sell failure (`0x59ab20`): a pending multi-sell ends, unlocked, its slot emptied,
    /// and fires `AUCTION_MULTISELL_FAILURE`.
    pub(crate) fn multisell_failure(&mut self) {
        if let Some(ms) = self.multisell.take() {
            if ms.pending {
                self.queue_multisell_event("AUCTION_MULTISELL_FAILURE", &[]);
            }
            self.sell_slot_clear = true;
        }
    }

    /// The next-item sell (`0x59ba80`): draw one stack from the end of the gathered items, as
    /// `(bag, slot, count)` per item; `None` once every stack is out, or on running dry.
    fn next_stack(&mut self) -> Option<(Stacks, MultiSell)> {
        let ms = self.multisell.as_mut()?;
        if ms.remaining == 0 {
            self.multisell = None;
            self.sell_slot_clear = true;
            return None;
        }
        ms.remaining -= 1;
        let mut rest = ms.stack_size;
        let mut last = ms.items.len().checked_sub(1)?;
        while rest > ms.items[last].2 {
            rest -= ms.items[last].2;
            let Some(prev) = last.checked_sub(1) else {
                self.multisell_failure();
                return None;
            };
            last = prev;
        }
        let mut send: Vec<(i64, u32, u32)> = ms.items[last + 1..].iter().rev().copied().collect();
        send.push((ms.items[last].0, ms.items[last].1, rest));
        let emptied = ms.items[last].2 == rest;
        ms.items[last].2 -= rest;
        ms.items.truncate(if emptied { last } else { last + 1 });
        let snapshot = ms.clone();
        Some((send, snapshot))
    }

    /// Close the window client-side (`CloseAuctionHouse` `0x59f150`); the reference sends nothing.
    pub(crate) fn clear(&mut self) {
        self.multisell_failure();
        self.auctioneer = None;
        self.house_id = 0;
        self.lists = Default::default();
        self.show_requested = false;
        self.list_result_landed = [false; 3];
        self.can_get_owner = false;
        self.can_get_bidder = false;
        self.owner_list_wanted = false;
        self.pending_sales_wanted = false;
        self.bidder_list_wanted = None;
        self.emote_wanted = false;
        self.sell_slot_clear = true;
    }
}

impl NpcSession for AuctionOpen {
    fn npc(&self) -> Option<u64> {
        self.auctioneer
    }

    fn close(&mut self) {
        self.clear();
    }
}

pub(crate) struct UiAuctionPlugin;

impl Plugin for UiAuctionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<AuctionOpen>().add_systems(
            Update,
            (
                // Range-close before the feed so the close fires the same frame; the feed before
                // the input pass, and after `UnitFeed` so a row's tooltip reads a landed template;
                // the drain after the input pass so a click goes out the same frame.
                close_npc_session_out_of_range::<AuctionOpen>.before(feed_auction),
                // Only with the interface up: a sale notice landing in the login burst would
                // otherwise be shown on the boot VM and lost.
                feed_auction
                    .after(crate::ui_unit::UnitFeed)
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
                drain_auction.after(UiInput),
            ),
        );
    }
}

/// `GetAuctionItemInfo`'s `highBidder` for a row of `list` (`0x59d5e0`): on an `"owner"` row the
/// player owns, the bidder's name through the name cache (nil with no bid or until it lands);
/// any other row, `1` if the player holds the bid, else nil.
fn high_bidder(
    list: usize,
    entry: &AuctionEntry,
    self_guid: Option<u64>,
    names: &NameCache,
    commands: &NetCommands,
) -> AuctionHighBidder {
    let owner = if entry.sold {
        self_guid.unwrap_or(0)
    } else {
        entry.wire.owner_guid
    };
    let owned = list == OWNER && self_guid.is_some_and(|g| g == owner);
    if owned {
        if entry.wire.bidder_guid == 0 {
            return AuctionHighBidder::Nil;
        }
        return names
            .resolve(entry.wire.bidder_guid, commands)
            .map_or(AuctionHighBidder::Nil, |n| {
                AuctionHighBidder::Name(n.to_string())
            });
    }
    if self_guid.is_some_and(|g| g == entry.wire.bidder_guid) {
        AuctionHighBidder::Player
    } else {
        AuctionHighBidder::Nil
    }
}

/// The DBC catalogs a row resolves through.
struct RowCatalogs<'a> {
    items: &'a Items,
    icons: Option<&'a ItemDisplays>,
    names: &'a NameCache,
    commands: &'a NetCommands,
    rolls: crate::items::RollCatalogs<'a>,
}

/// One entry of `list` as a VM row, through the ask-once template and name caches; a `None`
/// fills in when its answer lands.
fn resolve_row(
    list: usize,
    entry: &AuctionEntry,
    self_guid: Option<u64>,
    c: &RowCatalogs,
) -> AuctionItemRow {
    let w = &entry.wire;
    let template = c.items.template(w.item_entry, 0, c.commands);
    let roll = w.random_property_id as u32;
    let name = template.map(|t| c.rolls.name(&t.name, roll));
    let quality = template.map(|t| t.quality);
    let level = template.map_or(0, |t| t.required_level);
    let texture = crate::ui_items::item_icon(c.icons, template.map_or(0, |t| t.display_info_id));
    let link = name.as_ref().map(|n| {
        crate::ui_items::item_link_full(
            w.item_entry,
            w.enchants[0].id,
            roll,
            w.suffix_factor,
            n,
            quality.unwrap_or(0),
        )
    });
    let owner_guid = if entry.sold {
        self_guid.unwrap_or(0)
    } else {
        w.owner_guid
    };
    let bidder_status = if w.bidder_guid == 0 {
        0
    } else if self_guid == Some(w.bidder_guid) {
        2
    } else {
        1
    };
    AuctionItemRow {
        auction_id: w.auction_id,
        item_id: w.item_entry,
        random_property_id: roll,
        name,
        texture,
        count: w.count,
        quality,
        level,
        min_bid: w.start_bid,
        min_increment: w.min_increment,
        buyout_price: w.buyout,
        bid_amount: w.current_bid,
        high_bidder: high_bidder(list, entry, self_guid, c.names, c.commands),
        owner: c.names.resolve(owner_guid, c.commands).map(str::to_string),
        owner_guid,
        bidder_status,
        expires_at: entry.expires_at.unwrap_or(0.0),
        sold: entry.sold,
        link,
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

/// The DBC catalogs as one system param, since [`feed_auction`] is at Bevy's 16-param limit.
type AuctionCatalogs<'w> = (
    Option<Res<'w, crate::ui_items::ItemClasses>>,
    Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    Option<Res<'w, crate::items::RandomProperties>>,
    Option<Res<'w, crate::items::Enchants>>,
);

/// Push the auction house into the VM and fire its events on a change, diffed against `VmMemo`s.
fn feed_auction(
    script: Option<NonSendMut<UiScript>>,
    mut auction: ResMut<AuctionOpen>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    time: Res<Time>,
    catalogs: AuctionCatalogs,
    houses: Option<Res<AuctionHouses>>,
    mut sink: MessageSink,
    self_q: Query<&crate::net::Guid, With<SelfPlayer>>,
    mut last: Local<crate::ui_script::VmMemo<Option<AuctionState>>>,
    mut last_open: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_gate: Local<crate::ui_script::VmMemo<([bool; 3], bool)>>,
    mut last_sell: Local<crate::ui_script::VmMemo<Option<(i64, u32)>>>,
    mut last_classes: Local<crate::ui_script::VmMemo<Vec<AuctionCategory>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_open = last_open.get(&script);
    let last_gate = last_gate.get(&script);
    let last_sell = last_sell.get(&script);

    // The class tree needs no session: the stock `AuctionFrameBrowse_OnLoad` reads it at load.
    // The tree is a function of the two catalogs alone: rebuilt when either lands or changes, or
    // for a VM that has none yet.
    let last_classes = last_classes.get(&script);
    let catalogs_moved = catalogs.0.as_ref().is_some_and(|c| c.is_changed())
        || catalogs.1.as_ref().is_some_and(|c| c.is_changed());
    if catalogs_moved || last_classes.is_empty() {
        let classes_now = categories(catalogs.0.as_deref(), catalogs.1.as_deref());
        if *last_classes != classes_now {
            *last_classes = classes_now.clone();
            script.set_auction_item_classes(classes_now);
        }
    }

    let rolls = crate::items::RollCatalogs {
        props: catalogs.2.as_deref(),
        enchants: catalogs.3.as_deref(),
    };

    // A `_S` line whose item is not cached yet stays queued, as the reference defers it through
    // its item-cache callback (`0x59a400`).
    {
        let mut deferred = Vec::new();
        let mut lines = Vec::new();
        for msg in std::mem::take(&mut auction.messages) {
            let fill = match msg.item {
                None => None,
                Some((entry, property)) => match items.template(entry, 0, &commands) {
                    Some(t) => Some(rolls.name(&t.name, property as u32)),
                    None => {
                        deferred.push(msg);
                        continue;
                    }
                },
            };
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            let err = match fill {
                Some(s) => UiError::s(msg.key, s),
                None => UiError::key(msg.key),
            };
            if let Some(text) = ui_error_text(&err, &get) {
                lines.push(Shown::keyed(msg.key, text));
            }
        }
        auction.messages = deferred;
        show_messages(&mut script, &mut sink, "ui_auction", lines);
    }

    let self_guid = self_q.iter().next().map(|g| g.0);
    auction.player = self_guid.unwrap_or(0);

    // Stamp each new row's expiry on the VM clock, and hold a bid row open the extension.
    let vm_now = script.now();
    for slot in &mut auction.lists {
        for e in &mut slot.entries {
            let at = *e.expires_at.get_or_insert(vm_now + e.left_secs);
            if std::mem::take(&mut e.extend) {
                e.expires_at = Some(at.max(vm_now + BID_EXTENSION_SECS));
            }
        }
    }

    let row_catalogs = RowCatalogs {
        items: &items,
        icons: icons.as_deref(),
        names: &names,
        commands: &commands,
        rolls,
    };
    let fresh = auction.auctioneer.map(|_| {
        let lists = [LIST, BIDDER, OWNER].map(|i| {
            let slot = &auction.lists[i];
            AuctionListState {
                rows: slot
                    .entries
                    .iter()
                    .map(|e| resolve_row(i, e, self_guid, &row_catalogs))
                    .collect(),
                total: slot.total,
            }
        });
        AuctionState {
            lists,
            deposit_percent: houses
                .as_deref()
                .and_then(|h| h.0.deposit_percent(auction.house_id))
                .unwrap_or(0),
        }
    });

    let opened = last_open.is_none() && auction.auctioneer.is_some();
    let closed = last_open.is_some() && auction.auctioneer.is_none();
    let show_requested = std::mem::take(&mut auction.show_requested);
    // The owner list announces itself only with both of its replies in (`0x59e86a`).
    let owner_settled = auction.pending[OWNER] <= 0;
    let mut landed = std::mem::take(&mut auction.list_result_landed);
    if !owner_settled && landed[OWNER] {
        landed[OWNER] = false;
        auction.list_result_landed[OWNER] = true;
    }
    let changed = fresh != *last;
    let owed: [bool; 3] = std::array::from_fn(|i| {
        landed[i] || fresh.as_ref().map(|s| &s.lists[i]) != last.as_ref().map(|s| &s.lists[i])
    });
    if changed {
        script.set_auction(fresh.clone());
    }
    if std::mem::take(&mut auction.disabled_requested) {
        script.fire_event("AUCTION_HOUSE_DISABLED", vec![]);
    }
    if opened {
        script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
    } else if closed {
        script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
    } else if auction.auctioneer.is_some() {
        if show_requested {
            script.fire_event("AUCTION_HOUSE_SHOW", vec![]);
        }
        // Literal names, not a loop: `reference_ui`'s producer test finds fire sites by literal.
        if owed[LIST] {
            script.fire_event("AUCTION_ITEM_LIST_UPDATE", vec![]);
        }
        if owed[BIDDER] {
            script.fire_event("AUCTION_BIDDER_LIST_UPDATE", vec![]);
        }
        if owed[OWNER] && owner_settled {
            script.fire_event("AUCTION_OWNED_LIST_UPDATE", vec![]);
        }
    }
    *last = fresh;
    *last_open = auction.auctioneer;

    for (event, args) in std::mem::take(&mut auction.multisell_events) {
        match event {
            "AUCTION_MULTISELL_START" => script.fire_event("AUCTION_MULTISELL_START", args),
            "AUCTION_MULTISELL_UPDATE" => script.fire_event("AUCTION_MULTISELL_UPDATE", args),
            _ => script.fire_event("AUCTION_MULTISELL_FAILURE", args),
        }
    }
    if std::mem::take(&mut auction.sell_slot_clear) {
        script.clear_auction_sell_item();
    }
    let locked: Vec<(i64, u32)> = auction
        .multisell
        .as_ref()
        .filter(|ms| ms.pending)
        .map(|ms| ms.items.iter().map(|&(b, s, _)| (b, s)).collect())
        .unwrap_or_default();
    script.set_auction_locked(locked);

    let sell = script.auction_sell_item();
    if sell != *last_sell {
        *last_sell = sell;
        script.fire_event("NEW_AUCTION_UPDATE", vec![]);
    }

    // The throttle, pushed when it changes or the drain took a query; the Search button polls it.
    let now = time.elapsed_secs_f64();
    let gate = (
        [LIST, BIDDER, OWNER].map(|i| auction.can_send(i, now)),
        auction.can_get_all(now),
    );
    if gate != *last_gate || std::mem::take(&mut auction.gate_dirty) {
        script.set_auction_can_query(gate.0, gate.1);
        *last_gate = gate;
    }
}

/// The sends the drain makes, sharing one command channel.
struct Sender<'a> {
    commands: &'a NetCommands,
    auctioneer: u64,
}

impl Sender<'_> {
    fn send(&self, command: ClientCommand) {
        let _ = self.commands.0.send(command);
    }

    /// The owner-list ask (`0x59cc50`): the owner list and the pending sales, two answers owed.
    fn owner_list(&self, auction: &mut AuctionOpen, now: f64) {
        if !auction.can_get_owner || !auction.can_send(OWNER, now) {
            debug!(
                "ui_auction: owner list held: can_get {} pending {}",
                auction.can_get_owner, auction.pending[OWNER]
            );
            return;
        }
        debug!("ui_auction: owner list and pending sales asked");
        self.send(ClientCommand::AuctionListOwnerItems {
            auctioneer: self.auctioneer,
            list_from: 0,
        });
        self.send(ClientCommand::AuctionListPendingSales {
            auctioneer: self.auctioneer,
        });
        auction.sent(OWNER, now);
        auction.pending[OWNER] += 1;
        auction.can_get_owner = false;
    }

    /// The bidder-list ask (`0x59ce50`) with this house's outbid ids.
    fn bidder_list(&self, auction: &mut AuctionOpen, list_from: u32, now: f64) {
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
        self.send(ClientCommand::AuctionListBidderItems {
            auctioneer: self.auctioneer,
            list_from,
            auction_ids,
        });
        auction.sent(BIDDER, now);
        auction.can_get_bidder = false;
        auction.last_bidder_page = list_from;
    }
}

/// Drain the Lua intents into the auction `CMSG`s.
fn drain_auction(
    script: Option<NonSendMut<UiScript>>,
    mut auction: ResMut<AuctionOpen>,
    commands: Res<NetCommands>,
    time: Res<Time>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(auctioneer) = auction.auctioneer else {
        // No session: drop every intent.
        let _ = script.take_auction_close();
        let _ = script.take_auction_query();
        let _ = script.take_auction_owner_query();
        let _ = script.take_auction_bidder_query();
        let _ = script.take_auction_bids();
        let _ = script.take_auction_cancels();
        let _ = script.take_auction_start();
        let _ = script.take_auction_cancel_sell();
        return;
    };
    let now = time.elapsed_secs_f64();
    let tx = Sender {
        commands: &commands,
        auctioneer,
    };
    let guid_of = |bag: i64, slot: u32| -> Option<u64> {
        let store = self_q.iter().next()?;
        crate::ui_items::slot_guid(&store.0, bag, slot.saturating_sub(1) as u8, &objects)
    };

    // The browse query, behind the gate the binding already read (`0x59bcf0`).
    if let Some(q) = script.take_auction_query() {
        auction.gate_dirty = true;
        if auction.can_send(LIST, now) && (!q.get_all || auction.can_get_all(now)) {
            auction.wire.browse_sent += 1;
            auction.wire.last_sort = q.sort.clone();
            auction.wire.last_filter = Some((q.inv_type, q.class, q.sub_class));
            tx.send(ClientCommand::AuctionListItems {
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
                    sort: q.sort,
                },
            });
            auction.sent(LIST, now);
            if q.get_all {
                auction.last_get_all = Some(now);
            }
        }
    }

    if script.take_auction_owner_query() || std::mem::take(&mut auction.owner_list_wanted) {
        tx.owner_list(&mut auction, now);
    }
    if std::mem::take(&mut auction.pending_sales_wanted) && auction.can_send(OWNER, now) {
        tx.send(ClientCommand::AuctionListPendingSales { auctioneer });
        auction.sent(OWNER, now);
    }
    if let Some(list_from) = script.take_auction_bidder_query() {
        if auction.can_send(BIDDER, now) {
            tx.bidder_list(&mut auction, list_from, now);
        }
    }
    if let Some(list_from) = auction.bidder_list_wanted.take() {
        tx.bidder_list(&mut auction, list_from, now);
    }

    for bid in script.take_auction_bids() {
        tx.send(ClientCommand::AuctionPlaceBid {
            auctioneer,
            auction_id: bid.auction_id,
            price: bid.amount,
        });
        auction.last_bid_amount = bid.amount;
    }
    for auction_id in script.take_auction_cancels() {
        tx.send(ClientCommand::AuctionRemoveItem {
            auctioneer,
            auction_id,
        });
    }
    if std::mem::take(&mut auction.emote_wanted) {
        tx.send(ClientCommand::Emote {
            emote_id: BID_EMOTE,
        });
    }

    // StartAuction (`0x59f990`): the direct sale goes out as is; anything else is a multi-sell
    // whose stacks go out one per `STARTED` answer.
    if let Some(req) = script.take_auction_start() {
        if req.direct {
            let items: Vec<(u64, u32)> = req
                .items
                .iter()
                .filter_map(|&(bag, slot, count)| Some((guid_of(bag, slot)?, count)))
                .collect();
            if !items.is_empty() {
                auction.wire.sells.push(items.iter().map(|i| i.1).collect());
                debug!("ui_auction: sell {items:?}");
                tx.send(ClientCommand::AuctionSellItem {
                    auctioneer,
                    items,
                    bid: req.min_bid,
                    buyout: req.buyout,
                    etime_minutes: req.run_time_minutes,
                });
            }
        } else {
            let pending = req.num_stacks > 1;
            auction.multisell = Some(MultiSell {
                auctioneer,
                items: req.items,
                stack_size: req.stack_size,
                remaining: req.num_stacks,
                total: req.num_stacks,
                bid: req.min_bid,
                buyout: req.buyout,
                minutes: req.run_time_minutes,
                pending,
                send_next: true,
            });
            if pending {
                auction.queue_multisell_event("AUCTION_MULTISELL_START", &[req.num_stacks]);
            }
        }
    }
    if script.take_auction_cancel_sell() {
        auction.multisell_failure();
    }
    let send_next = auction.multisell.as_ref().is_some_and(|ms| ms.send_next);
    if send_next {
        if let Some(ms) = auction.multisell.as_mut() {
            ms.send_next = false;
        }
        if let Some((stack, ms)) = auction.next_stack() {
            let items: Vec<(u64, u32)> = stack
                .iter()
                .filter_map(|&(bag, slot, count)| Some((guid_of(bag, slot)?, count)))
                .collect();
            if items.len() == stack.len() {
                auction.wire.sells.push(items.iter().map(|i| i.1).collect());
                debug!("ui_auction: sell {items:?}");
                tx.send(ClientCommand::AuctionSellItem {
                    auctioneer: ms.auctioneer,
                    items,
                    bid: ms.bid,
                    buyout: ms.buyout,
                    etime_minutes: ms.minutes,
                });
                // A single stack is one send; nothing waits on its answer.
                if !ms.pending {
                    auction.multisell = None;
                    auction.sell_slot_clear = true;
                }
            } else {
                auction.multisell_failure();
            }
        }
    }

    if script.take_auction_close() {
        auction.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multisell(items: Vec<(i64, u32, u32)>, stack: u32, stacks: u32) -> AuctionOpen {
        let mut a = AuctionOpen::default();
        a.open(0x1234, 1);
        a.multisell = Some(MultiSell {
            auctioneer: 0x1234,
            items,
            stack_size: stack,
            remaining: stacks,
            total: stacks,
            bid: 100,
            buyout: 0,
            minutes: 720,
            pending: stacks > 1,
            send_next: true,
        });
        a
    }

    #[test]
    fn sell_next_draws_each_stack_from_the_end() {
        // Three stacks of 4, 3 and 5; two auctions of 5 each.
        let mut a = multisell(vec![(0, 1, 4), (0, 2, 3), (0, 3, 5)], 5, 2);
        let (first, _) = a.next_stack().unwrap();
        assert_eq!(
            first,
            vec![(0, 3, 5)],
            "the last stack covers the first auction"
        );
        let (second, _) = a.next_stack().unwrap();
        assert_eq!(
            second,
            vec![(0, 2, 3), (0, 1, 2)],
            "the next takes the 3, then 2 of the 4"
        );
        assert_eq!(a.multisell.as_ref().unwrap().items, vec![(0, 1, 2)]);
        assert!(a.next_stack().is_none(), "two of two: done");
        assert!(a.multisell.is_none());
        assert!(a.sell_slot_clear);
    }

    #[test]
    fn running_dry_is_a_multisell_failure() {
        let mut a = multisell(vec![(0, 1, 2)], 5, 2);
        assert!(a.next_stack().is_none());
        assert!(a.multisell.is_none());
        assert_eq!(a.multisell_events.len(), 1);
        assert_eq!(a.multisell_events[0].0, "AUCTION_MULTISELL_FAILURE");
    }

    #[test]
    fn the_owner_query_owes_two_answers() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut a = AuctionOpen::default();
        a.open(0x1234, 1);
        Sender {
            commands: &commands,
            auctioneer: 0x1234,
        }
        .owner_list(&mut a, 0.0);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(matches!(
            sent[0],
            ClientCommand::AuctionListOwnerItems { .. }
        ));
        assert!(matches!(
            sent[1],
            ClientCommand::AuctionListPendingSales { .. }
        ));
        assert_eq!(a.pending[OWNER], 2);
        assert!(
            !a.can_get_owner,
            "one owner list per opening until a listing reopens it"
        );
        a.answered(OWNER, 0.1, Some(0));
        assert!(!a.can_send(OWNER, 1.0), "the sales are still owed");
        a.answered(OWNER, 0.2, None);
        assert!(a.can_send(OWNER, 0.2));
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
