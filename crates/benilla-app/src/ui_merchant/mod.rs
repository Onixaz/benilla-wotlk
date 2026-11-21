//! The merchant window's app side: [`MerchantOpen`] holds the merchant globals, the handlers
//! write them and fire `MERCHANT_SHOW`, `MERCHANT_UPDATE` and `MERCHANT_CLOSED` in their turn
//! (`0x584500`, `0x5840e0`, `0x584600`), and [`MerchantLive`] lends them to the verbs at their
//! call. A bag click sells through [`crate::ui_items`].

use benilla_protocol::messages::{buy_result, sell_result, VendorItem};
use benilla_world::interact::WorldRightPress;
use bevy::ecs::message::Messages;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{
    CostItem, ExtendedCost, ItemStatsHead, MerchantAct, MerchantItem, MerchantList,
    MerchantOutcome, RepairAt, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_items::{item_link, slot_guid, wire_pos};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;
use benilla_protocol::messages::BAG_PLAYER_INVENTORY;

/// A vendor row's unlimited `current_count`, `-1` to Lua.
const STOCK_UNLIMITED: u32 = 0xFFFF_FFFF;

/// `UNIT_NPC_FLAG_REPAIR`, the bit `CanMerchantRepair` tests (`0x5845b0`).
const NPC_FLAG_REPAIR: u32 = 0x1000;

/// The first absolute buyback slot, 74-85 (`0x584bb0` stores `0x4b - 1`).
const BUYBACK_SLOT_FIRST: u32 = 74;

/// `DurabilityCosts.dbc` and `DurabilityQuality.dbc`, the client's repair prices; absent, every
/// cost shows 0.
#[derive(Resource)]
pub(crate) struct RepairTables(pub(crate) benilla_formats::DurabilityTables);

/// `ItemExtendedCost.dbc`, a row's costs beyond copper; absent, no row has an extended cost.
#[derive(Resource)]
pub(crate) struct ExtendedCosts(pub(crate) benilla_formats::ItemExtendedCosts);

/// The merchant globals: the vendor (`0xbfa3e8`), its rows (`0xbf9128`, count `0xbfa3f0`), the record misses its rows' reads left waiting (`0xbfa3f8`) and the sorted buyback slots (`0xbf90f8`, count `0xbfa3f4`).
#[derive(Resource, Default)]
pub(crate) struct MerchantOpen {
    pub(crate) vendor: Option<u64>,
    /// In wire order, which is the 1-based display order.
    pub(crate) items: Vec<VendorItem>,
    pending_stats: u32,
    /// The occupied buyback slots (0-11) by sale time, oldest first.
    buyback: Vec<u8>,
}

impl MerchantOpen {
    /// A test's vendor, set with no events.
    #[cfg(test)]
    pub(crate) fn open(&mut self, vendor: u64, items: Vec<VendorItem>) {
        self.vendor = Some(vendor);
        self.items = items;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.vendor.is_some()
    }

    /// The item entry and the row's own slot at a 0-based row, the pair `CMSG_BUY_ITEM` names (`0x584080`).
    fn row_at(&self, row: u32) -> Option<&VendorItem> {
        self.vendor?;
        self.items.get(row as usize)
    }

    /// A test's clear, with no events.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.vendor = None;
        self.items.clear();
    }
}

/// The range guard and the session-closing legs read the vendor as the windows' holder.
impl NpcSession for MerchantOpen {
    fn npc(&self) -> Option<u64> {
        self.vendor
    }

    /// The range guard's zero, its events fired by [`close_merchant`].
    fn close(&mut self) {
        self.vendor = None;
    }
}

mod net;

pub(crate) struct UiMerchantPlugin;

impl Plugin for UiMerchantPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<MerchantOpen>();
    }
}

/// The vendor window's open (`0x584500`): every row's record callback cancelled (`0x584525`), the vendors tutorial (`0x584537`), the interact target taken, which closes the window holding it (`0x584540` -> `0x51fce0`), then the vendor and its rows stored, the pending count zeroed and `MERCHANT_SHOW` fired with no argument (`0x584596`).
pub(crate) fn set_merchant(world: &mut World, vendor: u64, items: Vec<VendorItem>) {
    if let Some(mut waits) = world.get_non_send_resource_mut::<crate::items::RecordWaits>() {
        waits.cancel(|w| w == crate::items::RecordWait::Merchant);
    }
    if let Some(mut asks) = world.get_resource_mut::<Messages<crate::tutorial::TutorialEvent>>() {
        asks.write(crate::tutorial::TutorialEvent::trigger(
            crate::tutorial::id::VENDORS,
        ));
    }
    crate::ui_session::set_interact_target(world, vendor);
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut m) = w.get_resource_mut::<MerchantOpen>() {
            m.vendor = Some(vendor);
            m.items = items;
            m.pending_stats = 0;
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.fire("MERCHANT_SHOW", vec![]);
        }
    });
}

/// `CloseMerchant` (`0x584600`): with a vendor set, the interact target cleared on it, whose interaction close runs this leg first when the vendor held it (`0x584614` -> `0x512f3a`), then the vendor and rows zeroed and `MERCHANT_CLOSED` fired (`0x58463e`), then the held vendor row and repair mode cleared (`0x584646`-`0x58467c`).
pub(crate) fn close_merchant(world: &mut World) {
    let Some(vendor) = world.get_resource::<MerchantOpen>().and_then(|m| m.vendor) else {
        return;
    };
    crate::ui_session::clear_interact_target(world, vendor);
    zero_and_fire(world);
}

/// The close's tail: zero, `MERCHANT_CLOSED`, then its cursor legs.
fn zero_and_fire(world: &mut World) {
    if let Some(mut m) = world.get_resource_mut::<MerchantOpen>() {
        m.vendor = None;
        m.items.clear();
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("MERCHANT_CLOSED", vec![]);
        ui.write(UiScript::merchant_closed);
    }
}

/// The interaction close's merchant leg with the interact target already zeroed (`0x512f3a`): the close, whose own clear then finds no target.
pub(crate) fn close_by_interaction(world: &mut World) {
    if world
        .get_resource::<MerchantOpen>()
        .is_some_and(MerchantOpen::is_open)
    {
        zero_and_fire(world);
    }
}

/// The vendor row's count update (`0x5840e0`): for the open vendor, the first row on `slot` takes `count`, and `MERCHANT_UPDATE` fires whether or not a row matched (`0x58411b`, `0x584138`).
pub(crate) fn update_item_quantity(world: &mut World, vendor: u64, slot: u32, count: u32) {
    let Some(mut m) = world.get_resource_mut::<MerchantOpen>() else {
        return;
    };
    if m.vendor != Some(vendor) {
        return;
    }
    if let Some(row) = m.items.iter_mut().find(|r| r.slot == slot) {
        row.current_count = count;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("MERCHANT_UPDATE", vec![]);
    }
}

/// The vendor list's refresh (`0x5840c0`), the skill-rank and guild-id watches' leg (`0x6da602`, `0x6e1bf5`): `MERCHANT_UPDATE` with a vendor set.
pub(crate) fn refresh(world: &mut World) {
    if world
        .get_resource::<MerchantOpen>()
        .is_some_and(MerchantOpen::is_open)
    {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("MERCHANT_UPDATE", vec![]);
        }
    }
}

/// The rows' record callback (`0x5844c0`): one fewer miss pending, and `MERCHANT_UPDATE` with a vendor set once none is (`0x5844e9`).
pub(crate) fn record_callback(world: &mut World) {
    let Some(mut m) = world.get_resource_mut::<MerchantOpen>() else {
        return;
    };
    if m.pending_stats != 0 {
        m.pending_stats -= 1;
        if m.pending_stats != 0 {
            return;
        }
    }
    if m.vendor.is_some() {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("MERCHANT_UPDATE", vec![]);
        }
    }
}

/// The buyback rows' record callback (`0x5840b0`): `MERCHANT_UPDATE`, vendor or none.
pub(crate) fn buyback_record_callback(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("MERCHANT_UPDATE", vec![]);
    }
}

/// The buyback fields' watch (`0x585f60`): the slots re-sorted (`0x584bb0`), then `MERCHANT_UPDATE`, vendor or none.
pub(crate) fn buyback_changed(world: &mut World) {
    sort_buyback(world);
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("MERCHANT_UPDATE", vec![]);
    }
}

/// The merchant's world entry (`0x585f80`): the vendor and rows zeroed, the buyback slots sorted.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut m) = world.get_resource_mut::<MerchantOpen>() {
        m.vendor = None;
        m.items.clear();
    }
    sort_buyback(world);
}

/// The merchant's world leave (`0x585fe0`): the close.
pub(crate) fn leave_world(world: &mut World) {
    close_merchant(world);
}

/// `0x584bb0`: the slots holding an item with a price, by their timestamp, oldest first; none without our player.
fn sort_buyback(world: &mut World) {
    let order = world
        .query_filtered::<&ObjectStore, With<SelfPlayer>>()
        .iter(world)
        .next()
        .map(|s| buyback_order(&s.0))
        .unwrap_or_default();
    if let Some(mut m) = world.get_resource_mut::<MerchantOpen>() {
        m.buyback = order;
    }
}

/// The one `BuyResult` code the reference shows nothing for: `0x6cdba0`'s switch sends 6 to the
/// handler's `ret`.
const BUY_SILENT: u8 = 6;

/// The message a `SMSG_BUY_FAILED` code shows, from 12340's switch at `0x6cdc23`; its default arm
/// shows `ERR_ITEM_NOT_FOUND` for 0, 3, 9, 10 and 13 up.
fn buy_error_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        // One arm for both; only code 1 also zeroes the row.
        buy_result::ITEM_ALREADY_SOLD | buy_result::ITEM_SOLD_OUT => "ERR_VENDOR_SOLD_OUT", // 0x23
        buy_result::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // 0x25, speaks line 0x28
        buy_result::SELLER_DONT_LIKE_YOU => "ERR_VENDOR_HATES_YOU", // 0x22
        buy_result::DISTANCE_TOO_FAR => "ERR_VENDOR_TOO_FAR",   // 0x24
        BUY_SILENT => return None,
        buy_result::CANT_CARRY_MORE => "ERR_ITEM_MAX_COUNT", // 0x12, speaks line 0x1e
        buy_result::RANK_REQUIRE => "ERR_CANT_EQUIP_RANK",   // 0x05
        buy_result::REPUTATION_REQUIRE => "ERR_CANT_EQUIP_REPUTATION", // 0x06
        _ => "ERR_ITEM_NOT_FOUND",                           // 0x17, the switch's default
    })
}

/// The message a `SMSG_SELL_ITEM` error code shows, from 12340's switch at `0x6d157c`, which hands
/// the row's text to the error display (`0x6d15f7`) while the vendor's unit exists. Its default
/// is silence: 0 returns before the switch, and 5 and 10 up skip the message.
fn sell_error_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        sell_result::CANT_FIND_ITEM => "ERR_ITEM_NOT_FOUND", // 0x17
        sell_result::CANT_SELL_ITEM => "ERR_VENDOR_NOT_INTERESTED", // 0x21
        sell_result::CANT_FIND_VENDOR => "ERR_VENDOR_HATES_YOU", // 0x22
        sell_result::YOU_DONT_OWN_THAT_ITEM => "ERR_NOT_OWNER", // 0x1b
        sell_result::ONLY_EMPTY_BAG => "ERR_DESTROY_NONEMPTY_BAG", // 0x0b
        sell_result::CANT_SELL_TO_THIS_MERCHANT => "ERR_VENDOR_DOESNT_BUY",
        sell_result::MUST_REPAIR_DURABILITY => "ERR_MUST_REPAIR_DURABILITY",
        sell_result::INTERNAL_BAG_ERROR => "ERR_INTERNAL_BAG_ERROR",
        _ => return None,
    })
}

/// An item's icon and link once its template answers.
fn icon_and_link(
    entry: u32,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> (Option<String>, Option<String>) {
    let template = items.template(entry, 0, commands);
    let texture = template.and_then(|t| crate::ui_items::item_icon(icons, t.display_info_id));
    (
        texture,
        template.map(|t| item_link(entry, &t.name, t.quality)),
    )
}

/// A row's `ItemExtendedCost.dbc` record, its required items resolved; the VM tests its rating
/// demand against the arena teams it holds.
fn resolve_extended_cost(
    id: u32,
    costs: Option<&ExtendedCosts>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<ExtendedCost> {
    let row = costs?.0.get(id)?;
    let items = row
        .items
        .iter()
        .filter(|(item, _)| *item != 0)
        .map(|&(item_id, count)| {
            let (texture, link) = icon_and_link(item_id, items, icons, commands);
            CostItem {
                item_id,
                count,
                texture,
                link,
            }
        })
        .collect();
    Some(ExtendedCost {
        honor: row.honor,
        arena: row.arena,
        items,
        required_rating: row.personal_rating,
        arena_bracket: row.arena_slot as i32,
    })
}

/// One wire row as a Lua row: the icon from the wire `display_id` at once, the name and stats once
/// the item template answers.
fn resolve_item(
    item: &VendorItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    costs: Option<&ExtendedCosts>,
    commands: &NetCommands,
) -> MerchantItem {
    let template = items.template(item.entry, 0, commands);
    let name = template.map(|t| t.name.clone());
    let stats = template.map(|t| ItemStatsHead {
        quality: t.quality,
        inventory_type: t.inventory_type,
        class: t.class,
        subclass: t.subclass,
        dmg_min: t.dmg_min,
        dmg_max: t.dmg_max,
        dmg_type: t.dmg_type,
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        sell_price: t.sell_price,
    });
    let texture = crate::ui_items::item_icon(icons, item.display_id);
    let num_available = if item.current_count == STOCK_UNLIMITED {
        -1
    } else {
        item.current_count as i32
    };
    // `GetMerchantItemLink`'s link; a vendor row has no enchant or random property.
    let link = template.map(|t| item_link(item.entry, &t.name, t.quality));
    MerchantItem {
        name,
        texture,
        price: item.price,
        quantity: item.buy_count,
        num_available,
        item_id: item.entry,
        stats,
        link,
        max_stack: template.map(|t| t.stackable),
        extended_cost: resolve_extended_cost(item.extended_cost, costs, items, icons, commands),
    }
}

/// The occupied buyback slots (0-11) in the reference's display order, oldest first: `0x584bb0`
/// takes the slots with a guid and a price and sorts them by timestamp. Entry `i` is
/// `GetBuybackItemInfo(i + 1)`.
fn buyback_order(store: &benilla_protocol::ObjectFields) -> Vec<u8> {
    let mut v: Vec<(u8, u32)> = (0..12u8)
        .filter_map(|i| {
            let guid = store.player_buyback_slot(i).unwrap_or(0);
            let price = store.player_buyback_price(i).unwrap_or(0);
            (guid != 0 && price != 0).then(|| (i, store.player_buyback_timestamp(i).unwrap_or(0)))
        })
        .collect();
    v.sort_by_key(|&(_, ts)| ts);
    v.into_iter().map(|(i, _)| i).collect()
}

/// One buyback slot as a Lua row: the item from its object, which stays streamed while it sits in
/// the slot, and the price from the player's buyback price field.
fn resolve_buyback(
    idx: u8,
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> MerchantItem {
    let guid = store.player_buyback_slot(idx).unwrap_or(0);
    let obj = objects.object(guid);
    let entry = obj.and_then(|o| o.object_entry()).unwrap_or(0);
    let quantity = obj.and_then(|o| o.item_stack_count()).unwrap_or(1).max(1);
    let template = items.template(entry, guid, commands);
    let name = template.map(|t| t.name.clone());
    let stats = template.map(|t| ItemStatsHead {
        quality: t.quality,
        inventory_type: t.inventory_type,
        class: t.class,
        subclass: t.subclass,
        dmg_min: t.dmg_min,
        dmg_max: t.dmg_max,
        dmg_type: t.dmg_type,
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        sell_price: t.sell_price,
    });
    let display_id = template.map(|t| t.display_info_id).unwrap_or(0);
    let texture = crate::ui_items::item_icon(icons, display_id);
    MerchantItem {
        name,
        texture,
        price: store.player_buyback_price(idx).unwrap_or(0),
        quantity,
        num_available: 0,
        item_id: entry,
        stats,
        link: template.zip(obj).map(|(t, o)| {
            let fields = crate::ui_items::object_link_fields(o, t.flags);
            crate::ui_items::item_link_full(entry, &fields, &t.name, t.quality)
        }),
        // No max stack: `GetMerchantItemMaxStack` indexes the merchant list, and a buyback row
        // is bought whole.
        max_stack: None,
        extended_cost: None,
    }
}

/// One item's repair cost, by the reference's repair-cost arithmetic (`0x584b20`,
/// [`benilla_formats::DurabilityTables`]), after the price `discount`.
fn item_repair_cost(
    guid: u64,
    objects: &Objects,
    items: &Items,
    tables: &RepairTables,
    commands: &NetCommands,
    discount: f64,
) -> u32 {
    let Some(obj) = objects.object(guid) else {
        return 0;
    };
    let cur = obj.item_durability().unwrap_or(0);
    let max = obj.item_max_durability().unwrap_or(0);
    let entry = obj.object_entry().unwrap_or(0);
    if max == 0 || cur >= max || entry == 0 {
        return 0;
    }
    let points = max - cur;
    let Some(t) = items.template(entry, guid, commands) else {
        return 0;
    };
    let (level, quality, class, subclass) = (t.item_level, t.quality, t.class, t.subclass);
    tables
        .0
        .repair_cost(points, level, quality, class, subclass, discount)
}

/// The guids `RepairAllItems` (`0x585c90`) and `GetRepairAllCost` (`0x585990`) sweep: equipment
/// 0-18, the backpack 23-38 and the contents of bags 19-22, never the bank.
fn carried_item_guids(store: &benilla_protocol::ObjectFields, objects: &Objects) -> Vec<u64> {
    let mut guids: Vec<u64> = (0..19u8)
        .filter_map(|i| store.player_inv_slot(i))
        .chain((0..16u8).filter_map(|i| store.player_pack_slot(i)))
        .collect();
    for b in 1..=4u8 {
        let Some(bag) = store
            .player_inv_slot(18 + b)
            .filter(|g| *g != 0)
            .and_then(|g| objects.object(g))
        else {
            continue;
        };
        let n = bag.container_num_slots().unwrap_or(0).min(36) as u8;
        guids.extend((0..n).filter_map(|j| bag.container_slot(j)));
    }
    guids.retain(|g| *g != 0);
    guids
}

/// `ITEM_FIELD_FLAGS`' wrapped bit, which the repair sweeps skip (`0x585a64`).
const ITEM_FLAG_WRAPPED: u32 = 0x8;

/// An item the repair sweeps price: held, not wrapped, with a maximum durability (`0x585a64`).
fn swept<'a>(guid: u64, objects: &'a Objects) -> Option<&'a benilla_protocol::ObjectFields> {
    objects.object(guid).filter(|o| {
        o.item_flags().unwrap_or(0) & ITEM_FLAG_WRAPPED == 0
            && o.item_max_durability().unwrap_or(0) != 0
    })
}

/// The vendor's unit, `None` with no vendor or its unit not held (`0x584b20`'s lookup).
fn vendor_store<'a>(
    vendor: Option<u64>,
    units: &'a Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
) -> Option<&'a ObjectStore> {
    vendor
        .and_then(|g| units.iter().find(|(guid, _)| guid.0 == g))
        .map(|(_, store)| store)
}

/// The price discount `0x584b20` takes off with our player and the vendor's unit held, else none.
fn discount(
    vendor: Option<u64>,
    player: &ObjectStore,
    units: &Query<(&Guid, &ObjectStore), Without<SelfPlayer>>,
    reactions: &crate::target::ReactionInputs,
) -> f64 {
    vendor_store(vendor, units).map_or(0.0, |vendor| {
        crate::target::vendor_price_discount(
            reactions.factions.as_deref(),
            &reactions.reputations,
            reactions.party(),
            vendor,
            player,
        )
    })
}

/// What the purse test before a repair's send reads: the items' costs, the vendor for its price
/// discount, and the message path a refusal takes.
#[derive(SystemParam)]
pub(crate) struct RepairPurse<'w, 's> {
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
    tables: Option<Res<'w, RepairTables>>,
    units: Query<'w, 's, (&'static Guid, &'static ObjectStore), Without<SelfPlayer>>,
    reactions: crate::target::ReactionInputs<'w>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl RepairPurse<'_, '_> {
    /// The purse test of the item-click repair arms (`0x584b20` over `guids`): the costs, summed in
    /// a u32, above `PLAYER_FIELD_COINAGE` (unsigned, so an equal cost passes) raise
    /// `ERR_NOT_ENOUGH_MONEY` and answer `true`, and the caller sends nothing.
    pub(crate) fn refuses(
        &mut self,
        script: &mut UiScript,
        vendor: Option<u64>,
        player: &ObjectStore,
        guids: impl IntoIterator<Item = u64>,
    ) -> bool {
        let discount = discount(vendor, player, &self.units, &self.reactions);
        let cost = guids.into_iter().filter(|&g| g != 0).fold(0u32, |sum, g| {
            let item = self.tables.as_deref().map_or(0, |t| {
                item_repair_cost(g, &self.objects, &self.items, t, &self.commands, discount)
            });
            sum.wrapping_add(item)
        });
        let money = player.0.player_money().unwrap_or(0);
        if cost <= money {
            return false;
        }
        debug!("ui_merchant: repair of {cost} copper refused with {money} in the purse");
        if let Some(line) = crate::ui_action::keyed_line(script, "ERR_NOT_ENOUGH_MONEY") {
            crate::ui_action::show_messages(script, &mut self.sink, "ui_merchant", [line]);
        }
        true
    }
}

/// The merchant globals and what a row, a cost and a send read at the call.
#[derive(SystemParam)]
pub(crate) struct MerchantLive<'w, 's> {
    open: ResMut<'w, MerchantOpen>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    costs: Option<Res<'w, ExtendedCosts>>,
    tables: Option<Res<'w, RepairTables>>,
    commands: Res<'w, NetCommands>,
    units: Query<'w, 's, (&'static Guid, &'static ObjectStore), Without<SelfPlayer>>,
    me: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    reactions: crate::target::ReactionInputs<'w>,
}

impl MerchantLive<'_, '_> {
    /// Whether a vendor is set (`0xbfa3e8`).
    pub(crate) fn open(&self) -> bool {
        self.open.is_open()
    }

    /// `list`'s count: the vendor's rows (`0x5841d0`), or the sorted buyback slots with a vendor set (`0x5843f0`).
    pub(crate) fn len(&self, list: MerchantList) -> usize {
        match list {
            MerchantList::Items => self.open.items.len(),
            MerchantList::Buyback if self.open.is_open() => self.open.buyback.len(),
            MerchantList::Buyback => 0,
        }
    }

    /// Row `index` of `list` resolved at the call. A vendor row read with `ask` on a record miss counts one more pending and leaves the rows' callback (`0x584680` -> `0x5846bd`); a buyback row's miss leaves its own (`0x58515d`).
    pub(crate) fn row(
        &mut self,
        list: MerchantList,
        index: usize,
        ask: bool,
        waits: Option<&mut crate::items::RecordWaits>,
    ) -> Option<MerchantItem> {
        match list {
            MerchantList::Items => {
                let row = *self.open.row_at(u32::try_from(index).ok()?)?;
                if row.entry == 0 {
                    return None;
                }
                if ask && self.items.template_cached(row.entry).is_none() {
                    self.open.pending_stats += 1;
                    if let Some(w) = waits {
                        w.wait(row.entry, crate::items::RecordWait::Merchant, false);
                    }
                }
                Some(resolve_item(
                    &row,
                    &self.items,
                    self.icons.as_deref(),
                    self.costs.as_deref(),
                    &self.commands,
                ))
            }
            MerchantList::Buyback => {
                let &idx = self.open.buyback.get(index)?;
                let (me, _) = self.me.iter().next()?;
                let guid = me.0.player_buyback_slot(idx).filter(|&g| g != 0)?;
                let entry = self.objects.object(guid)?.object_entry().unwrap_or(0);
                if ask && self.items.template_cached(entry).is_none() {
                    if let Some(w) = waits {
                        w.wait(entry, crate::items::RecordWait::Buyback, false);
                    }
                }
                Some(resolve_buyback(
                    idx,
                    &me.0,
                    &self.objects,
                    &self.items,
                    self.icons.as_deref(),
                    &self.commands,
                ))
            }
        }
    }

    /// The vendor's unit held with the repair bit (`0x5845b0`).
    pub(crate) fn can_repair(&self) -> bool {
        vendor_store(self.open.vendor, &self.units)
            .is_some_and(|v| v.0.unit_npc_flags() & NPC_FLAG_REPAIR != 0)
    }

    /// `GetRepairAllCost`'s sweep (`0x585990`): the worn and carried items' costs summed in a u32 and whether one is below its maximum; `None` with no repairer or no player.
    pub(crate) fn repair_all_cost(&self) -> Option<(u32, bool)> {
        if !self.can_repair() {
            return None;
        }
        let (me, _) = self.me.iter().next()?;
        let discount = discount(self.open.vendor, me, &self.units, &self.reactions);
        let mut cost = 0u32;
        let mut damaged = false;
        for guid in carried_item_guids(&me.0, &self.objects) {
            let Some(item) = swept(guid, &self.objects) else {
                continue;
            };
            cost = cost.wrapping_add(self.item_cost(guid, discount));
            damaged |=
                item.item_durability().unwrap_or(0) < item.item_max_durability().unwrap_or(0);
        }
        Some((cost, damaged))
    }

    /// The item at `at`'s cost after the vendor's discount (`0x584b20`), 0 for none.
    pub(crate) fn repair_cost(&self, at: RepairAt) -> u32 {
        let Some((me, _)) = self.me.iter().next() else {
            return 0;
        };
        let guid = match at {
            RepairAt::Worn(id) => id
                .checked_sub(1)
                .and_then(|s| u8::try_from(s).ok())
                .and_then(|s| me.0.player_inv_slot(s)),
            RepairAt::Bag(bag, slot) => u8::try_from(slot.saturating_sub(1))
                .ok()
                .and_then(|s| slot_guid(&me.0, bag, s, &self.objects)),
        };
        let Some(guid) = guid.filter(|&g| g != 0) else {
            return 0;
        };
        self.item_cost(
            guid,
            discount(self.open.vendor, me, &self.units, &self.reactions),
        )
    }

    fn item_cost(&self, guid: u64, discount: f64) -> u32 {
        self.tables.as_deref().map_or(0, |t| {
            item_repair_cost(
                guid,
                &self.objects,
                &self.items,
                t,
                &self.commands,
                discount,
            )
        })
    }

    /// A merchant verb's act inside its call; `target` is the interact target the close clears.
    pub(crate) fn act(
        &mut self,
        act: MerchantAct,
        target: Option<&mut crate::ui_session::InteractTarget>,
    ) -> MerchantOutcome {
        let mut out = MerchantOutcome::default();
        let me = self.me.iter().next().map(|(s, g)| (s.0.clone(), g.0));
        match act {
            // `0x5854c0`: our player, a vendor and a row with a slot.
            MerchantAct::Buy { row, count } => {
                let (Some(vendor), Some(_)) = (self.open.vendor, me) else {
                    return out;
                };
                let Some(r) = self.open.row_at(row).filter(|r| r.slot != 0) else {
                    return out;
                };
                debug!(
                    "ui_merchant: buy row {row} (entry {}, slot {}, ×{count})",
                    r.entry, r.slot
                );
                let _ = self.commands.0.send(ClientCommand::BuyItem {
                    vendor,
                    entry: r.entry,
                    slot: r.slot,
                    count,
                });
            }
            // `0x5855c0`: a vendor, our player and an item in the sorted slot.
            MerchantAct::Buyback { index } => {
                let (Some(vendor), Some((me, _))) = (self.open.vendor, me) else {
                    return out;
                };
                let Some(&idx) = self.open.buyback.get(index as usize) else {
                    return out;
                };
                if me.player_buyback_slot(idx).unwrap_or(0) == 0 {
                    return out;
                }
                let slot = BUYBACK_SLOT_FIRST + u32::from(idx);
                debug!("ui_merchant: buyback index {index} → absolute slot {slot}");
                let _ = self
                    .commands
                    .0
                    .send(ClientCommand::BuybackItem { vendor, slot });
            }
            // `0x585c90`: a repairer and our player; the carried gear's costs over the purse refuse unless the guild bank pays (`0x585e7d`).
            MerchantAct::RepairAll { guild_bank } => {
                let (true, Some(vendor), Some((me, _))) = (self.can_repair(), self.open.vendor, me)
                else {
                    return out;
                };
                let player = self.me.iter().next().map(|(s, _)| s);
                let discount = player.map_or(0.0, |p| {
                    discount(self.open.vendor, p, &self.units, &self.reactions)
                });
                let cost = carried_item_guids(&me, &self.objects)
                    .into_iter()
                    .filter(|&g| swept(g, &self.objects).is_some())
                    .fold(0u32, |sum, g| sum.wrapping_add(self.item_cost(g, discount)));
                if !guild_bank && cost > me.player_money().unwrap_or(0) {
                    debug!("ui_merchant: repair all of {cost} refused");
                    out.error = Some("ERR_NOT_ENOUGH_MONEY");
                    return out;
                }
                debug!("ui_merchant: repair all (guild bank {guild_bank})");
                let _ = self.commands.0.send(ClientCommand::RepairItem {
                    vendor,
                    item_guid: 0,
                    guild_bank,
                });
            }
            // The sell fork (`0x585418`): our player, the held item sold to the vendor set, none included, with no stack split.
            MerchantAct::Sell { bag, slot } => {
                let Some((me, _)) = me else {
                    return out;
                };
                let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
                match slot_guid(&me, bag, slot0, &self.objects) {
                    Some(item_guid) => {
                        debug!(
                            "ui_merchant: cursor sell bag {bag} slot {slot} (item {item_guid:#x})"
                        );
                        let _ = self.commands.0.send(ClientCommand::SellItem {
                            vendor: self.open.vendor.unwrap_or(0),
                            item_guid,
                            count: 0,
                        });
                    }
                    None => debug!("ui_merchant: cursor sell on an empty slot ({bag}, {slot})"),
                }
            }
            // A held row dropped into a slot: `CMSG_BUY_ITEM_IN_SLOT` with count 1, the reference's fixed count on all three drop paths.
            MerchantAct::BuyInSlot { bag, slot, row } => {
                let (Some(vendor), Some((me, me_guid))) = (self.open.vendor, me) else {
                    return out;
                };
                let Some(&r) = self.open.row_at(row) else {
                    return out;
                };
                let Some((bag_index, bag_slot)) = wire_pos(bag, slot) else {
                    debug!("ui_merchant: slot buy to an unaddressable slot ({bag}, {slot})");
                    return out;
                };
                // The wire wants the container's guid: ours for a slot in our own array, the bag's for a real bag.
                let bag_guid = if bag_index == BAG_PLAYER_INVENTORY {
                    Some(me_guid)
                } else {
                    me.player_inv_slot(bag_index).filter(|g| *g != 0)
                };
                if let Some(bag_guid) = bag_guid {
                    let _ = self.commands.0.send(ClientCommand::BuyItemInSlot {
                        vendor,
                        entry: r.entry,
                        slot: r.slot,
                        bag_guid,
                        bag_slot,
                        count: 1,
                    });
                }
            }
            // A held row dropped on a bag's button (`0x5e79d0`): the same send, the bag's guid and slot `0xFF`.
            MerchantAct::BuyInBag { bag, row } => {
                let (Some(vendor), Some((me, me_guid))) = (self.open.vendor, me) else {
                    return out;
                };
                let (Some(&r), Some((bag_index, _))) = (self.open.row_at(row), wire_pos(bag, 1))
                else {
                    return out;
                };
                let bag_guid = if bag_index == BAG_PLAYER_INVENTORY {
                    Some(me_guid)
                } else {
                    me.player_inv_slot(bag_index).filter(|g| *g != 0)
                };
                if let Some(bag_guid) = bag_guid {
                    let _ = self.commands.0.send(ClientCommand::BuyItemInSlot {
                        vendor,
                        entry: r.entry,
                        slot: r.slot,
                        bag_guid,
                        bag_slot: 0xff,
                        count: 1,
                    });
                }
            }
            // A held row dropped on a slot whose item is no bag (`0x5e8fae`): that item's guid as the bag, for the server to refuse.
            MerchantAct::BuyInItem { player_slot, row } => {
                let (Some(vendor), Some((me, _))) = (self.open.vendor, me) else {
                    return out;
                };
                let (Some(&r), Some(bag_guid)) = (
                    self.open.row_at(row),
                    me.player_slot(player_slot).filter(|g| *g != 0),
                ) else {
                    return out;
                };
                let _ = self.commands.0.send(ClientCommand::BuyItemInSlot {
                    vendor,
                    entry: r.entry,
                    slot: r.slot,
                    bag_guid,
                    bag_slot: 0xff,
                    count: 1,
                });
            }
            // `0x584600`: with the vendor holding the interact target its clear runs the interaction close, whose merchant leg closes first (`0x584614` -> `0x512f3a`); no earlier leg holds it, as each window's open takes the target from the one before (`0x51fce0`).
            MerchantAct::Close => {
                let Some(vendor) = self.open.vendor else {
                    return out;
                };
                out.closed = 1;
                if target.is_some_and(|t| t.clear_in_call(vendor)) {
                    out.closed = 2;
                }
                self.open.vendor = None;
                self.open.items.clear();
            }
        }
        out
    }
}

/// A right mouse-down in the world ends repair mode: the WorldFrame hook resets the Repair base mode
/// to Point and consumes nothing. A held payload pre-empts the hook, and a press over a UI frame
/// never reaches it.
pub(crate) fn end_repair_mode_on_right_press(
    mut presses: MessageReader<WorldRightPress>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    script: Option<NonSendMut<UiScript>>,
) {
    if presses.read().last().is_none() || payload_held.0 {
        return;
    }
    if let Some(mut script) = script {
        script.end_repair_mode();
    }
}

/// A level-20 sword 40 points down at a repair vendor flagged `UNIT_FLAG_PVP`, for the purse tests
/// here and in [`crate::ui_items`].
#[cfg(test)]
pub(crate) mod purse_fixture {
    use super::*;
    use benilla_protocol::field::{
        FIELD_PLAYER_FIELD_COINAGE, FIELD_UNIT_FLAGS, FIELD_UNIT_NPC_FLAGS,
    };
    use benilla_protocol::ObjectFields;

    pub(crate) const VENDOR: u64 = 0xF130_0000_0000_0043;
    pub(crate) const SWORD: u64 = 0x4000_0000_0000_0707;
    pub(crate) const SWORD_ENTRY: u32 = 2488;
    /// Absolute descriptor indices.
    const OBJECT_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;
    const ITEM_DURABILITY: u16 = benilla_protocol::field::FIELD_ITEM_DURABILITY;
    const ITEM_MAXDURABILITY: u16 = benilla_protocol::field::FIELD_ITEM_MAXDURABILITY;
    /// The string `ERR_NOT_ENOUGH_MONEY` holds in 1.12's `GlobalStrings.lua`.
    pub(crate) const NOT_ENOUGH_MONEY: &str = "You don't have enough money.";

    /// One level-20 row at 1 copper a point for a sword (weapon subclass 7), and quality 1's
    /// multiplier (row `2·1 + 1`) at 1.
    fn tables() -> benilla_formats::DurabilityTables {
        let mut row = vec![0u32; 29];
        row[7] = 1;
        benilla_formats::DurabilityTables::from_rows([(20, row)], [(3, 1.0)])
    }

    /// The sword's repair at this vendor, undiscounted: the vendor is neutral and no 3.3.5a player
    /// field carries an honor rank.
    pub(crate) fn sword_price() -> u32 {
        tables().repair_cost(40, 20, 1, 2, 7, 0.0)
    }

    /// The merchant open on the vendor, whose unit is streamed, the tables, and the sword's
    /// template and object.
    pub(crate) fn seat(world: &mut World) {
        world.insert_resource(RepairTables(tables()));
        world
            .resource_mut::<MerchantOpen>()
            .open(VENDOR, Vec::new());
        world.spawn((
            Guid(VENDOR),
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_NPC_FLAGS, NPC_FLAG_REPAIR),
                (FIELD_UNIT_FLAGS, 0x1000),
            ])),
        ));
        let mut sword = crate::items::test_template("Sword");
        (sword.class, sword.subclass, sword.quality, sword.item_level) = (2, 7, 1, 20);
        world
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, Some(sword));
        crate::items::test_spawn_item(
            world,
            SWORD,
            ObjectFields::from_pairs(&[
                (OBJECT_ENTRY, SWORD_ENTRY),
                (ITEM_DURABILITY, 35),
                (ITEM_MAXDURABILITY, 75),
            ]),
            false,
        );
    }

    /// The player's fields: the sword in the guid pair at `slot_field` and `money` copper.
    pub(crate) fn player(slot_field: u16, money: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (slot_field, SWORD as u32),
            (slot_field + 1, (SWORD >> 32) as u32),
            (FIELD_PLAYER_FIELD_COINAGE, money),
        ]))
    }

    /// [`seat`] and the player with the sword at `slot_field`; returns the player's entity too.
    pub(crate) fn repair_app(slot_field: u16) -> (App, Entity) {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<MerchantOpen>()
            .init_resource::<crate::net::Reputations>()
            .insert_resource(NetCommands(tx));
        seat(app.world_mut());
        let me = app
            .world_mut()
            .spawn((SelfPlayer, Guid(0x5e1f), player(slot_field, 0)))
            .id();
        (app, me)
    }

    /// A VM with the sword at live id 16, backpack slot 1 and bank slot 1, and a tooltip `TT`.
    pub(crate) fn sword_vm() -> UiScript {
        let mut script = UiScript::new().unwrap();
        let mut inv: benilla_ui::script::InventorySlots = Default::default();
        inv[16] = Some(benilla_ui::script::InvSlotView {
            item_id: SWORD_ENTRY,
            ..Default::default()
        });
        script.set_inventory_slots(inv);
        let mut bag = benilla_ui::script::ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            benilla_ui::script::ContainerSlot {
                item_id: SWORD_ENTRY,
                count: 1,
                ..Default::default()
            },
        );
        script.set_container(0, Some(bag.clone()));
        script.set_container(crate::ui_items::BANK_CONTAINER, Some(bag));
        script
            .run(
                r#"CreateFrame("GameTooltip", "TT"):SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")"#,
            )
            .unwrap();
        script
    }

    /// The message's string, and a recorder of the `UI_ERROR_MESSAGE` lines [`shown`] reads.
    pub(crate) fn record_errors(script: &UiScript) {
        script
            .run(&format!(
                r#"
                ERR_NOT_ENOUGH_MONEY = "{NOT_ENOUGH_MONEY}"
                BENILLA_TEST_LINES = {{}}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(BENILLA_TEST_LINES, arg1) end)
                "#
            ))
            .unwrap();
    }

    /// The `UI_ERROR_MESSAGE` lines since [`record_errors`].
    pub(crate) fn shown(script: &UiScript) -> Vec<String> {
        script
            .eval::<Vec<String>>("return BENILLA_TEST_LINES")
            .unwrap()
    }

    /// The message rows queued for their sound.
    pub(crate) fn sounded(world: &World) -> Vec<&'static str> {
        world
            .resource::<crate::sound::MessageSounds>()
            .queued()
            .iter()
            .map(|r| r.key)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GetRepairAllCost` sums each item's cost after the open merchant's discount: at Honored, 0.1
    /// comes off each item before its rounding, and the undiscounted total is not what the VM
    /// reads.
    #[test]
    fn the_repair_all_total_takes_the_merchants_discount() {
        use benilla_protocol::field::{
            FIELD_PLAYER_INV_SLOT_HEAD, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS,
            FIELD_UNIT_NPC_FLAGS,
        };
        use benilla_protocol::ObjectFields;

        /// Absolute descriptor indices.
        const OBJECT_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const ITEM_DURABILITY: u16 = benilla_protocol::field::FIELD_ITEM_DURABILITY;
        const ITEM_MAXDURABILITY: u16 = benilla_protocol::field::FIELD_ITEM_MAXDURABILITY;
        const BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const SWORD: u64 = 0x4000_0000_0000_0007;
        const SWORD_ENTRY: u32 = 2488;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let tables = benilla_formats::load_durability_tables(&mut chain).expect("durability");
        let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);
        // A level 20 green sword (class 2, subclass 7) 40 points down.
        let disc = f64::from(0.1f32);
        let want = tables.repair_cost(40, 20, 2, 2, 7, disc);
        let full = tables.repair_cost(40, 20, 2, 2, 7, 0.0);
        assert!(
            want > 0 && want < full,
            "the case discriminates: {want} of {full}"
        );

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx))
            .insert_resource(RepairTables(tables))
            .insert_resource(factions)
            .insert_resource(reps);
        let mut open = MerchantOpen::default();
        open.open(VENDOR, Vec::new());
        app.insert_resource(open);
        let mut sword = crate::items::test_template("Sword");
        (sword.class, sword.subclass, sword.quality, sword.item_level) = (2, 7, 2, 20);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, Some(sword));
        crate::items::test_spawn_item(
            app.world_mut(),
            SWORD,
            ObjectFields::from_pairs(&[
                (OBJECT_ENTRY, SWORD_ENTRY),
                (ITEM_DURABILITY, 35),
                (ITEM_MAXDURABILITY, 75),
            ]),
            false,
        );
        // The sword in the main hand (slot 15).
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        app.world_mut().spawn((
            SelfPlayer,
            Guid(0x5e1f),
            ObjectStore(ObjectFields::from_pairs(&[
                (BYTES_0, crate::target::HUMAN_WARRIOR),
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (FIELD_UNIT_FLAGS, 0x8),
                (main_hand, SWORD as u32),
                (main_hand + 1, (SWORD >> 32) as u32),
            ])),
        ));
        app.world_mut().spawn((
            Guid(VENDOR),
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_NPC_FLAGS, NPC_FLAG_REPAIR),
                (FIELD_UNIT_FACTIONTEMPLATE, template),
                (FIELD_UNIT_FLAGS, 0x1000),
            ])),
        ));
        app.insert_non_send_resource(UiScript::new().unwrap());

        // Read at the call, the vendor's unit the repairer.
        let got = crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.eval::<i64>("return (GetRepairAllCost())").unwrap()
        })
        .unwrap();
        assert_eq!(got, i64::from(want), "the undiscounted total is {full}");
    }

    /// `RepairAllItems()` inside its call with the world lent, at [`purse_fixture`]'s vendor carrying
    /// `npc_flags`, with the sword at `slot_field` and `money` copper: the packets, the
    /// `UI_ERROR_MESSAGE` lines and the message rows queued for their sound.
    fn repair_all(
        npc_flags: u32,
        slot_field: u16,
        money: u32,
    ) -> (Vec<ClientCommand>, Vec<String>, Vec<&'static str>) {
        repair_all_with(npc_flags, slot_field, money, "RepairAllItems()")
    }

    /// [`repair_all`] with the Lua call `call`.
    fn repair_all_with(
        npc_flags: u32,
        slot_field: u16,
        money: u32,
        call: &str,
    ) -> (Vec<ClientCommand>, Vec<String>, Vec<&'static str>) {
        use benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
        use benilla_protocol::ObjectFields;
        use purse_fixture::*;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<MerchantOpen>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx));
        seat(app.world_mut());
        let mut units = app.world_mut().query::<(&Guid, &mut ObjectStore)>();
        for (guid, mut store) in units.iter_mut(app.world_mut()) {
            if guid.0 == VENDOR {
                let flags = ObjectFields::from_pairs(&[(FIELD_UNIT_NPC_FLAGS, npc_flags)]);
                store.0.merge(flags);
            }
        }
        app.world_mut()
            .spawn((SelfPlayer, Guid(0x5e1f), player(slot_field, money)));
        let script = UiScript::new().unwrap();
        record_errors(&script);
        app.insert_non_send_resource(script);
        crate::ui_script::live::with_live_world(app.world_mut(), |s| s.run(call).unwrap());
        let sent = rx.try_iter().collect();
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        let lines = shown(&script);
        (sent, lines, script.take_message_sounds())
    }

    /// At a vendor that does not repair, `RepairAllItems` sends nothing and says nothing: it
    /// returns unless the open merchant carries the repair flag (`0x585c90`), before its purse
    /// test. The same worn sword at a repairer goes out.
    #[test]
    fn repair_all_does_nothing_at_a_vendor_that_does_not_repair() {
        use crate::target::cursor_mode::npc_flags::VENDOR as NPC_FLAG_VENDOR;
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let price = sword_price();
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        for money in [price, price - 1] {
            let (sent, lines, rows) = repair_all(NPC_FLAG_VENDOR, main_hand, money);
            assert!(sent.is_empty(), "{money} copper: no repair goes out");
            assert!(
                lines.is_empty() && rows.is_empty(),
                "{money} copper: no purse refusal"
            );
        }
        let (sent, ..) = repair_all(NPC_FLAG_VENDOR | NPC_FLAG_REPAIR, main_hand, price);
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: 0,
                guild_bank: false,
            }]
        ));
    }

    /// `RepairAllItems` prices the equipment, the backpack and the bags for its purse test
    /// (`0x585c90`): the sword worn or in the backpack and over the purse is refused with
    /// `ERR_NOT_ENOUGH_MONEY` and no send.
    #[test]
    fn repair_all_tests_the_purse_against_the_carried_gear() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let price = sword_price();
        // The main hand (slot 15), then backpack slot 1 (inventory slot 23).
        for slot in [15, 23] {
            let field = FIELD_PLAYER_INV_SLOT_HEAD + 2 * slot;
            let (sent, lines, rows) = repair_all(NPC_FLAG_REPAIR, field, price - 1);
            assert!(
                sent.is_empty(),
                "slot {slot}, one copper short: nothing goes out"
            );
            assert_eq!(lines, [NOT_ENOUGH_MONEY]);
            assert_eq!(rows, ["ERR_NOT_ENOUGH_MONEY"]);
        }
    }

    /// `RepairAllItems(1)`, the guild-bank button's call, skips the purse test and sends the
    /// guild-bank byte (`0x585ef8`).
    #[test]
    fn a_guild_bank_repair_skips_the_purse_and_sends_the_flag() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        let (sent, lines, rows) =
            repair_all_with(NPC_FLAG_REPAIR, main_hand, 0, "RepairAllItems(1)");
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::RepairItem {
                    vendor: VENDOR,
                    item_guid: 0,
                    guild_bank: true,
                }]
            ),
            "{sent:?}"
        );
        assert!(lines.is_empty() && rows.is_empty());
    }

    /// An item's cost is priced at the vendor, with its discount, and once it closes, without
    /// (`0x584b20`); this neutral vendor earns none.
    #[test]
    fn each_items_cost_takes_the_open_vendors_discount() {
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        use purse_fixture::*;

        let full = sword_price();
        let priced = |slot_field: u16, read: &str| {
            let (mut app, _) = repair_app(slot_field);
            app.insert_non_send_resource(sword_vm());
            let read_cost = |app: &mut App| {
                crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                    s.eval::<i64>(read).unwrap()
                })
                .unwrap()
            };
            let at_vendor = read_cost(&mut app);
            app.world_mut().resource_mut::<MerchantOpen>().clear();
            (at_vendor, read_cost(&mut app))
        };
        let want = (i64::from(full), i64::from(full));

        // Main hand, inventory slot 15.
        let worn = priced(
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15,
            r#"local _, _, c = TT:SetInventoryItem("player", 16) return c"#,
        );
        assert_eq!(worn, want, "worn: at the vendor, then with none open");
        // Backpack slot 1, inventory slot 23.
        let carried = priced(
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23,
            "local _, c = TT:SetBagItem(0, 1) return c",
        );
        assert_eq!(
            carried, want,
            "in the backpack: at the vendor, then with none open"
        );
    }

    /// Each input alone moves the costs read at the call; a bank item is priced but left out of the
    /// total (`0x585990`).
    #[test]
    fn each_input_alone_moves_the_costs_read_at_the_call() {
        use benilla_protocol::field::{FIELD_PLAYER_INV_SLOT_HEAD, FIELD_UNIT_FLAGS};
        use benilla_protocol::ObjectFields;
        use purse_fixture::*;

        /// `ITEM_FIELD_DURABILITY`, absolute.
        const ITEM_DURABILITY: u16 = benilla_protocol::field::FIELD_ITEM_DURABILITY;
        let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
        let backpack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
        // `PLAYER_FIELD_BANK_SLOT_1`, inventory slot 39.
        let bank_1 = benilla_protocol::field::FIELD_PLAYER_BANK_SLOT_1;

        let (mut app, me) = repair_app(main_hand);
        let tables = app.world_mut().remove_resource::<RepairTables>().unwrap();
        app.insert_non_send_resource(sword_vm());

        let entity =
            |app: &App, guid: u64| app.world().resource::<crate::net::GuidIndex>().0[&guid];
        let write = |app: &mut App, e: Entity, pairs: &[(u16, u32)]| {
            let mut store = app.world_mut().get_mut::<ObjectStore>(e).unwrap();
            store.0.merge(ObjectFields::from_pairs(pairs));
        };
        // (worn, backpack, bank, `GetRepairAllCost`) after one frame.
        let frame = |app: &mut App| {
            crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.eval::<(i64, i64, i64, i64)>(
                    r#"local _, _, worn = TT:SetInventoryItem("player", 16)
                       local _, carried = TT:SetBagItem(0, 1)
                       local _, banked = TT:SetBagItem(-1, 1)
                       return worn, carried, banked, (GetRepairAllCost())"#,
                )
                .unwrap()
            })
            .unwrap()
        };
        let vendor = app
            .world_mut()
            .query::<(Entity, &Guid)>()
            .iter(app.world())
            .find(|(_, g)| g.0 == VENDOR)
            .unwrap()
            .0;
        let hi = (SWORD >> 32) as u32;
        let moved =
            |from: u16, to: u16| [(from, 0), (from + 1, 0), (to, SWORD as u32), (to + 1, hi)];

        assert_eq!(frame(&mut app), (0, 0, 0, 0), "no tables, no prices");
        // 1 copper a point: 40 points cost 40.
        app.insert_resource(tables);
        assert_eq!(frame(&mut app), (40, 0, 0, 40), "the tables arrive");
        let sword = entity(&app, SWORD);
        write(&mut app, sword, &[(ITEM_DURABILITY, 55)]);
        assert_eq!(
            frame(&mut app),
            (20, 0, 0, 20),
            "a repair's write: 20 points"
        );
        write(&mut app, me, &moved(main_hand, bank_1));
        assert_eq!(
            frame(&mut app),
            (0, 0, 20, 0),
            "banked: priced, not in the total"
        );
        write(&mut app, me, &moved(bank_1, backpack_1));
        assert_eq!(frame(&mut app), (0, 20, 0, 20), "the sword in the backpack");
        write(&mut app, vendor, &[(FIELD_UNIT_FLAGS, 0)]);
        assert_eq!(
            frame(&mut app),
            (0, 20, 0, 20),
            "the vendor's flags moved: the same price"
        );
        app.world_mut().resource_mut::<MerchantOpen>().clear();
        assert_eq!(
            frame(&mut app),
            (0, 20, 0, 0),
            "the repairer gone: no total"
        );
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(SWORD_ENTRY, None);
        assert_eq!(
            frame(&mut app),
            (0, 0, 0, 0),
            "an unknown template prices nothing"
        );
    }

    fn row(entry: u32, slot: u32, count: u32) -> VendorItem {
        VendorItem {
            slot,
            entry,
            display_id: 100 + entry,
            current_count: count,
            price: 500,
            max_durability: 0,
            buy_count: 1,
            extended_cost: 0,
        }
    }

    /// The ids are asserted, not just the keys: each is the message its arm pushes.
    #[test]
    fn the_refusal_tables_are_the_references_own() {
        let id = |key: &str| {
            benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a catalog row"))
                .key
        };
        // `SMSG_BUY_FAILED`, the switch at `0x6cdc23`: 1 and 7 share the sold-out row, 6 is
        // silent, and codes outside the table fall to `ERR_ITEM_NOT_FOUND`.
        for (code, want) in [
            (0u8, Some("ERR_ITEM_NOT_FOUND")),
            (1, Some("ERR_VENDOR_SOLD_OUT")),
            (2, Some("ERR_NOT_ENOUGH_MONEY")),
            (3, Some("ERR_ITEM_NOT_FOUND")),
            (4, Some("ERR_VENDOR_HATES_YOU")),
            (5, Some("ERR_VENDOR_TOO_FAR")),
            (6, None),
            (7, Some("ERR_VENDOR_SOLD_OUT")),
            (8, Some("ERR_ITEM_MAX_COUNT")),
            (9, Some("ERR_ITEM_NOT_FOUND")),
            (10, Some("ERR_ITEM_NOT_FOUND")),
            (11, Some("ERR_CANT_EQUIP_RANK")),
            (12, Some("ERR_CANT_EQUIP_REPUTATION")),
            (13, Some("ERR_ITEM_NOT_FOUND")),
            (255, Some("ERR_ITEM_NOT_FOUND")),
        ] {
            assert_eq!(buy_error_key(code).map(id), want, "buy code {code}");
        }
        // `SMSG_SELL_ITEM`, the switch at `0x6d157c`, where the default is silence.
        for (code, want) in [
            (0u8, None),
            (1, Some("ERR_ITEM_NOT_FOUND")),
            (2, Some("ERR_VENDOR_NOT_INTERESTED")),
            (3, Some("ERR_VENDOR_HATES_YOU")),
            (4, Some("ERR_NOT_OWNER")),
            (5, None),
            (6, Some("ERR_DESTROY_NONEMPTY_BAG")),
            (7, Some("ERR_VENDOR_DOESNT_BUY")),
            (8, Some("ERR_MUST_REPAIR_DURABILITY")),
            (9, Some("ERR_INTERNAL_BAG_ERROR")),
            (10, None),
            (255, None),
        ] {
            assert_eq!(sell_error_key(code).map(id), want, "sell code {code}");
        }
    }

    /// Only these two vendor refusals speak.
    #[test]
    fn the_purse_refusals_carry_their_voice_lines() {
        let tag = |key: &str| benilla_ui::messages::by_key(key).expect("row").type_tag;
        assert_eq!(tag("ERR_NOT_ENOUGH_MONEY"), 0x28);
        assert_eq!(tag("ERR_ITEM_MAX_COUNT"), 0x1e);
        for key in [
            "ERR_VENDOR_SOLD_OUT",
            "ERR_VENDOR_HATES_YOU",
            "ERR_VENDOR_TOO_FAR",
            "ERR_ITEM_NOT_FOUND",
            "ERR_VENDOR_NOT_INTERESTED",
            "ERR_NOT_OWNER",
            "ERR_DESTROY_NONEMPTY_BAG",
            "ERR_CANT_EQUIP_RANK",
            "ERR_CANT_EQUIP_REPUTATION",
        ] {
            assert_eq!(tag(key), 0x44, "{key}");
        }
    }

    /// A 0-based row maps to its entry and its own wire slot, the pair `CMSG_BUY_ITEM` names, with a vendor set (`0x584080`).
    #[test]
    fn row_at_maps_the_zero_based_row() {
        let mut open = MerchantOpen::default();
        assert!(!open.is_open());
        open.open(0x42, vec![row(159, 1, STOCK_UNLIMITED), row(4540, 7, 5)]);
        assert!(open.is_open());
        let at = |o: &MerchantOpen, r| o.row_at(r).map(|it| (it.entry, it.slot));
        assert_eq!(at(&open, 0), Some((159, 1)));
        assert_eq!(at(&open, 1), Some((4540, 7)));
        assert_eq!(at(&open, 2), None);
        open.vendor = None;
        assert_eq!(at(&open, 0), None, "no vendor, no row");
    }

    /// A row's extended cost resolves from `ItemExtendedCost.dbc`: points, the non-zero items in
    /// record order, and the rating demand with its bracket.
    #[test]
    fn an_extended_cost_row_resolves_its_record() {
        use benilla_formats::{ExtendedCostRow, ItemExtendedCosts};
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let costs = ExtendedCosts(ItemExtendedCosts::from_iter([
            (
                2276,
                ExtendedCostRow {
                    honor: 7000,
                    items: [(0, 0), (20560, 3), (0, 0), (0, 0), (0, 0)],
                    ..Default::default()
                },
            ),
            (
                2277,
                ExtendedCostRow {
                    arena: 150,
                    arena_slot: 1,
                    personal_rating: 1800,
                    ..Default::default()
                },
            ),
        ]));
        let mut token = row(42133, 1, STOCK_UNLIMITED);
        token.extended_cost = 2276;
        let cost = resolve_item(&token, &items, None, Some(&costs), &commands)
            .extended_cost
            .expect("the record");
        assert_eq!((cost.honor, cost.arena), (7000, 0));
        assert_eq!(cost.items.len(), 1);
        assert_eq!((cost.items[0].item_id, cost.items[0].count), (20560, 3));
        assert_eq!(cost.required_rating, 0);

        token.extended_cost = 2277;
        let rated = resolve_item(&token, &items, None, Some(&costs), &commands);
        let rated = rated.extended_cost.expect("the record");
        assert_eq!((rated.required_rating, rated.arena_bracket), (1800, 1));

        token.extended_cost = 9;
        assert_eq!(
            resolve_item(&token, &items, None, Some(&costs), &commands).extended_cost,
            None,
            "an id with no record"
        );
    }

    #[test]
    fn resolve_maps_unlimited_stock_to_minus_one() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        // Unlimited stock is `numAvailable` -1; a finite count passes through.
        let unlimited = resolve_item(&row(159, 1, STOCK_UNLIMITED), &items, None, None, &commands);
        assert_eq!(unlimited.num_available, -1);
        assert_eq!(unlimited.item_id, 159);
        let finite = resolve_item(&row(4540, 2, 5), &items, None, None, &commands);
        assert_eq!(finite.num_available, 5);
        // No template answer yet: no name or stats, the rest present.
        assert!(finite.name.is_none());
        assert!(finite.stats.is_none());
        assert_eq!(finite.price, 500);
    }

    #[test]
    fn resolve_fills_the_tooltip_stats_from_the_template() {
        let mut items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        items.insert_template(
            2129,
            Some(benilla_protocol::messages::ItemInfo {
                class: 4,
                subclass: 6,
                name: "Chipped Buckler".into(),
                display_info_id: 18730,
                quality: 1,
                flags: 0,
                buy_price: 0,
                sell_price: 0,
                inventory_type: 14,
                allowable_class: -1,
                allowable_race: -1,
                item_level: 0,
                required_level: 0,
                required_skill: 0,
                required_skill_rank: 0,
                required_spell: 0,
                required_honor_rank: 0,
                required_city_rank: 0,
                required_rep_faction: 0,
                required_rep_rank: 0,
                max_count: 0,
                stackable: 1,
                container_slots: 0,
                stats: Vec::new(),
                damages: Vec::new(),
                dmg_min: 0.0,
                dmg_max: 0.0,
                dmg_type: 0,
                armor: 85,
                resistances: [0; 6],
                delay_ms: 0,
                ammo_type: 0,
                ranged_mod_range: 0.0,
                spells: Vec::new(),
                spell_charges_0: 0,
                use_spell: None,
                bonding: 0,
                description: String::new(),
                page_text: 0,
                language_id: 0,
                page_material: 0,
                start_quest: 0,
                lock_id: 0,
                material: 0,
                sheath: 4,
                random_property: 0,
                block: 1,
                item_set: 0,
                max_durability: 0,
                area: 0,
                map: 0,
                bag_family: 0,
                totem_category: 0,
                socket_colors: [0; 3],
                socket_bonus: 0,
                gem_properties: 0,
                flags2: 0,
                required_disenchant_skill: -1,
                item_limit_category: 0,
                duration: 0,
                scaling_stat_distribution: 0,
                scaling_stat_value: 0,
                armor_damage_modifier: 0.0,
            }),
        );
        let resolved = resolve_item(&row(2129, 1, 3), &items, None, None, &commands);
        let stats = resolved.stats.expect("template answered → stats present");
        assert_eq!(
            (
                stats.quality,
                stats.inventory_type,
                stats.class,
                stats.subclass
            ),
            (1, 14, 4, 6)
        );
        assert_eq!((stats.armor, stats.block), (85, 1));
    }
}
