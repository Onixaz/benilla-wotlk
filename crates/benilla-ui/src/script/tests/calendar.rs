//! The calendar's month and day queries ([`crate::script::calendar`]).

use super::common::script;
use crate::script::{CalendarDate, CalendarEntryView, CalendarUiState, UiScript};

fn with_event() -> UiScript {
    let mut s = script();
    // 2026-10-20 12:00 UTC, a Tuesday.
    s.set_server_unix_time(1_792_497_600.0);
    s.set_calendar(CalendarUiState {
        entries: vec![CalendarEntryView {
            calendar_type: "PLAYER",
            title: "Naxx".into(),
            date: CalendarDate {
                year: 2026,
                month: 10,
                day: 22,
                hour: 20,
                minute: 30,
            },
            event_type: 0,
        }],
        pending_invites: 2,
    });
    s
}

#[test]
fn the_date_is_weekday_month_day_year_with_sunday_as_one() {
    let s = with_event();
    let d: (i64, i64, i64, i64) = s.eval("return CalendarGetDate()").unwrap();
    assert_eq!(d, (3, 10, 20, 2026));
}

#[test]
fn a_month_is_month_year_days_and_a_one_based_first_weekday() {
    let s = with_event();
    // October 2026 has 31 days and starts on a Thursday.
    let m: (i64, i64, i64, i64) = s.eval("return CalendarGetMonth()").unwrap();
    assert_eq!(m, (10, 2026, 31, 5));
    // The next month is relative to the viewed one, and December rolls into the next year.
    let n: (i64, i64, i64, i64) = s.eval("return CalendarGetMonth(1)").unwrap();
    assert_eq!(n, (11, 2026, 30, 1));
    let y: (i64, i64, i64, i64) = s
        .eval("CalendarSetAbsMonth(12, 2026) return CalendarGetMonth(1)")
        .unwrap();
    assert_eq!(y, (1, 2027, 31, 6));
}

#[test]
fn set_month_steps_from_the_viewed_month() {
    let s = with_event();
    let m: (i64, i64) = s
        .eval(
            "CalendarSetMonth(-1) CalendarSetMonth(-1) local m, y = CalendarGetMonth() return m, y",
        )
        .unwrap();
    assert_eq!(m, (8, 2026));
}

#[test]
fn the_navigable_range_ends_are_the_references() {
    let s = script();
    let lo: (i64, i64, i64, i64) = s.eval("return CalendarGetMinDate()").unwrap();
    assert_eq!(lo, (4, 11, 24, 2004));
    let hi: (i64, i64, i64, i64) = s.eval("return CalendarGetMaxDate()").unwrap();
    assert_eq!(hi, (3, 12, 31, 2030));
}

#[test]
fn a_day_lists_the_events_that_fall_on_it() {
    let s = with_event();
    let n: i64 = s.eval("return CalendarGetNumDayEvents(0, 22)").unwrap();
    assert_eq!(n, 1);
    let empty: i64 = s.eval("return CalendarGetNumDayEvents(0, 21)").unwrap();
    assert_eq!(empty, 0);
    let off: i64 = s.eval("return CalendarGetNumDayEvents(1, 22)").unwrap();
    assert_eq!(off, 0, "the offset moves off the event's month");

    let (title, hour, minute, kind, seq, event_type): (String, i64, i64, String, String, i64) =
        s.eval("return CalendarGetDayEvent(0, 22, 1)").unwrap();
    assert_eq!((title.as_str(), hour, minute), ("Naxx", 20, 30));
    assert_eq!((kind.as_str(), seq.as_str(), event_type), ("PLAYER", "", 1));
}

#[test]
fn a_day_event_miss_is_the_references_fifteen_value_tuple() {
    let s = with_event();
    let n: i64 = s
        .eval("return select('#', CalendarGetDayEvent(0, 1, 1))")
        .unwrap();
    assert_eq!(n, 15);
    let first_is_nil: bool = s
        .eval("return (CalendarGetDayEvent(0, 1, 1)) == nil")
        .unwrap();
    assert!(first_is_nil);
}

#[test]
fn open_calendar_asks_once_until_the_reply_lands() {
    let mut s = script();
    s.eval::<()>("OpenCalendar() OpenCalendar()").unwrap();
    assert_eq!(s.take_calendar_get_calendar_requests(), 1);
    s.eval::<()>("OpenCalendar()").unwrap();
    assert_eq!(s.take_calendar_get_calendar_requests(), 0, "still waiting");
    s.set_calendar(CalendarUiState::default());
    s.eval::<()>("OpenCalendar()").unwrap();
    assert_eq!(s.take_calendar_get_calendar_requests(), 1);
}

#[test]
fn the_pending_invite_count_is_the_snapshots() {
    let s = with_event();
    let n: i64 = s.eval("return CalendarGetNumPendingInvites()").unwrap();
    assert_eq!(n, 2);
}
