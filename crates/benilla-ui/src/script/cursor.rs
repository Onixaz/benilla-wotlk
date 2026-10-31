//! The cursor payload: the client's payload-mode global as a typed enum, and the drag gestures
//! ([`drag`]) that change it. Bags, the paper doll ([`doll`]), the action bars ([`bar`]) and the
//! spellbook all transition it through [`queue_cursor_update`] and [`queue_lock_changed`], so
//! sounds, `CURSOR_UPDATE`, grid events and lock display fire from one place.

use mlua::{Lua, MultiValue, Value};

use super::{Model, ScriptValue};

mod bag_verbs;
mod bar;
pub(super) mod doll;
mod drag;
pub(crate) mod money;
mod pet;

pub(crate) use bar::use_action_places;
pub(crate) use drag::{
    abandon_drag, arm_drag, maybe_start_drag, take_drag, DragGesture, DragRelease,
};

/// Sentinel bag id for the player's equipped slots, whose `slot` is a 1-based inventory slot id
/// (`GetInventorySlotInfo`'s, HeadSlot = 1). Never a Lua value, and disjoint from every real bag
/// id, -1 the bank and -2 the keyring included. Ammo (slot 0) is outside it.
pub const EQUIPMENT_BAG: i64 = -100;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The payload
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// What the cursor carries, by the client's payload mode: 1 item, 2 money, 3 spell, 4 pet action, 5
/// vendor row, 8 macro, 10 stabled pet; `Action` is the bar-slot pickup.
#[derive(Clone, Debug, PartialEq)]
pub enum CursorPayload {
    Item(CursorItem),
    Spell(CursorSpell),
    Action(CursorAction),
    Macro(CursorMacro),
    /// Mode 13, a saved equipment set (`0x520dc0`), placeable on a bar.
    EquipmentSet(CursorEquipmentSet),
    PetAction(CursorPetAction),
    /// Mode 5, a vendor row. Not an `Item`: `CursorHasItem` is true for modes 1 and 9 only, so all
    /// thirteen stock `CursorHasItem()` gates stay shut while one is held.
    Merchant(CursorMerchantItem),
    /// Mode 10, a pet taken from the stable; its grab is the only writer of mode 10.
    StablePet(CursorStablePet),
    /// Mode 2, coins taken off a money frame.
    Money(CursorMoney),
    /// Mode 11, an item taken from a guild bank slot (`0x520d30`).
    GuildBankItem(CursorGuildBankItem),
    /// Mode 12, coins taken off the guild bank (`PickupGuildBankMoney`, `0x5a4330`).
    GuildBankMoney(CursorMoney),
}

/// A guild bank item held on the cursor, mode 11: its slot stays locked until a drop or a clear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorGuildBankItem {
    /// 0-based tab and slot.
    pub tab: u8,
    pub slot: u8,
    pub item_id: u32,
    pub texture: Option<String>,
    pub link: Option<String>,
    /// `Some(n)` for a split of `n`, `None` for the whole stack.
    pub split: Option<u32>,
}

/// Copper held on the cursor, mode 2 (set by `PickupPlayerMoney`). The purse is not debited; the
/// stock money frame subtracts `GetCursorMoney()` (`MoneyFrame.lua:17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMoney {
    pub copper: u32,
}

/// A vendor row held on the cursor, mode 5, set by `PickupMerchantItem`. A drop on a bag slot is
/// the buy (`CMSG_BUY_ITEM_IN_SLOT`, `0x1a3`); in the reference a drop on the world, ESC or the
/// vendor closing clears it with no packet. No registered binding reports mode 5 to Lua.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMerchantItem {
    /// The row's item entry, which the buy names.
    pub item_id: u32,
    /// The 0-based vendor row (= `trunc(arg) - 1`): a second `PickupMerchantItem` on the same row
    /// drops the grab.
    pub row: u32,
    /// The icon at the mouse, resolved from the display id the reference stores.
    pub texture: Option<String>,
}

/// A pet held on the cursor from the stable window, mode 10, set by the grab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorStablePet {
    /// In `PickupStablePet`'s index space: 0 the summoned pet, 1..=2 a stable slot.
    pub slot: u8,
    /// The family icon at the mouse, never empty: the grab refuses a pet without one.
    pub texture: String,
}

/// An item held on the cursor: where it was picked from, so the next click routes the swap, and
/// the icon at the mouse. The server's field updates do the actual move.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorItem {
    /// The source bag id in the Lua API's numbering, or [`EQUIPMENT_BAG`].
    pub bag: i64,
    /// The 1-based source slot.
    pub slot: u32,
    pub item_id: u32,
    pub texture: Option<String>,
    pub link: Option<String>,
    /// `Some(n)` for a split of `n`, `None` for the whole stack.
    pub count: Option<u32>,
    /// Quality (0..6) at pickup, for a world drop's `DELETE_ITEM_CONFIRM`.
    pub quality: Option<u32>,
    /// The 1-based inventory slot ids the item can be equipped into, empty if none: the paper
    /// doll's place and `CursorCanGoInSlot` read it.
    pub equip_slots: Vec<u8>,
    /// Whether `PlaceAction` accepts it, its only item filter.
    pub bar_placeable: bool,
}

/// How many items the cursor holds. A whole-stack pickup has `count: None`, so the size is read
/// from the source slot, which a pickup locks but does not empty; 1 if that slot is unreadable.
pub(super) fn held_count(model: &super::Model, item: &CursorItem) -> u32 {
    if let Some(n) = item.count {
        return n.max(1);
    }
    model
        .containers
        .get(&item.bag)
        .and_then(|c| c.slots.get(&item.slot))
        .filter(|s| s.item_id == item.item_id)
        .map_or(1, |s| s.count.max(1))
}

/// A spell payload, mode 3, from `PickupSpell` on the player's spellbook.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorSpell {
    pub book_slot: u32,
    pub book_type: String,
    pub spell_id: u32,
    pub texture: Option<String>,
    /// `Attributes & 0x40` (`SPELL_ATTR_PASSIVE`): `PlaceAction` refuses a passive.
    pub passive: bool,
}

/// An action-bar payload from `PickupAction`: the source slot, the action's type byte as
/// `CMSG_SET_ACTION_BUTTON` packs it (spell, macro or item) and its id.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorAction {
    pub src_slot: u32,
    pub kind: u8,
    pub action: u32,
    pub texture: Option<String>,
}

/// A pet-bar payload, mode 4, from `PickupPetAction` or the pet spellbook: the packed action word
/// copied verbatim into the cursor's slot and dropped undecoded. Pet bar only: `PlaceAction`
/// refuses it.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorPetAction {
    /// The 1-based pet bar slot it came from, 0 from the spellbook.
    pub src_slot: u32,
    /// The packed action word, verbatim.
    pub packed: u32,
    /// `Attributes & 0x40` of a spell word, tested at the drop, not the pickup.
    pub passive: bool,
    pub texture: Option<String>,
}

/// A set on the cursor: its id and full icon path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorEquipmentSet {
    pub id: u32,
    pub texture: Option<String>,
}

/// A macro payload from `PickupMacro`, mode 8: the bare macro id, which `PlaceAction` accepts. It
/// has no source slot, so a refused place leaves it held.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorMacro {
    /// The 1-based macro index: 1..=36 account, 37..=54 character.
    pub index: u32,
    pub texture: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The transition events
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Queue `CURSOR_UPDATE`, once per payload transition, plus a grid event when what the bars can
/// take changes. Call it after `model.cursor` holds the new payload; an action hop keeps the grid
/// up rather than hiding and reshowing it.
pub(crate) fn queue_cursor_update(model: &mut Model) {
    model
        .pending_events
        .push(("CURSOR_UPDATE".to_string(), Vec::new()));
    // The action bar's grid shows for any payload but a pet action, which `PlaceAction` refuses, or
    // a vendor row, whose grab fires `ACTIONBAR_SHOWGRID` for mode 7 only. The reference fires it
    // only from the item, spell and macro setters and that mode-7 grab, so coins and a stabled pet
    // show it here and not there. The pet bar's follows the pet action alone.
    let pet_held = matches!(model.cursor, Some(CursorPayload::PetAction(_)));
    let bar_held = model.cursor.is_some()
        && !pet_held
        && !matches!(model.cursor, Some(CursorPayload::Merchant(_)));
    if bar_held != model.cursor_grid_shown {
        model.cursor_grid_shown = bar_held;
        let event = if bar_held {
            "ACTIONBAR_SHOWGRID"
        } else {
            "ACTIONBAR_HIDEGRID"
        };
        model.pending_events.push((event.to_string(), Vec::new()));
    }
    if pet_held != model.pet_grid_shown {
        model.pet_grid_shown = pet_held;
        let event = if pet_held {
            "PET_BAR_SHOWGRID"
        } else {
            "PET_BAR_HIDEGRID"
        };
        model.pending_events.push((event.to_string(), Vec::new()));
    }
}

/// Queue `ITEM_LOCK_CHANGED` for a source slot that a pickup locks or a put-down frees.
pub(crate) fn queue_lock_changed(model: &mut Model, bag: i64, slot: u32) {
    model
        .pending_events
        .push(("ITEM_LOCK_CHANGED".to_string(), item_lock_args(bag, slot)));
}

/// An item event's arguments for the item at `(bag, slot)`, as the item's event signal
/// (`0x60bfe0`) locates it: an inventory slot alone, but a bank-bag slot as `(-1, slot - 38)`;
/// anywhere else the container and its 1-based slot, which are the Lua API's numbering.
pub fn item_lock_args(bag: i64, slot: u32) -> Vec<ScriptValue> {
    let slot = i64::from(slot);
    match bag {
        // Inventory slots 68-74 are the bank's bag slots (0-based 67-73).
        EQUIPMENT_BAG if (68..=74).contains(&slot) => {
            vec![ScriptValue::Int(-1), ScriptValue::Int(slot - 39)]
        }
        EQUIPMENT_BAG => vec![ScriptValue::Int(slot)],
        _ => vec![ScriptValue::Int(bag), ScriptValue::Int(slot)],
    }
}

/// Queue `DELETE_ITEM_CONFIRM(name, quality)` for an item dropped on the world. The payload stays
/// held: the popup's `DeleteCursorItem` or `ClearCursor` clears it.
fn queue_delete_item_confirm(model: &mut Model, item: &CursorItem) {
    let name = item_link_name(item.link.as_deref());
    let quality = item.quality.unwrap_or(0);
    model.pending_events.push((
        "DELETE_ITEM_CONFIRM".to_string(),
        vec![ScriptValue::Str(name), ScriptValue::Int(i64::from(quality))],
    ));
}

/// An item link's name, from its `|h[Name]|h` segment; empty for no link or another shape.
pub(super) fn item_link_name(link: Option<&str>) -> String {
    link.and_then(|l| l.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(name, _)| name.to_string())
        .unwrap_or_default()
}

/// `ClearCursor()`: drops any payload and frees an item's source slot, then signals `CURSOR_UPDATE`
/// from its shared tail on every call, an empty cursor included.
pub(crate) fn clear_cursor(model: &mut Model) {
    // Any clear first cancels an armed gift wrap, whatever its parameters: the wrapping paper
    // unlocks and the cursor mode returns to the base.
    if let Some(wrap) = model.pending_wrap.take() {
        queue_lock_changed(model, wrap.bag, wrap.slot);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
    }
    // Coins go back to nothing, as the purse was never debited; a vendor row clears with no packet.
    let taken = model.cursor.take();
    queue_cursor_update(model);
    match taken {
        Some(CursorPayload::Item(item)) => queue_lock_changed(model, item.bag, item.slot),
        // Mode 11 unlocks its slot, mode 12 repaints the bank's money (`0x519280`).
        Some(CursorPayload::GuildBankItem(held)) => super::guild_bank::unlock_held(model, &held),
        Some(CursorPayload::GuildBankMoney(_)) => model
            .pending_events
            .push(("GUILDBANK_UPDATE_MONEY".to_string(), Vec::new())),
        _ => {}
    }
}

/// The sell fork's clear, the cursor item set to none, shared by `PickupMerchantItem` and the
/// interact ladder's vendor arm: unlike [`clear_cursor`] it leaves the source slot locked, grey
/// until the server removes the item. Any other payload is no sale and stays held.
pub(crate) fn take_cursor_item_for_sale(model: &mut Model) -> Option<CursorItem> {
    let item = match model.cursor.take() {
        Some(CursorPayload::Item(item)) => item,
        other => {
            model.cursor = other;
            return None;
        }
    };
    queue_cursor_update(model);
    Some(item)
}

/// What is under the cursor in the world this frame: the reference's click-time pick state
/// `[this+0x350]`. The app feeds it; tests and captures leave it `Nothing`.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum WorldPick {
    /// State 0: no world hit (the sky).
    #[default]
    Nothing,
    /// State 1: terrain, a WMO or a doodad, but no unit or GameObject.
    Terrain,
    /// State 2: a unit or GameObject.
    Object,
}

/// A left click on the world with a payload held, press and release both over the world and no
/// drag; returns whether it was consumed. Over an `Object` nothing drops and the click selects.
/// Over `Terrain` or `Nothing` an item raises `DELETE_ITEM_CONFIRM(name, quality)` and stays held
/// for the popup to clear; a spell, action, macro or pet action clears. Coins, a vendor row and a
/// stabled pet stay held here, where the reference puts a vendor row down on every leg and clears
/// any non-item payload over `Nothing`.
///
/// Deviation: on `Terrain` the reference keeps a spell, action, macro or pet action on the left
/// button; we clear them, because otherwise a spell has no left-click way off the cursor.
pub(crate) fn world_drop_click(model: &mut Model) -> bool {
    match (&model.cursor, model.world_pick) {
        (Some(CursorPayload::Item(item)), WorldPick::Terrain | WorldPick::Nothing) => {
            let item = item.clone();
            queue_delete_item_confirm(model, &item);
            true
        }
        (
            Some(
                CursorPayload::Spell(_)
                | CursorPayload::Action(_)
                | CursorPayload::Macro(_)
                | CursorPayload::EquipmentSet(_)
                | CursorPayload::PetAction(_),
            ),
            WorldPick::Terrain | WorldPick::Nothing,
        ) => {
            clear_cursor(model);
            true
        }
        _ => false,
    }
}

/// `SplitContainerItem(bag, slot, count)`: picks up `count` of an unlocked stack onto an empty
/// cursor. A count of the whole stack or more is a plain pickup, since vmangos refuses to split a
/// whole stack or more (`Player.cpp:11139`, `:11147`). Returns whether anything was picked up.
pub(crate) fn split_container_item(model: &mut Model, bag: i64, slot: u32, count: i64) -> bool {
    if model.cursor.is_some() || count < 1 {
        return false;
    }
    let Some(s) = model.containers.get(&bag).and_then(|c| c.slots.get(&slot)) else {
        return false;
    };
    if s.item_id == 0 || s.locked {
        return false;
    }
    let n = count.min(i64::from(s.count)) as u32;
    let whole = n >= s.count;
    let item = CursorItem {
        bag,
        slot,
        item_id: s.item_id,
        texture: s.texture.clone(),
        link: s.link.clone(),
        quality: s.quality,
        count: if whole { None } else { Some(n) },
        bar_placeable: s.bar_placeable,
        equip_slots: s.equip_slots.clone(),
    };
    model.cursor = Some(CursorPayload::Item(item));
    queue_cursor_update(model);
    queue_lock_changed(model, bag, slot);
    true
}

/// `DeleteCursorItem()`, the delete popup's accept: a held item queues its `(bag, slot, count)`
/// destroy, count 0 for the whole stack, and clears; anything else is a no-op.
pub(crate) fn delete_cursor_item(model: &mut Model) {
    let Some(CursorPayload::Item(item)) = &model.cursor else {
        return;
    };
    let item = item.clone();
    model.cursor = None;
    model
        .container_destroys
        .push((item.bag, item.slot, item.count.unwrap_or(0)));
    queue_cursor_update(model);
    queue_lock_changed(model, item.bag, item.slot);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// UiScript accessors
// ─────────────────────────────────────────────────────────────────────────────────────────────

impl super::UiScript {
    /// The held item, if the payload is one.
    pub fn cursor_item(&self) -> Option<CursorItem> {
        match self.model_ref().cursor.clone() {
            Some(CursorPayload::Item(item)) => Some(item),
            _ => None,
        }
    }

    /// The cursor payload, any arm.
    pub fn cursor_payload(&self) -> Option<CursorPayload> {
        self.model_ref().cursor.clone()
    }

    /// Feed the world pick under the cursor this frame ([`WorldPick`]).
    pub fn set_world_pick(&mut self, pick: WorldPick) {
        set_field!(self, world_pick, pick);
    }

    /// [`take_cursor_item_for_sale`], for the app's interact ladder.
    pub fn take_cursor_item_for_sale(&mut self) -> Option<CursorItem> {
        take_cursor_item_for_sale(&mut self.model_mut())
    }

    /// `ClearCursor()` from Rust, which the app calls on a right-click over empty world: both
    /// reference legs there call `ClearCursor(1, 1)`.
    pub fn clear_cursor_payload(&mut self) {
        clear_cursor(&mut self.model_mut());
    }

    /// Test-only: set the payload directly, bypassing the Lua bindings.
    #[cfg(test)]
    pub(crate) fn set_cursor_for_test(&mut self, payload: CursorPayload) {
        set_field!(self, cursor, Some(payload));
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The targeting cursor's item pick and the enchant confirms
// ─────────────────────────────────────────────────────────────────────────────────────────────
impl super::UiScript {
    /// Arm or disarm the targeting cursor's item half: while armed, a bag or paper-doll click
    /// queues its `(bag, slot)` as a pick instead of picking up, as the reference's two pickups do.
    pub fn set_item_pick_armed(&mut self, armed: bool) {
        set_field!(self, item_pick_armed, armed);
    }

    /// Push the guids of the units for which the armed word's unit checks all pass.
    /// `SpellCanTargetUnit` resolves its token to a guid, as the reference does, and asks this set,
    /// so a chain such as `"party1target"` reads the unit it ends on.
    pub fn set_spell_targetable_units(&mut self, guids: impl IntoIterator<Item = u64>) {
        set_field!(self, spell_targetable_units, guids.into_iter().collect());
    }

    /// Drain the `(bag, slot)` picks since the last call; a paper-doll pick reports as
    /// [`EQUIPMENT_BAG`] and its 1-based inventory slot.
    pub fn take_item_picks(&mut self) -> Vec<(i64, u32)> {
        drain_field!(self, item_picks)
    }

    /// Drain the enchant-confirm answers since the last call; both answer the pick the app parked.
    pub fn take_enchant_confirms(&mut self) -> Vec<EnchantConfirm> {
        drain_field!(self, enchant_confirms)
    }
}

/// A Yes on an enchant-apply confirm (`StaticPopup.lua:1245`, `:1257`); the app holds the item guid
/// it answers for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnchantConfirm {
    /// `BindEnchant()`: reruns the enchant-apply gate as already confirmed, so it may raise the
    /// replace popup next rather than cast.
    Bind,
    /// `ReplaceEnchant()`: binds the parked guid, skipping the gate. There is no replace packet:
    /// the popup only gates the same cast.
    Replace,
    /// `EndBoundTradeable("spellenchant")`: casts the parked weapon imbue at the parked item,
    /// confirmed (`0x52345b`).
    SpellEnchant,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Lua globals
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `GetCursorInfo()` (`0x515200`), switched on the payload mode:
/// - 1, an item: `"item", itemId, link`;
/// - 2, money: `"money", copper`;
/// - 3, a spell: `"spell", bookIndex, "spell"|"pet"`, the index 1-based in the player's book,
///   then the pet's (`0x53b4e0`), 0 when in neither;
/// - 5, a vendor row: `"merchant", row`, 1-based;
/// - 8, a macro: `"macro", index`;
/// - 11, a guild bank item: `"item", itemId, link`;
/// - 12, guild bank money: `"guildbankmoney", copper`;
/// - 4 (a pet action), 10 (a stabled pet) and an empty cursor: nothing.
///
/// A bar-slot pickup answers as the payload the reference would hold for it: an item by id, whose
/// link needs the template (`0x5153a9`), else nothing; a macro; a spell. A companion spell
/// answers `"companion", index, "CRITTER"|"MOUNT"` (`0x5152c0`), as `GetActionInfo` does. benilla
/// keeps no equipment sets, so mode 13 (`"equipmentset"`) never arises.
fn cursor_info(lua: &Lua, model: &Model) -> mlua::Result<MultiValue> {
    let str = |s: &str| lua.create_string(s).map(Value::String);
    let int = |n: u32| Value::Integer(i64::from(n));
    let spell = |spell_id: u32, fallback: Option<(u32, &str)>| -> mlua::Result<Vec<Value>> {
        if let Some((ty, at)) = model.companions.find(spell_id) {
            return Ok(vec![
                str("companion")?,
                int(at as u32 + 1),
                str(ty.token())?,
            ]);
        }
        let in_book = model
            .spellbook
            .slots
            .iter()
            .position(|s| s.spell_id == spell_id)
            .map(|i| (i as u32 + 1, "spell"));
        let (index, book) = in_book.or(fallback).unwrap_or((0, "spell"));
        Ok(vec![str("spell")?, int(index), str(book)?])
    };
    let values = match &model.cursor {
        None | Some(CursorPayload::PetAction(_) | CursorPayload::StablePet(_)) => Vec::new(),
        Some(CursorPayload::Item(item)) => vec![
            str("item")?,
            int(item.item_id),
            item.link.as_deref().map_or(Ok(Value::Nil), str)?,
        ],
        Some(CursorPayload::Money(m)) => vec![str("money")?, int(m.copper)],
        Some(CursorPayload::GuildBankMoney(m)) => vec![str("guildbankmoney")?, int(m.copper)],
        Some(CursorPayload::GuildBankItem(g)) => vec![
            str("item")?,
            int(g.item_id),
            g.link.as_deref().map_or(Ok(Value::Nil), str)?,
        ],
        Some(CursorPayload::Spell(s)) => {
            let pet = (s.book_type == "pet").then_some((s.book_slot, "pet"));
            spell(s.spell_id, pet)?
        }
        Some(CursorPayload::Merchant(m)) => vec![str("merchant")?, int(m.row + 1)],
        Some(CursorPayload::Macro(m)) => vec![str("macro")?, int(m.index)],
        // Mode 13 (`0x515200`): the set's name, nothing once the set is gone.
        Some(CursorPayload::EquipmentSet(e)) => match model.equipment_sets.get(&e.id) {
            Some(name) => vec![str("equipmentset")?, str(name)?],
            None => Vec::new(),
        },
        Some(CursorPayload::Action(a)) => match a.kind & 0xf0 {
            super::action::ACTION_KIND_ITEM => match model.item_templates.get(&a.action) {
                Some(t) => vec![
                    str("item")?,
                    int(a.action),
                    str(&super::item_stats::item_link(a.action, &t.name, t.quality))?,
                ],
                None => Vec::new(),
            },
            super::action::ACTION_KIND_MACRO => vec![str("macro")?, int(a.action)],
            super::action::ACTION_KIND_SPELL => spell(a.action, None)?,
            _ => Vec::new(),
        },
    };
    Ok(MultiValue::from_vec(values))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetCursorInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            cursor_info(lua, &model)
        })?,
    )?;

    // `BindEnchant()`, `ReplaceEnchant()`: queue an intent for the app; nothing is sent from here.
    for (name, answer) in [
        ("BindEnchant", EnchantConfirm::Bind),
        ("ReplaceEnchant", EnchantConfirm::Replace),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                model.enchant_confirms.push(answer);
                Ok(())
            })?,
        )?;
    }

    g.set(
        "CursorHasItem",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(matches!(model.cursor, Some(CursorPayload::Item(_))))
        })?,
    )?;
    g.set(
        "CursorHasSpell",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(matches!(model.cursor, Some(CursorPayload::Spell(_))))
        })?,
    )?;
    g.set(
        "ClearCursor",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            clear_cursor(&mut model);
            Ok(())
        })?,
    )?;

    g.set(
        "SplitContainerItem",
        lua.create_function(|lua, (bag, slot, count): (i64, u32, i64)| {
            let mut model = crate::script::model_mut_of(lua);
            Ok(split_container_item(&mut model, bag, slot, count))
        })?,
    )?;
    g.set(
        "DeleteCursorItem",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            delete_cursor_item(&mut model);
            Ok(())
        })?,
    )?;

    doll::install(lua)?;
    bag_verbs::install(lua)?;
    bar::install(lua)?;
    pet::install(lua)?;
    money::install(lua)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CursorAction, CursorItem, CursorPayload, CursorSpell};
    use crate::script::UiScript;

    /// The item's event signal (`0x60bfe0`): an inventory slot alone, a bank-bag slot as the bank's
    /// 29-35, and a container slot as `(bag, slot)`.
    #[test]
    fn an_item_event_locates_the_item_as_the_reference_does() {
        use super::{item_lock_args, EQUIPMENT_BAG};
        use crate::script::ScriptValue::Int;
        assert_eq!(
            item_lock_args(EQUIPMENT_BAG, 1),
            vec![Int(1)],
            "the head slot"
        );
        assert_eq!(
            item_lock_args(EQUIPMENT_BAG, 20),
            vec![Int(20)],
            "a bag slot"
        );
        assert_eq!(item_lock_args(EQUIPMENT_BAG, 68), vec![Int(-1), Int(29)]);
        assert_eq!(item_lock_args(EQUIPMENT_BAG, 74), vec![Int(-1), Int(35)]);
        assert_eq!(item_lock_args(0, 3), vec![Int(0), Int(3)], "the backpack");
        assert_eq!(item_lock_args(-1, 28), vec![Int(-1), Int(28)], "the bank");
        assert_eq!(item_lock_args(-2, 5), vec![Int(-2), Int(5)], "the keyring");
        assert_eq!(item_lock_args(7, 12), vec![Int(7), Int(12)], "a bank bag");
    }

    /// A backpack holding a five-stack in slot 1.
    fn one_item_backpack() -> crate::script::container::ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            crate::script::container::ContainerSlot {
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
                quality: Some(3),
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
        crate::script::container::ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// The sell clear skips `ClearCursor`'s unlock and takes only a held item.
    #[test]
    fn the_sell_clear_keeps_the_source_slot_locked() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));

        // Drain the pickup's events so only the sell clear's remain.
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some(), "the pickup did not take");
        s.model_mut().pending_events.clear();

        let sold = s.take_cursor_item_for_sale().expect("mode 1 is a sale");
        assert_eq!((sold.bag, sold.slot), (0, 1));
        assert!(
            s.cursor_payload().is_none(),
            "the cursor kept the sold item"
        );

        let fired: Vec<String> = s
            .model_ref()
            .pending_events
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert!(
            fired.iter().any(|e| e == "CURSOR_UPDATE"),
            "the sell clear must fire CURSOR_UPDATE — got {fired:?}"
        );
        assert!(
            !fired.iter().any(|e| e == "ITEM_LOCK_CHANGED"),
            "the sell clear must NOT un-lock the source slot (that is `ClearCursor`'s job, not \
             the sell clear's) — got {fired:?}"
        );

        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        assert!(
            s.take_cursor_item_for_sale().is_none(),
            "a spell is not a sale"
        );
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))),
            "asking for a sale dropped a non-item payload"
        );
    }

    #[test]
    fn the_has_checks_answer_per_arm() {
        let mut s = UiScript::new().unwrap();

        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: Some("|Hitem:117|h[Tough Jerky]|h".into()),
            count: None,
            quality: Some(3),
            equip_slots: Vec::new(),
        }));
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 12,
            kind: 0,
            action: 133,
            texture: None,
        }));
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());
    }

    #[test]
    fn clear_cursor_widens_to_any_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert!(s.cursor_payload().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_payload().is_none());
    }

    /// The clear's shared tail signals on every call: with nothing held, one `CURSOR_UPDATE` and no
    /// grid event.
    #[test]
    fn clear_cursor_with_nothing_held_still_signals() {
        let s = UiScript::new().unwrap();
        let before = s.model_ref().pending_events.len();
        s.run("ClearCursor()").unwrap();
        let fired: Vec<String> = s.model_ref().pending_events[before..]
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert_eq!(fired, vec!["CURSOR_UPDATE"]);
    }

    /// Clearing a spell on terrain is the deviation documented on `world_drop_click`.
    #[test]
    fn world_drop_terrain_and_nothing_both_clear_a_spell_payload() {
        use super::{world_drop_click, WorldPick};
        let mut s = UiScript::new().unwrap();
        for pick in [WorldPick::Terrain, WorldPick::Nothing] {
            s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
                passive: false,
                book_slot: 3,
                book_type: "spell".into(),
                spell_id: 133,
                texture: None,
            }));
            s.model_mut().world_pick = pick;
            assert!(
                world_drop_click(&mut s.model_mut()),
                "{pick:?}: the dismiss consumes the click"
            );
            assert!(
                s.cursor_payload().is_none(),
                "{pick:?}: the spell clears silently"
            );
        }

        let item = CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        });
        s.set_cursor_for_test(item.clone());
        s.model_mut().world_pick = WorldPick::Terrain;
        assert!(
            world_drop_click(&mut s.model_mut()),
            "terrain: the item pops the popup"
        );
        assert!(s.cursor_item().is_some(), "and stays held");
        s.model_mut().world_pick = WorldPick::Object;
        assert!(
            !world_drop_click(&mut s.model_mut()),
            "object: nothing drops at all"
        );
        assert!(s.cursor_item().is_some());
    }

    #[test]
    fn delete_cursor_item_queues_destroy_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("DeleteCursorItem()").unwrap();
        assert!(s.cursor_item().is_none(), "the payload clears");
        assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
        assert!(s.take_container_destroys().is_empty(), "drained");
    }

    #[test]
    fn delete_cursor_item_is_a_no_op_off_the_item_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
        }));
        s.run("DeleteCursorItem()").unwrap();
        assert!(
            s.cursor_payload().is_some(),
            "non-item payload is untouched"
        );
        assert!(s.take_container_destroys().is_empty());
    }

    #[test]
    fn split_of_the_whole_stack_degrades_to_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack())); // slot 1: a 5-stack
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 5)")
            .unwrap());
        let held = s.cursor_item().expect("picked up");
        assert_eq!(held.count, None, "count>=stack ⇒ whole-stack pickup");
    }

    /// `GetCursorInfo()` (`0x515200`): the payload's type word and its id, per mode.
    #[test]
    fn cursor_info_names_each_payload() {
        let mut s = UiScript::new().unwrap();
        let info = |s: &UiScript| -> Vec<String> {
            let values: mlua::Variadic<mlua::Value> = s.eval("return GetCursorInfo()").unwrap();
            values
                .iter()
                .map(|v| match v {
                    mlua::Value::String(t) => t.to_string_lossy().to_string(),
                    mlua::Value::Integer(n) => n.to_string(),
                    mlua::Value::Number(n) => n.to_string(),
                    other => format!("{other:?}"),
                })
                .collect()
        };
        assert!(info(&s).is_empty(), "an empty cursor answers nothing");

        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
            bar_placeable: true,
        }));
        assert_eq!(
            info(&s),
            ["item", "117", "|cffffffff|Hitem:117|h[Tough Jerky]|h|r"]
        );

        s.set_cursor_for_test(CursorPayload::Money(super::CursorMoney { copper: 250 }));
        assert_eq!(info(&s), ["money", "250"]);

        s.set_cursor_for_test(CursorPayload::Merchant(super::CursorMerchantItem {
            item_id: 159,
            row: 2,
            texture: None,
        }));
        assert_eq!(info(&s), ["merchant", "3"], "the row, 1-based");

        s.set_cursor_for_test(CursorPayload::Macro(super::CursorMacro {
            index: 19,
            texture: None,
        }));
        assert_eq!(info(&s), ["macro", "19"]);

        // A spell outside the pushed book: index 0, the player's book.
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 4,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        assert_eq!(info(&s), ["spell", "0", "spell"]);
        // From the pet's book: its slot and `"pet"`.
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 2,
            book_type: "pet".into(),
            spell_id: 17_253,
            texture: None,
            passive: false,
        }));
        assert_eq!(info(&s), ["spell", "2", "pet"]);

        // A bar macro pickup answers as the macro; an item one waits for its template.
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: crate::script::action::ACTION_KIND_MACRO,
            action: 3,
            texture: None,
        }));
        assert_eq!(info(&s), ["macro", "3"]);
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: crate::script::action::ACTION_KIND_ITEM,
            action: 6_948,
            texture: None,
        }));
        assert!(info(&s).is_empty(), "no template, no answer (`0x5153a9`)");
    }
}
