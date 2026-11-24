//! The socketing window's engine, 12340's socket state (statics `0xc20fa8`..`0xc20fe8`) and
//! its bindings (registrar table `0xad0568`), with the two openers `SocketContainerItem`
//! (`0x5d8b10`) and `SocketInventoryItem` (`0x5ea6a0`).
//!
//! The engine holds the socketed item's guid and a gem guid per socket; the stock window reads it
//! through the verbs on `SOCKET_INFO_UPDATE` and closes on `SOCKET_INFO_CLOSE`. The socketed item
//! is locked while it is set (`0x5c4680`). The reference re-signals the window when the item's
//! fields change (`0x5c4ef0`) and drops a gem whose object goes (
//! `0x5c4f40`); here [`watch`] reads both off the bag and doll snapshots each tick.

use mlua::{Lua, Value};

use super::binding_abi::{flag, number_arg};
use super::cursor::{self, CursorPayload, EQUIPMENT_BAG};
use super::{ContainerSlot, EnchantView, InvSlotView, ItemTemplateView, Model};

/// The sockets an item can have, and the engine's gem slots.
pub const MAX_SOCKETS: usize = 3;

/// `GetSocketTypes`' names by colour bit (`0xad6704`, the tooltip's table).
const SOCKET_COLOR_NAMES: [&str; 4] = ["Meta", "Red", "Yellow", "Blue"];

/// The engine's state: the socketed item and the gem placed per socket.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SocketInfo {
    /// The socketed item; 0 when the window is closed.
    pub(crate) item: u64,
    /// The gem placed per socket; 0 for an empty socket.
    pub(crate) gems: [u64; MAX_SOCKETS],
    /// The socketed item's enchant slots when last signalled, so a change re-signals the window.
    seen: Vec<EnchantView>,
}

/// An item the player holds, found by guid: its slot, as the lock and the tooltip address it.
#[derive(Clone)]
pub(super) enum Held {
    /// A bag slot, read off the World at the call.
    Bag(i64, u32, Box<ContainerSlot>),
    Worn(u32, Box<InvSlotView>),
}

impl Held {
    /// The Lua `(bag, slot)` the item's lock and events name.
    pub(super) fn position(&self) -> (i64, u32) {
        match self {
            Held::Bag(bag, slot, _) => (*bag, *slot),
            Held::Worn(id, _) => (EQUIPMENT_BAG, *id),
        }
    }

    pub(super) fn item_id(&self) -> u32 {
        match self {
            Held::Bag(_, _, s) => s.item_id,
            Held::Worn(_, s) => s.item_id,
        }
    }

    pub(super) fn enchants(&self) -> &[EnchantView] {
        match self {
            Held::Bag(_, _, s) => &s.enchants,
            Held::Worn(_, s) => &s.enchants,
        }
    }

    /// The gem item in each socket enchant, the enchant's `SrcItemID`.
    pub(super) fn gems(&self) -> [u32; 3] {
        match self {
            Held::Bag(_, _, s) => s.gems,
            Held::Worn(_, s) => s.gems,
        }
    }

    pub(super) fn link(&self) -> Option<&str> {
        match self {
            Held::Bag(_, _, s) => s.link.as_deref(),
            Held::Worn(_, s) => s.link.as_deref(),
        }
    }

    /// The full name, random suffix joined.
    pub(super) fn name(&self) -> Option<String> {
        match self {
            Held::Bag(_, _, s) => s
                .link
                .is_some()
                .then(|| cursor::item_link_name(s.link.as_deref())),
            Held::Worn(_, s) => s.name.clone(),
        }
    }

    pub(super) fn guid(&self) -> u64 {
        match self {
            Held::Bag(_, _, s) => s.guid,
            Held::Worn(_, s) => s.guid,
        }
    }

    /// Bound at runtime: `ITEM_FIELD_FLAGS & 1` or a binding enchant (`0x708520`).
    pub(super) fn bound(&self) -> bool {
        match self {
            Held::Bag(_, _, s) => s.already_bound,
            Held::Worn(_, s) => s.already_bound,
        }
    }
}

/// The player's item with `guid`, in a bag, the bank or worn; `None` for 0 or one not held.
pub(super) fn find(model: &Model, guid: u64) -> Option<Held> {
    if guid == 0 {
        return None;
    }
    for (i, slot) in model.worn_slots().into_iter().enumerate() {
        if let Some(s) = slot.filter(|s| s.guid == guid) {
            return Some(Held::Worn(i as u32, Box::new(s)));
        }
    }
    super::container::live_bags(model)
        .into_iter()
        .find_map(|(bag, c)| {
            c.slots
                .into_iter()
                .find(|(_, s)| s.guid == guid)
                .map(|(slot, s)| Held::Bag(bag, slot, Box::new(s)))
        })
}

/// A held item as the cursor carries it, whole (`0x5c4e76`: no split).
fn cursor_item(held: &Held) -> cursor::CursorItem {
    let (bag, slot) = held.position();
    match held {
        Held::Bag(_, _, s) => cursor::CursorItem {
            bag,
            slot,
            item_id: s.item_id,
            texture: s.texture.clone(),
            link: s.link.clone(),
            quality: s.quality,
            count: None,
            bar_placeable: s.bar_placeable,
            equip_slots: s.equip_slots.clone(),
        },
        Held::Worn(_, s) => cursor::CursorItem {
            bag,
            slot,
            item_id: s.item_id,
            texture: s.icon.clone(),
            link: s.link.clone(),
            quality: Some(u32::try_from(s.quality).unwrap_or(0)),
            count: None,
            bar_placeable: s.bar_placeable,
            equip_slots: s.equip_slots.clone(),
        },
    }
}

/// `GetNumSockets` (`0x7094e0`): the template's coloured sockets plus a prismatic enchant's.
pub(super) fn num_sockets(model: &Model, held: &Held) -> u32 {
    let colored = super::item_stats::item_record(model, held.item_id()).map_or(0, |t| {
        t.socket_colors.iter().filter(|&&c| c != 0).count() as u32
    });
    let prismatic = held
        .enchants()
        .iter()
        .find(|e| e.slot == 6)
        .map_or(0, |e| e.prismatic_sockets);
    colored + prismatic
}

/// Whether a gem fits a socket (`0x5c48d0`): an uncoloured socket takes anything; a coloured one a gem
/// sharing a colour bit; a gem with no `GemProperties` row fits no colour.
pub(super) fn gem_matches(
    socket: Option<&ItemTemplateView>,
    gem: &ItemTemplateView,
    i: usize,
) -> bool {
    let Some(socket) = socket else {
        return false;
    };
    let color = socket.socket_colors.get(i).copied().unwrap_or(0);
    color == 0
        || gem
            .gem
            .as_ref()
            .is_some_and(|g| g.gem_type & color & 0xf != 0)
}

/// Set the socketed item (`0x5c4680`): the old item unlocks while the window still holds it (`0x5c4692`), each placed gem unlocks (`0x5c46b4`), the gems and the item are written (`0x5c46d7`, `0x5c46fa`), then a known item locks and signals `SOCKET_INFO_UPDATE` (`0x5c4751`, `0x5c475d`); 0 or an unknown template signals `SOCKET_INFO_CLOSE` (`0x5c4770`).
pub(super) fn set_item(model: &mut crate::script::ModelGuard<'_>, guid: u64) {
    if let Some((bag, slot)) = find(model, model.socket_info.item).map(|h| h.position()) {
        cursor::set_item_lock(model, bag, slot, false);
    }
    for i in 0..MAX_SOCKETS {
        let gem = model.socket_info.gems[i];
        if let Some((bag, slot)) = find(model, gem).map(|h| h.position()) {
            cursor::set_item_lock(model, bag, slot, false);
        }
    }
    let info = &mut model.socket_info;
    info.gems = [0; MAX_SOCKETS];
    info.item = guid;
    let new = find(model, guid).map(|h| (h.position(), h.item_id(), h.enchants().to_vec()));
    let stats = new.filter(|&(_, id, _)| super::item_stats::item_record_cached(model, id));
    match stats {
        Some(((bag, slot), _, enchants)) => {
            model.socket_info.seen = enchants;
            cursor::set_item_lock(model, bag, slot, true);
            signal_event(model, "SOCKET_INFO_UPDATE");
        }
        // No template (`0x5c4740`): every reader tests it beside the guid, so the item reads as none.
        None => {
            model.socket_info.item = 0;
            model.socket_info.seen.clear();
            signal_event(model, "SOCKET_INFO_CLOSE");
        }
    }
}

fn signal_event(model: &mut crate::script::ModelGuard<'_>, event: &str) {
    model.raise(event, Vec::new());
}

/// Place `gem` (0 empties the socket) in socket `i` (`0x5c4780`). A gem goes only where
/// its meta bit matches the socket's, else its refusal is handed back for the caller to show at
/// once. Returns whether it took.
fn set_gem(
    model: &mut crate::script::ModelGuard<'_>,
    i: usize,
    gem: u64,
) -> Result<bool, &'static str> {
    let item = model.socket_info.item;
    let Some(socket_stats) =
        find(model, item).and_then(|h| super::item_stats::item_record(model, h.item_id()))
    else {
        return Ok(false);
    };
    if i >= MAX_SOCKETS {
        return Ok(false);
    }
    if gem == 0 {
        model.socket_info.gems[i] = 0;
        signal_event(model, "SOCKET_INFO_UPDATE");
        return Ok(true);
    }
    let Some(stats) =
        find(model, gem).and_then(|h| super::item_stats::item_record(model, h.item_id()))
    else {
        return Ok(false);
    };
    if stats.gem_properties == 0 {
        return Ok(false);
    }
    let meta = socket_stats.socket_colors[i] & 1;
    match &stats.gem {
        Some(g) if g.gem_type & 1 != meta => {
            let key = if meta != 0 {
                "ERR_SOCKETING_REQUIRES_META_GEM"
            } else {
                "ERR_SOCKETING_META_GEM_ONLY_IN_METASLOT"
            };
            Err(key)
        }
        _ => {
            model.socket_info.gems[i] = gem;
            signal_event(model, "SOCKET_INFO_UPDATE");
            Ok(true)
        }
    }
}

/// The item watchers (`0x5c4ef0`, `0x5c4f40`) off the snapshots: the socketed item gone closes the window, its
/// enchants changing re-signals it, and a gem gone empties its socket.
pub(super) fn watch(model: &mut crate::script::ModelGuard<'_>) {
    let item = model.socket_info.item;
    let Some(held) = find(model, item) else {
        set_item(model, 0);
        return;
    };
    let mut changed = held.enchants() != model.socket_info.seen.as_slice();
    if changed {
        model.socket_info.seen = held.enchants().to_vec();
    }
    for i in 0..MAX_SOCKETS {
        let gem = model.socket_info.gems[i];
        if gem != 0 && find(model, gem).is_none() {
            model.socket_info.gems[i] = 0;
            changed = true;
        }
    }
    if changed {
        signal_event(model, "SOCKET_INFO_UPDATE");
    }
}

/// The worn gems' colour counts (`0x6de4d0`): meta, red, yellow and blue gems socketed in the worn items,
/// a gem counting once per colour bit. A broken item (durability 0 and not wrapped) and one
/// flagged `0x10` count none.
pub(super) fn equipped_gem_counts(model: &Model) -> [i32; 4] {
    let mut counts = [0; 4];
    for s in model.worn_slots().iter().skip(1).take(19).flatten() {
        let broken =
            s.flags & 0x8 == 0 && s.durability.is_some_and(|(cur, max)| max > 0 && cur == 0);
        if broken || s.flags & 0x10 != 0 {
            continue;
        }
        for e in s.enchants.iter().filter(|e| (2..=4).contains(&e.slot)) {
            let Some(t) = e.gem_type.filter(|&t| t != 0) else {
                continue;
            };
            for (bit, n) in counts.iter_mut().enumerate() {
                if t & (1 << bit) != 0 {
                    *n += 1;
                }
            }
        }
    }
    counts
}

/// The item-by-name table (`0x709de0`): an item object the player holds whose full name matches,
/// case-insensitively; filled as items are created (`0x70aa40`) and emptied as they go
/// (`0x709e20`), so only held items answer.
pub(super) fn item_id_by_name(model: &Model, name: &str) -> Option<u32> {
    // The table compares in UTF-8 without case (`0x76ea40`).
    let name = name.to_lowercase();
    let matches = |n: &str| n.to_lowercase() == name;
    model
        .worn_slots()
        .into_iter()
        .flatten()
        .find(|s| s.item_id != 0 && s.name.as_deref().is_some_and(matches))
        .map(|s| s.item_id)
        .or_else(|| {
            super::container::live_bags(model)
                .into_iter()
                .flat_map(|(_, c)| c.slots.into_values())
                .find(|s| {
                    s.item_id != 0
                        && s.link.is_some()
                        && matches(&cursor::item_link_name(s.link.as_deref()))
                })
                .map(|s| s.item_id)
        })
}

/// Whether the item holds an enchant (`0x708ac0`): a permanent enchant, a gem in the first two sockets or a
/// prismatic socket; the third socket is not read.
pub(super) fn has_enchantments(held: &Held) -> bool {
    held.enchants()
        .iter()
        .any(|e| matches!(e.slot, 0 | 2 | 3 | 6))
}

/// The open socket's gem in socket `i`: its template, the existing gem's item id for the
/// `existing` verbs or the placed gem for the `new` ones. A missing existing-gem template is
/// asked for.
fn gem_template(model: &mut Model, i: usize, existing: bool) -> Option<(u32, ItemTemplateView)> {
    let held = find(model, model.socket_info.item)?;
    let id = if existing {
        held.gems().get(i).copied().filter(|&id| id != 0)?
    } else {
        let gem = *model.socket_info.gems.get(i)?;
        find(model, gem)?.item_id()
    };
    match super::item_stats::item_record(model, id) {
        Some(t) => Some((id, t)),
        None => {
            if existing {
                super::item_stats::wait_item_record(model, id, super::ItemRecordWait::Socket);
                super::item_stats::ask_item_record(model, id);
            }
            None
        }
    }
}

/// The socket item's template, which the socket fit reads.
fn socket_stats(model: &Model) -> Option<ItemTemplateView> {
    find(model, model.socket_info.item)
        .and_then(|h| super::item_stats::item_record(model, h.item_id()))
}

/// The three values of `GetExistingSocketInfo` and `GetNewSocketInfo`: name, texture, matches.
fn socket_info_values(lua: &Lua, i: usize, existing: bool) -> mlua::Result<mlua::MultiValue> {
    let nils = || mlua::MultiValue::from_vec(vec![Value::Nil, Value::Nil, Value::Nil]);
    let (gem, socket) = {
        let mut model = crate::script::model_mut_of(lua);
        (gem_template(&mut model, i, existing), socket_stats(&model))
    };
    let Some((_, gem)) = gem else {
        return Ok(nils());
    };
    let matches = gem_matches(socket.as_ref(), &gem, i);
    Ok(mlua::MultiValue::from_vec(vec![
        Value::String(lua.create_string(&gem.name)?),
        match &gem.icon {
            Some(icon) => Value::String(lua.create_string(icon)?),
            None => Value::Nil,
        },
        flag(matches),
    ]))
}

/// A Lua socket index less one, compared unsigned against the three sockets.
fn socket_index(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<Option<usize>> {
    let n = number_arg(lua, v, usage)?;
    Ok(usize::try_from(n.wrapping_sub(1))
        .ok()
        .filter(|&i| i < MAX_SOCKETS))
}

impl super::UiScript {
    /// Take the `AcceptSockets` sends: the item and a gem guid per socket.
    pub fn take_socket_sends(&mut self) -> Vec<(u64, [u64; MAX_SOCKETS])> {
        drain_field!(self, socket_sends)
    }

    /// The socketed item's guid, 0 while the window is closed.
    pub fn socket_item(&self) -> u64 {
        self.model_ref().socket_info.item
    }

    /// Run [`watch`] while an item is set; idle otherwise, never touching the model.
    pub(super) fn watch_socket_item(&mut self) {
        if self.model_ref().socket_info.item != 0 {
            watch(&mut self.model_mut());
        }
    }
}

/// Register the socketing bindings and the two openers.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SocketContainerItem(bag, slot) (`0x5d8b10`): opens an unlocked item with sockets.
    g.set(
        "SocketContainerItem",
        lua.create_function(|lua, (bag, slot): (Value, Value)| {
            const USAGE: &str = "Usage: SocketContainerItem(index, slot)";
            let bag = i64::from(number_arg(lua, bag, USAGE)?);
            let slot = number_arg(lua, slot, USAGE)?;
            let mut model = crate::script::model_mut_of(lua);
            let Some(s) = u32::try_from(slot)
                .ok()
                .and_then(|slot| Some((slot, super::container::index_slot(&model, bag, slot)?)))
            else {
                return Ok(());
            };
            let (slot, s) = (s.0, s.1.clone());
            if super::container::bag_slot_locked(&model, bag, slot, &s) {
                return Ok(());
            }
            let held = Held::Bag(bag, slot, Box::new(s.clone()));
            if num_sockets(&model, &held) == 0 {
                return Ok(());
            }
            set_item(&mut model, s.guid);
            Ok(())
        })?,
    )?;

    // SocketInventoryItem(slot) (`0x5ea6a0` → `0x5e8bc0`): the same for a worn item.
    g.set(
        "SocketInventoryItem",
        lua.create_function(|lua, slot: Value| {
            let Some(slot) = super::char_stats::live_slot_arg(lua, &slot)?.filter(|&s| s != 0)
            else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            let Some(s) = usize::try_from(slot)
                .ok()
                .and_then(|i| model.inv_slot("player", i))
                .filter(|s| s.item_id != 0)
            else {
                return Ok(());
            };
            let id = slot as u32;
            if super::cursor::doll::is_inventory_item_locked(&model, id) {
                return Ok(());
            }
            let guid = s.guid;
            if num_sockets(&model, &Held::Worn(id, Box::new(s))) == 0 {
                return Ok(());
            }
            set_item(&mut model, guid);
            Ok(())
        })?,
    )?;

    g.set(
        "CloseSocketInfo",
        lua.create_function(|lua, ()| {
            set_item(&mut crate::script::model_mut_of(lua), 0);
            Ok(())
        })?,
    )?;

    // GetSocketItemInfo() (`0x5c49d0`) → name, texture, quality; quality is -1 for a template with
    // no `InventoryType`, and the closed window answers nil, nil, 0.
    g.set(
        "GetSocketItemInfo",
        lua.create_function(|lua, ()| {
            let (name, view) = {
                let model = crate::script::model_mut_of(lua);
                let held = find(&model, model.socket_info.item);
                let view = held
                    .as_ref()
                    .and_then(|h| super::item_stats::item_record(&model, h.item_id()));
                (held.and_then(|h| h.name()), view)
            };
            let Some(v) = view else {
                return Ok(mlua::MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            let quality = if v.inventory_type != 0 {
                i64::from(v.quality)
            } else {
                -1
            };
            Ok(mlua::MultiValue::from_vec(vec![
                Value::String(lua.create_string(name.unwrap_or(v.name))?),
                match v.icon {
                    Some(icon) => Value::String(lua.create_string(icon)?),
                    None => Value::Nil,
                },
                Value::Integer(quality),
            ]))
        })?,
    )?;

    g.set(
        "GetNumSockets",
        lua.create_function(|lua, ()| {
            let model = crate::script::model_mut_of(lua);
            Ok(find(&model, model.socket_info.item).map_or(0, |h| num_sockets(&model, &h)))
        })?,
    )?;

    // GetSocketTypes(index) (`0x5c4550`) → one name per colour bit of the socket, or "Socket" for
    // an uncoloured one; nothing while closed. The usage text is the reference's.
    g.set(
        "GetSocketTypes",
        lua.create_function(|lua, index: Value| {
            let n = number_arg(lua, index, "Usage: GetSocketType(index)")?;
            let Some(stats) = socket_stats(&crate::script::model_mut_of(lua)) else {
                return Ok(mlua::MultiValue::new());
            };
            let color = usize::try_from(n.wrapping_sub(1))
                .ok()
                .and_then(|i| stats.socket_colors.get(i).copied())
                .unwrap_or(0);
            let mut out = Vec::new();
            for (bit, name) in SOCKET_COLOR_NAMES.iter().enumerate() {
                if color & (1 << bit) != 0 {
                    out.push(Value::String(lua.create_string(name)?));
                }
            }
            if out.is_empty() {
                out.push(Value::String(lua.create_string("Socket")?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;

    // GetExistingSocketInfo(index) (`0x5c5160`) → the socketed gem's name, texture and whether it
    // fits its socket.
    g.set(
        "GetExistingSocketInfo",
        lua.create_function(|lua, index: Value| {
            match socket_index(lua, index, "Usage: GetExistingSocketInfo(index)")? {
                Some(i) => socket_info_values(lua, i, true),
                None => Ok(mlua::MultiValue::from_vec(vec![Value::Nil; 3])),
            }
        })?,
    )?;

    // GetExistingSocketLink(index) (`0x5c5340`): the socketed gem's link, from its id and quality.
    g.set(
        "GetExistingSocketLink",
        lua.create_function(|lua, index: Value| {
            let Some(i) = socket_index(lua, index, "Usage: GetExistingSocketLink(index)")? else {
                return Ok(Value::Nil);
            };
            let gem = gem_template(&mut crate::script::model_mut_of(lua), i, true);
            let level = crate::script::item_stats::player_level();
            match gem {
                Some((id, t)) => Ok(Value::String(lua.create_string(
                    super::item_stats::item_link(id, &t.name, t.quality, level),
                )?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetNewSocketInfo(index) (`0x5c4b60`): the placed gem's name, texture and fit.
    g.set(
        "GetNewSocketInfo",
        lua.create_function(|lua, index: Value| {
            match socket_index(lua, index, "Usage: GetNewSocketInfo(index)")? {
                Some(i) => socket_info_values(lua, i, false),
                None => Ok(mlua::MultiValue::from_vec(vec![Value::Nil; 3])),
            }
        })?,
    )?;

    // GetNewSocketLink(index) (`0x5c4d10`): the placed gem object's own link.
    g.set(
        "GetNewSocketLink",
        lua.create_function(|lua, index: Value| {
            let Some(i) = socket_index(lua, index, "Usage: GetNewSocketLink(index)")? else {
                return Ok(Value::Nil);
            };
            let link = {
                let model = crate::script::model_mut_of(lua);
                find(&model, model.socket_info.gems[i]).and_then(|h| h.link().map(str::to_string))
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ClickSocketButton(index) (`0x5c4dc0`): the held item goes into the socket, and the gem the
    // socket held comes back onto the cursor. An empty cursor just takes the placed gem back.
    g.set(
        "ClickSocketButton",
        lua.create_function(|lua, index: Value| {
            let n = number_arg(lua, index, "Usage: ClickSocketButton(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let held = match &model.cursor {
                Some(CursorPayload::Item(c)) => super::container::live_slot(&model, c.bag, c.slot)
                    .map(|s| s.guid)
                    .or_else(|| {
                        (c.bag == EQUIPMENT_BAG)
                            .then(|| model.inv_slot("player", c.slot as usize).map(|s| s.guid))
                            .flatten()
                    })
                    .unwrap_or(0),
                _ => 0,
            };
            let Some(i) = usize::try_from(n.wrapping_sub(1)).ok() else {
                return Ok(());
            };
            let previous = model.socket_info.gems.get(i).copied().unwrap_or(0);
            match set_gem(&mut model, i, held) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(key) => {
                    drop(model);
                    crate::script::ui_errors::show_now(lua, key);
                    return Ok(());
                }
            }
            // The gem stays locked in its socket (`0x5c4e5c`); the one it replaced comes back through the item setter, locked as it was placed (`0x5c4e85`).
            cursor::clear_cursor_with(&mut model, false, true);
            if let Some(back) = find(&model, previous).map(|h| cursor_item(&h)) {
                cursor::set_cursor(&mut model, CursorPayload::Item(back));
            }
            drop(model);
            super::calls::press_item_picks(lua);
            Ok(())
        })?,
    )?;

    // AcceptSockets() (`0x5c5460` → `0x5c4ff0`): `CMSG_SOCKET_GEMS` when a gem is placed.
    g.set(
        "AcceptSockets",
        lua.create_function(|lua, ()| {
            accept(&mut crate::script::model_mut_of(lua));
            Ok(())
        })?,
    )?;

    // GetSocketItemRefundable() (`0x5c5470` → `0x5c50e0`): the item's refund window is open and it
    // holds no enchant.
    g.set(
        "GetSocketItemRefundable",
        lua.create_function(|lua, ()| {
            let model = crate::script::model_mut_of(lua);
            let refundable = find(&model, model.socket_info.item).is_some_and(|h| {
                !has_enchantments(&h) && super::refund::seconds_left(&model, h.guid()).is_some()
            });
            Ok(flag(refundable))
        })?,
    )?;

    // GetSocketItemBoundTradeable() (`0x5c4ea0` → `0x5c4970`): bound, but still in its trade
    // window (`0x709550`).
    g.set(
        "GetSocketItemBoundTradeable",
        lua.create_function(|lua, ()| {
            let model = crate::script::model_mut_of(lua);
            let tradeable = find(&model, model.socket_info.item)
                .is_some_and(|h| h.bound() && super::refund::bound_trade_open(&model, h.guid()));
            Ok(flag(tradeable))
        })?,
    )?;

    Ok(())
}

/// The accept (`0x5c4ff0`): queue the send when an item is set and a gem placed; the engine keeps its
/// gems until their objects go.
pub(super) fn accept(model: &mut Model) {
    let info = &model.socket_info;
    if info.item != 0 && info.gems.iter().any(|&g| g != 0) {
        let send = (info.item, info.gems);
        model.socket_sends.push(send);
    }
}

#[cfg(test)]
mod tests {
    use crate::script::{ContainerSlot, ContainerState, GemView, ItemTemplateView, UiScript};

    /// A backpack holding two socketed chest pieces (slots 1 and 4) and two red gems (2 and 3).
    fn socketing() -> UiScript {
        let mut s = UiScript::new().unwrap();
        let armor = |name: &str| ItemTemplateView {
            name: name.into(),
            inventory_type: 5,
            socket_colors: [2, 8, 0],
            ..Default::default()
        };
        let gem = |name: &str| ItemTemplateView {
            name: name.into(),
            gem_properties: 3000,
            gem: Some(GemView {
                gem_type: 2,
                enchant: 3000,
                enchant_name: None,
                condition: None,
                requirement: None,
            }),
            ..Default::default()
        };
        s.set_item_template(1000, armor("First Chest"));
        s.set_item_template(1001, armor("Second Chest"));
        s.set_item_template(2000, gem("Bold Gem"));
        s.set_item_template(2001, gem("Bright Gem"));
        let slot = |guid: u64, item_id: u32, name: &str| ContainerSlot {
            guid,
            item_id,
            count: 1,
            link: Some(format!("|Hitem:{item_id}|h[{name}]|h")),
            ..Default::default()
        };
        s.set_container(
            0,
            Some(ContainerState {
                num_slots: 16,
                slots: std::collections::HashMap::from([
                    (1, slot(11, 1000, "First Chest")),
                    (2, slot(12, 2000, "Bold Gem")),
                    (3, slot(13, 2001, "Bright Gem")),
                    (4, slot(14, 1001, "Second Chest")),
                ]),
                ..Default::default()
            }),
        );
        s.run(
            r#"Log = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"ITEM_LOCK_CHANGED", "ITEM_LOCKED", "ITEM_UNLOCKED",
                    "SOCKET_INFO_UPDATE", "SOCKET_INFO_CLOSE", "CURSOR_UPDATE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function(_, event, bag, slot)
                Log[#Log + 1] = event .. " " .. tostring(slot) .. " " .. tostring((GetSocketItemInfo()))
            end)"#,
        )
        .unwrap();
        s
    }

    fn log(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>("local l = Log Log = {} return l")
            .unwrap()
    }

    fn locked(s: &UiScript, slot: u32) -> bool {
        s.eval::<bool>(&format!(
            "return select(3, GetContainerItemInfo(0, {slot})) ~= nil"
        ))
        .unwrap()
    }

    /// The setter unlocks the old item and each placed gem while the window still holds the old item (`0x5c4692`, `0x5c46b4`), writes the new one (`0x5c46fa`), then locks it and signals (`0x5c4751`, `0x5c475d`).
    #[test]
    fn the_setter_unlocks_the_old_item_and_its_gems_before_it_writes_the_new_one() {
        let s = socketing();
        s.run("SocketContainerItem(0, 1)").unwrap();
        assert_eq!(
            log(&s),
            [
                "ITEM_LOCK_CHANGED 1 First Chest",
                "ITEM_LOCKED 1 First Chest",
                "SOCKET_INFO_UPDATE nil First Chest",
            ]
        );
        s.run("PickupContainerItem(0, 2) ClickSocketButton(1)")
            .unwrap();
        let _ = log(&s);
        s.run("SocketContainerItem(0, 4)").unwrap();
        assert_eq!(
            log(&s),
            [
                "ITEM_LOCK_CHANGED 1 First Chest",
                "ITEM_UNLOCKED 1 First Chest",
                "ITEM_LOCK_CHANGED 2 First Chest",
                "ITEM_UNLOCKED 2 First Chest",
                "ITEM_LOCK_CHANGED 4 Second Chest",
                "ITEM_LOCKED 4 Second Chest",
                "SOCKET_INFO_UPDATE nil Second Chest",
            ]
        );
        assert!(!locked(&s, 1) && !locked(&s, 2) && locked(&s, 4));
        s.run("CloseSocketInfo()").unwrap();
        assert_eq!(
            log(&s),
            [
                "ITEM_LOCK_CHANGED 4 Second Chest",
                "ITEM_UNLOCKED 4 Second Chest",
                "SOCKET_INFO_CLOSE nil nil",
            ]
        );
    }

    /// The socket click's clear leaves the placed gem locked (`0x5c4e5c`), and the gem it replaced comes back through the item setter as locked as it was, with no lock signal (`0x5c4e85`).
    #[test]
    fn a_placed_gem_stays_locked_and_the_one_it_replaces_comes_back_locked() {
        let s = socketing();
        s.run("SocketContainerItem(0, 1) PickupContainerItem(0, 2)")
            .unwrap();
        let _ = log(&s);
        s.run("ClickSocketButton(1)").unwrap();
        assert_eq!(
            log(&s),
            [
                "SOCKET_INFO_UPDATE nil First Chest",
                "CURSOR_UPDATE nil First Chest"
            ]
        );
        assert!(locked(&s, 2) && s.cursor_item().is_none());
        s.run("PickupContainerItem(0, 3)").unwrap();
        let _ = log(&s);
        s.run("ClickSocketButton(1)").unwrap();
        assert_eq!(
            log(&s),
            [
                "SOCKET_INFO_UPDATE nil First Chest",
                "CURSOR_UPDATE nil First Chest",
                "CURSOR_UPDATE nil First Chest",
                "CURSOR_UPDATE nil First Chest",
            ],
            "the click's clear, then the setter's clear and its own signal"
        );
        assert_eq!(s.cursor_item().map(|c| c.slot), Some(2));
        assert!(locked(&s, 2) && locked(&s, 3));
    }
}
