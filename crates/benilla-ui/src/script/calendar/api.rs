//! The calendar's Lua bindings, 12340's `s_ScriptFunctions` (`0xad0030`, registered by
//! `CalendarRegisterScriptFunctions` `0x5b8c30`). Days and indices are 1-based, the month offset
//! -1, 0 or 1 (the shown month's neighbours).

use mlua::{Lua, MultiValue, Value};

use super::engine::{calendar_type, kind, mark, max_create_date, CalendarModel, DayEvent};
use super::time::WowTime;
use super::{fire_signals, with_calendar};
use crate::script::get_text::global_text;
use crate::script::{Model, ModelCell};

/// `s_monthNames` (`0xacffb4`).
const MONTH_NAMES: [&str; 12] = [
    "MONTH_JANUARY",
    "MONTH_FEBRUARY",
    "MONTH_MARCH",
    "MONTH_APRIL",
    "MONTH_MAY",
    "MONTH_JUNE",
    "MONTH_JULY",
    "MONTH_AUGUST",
    "MONTH_SEPTEMBER",
    "MONTH_OCTOBER",
    "MONTH_NOVEMBER",
    "MONTH_DECEMBER",
];

/// `s_weekdayNames` (`0xad0014`).
const WEEKDAY_NAMES: [&str; 7] = [
    "WEEKDAY_SUNDAY",
    "WEEKDAY_MONDAY",
    "WEEKDAY_TUESDAY",
    "WEEKDAY_WEDNESDAY",
    "WEEKDAY_THURSDAY",
    "WEEKDAY_FRIDAY",
    "WEEKDAY_SATURDAY",
];

/// `s_eventTypeStrings` (`0xad1694`).
const EVENT_TYPES: [&str; 5] = [
    "CALENDAR_TYPE_RAID",
    "CALENDAR_TYPE_DUNGEON",
    "CALENDAR_TYPE_PVP",
    "CALENDAR_TYPE_MEETING",
    "CALENDAR_TYPE_OTHER",
];

/// `s_eventRepeatStrings` (`0xad1684`).
const REPEAT_OPTIONS: [&str; 4] = [
    "CALENDAR_REPEAT_NEVER",
    "CALENDAR_REPEAT_WEEKLY",
    "CALENDAR_REPEAT_BIWEEKLY",
    "CALENDAR_REPEAT_MONTHLY",
];

/// `s_eventStatusStrings` (`0xad16a8`); status 7 has none (`GetEventStatusString` `0x5f1000`).
const STATUS_STRINGS: [&str; 9] = [
    "CALENDAR_STATUS_INVITED",
    "CALENDAR_STATUS_ACCEPTED",
    "CALENDAR_STATUS_DECLINED",
    "CALENDAR_STATUS_CONFIRMED",
    "CALENDAR_STATUS_OUT",
    "CALENDAR_STATUS_STANDBY",
    "CALENDAR_STATUS_SIGNEDUP",
    "CALENDAR_STATUS_NOT_SIGNEDUP",
    "CALENDAR_STATUS_TENTATIVE",
];

/// The invite sort's criteria by number (`0x5b8570`).
const SORT_CRITERIA: [&str; 6] = ["name", "level", "class", "status", "party", "notes"];

/// `s_expansionMaxLevels` (`0xa18684`), `CalendarDefaultGuildFilter`'s level bounds.
const EXPANSION_MAX_LEVELS: [f64; 3] = [60.0, 70.0, 80.0];

fn model(lua: &Lua) -> mlua::AppDataRef<'_, ModelCell> {
    lua.app_data_ref::<ModelCell>().expect("model app_data")
}

/// Run on the calendar with its context, then fire what it signalled.
fn run<R>(lua: &Lua, f: impl FnOnce(&mut CalendarModel, &super::Ctx) -> R) -> R {
    let r = with_calendar(&mut crate::script::model_mut_of(lua), f);
    fire_signals(lua);
    r
}

/// `lua_isnumber` then `lua_tonumber`.
fn num(lua: &Lua, v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| lua.coerce_number(v.clone()).ok().flatten())
}

/// A double cast to the binding's `int32_t` or `uint32_t`, as `__ftol2` truncates.
fn int(n: f64) -> i32 {
    crate::script::binding_abi::ftol(n)
}

/// A 1-based argument made 0-based, wrapping as the reference's unsigned subtraction does.
fn index0(n: f64) -> u32 {
    (int(n) as u32).wrapping_sub(1)
}

fn usage(text: &'static str) -> mlua::Error {
    mlua::Error::RuntimeError(text.into())
}

/// `(month offset, 0-based day, 0-based index)` from three numbers, else the usage raises.
fn day_index(lua: &Lua, a: &MultiValue, text: &'static str) -> mlua::Result<(i32, u32, u32)> {
    day_index_opt(lua, a).ok_or_else(|| usage(text))
}

fn day_index_opt(lua: &Lua, a: &MultiValue) -> Option<(i32, u32, u32)> {
    let (o, d, i) = (num(lua, a.front()), num(lua, a.get(1)), num(lua, a.get(2))).zip3()?;
    Some((int(o), index0(d), index0(i)))
}

trait Zip3<T> {
    fn zip3(self) -> Option<(T, T, T)>;
}

impl<T> Zip3<T> for (Option<T>, Option<T>, Option<T>) {
    fn zip3(self) -> Option<(T, T, T)> {
        Some((self.0?, self.1?, self.2?))
    }
}

fn s(lua: &Lua, text: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(text)?))
}

fn opt_s(lua: &Lua, text: Option<&str>) -> mlua::Result<Value> {
    match text {
        Some(t) => s(lua, t),
        None => Ok(Value::Nil),
    }
}

fn n(v: impl Into<f64>) -> Value {
    Value::Number(v.into())
}

fn b(v: bool) -> Value {
    Value::Boolean(v)
}

/// `1` or nil, as the reference's `lua_pushnumber(1.0)` and `lua_pushnil` pairs answer.
fn one_or_nil(v: bool) -> Value {
    if v {
        n(1.0)
    } else {
        Value::Nil
    }
}

/// A date's four Lua fields: weekday, month, day, year.
fn push_date(out: &mut Vec<Value>, d: &WowTime) {
    out.extend([
        n(d.weekday + 1),
        n(d.month + 1),
        n(d.month_day + 1),
        n(d.year + 2000),
    ]);
}

fn date4(d: &WowTime) -> MultiValue {
    let mut out = Vec::with_capacity(4);
    push_date(&mut out, d);
    MultiValue::from_vec(out)
}

/// `GetSequenceLabel` (`0x5b8040`): a holiday's run position.
fn sequence_label(e: &DayEvent) -> &'static str {
    if e.kind & kind::HOLIDAY == 0 || e.sequence_length <= 1 {
        ""
    } else if e.sequence_index == 0 {
        "START"
    } else if e.sequence_index == e.sequence_length - 1 {
        "END"
    } else {
        "ONGOING"
    }
}

/// `GetInviteRoleLabel` (`0x5b8080`).
fn role_label(e: &DayEvent) -> &'static str {
    if e.flags & mark::CREATOR != 0 {
        "CREATOR"
    } else if e.flags & mark::MODERATOR != 0 {
        "MODERATOR"
    } else {
        ""
    }
}

/// `IsSignUpInvite` (`0x5b80b0`): a guild event the player has not been invited to.
fn is_sign_up(e: &DayEvent) -> bool {
    e.kind & kind::GUILD_EVENT != 0 && e.flags & mark::SIGN_UP != 0
}

/// A lockout's or reset's difficulty token through `MapDifficulty.dbc`, as `GetText` reads it.
fn difficulty_name(lua: &Lua, model: &Model, map: u32, difficulty: u32) -> String {
    model
        .lfg
        .catalog
        .map_difficulty(map as i32, difficulty as i32)
        .map(|d| global_text(lua, &d.difficulty_string))
        .unwrap_or_default()
}

/// `PushDayEventTexture` (`0x5c0fb0`): a holiday's texture, a fetched event's dungeon texture.
fn day_texture(model: &Model, e: &DayEvent) -> String {
    let cal = &model.calendar;
    if e.kind & kind::HOLIDAY != 0 {
        if let Some(t) = cal.dynamic_texture(e.event_id as u32) {
            return t.to_string();
        }
        if let Some(h) = cal.holiday(e.event_id as u32) {
            return h.texture.clone();
        }
    } else if e.kind & kind::FETCHED != 0 && e.texture >= 0 {
        if let Some(d) = model.lfg.catalog.dungeon(e.texture) {
            return d.texture.clone();
        }
    }
    String::new()
}

/// A creator's name for a Lua answer; one not yet cached is asked for and repaints on landing.
fn creator_name(lua: &Lua, guid: u64) -> String {
    if let Some(name) = model(lua).calendar.names.get(&guid) {
        return name.name.clone();
    }
    crate::script::model_mut_of(lua).calendar.name_or_ask(guid)
}

/// The rights the open event's setters test (`0x5bb22c`): an announcement takes the guild
/// right, anything else a moderator.
fn can_edit(lua: &Lua) -> bool {
    with_calendar(&mut crate::script::model_mut_of(lua), |c, ctx| {
        c.can_edit_current(ctx)
    })
}

/// Edit the open event when the player may (`0x5bb210`-`0x5bb930`).
fn edit(lua: &Lua, f: impl FnOnce(&mut super::event::CurrentEvent)) {
    if !can_edit(lua) {
        return;
    }
    if let Some(cur) = crate::script::model_mut_of(lua).calendar.current.as_mut() {
        f(cur);
    }
}

fn set(lua: &Lua, name: &str, f: mlua::Function) -> mlua::Result<()> {
    lua.globals().set(name, f)
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    install_dates(lua)?;
    install_days(lua)?;
    install_event(lua)?;
    install_context(lua)?;
    install_editor(lua)?;
    Ok(())
}

fn install_dates(lua: &Lua) -> mlua::Result<()> {
    // CalendarGetMonthNames() (`0x5b80e0`) and CalendarGetWeekdayNames() (`0x5b8120`).
    set(
        lua,
        "CalendarGetMonthNames",
        lua.create_function(|lua, ()| {
            MONTH_NAMES
                .iter()
                .map(|t| s(lua, &global_text(lua, t)))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    set(
        lua,
        "CalendarGetWeekdayNames",
        lua.create_function(|lua, ()| {
            WEEKDAY_NAMES
                .iter()
                .map(|t| s(lua, &global_text(lua, t)))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    // CalendarGetDate() (`0x5b8160`): today on the game clock.
    set(
        lua,
        "CalendarGetDate",
        lua.create_function(|lua, ()| Ok(date4(&model(lua).calendar.now)))?,
    )?;
    // CalendarGetMinDate() (`0x5b81f0`): the 24th of November 2004.
    set(
        lua,
        "CalendarGetMinDate",
        lua.create_function(|_, ()| {
            let mut d = WowTime::date(10, 23, 4);
            d.add_days(0, false);
            Ok(date4(&d))
        })?,
    )?;
    // CalendarGetMaxDate() (`0x5b82a0`): the last day of 2030.
    set(
        lua,
        "CalendarGetMaxDate",
        lua.create_function(|_, ()| {
            let mut d = WowTime::date(0, 0, 31);
            d.add_days(-1, false);
            Ok(date4(&d))
        })?,
    )?;
    // CalendarGetMinHistoryDate() (`0x5b8350`): two weeks back.
    set(
        lua,
        "CalendarGetMinHistoryDate",
        lua.create_function(|lua, ()| {
            let now = model(lua).calendar.now;
            let mut d = WowTime {
                weekday: now.weekday,
                ..WowTime::date(now.month, now.month_day, now.year)
            };
            d.add_days(-14, false);
            Ok(date4(&d))
        })?,
    )?;
    // CalendarGetMaxCreateDate() (`0x5b8410`).
    set(
        lua,
        "CalendarGetMaxCreateDate",
        lua.create_function(|lua, ()| Ok(date4(&max_create_date(&model(lua).calendar.now))))?,
    )?;
    // CalendarGetMonth([offset]) (`0x5b99f0`): month, year, days, weekday of the 1st.
    set(
        lua,
        "CalendarGetMonth",
        lua.create_function(|lua, a: MultiValue| {
            let offset = num(lua, a.front()).map_or(0, int);
            let (month, year, days, weekday) = model(lua).calendar.month(offset);
            Ok((n(month + 1), n(year + 2000), n(days as u32), n(weekday + 1)))
        })?,
    )?;
    // CalendarGetAbsMonth(month[, year]) (`0x5b9ac0`).
    set(
        lua,
        "CalendarGetAbsMonth",
        lua.create_function(|lua, a: MultiValue| {
            let month = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarGetAbsMonth(month[, year])"))?;
            let mut m = crate::script::model_mut_of(lua);
            let mut year = m.calendar.abs_year_or_now();
            if let Some(y) = num(lua, a.get(1)) {
                year = int(y) - 2000;
            }
            let (month, year, days, weekday) = m.calendar.clamp_abs_month(int(month) - 1, year);
            Ok((n(month + 1), n(year + 2000), n(days as u32), n(weekday + 1)))
        })?,
    )?;
    // CalendarSetMonth(offsetMonths) (`0x5bcc00`).
    set(
        lua,
        "CalendarSetMonth",
        lua.create_function(|lua, a: MultiValue| {
            let offset = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarSetMonth(offsetMonths)"))?;
            run(lua, |c, ctx| {
                let (month, year, days, _) = c.month(int(offset));
                c.show_month(ctx, month, year, days);
            });
            Ok(())
        })?,
    )?;
    // CalendarSetAbsMonth(month[, year]) (`0x5c43b0`).
    set(
        lua,
        "CalendarSetAbsMonth",
        lua.create_function(|lua, a: MultiValue| {
            let month = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarSetAbsMonth(month[, year])"))?;
            let year_arg = num(lua, a.get(1));
            run(lua, |c, ctx| {
                let mut year = c.abs_year_or_now();
                if let Some(y) = year_arg {
                    year = int(y) - 2000;
                }
                let (month, year, days, _) = c.clamp_abs_month(int(month) - 1, year);
                c.show_month(ctx, month, year, days);
            });
            Ok(())
        })?,
    )?;
    Ok(())
}

fn install_days(lua: &Lua) -> mlua::Result<()> {
    // CalendarGetNumDayEvents(offset, monthDay) (`0x5b9bf0`).
    set(
        lua,
        "CalendarGetNumDayEvents",
        lua.create_function(|lua, a: MultiValue| {
            let (Some(o), Some(d)) = (num(lua, a.front()), num(lua, a.get(1))) else {
                return Err(usage("Usage: CalendarGetNumDayEvents([-1,0,1], monthDay)"));
            };
            Ok(model(lua).calendar.num_day_events(int(o), index0(d)) as f64)
        })?,
    )?;
    // CalendarGetFirstPendingInvite(offset, monthDay) (`0x5b9e10`): 1-based, 0 none.
    set(
        lua,
        "CalendarGetFirstPendingInvite",
        lua.create_function(|lua, a: MultiValue| {
            let (Some(o), Some(d)) = (num(lua, a.front()), num(lua, a.get(1))) else {
                return Err(usage("Usage: CalendarGetDay([-1,0,1], monthDay)"));
            };
            Ok(i64::from(
                model(lua).calendar.first_pending(int(o), index0(d)) + 1,
            ))
        })?,
    )?;
    // CalendarGetDayEvent(offset, monthDay, index) (`0x5c1070`): fifteen values.
    set(
        lua,
        "CalendarGetDayEvent",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarGetDayEvent([-1.0.1], monthDay, index)",
            )?;
            let e = model(lua).calendar.day_event(o, d, i).cloned();
            let Some(e) = e else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    n(0.0),
                    n(0.0),
                    s(lua, "")?,
                    s(lua, "")?,
                    n(0.0),
                    s(lua, "")?,
                    s(lua, "")?,
                    n(0.0),
                    s(lua, "")?,
                    n(0.0),
                    n(0.0),
                    n(0.0),
                    n(0.0),
                    s(lua, "")?,
                ]));
            };
            let start = WowTime::decode(e.start);
            let lockout = e.kind & (kind::RAID_LOCKOUT | kind::RAID_RESET) != 0;
            let (title, texture, difficulty) = {
                let m = model(lua);
                let title = if e.kind & kind::HOLIDAY != 0 {
                    m.calendar
                        .holiday(e.event_id as u32)
                        .and_then(|h| h.name.clone())
                } else if lockout {
                    Some(
                        m.calendar
                            .map_name(e.holiday_id)
                            .unwrap_or_default()
                            .to_string(),
                    )
                } else {
                    // The profanity filter is not modelled.
                    Some(e.title.clone())
                };
                let difficulty = if lockout {
                    difficulty_name(lua, &m, e.holiday_id, e.end)
                } else {
                    String::new()
                };
                (title, day_texture(&m, &e), difficulty)
            };
            let creator = creator_name(lua, e.creator);
            Ok(MultiValue::from_vec(vec![
                opt_s(lua, title.as_deref())?,
                n(start.hour),
                n(start.minute),
                s(lua, calendar_type(e.kind))?,
                s(lua, sequence_label(&e))?,
                n(e.event_type + 1),
                s(lua, &texture)?,
                s(lua, role_label(&e))?,
                n(i32::from(e.invite_status) + 1),
                s(lua, &creator)?,
                n(if lockout {
                    (e.end as i32 + 1) as f64
                } else {
                    0.0
                }),
                n(i32::from(is_sign_up(&e)) + 1),
                n(e.sequence_index + 1),
                n(e.sequence_length),
                s(lua, &difficulty)?,
            ]))
        })?,
    )?;
    // CalendarGetDayEventSequenceInfo(offset, monthDay, index) (`0x5b9ca0`).
    set(
        lua,
        "CalendarGetDayEventSequenceInfo",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarGetDayEventSequenceInfo([-1.0.1], monthDay, index)",
            )?;
            let m = model(lua);
            Ok(match m.calendar.day_event(o, d, i) {
                Some(e) => (
                    n(e.sequence_index),
                    n(e.sequence_length),
                    s(lua, sequence_label(e))?,
                ),
                None => (n(0.0), n(0.0), s(lua, "")?),
            })
        })?,
    )?;
    // CalendarOpenEvent(offset, monthDay, index) (`0x5b9eb0`).
    set(
        lua,
        "CalendarOpenEvent",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarOpenEvent([-1,0,1], monthDay, index)",
            )?;
            run(lua, |c, _| c.open_event(o, d, i));
            Ok(())
        })?,
    )?;
    // CalendarGetEventIndex() and CalendarContextGetEventIndex() (`0x5b9f80`, `0x5ba680`).
    for (name, context) in [
        ("CalendarGetEventIndex", false),
        ("CalendarContextGetEventIndex", true),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, ()| {
                Ok(match model(lua).calendar.event_index(context) {
                    Some((month, day, index)) => {
                        (i64::from(month), i64::from(day) + 1, i64::from(index) + 1)
                    }
                    None => (0, 0, 0),
                })
            })?,
        )?;
    }
    // CalendarCloseEvent() (`0x5b84a0`): only the selection clears.
    set(
        lua,
        "CalendarCloseEvent",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).calendar.close_selected();
            Ok(())
        })?,
    )?;
    // CalendarGetHolidayInfo(offset, monthDay, index) (`0x5c1480`): name, description, texture.
    set(
        lua,
        "CalendarGetHolidayInfo",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarGetHolidayInfo([-1,0,1], monthDay, index)",
            )?;
            let m = model(lua);
            let found = m.calendar.day_event(o, d, i).and_then(|e| {
                let h = m.calendar.holiday(e.event_id as u32)?;
                let texture = m
                    .calendar
                    .dynamic_texture(h.id)
                    .map_or_else(|| h.texture.clone(), str::to_string);
                Some((h.name.clone()?, h.description.clone()?, texture))
            });
            Ok(match found {
                Some((name, description, texture)) => {
                    (s(lua, &name)?, s(lua, &description)?, s(lua, &texture)?)
                }
                None => (Value::Nil, Value::Nil, Value::Nil),
            })
        })?,
    )?;
    // CalendarGetRaidInfo(offset, monthDay, index) (`0x5ba000`): seven values.
    set(
        lua,
        "CalendarGetRaidInfo",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarGetRaidInfo([-1,0,1], monthDay, index)",
            )?;
            let m = model(lua);
            let Some(e) = m.calendar.day_event(o, d, i) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    s(lua, "")?,
                    n(0.0),
                    n(0.0),
                    n(0.0),
                    n(0.0),
                    s(lua, "")?,
                ]));
            };
            let start = WowTime::decode(e.start);
            Ok(MultiValue::from_vec(vec![
                s(lua, m.calendar.map_name(e.holiday_id).unwrap_or_default())?,
                s(lua, calendar_type(e.kind))?,
                n(e.event_id as i32),
                n(start.hour),
                n(start.minute),
                n(e.end as i32 + 1),
                s(lua, &difficulty_name(lua, &m, e.holiday_id, e.end))?,
            ]))
        })?,
    )?;
    // CalendarGetNumPendingInvites() (`0x5bf830`).
    set(
        lua,
        "CalendarGetNumPendingInvites",
        lua.create_function(|lua, ()| Ok(f64::from(model(lua).calendar.count_pending())))?,
    )?;
    // OpenCalendar() (`0x5bd7e0`): the first open fetches the calendar.
    set(
        lua,
        "OpenCalendar",
        lua.create_function(|lua, ()| {
            run(lua, |c, _| {
                if !c.opened() {
                    c.get_calendar();
                }
            });
            Ok(())
        })?,
    )?;
    // CalendarIsActionPending() (`0x5b8c10`), CalendarCanAddEvent() (`0x5b8ba0`) and
    // CalendarCanSendInvite() (`0x5b8b30`).
    set(
        lua,
        "CalendarIsActionPending",
        lua.create_function(|lua, ()| Ok(b(model(lua).calendar.action_pending)))?,
    )?;
    set(
        lua,
        "CalendarCanAddEvent",
        lua.create_function(|lua, ()| {
            Ok(b(with_calendar(
                &mut crate::script::model_mut_of(lua),
                |c, ctx| c.can_add_event(ctx),
            )))
        })?,
    )?;
    set(
        lua,
        "CalendarCanSendInvite",
        lua.create_function(|lua, ()| {
            Ok(b(with_calendar(
                &mut crate::script::model_mut_of(lua),
                |c, ctx| c.can_send_invite(ctx),
            )))
        })?,
    )?;
    Ok(())
}

fn install_event(lua: &Lua) -> mlua::Result<()> {
    // CalendarGetEventInfo() (`0x5bd8a0`): twenty-five values.
    set(
        lua,
        "CalendarGetEventInfo",
        lua.create_function(|lua, ()| {
            let cur = model(lua).calendar.current.clone();
            let Some(cur) = cur else {
                let mut out = vec![Value::Nil, Value::Nil, s(lua, "")?];
                out.extend(std::iter::repeat_n(n(0.0), 16));
                out.extend([Value::Nil, Value::Nil, Value::Nil, n(0.0), n(0.0)]);
                out.push(s(lua, "")?);
                return Ok(MultiValue::from_vec(out));
            };
            let creator = creator_name(lua, cur.creator);
            let m = model(lua);
            let me = m.calendar.player.as_ref().map_or(0, |p| p.guid);
            let texture = m.calendar.texture_index(cur.event_type, cur.texture);
            let pending = m.calendar.has_pending(cur.event_id);
            let (status, invite_type) = match cur.invite_of(me).filter(|_| me != 0) {
                Some(i) => (f64::from(i.status) + 1.0, f64::from(i.invite_type) + 1.0),
                None if cur.flags & kind::GUILD_EVENT != 0 => (8.0, 2.0),
                None => (0.0, 0.0),
            };
            // The profanity filter is not modelled.
            let mut out = vec![
                s(lua, &cur.title)?,
                s(lua, &cur.description)?,
                s(lua, &creator)?,
                n(cur.event_type as i32 + 1),
                n(cur.repeat as i32 + 1),
                n(cur.max_size),
                n(texture + 1),
            ];
            push_date(&mut out, &cur.date);
            out.extend([n(cur.date.hour), n(cur.date.minute)]);
            push_date(&mut out, &cur.lockout_date);
            out.extend([n(cur.lockout_date.hour), n(cur.lockout_date.minute)]);
            out.extend([
                one_or_nil(cur.flags & kind::LOCKED != 0),
                one_or_nil(cur.flags & kind::AUTO_APPROVE != 0),
                one_or_nil(pending),
                n(status),
                n(invite_type),
                s(lua, calendar_type(cur.flags))?,
            ]);
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    // CalendarEventGetNumInvites() (`0x5b84d0`).
    set(
        lua,
        "CalendarEventGetNumInvites",
        lua.create_function(|lua, ()| {
            Ok(model(lua)
                .calendar
                .current
                .as_ref()
                .map_or(0.0, |c| c.invites.len() as f64))
        })?,
    )?;
    // CalendarEventGetInvite(index) (`0x5ba220`): nine values.
    set(
        lua,
        "CalendarEventGetInvite",
        lua.create_function(|lua, a: MultiValue| {
            let index =
                num(lua, a.front()).ok_or_else(|| usage("Usage: CalendarEventGetInvite(index)"))?;
            let invite = model(lua)
                .calendar
                .current
                .as_ref()
                .and_then(|c| c.invites.get(index0(index) as usize).cloned());
            let Some(invite) = invite else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    n(0.0),
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    b(false),
                    Value::Integer(0),
                    Value::Nil,
                ]));
            };
            let m = model(lua);
            let named = m.calendar.names.get(&invite.guid);
            let me = m.calendar.player.as_ref().map_or(0, |p| p.guid);
            let (class_name, class_file) = named
                .filter(|c| !c.class_file.is_empty())
                .map_or((String::new(), String::new()), |c| {
                    (c.class_name.clone(), c.class_file.clone())
                });
            Ok(MultiValue::from_vec(vec![
                s(lua, named.map_or("", |c| c.name.as_str()))?,
                n(invite.level),
                s(lua, &class_name)?,
                s(lua, &class_file)?,
                Value::Integer(i64::from(invite.status) + 1),
                s(
                    lua,
                    match invite.moderator {
                        1 => "MODERATOR",
                        2 => "CREATOR",
                        _ => "",
                    },
                )?,
                b(me != 0 && me == invite.guid),
                Value::Integer(i64::from(invite.invite_type) + 1),
                s(lua, &invite.notes)?,
            ]))
        })?,
    )?;
    // CalendarEventGetInviteResponseTime(index) (`0x5ba420`): six values, zeros unanswered.
    set(
        lua,
        "CalendarEventGetInviteResponseTime",
        lua.create_function(|lua, a: MultiValue| {
            let index = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventGetInviteResponseTime(index)"))?;
            let time = model(lua)
                .calendar
                .current
                .as_ref()
                .and_then(|c| c.invites.get(index0(index) as usize))
                .map_or(0, |i| i.response_time);
            if time == 0 {
                return Ok(MultiValue::from_vec(vec![n(0.0); 6]));
            }
            let d = WowTime::decode(time);
            let mut out = Vec::with_capacity(6);
            push_date(&mut out, &d);
            out.extend([n(d.hour), n(d.minute)]);
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    // CalendarEventGetSelectedInvite() (`0x5bbb00`): 1-based, 0 none.
    set(
        lua,
        "CalendarEventGetSelectedInvite",
        lua.create_function(|lua, ()| {
            Ok(model(lua)
                .calendar
                .current
                .as_ref()
                .map_or(0, |c| i64::from(c.selected_index()) + 1))
        })?,
    )?;
    // CalendarEventSelectInvite(index) (`0x5b8ae0`).
    set(
        lua,
        "CalendarEventSelectInvite",
        lua.create_function(|lua, a: MultiValue| {
            let index = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventSelectInvite(index)"))?;
            if let Some(cur) = crate::script::model_mut_of(lua).calendar.current.as_mut() {
                cur.select_invite((index as i64 as u32).wrapping_sub(1));
            }
            Ok(())
        })?,
    )?;
    // CalendarEventHasPendingInvite() (`0x5bdd90`), HaveSettingsChanged (`0x5b8a10`),
    // IsModerator (`0x5b88b0`), CanEdit (`0x5b8a50`) and GetCalendarType (`0x5b8aa0`).
    set(
        lua,
        "CalendarEventHasPendingInvite",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            Ok(b(m
                .calendar
                .current
                .as_ref()
                .is_some_and(|c| m.calendar.has_pending(c.event_id))))
        })?,
    )?;
    set(
        lua,
        "CalendarEventHaveSettingsChanged",
        lua.create_function(|lua, ()| {
            Ok(b(model(lua)
                .calendar
                .current
                .as_ref()
                .is_some_and(|c| c.changed)))
        })?,
    )?;
    set(
        lua,
        "CalendarEventIsModerator",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            let me = m.calendar.player.as_ref().map_or(0, |p| p.guid);
            Ok(b(m
                .calendar
                .current
                .as_ref()
                .is_some_and(|c| c.is_moderator(me))))
        })?,
    )?;
    set(
        lua,
        "CalendarEventCanEdit",
        lua.create_function(|lua, ()| Ok(b(can_edit(lua))))?,
    )?;
    set(
        lua,
        "CalendarEventGetCalendarType",
        lua.create_function(|lua, ()| {
            let t = model(lua)
                .calendar
                .current
                .as_ref()
                .map(|c| calendar_type(c.flags));
            opt_s(lua, t)
        })?,
    )?;
    // CalendarEventGetTypes() (`0x5b8970`) and CalendarEventGetRepeatOptions() (`0x5b89c0`).
    set(
        lua,
        "CalendarEventGetTypes",
        lua.create_function(|lua, ()| {
            EVENT_TYPES
                .iter()
                .map(|t| s(lua, &global_text(lua, t)))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    set(
        lua,
        "CalendarEventGetRepeatOptions",
        lua.create_function(|lua, ()| {
            REPEAT_OPTIONS
                .iter()
                .map(|t| s(lua, &global_text(lua, t)))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    // CalendarEventGetTextures(eventType) (`0x5bb970`): name, texture, expansion and a raid's
    // difficulty name per row.
    set(
        lua,
        "CalendarEventGetTextures",
        lua.create_function(|lua, a: MultiValue| {
            let lua_type = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventGetTextures(eventType)"))?;
            let m = model(lua);
            let mut out = Vec::new();
            for &id in m.calendar.textures_for(lua_type) {
                match m.lfg.catalog.dungeon(id) {
                    Some(d) => {
                        let difficulty = if lua_type == 1.0 {
                            difficulty_name(lua, &m, d.map_id as u32, d.difficulty as u32)
                        } else {
                            String::new()
                        };
                        out.extend([
                            s(lua, &d.name)?,
                            s(lua, &d.texture)?,
                            Value::Integer(i64::from(d.expansion_level)),
                            s(lua, &difficulty)?,
                        ]);
                    }
                    None => out.extend([s(lua, "")?, s(lua, "")?, Value::Integer(-1), s(lua, "")?]),
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    // CalendarEventSortInvites(criterion, reverse) (`0x5b8570`): the second argument toggles.
    set(
        lua,
        "CalendarEventSortInvites",
        lua.create_function(|lua, a: MultiValue| {
            let name = a
                .front()
                .and_then(|v| crate::script::binding_abi::optional_string(lua, v))
                .ok_or_else(|| usage("Usage: CalendarEventSortInvites(\"criteria\", reverse)"))?;
            let criterion = SORT_CRITERIA
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&name))
                .unwrap_or(3) as u32;
            let toggle = crate::script::binding_abi::bool_or_default(a.get(1), true);
            {
                let mut m = crate::script::model_mut_of(lua);
                let cal = &mut m.calendar;
                let (names, raid) = (cal.names.clone(), cal.raid_members.clone());
                let Some(cur) = cal.current.as_mut() else {
                    return Ok(());
                };
                cur.sort(criterion, toggle, &names, &raid);
            }
            crate::script::tick::fire_event_into(lua, "CALENDAR_UPDATE_INVITE_LIST", vec![]);
            Ok(())
        })?,
    )?;
    // CalendarEventGetInviteSortCriterion() (`0x5b8690`).
    set(
        lua,
        "CalendarEventGetInviteSortCriterion",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            let (name, reverse) = m.calendar.current.as_ref().map_or(("", false), |c| {
                let t = c.sort_type as usize;
                (
                    SORT_CRITERIA.get(t).copied().unwrap_or(""),
                    c.reverse.get(t).copied().unwrap_or(false),
                )
            });
            Ok((s(lua, name)?, b(reverse)))
        })?,
    )?;
    // CalendarEventGetStatusOptions(index) (`0x5bb130`): `(status, text)` pairs a moderator may
    // set.
    set(
        lua,
        "CalendarEventGetStatusOptions",
        lua.create_function(|lua, a: MultiValue| {
            let index = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventGetStatusOptions(index)"))?;
            let (allowed, options) = {
                let m = model(lua);
                let me = m.calendar.player.as_ref().map_or(0, |p| p.guid);
                match m.calendar.current.as_ref() {
                    None => return Ok(MultiValue::new()),
                    Some(c) if !c.is_moderator(me) => (false, Vec::new()),
                    Some(c) => (
                        true,
                        (0..9)
                            .filter(|&st| c.can_set_status(index0(index), st, me))
                            .collect::<Vec<u32>>(),
                    ),
                }
            };
            if !allowed {
                run(lua, |c, _| c.error("CALENDAR_ERROR_PERMISSIONS", None));
                return Ok(MultiValue::new());
            }
            let mut out = Vec::new();
            for st in options {
                out.push(Value::Integer(i64::from(st) + 1));
                out.push(s(lua, &global_text(lua, STATUS_STRINGS[st as usize]))?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    // CalendarEventSetStatus(index, status) (`0x5b8730`), SetModerator (`0x5b87d0`),
    // ClearModerator (`0x5b8840`) and CanModerate (`0x5b88e0`).
    set(
        lua,
        "CalendarEventSetStatus",
        lua.create_function(|lua, a: MultiValue| {
            let (Some(i), Some(st)) = (num(lua, a.front()), num(lua, a.get(1))) else {
                return Err(usage("Usage: CalendarEventSetStatus(index, status)"));
            };
            run(lua, |c, ctx| {
                c.set_invite_field(ctx, index0(i), index0(st), false)
            });
            Ok(())
        })?,
    )?;
    for (name, status, text) in [
        (
            "CalendarEventSetModerator",
            1,
            "Usage: CalendarEventSetModerator(index)",
        ),
        (
            "CalendarEventClearModerator",
            0,
            "Usage: CalendarEventClearModerator(index)",
        ),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, a: MultiValue| {
                let i = num(lua, a.front()).ok_or_else(|| usage(text))?;
                run(lua, |c, ctx| {
                    c.set_invite_field(ctx, index0(i), status, true)
                });
                Ok(())
            })?,
        )?;
    }
    set(
        lua,
        "CalendarEventCanModerate",
        lua.create_function(|lua, a: MultiValue| {
            let i = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventCanModerate(index)"))?;
            let can = run(lua, |c, _| {
                let r = c.current.as_ref().map(|cur| cur.can_moderate(index0(i)));
                match r {
                    Some(Ok(v)) => v,
                    Some(Err(key)) => {
                        c.error(key, None);
                        false
                    }
                    None => false,
                }
            });
            Ok(b(can))
        })?,
    )?;
    Ok(())
}

/// The day event a context verb names: three numbers name a day's event (absent answers the
/// verb's empty form), none the context menu's (absent raises).
enum Target {
    Event(usize),
    Missing,
}

fn target(lua: &Lua, a: &MultiValue, text: &'static str) -> mlua::Result<Target> {
    let m = model(lua);
    match day_index_opt(lua, a) {
        Some((o, d, i)) => Ok(m
            .calendar
            .day_event_id(o, d, i)
            .map_or(Target::Missing, Target::Event)),
        None => m
            .calendar
            .context_or(None)
            .map(Target::Event)
            .ok_or_else(|| usage(text)),
    }
}

fn install_context(lua: &Lua) -> mlua::Result<()> {
    // CalendarContextSelectEvent(offset, monthDay, index) (`0x5ba5b0`) and
    // CalendarContextDeselectEvent() (`0x5b8510`).
    set(
        lua,
        "CalendarContextSelectEvent",
        lua.create_function(|lua, a: MultiValue| {
            let (o, d, i) = day_index(
                lua,
                &a,
                "Usage: CalendarContextSelectEvent([-1.0.1], monthDay, index)",
            )?;
            crate::script::model_mut_of(lua)
                .calendar
                .select_context(o, d, i);
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarContextDeselectEvent",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).calendar.deselect_context();
            Ok(())
        })?,
    )?;
    // CalendarContextEventClipboard() (`0x5b8540`).
    set(
        lua,
        "CalendarContextEventClipboard",
        lua.create_function(|lua, ()| Ok(b(model(lua).calendar.clipboard_full())))?,
    )?;
    // CalendarContextInviteIsPending (`0x5ba700`) and ModeratorStatus (`0x5ba7f0`): with no
    // numbers they only raise without a context event, and otherwise answer false and nil.
    set(
        lua,
        "CalendarContextInviteIsPending",
        lua.create_function(|lua, a: MultiValue| {
            let m = model(lua);
            Ok(b(match day_index_opt(lua, &a) {
                Some((o, d, i)) => m
                    .calendar
                    .day_event(o, d, i)
                    .is_some_and(|e| e.flags & mark::PENDING != 0),
                None if m.calendar.context_event().is_none() => {
                    return Err(usage(
                        "Usage: CalendarContextInviteIsPending([-1.0.1], monthDay, index)",
                    ))
                }
                None => false,
            }))
        })?,
    )?;
    set(
        lua,
        "CalendarContextInviteModeratorStatus",
        lua.create_function(|lua, a: MultiValue| {
            let m = model(lua);
            match day_index_opt(lua, &a) {
                Some((o, d, i)) => {
                    let role = m.calendar.day_event(o, d, i).map(role_label);
                    opt_s(lua, role.filter(|r| !r.is_empty()))
                }
                None if m.calendar.context_event().is_none() => Err(usage(
                    "Usage: CalendarContextInviteModeratorStatus([-1.0.1], monthDay, index)",
                )),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // CalendarContextInviteStatus (`0x5ba910`) and InviteType (`0x5baa20`).
    set(
        lua,
        "CalendarContextInviteStatus",
        lua.create_function(|lua, a: MultiValue| {
            match target(
                lua,
                &a,
                "Usage: CalendarContextInviteStatus([-1.0.1], monthDay, index)",
            )? {
                Target::Missing => Ok(0),
                Target::Event(id) => Ok(i64::from(model(lua).calendar.get(id).invite_status) + 1),
            }
        })?,
    )?;
    set(
        lua,
        "CalendarContextInviteType",
        lua.create_function(|lua, a: MultiValue| {
            match target(
                lua,
                &a,
                "Usage: CalendarContextInviteType([-1.0.1], monthDay, index)",
            )? {
                Target::Missing => Ok(0),
                Target::Event(id) => Ok(if is_sign_up(model(lua).calendar.get(id)) {
                    2
                } else {
                    1
                }),
            }
        })?,
    )?;
    // CalendarContextInviteAvailable (`0x5bab50`) and Decline (`0x5bad00`): an RSVP straight
    // out, never on a guild sign-up.
    for (name, status, text) in [
        (
            "CalendarContextInviteAvailable",
            1,
            "Usage: CalendarContextInviteAvailable([-1.0.1], monthDay, index)",
        ),
        (
            "CalendarContextInviteDecline",
            2,
            "Usage: CalendarContextInviteDecline([-1.0.1], monthDay, index)",
        ),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, a: MultiValue| {
                if let Target::Event(id) = target(lua, &a, text)? {
                    run(lua, |c, _| c.direct_rsvp(id, status));
                }
                Ok(())
            })?,
        )?;
    }
    // CalendarContextInviteTentative (`0x5bcf30`): a guild sign-up signs up tentatively.
    set(
        lua,
        "CalendarContextInviteTentative",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: Script_CalendarContextInviteTentative([-1.0.1], monthDay, index)",
            )?;
            if let Target::Event(id) = t {
                run(lua, |c, _| {
                    if is_sign_up(c.get(id)) {
                        c.context_sign_up(id, true);
                    } else {
                        c.context_rsvp(id, 8);
                    }
                });
            }
            Ok(())
        })?,
    )?;
    // CalendarContextInviteRemove (`0x5bd040`) and EventSignUp (`0x5bd130`).
    set(
        lua,
        "CalendarContextInviteRemove",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextInviteRemove([-1.0.1], monthDay, index)",
            )?;
            if let Target::Event(id) = t {
                run(lua, |c, _| {
                    if c.get(id).kind & (kind::GUILD_EVENT | kind::PLAYER) != 0 {
                        c.context_remove_invite(id);
                    }
                });
            }
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarContextEventSignUp",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextEventSignUp([-1.0.1], monthDay, index)",
            )?;
            if let Target::Event(id) = t {
                run(lua, |c, _| {
                    if c.get(id).kind & kind::GUILD_EVENT != 0 {
                        c.context_sign_up(id, false);
                    }
                });
            }
            Ok(())
        })?,
    )?;
    // CalendarContextEventRemove (`0x5bd220`) and Copy (`0x5bd340`): the editing right first.
    for (name, copy, text) in [
        (
            "CalendarContextEventRemove",
            false,
            "Usage: Script_CalendarContextEventRemove([-1.0.1], monthDay, index)",
        ),
        (
            "CalendarContextEventCopy",
            true,
            "Usage: Script_CalendarContextEventCopy([-1.0.1], monthDay, index)",
        ),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, a: MultiValue| {
                if let Target::Event(id) = target(lua, &a, text)? {
                    run(lua, |c, ctx| {
                        let e = c.get(id).clone();
                        if !c.can_edit_day_event(ctx, &e) {
                            c.error("CALENDAR_ERROR_PERMISSIONS", None);
                        } else if copy {
                            c.context_copy(ctx, id);
                        } else {
                            c.context_remove_event(id);
                        }
                    });
                }
                Ok(())
            })?,
        )?;
    }
    // CalendarContextEventPaste(offset, monthDay) (`0x5bd460`).
    set(
        lua,
        "CalendarContextEventPaste",
        lua.create_function(|lua, a: MultiValue| {
            let (Some(o), Some(d)) = (num(lua, a.front()), num(lua, a.get(1))) else {
                return Err(usage(
                    "Usage: Script_CalendarContextEventPaste([-1.0.1], monthDay)",
                ));
            };
            run(lua, |c, ctx| c.context_paste(ctx, int(o), index0(d)));
            Ok(())
        })?,
    )?;
    // CalendarContextEventCanComplain (`0x5bd4f0`) and Complain (`0x5bd600`).
    set(
        lua,
        "CalendarContextEventCanComplain",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextEventCanComplain([-1.0.1], monthDay, index)",
            )?;
            let m = model(lua);
            Ok(b(match t {
                Target::Event(id) => {
                    !m.calendar.action_pending && m.calendar.can_complain(m.calendar.get(id))
                }
                Target::Missing => false,
            }))
        })?,
    )?;
    set(
        lua,
        "CalendarContextEventComplain",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextEventComplain([-1.0.1], monthDay, index)",
            )?;
            if let Target::Event(id) = t {
                run(lua, |c, _| c.complain(id));
            }
            Ok(())
        })?,
    )?;
    // CalendarContextEventCanEdit (`0x5baeb0`) and GetCalendarType (`0x5bafc0`).
    set(
        lua,
        "CalendarContextEventCanEdit",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextEventCanEdit([-1.0.1], monthDay, index)",
            )?;
            Ok(b(match t {
                Target::Missing => false,
                Target::Event(id) => {
                    with_calendar(&mut crate::script::model_mut_of(lua), |c, ctx| {
                        c.can_edit_day_event(ctx, c.get(id))
                    })
                }
            }))
        })?,
    )?;
    set(
        lua,
        "CalendarContextEventGetCalendarType",
        lua.create_function(|lua, a: MultiValue| {
            let t = target(
                lua,
                &a,
                "Usage: CalendarContextEventGetCalendarType([-1.0.1], monthDay, index)",
            )?;
            match t {
                Target::Missing => Ok(Value::Nil),
                Target::Event(id) => s(lua, calendar_type(model(lua).calendar.get(id).kind)),
            }
        })?,
    )?;
    Ok(())
}

fn install_editor(lua: &Lua) -> mlua::Result<()> {
    // CalendarNewEvent (`0x5bcca0`), NewGuildAnnouncement (`0x5bcd80`), NewGuildEvent
    // (`0x5bcd90`).
    for (name, calendar_type) in [
        ("CalendarNewEvent", 0),
        ("CalendarNewGuildAnnouncement", 1),
        ("CalendarNewGuildEvent", 2),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, ()| {
                run(lua, |c, _| c.new_event(calendar_type));
                Ok(())
            })?,
        )?;
    }
    // CalendarAddEvent (`0x5bcc70`), UpdateEvent (`0x5bced0`) and RemoveEvent (`0x5ba5a0`).
    set(
        lua,
        "CalendarAddEvent",
        lua.create_function(|lua, ()| {
            run(lua, |c, ctx| c.add_event(ctx));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarUpdateEvent",
        lua.create_function(|lua, ()| {
            run(lua, |c, ctx| c.update_event(ctx));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarRemoveEvent",
        lua.create_function(|lua, ()| {
            run(lua, |c, ctx| c.remove_event(ctx));
            Ok(())
        })?,
    )?;
    // CalendarMassInviteGuild(minLevel, maxLevel, minRank) (`0x5bccb0`): raises only when all
    // three are not numbers.
    set(
        lua,
        "CalendarMassInviteGuild",
        lua.create_function(|lua, a: MultiValue| {
            let (lo, hi, rank) = (num(lua, a.front()), num(lua, a.get(1)), num(lua, a.get(2)));
            if lo.is_none() && hi.is_none() && rank.is_none() {
                return Err(usage(
                    "Usage: CalendarMassInviteGuild(minLevel, maxLevel, minRank)",
                ));
            }
            let (lo, hi, rank) = (
                int(lo.unwrap_or(0.0)) as u32,
                int(hi.unwrap_or(0.0)) as u32,
                index0(rank.unwrap_or(0.0)),
            );
            run(lua, |c, _| c.guild_filter(lo, hi, rank));
            Ok(())
        })?,
    )?;
    // CalendarMassInviteArenaTeam(index) (`0x5bce60`).
    set(
        lua,
        "CalendarMassInviteArenaTeam",
        lua.create_function(|lua, a: MultiValue| {
            let i = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarNewArenaTeamEvent(index)"))?;
            run(lua, |c, _| c.arena_team(index0(i)));
            Ok(())
        })?,
    )?;
    // CalendarDefaultGuildFilter() (`0x5bcda0`): the expansion's level cap twice and the rank
    // count.
    set(
        lua,
        "CalendarDefaultGuildFilter",
        lua.create_function(|lua, ()| {
            let m = model(lua);
            if m.calendar.player.is_none() {
                return Ok((n(0.0), n(0.0), n(0.0)));
            }
            let cap = EXPANSION_MAX_LEVELS
                .get(usize::from(m.account_expansion))
                .copied()
                .unwrap_or(80.0);
            Ok((n(cap), n(cap), n(m.guild.ranks.len() as u32)))
        })?,
    )?;
    // CalendarEventInvite(name) (`0x5bcf00`), RemoveInvite(index) (`0x5bb0c0`).
    set(
        lua,
        "CalendarEventInvite",
        lua.create_function(|lua, a: MultiValue| {
            let name = a
                .front()
                .and_then(|v| crate::script::binding_abi::optional_string(lua, v));
            if let Some(name) = name {
                run(lua, |c, ctx| c.invite_name(ctx, &name));
            }
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventRemoveInvite",
        lua.create_function(|lua, a: MultiValue| {
            let i = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventRemoveInvite(index)"))?;
            run(lua, |c, _| c.remove_invite(index0(i)));
            Ok(())
        })?,
    )?;
    // CalendarEventAvailable (`0x5bd6e0`), Tentative (`0x5bd750`), Decline (`0x5bd760`) and
    // SignUp (`0x5bd7d0`).
    set(
        lua,
        "CalendarEventAvailable",
        lua.create_function(|lua, ()| {
            run(lua, |c, _| c.answer_own_invite(1));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventTentative",
        lua.create_function(|lua, ()| {
            run(lua, |c, _| c.invite_rsvp(8));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventDecline",
        lua.create_function(|lua, ()| {
            run(lua, |c, _| c.answer_own_invite(2));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventSignUp",
        lua.create_function(|lua, ()| {
            run(lua, |c, _| c.event_sign_up());
            Ok(())
        })?,
    )?;
    // The setters (`0x5bb210`-`0x5bb930`): each needs the editing right.
    set(
        lua,
        "CalendarEventSetTitle",
        lua.create_function(|lua, a: MultiValue| {
            let title = a
                .front()
                .and_then(|v| crate::script::binding_abi::optional_string(lua, v));
            edit(lua, |cur| cur.set_title(title.as_deref()));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventSetDescription",
        lua.create_function(|lua, a: MultiValue| {
            let d = a
                .front()
                .and_then(|v| crate::script::binding_abi::optional_string(lua, v));
            edit(lua, |cur| cur.set_description(d.as_deref()));
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventSetType",
        lua.create_function(|lua, a: MultiValue| {
            if let Some(t) = num(lua, a.front()) {
                edit(lua, |cur| cur.set_type((t - 1.0) as i64 as u32));
            }
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventSetRepeatOption",
        lua.create_function(|lua, a: MultiValue| {
            if let Some(r) = num(lua, a.front()) {
                edit(lua, |cur| cur.set_repeat((r - 1.0) as i64 as u32));
            }
            Ok(())
        })?,
    )?;
    set(
        lua,
        "CalendarEventSetSize",
        lua.create_function(|lua, a: MultiValue| {
            if let Some(size) = num(lua, a.front()) {
                edit(lua, |cur| cur.set_size(size as i64 as u32));
            }
            Ok(())
        })?,
    )?;
    // CalendarEventSetDate (`0x5bb4a0`) and SetLockoutDate (`0x5bb650`): a refusal, rights
    // included, is an invalid date.
    for (name, lockout, text) in [
        (
            "CalendarEventSetDate",
            false,
            "Usage: CalendarEventSetDate(month, day, year)",
        ),
        (
            "CalendarEventSetLockoutDate",
            true,
            "Usage: CalendarEventSetLockoutDate(month, day, year)",
        ),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, a: MultiValue| {
                let (m, d, y) = (num(lua, a.front()), num(lua, a.get(1)), num(lua, a.get(2)))
                    .zip3()
                    .ok_or_else(|| usage(text))?;
                let (m, d, y) = (index0(m), index0(d), (int(y) as u32).wrapping_sub(2000));
                let ok = can_edit(lua)
                    && crate::script::model_mut_of(lua)
                        .calendar
                        .current
                        .as_mut()
                        .is_some_and(|cur| {
                            if lockout {
                                cur.set_lockout_date(m, d, y)
                            } else {
                                cur.set_date(m, d, y)
                            }
                        });
                if !ok {
                    run(lua, |c, _| c.error("CALENDAR_ERROR_INVALID_DATE", None));
                }
                Ok(())
            })?,
        )?;
    }
    // CalendarEventSetTime (`0x5bb5a0`) and SetLockoutTime (`0x5bb750`).
    for (name, lockout, text) in [
        (
            "CalendarEventSetTime",
            false,
            "Usage: CalendarEventSetTime(hour, minute)",
        ),
        (
            "CalendarEventSetLockoutTime",
            true,
            "Usage: CalendarEvenSettLockoutTime(hour, minute)",
        ),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, a: MultiValue| {
                let (Some(h), Some(mi)) = (num(lua, a.front()), num(lua, a.get(1))) else {
                    return Err(usage(text));
                };
                let (h, mi) = (int(h) as u32, int(mi) as u32);
                let ok = can_edit(lua)
                    && crate::script::model_mut_of(lua)
                        .calendar
                        .current
                        .as_mut()
                        .is_some_and(|cur| {
                            if lockout {
                                cur.set_lockout_time(h, mi)
                            } else {
                                cur.set_time(h, mi)
                            }
                        });
                if !ok {
                    run(lua, |c, _| c.error("CALENDAR_ERROR_INVALID_TIME", None));
                }
                Ok(())
            })?,
        )?;
    }
    // CalendarEventSetTextureID(textureIndex) (`0x5bb800`).
    set(
        lua,
        "CalendarEventSetTextureID",
        lua.create_function(|lua, a: MultiValue| {
            let i = num(lua, a.front())
                .ok_or_else(|| usage("Usage: CalendarEventSetTextureID(textureIndex)"))?;
            run(lua, |c, ctx| c.set_texture_index(ctx, index0(i)));
            Ok(())
        })?,
    )?;
    // CalendarEventSetLocked, ClearLocked, SetAutoApprove, ClearAutoApprove (`0x5bb870`-
    // `0x5bb930`).
    for (name, flag, on) in [
        ("CalendarEventSetLocked", kind::LOCKED, true),
        ("CalendarEventClearLocked", kind::LOCKED, false),
        ("CalendarEventSetAutoApprove", kind::AUTO_APPROVE, true),
        ("CalendarEventClearAutoApprove", kind::AUTO_APPROVE, false),
    ] {
        set(
            lua,
            name,
            lua.create_function(move |lua, ()| {
                edit(lua, |cur| cur.set_flag(flag, on));
                Ok(())
            })?,
        )?;
    }
    Ok(())
}
