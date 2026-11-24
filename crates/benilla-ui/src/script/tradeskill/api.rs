//! The tradeskill Lua globals, the 36 of the client's registrar (`0xad0e80`), over the app's
//! [`TradeSkillList`] and the game, both read through the lent view at the call.

use std::collections::HashSet;

use mlua::{Lua, MultiValue, Value};

use crate::script::binding_abi::{self, bool_or_default, flag, number_arg};
use crate::script::item_stats::item_link;
use crate::script::live::LiveState;
use crate::script::ModelCell;

use super::{TradeSkillList, TradeSkillRecipeView};

/// The slot menu's `GlobalStrings.lua` token per filter bit (`0xad0e10`).
const INV_SLOT_TOKENS: [&str; 27] = [
    "HEADSLOT",
    "NECKSLOT",
    "SHOULDERSLOT",
    "SHIRTSLOT",
    "CHESTSLOT",
    "WAISTSLOT",
    "LEGSSLOT",
    "FEETSLOT",
    "WRISTSLOT",
    "HANDSSLOT",
    "FINGER0SLOT",
    "FINGER1SLOT",
    "TRINKET0SLOT",
    "TRINKET1SLOT",
    "BACKSLOT",
    "MAINHANDSLOT",
    "SECONDARYHANDSLOT",
    "RANGEDSLOT",
    "TABARDSLOT",
    "BAGSLOT",
    "BAGSLOT",
    "BAGSLOT",
    "BAGSLOT",
    "ENCHSLOT_WEAPON",
    "ENCHSLOT_2HWEAPON",
    "SHIELDSLOT",
    "NONEQUIPSLOT",
];

fn opt_str(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

fn nils(n: usize) -> MultiValue {
    MultiValue::from_vec(vec![Value::Nil; n])
}

/// `f` over the view lent at the call that holds the list; with none, a test's window; `None`
/// with neither.
fn with_live<R>(lua: &Lua, f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    let mut f = Some(f);
    let lent = crate::script::write_live(|l| {
        l.trade_skill()
            .is_some()
            .then(|| (f.take().expect("once"))(l))
    })
    .flatten();
    if lent.is_some() {
        return lent;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let fixture = crate::script::model_mut_of(lua).trade_skill.take();
        if let Some(mut fixture) = fixture {
            let r = (f.take().expect("once"))(&mut *fixture);
            crate::script::model_mut_of(lua).trade_skill = Some(fixture);
            return Some(r);
        }
    }
    let _ = lua;
    None
}

/// `f` over the list at the call, `None` with none.
fn with_list<R>(lua: &Lua, f: impl FnOnce(&TradeSkillList) -> R) -> Option<R> {
    with_live(lua, |l| l.trade_skill().map(f)).flatten()
}

/// Fire `events` in order, each at its step, the view no longer held.
fn raise(lua: &Lua, events: Vec<&'static str>) {
    for e in events {
        crate::script::model_mut_of(lua).raise(e, Vec::new());
    }
}

/// The slot filter pass (`0x5dcc40`) on the list with the text filter read at the call
/// (`0x5dc110`), which only a name or level filter asks.
fn reapply(lua: &Lua, l: &mut dyn LiveState, from_bits: bool) {
    let Some((spells, name, min, max)) = l.trade_skill().map(|list| {
        let (name, min, max) = list.text_filter();
        (
            list.spells().collect::<Vec<_>>(),
            name.map(str::to_string),
            min,
            max,
        )
    }) else {
        return;
    };
    let global = |key: &str| crate::strings::global(lua, key);
    let passes: HashSet<u32> = if name.is_some() || min != 0 || max != 0 {
        spells
            .into_iter()
            .filter(|&s| l.trade_skill_filter_spell(s, min, max, name.as_deref(), &global))
            .collect()
    } else {
        HashSet::new()
    };
    if let Some(list) = l.trade_skill_mut() {
        list.apply(from_bits, &|s| passes.contains(&s));
    }
}

/// The recipe at a 1-based index and its spell record at the call; `None` on a header, past the
/// list or without a record.
pub(crate) fn recipe_at(lua: &Lua, index: i64) -> Option<(u32, TradeSkillRecipeView)> {
    with_live(lua, |l| {
        let spell = l.trade_skill()?.recipe_at(index)?.spell_id;
        Some((spell, l.trade_skill_recipe(spell)?))
    })
    .flatten()
}

/// `lua_isnumber` then the truncating cast every index argument takes.
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    number_arg(lua, v, usage).map(i64::from)
}

/// `lua_isnumber` without a raise: `None` for a non-number.
fn opt_number(lua: &Lua, v: &Value) -> mlua::Result<Option<i32>> {
    Ok(lua.coerce_number(v.clone())?.map(|n| n as i64 as i32))
}

/// `lua_isstring`: a number or a string.
fn is_string(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_))
}

fn error(msg: &str) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

/// The `n`-th set bit (0-based) of `mask` within the 27 slot bits.
fn nth_slot(mask: u32, n: i32) -> Option<u32> {
    let mut count = 0;
    for slot in 0..27 {
        if mask & (1 << slot) != 0 {
            if count == n {
                return Some(slot);
            }
            count += 1;
        }
    }
    None
}

/// A filter verb's write: `edit` on the list returns whether the pass and `event` follow; an
/// error it returns raises instead.
fn filter_edit(
    lua: &Lua,
    event: &'static str,
    edit: impl FnOnce(&mut TradeSkillList) -> mlua::Result<bool>,
) -> mlua::Result<()> {
    let ran = with_live(lua, |l| {
        let changed = match l.trade_skill_mut() {
            Some(list) => edit(list)?,
            None => return Ok(false),
        };
        if changed {
            reapply(lua, l, true);
        }
        Ok(changed)
    });
    match ran {
        Some(Ok(true)) => {
            raise(lua, vec![event]);
            Ok(())
        }
        Some(Err(e)) => Err(e),
        _ => Ok(()),
    }
}

/// Register the tradeskill globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CloseTradeSkill() (`0x5da8b0`): no line or opener, the repeat stopped, then the close; no
    // packet.
    g.set(
        "CloseTradeSkill",
        lua.create_function(|lua, ()| {
            let stopped =
                with_live(lua, |l| l.trade_skill_mut().is_some_and(|t| t.close())).unwrap_or(false);
            let mut ev = Vec::new();
            if stopped {
                ev.push("UPDATE_TRADESKILL_RECAST");
            }
            ev.push("TRADE_SKILL_CLOSE");
            raise(lua, ev);
            Ok(())
        })?,
    )?;

    // GetNumTradeSkills() (`0x5da120`): the visible entries, headers included.
    g.set(
        "GetNumTradeSkills",
        lua.create_function(|lua, ()| Ok(with_list(lua, |t| t.filtered() as i64).unwrap_or(0)))?,
    )?;

    // GetTradeSkillInfo(index) (`0x5db2a0`): five values. A header is (name, "header", 0,
    // isExpanded, nil), the `(-1, -1)` header named by the line's alternate verb; a recipe is
    // (name, difficulty, numAvailable, nil, altVerb), altVerb nil for one that makes an item.
    g.set(
        "GetTradeSkillInfo",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillInfo(index)")?;
            enum Info {
                Header(Option<String>, bool),
                Recipe(String, &'static str, i32, Option<String>),
            }
            let info = with_live(lua, |l| {
                let t = l.trade_skill()?;
                let e = *t.entry(index)?;
                match t.recipe(&e).cloned() {
                    Some(r) => {
                        let spell = l.trade_skill_recipe(r.spell_id)?;
                        let verb = (!spell.creates_item)
                            .then(|| l.trade_skill_line().alt_verb)
                            .flatten();
                        Some(Info::Recipe(
                            spell.name,
                            r.difficulty.as_str(),
                            r.num_available,
                            verb,
                        ))
                    }
                    None => {
                        let expanded = !t.is_collapsed_header((index - 1) as usize);
                        if e.key == (-1, -1) {
                            Some(Info::Header(l.trade_skill_line().alt_verb, expanded))
                        } else {
                            let name = t
                                .sub_classes
                                .iter()
                                .find(|c| c.key == e.key)
                                .and_then(|c| c.name.clone())?;
                            Some(Info::Header(Some(name), expanded))
                        }
                    }
                }
            })
            .flatten();
            Ok(match info {
                Some(Info::Recipe(name, kind, available, verb)) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&name)?),
                    Value::String(lua.create_string(kind)?),
                    Value::Integer(i64::from(available)),
                    Value::Nil,
                    opt_str(lua, verb.as_deref())?,
                ]),
                Some(Info::Header(name, expanded)) => MultiValue::from_vec(vec![
                    opt_str(lua, name.as_deref())?,
                    Value::String(lua.create_string("header")?),
                    Value::Integer(0),
                    flag(expanded),
                    Value::Nil,
                ]),
                None => MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                ]),
            })
        })?,
    )?;

    // SelectTradeSkill(index) (`0x5da900`): the entry's recipe, cleared by a header or a miss.
    g.set(
        "SelectTradeSkill",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: SelectTradeSkill(index)")?;
            with_live(lua, |l| {
                if let Some(t) = l.trade_skill_mut() {
                    t.selected = t.recipe_at(index).map_or(0, |r| r.spell_id);
                }
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillSelectionIndex() (`0x5da970`): 1-based, hidden entries included; 0 for none.
    g.set(
        "GetTradeSkillSelectionIndex",
        lua.create_function(|lua, ()| {
            Ok(with_list(lua, |t| t.selection_index().map_or(0, |i| i as i64 + 1)).unwrap_or(0))
        })?,
    )?;

    // GetTradeSkillCooldown(index) (`0x5da9a0`): the seconds left of the recipe's cooldown read
    // at the call, nil when ready.
    g.set(
        "GetTradeSkillCooldown",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillCooldown(index)")?;
            let (anchor, ui_now) = {
                let model = lua.app_data_ref::<ModelCell>().expect("model");
                (model.clock_wall.get(), model.now)
            };
            let cooldown = with_live(lua, |l| {
                let spell = l.trade_skill()?.recipe_at(index)?.spell_id;
                l.trade_skill_cooldown(spell, anchor, ui_now)
            })
            .flatten();
            // The clock is read inside the call (`0x5daa17`) and subtracted from the end (`0x5daa24`).
            let now = crate::script::clock::now(lua);
            // Any answer of the history counts (`0x5daa13`): an on-hold record reads as started now
            // and 1 ms long (`0x807ad4`, `0x807af2`), so 1 ms is left.
            Ok(match cooldown {
                Some((_, _, false)) => Value::Number(0.001),
                Some((start_ms, duration_ms, true)) => {
                    let end = (start_ms + i64::from(duration_ms)) as f64 / 1000.0;
                    // The history keeps a record with no time left (`0x807ae5` skips only a negative one), so 0 at the end's millisecond.
                    if end >= now {
                        Value::Number(end - now)
                    } else {
                        Value::Nil
                    }
                }
                _ => Value::Nil,
            })
        })?,
    )?;

    // GetTradeSkillIcon(index) (`0x5db550`): the product's icon, else the spell's; nil on a miss.
    g.set(
        "GetTradeSkillIcon",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillIcon(index)")?;
            let icon = recipe_at(lua, index).and_then(|(_, r)| r.icon);
            opt_str(lua, icon.as_deref())
        })?,
    )?;

    // GetTradeSkillNumMade(index) (`0x5db6e0`): effect 0's points, each at least 1; (1, 1) on a
    // miss.
    g.set(
        "GetTradeSkillNumMade",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillNumMade(index)")?;
            let (lo, hi) = recipe_at(lua, index).map_or((1, 1), |(_, r)| (r.min_made, r.max_made));
            Ok((i64::from(lo), i64::from(hi)))
        })?,
    )?;

    // GetTradeSkillLine() (`0x5db810`): the line's name and the player's rank and max rank read
    // at the call; ("UNKNOWN", 0, 0) with no line.
    g.set(
        "GetTradeSkillLine",
        lua.create_function(|lua, ()| {
            let line = with_live(lua, |l| l.trade_skill_line()).unwrap_or_default();
            Ok(match line.name {
                Some(name) => (name, line.rank, line.max_rank),
                None => ("UNKNOWN".to_string(), 0, 0),
            })
        })?,
    )?;

    // GetTradeSkillItemLink(index) (`0x5dba70`): the product's link while its record is cached,
    // else the recipe's enchant link; nothing on a header or a miss.
    g.set(
        "GetTradeSkillItemLink",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillItemLink(index)")?;
            let Some((spell, r)) = recipe_at(lua, index) else {
                return Ok(MultiValue::new());
            };
            let item = {
                let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
                r.creates_item
                    .then(|| crate::script::item_stats::item_record(&model, r.product_item))
                    .flatten()
            };
            let link = match item {
                Some(t) => item_link(
                    r.product_item,
                    &t.name,
                    t.quality,
                    crate::script::item_stats::player_level(),
                ),
                None => format!("|cffffd000|Henchant:{spell}|h[{}]|h|r", r.name),
            };
            Ok(MultiValue::from_vec(vec![Value::String(
                lua.create_string(&link)?,
            )]))
        })?,
    )?;

    // SetTradeSkillItemNameFilter(text) (`0x5dd070` -> `0x5dcf60`): an empty or absent text clears
    // it; the same text, case aside, is a no-op. Fires TRADE_SKILL_FILTER_UPDATE.
    g.set(
        "SetTradeSkillItemNameFilter",
        lua.create_function(|lua, text: Value| {
            let text = binding_abi::optional_string(lua, &text).filter(|s| !s.is_empty());
            let folded = text.map(|s| s.to_lowercase());
            filter_edit(lua, "TRADE_SKILL_FILTER_UPDATE", |t| {
                if folded == t.name_filter {
                    return Ok(false);
                }
                t.name_filter = folded;
                Ok(true)
            })
        })?,
    )?;

    // GetTradeSkillItemNameFilter() (`0x5da150`): the folded filter, nil for none.
    g.set(
        "GetTradeSkillItemNameFilter",
        lua.create_function(|lua, ()| {
            let name = with_list(lua, |t| t.name_filter.clone()).flatten();
            opt_str(lua, name.as_deref())
        })?,
    )?;

    // SetTradeSkillItemLevelFilter(minLevel, maxLevel) (`0x5dd090` -> `0x5dd030`): the product's
    // required level range, 0 for open; a change fires TRADE_SKILL_UPDATE.
    g.set(
        "SetTradeSkillItemLevelFilter",
        lua.create_function(|lua, (min, max): (Value, Value)| {
            let usage = "Usage: SetTradeSkillItemLevelFilter(minLevel, maxLevel)";
            let min = number_arg(lua, min, usage)?;
            let max = number_arg(lua, max, usage)?;
            filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                if (min, max) == (t.min_level, t.max_level) {
                    return Ok(false);
                }
                t.min_level = min;
                t.max_level = max;
                Ok(true)
            })
        })?,
    )?;

    // GetTradeSkillItemLevelFilter() (`0x5da170`).
    g.set(
        "GetTradeSkillItemLevelFilter",
        lua.create_function(|lua, ()| {
            Ok(
                with_list(lua, |t| (i64::from(t.min_level), i64::from(t.max_level)))
                    .unwrap_or((0, 0)),
            )
        })?,
    )?;

    // GetTradeSkillNumReagents(index) (`0x5dbba0`): the recipe's nonzero reagent slots.
    g.set(
        "GetTradeSkillNumReagents",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillNumReagents(index)")?;
            Ok(recipe_at(lua, index).map_or(0, |(_, r)| r.reagents.len() as i64))
        })?,
    )?;

    // GetTradeSkillReagentInfo(index, reagentIndex) (`0x5de400`): (name, texture, need, have),
    // the record asked with the list's item callback and the bags counted at the call; name and
    // texture nil until the record arrives; four nils on a miss.
    g.set(
        "GetTradeSkillReagentInfo",
        lua.create_function(|lua, (index, reagent): (Value, Value)| {
            let usage = "Usage: GetTradeSkillReagentInfo(index, reagentIndex)";
            let index = index_arg(lua, index, usage)?;
            let reagent = index_arg(lua, reagent, usage)?;
            let read = with_live(lua, |l| {
                let spell = l.trade_skill()?.recipe_at(index)?.spell_id;
                let r = l.trade_skill_recipe(spell)?;
                let &(item, need) = usize::try_from(reagent - 1)
                    .ok()
                    .and_then(|i| r.reagents.get(i))?;
                Some((l.trade_skill_reagent(spell, item), need))
            })
            .flatten();
            let Some((re, need)) = read else {
                return Ok(nils(4));
            };
            Ok(MultiValue::from_vec(vec![
                opt_str(lua, re.name.as_deref())?,
                opt_str(lua, re.icon.as_deref())?,
                Value::Integer(i64::from(need)),
                Value::Integer(i64::from(re.have)),
            ]))
        })?,
    )?;

    // GetTradeSkillReagentItemLink(index, reagentIndex) (`0x5dbcc0`): the link or nil; the usage
    // error's name is the reference's own.
    g.set(
        "GetTradeSkillReagentItemLink",
        lua.create_function(|lua, (index, reagent): (Value, Value)| {
            let usage = "Usage: GetTradeReagentSkillItemLink(index, reagentIndex)";
            let index = index_arg(lua, index, usage)?;
            let reagent = index_arg(lua, reagent, usage)?;
            let item = recipe_at(lua, index).and_then(|(_, r)| {
                usize::try_from(reagent - 1)
                    .ok()
                    .and_then(|i| r.reagents.get(i))
                    .map(|&(item, _)| item)
            });
            let link = item.and_then(|item| {
                let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
                crate::script::item_stats::item_record(&model, item).map(|t| {
                    item_link(
                        item,
                        &t.name,
                        t.quality,
                        crate::script::item_stats::player_level(),
                    )
                })
            });
            opt_str(lua, link.as_deref())
        })?,
    )?;

    // GetTradeSkillTools(index) (`0x5de620`): alternating (name, has) pairs read at the call: the
    // spell focus and the required area, always met, then the tools and tool categories.
    g.set(
        "GetTradeSkillTools",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillTools(index)")?;
            let tools = with_live(lua, |l| {
                let spell = l.trade_skill()?.recipe_at(index)?.spell_id;
                Some(l.trade_skill_tools(spell))
            })
            .flatten()
            .unwrap_or_default();
            let mut out = Vec::with_capacity(tools.len() * 2);
            for (name, have) in &tools {
                out.push(Value::String(lua.create_string(name)?));
                out.push(flag(*have));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTradeSkillDescription(index) (`0x5dbe10`): the spell's description parsed at the call,
    // nil for none.
    g.set(
        "GetTradeSkillDescription",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillDescription(index)")?;
            let global = |key: &str| crate::strings::global(lua, key);
            let text = with_live(lua, |l| {
                let spell = l.trade_skill()?.recipe_at(index)?.spell_id;
                l.trade_skill_description(spell, &global)
            })
            .flatten()
            .filter(|d| !d.is_empty());
            opt_str(lua, text.as_deref())
        })?,
    )?;

    // GetTradeSkillSubClasses() (`0x5daa70`): each sub-class's name in list order, the `(-1, -1)`
    // one the line's alternate verb.
    g.set(
        "GetTradeSkillSubClasses",
        lua.create_function(|lua, ()| {
            let names = with_live(lua, |l| {
                let verb = l.trade_skill_line().alt_verb;
                l.trade_skill().map(|t| {
                    t.sub_classes
                        .iter()
                        .map(|c| {
                            if c.key == (-1, -1) {
                                verb.clone()
                            } else {
                                c.name.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            .unwrap_or_default();
            let mut out = Vec::with_capacity(names.len());
            for name in &names {
                out.push(opt_str(lua, name.as_deref())?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetTradeSkillInvSlots() (`0x5dbf10`): the slot names of the picked sub-class, else of every
    // recipe built, in bit order.
    g.set(
        "GetTradeSkillInvSlots",
        lua.create_function(|lua, ()| {
            let mask = with_list(lua, |t| t.listed_inv_slots()).unwrap_or(0);
            let mut out = Vec::new();
            for (slot, token) in INV_SLOT_TOKENS.iter().enumerate() {
                if mask & (1 << slot) != 0 {
                    let word =
                        crate::strings::global(lua, token).unwrap_or_else(|| token.to_string());
                    out.push(Value::String(lua.create_string(&word)?));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // SetTradeSkillSubClassFilter(index [, on, exclusive]) (`0x5dd0f0`): index 0 shows all;
    // otherwise `on` 0 hides sub-class index, exclusive shows it alone, else shows it too.
    g.set(
        "SetTradeSkillSubClassFilter",
        lua.create_function(|lua, (index, on, exclusive): (Value, Value, Value)| {
            if !is_string(&index) {
                return Err(error(
                    "Usage: SetTradeSkillSubClassFilter(index [, on\\off, exclusive])",
                ));
            }
            let i = opt_number(lua, &index)?.unwrap_or(0) - 1;
            let on = opt_number(lua, &on)?;
            let exclusive = opt_number(lua, &exclusive)?.unwrap_or(0) != 0;
            filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                if i < 0 {
                    t.sub_class_filter = u32::MAX;
                    t.sub_class_filter_index = u32::MAX;
                    return Ok(true);
                }
                if i as usize >= t.sub_classes.len() {
                    return Err(error("Bad sub class in SetTradeSkillSubClassFilter"));
                }
                let Some(on) = on else {
                    return Err(error(
                        "Missing on//off parameter in SetTradeSkillSubClassFilter",
                    ));
                };
                let b = 1u32.wrapping_shl(i as u32);
                t.sub_class_filter = if on == 0 {
                    t.sub_class_filter & !b
                } else if exclusive {
                    b
                } else {
                    t.sub_class_filter | b
                };
                t.sub_class_filter_index = i as u32;
                Ok(true)
            })
        })?,
    )?;

    // GetTradeSkillSubClassFilter(index) (`0x5da1b0`): index 0 asks whether all are shown.
    g.set(
        "GetTradeSkillSubClassFilter",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetTradeSkillSubClassFilter(index)")? - 1;
            let (n, filter) =
                with_list(lua, |t| (t.sub_classes.len(), t.sub_class_filter)).unwrap_or((0, 0));
            if i < 0 {
                return Ok(flag(
                    (0..n).all(|b| filter & 1u32.wrapping_shl(b as u32) != 0),
                ));
            }
            if i as usize >= n {
                return Err(error("Bad sub class in GetTradeSkillSubClassFilter"));
            }
            Ok(flag(filter & 1u32.wrapping_shl(i as u32) != 0))
        })?,
    )?;

    // SetTradeSkillInvSlotFilter(index [, on, exclusive]) (`0x5dd230`): index counts the listed
    // slots; 0 shows all.
    g.set(
        "SetTradeSkillInvSlotFilter",
        lua.create_function(|lua, (index, on, exclusive): (Value, Value, Value)| {
            if !is_string(&index) {
                return Err(error(
                    "Usage: SetTradeSkillInvSlotFilter(index [, on\\off, exclusive])",
                ));
            }
            let i = opt_number(lua, &index)?.unwrap_or(0) - 1;
            let on = opt_number(lua, &on)?;
            let exclusive = opt_number(lua, &exclusive)?.unwrap_or(0) != 0;
            filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                if i < 0 {
                    t.inv_type_filter = u32::MAX;
                    return Ok(true);
                }
                let Some(slot) = nth_slot(t.listed_inv_slots(), i) else {
                    return Err(error("Bad inv slot in SetTradeSkillInvSlotFilter"));
                };
                let Some(on) = on else {
                    return Err(error(
                        "Missing on//off parameter in SetTradeSkillInvSlotFilter",
                    ));
                };
                let b = 1u32 << slot;
                t.inv_type_filter = if on == 0 {
                    t.inv_type_filter & !b
                } else if exclusive {
                    b
                } else {
                    t.inv_type_filter | b
                };
                Ok(true)
            })
        })?,
    )?;

    // GetTradeSkillInvSlotFilter(index) (`0x5dbf90`): index 0 asks whether all are shown; with
    // all shown every single slot answers nil.
    g.set(
        "GetTradeSkillInvSlotFilter",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetTradeSkillInvSlotFilter(index)")? - 1;
            let (filter, listed) = with_list(lua, |t| (t.inv_type_filter, t.listed_inv_slots()))
                .unwrap_or((u32::MAX, 0));
            let all = filter == u32::MAX;
            if i < 0 || all {
                return Ok(flag(i < 0 && all));
            }
            let Some(slot) = nth_slot(listed, i as i32) else {
                return Err(error("Bad inv type in GetTradeSkillInvSlotFilter"));
            };
            Ok(flag(filter & (1 << slot) != 0))
        })?,
    )?;

    // TradeSkillOnlyShowMakeable(flag) / TradeSkillOnlyShowSkillUps(flag) (`0x5dd3e0`,
    // `0x5dd420`): the flag read as a string boolean, absent meaning on; a change fires
    // TRADE_SKILL_UPDATE.
    g.set(
        "TradeSkillOnlyShowMakeable",
        lua.create_function(|lua, v: mlua::Variadic<Value>| {
            let on = bool_or_default(v.first(), true);
            filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                let changed = on != t.only_makeable;
                t.only_makeable = on;
                Ok(changed)
            })
        })?,
    )?;
    g.set(
        "TradeSkillOnlyShowSkillUps",
        lua.create_function(|lua, v: mlua::Variadic<Value>| {
            let on = bool_or_default(v.first(), true);
            filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                let changed = on != t.only_skill_ups;
                t.only_skill_ups = on;
                Ok(changed)
            })
        })?,
    )?;

    // Collapse/ExpandTradeSkillSubClass(index) (`0x5dd460`, `0x5dd500`): the header at index, 0
    // for all; any other entry raises.
    for (name, collapse, bad) in [
        (
            "CollapseTradeSkillSubClass",
            true,
            "Bad sub class in CollapseTradeSkillSubClass",
        ),
        (
            "ExpandTradeSkillSubClass",
            false,
            "Bad skill line in ExpandTradeSkillSubClass",
        ),
    ] {
        let usage: &'static str = if collapse {
            "Usage: CollapseTradeSkillSubClass(index)"
        } else {
            "Usage: ExpandTradeSkillSubClass(index)"
        };
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let i = index_arg(lua, index, usage)? - 1;
                filter_edit(lua, "TRADE_SKILL_UPDATE", |t| {
                    if i >= 0 {
                        let Some(sc) = t.sub_class_of_header(i as usize) else {
                            return Err(error(bad));
                        };
                        let b = 1u32.wrapping_shl(sc as u32);
                        if collapse {
                            t.collapse_filter &= !b;
                        } else {
                            t.collapse_filter |= b;
                        }
                    } else {
                        t.collapse_filter = if collapse { 0 } else { u32::MAX };
                    }
                    Ok(true)
                })
            })?,
        )?;
    }

    // GetFirstTradeSkill() (`0x5dab70`): the first visible recipe, 0 for none.
    g.set(
        "GetFirstTradeSkill",
        lua.create_function(|lua, ()| {
            Ok(with_list(lua, |t| {
                t.display
                    .iter()
                    .take(t.filtered)
                    .position(|e| e.recipe.is_some())
                    .map_or(0, |p| p as i64 + 1)
            })
            .unwrap_or(0))
        })?,
    )?;

    // GetTradeskillRepeatCount() (`0x5da270`): the lowercase "s" is the reference's.
    g.set(
        "GetTradeskillRepeatCount",
        lua.create_function(|lua, ()| {
            Ok(with_list(lua, |t| i64::from(t.repeat_count())).unwrap_or(1))
        })?,
    )?;

    // DoTradeSkill(index [, repeat]) (`0x5dac00`): casts the recipe inside the call, the count
    // capped by what the reagents cover; a running repeat of it only takes the new count.
    g.set(
        "DoTradeSkill",
        lua.create_function(|lua, (index, times): (Value, Value)| {
            let index = index_arg(lua, index, "Usage: DoTradeSkill(index [, repeat])")?;
            let times = opt_number(lua, &times)?.map_or(1, |n| n as u32);
            let queued = with_live(lua, |l| {
                let t = l.trade_skill_mut()?;
                let (spell, available) = t
                    .recipe_at(index)
                    .map(|r| (r.spell_id, r.num_available as u32))?;
                let times = if available != u32::MAX && times >= available {
                    available
                } else {
                    times
                };
                // Another recipe casts first and takes its repeat once the cast went (`0x5daca6`).
                if t.recast_spell() != spell {
                    t.casts.push((spell, times));
                    return Some(true);
                }
                t.set_recast(spell, times);
                Some(false)
            })
            .flatten();
            match queued {
                Some(true) => {
                    crate::script::calls::press(lua, crate::script::VerbPress::TradeSkill)
                }
                Some(false) => raise(lua, vec!["UPDATE_TRADESKILL_RECAST"]),
                None => {}
            }
            Ok(())
        })?,
    )?;

    // GetTradeSkillRecipeLink(index) (`0x5db920`): "|Henchant:<spell>|h[<line>: <name>]|h" with
    // the recipe's own line, "[<name>]" without one; nothing for a spell without the tradeskill
    // attribute.
    g.set(
        "GetTradeSkillRecipeLink",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillRecipeLink(index)")?;
            let Some((spell, r)) = recipe_at(lua, index).filter(|(_, r)| r.tradeskill) else {
                return Ok(MultiValue::new());
            };
            let link = match r.line_name.as_deref() {
                Some(line) => format!("|cffffd000|Henchant:{spell}|h[{line}: {}]|h|r", r.name),
                None => format!("|cffffd000|Henchant:{spell}|h[{}]|h|r", r.name),
            };
            Ok(MultiValue::from_vec(vec![Value::String(
                lua.create_string(&link)?,
            )]))
        })?,
    )?;

    // StopTradeSkillRepeat() (`0x5da2e0`).
    g.set(
        "StopTradeSkillRepeat",
        lua.create_function(|lua, ()| {
            let stopped = with_live(lua, |l| {
                l.trade_skill_mut().is_some_and(|t| t.stop_recast())
            })
            .unwrap_or(false);
            if stopped {
                raise(lua, vec!["UPDATE_TRADESKILL_RECAST"]);
            }
            Ok(())
        })?,
    )?;

    // GetTradeSkillListLink() (`0x5dc0b0`): the open line's "|Htrade:" link built at the call,
    // nil when it cannot link.
    g.set(
        "GetTradeSkillListLink",
        lua.create_function(|lua, ()| {
            let link = with_live(lua, |l| l.trade_skill_list_link()).flatten();
            opt_str(lua, link.as_deref())
        })?,
    )?;

    // IsTradeSkillLinked() (`0x5da310`): (1, name) while viewing a linked list, which no link
    // opens here yet, so (nil, nil).
    g.set(
        "IsTradeSkillLinked",
        lua.create_function(|_, ()| Ok(nils(2)))?,
    )?;

    Ok(())
}
