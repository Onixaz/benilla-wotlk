//! The socketing and purchase-refund round trips. The VM's socketing engine and refund verbs
//! ([`benilla_ui::script`]) queue sends; here they go out, `SMSG_SOCKET_GEMS_RESULT` writes the
//! socketed item's enchants (`0x7081b0`), the refund records (`item+0x3d4`) are asked and kept,
//! and `SMSG_ITEM_REFUND_RESULT` prints its lines (`0x6d9b40`).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::field::{item_enchant_field, FIELD_ITEM_FLAGS};
use benilla_protocol::messages::{ObjectFields, PurchaseCost, RefundRecord};
use benilla_protocol::{SessionEvent, SessionEventKind as K};
use benilla_ui::script::{RefundView, UiScript};
use benilla_ui::strings::Arg;

use crate::items::Items;
use crate::net::{
    merge_store_fields, ClientCommand, FieldChanged, GuidIndex, NetCommands, NetHandlerApp,
    ObjectStore, Objects, SelfGuid, SelfPlayer,
};
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_chat::ChatLog;
use crate::ui_script::{UiFeed, UiInput};

/// The template flag that marks a purchase with a refund record (`template+0x18 & 0x1000`).
const ITEM_FLAG_REFUNDABLE: u32 = 0x1000;
/// `ITEM_FIELD_FLAGS & 0x100`: a bound item still in its trade window.
const ITEM_FIELD_FLAG_BOP_TRADEABLE: u32 = 0x100;
/// `ITEM_FIELD_CREATE_PLAYED_TIME`'s shared index.
const FIELD_ITEM_CREATE_PLAYED_TIME: u16 = 45;
/// The refund lines' currency caps (`0x6d9c4f`, `0x6d9cbf`).
const HONOR_CAP: u32 = 75_000;
const ARENA_CAP: u32 = 10_000;

/// The refund records and their asks.
#[derive(Resource, Default)]
pub(crate) struct RefundRecords {
    records: HashMap<u64, RefundRecord>,
    /// The pending bit (`item+0x394 & 4`): asked and unanswered. A silent server leaves it set, so
    /// the item is never asked again.
    asked: HashSet<u64>,
    /// Items created since the last feed, whose templates decide whether to ask.
    created: Vec<u64>,
    /// Steps on every record change, so the feed re-pushes.
    epoch: u64,
}

impl RefundRecords {
    /// Whether `item` holds a record whose window is open at `played_now`.
    pub(crate) fn open(&self, item: u64, played_now: i64) -> bool {
        self.records
            .get(&item)
            .is_some_and(|r| window_open(r.played_at_purchase, played_now))
    }
}

/// A two-hour played-time window from `stamp` is open (`stamp - played + 7200 > 0`).
fn window_open(stamp: u32, played_now: i64) -> bool {
    i64::from(stamp) - played_now + 7200 > 0
}

/// `IsBound && !IsBoundForTrade` (`0x709550`): bound by its flag, the player's own, flagged
/// `0x100`, with no binding enchant, and its trade window open.
pub(crate) fn bound_tradeable(
    fields: &ObjectFields,
    me: Option<u64>,
    enchants: Option<&crate::items::Enchants>,
    played_now: i64,
) -> bool {
    fields.item_flags().is_some_and(|f| f & 1 != 0)
        && trade_stamp(fields, me, enchants).is_some_and(|s| window_open(s, played_now))
}

/// The refund results waiting for the VM's strings.
#[derive(Resource, Default)]
pub(crate) struct RefundResults(Vec<(u32, Option<PurchaseCost>)>);

pub(crate) struct UiSocketPlugin;

impl Plugin for UiSocketPlugin {
    fn build(&self, app: &mut App) {
        crate::item_names::register(app);
        app.init_resource::<RefundRecords>()
            .init_resource::<RefundResults>()
            .net_handler(K::SocketGemsResult, on_socket_gems_result)
            .net_handler(K::ItemRefundInfo, on_refund_info)
            .net_handler(K::ItemRefundResult, on_refund_result)
            .net_handler(K::ItemCreate, on_item_create)
            .add_systems(
                Update,
                (
                    drain_item_sends.after(UiInput),
                    // The refund results print with the VM's strings, so they wait for its frames.
                    feed_refunds
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                ),
            );
    }
}

/// `SMSG_SOCKET_GEMS_RESULT` (`0x6e6330` → `0x7081b0`): each nonzero enchant lands in its slot,
/// 2 to 5, and the watchers refresh off the edges. Deviation: an item not held drops the packet,
/// where 12340 reads through a null item.
fn on_socket_gems_result(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut stores: Query<&mut ObjectStore>,
    mut edges: MessageWriter<FieldChanged>,
) {
    let SessionEvent::SocketGemsResult { item, enchants } = ev else {
        return;
    };
    let Some(mut store) = index.0.get(&item).and_then(|&e| stores.get_mut(e).ok()) else {
        debug!("ui_socket: socket result for {item:#x}, not held — dropped");
        return;
    };
    let pairs: Vec<(u16, u32)> = enchants
        .iter()
        .enumerate()
        .filter(|&(_, &id)| id != 0)
        .filter_map(|(i, &id)| Some((item_enchant_field(2 + i as u8)?, id)))
        .collect();
    debug!("ui_socket: socket result for {item:#x}: {enchants:?}");
    let entity = index.0[&item];
    merge_store_fields(
        &mut store.0,
        ObjectFields::from_pairs(&pairs),
        entity,
        item,
        |e| {
            edges.write(e);
        },
    );
}

/// `SMSG_ITEM_REFUND_INFO_RESPONSE` (`0x6d1650`): the record lands on a held item and clears its
/// pending bit; one for an item not held is skipped whole.
fn on_refund_info(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut records: ResMut<RefundRecords>,
) {
    let SessionEvent::ItemRefundInfo { item, record } = ev else {
        return;
    };
    if !index.0.contains_key(&item) {
        return;
    }
    debug!("ui_socket: refund record for {item:#x}: {record:?}");
    records.asked.remove(&item);
    records.records.insert(item, record);
    records.epoch += 1;
}

fn on_refund_result(In(ev): In<SessionEvent>, mut results: ResMut<RefundResults>) {
    if let SessionEvent::ItemRefundResult { item, result, cost } = ev {
        debug!("ui_socket: refund of {item:#x} → {result}");
        results.0.push((result, cost));
    }
}

/// An item's create (`0x70aa40`), where 12340 asks a refundable purchase's record.
fn on_item_create(In(ev): In<SessionEvent>, mut records: ResMut<RefundRecords>) {
    if let SessionEvent::ItemCreate { guid, .. } = ev {
        records.created.push(guid);
    }
}

/// The VM's socketing accepts and refund sends.
fn drain_item_sends(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for (item, gems) in script.take_socket_sends() {
        debug!("ui_socket: socket gems into {item:#x}: {gems:x?}");
        let _ = commands.0.send(ClientCommand::SocketGems { item, gems });
    }
    for item in script.take_refund_sends() {
        debug!("ui_socket: refund {item:#x}");
        let _ = commands.0.send(ClientCommand::ItemRefund { item });
    }
}

/// A bound item's trade-window stamp (`0x708b40`): the player's own, flagged `0x100`, with no
/// binding enchant.
fn trade_stamp(
    fields: &ObjectFields,
    me: Option<u64>,
    enchants: Option<&crate::items::Enchants>,
) -> Option<u32> {
    let flags = fields.item_flags()?;
    if flags & ITEM_FIELD_FLAG_BOP_TRADEABLE == 0 || fields.item_owner() != me {
        return None;
    }
    let enchant_binds = (0..12u8).any(|slot| {
        crate::items::live_enchant(fields, slot, enchants)
            .is_some_and(|id| crate::items::binds(id, enchants))
    });
    (!enchant_binds).then(|| fields.item_create_played_time().unwrap_or(0))
}

/// Ask each created refundable purchase's record, drop records and stamps whose items went, keep
/// the trade stamps, push all of it to the VM, and print the refund results.
#[allow(clippy::too_many_arguments)] // the param list is the input set
fn feed_refunds(
    script: Option<NonSendMut<UiScript>>,
    mut records: ResMut<RefundRecords>,
    mut results: ResMut<RefundResults>,
    mut field_edges: MessageReader<FieldChanged>,
    objects: Objects,
    items: Res<Items>,
    self_guid: Res<SelfGuid>,
    enchants: Option<Res<crate::items::Enchants>>,
    commands: Res<NetCommands>,
    player_q: Query<&ObjectStore, With<SelfPlayer>>,
    io: (ResMut<ChatLog>, ResMut<UiErrorKeys>),
    mut memo: Local<crate::ui_script::VmMemo<FeedMemo>>,
) {
    let Some(mut script) = script else {
        field_edges.clear();
        return;
    };
    let memo = memo.get(&script);
    let me = self_guid.0;
    let enchants = enchants.as_deref();
    let records = &mut *records;
    // The created items wait for their templates.
    let created = std::mem::take(&mut records.created);
    for guid in created {
        let Some(fields) = objects.object(guid) else {
            continue;
        };
        let Some(entry) = fields.object_entry() else {
            continue;
        };
        let Some(template) = items.template_cached(entry) else {
            records.created.push(guid);
            continue;
        };
        memo.trade_dirty.insert(guid);
        let refundable = template.flags & ITEM_FLAG_REFUNDABLE != 0;
        if refundable
            && fields.item_owner() == me
            && !records.records.contains_key(&guid)
            && records.asked.insert(guid)
        {
            debug!("ui_socket: ask refund record of {guid:#x}");
            let _ = commands
                .0
                .send(ClientCommand::ItemRefundInfo { item: guid });
        }
    }
    for e in field_edges.read() {
        if matches!(e.index, FIELD_ITEM_FLAGS | FIELD_ITEM_CREATE_PLAYED_TIME)
            && e.kind == benilla_protocol::messages::ObjectType::Item
        {
            memo.trade_dirty.insert(e.guid);
        }
    }
    // A record goes with its item.
    let before = records.records.len();
    records.records.retain(|g, _| objects.object(*g).is_some());
    records.asked.retain(|g| objects.object(*g).is_some());
    if records.records.len() != before {
        records.epoch += 1;
    }
    let mut trade_changed = false;
    for guid in std::mem::take(&mut memo.trade_dirty) {
        let stamp = objects
            .object(guid)
            .and_then(|f| trade_stamp(f, me, enchants));
        trade_changed |= match stamp {
            Some(s) => memo.trade.insert(guid, s) != Some(s),
            None => memo.trade.remove(&guid).is_some(),
        };
    }
    let before = memo.trade.len();
    memo.trade.retain(|g, _| objects.object(*g).is_some());
    trade_changed |= memo.trade.len() != before;
    if trade_changed {
        script.set_bound_trade_stamps(memo.trade.clone());
    }
    if memo.epoch != records.epoch {
        memo.epoch = records.epoch;
        for gone in memo
            .pushed
            .drain()
            .filter(|g| !records.records.contains_key(g))
            .collect::<Vec<_>>()
        {
            script.set_refund_record(gone, None);
        }
        for (&guid, r) in &records.records {
            script.set_refund_record(guid, Some(view_of(r)));
            memo.pushed.insert(guid);
        }
    }
    let (mut chat, mut errors) = io;
    let player = player_q.iter().next().map(|s| &s.0);
    for (result, cost) in results.0.drain(..) {
        refund_lines(
            &script,
            &items,
            player,
            result,
            cost,
            &mut chat,
            &mut errors,
        );
    }
}

/// The feed's memory: the pushed records, the trade stamps, and the items to re-read.
#[derive(Default)]
pub(crate) struct FeedMemo {
    epoch: u64,
    pushed: HashSet<u64>,
    trade: HashMap<u64, u32>,
    trade_dirty: HashSet<u64>,
}

fn view_of(r: &RefundRecord) -> RefundView {
    RefundView {
        money: r.cost.money,
        honor: r.cost.honor,
        arena: r.cost.arena,
        items: r.cost.items,
        played_at_purchase: r.played_at_purchase,
    }
}

/// `SMSG_ITEM_REFUND_RESULT`'s lines (`0x6d9b40`): a refusal shows the bag-full or currency-full
/// error, else `UNABLE_TO_REFUND_ITEM`; a success prints `ITEM_REFUND_MSG`, then the money, the
/// honor and arena points (each capped at what fits), and each cost item, as system lines.
fn refund_lines(
    script: &UiScript,
    items: &Items,
    player: Option<&ObjectFields>,
    result: u32,
    cost: Option<PurchaseCost>,
    chat: &mut ChatLog,
    errors: &mut UiErrorKeys,
) {
    let get = |key: &str| crate::ui_chat::combat::global_string(script, key);
    let mut line = |text: String| crate::ui_chat::away::push_system(chat, text);
    let Some(cost) = cost.filter(|_| result == 0) else {
        match result {
            10 => errors.0.push(UiError::key("ERR_INV_FULL")),
            11 => errors.0.push(UiError::key("ERR_CURRENCY_FULL")),
            _ => {
                if let Some(t) = get("UNABLE_TO_REFUND_ITEM") {
                    line(t);
                }
            }
        }
        warn!("Item purchase refund error code {result}");
        return;
    };
    if let Some(t) = get("ITEM_REFUND_MSG") {
        line(t);
    }
    if cost.money > 0 {
        let coins = crate::ui_quest::lines::coin_text(cost.money, &|k| get(k).unwrap_or_default());
        line(coins);
    }
    let format = get("CURRENCY_AMOUNT_REFUND_FORMAT").unwrap_or_default();
    let refund = |n: u32, name: &str| {
        benilla_ui::strings::fill(&format, &[Arg::D(i64::from(n)), Arg::S(name)])
    };
    let currencies = [
        (
            cost.honor,
            player
                .and_then(ObjectFields::player_honor_currency)
                .unwrap_or(0),
            HONOR_CAP,
            "HONOR_POINTS",
        ),
        (
            cost.arena,
            player
                .and_then(ObjectFields::player_arena_currency)
                .unwrap_or(0),
            ARENA_CAP,
            "ARENA_POINTS",
        ),
    ];
    for (amount, current, cap, key) in currencies {
        if amount == 0 {
            continue;
        }
        let shown = if current + amount <= cap {
            amount
        } else {
            cap.saturating_sub(current)
        };
        line(refund(shown, &get(key).unwrap_or_default()));
    }
    for (id, count) in cost.items {
        if count == 0 {
            continue;
        }
        let Some(t) = items.template_cached(id) else {
            continue;
        };
        line(refund(count, &t.name));
    }
}
