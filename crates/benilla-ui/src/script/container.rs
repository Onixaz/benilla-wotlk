//! The 1.12 bag verbs, all bare globals. The app pushes each bag's resolved snapshot and drains
//! the intents the verbs queue; the engine holds no item knowledge. Bag ids are the 1.12 client's
//! (0 the backpack, 1-4 the worn bags, -1 the bank's own slots, 5-11 the bank bags, -2 the
//! keyring); slots are 1-based.

use mlua::{Lua, Table, Value};

use super::cursor::{self, CursorItem, CursorPayload};

/// The petition lines an item tooltip prints under the name: the title and the creator, keyed
/// `GUILD_CHARTER_*` for a charter, else `PETITION_*`. The signature-count line is not built: its
/// source is untraced, and vmangos's write of the on-item count is commented out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSlotView {
    /// The record's charter bit, also `GetPetitionInfo`'s first return.
    pub is_charter: bool,
    /// The proposed guild's (or petition's) name.
    pub title: String,
    /// The owner's name from the name cache; `None`, and no line, while the query is in flight.
    pub owner: Option<String>,
}

/// One occupied bag slot, resolved by the app from the item object and its template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerSlot {
    /// The item object's guid, which an equipment set records.
    pub guid: u64,
    /// The gem item in each socket enchant (slots 2..4), 0 for none.
    pub gems: [u32; 3],
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub texture: Option<String>,
    pub count: u32,
    /// Item quality 0..6; `None` while unresolved (the API reports `nil`).
    pub quality: Option<u32>,
    /// The item's template entry; 0 while unresolved.
    pub item_id: u32,
    /// An `|Hitem:…|h[Name]|h` link once the name is known.
    pub link: Option<String>,
    pub locked: bool,
    /// The 1-based inventory slots the item equips into, from `inventoryType`; empty if none.
    pub equip_slots: Vec<u8>,
    /// Whether the item may go on an action bar: `PlaceAction`'s only item filter, an on-use spell
    /// or equippable.
    pub bar_placeable: bool,
    /// Current and max durability; `None` when the max is 0 or the item object has not arrived.
    pub durability: Option<(u32, u32)>,
    /// The use cooldown, `(start_ms on the GetTime clock, duration_ms, enabled)`; `None` if cold.
    pub cooldown: Option<(i64, u32, bool)>,
    /// The instance carries its own text (`ITEM_FIELD_FLAGS & 0x200`, a mail copy):
    /// `GetContainerItemInfo`'s `readable`, which shows the Inspect cursor (`ContainerFrame.lua:638`).
    pub readable: bool,
    /// The `ITEM_FIELD_CREATOR` name for the "Written by" or "<Made by>" line, once resolved.
    pub creator: Option<String>,
    /// `ITEM_FIELD_FLAGS`; the tooltip reads UNLOCKED `0x4` and WRAPPED `0x8`.
    pub flags: u32,
    /// Bound at runtime: `ITEM_FIELD_FLAGS & 1`, or an enchant whose `SpellItemEnchantment` row
    /// binds. The tooltip's bind line then says Soulbound.
    pub already_bound: bool,
    /// The enchant slots in slot order, named from `SpellItemEnchantment.dbc`; empty when none.
    pub enchants: Vec<super::EnchantView>,
    /// A timed item's lifetime left in ms (`SMSG_ITEM_TIME_UPDATE`), floored to the second.
    pub duration_ms: Option<u64>,
    /// A signable charter's petition, printed between the name and the green `ITEM_SIGNABLE`
    /// line; `None` until the record the hover queries arrives.
    pub petition: Option<PetitionSlotView>,
    /// The template's `Bonding` is 4, a quest item: `GetContainerItemQuestInfo`'s `isQuestItem`
    /// (template `+0x178`, `0x5d9490`). Bonding 5 does not count.
    pub quest_item: bool,
    /// The template's `StartQuest` (`+0x18c`), the quest the item starts; 0 for none.
    pub start_quest: u32,
}

/// One enchant slot as the tooltip renders it; the app resolves the row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnchantView {
    /// `ITEM_FIELD_ENCHANTMENT` slot: 0 permanent, 1 temporary, 2-4 sockets, 5 socket bonus, 6
    /// prismatic, 7-11 the random suffix; only 0 and 1 are coloured.
    pub slot: u8,
    /// `abs(id)`, the `SpellItemEnchantment` row, which the socket walk compares to the template's
    /// socket bonus.
    pub id: u32,
    /// A socketed gem's colour (`GemProperties.Type`), for the socket bonus's match.
    pub gem_type: Option<u32>,
    /// The id was negative: `abs(id)` names the row, and slots 0 and 1 paint red, not green.
    pub negative: bool,
    /// The `SpellItemEnchantment` row's name, copied unformatted.
    pub name: String,
    /// The slot's charges; nonzero appends " (N Charges)".
    pub charges: u32,
    /// Ms left on a temporary enchant, from `SMSG_ITEM_ENCHANT_TIME_UPDATE`, never the item's own
    /// duration field; `Some` prints the countdown form.
    pub remaining_ms: Option<u64>,
    /// The spell an enchant's use effect (7) casts, 0 for none (`0x707c60`).
    pub use_spell: u32,
    /// The sockets a prismatic enchant (slot 6) adds, its effect 8's points (`0x708140`).
    pub prismatic_sockets: u32,
    /// A gem enchant's activation condition, a meta gem's.
    pub condition: Option<EnchantConditionView>,
    /// The gem item a socket enchant came from (`SrcItemID`), whose icon the line shows; 0 for none.
    pub gem_item: u32,
    /// The skill and level the enchant asks of its wearer.
    pub requirement: Option<EnchantRequirementView>,
}

/// An enchant's requirement: a skill at a rank, by name, and a minimum level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnchantRequirementView {
    pub skill: u32,
    pub skill_rank: u32,
    /// The skill's `SkillLine.dbc` name; `None` prints "UNKNOWN".
    pub skill_name: Option<String>,
    pub min_level: u32,
}

/// One `SpellItemEnchantmentCondition.dbc` row as the app resolves it: five tests on the equipped
/// gems' colour counts, read while `logic` is nonzero.
pub type EnchantConditionView = [EnchantConditionTest; 5];

/// One condition test: the left colour (`1` meta .. `4` blue), the operator, and a right colour or,
/// with a right type of 0, a count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnchantConditionTest {
    pub left_type: u8,
    pub operator: u8,
    pub right_type: u8,
    pub right_value: i32,
    pub logic: u8,
}

/// One `ItemRandomProperties` row, an item's random-suffix roll. The app pushes the whole table at
/// load, and a tooltip source supplies only the id, as in the reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RandomPropertyView {
    /// The suffix (`"of the Monkey"`), joined on by `ITEM_SUFFIX_TEMPLATE`, for the names the app
    /// does not compose itself.
    pub suffix: String,
    /// The roll's five enchants, named, in slots 7-11.
    pub enchants: Vec<EnchantView>,
}

/// One bag's snapshot: its name, capacity, and occupied slots (1-based).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerState {
    /// `GetBagName` (the bag item's own name; the backpack is the client-local "Backpack").
    pub name: Option<String>,
    pub num_slots: u32,
    pub slots: std::collections::HashMap<u32, ContainerSlot>,
    /// The bag template's `BagFamily` mask (`+0x1b8`, read by `0x7072b0`): 0 for a general bag,
    /// the backpack and the bank.
    pub bag_family: u32,
}

/// One queued move from `(src_bag, src_slot)` to `(dst_bag, dst_slot)`. `count` is `None` for a
/// whole stack (a move, a swap, or a merge the server tops up), `Some(n)` for a split placement
/// (`CMSG_SPLIT_ITEM`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerMove {
    pub src_bag: i64,
    pub src_slot: u32,
    pub dst_bag: i64,
    pub dst_slot: u32,
    pub count: Option<u32>,
}

/// One queued auto-store (`PutItemInBag`, `PutItemInBackpack`): the item at `(src_bag, src_slot)`
/// into container `dst_bag`, with no slot: `CMSG_AUTOSTORE_BAG_ITEM` (0x10B) carries none and
/// `CMSG_SPLIT_ITEM` (0x10E) sends `0xFF`, so the server picks. `Some(n)` in `count` is a split,
/// the fork the reference makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BagAutoStore {
    pub src_bag: i64,
    pub src_slot: u32,
    /// The container id: 0 the backpack, 1-4 a worn bag, 5-11 a bank bag.
    pub dst_bag: i64,
    /// The 0-based player slot of a target no container id names, a bank item or a key: the packet's bag byte is the slot holding the target (`0x6dbb30`).
    pub dst_player_slot: Option<u8>,
    pub count: Option<u32>,
}

/// Container `bag` read off the World at the call (`0x5d7380`); with no active player held, the stand-in a test or capture fixture set.
/// What one of `UseContainerItem`'s window arms did with the click.
pub(super) enum Arm {
    /// Not its window: the next arm reads the click.
    Pass,
    /// Took the item.
    Took,
    /// Refused it with this line, which shows at once and ends the click.
    Refused(&'static str),
}

pub(crate) fn live_bag(model: &super::Model, bag: i64) -> Option<ContainerState> {
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.container(bag, anchor, ui_now)).flatten() {
        Some(live) => live,
        None if bag == -4 => model.token_slots.clone(),
        None => model.containers.get(&bag).cloned(),
    }
}

/// One 1-based slot of container `bag` read at the call, as [`live_bag`] reads the bag; the slot resolver leaves the token container's slots unresolved (`0x5d7380`).
pub(crate) fn live_slot(model: &super::Model, bag: i64, slot: u32) -> Option<ContainerSlot> {
    if bag == -4 {
        return None;
    }
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.container_slot(bag, slot, anchor, ui_now)).flatten() {
        Some(live) => live,
        None => model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .cloned(),
    }
}

/// Container `bag` as a Lua bag index names it, read at the call (`0x5d7380` -> `0x5d6f20`): bags 1..11 through the watched-bag array, the bank's only with a banker set.
pub(crate) fn index_bag(model: &super::Model, bag: i64) -> Option<ContainerState> {
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.indexed_container(bag, anchor, ui_now)).flatten() {
        Some(live) => live,
        None => live_bag(model, bag),
    }
}

/// One 1-based slot of container `bag` as [`index_bag`] names the bag.
pub(crate) fn index_slot(model: &super::Model, bag: i64, slot: u32) -> Option<ContainerSlot> {
    if bag == -4 {
        return None;
    }
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.indexed_container_slot(bag, slot, anchor, ui_now)).flatten() {
        Some(live) => live,
        None => live_slot(model, bag, slot),
    }
}

/// A slot's cooldown as [`index_slot`] names the bag, read at the call (`0x5d7d90`).
pub(crate) fn index_cooldown(
    model: &super::Model,
    bag: i64,
    slot: u32,
) -> Option<(f64, f64, bool)> {
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.indexed_container_slot(bag, slot, anchor, ui_now)).flatten() {
        Some(live) => live?.cooldown.map(|(start_ms, duration_ms, enabled)| {
            (
                start_ms as f64 / 1000.0,
                f64::from(duration_ms) / 1000.0,
                enabled,
            )
        }),
        None => live_cooldown(model, bag, slot),
    }
}

/// Every container the player holds, read at the call: the keyring, the vault, the backpack and the eleven bags.
pub(crate) fn live_bags(model: &super::Model) -> Vec<(i64, ContainerState)> {
    (-2..=11)
        .filter_map(|bag| live_bag(model, bag).map(|c| (bag, c)))
        .collect()
}

/// A slot's cooldown read at the call, in seconds on the `GetTime` clock (`0x5d7d90`).
pub(crate) fn live_cooldown(model: &super::Model, bag: i64, slot: u32) -> Option<(f64, f64, bool)> {
    let (anchor, ui_now) = (model.clock_wall.get(), model.now);
    match super::read_live(|l| l.container_slot(bag, slot, anchor, ui_now)).flatten() {
        Some(live) => live?.cooldown.map(|(start_ms, duration_ms, enabled)| {
            (
                start_ms as f64 / 1000.0,
                f64::from(duration_ms) / 1000.0,
                enabled,
            )
        }),
        None => model.container_cooldowns.get(&(bag, slot)).copied(),
    }
}

/// `HasKey()` read at the call (`0x51ae80`), the fixture's answer with no active player held.
pub(crate) fn live_has_key(model: &super::Model) -> bool {
    super::read_live(|l| l.has_key())
        .flatten()
        .unwrap_or(model.has_key)
}

impl super::UiScript {
    /// Drain the auto-stores `PutItemInBag`/`PutItemInBackpack` queued since the last call.
    pub fn take_bag_autostores(&mut self) -> Vec<BagAutoStore> {
        drain_field!(self, bag_autostores)
    }

    /// Push one bag's snapshot, or remove it with `None`; cooldowns arrive in ms on the `GetTime`
    /// clock and are stored in seconds.
    pub fn set_container(&mut self, bag: i64, state: Option<ContainerState>) {
        let mut model = self.model_mut();
        model.container_cooldowns.retain(|&(b, _), _| b != bag);
        match state {
            Some(s) => {
                for (&slot, cs) in &s.slots {
                    let Some((start_ms, duration_ms, enabled)) = cs.cooldown else {
                        continue;
                    };
                    let abs = (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    );
                    model.container_cooldowns.insert((bag, slot), abs);
                }
                model.containers.insert(bag, s);
            }
            None => {
                model.containers.remove(&bag);
            }
        }
    }

    /// Push `HasKey()`'s answer: whether the player owns a `BagFamily` key item anywhere.
    pub fn set_has_key(&mut self, has_key: bool) {
        set_field!(self, has_key, has_key);
    }

    /// Take the `UseContainerItem` calls out of the call stream, as `(bag, slot)`.
    pub fn take_container_uses(&mut self) -> Vec<(i64, u32)> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::UseContainerItem { bag, slot } => Some((*bag, *slot)),
            _ => None,
        })
    }

    /// Drain the pick/place/swap moves queued by `PickupContainerItem` since the last call.
    pub fn take_container_moves(&mut self) -> Vec<ContainerMove> {
        drain_field!(self, container_moves)
    }

    /// Drain the `(bag, slot)` sources `AutoEquipCursorItem` queued, for `CMSG_AUTOEQUIP_ITEM`.
    pub fn take_container_autoequips(&mut self) -> Vec<(i64, u32)> {
        drain_field!(self, container_autoequips)
    }

    /// The auto-equips `EquipCursorItem` queued, sent with the bind question off (`0x51a51a`).
    pub fn take_container_autoequips_unasked(&mut self) -> Vec<(i64, u32)> {
        drain_field!(self, container_autoequips_unasked)
    }

    /// Drain the `(bag, slot)` clicks repair mode queued, for `CMSG_REPAIR_ITEM`.
    pub fn take_container_repairs(&mut self) -> Vec<(i64, u32)> {
        drain_field!(self, container_repairs)
    }

    /// Drain the `(bag, slot, count)` destroys `DeleteCursorItem` queued, a 0 count being the whole
    /// stack, for `CMSG_DESTROYITEM`.
    pub fn take_container_destroys(&mut self) -> Vec<(i64, u32, u32)> {
        drain_field!(self, container_destroys)
    }

    /// Arm a gift wrap with the paper at `(bag, slot)`, as the reference does: the paper locks, the
    /// cursor takes mode 2 (the cast art), and nothing is sent. The next left-click on a container
    /// slot completes it.
    pub fn arm_gift_wrap(&mut self, bag: i64, slot: u32) {
        let mut model = self.model_mut();
        model.pending_wrap = Some(PendingWrap { bag, slot });
        model.ui_cursor = Some(UiCursorMode::Cast);
        model.ui_cursor_dirty = true;
        cursor::set_item_lock(&mut model, bag, slot, true);
    }

    /// The armed gift wrap, if any; the app reads it to hold the cursor's base at Cast.
    pub fn gift_wrap_armed(&self) -> Option<PendingWrap> {
        self.model_ref().pending_wrap
    }

    /// Drain the `(giftBag, giftSlot, itemBag, itemSlot)` wraps queued, for `CMSG_WRAP_ITEM`.
    pub fn take_container_wraps(&mut self) -> Vec<(i64, u32, i64, u32)> {
        drain_field!(self, container_wraps)
    }

    /// The mode the last FrameXML cursor call set, `None` after a `ResetCursor`; the app acts on
    /// [`Self::take_cursor_write`] instead.
    pub fn ui_cursor(&self) -> Option<UiCursorMode> {
        self.model_ref().ui_cursor
    }

    /// Drain the pending cursor write: `Some(mode)` a set, `Some(None)` a reset, `None` no call,
    /// which must leave the cursor as it was, so an armed spell keeps its cast cursor over a
    /// spellbook button.
    #[allow(clippy::option_option)]
    pub fn take_cursor_write(&mut self) -> Option<Option<UiCursorMode>> {
        if !self.model_ref().ui_cursor_dirty {
            return None;
        }
        let mut model = self.model_mut();
        std::mem::take(&mut model.ui_cursor_dirty).then_some(model.ui_cursor)
    }
}

/// The armed gift wrap's paper. The reference keeps it beside the cursor mode, not as a cursor
/// payload, so `CursorHasItem()` stays false while a wrap is armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingWrap {
    /// The paper's container id.
    pub bag: i64,
    /// The paper's 1-based slot within that container.
    pub slot: u32,
}

/// A displayed-cursor override from the FrameXML cursor verbs, the reference's one displayed mode,
/// until `ResetCursor`; the app maps each to its `Interface\Cursor` art.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiCursorMode {
    /// Buy (3): selling a bag item, or a merchant or buyback item the player can afford.
    Buy,
    /// UnableBuy (23): a merchant or buyback item the player cannot afford.
    UnableBuy,
    /// Inspect (7): the magnifier, `ShowInspectCursor`.
    Inspect,
    /// Point (1), `SetCursor("POINT_CURSOR")`.
    Point,
    /// Cast (2), `SetCursor("CAST_CURSOR")`: a unit frame the armed spell can target.
    Cast,
    /// UnableCast (22), `SetCursor("CAST_ERROR_CURSOR")`: a unit frame the armed spell cannot
    /// target, and leaving one while still targeting.
    CastError,
}

/// Whether a bag slot's item is locked (`item+0x394` bit 0): the bit its read reports, or the one kept here with no item store lent.
pub(super) fn bag_slot_locked(
    model: &super::Model,
    bag: i64,
    slot: u32,
    s: &ContainerSlot,
) -> bool {
    s.locked || model.item_locks.contains(&(bag, slot))
}

/// The slot's item taken with no item and no virtual item held (`0x5d8110`-`0x5d822f`): an empty slot does nothing; a targeting cursor stops (`0x5d811f`); repair mode repairs the item (`0x5d812c`); an armed gift wrap wraps it (`0x5d81e1`); else an unlocked item goes up through the setter and locks (`0x5d8208`, `0x5d8228`, `0x5d822f`).
fn take_slot_item(model: &mut crate::script::ModelGuard<'_>, bag: i64, slot: u32) {
    let Some(s) = index_slot(model, bag, slot).filter(|s| s.item_id != 0) else {
        return;
    };
    if super::spellbook::targeting().active {
        model.call(super::ScriptCall::SpellStopTargeting);
    }
    // The app's repair drain runs the arm's `ITEM_REPAIR` and send.
    if super::merchant::repair_mode(model) {
        model.container_repairs.push((bag, slot));
        return;
    }
    if let Some(wrap) = model.pending_wrap {
        // A vanished paper bails, the wrap armed and the paper locked.
        if !index_slot(model, wrap.bag, wrap.slot).is_some_and(|s| s.item_id != 0) {
            return;
        }
        // No eligibility test: the server refuses with an `ERR_CANT_WRAP_*` reason
        // (`SMSG_INVENTORY_CHANGE_FAILURE` 43-48), and a local gate would eat that error line.
        model.pending_wrap = None;
        model.container_wraps.push((wrap.bag, wrap.slot, bag, slot));
        // The paper unlocks and the cursor resets right after the send, not on the answer; the
        // target never locks.
        cursor::set_item_lock(model, wrap.bag, wrap.slot, false);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
        return;
    }
    // A locked item never picks up.
    if bag_slot_locked(model, bag, slot, &s) {
        return;
    }
    let item = CursorItem {
        bag,
        slot,
        item_id: s.item_id,
        texture: s.texture.clone(),
        link: s.link.clone(),
        quality: s.quality,
        count: None,
        bar_placeable: s.bar_placeable,
        equip_slots: s.equip_slots.clone(),
    };
    cursor::set_cursor(model, CursorPayload::Item(item));
    cursor::set_item_lock(model, bag, slot, true);
}

/// `PickupContainerItem`'s gesture (`0x5d7ff0`):
/// - a click on the held item's own slot cancels;
/// - a whole stack placed anywhere else locks the slot's item and runs the swap, whose send clears
///   the cursor with its item still locked; a swap lands the displaced item in the source slot,
///   never on the cursor (no cursor set);
/// - a split carry queues its split onto whatever the slot holds and clears the same way;
/// - a vendor row buys into the slot and a guild bank item withdraws into it; a bar slot's item only clears;
/// - anything else held carries no item (`0x5d80ff`, `0x5d8108`): the slot's item is taken as on an empty cursor, the setter's clear dropping what was held.
pub(super) fn pickup_container_item(
    model: &mut crate::script::ModelGuard<'_>,
    bag: i64,
    slot: u32,
) {
    match model.cursor.clone() {
        Some(CursorPayload::Item(held)) if held.bag == bag && held.slot == slot => {
            cursor::clear_cursor(model);
        }
        // A split goes out as it is and the cursor clears, its item still locked (`0x5d8278`, `0x5d8281`).
        Some(CursorPayload::Item(held)) if held.count.is_some() => {
            queue_move(model, &held, bag, slot, held.count);
            cursor::clear_cursor_with(model, false, true);
        }
        Some(CursorPayload::Item(held)) => {
            // A locked item in the slot refuses the swap, the cursor as it was (`0x5d82a4`).
            let refused = index_slot(model, bag, slot)
                .is_some_and(|d| d.item_id != 0 && bag_slot_locked(model, bag, slot, &d));
            if refused {
                return;
            }
            // The slot's item locks (`0x5d82af`), then the swap runs (`0x5d82dc`).
            cursor::set_item_lock(model, bag, slot, true);
            swap_held(model, &held, bag, slot);
        }
        // Mode 5, a held vendor row, buys into this slot whatever it holds: `CMSG_BUY_ITEM_IN_SLOT`
        // (0x1a3) with count 1, and the cursor clears either way. Deviation: a row that no longer
        // resolves is refused, as the reference does, because this path dereferences it unchecked,
        // a latent null deref.
        Some(CursorPayload::Merchant(held)) => {
            let row = super::merchant::merchant_row(
                model,
                super::MerchantList::Items,
                held.row as usize,
                false,
            )
            .filter(|it| it.item_id == held.item_id)
            .map(|_| held.row);
            if let Some(row) = row {
                super::merchant::merchant_act(
                    model,
                    super::MerchantAct::BuyInSlot { bag, slot, row },
                );
            }
            cursor::clear_cursor(model);
        }
        // Mode 11, a guild bank item, withdraws into this slot (`0x5d7ff0`).
        Some(CursorPayload::GuildBankItem(held)) => {
            super::guild_bank::drop_on_bag_slot(model, held, bag, slot);
        }
        // A bar slot's item is a virtual one no arm here takes: the cursor clears (`0x5d8477`).
        Some(CursorPayload::Action(a)) if a.kind & 0xf0 == super::action::ACTION_KIND_ITEM => {
            cursor::clear_cursor(model);
        }
        _ => take_slot_item(model, bag, slot),
    }
}

/// What the item swap does with its two ends before its send (`0x6df890`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapGate {
    /// The swap goes out and the cursor clears, its item still locked (`0x6dfc04`, `0x6dfc0d`).
    Send,
    /// An unbound item is being equipped and its record is not cached: the swap is kept for the record's callback and nothing else moves (`0x6dfa1d`, `0x6db6f0`).
    Wait(u32),
    /// The item being equipped binds when equipped (`0x6dfaac`): `EQUIP_BIND_CONFIRM` is asked if the player can use it (`0x6dfac1`, `0x6dfb3d`), and the cursor stays as it was.
    Ask(u32),
    /// The other end's bag is not held: nothing goes out (`0x6df98f` -> `0x6dfb45`).
    Refuse,
}

/// The item swap of the held item with `(dst_bag, dst_slot)` (`0x6df890`): the send clears the cursor with its item still locked (`0x6dfc0d`); a bind question, a record still out and a refusal return before it (`0x6dfb45`, `0x6dfb4d`), the cursor holding.
pub(crate) fn swap_held(
    model: &mut crate::script::ModelGuard<'_>,
    held: &CursorItem,
    dst_bag: i64,
    dst_slot: u32,
) {
    let mv = ContainerMove {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        dst_slot,
        count: None,
    };
    match swap_gate(model, &mv) {
        SwapGate::Send => {
            model.container_moves.push(mv);
            cursor::clear_cursor_with(model, false, true);
        }
        SwapGate::Ask(_) => ask_bind(model, mv),
        SwapGate::Wait(entry) => {
            super::item_stats::wait_item_record(model, entry, super::ItemRecordWait::Swap(mv));
            super::item_stats::ask_item_record(model, entry);
        }
        SwapGate::Refuse => {}
    }
}

/// The swap's bind question (`0x6dface`-`0x6dfb3d`): the pending equip is filed and `EQUIP_BIND_CONFIRM` signals with its index inside the call; with no host answering, the app's drain files it.
fn ask_bind(model: &mut crate::script::ModelGuard<'_>, mv: ContainerMove) {
    match model.ask(&super::HostAsk::BindSwap(mv)) {
        Some(super::HostAnswer::BindIndex(index)) => model.raise(
            "EQUIP_BIND_CONFIRM",
            vec![super::ScriptValue::Int(i64::from(index))],
        ),
        _ => model.container_bind_asks.push(mv),
    }
}

/// The swap's gate read at the call; an item the player cannot use asks nothing (`0x6dfac1`), and with no view answering the swap goes out.
fn swap_gate(model: &super::Model, mv: &ContainerMove) -> SwapGate {
    let gate =
        super::read_live(|l| l.swap_gate((mv.src_bag, mv.src_slot), (mv.dst_bag, mv.dst_slot)))
            .flatten();
    #[cfg(any(test, feature = "fixtures"))]
    let gate = gate.or(model.swap_gates.get(&(mv.dst_bag, mv.dst_slot)).copied());
    match gate.unwrap_or(SwapGate::Send) {
        SwapGate::Ask(entry) if !super::item_stats::item_usable_by_id(model, entry) => {
            SwapGate::Send
        }
        gate => gate,
    }
}

impl super::UiScript {
    /// The swaps that asked `EQUIP_BIND_CONFIRM` where no host answered in the call, taken: the app files each and raises its question.
    pub fn take_container_bind_asks(&mut self) -> Vec<ContainerMove> {
        drain_field!(self, container_bind_asks)
    }

    /// A fixture's stand-in for the swap gate of a put-down at `(bag, slot)`.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_swap_gate(&mut self, bag: i64, slot: u32, gate: SwapGate) {
        self.model_mut().swap_gates.insert((bag, slot), gate);
    }

    /// A swap whose record is not cached is kept on that record's callback, as its gate keeps it (`0x6dfa1d`); the caller sends nothing.
    pub fn keep_swap_for_record(&mut self, entry: u32, mv: ContainerMove) {
        let mut model = self.model_mut();
        super::item_stats::wait_item_record(&mut model, entry, super::ItemRecordWait::Swap(mv));
    }

    /// A kept swap's record landed (`0x6db6f0`): the swap runs again from its two ends, unsuppressed, and its send clears whatever the cursor holds (`0x6db784`).
    pub(crate) fn swap_record_landed(&mut self, mv: ContainerMove) {
        let mut model = self.model_mut();
        match swap_gate(&model, &mv) {
            SwapGate::Send => {
                model.container_moves.push(mv);
                cursor::clear_cursor_with(&mut model, false, true);
            }
            SwapGate::Ask(_) => ask_bind(&mut model, mv),
            SwapGate::Wait(_) | SwapGate::Refuse => {}
        }
    }
}

/// Queue one move, the shared tail of every clearing branch above.
fn queue_move(
    model: &mut super::Model,
    held: &CursorItem,
    dst_bag: i64,
    dst_slot: u32,
    count: Option<u32>,
) {
    model.container_moves.push(ContainerMove {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        dst_slot,
        count,
    });
}

/// Register the container verbs as bare globals, as `reference/3.3.5a-globals.tsv` lists them;
/// `C_Container` is a later client's namespace, and an addon that finds it assumes that client.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // ContainerIDToInventoryID(containerID) (`0x5d6f60`): `id - 1` must be 0..=10 unsigned
    // (`0x5d6f9f`), else it raises, so the backpack 0, the bank -1 and the keyring -2 raise; then
    // +20 below 4 and +64 from 4, bags 1-4 → 20-23 and bank bags 5-11 → 68-74.
    lua.globals().set(
        "ContainerIDToInventoryID",
        lua.create_function(|lua, id: Value| {
            let id = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: ContainerIDToInventoryID(containerID)",
            )?;
            let t = id.wrapping_sub(1);
            if !(0..=10).contains(&t) {
                return Err(mlua::Error::runtime(
                    "ContainerIDToInventoryID(): invalid container ID",
                ));
            }
            Ok(i64::from(if t < 4 { t + 20 } else { t + 64 }))
        })?,
    )?;

    // SetBagPortraitTexture(texture, containerID) (`0x5d7180`): a non-number id raises its usage
    // text; 0 and below return before the clear, 1-11 clear and then set the bag's icon as a
    // portrait (`0x619330`, `SetPortraitToTexture`'s core), 12 and up raise. Bank bags 5-11
    // resolve only while the bank is open (`0x5d6f20` tests the banker guid `[0xbeb9a0]`).
    lua.globals().set(
        "SetBagPortraitTexture",
        lua.create_function(|lua, (region, container): (Table, Value)| {
            let container = super::binding_abi::number_arg(
                lua,
                container,
                "Usage: SetBagPortraitTexture(texture, slot)",
            )?;
            let index = container.wrapping_sub(1);
            if index < 0 {
                return Ok(());
            }
            if index >= 11 {
                return Err(mlua::Error::runtime(
                    "Invalid slot in SetBagPortraitTexture",
                ));
            }
            let path = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                // Worn bags sit in inventory slots 20-23, bank bags in 68-74.
                let slot = if index < 4 {
                    Some(index + 20)
                } else {
                    super::bank::bank_open().then_some(index + 64)
                };
                slot.and_then(|s| usize::try_from(s).ok())
                    .and_then(|s| model.inv_slot("player", s))
                    .and_then(|v| v.icon.clone())
            };
            let rh = crate::script::region::region_handle_of(lua, &region)?;
            let mut model = crate::script::model_mut_of(lua);
            let data = model.region_data.entry(rh).or_default();
            // An empty slot's `None` is the answer: the reference clears first, then sets.
            data.texture = path;
            data.circular = true;
            data.portrait_unit = None;
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "GetContainerNumSlots",
        lua.create_function(|lua, bag: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // The currency container -4 counts the 32 token slots (`0x5d7529`).
            Ok(index_bag(&model, bag).map_or(0, |c| c.num_slots))
        })?,
    )?;

    // GetContainerNumFreeSlots(bag) (`0x5d7590`) → freeSlots, bagFamily: the backpack's and the
    // bank's (-1) empty slots with family 0, a worn or bank bag's empty slots and its template's
    // `BagFamily`. Anything it cannot resolve answers the lone number 0: no player, the bank
    // closed, an empty bag slot, an id outside -1..=11 (the keyring included). The usage text is
    // the reference's, which names `GetContainerFreeSlots`.
    lua.globals().set(
        "GetContainerNumFreeSlots",
        lua.create_function(|lua, bag: Value| {
            let bag = i64::from(super::binding_abi::number_arg(
                lua,
                bag,
                "Usage: GetContainerFreeSlots(index)",
            )?);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // The backpack's snapshot stands for the active player, which every branch needs.
            if index_bag(&model, 0).is_none() {
                return Ok(mlua::MultiValue::from_vec(vec![Value::Integer(0)]));
            }
            let bank_open = super::bank::bank_open();
            let resolved = match bag {
                0 => true,
                -1 | 5..=11 => bank_open,
                1..=4 => true,
                _ => false,
            };
            let answer = resolved.then(|| index_bag(&model, bag)).flatten().map(|c| {
                let free = c.num_slots.saturating_sub(c.slots.len() as u32);
                (free, c.bag_family)
            });
            Ok(mlua::MultiValue::from_vec(match answer {
                Some((free, family)) => vec![
                    Value::Integer(i64::from(free)),
                    Value::Integer(i64::from(family)),
                ],
                None => vec![Value::Integer(0)],
            }))
        })?,
    )?;

    // GetContainerItemQuestInfo(bag, slot) (`0x5d9400`) → isQuestItem, questId, isActive, always
    // three values: `true` or nil for a `Bonding` 4 template, then the `StartQuest` id and whether
    // that quest is already in the log (`0x5e11d0`), or nil, nil. An empty slot or an unresolved
    // template answers three nils.
    lua.globals().set(
        "GetContainerItemQuestInfo",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetContainerItemQuestInfo(index, slot)";
            let bag = i64::from(super::binding_abi::number_arg(lua, bag, USAGE)?);
            let slot = super::binding_abi::number_arg(lua, slot, USAGE)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(s) = u32::try_from(slot)
                .ok()
                .and_then(|slot| index_slot(&model, bag, slot))
            else {
                return Ok((Value::Nil, Value::Nil, Value::Nil));
            };
            let quest_item = if s.quest_item {
                Value::Boolean(true)
            } else {
                Value::Nil
            };
            if s.start_quest == 0 {
                return Ok((quest_item, Value::Nil, Value::Nil));
            }
            let active = super::quest_log::quest_row_of(&model, s.start_quest).is_some();
            Ok((
                quest_item,
                Value::Integer(i64::from(s.start_quest)),
                Value::Boolean(active),
            ))
        })?,
    )?;

    lua.globals().set(
        "GetBagName",
        lua.create_function(|lua, bag: i64| {
            let name = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                index_bag(&model, bag).and_then(|c| c.name.clone())
            };
            match name {
                Some(n) => Ok(Value::String(lua.create_string(&n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetContainerItemCooldown(bag, slot) → start, duration, enable on the `GetTime` clock, as
    // `GetActionCooldown`; enable 0 is a held cooldown. An expired one reads cold, so a re-feed
    // cannot replay the finish flash.
    lua.globals().set(
        "GetContainerItemCooldown",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let now = crate::script::clock::now(lua);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(match index_cooldown(&model, bag, slot) {
                Some((start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    lua.globals().set(
        "GetContainerItemLink",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                index_slot(&model, bag, slot).and_then(|s| s.link.clone())
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetContainerItemInfo(bag, slot) (`0x5d7a90`) → texture, itemCount, locked, quality, readable,
    // lootable, link; no values for an empty or unknown slot. The three flags answer 1 or nil; the
    // quality is -1 until the template is in, and for one with no `InventoryType`. Lootable is the
    // template's `0x4`. The refund confirm reads the link (`ContainerFrame.lua:684`).
    lua.globals().set(
        "GetContainerItemInfo",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let (info, held_here, lootable, quality) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let info = index_slot(&model, bag, slot);
                let held = info
                    .as_ref()
                    .is_some_and(|s| bag_slot_locked(&model, bag, slot, s));
                let template = info
                    .as_ref()
                    .and_then(|s| super::item_stats::item_record(&model, s.item_id));
                let lootable = template.as_ref().is_some_and(|t| t.flags & 0x4 != 0);
                let quality = template
                    .filter(|t| t.inventory_type != 0)
                    .map_or(-1, |t| i64::from(t.quality));
                (info, held, lootable, quality)
            };
            let Some(s) = info else {
                return Ok(mlua::MultiValue::new());
            };
            Ok(mlua::MultiValue::from_vec(vec![
                // Always a path for an occupied slot (`0x5d7b18`); the app fills the real one.
                Value::String(
                    lua.create_string(
                        s.texture
                            .as_deref()
                            .unwrap_or(r"Interface\Icons\INV_Misc_QuestionMark"),
                    )?,
                ),
                Value::Integer(i64::from(s.count)),
                // The item's own lock bit (`0x5d7b5e`).
                super::binding_abi::flag(held_here),
                Value::Integer(quality),
                super::binding_abi::flag(s.readable),
                super::binding_abi::flag(lootable),
                match &s.link {
                    Some(l) => Value::String(lua.create_string(l.as_str())?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    // GetContainerItemID(bag, slot) (`0x5d7d00`): the entry, or no value for an empty
    // or unresolved slot.
    lua.globals().set(
        "GetContainerItemID",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            let (Some(bag), Some(slot)) = (lua.coerce_number(bag)?, lua.coerce_number(slot)?)
            else {
                return Err(mlua::Error::runtime(
                    "Usage: GetContainerItemID(index, slot)",
                ));
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let entry = index_slot(&model, bag as i64, slot as u32).map_or(0, |s| s.item_id);
            Ok(if entry == 0 {
                mlua::MultiValue::new()
            } else {
                mlua::MultiValue::from_vec(vec![Value::Integer(i64::from(entry))])
            })
        })?,
    )?;

    lua.globals().set(
        "UseContainerItem",
        lua.create_function(|lua, (bag, slot, _rest): (i64, u32, mlua::MultiValue)| {
            let mut model = crate::script::model_mut_of(lua);
            // Every right-click clears the cursor before it reads the slot: a held payload goes
            // back, never placed, and an armed gift wrap is cancelled.
            cursor::clear_cursor(&mut model);
            if super::merchant::repair_mode(&model) {
                // Then the item in the slot is repaired.
                let occupied = index_slot(&model, bag, slot).is_some_and(|s| s.item_id != 0);
                if occupied {
                    model.container_repairs.push((bag, slot));
                }
            } else {
                // Past repair, the Send tab attaches the item (`0x5d88cf`), then the Auctions tab
                // takes it for the sell slot (`0x5d88e0`), then an open guild bank stores it. A
                // refusing arm shows its line at once and ends the click.
                let arm = match super::mail::use_container_item(&mut model, bag, slot) {
                    Arm::Pass => super::auction::use_container_item(&mut model, bag, slot),
                    took => took,
                };
                // Then an open trade takes it (`0x5d896d`), ahead of the guild bank.
                let arm = match arm {
                    Arm::Pass => super::trade::use_container_item(&mut model, bag, slot),
                    took => took,
                };
                match arm {
                    Arm::Refused(key) => {
                        drop(model);
                        crate::script::ui_errors::show_now(lua, key);
                    }
                    Arm::Took => {}
                    Arm::Pass => {
                        if !super::guild_bank::use_container_item(&mut model, bag, slot) {
                            drop(model);
                            super::calls::make(
                                lua,
                                super::ScriptCall::UseContainerItem { bag, slot },
                            );
                        }
                    }
                }
            }
            Ok(())
        })?,
    )?;

    // PickupContainerItem(bag, slot): pick up, or place and swap; no return value on any path.
    lua.globals().set(
        "PickupContainerItem",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            pickup_container_item(&mut crate::script::model_mut_of(lua), bag, slot);
            crate::script::calls::press_item_picks(lua);
            Ok(())
        })?,
    )?;

    // ShowContainerSellCursor(bag, slot): Buy (3) over an occupied slot, with no price check, so
    // selling never greys; an empty slot leaves the cursor alone.
    lua.globals().set(
        "ShowContainerSellCursor",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let mut model = crate::script::model_mut_of(lua);
            let occupied = index_slot(&model, bag, slot).is_some();
            // An armed spell or repair cursor suppresses it first; each is sticky, so a Buy here
            // would stamp out the active base cursor.
            if occupied
                && !super::spellbook::targeting().active
                && !super::merchant::repair_mode(&model)
            {
                model.ui_cursor = Some(UiCursorMode::Buy);
                model.ui_cursor_dirty = true;
            }
            Ok(())
        })?,
    )?;
    // ShowInspectCursor(): Inspect (7), unconditionally.
    lua.globals().set(
        "ShowInspectCursor",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.ui_cursor = Some(UiCursorMode::Inspect);
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    // HasKey() → the number 1 if the player owns a key (`BagFamily` 9) anywhere, else nil, never a
    // boolean; the app does the search.
    lua.globals().set(
        "HasKey",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if live_has_key(&model) {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;
    // SetCursor(name) (`0x5104a0`): a cursor name matched in any case sets its mode and answers 1,
    // and nil is a `ResetCursor`. Stock `ShowInspectCursor` is `SetCursor("INSPECT_CURSOR")`
    // (`UIParent.lua:2917-2919`). Deviation: a name whose mode benilla does not draw, or a custom
    // cursor file, changes nothing and answers nil.
    lua.globals().set(
        "SetCursor",
        lua.create_function(|lua, name: Option<String>| {
            let Some(name) = name else {
                let mut model = crate::script::model_mut_of(lua);
                model.ui_cursor = None;
                model.ui_cursor_dirty = true;
                return Ok(mlua::MultiValue::new());
            };
            let mode = match name.to_ascii_uppercase().as_str() {
                "POINT_CURSOR" => UiCursorMode::Point,
                "CAST_CURSOR" => UiCursorMode::Cast,
                "CAST_ERROR_CURSOR" => UiCursorMode::CastError,
                "BUY_CURSOR" => UiCursorMode::Buy,
                "BUY_ERROR_CURSOR" => UiCursorMode::UnableBuy,
                "INSPECT_CURSOR" => UiCursorMode::Inspect,
                _ => return Ok(mlua::MultiValue::from_vec(vec![Value::Nil])),
            };
            let mut model = crate::script::model_mut_of(lua);
            model.ui_cursor = Some(mode);
            model.ui_cursor_dirty = true;
            Ok(mlua::MultiValue::from_vec(vec![Value::Integer(1)]))
        })?,
    )?;
    // ResetCursor(): back to the base mode, whatever set the override.
    lua.globals().set(
        "ResetCursor",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.ui_cursor = None;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ContainerMove, ContainerSlot, ContainerState, PendingWrap, SwapGate, UiCursorMode,
    };
    use crate::script::cursor::EQUIPMENT_BAG;
    use crate::script::{MerchantItem, MerchantState, UiScript};
    use std::collections::HashMap;

    fn backpack() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                guid: 0,
                gems: [0; 3],
                quest_item: false,
                start_quest: 0,
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                count: 5,
                quality: Some(1),
                item_id: 117,
                link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        // An in-flight slot: the create arrived, the template answer hasn't.
        slots.insert(3, ContainerSlot::default());
        // A readable letter, a mail copy with item text.
        slots.insert(
            4,
            ContainerSlot {
                item_id: 8383,
                count: 1,
                readable: true,
                ..Default::default()
            },
        );
        ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    #[test]
    fn an_armed_gift_wrap_spends_on_the_next_container_click() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        assert_eq!(s.gift_wrap_armed(), Some(PendingWrap { bag: 0, slot: 1 }));
        assert!(s.take_container_wraps().is_empty(), "arming sends nothing");
        assert_eq!(s.take_cursor_write(), Some(Some(UiCursorMode::Cast)));
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper dims while the wrap stands"
        );
        assert!(
            !s.eval::<bool>("return CursorHasItem()").unwrap(),
            "the wrap writes no payload, so every stock CursorHasItem() gate stays shut"
        );
        // A click on an empty slot leaves the wrap armed.
        s.eval::<()>("PickupContainerItem(0, 9)").unwrap();
        assert_eq!(s.gift_wrap_armed(), Some(PendingWrap { bag: 0, slot: 1 }));
        assert!(s.take_container_wraps().is_empty());
        // A click on a real item spends it: the paper's pair leads, as the wire does.
        s.eval::<()>("PickupContainerItem(0, 4)").unwrap();
        assert_eq!(s.take_container_wraps(), vec![(0, 1, 0, 4)]);
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(
            s.take_cursor_write(),
            Some(None),
            "the cursor resets on the send, not on the server's answer"
        );
        assert!(
            s.cursor_payload().is_none(),
            "the completing click is spent on the wrap, not on picking the target up"
        );
    }

    /// `ClearCursor` tests for an armed wrap first, whatever its arguments.
    #[test]
    fn clear_cursor_cancels_an_armed_gift_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        let _ = s.take_cursor_write();
        s.eval::<()>("ClearCursor()").unwrap();
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(s.take_cursor_write(), Some(None), "and the mode goes back");
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper unlocks with the cancel — the lock never waited on a server"
        );
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.take_container_wraps().is_empty());
        assert!(s.cursor_payload().is_some(), "a plain pickup, as before");
    }

    #[test]
    fn container_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 1) == nil")
            .unwrap());

        s.set_container(0, Some(backpack()));
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(
            s.eval::<String>("return GetBagName(0)").unwrap(),
            "Backpack"
        );
        // The five values `ContainerFrame.lua:241` reads, in its order.
        let (icon, count, quality) = s
            .eval::<(String, i64, i64)>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 1)\n\
                 return texture, itemCount, quality",
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\INV_Misc_Food_16");
        // No pushed template, or one with no `InventoryType`, reads -1 (`0x5d7ba7`).
        assert_eq!((count, quality), (5, -1));
        let quality_of = |s: &mut UiScript| {
            s.eval::<i64>("return (select(4, GetContainerItemInfo(0, 1)))")
                .unwrap()
        };
        s.set_item_template(
            117,
            crate::script::ItemTemplateView {
                quality: 1,
                ..Default::default()
            },
        );
        assert_eq!(quality_of(&mut s), -1, "jerky has no InventoryType");
        s.set_item_template(
            117,
            crate::script::ItemTemplateView {
                quality: 1,
                inventory_type: 1,
                ..Default::default()
            },
        );
        assert_eq!(quality_of(&mut s), 1);
        // The flags answer 1 or nil, never a boolean.
        assert!(s
            .eval::<bool>("return select(3, GetContainerItemInfo(0, 1)) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return select(5, GetContainerItemInfo(0, 4)) == 1")
            .unwrap());
        assert_eq!(
            s.eval::<i64>(
                "local _, _, id = string.find(GetContainerItemLink(0, 1), 'item:(%d+)') \
                 return tonumber(id)"
            )
            .unwrap(),
            117
        );
        // readable: the letter in 4, not the jerky in 1.
        assert!(!s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 1) return readable")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 4) return readable")
            .unwrap());

        // An in-flight slot: the fallback icon, quality -1, itemCount real.
        assert!(s
            .eval::<bool>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 3)\n\
                 return texture == [[Interface\\Icons\\INV_Misc_QuestionMark]] and quality == -1 and itemCount ~= nil",
            )
            .unwrap());
        assert_eq!(
            s.eval::<Option<i64>>("return GetContainerItemID(0, 99)")
                .unwrap(),
            None,
            "an empty slot answers no value"
        );
        assert!(s
            .eval::<bool>("return GetContainerItemLink(0, 3) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 2) == nil")
            .unwrap());
    }

    #[test]
    fn use_container_item_queues_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("UseContainerItem(0, 1)").unwrap();
        s.run("UseContainerItem(0, 3, 'target')").unwrap();
        assert_eq!(s.take_container_uses(), vec![(0, 1), (0, 3)]);
        assert!(s.take_container_uses().is_empty(), "drained");
    }

    /// A right-click clears the cursor before it reads the slot: a held item goes back to its slot,
    /// unlocked, and the clicked item is still used.
    #[test]
    fn right_click_puts_a_held_item_back_then_uses_the_clicked_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("UseContainerItem(0, 4)").unwrap();
        assert!(s.cursor_payload().is_none(), "the held item went back");
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "its slot unlocks"
        );
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_container_uses(), vec![(0, 4)]);
    }

    /// The same clear cancels an armed gift wrap, and the clicked item is used, not wrapped.
    #[test]
    fn right_click_cancels_an_armed_gift_wrap_then_uses_the_clicked_one() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        s.run("UseContainerItem(0, 4)").unwrap();
        assert_eq!(s.gift_wrap_armed(), None);
        assert!(s.take_container_wraps().is_empty(), "nothing wrapped");
        assert_eq!(s.take_container_uses(), vec![(0, 4)]);
    }

    /// With no item and no virtual item held the slot's item is taken as on an empty cursor (`0x5d80ff`, `0x5d8108`): a held spell is dropped by the setter's clear, a targeting cursor stops first (`0x5d811f`), and a bar slot's item only clears (`0x5d8477`).
    #[test]
    fn a_click_with_no_item_held_takes_the_slots_item() {
        use crate::script::cursor::{CursorAction, CursorPayload, CursorSpell};
        use crate::script::live::TestTargeting;

        let mut s = swap_rig();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some_and(|i| (i.bag, i.slot) == (0, 1)));
        assert!(slot_locked(&s, 1));
        // An empty slot leaves the held payload alone (`0x5d8110`).
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        s.run("PickupContainerItem(0, 9)").unwrap();
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))));

        let mut s = swap_rig();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: crate::script::action::ACTION_KIND_ITEM,
            action: 117,
            texture: None,
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_payload().is_none() && !slot_locked(&s, 1));

        let mut s = swap_rig();
        crate::script::live::lending(&mut crate::script::live::TestView::default(), || {
            s.set_spell_targeting(true);
            s.run("PickupContainerItem(0, 1)").unwrap();
            assert!(s.take_stop_targeting(), "the targeting cursor stops");
            assert!(s.cursor_item().is_some());
            // The split stops it too, after its clear (`0x5d85f4`).
            s.run("ClearCursor() SplitContainerItem(0, 2, 1)").unwrap();
            assert!(s.take_stop_targeting());
            s.set_spell_targeting(false);
            s.run("ClearCursor() SplitContainerItem(0, 2, 1)").unwrap();
            assert!(!s.take_stop_targeting());
        });
    }

    /// A held guild bank item dropped on a bag slot with no banker open moves nothing and stays held (`0x5d837c`).
    #[test]
    fn a_bank_item_dropped_with_no_banker_stays_on_the_cursor() {
        use crate::script::cursor::{CursorGuildBankItem, CursorPayload};
        let mut s = swap_rig();
        s.set_cursor_for_test(CursorPayload::GuildBankItem(CursorGuildBankItem {
            tab: 0,
            slot: 3,
            item_id: 2589,
            texture: None,
            link: None,
            split: None,
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(matches!(
            s.cursor_payload(),
            Some(CursorPayload::GuildBankItem(_))
        ));
        assert!(s.take_guild_bank_requests().is_empty());
    }

    /// A two-item backpack and a helmet worn in the head slot.
    fn swap_rig() -> UiScript {
        let mut s = UiScript::new().unwrap();
        let mut slots = HashMap::new();
        for (slot, item_id) in [(1, 1234), (2, 4500)] {
            slots.insert(
                slot,
                ContainerSlot {
                    item_id,
                    count: 1,
                    equip_slots: vec![1],
                    ..Default::default()
                },
            );
        }
        s.set_container(
            0,
            Some(ContainerState {
                bag_family: 0,
                name: None,
                num_slots: 16,
                slots,
            }),
        );
        let mut worn: crate::script::InventorySlots = Default::default();
        worn[1] = Some(crate::script::InvSlotView {
            item_id: 77,
            count: 1,
            equip_slots: vec![1],
            ..Default::default()
        });
        s.set_inventory_slots(worn);
        s
    }

    fn slot_locked(s: &UiScript, slot: u32) -> bool {
        s.eval::<bool>(&format!(
            "return select(3, GetContainerItemInfo(0, {slot})) == 1"
        ))
        .unwrap()
    }

    /// A swap that asks `EQUIP_BIND_CONFIRM` returns before the send and the clear (`0x6dfb3d`, `0x6dfb45`): the cursor still holds the item, both ends locked as the callers left them (`0x5d82af`, `0x5e89c7`), and nothing goes out.
    #[test]
    fn a_swap_that_asks_the_bind_leaves_the_cursor_holding() {
        // A bag slot put-down of a worn item over a bind-on-equip one.
        let mut s = swap_rig();
        s.set_swap_gate(0, 1, SwapGate::Ask(0));
        s.run("PickupInventoryItem(1)").unwrap();
        crate::script::tick::fired::take();
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            crate::script::tick::fired::names(),
            ["ITEM_LOCK_CHANGED", "ITEM_LOCKED"],
            "the slot's item locks and no CURSOR_UPDATE follows"
        );
        assert!(s.cursor_item().is_some_and(|i| i.bag == EQUIPMENT_BAG));
        assert!(s.take_container_moves().is_empty());
        assert_eq!(
            s.take_container_bind_asks(),
            [ContainerMove {
                src_bag: EQUIPMENT_BAG,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 1,
                count: None,
            }]
        );
        assert!(slot_locked(&s, 1));

        // The doll's put-down of a bag item: the slot's item locks after the swap returns.
        let mut s = swap_rig();
        s.set_swap_gate(EQUIPMENT_BAG, 1, SwapGate::Ask(0));
        s.run("PickupContainerItem(0, 1)").unwrap();
        crate::script::tick::fired::take();
        s.run("PickupInventoryItem(1)").unwrap();
        assert_eq!(
            crate::script::tick::fired::names(),
            ["ITEM_LOCK_CHANGED", "ITEM_LOCKED"]
        );
        assert!(s.cursor_item().is_some_and(|i| (i.bag, i.slot) == (0, 1)));
        assert!(s.take_container_moves().is_empty());
        assert_eq!(s.take_container_bind_asks().len(), 1);
        assert!(s
            .eval::<bool>("return IsInventoryItemLocked(1) == 1")
            .unwrap());
    }

    /// A swap refused before its send (`0x6df991` -> `0x6dfb45`) and one kept for its record (`0x6dfa1d`) both return with the cursor holding and nothing sent; the record's landing runs the kept swap, whose send clears the cursor with the item still locked (`0x6db784`, `0x6dfc0d`).
    #[test]
    fn a_refused_or_waiting_swap_leaves_the_cursor_holding() {
        let mut s = swap_rig();
        s.set_swap_gate(0, 2, SwapGate::Refuse);
        s.run("PickupContainerItem(0, 1) PickupContainerItem(0, 2)")
            .unwrap();
        assert!(s.cursor_item().is_some_and(|i| i.slot == 1));
        assert!(s.take_container_moves().is_empty() && s.take_container_bind_asks().is_empty());
        assert!(
            slot_locked(&s, 2),
            "the slot's item locked before the swap ran"
        );

        let mut s = swap_rig();
        s.set_swap_gate(0, 2, SwapGate::Wait(4500));
        s.run("PickupContainerItem(0, 1) PickupContainerItem(0, 2)")
            .unwrap();
        assert!(s.cursor_item().is_some() && s.take_container_moves().is_empty());
        let mv = ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 2,
            count: None,
        };
        assert_eq!(
            std::mem::take(&mut s.model_mut().item_record_waits),
            [(4500, crate::script::ItemRecordWait::Swap(mv))]
        );
        // A record that did not land runs nothing (`0x6db760`).
        s.item_record_callback(4500, false, crate::script::ItemRecordWait::Swap(mv));
        assert!(s.cursor_item().is_some() && s.take_container_moves().is_empty());
        s.set_swap_gate(0, 2, SwapGate::Send);
        crate::script::tick::fired::take();
        s.item_record_callback(4500, true, crate::script::ItemRecordWait::Swap(mv));
        assert_eq!(crate::script::tick::fired::names(), ["CURSOR_UPDATE"]);
        assert_eq!(s.take_container_moves(), [mv]);
        assert!(s.cursor_payload().is_none() && slot_locked(&s, 1));
    }

    /// An item the player cannot use asks nothing: the swap goes out (`0x6dfac8`).
    #[test]
    fn a_bind_on_equip_item_the_player_cannot_use_swaps_unasked() {
        let mut s = swap_rig();
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 40,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        });
        s.set_item_template(
            4500,
            crate::script::ItemTemplateView {
                allowable_class: -1,
                allowable_race: -1,
                required_level: 60,
                ..Default::default()
            },
        );
        s.set_swap_gate(0, 2, SwapGate::Ask(4500));
        assert!(!s.item_usable(4500));
        s.run("PickupContainerItem(0, 1) PickupContainerItem(0, 2)")
            .unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(s.take_container_moves().len(), 1);
        assert!(s.take_container_bind_asks().is_empty());
    }

    /// The put-down's clear leaves the held item locked until the answer (`0x6dfc0d`).
    #[test]
    fn pickup_then_place_swaps_and_clears_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1 = Tough Jerky (resolved), slot 3 = in-flight
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());

        s.run("PickupContainerItem(0, 1)").unwrap();
        let held = s.cursor_item().expect("cursor holds the picked item");
        assert_eq!((held.bag, held.slot, held.item_id), (0, 1, 117));
        assert_eq!(
            held.texture.as_deref(),
            Some("Interface\\Icons\\INV_Misc_Food_16")
        );
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
        assert!(s.take_container_moves().is_empty());

        assert_eq!(s.cursor_item().map(|c| c.item_id), Some(117));

        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the swap's clear does not unlock"
        );
    }

    /// The reference's place never sets the cursor: the displaced item is never held. The slot's item locks before the swap goes out (`0x5d82af`) and the held one stays locked (`0x6dfc0d`).
    #[test]
    fn pickup_place_onto_occupied_different_item_swaps_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1 = item 117 (A)
        state.slots.insert(
            5,
            ContainerSlot {
                guid: 0,
                gems: [0; 3],
                quest_item: false,
                start_quest: 0,
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
                count: 1,
                quality: Some(2),
                item_id: 200, // item B, a different id
                link: Some("|Hitem:200|h[Shiny Gem]|h".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.cursor_item().unwrap().item_id, 117);

        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(
            s.cursor_item().is_none(),
            "a swap clears the cursor — the displaced item never hops on"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        for slot in [1, 5] {
            assert!(
                s.eval::<bool>(&format!(
                    "local _, _, locked = GetContainerItemInfo(0, {slot}) return locked"
                ))
                .unwrap(),
                "slot {slot} stays locked until the answer"
            );
        }
        // A locked item in the slot refuses the next swap, the cursor as it was (`0x5d82a4`).
        s.run("ClearCursor() PickupContainerItem(0, 4)").unwrap();
        let held = s.cursor_item().map(|c| c.slot);
        s.run("PickupContainerItem(0, 5)").unwrap();
        assert_eq!(s.cursor_item().map(|c| c.slot), held);
        assert!(s.take_container_moves().is_empty());
    }

    /// A same-item placement is a plain move: the server merges the stacks.
    #[test]
    fn pickup_place_onto_same_item_merges_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack();
        state.slots.insert(
            5,
            ContainerSlot {
                durability: None,
                item_id: 117, // same id as slot 1
                count: 2,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run("PickupContainerItem(0, 5)").unwrap();
        assert!(
            s.cursor_item().is_none(),
            "same-item merge clears the cursor"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
    }

    #[test]
    fn pickup_refuses_a_locked_slot() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack();
        state.slots.get_mut(&1).unwrap().locked = true;
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn split_container_item_carries_a_partial_stack() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1: a 5-stack of item 117
        state.slots.insert(
            7,
            ContainerSlot {
                durability: None,
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        state.slots.insert(
            9,
            ContainerSlot {
                durability: None,
                item_id: 999, // a different item
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 3)")
            .unwrap());
        let held = s.cursor_item().expect("the split carry");
        assert_eq!(
            (held.bag, held.slot, held.item_id, held.count),
            (0, 1, 117, Some(3))
        );

        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 2,
                count: Some(3),
            }]
        );

        // The stack stays locked until the answer (`0x5d8281`), and a locked stack does not split (`0x5d85b9`).
        assert!(!s
            .eval::<bool>("return SplitContainerItem(0, 1, 3)")
            .unwrap());
        assert!(s.cursor_item().is_none());
    }

    /// A split over another item goes out as it is, the server's to refuse (`0x5d8278`): no test of the slot's item comes before it.
    #[test]
    fn a_split_carry_goes_out_over_whatever_the_slot_holds() {
        for dst in [7, 9] {
            let mut s = UiScript::new().unwrap();
            let mut state = backpack();
            for (slot, item_id) in [(7, 117), (9, 999)] {
                state.slots.insert(
                    slot,
                    ContainerSlot {
                        item_id,
                        count: 1,
                        ..Default::default()
                    },
                );
            }
            s.set_container(0, Some(state));
            s.run("SplitContainerItem(0, 1, 3)").unwrap();
            s.run(&format!("PickupContainerItem(0, {dst})")).unwrap();
            assert!(s.cursor_item().is_none(), "slot {dst}");
            assert_eq!(
                s.take_container_moves(),
                vec![ContainerMove {
                    src_bag: 0,
                    src_slot: 1,
                    dst_bag: 0,
                    dst_slot: dst,
                    count: Some(3),
                }]
            );
            assert!(
                !s.eval::<bool>(&format!(
                    "local _, _, locked = GetContainerItemInfo(0, {dst}) return locked"
                ))
                .unwrap(),
                "a split locks nothing in the slot"
            );
        }
    }

    /// The whole stack goes up with no split count (`0x5d8609`); a count past the stack is refused (`0x5d85e6`).
    #[test]
    fn split_of_the_whole_stack_is_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1: a 5-stack
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 5)")
            .unwrap());
        assert_eq!(s.cursor_item().unwrap().count, None);
        s.run("ClearCursor()").unwrap();
        assert!(!s
            .eval::<bool>("return SplitContainerItem(0, 1, 99)")
            .unwrap());
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn pickup_same_slot_cancels_no_move() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn pickup_empty_slot_holds_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        // Slot 2 is empty and slot 3 unresolved: neither can be picked up.
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(s.cursor_item().is_none());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn clear_cursor_drops_the_held_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn repair_cursor_wins_over_a_sellable_bag_hover() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));

        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));

        s.run("ShowRepairCursor()").unwrap();
        assert_eq!(
            s.ui_cursor(),
            None,
            "repair replaces the existing Buy hover"
        );
        s.run("ShowContainerSellCursor(0, 1)").unwrap();
        assert_eq!(
            s.ui_cursor(),
            None,
            "a bag hover keeps the repair base cursor"
        );
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_container_repairs(),
            vec![(0, 1), (0, 1)],
            "repair mode sticks across clicks"
        );
    }

    #[test]
    fn right_click_repairs_bag_item_instead_of_selling_it() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor() UseContainerItem(0, 1) UseContainerItem(0, 1)")
            .unwrap();

        assert_eq!(s.take_container_repairs(), vec![(0, 1), (0, 1)]);
        assert!(s.take_container_uses().is_empty(), "no sell/use intent");
    }

    #[test]
    fn right_click_in_repair_mode_puts_a_held_item_back_then_repairs() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        // A held item and repair mode together, as after a cursor-mode change.
        s.model_mut().repair_mode = true;

        s.run("UseContainerItem(0, 4)").unwrap();
        assert!(s.cursor_payload().is_none(), "the held item went back");
        assert!(s.take_container_moves().is_empty(), "never placed");
        assert_eq!(s.take_container_repairs(), vec![(0, 4)]);

        s.model_mut().repair_mode = false;
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.model_mut().repair_mode = true;
        s.run("UseContainerItem(0, 2)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "an empty slot still clears the cursor"
        );
        assert!(s.take_container_repairs().is_empty(), "and repairs nothing");
    }

    #[test]
    fn held_vendor_row_buys_into_bag_before_repair_mode() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.set_merchant(Some(MerchantState {
            items: vec![MerchantItem {
                item_id: 159,
                ..Default::default()
            }],
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor() PickupMerchantItem(1)").unwrap();
        assert!(
            s.repair_mode(),
            "vendor grab preserves the repair base mode"
        );
        assert!(s.cursor_payload().is_some());

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(s.take_merchant_slot_buys(), vec![(0, 1, 1)]);
        assert!(s.cursor_payload().is_none());
        assert!(s.take_container_repairs().is_empty());
        assert!(s.take_sounds().is_empty());

        // The right-click clears the vendor row with no packet and repairs instead.
        s.run("PickupMerchantItem(1) UseContainerItem(0, 1)")
            .unwrap();
        assert!(s.take_merchant_slot_buys().is_empty());
        assert!(s.cursor_payload().is_none());
        assert_eq!(s.take_container_repairs(), vec![(0, 1)]);
        assert!(s.take_container_uses().is_empty());
    }

    #[test]
    fn held_item_places_before_repair_mode() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        // A held item and repair base mode can coexist after a cursor-mode change.
        s.model_mut().repair_mode = true;

        s.run("PickupContainerItem(0, 2)").unwrap();
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 2,
                count: None,
            }]
        );
        assert!(s.cursor_payload().is_none());
        assert!(s.take_container_repairs().is_empty());
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn repair_mode_repairs_under_held_coins_and_keeps_them() {
        use crate::script::cursor::{CursorMoney, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.model_mut().repair_mode = true;
        let coins = CursorPayload::Money(CursorMoney { copper: 50 });
        s.model_mut().cursor = Some(coins.clone());

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_container_repairs(),
            vec![(0, 1)],
            "coins pass both payload tests"
        );
        assert_eq!(s.cursor_payload(), Some(coins), "and stay held");
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert!(
            s.take_container_repairs().is_empty(),
            "an empty slot does nothing"
        );
    }

    #[test]
    fn removing_a_bag_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_container(2, Some(backpack()));
        s.set_container(2, None);
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(2)").unwrap(), 0);
    }

    #[test]
    fn container_item_cooldown_reads_the_gettime_triple() {
        let mut s = UiScript::new().unwrap();
        s.tick(100.0); // an arbitrary clock epoch
        let mut state = backpack();
        // Slot 1's item is 4 s into a 10 s use-cooldown: started at GetTime 96.
        state.slots.get_mut(&1).unwrap().cooldown = Some((96_000, 10_000, true));
        s.set_container(0, Some(state));

        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 96 and d == 10 and e == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 3)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // Past expiry, the same stored triple reads cold.
        s.tick(7.0);
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // A re-push without the cooldown clears the stored triple.
        s.set_container(0, Some(backpack()));
        assert!(s
            .eval::<bool>(
                "local s, d = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0"
            )
            .unwrap());
    }

    // ── `ContainerIDToInventoryID` (`0x5d6f60`) ─────────────────────────────────────

    #[test]
    fn container_id_to_inventory_id_maps_bags_1_to_11() {
        let s = UiScript::new().unwrap();
        let slot = |id: &str| {
            s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                .unwrap()
        };
        assert_eq!(
            (slot("1"), slot("2"), slot("3"), slot("4")),
            (20, 21, 22, 23)
        );
        // The bank bags: +63 from 5.
        assert_eq!(slot("5"), 68, "the jump — +63, not +19");
        assert_eq!((slot("6"), slot("11")), (69, 74));
        assert_eq!(
            s.arity("ContainerIDToInventoryID(11)").unwrap(),
            1,
            "one value on every non-raising path"
        );
        // `0x88b9c0` truncates toward zero, a C cast, not `floor`.
        assert_eq!(slot("2.9"), 21, "2.9 -> 2");
        assert_eq!(slot("11.9"), 74, "11.9 -> 11");
    }

    /// `id - 1` outside 0..=10 raises (`0x5d6fd6`): the backpack, the bank and the keyring too.
    #[test]
    fn container_id_to_inventory_id_raises_outside_bags_1_to_11() {
        let s = UiScript::new().unwrap();
        for id in ["0", "-1", "-2", "12", "-0.5", "99"] {
            let err = s
                .eval::<mlua::Value>(&format!("return ContainerIDToInventoryID({id})"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("ContainerIDToInventoryID(): invalid container ID"),
                "{id} must raise, got {err}"
            );
        }
        // The argument truncates first: -0.5 is 0 above, and 1.5 is bag 1.
        assert_eq!(
            s.eval::<i64>("return ContainerIDToInventoryID(1.5)")
                .unwrap(),
            20
        );
    }

    /// A missing or non-number argument raises: the type test's error does not return.
    #[test]
    fn container_id_to_inventory_id_raises_on_a_bad_argument() {
        let s = UiScript::new().unwrap();
        for call in ["ContainerIDToInventoryID()", "ContainerIDToInventoryID({})"] {
            let err = s
                .eval::<mlua::Value>(&format!("return {call}"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: ContainerIDToInventoryID(containerID)"),
                "{call} must raise, got {err}"
            );
        }
    }

    // ── The 3.3.5a bag verbs ──────────────────────────────────────────────────────────

    /// A worn bag of `size` slots holding one item, of template family `family`.
    fn worn_bag(size: u32, family: u32) -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                item_id: 2_512,
                count: 200,
                ..Default::default()
            },
        );
        ContainerState {
            name: Some("Quiver".into()),
            num_slots: size,
            slots,
            bag_family: family,
        }
    }

    /// `0x5d7590`: two values for a resolved container, the lone 0 for everything else.
    #[test]
    fn free_slots_count_empties_and_name_the_bag_family() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetContainerNumFreeSlots(0)").unwrap(),
            1,
            "no player yet: the lone 0"
        );
        s.set_container(0, Some(backpack()));
        s.set_container(1, Some(worn_bag(6, 1)));
        s.set_container(-1, Some(worn_bag(28, 0)));
        let free = |s: &UiScript, bag: &str| -> (i64, Option<i64>) {
            s.eval(&format!("return GetContainerNumFreeSlots({bag})"))
                .unwrap()
        };
        // The backpack holds three items (one still in flight) in 16 slots.
        assert_eq!(free(&s, "0"), (13, Some(0)));
        assert_eq!(free(&s, "1"), (5, Some(1)), "a quiver names its family");
        assert_eq!(
            s.arity("GetContainerNumFreeSlots(2)").unwrap(),
            1,
            "empty bag slot"
        );
        assert_eq!(free(&s, "2"), (0, None));
        assert_eq!(free(&s, "-1"), (0, None), "the bank, closed");
        let mut bank_v = crate::script::live::TestView {
            banker: Some(0x42),
            ..Default::default()
        };
        crate::script::live::lending(&mut bank_v, || {
            assert_eq!(free(&s, "-1"), (27, Some(0)), "the bank, open");
            assert_eq!(free(&s, "-2"), (0, None), "the keyring is not a case");
            assert_eq!(free(&s, "12"), (0, None));
            let err = s
                .eval::<mlua::Value>("return GetContainerNumFreeSlots()")
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: GetContainerFreeSlots(index)"),
                "the reference's own usage name, got {err}"
            );
        });
    }

    /// `0x5d9400`: always three values; `true` or nil, the quest id or nil, then whether the
    /// quest is already in the log.
    #[test]
    fn quest_info_marks_quest_items_and_quest_starters() {
        let mut s = UiScript::new().unwrap();
        let mut bag = backpack();
        bag.slots.get_mut(&1).unwrap().quest_item = true;
        bag.slots.insert(
            5,
            ContainerSlot {
                item_id: 1_307,
                start_quest: 123,
                ..Default::default()
            },
        );
        bag.slots.insert(
            6,
            ContainerSlot {
                item_id: 1_972,
                start_quest: 456,
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        s.seed_quest_log(crate::script::QuestLogState {
            entries: vec![crate::script::QuestLogEntryView {
                quest_id: 456,
                ..Default::default()
            }],
            ..Default::default()
        });
        let info = |slot: u32| -> (Option<bool>, Option<i64>, Option<bool>) {
            s.eval(&format!("return GetContainerItemQuestInfo(0, {slot})"))
                .unwrap()
        };
        assert_eq!(info(1), (Some(true), None, None), "a bonding-4 item");
        assert_eq!(
            info(5),
            (None, Some(123), Some(false)),
            "a starter not taken"
        );
        assert_eq!(
            info(6),
            (None, Some(456), Some(true)),
            "a starter in the log"
        );
        assert_eq!(info(3), (None, None, None), "an unresolved template");
        assert_eq!(info(2), (None, None, None), "an empty slot");
        assert_eq!(s.arity("GetContainerItemQuestInfo(0, 2)").unwrap(), 3);
        assert!(s
            .eval::<mlua::Value>("return GetContainerItemQuestInfo(0)")
            .is_err());
    }

    /// `0x5d7180`: 0 and below return untouched, 12 and up raise; 1-11 clear and set.
    #[test]
    fn bag_portrait_takes_the_3_3_5a_name_and_range() {
        let s = UiScript::new().unwrap();
        s.run("PortraitProbe = CreateFrame('Frame'):CreateTexture()")
            .unwrap();
        for id in ["0", "-1", "1", "11"] {
            s.run(&format!("SetBagPortraitTexture(PortraitProbe, {id})"))
                .unwrap_or_else(|e| panic!("id {id} must not raise: {e}"));
        }
        let err = s
            .run("SetBagPortraitTexture(PortraitProbe, 12)")
            .unwrap_err();
        assert!(format!("{err}").contains("Invalid slot in SetBagPortraitTexture"));
        let err = s.run("SetBagPortraitTexture(PortraitProbe)").unwrap_err();
        assert!(format!("{err}").contains("Usage: SetBagPortraitTexture(texture, slot)"));
        assert!(
            s.eval::<bool>("return SetBagPortaitTexture == nil")
                .unwrap(),
            "1.12's misspelling is not a 3.3.5a global"
        );
    }

    /// No refund record is held, so both purchase verbs answer nothing; bad arguments raise.
    #[test]
    fn purchase_info_answers_nothing_without_a_refund_record() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 0);
        assert_eq!(s.arity("GetContainerItemPurchaseItem(0, 1, 1)").unwrap(), 0);
        let err = s
            .eval::<mlua::Value>("return GetContainerItemPurchaseItem(0, 1)")
            .unwrap_err();
        assert!(format!("{err}").contains("Usage: ContainerItemPurchaseItem("));
    }

    /// With the host's state lent the swap's question is filed and signalled inside the call (`0x6dface`, `0x6dfb3d`), after the lock the bag slot's click takes before the swap (`0x5d82af`, `0x5d82dc`), and nothing waits for the drain.
    #[test]
    fn a_swap_that_asks_signals_its_question_inside_the_call() {
        use crate::script::{HostAnswer, HostAsk};
        let mut s = swap_rig();
        s.set_swap_gate(0, 1, SwapGate::Ask(0));
        s.run(
            r#"local f = CreateFrame("Frame")
               f:RegisterEvent("EQUIP_BIND_CONFIRM")
               f:SetScript("OnEvent", function(_, _, index) ASKED = { index, CursorHasItem(), SEEN } end)
               PickupInventoryItem(1)"#,
        )
        .unwrap();
        crate::script::tick::fired::take();
        let asked = crate::script::calls::in_call::run(
            &mut s,
            Some(HostAnswer::BindIndex(3)),
            "PickupContainerItem(0, 1) SEEN = true",
        );
        assert_eq!(
            asked,
            [HostAsk::BindSwap(ContainerMove {
                src_bag: EQUIPMENT_BAG,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 1,
                count: None,
            })]
        );
        assert_eq!(
            crate::script::tick::fired::names(),
            ["ITEM_LOCK_CHANGED", "ITEM_LOCKED", "EQUIP_BIND_CONFIRM"]
        );
        assert!(s
            .eval::<bool>("return ASKED[1] == 3 and ASKED[2] == 1 and ASKED[3] == nil")
            .unwrap());
        assert!(s.cursor_item().is_some_and(|i| i.bag == EQUIPMENT_BAG));
        assert!(s.take_container_bind_asks().is_empty() && s.take_container_moves().is_empty());
    }

    /// A targeting cursor is stopped inside the call, before the item goes up (`0x5d811f` ahead of `0x5d8228`), and by the split between its clear and its setter (`0x5d85ec`, `0x5d85f4`, `0x5d8621`).
    #[test]
    fn a_pickup_stops_targeting_before_its_cursor_signals() {
        use crate::script::live::TestTargeting;
        let mut s = swap_rig();
        s.set_spell_targeting(true);
        crate::script::tick::fired::take();
        crate::script::calls::in_call::run(&mut s, None, "PickupContainerItem(0, 1)");
        let fired = crate::script::tick::fired::names();
        assert_eq!(fired[0], "APPLIED SpellStopTargeting", "{fired:?}");
        assert!(
            fired[1..].contains(&"CURSOR_UPDATE".to_string()),
            "{fired:?}"
        );
        assert!(!s.take_stop_targeting(), "nothing waits for the press pass");

        crate::script::calls::in_call::run(&mut s, None, "SplitContainerItem(0, 2, 1)");
        let fired = crate::script::tick::fired::names();
        let stop = fired
            .iter()
            .position(|e| e == "APPLIED SpellStopTargeting")
            .expect("the split stops it");
        // The clear's unlock of the held item, the stop, then the setter's signals and the lock.
        assert_eq!(
            fired[..stop].last().map(String::as_str),
            Some("CURSOR_UPDATE"),
            "{fired:?}"
        );
        assert!(
            fired[..stop].contains(&"ITEM_UNLOCKED".to_string()),
            "{fired:?}"
        );
        assert!(
            fired[stop..].contains(&"ITEM_LOCKED".to_string()),
            "{fired:?}"
        );
        assert!(
            !fired[stop..].contains(&"ITEM_UNLOCKED".to_string()),
            "{fired:?}"
        );
    }
}
