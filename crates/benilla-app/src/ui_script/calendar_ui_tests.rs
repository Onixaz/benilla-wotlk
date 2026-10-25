//! The stock 3.3.5a calendar (`Blizzard_Calendar`, the minimap clock's `GameTime.lua`) on the
//! production load, fed the install's holiday tables and a calendar shaped like the server's:
//! the month view, a day's events and pending invite, the event viewer and the editor's create.

use std::collections::HashMap;

use benilla_ui::script::{
    CalendarEventRowView, CalendarEventView, CalendarInviteRefView, CalendarInviteeView,
    CalendarName, CalendarPlayerView, CalendarRequest, CalendarSnapshotView, CalendarWire,
    UiScript, WowTime,
};

const ME: u64 = 0x10;
const FRIEND: u64 = 0x20;

/// Errors from the calendar's own code; the production load carries unrelated 3.3.5a gaps.
fn calendar_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("Calendar") || e.contains("GameTime"))
        .collect()
}

/// A stand-in font engine: each byte 7 wide, each line 14 tall, and an edit box's advances
/// answered inline as the live engine does, so the editor's caret event precedes its text event.
struct Glyphs;

impl benilla_ui::script::TextMeasure for Glyphs {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.split('\n').map(str::len).max().unwrap_or(0) as f32 * 7.0;
        (natural, req.text.split('\n').count() as f32 * 14.0, natural)
    }

    fn editbox_advances(
        &mut self,
        req: &benilla_ui::script::EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let cum = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        if req.wrap_width.is_none() {
            return Some((cum, vec![0], 0.0));
        }
        let breaks = req.text.match_indices('\n').map(|(i, _)| i + 1);
        Some((cum, std::iter::once(0).chain(breaks).collect(), 14.0))
    }
}

/// One app frame: the layout resolve, then the tick.
fn frame(s: &mut UiScript) {
    s.resolve();
    s.tick(0.0);
}

/// Sunday the 25th of October 2026, 14:30.
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

fn october(month_day: i32, hour: i32) -> u32 {
    let mut t = WowTime {
        hour,
        minute: 0,
        ..WowTime::date(9, month_day, 26)
    };
    t.add_days(0, true);
    t.encode()
}

/// The production VM with the holidays pushed, the player in world and the clock set.
fn calendar() -> Option<UiScript> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = crate::ui_calendar::read_catalog(&mut chain).expect("the holiday tables");
    let (mut s, _) =
        super::layer_tests::production_load_seeded("calendar", false, "", |_| {}, |_| {});
    let _ = s.take_errors();
    s.set_text_measurer(Box::new(Glyphs));
    s.set_calendar_catalog(catalog);
    s.set_calendar_player(Some(CalendarPlayerView {
        guid: ME,
        level: 80,
        guild_id: 0,
        arena_teams: [0; 3],
        pvp_faction: 1,
        faction_group: 1,
    }));
    s.set_calendar_clock(today());
    s.set_calendar_names(HashMap::from([
        (
            ME,
            CalendarName {
                name: "Claude".into(),
                class_name: "Hunter".into(),
                class_file: "HUNTER".into(),
            },
        ),
        (
            FRIEND,
            CalendarName {
                name: "Juozas".into(),
                class_name: "Mage".into(),
                class_file: "MAGE".into(),
            },
        ),
    ]));
    s.calendar_enter_world();
    Some(s)
}

/// The server's calendar: one invite from a friend to a raid on the 28th.
fn snapshot() -> CalendarSnapshotView {
    CalendarSnapshotView {
        invites: vec![CalendarInviteRefView {
            event_id: 7,
            invite_id: 70,
            inviter: FRIEND,
            ..Default::default()
        }],
        events: vec![CalendarEventRowView {
            event_id: 7,
            title: "Naxxramas".into(),
            event_type: 0,
            start: october(27, 20),
            flags: 0x1,
            texture: -1,
            creator: FRIEND,
        }],
        server_now: 1_000_000,
        server_date: today().encode(),
        ..Default::default()
    }
}

/// The badge on the minimap clock and the month view: the clock button reads today's date and
/// the pending count with no calendar loaded; the window opens on this month, fetches once, shows
/// the invite on its day and the holidays the install schedules, and opens the invite.
#[test]
fn the_stock_calendar_shows_the_month_and_opens_an_invite() {
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut s) = calendar() else {
        return;
    };
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "clock button errors: {errors:#?}");
    assert_eq!(
        s.eval::<String>("return GameTimeFrame:GetText()").unwrap(),
        "25"
    );
    assert!(s
        .take_calendar_requests()
        .contains(&CalendarRequest::GetNumPending));

    s.run("ToggleCalendar()").unwrap();
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
    assert!(s
        .eval::<bool>("return CalendarFrame:IsShown() == 1")
        .unwrap());
    assert_eq!(s.take_calendar_requests(), [CalendarRequest::GetCalendar]);
    assert_eq!(
        s.eval::<String>("return CalendarFrame.viewedMonth .. '/' .. CalendarFrame.viewedYear")
            .unwrap(),
        "10/2026"
    );

    s.calendar_packet(CalendarWire::SendCalendar(Box::new(snapshot())));
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "event list errors: {errors:#?}");
    let pending: f64 = s.eval("return CalendarGetNumPendingInvites()").unwrap();
    assert_eq!(pending, 1.0);
    // The 28th holds the invite, first as a pending invite, beside the install's holidays.
    let (n, first): (f64, i64) = s
        .eval("return CalendarGetNumDayEvents(0, 28), CalendarGetFirstPendingInvite(0, 28)")
        .unwrap();
    assert!(n >= 1.0 && first == 1, "{n} events, pending at {first}");
    assert_eq!(
        s.eval::<String>("return (CalendarGetDayEvent(0, 28, 1))")
            .unwrap(),
        "Naxxramas"
    );

    // Clicking it opens the event, which the server answers.
    s.run("CalendarOpenEvent(0, 28, 1)").unwrap();
    assert_eq!(
        s.take_calendar_requests(),
        [CalendarRequest::GetEvent { event_id: 7 }]
    );
    s.calendar_packet(CalendarWire::SendEvent(Box::new(CalendarEventView {
        creator: FRIEND,
        event_id: 7,
        title: "Naxxramas".into(),
        description: "Bring flasks".into(),
        max_size: 100,
        texture: -1,
        flags: 0x1,
        date: october(27, 20),
        invites: vec![
            CalendarInviteeView {
                guid: FRIEND,
                level: 80,
                status: 3,
                moderator: 2,
                invite_id: 71,
                ..Default::default()
            },
            CalendarInviteeView {
                guid: ME,
                level: 80,
                status: 0,
                invite_id: 70,
                ..Default::default()
            },
        ],
        ..Default::default()
    })));
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "viewer errors: {errors:#?}");
    assert!(s
        .eval::<bool>("return CalendarViewEventFrame:IsShown() == 1")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return CalendarViewEventTitle:GetText()")
            .unwrap(),
        "Naxxramas"
    );

    // Accept answers the invite.
    s.run("CalendarViewEventAcceptButton:Click()").unwrap();
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "accept errors: {errors:#?}");
    assert_eq!(
        s.take_calendar_requests(),
        [CalendarRequest::Rsvp {
            event_id: 7,
            invite_id: 70,
            status: 1
        }]
    );
}

/// The editor's create: the day menu's Create Event opens the editor with the player invited,
/// and Create sends the titled event at its day and time.
#[test]
fn the_stock_editor_creates_an_event() {
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut s) = calendar() else {
        return;
    };
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.run("ToggleCalendar()").unwrap();
    s.calendar_packet(CalendarWire::SendCalendar(Box::new(CalendarSnapshotView {
        server_date: today().encode(),
        ..Default::default()
    })));
    frame(&mut s);
    let _ = s.take_calendar_requests();
    let _ = s.take_errors();

    // The 30th's button, then Create Event from its menu.
    s.run(
        "for i = 1, 42 do local b = _G['CalendarDayButton' .. i]          if b.day == 30 and b.monthOffset == 0 then CalendarContextMenu.dayButton = b end end          CalendarDayContextMenu_CreateEvent()",
    )
    .unwrap_or_else(|e| panic!("create: {e}"));
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "editor errors: {errors:#?}");
    assert!(s
        .eval::<bool>("return CalendarCreateEventFrame:IsShown() == 1")
        .unwrap());
    let invites: f64 = s.eval("return CalendarEventGetNumInvites()").unwrap();
    assert_eq!(invites, 1.0, "the creator is the first invitee");

    s.run(
        "CalendarCreateEventTitleEdit:SetText('Raid night') \
         CalendarCreateEventTitleEdit:GetScript('OnTextChanged')(CalendarCreateEventTitleEdit) \
         CalendarCreateEventCreateButton:Click()",
    )
    .unwrap();
    frame(&mut s);
    let errors = calendar_errors(s.take_errors());
    assert!(errors.is_empty(), "create errors: {errors:#?}");
    let requests = s.take_calendar_requests();
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, CalendarRequest::AddEvent { title, .. } if title == "Raid night")),
        "the add went out: {requests:?}"
    );
}
