//! 3.3.5a's equipment manager (`CGEquipmentManager`, `EquipmentManager_Scripts.cpp`'s 17 globals):
//! the saved sets the client owns and edits, the server's list and id replies, the set swap's
//! `CMSG_EQUIPMENT_SET_USE`, and the paper doll's item-location verbs the stock `EquipmentManager.lua`
//! and the slot flyout read (`GetInventoryItemsForSlot` `0x5e95c0`).
//!
//! An item location packs where an item is as the stock `EquipmentManager_UnpackLocation` reads
//! it: `0x100000` the player's own slots, `0x200000` the bags, `0x400000` the bank, the bag in bits
//! 8..15 and the 1-based slot below.

use mlua::{Lua, MultiValue, Table, Value};

use super::container::ContainerSlot;
use super::cursor::queue_cursor_update;
use super::CursorPayload;

/// The slots a set records, every equipment slot.
pub const EQUIPMENT_SET_SLOTS: usize = 19;
/// The most sets a player keeps (`CanCreateSet` `0x5af460`).
pub const MAX_EQUIPMENT_SETS: usize = 10;
/// A set slot's guid for "leave this slot as it is" (`m_ignoredSlots`, `0x5ade50`).
pub const IGNORED_SLOT: u64 = 1;

const LOCATION_PLAYER: i64 = 0x10_0000;
const LOCATION_BAGS: i64 = 0x20_0000;
const LOCATION_BANK: i64 = 0x40_0000;
/// The equipment, backpack and bank vault bands of the player's own slots.
const BACKPACK_FIRST: i64 = 23;
const BANK_VAULT_FIRST: i64 = 39;
/// A worn bag's inventory slot is 19 + bag - 1; a bank bag's 67 + bag - 5.
const BAG_INV_FIRST: u8 = 19;
const BANK_BAG_INV_FIRST: u8 = 67;
/// The player's own bag for `CMSG_EQUIPMENT_SET_USE` (`0x5adb7f`).
const PLAYER_BAG: u8 = 0xff;
/// The `ItemFlags` lock bit `SetContainsLockedItems` tests (`0x5ad9a1`).
const ITEM_FLAG_LOCKED: u32 = 0x1;

/// One saved set (`EQUIPMENT_SET`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EquipmentSetView {
    /// The server's key (`+0x8`), 0 until `SMSG_EQUIPMENT_SET_SAVED` names it.
    pub guid: u64,
    /// The id an action button names (`+0x10`), 0..9.
    pub id: u32,
    pub name: String,
    /// The icon's file name under `Interface\Icons\`.
    pub icon: String,
    /// Each slot's item guid: 0 empty, [`IGNORED_SLOT`] ignored.
    pub items: [u64; EQUIPMENT_SET_SLOTS],
}

/// A send the app makes for the equipment manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EquipmentSetRequest {
    /// `CMSG_EQUIPMENT_SET_SAVE`.
    Save(EquipmentSetView),
    /// `CMSG_EQUIPMENT_SET_DELETE`, by the set's guid.
    Delete(u64),
    /// `CMSG_EQUIPMENT_SET_USE`: per slot the item and its `(bag, slot)`.
    Use([(u64, u8, u8); EQUIPMENT_SET_SLOTS]),
}

/// The client's set state (`CGEquipmentManager`'s statics, `0xc1e908`..`0xc1e95c`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EquipmentManager {
    pub(crate) sets: Vec<EquipmentSetView>,
    /// Ids saved and not yet named by the server (`+0x230`): a save of one sends nothing.
    awaiting_guid: Vec<u32>,
    /// A list has arrived (`m_loaded`); `SaveEquipmentSet` does nothing before it.
    loaded: bool,
    pub(crate) ignored: [bool; EQUIPMENT_SET_SLOTS],
    /// A swap is out (`m_swapPending`), refusing another until its result.
    swap_pending: bool,
    pending_set: u32,
}

impl EquipmentManager {
    fn by_name(&self, name: &str) -> Option<&EquipmentSetView> {
        self.sets.iter().find(|s| s.name == name)
    }

    fn by_id(&self, id: u32) -> Option<&EquipmentSetView> {
        self.sets.iter().find(|s| s.id == id)
    }

    /// The lowest id no set holds (`GetNextFreeSetID` `0x5ae530`), `None` past 9.
    fn next_free_id(&self) -> Option<u32> {
        (0..MAX_EQUIPMENT_SETS as u32).find(|id| self.by_id(*id).is_none())
    }
}

impl super::UiScript {
    /// `SMSG_EQUIPMENT_SET_LIST` (`0x5af490`): once a list has arrived, one with as many sets as
    /// held is skipped whole; any other replaces the sets, ten at most.
    pub fn set_equipment_set_list(&mut self, sets: Vec<EquipmentSetView>) {
        let mut model = self.model_mut();
        let mgr = &mut model.equipment_manager;
        if mgr.loaded && sets.len() == mgr.sets.len() {
            return;
        }
        mgr.sets = sets.into_iter().take(MAX_EQUIPMENT_SETS).collect();
        mgr.awaiting_guid.clear();
        mgr.loaded = true;
        sync_bar_names(&mut model);
    }

    /// `SMSG_EQUIPMENT_SET_SAVED` (`0x5ae760`): a guid names the set; 0 refuses it and the set is
    /// dropped, with no event.
    pub fn equipment_set_saved(&mut self, id: u32, guid: u64) {
        let mut model = self.model_mut();
        let mgr = &mut model.equipment_manager;
        mgr.awaiting_guid.retain(|&i| i != id);
        if guid != 0 {
            if let Some(set) = mgr.sets.iter_mut().find(|s| s.id == id) {
                set.guid = guid;
            }
        } else {
            mgr.sets.retain(|s| s.id != id);
            sync_bar_names(&mut model);
        }
    }

    /// `SMSG_EQUIPMENT_SET_USE_RESULT` (`0x5af710`): the swap ends; `EQUIPMENT_SWAP_FINISHED`
    /// carries whether it worked and the set's name, and a result of 4 adds the bags-full error.
    pub fn equipment_set_use_result(&mut self, result: u8) {
        let name = {
            let mut model = self.model_mut();
            let mgr = &mut model.equipment_manager;
            mgr.swap_pending = false;
            let name = mgr.by_id(mgr.pending_set).map(|s| s.name.clone());
            mgr.pending_set = 0;
            if result == 4 {
                model.ui_errors.push("EQUIPMENT_MANAGER_BAGS_FULL");
            }
            name
        };
        let name = name.map_or(super::ScriptValue::Nil, super::ScriptValue::Str);
        self.fire_event(
            "EQUIPMENT_SWAP_FINISHED",
            vec![super::ScriptValue::Bool(result == 0), name],
        );
    }

    /// The sets as the client holds them, for the action bar's icons.
    pub fn equipment_sets(&self) -> Vec<EquipmentSetView> {
        self.model_ref().equipment_manager.sets.clone()
    }

    /// Take the queued saves, deletes and swaps, in call order.
    pub fn take_equipment_set_requests(&mut self) -> Vec<EquipmentSetRequest> {
        drain_field!(self, equipment_set_requests)
    }
}

/// The action bar's id → name map follows the set list.
fn sync_bar_names(model: &mut super::Model) {
    model.equipment_sets = model
        .equipment_manager
        .sets
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
}

/// Where an item is, the player's slots and bags only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemPlace {
    /// A player slot: 0..18 equipped, 23..38 the backpack, 39..66 the bank vault.
    Player(i64),
    /// A bag's container id (1..4 worn, 5..11 bank) and its 1-based slot.
    Bag(i64, u32),
}

impl ItemPlace {
    /// The stock location number.
    fn location(self) -> i64 {
        match self {
            ItemPlace::Player(i) if i < BACKPACK_FIRST => LOCATION_PLAYER | (i + 1),
            ItemPlace::Player(i) if i < BANK_VAULT_FIRST => {
                LOCATION_PLAYER | LOCATION_BAGS | (i - BACKPACK_FIRST + 1)
            }
            ItemPlace::Player(i) => LOCATION_BANK | (i + 1),
            ItemPlace::Bag(bag, slot) if bag <= 4 => {
                LOCATION_PLAYER | LOCATION_BAGS | (bag << 8) | i64::from(slot)
            }
            ItemPlace::Bag(bag, slot) => {
                LOCATION_BANK | LOCATION_BAGS | ((bag - 4) << 8) | i64::from(slot)
            }
        }
    }

    /// `CMSG_EQUIPMENT_SET_USE`'s `(bag, slot)`: 255 and the player slot, or the bag's inventory
    /// slot and the 0-based slot in it.
    fn wire(self) -> (u8, u8) {
        match self {
            ItemPlace::Player(i) => (PLAYER_BAG, i as u8),
            ItemPlace::Bag(bag, slot) if bag <= 4 => {
                (BAG_INV_FIRST + bag as u8 - 1, (slot - 1) as u8)
            }
            ItemPlace::Bag(bag, slot) => (BANK_BAG_INV_FIRST + bag as u8 - 5, (slot - 1) as u8),
        }
    }
}

/// The equipped item in slot `slot0` (0..18).
fn equipped(model: &super::Model, slot0: usize) -> Option<&super::InvSlotView> {
    model.inventory_slots.get(slot0 + 1)?.as_ref()
}

/// Every held item and its place, in the reference's search order: the equipment, the backpack,
/// the worn bags, then with a banker open the vault and the bank bags (`0x5ae860`).
fn held_items(model: &super::Model) -> Vec<(ItemPlace, u64, &ContainerSlot)> {
    let mut out = Vec::new();
    let mut bag = |cid: i64, place: &dyn Fn(u32) -> ItemPlace| {
        if let Some(c) = model.containers.get(&cid) {
            let mut slots: Vec<_> = c.slots.iter().collect();
            slots.sort_by_key(|(n, _)| **n);
            for (&n, s) in slots {
                if s.guid != 0 {
                    out.push((place(n), s.guid, s));
                }
            }
        }
    };
    bag(0, &|n| ItemPlace::Player(BACKPACK_FIRST + i64::from(n) - 1));
    for cid in 1..=4 {
        bag(cid, &|n| ItemPlace::Bag(cid, n));
    }
    if model.bank.is_some() {
        bag(-1, &|n| {
            ItemPlace::Player(BANK_VAULT_FIRST + i64::from(n) - 1)
        });
        for cid in 5..=11 {
            bag(cid, &|n| ItemPlace::Bag(cid, n));
        }
    }
    out
}

/// Where `guid` is: equipped first, then [`held_items`]' order.
fn find_item(model: &super::Model, guid: u64) -> Option<ItemPlace> {
    (0..EQUIPMENT_SET_SLOTS)
        .find(|&s| equipped(model, s).is_some_and(|v| v.guid == guid))
        .map(|s| ItemPlace::Player(s as i64))
        .or_else(|| {
            held_items(model)
                .into_iter()
                .find(|(_, g, _)| *g == guid)
                .map(|(p, _, _)| p)
        })
}

/// An item's entry and flags by guid, wherever the views hold it.
fn item_by_guid(model: &super::Model, guid: u64) -> Option<(u32, u32)> {
    (0..EQUIPMENT_SET_SLOTS)
        .filter_map(|s| equipped(model, s))
        .find(|v| v.guid == guid)
        .map(|v| (v.item_id, v.flags))
        .or_else(|| {
            model
                .containers
                .values()
                .flat_map(|c| c.slots.values())
                .find(|s| s.guid == guid)
                .map(|s| (s.item_id, s.flags))
        })
}

/// `CanUseEquipmentSets` (`0x5ad6b0`): no swap out and `equipmentManager` on.
fn can_use(model: &super::Model) -> bool {
    !model.equipment_manager.swap_pending
        && model
            .cvars
            .get("equipmentmanager")
            .is_some_and(|c| super::cvars::sstr_to_int(&c.value) != 0)
}

/// `SetContainsLockedItems` (`0x5ad940`): an item of the set carries the lock flag. The buyback
/// exception has no buyback items here to meet.
fn contains_locked(model: &super::Model, set: &EquipmentSetView) -> bool {
    set.items.iter().any(|&g| {
        g > IGNORED_SLOT
            && item_by_guid(model, g).is_some_and(|(_, flags)| flags & ITEM_FLAG_LOCKED != 0)
    })
}

/// `CGEquipmentManager::SaveSet` (`0x5ade50`): the name, the icon and each slot's equipped item,
/// or [`IGNORED_SLOT`]; a set still waiting for its guid takes nothing. Queues the save and
/// fires `EQUIPMENT_SETS_CHANGED`.
fn save_set(lua: &Lua, index: usize, name: &str, icon: &str) {
    {
        let mut model = super::model_mut_of(lua);
        let items: [u64; EQUIPMENT_SET_SLOTS] = std::array::from_fn(|s| {
            if model.equipment_manager.ignored[s] {
                IGNORED_SLOT
            } else {
                equipped(&model, s).map_or(0, |v| v.guid)
            }
        });
        let mgr = &mut model.equipment_manager;
        let Some(set) = mgr.sets.get_mut(index) else {
            return;
        };
        if mgr.awaiting_guid.contains(&set.id) {
            return;
        }
        set.name = name.to_owned();
        set.icon = icon.to_owned();
        set.items = items;
        let saved = set.clone();
        model
            .equipment_set_requests
            .push(EquipmentSetRequest::Save(saved));
        sync_bar_names(&mut model);
    }
    super::tick::fire_event_into(lua, "EQUIPMENT_SETS_CHANGED", Vec::new());
}

/// `SaveEquipmentSet`'s icon (`0x5afa72`): a positive index names a macro icon, a negative one
/// the art of the item worn in slot `-index`, else the question mark.
fn save_icon(model: &mut super::Model, index: i32) -> String {
    if let Some(source) = model.macro_icon_source.take() {
        model.macro_icons = source();
    }
    let icon = if index >= 0 {
        usize::try_from(index - 1)
            .ok()
            .and_then(|i| model.macro_icons.get(i).cloned())
    } else {
        let slot0 = (index.unsigned_abs() - 1) as usize;
        (slot0 < EQUIPMENT_SET_SLOTS)
            .then(|| equipped(model, slot0).and_then(|v| v.icon.clone()))
            .flatten()
    };
    icon.map(|path| icon_file(&path))
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "INV_Misc_QuestionMark".to_owned())
}

/// The file name of an `Interface\Icons\` path, which is what a set stores.
fn icon_file(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_owned()
}

/// `ScrubString` (`0x5af9fc`, 32 characters): the name trimmed and cut to fit, refused if empty.
fn scrub_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(32).collect();
    (!name.is_empty()).then_some(name)
}

/// `CGEquipmentManager::UseEquipmentSet` (`0x5ada20`): each slot's item and where it is; in combat
/// only the weapon slots swap, and the first other slot that would move warns once. A missing
/// item warns once and leaves its slot. Queues the send and fires `EQUIPMENT_SWAP_PENDING`.
fn use_set(lua: &Lua, set: &EquipmentSetView) -> bool {
    let warn_s = |model: &mut super::Model, key: &'static str| {
        model.ui_errors_s.push((key, set.name.clone()));
    };
    {
        let mut model = super::model_mut_of(lua);
        if !can_use(&model) {
            if !model.equipment_manager.swap_pending {
                model.ui_errors.push("EQUIPMENT_MANAGER_IS_DISABLED");
            }
            return false;
        }
        if model.cursor.is_some() {
            model.ui_errors.push("SPELL_FAILED_CANT_DO_THAT_RIGHT_NOW");
            return false;
        }
        let in_combat = model.unit("player").is_some_and(|u| u.in_combat);
        let (mut combat_warned, mut missing_warned) = (false, false);
        let mut wire = [(IGNORED_SLOT, 0u8, 0u8); EQUIPMENT_SET_SLOTS];
        for (slot, entry) in wire.iter_mut().enumerate() {
            let guid = set.items[slot];
            let weapon = (15..=17).contains(&slot);
            if (in_combat && !weapon) || guid == IGNORED_SLOT {
                let worn = equipped(&model, slot).map_or(0, |v| v.guid);
                if in_combat && !combat_warned && guid != IGNORED_SLOT && guid != worn {
                    warn_s(&mut model, "EQUIPMENT_MANAGER_COMBAT_SWAP");
                    combat_warned = true;
                }
            } else if guid == 0 {
                *entry = (0, 0, 0);
            } else {
                match find_item(&model, guid) {
                    Some(place) => {
                        let (bag, bag_slot) = place.wire();
                        *entry = (guid, bag, bag_slot);
                    }
                    None if !missing_warned => {
                        warn_s(&mut model, "EQUIPMENT_MANAGER_MISSING_ITEM");
                        missing_warned = true;
                    }
                    None => {}
                }
            }
        }
        model
            .equipment_set_requests
            .push(EquipmentSetRequest::Use(wire));
        model.equipment_manager.swap_pending = true;
        model.equipment_manager.pending_set = set.id;
    }
    super::tick::fire_event_into(lua, "EQUIPMENT_SWAP_PENDING", Vec::new());
    true
}

/// A slot for the ignore verbs: `lua_tonumber` truncated, 1..19, else the usage error.
fn ignore_slot(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<usize> {
    let n = super::binding_abi::coerced_number(lua, Some(v));
    let slot = (super::binding_abi::ftol(n) as u32).wrapping_sub(1) as usize;
    if slot >= EQUIPMENT_SET_SLOTS {
        return Err(mlua::Error::runtime(usage));
    }
    Ok(slot)
}

/// The set a name argument names; a non-string raises `usage`.
fn set_by_name_arg(
    lua: &Lua,
    v: Value,
    usage: &'static str,
) -> mlua::Result<Option<EquipmentSetView>> {
    let name = super::binding_abi::string_arg(lua, v, usage)?;
    let model = read(lua);
    Ok(model.equipment_manager.by_name(&name).cloned())
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
}

/// The result table: the caller's second argument when it is a table, else a new one.
fn result_table(lua: &Lua, v: Option<&Value>) -> mlua::Result<Table> {
    match v {
        Some(Value::Table(t)) => Ok(t.clone()),
        _ => lua.create_table(),
    }
}

/// `CGEquipmentManager::EquipSet` (`0x5ae660`), an action button's use of a set slot: the stock
/// `EquipmentManager.lua` hears `WEAR_EQUIPMENT_SET` and swaps.
pub(crate) fn wear_set(lua: &Lua, id: u32) {
    let name = read(lua)
        .equipment_manager
        .by_id(id)
        .map(|s| s.name.clone());
    if let Some(name) = name {
        super::tick::fire_event_into(
            lua,
            "WEAR_EQUIPMENT_SET",
            vec![super::ScriptValue::Str(name)],
        );
    }
}

/// Register the equipment manager globals and the item-location verbs.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SaveEquipmentSet(name, iconIndex) (`0x5af9c0`): nothing before the list arrives; an existing
    // name re-saves that set, a new one takes the next free id, and an eleventh raises.
    g.set(
        "SaveEquipmentSet",
        lua.create_function(|lua, (name, icon): (Value, Value)| {
            const USAGE: &str = "Usage: SaveEquipmentSet(\"setName\", iconIndex)";
            if !read(lua).equipment_manager.loaded {
                return Ok(());
            }
            let name = super::binding_abi::string_arg(lua, name, USAGE)?;
            let icon = super::binding_abi::number_arg(lua, icon, USAGE)?;
            let name = scrub_name(&name)
                .ok_or_else(|| mlua::Error::runtime("Invalid string for setName"))?;
            let (index, icon) = {
                let mut model = super::model_mut_of(lua);
                let icon = save_icon(&mut model, icon);
                let mgr = &mut model.equipment_manager;
                match mgr.sets.iter().position(|s| s.name == name) {
                    Some(i) => (i, icon),
                    None => {
                        if mgr.sets.len() >= MAX_EQUIPMENT_SETS {
                            return Err(mlua::Error::runtime(format!(
                                "Too many sets! You can only have {MAX_EQUIPMENT_SETS} sets"
                            )));
                        }
                        let Some(id) = mgr.next_free_id() else {
                            return Ok(());
                        };
                        mgr.sets.push(EquipmentSetView {
                            id,
                            ..Default::default()
                        });
                        (mgr.sets.len() - 1, icon)
                    }
                }
            };
            save_set(lua, index, &name, &icon);
            // A new set waits for its guid (`0x5af8c2`): a second save before the reply sends
            // nothing.
            let mut model = super::model_mut_of(lua);
            let mgr = &mut model.equipment_manager;
            if let Some(set) = mgr.sets.get(index).filter(|s| s.guid == 0) {
                let id = set.id;
                if !mgr.awaiting_guid.contains(&id) {
                    mgr.awaiting_guid.push(id);
                }
            }
            Ok(())
        })?,
    )?;

    // DeleteEquipmentSet(name) (`0x5ae800` → `0x5ae260`): the set and its bar slots go, and
    // `CMSG_EQUIPMENT_SET_DELETE` names its guid.
    g.set(
        "DeleteEquipmentSet",
        lua.create_function(|lua, name: Value| {
            let Some(set) = set_by_name_arg(lua, name, "Usage: DeleteEquipmentSet(\"setName\")")?
            else {
                return Ok(());
            };
            {
                let mut model = super::model_mut_of(lua);
                model.equipment_manager.sets.retain(|s| s.id != set.id);
                model
                    .equipment_set_requests
                    .push(EquipmentSetRequest::Delete(set.guid));
                sync_bar_names(&mut model);
            }
            super::tick::fire_event_into(lua, "EQUIPMENT_SETS_CHANGED", Vec::new());
            Ok(())
        })?,
    )?;

    // RenameEquipmentSet(old, new) (`0x5af910` → `0x5af400`): a taken name does nothing; else the
    // set re-saves under the new name.
    g.set(
        "RenameEquipmentSet",
        lua.create_function(|lua, (old, new): (Value, Value)| {
            const USAGE: &str = "Usage: RenameEquipmentSet(\"oldName\", \"newName\")";
            let old = super::binding_abi::string_arg(lua, old, USAGE)?;
            let new = super::binding_abi::string_arg(lua, new, USAGE)?;
            let new = scrub_name(&new)
                .ok_or_else(|| mlua::Error::runtime("Invalid string for setName"))?;
            let found = {
                let model = read(lua);
                let mgr = &model.equipment_manager;
                if mgr.by_name(&new).is_some() {
                    None
                } else {
                    mgr.sets
                        .iter()
                        .position(|s| s.name == old)
                        .map(|i| (i, mgr.sets[i].icon.clone()))
                }
            };
            if let Some((index, icon)) = found {
                save_set(lua, index, &new, &icon);
            }
            Ok(())
        })?,
    )?;

    // The save's ignored slots (`m_ignoredSlots`, `0xc1e908`): a slot 1..19, else the usage.
    g.set(
        "EquipmentManagerIgnoreSlotForSave",
        lua.create_function(|lua, slot: Value| {
            let slot = ignore_slot(lua, slot, "Usage: EquipmentManagerIgnoreSlotForSave(slot)")?;
            super::model_mut_of(lua).equipment_manager.ignored[slot] = true;
            Ok(())
        })?,
    )?;
    g.set(
        "EquipmentManagerUnignoreSlotForSave",
        lua.create_function(|lua, slot: Value| {
            let slot = ignore_slot(
                lua,
                slot,
                "Usage: EquipmentManagerUnignoreSlotForSave(slot)",
            )?;
            super::model_mut_of(lua).equipment_manager.ignored[slot] = false;
            Ok(())
        })?,
    )?;
    g.set(
        "EquipmentManagerIsSlotIgnoredForSave",
        lua.create_function(|lua, slot: Value| {
            let slot = ignore_slot(
                lua,
                slot,
                "Usage: EquipmentManagerIsSlotIgnoredForSave(slot)",
            )?;
            Ok(read(lua).equipment_manager.ignored[slot])
        })?,
    )?;
    g.set(
        "EquipmentManagerClearIgnoredSlotsForSave",
        lua.create_function(|lua, _: MultiValue| {
            super::model_mut_of(lua).equipment_manager.ignored = [false; EQUIPMENT_SET_SLOTS];
            Ok(())
        })?,
    )?;

    // GetNumEquipmentSets() (`0x5af0d0`).
    g.set(
        "GetNumEquipmentSets",
        lua.create_function(
            |lua, _: MultiValue| Ok(read(lua).equipment_manager.sets.len() as f64),
        )?,
    )?;

    // GetEquipmentSetInfo(index) (`0x5af120`): name, the icon's full path and the id; an index
    // outside 1..10 raises, an empty one answers nothing.
    g.set(
        "GetEquipmentSetInfo",
        lua.create_function(|lua, index: Value| {
            let n = super::binding_abi::coerced_number(lua, Some(index));
            let i = (super::binding_abi::ftol(n) as u32).wrapping_sub(1) as usize;
            if i >= MAX_EQUIPMENT_SETS {
                return Err(mlua::Error::runtime("Usage: GetEquipmentSetInfo(setIndex)"));
            }
            let set = read(lua).equipment_manager.sets.get(i).cloned();
            let Some(set) = set else {
                return Ok(MultiValue::new());
            };
            let mut out = MultiValue::new();
            out.push_back(Value::String(lua.create_string(&set.name)?));
            out.push_back(Value::String(
                lua.create_string(format!("Interface\\Icons\\{}", set.icon))?,
            ));
            out.push_back(Value::Number(f64::from(set.id)));
            Ok(out)
        })?,
    )?;

    // GetEquipmentSetInfoByName(name) (`0x5af1d0`): the bare icon name and the id.
    g.set(
        "GetEquipmentSetInfoByName",
        lua.create_function(|lua, name: Value| {
            let Some(set) =
                set_by_name_arg(lua, name, "Usage: GetEquipmentSetInfoByName(\"setName\")")?
            else {
                return Ok(MultiValue::new());
            };
            let mut out = MultiValue::new();
            out.push_back(Value::String(lua.create_string(&set.icon)?));
            out.push_back(Value::Number(f64::from(set.id)));
            Ok(out)
        })?,
    )?;

    // GetEquipmentSetItemIDs(name [, table]) (`0x5aef90`): per slot 0 empty, 1 ignored, the
    // item's entry, or -1 for an item no longer held.
    g.set(
        "GetEquipmentSetItemIDs",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let name = it.next().unwrap_or(Value::Nil);
            let into = it.next();
            let Some(set) = set_by_name_arg(
                lua,
                name,
                "Usage: GetEquipmentSetItemIDs(\"setName\" [, returnTable])",
            )?
            else {
                return Ok(Value::Nil);
            };
            let t = result_table(lua, into.as_ref())?;
            let model = read(lua);
            for (slot, &guid) in set.items.iter().enumerate() {
                let v = match guid {
                    0 => 0,
                    IGNORED_SLOT => 1,
                    g => item_by_guid(&model, g).map_or(-1, |(entry, _)| i64::from(entry)),
                };
                t.set(slot + 1, v)?;
            }
            Ok(Value::Table(t))
        })?,
    )?;

    // GetEquipmentSetLocations(name [, table]) (`0x5ae860`): per slot 0 empty, 1 ignored, the
    // item's location, or -1 for an item not found; the bank only with a banker open.
    g.set(
        "GetEquipmentSetLocations",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let name = it.next().unwrap_or(Value::Nil);
            let into = it.next();
            const USAGE: &str = "Usage: GetEquipmentSetLocations(\"setName\" [, returnTable])";
            if !matches!(into, None | Some(Value::Nil) | Some(Value::Table(_))) {
                return Err(mlua::Error::runtime(USAGE));
            }
            let Some(set) = set_by_name_arg(lua, name, USAGE)? else {
                return Ok(Value::Nil);
            };
            let t = result_table(lua, into.as_ref())?;
            let model = read(lua);
            for (slot, &guid) in set.items.iter().enumerate() {
                let v = match guid {
                    0 => 0,
                    IGNORED_SLOT => 1,
                    g => find_item(&model, g).map_or(-1, ItemPlace::location),
                };
                t.set(slot + 1, v)?;
            }
            Ok(Value::Table(t))
        })?,
    )?;

    // EquipmentSetContainsLockedItems(name) (`0x5af320`): true, or nothing.
    g.set(
        "EquipmentSetContainsLockedItems",
        lua.create_function(|lua, name: Value| {
            let set = set_by_name_arg(
                lua,
                name,
                "Usage: EquipmentSetContainsLockedItems(\"setName\")",
            )?;
            let locked = set.is_some_and(|s| contains_locked(&read(lua), &s));
            Ok(if locked {
                Value::Boolean(true)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // PickupEquipmentSetByName(name) (`0x5af250`), PickupEquipmentSet(index) (`0x5af2c0`): the set
    // on the cursor (`SetCursorEquipmentSet` `0x520dc0`), its full icon path as the art.
    g.set(
        "PickupEquipmentSetByName",
        lua.create_function(|lua, name: Value| {
            let set = set_by_name_arg(lua, name, "Usage: PickupEquipmentSetByName(\"setName\")")?;
            if let Some(set) = set {
                pickup(lua, &set);
            }
            Ok(())
        })?,
    )?;
    g.set(
        "PickupEquipmentSet",
        lua.create_function(|lua, index: Value| {
            let i =
                super::binding_abi::number_arg(lua, index, "Usage: PickupEquipmentSet(setIndex)")?;
            let set = usize::try_from(i - 1)
                .ok()
                .and_then(|i| read(lua).equipment_manager.sets.get(i).cloned());
            if let Some(set) = set {
                pickup(lua, &set);
            }
            Ok(())
        })?,
    )?;

    // UseEquipmentSet(name) (`0x5af380`): true when the swap went out, nil when refused.
    g.set(
        "UseEquipmentSet",
        lua.create_function(|lua, name: Value| {
            let Some(set) = set_by_name_arg(lua, name, "Usage: UseEquipmentSet(\"setName\")")?
            else {
                return Ok(MultiValue::new());
            };
            let mut out = MultiValue::new();
            out.push_back(if use_set(lua, &set) {
                Value::Boolean(true)
            } else {
                Value::Nil
            });
            Ok(out)
        })?,
    )?;

    // CanUseEquipmentSets() (`0x5ad880`): 1 with no swap out and `equipmentManager` on, else 0.
    g.set(
        "CanUseEquipmentSets",
        lua.create_function(|lua, _: MultiValue| Ok(if can_use(&read(lua)) { 1.0 } else { 0.0 }))?,
    )?;

    install_item_verbs(lua)
}

/// `SetCursorEquipmentSet` (`0x520dc0`).
fn pickup(lua: &Lua, set: &EquipmentSetView) {
    let mut model = super::model_mut_of(lua);
    model.cursor = Some(CursorPayload::EquipmentSet(
        super::cursor::CursorEquipmentSet {
            id: set.id,
            texture: (!set.icon.is_empty()).then(|| format!("Interface\\Icons\\{}", set.icon)),
        },
    ));
    queue_cursor_update(&mut model);
}

/// The paper doll's item verbs 3.3.5a added: ids, gems, durability and the flyout's list.
fn install_item_verbs(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetInventoryItemID(unit, slot) (`0x5ea3e0`): the item's entry, the ammo's for slot 0, an
    // inspected unit's visible item; nothing for an empty slot.
    g.set(
        "GetInventoryItemID",
        lua.create_function(|lua, (token, slot): (Option<String>, i64)| {
            let id = super::char_stats::player_inv_slot(lua, &token, slot)
                .map(|v| v.item_id)
                .filter(|&id| id != 0);
            Ok(id.map(f64::from))
        })?,
    )?;

    // GetInventoryItemGems(slot) (`0x5ea4f0`): the gem item of each socket enchant (slots 2..4),
    // nil per empty socket; nothing for an empty slot.
    g.set(
        "GetInventoryItemGems",
        lua.create_function(|lua, slot: Value| {
            let slot =
                super::binding_abi::number_arg(lua, slot, "Usage: GetInventoryItemGems(slot)")?;
            let player = Some("player".to_string());
            let Some(v) = super::char_stats::player_inv_slot(lua, &player, i64::from(slot)) else {
                return Ok(MultiValue::new());
            };
            gems(v.gems)
        })?,
    )?;

    // GetContainerItemID(bag, slot) (`0x5d7d00`): the slot's item entry, nothing if empty.
    g.set(
        "GetContainerItemID",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            Ok(container_slot(lua, bag, slot, |s| f64::from(s.item_id)).filter(|&id| id != 0.0))
        })?,
    )?;

    // GetContainerItemDurability(bag, slot) (`0x5d7ef0`): current and max, nothing for an item
    // with no durability or a wrapped one (`ITEM_FIELD_FLAGS & 0x8`).
    g.set(
        "GetContainerItemDurability",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let durability = container_slot(lua, bag, slot, |s| {
                s.durability.filter(|_| s.flags & 0x8 == 0)
            });
            Ok(match durability.flatten() {
                Some((cur, max)) if max > 0 => (Some(f64::from(cur)), Some(f64::from(max))),
                _ => (None, None),
            })
        })?,
    )?;

    // GetContainerItemGems(bag, slot) (`0x5d9300`): as `GetInventoryItemGems`.
    g.set(
        "GetContainerItemGems",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            match container_slot(lua, bag, slot, |s| s.gems) {
                Some(g) => gems(g),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // GetInventoryItemsForSlot(slot [, table]) (`0x5e95c0`): every held item that fits the
    // 1-based slot, location → entry: the equipment, the backpack and bags, and with a banker the
    // bank. A slot outside 1..19 answers nothing; a slot name reads `PaperDollItemFrame.dbc`.
    g.set(
        "GetInventoryItemsForSlot",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let slot = it.next().unwrap_or(Value::Nil);
            let into = it.next();
            let slot0 = match &slot {
                Value::Integer(_) | Value::Number(_) => {
                    let n = super::binding_abi::coerced_number(lua, Some(slot.clone()));
                    (n - 1.0) as i64
                }
                Value::String(s) => {
                    let name = s.to_string_lossy();
                    match super::char_stats::slot_id_by_name(&name).filter(|&id| id > 0) {
                        Some(id) => id - 1,
                        None => {
                            return Err(mlua::Error::runtime(
                                "Usage: GetInventoryItemsForSlot(slot [, returnTable])",
                            ))
                        }
                    }
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "Usage: GetInventoryItemsForSlot(slot [, returnTable])",
                    ))
                }
            };
            if !(0..EQUIPMENT_SET_SLOTS as i64).contains(&slot0) {
                return Ok(Value::Nil);
            }
            let t = result_table(lua, into.as_ref())?;
            let fits = slot0 as u8 + 1;
            let model = read(lua);
            let usable = |item_id: u32| super::item_stats::item_usable_by_id(&model, item_id);
            for s in 0..EQUIPMENT_SET_SLOTS {
                if let Some(v) = equipped(&model, s) {
                    if v.item_id != 0 && v.equip_slots.contains(&fits) && usable(v.item_id) {
                        t.set(ItemPlace::Player(s as i64).location(), v.item_id)?;
                    }
                }
            }
            for (place, _, c) in held_items(&model) {
                if c.item_id != 0 && c.equip_slots.contains(&fits) && usable(c.item_id) {
                    t.set(place.location(), c.item_id)?;
                }
            }
            Ok(Value::Table(t))
        })?,
    )?;
    Ok(())
}

/// A container slot's field, `None` for an empty slot or unknown bag.
fn container_slot<T>(lua: &Lua, bag: i64, slot: u32, f: impl Fn(&ContainerSlot) -> T) -> Option<T> {
    let model = read(lua);
    model
        .containers
        .get(&bag)
        .and_then(|c| c.slots.get(&slot))
        .filter(|s| s.item_id != 0 || s.guid != 0)
        .map(f)
}

/// The three gem returns, nil per empty socket.
fn gems(gems: [u32; 3]) -> mlua::Result<MultiValue> {
    Ok(gems
        .iter()
        .map(|&g| {
            if g != 0 {
                Value::Number(f64::from(g))
            } else {
                Value::Nil
            }
        })
        .collect())
}

/// `GameTooltip:SetEquipmentSet(name)` (`0x6264d0` → `0x622dd0`): the set's name and its item
/// count, then how many are equipped, in the bags, ignored, and a red line per missing slot, last
/// slot first.
pub(crate) fn install_tooltip_method(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetEquipmentSet",
        lua.create_function(|lua, (this, name): (Table, Value)| {
            let h = super::object::frame_handle_of(lua, &this)?;
            let Some(name) = super::binding_abi::optional_string(lua, &name) else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetEquipmentSet(\"setName\")",
                    super::tooltip::display_name(lua, h)
                )));
            };
            let set = read(lua).equipment_manager.by_name(&name).cloned();
            let Some(set) = set else {
                return Ok(());
            };
            super::tooltip::clear_content(&mut super::model_mut_of(lua), h);
            super::tooltip::fire_cleared(lua, h);
            let (equipped_n, bagged, ignored, missing) = {
                let model = read(lua);
                let held = held_items(&model);
                let (mut e, mut b, mut i, mut missing) = (0, 0, 0, Vec::new());
                for (slot, &guid) in set.items.iter().enumerate() {
                    match guid {
                        0 => {}
                        IGNORED_SLOT => i += 1,
                        g if (0..EQUIPMENT_SET_SLOTS)
                            .any(|s| equipped(&model, s).is_some_and(|v| v.guid == g)) =>
                        {
                            e += 1
                        }
                        g if held.iter().any(|(_, hg, _)| *hg == g) => b += 1,
                        _ => missing.push(slot),
                    }
                }
                (e, b, i, missing)
            };
            let text = |key: &str, n: usize| {
                crate::strings::global(lua, key)
                    .map(|t| t.replacen("%d", &n.to_string(), 1))
                    .unwrap_or_default()
            };
            const GOLD: [f32; 4] = super::tooltip::DEFAULT_TEXT_GOLD;
            const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
            const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
            const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
            let count = text(
                "ITEMS_VARIABLE_QUANTITY",
                equipped_n + bagged + missing.len(),
            );
            let right = (!count.is_empty()).then_some((count, GOLD));
            super::tooltip::append_line(lua, &this, (set.name.clone(), WHITE), right, false)?;
            if equipped_n > 0 {
                super::tooltip::append_line(
                    lua,
                    &this,
                    (text("ITEMS_EQUIPPED", equipped_n), GOLD),
                    None,
                    false,
                )?;
            }
            if bagged > 0 {
                super::tooltip::append_line(
                    lua,
                    &this,
                    (text("ITEMS_IN_INVENTORY", bagged), WHITE),
                    None,
                    false,
                )?;
            }
            if ignored > 0 {
                super::tooltip::append_line(
                    lua,
                    &this,
                    (text("ITEM_SLOTS_IGNORED", ignored), GRAY),
                    None,
                    false,
                )?;
            }
            for &slot in missing.iter().rev() {
                let slot_name = super::char_stats::slot_token(slot)
                    .and_then(|k| crate::strings::global(lua, k))
                    .unwrap_or_default();
                let line = crate::strings::global(lua, "ITEM_MISSING")
                    .map(|t| t.replacen("%s", &slot_name, 1))
                    .unwrap_or_default();
                super::tooltip::append_line(lua, &this, (line, RED), None, false)?;
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{ContainerState, InvSlotView, UiScript, UnitState};

    const HELM: u64 = 0x4000_0000_0000_0010;
    const SWORD: u64 = 0x4000_0000_0000_0011;
    const SPARE: u64 = 0x4000_0000_0000_0012;
    const BANKED: u64 = 0x4000_0000_0000_0013;
    const GONE: u64 = 0x4000_0000_0000_0099;

    fn slot(guid: u64, item_id: u32, fits: u8) -> InvSlotView {
        InvSlotView {
            guid,
            item_id,
            icon: Some(format!("Interface\\Icons\\Item{item_id}")),
            equip_slots: vec![fits],
            ..Default::default()
        }
    }

    fn bag_slot(guid: u64, item_id: u32, fits: u8) -> ContainerSlot {
        ContainerSlot {
            guid,
            item_id,
            equip_slots: vec![fits],
            gems: [40001, 0, 40003],
            ..Default::default()
        }
    }

    /// A player wearing a helm (slot 1) and a sword (slot 16), a spare sword in bag 1 slot 3 and
    /// one in the bank vault's slot 2, with the manager on and the list loaded empty.
    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("equipmentManager", "1")]);
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                is_player: true,
                name: Some("Elza".into()),
                ..Default::default()
            }),
        );
        let mut inv: crate::script::InventorySlots = Default::default();
        inv[1] = Some(slot(HELM, 100, 1));
        inv[16] = Some(slot(SWORD, 200, 16));
        s.set_inventory_slots(inv);
        let bag = |guid, item, fits, n: u32| ContainerState {
            num_slots: 16,
            slots: [(n, bag_slot(guid, item, fits))].into(),
            ..Default::default()
        };
        s.set_container(1, Some(bag(SPARE, 201, 16, 3)));
        s.set_container(-1, Some(bag(BANKED, 202, 16, 2)));
        s.set_equipment_set_list(Vec::new());
        s
    }

    fn set(id: u32, name: &str, items: [u64; EQUIPMENT_SET_SLOTS]) -> EquipmentSetView {
        EquipmentSetView {
            guid: 0x100 + u64::from(id),
            id,
            name: name.into(),
            icon: "Ability_Defend".into(),
            items,
        }
    }

    /// `SaveEquipmentSet` builds the set from the worn items, the ignored slots as 1, takes the
    /// next free id, queues the save and fires `EQUIPMENT_SETS_CHANGED`; the reply's guid names it,
    /// and a re-save before the reply sends nothing.
    #[test]
    fn a_save_takes_the_worn_items_and_waits_for_its_guid() {
        let mut s = script();
        s.run(
            r#"
            changes = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("EQUIPMENT_SETS_CHANGED")
            f:SetScript("OnEvent", function() changes = changes + 1 end)
            EquipmentManagerIgnoreSlotForSave(2)
            SaveEquipmentSet("Tank", -1)
            "#,
        )
        .unwrap();
        assert_eq!(s.eval::<i64>("return changes").unwrap(), 1);
        let requests = s.take_equipment_set_requests();
        let [EquipmentSetRequest::Save(saved)] = requests.as_slice() else {
            panic!("{requests:?}");
        };
        assert_eq!(
            (saved.id, saved.guid, saved.icon.as_str()),
            (0, 0, "Item100")
        );
        assert_eq!(saved.items[0], HELM);
        assert_eq!(saved.items[1], IGNORED_SLOT);
        assert_eq!(saved.items[15], SWORD);
        assert_eq!(saved.items[2], 0);
        s.run(r#"SaveEquipmentSet("Tank", 1)"#).unwrap();
        assert!(
            s.take_equipment_set_requests().is_empty(),
            "awaiting its guid"
        );
        s.equipment_set_saved(0, 0x77);
        s.run(r#"SaveEquipmentSet("Tank", -1)"#).unwrap();
        let requests = s.take_equipment_set_requests();
        assert!(matches!(
            requests.as_slice(),
            [EquipmentSetRequest::Save(EquipmentSetView {
                guid: 0x77,
                ..
            })]
        ));
        let (name, icon, id): (String, String, i64) =
            s.eval("return GetEquipmentSetInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), icon.as_str(), id),
            ("Tank", "Interface\\Icons\\Item100", 0)
        );
        assert!(s.run("GetEquipmentSetInfo(11)").is_err());
        assert_eq!(
            s.eval::<i64>("return select('#', GetEquipmentSetInfo(2))")
                .unwrap(),
            0
        );
    }

    /// The server's list: once loaded, one with as many sets as held is skipped; a refused save
    /// drops the set.
    #[test]
    fn the_list_skips_a_same_sized_resend_and_a_refused_save_drops_the_set() {
        let mut s = script();
        s.set_equipment_set_list(vec![set(3, "Pvp", [0; EQUIPMENT_SET_SLOTS])]);
        assert_eq!(s.eval::<i64>("return GetNumEquipmentSets()").unwrap(), 1);
        s.set_equipment_set_list(vec![set(4, "Other", [0; EQUIPMENT_SET_SLOTS])]);
        assert_eq!(s.equipment_sets()[0].name, "Pvp", "same size: skipped");
        s.equipment_set_saved(3, 0);
        assert_eq!(s.eval::<i64>("return GetNumEquipmentSets()").unwrap(), 0);
    }

    /// The location and item-id reads: worn, in a bag, the vault only with a banker, ignored,
    /// empty, and gone.
    #[test]
    fn locations_and_item_ids_follow_the_items() {
        let mut s = script();
        let mut items = [0; EQUIPMENT_SET_SLOTS];
        items[0] = HELM;
        items[1] = IGNORED_SLOT;
        items[15] = SPARE;
        items[16] = BANKED;
        items[17] = GONE;
        s.set_equipment_set_list(vec![set(0, "Mix", items)]);
        let loc = |s: &UiScript, i: u32| -> i64 {
            s.eval(&format!("return GetEquipmentSetLocations(\"Mix\")[{i}]"))
                .unwrap()
        };
        assert_eq!(loc(&s, 1), 0x100001);
        assert_eq!(loc(&s, 2), 1);
        assert_eq!(loc(&s, 3), 0);
        assert_eq!(loc(&s, 16), 0x300000 | (1 << 8) | 3);
        assert_eq!(loc(&s, 17), -1, "the vault needs a banker");
        assert_eq!(loc(&s, 18), -1);
        s.set_bank(Some(Default::default()));
        assert_eq!(loc(&s, 17), 0x400000 | 41, "vault slot 2 is player slot 40");
        let id = |s: &UiScript, i: u32| -> i64 {
            s.eval(&format!("return GetEquipmentSetItemIDs(\"Mix\")[{i}]"))
                .unwrap()
        };
        assert_eq!(
            (id(&s, 1), id(&s, 2), id(&s, 3), id(&s, 16)),
            (100, 1, 0, 201)
        );
        assert_eq!(id(&s, 18), -1);
    }

    /// `UseEquipmentSet`: each slot's item and place on the wire, an empty slot 0, a missing item
    /// warned once and left as 1; the swap is pending until its result fires
    /// `EQUIPMENT_SWAP_FINISHED`, and a second use meanwhile is refused.
    #[test]
    fn a_swap_sends_each_slots_place_and_waits_for_its_result() {
        let mut s = script();
        let mut items = [IGNORED_SLOT; EQUIPMENT_SET_SLOTS];
        items[0] = HELM;
        items[2] = 0;
        items[15] = SPARE;
        items[16] = GONE;
        s.set_equipment_set_list(vec![set(2, "Dps", items)]);
        s.run(
            r#"
            seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("EQUIPMENT_SWAP_PENDING")
            f:RegisterEvent("EQUIPMENT_SWAP_FINISHED")
            f:SetScript("OnEvent", function(_, e, ok, name)
                seen[#seen + 1] = e .. ":" .. tostring(ok) .. ":" .. tostring(name)
            end)
            first = UseEquipmentSet("Dps")
            second = UseEquipmentSet("Dps")
            "#,
        )
        .unwrap();
        assert!(s
            .eval::<bool>("return first == true and second == nil")
            .unwrap());
        let requests = s.take_equipment_set_requests();
        let [EquipmentSetRequest::Use(wire)] = requests.as_slice() else {
            panic!("{requests:?}");
        };
        assert_eq!(wire[0], (HELM, 0xff, 0));
        assert_eq!(wire[1], (IGNORED_SLOT, 0, 0));
        assert_eq!(wire[2], (0, 0, 0));
        assert_eq!(
            wire[15],
            (SPARE, 19, 2),
            "bag 1 is inventory slot 19, slot 3 is 2"
        );
        assert_eq!(wire[16], (IGNORED_SLOT, 0, 0), "missing: left as it is");
        assert_eq!(
            s.take_ui_errors_s(),
            vec![("EQUIPMENT_MANAGER_MISSING_ITEM", "Dps".to_string())]
        );
        s.equipment_set_use_result(0);
        assert_eq!(
            s.eval::<String>("return table.concat(seen, \",\")")
                .unwrap(),
            "EQUIPMENT_SWAP_PENDING:nil:nil,EQUIPMENT_SWAP_FINISHED:true:Dps"
        );
        assert_eq!(s.eval::<i64>("return CanUseEquipmentSets()").unwrap(), 1);
        s.set_cvar_host("equipmentManager", "0");
        s.run(r#"UseEquipmentSet("Dps")"#).unwrap();
        assert_eq!(s.take_ui_errors(), vec!["EQUIPMENT_MANAGER_IS_DISABLED"]);
    }

    /// Delete and rename edit the list, and a set on an action slot wears through
    /// `WEAR_EQUIPMENT_SET`.
    #[test]
    fn delete_rename_and_the_bar_slot() {
        let mut s = script();
        s.set_equipment_set_list(vec![
            set(0, "A", [0; EQUIPMENT_SET_SLOTS]),
            set(1, "B", [0; EQUIPMENT_SET_SLOTS]),
        ]);
        s.run(r#"RenameEquipmentSet("A", "B") RenameEquipmentSet("A", "C")"#)
            .unwrap();
        let names: Vec<String> = s.equipment_sets().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["C", "B"], "a taken name does nothing");
        let _ = s.take_equipment_set_requests();
        s.run(r#"DeleteEquipmentSet("B")"#).unwrap();
        assert_eq!(
            s.take_equipment_set_requests(),
            vec![EquipmentSetRequest::Delete(0x101)]
        );
        s.run(
            r#"
            worn = nil
            local f = CreateFrame("Frame")
            f:RegisterEvent("WEAR_EQUIPMENT_SET")
            f:SetScript("OnEvent", function(_, _, name) worn = name end)
            PickupEquipmentSetByName("C")
            local kind, name = GetCursorInfo()
            assert(kind == "equipmentset" and name == "C", tostring(kind))
            PlaceAction(5)
            UseAction(5)
            "#,
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return worn").unwrap(), "C");
    }

    /// The flyout's list: every held item fitting the slot, by location; the ids and gems verbs
    /// read the slots.
    #[test]
    fn the_flyout_lists_what_fits_and_the_item_verbs_read_the_slots() {
        let s = script();
        s.run(
            r#"
            local t = GetInventoryItemsForSlot(16)
            local n = 0
            for loc, id in pairs(t) do n = n + 1 end
            assert(n == 2, n)
            assert(t[0x100010] == 200)
            assert(t[0x300103] == 201)
            assert(GetInventoryItemsForSlot("HeadSlot")[0x100001] == 100)
            assert(GetInventoryItemsForSlot(25) == nil)
            assert(GetInventoryItemID("player", 1) == 100)
            assert(GetInventoryItemID("player", 2) == nil)
            assert(GetContainerItemID(1, 3) == 201)
            assert(GetContainerItemID(1, 4) == nil)
            local a, b, c = GetContainerItemGems(1, 3)
            assert(a == 40001 and b == nil and c == 40003)
            "#,
        )
        .unwrap();
        assert!(s.run("GetInventoryItemsForSlot({})").is_err());
    }

    /// `GameTooltip:SetEquipmentSet`: the name and count, then equipped, in bags, ignored and a
    /// red line per missing slot.
    #[test]
    fn the_set_tooltip_counts_where_the_items_are() {
        let mut s = script();
        let mut items = [0; EQUIPMENT_SET_SLOTS];
        items[0] = HELM;
        items[1] = IGNORED_SLOT;
        items[15] = SPARE;
        items[16] = GONE;
        s.set_equipment_set_list(vec![set(0, "Mix", items)]);
        s.run(
            r#"
            ITEMS_VARIABLE_QUANTITY = "%d items"
            ITEMS_EQUIPPED = "%d equipped"
            ITEMS_IN_INVENTORY = "%d in bags"
            ITEM_SLOTS_IGNORED = "%d ignored"
            ITEM_MISSING = "%s missing"
            SECONDARYHANDSLOT = "Off Hand"
            local o = CreateFrame("Frame")
            local tt = CreateFrame("GameTooltip", "SetTip")
            tt:SetOwner(o, "ANCHOR_RIGHT")
            tt:SetEquipmentSet("Mix")
            "#,
        )
        .unwrap();
        let lines: Vec<String> = (1..=5)
            .map(|i| {
                s.eval::<String>(&format!("return SetTipTextLeft{i}:GetText()"))
                    .unwrap()
            })
            .collect();
        assert_eq!(
            lines,
            [
                "Mix",
                "1 equipped",
                "1 in bags",
                "1 ignored",
                "Off Hand missing"
            ]
        );
        assert_eq!(
            s.eval::<String>("return SetTipTextRight1:GetText()")
                .unwrap(),
            "3 items"
        );
        assert_eq!(s.eval::<i64>("return SetTip:NumLines()").unwrap(), 5);
    }
}
