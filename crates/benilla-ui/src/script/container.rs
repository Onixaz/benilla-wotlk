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
    /// `ITEM_FIELD_ENCHANTMENT` slot: 0 permanent, 1 temporary, 2-6 the random suffix; only 0 and
    /// 1 are coloured.
    pub slot: u8,
    /// The id was negative: `abs(id)` names the row, and slots 0 and 1 paint red, not green.
    pub negative: bool,
    /// The `SpellItemEnchantment` row's name, copied unformatted.
    pub name: String,
    /// The slot's charges; nonzero appends " (N Charges)".
    pub charges: u32,
    /// Ms left on a temporary enchant, from `SMSG_ITEM_ENCHANT_TIME_UPDATE`, never the item's own
    /// duration field; `Some` prints the countdown form.
    pub remaining_ms: Option<u64>,
}

/// One `ItemRandomProperties` row, an item's random-suffix roll. The app pushes the whole table at
/// load, and a tooltip source supplies only the id, as in the reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RandomPropertyView {
    /// The suffix (`"of the Monkey"`), joined on by `ITEM_SUFFIX_TEMPLATE`, for the names the app
    /// does not compose itself.
    pub suffix: String,
    /// The roll's five enchants, named, in slots 2-6; always white.
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub count: Option<u32>,
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
        cursor::queue_lock_changed(&mut model, bag, slot);
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

/// `PickupContainerItem`'s gesture:
/// - an empty cursor picks up a resolved item whose slot is not locked;
/// - a click on the held item's own slot cancels;
/// - a whole stack placed anywhere else queues the move and clears the cursor; a swap lands the
///   displaced item in the source slot, never on the cursor (no cursor set);
/// - a split carry places onto an empty or same-item slot, and stays held over another item;
/// - a spell or action payload stays held.
fn pickup_container_item(model: &mut super::Model, bag: i64, slot: u32) {
    // An armed gift wrap completes here, before the cursor is read. `PickupContainerItem` is the
    // only caller of the wrap sender, so an equipped item cannot be wrapped.
    if let Some(wrap) = model.pending_wrap {
        // An empty or unresolved target leaves the wrap armed, as in the reference.
        if !model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return;
        }
        // A vanished paper also bails, the wrap armed and the paper locked.
        if !model
            .containers
            .get(&wrap.bag)
            .and_then(|c| c.slots.get(&wrap.slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return;
        }
        // No eligibility test: the server refuses with an `ERR_CANT_WRAP_*` reason
        // (`SMSG_INVENTORY_CHANGE_FAILURE` 43-48), and a local gate would eat that error line.
        model.pending_wrap = None;
        model.container_wraps.push((wrap.bag, wrap.slot, bag, slot));
        // The paper unlocks and the cursor resets right after the send, not on the answer; the
        // target never locks.
        cursor::queue_lock_changed(model, wrap.bag, wrap.slot);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
        return;
    }
    // Only a held item or vendor row takes the click first. Past them an empty slot does nothing,
    // an armed item-targeting spell binds the item and repair mode repairs it, any other payload
    // staying held; the app's repair drain runs the arm's `ITEM_REPAIR` and send.
    let placing = matches!(
        model.cursor,
        Some(CursorPayload::Item(_) | CursorPayload::Merchant(_))
    );
    if !placing && (model.item_pick_armed || model.repair_mode) {
        let occupied = model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_id != 0);
        if occupied && model.item_pick_armed {
            model.item_picks.push((bag, slot));
        } else if occupied {
            model.container_repairs.push((bag, slot));
        }
        return;
    }
    match model.cursor.take() {
        None => {
            let picked = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .filter(|s| s.item_id != 0 && !s.locked)
                .map(|s| CursorItem {
                    bag,
                    slot,
                    item_id: s.item_id,
                    texture: s.texture.clone(),
                    link: s.link.clone(),
                    quality: s.quality,
                    count: None,
                    bar_placeable: s.bar_placeable,
                    equip_slots: s.equip_slots.clone(),
                });
            if let Some(item) = picked {
                model.cursor = Some(CursorPayload::Item(item));
                cursor::queue_cursor_update(model);
                cursor::queue_lock_changed(model, bag, slot);
            }
        }
        Some(CursorPayload::Item(held)) if held.bag == bag && held.slot == slot => {
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, held.bag, held.slot);
        }
        Some(CursorPayload::Item(held)) => {
            // Cloned so the moves below can borrow `model`; an unresolved slot reads as empty.
            let dest = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .cloned()
                .filter(|d| d.item_id != 0);
            match (held.count, dest) {
                // A split carry cannot swap with a different item: it stays held.
                (Some(_), Some(d)) if d.item_id != held.item_id => {
                    model.cursor = Some(CursorPayload::Item(held));
                }
                // Anything else queues the move and clears: a split, a merge the server tops up,
                // or a swap that lands the displaced item in the source slot.
                (count, _) => {
                    queue_move(model, &held, bag, slot, count);
                    cursor::queue_cursor_update(model);
                    cursor::queue_lock_changed(model, held.bag, held.slot);
                }
            }
        }
        // Mode 5, a held vendor row, buys into this slot whatever it holds: `CMSG_BUY_ITEM_IN_SLOT`
        // (0x1a3) with count 1, and the cursor clears either way. Deviation: a row that no longer
        // resolves is refused, as the reference does, because this path dereferences it unchecked,
        // a latent null deref.
        Some(CursorPayload::Merchant(held)) => {
            let row = model
                .merchant
                .as_ref()
                .and_then(|m| m.items.get(held.row as usize))
                .filter(|it| it.item_id == held.item_id)
                .map(|_| held.row + 1);
            if let Some(row) = row {
                model.merchant_slot_buys.push((bag, slot, row));
            }
            cursor::queue_cursor_update(model);
        }
        // Mode 11, a guild bank item, withdraws into this slot (`0x5d7ff0`).
        Some(CursorPayload::GuildBankItem(held)) => {
            super::guild_bank::drop_on_bag_slot(model, held, bag, slot);
        }
        Some(
            other @ (CursorPayload::Spell(_)
            | CursorPayload::Action(_)
            // Mode 12, bank money, stays held for the bag button's `WithdrawGuildBankMoney`.
            | CursorPayload::GuildBankMoney(_)
            | CursorPayload::Macro(_)
            | CursorPayload::EquipmentSet(_)
            | CursorPayload::PetAction(_)
            // Mode 10, a stabled pet, stays held for the stable window.
            | CursorPayload::StablePet(_)
            // Mode 2, money, stays held for a money frame's DropFunc.
            | CursorPayload::Money(_)),
        ) => {
            model.cursor = Some(other);
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
                    model.bank.is_some().then_some(index + 64)
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
            Ok(model.containers.get(&bag).map_or(0, |c| c.num_slots))
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
            if !model.containers.contains_key(&0) {
                return Ok(mlua::MultiValue::from_vec(vec![Value::Integer(0)]));
            }
            let bank_open = model.bank.is_some();
            let resolved = match bag {
                0 => true,
                -1 | 5..=11 => bank_open,
                1..=4 => true,
                _ => false,
            };
            let answer = resolved
                .then(|| model.containers.get(&bag))
                .flatten()
                .map(|c| {
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
                .and_then(|slot| model.containers.get(&bag)?.slots.get(&slot))
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
            let log = &model.quest_log;
            let active = log
                .entries
                .iter()
                .any(|e| !e.is_header && e.quest_id == s.start_quest)
                || log.hidden_quest_ids.contains(&s.start_quest);
            Ok((
                quest_item,
                Value::Integer(i64::from(s.start_quest)),
                Value::Boolean(active),
            ))
        })?,
    )?;

    // GetContainerItemPurchaseInfo(bag, slot[, isEquipped]) (`0x5d8d80`) → money, honorPoints,
    // arenaPoints, itemCount, refundSec, read off the item's refund record (`item+0x3d4`) while
    // its two-hour window is open. The record is filled only by `SMSG_ITEM_REFUND_INFO_RESPONSE`,
    // which benilla does not read yet, so every item answers as one without a record: nothing.
    lua.globals().set(
        "GetContainerItemPurchaseInfo",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetContainerItemPurchaseInfo(index, slot[, isEquipped])";
            super::binding_abi::number_arg(lua, bag, USAGE)?;
            super::binding_abi::number_arg(lua, slot, USAGE)?;
            Ok(())
        })?,
    )?;

    // GetContainerItemPurchaseItem(bag, slot, itemIndex[, isEquipped]) (`0x5d8f70`) → texture,
    // count, link of the record's `itemIndex`th cost item; nothing without a refund record, as
    // above. The usage text is the reference's, which drops the `Get`.
    lua.globals().set(
        "GetContainerItemPurchaseItem",
        lua.create_function(|lua, (bag, slot, index): (Value, Value, Value)| {
            const USAGE: &str =
                "Usage: ContainerItemPurchaseItem(bag, slot, itemIndex[, isEquipped])";
            for v in [bag, slot, index] {
                super::binding_abi::number_arg(lua, v, USAGE)?;
            }
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "GetBagName",
        lua.create_function(|lua, bag: i64| {
            let name = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.containers.get(&bag).and_then(|c| c.name.clone())
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
            Ok(match model.container_cooldowns.get(&(bag, slot)) {
                Some(&(start, duration, enabled)) if start + duration > now || !enabled => {
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
                model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .and_then(|s| s.link.clone())
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetContainerItemInfo(bag, slot) → texture, itemCount, locked, quality, readable, the five
    // `ContainerFrame.lua:241` reads; no values for an empty or unknown slot.
    lua.globals().set(
        "GetContainerItemInfo",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let (info, held_here) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let held = matches!(
                    &model.cursor,
                    Some(CursorPayload::Item(c)) if c.bag == bag && c.slot == slot
                ) ||
                // An armed wrap's paper reads locked until the send or a cancel.
                    matches!(
                        &model.pending_wrap,
                        Some(w) if w.bag == bag && w.slot == slot
                    ) ||
                // The auction sell slot, a multi-sell's items and the mail attachments read
                // locked, as the client's item lock marks them.
                    super::auction::holds_slot(&model, bag, slot)
                    || super::mail::holds_slot(&model, bag, slot);
                (
                    model
                        .containers
                        .get(&bag)
                        .and_then(|c| c.slots.get(&slot))
                        .cloned(),
                    held,
                )
            };
            let Some(s) = info else {
                return Ok(mlua::MultiValue::new());
            };
            Ok(mlua::MultiValue::from_vec(vec![
                match &s.texture {
                    Some(p) => Value::String(lua.create_string(p.as_str())?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(s.count)),
                // A held item's source slot reads locked, derived from the cursor so the app's
                // re-push cannot wipe it.
                Value::Boolean(s.locked || held_here),
                match s.quality {
                    Some(q) => Value::Integer(i64::from(q)),
                    None => Value::Nil,
                },
                Value::Boolean(s.readable),
            ]))
        })?,
    )?;

    lua.globals().set(
        "UseContainerItem",
        lua.create_function(|lua, (bag, slot, _rest): (i64, u32, mlua::MultiValue)| {
            let mut model = crate::script::model_mut_of(lua);
            // Every right-click clears the cursor before it reads the slot: a held payload goes
            // back, never placed, and an armed gift wrap is cancelled.
            cursor::clear_cursor(&mut model);
            if model.repair_mode {
                // Then the item in the slot is repaired.
                let occupied = model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .is_some_and(|s| s.item_id != 0);
                if occupied {
                    model.container_repairs.push((bag, slot));
                }
            } else if !super::mail::use_container_item(&mut model, bag, slot)
                && !super::auction::use_container_item(&mut model, bag, slot)
                && !super::guild_bank::use_container_item(&mut model, bag, slot)
            {
                // Past repair, the Send tab attaches the item (`0x5d88cf`), then the Auctions tab
                // takes it for the sell slot (`0x5d88e0`), then an open guild bank stores it.
                model
                    .script_calls
                    .push(super::ScriptCall::UseContainerItem { bag, slot });
            }
            Ok(())
        })?,
    )?;

    // PickupContainerItem(bag, slot): pick up, or place and swap; no return value on any path.
    lua.globals().set(
        "PickupContainerItem",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let mut model = crate::script::model_mut_of(lua);
            pickup_container_item(&mut model, bag, slot);
            Ok(())
        })?,
    )?;

    // ShowContainerSellCursor(bag, slot): Buy (3) over an occupied slot, with no price check, so
    // selling never greys; an empty slot leaves the cursor alone.
    lua.globals().set(
        "ShowContainerSellCursor",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let mut model = crate::script::model_mut_of(lua);
            let occupied = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .is_some();
            // An armed spell or repair cursor suppresses it first; each is sticky, so a Buy here
            // would stamp out the active base cursor.
            if occupied && !model.spell_targeting && !model.repair_mode {
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
            Ok(if model.has_key {
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
    use super::{ContainerMove, ContainerSlot, ContainerState, PendingWrap, UiCursorMode};
    use crate::script::{MerchantItem, MerchantState, UiScript};

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
        assert_eq!((count, quality), (5, 1));
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

        // An in-flight slot: texture and quality nil, itemCount real.
        assert!(s
            .eval::<bool>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 3)\n\
                 return texture == nil and quality == nil and itemCount ~= nil",
            )
            .unwrap());
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
        assert!(!s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
    }

    /// The reference's place never sets the cursor: the displaced item is never held.
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
        assert!(!s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
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

        s.run("SplitContainerItem(0, 1, 3)").unwrap();
        s.run("PickupContainerItem(0, 9)").unwrap();
        let held = s.cursor_item().expect("kept — can't swap a partial stack");
        assert_eq!(held.count, Some(3));
        assert!(s.take_container_moves().is_empty());

        s.run("PickupContainerItem(0, 7)").unwrap();
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 7,
                count: Some(3),
            }]
        );
    }

    #[test]
    fn split_of_the_whole_stack_is_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1: a 5-stack
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 5)")
            .unwrap());
        assert_eq!(s.cursor_item().unwrap().count, None);
        // A split of more than the stack is a plain pickup too.
        s.run("ClearCursor()").unwrap();
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 99)")
            .unwrap());
        assert_eq!(s.cursor_item().unwrap().count, None);
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
        s.set_bank(Some(crate::script::BankState::default()));
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
        s.set_quest_log(crate::script::QuestLogState {
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
}
