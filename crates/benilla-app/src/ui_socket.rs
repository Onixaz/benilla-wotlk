//! The socketing and purchase-refund round trips. The VM's socketing engine and refund verbs
//! ([`benilla_ui::script`]) queue sends; here they go out, `SMSG_SOCKET_GEMS_RESULT` writes the
//! socketed item's enchants (`0x7081b0`), the refund records (`item+0x3d4`) are asked and kept for the verbs to read at the call, and `SMSG_ITEM_REFUND_RESULT` prints its lines in its handler (`0x6d9b40`).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::field::item_enchant_field;
use benilla_protocol::messages::{ObjectFields, PurchaseCost, RefundRecord};
use benilla_protocol::{SessionEvent, SessionEventKind as K};
use benilla_ui::script::{RefundView, UiScript};
use benilla_ui::strings::Arg;

use crate::items::Items;
use crate::net::{
    merge_store_fields, ClientCommand, FieldChanged, GuidIndex, NetCommands, NetHandlerApp,
    ObjectStore, Objects,
};
use crate::ui_action::UiError;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;

/// The template flag that marks a purchase with a refund record (`template+0x18 & 0x1000`).
pub(crate) const ITEM_FLAG_REFUNDABLE: u32 = 0x1000;
/// `ITEM_FIELD_FLAGS & 0x100`: a bound item still in its trade window.
const ITEM_FIELD_FLAG_BOP_TRADEABLE: u32 = 0x100;
/// The refund lines' currency caps (`0x6d9c4f`, `0x6d9cbf`).
const HONOR_CAP: u32 = 75_000;
const ARENA_CAP: u32 = 10_000;

/// The refund records on the held items (`item+0x3d4`) and their asks; main-thread state the handlers write and a verb's call asks through ([`crate::ui_script::Lent`]).
#[derive(Default)]
pub(crate) struct RefundRecords {
    records: HashMap<u64, RefundRecord>,
    /// The pending bit (`item+0x394 & 4`): asked and unanswered; a silent server leaves it set, so the item is never asked again.
    asked: HashSet<u64>,
}

impl RefundRecords {
    /// Whether `item` holds a record whose window is open at `played_now`.
    pub(crate) fn open(&self, item: u64, played_now: i64) -> bool {
        self.records
            .get(&item)
            .is_some_and(|r| window_open(r.played_at_purchase, played_now))
    }

    /// The record on `item`, as the verbs read it.
    pub(crate) fn view(&self, item: u64) -> Option<RefundView> {
        self.records.get(&item).map(view_of)
    }

    /// Ask `item`'s record unless it holds one or is pending (`0x7089e0`, its callers' `+0x3d4` and `+0x394 & 4` tests): the pending bit is set with the send.
    pub(crate) fn ask(&mut self, item: u64, commands: &NetCommands) {
        if self.records.contains_key(&item) || !self.asked.insert(item) {
            return;
        }
        debug!("ui_socket: ask refund record of {item:#x}");
        let _ = commands.0.send(ClientCommand::ItemRefundInfo { item });
    }

    /// The item went (`0x70afc0`): its record and pending bit go with it.
    fn forget(&mut self, item: u64) {
        self.records.remove(&item);
        self.asked.remove(&item);
    }
}

/// Whether `entry`'s cached template is a refundable purchase (`template+0x18 & 0x1000`); `false` while it is uncached, as `0x707360` answers null.
pub(crate) fn refundable(items: &Items, entry: u32) -> bool {
    items
        .template_cached(entry)
        .is_some_and(|t| t.flags & ITEM_FLAG_REFUNDABLE != 0)
}

/// A two-hour played-time window from `stamp` is open (`stamp - played + 7200 > 0`).
fn window_open(stamp: u32, played_now: i64) -> bool {
    i64::from(stamp) - played_now + 7200 > 0
}

/// Bound but still tradeable (`0x709550`, `0x708b40`): bound by its flag, the player's own, flagged
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

/// A weapon imbue held at the trade-window confirm (`0x80cafe`): the spell and the bound item it
/// would end the window of, and whether `END_BOUND_TRADEABLE` is still to fire.
#[derive(Resource, Default)]
pub(crate) struct SpellEnchantConfirm {
    pub(crate) parked: Option<(u32, u64)>,
    pub(crate) raise: bool,
}

pub(crate) struct UiSocketPlugin;

impl Plugin for UiSocketPlugin {
    fn build(&self, app: &mut App) {
        crate::item_names::register(app);
        app.init_non_send_resource::<RefundRecords>()
            .init_resource::<SpellEnchantConfirm>()
            .net_handler(K::SocketGemsResult, on_socket_gems_result)
            .net_handler(K::ItemRefundInfo, on_refund_info)
            .net_handler(K::ItemRefundResult, on_refund_result)
            .net_handler(K::VendorInventory, on_vendor_inventory)
            .net_handler(K::ObjectDestroyed, on_object_destroyed)
            .net_handler(K::Disconnected, on_session_end)
            .add_systems(Update, drain_item_sends.in_set(UiDrain));
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
/// pending bit, and each cost item's record is asked (`0x6d172b`); one for an item not held is skipped whole.
fn on_refund_info(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    records: Option<NonSendMut<RefundRecords>>,
    items: Res<Items>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::ItemRefundInfo { item, record } = ev else {
        return;
    };
    let Some(mut records) = records else {
        return;
    };
    if !index.0.contains_key(&item) {
        return;
    }
    debug!("ui_socket: refund record for {item:#x}: {record:?}");
    for &(id, _) in &record.cost.items {
        if id != 0 {
            let _ = items.template(id, 0, &commands);
        }
    }
    records.asked.remove(&item);
    records.records.insert(item, record);
}

/// `SMSG_ITEM_REFUND_RESULT` (`0x6d9b40`): its lines print in the handler, with the VM's strings.
fn on_refund_result(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    let SessionEvent::ItemRefundResult { item, result, cost } = ev else {
        return;
    };
    debug!("ui_socket: refund of {item:#x} → {result}");
    ui.world(move |world| refund_result(world, result, cost));
}

/// `SMSG_LIST_INVENTORY`'s ask of every held refundable purchase without a record (`0x6d9ae1` -> `0x6d1760`): the worn and bag slots 0-18, the pack 23-38 and the four bags' contents.
fn on_vendor_inventory(
    In(ev): In<SessionEvent>,
    objects: Objects,
    items: Res<Items>,
    records: Option<NonSendMut<RefundRecords>>,
    commands: Res<NetCommands>,
) {
    if !matches!(ev, SessionEvent::VendorInventory { .. }) {
        return;
    }
    let (Some(mut records), Some(me)) = (records, objects.player_object()) else {
        return;
    };
    let mut held: Vec<u64> = (0..=18u8)
        .filter_map(|slot| me.player_inv_slot(slot))
        .chain((0..16u8).filter_map(|i| me.player_pack_slot(i)))
        .collect();
    for bag in (19..=22u8).filter_map(|slot| me.player_inv_slot(slot)) {
        let Some(bag) = objects.object(bag) else {
            continue;
        };
        let n = bag.container_num_slots().unwrap_or(0).min(36) as u8;
        held.extend((0..n).filter_map(|j| bag.container_slot(j)));
    }
    for item in held.into_iter().filter(|&g| g != 0) {
        let entry = objects.object(item).and_then(ObjectFields::object_entry);
        if entry.is_some_and(|e| refundable(&items, e)) {
            records.ask(item, &commands);
        }
    }
}

/// An item's destroy (`0x70afc0`): the record lives on the item object, so it goes with it.
fn on_object_destroyed(In(ev): In<SessionEvent>, records: Option<NonSendMut<RefundRecords>>) {
    if let (SessionEvent::ObjectDestroyed(guid), Some(mut records)) = (ev, records) {
        records.forget(guid);
    }
}

/// A session's end drops every item, and the records on them.
fn on_session_end(In(_): In<SessionEvent>, records: Option<NonSendMut<RefundRecords>>) {
    if let Some(mut records) = records {
        *records = RefundRecords::default();
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
pub(crate) fn trade_stamp(
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

fn view_of(r: &RefundRecord) -> RefundView {
    RefundView {
        money: r.cost.money,
        honor: r.cost.honor,
        arena: r.cost.arena,
        items: r.cost.items,
        played_at_purchase: r.played_at_purchase,
    }
}

/// The result's lines with the `World` lent (`0x6d9b40`): the VM's strings, the player's points and the cost items' cached names.
fn refund_result(world: &mut World, result: u32, cost: Option<PurchaseCost>) {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    let player = world
        .resource::<crate::net::SelfGuid>()
        .0
        .and_then(|g| world.resource::<GuidIndex>().0.get(&g).copied())
        .and_then(|e| world.get::<ObjectStore>(e))
        .map(|s| s.0.clone());
    let names: HashMap<u32, String> = cost
        .as_ref()
        .map(|c| {
            let items = world.resource::<Items>();
            c.items
                .iter()
                .filter_map(|&(id, _)| Some((id, items.template_cached(id)?.name.clone())))
                .collect()
        })
        .unwrap_or_default();
    crate::ui_script::live::with_live_world(world, |script| {
        refund_lines(
            script,
            &names,
            player.as_ref(),
            result,
            cost,
            &mut |t| lines.push(t),
            &mut errors,
        );
    });
    // Each line in the handler's turn (`0x5216f0`; the system lines through `0x509dd0`).
    for line in lines {
        crate::ui_chat::add_chat_line_now(world, 0, line);
    }
    for error in errors {
        crate::ui_action::show_line_in_world(world, "ui_socket", error);
    }
}

/// `SMSG_ITEM_REFUND_RESULT`'s lines (`0x6d9b40`): a refusal shows the bag-full or currency-full
/// error, else `UNABLE_TO_REFUND_ITEM`; a success prints `ITEM_REFUND_MSG`, then the money, the
/// honor and arena points (each capped at what fits), and each cost item, as system lines.
fn refund_lines(
    script: &UiScript,
    names: &HashMap<u32, String>,
    player: Option<&ObjectFields>,
    result: u32,
    cost: Option<PurchaseCost>,
    line: &mut dyn FnMut(String),
    errors: &mut Vec<UiError>,
) {
    let get = |key: &str| crate::ui_chat::combat::global_string(script, key);
    let Some(cost) = cost.filter(|_| result == 0) else {
        match result {
            10 => errors.push(UiError::key("ERR_INV_FULL")),
            11 => errors.push(UiError::key("ERR_CURRENCY_FULL")),
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
        let Some(name) = names.get(&id) else {
            continue;
        };
        line(refund(count, name));
    }
}
