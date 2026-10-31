//! The socketing window's engine, 12340's `CGItemSocketInfo` (statics `0xc20fa8`..`0xc20fe8`) and
//! its bindings (registrar table `0xad0568`), with the two openers `SocketContainerItem`
//! (`0x5d8b10`) and `SocketInventoryItem` (`0x5ea6a0`).
//!
//! The engine holds the socketed item's guid and a gem guid per socket; the stock window reads it
//! through the verbs on `SOCKET_INFO_UPDATE` and closes on `SOCKET_INFO_CLOSE`. The socketed item
//! is locked while it is set (`0x5c4680`). The reference re-signals the window when the item's
//! fields change (`UpdateItem` `0x5c4ef0`) and drops a gem whose object goes (`RemoveItem`
//! `0x5c4f40`); here [`watch`] reads both off the bag and doll snapshots each tick.

use mlua::{Lua, Value};

use super::binding_abi::{flag, number_arg};
use super::cursor::{self, CursorPayload, EQUIPMENT_BAG};
use super::{ContainerSlot, EnchantView, InvSlotView, ItemTemplateView, Model};

/// The sockets an item can have, and the engine's gem slots.
pub const MAX_SOCKETS: usize = 3;

/// `GetSocketTypes`' names by colour bit (`0xa25f60`, the tooltip's table).
const SOCKET_COLOR_NAMES: [&str; 4] = ["Meta", "Red", "Yellow", "Blue"];

/// The engine's state: the socketed item and the gem placed per socket.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SocketInfo {
    /// `m_socketItem`; 0 when the window is closed.
    pub(crate) item: u64,
    /// `m_gemItems`; 0 for an empty socket.
    pub(crate) gems: [u64; MAX_SOCKETS],
    /// The socketed item's enchant slots when last signalled, so a change re-signals the window.
    seen: Vec<EnchantView>,
    /// Gem templates `GetExistingSocketInfo` asked for; one arriving re-signals the window, as the
    /// reference's cache callback does (`0x5c44d0`).
    awaiting: Vec<u32>,
}

/// An item the player holds, found by guid: its slot, as the lock and the tooltip address it.
#[derive(Clone, Copy)]
pub(super) enum Held<'a> {
    Bag(i64, u32, &'a ContainerSlot),
    Worn(u32, &'a InvSlotView),
}

impl<'a> Held<'a> {
    /// The Lua `(bag, slot)` the item's lock and events name.
    pub(super) fn position(&self) -> (i64, u32) {
        match *self {
            Held::Bag(bag, slot, _) => (bag, slot),
            Held::Worn(id, _) => (EQUIPMENT_BAG, id),
        }
    }

    pub(super) fn item_id(&self) -> u32 {
        match self {
            Held::Bag(_, _, s) => s.item_id,
            Held::Worn(_, s) => s.item_id,
        }
    }

    pub(super) fn enchants(&self) -> &'a [EnchantView] {
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

    pub(super) fn link(&self) -> Option<&'a str> {
        match self {
            Held::Bag(_, _, s) => s.link.as_deref(),
            Held::Worn(_, s) => s.link.as_deref(),
        }
    }

    /// The full name, random suffix joined (`GetFullItemName`).
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
pub(super) fn find(model: &Model, guid: u64) -> Option<Held<'_>> {
    if guid == 0 {
        return None;
    }
    for (i, slot) in model.inventory_slots.iter().enumerate() {
        if let Some(s) = slot.as_ref().filter(|s| s.guid == guid) {
            return Some(Held::Worn(i as u32, s));
        }
    }
    model.containers.iter().find_map(|(&bag, c)| {
        c.slots
            .iter()
            .find(|(_, s)| s.guid == guid)
            .map(|(&slot, s)| Held::Bag(bag, slot, s))
    })
}

/// Whether the socketed item sits at `(bag, slot)`: it reads locked there (`0x5c4680`).
pub(super) fn holds_slot(model: &Model, bag: i64, slot: u32) -> bool {
    find(model, model.socket_info.item).is_some_and(|h| h.position() == (bag, slot))
}

/// `GetNumSockets` (`0x7094e0`): the template's coloured sockets plus a prismatic enchant's.
pub(super) fn num_sockets(model: &Model, held: &Held<'_>) -> u32 {
    let colored = model.item_templates.get(&held.item_id()).map_or(0, |t| {
        t.socket_colors.iter().filter(|&&c| c != 0).count() as u32
    });
    let prismatic = held
        .enchants()
        .iter()
        .find(|e| e.slot == 6)
        .map_or(0, |e| e.prismatic_sockets);
    colored + prismatic
}

/// `GemMatchesSocket` (`0x5c48d0`): an uncoloured socket takes anything; a coloured one a gem
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

/// `SetItem` (`0x5c4680`): unlocks the old item, clears the gems, then locks the new one and
/// signals `SOCKET_INFO_UPDATE`, or signals `SOCKET_INFO_CLOSE` for 0 or an unknown template.
pub(super) fn set_item(model: &mut Model, guid: u64) {
    let old = find(model, model.socket_info.item).map(|h| h.position());
    let new = find(model, guid).map(|h| (h.position(), h.item_id(), h.enchants().to_vec()));
    if let Some((bag, slot)) = old {
        cursor::queue_lock_changed(model, bag, slot);
    }
    let info = &mut model.socket_info;
    info.gems = [0; MAX_SOCKETS];
    info.awaiting.clear();
    info.item = guid;
    let stats = new.filter(|(_, id, _)| model.item_templates.contains_key(id));
    match stats {
        Some(((bag, slot), _, enchants)) => {
            model.socket_info.seen = enchants;
            cursor::queue_lock_changed(model, bag, slot);
            queue(model, "SOCKET_INFO_UPDATE");
        }
        None => {
            model.socket_info.item = 0;
            model.socket_info.seen.clear();
            queue(model, "SOCKET_INFO_CLOSE");
        }
    }
}

fn queue(model: &mut Model, event: &str) {
    model.pending_events.push((event.to_string(), Vec::new()));
}

/// `SetGem` (`0x5c4780`): place `gem` (0 empties the socket) in socket `i`. A gem goes only where
/// its meta bit matches the socket's, else the reference's error shows. Returns whether it took.
fn set_gem(model: &mut Model, i: usize, gem: u64) -> bool {
    let item = model.socket_info.item;
    let Some(socket_stats) = find(model, item).and_then(|h| model.item_templates.get(&h.item_id()))
    else {
        return false;
    };
    if i >= MAX_SOCKETS {
        return false;
    }
    if gem == 0 {
        model.socket_info.gems[i] = 0;
        queue(model, "SOCKET_INFO_UPDATE");
        return true;
    }
    let Some(stats) = find(model, gem).and_then(|h| model.item_templates.get(&h.item_id())) else {
        return false;
    };
    if stats.gem_properties == 0 {
        return false;
    }
    let meta = socket_stats.socket_colors[i] & 1;
    match &stats.gem {
        Some(g) if g.gem_type & 1 != meta => {
            let key = if meta != 0 {
                "ERR_SOCKETING_REQUIRES_META_GEM"
            } else {
                "ERR_SOCKETING_META_GEM_ONLY_IN_METASLOT"
            };
            model.ui_errors.push(key);
            false
        }
        _ => {
            model.socket_info.gems[i] = gem;
            queue(model, "SOCKET_INFO_UPDATE");
            true
        }
    }
}

/// `UpdateItem` and `RemoveItem` off the snapshots: the socketed item gone closes the window, its
/// enchants changing re-signals it, and a gem gone empties its socket.
pub(super) fn watch(model: &mut Model) {
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
        queue(model, "SOCKET_INFO_UPDATE");
    }
}

/// A template `GetExistingSocketInfo` waited on arrived: re-signal the window.
pub(super) fn template_arrived(model: &mut Model, item_id: u32) {
    let info = &mut model.socket_info;
    if let Some(i) = info.awaiting.iter().position(|&id| id == item_id) {
        info.awaiting.swap_remove(i);
        if info.item != 0 {
            queue(model, "SOCKET_INFO_UPDATE");
        }
    }
}

/// `HasEnchantments` (`0x708ac0`): a permanent enchant, a gem in the first two sockets or a
/// prismatic socket; the third socket is not read.
pub(super) fn has_enchantments(held: &Held<'_>) -> bool {
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
    match model.item_templates.get(&id).cloned() {
        Some(t) => Some((id, t)),
        None => {
            if existing {
                model.item_stat_asks.insert(id);
                if !model.socket_info.awaiting.contains(&id) {
                    model.socket_info.awaiting.push(id);
                }
            }
            None
        }
    }
}

/// The socket item's template, which `GemMatchesSocket` reads.
fn socket_stats(model: &Model) -> Option<ItemTemplateView> {
    find(model, model.socket_info.item)
        .and_then(|h| model.item_templates.get(&h.item_id()).cloned())
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
                .and_then(|slot| Some((slot, model.containers.get(&bag)?.slots.get(&slot)?)))
            else {
                return Ok(());
            };
            let (slot, s) = (s.0, s.1.clone());
            if super::container::bag_slot_locked(&model, bag, slot, &s) {
                return Ok(());
            }
            let held = Held::Bag(bag, slot, &s);
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
            let slot = number_arg(lua, slot, "Usage: SocketInventoryItem(slot)")?;
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
            if num_sockets(&model, &Held::Worn(id, &s)) == 0 {
                return Ok(());
            }
            set_item(&mut model, s.guid);
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
                let view = held.and_then(|h| model.item_templates.get(&h.item_id()).cloned());
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
            match gem {
                Some((id, t)) => Ok(Value::String(
                    lua.create_string(super::item_stats::item_link(id, &t.name, t.quality))?,
                )),
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
                Some(CursorPayload::Item(c)) => model
                    .containers
                    .get(&c.bag)
                    .and_then(|b| b.slots.get(&c.slot))
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
            if !set_gem(&mut model, i, held) {
                return Ok(());
            }
            cursor::clear_cursor(&mut model);
            if let Some((bag, slot)) = find(&model, previous).map(|h| h.position()) {
                super::container::pickup_container_item(&mut model, bag, slot);
            }
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

/// `Accept` (`0x5c4ff0`): queue the send when an item is set and a gem placed; the engine keeps its
/// gems until their objects go.
pub(super) fn accept(model: &mut Model) {
    let info = &model.socket_info;
    if info.item != 0 && info.gems.iter().any(|&g| g != 0) {
        let send = (info.item, info.gems);
        model.socket_sends.push(send);
    }
}
