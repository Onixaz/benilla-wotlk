//! The merchant bindings: the app pushes the open vendor ([`UiScript::set_merchant`]), and the
//! Lua verbs queue intents it drains. A row is 1-based; the app maps it to the item entry and row
//! slot `CMSG_BUY_ITEM` needs.
//!
//! `GetMerchantItemInfo(index)` answers `name, texture, price, quantity, numAvailable, isUsable,
//! extendedCost` (`0x584e10`), or `(nil, nil, 0, 1, 0, 1, nil)` for an invalid index. Unlimited
//! stock is `numAvailable` -1; `isUsable` is 1 or nil, the [`super::item_stats::item_usable`] gate
//! behind the extended cost's arena-rating test, and a template still in flight reads usable.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, number_arg};
use super::container::UiCursorMode;
use super::cursor::CursorPayload;

/// One vendor row, resolved by the app; its 1-based index is its place in [`MerchantState::items`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantItem {
    /// The item name; `None` while its template is in flight, answered as nil.
    pub name: Option<String>,
    /// Icon path; `None` while the template is in flight.
    pub texture: Option<String>,
    /// Buy price in copper, already reputation-discounted by the server.
    pub price: u32,
    /// Stack size per purchase (`item_template.buy_count`).
    pub quantity: u32,
    /// Remaining stock, -1 for unlimited (the wire's `0xFFFF_FFFF`).
    pub num_available: i32,
    /// The template entry `CMSG_BUY_ITEM` addresses; Lua never sees it.
    pub item_id: u32,
    /// The hover tooltip's stat head; `None` while the template is in flight.
    pub stats: Option<ItemStatsHead>,
    /// `GetMerchantItemLink`'s or `GetBuybackItemLink`'s answer; `None` while in flight.
    pub link: Option<String>,
    /// The template's `stackable`, `GetMerchantItemMaxStack`'s answer for a row sold singly;
    /// `None` while in flight and on a buyback row.
    pub max_stack: Option<u32>,
    /// The row's `ItemExtendedCost.dbc` record, `None` when the id resolves to none.
    pub extended_cost: Option<ExtendedCost>,
}

/// A row's `ItemExtendedCost.dbc` record as the cost bindings read it (`0x5846e0`, `0x584820`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtendedCost {
    /// Honor points (`+4`).
    pub honor: u32,
    /// Arena points (`+8`).
    pub arena: u32,
    /// The required items with a non-zero id, in record order (`+0x10..+0x20`).
    pub items: Vec<CostItem>,
    /// The record's arena-rating requirement passes `CheckPersonalAndTeamRating` (`0x5a37b0`), or
    /// it has none.
    pub rating_met: bool,
}

/// One item an extended cost asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostItem {
    pub item_id: u32,
    /// How many (`+0x24..+0x34`).
    pub count: u32,
    /// Icon path; `None` while the item's template is in flight.
    pub texture: Option<String>,
    /// The item's link; `None` while in flight.
    pub link: Option<String>,
}

/// An item template's tooltip stat head, resolved per row by the app, which `SetMerchantItem` and
/// `SetBuybackItem` draw while the VM's template is in flight. The reference reads the template in
/// C++; no 1.12 Lua API carries these.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ItemStatsHead {
    /// 0 poor to 6 artifact; colours the tooltip's name line.
    pub quality: u32,
    /// `InventoryType`, the tooltip's slot line ("Main Hand", "Chest"); 0 for none.
    pub inventory_type: u32,
    /// Item class (2 weapon, 4 armor, 6 projectile); with `subclass`, the slot line's right side.
    pub class: u32,
    /// Item subclass within `class` (7 = sword, 1 = cloth, …).
    pub subclass: u32,
    /// Damage block 0's per-hit minimum, 0 for a non-weapon.
    pub dmg_min: f32,
    /// Damage block 0's per-hit maximum.
    pub dmg_max: f32,
    /// Damage block 0's school (0 physical, 1 Holy to 6 Arcane).
    pub dmg_type: u32,
    /// Attack delay in milliseconds; the tooltip's "Speed" is delay / 1000.
    pub delay_ms: u32,
    /// The armor line's value; 0 for none.
    pub armor: u32,
    /// A shield's block line; 0 for none.
    pub block: u32,
    /// What a vendor pays per unit; 0 shows the tooltip's "No sell price" line.
    pub sell_price: u32,
}

/// Each damaged item's repair cost in copper after the vendor's discount (`0x584b20`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepairCosts {
    /// Equipped items by live inventory id.
    pub equipped: std::collections::HashMap<u32, u32>,
    /// Container items by `(bag, slot)`: bags 0-4, the bank vault -1 and bank bags 5-11.
    pub bags: std::collections::HashMap<(i64, u32), u32>,
}

/// One open merchant window, pushed whole by the app; `None` means no vendor is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MerchantState {
    pub items: Vec<MerchantItem>,
    /// Buyback slots, oldest first; the last, the latest sale, is the one the merchant page shows
    /// (`MerchantFrame.lua:133`). From the player's VENDORBUYBACK fields: `price` is the buyback
    /// price, `quantity` the stored stack.
    pub buyback: Vec<MerchantItem>,
    /// Whether this vendor repairs (the `UNIT_NPC_FLAGS` repair bit).
    pub can_repair: bool,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open vendor. Clearing it leaves repair mode, as the
    /// merchant-close handler restores the Point base mode.
    pub fn set_merchant(&mut self, state: Option<MerchantState>) {
        let closing = state.is_none();
        set_field!(self, merchant, state);
        if closing {
            set_field!(self, repair_mode, false);
        }
    }

    /// Push `GetRepairAllCost`'s total in copper and whether a swept item is damaged, swept by the
    /// app ahead of the events that read it.
    pub fn set_repair_all_cost(&mut self, copper: u32, damaged: bool) {
        set_field!(self, repair_all_cost, copper);
        set_field!(self, repair_all_damaged, damaged);
    }

    /// Push the repair costs the item tooltip bindings return.
    pub fn set_repair_costs(&mut self, costs: RepairCosts) {
        set_field!(self, repair_costs, costs);
    }

    /// Drain the `(row, quantity)` buys `BuyMerchantItem` queued, the row 1-based.
    pub fn take_merchant_buys(&mut self) -> Vec<(u32, u32)> {
        drain_field!(self, merchant_buys)
    }

    /// Drain the `(bag, slot)` of each cursor item dropped on the vendor; the app sells it.
    pub fn take_merchant_cursor_sells(&mut self) -> Vec<(i64, u32)> {
        drain_field!(self, merchant_cursor_sells)
    }

    /// Drain the `(bag, slot, row)` of held rows dropped into bags (`CMSG_BUY_ITEM_IN_SLOT`), the
    /// row 1-based.
    pub fn take_merchant_slot_buys(&mut self) -> Vec<(i64, u32, u32)> {
        drain_field!(self, merchant_slot_buys)
    }

    /// Whether `CloseMerchant` was called since the last drain; the close sends no packet.
    pub fn take_merchant_close(&mut self) -> bool {
        drain_field!(self, merchant_close)
    }

    /// Drain the 1-based buyback slots `BuybackItem` queued.
    pub fn take_merchant_buybacks(&mut self) -> Vec<u32> {
        drain_field!(self, merchant_buybacks)
    }

    /// The guild-bank flag of a `RepairAllItems` called since the last drain.
    pub fn take_repair_all(&mut self) -> Option<bool> {
        drain_field!(self, repair_all)
    }

    /// The repair-mode latch: while set, a bag or equipment click repairs the item.
    pub fn repair_mode(&self) -> bool {
        self.model_ref().repair_mode
    }

    /// Leave repair mode, as `HideRepairCursor` does, for the world's right mouse-down, which
    /// resets the Repair base mode to Point.
    pub fn end_repair_mode(&mut self) {
        set_field!(self, repair_mode, false);
    }
}

/// 1 or nil, as the client pushes a usable flag (`pushnumber(1.0)` / `pushnil`).
fn usable_value(usable: bool) -> Value {
    if usable {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// `GetBuybackItemInfo`'s invalid-index six-tuple (`0x585070`); `GetMerchantItemInfo` adds a nil
/// `extendedCost` (`0x584e10`).
fn invalid_tuple() -> Vec<Value> {
    vec![
        Value::Nil,
        Value::Nil,
        Value::Integer(0),
        Value::Integer(1),
        Value::Integer(0),
        Value::Integer(1),
    ]
}

/// The open merchant's row at a 1-based Lua index.
fn merchant_row(model: &super::Model, index: i32) -> Option<&MerchantItem> {
    let row = usize::try_from(index).ok()?.checked_sub(1)?;
    model.merchant.as_ref()?.items.get(row)
}

/// Register the merchant globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetMerchantNumItems",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.items.len()) as i64)
        })?,
    )?;

    g.set(
        "GetMerchantItemInfo",
        lua.create_function(|lua, index: usize| {
            let (item, usable) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let item = model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                    .cloned();
                // A failed arena-rating test reads unusable before the item gate (`0x584fa4`).
                let usable = item.as_ref().is_none_or(|it| {
                    it.extended_cost.as_ref().is_none_or(|c| c.rating_met)
                        && super::item_stats::item_usable_by_id(&model, it.item_id)
                });
                (item, usable)
            };
            let Some(it) = item else {
                let mut tuple = invalid_tuple();
                tuple.push(Value::Nil);
                return Ok(MultiValue::from_vec(tuple));
            };
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(it.price)),
                Value::Integer(i64::from(it.quantity)),
                Value::Integer(i64::from(it.num_available)),
                usable_value(usable),
                usable_value(it.extended_cost.is_some()),
            ]))
        })?,
    )?;

    // GetMerchantItemCostInfo(index) (`0x5846e0`): honor, arena points and the count of required
    // items, `0, 0, 0` for a row with no record or an invalid index.
    g.set(
        "GetMerchantItemCostInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetMerchantItemCostInfo(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (honor, arena, items) = merchant_row(&model, index)
                .and_then(|it| it.extended_cost.as_ref())
                .map_or((0, 0, 0), |c| (c.honor, c.arena, c.items.len()));
            Ok((i64::from(honor), i64::from(arena), items as i64))
        })?,
    )?;

    // GetMerchantItemCostItem(index, itemIndex) (`0x584820`): the itemIndex-th required item as
    // texture, count and link; its count alone after a nil while its template is in flight, and
    // `nil, 0` for any miss.
    g.set(
        "GetMerchantItemCostItem",
        lua.create_function(|lua, (index, item_index): (Value, Value)| {
            let usage = "Usage: GetMerchantItemCostItem(index, itemIndex)";
            let index = number_arg(lua, index, usage)?;
            let item_index = number_arg(lua, item_index, usage)?;
            let cost = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                usize::try_from(item_index)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| {
                        merchant_row(&model, index)?
                            .extended_cost
                            .as_ref()?
                            .items
                            .get(i)
                            .cloned()
                    })
            };
            let Some(cost) = cost else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Integer(0)]));
            };
            let count = Value::Integer(i64::from(cost.count));
            Ok(MultiValue::from_vec(match (cost.texture, cost.link) {
                (Some(texture), Some(link)) => vec![
                    Value::String(lua.create_string(&texture)?),
                    count,
                    Value::String(lua.create_string(&link)?),
                ],
                _ => vec![Value::Nil, count],
            }))
        })?,
    )?;

    // GetMerchantItemLink(index): nil out of range and while in flight; both stock row-click
    // arms accept a nil (`MerchantFrame.lua:303`, `:306`).
    g.set(
        "GetMerchantItemLink",
        lua.create_function(|lua, index: usize| {
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.items.get(n)))
                    .and_then(|it| it.link.clone())
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetMerchantItemMaxStack(index) (`0x5842d0`): the template's stack size, or 1 for a row sold
    // in bundles (`[row+0x18] > 1`) and for every miss: out of range, no vendor, a template in
    // flight. A right shift-click, or a left one with the chat box closed, asks it and
    // opens the stack-split spinner only above 1 (`MerchantFrame.lua:313`, `:340`).
    g.set(
        "GetMerchantItemMaxStack",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetMerchantItemMaxStack(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // Signed, as the reference reads both.
            let max_stack = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|n| model.merchant.as_ref()?.items.get(n))
                .filter(|it| it.quantity as i32 <= 1)
                .and_then(|it| it.max_stack)
                .map_or(1, |n| n as i32);
            Ok(i64::from(max_stack))
        })?,
    )?;

    // BuyMerchantItem(index[, quantity]) (`0x5854c0`): a quantity above 0 is capped at 255, any
    // other is 1.
    g.set(
        "BuyMerchantItem",
        lua.create_function(|lua, (index, quantity): (u32, Option<f64>)| {
            let quantity = quantity
                .map(f64::trunc)
                .filter(|q| *q > 0.0)
                .map_or(1, |q| q.min(255.0) as u32);
            let mut model = crate::script::model_mut_of(lua);
            model.merchant_buys.push((index, quantity));
            Ok(())
        })?,
    )?;

    // PickupMerchantItem(index): two verbs in one (`0x5853a0`), forked on `GetCursorItem`, which
    // answers for mode 1 only; stock never checks `CursorHasItem()` first. A bag item on the
    // cursor is sold, index ignored and no merchant-open gate, which stock `MerchantFrame.xml`'s
    // `PickupMerchantItem(0)` relies on; otherwise row `index - 1` is grabbed as cursor mode 5.
    //
    // Every refusal is silent and clears the cursor: a non-number (a numeric string passes),
    // `index < 1`, `index > count`, no merchant, a dead row, or the held row again (the
    // toggle-off). Neither stock nor the item cache is tested: a sold-out row grabs, and the
    // server refuses the buy.
    g.set(
        "PickupMerchantItem",
        lua.create_function(|lua, index: Option<f64>| {
            let mut model = crate::script::model_mut_of(lua);

            // The sell fork comes first, before the argument is read. Its clear is the sale's, not
            // `ClearCursor`: `CURSOR_UPDATE` fires and the source slot stays greyed until the
            // server's inventory update.
            if let Some(item) = crate::script::cursor::take_cursor_item_for_sale(&mut model) {
                model.merchant_cursor_sells.push((item.bag, item.slot));
                return Ok(());
            }

            // Every other leg clears with the real `ClearCursor(1,1)`, not a bare drop: a held
            // spell fires `CURSOR_UPDATE` and hides the bar grid, and an armed gift wrap is
            // cancelled.
            let held = match &model.cursor {
                Some(CursorPayload::Merchant(m)) => Some(m.row),
                _ => None,
            };
            crate::script::cursor::clear_cursor(&mut model);

            let Some(index) = index else { return Ok(()) };
            // `_ftol` truncates toward zero, then the bound applies signed. The upper bound is
            // load-bearing: `2^32 + 1` would narrow to row 0, where the reference bounds against
            // the vendor count before any narrowing.
            let row = index.trunc();
            if row.is_nan() || row < 1.0 || row > f64::from(u32::MAX) {
                return Ok(());
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let row0 = row as u32 - 1;
            let Some(merchant) = model.merchant.as_ref() else {
                return Ok(());
            };
            let Some(item) = merchant.items.get(row0 as usize) else {
                return Ok(());
            };
            // The toggle-off: naming the held row puts nothing back.
            if held == Some(row0) {
                return Ok(());
            }
            let payload = CursorPayload::Merchant(super::cursor::CursorMerchantItem {
                item_id: item.item_id,
                row: row0,
                texture: item.texture.clone(),
            });
            // The grab's `SignalEvent(CURSOR_UPDATE)`; mode 5 skips the mode-7 `ACTIONBAR_SHOWGRID`
            // branch.
            model.cursor = Some(payload);
            crate::script::cursor::queue_cursor_update(&mut model);
            Ok(())
        })?,
    )?;

    g.set(
        "CloseMerchant",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.merchant_close = true;
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBuybackItems",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.merchant.as_ref().map_or(0, |m| m.buyback.len()) as i64)
        })?,
    )?;

    // GetBuybackItemInfo(index): six values (`0x585070`), oldest first. isUsable is the item gate
    // over the sold item: one its seller cannot use reads red.
    g.set(
        "GetBuybackItemInfo",
        lua.create_function(|lua, index: usize| {
            let (item, usable) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let item = model
                    .merchant
                    .as_ref()
                    .and_then(|m| index.checked_sub(1).and_then(|n| m.buyback.get(n)))
                    .cloned();
                let usable = item
                    .as_ref()
                    .is_none_or(|it| super::item_stats::item_usable_by_id(&model, it.item_id));
                (item, usable)
            };
            let Some(it) = item else {
                return Ok(MultiValue::from_vec(invalid_tuple()));
            };
            let name = match &it.name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(it.price)),
                Value::Integer(i64::from(it.quantity)),
                Value::Integer(i64::from(it.num_available)),
                usable_value(usable),
            ]))
        })?,
    )?;

    // GetBuybackItemLink(slot) (`0x5852c0`): the sold item's link, or no value at all for an empty
    // slot; a non-number raises.
    g.set(
        "GetBuybackItemLink",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBuybackItemLink(slot)")?;
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                usize::try_from(index)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| model.merchant.as_ref()?.buyback.get(i)?.link.clone())
            };
            Ok(match link {
                Some(link) => MultiValue::from_vec(vec![Value::String(lua.create_string(&link)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    g.set(
        "BuybackItem",
        lua.create_function(|lua, index: u32| {
            let mut model = crate::script::model_mut_of(lua);
            model.merchant_buybacks.push(index);
            Ok(())
        })?,
    )?;

    g.set(
        "CanMerchantRepair",
        lua.create_function(|lua, ()| {
            let can = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.merchant.as_ref().is_some_and(|m| m.can_repair)
            };
            Ok(if can { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // GetRepairAllCost() (`0x585990`) → cost, canRepair: 1 when a swept item is damaged, which
    // enables the repair-all button; a lone 0 away from a vendor that repairs.
    g.set(
        "GetRepairAllCost",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !model.merchant.as_ref().is_some_and(|m| m.can_repair) {
                return Ok(MultiValue::from_vec(vec![Value::Integer(0)]));
            }
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(model.repair_all_cost)),
                usable_value(model.repair_all_damaged),
            ]))
        })?,
    )?;

    // RepairAllItems([guildBank]) (`0x585c90`): nothing unless the open merchant repairs
    // (`UNIT_NPC_FLAGS` bit 12); the flag is `GetBoolOrDefault(1, false)`.
    g.set(
        "RepairAllItems",
        lua.create_function(|lua, args: MultiValue| {
            let guild_bank = binding_abi::bool_or_default(args.front(), false);
            let mut model = crate::script::model_mut_of(lua);
            if model.merchant.as_ref().is_some_and(|m| m.can_repair) {
                model.repair_all = Some(guild_bank);
            }
            Ok(())
        })?,
    )?;

    g.set(
        "ShowRepairCursor",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            // `0x584a60`: a targeting spell or a vendor without repair service leaves the
            // cursor untouched. `ClearCursor(1,1)` returns any held item before mode 0x11 arms.
            if model.spell_targeting || !model.merchant.as_ref().is_some_and(|m| m.can_repair) {
                return Ok(());
            }
            crate::script::cursor::clear_cursor(&mut model);
            model.repair_mode = true;
            model.ui_cursor = None;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    g.set(
        "HideRepairCursor",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).repair_mode = false;
            Ok(())
        })?,
    )?;
    g.set(
        "InRepairMode",
        lua.create_function(|lua, ()| {
            let mode = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .repair_mode;
            Ok(if mode { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // ShowMerchantSellCursor(index) (`0x5856d0`) and ShowBuybackSellCursor(index) (`0x585890`):
    // despite the names, the buy cursor, re-armed each frame an item is hovered. Every fail path
    // returns without `CursorSetMode`; the base-mode Point gate lives app-side. The merchant
    // cursor's reputation, points and cost-item tests are not built: only the copper is.
    g.set(
        "ShowMerchantSellCursor",
        lua.create_function(|lua, index: usize| {
            arm_vendor_cursor(lua, |m| {
                m.items.get(index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;
    g.set(
        "ShowBuybackSellCursor",
        lua.create_function(|lua, index: usize| {
            arm_vendor_cursor(lua, |m| {
                m.buyback.get(index.wrapping_sub(1)).map(|it| it.price)
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

/// Arm the vendor hover cursor from a row's price: Buy(3) if the player can afford it, else the
/// greyed UnableBuy(23); no price leaves the cursor untouched.
fn arm_vendor_cursor(lua: &Lua, price_of: impl FnOnce(&MerchantState) -> Option<u32>) {
    let mut model = crate::script::model_mut_of(lua);
    // The family bails on `IsTargeting` first: an armed spell keeps its cursor.
    if model.spell_targeting {
        return;
    }
    let Some(price) = model.merchant.as_ref().and_then(price_of) else {
        return;
    };
    model.ui_cursor = Some(if model.money >= u64::from(price) {
        UiCursorMode::Buy
    } else {
        UiCursorMode::UnableBuy
    });
    model.ui_cursor_dirty = true;
}

#[cfg(test)]
mod tests {
    use super::{ItemStatsHead, MerchantItem, MerchantState};
    use crate::script::{ContainerSlot, ContainerState, UiScript};

    fn stock() -> MerchantState {
        MerchantState {
            items: vec![
                MerchantItem {
                    name: Some("Refreshing Spring Water".into()),
                    texture: Some("Interface\\Icons\\INV_Drink_18".into()),
                    price: 25,
                    quantity: 5,
                    num_available: -1, // unlimited
                    item_id: 159,
                    stats: Some(ItemStatsHead {
                        quality: 1,
                        ..Default::default()
                    }),
                    link: Some("|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r".into()),
                    max_stack: Some(20),
                    extended_cost: None,
                },
                // In flight: the list arrived, its item template has not.
                MerchantItem {
                    name: None,
                    texture: None,
                    price: 1500,
                    quantity: 1,
                    num_available: 3,
                    item_id: 4540,
                    stats: None,
                    link: None,
                    max_stack: None, // in flight, like `name` above
                    extended_cost: None,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn merchant_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetMerchantItemInfo(1) == nil")
            .unwrap());

        s.set_merchant(Some(stock()));
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 2);
        let (name, texture, price, quantity, num, usable) = s
            .eval::<(String, String, i64, i64, i64, i64)>("return GetMerchantItemInfo(1)")
            .unwrap();
        assert_eq!(name, "Refreshing Spring Water");
        assert_eq!(texture, "Interface\\Icons\\INV_Drink_18");
        assert_eq!((price, quantity, num), (25, 5, -1));
        assert_eq!(usable, 1);

        // In flight: name and texture nil, and usable (the null-record skip).
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(2)\n\
                 return n == nil and t == nil and p == 1500 and u == 1",
            )
            .unwrap());
        // Seven values, the last nil for a copper-only row and for an invalid index.
        assert_eq!(
            s.eval::<i64>("return select('#', GetMerchantItemInfo(1))")
                .unwrap(),
            7
        );
        assert!(s
            .eval::<bool>("return select(7, GetMerchantItemInfo(1)) == nil")
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return select('#', GetMerchantItemInfo(9))")
                .unwrap(),
            7
        );
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u, e = GetMerchantItemInfo(9)\n\
                 return n == nil and t == nil and p == 0 and q == 1 and a == 0 and u == 1 \
                 and e == nil",
            )
            .unwrap());

        assert_eq!(
            s.eval::<String>("return GetMerchantItemLink(1)").unwrap(),
            "|cffffffff|Hitem:159:0:0:0|h[Refreshing Spring Water]|h|r"
        );
        assert!(s
            .eval::<bool>("return GetMerchantItemLink(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetMerchantItemLink(9) == nil")
            .unwrap());
    }

    #[test]
    fn merchant_max_stack_is_one_for_bundles_and_misses() {
        let mut s = UiScript::new().unwrap();
        let max = |s: &mut UiScript, arg: &str| {
            s.eval::<i64>(&format!("return GetMerchantItemMaxStack({arg})"))
                .unwrap()
        };
        assert_eq!(max(&mut s, "1"), 1, "no vendor open");

        let mut state = stock();
        let single = |quantity: u32, max_stack: u32| MerchantItem {
            name: Some("Flask of Oil".into()),
            price: 100,
            quantity,
            num_available: -1,
            item_id: 814,
            max_stack: Some(max_stack),
            ..Default::default()
        };
        state.items.push(single(1, 20));
        state.items.push(single(1, 0));
        state.items.push(single(0x8000_0000, u32::MAX));
        s.set_merchant(Some(state));

        assert_eq!(max(&mut s, "1"), 1, "a bundle row takes no split");
        assert_eq!(max(&mut s, "2"), 1, "a template in flight");
        assert_eq!(max(&mut s, "3"), 20, "a single row answers its stack size");
        assert_eq!(max(&mut s, "4"), 0, "the stack size is not floored");
        assert_eq!(max(&mut s, "5"), -1, "both fields read signed");
        for out_of_range in ["0", "-1", "6", "9"] {
            assert_eq!(max(&mut s, out_of_range), 1, "index {out_of_range}");
        }
        assert_eq!(max(&mut s, "\"3\""), 20);
        assert_eq!(max(&mut s, "3.9"), 20);
        for bad in ["", "nil", "\"x\"", "{}"] {
            let err = s
                .eval::<i64>(&format!("return GetMerchantItemMaxStack({bad})"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("Usage: GetMerchantItemMaxStack(index)"),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn merchant_usable_tracks_the_item_gate() {
        use crate::script::{ItemTemplateView, PlayerReqState};
        let mut s = UiScript::new().unwrap();
        let mut state = stock();
        state.buyback = vec![MerchantItem {
            name: Some("Light Mail Gloves".into()),
            texture: Some("Interface\\Icons\\INV_Gauntlets_05".into()),
            price: 47,
            quantity: 1,
            item_id: 2418,
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        s.set_item_template(
            159, // row 1, level-gated for the test
            ItemTemplateView {
                name: "Refreshing Spring Water".into(),
                required_level: 5,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        s.set_item_template(
            2418,
            ItemTemplateView {
                name: "Light Mail Gloves".into(),
                required_level: 5,
                allowable_class: -1,
                allowable_race: -1,
                ..Default::default()
            },
        );
        let req = |level| PlayerReqState {
            level,
            class_id: 1,
            race_id: 1,
            ..Default::default()
        };
        s.set_player_req_state(req(4));
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetMerchantItemInfo(1)\nreturn u == nil and n ~= nil",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetBuybackItemInfo(1)\nreturn u == nil and n ~= nil",
            )
            .unwrap());
        // Row 2 has no template, so it is usable whatever the player state.
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetMerchantItemInfo(2)\nreturn u == 1")
            .unwrap());
        s.set_player_req_state(req(5));
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetMerchantItemInfo(1)\nreturn u == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("local n, t, p, q, a, u = GetBuybackItemInfo(1)\nreturn u == 1")
            .unwrap());
    }

    #[test]
    fn buy_merchant_item_queues_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("BuyMerchantItem(1)").unwrap(); // default quantity 1
        s.run("BuyMerchantItem(2, 5)").unwrap();
        assert_eq!(s.take_merchant_buys(), vec![(1, 1), (2, 5)]);
        assert!(s.take_merchant_buys().is_empty(), "drained");
    }

    /// `0x5854c0`: a quantity above 0 is capped at 255; 0, a negative or none is 1.
    #[test]
    fn buy_merchant_item_caps_the_quantity() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        for q in ["0", "-3", "300", "2.9"] {
            s.run(&format!("BuyMerchantItem(1, {q})")).unwrap();
        }
        assert_eq!(
            s.take_merchant_buys(),
            vec![(1, 1), (1, 1), (1, 255), (1, 2)]
        );
    }

    /// A row sold for honor, arena points and two items: three tokens and a badge.
    fn token_row() -> MerchantItem {
        MerchantItem {
            name: Some("Battlemaster's Cruelty".into()),
            price: 0,
            quantity: 1,
            num_available: -1,
            item_id: 42133,
            extended_cost: Some(super::ExtendedCost {
                honor: 7000,
                arena: 0,
                items: vec![
                    super::CostItem {
                        item_id: 20560,
                        count: 3,
                        texture: Some("Interface\\Icons\\INV_Jewelry_Amulet_07".into()),
                        link: Some(
                            "|cffffffff|Hitem:20560:0:0:0|h[Alterac Valley Mark of Honor]|h|r"
                                .into(),
                        ),
                    },
                    // In flight.
                    super::CostItem {
                        item_id: 29434,
                        count: 1,
                        texture: None,
                        link: None,
                    },
                ],
                rating_met: true,
            }),
            ..Default::default()
        }
    }

    /// `GetMerchantItemInfo`'s seventh value is 1 for a row with an `ItemExtendedCost` record
    /// (`0x584e10`), and a failed arena-rating test reads unusable.
    #[test]
    fn an_extended_cost_row_reads_its_seventh_value_and_rating_gate() {
        let mut s = UiScript::new().unwrap();
        let mut state = stock();
        state.items.push(token_row());
        s.set_merchant(Some(state.clone()));
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u, e = GetMerchantItemInfo(3)\n\
                 return e == 1 and u == 1 and p == 0",
            )
            .unwrap());

        state.items[2].extended_cost.as_mut().unwrap().rating_met = false;
        s.set_merchant(Some(state));
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u, e = GetMerchantItemInfo(3)\n\
                 return e == 1 and u == nil",
            )
            .unwrap());
    }

    /// `GetMerchantItemCostInfo` (`0x5846e0`) and `GetMerchantItemCostItem` (`0x584820`).
    #[test]
    fn the_cost_bindings_read_the_extended_cost() {
        let mut s = UiScript::new().unwrap();
        let mut state = stock();
        state.items.push(token_row());
        s.set_merchant(Some(state));

        let (honor, arena, items) = s
            .eval::<(i64, i64, i64)>("return GetMerchantItemCostInfo(3)")
            .unwrap();
        assert_eq!((honor, arena, items), (7000, 0, 2));
        let none = s
            .eval::<(i64, i64, i64)>("return GetMerchantItemCostInfo(1)")
            .unwrap();
        assert_eq!(none, (0, 0, 0), "a copper-only row");
        assert_eq!(
            s.eval::<(i64, i64, i64)>("return GetMerchantItemCostInfo(9)")
                .unwrap(),
            (0, 0, 0)
        );

        let (texture, count, link) = s
            .eval::<(String, i64, String)>("return GetMerchantItemCostItem(3, 1)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Jewelry_Amulet_07");
        assert_eq!(count, 3);
        assert!(link.contains("Hitem:20560"));
        assert!(s
            .eval::<bool>(
                "local n = select('#', GetMerchantItemCostItem(3, 2))\n\
                 local t, c = GetMerchantItemCostItem(3, 2)\n\
                 return n == 2 and t == nil and c == 1",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, c = GetMerchantItemCostItem(3, 3)\n\
                 return t == nil and c == 0",
            )
            .unwrap());
        let err = s
            .eval::<i64>("return GetMerchantItemCostItem(3)")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Usage: GetMerchantItemCostItem(index, itemIndex)"));
    }

    #[test]
    fn close_merchant_flags_the_intent() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        assert!(!s.take_merchant_close());
        s.run("CloseMerchant()").unwrap();
        assert!(s.take_merchant_close());
        assert!(!s.take_merchant_close(), "drained");
    }

    #[test]
    fn buyback_reads_and_intents() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumBuybackItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>(
                "local n, t, p, q, a, u = GetBuybackItemInfo(1)\n\
                 return n == nil and t == nil and p == 0 and q == 1 and a == 0 and u == 1",
            )
            .unwrap());

        let mut state = stock();
        state.buyback = vec![MerchantItem {
            name: Some("Worn Dagger".into()),
            texture: Some("Interface\\Icons\\INV_Weapon_ShortBlade_01".into()),
            price: 47,
            quantity: 1,
            stats: Some(ItemStatsHead {
                quality: 1,
                ..Default::default()
            }),
            link: Some("|cffffffff|Hitem:2092:0:0:0|h[Worn Dagger]|h|r".into()),
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        assert_eq!(s.eval::<i64>("return GetNumBuybackItems()").unwrap(), 1);
        assert!(s
            .eval::<String>("return GetBuybackItemLink(1)")
            .unwrap()
            .contains("Hitem:2092"));
        assert_eq!(
            s.eval::<i64>("return select('#', GetBuybackItemLink(2))")
                .unwrap(),
            0,
            "an empty slot pushes nothing"
        );
        let (name, _tex, price): (String, String, i64) =
            s.eval("return GetBuybackItemInfo(1)").unwrap();
        assert_eq!((name.as_str(), price), ("Worn Dagger", 47));

        s.run("BuybackItem(1)").unwrap();
        assert_eq!(s.take_merchant_buybacks(), vec![1]);
        assert!(s.take_merchant_buybacks().is_empty(), "drained");
    }

    /// Away from a vendor that repairs, the total is a lone 0 (`0x585990`).
    #[test]
    fn the_repair_all_total_reads_zero_away_from_a_repairer() {
        let mut s = UiScript::new().unwrap();
        s.set_repair_all_cost(1234, true);
        let lone_zero = "return select('#', GetRepairAllCost()) == 1 and GetRepairAllCost() == 0";
        assert!(s.eval::<bool>(lone_zero).unwrap(), "no vendor");
        s.set_merchant(Some(stock()));
        assert!(
            s.eval::<bool>(lone_zero).unwrap(),
            "a vendor that does not repair"
        );
    }

    /// `RepairAllItems` returns before anything else unless the open merchant repairs
    /// (`0x585c90`, `UNIT_NPC_FLAGS` bit 12): no vendor, or one without the repair flag, queues
    /// nothing.
    #[test]
    fn repair_all_queues_nothing_away_from_a_repairer() {
        let mut s = UiScript::new().unwrap();
        s.run("RepairAllItems()").unwrap();
        assert_eq!(s.take_repair_all(), None, "no vendor");
        s.set_merchant(Some(stock()));
        s.run("RepairAllItems(1)").unwrap();
        assert_eq!(s.take_repair_all(), None, "a vendor that does not repair");
    }

    /// `RepairAllItems(guildBank)` reads its flag through `GetBoolOrDefault`, as the stock
    /// guild-bank button's `RepairAllItems(1)` passes it.
    #[test]
    fn repair_all_carries_the_guild_bank_flag() {
        let mut s = UiScript::new().unwrap();
        let mut state = stock();
        state.can_repair = true;
        s.set_merchant(Some(state));
        for (call, guild) in [
            ("RepairAllItems()", false),
            ("RepairAllItems(nil)", false),
            ("RepairAllItems(1)", true),
            ("RepairAllItems(0)", false),
            ("RepairAllItems(true)", true),
        ] {
            s.run(call).unwrap();
            assert_eq!(s.take_repair_all(), Some(guild), "{call}");
        }
    }

    #[test]
    fn repair_reads_intents_and_mode_latch() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return CanMerchantRepair() == nil").unwrap());

        let mut state = stock();
        state.can_repair = true;
        s.set_merchant(Some(state));
        s.set_repair_all_cost(1234, true);
        assert!(s.eval::<bool>("return CanMerchantRepair() == 1").unwrap());
        assert!(s
            .eval::<bool>("local c, can = GetRepairAllCost()\nreturn c == 1234 and can == 1")
            .unwrap());
        s.set_repair_all_cost(0, false);
        assert!(s
            .eval::<bool>(
                "local n = select('#', GetRepairAllCost())\n\
                 local c, can = GetRepairAllCost()\n\
                 return n == 2 and c == 0 and can == nil",
            )
            .unwrap());

        s.run("RepairAllItems()").unwrap();
        assert_eq!(s.take_repair_all(), Some(false));
        assert_eq!(s.take_repair_all(), None, "drained");
        assert!(
            s.take_sounds().is_empty(),
            "the stock button's OnClick plays ITEM_REPAIR"
        );

        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());
        s.run("ShowRepairCursor()").unwrap();
        assert!(s.repair_mode());
        assert!(s.eval::<bool>("return InRepairMode() == 1").unwrap());
        assert_eq!(s.ui_cursor(), None, "repair clears a stale hover override");
        s.run("HideRepairCursor()").unwrap();
        assert!(!s.repair_mode());

        s.run("ShowRepairCursor()").unwrap();
        s.set_merchant(None);
        assert!(
            !s.repair_mode(),
            "closing the merchant hides the repair cursor"
        );
    }

    #[test]
    fn show_repair_cursor_checks_targeting_and_vendor_then_returns_held_item() {
        let mut s = UiScript::new().unwrap();
        let mut bag = ContainerState {
            num_slots: 1,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            ContainerSlot {
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(bag));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "no vendor cannot arm repair");
        assert!(s.cursor_item().is_some(), "early return keeps held item");

        s.set_merchant(Some(stock()));
        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "non-repair vendor cannot arm repair");
        assert!(s.cursor_item().is_some());

        let mut vendor = stock();
        vendor.can_repair = true;
        s.set_merchant(Some(vendor));
        s.set_spell_targeting(true);
        s.run("ShowRepairCursor()").unwrap();
        assert!(!s.repair_mode(), "targeting spell wins over repair");
        assert!(s.cursor_item().is_some());

        s.set_spell_targeting(false);
        s.run("ShowRepairCursor()").unwrap();
        assert!(s.repair_mode());
        assert!(
            s.cursor_item().is_none(),
            "ClearCursor returned the held item"
        );
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return not locked")
            .unwrap());
        assert!(s.take_container_moves().is_empty());
    }

    /// The targeting arm writes the Cast base mode over Repair and its end restores Point, so
    /// arming ends repair mode for good. The idle feed pushes false every frame, which must leave
    /// repair mode standing.
    #[test]
    fn arming_spell_targeting_ends_repair_mode_and_disarming_does_not_restore_it() {
        let mut s = UiScript::new().unwrap();
        let mut vendor = stock();
        vendor.can_repair = true;
        s.set_merchant(Some(vendor));
        s.run("ShowRepairCursor()").unwrap();
        s.set_spell_targeting(false);
        assert!(s.repair_mode(), "the idle feed keeps repair mode");

        s.set_spell_targeting(true);
        assert!(!s.repair_mode(), "the targeting arm ends repair mode");
        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());

        s.set_spell_targeting(false);
        assert!(
            !s.repair_mode(),
            "ending the targeting restores Point, not Repair"
        );
        assert!(s.eval::<bool>("return InRepairMode() == nil").unwrap());
    }

    #[test]
    fn clearing_the_merchant_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.set_merchant(None);
        assert_eq!(s.eval::<i64>("return GetMerchantNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetMerchantItemInfo(1) == nil")
            .unwrap());
    }

    #[test]
    fn vendor_hover_cursor_gates_on_affordability() {
        use crate::script::UiCursorMode;
        let mut s = UiScript::new().unwrap();

        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), None);

        let mut state = stock(); // row 1 price 25, row 2 price 1500
        state.buyback = vec![MerchantItem {
            price: 47,
            ..Default::default()
        }];
        s.set_merchant(Some(state));
        s.set_money(100); // affords row 1 (25) + buyback (47), not row 2 (1500)

        s.run("ShowMerchantSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
        s.run("ShowMerchantSellCursor(2)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::UnableBuy));

        s.run("ShowInspectCursor()").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Inspect));
        s.run("ResetCursor()").unwrap();
        assert_eq!(s.ui_cursor(), None);

        s.run("ShowBuybackSellCursor(1)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));

        // Out of range leaves the armed cursor as it was.
        s.run("ShowMerchantSellCursor(99)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
    }
    #[test]
    fn pickup_merchant_item_grabs_a_row_as_mode_5() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));

        s.run("PickupMerchantItem(1)").unwrap();
        // Mode 5 is invisible to Lua: every stock `CursorHasItem()` gate stays closed.
        assert!(
            s.eval::<bool>("return not CursorHasItem()").unwrap(),
            "CursorHasItem is nil for mode 5"
        );
        assert!(s.eval::<bool>("return not CursorHasSpell()").unwrap());

        // Yet it is held: a second call toggles it off, a third re-grabs.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem(1)").unwrap();
        assert!(s.take_merchant_slot_buys().is_empty(), "no drop yet");
    }

    #[test]
    fn pickup_merchant_item_refuses_silently_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));

        for bad in [
            "PickupMerchantItem(0)",
            "PickupMerchantItem(-1)",
            "PickupMerchantItem(99)",
        ] {
            s.run("PickupMerchantItem(1)").unwrap(); // hold something first
            s.run(bad)
                .unwrap_or_else(|e| panic!("{bad} must not raise: {e}"));
            assert!(s.cursor_payload().is_none(), "{bad} cleared the cursor");
        }
        // A missing argument is the same silent clear.
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupMerchantItem()").unwrap();
        // `2^32 + 1` narrows to row 0 on a naive cast.
        s.run("PickupMerchantItem(4294967297)").unwrap();
        assert!(s.cursor_payload().is_none());
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "the wrapping index grabbed nothing"
        );
        // A numeric string is accepted, and 1.9 truncates to the held row 1, toggling it off.
        s.run(r#"PickupMerchantItem("1")"#).unwrap();
        s.run("PickupMerchantItem(1.9)").unwrap();
        assert!(s.cursor_payload().is_none());
    }

    #[test]
    fn pickup_merchant_item_sells_what_the_cursor_holds() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            bag: 0,
            slot: 7,
            item_id: 4540,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
            bar_placeable: false,
        }));

        s.run("PickupMerchantItem(0)").unwrap();
        assert_eq!(
            s.take_merchant_cursor_sells(),
            vec![(0, 7)],
            "index 0 is ignored — the cursor decides"
        );
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
        assert!(s.take_merchant_cursor_sells().is_empty(), "drained");
    }

    #[test]
    fn a_held_vendor_row_dropped_in_a_bag_is_a_buy() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert_eq!(
            s.take_merchant_slot_buys(),
            vec![(0, 3, 1)],
            "the held row, aimed at the dropped-on slot"
        );
        assert!(
            s.cursor_payload().is_none(),
            "the cursor clears on the drop"
        );
    }

    /// Deviation: a stale row drops without buying on all three drop paths, because two of the
    /// reference's three dereference a null here (a fresh `SMSG_LIST_INVENTORY` rewrites the count
    /// without clearing the cursor); we take the third path's behaviour.
    #[test]
    fn a_stale_vendor_row_drops_without_buying() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(2)").unwrap();
        // The list is replaced while the cursor holds row 2 of the old one.
        let mut shorter = stock();
        shorter.items.truncate(1);
        s.set_merchant(Some(shorter));
        s.run("PickupContainerItem(0, 3)").unwrap();
        assert!(
            s.take_merchant_slot_buys().is_empty(),
            "no buy goes out for a row that no longer resolves"
        );
        assert!(s.cursor_payload().is_none());
    }

    #[test]
    fn the_action_bar_refuses_a_vendor_row_and_keeps_it_held() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.run("PickupMerchantItem(1)").unwrap();
        s.run("PlaceAction(1)").unwrap();
        // Still held: dropping it in a bag afterwards buys.
        s.run("PickupContainerItem(0, 5)").unwrap();
        assert_eq!(s.take_merchant_slot_buys(), vec![(0, 5, 1)]);
    }

    /// Count the cursor's three events from Lua, the way a stock handler would see them.
    fn listen_cursor_events(s: &mut UiScript) {
        s.run(
            r#"
            updates, shows, hides = 0, 0, 0
            local f = CreateFrame("Frame", "MerchantCursorListener")
            f:RegisterEvent("CURSOR_UPDATE")
            f:RegisterEvent("ACTIONBAR_SHOWGRID")
            f:RegisterEvent("ACTIONBAR_HIDEGRID")
            f:SetScript("OnEvent", function()
                if event == "CURSOR_UPDATE" then updates = updates + 1 end
                if event == "ACTIONBAR_SHOWGRID" then shows = shows + 1 end
                if event == "ACTIONBAR_HIDEGRID" then hides = hides + 1 end
            end)
            "#,
        )
        .unwrap();
    }

    /// `(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID)` delivered so far.
    fn cursor_event_counts(s: &mut UiScript) -> (i64, i64, i64) {
        s.tick(0.01);
        let n = |g: &str| s.eval::<i64>(&format!("return {g}")).unwrap();
        (n("updates"), n("shows"), n("hides"))
    }

    /// The grab goes through the cursor setter: its `ClearCursor(1,1)` signals `CURSOR_UPDATE` from
    /// the clear's tail, the set signals it again, and no `ACTIONBAR_SHOWGRID` since mode 5 is not
    /// mode 7.
    #[test]
    fn a_vendor_grab_fires_cursor_update_but_not_the_bar_grid() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            cursor_event_counts(&mut s),
            (2, 0, 0),
            "(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID) after a vendor grab"
        );
        // The toggle-off is a plain clear: one more CURSOR_UPDATE, no HIDEGRID.
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(cursor_event_counts(&mut s), (3, 0, 0));
    }

    #[test]
    fn a_spell_dropped_on_the_vendor_is_a_real_clear() {
        use crate::script::cursor::{self, CursorPayload, CursorSpell};
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        listen_cursor_events(&mut s);
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        cursor::queue_cursor_update(&mut s.model_mut()); // the pickup's own transition
        assert_eq!(cursor_event_counts(&mut s), (1, 1, 0), "the spell pickup");

        s.run("PickupMerchantItem(0)").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(
            cursor_event_counts(&mut s),
            (2, 1, 1),
            "(CURSOR_UPDATE, ACTIONBAR_SHOWGRID, ACTIONBAR_HIDEGRID) after the vendor drop"
        );
    }

    /// `ClearCursor` opens with the gift-wrap cancel, and the grab opens with that clear.
    #[test]
    fn a_vendor_grab_cancels_an_armed_gift_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(stock()));
        s.arm_gift_wrap(0, 1);
        s.run("PickupMerchantItem(1)").unwrap();
        assert_eq!(
            s.gift_wrap_armed(),
            None,
            "the grab's ClearCursor cancels the wrap"
        );
    }
}
