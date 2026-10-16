//! The achievement bindings (`0xacfc70`). Each reads the pushed tables and the VM's book; an id
//! the tables lack answers nothing, as the reference's record lookups do.

use std::sync::Arc;

use mlua::{Lua, MultiValue, Table, Value};

use super::text::{build_output, Output, TextCtx};
use super::{
    add_tracked, flag, save_tracked_model, wrong_side, AchievementDb, AchievementModel,
    AchievementRow, CriteriaState, Dated, FEATS_OF_STRENGTH,
};
use crate::script::{Model, ModelCell};

type Values = MultiValue;

fn values(v: Vec<Value>) -> mlua::Result<Values> {
    Ok(MultiValue::from_vec(v))
}

fn none() -> mlua::Result<Values> {
    values(Vec::new())
}

fn num(n: impl Into<f64>) -> Value {
    Value::Number(n.into())
}

fn text(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

/// `lua_isnumber` then `lua_tonumber` truncated, `None` for anything else.
fn number(lua: &Lua, args: &MultiValue, i: usize) -> Option<i64> {
    let v = args.get(i).cloned().unwrap_or(Value::Nil);
    lua.coerce_number(v).ok().flatten().map(|n| n as i64)
}

/// A required number argument, raising the binding's usage line otherwise.
fn number_or_usage(lua: &Lua, args: &MultiValue, i: usize, usage: &str) -> mlua::Result<i64> {
    number(lua, args, i).ok_or_else(|| mlua::Error::RuntimeError(usage.into()))
}

/// A bare `lua_tonumber`, 0 for anything that is not a number.
fn number_or_zero(lua: &Lua, args: &MultiValue, i: usize) -> i64 {
    number(lua, args, i).unwrap_or(0)
}

fn model(lua: &Lua) -> mlua::AppDataRef<'_, ModelCell> {
    lua.app_data_ref::<ModelCell>().expect("model app_data")
}

fn db_of(model: &Model) -> Option<Arc<AchievementDb>> {
    model.achievement.db.clone()
}

/// An id argument as a table key; negative and oversized ids name nothing.
fn id_of(n: i64) -> Option<u32> {
    u32::try_from(n).ok()
}

/// The completed date's three values: month and day 1-based, year since 2000.
fn date_values(d: &Dated) -> [Value; 3] {
    [
        num(d.date.month() + 1),
        num(d.date.month_day() + 1),
        num(d.date.year()),
    ]
}

fn colorblind(model: &Model) -> bool {
    model
        .cvars
        .get("colorblindmode")
        .is_some_and(|s| crate::script::cvars::sstr_to_int(&s.value) != 0)
}

fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

/// `GetCategoryList` (`0x5b1390`) and `GetStatisticsCategoryList` (`0x5b14b0`): the categories
/// outside, or inside, the Statistics tree, file order, into the caller's table or a new one.
fn category_list(lua: &Lua, args: &MultiValue, statistics: bool) -> mlua::Result<Table> {
    let t = match args.front() {
        Some(Value::Table(t)) => t.clone(),
        _ => lua.create_table()?,
    };
    let Some(db) = db_of(&model(lua)) else {
        return Ok(t);
    };
    let mut n = 1;
    for c in db.categories() {
        // The Statistics root itself is in neither list.
        if !statistics && c.id == super::STATISTICS_ROOT {
            continue;
        }
        if db.under_statistics(c.parent) != statistics {
            continue;
        }
        t.set(n, c.id)?;
        n += 1;
    }
    Ok(t)
}

/// `GetCategoryNumAchievements` (`0x5b38e0`): the listed count and the completed count. A series
/// lists its first incomplete step only; a completed step lists unless a later one is complete.
fn category_num(model: &Model, db: &AchievementDb, category: i64, all: bool) -> (i64, i64) {
    let book = &model.achievement.book;
    let faction = AchievementModel::player_faction(model);
    let (mut total, mut pending, mut completed) = (0i64, 0i64, 0i64);
    let mut previous = 0u32;
    for a in db.rows() {
        let statistic = a.flags & flag::STATISTIC != 0;
        let any = (category == -1 && !statistic) || (category == -2 && statistic);
        let include_all = any || all;
        if (i64::from(a.category) != category && !any) || a.flags & flag::HIDDEN != 0 {
            continue;
        }
        let done = book.completed(a.id).is_some();
        let inspected = book.inspected(a.id).is_some();
        if wrong_side(a, faction) && !inspected {
            continue;
        }
        if (a.category == FEATS_OF_STRENGTH || a.flags & flag::EARNED_ONLY != 0)
            && !done
            && !inspected
        {
            continue;
        }
        if previous != 0 && previous == a.supercedes {
            previous = a.id;
            if include_all {
                total += 1;
            } else if inspected {
                pending += 1;
            }
            continue;
        }
        if done {
            if !book.superseded(db, a.id) || include_all {
                completed += 1;
                total += 1;
            }
            continue;
        }
        if !inspected {
            previous = a.id;
            total += 1;
            continue;
        }
        pending += 1;
    }
    let listed = if category == i64::from(FEATS_OF_STRENGTH) {
        pending + completed
    } else {
        pending + total
    };
    (listed, completed)
}

/// `GetNumCompletedAchievements` (`0x5b3ca0`) and its comparison twin (`0x5b3dc0`): real
/// achievements on the side shown, bar Feats of Strength; earned-only rows count once earned.
fn num_completed(db: &AchievementDb, held: &[Dated], faction: Option<i32>) -> (i64, i64) {
    let (mut total, mut completed) = (0, 0);
    for a in db.rows() {
        if !db.is_real(a.id) || a.flags & flag::HIDDEN != 0 || wrong_side(a, faction) {
            continue;
        }
        if a.category == FEATS_OF_STRENGTH {
            continue;
        }
        if held.iter().any(|d| d.id == a.id) {
            completed += 1;
            total += 1;
        } else if a.flags & flag::EARNED_ONLY == 0 {
            total += 1;
        }
    }
    (total, completed)
}

/// One step of `GetAchievementInfo`'s index walk over a held list (`0x5b3fc0`): the -1 and -2
/// pseudo-categories count every non-hidden achievement or statistic, a real category its
/// rows not superseded by a completed one.
fn step(
    db: &AchievementDb,
    book: &super::AchievementBook,
    a: Option<&AchievementRow>,
    category: i64,
    index: &mut i64,
) -> Option<u32> {
    let a = a?;
    let statistic = a.flags & flag::STATISTIC != 0;
    if (category == -1 && !statistic) || (category == -2 && statistic) {
        if a.flags & flag::HIDDEN == 0 {
            *index -= 1;
        }
        return (*index == 0).then_some(a.id);
    }
    if i64::from(a.category) == category && !book.superseded(db, a.id) {
        *index -= 1;
        if *index == 0 {
            return Some(a.id);
        }
    }
    None
}

/// `GetAchievementInfo(category, index)`'s id (`0x5b3fc0`): completed rows also inspected, then
/// completed only, then inspected only, then the table's rows not earned by either.
fn info_index(model: &Model, db: &AchievementDb, category: i64, mut index: i64) -> Option<u32> {
    let book = &model.achievement.book;
    let both = book
        .completed
        .iter()
        .filter(|d| book.inspected(d.id).is_some());
    for d in both {
        if let Some(id) = step(db, book, db.get(d.id), category, &mut index) {
            return Some(id);
        }
    }
    if index > 0 {
        let own = book
            .completed
            .iter()
            .filter(|d| book.inspected(d.id).is_none());
        for d in own {
            if let Some(id) = step(db, book, db.get(d.id), category, &mut index) {
                return Some(id);
            }
        }
        if index > 0 {
            let theirs = book
                .inspect_completed
                .iter()
                .filter(|d| book.completed(d.id).is_none());
            for d in theirs {
                if let Some(id) = step(db, book, db.get(d.id), category, &mut index) {
                    return Some(id);
                }
            }
        }
    }
    if category == i64::from(FEATS_OF_STRENGTH) {
        return None;
    }
    let faction = AchievementModel::player_faction(model);
    let mut previous = 0u32;
    for a in db.rows() {
        if wrong_side(a, faction) {
            continue;
        }
        let statistic = a.flags & flag::STATISTIC != 0;
        let earned = book.completed(a.id).is_some() || book.inspected(a.id).is_some();
        if category == -1 && !statistic && a.category != FEATS_OF_STRENGTH {
            if earned {
                continue;
            }
            if a.flags & flag::EARNED_ONLY == 0 {
                if a.flags & flag::HIDDEN == 0 {
                    index -= 1;
                }
                if index == 0 {
                    return Some(a.id);
                }
            }
            continue;
        }
        if category == -2 && statistic && a.category != FEATS_OF_STRENGTH {
            if a.flags & flag::EARNED_ONLY != 0 {
                continue;
            }
            if a.flags & flag::HIDDEN == 0 {
                index -= 1;
            }
            if index == 0 {
                return Some(a.id);
            }
            continue;
        }
        if i64::from(a.category) != category {
            continue;
        }
        if previous != 0 && previous == a.supercedes {
            previous = a.id;
        } else if !earned && a.flags & flag::EARNED_ONLY == 0 {
            previous = a.id;
            index -= 1;
            if index == 0 {
                return Some(a.id);
            }
        }
    }
    None
}

/// The source's criteria walk shared by the criteria and statistic bindings: the sum, the
/// highest and its criterion, and the count of met criteria (`0x5b58b0`).
struct Walk {
    sum: u64,
    highest: u64,
    highest_id: i64,
    count: u64,
}

fn walk<'a>(
    db: &AchievementDb,
    a: &AchievementRow,
    source: Option<&AchievementRow>,
    held: &'a [CriteriaState],
    mut seen: impl FnMut(&'a CriteriaState),
) -> Walk {
    let mut w = Walk {
        sum: 0,
        highest: 0,
        highest_id: -1,
        count: 0,
    };
    let mut tie = false;
    let Some(source) = source else {
        return w;
    };
    for c in db.criteria_list(source.id) {
        let Some(node) = held.iter().find(|h| h.id == c.id) else {
            continue;
        };
        if a.flags & flag::SUM != 0 {
            w.sum += node.counter;
        }
        if a.flags & flag::MAX != 0 {
            if node.counter > w.highest {
                w.highest = node.counter;
                w.highest_id = c.id.into();
                tie = false;
            } else if node.counter == w.highest {
                tie = true;
            }
        }
        if a.flags & flag::COUNT != 0 && node.counter >= u64::from(c.quantity) {
            w.count += 1;
        }
        seen(node);
    }
    if tie && source.flags & flag::TIE_NAMES_NONE != 0 {
        w.highest_id = 0;
    }
    w
}

/// `GetStatistic` (`0x5b5620`) and `GetComparisonStatistic` (`0x5b5e70`): the statistic's text
/// over the player's or the inspected criteria.
fn statistic(lua: &Lua, args: &MultiValue, inspect: bool, usage: &str) -> mlua::Result<Values> {
    let id = number_or_usage(lua, args, 0, usage)?;
    let m = model(lua);
    let Some(db) = db_of(&m) else { return none() };
    let Some(a) = id_of(id).and_then(|id| db.get(id)) else {
        return none();
    };
    let book = &m.achievement.book;
    let held = if inspect {
        &book.inspect_criteria
    } else {
        &book.criteria
    };
    let mut value = 0u64;
    let mut criteria_flags = 0u32;
    let mut earliest: Option<&CriteriaState> = None;
    let w = walk(&db, a, db.source(a.id), held, |node| {
        value = node.counter;
        if let Some(rec) = db.criterion(node.id) {
            criteria_flags = rec.flags;
        }
        if earliest.is_none_or(|e| node.timer2 < e.timer2) {
            earliest = Some(node);
        }
    });
    let ctx = TextCtx {
        lua,
        colorblind: colorblind(&m),
        now: unix_now(),
    };
    let (s, _) = build_output(
        &ctx,
        &db,
        &Output {
            sum: w.sum,
            highest: w.highest,
            highest_id: w.highest_id,
            count: w.count,
            value,
            progress: earliest,
            flags: a.flags,
            criteria_flags,
            completed: false,
            achievement: a,
            criterion: earliest.and_then(|e| db.criterion(e.id)),
            required: 0,
        },
    );
    values(vec![text(lua, &s)?])
}

/// `GetAchievementCriteriaInfo(achievementID, index)` or `(criteriaID)` (`0x5b58b0`): ten values,
/// description to criteria id.
fn criteria_info(lua: &Lua, args: &MultiValue) -> mlua::Result<Values> {
    const USAGE: &str = "Usage: GetAchievementCrieriaInfo(achievementID, criteriaIndex)";
    let first = number_or_usage(lua, args, 0, USAGE)?;
    let m = model(lua);
    let Some(db) = db_of(&m) else { return none() };
    let mut achievement_id: Option<u32> = None;
    let mut show_all = false;
    let criteria_id = match number(lua, args, 1) {
        Some(mut index) => {
            let Some(a) = id_of(first).and_then(|id| db.get(id)) else {
                return none();
            };
            achievement_id = Some(a.id);
            let Some(source) = db.source(a.id) else {
                return none();
            };
            let mut picked = None;
            for c in db.criteria_list(source.id) {
                if c.flags & 2 == 0 || a.flags & 0xb0 != 0 {
                    if a.flags & 0xb0 != 0 {
                        show_all = true;
                    }
                    index -= 1;
                    if index == 0 {
                        picked = Some(c.id);
                        break;
                    }
                }
            }
            let Some(id) = picked else { return none() };
            id
        }
        None => match id_of(first) {
            Some(id) => id,
            None => return none(),
        },
    };
    let Some(criterion) = db.criterion(criteria_id) else {
        return none();
    };
    let achievement_id = achievement_id.unwrap_or(criterion.achievement);
    let Some(a) = db
        .get(achievement_id)
        .filter(|a| a.flags & flag::HIDDEN == 0)
    else {
        return none();
    };
    let book = &m.achievement.book;
    let progress = book.progress(criteria_id);
    let done = book.completed(achievement_id).is_some();
    let mut w = walk(&db, a, db.source(a.id), &book.criteria, |_| {});
    let mut criteria_flags = criterion.flags;
    let mut required = 0;
    if a.flags & flag::PROGRESS_BAR != 0 {
        criteria_flags |= 1;
        if done && a.flags & flag::SUM != 0 {
            w.sum = criterion.quantity.into();
        }
        if a.flags & flag::COUNT != 0 {
            required = a.minimum_criteria;
        }
    }
    let ctx = TextCtx {
        lua,
        colorblind: colorblind(&m),
        now: unix_now(),
    };
    let (s, shown) = build_output(
        &ctx,
        &db,
        &Output {
            sum: w.sum,
            highest: w.highest,
            highest_id: w.highest_id,
            count: w.count,
            value: progress.map_or(0, |p| p.counter),
            progress,
            flags: a.flags,
            criteria_flags,
            completed: done,
            achievement: a,
            criterion: Some(criterion),
            required,
        },
    );
    let quantity = if required != 0 {
        required
    } else {
        criterion.quantity
    };
    let mut out = vec![text(lua, &criterion.description)?, num(criterion.kind)];
    match progress.filter(|_| !show_all) {
        None => {
            if !done || a.minimum_criteria != 0 {
                out.extend([Value::Boolean(false), num(shown as f64), num(quantity)]);
            } else {
                out.extend([Value::Boolean(true), num(quantity), num(quantity)]);
            }
            out.push(Value::Nil);
        }
        Some(p) => {
            let met =
                !((!done || a.minimum_criteria != 0) && p.counter < u64::from(criterion.quantity));
            out.extend([Value::Boolean(met), num(shown as f64), num(quantity)]);
            out.push(match m.achievement.names.get(&p.player) {
                Some(name) => text(lua, name)?,
                None => Value::Nil,
            });
        }
    }
    out.extend([
        num(criteria_flags),
        num(criterion.asset),
        text(lua, &s)?,
        num(criteria_id),
    ]);
    values(out)
}

/// The met-criteria bits a link carries: one per visible criterion of the source, in order, set
/// when its counter reaches the quantity (`0x5b6110`).
fn criteria_bits(db: &AchievementDb, source: &AchievementRow, held: &[CriteriaState]) -> [u32; 4] {
    let mut bits = [0u32; 4];
    let visible = db.criteria_list(source.id).filter(|c| c.flags & 2 == 0);
    for (i, c) in visible.enumerate() {
        let met = held
            .iter()
            .find(|h| h.id == c.id)
            .is_some_and(|h| h.counter >= u64::from(c.quantity));
        if met && i < 128 {
            bits[i / 32] |= 1 << (i % 32);
        }
    }
    bits
}

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CanShowAchievementUI() (`0x5b0ce0`): a list or a criteria update has arrived.
    g.set(
        "CanShowAchievementUI",
        lua.create_function(|lua, ()| Ok(model(lua).achievement.book.received))?,
    )?;

    // HasCompletedAnyAchievement() (`0x5b1ec0`).
    g.set(
        "HasCompletedAnyAchievement",
        lua.create_function(|lua, ()| Ok(!model(lua).achievement.book.completed.is_empty()))?,
    )?;

    // GetCategoryList([table]) (`0x5b1390`).
    g.set(
        "GetCategoryList",
        lua.create_function(|lua, args: MultiValue| category_list(lua, &args, false))?,
    )?;

    // GetStatisticsCategoryList([table]) (`0x5b14b0`).
    g.set(
        "GetStatisticsCategoryList",
        lua.create_function(|lua, args: MultiValue| category_list(lua, &args, true))?,
    )?;

    // GetCategoryInfo(categoryID) -> name, parentID, flags (`0x5b15c0`); a root's parent is -1.
    g.set(
        "GetCategoryInfo",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(lua, &args, 0, "Usage: GetCategoryInfo(categoryID)")?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let Some(c) = i32::try_from(id).ok().and_then(|id| db.category(id)) else {
                return none();
            };
            let parent = if c.parent == super::STATISTICS_ROOT {
                -1
            } else {
                c.parent
            };
            values(vec![text(lua, &c.name)?, num(parent), num(0)])
        })?,
    )?;

    // GetCategoryNumAchievements(categoryID[, includeAll]) -> listed, completed (`0x5b38e0`).
    g.set(
        "GetCategoryNumAchievements",
        lua.create_function(|lua, args: MultiValue| {
            let category = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetCategoryNumAchievements(categoryID)",
            )?;
            let all = matches!(args.get(1), Some(Value::Boolean(true)));
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let (listed, completed) = category_num(&m, &db, category, all);
            values(vec![num(listed as f64), num(completed as f64)])
        })?,
    )?;

    // GetComparisonCategoryNumAchievements(categoryID) (`0x5b3b30`): the inspected player's
    // completed rows, bar those a later completed step supersedes.
    g.set(
        "GetComparisonCategoryNumAchievements",
        lua.create_function(|lua, args: MultiValue| {
            let category = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetComparisonCategoryNumAchievements(categoryID)",
            )?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let book = &m.achievement.book;
            let faction = AchievementModel::inspect_faction(&m);
            let mut count = 0;
            for a in db.rows() {
                let statistic = a.flags & flag::STATISTIC != 0;
                let all = (category == -1 && !statistic) || (category == -2 && statistic);
                if (i64::from(a.category) != category && !all)
                    || a.flags & flag::HIDDEN != 0
                    || wrong_side(a, faction)
                {
                    continue;
                }
                if book.inspected(a.id).is_some() && (!book.superseded(&db, a.id) || all) {
                    count += 1;
                }
            }
            values(vec![num(count)])
        })?,
    )?;

    // GetAchievementInfo(achievementID) or (categoryID, index) (`0x5b3fc0`): id, name, points,
    // completed, month, day, year, description, flags, icon, rewardText.
    g.set(
        "GetAchievementInfo",
        lua.create_function(|lua, args: MultiValue| {
            let first = number_or_usage(lua, &args, 0, "Usage: GetAchievementInfo(achievementID)")?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let id = match number(lua, &args, 1) {
                Some(index) => match info_index(&m, &db, first, index) {
                    Some(id) => id,
                    None => return none(),
                },
                None => match id_of(first) {
                    Some(id) => id,
                    None => return none(),
                },
            };
            let Some(a) = db.get(id).filter(|a| a.flags & flag::HIDDEN == 0) else {
                return none();
            };
            let done = m.achievement.book.completed(id);
            if a.flags & flag::EARNED_ONLY != 0 && done.is_none() {
                return none();
            }
            let mut out = vec![num(id), text(lua, &a.title)?, num(a.points)];
            match done {
                Some(d) => {
                    out.push(Value::Boolean(true));
                    out.extend(date_values(d));
                }
                None => out.extend([Value::Boolean(false), Value::Nil, Value::Nil, Value::Nil]),
            }
            out.extend([
                text(lua, &a.description)?,
                num(a.flags),
                text(lua, &a.icon)?,
                text(lua, &a.reward)?,
            ]);
            values(out)
        })?,
    )?;

    // GetAchievementInfoFromCriteria(criteriaID) (`0x5b1720`): id, name, points, description,
    // flags, icon, rewardText.
    g.set(
        "GetAchievementInfoFromCriteria",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetAchievementInfoFromCriteria(criteriaID)",
            )?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let Some(c) = id_of(id).and_then(|id| db.criterion(id)) else {
                return none();
            };
            let Some(a) = db.get(c.achievement) else {
                return none();
            };
            values(vec![
                num(c.achievement),
                text(lua, &a.title)?,
                num(a.points),
                text(lua, &a.description)?,
                num(a.flags),
                text(lua, &a.icon)?,
                text(lua, &a.reward)?,
            ])
        })?,
    )?;

    // GetAchievementNumRewards(achievementID) (`0x5b1970`): 1, nil for a hidden or unknown row.
    g.set(
        "GetAchievementNumRewards",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetAchievementNumRewards(achievementID)",
            )?;
            let m = model(lua);
            let shown = db_of(&m)
                .and_then(|db| id_of(id).and_then(|id| db.get(id).map(|a| a.flags)))
                .is_some_and(|f| f & flag::HIDDEN == 0);
            values(vec![if shown { num(1) } else { Value::Nil }])
        })?,
    )?;

    // GetAchievementReward(achievementID, index) (`0x5b1a00`): the points.
    g.set(
        "GetAchievementReward",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: GetAchievementNumRewards(achievementID, rewardIndex)";
            let id = number_or_usage(lua, &args, 0, USAGE)?;
            number_or_usage(lua, &args, 1, USAGE)?;
            let m = model(lua);
            let points = db_of(&m).and_then(|db| {
                id_of(id)
                    .and_then(|id| db.get(id))
                    .filter(|a| a.flags & flag::HIDDEN == 0)
                    .map(|a| a.points)
            });
            values(vec![points.map_or(Value::Nil, num)])
        })?,
    )?;

    // GetAchievementNumCriteria(achievementID) (`0x5b5530`): the visible criteria, one more for a
    // statistic-shaped row.
    g.set(
        "GetAchievementNumCriteria",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetAchievementNumCriteria(achievementID)",
            )?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let Some(a) = id_of(id).and_then(|id| db.get(id)) else {
                return none();
            };
            let mut count = 0;
            if let Some(source) = db.source(a.id).filter(|_| a.flags & flag::HIDDEN == 0) {
                if a.flags & 0xb0 != 0 {
                    count = 1;
                }
                count += db
                    .criteria_list(source.id)
                    .filter(|c| c.flags & 2 == 0)
                    .count();
            }
            values(vec![num(count as f64)])
        })?,
    )?;

    // GetAchievementCriteriaInfo (`0x5b58b0`).
    g.set(
        "GetAchievementCriteriaInfo",
        lua.create_function(|lua, args: MultiValue| criteria_info(lua, &args))?,
    )?;

    // GetPreviousAchievement(achievementID) (`0x5b1690`): the row it supersedes.
    g.set(
        "GetPreviousAchievement",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetPreviousAchievement(achievementID)",
            )?;
            let m = model(lua);
            let prev = db_of(&m)
                .and_then(|db| id_of(id).and_then(|id| db.get(id)).map(|a| a.supercedes))
                .filter(|&p| p != 0);
            match prev {
                Some(p) => values(vec![num(p)]),
                None => none(),
            }
        })?,
    )?;

    // GetNextAchievement(achievementID) -> nextID[, completed] (`0x5b3ef0`): the first row in
    // file order that supersedes it.
    g.set(
        "GetNextAchievement",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(lua, &args, 0, "Usage: GetNextAchievement(achievementID)")?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let Some(next) = db.rows().iter().find(|a| i64::from(a.supercedes) == id) else {
                return none();
            };
            if m.achievement.book.completed(next.id).is_some() {
                values(vec![num(next.id), Value::Boolean(true)])
            } else {
                values(vec![num(next.id)])
            }
        })?,
    )?;

    // GetAchievementCategory(achievementID) (`0x5b18e0`).
    g.set(
        "GetAchievementCategory",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetAchievementCategory(achievementID)",
            )?;
            let m = model(lua);
            let category = db_of(&m).and_then(|db| {
                id_of(id)
                    .and_then(|id| db.get(id))
                    .filter(|a| a.category != 0 && a.flags & flag::HIDDEN == 0)
                    .map(|a| a.category)
            });
            match category {
                Some(c) => values(vec![num(c)]),
                None => none(),
            }
        })?,
    )?;

    // GetAchievementLink(achievementID) (`0x5b6110`): the player's link, every criterion met once
    // completed unless it needs only some.
    g.set(
        "GetAchievementLink",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(lua, &args, 0, "Usage: GetAchievementLink(achievementID)")?;
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let Some(a) = id_of(id).and_then(|id| db.get(id)) else {
                return none();
            };
            let book = &m.achievement.book;
            let source = db.source(a.id);
            let mut bits = source.map_or([0; 4], |s| criteria_bits(&db, s, &book.criteria));
            let done = book.completed(a.id);
            if done.is_some() && source.is_some_and(|s| s.minimum_criteria == 0) {
                bits = [u32::MAX; 4];
            }
            let date = done.map_or(super::WowDate(u32::MAX), |d| d.date);
            let link = super::achievement_hyperlink(
                &db,
                a.id,
                m.achievement.player,
                done.is_some(),
                date,
                bits,
            );
            values(vec![text(lua, &link)?])
        })?,
    )?;

    // GetNumCompletedAchievements() -> total, completed (`0x5b3ca0`).
    g.set(
        "GetNumCompletedAchievements",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let faction = AchievementModel::player_faction(&m);
            let (total, completed) = num_completed(&db, &m.achievement.book.completed, faction);
            values(vec![num(total as f64), num(completed as f64)])
        })?,
    )?;

    // GetNumComparisonCompletedAchievements() (`0x5b3dc0`).
    g.set(
        "GetNumComparisonCompletedAchievements",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            let Some(db) = db_of(&m) else { return none() };
            let faction = AchievementModel::inspect_faction(&m);
            let (total, completed) =
                num_completed(&db, &m.achievement.book.inspect_completed, faction);
            values(vec![num(total as f64), num(completed as f64)])
        })?,
    )?;

    // GetLatestCompletedAchievements() (`0x5b1c20`) and the comparison twin (`0x5b1cf0`): up to
    // five real achievements, newest first.
    for (name, inspect) in [
        ("GetLatestCompletedAchievements", false),
        ("GetLatestCompletedComparisonAchievements", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let m = model(lua);
                let Some(db) = db_of(&m) else { return none() };
                let book = &m.achievement.book;
                let list = if inspect {
                    &book.inspect_completed
                } else {
                    &book.completed
                };
                let ids = list
                    .iter()
                    .filter(|d| db.is_real(d.id))
                    .take(5)
                    .map(|d| num(d.id))
                    .collect();
                values(ids)
            })?,
        )?;
    }

    // GetLatestUpdatedStats() (`0x5b1c90`) and the comparison twin (`0x5b1d60`): up to five
    // statistic criteria, newest first.
    for (name, inspect) in [
        ("GetLatestUpdatedStats", false),
        ("GetLatestUpdatedComparisonStats", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let m = model(lua);
                let book = &m.achievement.book;
                let list = if inspect {
                    &book.inspect_recent
                } else {
                    &book.recent
                };
                values(list.iter().take(5).map(|c| num(c.id)).collect())
            })?,
        )?;
    }

    // GetTotalAchievementPoints() (`0x5b1dc0`) and GetComparisonAchievementPoints() (`0x5b1e40`).
    for (name, inspect) in [
        ("GetTotalAchievementPoints", false),
        ("GetComparisonAchievementPoints", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let m = model(lua);
                let Some(db) = db_of(&m) else {
                    return values(vec![num(0)]);
                };
                let book = &m.achievement.book;
                let list = if inspect {
                    &book.inspect_completed
                } else {
                    &book.completed
                };
                let points: u32 = list
                    .iter()
                    .filter_map(|d| db.get(d.id))
                    .map(|a| a.points)
                    .sum();
                values(vec![num(points)])
            })?,
        )?;
    }

    // GetStatistic(achievementID) (`0x5b5620`) and GetComparisonStatistic (`0x5b5e70`).
    g.set(
        "GetStatistic",
        lua.create_function(|lua, args: MultiValue| {
            statistic(lua, &args, false, "Usage: GetStatistic(achievementID)")
        })?,
    )?;
    g.set(
        "GetComparisonStatistic",
        lua.create_function(|lua, args: MultiValue| {
            statistic(
                lua,
                &args,
                true,
                "Usage: GetComparisonStatistic(achievementID)",
            )
        })?,
    )?;

    // SetAchievementComparisonUnit(unit) (`0x5b1ab0`): 1 and a query for a unit naming someone.
    g.set(
        "SetAchievementComparisonUnit",
        lua.create_function(|lua, args: MultiValue| {
            let v = args.front().cloned().unwrap_or(Value::Nil);
            let token = crate::script::binding_abi::string_arg(
                lua,
                v,
                "Usage: AddAchievementComparisonUnit(unitToken)",
            )?;
            let mut m = crate::script::model_mut_of(lua);
            let Some(guid) = m.unit_guids.guid_of(&token).filter(|&g| g != 0) else {
                return none();
            };
            m.achievement.book.inspect_guid = guid;
            m.achievement.inspect_queries.push(guid);
            values(vec![num(1)])
        })?,
    )?;

    // ClearAchievementComparisonUnit() (`0x5b4760`): the inspected lists empty.
    g.set(
        "ClearAchievementComparisonUnit",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua)
                .achievement
                .book
                .release_inspect();
            Ok(())
        })?,
    )?;

    // GetAchievementComparisonInfo(achievementID) -> completed, month, day, year, flags
    // (`0x5b4770`).
    g.set(
        "GetAchievementComparisonInfo",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_usage(
                lua,
                &args,
                0,
                "Usage: GetAchievementComparisonInfo(achievementID)",
            )?;
            let m = model(lua);
            let book = &m.achievement.book;
            if book.inspect_guid == 0 {
                return none();
            }
            let Some(db) = db_of(&m) else { return none() };
            let (Some(d), Some(a)) = (
                id_of(id).and_then(|id| book.inspected(id)),
                id_of(id).and_then(|id| db.get(id)),
            ) else {
                return none();
            };
            let mut out = vec![Value::Boolean(true)];
            out.extend(date_values(d));
            out.push(num(a.flags));
            values(out)
        })?,
    )?;

    // GetTrackedAchievements() (`0x5b2040`), add order.
    g.set(
        "GetTrackedAchievements",
        lua.create_function(|lua, ()| {
            values(
                model(lua)
                    .achievement
                    .tracked
                    .iter()
                    .map(|&id| num(id))
                    .collect(),
            )
        })?,
    )?;

    // GetNumTrackedAchievements() (`0x5b2120`).
    g.set(
        "GetNumTrackedAchievements",
        lua.create_function(|lua, ()| Ok(model(lua).achievement.tracked.len() as f64))?,
    )?;

    // IsTrackedAchievement(achievementID) (`0x5b2090`).
    g.set(
        "IsTrackedAchievement",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_zero(lua, &args, 0);
            Ok(id != 0
                && model(lua)
                    .achievement
                    .tracked
                    .iter()
                    .any(|&t| i64::from(t) == id))
        })?,
    )?;

    // AddTrackedAchievement(achievementID) (`0x5b4870`): raises for 0 or an unknown id; announces
    // the id when held or added.
    g.set(
        "AddTrackedAchievement",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_zero(lua, &args, 0);
            let added = {
                let mut m = crate::script::model_mut_of(lua);
                let known =
                    db_of(&m).is_some_and(|db| id_of(id).is_some_and(|id| db.get(id).is_some()));
                let Some(id) = id_of(id).filter(|&id| id != 0 && known) else {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: AddTrackedAchievement(achievementID)".into(),
                    ));
                };
                let added = add_tracked(&mut m.achievement.tracked, id);
                if added {
                    save_tracked_model(&mut m);
                }
                added.then_some(id)
            };
            if let Some(id) = added {
                crate::script::tick::fire_event_into(
                    lua,
                    "TRACKED_ACHIEVEMENT_UPDATE",
                    vec![crate::script::ScriptValue::Int(id.into())],
                );
            }
            Ok(())
        })?,
    )?;

    // RemoveTrackedAchievement(achievementID) (`0x5b48d0`): raises for 0; announces a removal.
    g.set(
        "RemoveTrackedAchievement",
        lua.create_function(|lua, args: MultiValue| {
            let id = number_or_zero(lua, &args, 0);
            if id == 0 {
                return Err(mlua::Error::RuntimeError(
                    "Usage: RemoveTrackedAchievement(achievementID)".into(),
                ));
            }
            let removed = {
                let mut m = crate::script::model_mut_of(lua);
                let at = m
                    .achievement
                    .tracked
                    .iter()
                    .position(|&t| i64::from(t) == id);
                if let Some(at) = at {
                    m.achievement.tracked.remove(at);
                    save_tracked_model(&mut m);
                }
                at.is_some()
            };
            if removed {
                crate::script::tick::fire_event_into(
                    lua,
                    "TRACKED_ACHIEVEMENT_UPDATE",
                    vec![crate::script::ScriptValue::Int(id)],
                );
            }
            Ok(())
        })?,
    )?;

    Ok(())
}
