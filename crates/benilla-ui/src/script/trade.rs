//! The trade bindings: the app pushes both sides of the open trade ([`UiScript::set_trade`]), and
//! the trade verbs queue intents it drains. Slots are 1-based, 1..=7, the seventh the non-traded
//! enchant slot; an empty or out-of-range slot answers nil. The partner rides the `"npc"` unit,
//! which the app points at them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorItem, CursorPayload};
use super::{Model, ScriptValue};

/// Slots per side, six traded and the non-traded enchant slot: vmangos `TRADE_SLOT_COUNT`
/// (`TradeData.h`), kept equal to the protocol crate's, which this crate does not depend on.
pub const TRADE_SLOTS: usize = 7;

/// One filled trade slot, resolved by the app; an empty slot is `None`, never a zeroed `Some`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSlotItem {
    /// The item entry, the key for `isUsable` and the tooltip.
    pub item_id: u32,
    /// `None` while the item template is in flight.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// `None` while the template is in flight; only the target side's info returns it
    /// (`TradeFrame.lua:84`).
    pub quality: Option<u32>,
    /// The name of the enchant being applied to slot 7, which stock shows in green
    /// (`TradeFrame.lua:62`); the app always sends `None`.
    pub enchantment: Option<String>,
    /// The escaped item link the link verbs answer; `None` while the template is in flight, since
    /// it embeds the name and the quality.
    pub link: Option<String>,
}

/// One side's offer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSideState {
    /// Index 0 is trade slot 1, index 6 the enchant slot.
    pub slots: [Option<TradeSlotItem>; TRADE_SLOTS],
    /// In copper.
    pub gold: u32,
}

/// The open trade, pushed whole by the app. The accept highlight is not here: it rides the
/// `TRADE_ACCEPT_UPDATE` event (`TradeFrame.lua:35`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeState {
    /// Our offer.
    pub player: TradeSideState,
    /// The partner's offer, the wire's `their_window` snapshot.
    pub target: TradeSideState,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open trade window's snapshot.
    pub fn set_trade(&mut self, state: Option<TradeState>) {
        set_field!(self, trade, state);
    }

    /// Drain the tokens `InitiateTrade` queued; the app sends `CMSG_INITIATE_TRADE` for each.
    pub fn take_trade_initiates(&mut self) -> Vec<String> {
        drain_field!(self, trade_initiates)
    }

    /// Whether `AcceptTrade` was called since the last drain: `CMSG_ACCEPT_TRADE`.
    pub fn take_trade_accept(&mut self) -> bool {
        drain_field!(self, trade_accept)
    }

    /// Whether `CancelTradeAccept` was called since the last drain: `CMSG_UNACCEPT_TRADE`.
    pub fn take_trade_unaccept(&mut self) -> bool {
        drain_field!(self, trade_unaccept)
    }

    /// Whether `CloseTrade`, the window's OnHide verb, was called since the last drain; the app
    /// sends `CMSG_CANCEL_TRADE` and ends its session.
    pub fn take_trade_close(&mut self) -> bool {
        drain_field!(self, trade_close)
    }

    /// Whether `BeginTrade()` was called since the last drain: an empty `CMSG_BEGIN_TRADE`.
    pub fn take_trade_begin(&mut self) -> bool {
        drain_field!(self, trade_begin)
    }

    /// Whether `CancelTrade()` was called since the last drain: the bare `CMSG_CANCEL_TRADE`.
    pub fn take_trade_cancel(&mut self) -> bool {
        drain_field!(self, trade_cancel)
    }

    /// The copper `SetTradeMoney` last offered since the last drain: `CMSG_SET_TRADE_GOLD`.
    pub fn take_trade_money(&mut self) -> Option<u32> {
        drain_field!(self, trade_set_money)
    }

    /// Our slots' placements and clears in call order, for `CMSG_SET_TRADE_ITEM` and `CMSG_CLEAR_TRADE_ITEM`.
    pub fn take_trade_item_sends(&mut self) -> Vec<TradeItemSend> {
        drain_field!(self, trade_item_sends)
    }

    /// The `(trade_id, bag, slot)` placements among [`Self::take_trade_item_sends`], which it empties.
    pub fn take_trade_set_items(&mut self) -> Vec<(u32, i64, u32)> {
        self.take_trade_item_sends()
            .into_iter()
            .filter_map(|send| match send {
                TradeItemSend::Set { id, bag, slot } => Some((id, bag, slot)),
                TradeItemSend::Clear { .. } => None,
            })
            .collect()
    }

    /// The 1-based slots cleared among [`Self::take_trade_item_sends`], which it empties.
    pub fn take_trade_clear_items(&mut self) -> Vec<u32> {
        self.take_trade_item_sends()
            .into_iter()
            .filter_map(|send| match send {
                TradeItemSend::Clear { id } => Some(id),
                TradeItemSend::Set { .. } => None,
            })
            .collect()
    }
}

/// The slot at a 1-based index on the side `pick` selects.
fn slot_at(
    model: &Model,
    index: usize,
    pick: impl Fn(&TradeState) -> &TradeSideState,
) -> Option<TradeSlotItem> {
    model
        .trade
        .as_ref()
        .map(pick)
        .and_then(|side| index.checked_sub(1).and_then(|n| side.slots.get(n)))
        .cloned()
        .flatten()
}

fn gold(model: &Model, pick: impl Fn(&TradeState) -> &TradeSideState) -> u32 {
    model.trade.as_ref().map(pick).map_or(0, |side| side.gold)
}

/// One send of our offer's slot setter: `id` the 1-based trade slot, `bag` and `slot` in the cursor's space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeItemSend {
    /// `CMSG_SET_TRADE_ITEM` (`0x7040b0`).
    Set { id: u32, bag: i64, slot: u32 },
    /// `CMSG_CLEAR_TRADE_ITEM` (`0x7041a0`).
    Clear { id: u32 },
}

/// One of our offer's slots (`0xbfa620`, `0xbfa5e8`, `0xbfa5dc`): the item's guid and where it sits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradeOwnItem {
    /// 0 with no item store lent.
    pub(crate) guid: u64,
    pub(crate) item: CursorItem,
}

/// What the offer's gates read off a held item (`0x587aff`-`0x587bcf`, `0x5877b0`, `0x5d896d`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TradeItemFacts {
    /// The soulbound flag or a binding enchant (`0x708520`).
    pub bound: bool,
    /// Its create stamp when flagged `0x100` and owned by the player, the trade window's start (`0x708b66`-`0x708ba3`).
    pub trade_stamp: Option<u32>,
    /// A binding enchant on an item whose template is not flagged `0x2000` (`0x7073e0`).
    pub soulbound_enchant: bool,
    /// The template's bonding is 4, a quest item (`0x587b2c`).
    pub quest_bound: bool,
    /// A bag's held items, `None` for an item that is no bag (`0x587b82`, `0x5125b0`).
    pub bag_items: Option<u32>,
    /// A use spell with an enchant or open-lock effect (`0x707c60` over 53, 54, 156 and 33).
    pub enchants_on_use: bool,
}

/// The facts of the item at `(bag, slot)` read at the call; with no view answering, the slot's own bound read.
fn item_facts(model: &Model, bag: i64, slot: u32) -> TradeItemFacts {
    let guid = cursor::guid_at(model, bag, slot).unwrap_or(0);
    if let Some(facts) = crate::script::read_live(|l| l.trade_item_facts(guid)).flatten() {
        return facts;
    }
    #[cfg(any(test, feature = "fixtures"))]
    if let Some(facts) = model.trade_item_facts.get(&(bag, slot)) {
        return *facts;
    }
    let bound = super::container::live_slot(model, bag, slot).is_some_and(|s| s.already_bound);
    TradeItemFacts {
        bound,
        ..Default::default()
    }
}

impl TradeItemFacts {
    /// Bound with no trade window left (`0x708b40`): the stamp less the played seconds at or under two hours ago.
    fn permanently_bound(&self, model: &Model) -> bool {
        let open = self
            .trade_stamp
            .is_some_and(|stamp| i64::from(stamp) - super::refund::played_now(model) + 7200 > 0);
        self.bound && !open
    }

    /// Bound for the trade (`0x709550`).
    fn bound_for_trade(&self, model: &Model) -> bool {
        self.permanently_bound(model) || self.soulbound_enchant
    }
}

/// Our offer's slot `slot`, 0-based (`0x586010`): empty past the seven.
fn own_item(model: &Model, slot: i64) -> Option<TradeOwnItem> {
    usize::try_from(slot)
        .ok()
        .and_then(|i| model.trade_own_items.get(i))
        .cloned()
        .flatten()
}

/// The row a placed item shows, off its bag slot at the call.
fn shown(model: &Model, item: &CursorItem) -> TradeSlotItem {
    let held = super::container::live_slot(model, item.bag, item.slot);
    let link = held
        .as_ref()
        .and_then(|s| s.link.clone())
        .or(item.link.clone());
    TradeSlotItem {
        item_id: item.item_id,
        name: link
            .as_deref()
            .map(|l| cursor::item_link_name(Some(l)))
            .filter(|n| !n.is_empty()),
        texture: item.texture.clone(),
        count: cursor::held_count(model, item),
        quality: item.quality,
        enchantment: None,
        link,
    }
}

/// The offer's slot setter (`0x5869a0`): refused past the seven; an empty item sends the clear (`0x5869bf`) and an item the placement, which fails with no item object (`0x5869df`); slot 7 signals `TRADE_POTENTIAL_BIND_ENCHANT` (`0x586a3d`); then the slot is written and `TRADE_PLAYER_ITEM_CHANGED` signalled (`0x586a7e`).
fn set_own_item(
    model: &mut crate::script::ModelGuard<'_>,
    slot: i64,
    item: Option<CursorItem>,
) -> bool {
    let Some(i) = usize::try_from(slot).ok().filter(|&i| i < TRADE_SLOTS) else {
        return false;
    };
    let id = i as u32 + 1;
    let entry = match item {
        None => {
            model.trade_item_sends.push(TradeItemSend::Clear { id });
            if i == 6 {
                model.raise(
                    "TRADE_POTENTIAL_BIND_ENCHANT",
                    vec![ScriptValue::Bool(false)],
                );
            }
            None
        }
        Some(item) => {
            let Some(guid) = cursor::guid_at(model, item.bag, item.slot) else {
                return false;
            };
            model.trade_item_sends.push(TradeItemSend::Set {
                id,
                bag: item.bag,
                slot: item.slot,
            });
            if i == 6 {
                // Bound, and not bound for the trade (`0x586a16`, `0x586a22`).
                let facts = item_facts(model, item.bag, item.slot);
                let binds = facts.bound && !facts.bound_for_trade(model);
                model.raise(
                    "TRADE_POTENTIAL_BIND_ENCHANT",
                    vec![ScriptValue::Bool(binds)],
                );
            }
            Some(TradeOwnItem { guid, item })
        }
    };
    let row = entry.as_ref().map(|e| shown(model, &e.item));
    model.trade_own_items[i] = entry;
    if let Some(trade) = model.trade.as_mut() {
        trade.player.slots[i] = row;
    }
    model.raise(
        "TRADE_PLAYER_ITEM_CHANGED",
        vec![ScriptValue::Int(i64::from(id))],
    );
    true
}

/// The bag gates a placement runs (`0x587b79`-`0x587bcf`, `0x58786f`-`0x5878f5`): a bag holding items, then a bag worn in a bag slot.
fn bag_refusal(item: &CursorItem, facts: &TradeItemFacts) -> Option<&'static str> {
    let held = facts.bag_items?;
    if held != 0 {
        return Some("ERR_TRADE_BAG");
    }
    // Inventory slots 19..22, the worn bags (`0x587bc7`).
    (item.bag == cursor::EQUIPMENT_BAG && (20..=23).contains(&item.slot))
        .then_some("ERR_TRADE_EQUIPPED_BAG")
}

/// `ClickTradeButton(index[, remove])` past its argument gate (`0x5879d0`); a refusal's line comes back for the verb to show.
fn click_trade_button(
    model: &mut crate::script::ModelGuard<'_>,
    slot: i64,
    remove: bool,
) -> Option<&'static str> {
    if remove {
        // The slot empties, then its item unlocks (`0x587a51`, `0x587a58`).
        if let Some(old) = own_item(model, slot) {
            set_own_item(model, slot, None);
            cursor::set_item_lock(model, old.item.bag, old.item.slot, false);
        }
        return None;
    }
    // Coins on the cursor go into the offer, the index unread (`0x587a9b`).
    if cursor::money::cursor_money(model) != 0 {
        cursor::money::add_trade_money(model);
        return None;
    }
    // Any other payload reads as no item (`0x587acb`).
    let held = match model.cursor.clone() {
        Some(CursorPayload::Item(item)) => Some(item),
        _ => None,
    };
    // The six traded slots gate the held item (`0x587aec`): bound, a binding enchant, then the bag gates.
    if let Some(item) = held.as_ref().filter(|_| slot < 6) {
        let facts = item_facts(model, item.bag, item.slot);
        if facts.permanently_bound(model) {
            return Some(if facts.quest_bound {
                "ERR_TRADE_QUEST_ITEM"
            } else {
                "ERR_TRADE_BOUND_ITEM"
            });
        }
        if facts.soulbound_enchant {
            return Some("ERR_TRADE_TEMP_ENCHANT_BOUND");
        }
        if let Some(line) = bag_refusal(item, &facts) {
            return Some(line);
        }
    }
    let old = own_item(model, slot);
    // The slot's own item, or nothing on nothing: the full clear (`0x587c0f`).
    let same = match (&held, &old) {
        (None, None) => true,
        (Some(item), Some(old)) => (item.bag, item.slot) == (old.item.bag, old.item.slot),
        _ => false,
    };
    if same {
        cursor::clear_cursor(model);
        return None;
    }
    // The slot takes the held item and hands back its own; the item put down unlocks only when the slot refused it (`0x587c2f`, `0x587c41`, `0x587c4f`).
    let placed = set_own_item(model, slot, held);
    match old {
        Some(old) => cursor::set_cursor_with(model, CursorPayload::Item(old.item), !placed),
        None => cursor::clear_cursor_with(model, !placed, true),
    }
    None
}

/// The first of the six traded slots holding nothing (`0x587840`).
fn free_slot(model: &Model) -> Option<i64> {
    (0..6)
        .find(|&i| model.trade_own_items[i].is_none())
        .map(|i| i as i64)
}

/// The offer's add of a bag item (`0x5877b0`): an unbound item to the first free traded slot, a bound one to slot 7 unless a quest item, past the bag gates; then the slot's write and the item's lock (`0x587918`, `0x587922`).
fn add_own_item(
    model: &mut crate::script::ModelGuard<'_>,
    item: CursorItem,
) -> Result<(), Option<&'static str>> {
    let facts = item_facts(model, item.bag, item.slot);
    let slot = if !facts.bound {
        free_slot(model).ok_or(None)?
    } else if facts.quest_bound {
        return Err(Some("ERR_TRADE_QUEST_ITEM"));
    } else if model.trade_own_items[6].is_none() {
        6
    } else {
        return Err(None);
    };
    if let Some(line) = bag_refusal(&item, &facts) {
        return Err(Some(line));
    }
    let (bag, bag_slot) = (item.bag, item.slot);
    set_own_item(model, slot, Some(item));
    cursor::set_item_lock(model, bag, bag_slot, true);
    Ok(())
}

/// The enchanting item's release (`0x586c20`, `0x586fb6`, `0x5870d5`, `0x587985`, `0x5880f4`): it unlocks and its slot zeroes.
pub(crate) fn release_enchanting_item(model: &mut crate::script::ModelGuard<'_>) {
    if let Some((bag, slot)) = model.trade_enchanting_item.take() {
        cursor::set_item_lock(model, bag, slot, false);
    }
}

/// `UseContainerItem`'s trade arm (`0x5d896d`): with the window open, an item whose use enchants goes at the partner's seventh slot when that holds one (`0x5d89da` -> `0x586c00`: the one before unlocks, the use, the lock), any other into our offer (`0x5d89f9`).
pub(super) fn use_container_item(
    model: &mut crate::script::ModelGuard<'_>,
    bag: i64,
    slot: u32,
) -> super::container::Arm {
    use super::container::Arm;
    if model.trade.is_none() {
        return Arm::Pass;
    }
    let Some(s) = super::container::index_slot(model, bag, slot).filter(|s| s.item_id != 0) else {
        return Arm::Pass;
    };
    let partner_holds = model
        .trade
        .as_ref()
        .is_some_and(|t| t.target.slots[6].is_some());
    if partner_holds && item_facts(model, bag, slot).enchants_on_use {
        release_enchanting_item(model);
        model.call(super::ScriptCall::UseContainerItem { bag, slot });
        cursor::set_item_lock(model, bag, slot, true);
        model.trade_enchanting_item = Some((bag, slot));
        return Arm::Took;
    }
    let added = add_own_item(
        model,
        CursorItem {
            bag,
            slot,
            item_id: s.item_id,
            texture: s.texture,
            link: s.link,
            count: None,
            quality: s.quality,
            equip_slots: s.equip_slots,
            bar_placeable: s.bar_placeable,
        },
    );
    match added {
        Err(Some(line)) => Arm::Refused(line),
        _ => Arm::Took,
    }
}

impl super::UiScript {
    /// The offer's items unlock (`0x5864a0`), as the cancel, the completion, the close and the world leave call it; the slots stand.
    pub fn unlock_trade_items(&mut self) {
        let mut model = self.model_mut();
        for i in 0..TRADE_SLOTS {
            if let Some(own) = model.trade_own_items[i].clone() {
                cursor::set_item_lock(&mut model, own.item.bag, own.item.slot, false);
            }
        }
    }

    /// The server's refusal of our slot `slot`, 0-based (`0x586410`): its item unlocks (`0x586455`), the slot zeroes and `TRADE_PLAYER_ITEM_CHANGED` fires (`0x58648c`).
    pub fn deny_trade_item(&mut self, slot: u8) {
        let mut model = self.model_mut();
        let i = usize::from(slot);
        if i >= TRADE_SLOTS {
            return;
        }
        if let Some(own) = model.trade_own_items[i].take() {
            cursor::set_item_lock(&mut model, own.item.bag, own.item.slot, false);
        }
        if let Some(trade) = model.trade.as_mut() {
            trade.player.slots[i] = None;
        }
        model.raise(
            "TRADE_PLAYER_ITEM_CHANGED",
            vec![ScriptValue::Int(i64::from(slot) + 1)],
        );
    }

    /// An item object's end takes it out of our offer (`0x70b0df` -> `0x586aa0`): the clear goes out, slot 7 signals no bind, the slot zeroes and `TRADE_PLAYER_ITEM_CHANGED` fires.
    pub fn trade_item_destroyed(&mut self, guid: u64) {
        let found = {
            let model = self.model_ref();
            (0..TRADE_SLOTS).find(|&i| {
                guid != 0
                    && model.trade_own_items[i]
                        .as_ref()
                        .is_some_and(|own| own.guid == guid)
            })
        };
        if let Some(i) = found {
            set_own_item(&mut self.model_mut(), i as i64, None);
        }
    }

    /// The window's open zeroes our offer's slots and the completion flag (`0x5870a0`, its partner arm).
    pub fn reset_trade_offer(&mut self) {
        let stale = {
            let model = self.model_ref();
            model.trade_completed || model.trade_own_items.iter().any(Option::is_some)
        };
        if stale {
            let mut model = self.model_mut();
            model.trade_own_items = Default::default();
            model.trade_completed = false;
        }
    }

    /// The enchanting item unlocks (`0x586fb6`): the partner's seventh slot changed.
    pub fn release_trade_enchanting_item(&mut self) {
        if self.model_ref().trade_enchanting_item.is_some() {
            release_enchanting_item(&mut self.model_mut());
        }
    }

    /// The partner setter's close on an open window (`0x5870d5`, `0x5870f2`, `0x5880f4`): the enchanting item unlocks and the completion flag is kept for `CloseTrade`.
    pub fn close_trade_partner(&mut self, completed: bool) {
        self.release_trade_enchanting_item();
        if self.model_ref().trade_completed != completed {
            self.model_mut().trade_completed = completed;
        }
    }

    /// A fixture's stand-in for the offer gates' reads of the item at `(bag, slot)`.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_trade_item_facts(&mut self, bag: i64, slot: u32, facts: TradeItemFacts) {
        self.model_mut().trade_item_facts.insert((bag, slot), facts);
    }

    /// Whether our offer's slot `slot`, 0-based, holds an item (`0xbfa620`).
    pub fn trade_offers_slot(&self, slot: usize) -> bool {
        self.model_ref()
            .trade_own_items
            .get(slot)
            .is_some_and(Option::is_some)
    }
}

/// Register the trade globals.
impl super::UiScript {
    /// Drain the partner's enchant slot asks: `false` the click, `true` the replace Accept.
    pub fn take_trade_enchant_targets(&mut self) -> Vec<bool> {
        drain_field!(self, trade_enchant_targets)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetTradePlayerItemInfo(id): the five values `TradeFrame.lua:55` reads.
    g.set(
        "GetTradePlayerItemInfo",
        lua.create_function(|lua, id: usize| {
            let (slot, usable) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.player);
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            let Some(s) = slot else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                opt_str(&s.texture)?,
                Value::Integer(i64::from(s.count.max(1))),
                flag(usable),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    // The link verbs, for a slot's ctrl and shift clicks (`TradeFrame.xml:129`, `:95`).
    for (name, side) in [
        ("GetTradePlayerItemLink", true),
        ("GetTradeTargetItemLink", false),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, id: usize| {
                let link = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    let slot = if side {
                        slot_at(&model, id, |t| &t.player)
                    } else {
                        slot_at(&model, id, |t| &t.target)
                    };
                    slot.and_then(|s| s.link.clone())
                };
                match link {
                    Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }

    // GetTradeTargetItemInfo(id): the six values `TradeFrame.lua:84` reads.
    g.set(
        "GetTradeTargetItemInfo",
        lua.create_function(|lua, id: usize| {
            let (slot, usable) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let slot = slot_at(&model, id, |t| &t.target);
                let usable = slot
                    .as_ref()
                    .is_none_or(|s| super::item_stats::item_usable_by_id(&model, s.item_id));
                (slot, usable)
            };
            let Some(s) = slot else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let opt_str = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            let quality = match s.quality {
                Some(q) => Value::Integer(i64::from(q)),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(&s.name)?,
                opt_str(&s.texture)?,
                Value::Integer(i64::from(s.count.max(1))),
                quality,
                flag(usable),
                opt_str(&s.enchantment)?,
            ]))
        })?,
    )?;

    g.set(
        "GetPlayerTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(gold(&model, |t| &t.player)))
        })?,
    )?;
    g.set(
        "GetTargetTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(gold(&model, |t| &t.target)))
        })?,
    )?;

    // InitiateTrade(unit): the unit menu's TRADE row.
    g.set(
        "InitiateTrade",
        lua.create_function(|lua, unit: String| {
            crate::script::model_mut_of(lua).trade_initiates.push(unit);
            Ok(())
        })?,
    )?;

    g.set(
        "AcceptTrade",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).trade_accept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTradeAccept",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).trade_unaccept = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CloseTrade",
        lua.create_function(|lua, ()| {
            // `0x587940`: the offer's coins signal unless the last close was a completion (`0x587949`); an open window releases the enchanting item (`0x587985`), closes and signals (`0x5879bc`); the cancel goes out either way (`0x5879c4`).
            let mut model = crate::script::model_mut_of(lua);
            if !model.trade_completed {
                model.raise("PLAYER_TRADE_MONEY", Vec::new());
            }
            if model.trade.is_some() {
                release_enchanting_item(&mut model);
                model.trade_completed = false;
                model.trade = None;
                model.raise("TRADE_CLOSED", Vec::new());
            }
            model.trade_close = true;
            Ok(())
        })?,
    )?;
    // BeginTrade() and CancelTrade(): no arguments, no returns, no gate, and an empty `0x117` or
    // `0x11C`. They are the `TRADE` popup's Yes and No (`StaticPopup.lua:517-522`), which never
    // shows in 1.12 as nothing signals `TRADE_REQUEST`.
    g.set(
        "BeginTrade",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).trade_begin = true;
            Ok(())
        })?,
    )?;
    g.set(
        "CancelTrade",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).trade_cancel = true;
            Ok(())
        })?,
    )?;

    // SetTradeMoney(copper): the money input's callback (`TradeFrame.lua:173`).
    g.set(
        "SetTradeMoney",
        lua.create_function(|lua, copper: Value| {
            // A non-number raises; the low dword is the whole offer, sent only when the purse
            // covers it, and a refusal is silent.
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetTradeMoney(amount)",
            )? as u32;
            let mut model = crate::script::model_mut_of(lua);
            if u64::from(n) > crate::script::live::money(&model) {
                return Ok(());
            }
            model.trade_set_money = Some(n);
            Ok(())
        })?,
    )?;

    // ClickTradeButton(index[, remove]) (`0x5879d0`): a non-number raises; `trunc(index) - 1` is the slot.
    g.set(
        "ClickTradeButton",
        lua.create_function(|lua, (index, remove): (Value, Value)| {
            let slot = crate::script::binding_abi::number_arg(
                lua,
                index,
                "Usage: ClickTradeButton(index)",
            )? as i32;
            let remove = !matches!(remove, Value::Nil | Value::Boolean(false));
            let mut model = crate::script::model_mut_of(lua);
            let line = click_trade_button(&mut model, i64::from(slot) - 1, remove);
            drop(model);
            if let Some(key) = line {
                crate::script::ui_errors::show_now(lua, key);
            }
            Ok(())
        })?,
    )?;

    // ClickTargetTradeButton(id) (`TradeFrame.xml:101`): the partner's column takes nothing.
    g.set(
        "ClickTargetTradeButton",
        lua.create_function(|lua, index: Value| {
            // `0x586c80`: a non-number raises; coins on the cursor go into our offer, and with none slot 7 takes the targeting spell.
            let id = crate::script::binding_abi::number_arg(
                lua,
                index,
                "Usage: ClickTargetTradeButton(index)",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            let coins = cursor::money::cursor_money(&model) != 0;
            cursor::money::add_trade_money(&mut model);
            if !coins && id == 7 {
                model.trade_enchant_targets.push(false);
            }
            Ok(())
        })?,
    )?;

    // ReplaceTradeEnchant() (`0x510b80`): `TRADE_REPLACE_ENCHANT`'s Accept, the cast at slot 7.
    g.set(
        "ReplaceTradeEnchant",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua)
                .trade_enchant_targets
                .push(true);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn item(item_id: u32, count: u32, quality: u32) -> TradeSlotItem {
        TradeSlotItem {
            item_id,
            name: Some("Linen Cloth".into()),
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            count,
            quality: Some(quality),
            enchantment: None,
            link: Some(format!(
                "|cffffffff|Hitem:{item_id}:0:0:0|h[Linen Cloth]|h|r"
            )),
        }
    }

    fn state() -> TradeState {
        let mut player = TradeSideState {
            gold: 12_345,
            ..Default::default()
        };
        player.slots[0] = Some(item(2589, 5, 1));
        let mut target = TradeSideState {
            gold: 500,
            ..Default::default()
        };
        target.slots[0] = Some(item(4306, 1, 2));
        target.slots[6] = Some(item(6217, 1, 1)); // the enchant slot carries an item
        TradeState { player, target }
    }

    #[test]
    fn the_trade_slots_answer_their_item_links() {
        let mut s = UiScript::new().unwrap();
        // No trade open: both sides answer nil rather than raising.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(1) == nil")
            .unwrap());

        let mut st = state();
        // A slot whose template is in flight has no link.
        st.target.slots[1] = Some(TradeSlotItem {
            item_id: 4306,
            name: None,
            texture: None,
            count: 1,
            quality: None,
            enchantment: None,
            link: None,
        });
        s.set_trade(Some(st));

        assert_eq!(
            s.eval::<String>("return GetTradePlayerItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTradeTargetItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:4306:0:0:0|h[Linen Cloth]|h|r"
        );
        // The in-flight slot, an empty one, and one past the seven.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemLink(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(7) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemLink(9) == nil")
            .unwrap());
    }

    #[test]
    fn player_and_target_item_info_read_the_reference_tuples() {
        let mut s = UiScript::new().unwrap();
        // No trade open → every slot is nil, money 0.
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);

        s.set_trade(Some(state()));

        let (name, tex, count, usable): (String, String, i64, i64) = s
            .eval(
                "local n,t,c,u,e = GetTradePlayerItemInfo(1)\n\
                 return n,t,c,u",
            )
            .unwrap();
        assert_eq!((name.as_str(), count, usable), ("Linen Cloth", 5, 1));
        assert_eq!(tex, "Interface\\Icons\\INV_Fabric_Linen_01");
        assert!(s
            .eval::<bool>("local n,t,c,u,e = GetTradePlayerItemInfo(1)\nreturn e == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());

        // The target side adds quality as the fourth return.
        let (name, _tex, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetTradeTargetItemInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, usable),
            ("Linen Cloth", 1, 2, 1)
        );
        // The enchant slot (7) is filled on the target side.
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(7) ~= nil")
            .unwrap());
    }

    #[test]
    fn money_reads_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            12_345
        );
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
    }

    #[test]
    fn clearing_the_trade_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_trade(Some(state()));
        s.set_trade(None);
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(1) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 0);
    }

    #[test]
    fn intents_queue_and_drain() {
        let mut s = UiScript::new().unwrap();
        s.run("InitiateTrade('target')").unwrap();
        s.run("InitiateTrade('party2')").unwrap();
        assert_eq!(s.take_trade_initiates(), vec!["target", "party2"]);
        assert!(s.take_trade_initiates().is_empty(), "drained");

        s.run("AcceptTrade()").unwrap();
        assert!(s.take_trade_accept());
        assert!(!s.take_trade_accept(), "drained");

        s.run("CancelTradeAccept()").unwrap();
        assert!(s.take_trade_unaccept());

        s.run("CloseTrade()").unwrap();
        assert!(s.take_trade_close());
        assert!(!s.take_trade_close(), "drained");

        // An offer is sent only when the purse covers it.
        s.set_money(20_000);
        s.run("SetTradeMoney(1 * 10000 + 23 * 100 + 45)").unwrap();
        assert_eq!(s.take_trade_money(), Some(12_345));
        assert_eq!(s.take_trade_money(), None, "drained");
        // The low dword of -5 is 0xFFFFFFFB, past any purse: refused silently.
        s.run("SetTradeMoney(-5)").unwrap();
        assert_eq!(s.take_trade_money(), None);
        s.run("SetTradeMoney(30000)").unwrap();
        assert_eq!(s.take_trade_money(), None, "more than the purse: silent");
        assert!(s.run("SetTradeMoney(nil)").is_err(), "a non-number raises");
    }

    /// A backpack of four items: cloth in 1, ore in 2, a potion in 3 and a bag in 4.
    fn pack(s: &mut UiScript) {
        use crate::script::{ContainerSlot, ContainerState};
        let mut slots = std::collections::HashMap::new();
        for (slot, item_id) in [(1, 2589), (2, 2770), (3, 118), (4, 4500)] {
            slots.insert(
                slot,
                ContainerSlot {
                    guid: 0x4000_0000_0000_0000 + u64::from(slot),
                    item_id,
                    count: 3,
                    link: Some(format!(
                        "|cffffffff|Hitem:{item_id}:0:0:0|h[Item {item_id}]|h|r"
                    )),
                    ..Default::default()
                },
            );
        }
        s.set_container(
            0,
            Some(ContainerState {
                bag_family: 0,
                name: Some("Backpack".into()),
                num_slots: 16,
                slots,
            }),
        );
    }

    /// An open trade with nothing offered.
    fn open(s: &mut UiScript) {
        pack(s);
        s.set_trade(Some(TradeState::default()));
        s.reset_trade_offer();
    }

    fn locked(s: &UiScript, slot: u32) -> bool {
        s.eval::<bool>(&format!(
            "return select(3, GetContainerItemInfo(0, {slot})) == 1"
        ))
        .unwrap()
    }

    fn fired() -> Vec<String> {
        crate::script::tick::fired::names()
    }

    /// The placed item's clear runs with the unlock off (`0x587c4f`): the slot's send and signal (`0x7040b0`, `0x586a7e`) come first, inside the call, and the item stays locked.
    #[test]
    fn a_placed_item_stays_locked_and_its_slot_signals_inside_the_call() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run(
            "seen = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_PLAYER_ITEM_CHANGED') \
             f:SetScript('OnEvent', function(_, _, id) \
               seen[#seen + 1] = id .. ':' .. tostring(GetTradePlayerItemInfo(id)) .. ':' .. tostring(CursorHasItem()) end)",
        )
        .unwrap();
        s.run("PickupContainerItem(0, 1)").unwrap();
        fired();
        s.run("ClickTradeButton(2) seen[#seen + 1] = 'after'")
            .unwrap();
        assert_eq!(
            s.eval::<String>("return table.concat(seen, ' ')").unwrap(),
            "2:Item 2589:1 after",
            "the listener runs inside the call, the slot written and the cursor still holding"
        );
        assert_eq!(fired(), ["TRADE_PLAYER_ITEM_CHANGED", "CURSOR_UPDATE"]);
        assert_eq!(
            s.take_trade_item_sends(),
            [TradeItemSend::Set {
                id: 2,
                bag: 0,
                slot: 1
            }]
        );
        assert!(s.cursor_payload().is_none());
        assert!(locked(&s, 1), "no unlock at the drop");
    }

    /// An empty cursor on a filled slot empties it and takes its item up, still locked (`0x587c2f` with no item, `0x587c4f`).
    #[test]
    fn an_empty_cursor_on_a_filled_slot_picks_its_item_up() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(2)")
            .unwrap();
        s.take_trade_item_sends();
        fired();
        s.run("ClickTradeButton(2)").unwrap();
        assert_eq!(
            fired(),
            [
                "TRADE_PLAYER_ITEM_CHANGED",
                "CURSOR_UPDATE",
                "CURSOR_UPDATE"
            ],
            "the slot's signal, the setter's clear, the setter's own"
        );
        assert_eq!(s.take_trade_item_sends(), [TradeItemSend::Clear { id: 2 }]);
        let held = s.cursor_item().expect("the slot's item is on the cursor");
        assert_eq!((held.bag, held.slot, held.item_id), (0, 1, 2589));
        assert!(locked(&s, 1));
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(2) == nil")
            .unwrap());
        // On an empty slot the held item's clear is the full one (`0x587c0f` is not reached: the slot takes it).
        s.run("ClickTradeButton(3)").unwrap();
        assert!(s.trade_offers_slot(2) && !s.trade_offers_slot(1));
    }

    /// A held item on a filled slot swaps: the slot's own item goes up and neither unlocks (`0x587c41` with the placement made).
    #[test]
    fn a_held_item_on_a_filled_slot_swaps_and_both_stay_locked() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1) PickupContainerItem(0, 2)")
            .unwrap();
        s.take_trade_item_sends();
        s.run("ClickTradeButton(1)").unwrap();
        assert_eq!(
            s.take_trade_item_sends(),
            [TradeItemSend::Set {
                id: 1,
                bag: 0,
                slot: 2
            }],
            "the placement alone goes out"
        );
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(1));
        assert!(locked(&s, 1) && locked(&s, 2));
    }

    /// The remove click empties the slot, then unlocks its item (`0x587a51`, `0x587a58`), and leaves the cursor alone.
    #[test]
    fn the_remove_click_unlocks_after_the_slot_empties() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(2)")
            .unwrap();
        s.take_trade_item_sends();
        fired();
        s.run("ClickTradeButton(2, true)").unwrap();
        assert_eq!(
            fired(),
            [
                "TRADE_PLAYER_ITEM_CHANGED",
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED"
            ]
        );
        assert_eq!(s.take_trade_item_sends(), [TradeItemSend::Clear { id: 2 }]);
        assert!(!locked(&s, 1) && s.cursor_payload().is_none());
        // An empty slot's remove click does nothing (`0x587a40`).
        s.run("ClickTradeButton(2, 1)").unwrap();
        assert!(fired().is_empty() && s.take_trade_item_sends().is_empty());
    }

    /// The slot's own item, or nothing on nothing, is the full clear (`0x587c0f`): a held spell is dropped.
    #[test]
    fn nothing_on_an_empty_slot_clears_the_cursor() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.model_mut().cursor = Some(CursorPayload::Spell(crate::script::cursor::CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        fired();
        s.run("ClickTradeButton(1)").unwrap();
        assert_eq!(fired(), ["CURSOR_UPDATE"]);
        assert!(s.cursor_payload().is_none());
        assert!(s.take_trade_item_sends().is_empty());
    }

    /// A slot past the seven refuses the placement (`0x5869aa`), so the setter's clear unlocks the held item (`0x587c41`).
    #[test]
    fn a_slot_past_the_seven_drops_and_unlocks_the_held_item() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        for index in [8, 0] {
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert!(locked(&s, 1));
            s.run(&format!("ClickTradeButton({index})")).unwrap();
            assert!(s.cursor_payload().is_none() && !locked(&s, 1));
            assert!(s.take_trade_item_sends().is_empty());
        }
        assert!(
            s.run("ClickTradeButton('x')").is_err(),
            "a non-number raises"
        );
    }

    /// The six traded slots gate the held item in the reference's order (`0x587b01`, `0x587b5d`, `0x587b95`, `0x587bc2`), each refusal leaving it held and locked; the seventh takes it and signals the bind (`0x586a3d`).
    #[test]
    fn the_traded_slots_refuse_a_bound_item_and_the_seventh_takes_it() {
        let cases: [(TradeItemFacts, &str); 5] = [
            (
                TradeItemFacts {
                    bound: true,
                    quest_bound: true,
                    soulbound_enchant: true,
                    ..Default::default()
                },
                "ERR_TRADE_QUEST_ITEM",
            ),
            (
                TradeItemFacts {
                    bound: true,
                    soulbound_enchant: true,
                    bag_items: Some(2),
                    ..Default::default()
                },
                "ERR_TRADE_BOUND_ITEM",
            ),
            (
                // Inside its trade window a bound item is refused only for its enchant.
                TradeItemFacts {
                    bound: true,
                    trade_stamp: Some(100),
                    soulbound_enchant: true,
                    bag_items: Some(2),
                    ..Default::default()
                },
                "ERR_TRADE_TEMP_ENCHANT_BOUND",
            ),
            (
                TradeItemFacts {
                    bag_items: Some(2),
                    ..Default::default()
                },
                "ERR_TRADE_BAG",
            ),
            (
                TradeItemFacts {
                    bound: true,
                    trade_stamp: Some(100),
                    ..Default::default()
                },
                "",
            ),
        ];
        for (facts, line) in cases {
            let mut s = UiScript::new().unwrap();
            open(&mut s);
            s.model_mut().trade_item_facts.insert((0, 1), facts);
            s.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
                .unwrap();
            if line.is_empty() {
                assert!(s.take_shown().is_empty() && s.trade_offers_slot(0));
                continue;
            }
            assert_eq!(s.take_shown(), [line]);
            assert!(s.cursor_item().is_some() && locked(&s, 1), "{line}");
            assert!(s.take_trade_item_sends().is_empty(), "{line}");
            fired();
            s.run("ClickTradeButton(7)").unwrap();
            assert_eq!(
                crate::script::tick::fired::take()[0],
                (
                    "TRADE_POTENTIAL_BIND_ENCHANT".to_string(),
                    vec![ScriptValue::Bool(false)]
                ),
                "{line}: bound for the trade, so no bind is offered"
            );
            assert!(s.trade_offers_slot(6) && s.take_shown().is_empty());
        }
    }

    /// A worn bag is refused where an empty bag out of a pack is not (`0x587bc7`).
    #[test]
    fn a_worn_bag_is_refused_and_an_empty_one_from_the_pack_is_placed() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        let empty_bag = TradeItemFacts {
            bag_items: Some(0),
            ..Default::default()
        };
        s.model_mut().trade_item_facts.insert((0, 4), empty_bag);
        s.model_mut()
            .trade_item_facts
            .insert((cursor::EQUIPMENT_BAG, 20), empty_bag);
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            bag: cursor::EQUIPMENT_BAG,
            slot: 20,
            item_id: 4500,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: vec![20, 21, 22, 23],
            bar_placeable: true,
        }));
        s.run("ClickTradeButton(1)").unwrap();
        assert_eq!(s.take_shown(), ["ERR_TRADE_EQUIPPED_BAG"]);
        s.run("ClearCursor() PickupContainerItem(0, 4) ClickTradeButton(1)")
            .unwrap();
        assert!(s.take_shown().is_empty() && s.trade_offers_slot(0));
    }

    /// The offer's unlock walks every filled slot and leaves the slots standing (`0x5864b3`).
    #[test]
    fn the_offers_unlock_releases_each_item_and_keeps_the_slots() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run(
            "PickupContainerItem(0, 1) ClickTradeButton(1) PickupContainerItem(0, 2) ClickTradeButton(5)",
        )
        .unwrap();
        fired();
        s.unlock_trade_items();
        assert_eq!(
            fired(),
            [
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED",
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED"
            ]
        );
        assert!(!locked(&s, 1) && !locked(&s, 2));
        assert!(s.trade_offers_slot(0) && s.trade_offers_slot(4));
        // A second walk finds nothing locked; the window's open zeroes the slots (`0x5870a0`).
        s.unlock_trade_items();
        assert!(fired().is_empty());
        s.reset_trade_offer();
        assert!(!s.trade_offers_slot(0) && !s.trade_offers_slot(4));
    }

    /// The server's refusal of a slot unlocks its item, zeroes the slot and signals it, in that order (`0x586455`, `0x58648c`).
    #[test]
    fn a_denied_slot_unlocks_its_item_then_signals() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(3)")
            .unwrap();
        fired();
        s.deny_trade_item(2);
        assert_eq!(
            fired(),
            [
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED",
                "TRADE_PLAYER_ITEM_CHANGED"
            ]
        );
        assert!(!locked(&s, 1) && !s.trade_offers_slot(2));
        assert!(s
            .eval::<bool>("return GetTradePlayerItemInfo(3) == nil")
            .unwrap());
        // An empty slot's refusal still signals (`0x586410` has no gate).
        s.deny_trade_item(2);
        assert_eq!(fired(), ["TRADE_PLAYER_ITEM_CHANGED"]);
    }

    /// An offered item's object ending sends the clear and signals the slot (`0x586aa0`); another item's does nothing.
    #[test]
    fn a_destroyed_item_leaves_the_offer() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 2) ClickTradeButton(7)")
            .unwrap();
        s.take_trade_item_sends();
        fired();
        s.trade_item_destroyed(0x4000_0000_0000_0001);
        assert!(fired().is_empty() && s.take_trade_item_sends().is_empty());
        s.trade_item_destroyed(0x4000_0000_0000_0002);
        assert_eq!(
            fired(),
            ["TRADE_POTENTIAL_BIND_ENCHANT", "TRADE_PLAYER_ITEM_CHANGED"]
        );
        assert_eq!(s.take_trade_item_sends(), [TradeItemSend::Clear { id: 7 }]);
        assert!(!s.trade_offers_slot(6));
    }

    /// `UseContainerItem` with the window open adds the item to the first free traded slot and locks it after the slot's signal (`0x587918`, `0x587922`); a bound one goes to the seventh, a bound quest item is refused (`0x587815`).
    #[test]
    fn a_right_click_adds_the_item_to_the_offer_and_locks_it() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1) ClearCursor()")
            .unwrap();
        s.take_trade_item_sends();
        fired();
        s.run("UseContainerItem(0, 2)").unwrap();
        assert_eq!(
            fired(),
            [
                "CURSOR_UPDATE",
                "TRADE_PLAYER_ITEM_CHANGED",
                "ITEM_LOCK_CHANGED",
                "ITEM_LOCKED"
            ]
        );
        assert_eq!(
            s.take_trade_item_sends(),
            [TradeItemSend::Set {
                id: 2,
                bag: 0,
                slot: 2
            }]
        );
        assert!(locked(&s, 2) && s.take_script_calls().is_empty());

        let bound = TradeItemFacts {
            bound: true,
            ..Default::default()
        };
        s.model_mut().trade_item_facts.insert((0, 3), bound);
        s.run("UseContainerItem(0, 3)").unwrap();
        assert!(s.trade_offers_slot(6) && locked(&s, 3));
        // The seventh slot is taken: a second bound item goes nowhere, silently (`0x587831`).
        s.model_mut().trade_item_facts.insert((0, 4), bound);
        s.run("UseContainerItem(0, 4)").unwrap();
        assert!(!locked(&s, 4) && s.take_shown().is_empty());

        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.model_mut().trade_item_facts.insert(
            (0, 3),
            TradeItemFacts {
                quest_bound: true,
                ..bound
            },
        );
        s.run("UseContainerItem(0, 3)").unwrap();
        assert_eq!(s.take_shown(), ["ERR_TRADE_QUEST_ITEM"]);
        assert!(!locked(&s, 3) && !s.trade_offers_slot(6));
    }

    /// An enchanting item aimed at the partner's seventh slot is used, then locked (`0x586c48`, `0x586c57`), and released by the next one (`0x586c20`), the partner's slot changing (`0x586fb6`), the partner setter's close (`0x5870d5`, `0x5880f4`) and `CloseTrade` (`0x587985`).
    #[test]
    fn the_enchanting_item_locks_until_each_of_its_releases() {
        let enchants = TradeItemFacts {
            enchants_on_use: true,
            ..Default::default()
        };
        let setup = || {
            let mut s = UiScript::new().unwrap();
            pack(&mut s);
            let mut st = TradeState::default();
            st.target.slots[6] = Some(item(6217, 1, 1));
            s.set_trade(Some(st));
            s.model_mut().trade_item_facts.insert((0, 1), enchants);
            s.model_mut().trade_item_facts.insert((0, 2), enchants);
            s.run("UseContainerItem(0, 1)").unwrap();
            assert_eq!(
                s.take_script_calls(),
                [crate::script::ScriptCall::UseContainerItem { bag: 0, slot: 1 }],
                "the item's own use"
            );
            assert!(locked(&s, 1) && !s.trade_offers_slot(0));
            fired();
            s
        };

        let s = setup();
        s.run("UseContainerItem(0, 2)").unwrap();
        assert!(!locked(&s, 1) && locked(&s, 2), "the next one takes over");

        let mut s = setup();
        s.release_trade_enchanting_item();
        assert_eq!(fired(), ["ITEM_LOCK_CHANGED", "ITEM_UNLOCKED"]);
        assert!(!locked(&s, 1));
        s.release_trade_enchanting_item();
        assert!(fired().is_empty(), "released once");

        let mut s = setup();
        s.close_trade_partner(false);
        assert!(!locked(&s, 1));

        let mut s = setup();
        s.run("CloseTrade()").unwrap();
        assert_eq!(
            fired(),
            [
                "PLAYER_TRADE_MONEY",
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED",
                "TRADE_CLOSED"
            ],
            "the coins, the release, the close (`0x587957`, `0x587985`, `0x5879bc`)"
        );
        assert!(!locked(&s, 1) && s.take_trade_close());

        // With no item in the partner's seventh slot the same item joins our offer (`0x5d89f9`).
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.model_mut().trade_item_facts.insert((0, 1), enchants);
        s.run("UseContainerItem(0, 1)").unwrap();
        assert!(s.trade_offers_slot(0) && s.take_script_calls().is_empty());
    }

    /// `CloseTrade` signals the coins unless the last close was a completion (`0x587949`), and the cancel goes out with no window too (`0x5879c4`).
    #[test]
    fn close_trade_skips_the_coins_after_a_completed_trade() {
        let mut s = UiScript::new().unwrap();
        fired();
        s.run("CloseTrade()").unwrap();
        assert_eq!(fired(), ["PLAYER_TRADE_MONEY"]);
        assert!(s.take_trade_close());
        s.close_trade_partner(true);
        s.run("CloseTrade()").unwrap();
        assert!(fired().is_empty() && s.take_trade_close());
        // The next window's open zeroes the flag (`0x5870a0`).
        s.reset_trade_offer();
        s.run("CloseTrade()").unwrap();
        assert_eq!(fired(), ["PLAYER_TRADE_MONEY"]);
    }

    #[test]
    fn the_partners_column_takes_coins_or_the_enchant_target_alone() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTargetTradeButton(1)")
            .unwrap();
        assert!(s.take_trade_item_sends().is_empty());
        assert!(
            s.cursor_item().is_some(),
            "a partner-side click leaves the cursor untouched"
        );
        assert!(
            s.run("ClickTargetTradeButton()").is_err(),
            "a non-number raises (`0x586c9c`)"
        );
    }

    /// The enchanting item's use runs inside the call between the unlock of the one before and its own lock (`0x586c20`, `0x586c48`, `0x586c57`).
    #[test]
    fn the_enchanting_items_use_runs_between_the_unlock_and_the_lock() {
        let enchants = TradeItemFacts {
            enchants_on_use: true,
            ..Default::default()
        };
        let mut s = UiScript::new().unwrap();
        pack(&mut s);
        let mut st = TradeState::default();
        st.target.slots[6] = Some(item(6217, 1, 1));
        s.set_trade(Some(st));
        s.model_mut().trade_item_facts.insert((0, 1), enchants);
        s.model_mut().trade_item_facts.insert((0, 2), enchants);
        s.run("UseContainerItem(0, 1)").unwrap();
        s.take_script_calls();
        fired();
        crate::script::calls::in_call::run(&mut s, None, "UseContainerItem(0, 2)");
        assert_eq!(
            fired(),
            [
                "CURSOR_UPDATE",
                "ITEM_LOCK_CHANGED",
                "ITEM_UNLOCKED",
                "APPLIED UseContainerItem { bag: 0, slot: 2 }",
                "ITEM_LOCK_CHANGED",
                "ITEM_LOCKED",
            ]
        );
        assert!(
            s.take_script_calls().is_empty(),
            "nothing waits for the press pass"
        );
        assert!(!locked(&s, 1) && locked(&s, 2));
    }
}
