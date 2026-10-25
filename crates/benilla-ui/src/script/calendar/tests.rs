use super::*;
use crate::script::UiScript;

const ME: u64 = 0x10;
const FRIEND: u64 = 0x20;

/// Sunday the 25th of October 2026, 14:30, as the clock packet decodes it.
fn today() -> WowTime {
    WowTime {
        minute: 30,
        hour: 14,
        weekday: 0,
        month_day: 24,
        month: 9,
        year: 26,
        flags: 0,
        tz_hours: 0,
    }
}

fn at(month_day: i32, hour: i32, minute: i32) -> u32 {
    let mut t = WowTime {
        hour,
        minute,
        ..WowTime::date(9, month_day, 26)
    };
    t.add_days(0, true);
    t.encode()
}

/// A VM with the filter CVars, the player, the clock and an event log.
fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.register_cvars([
        ("calendarShowWeeklyHolidays", "1"),
        ("calendarShowDarkmoon", "1"),
        ("calendarShowBattlegrounds", "0"),
        ("calendarShowLockouts", "1"),
        ("calendarShowResets", "0"),
    ]);
    s.set_calendar_player(Some(CalendarPlayerView {
        guid: ME,
        level: 80,
        guild_id: 0,
        arena_teams: [0; 3],
        pvp_faction: 1,
        faction_group: 1,
    }));
    s.set_calendar_clock(today());
    s.run(
        r#"
        LOG = {}
        CALENDAR_ERROR_EVENTS_EXCEEDED = "You have reached your limit of %d events."
        CALENDAR_ERROR_NEEDS_TITLE = "Needs a title."
        local f = CreateFrame("Frame")
        for _, e in ipairs({"CALENDAR_UPDATE_EVENT_LIST", "CALENDAR_UPDATE_PENDING_INVITES",
            "CALENDAR_UPDATE_INVITE_LIST", "CALENDAR_OPEN_EVENT", "CALENDAR_NEW_EVENT",
            "CALENDAR_UPDATE_ERROR", "CALENDAR_ACTION_PENDING", "CALENDAR_EVENT_ALARM"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function(self, event, a)
            LOG[#LOG + 1] = event .. (a ~= nil and ("=" .. tostring(a)) or "")
        end)
        "#,
    )
    .unwrap();
    s
}

fn log(s: &UiScript) -> Vec<String> {
    let out: Vec<String> = s.eval("return LOG").unwrap();
    s.run("LOG = {}").unwrap();
    out
}

fn eval_nums(s: &UiScript, chunk: &str) -> Vec<f64> {
    s.eval(chunk).unwrap()
}

/// With no calendar yet the date verbs read the game clock, the month is this one and nothing
/// is pending; the first `OpenCalendar` fetches, a second does not.
#[test]
fn an_empty_calendar_reads_the_clock() {
    let mut s = script();
    let date = eval_nums(&s, "return {CalendarGetDate()}");
    assert_eq!(date, [1.0, 10.0, 25.0, 2026.0]);
    let month = eval_nums(&s, "return {CalendarGetMonth()}");
    // October 2026 has 31 days and starts on a Thursday.
    assert_eq!(month, [10.0, 2026.0, 31.0, 5.0]);
    let pending: f64 = s.eval("return CalendarGetNumPendingInvites()").unwrap();
    assert_eq!(pending, 0.0);
    s.run("OpenCalendar(); OpenCalendar()").unwrap();
    assert_eq!(s.take_calendar_requests(), [CalendarRequest::GetCalendar]);
}

fn snapshot_with_invite() -> CalendarSnapshotView {
    CalendarSnapshotView {
        invites: vec![CalendarInviteRefView {
            event_id: 7,
            invite_id: 70,
            status: 0,
            rank: 0,
            invite_type: 0,
            inviter: FRIEND,
        }],
        events: vec![CalendarEventRowView {
            event_id: 7,
            title: "Naxx".into(),
            event_type: 0,
            start: at(27, 20, 0),
            flags: 0x1,
            texture: -1,
            creator: FRIEND,
        }],
        server_now: 1_000_000,
        server_date: today().encode(),
        ..Default::default()
    }
}

/// An unanswered invite to a future event lands on its day, first, as a pending invite.
#[test]
fn a_pending_invite_lands_on_its_day() {
    let mut s = script();
    s.run("CalendarSetAbsMonth(10, 2026)").unwrap();
    log(&s);
    s.calendar_packet(CalendarWire::SendCalendar(Box::new(snapshot_with_invite())));
    let fired = log(&s);
    assert!(fired.contains(&"CALENDAR_UPDATE_EVENT_LIST".to_string()));
    assert!(fired.contains(&"CALENDAR_UPDATE_PENDING_INVITES".to_string()));
    let n: f64 = s.eval("return CalendarGetNumDayEvents(0, 28)").unwrap();
    assert_eq!(n, 1.0);
    let first: i64 = s
        .eval("return CalendarGetFirstPendingInvite(0, 28)")
        .unwrap();
    assert_eq!(first, 1);
    let empty: f64 = s.eval("return CalendarGetNumDayEvents(0, 27)").unwrap();
    assert_eq!(empty, 0.0);
    let pending: f64 = s.eval("return CalendarGetNumPendingInvites()").unwrap();
    assert_eq!(pending, 1.0);
    let (title, hour, kind): (String, f64, String) = s
        .eval("local t, h, m, k = CalendarGetDayEvent(0, 28, 1) return t, h, k")
        .unwrap();
    assert_eq!(
        (title.as_str(), hour, kind.as_str()),
        ("Naxx", 20.0, "PLAYER")
    );
    // The inviter's name is asked for and its arrival repaints the list.
    assert!(s.calendar_names_wanted().contains(&FRIEND));
    s.set_calendar_names(HashMap::from([(
        FRIEND,
        CalendarName {
            name: "Juozas".into(),
            ..Default::default()
        },
    )]));
    assert_eq!(log(&s), ["CALENDAR_UPDATE_EVENT_LIST"]);
    // Opening a fetched event asks the server and holds the action pending.
    s.run("CalendarOpenEvent(0, 28, 1)").unwrap();
    assert_eq!(
        s.take_calendar_requests(),
        [CalendarRequest::GetEvent { event_id: 7 }]
    );
    let busy: bool = s.eval("return CalendarIsActionPending()").unwrap();
    assert!(busy);
}

/// A holiday row of the player's side shows on each day of its run, labelled by position.
#[test]
fn a_holiday_runs_across_its_days() {
    let mut s = script();
    let start = WowTime {
        hour: 0,
        minute: 0,
        ..WowTime::date(9, 9, 26)
    };
    let mut durations = [0; 10];
    durations[0] = 72;
    let mut dates = [0; 26];
    dates[0] = start.encode() as i32;
    let mut flags = [0; 10];
    flags[0] = 1;
    s.set_calendar_catalog(CalendarCatalogView {
        holidays: vec![CalendarHolidayView {
            id: 181,
            durations,
            dates,
            calendar_flags: flags,
            name: Some("Brewfest".into()),
            description: Some("Drink".into()),
            texture: "Calendar_Brewfest".into(),
            priority: 1,
            filter_type: -1,
            ..Default::default()
        }],
        map_names: HashMap::new(),
    });
    s.run("CalendarSetAbsMonth(10, 2026)").unwrap();
    let seq: Vec<String> = s
        .eval(
            "local out = {} for d = 10, 12 do local _, _, _, _, l = CalendarGetDayEvent(0, d, 1) \
             out[#out + 1] = l end return out",
        )
        .unwrap();
    assert_eq!(seq, ["START", "ONGOING", "END"]);
    let (name, description, texture): (String, String, String) =
        s.eval("return CalendarGetHolidayInfo(0, 11, 1)").unwrap();
    assert_eq!(
        (name.as_str(), description.as_str(), texture.as_str()),
        ("Brewfest", "Drink", "Calendar_Brewfest")
    );
    let none: f64 = s.eval("return CalendarGetNumDayEvents(0, 14)").unwrap();
    assert_eq!(none, 0.0);
}

/// The editor's new event: the player is its confirmed creator, the setters stage the fields,
/// and the add sends them with the invite list.
#[test]
fn a_new_event_is_staged_and_added() {
    let mut s = script();
    s.run("CalendarNewEvent()").unwrap();
    assert_eq!(log(&s), ["CALENDAR_UPDATE_INVITE_LIST=true"]);
    s.run(
        "CalendarEventSetTitle('Raid\\n night') CalendarEventSetDate(10, 30, 2026) \
         CalendarEventSetTime(19, 45) CalendarEventSetType(2) CalendarAddEvent()",
    )
    .unwrap();
    let requests = s.take_calendar_requests();
    let [CalendarRequest::AddEvent {
        title,
        event_type,
        date,
        invites: Some(invites),
        flags,
        ..
    }] = requests.as_slice()
    else {
        panic!("one add: {requests:?}");
    };
    assert_eq!(title, "Raid");
    assert_eq!(*event_type, 1);
    assert_eq!(*flags, 0x1);
    let d = WowTime::decode(*date);
    assert_eq!((d.month, d.month_day, d.hour, d.minute), (9, 29, 19, 45));
    assert_eq!(
        invites,
        &[CalendarNewInviteView {
            guid: ME,
            status: 3,
            moderator: 2
        }]
    );
    assert!(log(&s).contains(&"CALENDAR_ACTION_PENDING=true".to_string()));
}

/// An untitled add is refused locally with the stock popup's line, and the action is released.
#[test]
fn an_untitled_add_shows_the_error_line() {
    let mut s = script();
    s.run("CalendarNewEvent() CalendarEventSetDate(10, 30, 2026) CalendarEventSetTime(19, 0)")
        .unwrap();
    log(&s);
    s.run("CalendarAddEvent()").unwrap();
    assert!(s.take_calendar_requests().is_empty());
    assert_eq!(
        log(&s),
        [
            "CALENDAR_UPDATE_ERROR=Needs a title.",
            "CALENDAR_ACTION_PENDING=false"
        ]
    );
}

/// A server refusal fills its count into the line (`0x6cd380`).
#[test]
fn a_command_result_fills_its_line() {
    let mut s = script();
    s.calendar_packet(CalendarWire::CommandResult(CalendarCommandResultView {
        result: 2,
        ..Default::default()
    }));
    assert_eq!(
        log(&s),
        [
            "CALENDAR_UPDATE_ERROR=You have reached your limit of 30 events.",
            "CALENDAR_ACTION_PENDING=false"
        ]
    );
}

/// The server's pending count stands until the calendar lands.
#[test]
fn the_server_pending_count_shows_before_the_calendar() {
    let mut s = script();
    s.calendar_packet(CalendarWire::NumPending(2));
    assert!(log(&s).contains(&"CALENDAR_UPDATE_PENDING_INVITES".to_string()));
    let n: f64 = s.eval("return CalendarGetNumPendingInvites()").unwrap();
    assert_eq!(n, 2.0);
}

/// The date line of a mailed notice, in 12 and 24 hours.
#[test]
fn the_date_time_text_follows_the_military_switch() {
    let get = |k: &str| {
        match k {
            "FULLDATE" => "%1$s, %2$s %3$d %4$d",
            "FULLDATE_AND_TIME" => "%1$s, %2$s",
            "TIME_TWELVEHOURPM" => "%d:%02d PM",
            "TIME_TWENTYFOURHOURS" => "%d:%02d",
            "WEEKDAY_SUNDAY" => "Sunday",
            "FULLDATE_MONTH_OCTOBER" => "October",
            _ => "",
        }
        .to_string()
    };
    let t = today().encode();
    assert_eq!(
        date_time_text(t, false, &get),
        "Sunday, October 25 2026, 2:30 PM"
    );
    assert_eq!(
        date_time_text(t, true, &get),
        "Sunday, October 25 2026, 14:30"
    );
}

/// An accepted invite's alarm fires 15 minutes before it starts.
#[test]
fn an_accepted_event_alarms_fifteen_minutes_early() {
    let mut s = script();
    let mut snapshot = snapshot_with_invite();
    snapshot.invites[0].status = 1;
    snapshot.events[0].start = at(24, 15, 0);
    s.calendar_packet(CalendarWire::SendCalendar(Box::new(snapshot)));
    log(&s);
    s.set_calendar_clock(WowTime {
        minute: 45,
        ..today()
    });
    assert_eq!(log(&s), ["CALENDAR_EVENT_ALARM=Naxx"]);
}
