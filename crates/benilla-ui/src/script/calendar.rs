//! The calendar's month and day queries, read off the one `SMSG_CALENDAR_SEND_CALENDAR` snapshot
//! the app pushes after `OpenCalendar()` asks for it. The viewed month is engine state: every
//! `monthOffset` the stock Blizzard_Calendar passes is relative to it.
//!
//! Not built here: invites (the packet gives an invite no date, so `CalendarGetFirstPendingInvite`
//! has nothing to place on a day until the per-event fetch exists), holidays, raid lockouts, event
//! selection and everything that creates or edits an event.

use mlua::{Lua, MultiValue, Value};

use crate::civil;

/// A broken-down event time as the packet carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CalendarDate {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// One event the player holds, from the packet's event section.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEntryView {
    /// `"PLAYER"` or `"GUILD_EVENT"`, the labels `CalendarEventTypeName` (`0x005b7fd0`) returns.
    pub calendar_type: &'static str,
    pub title: String,
    pub date: CalendarDate,
    /// The packet's `CalendarEventType`, 0 raid to 5 heroic; the binding returns it plus one.
    pub event_type: u32,
}

/// The snapshot behind the queries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalendarUiState {
    pub entries: Vec<CalendarEntryView>,
    /// Invites still unanswered (`CALENDAR_STATUS_INVITED`).
    pub pending_invites: u32,
}

impl super::UiScript {
    /// Replace the snapshot, clear the open request and queue `CALENDAR_UPDATE_EVENT_LIST` (575).
    pub fn set_calendar(&mut self, state: CalendarUiState) {
        let mut model = self.model_mut();
        model.calendar_request_pending = false;
        model.calendar = state;
        model
            .pending_events
            .push(("CALENDAR_UPDATE_EVENT_LIST".to_string(), Vec::new()));
    }

    /// Drain the `OpenCalendar()` requests, each a `CMSG_CALENDAR_GET_CALENDAR`.
    pub fn take_calendar_get_calendar_requests(&mut self) -> u32 {
        drain_field!(self, calendar_get_requests)
    }
}

/// Today as the server clock reads it, else the wall clock before the first sync.
fn now(model: &super::Model) -> civil::Civil {
    let secs = model
        .server_unix_time
        .get()
        .map_or_else(civil::unix_seconds, |s| s as i64);
    civil::from_unix(secs)
}

/// The viewed month, the current one until `CalendarSetMonth`/`CalendarSetAbsMonth` picks another.
fn viewed(model: &super::Model) -> (i32, u32) {
    model.calendar_viewed.unwrap_or_else(|| {
        let n = now(model);
        (n.year as i32, n.month)
    })
}

/// `(year, month)` shifted by `offset` months, carrying into the year.
fn offset_month((year, month): (i32, u32), offset: i64) -> (i32, u32) {
    let zero_based = i64::from(month) - 1 + offset;
    (
        (i64::from(year) + zero_based.div_euclid(12)) as i32,
        zero_based.rem_euclid(12) as u32 + 1,
    )
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (civil::days_from_civil(i64::from(ny), nm, 1)
        - civil::days_from_civil(i64::from(year), month, 1)) as u32
}

/// Weekday, 0 = Sunday.
fn weekday(year: i32, month: u32, day: u32) -> u32 {
    (civil::days_from_civil(i64::from(year), month, day) + 4).rem_euclid(7) as u32
}

/// The four numbers every date getter pushes: weekday (Sunday is 1), month, day, year.
fn date_tuple(year: i32, month: u32, day: u32) -> (i64, i64, i64, i64) {
    (
        i64::from(weekday(year, month, day)) + 1,
        i64::from(month),
        i64::from(day),
        i64::from(year),
    )
}

fn entries_on(model: &super::Model, year: i32, month: u32, day: u32) -> Vec<&CalendarEntryView> {
    model
        .calendar
        .entries
        .iter()
        .filter(|e| e.date.year == year && e.date.month == month && e.date.day == day)
        .collect()
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x005bd7e0`: sends the request only while the open-request byte is clear, then sets it;
    // the reply clears it ([`UiScript::set_calendar`]).
    g.set(
        "OpenCalendar",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if !model.calendar_request_pending {
                model.calendar_request_pending = true;
                model.calendar_get_requests += 1;
            }
            Ok(())
        })?,
    )?;

    // `0x005b80e0`: weekday, month, day, year, with the first three stored zero-based.
    g.set(
        "CalendarGetDate",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let n = now(&model);
            Ok((
                i64::from(n.weekday) + 1,
                i64::from(n.month),
                i64::from(n.day),
                n.year,
            ))
        })?,
    )?;

    // `0x005b81f0`, `0x005b82a0`: the navigable range, 2004-11-24 to 2030-12-31 (month 10, day 23,
    // year 4 set by the first; month 0, day 0, year 31 less one day by the second).
    g.set(
        "CalendarGetMinDate",
        lua.create_function(|_, ()| Ok(date_tuple(2004, 11, 24)))?,
    )?;
    g.set(
        "CalendarGetMaxDate",
        lua.create_function(|_, ()| Ok(date_tuple(2030, 12, 31)))?,
    )?;

    // `0x005b8350`: today less fourteen days.
    g.set(
        "CalendarGetMinHistoryDate",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let n = now(&model);
            let back = civil::from_unix(
                civil::days_from_civil(n.year, n.month, n.day) * 86_400 - 14 * 86_400,
            );
            Ok(date_tuple(back.year as i32, back.month, back.day))
        })?,
    )?;

    // A relative step; a non-number raises.
    g.set(
        "CalendarSetMonth",
        lua.create_function(|lua, offset: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let to = offset_month(viewed(&model), offset);
            model.calendar_viewed = Some(to);
            Ok(())
        })?,
    )?;

    // Month required, year optional (the viewed year when omitted).
    g.set(
        "CalendarSetAbsMonth",
        lua.create_function(|lua, (month, year): (i64, Option<i64>)| {
            let mut model = crate::script::model_mut_of(lua);
            let year = year.map_or_else(|| viewed(&model).0, |y| y as i32);
            model.calendar_viewed = Some((year, month.clamp(1, 12) as u32));
            Ok(())
        })?,
    )?;

    // month, year, days in month, first weekday (Sunday is 1) of the viewed month plus `offset`.
    g.set(
        "CalendarGetMonth",
        lua.create_function(|lua, offset: Option<i64>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (year, month) = offset_month(viewed(&model), offset.unwrap_or(0));
            Ok((
                i64::from(month),
                i64::from(year),
                i64::from(days_in_month(year, month)),
                i64::from(weekday(year, month, 1)) + 1,
            ))
        })?,
    )?;

    // `0x005b9bf0`.
    g.set(
        "CalendarGetNumDayEvents",
        lua.create_function(|lua, (offset, day): (i64, f64)| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (year, month) = offset_month(viewed(&model), offset);
            Ok(entries_on(&model, year, month, day.round() as u32).len() as i64)
        })?,
    )?;

    // `0x005c1070`: fifteen values; a miss is nil, 0, 0, "", "", 0, "", "", 0, "", 0, 0, 0, 0, "".
    g.set(
        "CalendarGetDayEvent",
        lua.create_function(|lua, (offset, day, index): (i64, f64, f64)| {
            let row = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let (year, month) = offset_month(viewed(&model), offset);
                let entries = entries_on(&model, year, month, day.round() as u32);
                (index.round() as i64)
                    .checked_sub(1)
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| entries.get(i).map(|e| (*e).clone()))
            };
            let s = |v: &str| -> mlua::Result<Value> { Ok(Value::String(lua.create_string(v)?)) };
            let Some(e) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    s("")?,
                    s("")?,
                    Value::Integer(0),
                    s("")?,
                    s("")?,
                    Value::Integer(0),
                    s("")?,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    s("")?,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                s(&e.title)?,
                Value::Integer(i64::from(e.date.hour)),
                Value::Integer(i64::from(e.date.minute)),
                s(e.calendar_type)?,
                s("")?,
                Value::Integer(i64::from(e.event_type) + 1),
                s("")?,
                s("")?,
                Value::Integer(0),
                s("")?,
                Value::Integer(0),
                Value::Integer(0),
                // A lone event is a one-day sequence.
                Value::Integer(1),
                Value::Integer(1),
                s("")?,
            ]))
        })?,
    )?;

    // `0x005bf830`.
    g.set(
        "CalendarGetNumPendingInvites",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.calendar.pending_invites))
        })?,
    )?;

    // The action-pending byte `0x00c207f3`, distinct from the open-request byte `OpenCalendar`
    // sets. Only the event actions set it, and none is built here, so it is never up.
    g.set(
        "CalendarIsActionPending",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let _ = &model;
            Ok(false)
        })?,
    )?;

    Ok(())
}
