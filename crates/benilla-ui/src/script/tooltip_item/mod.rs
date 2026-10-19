//! The engine item-tooltip renderer: every item hover renders through one line law, as the
//! reference's `Set*Item` bindings share the builder `CGTooltip::SetItem 0x6277f0`. A template in
//! flight shows `RETRIEVING_ITEM_INFO` and repaints on arrival ([`wait`]). The methods register
//! into the GameTooltip kind table beside the widget verbs ([`super::tooltip`]). Every family's order,
//! gate and colour is the reference's, and every sentence is a key resolved off the player's own
//! `GlobalStrings.lua` at render time. Red (`0xc0d390`, `0xffff2020`) marks unmet requirements,
//! LOCKED, broken durability and "Already known"; the name never recolors.
//!
//! Stock FrameXML seats the shopping plates (`MerchantFrame.xml:63-80`,
//! `Blizzard_AuctionUI.lua:1078`). There is no hover compare: `SHOW_COMPARE_TOOLTIP` has no fire
//! site (`0x49211a` and `0x49630d` never produce event 377), so `PaperDollFrame.lua`'s listener
//! for it is dead code.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{clear_content, fire_cleared, show_or_hide_empty};
use super::ItemTemplateView;

mod names;
mod render;
mod wait;

pub(super) use render::Sell;
use render::{render_view, BuilderFlags};
use wait::{note_item_miss, retrieving};
pub(crate) use wait::{note_set_miss, wrap_setters, ItemRender, ItemWait};

/// The body both compare bindings run (`0x536080`, `0x535d70`): walk `0x809200[InventoryType]`'s
/// slots, skipping empty ones uncounted and ones whose worn item's class differs (`0x5d9f30`),
/// until `left` matches have passed; then run `SetInventoryItem` there with the header latch armed,
/// as `0x536080` calls `0x52b650` with p4 = 0, p5 = 1. The number 1, or nil on a miss, which leaves
/// the tooltip untouched.
fn compare_against_worn(
    lua: &Lua,
    this: &Table,
    h: crate::widget::FrameHandle,
    offered: &ItemTemplateView,
    mut left: f64,
) -> mlua::Result<Value> {
    let found = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        let mut hit = None;
        for &slot in equip_slots_for(offered.inventory_type) {
            let Some(worn) = model
                .inv_slot("player", slot as usize)
                .filter(|s| s.item_id != 0)
            else {
                continue; // an empty slot is not counted
            };
            let same_class = model
                .item_templates
                .get(&worn.item_id)
                .is_some_and(|w| w.class == offered.class);
            if !same_class {
                continue;
            }
            if left <= 0.0 {
                hit = Some(slot as usize);
                break;
            }
            left -= 1.0;
        }
        hit
    };
    let Some(slot) = found else {
        return Ok(Value::Nil);
    };
    {
        let mut model = crate::script::model_mut_of(lua);
        if let Ok(t) = super::tooltip::tip_mut(&mut model, h) {
            t.equipped_header_armed = true;
        }
    }
    this.get::<mlua::Function>("SetInventoryItem")?
        .call::<mlua::MultiValue>((this.clone(), "player", slot))?;
    Ok(Value::Integer(1))
}

/// The cached template; a miss records the ask and the running setter's wait.
fn view_of(lua: &Lua, item_id: u32) -> Option<ItemTemplateView> {
    let mut model = crate::script::model_mut_of(lua);
    let v = model.item_templates.get(&item_id).cloned();
    if v.is_none() && item_id != 0 {
        model.item_stat_asks.insert(item_id);
        note_item_miss(&mut model, item_id);
    }
    v
}

/// A random-suffix roll's enchant lines: the reference copies the suffix row's five ids into
/// enchant slots 2..6 (`0x52b7bf`) for a block source with no item object.
fn roll_enchants(lua: &Lua, random_property_id: u32) -> Vec<crate::script::EnchantView> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model
        .random_properties
        .get(&random_property_id)
        .map(|v| v.enchants.clone())
        .unwrap_or_default()
}

fn link_name(link: &str) -> Option<&str> {
    let start = link.find('[')? + 1;
    let end = link[start..].find(']')? + start;
    Some(&link[start..end])
}

/// Fire `OnTooltipAddMoney(cost, maxcost)`, maxcost nil when absent (`CGTooltip::AddMoney`
/// `0x61a0d0`): the engine computes, FrameXML's `SetTooltipMoney` draws.
fn fire_add_money(lua: &Lua, h: crate::widget::FrameHandle, cost: u64, max: Option<u64>) {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        model.frame_id(h)
    };
    let max = max.map_or(Value::Nil, |m| Value::Number(m as f64));
    if let Err(e) = super::event::fire_widget_handler(
        lua,
        id,
        "OnTooltipAddMoney",
        vec![Value::Number(cost as f64), max],
    ) {
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
}

fn hyperlink_enchant_spell(link: &str) -> Option<u32> {
    let at = link.find("enchant:")? + 8;
    let tail = &link[at..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse().ok().filter(|&id: &u32| id != 0)
}

fn hyperlink_item_fields(link: &str) -> Option<(u32, u32, u32)> {
    let at = link.find("item:")? + 5;
    let tail = &link[at..];
    let end = tail.find("|h").unwrap_or(tail.len());
    let mut fields = tail[..end]
        .split(':')
        .map(|f| f.trim().parse().unwrap_or(0));
    let item_id: u32 = fields.next()?;
    let enchant_id = fields.next().unwrap_or(0);
    let random_property_id = fields.next().unwrap_or(0);
    (item_id != 0).then_some((item_id, enchant_id, random_property_id))
}

/// The id-keyed render; on a miss, the ask and the retrieving line. The engine's own item hovers
/// reach it from Rust (the action bar's, `SetTrainerService`, `SetTradeSkillItem`).
pub(super) fn render_by_id(lua: &Lua, this: &Table, item_id: u32, sell: Sell) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    match view_of(lua, item_id) {
        Some(v) => {
            let flags = BuilderFlags {
                sell,
                ..Default::default()
            };
            render_view(lua, this, &v, flags, None)?;
        }
        None => retrieving(lua, this)?,
    }
    show_or_hide_empty(lua, h);
    Ok(())
}

/// An action-bar item's money source: the copy `SetAction` (`0x631000`) finds through
/// `FindItemOfType`: a worn copy of equipment, else the first carried stack, else the template.
pub(super) fn carried_sell(lua: &Lua, item_id: u32) -> Sell {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let equippable = model
        .item_templates
        .get(&item_id)
        .is_some_and(|t| t.inventory_type != 0);
    if equippable {
        for slot in 1..=19 {
            if let Some(s) = model
                .inv_slot("player", slot)
                .filter(|s| s.item_id == item_id)
            {
                return Sell::Object {
                    stack: s.count,
                    repair_cost: model.inv_repair_cost(slot),
                    worn: true,
                };
            }
        }
    }
    for bag in 0..=4 {
        let Some(c) = model.containers.get(&bag) else {
            continue;
        };
        for slot in 1..=c.num_slots {
            if let Some(s) = c.slots.get(&slot).filter(|s| s.item_id == item_id) {
                return Sell::Object {
                    stack: s.count.max(1),
                    repair_cost: model
                        .repair_costs
                        .bags
                        .get(&(bag, slot))
                        .copied()
                        .unwrap_or(0),
                    worn: false,
                };
            }
        }
    }
    Sell::ONE
}

fn set_quest_item_view(
    lua: &Lua,
    this: &Table,
    item: Option<crate::script::quest::QuestItemView>,
) -> mlua::Result<()> {
    match item {
        // The count as the link-info stack (`0x62e670`, `0x62e790`).
        Some(it) => render_by_id(
            lua,
            this,
            it.item_id,
            Sell::Block {
                stack: it.count,
                max: None,
            },
        ),
        None => {
            let h = frame_handle_of(lua, this)?;
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        }
    }
}

/// An InventoryType's compare candidate slots, as `GetInventorySlotInfo` ids: the reference's
/// mask table `0x809200[InventoryType]`.
fn equip_slots_for(inventory_type: u32) -> &'static [u32] {
    match inventory_type {
        1 => &[1],             // head
        2 => &[2],             // neck
        3 => &[3],             // shoulder
        4 => &[4],             // shirt
        5 | 20 => &[5],        // chest / robe
        6 => &[6],             // waist
        7 => &[7],             // legs
        8 => &[8],             // feet
        9 => &[9],             // wrist
        10 => &[10],           // hands
        11 => &[11, 12],       // finger
        12 => &[13, 14],       // trinket
        16 => &[15],           // back
        19 => &[19],           // tabard
        13 => &[16, 17],       // one-hand
        21 => &[16],           // main hand
        14 | 22 | 23 => &[17], // shield / off-hand weapon / held
        // Two-hand: main hand only, though a two-hander displaces both: `0x809200[17]` is
        // `0x8000`, bit 15.
        17 => &[16],
        15 | 25 | 26 | 28 => &[18], // bow / thrown / wand-gun / relic
        _ => &[],
    }
}

/// Register the item content channels into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetQuestItem(type, index) / SetQuestLogItem(type, index) (`0x533610`,
    // `0x533760`; `QuestFrameTemplates.xml:148`, `QuestLogFrame.xml:113`), no returns. `type` is
    // `GetQuestItemInfo`'s; an unknown type or index leaves the tooltip empty, as in the reference.
    m.set(
        "SetQuestItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    crate::script::quest::item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;
    m.set(
        "SetQuestLogItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.selected_quest_detail().and_then(|d| {
                    let v = match kind.as_str() {
                        "choice" => Some(&d.choices),
                        "reward" => Some(&d.rewards),
                        _ => None,
                    };
                    v.and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;

    // GameTooltip:SetHyperlink(link): the chat-link tooltip (`ItemRef.lua:60`). Takes an item
    // link, escaped or a bare "item:<id>", an enchant link or an achievement link; any other
    // link does nothing.
    m.set(
        "SetHyperlink",
        lua.create_function(|lua, (this, link): (Table, String)| {
            if super::achievement::set_achievement_link(lua, &this, &link)? {
                return Ok(());
            }
            // The only other link it takes, a recipe's `|Henchant:<spellId>|h[name]|h`
            // (`GetTradeSkillRecipeLink`): the spell's own tooltip.
            if let Some(spell) = hyperlink_enchant_spell(&link) {
                return super::tooltip_spell::set_spell_by_id(
                    lua,
                    &this,
                    spell,
                    link_name(&link).map(str::to_string),
                    Default::default(),
                    None,
                );
            }
            let Some((id, _enchant_id, roll)) = hyperlink_item_fields(&link) else {
                return Ok(());
            };
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The reference's format is `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"` (`0x8549c8`), and
            // `SetHyperlink` (`0x532181`) parses it into a block (p6=1): the enchant id to slot 0
            // (`+0x3d0`), the roll to `+0x424` (expanded into slots 2..6 by `0x52b7bf`), the
            // unique id to `+0x420`, unread; a missing field is 0. So a link shows its roll's
            // lines, never the placeholder (`0x52cc33`). The name is the link's own, built with
            // the suffix joined (`0x5d8b00`). The enchant id is parsed, not rendered: naming it
            // needs its `SpellItemEnchantment` row, and no link benilla composes carries one.
            let inst = render::ItemInstance {
                name: link_name(&link).map(str::to_string),
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            // A link's block keeps `Clear`'s stack of 1: the link carries no count (`0x62dae0`).
            let flags = BuilderFlags {
                sell: Sell::ONE,
                ..Default::default()
            };
            match view_of(lua, id) {
                Some(v) => {
                    render_view(lua, &this, &v, flags, Some(&inst))?;
                }
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetInventoryItem(unit, slot [, nameOnly]) -> hasItem, hasCooldown, repairCost
    // (`0x532ee0`), three as `PaperDollFrame.lua:741` reads them; the arity is that consumer's,
    // not byte-read as `SetBagItem`'s two is (`0x534985`). Unit-keyed through `Model::inv_slot`:
    // an inspected item (`InspectPaperDollFrame.xml:20`) has no durability or creator, so those
    // lines do not show, as in the reference. An armed shopping tooltip renders the compare
    // shape; the arm is consumed either way. repairCost is the item's cost (`0x5332fb`), 0 for an
    // inspected unit's.
    m.set(
        "SetInventoryItem",
        lua.create_function(
            |lua, (this, unit, slot, name_only): (Table, String, usize, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // `nameOnly` (usage string `0x8552dc`) is set only by a number above 0
                // (`0x533027..0x53304c`); anything else builds the ordinary tooltip, without an
                // error. Three of the binding's four builder legs pass it on, not `0x533106`.
                let name_only = super::binding_abi::positive_number_flag(lua, name_only)?;
                let (item_id, inst, repair_cost, currently_equipped, stack) = {
                    let mut model = crate::script::model_mut_of(lua);
                    let armed = match super::tooltip::tip_mut(&mut model, h) {
                        Ok(t) => std::mem::take(&mut t.equipped_header_armed),
                        Err(_) => false,
                    };
                    let view = model
                        .inv_slot(&unit, slot)
                        .filter(|s| s.item_id != 0)
                        .map(|s| {
                            (
                                s.item_id,
                                s.count,
                                render::ItemInstance {
                                    // App-composed, with the suffix off
                                    // `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
                                    name: s.name.clone(),
                                    durability: s.durability,
                                    creator: s.creator.clone(),
                                    has_text: false,
                                    flags: s.flags,
                                    already_bound: s.already_bound,
                                    // A charter has `InventoryType` 0 and is never equipped.
                                    petition: None,
                                    enchants: s.enchants.clone(),
                                    // `0x532ee0` has p6=0 legs too (`0x533106`, `0x5332ad`), but
                                    // nothing openable is equippable.
                                    openable_source: false,
                                    duration_ms: s.duration_ms,
                                },
                            )
                        });
                    let cost = if unit.eq_ignore_ascii_case("player") {
                        model.inv_repair_cost(slot)
                    } else {
                        0
                    };
                    match view {
                        Some((id, stack, inst)) => (id, inst, cost, armed, stack),
                        // An empty slot answers nil and still pushes the other two: callers
                        // destructure all three first, and an addon may add up repairCost.
                        None => {
                            return Ok(MultiValue::from_vec(vec![
                                Value::Nil,
                                Value::Nil,
                                Value::Integer(0),
                            ]))
                        }
                    }
                };
                // Ours is an item object, 1-23 worn; an inspected one a block (`0x62e050`).
                let sell = if unit.eq_ignore_ascii_case("player") {
                    Sell::Object {
                        stack,
                        repair_cost,
                        worn: slot <= 23,
                    }
                } else {
                    Sell::ONE
                };
                let flags = BuilderFlags {
                    currently_equipped,
                    name_only,
                    sell,
                };
                {
                    let mut model = crate::script::model_mut_of(lua);
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                match view_of(lua, item_id) {
                    Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                    None => retrieving(lua, &this)?,
                }
                show_or_hide_empty(lua, h);
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(1),
                    // hasCooldown is the builder's `[ebp-0x38]`, set by four lines:
                    // LOCKED_WITH_ITEM, the temporary-enchant countdown, the item's duration
                    // (`0x52ce0d`) and ITEM_COOLDOWN_TIME. Only the duration is answered here.
                    if inst.duration_ms.is_some() {
                        Value::Boolean(true)
                    } else {
                        Value::Nil
                    },
                    Value::Integer(i64::from(repair_cost)),
                ]))
            },
        )?,
    )?;

    // GameTooltip:SetBagItem(bag, slot) -> hasCooldown, repairCost (`0x62f420`): the real-instance
    // hover, an item object to the money block. repairCost shows in repair mode.
    m.set(
        "SetBagItem",
        lua.create_function(|lua, (this, bag, slot): (Table, i64, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, count, has_cd, inst, repair_cost) = {
                let model = crate::script::model_mut_of(lua);
                let cost = model.repair_costs.bags.get(&(bag, slot)).copied();
                match model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .filter(|s| s.item_id != 0)
                {
                    // `0x534620` takes the p6=1 leg iff the cooldown query `0x6e2ed0` reports
                    // enable, start and duration all non-zero; that one boolean is both the
                    // `hasCooldown` return and the inverse of the openable gate.
                    Some(s) => {
                        let has_cd = s
                            .cooldown
                            .is_some_and(|(start, dur, en)| en && start > 0 && dur > 0);
                        (
                            s.item_id,
                            s.count.max(1),
                            has_cd,
                            render::ItemInstance {
                                // The link's name: the reference builds both from `0x5d8b00`.
                                name: s.link.as_deref().and_then(link_name).map(str::to_string),
                                durability: s.durability,
                                creator: s.creator.clone(),
                                has_text: s.readable,
                                flags: s.flags,
                                already_bound: s.already_bound,
                                petition: s.petition.clone(),
                                enchants: s.enchants.clone(),
                                // p6 = 0 iff no cooldown runs; mid-cooldown the reference shows
                                // ITEM_COOLDOWN_TIME instead, which is not built.
                                openable_source: !has_cd,
                                duration_ms: s.duration_ms,
                            },
                            cost.unwrap_or(0),
                        )
                    }
                    None => return Ok(MultiValue::from_vec(vec![Value::Nil])),
                }
            };
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            match view_of(lua, item_id) {
                Some(v) => {
                    let flags = BuilderFlags {
                        sell: Sell::Object {
                            stack: count,
                            repair_cost,
                            worn: false,
                        },
                        ..Default::default()
                    };
                    render_view(lua, &this, &v, flags, Some(&inst))?;
                }
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(MultiValue::from_vec(vec![
                // The builder's `[ebp-0x38]`: a running cooldown or the item's duration line.
                Value::Boolean(has_cd || inst.duration_ms.is_some()),
                Value::Integer(i64::from(repair_cost)),
            ]))
        })?,
    )?;

    // GameTooltip:SetTradePlayerItem(slot) / SetTradeTargetItem(slot) (`0x5341e0`, `0x534410`;
    // `TradeFrame.xml:150`, `TradeFrame.xml:106`), no returns: the slot's item, the target side
    // off the partner's slot guids (`[0xb715a0 + slot*4]`). An empty or out-of-range slot leaves
    // the tooltip untouched. The enchant line is not rendered: the trade view has no enchant id.
    for (name, player_side) in [("SetTradePlayerItem", true), ("SetTradeTargetItem", false)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, slot): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let item = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    model.trade.as_ref().and_then(|t| {
                        let side = if player_side { &t.player } else { &t.target };
                        slot.checked_sub(1)
                            .and_then(|n| side.slots.get(n))
                            .and_then(|s| s.clone())
                    })
                };
                let Some(item) = item.filter(|i| i.item_id != 0) else {
                    return Ok(());
                };
                {
                    let mut model = crate::script::model_mut_of(lua);
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                let inst = render::ItemInstance {
                    name: item.name.clone(),
                    ..Default::default()
                };
                // Ours is an item object (`0x62eff0`), theirs a block of its count (`0x62f1e0`).
                let sell = if player_side {
                    Sell::Object {
                        stack: item.count,
                        repair_cost: 0,
                        worn: false,
                    }
                } else {
                    Sell::Block {
                        stack: item.count,
                        max: None,
                    }
                };
                let flags = BuilderFlags {
                    sell,
                    ..Default::default()
                };
                match view_of(lua, item.item_id) {
                    Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                    None => retrieving(lua, &this)?,
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetLootItem(slot): the loot-row hover (`0x533470`, `LootFrame.xml:42`, behind
    // `LootSlotIsItem`). A block source (`0x533564`, p6=1): a loot slot is no item object, so a
    // rolled suffix's enchants reach the builder through the block, never through
    // `ITEM_FIELD_ENCHANTMENT`. `slot` is the 1-based row; coin and cleared rows have no tooltip.
    m.set(
        "SetLootItem",
        lua.create_function(|lua, (this, slot): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let row = {
                let model = crate::script::model_mut_of(lua);
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .and_then(|r| r.clone())
                    .filter(|r| !r.is_coin && r.item_id != 0)
            };
            let Some(row) = row else {
                return Ok(());
            };
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The row's app-composed name has the suffix joined, as `0x5d8b00` does; the lines
            // come from the roll id, the reference's `+0x424` (`0x52b7bf`).
            let inst = render::ItemInstance {
                name: row.name.clone(),
                enchants: roll_enchants(lua, row.random_property_id),
                ..Default::default()
            };
            // The slot's count as the block's stack (`0x62e520`).
            let flags = BuilderFlags {
                sell: Sell::Block {
                    stack: row.quantity,
                    max: None,
                },
                ..Default::default()
            };
            match view_of(lua, row.item_id) {
                Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetLootRollItem(rollId): the roll window's hover (`0x5364a0`,
    // `LootFrame.xml:343`). Like `SetLootItem`: p6=1 with `+0x424` = `[roll+0x20]`, the roll's
    // randomPropertyId, every enchant slot zero and no item object, so the suffix-row copy
    // (`0x52b7bf`) is the only enchant source.
    m.set(
        "SetLootRollItem",
        lua.create_function(|lua, (this, roll_id): (Table, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let entry = {
                let model = crate::script::model_mut_of(lua);
                model
                    .loot_rolls
                    .rolls
                    .iter()
                    .find(|r| r.roll_id == roll_id)
                    .cloned()
                    .filter(|r| r.item_id != 0)
            };
            let Some(entry) = entry else {
                return Ok(());
            };
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            let inst = render::ItemInstance {
                name: entry.name.clone(),
                enchants: roll_enchants(lua, entry.random_property_id),
                ..Default::default()
            };
            let flags = BuilderFlags {
                sell: Sell::Block {
                    stack: entry.quantity,
                    max: None,
                },
                ..Default::default()
            };
            match view_of(lua, entry.item_id) {
                Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetMerchantCompareItem(index [, offset]) (`0x536080`, method-table entry 43):
    // the shopping tooltip beside a vendor row. `offset` counts candidate slots, 1-based: set in
    // `0x809200[InventoryType]`, occupied, and holding an item of the offered class (never
    // subclass), so a worn shield makes offset 2 nil for a one-hand weapon and a worn wand compares
    // against a bow. One value on every non-raising path: the number 1 (`lua_pushnumber`) or nil.
    // A bad `this` or a non-number index raises; a missing `offset` is 1, one of 0 or less is nil.
    // An uncached template answers nil and fires the query, as in the reference.
    m.set(
        "SetMerchantCompareItem",
        lua.create_function(
            |lua, (this, index, offset): (Table, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        // `lua_isnumber` accepts a numeric string, here as everywhere.
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                // The one argument whose absence raises, with the reference's usage text.
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetMerchantCompareItem(\"slot\" [, offset])",
                    ));
                };
                // Re-based in f64, then truncated toward zero, as the reference does: 0.5 is row 1.
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                // `offset` is truncated before it is re-based; absent, it is 1.
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    model
                        .merchant
                        .as_ref()
                        .and_then(|m| m.items.get(row))
                        .map(|it| it.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };

                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetAuctionCompareItem("type", index [, offset]) (`0x535d70`): the vendor
    // compare's body (`0x53603e` and `0x5362d4` both pass p4 = 0, p5 = 1) over an auction list.
    // `type` is `lua_isstring`-gated (`0x535e28`); a non-string type or non-number index raises
    // the reference's usage, and a bad list, row or uncached template is nil. The number 1 on
    // success, which `Blizzard_AuctionUI.lua:1081` gates each shopping tooltip on.
    m.set(
        "SetAuctionCompareItem",
        lua.create_function(
            |lua, (this, kind, index, offset): (Table, Value, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                let kind = match &kind {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Integer(i) => i.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                        ))
                    }
                };
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
                };
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    let list = super::auction::list_index_of(&kind);
                    model
                        .auction
                        .as_ref()
                        .zip(list)
                        .and_then(|(a, l)| a.lists[l].rows.get(row))
                        .map(|r| r.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };
                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetMerchantItem(index) / SetBuybackItem(index): template sources. A vendor's own
    // goods skip the sell price (`0x62ed70`); a buyback row is a priced object (`0x62ff60`).
    for (method, buyback) in [("SetMerchantItem", false), ("SetBuybackItem", true)] {
        m.set(
            method,
            lua.create_function(move |lua, (this, index): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let row = {
                    let model = crate::script::model_mut_of(lua);
                    let Some(merchant) = &model.merchant else {
                        return Ok(());
                    };
                    let list = if buyback {
                        &merchant.buyback
                    } else {
                        &merchant.items
                    };
                    list.get(index.saturating_sub(1)).cloned()
                };
                let Some(row) = row else { return Ok(()) };
                {
                    let mut model = crate::script::model_mut_of(lua);
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                let flags = BuilderFlags {
                    sell: if buyback {
                        Sell::Object {
                            stack: row.quantity,
                            repair_cost: 0,
                            worn: false,
                        }
                    } else {
                        Sell::Skip
                    },
                    ..Default::default()
                };
                // Nothing to arm: the vendor tab's row seats `ShoppingTooltip1/2` itself, and the
                // buyback tab seats none.
                match view_of(lua, row.item_id) {
                    Some(v) => render_view(lua, &this, &v, flags, None)?,
                    None => retrieving(lua, &this)?,
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetMerchantCostItem(index, itemIndex) (`0x62ee70`): the itemIndex-th item a
    // row's extended cost asks for, built with the sell price skipped. A non-number argument
    // raises; an invalid row or item index leaves the tooltip as it is.
    m.set(
        "SetMerchantCostItem",
        lua.create_function(|lua, (this, index, item_index): (Table, Value, Value)| {
            let as_index = |v: &Value| match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Number(n) => Some(*n),
                Value::String(s) => s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()),
                _ => None,
            };
            let (Some(index), Some(item_index)) = (as_index(&index), as_index(&item_index)) else {
                return Err(mlua::Error::runtime(
                    "Invalid merchant slot in SetMerchantCostItem",
                ));
            };
            let cost_item = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let at = |n: f64| usize::try_from((n - 1.0).round() as i64).ok();
                at(index).zip(at(item_index)).and_then(|(row, item)| {
                    model
                        .merchant
                        .as_ref()?
                        .items
                        .get(row)?
                        .extended_cost
                        .as_ref()?
                        .items
                        .get(item)
                        .map(|c| c.item_id)
                })
            };
            let Some(item_id) = cost_item else {
                return Ok(());
            };
            render_by_id(lua, &this, item_id, Sell::Skip)
        })?,
    )?;

    // GameTooltip:SetInboxItem(index): the mail item hover (`MailFrame.lua:218`,
    // `MailFrame.lua:470`). A row with no item does nothing; stock calls it only when there is one.
    m.set(
        "SetInboxItem",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name, stack) = {
                let model = crate::script::model_mut_of(lua);
                let Some(mail) = &model.mail else {
                    return Ok(());
                };
                match mail.inbox.get(index.saturating_sub(1)) {
                    Some(r) => (
                        r.item_id,
                        r.item_random_property_id,
                        r.item_name.clone(),
                        r.item_count,
                    ),
                    None => (0, 0, None, 0),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`0x5355fa`, p6=1) carrying the attachment's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            // The attachment's count as the block's stack (`0x62f9e0`).
            let flags = BuilderFlags {
                sell: Sell::Block { stack, max: None },
                ..Default::default()
            };
            match view_of(lua, item_id) {
                Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionItem(type, index): an auction row's hover, `type` "list", "bidder" or
    // "owner". No creator, gift or open line: the reference zeroes the guid arguments
    // (`0x535a6d`, `0x535a73`), so the item-object gate those lines hang off fails.
    m.set(
        "SetAuctionItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = crate::script::model_mut_of(lua);
                let Some(auction) = &model.auction else {
                    return Ok(());
                };
                let Some(list) = super::auction::list_index_of(&kind) else {
                    return Ok(());
                };
                match auction.lists[list].rows.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.random_property_id, r.name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`0x5359d9`, p6=1) carrying the listing's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            // A listing skips the sell price (`0x62fcf0`).
            match view_of(lua, item_id) {
                Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionSellItem(): the sell slot's hover (`Blizzard_AuctionUI.xml:1791`); an
    // empty slot does nothing. The reference resolves the live sell-slot object (`[0xb72608]`,
    // `[0xb7260c]`, `0x5357b4`), so its creator line can show; this renders the template alone.
    m.set(
        "SetAuctionSellItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, stack) = {
                let model = crate::script::model_mut_of(lua);
                model
                    .auction_sell_item
                    .as_ref()
                    .map_or((0, 1), |it| (it.item_id, it.count.unwrap_or(1)))
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The slot's item object (`0x62fc20`).
            let flags = BuilderFlags {
                sell: Sell::Object {
                    stack,
                    repair_cost: 0,
                    worn: false,
                },
                ..Default::default()
            };
            match view_of(lua, item_id) {
                Some(v) => render_view(lua, &this, &v, flags, None)?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetSendMailItem(): the attached item's hover (`MailFrame.xml:952`); an empty
    // slot does nothing.
    m.set(
        "SetSendMailItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, stack) = {
                let model = crate::script::model_mut_of(lua);
                model
                    .mail_send_item
                    .as_ref()
                    .map_or((0, 1), |it| (it.item_id, it.count.unwrap_or(1)))
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The slot's item object (`0x62f8b0`).
            let flags = BuilderFlags {
                sell: Sell::Object {
                    stack,
                    repair_cost: 0,
                    worn: false,
                },
                ..Default::default()
            };
            match view_of(lua, item_id) {
                Some(v) => render_view(lua, &this, &v, flags, None)?,
                None => retrieving(lua, &this)?,
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetTradeSkillItem(skillIndex [, reagentIndex]) (`0x62eae0`): the reagent's
    // item, else the product with its made counts, else the recipe's own spell; a header or a bad
    // reagent raises.
    m.set(
        "SetTradeSkillItem",
        lua.create_function(
            |lua, (this, skill_index, reagent_index): (Table, Value, Value)| {
                let invalid = || {
                    mlua::Error::RuntimeError(
                        "Invalid trade skill item in SetTradeSkillItem(index [,reagent])".into(),
                    )
                };
                let index = lua.coerce_number(skill_index)?.ok_or_else(invalid)? as i64;
                let reagent = lua.coerce_number(reagent_index)?.map(|n| n as i64);
                enum Hover {
                    Item(u32, Sell),
                    Spell(u32),
                }
                let hover = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    let recipe = super::tradeskill::recipe_at(&model, index).ok_or_else(invalid)?;
                    match reagent {
                        Some(ri) => {
                            let r = usize::try_from(ri - 1)
                                .ok()
                                .and_then(|i| recipe.reagents.get(i))
                                .ok_or_else(invalid)?;
                            Hover::Item(
                                r.item,
                                Sell::Block {
                                    stack: r.need,
                                    max: None,
                                },
                            )
                        }
                        None if recipe.creates_item && recipe.product_item != 0 => Hover::Item(
                            recipe.product_item,
                            Sell::Block {
                                stack: recipe.min_made,
                                max: Some(recipe.max_made),
                            },
                        ),
                        None => Hover::Spell(recipe.spell_id),
                    }
                };
                match hover {
                    Hover::Item(item_id, sell) => render_by_id(lua, &this, item_id, sell),
                    Hover::Spell(spell) => super::tooltip_spell::set_spell_by_id(
                        lua,
                        &this,
                        spell,
                        None,
                        Default::default(),
                        None,
                    ),
                }
            },
        )?,
    )?;

    Ok(())
}
