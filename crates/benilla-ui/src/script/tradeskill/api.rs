//! The tradeskill Lua globals, the 36 of the client's registrar (`0xad0e80`), over
//! [`super::view`]'s display list.

use mlua::{Lua, MultiValue, Value};

use crate::script::binding_abi::{self, bool_or_default, flag, number_arg};
use crate::script::item_stats::item_link;
use crate::script::ModelCell;

use super::view::TradeSkillModel;

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

/// Reads the window's state.
fn with<R>(lua: &Lua, f: impl FnOnce(&TradeSkillModel) -> R) -> R {
    let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
    f(&model.trade_skill)
}

/// Mutates the window's state; the events it returns fire at the next tick.
fn with_mut(lua: &Lua, f: impl FnOnce(&mut TradeSkillModel) -> Vec<&'static str>) {
    let mut model = crate::script::model_mut_of(lua);
    let events = f(&mut model.trade_skill);
    for e in events {
        model.pending_events.push((e.to_string(), Vec::new()));
    }
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

fn raise(msg: &str) -> mlua::Error {
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

/// Register the tradeskill globals.
pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CloseTradeSkill() (`0x5da8b0`): stops the repeat and fires the close; no packet.
    g.set(
        "CloseTradeSkill",
        lua.create_function(|lua, ()| {
            with_mut(lua, |t| {
                t.close = true;
                let mut ev = Vec::new();
                if t.stop_recast() {
                    ev.push("UPDATE_TRADESKILL_RECAST");
                }
                ev.push("TRADE_SKILL_CLOSE");
                ev
            });
            Ok(())
        })?,
    )?;

    // GetNumTradeSkills() (`0x5da120`): the visible entries, headers included.
    g.set(
        "GetNumTradeSkills",
        lua.create_function(|lua, ()| Ok(with(lua, |t| t.filtered as i64)))?,
    )?;

    // GetTradeSkillInfo(index) (`0x5db2a0`): five values. A header is (name, "header", 0,
    // isExpanded, nil), the `(-1, -1)` header named by the alternate verb; a recipe is (name,
    // difficulty, numAvailable, nil, altVerb), altVerb nil for one that makes an item.
    g.set(
        "GetTradeSkillInfo",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillInfo(index)")?;
            with(lua, |t| {
                let Some(e) = t.entry(index) else {
                    return Ok(MultiValue::from_vec(vec![
                        Value::Nil,
                        Value::Nil,
                        Value::Integer(0),
                        Value::Nil,
                        Value::Nil,
                    ]));
                };
                let state = t.state.as_ref();
                let verb = state.map(|s| s.alt_verb.as_str());
                match t.recipe(e) {
                    Some(r) => Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&r.name)?),
                        Value::String(lua.create_string(r.difficulty.as_str())?),
                        Value::Integer(i64::from(r.num_available)),
                        Value::Nil,
                        opt_str(lua, verb.filter(|_| !r.creates_item))?,
                    ])),
                    None => {
                        let name = if e.key == (-1, -1) {
                            Some(verb)
                        } else {
                            t.sub_classes
                                .iter()
                                .find(|c| c.key == e.key)
                                .and_then(|c| c.name.as_deref())
                                .map(Some)
                        };
                        let Some(name) = name else {
                            return Ok(MultiValue::from_vec(vec![
                                Value::Nil,
                                Value::Nil,
                                Value::Integer(0),
                                Value::Nil,
                                Value::Nil,
                            ]));
                        };
                        let i = (index - 1) as usize;
                        Ok(MultiValue::from_vec(vec![
                            opt_str(lua, name)?,
                            Value::String(lua.create_string("header")?),
                            Value::Integer(0),
                            flag(!t.is_collapsed_header(i)),
                            Value::Nil,
                        ]))
                    }
                }
            })
        })?,
    )?;

    // SelectTradeSkill(index) (`0x5da900`): the entry's recipe, cleared by a header or a miss.
    g.set(
        "SelectTradeSkill",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: SelectTradeSkill(index)")?;
            with_mut(lua, |t| {
                t.selected = t.recipe_at(index).map_or(0, |r| r.spell_id);
                Vec::new()
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillSelectionIndex() (`0x5da970`): 1-based, hidden entries included; 0 for none.
    g.set(
        "GetTradeSkillSelectionIndex",
        lua.create_function(|lua, ()| {
            Ok(with(lua, |t| {
                t.selection_index().map_or(0, |i| i as i64 + 1)
            }))
        })?,
    )?;

    // GetTradeSkillCooldown(index) (`0x5da9a0`): the seconds left, nil when ready.
    g.set(
        "GetTradeSkillCooldown",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillCooldown(index)")?;
            let now = lua.app_data_ref::<ModelCell>().expect("model").now;
            Ok(with(lua, |t| {
                match t.recipe_at(index).and_then(|r| r.cooldown_end) {
                    Some(end) if end > now => Value::Number(end - now),
                    _ => Value::Nil,
                }
            }))
        })?,
    )?;

    // GetTradeSkillIcon(index) (`0x5db550`): the product's icon, else the spell's; nil on a miss.
    g.set(
        "GetTradeSkillIcon",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillIcon(index)")?;
            with(lua, |t| {
                opt_str(lua, t.recipe_at(index).and_then(|r| r.icon.as_deref()))
            })
        })?,
    )?;

    // GetTradeSkillNumMade(index) (`0x5db6e0`): effect 0's points, each at least 1; (1, 1) on a
    // miss.
    g.set(
        "GetTradeSkillNumMade",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillNumMade(index)")?;
            let (lo, hi) = with(lua, |t| {
                t.recipe_at(index)
                    .map_or((1, 1), |r| (r.min_made, r.max_made))
            });
            Ok((i64::from(lo), i64::from(hi)))
        })?,
    )?;

    // GetTradeSkillLine() (`0x5db810`): ("UNKNOWN", 0, 0) with no window open.
    g.set(
        "GetTradeSkillLine",
        lua.create_function(|lua, ()| {
            let (name, rank, max_rank) = with(lua, |t| match &t.state {
                Some(s) => (s.line_name.clone(), s.rank, s.max_rank),
                None => ("UNKNOWN".to_string(), 0, 0),
            });
            Ok((name, i64::from(rank), i64::from(max_rank)))
        })?,
    )?;

    // GetTradeSkillItemLink(index) (`0x5dba70`): the product's link while its template is cached,
    // else the recipe's enchant link; nothing on a header or a miss.
    g.set(
        "GetTradeSkillItemLink",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillItemLink(index)")?;
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let Some(r) = model.trade_skill.recipe_at(index) else {
                return Ok(MultiValue::new());
            };
            let item = (r.creates_item && r.product_item != 0)
                .then(|| model.item_templates.get(&r.product_item))
                .flatten();
            let link = match item {
                Some(t) => item_link(r.product_item, &t.name, t.quality),
                None => format!("|cffffd000|Henchant:{}|h[{}]|h|r", r.spell_id, r.name),
            };
            Ok(MultiValue::from_vec(vec![Value::String(
                lua.create_string(&link)?,
            )]))
        })?,
    )?;

    // SetTradeSkillItemNameFilter(text) (`0x5dcf60`): an empty or absent text clears it; the same
    // text, case aside, is a no-op. Fires TRADE_SKILL_FILTER_UPDATE.
    g.set(
        "SetTradeSkillItemNameFilter",
        lua.create_function(|lua, text: Value| {
            let text = binding_abi::optional_string(lua, &text).filter(|s| !s.is_empty());
            with_mut(lua, |t| {
                let folded = text.map(|s| s.to_lowercase());
                if folded == t.name_filter {
                    return Vec::new();
                }
                t.name_filter = folded;
                t.apply(true);
                vec!["TRADE_SKILL_FILTER_UPDATE"]
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillItemNameFilter() (`0x5da150`): the folded filter, nil for none.
    g.set(
        "GetTradeSkillItemNameFilter",
        lua.create_function(|lua, ()| with(lua, |t| opt_str(lua, t.name_filter.as_deref())))?,
    )?;

    // SetTradeSkillItemLevelFilter(minLevel, maxLevel) (`0x5dd090`): the product's required level
    // range, 0 for open; a change fires TRADE_SKILL_UPDATE.
    g.set(
        "SetTradeSkillItemLevelFilter",
        lua.create_function(|lua, (min, max): (Value, Value)| {
            let usage = "Usage: SetTradeSkillItemLevelFilter(minLevel, maxLevel)";
            let min = number_arg(lua, min, usage)?;
            let max = number_arg(lua, max, usage)?;
            with_mut(lua, |t| {
                if (min, max) == (t.min_level, t.max_level) {
                    return Vec::new();
                }
                t.min_level = min;
                t.max_level = max;
                t.apply(true);
                vec!["TRADE_SKILL_UPDATE"]
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillItemLevelFilter() (`0x5da170`).
    g.set(
        "GetTradeSkillItemLevelFilter",
        lua.create_function(|lua, ()| {
            Ok(with(lua, |t| {
                (i64::from(t.min_level), i64::from(t.max_level))
            }))
        })?,
    )?;

    // GetTradeSkillNumReagents(index) (`0x5dbba0`).
    g.set(
        "GetTradeSkillNumReagents",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillNumReagents(index)")?;
            Ok(with(lua, |t| {
                t.recipe_at(index).map_or(0, |r| r.reagents.len() as i64)
            }))
        })?,
    )?;

    // GetTradeSkillReagentInfo(index, reagentIndex) (`0x5de400`): (name, texture, need, have),
    // name and texture nil until the template arrives; four nils on a miss.
    g.set(
        "GetTradeSkillReagentInfo",
        lua.create_function(|lua, (index, reagent): (Value, Value)| {
            let usage = "Usage: GetTradeSkillReagentInfo(index, reagentIndex)";
            let index = index_arg(lua, index, usage)?;
            let reagent = index_arg(lua, reagent, usage)?;
            with(lua, |t| {
                let Some(re) = t.recipe_at(index).and_then(|r| {
                    usize::try_from(reagent - 1)
                        .ok()
                        .and_then(|i| r.reagents.get(i))
                }) else {
                    return Ok(nils(4));
                };
                Ok(MultiValue::from_vec(vec![
                    opt_str(lua, re.name.as_deref())?,
                    opt_str(lua, re.icon.as_deref())?,
                    Value::Integer(i64::from(re.need)),
                    Value::Integer(i64::from(re.have)),
                ]))
            })
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
            let model = lua.app_data_ref::<ModelCell>().expect("model app_data");
            let link = model
                .trade_skill
                .recipe_at(index)
                .and_then(|r| {
                    usize::try_from(reagent - 1)
                        .ok()
                        .and_then(|i| r.reagents.get(i))
                })
                .and_then(|re| {
                    model
                        .item_templates
                        .get(&re.item)
                        .map(|t| item_link(re.item, &t.name, t.quality))
                });
            opt_str(lua, link.as_deref())
        })?,
    )?;

    // GetTradeSkillTools(index) (`0x5de620`): alternating (name, has) pairs: the spell focus and
    // the required area, always met, then the tools and tool categories.
    g.set(
        "GetTradeSkillTools",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillTools(index)")?;
            with(lua, |t| {
                let mut out = Vec::new();
                if let Some(r) = t.recipe_at(index) {
                    for (name, have) in &r.tools {
                        out.push(Value::String(lua.create_string(name)?));
                        out.push(flag(*have));
                    }
                }
                Ok(MultiValue::from_vec(out))
            })
        })?,
    )?;

    // GetTradeSkillDescription(index) (`0x5dbe10`): the spell's parsed description, nil for none.
    g.set(
        "GetTradeSkillDescription",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillDescription(index)")?;
            with(lua, |t| {
                opt_str(
                    lua,
                    t.recipe_at(index)
                        .and_then(|r| r.description.as_deref())
                        .filter(|d| !d.is_empty()),
                )
            })
        })?,
    )?;

    // GetTradeSkillSubClasses() (`0x5daa70`): each sub-class's name in list order, the `(-1, -1)`
    // one the alternate verb.
    g.set(
        "GetTradeSkillSubClasses",
        lua.create_function(|lua, ()| {
            with(lua, |t| {
                let verb = t.state.as_ref().map(|s| s.alt_verb.as_str());
                let mut out = Vec::new();
                for c in &t.sub_classes {
                    let name = if c.key == (-1, -1) {
                        verb
                    } else {
                        c.name.as_deref()
                    };
                    out.push(opt_str(lua, name)?);
                }
                Ok(MultiValue::from_vec(out))
            })
        })?,
    )?;

    // GetTradeSkillInvSlots() (`0x5dbf10`): the slot names of the picked sub-class, else of every
    // recipe built, in bit order.
    g.set(
        "GetTradeSkillInvSlots",
        lua.create_function(|lua, ()| {
            let mask = with(lua, |t| t.listed_inv_slots());
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
                return Err(raise(
                    "Usage: SetTradeSkillSubClassFilter(index [, on\\off, exclusive])",
                ));
            }
            let i = opt_number(lua, &index)?.unwrap_or(0) - 1;
            let on = opt_number(lua, &on)?;
            let exclusive = opt_number(lua, &exclusive)?.unwrap_or(0) != 0;
            let mut result = Ok(());
            with_mut(lua, |t| {
                if i < 0 {
                    t.sub_class_filter = u32::MAX;
                    t.sub_class_filter_index = u32::MAX;
                } else if i as usize >= t.sub_classes.len() {
                    result = Err(raise("Bad sub class in SetTradeSkillSubClassFilter"));
                    return Vec::new();
                } else {
                    let Some(on) = on else {
                        result = Err(raise(
                            "Missing on//off parameter in SetTradeSkillSubClassFilter",
                        ));
                        return Vec::new();
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
                }
                t.apply(true);
                vec!["TRADE_SKILL_UPDATE"]
            });
            result
        })?,
    )?;

    // GetTradeSkillSubClassFilter(index) (`0x5da1b0`): index 0 asks whether all are shown.
    g.set(
        "GetTradeSkillSubClassFilter",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetTradeSkillSubClassFilter(index)")? - 1;
            with(lua, |t| {
                let n = t.sub_classes.len();
                if i < 0 {
                    return Ok(flag(
                        (0..n).all(|b| t.sub_class_filter & 1u32.wrapping_shl(b as u32) != 0),
                    ));
                }
                if i as usize >= n {
                    return Err(raise("Bad sub class in GetTradeSkillSubClassFilter"));
                }
                Ok(flag(t.sub_class_filter & 1u32.wrapping_shl(i as u32) != 0))
            })
        })?,
    )?;

    // SetTradeSkillInvSlotFilter(index [, on, exclusive]) (`0x5dd230`): index counts the listed
    // slots; 0 shows all.
    g.set(
        "SetTradeSkillInvSlotFilter",
        lua.create_function(|lua, (index, on, exclusive): (Value, Value, Value)| {
            if !is_string(&index) {
                return Err(raise(
                    "Usage: SetTradeSkillInvSlotFilter(index [, on\\off, exclusive])",
                ));
            }
            let i = opt_number(lua, &index)?.unwrap_or(0) - 1;
            let on = opt_number(lua, &on)?;
            let exclusive = opt_number(lua, &exclusive)?.unwrap_or(0) != 0;
            let mut result = Ok(());
            with_mut(lua, |t| {
                if i < 0 {
                    t.inv_type_filter = u32::MAX;
                } else {
                    let Some(slot) = nth_slot(t.listed_inv_slots(), i) else {
                        result = Err(raise("Bad inv slot in SetTradeSkillInvSlotFilter"));
                        return Vec::new();
                    };
                    let Some(on) = on else {
                        result = Err(raise(
                            "Missing on//off parameter in SetTradeSkillInvSlotFilter",
                        ));
                        return Vec::new();
                    };
                    let b = 1u32 << slot;
                    t.inv_type_filter = if on == 0 {
                        t.inv_type_filter & !b
                    } else if exclusive {
                        b
                    } else {
                        t.inv_type_filter | b
                    };
                }
                t.apply(true);
                vec!["TRADE_SKILL_UPDATE"]
            });
            result
        })?,
    )?;

    // GetTradeSkillInvSlotFilter(index) (`0x5dbf90`): index 0 asks whether all are shown; with
    // all shown every single slot answers nil.
    g.set(
        "GetTradeSkillInvSlotFilter",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetTradeSkillInvSlotFilter(index)")? - 1;
            with(lua, |t| {
                let all = t.inv_type_filter == u32::MAX;
                if i < 0 || all {
                    return Ok(flag(i < 0 && all));
                }
                let Some(slot) = nth_slot(t.listed_inv_slots(), i as i32) else {
                    return Err(raise("Bad inv type in GetTradeSkillInvSlotFilter"));
                };
                Ok(flag(t.inv_type_filter & (1 << slot) != 0))
            })
        })?,
    )?;

    // TradeSkillOnlyShowMakeable(flag) / TradeSkillOnlyShowSkillUps(flag) (`0x5dd3e0`,
    // `0x5dd420`): the flag read as a string boolean, absent meaning on; a change fires
    // TRADE_SKILL_UPDATE.
    g.set(
        "TradeSkillOnlyShowMakeable",
        lua.create_function(|lua, v: mlua::Variadic<Value>| {
            let on = bool_or_default(v.first(), true);
            with_mut(lua, |t| {
                if on == t.only_makeable {
                    return Vec::new();
                }
                t.only_makeable = on;
                t.apply(true);
                vec!["TRADE_SKILL_UPDATE"]
            });
            Ok(())
        })?,
    )?;
    g.set(
        "TradeSkillOnlyShowSkillUps",
        lua.create_function(|lua, v: mlua::Variadic<Value>| {
            let on = bool_or_default(v.first(), true);
            with_mut(lua, |t| {
                if on == t.only_skill_ups {
                    return Vec::new();
                }
                t.only_skill_ups = on;
                t.apply(true);
                vec!["TRADE_SKILL_UPDATE"]
            });
            Ok(())
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
                let mut result = Ok(());
                with_mut(lua, |t| {
                    if i >= 0 {
                        let Some(sc) = t.sub_class_of_header(i as usize) else {
                            result = Err(raise(bad));
                            return Vec::new();
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
                    t.apply(true);
                    vec!["TRADE_SKILL_UPDATE"]
                });
                result
            })?,
        )?;
    }

    // GetFirstTradeSkill() (`0x5dab70`): the first visible recipe, 0 for none.
    g.set(
        "GetFirstTradeSkill",
        lua.create_function(|lua, ()| {
            Ok(with(lua, |t| {
                t.display[..t.filtered]
                    .iter()
                    .position(|e| e.recipe.is_some())
                    .map_or(0, |p| p as i64 + 1)
            }))
        })?,
    )?;

    // GetTradeskillRepeatCount() (`0x5da270`): the lowercase "s" is the reference's.
    g.set(
        "GetTradeskillRepeatCount",
        lua.create_function(|lua, ()| Ok(with(lua, |t| i64::from(t.repeat_count()))))?,
    )?;

    // DoTradeSkill(index [, repeat]) (`0x5dac00`): casts the recipe, the count capped by what the
    // reagents cover; a running repeat of it only takes the new count.
    g.set(
        "DoTradeSkill",
        lua.create_function(|lua, (index, times): (Value, Value)| {
            let index = index_arg(lua, index, "Usage: DoTradeSkill(index [, repeat])")?;
            let times = opt_number(lua, &times)?.map_or(1, |n| n as u32);
            with_mut(lua, |t| {
                let Some((spell, available)) = t
                    .recipe_at(index)
                    .map(|r| (r.spell_id, r.num_available as u32))
                else {
                    return Vec::new();
                };
                let times = if available != u32::MAX && times >= available {
                    available
                } else {
                    times
                };
                if t.recast_spell() != spell {
                    t.casts.push(spell);
                }
                t.set_recast(spell, times);
                vec!["UPDATE_TRADESKILL_RECAST"]
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillRecipeLink(index) (`0x5db920`): "|Henchant:<spell>|h[<line>: <name>]|h".
    g.set(
        "GetTradeSkillRecipeLink",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetTradeSkillRecipeLink(index)")?;
            with(lua, |t| {
                let (Some(r), Some(s)) = (t.recipe_at(index), t.state.as_ref()) else {
                    return Ok(MultiValue::new());
                };
                let link = format!(
                    "|cffffd000|Henchant:{}|h[{}: {}]|h|r",
                    r.spell_id, s.line_name, r.name
                );
                Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(&link)?,
                )]))
            })
        })?,
    )?;

    // StopTradeSkillRepeat() (`0x5da2e0`).
    g.set(
        "StopTradeSkillRepeat",
        lua.create_function(|lua, ()| {
            with_mut(lua, |t| {
                if t.stop_recast() {
                    vec!["UPDATE_TRADESKILL_RECAST"]
                } else {
                    Vec::new()
                }
            });
            Ok(())
        })?,
    )?;

    // GetTradeSkillListLink() (`0x5dc0b0`): the open line's "|Htrade:" link, nil when it cannot
    // link.
    g.set(
        "GetTradeSkillListLink",
        lua.create_function(|lua, ()| {
            with(lua, |t| {
                opt_str(lua, t.state.as_ref().and_then(|s| s.list_link.as_deref()))
            })
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
