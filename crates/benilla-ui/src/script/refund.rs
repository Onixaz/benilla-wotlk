//! Purchase refunds: the refund record the server answers per refundable item (`item+0x3d4`,
//! filled by `0x6d1650`), the bag verbs that read it, the refund send, and the two confirms'
//! answers `EndRefund` (`0x523370`) and `EndBoundTradeable` (`0x5233d0`).
//!
//! The record's window runs on the player's played clock: open while the purchase stamp less the
//! played seconds now, plus two hours, is above 0 (`0x5d8d80`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, number_arg};
use super::socket::{self, Held};
use super::Model;

/// The refund window, two hours of played time.
const REFUND_WINDOW_SECS: i64 = 7200;

/// One item's refund record, as the app pushes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefundView {
    pub money: u32,
    pub honor: u32,
    pub arena: u32,
    /// `(item id, count)` per cost item, wire order; a 0 id is no item.
    pub items: [(u32, u32); 5],
    /// The player's total played seconds at the purchase.
    pub played_at_purchase: u32,
}

/// `GetPlayedTime` (`0x6cf440`): the last total the server said plus the seconds since; 0 before
/// the first answer.
pub(super) fn played_now(model: &Model) -> i64 {
    model.played_time.map_or(0, |(total, at)| {
        i64::from(total) + (model.now - at).max(0.0) as i64
    })
}

/// The seconds left on `item`'s refund window, `None` with no record or a closed window.
pub(super) fn seconds_left(model: &Model, item: u64) -> Option<i64> {
    let record = model.refund_records.get(&item)?;
    let left = i64::from(record.played_at_purchase) - played_now(model) + REFUND_WINDOW_SECS;
    (left > 0).then_some(left)
}

/// A bound item's trade window still runs (`0x708b40`): its create stamp less the played seconds
/// now, plus two hours, above 0.
pub(super) fn bound_trade_open(model: &Model, item: u64) -> bool {
    bound_trade_left(model, item).is_some()
}

/// The seconds left on a bound item's trade window, `None` when it has none or it closed.
pub(super) fn bound_trade_left(model: &Model, item: u64) -> Option<i64> {
    let stamp = *model.bound_trade_stamps.get(&item)?;
    let left = i64::from(stamp) - played_now(model) + REFUND_WINDOW_SECS;
    (left > 0).then_some(left)
}

/// The item `(bag, slot[, isEquipped])` names: with `isEquipped`, `slot` is an inventory slot.
fn held_at<'a>(model: &'a Model, bag: i64, slot: u32, equipped: bool) -> Option<Held<'a>> {
    if equipped {
        let s = model.inventory_slots.get(slot as usize)?.as_ref()?;
        return Some(Held::Worn(slot, s));
    }
    let s = model.containers.get(&bag)?.slots.get(&slot)?;
    Some(Held::Bag(bag, slot, s))
}

/// The record a purchase verb reads: present, enchant-free and in its window, on an item no open
/// loot window takes from (`0x5d8d80`).
fn open_record<'a>(model: &'a Model, held: &Held<'_>) -> Option<(&'a RefundView, i64)> {
    if socket::has_enchantments(held) || held.guid() == model.loot_source {
        return None;
    }
    let left = seconds_left(model, held.guid())?;
    Some((model.refund_records.get(&held.guid())?, left))
}

fn args(lua: &Lua, bag: Value, slot: Value, usage: &'static str) -> mlua::Result<(i64, u32)> {
    let bag = i64::from(number_arg(lua, bag, usage)?);
    let slot = number_arg(lua, slot, usage)?;
    Ok((bag, u32::try_from(slot).unwrap_or(u32::MAX)))
}

impl super::UiScript {
    /// Push one item's refund record, or drop it with `None`.
    pub fn set_refund_record(&mut self, item: u64, record: Option<RefundView>) {
        let differs = self.model_ref().refund_records.get(&item) != record.as_ref();
        if differs {
            let mut model = self.model_mut();
            match record {
                Some(r) => model.refund_records.insert(item, r),
                None => model.refund_records.remove(&item),
            };
        }
    }

    /// Push the bound-but-tradeable items' create stamps.
    pub fn set_bound_trade_stamps(&mut self, stamps: std::collections::HashMap<u64, u32>) {
        set_field!(self, bound_trade_stamps, stamps);
    }

    /// The server's played total, stamped now on the `GetTime` clock.
    pub fn set_played_time(&mut self, total: u32) {
        let now = self.model_ref().now;
        self.model_mut().played_time = Some((total, now));
    }

    /// Take the `ContainerRefundItemPurchase` sends, item guids.
    pub fn take_refund_sends(&mut self) -> Vec<u64> {
        drain_field!(self, refund_sends)
    }
}

/// Register the refund verbs.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetContainerItemPurchaseInfo(bag, slot[, isEquipped]) (`0x5d8d80`) → money, honorPoints,
    // arenaPoints, itemCount, refundSec; nothing without an open record.
    g.set(
        "GetContainerItemPurchaseInfo",
        lua.create_function(|lua, (bag, slot, equipped): (Value, Value, Value)| {
            const USAGE: &str = "Usage: GetContainerItemPurchaseInfo(index, slot[, isEquipped])";
            let (bag, slot) = args(lua, bag, slot, USAGE)?;
            let model = crate::script::model_mut_of(lua);
            let Some(held) = held_at(&model, bag, slot, bool_or_default(Some(&equipped), false))
            else {
                return Ok(MultiValue::new());
            };
            let Some((r, left)) = open_record(&model, &held) else {
                return Ok(MultiValue::new());
            };
            let count = r.items.iter().filter(|&&(_, n)| n > 0).count();
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(r.money)),
                Value::Integer(i64::from(r.honor)),
                Value::Integer(i64::from(r.arena)),
                Value::Integer(count as i64),
                Value::Integer(left),
            ]))
        })?,
    )?;

    // GetContainerItemPurchaseItem(bag, slot, itemIndex[, isEquipped]) (`0x5d8f70`) → texture,
    // count, link of the record's `itemIndex`th cost item with a nonzero id; texture and link are
    // nil while that item's template is unknown. The usage text is the reference's.
    g.set(
        "GetContainerItemPurchaseItem",
        lua.create_function(
            |lua, (bag, slot, index, equipped): (Value, Value, Value, Value)| {
                const USAGE: &str =
                    "Usage: ContainerItemPurchaseItem(bag, slot, itemIndex[, isEquipped])";
                let (bag, slot) = args(lua, bag, slot, USAGE)?;
                let index = number_arg(lua, index, USAGE)?;
                let mut model = crate::script::model_mut_of(lua);
                let equipped = bool_or_default(Some(&equipped), false);
                let cost = {
                    let Some(held) = held_at(&model, bag, slot, equipped) else {
                        return Ok(MultiValue::new());
                    };
                    let Some((r, _)) = open_record(&model, &held) else {
                        return Ok(MultiValue::new());
                    };
                    usize::try_from(index.wrapping_sub(1))
                        .ok()
                        .and_then(|i| r.items.iter().filter(|&&(id, _)| id != 0).nth(i))
                        .copied()
                };
                let Some((id, count)) = cost else {
                    return Ok(MultiValue::new());
                };
                let view = model.item_templates.get(&id).cloned();
                if view.is_none() {
                    model.item_stat_asks.insert(id);
                }
                drop(model);
                Ok(MultiValue::from_vec(vec![
                    match view.as_ref().and_then(|v| v.icon.as_deref()) {
                        Some(icon) => Value::String(lua.create_string(icon)?),
                        None => Value::Nil,
                    },
                    Value::Integer(i64::from(count)),
                    match &view {
                        Some(v) => {
                            Value::String(lua.create_string(super::item_stats::item_link(
                                id, &v.name, v.quality,
                            ))?)
                        }
                        None => Value::Nil,
                    },
                ]))
            },
        )?,
    )?;

    // ContainerRefundItemPurchase(bag, slot[, isEquipped]) (`0x5d91b0`): `CMSG_ITEM_REFUND` for an
    // item with a record while a merchant is open; else the bag-error line. The window is the
    // server's to judge.
    g.set(
        "ContainerRefundItemPurchase",
        lua.create_function(|lua, (bag, slot, equipped): (Value, Value, Value)| {
            const USAGE: &str = "Usage: ContainerRefundItemPurchase(index, slot[, isEquipped])";
            let (bag, slot) = args(lua, bag, slot, USAGE)?;
            let mut model = crate::script::model_mut_of(lua);
            let equipped = bool_or_default(Some(&equipped), false);
            let item = held_at(&model, bag, slot, equipped)
                .map(|h| h.guid())
                .filter(|g| model.refund_records.contains_key(g));
            match item {
                Some(guid) if model.merchant.is_some() => model.refund_sends.push(guid),
                _ => model.ui_errors.push("ERR_INTERNAL_BAG_ERROR"),
            }
            Ok(())
        })?,
    )?;

    // EndRefund(kind) (`0x523370`): the refund-ending confirm's Yes. 1 re-runs the item enchant
    // confirmed, the `BindEnchant` path; 2 accepts the socketing window.
    g.set(
        "EndRefund",
        lua.create_function(|lua, kind: Value| {
            let Some(kind) = lua.coerce_number(kind)? else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            match kind as i64 as i32 {
                1 => model
                    .enchant_confirms
                    .push(super::cursor::EnchantConfirm::Bind),
                2 => socket::accept(&mut model),
                _ => {}
            }
            Ok(())
        })?,
    )?;

    // EndBoundTradeable(kind) (`0x5233d0`): the trade-window-ending confirm's Yes. "itemenchant"
    // re-runs the item enchant confirmed, "gem" accepts the socketing window and "spellenchant"
    // casts the parked weapon imbue.
    g.set(
        "EndBoundTradeable",
        lua.create_function(|lua, kind: Value| {
            let kind =
                super::binding_abi::string_arg(lua, kind, "Usage: EndBoundTradeable(\"type\")")?;
            let mut model = crate::script::model_mut_of(lua);
            if kind.eq_ignore_ascii_case("itemenchant") {
                model
                    .enchant_confirms
                    .push(super::cursor::EnchantConfirm::Bind);
            } else if kind.eq_ignore_ascii_case("gem") {
                socket::accept(&mut model);
            } else if kind.eq_ignore_ascii_case("spellenchant") {
                model
                    .enchant_confirms
                    .push(super::cursor::EnchantConfirm::SpellEnchant);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::RefundView;
    use crate::script::{ContainerSlot, ContainerState, EnchantView, MerchantState, UiScript};

    const ITEM: u64 = 0x4000_0000_0000_0007;

    fn script_with_purchase(enchants: Vec<EnchantView>) -> UiScript {
        let mut s = UiScript::new().unwrap();
        let slot = ContainerSlot {
            guid: ITEM,
            item_id: 40_000,
            count: 1,
            enchants,
            ..Default::default()
        };
        s.set_container(
            0,
            Some(ContainerState {
                num_slots: 16,
                slots: HashMap::from([(1, slot)]),
                ..Default::default()
            }),
        );
        s.set_refund_record(
            ITEM,
            Some(RefundView {
                honor: 2000,
                items: [(29_434, 3), (0, 0), (0, 0), (0, 0), (0, 0)],
                played_at_purchase: 10_000,
                ..Default::default()
            }),
        );
        s
    }

    /// The record answers while `stamp - played + 7200` stays above 0, the seconds left being that
    /// sum; at 0 the window has closed and nothing answers.
    #[test]
    fn purchase_info_reads_the_record_while_the_window_runs() {
        let mut s = script_with_purchase(Vec::new());
        s.set_played_time(10_600);
        let info: (i64, i64, i64, i64, i64) =
            s.eval("return GetContainerItemPurchaseInfo(0, 1)").unwrap();
        assert_eq!(info, (0, 2000, 0, 1, 6600));
        let (_, count): (mlua::Value, i64) = s
            .eval("local t, n = GetContainerItemPurchaseItem(0, 1, 1) return t, n")
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            s.arity("GetContainerItemPurchaseItem(0, 1, 2)").unwrap(),
            0,
            "one cost item"
        );
        s.set_played_time(17_200);
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 0);
    }

    /// The item an open loot window takes from answers nothing (`[0xbfa8d8]`).
    #[test]
    fn a_looted_purchase_answers_nothing() {
        let mut s = script_with_purchase(Vec::new());
        s.set_loot_source(ITEM);
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 0);
        s.set_loot_source(0);
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 5);
    }

    /// A permanent enchant or a gem ends the refund (`0x708ac0`); the third socket is not read.
    #[test]
    fn an_enchanted_purchase_answers_nothing() {
        let enchant = |slot| EnchantView {
            slot,
            id: 1,
            name: "x".into(),
            ..Default::default()
        };
        let mut s = script_with_purchase(vec![enchant(4)]);
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 5);
        s = script_with_purchase(vec![enchant(2)]);
        assert_eq!(s.arity("GetContainerItemPurchaseInfo(0, 1)").unwrap(), 0);
    }

    /// The refund goes out only while a merchant is open; else the bag error line.
    #[test]
    fn the_refund_needs_an_open_merchant() {
        let mut s = script_with_purchase(Vec::new());
        s.run("ContainerRefundItemPurchase(0, 1)").unwrap();
        assert!(s.take_refund_sends().is_empty());
        assert_eq!(s.take_ui_errors(), ["ERR_INTERNAL_BAG_ERROR"]);
        s.set_merchant(Some(MerchantState::default()));
        s.run("ContainerRefundItemPurchase(0, 1)").unwrap();
        assert_eq!(s.take_refund_sends(), [ITEM]);
    }
}
