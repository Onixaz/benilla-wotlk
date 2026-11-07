//! The item verbs that take an id, a name or a link and read the template or the player's items:
//! `GetItemCount` and its neighbours. A string resolves as the name table does (`0x709de0`): a
//! link's id, else an item the player holds by that name.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::{flag, ftol};
use super::item_link::{item_hyperlink, reference_atoi, ItemLinkFields};
use super::item_search::{find_item, held_count, ItemQuery, CARRIED};
use super::item_summary::StatSummary;
use super::model::Model;
use super::{ItemTemplateView, ModelCell};

/// `ITEM_FLAG_UNIQUE_EQUIPPED`, which `GetItemUniqueness` answers as `-1, 1`.
const FLAG_UNIQUE_EQUIPPED: u32 = 0x80000;

/// The name table's read (`0x709de0`): the first `item:` id, else a held item's.
pub(super) fn item_id_by_name(model: &Model, text: &str) -> i64 {
    match text.find("item:") {
        Some(at) => reference_atoi(&text[at + 5..]),
        None => super::socket::item_id_by_name(model, text).map_or(0, i64::from),
    }
}

/// The argument these verbs share: a number is an id, a string goes through the name table, and
/// anything else is id 0.
fn item_arg(lua: &Lua, model: &Model, v: &Value) -> mlua::Result<u32> {
    if let Some(n) = lua.coerce_number(v.clone())? {
        return Ok(u32::try_from(ftol(n)).unwrap_or(0));
    }
    Ok(match v {
        Value::String(s) => {
            u32::try_from(item_id_by_name(model, &s.to_string_lossy())).unwrap_or(0)
        }
        _ => 0,
    })
}

/// A template by argument; a miss records the ask, which the reference's cache read also makes.
fn template_of(lua: &Lua, v: &Value) -> mlua::Result<Option<ItemTemplateView>> {
    let mut model = crate::script::model_mut_of(lua);
    let id = item_arg(lua, &model, v)?;
    let view = model.item_templates.get(&id).cloned();
    if view.is_none() && id != 0 {
        model.item_stat_asks.insert(id);
    }
    Ok(view)
}

/// A Lua boolean argument as the reference's string-or-number read takes it: nil, false, 0 and
/// `"0"` are off.
fn bool_arg(v: &Value) -> bool {
    match v {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(0) => false,
        Value::Number(n) => *n != 0.0,
        Value::String(s) => s.to_string_lossy() != "0",
        _ => true,
    }
}

/// `GetItemStats`'s and `GetItemStatDelta`'s item: a link's template and fields (`0x6207a0`).
fn link_summary(model: &mut Model, link: &str) -> Option<StatSummary> {
    let (id, fields) = ItemLinkFields::parse(link)?;
    match model.item_templates.get(&id) {
        Some(v) => Some(StatSummary::of(model, v, &fields)),
        None => {
            model.item_stat_asks.insert(id);
            None
        }
    }
}

/// Fills `table`, or a new one, as `0x6085d0` does: the DPS, then every named stat.
fn push_stats(lua: &Lua, summary: &StatSummary, table: Option<Table>) -> mlua::Result<Value> {
    let t = match table {
        Some(t) => t,
        None => lua.create_table()?,
    };
    if let Some(dps) = summary.dps() {
        t.set("ITEM_MOD_DAMAGE_PER_SECOND_SHORT", f64::from(dps))?;
    }
    for (key, value) in summary.entries() {
        t.set(key, value)?;
    }
    Ok(Value::Table(t))
}

fn table_arg(v: Value, usage: &str) -> mlua::Result<Option<Table>> {
    match v {
        Value::Nil => Ok(None),
        Value::Table(t) => Ok(Some(t)),
        _ => Err(mlua::Error::runtime(usage.to_string())),
    }
}

fn string_arg(lua: &Lua, v: Value, usage: &str) -> mlua::Result<String> {
    lua.coerce_string(v)?
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| mlua::Error::runtime(usage.to_string()))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetItemCount(item[, includeBank[, includeCharges]]) (`0x51c2e0`): the stacks the search walks.
    // The bank flag adds the bank; the charges flag reads broken items too. Not built: its
    // charge count for an unstackable item.
    g.set(
        "GetItemCount",
        lua.create_function(|lua, (item, bank, charges): (Value, Value, Value)| {
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let id = item_arg(lua, &model, &item)?;
            Ok(i64::from(held_count(
                &model,
                id,
                bool_arg(&bank),
                bool_arg(&charges),
            )))
        })?,
    )?;

    // GetItemIcon(item) (`0x517020`): the icon path. Not built: the `Item.dbc` read, so an uncached
    // id answers nothing and asks.
    g.set(
        "GetItemIcon",
        lua.create_function(|lua, item: Value| {
            Ok(match template_of(lua, &item)?.and_then(|t| t.icon) {
                Some(icon) => MultiValue::from_vec(vec![Value::String(lua.create_string(icon)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetItemFamily(item) (`0x510f20`): the template's bag family mask.
    g.set(
        "GetItemFamily",
        lua.create_function(|lua, item: Value| {
            Ok(match template_of(lua, &item)? {
                Some(t) => MultiValue::from_vec(vec![Value::Integer(i64::from(t.bag_family))]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetItemUniqueness(item) (`0x516ed0`): -1, 1 for unique-equipped, else the limit category and
    // its quantity, else nothing. A non-number non-string raises.
    g.set(
        "GetItemUniqueness",
        lua.create_function(|lua, item: Value| {
            if !matches!(
                item,
                Value::String(_) | Value::Integer(_) | Value::Number(_)
            ) {
                return Err(mlua::Error::runtime(
                    "Usage: GetItemUniqueness(itemID|\"name\"|\"itemlink\")",
                ));
            }
            let Some(t) = template_of(lua, &item)? else {
                return Ok(MultiValue::new());
            };
            if t.flags & FLAG_UNIQUE_EQUIPPED != 0 {
                return Ok(MultiValue::from_vec(vec![
                    Value::Integer(-1),
                    Value::Integer(1),
                ]));
            }
            Ok(match t.item_limit {
                Some((category, quantity)) => MultiValue::from_vec(vec![
                    Value::Integer(i64::from(category)),
                    Value::Integer(i64::from(quantity)),
                ]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetItemGem("name" | "link", index) (`0x51c150`): the gem in socket `index` of 1..3, by the
    // link's fields or the held item's, as name and link.
    g.set(
        "GetItemGem",
        lua.create_function(|lua, (item, index): (Value, Value)| {
            const USAGE: &str = "Usage: GetItemGem(\"name\"|\"itemlink\", index)";
            let index = match lua.coerce_number(index)? {
                Some(n) => ftol(n) - 1,
                None => return Err(mlua::Error::runtime(USAGE)),
            };
            let text = string_arg(lua, item, USAGE)?;
            let Some(i) = usize::try_from(index).ok().filter(|&i| i < 3) else {
                return Ok(MultiValue::new());
            };
            let mut model = crate::script::model_mut_of(lua);
            let enchant = if text.contains("item:") {
                ItemLinkFields::parse(&text).map_or(0, |(_, f)| f.gems[i])
            } else {
                find_item(&model, &ItemQuery::Name(text), 0)
                    .and_then(|h| h.link_fields())
                    .map_or(0, |f| f.gems[i])
            };
            let gem = u32::try_from(enchant)
                .ok()
                .and_then(|id| model.enchant_rows.get(&id))
                .map_or(0, |r| r.gem_item);
            let level = model.player_req.level;
            let Some(t) = model.item_templates.get(&gem).cloned() else {
                if gem != 0 {
                    model.item_stat_asks.insert(gem);
                }
                return Ok(MultiValue::new());
            };
            let link = item_hyperlink(gem, t.quality, &ItemLinkFields::default(), level, &t.name);
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&t.name)?),
                Value::String(lua.create_string(&link)?),
            ]))
        })?,
    )?;

    // GetItemCooldown(item) (`0x510fc0`) → start, duration, enable, as a carried stack's. Not
    // built: a shared cooldown on an item not held.
    g.set(
        "GetItemCooldown",
        lua.create_function(|lua, item: Value| {
            let now = super::clock::now(lua);
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let id = item_arg(lua, &model, &item)?;
            let held = (id != 0)
                .then(|| find_item(&model, &ItemQuery::Id(id), CARRIED))
                .flatten();
            let cooldown = held.and_then(|h| model.container_cooldowns.get(&(h.bag, h.slot)));
            Ok(match cooldown {
                Some(&(start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    // IsConsumableItem(item) (`0x5173e0`): ammo and thrown, or a use spell with a reagent or that
    // uses up a charge; 1 or nil, nil for an uncached id.
    g.set(
        "IsConsumableItem",
        lua.create_function(|lua, item: Value| {
            let Some(t) = template_of(lua, &item)? else {
                return Ok(Value::Nil);
            };
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let source = model.spell_info.clone();
            let consumed = matches!(t.inventory_type, 24 | 25)
                || t.item_spells.iter().any(|s| {
                    s.trigger == 0
                        && s.spell != 0
                        && source
                            .as_ref()
                            .and_then(|src| src.spell_stat_effects(s.spell))
                            .is_some_and(|e| s.charges < 0 || e.has_reagents)
                });
            Ok(flag(consumed))
        })?,
    )?;

    // IsHelpfulItem(item) (`0x5171f0`), IsHarmfulItem(item) (`0x5172e0`): the use spell's intent, 1
    // or nil. Not built: an on-use enchant standing in for a use spell.
    for (name, helpful) in [("IsHelpfulItem", true), ("IsHarmfulItem", false)] {
        g.set(
            name,
            lua.create_function(move |lua, item: Value| {
                let Some(t) = template_of(lua, &item)? else {
                    return Ok(Value::Nil);
                };
                let source = crate::script::model_mut_of(lua).spell_info.clone();
                let intent = t
                    .item_spells
                    .iter()
                    .find(|s| s.trigger == 0 && s.spell != 0)
                    .and_then(|s| source?.spell_stat_effects(s.spell));
                Ok(flag(intent.is_some_and(|e| {
                    if helpful {
                        e.helpful
                    } else {
                        e.harmful
                    }
                })))
            })?,
        )?;
    }

    // IsEquippedItemType("type") (`0x51c740`): an `INVTYPE_` token or its text names an inventory
    // type; anything else an item class or subclass name, against the worn items.
    g.set(
        "IsEquippedItemType",
        lua.create_function(|lua, kind: Value| {
            let Some(kind) =
                super::binding_abi::optional_string(lua, &kind).filter(|k| !k.is_empty())
            else {
                return Err(mlua::Error::runtime("Usage: IsEquippedItemType(\"type\")"));
            };
            let wanted = kind.to_lowercase();
            let inventory_type = (1..29u32).find(|&i| {
                let token = super::item_stats::equip_loc_token(i);
                token.to_lowercase() == wanted
                    || crate::strings::global(lua, token)
                        .is_some_and(|t| t.to_lowercase() == wanted)
            });
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let names =
                |t: &Option<String>| t.as_deref().is_some_and(|n| n.to_lowercase() == wanted);
            let hit = (1..=19).any(|slot| {
                model
                    .inv_slot("player", slot)
                    .filter(|s| s.item_id != 0)
                    .and_then(|s| model.item_templates.get(&s.item_id))
                    .is_some_and(|t| match inventory_type {
                        Some(i) => t.inventory_type == i,
                        None => names(&t.item_type) || names(&t.item_sub_type),
                    })
            });
            Ok(flag(hit))
        })?,
    )?;

    // GetItemStats("link"[, table]) (`0x608690`): the stat table, nothing for no item.
    g.set(
        "GetItemStats",
        lua.create_function(|lua, (link, table): (Value, Value)| {
            const USAGE: &str = "Usage: GetItemStats(itemLink[, statTable])";
            let link = string_arg(lua, link, USAGE)?;
            let table = table_arg(table, USAGE)?;
            let summary = link_summary(&mut crate::script::model_mut_of(lua), &link);
            Ok(match summary {
                Some(s) => MultiValue::from_vec(vec![push_stats(lua, &s, table)?]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetItemStatDelta("link1", "link2"[, table]) (`0x608760`): the first's stats less the second's.
    g.set(
        "GetItemStatDelta",
        lua.create_function(|lua, (first, second, table): (Value, Value, Value)| {
            const USAGE: &str = "Usage: GetItemStatSummary(itemLink1, itemLink2[, statTable])";
            let first = string_arg(lua, first, USAGE)?;
            let second = string_arg(lua, second, USAGE)?;
            let table = table_arg(table, USAGE)?;
            let (a, b) = {
                let mut model = crate::script::model_mut_of(lua);
                (
                    link_summary(&mut model, &first),
                    link_summary(&mut model, &second),
                )
            };
            Ok(match (a, b) {
                (Some(a), Some(b)) => {
                    MultiValue::from_vec(vec![push_stats(lua, &StatSummary::delta(&a, &b), table)?])
                }
                _ => MultiValue::new(),
            })
        })?,
    )?;

    // A registered no-op in the reference (`0x8e5250`).
    g.set(
        "GetExtendedItemInfo",
        lua.create_function(|_, ()| Ok(MultiValue::new()))?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{ItemTemplateView, UiScript};

    fn with_item(view: ItemTemplateView) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_item_template(7000, view);
        s
    }

    #[test]
    fn the_template_verbs_read_by_id_and_link_and_ask_on_a_miss() {
        let mut s = with_item(ItemTemplateView {
            name: "Loop".into(),
            bag_family: 8,
            icon: Some("Interface\\Icons\\X".into()),
            item_limit: Some((12, 3)),
            ..Default::default()
        });
        assert_eq!(s.eval::<i64>("return GetItemFamily(7000)").unwrap(), 8);
        assert_eq!(
            s.eval::<i64>(
                r#"return GetItemFamily("|cffffffff|Hitem:7000:0:0:0:0:0:0:0:80|h[Loop]|h|r")"#
            )
            .unwrap(),
            8
        );
        assert_eq!(
            s.eval::<String>("return GetItemIcon(7000)").unwrap(),
            "Interface\\Icons\\X"
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetItemUniqueness(7000)")
                .unwrap(),
            (12, 3)
        );
        assert!(s.eval::<bool>("return GetItemIcon(7001) == nil").unwrap());
        assert_eq!(s.take_item_stat_asks(), vec![7001]);
    }

    #[test]
    fn a_unique_equipped_flag_answers_before_the_limit_category() {
        let s = with_item(ItemTemplateView {
            flags: super::FLAG_UNIQUE_EQUIPPED,
            item_limit: Some((12, 3)),
            ..Default::default()
        });
        assert_eq!(
            s.eval::<(i64, i64)>("return GetItemUniqueness(7000)")
                .unwrap(),
            (-1, 1)
        );
    }

    #[test]
    fn stats_fill_a_passed_table_and_a_delta_subtracts() {
        let mut s = with_item(ItemTemplateView {
            stats: vec![(3, 10)],
            ..Default::default()
        });
        s.set_item_template(
            7001,
            ItemTemplateView {
                stats: vec![(3, 4), (7, 2)],
                ..Default::default()
            },
        );
        s.run(
            r#"
            t = {keep = 1}
            GetItemStats("item:7000", t)
            d = GetItemStatDelta("item:7000", "item:7001")
            "#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>("return t.ITEM_MOD_AGILITY_SHORT").unwrap(),
            10
        );
        assert_eq!(s.eval::<i64>("return t.keep").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return d.ITEM_MOD_AGILITY_SHORT").unwrap(), 6);
        assert_eq!(
            s.eval::<i64>("return d.ITEM_MOD_STAMINA_SHORT").unwrap(),
            -2
        );
        assert!(s
            .eval::<bool>(r#"return GetItemStats("item:7002") == nil"#)
            .unwrap());
    }
}
