//! The socketing window's three tooltip setters (`0x6301a0`, `0x630250`, `0x630370`), over the
//! engine's socketed item and gems.

use mlua::{Lua, Table, Value};

use super::render::{render_view, BuilderFlags, ItemInstance, Sell};
use super::{retrieving, view_of};
use crate::script::object::frame_handle_of;
use crate::script::socket::{find, Held, MAX_SOCKETS};
use crate::script::tooltip::{clear_content, fire_cleared, show_or_hide_empty};

/// A held item's instance block, as `SetBagItem` and `SetInventoryItem` build theirs.
fn instance_of(held: &Held<'_>) -> (ItemInstance, u32) {
    match *held {
        Held::Bag(_, _, s) => (
            ItemInstance {
                name: held.name(),
                durability: s.durability,
                creator: s.creator.clone(),
                has_text: s.readable,
                flags: s.flags,
                already_bound: s.already_bound,
                petition: s.petition.clone(),
                enchants: s.enchants.clone(),
                openable_source: true,
                duration_ms: s.duration_ms,
                guid: s.guid,
                socket_preview: None,
                inspected: false,
            },
            s.count.max(1),
        ),
        Held::Worn(_, s) => (
            ItemInstance {
                name: s.name.clone(),
                durability: s.durability,
                creator: s.creator.clone(),
                has_text: false,
                flags: s.flags,
                already_bound: s.already_bound,
                petition: None,
                enchants: s.enchants.clone(),
                openable_source: true,
                duration_ms: s.duration_ms,
                guid: s.guid,
                socket_preview: None,
                inspected: false,
            },
            s.count.max(1),
        ),
    }
}

/// Render `item_id` into the tooltip, the setters' shared tail.
fn render(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    flags: BuilderFlags,
    inst: Option<&ItemInstance>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    clear_content(&mut crate::script::model_mut_of(lua), h);
    fire_cleared(lua, h);
    match view_of(lua, item_id) {
        Some(v) => render_view(lua, this, &v, flags, inst)?,
        None => retrieving(lua, this)?,
    }
    show_or_hide_empty(lua, h);
    Ok(())
}

/// An item object's money block: the stack, no repair, worn below inventory slot 23.
fn sell_of(held: &Held<'_>, stack: u32) -> Sell {
    Sell::Object {
        stack,
        repair_cost: 0,
        worn: matches!(held, Held::Worn(id, _) if *id <= 23),
    }
}

/// `Usage: <name>:<method>(…)`, the reference's text with the tooltip's own name.
fn usage(this: &Table, call: &str) -> mlua::Error {
    let name = this
        .get::<mlua::Function>("GetName")
        .and_then(|f| f.call::<Option<String>>(this.clone()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".into());
    mlua::Error::RuntimeError(format!("Usage: {name}:{call}"))
}

fn index_arg(lua: &Lua, this: &Table, v: Value, call: &str) -> mlua::Result<Option<usize>> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(usize::try_from((n as i64 as i32).wrapping_sub(1))
            .ok()
            .filter(|&i| i < MAX_SOCKETS)),
        None => Err(usage(this, call)),
    }
}

pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetSocketedItem() (`0x6301a0`): the socketed item in the socket preview, each
    // placed gem standing in its socket; nothing while the window is closed.
    m.set(
        "SetSocketedItem",
        lua.create_function(|lua, this: Table| {
            let built = {
                let model = crate::script::model_mut_of(lua);
                find(&model, model.socket_info.item).map(|held| {
                    let (mut inst, stack) = instance_of(&held);
                    let preview = model.socket_info.gems.map(|gem| {
                        find(&model, gem)
                            .and_then(|g| model.item_templates.get(&g.item_id()).cloned())
                    });
                    inst.socket_preview = Some(preview);
                    (held.item_id(), inst, sell_of(&held, stack))
                })
            };
            let Some((item_id, inst, sell)) = built else {
                return Ok(());
            };
            let flags = BuilderFlags {
                sell,
                ..Default::default()
            };
            render(lua, &this, item_id, flags, Some(&inst))
        })?,
    )?;

    // GameTooltip:SetSocketGem(index) (`0x630250`): the gem placed in socket `index`, as an item
    // object.
    m.set(
        "SetSocketGem",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            let Some(i) = index_arg(lua, &this, index, "SetSocketGem(index)")? else {
                return Ok(());
            };
            let built = {
                let model = crate::script::model_mut_of(lua);
                find(&model, model.socket_info.gems[i]).map(|held| {
                    let (inst, stack) = instance_of(&held);
                    (held.item_id(), inst, sell_of(&held, stack))
                })
            };
            let Some((item_id, inst, sell)) = built else {
                return Ok(());
            };
            let flags = BuilderFlags {
                sell,
                ..Default::default()
            };
            render(lua, &this, item_id, flags, Some(&inst))
        })?,
    )?;

    // GameTooltip:SetExistingSocketGem(index[, toDestroy]) (`0x630370`): the gem socket `index`
    // holds, by its template alone; `toDestroy` heads it with DESTROY_GEM. The usage text is the
    // reference's.
    m.set(
        "SetExistingSocketGem",
        lua.create_function(
            |lua, (this, index, destroy): (Table, Value, Option<Value>)| {
                let Some(i) = index_arg(lua, &this, index, "SetSocketGem(index, [toDestroy])")?
                else {
                    return Ok(());
                };
                let destroy = crate::script::binding_abi::bool_or_default(destroy.as_ref(), false);
                let gem = {
                    let model = crate::script::model_mut_of(lua);
                    find(&model, model.socket_info.item)
                        .and_then(|h| h.gems().get(i).copied())
                        .filter(|&id| id != 0)
                };
                let Some(gem) = gem else {
                    return Ok(());
                };
                let flags = BuilderFlags {
                    currently_equipped: destroy,
                    sell: Sell::ONE,
                    ..Default::default()
                };
                render(lua, &this, gem, flags, None)
            },
        )?,
    )?;

    Ok(())
}
