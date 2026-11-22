//! The cursor payload: the client's payload-mode global as a typed enum, and the drag gestures
//! ([`drag`]) that change it. Bags, the paper doll ([`doll`]), the action bars ([`bar`]) and the
//! spellbook all transition it through the setters and the clear ([`set_cursor`],
//! [`clear_cursor_with`]) and lock their items through [`set_item_lock`], so `CURSOR_UPDATE`, the
//! grid events and the lock's signals fire from one place.

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
pub(crate) use pet::toggle_cursor_fork;

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
    /// In `PickupStablePet`'s index space: 0 the current pet, `1..=n` a stabled pet.
    pub slot: u8,
    /// The family icon at the mouse, empty while the pet's creature query is out.
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
    super::container::live_slot(model, item.bag, item.slot)
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

/// Signal `CURSOR_UPDATE` (event 275).
pub(crate) fn raise_cursor_update(model: &mut crate::script::ModelGuard<'_>) {
    model.raise("CURSOR_UPDATE", Vec::new());
}

/// A cursor setter (`0x520770`, `0x520880`, `0x5208f0`, `0x520960`, `0x520a80`, `0x520be0`, `0x520c60`, `0x520d30`, `0x520dc0`): the clear with both flags, then the payload, then its own signals.
pub(crate) fn set_cursor(model: &mut crate::script::ModelGuard<'_>, payload: CursorPayload) {
    set_cursor_with(model, payload, true);
}

/// [`set_cursor`], the item setter's clear unlocking the item held before only with `unlock` (`0x5207b7`).
pub(crate) fn set_cursor_with(
    model: &mut crate::script::ModelGuard<'_>,
    payload: CursorPayload,
    unlock: bool,
) {
    clear_cursor_with(model, unlock, true);
    // What follows the payload's write: the bar's grid, the pet bar's, and the setter's own event.
    let (grid, event) = match &payload {
        // An item shows the grid only when it can be used or worn (`0x520835`-`0x520849`), then signals (`0x520870`).
        CursorPayload::Item(item) => (
            item.bar_placeable.then_some("ACTIONBAR_SHOWGRID"),
            Some("CURSOR_UPDATE"),
        ),
        // Coins signal the money frames alone (`0x5208da`, `0x52094f`).
        CursorPayload::Money(_) => (None, Some("PLAYER_MONEY")),
        CursorPayload::GuildBankMoney(_) => (None, Some("GUILDBANK_UPDATE_MONEY")),
        // A spell, a macro and a set show the grid and signal nothing more (`0x520a38`, `0x520c43`, `0x520e24`).
        CursorPayload::Spell(_) | CursorPayload::Macro(_) | CursorPayload::EquipmentSet(_) => {
            (Some("ACTIONBAR_SHOWGRID"), None)
        }
        // A bar slot's action goes up as what it holds: an item as mode 7, whose setter shows the grid and signals (`0x520d83`, `0x520da7`); a spell, a macro or a set through its own setter.
        CursorPayload::Action(a) => (
            Some("ACTIONBAR_SHOWGRID"),
            (a.kind & 0xf0 == super::action::ACTION_KIND_ITEM).then_some("CURSOR_UPDATE"),
        ),
        CursorPayload::PetAction(_) => (Some("PET_BAR_SHOWGRID"), None),
        // A stabled pet shows no grid and signals nothing (`0x520d08`).
        CursorPayload::StablePet(_) => (None, None),
        // Modes 5 and 11 go through the virtual-item setter with no grid (`0x520d81`).
        CursorPayload::Merchant(_) | CursorPayload::GuildBankItem(_) => {
            (None, Some("CURSOR_UPDATE"))
        }
    };
    model.cursor = Some(payload);
    if let Some(grid) = grid {
        model.raise(grid, Vec::new());
        model.cursor_grid_shown = true;
    }
    if let Some(event) = event {
        model.raise(event, Vec::new());
    }
}

/// The guid of the item at `(bag, slot)`, a worn slot or a bag slot read at the call; 0 for none.
pub(crate) fn guid_at(model: &Model, bag: i64, slot: u32) -> Option<u64> {
    if bag == EQUIPMENT_BAG {
        let worn = model.inv_slot("player", usize::try_from(slot).ok()?)?;
        return (worn.item_id != 0).then_some(worn.guid);
    }
    let held = super::container::live_slot(model, bag, slot)?;
    (held.item_id != 0).then_some(held.guid)
}

/// Whether the item at `(bag, slot)` is locked (`item+0x394` bit 0), beside what its slot's own read reports.
pub(crate) fn item_locked(model: &Model, bag: i64, slot: u32) -> bool {
    model.item_locks.contains(&(bag, slot))
        || guid_at(model, bag, slot).is_some_and(|guid| {
            guid != 0
                && super::read_live(|l| l.item_locked(guid))
                    .flatten()
                    .unwrap_or(false)
        })
}

/// The item lock (`0x513740` -> `0x706fe0`) or unlock (`0x513770` -> `0x707020`) of the item at `(bag, slot)`: its bit moves, then `ITEM_LOCK_CHANGED` and `ITEM_LOCKED` (`0x706ff0`, `0x707001`) or `ITEM_UNLOCKED` (`0x707030`, `0x707041`); no item, or a bit already there, signals nothing (`0x513761`, `0x706feb`, `0x70702b`).
pub(crate) fn set_item_lock(
    model: &mut crate::script::ModelGuard<'_>,
    bag: i64,
    slot: u32,
    locked: bool,
) {
    let guid = guid_at(model, bag, slot);
    let lent = guid
        .filter(|&guid| guid != 0)
        .and_then(|guid| super::write_live(|l| l.set_item_lock(guid, locked)).flatten());
    // A bit kept here from a call no store was lent to counts beside the store's.
    let at = model.item_locks.iter().position(|&p| p == (bag, slot));
    let moved = match (locked, lent) {
        (true, Some(moved)) => moved && at.is_none(),
        (true, None) if at.is_none() && guid.is_some() => {
            model.item_locks.push((bag, slot));
            true
        }
        (true, None) => false,
        (false, lent) => {
            if let Some(at) = at {
                model.item_locks.swap_remove(at);
            }
            guid.is_some() && (at.is_some() || lent == Some(true))
        }
    };
    if moved {
        model.raise("ITEM_LOCK_CHANGED", item_lock_args(bag, slot));
        let edge = if locked {
            "ITEM_LOCKED"
        } else {
            "ITEM_UNLOCKED"
        };
        model.raise(edge, item_lock_args(bag, slot));
    }
    // The bar's usable update follows every lock and unlock, an item or none (`0x51376b`, `0x51379b`).
    update_bar_usable(model);
}

/// The bar's usable update (`0x5aa470`): every slot's bytes recomputed, the walk reading whether sets can be used (`0x5a9fe3`), and `ACTIONBAR_UPDATE_USABLE` if one moved (`0x5aa4cd`).
pub(crate) fn update_bar_usable(model: &mut crate::script::ModelGuard<'_>) {
    let sets_usable = super::equipment_manager::sets_usable_now(model);
    if super::write_live(|l| l.bar_usable_update(sets_usable)).flatten() == Some(true) {
        model.raise("ACTIONBAR_UPDATE_USABLE", Vec::new());
    }
}

impl super::UiScript {
    /// The bar's usable update from an app site holding the VM, in its turn (`0x5aa470`).
    pub fn update_bar_usable(&mut self) {
        update_bar_usable(&mut self.model_mut());
    }
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

/// Signal `DELETE_ITEM_CONFIRM(name, quality)` for an item dropped on the world. The payload stays
/// held: the popup's `DeleteCursorItem` or `ClearCursor` clears it.
fn raise_delete_item_confirm(model: &mut crate::script::ModelGuard<'_>, item: &CursorItem) {
    let name = item_link_name(item.link.as_deref());
    let quality = item.quality.unwrap_or(0);
    model.raise(
        "DELETE_ITEM_CONFIRM",
        vec![ScriptValue::Str(name), ScriptValue::Int(i64::from(quality))],
    );
}

/// An item link's name, from its `|h[Name]|h` segment; empty for no link or another shape.
pub(crate) fn item_link_name(link: Option<&str>) -> String {
    link.and_then(|l| l.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(name, _)| name.to_string())
        .unwrap_or_default()
}

/// The item setter keeping the item held before locked (`0x520770` with its unlock flag clear, as a slot that takes the held item hands back its own): an empty hand-back stops after the clear (`0x5207cf`).
pub(crate) fn swap_cursor_item(
    model: &mut crate::script::ModelGuard<'_>,
    back: Option<CursorItem>,
) {
    match back {
        Some(item) => set_cursor_with(model, CursorPayload::Item(item), false),
        None => clear_cursor_with(model, false, true),
    }
}

/// The cursor clear with both flags set (`0x519280`), as `ClearCursor()` and every setter call it.
pub(crate) fn clear_cursor(model: &mut crate::script::ModelGuard<'_>) {
    clear_cursor_with(model, true, true);
}

/// A payload's mode (`0xbd0748`): a bar slot's action goes up as what it holds, an item as mode 7 (`0x5abf3b`-`0x5abfd6`).
fn mode_of(payload: &CursorPayload) -> u8 {
    match payload {
        CursorPayload::Item(_) => 1,
        CursorPayload::Money(_) => 2,
        CursorPayload::Spell(_) => 3,
        CursorPayload::PetAction(_) => 4,
        CursorPayload::Merchant(_) => 5,
        CursorPayload::Action(a) => match a.kind & 0xf0 {
            super::action::ACTION_KIND_ITEM => 7,
            super::action::ACTION_KIND_MACRO => 8,
            super::action::ACTION_KIND_SPELL => 3,
            _ => 13,
        },
        CursorPayload::Macro(_) => 8,
        CursorPayload::StablePet(_) => 10,
        CursorPayload::GuildBankItem(_) => 11,
        CursorPayload::GuildBankMoney(_) => 12,
        CursorPayload::EquipmentSet(_) => 13,
    }
}

/// The cursor clear (`0x519280`): an armed gift wrap is cancelled; the payload's fields zero by its mode (an item's source unlocking first, only with `unlock`; coins signalling their frame, only with `signal`); a shown grid hides under the mode still standing; then the mode zeroes and `CURSOR_UPDATE` fires, on every call, an empty cursor included.
pub(crate) fn clear_cursor_with(
    model: &mut crate::script::ModelGuard<'_>,
    unlock: bool,
    signal: bool,
) {
    // The wrap's cancel unlocks the paper, then puts the cursor mode back (`0x6cef8d`, `0x6cefa0`).
    if let Some(wrap) = model.pending_wrap.take() {
        set_item_lock(model, wrap.bag, wrap.slot, false);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
    }
    let held = model.cursor.clone();
    let mode = held.as_ref().map_or(0, mode_of);
    match held {
        // The item unlocks while the cursor still holds it (`0x5192c9`), then its fields zero (`0x519301`).
        Some(CursorPayload::Item(item)) if unlock => {
            set_item_lock(model, item.bag, item.slot, false);
        }
        // The coins zero under their mode, then signal (`0x51932c` -> `0x519349`, `0x51935e` -> `0x51937b`).
        Some(CursorPayload::Money(_)) => {
            model.cursor = Some(CursorPayload::Money(CursorMoney { copper: 0 }));
            if signal {
                model.raise("PLAYER_MONEY", Vec::new());
            }
        }
        Some(CursorPayload::GuildBankMoney(_)) => {
            model.cursor = Some(CursorPayload::GuildBankMoney(CursorMoney { copper: 0 }));
            if signal {
                model.raise("GUILDBANK_UPDATE_MONEY", Vec::new());
            }
        }
        // Mode 11 unlocks its slot with its event (`0x519406` -> `0x519411`), then zeroes.
        Some(CursorPayload::GuildBankItem(held)) if unlock => {
            super::guild_bank::unlock_held(model, &held);
        }
        // A spell, a pet action, a macro, a stabled pet and a set only zero (`0x519388`, `0x519393`, `0x5193d0`, `0x5193d8`, `0x51942d`), a vendor row and a bar item with theirs (`0x5193b4`).
        _ => {}
    }
    // The fields are zero and the mode stands (`0x519447`): the grid's listeners read the mode alone.
    model.cursor = None;
    model.cursor_standing = mode;
    // One flag serves both grids; the mode picks which hides (`0x51944f`-`0x51945f`), then the flag drops (`0x519464`).
    if model.cursor_grid_shown {
        let event = if mode == 4 {
            "PET_BAR_HIDEGRID"
        } else {
            "ACTIONBAR_HIDEGRID"
        };
        model.raise(event, Vec::new());
        model.cursor_grid_shown = false;
    }
    // The mode zeroes just before the signal (`0x519470`, `0x519476`).
    model.cursor_standing = 0;
    raise_cursor_update(model);
}

/// The sell fork's clear, the cursor item set to none, shared by `PickupMerchantItem` and the
/// interact ladder's vendor arm: unlike [`clear_cursor`] it leaves the source slot locked, grey
/// until the server removes the item. Any other payload is no sale and stays held.
pub(crate) fn take_cursor_item_for_sale(
    model: &mut crate::script::ModelGuard<'_>,
) -> Option<CursorItem> {
    let Some(CursorPayload::Item(item)) = model.cursor.clone() else {
        return None;
    };
    // The sale's clear leaves the item locked (`0x585421`).
    clear_cursor_with(model, false, true);
    Some(item)
}

/// What a press hit: the kind at `[worldframe+0x2d8]`, stored `0x4fa5c6`, switched on `0x4f78f9`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorldPick {
    /// Kind 0: no world hit (the sky).
    Nothing,
    /// Kinds 1 and 3: terrain, a WMO or a doodad.
    Terrain,
    /// Kind 2, a unit, a player or a corpse: the object leg (`0x5274f0`) has no route to the prompt.
    Object,
    /// Kind 2, a GameObject: slot `+0xa8` is a constant 0 (`0x8e5250`), so it prompts (`0x527792`).
    GameObject,
}

/// A settled left click on the press's pick: an item prompts and stays held, any other drops off.
///
/// Deviation: on `Terrain` the reference keeps a spell, action, macro or pet action on the left
/// button; we clear them, because otherwise a spell has no left-click way off the cursor.
pub(crate) fn world_drop_click(model: &mut crate::script::ModelGuard<'_>, pick: WorldPick) -> bool {
    match (&model.cursor, pick) {
        (
            Some(CursorPayload::Item(item)),
            WorldPick::Terrain | WorldPick::Nothing | WorldPick::GameObject,
        ) => {
            let item = item.clone();
            raise_delete_item_confirm(model, &item);
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

/// `SplitContainerItem(bag, slot, count)` (`0x5d84f0`): an unlocked stack and a count from 1 to its size (`0x5d85b9`-`0x5d85e6`) clear the cursor of whatever it held (`0x5d85ec`) and stop a targeting cursor (`0x5d85f4`), then `count` of the stack goes up through the item setter, the whole stack as a plain pickup (`0x5d8609`), and the slot locks (`0x5d8628`). Returns whether anything was picked up.
pub(crate) fn split_container_item(
    model: &mut crate::script::ModelGuard<'_>,
    bag: i64,
    slot: u32,
    count: i64,
) -> bool {
    let Some(s) = super::container::index_slot(model, bag, slot) else {
        return false;
    };
    if s.item_id == 0
        || super::container::bag_slot_locked(model, bag, slot, &s)
        || count < 1
        || count > i64::from(s.count)
    {
        return false;
    }
    let n = count as u32;
    let item = CursorItem {
        bag,
        slot,
        item_id: s.item_id,
        texture: s.texture.clone(),
        link: s.link.clone(),
        quality: s.quality,
        count: (n < s.count).then_some(n),
        bar_placeable: s.bar_placeable,
        equip_slots: s.equip_slots.clone(),
    };
    clear_cursor(model);
    // A targeting cursor stops after the clear (`0x5d85f4`).
    if super::spellbook::targeting().active {
        model.call(super::ScriptCall::SpellStopTargeting);
    }
    set_cursor(model, CursorPayload::Item(item));
    set_item_lock(model, bag, slot, true);
    true
}

/// `DeleteCursorItem()` (`0x522320`), the delete popup's accept: a held item queues its `(bag, slot, count)` destroy, count 0 for the whole stack, the cursor clears with both flags (`0x522429`) and the item locks again until the answer (`0x52244a`); an item whose template is flagged `0x20` is refused with its message, still held (`0x5223b5`). Anything else is a no-op.
pub(crate) fn delete_cursor_item(
    model: &mut crate::script::ModelGuard<'_>,
) -> Result<(), &'static str> {
    let Some(CursorPayload::Item(item)) = &model.cursor else {
        return Ok(());
    };
    let item = item.clone();
    if super::item_stats::item_record(model, item.item_id).is_some_and(|t| t.flags & 0x20 != 0) {
        return Err("ERR_ITEM_CANT_BE_DESTROYED");
    }
    model
        .container_destroys
        .push((item.bag, item.slot, item.count.unwrap_or(0)));
    clear_cursor(model);
    set_item_lock(model, item.bag, item.slot, true);
    Ok(())
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

    /// A completed left click on the world, on the press's pick ([`world_drop_click`]).
    pub fn world_drop_click(&mut self, pick: WorldPick) -> bool {
        self.model_ref().cursor.is_some() && world_drop_click(&mut self.model_mut(), pick)
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
        if let Some((ty, at)) = super::companion::find(spell_id) {
            return Ok(vec![
                str("companion")?,
                int(at as u32 + 1),
                str(ty.token())?,
            ]);
        }
        let in_book = super::spellbook::live_book()
            .book
            .slots
            .iter()
            .position(|s| s.spell_id == spell_id)
            .map(|i| (i as u32 + 1, "spell"));
        let (index, book) = in_book.or(fallback).unwrap_or((0, "spell"));
        Ok(vec![str("spell")?, int(index), str(book)?])
    };
    let values = match &model.cursor {
        // A mode standing over zeroed fields, as a grid hide's listener finds it: a spell answers its zero id's slot (`0x5153bd`), a macro the first empty slot (`0x51542d` -> `0x564ab0`), the rest find no object or record and answer nothing.
        None if model.cursor_standing == 3 => vec![str("spell")?, int(0), str("spell")?],
        None if model.cursor_standing == 8 => {
            vec![str("macro")?, int(model.macros.first_empty_index())]
        }
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
        Some(CursorPayload::EquipmentSet(e)) => {
            match model.equipment_manager.sets.iter().find(|s| s.id == e.id) {
                Some(set) => vec![str("equipmentset")?, str(&set.name)?],
                None => Vec::new(),
            }
        }
        Some(CursorPayload::Action(a)) => match a.kind & 0xf0 {
            super::action::ACTION_KIND_ITEM => {
                match super::item_stats::item_record(model, a.action) {
                    Some(t) => vec![
                        str("item")?,
                        int(a.action),
                        str(&super::item_stats::item_link(
                            a.action,
                            &t.name,
                            t.quality,
                            crate::script::item_stats::player_level(),
                        ))?,
                    ],
                    None => Vec::new(),
                }
            }
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

    // `BindEnchant()`, `ReplaceEnchant()`: the cast at the parked item, inside the call.
    for (name, answer) in [
        ("BindEnchant", EnchantConfirm::Bind),
        ("ReplaceEnchant", EnchantConfirm::Replace),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                crate::script::model_mut_of(lua)
                    .enchant_confirms
                    .push(answer);
                crate::script::calls::press_item_picks(lua);
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
            // The mode alone (`0x515108`), which stands through the clear's grid hide.
            Ok(super::binding_abi::flag(
                matches!(model.cursor, Some(CursorPayload::Item(_))) || model.cursor_standing == 1,
            ))
        })?,
    )?;
    g.set(
        "CursorHasSpell",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // The mode alone (`0x515143`): a spell off the book or off a bar slot, and the mode standing through the clear's grid hide.
            let mode = model.cursor.as_ref().map_or(model.cursor_standing, mode_of);
            Ok(super::binding_abi::flag(mode == 3))
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
            let refused = delete_cursor_item(&mut crate::script::model_mut_of(lua));
            if let Err(key) = refused {
                crate::script::ui_errors::show_now(lua, key);
            }
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
        crate::script::tick::fired::take();

        let sold = s.take_cursor_item_for_sale().expect("mode 1 is a sale");
        assert_eq!((sold.bag, sold.slot), (0, 1));
        assert!(
            s.cursor_payload().is_none(),
            "the cursor kept the sold item"
        );

        let fired = crate::script::tick::fired::names();
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
        assert!(
            s.eval::<bool>("return CursorHasSpell()").unwrap(),
            "a bar slot's spell goes up through the spell setter, mode 3 (`0x5abf66`, `0x515143`)"
        );
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
        crate::script::tick::fired::take();
        s.run("ClearCursor()").unwrap();
        assert_eq!(crate::script::tick::fired::names(), vec!["CURSOR_UPDATE"]);
    }

    /// Clearing a spell on terrain is the deviation documented on `world_drop_click`.
    #[test]
    fn a_spell_payload_clears_over_terrain_and_nothing_and_stays_over_an_object() {
        use super::WorldPick;
        let spell = CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        });
        let mut s = UiScript::new().unwrap();
        for pick in [WorldPick::Terrain, WorldPick::Nothing] {
            s.set_cursor_for_test(spell.clone());
            assert!(s.world_drop_click(pick), "{pick:?}: the click dismisses it");
            assert!(
                s.cursor_payload().is_none(),
                "{pick:?}: the spell clears silently"
            );
        }
        for pick in [WorldPick::Object, WorldPick::GameObject] {
            s.set_cursor_for_test(spell.clone());
            assert!(!s.world_drop_click(pick), "{pick:?}: nothing drops");
            assert!(s.cursor_payload().is_some(), "{pick:?}: the spell stays");
        }
    }

    /// Terrain, the sky and a GameObject raise the prompt, a unit does not; the item stays held.
    #[test]
    fn an_item_payload_raises_the_prompt_on_every_leg_but_a_unit() {
        use super::WorldPick;
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
        for (pick, prompts) in [
            (WorldPick::Terrain, true),
            (WorldPick::Nothing, true),
            (WorldPick::GameObject, true),
            (WorldPick::Object, false),
        ] {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                heard = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("DELETE_ITEM_CONFIRM")
                f:SetScript("OnEvent", function() heard = heard + 1 end)
                "#,
            )
            .unwrap();
            s.set_cursor_for_test(item.clone());
            assert_eq!(s.world_drop_click(pick), prompts, "{pick:?}");
            s.tick(0.01);
            assert_eq!(
                s.eval::<i64>("return heard").unwrap(),
                i64::from(prompts),
                "{pick:?}"
            );
            assert!(s.cursor_item().is_some(), "{pick:?}: the item stays held");
        }
    }

    #[test]
    fn a_world_click_with_nothing_held_drops_nothing() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.world_drop_click(super::WorldPick::Terrain));
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

    /// A logger on the cursor's and the item lock's events: each entry is the event, whether the
    /// cursor holds an item and whether the backpack's slot 1 reads locked, at the event.
    const STEP_LOGGER: &str = r#"
        Log = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"CURSOR_UPDATE", "ITEM_LOCK_CHANGED", "ITEM_LOCKED", "ITEM_UNLOCKED",
                "ACTIONBAR_SHOWGRID", "ACTIONBAR_HIDEGRID", "PLAYER_MONEY"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function(_, event)
            local _, _, locked = GetContainerItemInfo(0, 1)
            Log[#Log + 1] = event .. (CursorHasItem() and " held" or " empty") .. (locked and " locked" or " free")
        end)
    "#;

    /// A logger on the same events: each entry is the event and its arguments.
    const ARG_LOGGER: &str = r#"
        Log = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"CURSOR_UPDATE", "ITEM_LOCK_CHANGED", "ITEM_LOCKED", "ITEM_UNLOCKED",
                "ACTIONBAR_SHOWGRID", "ACTIONBAR_HIDEGRID"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function(_, ...)
            local t = {...}
            for i = 1, #t do t[i] = tostring(t[i]) end
            Log[#Log + 1] = table.concat(t, " ")
        end)
    "#;

    /// The log since the last read.
    fn step_log(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>("local l = Log Log = {} return l")
            .unwrap()
    }

    /// A verb's events fire inside its call, before the Lua code after it (`0x81b530` runs the listeners in the caller).
    #[test]
    fn a_verbs_event_fires_before_the_code_after_the_call() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.set_money(1_000);
        s.run(STEP_LOGGER).unwrap();
        for call in [
            "ClearCursor()",
            "PickupContainerItem(0, 1)",
            "PickupContainerItem(0, 1)",
            "SplitContainerItem(0, 1, 2)",
            "DeleteCursorItem()",
            "PickupPlayerMoney(10)",
            "DropCursorMoney()",
        ] {
            s.run(&format!("{call} Log[#Log + 1] = 'after'")).unwrap();
            let log = step_log(&s);
            assert!(log.len() > 1, "{call} signalled nothing: {log:?}");
            assert_eq!(log.last().map(String::as_str), Some("after"), "{call}");
        }
    }

    /// The item pickup: the setter's clear signals over an empty cursor (`0x519476`), the setter shows the grid and signals with the item held and not yet locked (`0x520849`, `0x520870`), then the lock's two signals (`0x5d822f` -> `0x706ff0`, `0x707001`).
    #[test]
    fn an_item_pickup_signals_at_each_step_of_the_setter_then_locks() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run(STEP_LOGGER).unwrap();
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            step_log(&s),
            [
                "CURSOR_UPDATE empty free",
                "ACTIONBAR_SHOWGRID held free",
                "CURSOR_UPDATE held free",
                "ITEM_LOCK_CHANGED held locked",
                "ITEM_LOCKED held locked",
            ]
        );
    }

    /// The clear: the item unlocks while the cursor still holds it (`0x5192c9`), the grid hides over zeroed fields with the mode still standing, which is all `CursorHasItem` reads (`0x51945f`, `0x515108`), and `CURSOR_UPDATE` follows the mode's zeroing (`0x519470`, `0x519476`).
    #[test]
    fn the_clear_unlocks_the_item_then_hides_the_grid_then_signals() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run(STEP_LOGGER).unwrap();
        s.run("ClearCursor()").unwrap();
        assert_eq!(
            step_log(&s),
            [
                "ITEM_LOCK_CHANGED held free",
                "ITEM_UNLOCKED held free",
                "ACTIONBAR_HIDEGRID held free",
                "CURSOR_UPDATE empty free",
            ]
        );
    }

    /// The fields zero before the grid hides (`0x519301`, `0x519388`, `0x5193d0`): its listener finds no item behind mode 1 (`0x515243`), a spell's mode over id 0 (`0x5153bd`), and a macro's over the first empty slot (`0x564ab0`).
    #[test]
    fn the_grid_hide_reads_the_mode_over_zeroed_fields() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run(
            r#"local f = CreateFrame("Frame") f:RegisterEvent("ACTIONBAR_HIDEGRID")
               f:SetScript("OnEvent", function()
                   local t = {tostring(CursorHasItem()), tostring(CursorHasSpell()), GetCursorInfo()}
                   for i = 1, #t do t[i] = tostring(t[i]) end
                   Seen = table.concat(t, " ")
               end)"#,
        )
        .unwrap();
        s.run("PickupContainerItem(0, 1) ClearCursor()").unwrap();
        assert_eq!(s.eval::<String>("return Seen").unwrap(), "1 nil");
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        s.model_mut().cursor_grid_shown = true;
        s.run("ClearCursor()").unwrap();
        assert_eq!(
            s.eval::<String>("return Seen").unwrap(),
            "nil 1 spell 0 spell"
        );
        s.set_cursor_for_test(CursorPayload::Macro(super::CursorMacro {
            index: 4,
            texture: None,
        }));
        s.model_mut().cursor_grid_shown = true;
        s.run("ClearCursor()").unwrap();
        assert_eq!(s.eval::<String>("return Seen").unwrap(), "nil nil macro 1");
        assert!(s
            .eval::<bool>("return GetCursorInfo() == nil and not CursorHasSpell()")
            .unwrap());
    }

    /// A bag put-down: the slot's item locks with its two signals (`0x5d82af`), then the swap's clear hides the grid and signals with no unlock (`0x6dfc0d`); both items read locked after.
    #[test]
    fn a_put_down_locks_the_slots_item_and_never_unlocks_the_held_one() {
        let mut s = UiScript::new().unwrap();
        let mut bag = one_item_backpack();
        let other = bag.slots[&1].clone();
        bag.slots.insert(2, other);
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run(ARG_LOGGER).unwrap();
        s.run("PickupContainerItem(0, 2)").unwrap();
        assert_eq!(
            step_log(&s),
            [
                "ITEM_LOCK_CHANGED 0 2",
                "ITEM_LOCKED 0 2",
                "ACTIONBAR_HIDEGRID",
                "CURSOR_UPDATE",
            ]
        );
        assert!(s
            .eval::<bool>("return select(3, GetContainerItemInfo(0, 1)) and select(3, GetContainerItemInfo(0, 2))")
            .unwrap());
    }

    /// The delete: the clear unlocks the item with both flags (`0x522429`), then the item locks again until the answer (`0x52244a` -> `0x706ff0`, `0x707001`).
    #[test]
    fn a_delete_clears_then_locks_the_item_again() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        s.run(ARG_LOGGER).unwrap();
        s.run("DeleteCursorItem()").unwrap();
        assert_eq!(
            step_log(&s),
            [
                "ITEM_LOCK_CHANGED 0 1",
                "ITEM_UNLOCKED 0 1",
                "ACTIONBAR_HIDEGRID",
                "CURSOR_UPDATE",
                "ITEM_LOCK_CHANGED 0 1",
                "ITEM_LOCKED 0 1",
            ]
        );
        assert!(s
            .eval::<bool>("return select(3, GetContainerItemInfo(0, 1)) and not CursorHasItem()")
            .unwrap());
    }

    /// An item already locked signals nothing on a second lock, an unlocked one nothing on an unlock (`0x706feb`, `0x70702b`).
    #[test]
    fn a_lock_signals_only_on_its_edge() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run(ARG_LOGGER).unwrap();
        for (locked, signals) in [(false, 0), (true, 2), (true, 0), (false, 2), (false, 0)] {
            super::set_item_lock(&mut s.model_mut(), 0, 1, locked);
            assert_eq!(step_log(&s).len(), signals, "lock {locked}");
        }
        // No item, no lock (`0x513761`).
        super::set_item_lock(&mut s.model_mut(), 0, 9, true);
        assert!(step_log(&s).is_empty() && !super::item_locked(&s.model_ref(), 0, 9));
    }

    /// The coins' clear zeroes them under their mode before `PLAYER_MONEY` (`0x51932c`, `0x519349`): the listener reads no cursor money and still a money cursor.
    #[test]
    fn dropped_coins_read_zero_under_their_mode_at_player_money() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.run("PickupPlayerMoney(10)").unwrap();
        s.run(
            r#"local f = CreateFrame("Frame") f:RegisterEvent("PLAYER_MONEY")
               f:SetScript("OnEvent", function() Seen = GetCursorMoney() .. (CursorHasMoney() and " money" or " none") end)
               DropCursorMoney()"#,
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return Seen").unwrap(), "0 money");
        assert!(s.eval::<bool>("return CursorHasMoney() == nil").unwrap());
    }

    /// A listener's write inside a verb's event is what the rest of the verb reads: a `CURSOR_UPDATE` listener that drops the item the setter just placed leaves the lock's signals reading an empty cursor, and the item locked by its guid all the same (`0x5d822f`).
    #[test]
    fn a_listeners_write_reaches_the_rest_of_the_verb() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run(STEP_LOGGER).unwrap();
        s.run(
            r#"local f = CreateFrame("Frame") f:RegisterEvent("CURSOR_UPDATE")
               f:SetScript("OnEvent", function() if CursorHasItem() and not Dropped then Dropped = true ClearCursor() end end)
               PickupContainerItem(0, 1)"#,
        )
        .unwrap();
        let log = step_log(&s);
        assert_eq!(
            log.last().map(String::as_str),
            Some("ITEM_LOCKED empty locked"),
            "{log:?}"
        );
        assert!(s.cursor_payload().is_none());
    }
}
