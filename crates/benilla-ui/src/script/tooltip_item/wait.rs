//! The retrieving tooltip. A `Set*Item` whose template is in flight shows `RETRIEVING_ITEM_INFO` in
//! red and shows the plate (the no-stats item builder, `0x623760`); when a record the call missed
//! lands, the call runs again with its stored arguments (the tooltip's item callback, `0x626650`),
//! whatever the tooltip's owner does.

use std::collections::HashSet;

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::super::object::{frame_handle_of, frame_wrapper};
use super::super::tooltip::{append_line, tip_mut};
use super::super::tooltip_spell::{drawn_lines, Drawn};
use super::names::RED;
use crate::widget::FrameHandle;

/// Registry table of the stored setter calls, by tooltip frame id: `{method = name, n = count,
/// [1..n] = arguments}`, `this` first.
const REG_ITEM_CALLS: &str = "__benilla_item_tooltip_calls";

/// The item templates and item sets one setter call read and the stores lacked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ItemMisses {
    pub(crate) items: HashSet<u32>,
    pub(crate) sets: HashSet<u32>,
}

impl ItemMisses {
    fn is_empty(&self) -> bool {
        self.items.is_empty() && self.sets.is_empty()
    }
}

/// The setter call running on a tooltip, collecting its misses.
#[derive(Clone, Debug)]
pub(crate) struct ItemRender {
    h: FrameHandle,
    misses: ItemMisses,
}

impl ItemRender {
    /// The tooltip the setter call runs on.
    pub(crate) fn tooltip(&self) -> FrameHandle {
        self.h
    }
}

/// A setter call that missed, re-run when a record it missed lands. The re-run is skipped unless
/// the tooltip still shows and still holds `drawn`, so a line Lua added or rewrote since is kept.
#[derive(Clone, Debug)]
pub(crate) struct ItemWait {
    misses: ItemMisses,
    drawn: Drawn,
}

/// Record an item template the running setter call lacked.
pub(crate) fn note_item_miss(model: &mut crate::script::Model, item_id: u32) {
    if let Some(r) = model.item_render.as_mut() {
        r.misses.items.insert(item_id);
    }
}

/// Record an item set the running setter call lacked.
pub(crate) fn note_set_miss(model: &mut crate::script::Model, set_id: u32) {
    if let Some(r) = model.item_render.as_mut() {
        r.misses.sets.insert(set_id);
    }
}

/// The no-stats builder's line: `RETRIEVING_ITEM_INFO` in the tooltip red (`0x623765`).
pub(super) fn retrieving(lua: &Lua, this: &Table) -> mlua::Result<()> {
    if let Some(text) = crate::strings::global(lua, "RETRIEVING_ITEM_INFO") {
        append_line(lua, this, (text, RED), None, false)?;
    }
    Ok(())
}

/// The bindings whose builder registers the item callback: every item content setter.
const ITEM_SETTERS: &[&str] = &[
    "SetAction",
    "SetAuctionCompareItem",
    "SetAuctionItem",
    "SetAuctionSellItem",
    "SetBagItem",
    "SetBuybackItem",
    "SetExistingSocketGem",
    "SetGuildBankItem",
    "SetHyperlink",
    "SetInboxItem",
    "SetInventoryItem",
    "SetLFGCompletionReward",
    "SetLFGDungeonReward",
    "SetLootItem",
    "SetLootRollItem",
    "SetMerchantCompareItem",
    "SetMerchantCostItem",
    "SetMerchantItem",
    "SetQuestItem",
    "SetQuestLogItem",
    "SetSendMailItem",
    "SetSocketGem",
    "SetSocketedItem",
    "SetTradePlayerItem",
    "SetTradeSkillItem",
    "SetTradeTargetItem",
    "SetTrainerService",
];

/// Wrap each item setter on the GameTooltip method table so its misses are collected and, when
/// there are any, the call is kept to run again.
pub(crate) fn wrap_setters(lua: &Lua, m: &Table) -> mlua::Result<()> {
    lua.set_named_registry_value(REG_ITEM_CALLS, lua.create_table()?)?;
    for &name in ITEM_SETTERS {
        let Ok(inner) = m.get::<Function>(name) else {
            continue;
        };
        m.set(
            name,
            lua.create_function(move |lua, args: MultiValue| call_setter(lua, &inner, name, args))?,
        )?;
    }
    Ok(())
}

fn call_setter(
    lua: &Lua,
    inner: &Function,
    name: &str,
    args: MultiValue,
) -> mlua::Result<MultiValue> {
    let Some(Value::Table(this)) = args.front().cloned() else {
        return inner.call(args);
    };
    let h = frame_handle_of(lua, &this)?;
    // A nested setter (the compare's `SetInventoryItem`) collects its own misses.
    let outer = {
        let mut model = crate::script::model_mut_of(lua);
        model.item_tooltip_waits.remove(&h);
        model.item_render.replace(ItemRender {
            h,
            misses: ItemMisses::default(),
        })
    };
    let result = inner.call::<MultiValue>(args.clone());
    let render = {
        let mut model = crate::script::model_mut_of(lua);
        std::mem::replace(&mut model.item_render, outer)
    };
    let Some(render) = render.filter(|r| r.h == h && !r.misses.is_empty()) else {
        return result;
    };
    if result.is_ok() {
        let id = {
            let mut model = crate::script::model_mut_of(lua);
            let drawn = drawn_lines(&mut model, h)?;
            model.item_tooltip_waits.insert(
                h,
                ItemWait {
                    misses: render.misses,
                    drawn,
                },
            );
            model.frame_id(h)
        };
        let call = lua.create_table()?;
        call.set("method", name)?;
        call.set("n", args.len())?;
        for (i, v) in args.into_iter().enumerate() {
            call.raw_set(i + 1, v)?;
        }
        lua.named_registry_value::<Table>(REG_ITEM_CALLS)?
            .raw_set(id, call)?;
    }
    result
}

impl crate::script::UiScript {
    /// Re-run every waiting setter call `landed` answers, as the tooltip's item callback does.
    pub(crate) fn answer_item_waits(&mut self, landed: impl Fn(&ItemMisses) -> bool) {
        let waiting: Vec<FrameHandle> = {
            let model = self.model_ref();
            model
                .item_tooltip_waits
                .iter()
                .filter(|(_, w)| landed(&w.misses))
                .map(|(h, _)| *h)
                .collect()
        };
        for h in waiting {
            if let Err(e) = answer_wait(&self.lua, h) {
                self.push_error(e);
            }
        }
    }
}

/// Run `h`'s stored call again, unless the tooltip was hidden or its lines changed since.
fn answer_wait(lua: &Lua, h: FrameHandle) -> mlua::Result<()> {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        let Some(wait) = model.item_tooltip_waits.get(&h).cloned() else {
            return Ok(());
        };
        let shown = model.arena.frame(h).is_some_and(|f| f.shown);
        let fading = tip_mut(&mut model, h)?.fade_start.is_some();
        if !shown || fading || drawn_lines(&mut model, h)? != wait.drawn {
            model.item_tooltip_waits.remove(&h);
            return Ok(());
        }
        model.frame_id(h)
    };
    let Ok(call) = lua
        .named_registry_value::<Table>(REG_ITEM_CALLS)?
        .raw_get::<Table>(id)
    else {
        return Ok(());
    };
    let method: String = call.get("method")?;
    let n: usize = call.get("n")?;
    let args = (1..=n)
        .map(|i| call.raw_get::<Value>(i))
        .collect::<mlua::Result<Vec<_>>>()?;
    let f: Function = frame_wrapper(lua, id)?.get(method)?;
    f.call::<MultiValue>(MultiValue::from_vec(args))?;
    Ok(())
}
