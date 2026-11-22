//! The trade bindings: the window's own state (`0xbfa400`-`0xbfa684`), written by the app's trade
//! handlers at their step and by the verbs in their call, and the trade verbs, which read it and
//! the network cells at the call; the verbs' sends queue as intents the app drains. Slots are
//! 1-based, 1..=7, the seventh the non-traded
//! enchant slot; an empty or out-of-range slot answers nil. The partner rides the `"npc"` unit,
//! which the app points at them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorItem, CursorPayload};
use super::{Model, ScriptValue};

/// Slots per side, six traded and the non-traded enchant slot: vmangos `TRADE_SLOT_COUNT`
/// (`TradeData.h`), kept equal to the protocol crate's, which this crate does not depend on.
pub const TRADE_SLOTS: usize = 7;

/// One slot of the partner's offer as the window keeps it (`0xbfa400`-`0xbfa5c0`), the wire's
/// item block; an empty slot is all zero, as the packet's `memset` leaves it (`0x70489a`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TradeTargetItem {
    pub item_id: u32,
    pub count: u32,
    pub enchant: u32,
    pub gems: [u32; 3],
    pub creator: u64,
    pub wrapped: bool,
    pub gift_creator: u64,
    pub charges: i32,
    pub suffix_factor: u32,
    pub random_property: i32,
    pub lock_id: u32,
    pub max_durability: u32,
    pub durability: u32,
}

/// The trade's network cells a verb reads at the call: the partner's offered coins (`0xca0b78`) and
/// each window's enchant spell, ours then the partner's (`0xca0ff0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TradeCells {
    pub target_gold: u32,
    pub enchant_spells: [u32; 2],
}

/// The trade's network cells at the call; with no view lent, a fixture's.
fn cells(model: &Model) -> TradeCells {
    if let Some(Some(cells)) = super::read_live(|l| l.trade_cells()) {
        return cells;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model.trade_cells
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        TradeCells::default()
    }
}

impl super::UiScript {
    /// A fixture's trade network cells, read where no view is lent.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_trade_cells(&mut self, cells: TradeCells) {
        self.model_mut().trade_cells = cells;
    }

    /// Drain the tokens `InitiateTrade` queued; the app sends `CMSG_INITIATE_TRADE` for each.
    pub fn take_trade_initiates(&mut self) -> Vec<String> {
        drain_field!(self, trade_initiates)
    }

    /// The accept sends since the last drain, in call order: `true` `CMSG_ACCEPT_TRADE` (`0x703f50`), `false` `CMSG_UNACCEPT_TRADE` (`0x703fd0`).
    pub fn take_trade_accept_sends(&mut self) -> Vec<bool> {
        drain_field!(self, trade_accept_sends)
    }

    /// Our accept flag and the partner's (`0xbfa660`, `0xbfa664`).
    #[cfg(any(test, feature = "fixtures"))]
    pub fn trade_accepts(&self) -> (bool, bool) {
        self.model_ref().trade_accepts
    }

    /// A fixture's accept flags, unsignalled.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_trade_accepts(&mut self, accepts: (bool, bool)) {
        if self.model_ref().trade_accepts != accepts {
            self.model_mut().trade_accepts = accepts;
        }
    }

    /// Our offered coins as a status or our own snapshot leaves them (`0x704545`, `0x70457c`, `0x70459f`, `0x7045eb`, `0x7046ff`).
    pub fn set_trade_offer_gold(&mut self, copper: u32) {
        if self.model_ref().trade_offer_gold != copper {
            self.model_mut().trade_offer_gold = copper;
        }
    }

    /// Our offered coins (`0xca0b74`), whatever the window's state.
    pub fn trade_offer_gold(&self) -> u32 {
        self.model_ref().trade_offer_gold
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

    /// The coin offers sent since the last drain, in call order, each a `CMSG_SET_TRADE_GOLD` (`0x704463`, `0x7042d8`, `0x704382`).
    pub fn take_trade_money_sends(&mut self) -> Vec<u32> {
        drain_field!(self, trade_set_money)
    }

    /// The last of [`Self::take_trade_money_sends`], which it empties.
    pub fn take_trade_money(&mut self) -> Option<u32> {
        self.take_trade_money_sends().pop()
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

/// `GetPlayerTradeMoney`'s read (`0x586780`): our offered coins while a partner is set, else 0.
fn own_gold(model: &Model) -> u32 {
    if model.trade_partner.is_some() {
        model.trade_offer_gold
    } else {
        0
    }
}

/// The 0-based slot a trade verb's index names: a non-number raises `usage`, and `trunc(index) - 1` past the seven names none (`0x587c9e`, `0x587eed`).
fn verb_slot(lua: &Lua, index: Value, usage: &'static str) -> mlua::Result<Option<usize>> {
    let n = super::binding_abi::number_arg(lua, index, usage)?;
    Ok(usize::try_from(n.wrapping_sub(1))
        .ok()
        .filter(|&i| i < TRADE_SLOTS))
}

/// Our offer's slot `i` as its item object reads at the call: the bag slot its guid sits in (`0xbfa620`), none once the item is gone.
fn own_held(model: &Model, i: usize) -> Option<super::ContainerSlot> {
    let own = model.trade_own_items[i].as_ref()?;
    let held = super::container::live_slot(model, own.item.bag, own.item.slot)?;
    (held.item_id != 0 && (own.guid == 0 || held.guid == own.guid)).then_some(held)
}

/// A missed record's ask with the trade's callback left on it (`0x67ca30` with `0x586080`), one per read.
fn wait_trade_record(model: &mut crate::script::ModelGuard<'_>, item_id: u32) {
    super::item_stats::ask_item_record(model, item_id);
    super::item_stats::wait_item_record(model, item_id, super::ItemRecordWait::Trade);
}

/// An item's quality off its record read with the trade's callback (`0x587fe0`): -1 for a record with no inventory type or none cached (`0x587fe9`).
fn record_quality(model: &mut crate::script::ModelGuard<'_>, item_id: u32) -> i64 {
    match super::item_stats::item_record(model, item_id) {
        Some(rec) if rec.inventory_type != 0 => i64::from(rec.quality),
        Some(_) => -1,
        None => {
            wait_trade_record(model, item_id);
            -1
        }
    }
}

/// A trade item verb's answer for no item: `nil, nil, 0, 0` and `nil` up to `n` values.
fn miss_tuple(n: usize) -> mlua::Result<MultiValue> {
    let mut out = vec![Value::Nil, Value::Nil, Value::Integer(0), Value::Integer(0)];
    out.resize(n, Value::Nil);
    Ok(MultiValue::from_vec(out))
}

fn opt_string(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Slot 7's enchant text: the window spell's record name, nil with none (`0x587e05`, `0x588019`).
fn enchant_name(lua: &Lua, spell: Option<u32>) -> mlua::Result<Value> {
    let name = spell
        .filter(|&id| id != 0)
        .and_then(|id| super::spell_info::with_spell_source(lua, |s| s.spell_info(id, false)))
        .flatten()
        .map(|info| info.name);
    opt_string(lua, name.as_deref())
}

/// What a trade tooltip shows of a slot, read at the call.
pub(crate) struct TradeShown {
    pub(crate) item_id: u32,
    pub(crate) name: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) count: u32,
}

/// The 1-based slot `slot` of our offer (its item object) or of the partner's (the window's copy, its link built off the record when cached); `None` for an empty or out-of-range slot.
pub(crate) fn shown_slot(lua: &Lua, player_side: bool, slot: usize) -> Option<TradeShown> {
    let i = slot.checked_sub(1).filter(|&i| i < TRADE_SLOTS)?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    if player_side {
        let held = own_held(&model, i)?;
        return Some(TradeShown {
            item_id: held.item_id,
            name: Some(cursor::item_link_name(held.link.as_deref())).filter(|n| !n.is_empty()),
            link: held.link,
            count: held.count,
        });
    }
    let slot = model.trade_target[i];
    if slot.item_id == 0 {
        return None;
    }
    let rec = super::item_stats::item_record(&model, slot.item_id);
    let level = super::item_stats::player_level();
    drop(model);
    let (name, link) = match rec {
        Some(rec) => {
            let name = super::item_stats::rolled_name(lua, &rec.name, slot.random_property);
            let fields = super::ItemLinkFields {
                enchant: slot.enchant as i32,
                gems: slot.gems.map(|g| g as i32),
                random_property: slot.random_property,
                seed: slot.suffix_factor as i32,
                ..Default::default()
            };
            let link = super::item_hyperlink(slot.item_id, rec.quality, &fields, level, &name);
            (Some(name), Some(link))
        }
        None => (None, None),
    };
    Some(TradeShown {
        item_id: slot.item_id,
        name,
        link,
        count: slot.count,
    })
}

/// An item's put-down sound (`0x4d0020` with 1): a wrapped item's own kit (`0x4d006d`), else its sound group's; nothing with no item object (`0x587a75`).
fn put_down_sound(model: &mut Model, item: &CursorItem) {
    if cursor::guid_at(model, item.bag, item.slot).is_none() {
        return;
    }
    let wrapped =
        super::container::live_slot(model, item.bag, item.slot).is_some_and(|s| s.flags & 0x8 != 0);
    model.sound_queue.push(if wrapped {
        super::SoundRequest::KitName("(DONOTRENAME)WrappedItemDrop".to_string())
    } else {
        super::SoundRequest::ItemPutDown(item.item_id)
    });
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
    model.trade_own_items[i] = entry;
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
        // The slot empties, its item unlocks and its put-down sound plays (`0x587a51`, `0x587a58`, `0x587a7e`).
        if let Some(old) = own_item(model, slot) {
            set_own_item(model, slot, None);
            cursor::set_item_lock(model, old.item.bag, old.item.slot, false);
            put_down_sound(model, &old.item);
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

/// The offer's add of a bag item (`0x5877b0`): an unbound item to the first free traded slot, a bound one to slot 7 unless a quest item, past the bag gates; then the slot's write, the item's lock and its put-down sound (`0x587918`, `0x587922`, `0x58792a`).
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
    set_own_item(model, slot, Some(item.clone()));
    cursor::set_item_lock(model, bag, bag_slot, true);
    put_down_sound(model, &item);
    Ok(())
}

/// The open leg's cursor arm (`0x587280`-`0x587391`): held coins fold into the offer and signal, else a held item goes out to its slot.
fn open_with_cursor(model: &mut crate::script::ModelGuard<'_>) {
    if cursor::money::cursor_money(model) != 0 {
        if cursor::money::add_trade_money(model) {
            model.trade_last_money[0] = model.trade_offer_gold;
            model.raise("PLAYER_TRADE_MONEY", Vec::new());
            model.raise("PLAYER_MONEY", Vec::new());
        }
        return;
    }
    let Some(CursorPayload::Item(item)) = model.cursor.clone() else {
        return;
    };
    let Some(guid) = cursor::guid_at(model, item.bag, item.slot) else {
        return;
    };
    let facts = item_facts(model, item.bag, item.slot);
    let i = if facts.bound { 6 } else { 0 };
    if facts.bound {
        let binds = !facts.bound_for_trade(model);
        model.raise(
            "TRADE_POTENTIAL_BIND_ENCHANT",
            vec![ScriptValue::Bool(binds)],
        );
    }
    model.trade_item_sends.push(TradeItemSend::Set {
        id: i as u32 + 1,
        bag: item.bag,
        slot: item.slot,
    });
    cursor::clear_cursor_with(model, false, true);
    model.trade_own_items[i] = Some(TradeOwnItem { guid, item });
}

/// Our accept's drop (`0x586930`): a raised flag drops, signals and sends `CMSG_UNACCEPT_TRADE` (`0x586959`, `0x586963`, `0x58696b`), past the action gate (`0x586932`), which only tainted code fails and a packet handler never is; then a raised partner flag drops and signals (`0x58698c`, `0x586996`).
fn player_accept(model: &mut crate::script::ModelGuard<'_>) {
    if model.trade_accepts.0 {
        model.trade_accepts.0 = false;
        let theirs = model.trade_accepts.1;
        model.raise(
            "TRADE_ACCEPT_UPDATE",
            vec![ScriptValue::Int(0), ScriptValue::Int(i64::from(theirs))],
        );
        model.trade_accept_sends.push(false);
    }
    if model.trade_accepts.1 {
        model.trade_accepts.1 = false;
        let ours = model.trade_accepts.0;
        model.raise(
            "TRADE_ACCEPT_UPDATE",
            vec![ScriptValue::Int(i64::from(ours)), ScriptValue::Int(0)],
        );
    }
}

/// The enchanting item's release (`0x586c20`, `0x586fb6`, `0x5870d5`, `0x587985`, `0x5880f4`): it unlocks and its slot zeroes.
pub(crate) fn release_enchanting_item(model: &mut crate::script::ModelGuard<'_>) {
    if let Some((_, bag, slot)) = model.trade_enchanting_item.take() {
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
    if model.trade_partner.is_none() {
        return Arm::Pass;
    }
    let Some(s) = super::container::index_slot(model, bag, slot).filter(|s| s.item_id != 0) else {
        return Arm::Pass;
    };
    // The partner's seventh slot in the window's own copy (`0x5d897e` reads `0xbfa5d8`).
    let partner_holds = model.trade_target[6].item_id != 0;
    if partner_holds && item_facts(model, bag, slot).enchants_on_use {
        release_enchanting_item(model);
        model.call(super::ScriptCall::UseContainerItem { bag, slot });
        cursor::set_item_lock(model, bag, slot, true);
        let guid = cursor::guid_at(model, bag, slot).unwrap_or(0);
        model.trade_enchanting_item = Some((guid, bag, slot));
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

    /// The server's refusal of our slot `slot`, 0-based (`0x586410`): its item unlocks (`0x586455`), the slot zeroes and `TRADE_PLAYER_ITEM_CHANGED` fires (`0x58648c`); `0xff`, the coins, zeroes the coins last signalled and signals `PLAYER_TRADE_MONEY` then `PLAYER_MONEY` (`0x586428`, `0x58642e`, `0x586439`).
    pub fn deny_trade_item(&mut self, slot: u8) {
        let mut model = self.model_mut();
        if slot == 0xff {
            model.trade_last_money[0] = 0;
            model.raise("PLAYER_TRADE_MONEY", Vec::new());
            model.raise("PLAYER_MONEY", Vec::new());
            return;
        }
        let i = usize::from(slot);
        if i >= TRADE_SLOTS {
            return;
        }
        if let Some(own) = model.trade_own_items[i].take() {
            cursor::set_item_lock(&mut model, own.item.bag, own.item.slot, false);
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

    /// The partner setter's open leg (`0x587124`-`0x5873c4`), past the interact target's set (`0x587124`): the partner kept, both accept flags, the enchant spells and coins last signalled and the completion flag zero (`0x587143`-`0x587167`), then each slot's target id, count and enchant (`0x58716d`-`0x5871d1`) and our slots (`0x5871d6`-`0x58727a`). When our own initiate asked for the cursor (`0x703c30`), held coins fold into the offer, are signalled with `PLAYER_TRADE_MONEY` then `PLAYER_MONEY` (`0x587297`-`0x5872ca`) and `TRADE_SHOW` follows (`0x5872d8`); else a held item goes out to slot 1, or to slot 7 past `TRADE_POTENTIAL_BIND_ENCHANT` when bound (`0x58735e`, `0x58737b`), the cursor clearing with the item still locked (`0x587383`) and the slot written unsignalled (`0x587391`). `TRADE_SHOW` ends it (`0x5873c4`).
    pub fn open_trade_partner(&mut self, partner: u64, use_cursor: bool) {
        let mut model = self.model_mut();
        model.trade_partner = Some(partner);
        model.trade_accepts = (false, false);
        model.trade_last_enchants = [0; 2];
        model.trade_last_money = [0; 2];
        model.trade_completed = false;
        for slot in &mut model.trade_target {
            slot.item_id = 0;
            slot.count = 0;
            slot.enchant = 0;
        }
        model.trade_own_items = Default::default();
        if use_cursor {
            open_with_cursor(&mut model);
        }
        model.raise("TRADE_SHOW", Vec::new());
    }

    /// The UI world entry's trade part (`0x528010` -> `0x5860a0`): the partner, both accept flags, the enchant spells and coins last signalled, the whole target copy and our slots zero.
    pub fn enter_trade_world(&mut self) {
        let mut model = self.model_mut();
        model.trade_partner = None;
        model.trade_accepts = (false, false);
        model.trade_last_enchants = [0; 2];
        model.trade_last_money = [0; 2];
        model.trade_target = Default::default();
        model.trade_own_items = Default::default();
    }

    /// The offer update at the end of the partner's or our own snapshot (`0x7048b2` -> `0x586dc0`), over the partner's window as the packet left it and the cells it wrote. Our accept drops first (`0x586dc7` -> `0x586930`); then each partner slot that differs from the window's copy is copied and signals `TRADE_TARGET_ITEM_CHANGED` (`0x586f96`), the seventh releasing the enchanting item (`0x586fb6`); then each window's enchant spell, coins of ours and of the partner's that moved since last signalled (`0x587008`, `0x58703a`, `0x587060`-`0x58706c`, `0x587092`).
    pub fn trade_updated(&mut self, target: [TradeTargetItem; TRADE_SLOTS], cells: TradeCells) {
        let mut model = self.model_mut();
        player_accept(&mut model);
        for (i, slot) in target.into_iter().enumerate() {
            if model.trade_target[i] == slot {
                continue;
            }
            model.trade_target[i] = slot;
            model.raise(
                "TRADE_TARGET_ITEM_CHANGED",
                vec![ScriptValue::Int(i as i64 + 1)],
            );
            if i == 6 {
                release_enchanting_item(&mut model);
            }
        }
        if model.trade_last_enchants[0] != cells.enchant_spells[0] {
            model.trade_last_enchants[0] = cells.enchant_spells[0];
            model.raise("TRADE_TARGET_ITEM_CHANGED", vec![ScriptValue::Int(7)]);
        }
        if model.trade_last_enchants[1] != cells.enchant_spells[1] {
            model.trade_last_enchants[1] = cells.enchant_spells[1];
            model.raise("TRADE_PLAYER_ITEM_CHANGED", vec![ScriptValue::Int(7)]);
        }
        if model.trade_last_money[0] != model.trade_offer_gold {
            model.trade_last_money[0] = model.trade_offer_gold;
            model.raise("PLAYER_TRADE_MONEY", Vec::new());
            model.raise("PLAYER_MONEY", Vec::new());
        }
        if model.trade_last_money[1] != cells.target_gold {
            model.trade_last_money[1] = cells.target_gold;
            model.raise("TRADE_MONEY_CHANGED", Vec::new());
        }
    }

    /// The coinage watch's trade tail (`0x6da0bc`-`0x6da104` -> `0x704320`): an offer past the new purse drops to it, kept and sent, with no event.
    pub fn trim_trade_offer(&mut self, coinage: u32) {
        let mut model = self.model_mut();
        if model.trade_offer_gold > coinage {
            model.trade_offer_gold = coinage;
            model.trade_set_money.push(coinage);
        }
    }

    /// `TRADE_ACCEPT` and `REJECTED` (`0x5874d9`, `0x5874e7` -> `0x586390`): the partner's flag, signalled with ours when it moves.
    pub fn set_trade_target_accepted(&mut self, accepted: bool) {
        let mut model = self.model_mut();
        if model.trade_accepts.1 == accepted {
            return;
        }
        model.trade_accepts.1 = accepted;
        let ours = model.trade_accepts.0;
        model.raise(
            "TRADE_ACCEPT_UPDATE",
            vec![
                ScriptValue::Int(i64::from(ours)),
                ScriptValue::Int(i64::from(accepted)),
            ],
        );
    }

    /// `BACK_TO_TRADE` (`0x5874f3` -> `0x586930`): both accepts drop, as an offer update's does.
    pub fn trade_back_to_trade(&mut self) {
        player_accept(&mut self.model_mut());
    }

    /// A cancelled cast's item (`0x80624b`-`0x806258`): when it is the enchanting item, that unlocks and its slot zeroes.
    pub fn cast_item_cancelled(&mut self, item: u64) {
        let ours = self
            .model_ref()
            .trade_enchanting_item
            .is_some_and(|(guid, ..)| guid != 0 && guid == item);
        if ours {
            release_enchanting_item(&mut self.model_mut());
        }
    }

    /// The enchanting item unlocks (`0x586fb6`): the partner's seventh slot changed.
    pub fn release_trade_enchanting_item(&mut self) {
        if self.model_ref().trade_enchanting_item.is_some() {
            release_enchanting_item(&mut self.model_mut());
        }
    }

    /// The partner setter's close, ahead of its interact target's clear (`0x5870a0`): with a partner set, the enchanting item unlocks (`0x5870d5`) and the completion flag is kept for `CloseTrade` (`0x5870f2`).
    pub fn close_trade_partner(&mut self, completed: bool) {
        if self.model_ref().trade_partner.is_none() {
            return;
        }
        self.release_trade_enchanting_item();
        if self.model_ref().trade_completed != completed {
            self.model_mut().trade_completed = completed;
        }
    }

    /// The partner setter's close past the interact target's clear: the partner zeroes (`0x587102`) and `TRADE_CLOSED` fires (`0x58710e`), with a partner set.
    pub fn trade_partner_closed(&mut self) {
        let mut model = self.model_mut();
        if model.trade_partner.take().is_some() {
            model.raise("TRADE_CLOSED", Vec::new());
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

    // GetTradePlayerItemInfo(index) (`0x587eb0`): our slot's item object read at the call, its full name, icon and stack count, then its record's quality (-1 for a record with no inventory type, or none cached, which leaves the trade's callback), then slot 7's enchant name off the partner's window spell (`0x588017`); a slot with no item object answers `nil, nil, 0, 0, nil`.
    g.set(
        "GetTradePlayerItemInfo",
        lua.create_function(|lua, index: Value| {
            let i = verb_slot(lua, index, "Usage: GetTradePlayerItemInfo(index)")?;
            let read = {
                let mut model = crate::script::model_mut_of(lua);
                i.and_then(|i| own_held(&model, i)).map(|held| {
                    let quality = record_quality(&mut model, held.item_id);
                    let spell = (i == Some(6)).then(|| cells(&model).enchant_spells[1]);
                    (held, quality, spell)
                })
            };
            let Some((held, quality, spell)) = read else {
                return miss_tuple(5);
            };
            let name = cursor::item_link_name(held.link.as_deref());
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                opt_string(lua, held.texture.as_deref())?,
                Value::Integer(i64::from(held.count)),
                Value::Integer(quality),
                enchant_name(lua, spell)?,
            ]))
        })?,
    )?;

    // GetTradeTargetItemInfo(index) (`0x587c60`): the partner's slot in the window's copy, its record read with the trade's callback (`0x587ce4`): the rolled name, the record's icon, the copy's count, the record's quality (-1 with no inventory type), 1 or nil for usable, then slot 7's enchant name off our window's spell (`0x587e04`); an empty slot or a record not cached answers `nil, nil, 0, 0, nil, nil`.
    g.set(
        "GetTradeTargetItemInfo",
        lua.create_function(|lua, index: Value| {
            let i = verb_slot(lua, index, "Usage: GetTradeTargetItemInfo(index)")?;
            let read = {
                let mut model = crate::script::model_mut_of(lua);
                i.map(|i| (i, model.trade_target[i]))
                    .filter(|(_, slot)| slot.item_id != 0)
                    .and_then(|(i, slot)| {
                        let rec = super::item_stats::item_record(&model, slot.item_id);
                        if rec.is_none() {
                            wait_trade_record(&mut model, slot.item_id);
                        }
                        let rec = rec?;
                        let usable = super::item_stats::item_usable_by_id(&model, slot.item_id);
                        let spell = (i == 6).then(|| cells(&model).enchant_spells[0]);
                        Some((slot, rec, usable, spell))
                    })
            };
            let Some((slot, rec, usable, spell)) = read else {
                return miss_tuple(6);
            };
            let name = super::item_stats::rolled_name(lua, &rec.name, slot.random_property);
            let quality = if rec.inventory_type != 0 {
                i64::from(rec.quality)
            } else {
                -1
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                opt_string(lua, rec.icon.as_deref())?,
                Value::Integer(i64::from(slot.count)),
                Value::Integer(quality),
                flag(usable),
                enchant_name(lua, spell)?,
            ]))
        })?,
    )?;

    // GetTradePlayerItemLink(index) (`0x586d00`): our slot's item object's link, nothing with no item object.
    g.set(
        "GetTradePlayerItemLink",
        lua.create_function(|lua, index: Value| {
            let i = verb_slot(lua, index, "Usage: GetTradePlayerItemLink(index)")?;
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                i.and_then(|i| own_held(&model, i)).and_then(|h| h.link)
            };
            match link {
                Some(link) => Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(&link)?,
                )])),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // GetTradeTargetItemLink(index) (`0x5865f0`): the partner's slot in the window's copy built into a link off its record, with its enchant, gems, property and seed; nothing for an empty slot or a record not cached, which the read leaves unasked, passing no callback (`0x586662`).
    g.set(
        "GetTradeTargetItemLink",
        lua.create_function(|lua, index: Value| {
            let i = verb_slot(lua, index, "Usage: GetTradeTargetItemLink(index)")?;
            let read = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                i.map(|i| model.trade_target[i])
                    .filter(|slot| slot.item_id != 0)
                    .and_then(|slot| {
                        let rec = super::item_stats::item_record(&model, slot.item_id)?;
                        Some((slot, rec, super::item_stats::player_level()))
                    })
            };
            let Some((slot, rec, level)) = read else {
                return Ok(MultiValue::new());
            };
            let name = super::item_stats::rolled_name(lua, &rec.name, slot.random_property);
            let fields = super::ItemLinkFields {
                enchant: slot.enchant as i32,
                gems: slot.gems.map(|g| g as i32),
                random_property: slot.random_property,
                seed: slot.suffix_factor as i32,
                ..Default::default()
            };
            let link = super::item_hyperlink(slot.item_id, rec.quality, &fields, level, &name);
            Ok(MultiValue::from_vec(vec![Value::String(
                lua.create_string(&link)?,
            )]))
        })?,
    )?;

    // GetPlayerTradeMoney() (`0x586780`): our offered coins while a partner is set, else 0.
    g.set(
        "GetPlayerTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(own_gold(&model)))
        })?,
    )?;
    // GetTargetTradeMoney() (`0x5867d0`): the partner's offered coins on the wire's cell (`0xca0b78`), with or without a window.
    g.set(
        "GetTargetTradeMoney",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(cells(&model).target_gold))
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

    // AcceptTrade() and CancelTradeAccept() (`0x5866e0`, `0x586730`), past the action gate (`0x5191c0`, #1227): a flag that moves is written (`0x58670d`, `0x586759`), `TRADE_ACCEPT_UPDATE` signals with the partner's (`0x586712`, `0x586763`), then the send goes out (`0x58671a`, `0x58676b`); no window is asked for.
    for (name, accept) in [("AcceptTrade", true), ("CancelTradeAccept", false)] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                if model.trade_accepts.0 == accept {
                    return Ok(());
                }
                model.trade_accepts.0 = accept;
                let theirs = model.trade_accepts.1;
                model.raise(
                    "TRADE_ACCEPT_UPDATE",
                    vec![
                        ScriptValue::Int(i64::from(accept)),
                        ScriptValue::Int(i64::from(theirs)),
                    ],
                );
                model.trade_accept_sends.push(accept);
                Ok(())
            })?,
        )?;
    }
    g.set(
        "CloseTrade",
        lua.create_function(|lua, ()| {
            // `0x587940`: the offer's coins signal unless the last close was a completion (`0x587949`); an open window releases the enchanting item (`0x587985`), clears the interact target on the partner (`0x5879a5`), zeroes the partner and signals (`0x5879b0`, `0x5879bc`); the cancel goes out either way (`0x5879c4`).
            let mut model = crate::script::model_mut_of(lua);
            if !model.trade_completed {
                model.raise("PLAYER_TRADE_MONEY", Vec::new());
            }
            if let Some(partner) = model.trade_partner {
                release_enchanting_item(&mut model);
                model.trade_completed = false;
                super::write_live(|l| l.clear_interact_target(partner));
                model.trade_partner = None;
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
            // `0x586870` -> `0x7043c0`: a non-number raises; the low dword is the whole offer, sent and kept (`0x70445d`) only when the purse covers it (`0x70441e`), with or without a window; a refusal is silent.
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetTradeMoney(amount)",
            )? as u32;
            let mut model = crate::script::model_mut_of(lua);
            if u64::from(n) > crate::script::live::money(&model) {
                return Ok(());
            }
            model.trade_offer_gold = n;
            model.trade_set_money.push(n);
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
            drop(model);
            crate::script::calls::press_item_picks(lua);
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
            crate::script::calls::press_item_picks(lua);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    const PARTNER: u64 = 0x7;

    fn target(item_id: u32, count: u32) -> TradeTargetItem {
        TradeTargetItem {
            item_id,
            count,
            ..Default::default()
        }
    }

    fn template(name: &str, quality: u32, inventory_type: u32) -> crate::script::ItemTemplateView {
        crate::script::ItemTemplateView {
            name: name.into(),
            quality,
            inventory_type,
            icon: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            ..Default::default()
        }
    }

    /// The window's copy, not the wire, answers the partner's column; our column reads our slot's item object; both links come back as nothing past the seven or with no item (`0x5865f0`, `0x586d00`).
    #[test]
    fn the_trade_slots_answer_their_item_links() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<mlua::MultiValue>("return GetTradePlayerItemLink(1)")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            s.eval::<mlua::MultiValue>("return GetTradeTargetItemLink(1)")
                .unwrap()
                .len(),
            0
        );
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
            .unwrap();
        s.set_item_template(4306, template("Silk Cloth", 1, 0));
        let mut copy = [TradeTargetItem::default(); TRADE_SLOTS];
        copy[0] = target(4306, 2);
        copy[1] = target(4307, 1);
        s.trade_updated(copy, TradeCells::default());
        assert_eq!(
            s.eval::<String>("return GetTradePlayerItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Item 2589]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTradeTargetItemLink(1)")
                .unwrap(),
            "|cffffffff|Hitem:4306:0:0:0:0:0:0:0:0|h[Silk Cloth]|h|r"
        );
        // A record not cached, an empty slot, and one past the seven.
        for chunk in [
            "return GetTradeTargetItemLink(2)",
            "return GetTradeTargetItemLink(3)",
            "return GetTradePlayerItemLink(9)",
        ] {
            assert_eq!(
                s.eval::<mlua::MultiValue>(chunk).unwrap().len(),
                0,
                "{chunk}"
            );
        }
        assert!(
            s.run("GetTradeTargetItemLink()").is_err(),
            "a non-number raises"
        );
        // The link's read passes no callback, so a miss asks nothing (`0x586662`).
        s.take_item_stat_asks();
        s.run("GetTradeTargetItemLink(2)").unwrap();
        assert!(s.take_item_stat_asks().is_empty());
    }

    /// The item verbs' tuples (`0x587eb0`, `0x587c60`): ours is name, icon, count, quality and enchant, quality -1 for a record with no inventory type; the partner's adds usable after quality; no item answers `nil, nil, 0, 0, nil`, the partner's one more nil.
    #[test]
    fn player_and_target_item_info_read_the_reference_tuples() {
        let mut s = UiScript::new().unwrap();
        let miss: Vec<String> = s
            .eval::<mlua::MultiValue>("return GetTradePlayerItemInfo(1)")
            .unwrap()
            .iter()
            .map(|v| format!("{v:?}"))
            .collect();
        assert_eq!(miss, ["Nil", "Nil", "Integer(0)", "Integer(0)", "Nil"]);
        assert_eq!(
            s.eval::<mlua::MultiValue>("return GetTradeTargetItemInfo(1)")
                .unwrap()
                .len(),
            6
        );
        open(&mut s);
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
            .unwrap();
        s.set_item_template(2589, template("Item 2589", 1, 0));
        let (name, count, quality): (String, i64, i64) = s
            .eval("local n, t, c, q = GetTradePlayerItemInfo(1) return n, c, q")
            .unwrap();
        assert_eq!((name.as_str(), count, quality), ("Item 2589", 3, -1));

        s.set_item_template(4306, template("Silk Cloth", 2, 5));
        let mut copy = [TradeTargetItem::default(); TRADE_SLOTS];
        copy[0] = target(4306, 4);
        s.trade_updated(copy, TradeCells::default());
        let (name, tex, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetTradeTargetItemInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, usable),
            ("Silk Cloth", 4, 2, 1)
        );
        assert_eq!(tex, "Interface\\Icons\\INV_Fabric_Linen_01");
    }

    /// A missed record leaves the trade's callback, one per read, and its landing signals `TRADE_UPDATE` (`0x587ce4`, `0x586080`).
    #[test]
    fn a_missed_record_signals_trade_update_when_it_lands() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        let mut copy = [TradeTargetItem::default(); TRADE_SLOTS];
        copy[0] = target(4306, 1);
        s.trade_updated(copy, TradeCells::default());
        s.run("GetTradeTargetItemInfo(1) GetTradeTargetItemInfo(1)")
            .unwrap();
        fired();
        s.set_item_template(4306, template("Silk Cloth", 1, 0));
        assert_eq!(fired(), ["TRADE_UPDATE", "TRADE_UPDATE"]);
        s.run("GetTradeTargetItemInfo(1)").unwrap();
        s.set_item_template(4306, template("Silk Cloth", 1, 0));
        assert!(fired().is_empty(), "a hit leaves none");
    }

    /// `GetPlayerTradeMoney` reads our offered coins only while a partner is set (`0x586780`); `GetTargetTradeMoney` reads the wire's cell with or without one (`0x5867d0`).
    #[test]
    fn money_reads_the_offer_while_a_partner_is_set() {
        let mut s = UiScript::new().unwrap();
        s.set_trade_offer_gold(12_345);
        s.set_trade_cells(TradeCells {
            target_gold: 500,
            ..Default::default()
        });
        assert_eq!(s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetTargetTradeMoney()").unwrap(), 500);
        s.open_trade_partner(PARTNER, false);
        assert_eq!(
            s.eval::<i64>("return GetPlayerTradeMoney()").unwrap(),
            12_345
        );
    }

    /// The offer update (`0x586dc0`): our accept drops first, each differing partner slot signals in order, the seventh releasing the enchanting item, then the enchant spells and both coins that moved; a second identical update signals nothing.
    #[test]
    fn the_offer_update_signals_what_moved_in_the_reference_order() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.set_trade_accepts((true, true));
        s.set_trade_offer_gold(40);
        fired();
        let mut copy = [TradeTargetItem::default(); TRADE_SLOTS];
        copy[1] = target(4306, 1);
        copy[6] = target(6217, 1);
        let cells = TradeCells {
            target_gold: 500,
            enchant_spells: [11, 22],
        };
        s.trade_updated(copy, cells);
        assert_eq!(
            fired(),
            [
                "TRADE_ACCEPT_UPDATE",
                "TRADE_ACCEPT_UPDATE",
                "TRADE_TARGET_ITEM_CHANGED",
                "TRADE_TARGET_ITEM_CHANGED",
                "TRADE_TARGET_ITEM_CHANGED",
                "TRADE_PLAYER_ITEM_CHANGED",
                "PLAYER_TRADE_MONEY",
                "PLAYER_MONEY",
                "TRADE_MONEY_CHANGED",
            ]
        );
        assert_eq!(s.trade_accepts(), (false, false));
        assert_eq!(s.take_trade_accept_sends(), [false]);
        s.trade_updated(copy, cells);
        assert!(fired().is_empty());
        // The open zeroes the coins and spells last signalled and each slot's id, so they signal again.
        s.open_trade_partner(PARTNER, false);
        fired();
        s.trade_updated(copy, cells);
        assert_eq!(fired().len(), 7);
    }

    /// The partner setter's close: the enchanting item and the completion flag first, then the partner zeroes and `TRADE_CLOSED` fires, once (`0x5870d5`, `0x5870f2`, `0x587102`, `0x58710e`); the copy stays until the next open, as `GetTradeTargetItemInfo` reads it with no partner set.
    #[test]
    fn the_close_signals_once_and_keeps_the_partners_copy() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.set_item_template(4306, template("Silk Cloth", 1, 0));
        let mut copy = [TradeTargetItem::default(); TRADE_SLOTS];
        copy[0] = target(4306, 1);
        s.trade_updated(copy, TradeCells::default());
        fired();
        s.close_trade_partner(false);
        s.trade_partner_closed();
        s.trade_partner_closed();
        assert_eq!(fired(), ["TRADE_CLOSED"]);
        assert_eq!(
            s.eval::<String>("return (GetTradeTargetItemInfo(1))")
                .unwrap(),
            "Silk Cloth"
        );
        s.enter_trade_world();
        assert!(s
            .eval::<bool>("return GetTradeTargetItemInfo(1) == nil")
            .unwrap());
    }

    /// The money verbs keep and send the offer with no window (`0x7043c0`, `0x704220`, `0x704320`), and the offer reads back inside the chunk that set it (`0x70445d`).
    #[test]
    fn the_money_verbs_keep_and_send_the_offer_with_no_window() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.run("SetTradeMoney(500)").unwrap();
        assert_eq!(s.take_trade_money(), Some(500));
        assert_eq!(s.trade_offer_gold(), 500);
        s.run("PickupTradeMoney(200)").unwrap();
        assert_eq!(s.take_trade_money(), Some(300));
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 200);
        s.run("AddTradeMoney()").unwrap();
        assert_eq!(s.take_trade_money(), Some(500));
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        // The purse does not cover it: nothing is kept or sent (`0x70441e`).
        s.run("SetTradeMoney(1001)").unwrap();
        assert_eq!((s.take_trade_money(), s.trade_offer_gold()), (None, 500));
        // Each send goes out, in call order (`0x704463`, `0x704382`, `0x7042d8`).
        s.run("SetTradeMoney(100) SetTradeMoney(200) PickupTradeMoney(50) AddTradeMoney()")
            .unwrap();
        assert_eq!(s.take_trade_money_sends(), [100, 200, 150, 200]);
        s.open_trade_partner(PARTNER, false);
        assert_eq!(
            s.eval::<i64>("SetTradeMoney(700) return GetPlayerTradeMoney()")
                .unwrap(),
            700
        );
    }

    /// `AcceptTrade` writes its flag, signals with the partner's and sends, with no window asked for, and a flag already up does nothing (`0x5866f9`, `0x58670d`, `0x586712`, `0x58671a`); `CancelTradeAccept` mirrors it (`0x586745`, `0x586763`, `0x58676b`).
    #[test]
    fn the_accept_verbs_signal_inside_the_call_and_send_in_call_order() {
        let mut s = UiScript::new().unwrap();
        s.set_trade_accepts((false, true));
        s.run(
            "seen = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_ACCEPT_UPDATE') \
             f:SetScript('OnEvent', function(_, _, mine, theirs) seen[#seen + 1] = mine .. ':' .. theirs end)",
        )
        .unwrap();
        s.run(
            "AcceptTrade() seen[#seen + 1] = 'after' AcceptTrade() \
             CancelTradeAccept() CancelTradeAccept()",
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return table.concat(seen, ',')").unwrap(),
            "1:1,after,0:1"
        );
        assert_eq!(s.take_trade_accept_sends(), [true, false]);
        assert_eq!(s.trade_accepts(), (false, true));
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The remove click and the right-click add play the item's put-down sound (`0x587a7e`, `0x58792a`), a wrapped item its own kit (`0x4d006d`); an empty slot's remove plays none (`0x587a40`), and a placing click queues none of its own.
    #[test]
    fn the_remove_click_and_the_add_play_the_put_down_sound() {
        use crate::script::SoundRequest;
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.run("UseContainerItem(0, 2)").unwrap();
        assert_eq!(s.take_sounds(), [SoundRequest::ItemPutDown(2770)]);
        s.run("ClickTradeButton(1, true)").unwrap();
        assert_eq!(s.take_sounds(), [SoundRequest::ItemPutDown(2770)]);
        s.run("ClickTradeButton(1, true)").unwrap();
        assert!(s.take_sounds().is_empty());
        s.run("PickupContainerItem(0, 1) ClickTradeButton(1)")
            .unwrap();
        assert!(s.take_sounds().is_empty());

        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            crate::script::ContainerSlot {
                guid: 0x4000_0000_0000_0009,
                item_id: 5043,
                count: 1,
                flags: 0x8,
                ..Default::default()
            },
        );
        s.set_container(
            1,
            Some(crate::script::ContainerState {
                bag_family: 0,
                name: Some("Bag".into()),
                num_slots: 4,
                slots,
            }),
        );
        s.run("UseContainerItem(1, 1)").unwrap();
        assert_eq!(
            s.take_sounds(),
            [SoundRequest::KitName("(DONOTRENAME)WrappedItemDrop".into())]
        );
    }

    /// The partner setter's open leg zeroes the flags and takes the cursor only when our own initiate asked (`0x703c30`): coins fold in and signal once (`0x587297`-`0x5872ca`), and `TRADE_SHOW` ends every open (`0x5872d8`, `0x5873c4`); an item goes out to slot 1 unsignalled, still locked (`0x58737b`, `0x587383`, `0x587391`), a bound one to slot 7 past `TRADE_POTENTIAL_BIND_ENCHANT` (`0x58735e`).
    #[test]
    fn the_open_leg_takes_the_cursors_coins_or_item_when_asked() {
        let mut s = UiScript::new().unwrap();
        pack(&mut s);
        s.set_money(1_000);
        s.open_trade_partner(PARTNER, false);
        s.set_trade_accepts((true, true));
        s.run("PickupPlayerMoney(300)").unwrap();
        fired();
        s.open_trade_partner(PARTNER, false);
        assert_eq!(s.trade_accepts(), (false, false));
        assert_eq!(fired(), ["TRADE_SHOW"]);
        assert!(s.take_trade_money().is_none());
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 300);
        s.open_trade_partner(PARTNER, true);
        assert_eq!(
            fired(),
            [
                "CURSOR_UPDATE",
                "PLAYER_TRADE_MONEY",
                "PLAYER_MONEY",
                "TRADE_SHOW"
            ]
        );
        assert_eq!(
            s.model_ref().trade_last_money[0],
            300,
            "the fold is the coins last signalled (`0x5872ba`)"
        );
        assert_eq!(s.take_trade_money(), Some(300));
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        // The purse does not cover offer plus cursor: the coins stay held, unsignalled (`0x5872a1`).
        s.run("PickupPlayerMoney(800)").unwrap();
        fired();
        s.open_trade_partner(PARTNER, true);
        assert_eq!(fired(), ["TRADE_SHOW"]);
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 800);
        s.run("ClearCursor()").unwrap();

        s.run("PickupContainerItem(0, 1)").unwrap();
        fired();
        s.open_trade_partner(PARTNER, true);
        assert_eq!(fired(), ["CURSOR_UPDATE", "TRADE_SHOW"]);
        assert_eq!(
            s.take_trade_item_sends(),
            [TradeItemSend::Set {
                id: 1,
                bag: 0,
                slot: 1
            }]
        );
        assert!(s.cursor_payload().is_none() && locked(&s, 1) && s.trade_offers_slot(0));

        let bound = TradeItemFacts {
            bound: true,
            ..Default::default()
        };
        s.model_mut().trade_item_facts.insert((0, 2), bound);
        s.run("PickupContainerItem(0, 2)").unwrap();
        fired();
        s.open_trade_partner(PARTNER, true);
        assert_eq!(
            fired(),
            [
                "TRADE_POTENTIAL_BIND_ENCHANT",
                "CURSOR_UPDATE",
                "TRADE_SHOW"
            ]
        );
        assert_eq!(
            s.take_trade_item_sends(),
            [TradeItemSend::Set {
                id: 7,
                bag: 0,
                slot: 2
            }]
        );
        assert!(s.trade_offers_slot(6) && !s.trade_offers_slot(0) && locked(&s, 2));
    }

    #[test]
    fn intents_queue_and_drain() {
        let mut s = UiScript::new().unwrap();
        s.run("InitiateTrade('target')").unwrap();
        s.run("InitiateTrade('party2')").unwrap();
        assert_eq!(s.take_trade_initiates(), vec!["target", "party2"]);
        assert!(s.take_trade_initiates().is_empty(), "drained");

        // The accept's send follows its flag (`0x58671a`); the unaccept goes out only with the flag up (`0x586745`, `0x58676b`).
        s.run("AcceptTrade()").unwrap();
        assert_eq!(s.take_trade_accept_sends(), [true]);
        assert!(s.take_trade_accept_sends().is_empty(), "drained");

        s.run("CancelTradeAccept()").unwrap();
        assert_eq!(s.take_trade_accept_sends(), [false]);

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
        s.open_trade_partner(PARTNER, false);
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
        s.open_trade_partner(PARTNER, false);
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

    /// An enchanting item aimed at the partner's seventh slot is used, then locked (`0x586c48`, `0x586c57`), and released by the next one (`0x586c20`), the partner's slot changing (`0x586fb6`), the partner setter's close (`0x5870d5`, `0x5880f4`), its cast's cancel (`0x806258`) and `CloseTrade` (`0x587985`).
    #[test]
    fn the_enchanting_item_locks_until_each_of_its_releases() {
        let enchants = TradeItemFacts {
            enchants_on_use: true,
            ..Default::default()
        };
        let setup = || {
            let mut s = UiScript::new().unwrap();
            pack(&mut s);
            s.open_trade_partner(PARTNER, false);
            s.model_mut().trade_target[6] = target(6217, 1);
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

        // A cancelled cast releases it only when the cast's item is that item (`0x80624d`, `0x806258`).
        let mut s = setup();
        s.cast_item_cancelled(0);
        s.cast_item_cancelled(0x4000_0000_0000_0002);
        assert!(fired().is_empty() && locked(&s, 1));
        s.cast_item_cancelled(0x4000_0000_0000_0001);
        assert_eq!(fired(), ["ITEM_LOCK_CHANGED", "ITEM_UNLOCKED"]);
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

    /// `CloseTrade` signals the coins unless the last close was a completion (`0x587949`), and closes an open window (`0x5879bc`), and the cancel goes out with no window too (`0x5879c4`).
    #[test]
    fn close_trade_skips_the_coins_after_a_completed_trade() {
        let mut s = UiScript::new().unwrap();
        fired();
        s.run("CloseTrade()").unwrap();
        assert_eq!(fired(), ["PLAYER_TRADE_MONEY"]);
        assert!(s.take_trade_close());
        // The flag is written only with a partner set (`0x5870af`-`0x5870bb`).
        s.close_trade_partner(true);
        s.run("CloseTrade()").unwrap();
        assert_eq!(fired(), ["PLAYER_TRADE_MONEY"]);
        s.open_trade_partner(PARTNER, false);
        s.close_trade_partner(true);
        s.trade_partner_closed();
        fired();
        s.run("CloseTrade()").unwrap();
        assert!(fired().is_empty() && s.take_trade_close());
        // The next window's open zeroes the flag (`0x587167`).
        s.open_trade_partner(PARTNER, false);
        fired();
        s.run("CloseTrade()").unwrap();
        assert_eq!(fired(), ["PLAYER_TRADE_MONEY", "TRADE_CLOSED"]);
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
        s.open_trade_partner(PARTNER, false);
        s.model_mut().trade_target[6] = target(6217, 1);
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
