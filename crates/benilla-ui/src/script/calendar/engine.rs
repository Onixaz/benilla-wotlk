//! 12340's `CGCalendar` (`0x5b7820`-`0x5c43b0`): eight event tables keyed by date, three months
//! of day lists built from them, the holidays and raid resets computed for the shown window, the
//! pending invites, the clipboard, and the event held open. Each server message is applied as its
//! `PH_CALENDAR_GROUP` arm (`0x5c3fe0`) applies it; what it signals is queued for the caller to
//! fire once the model is released.

use std::collections::{HashMap, HashSet};

use super::event::{CurrentEvent, Invite, InviteEdit};
use super::time::WowTime;
use super::{
    CalendarCatalogView, CalendarHolidayView, CalendarName, CalendarPlayerView, CalendarRequest,
    CalendarWire,
};
use crate::script::lfg::Catalog;
use crate::script::ScriptValue;

/// `CalendarDayEvent::m_type`, the calendar flags the server sends and the client adds to.
pub(crate) mod kind {
    pub const PLAYER: u32 = 0x1;
    pub const SYSTEM: u32 = 0x4;
    pub const HOLIDAY: u32 = 0x8;
    pub const LOCKED: u32 = 0x10;
    pub const AUTO_APPROVE: u32 = 0x20;
    pub const ANNOUNCEMENT: u32 = 0x40;
    pub const RAID_LOCKOUT: u32 = 0x80;
    pub const RAID_RESET: u32 = 0x200;
    pub const GUILD_EVENT: u32 = 0x400;
    /// An announcement or a guild event: the guild table's.
    pub const GUILD: u32 = ANNOUNCEMENT | GUILD_EVENT;
    /// The kinds `CalendarOpenEvent` fetches from the server (`0x5b936b`).
    pub const FETCHED: u32 = 0x543;
}

/// `CalendarDayEvent::m_flags`, the client's own marks.
pub(crate) mod mark {
    pub const PENDING: u32 = 0x1;
    pub const MODERATOR: u32 = 0x2;
    pub const CREATOR: u32 = 0x4;
    /// A guild event's sign-up rather than an invite.
    pub const SIGN_UP: u32 = 0x8;
}

/// The event tables (`CalendarEventTables`, `0xc20dd8`): player, guild, lockouts, resets, plain
/// holidays, then the weekly, Darkmoon and battleground holidays.
pub(crate) const TABLES: usize = 8;
const TABLE_GUILD: usize = 1;
const TABLE_LOCKOUTS: usize = 2;
const TABLE_RESETS: usize = 3;
const TABLE_HOLIDAYS: usize = 4;

/// Each table's filter CVar (`s_eventFilters` `0xacff94`); `None` is shown always.
pub(crate) const FILTERS: [Option<&str>; TABLES] = [
    None,
    None,
    Some("calendarShowLockouts"),
    Some("calendarShowResets"),
    None,
    Some("calendarShowWeeklyHolidays"),
    Some("calendarShowDarkmoon"),
    Some("calendarShowBattlegrounds"),
];

/// Three months of 31 day lists: the one before the shown month, it, and the one after.
const MONTHS: usize = 3;
const DAYS: usize = 31;

/// The earliest calendar month, November 2004 (`0x5b8f3c`), and the latest year, 2030.
const MIN_YEAR: i32 = 4;
const MIN_MONTH: i32 = 10;
const MAX_YEAR: i32 = 30;

/// `CalendarDayEvent` (`0x5b7820`): one day's copy of an event, holiday, lockout or reset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DayEvent {
    /// A player event's id, a holiday's `Holidays.dbc` row, a lockout's instance id.
    pub event_id: u64,
    pub title: String,
    pub start: u32,
    pub alarm: u32,
    /// A holiday's end; a lockout's difficulty.
    pub end: u32,
    /// A lockout's or reset's map.
    pub holiday_id: u32,
    /// [`kind`].
    pub kind: u32,
    /// [`mark`].
    pub flags: u32,
    pub event_type: i32,
    pub texture: i32,
    pub invite_status: u8,
    pub invite_id: u64,
    pub creator: u64,
    /// The 15-minute alarm is armed (`m_alarmCallback`).
    pub alarm_armed: bool,
    pub sequence_index: u32,
    pub sequence_length: u32,
}

impl Default for DayEvent {
    fn default() -> Self {
        Self {
            event_id: 0,
            title: String::new(),
            start: 0,
            alarm: 0,
            end: 0,
            holiday_id: 0,
            kind: 0,
            flags: 0,
            event_type: -1,
            texture: 0,
            invite_status: 0,
            invite_id: 0,
            creator: 0,
            alarm_armed: false,
            sequence_index: 0,
            sequence_length: 0,
        }
    }
}

impl DayEvent {
    /// `GetSequenceType` (`0x5b7890`): 4 a one-day event, 1 its first day, 3 its last, 2 between.
    fn sequence_type(&self) -> i32 {
        if self.sequence_length <= 1 {
            return 4;
        }
        if self.sequence_index == 0 {
            return 1;
        }
        i32::from(self.sequence_index != self.sequence_length - 1) + 2
    }

    /// `ScheduleAlarm` (`0x5b79b0`): 15 minutes before the start.
    fn schedule_alarm(&mut self) {
        let mut alarm = WowTime::decode(self.start);
        alarm.add_minutes(-15);
        self.alarm = alarm.encode();
        self.alarm_armed = alarm.hour >= 0 && alarm.minute >= 0;
    }
}

/// `CalendarEventSelection` (`0xc20f78`, `0xc20f90`): an event named by its fields, found again
/// in whatever day list holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub event_id: u64,
    pub start: u32,
    pub end: u32,
    pub holiday_id: u32,
    pub kind: u32,
}

impl Selection {
    fn of(e: &DayEvent) -> Self {
        Self {
            event_id: e.event_id,
            start: e.start,
            end: e.end,
            holiday_id: e.holiday_id,
            kind: e.kind,
        }
    }
}

/// `CALENDARHOLIDAYNODE`: one of a holiday's ten date slots this faction sees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HolidayNode {
    /// The `Holidays.dbc` row, by position.
    holiday: usize,
    index: usize,
    start: u32,
    end: u32,
    /// A looping holiday's cycle, minutes.
    duration: i32,
    table: usize,
}

/// `CALENDARRAIDRESETNODE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ResetNode {
    map: i32,
    start: u32,
    /// Minutes.
    interval: i32,
}

/// `CalendarPendingInvite`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PendingInvite {
    event_id: u64,
    invite_id: u64,
    inviter: u64,
    status: u8,
    rank: u8,
    invite_type: u8,
    valid: bool,
}

/// A server-sent holiday that overrides the client's row (`DYNAMICHOLIDAYHASH`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DynamicHoliday {
    region: i32,
    looping: i32,
    priority: i32,
    filter_type: i32,
    dates: [i32; 26],
    durations: [i32; 10],
    flags: [i32; 10],
    texture: String,
}

/// What a verb or a message signals, fired in order once the model is released.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Signal {
    Event(&'static str, Vec<ScriptValue>),
    /// `CalendarError` (`0x5b9960`): the key's text, filled with the argument, is
    /// `CALENDAR_UPDATE_ERROR`'s.
    Error(&'static str, Option<ErrorArg>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ErrorArg {
    Int(i64),
    Str(String),
}

/// The model's other facts the calendar reads: the dungeon tables, the filter CVars, the guild.
pub(crate) struct Ctx<'a> {
    pub lfg: &'a Catalog,
    /// Each table's filter: shown, by its CVar.
    pub filters: [bool; TABLES],
    /// `CGGuildInfo::CanEditGuildEvent` (`0x5cbca0`): the rank's right `0x100000`.
    pub can_edit_guild_event: bool,
    /// `GetTime`, the throttles' clock.
    pub now_secs: f64,
}

/// The calendar's whole state.
#[derive(Clone, Debug, PartialEq)]
pub struct CalendarModel {
    /// `m_initialize` (`0xc207f0`): the next world entry is the first since login.
    initialize: bool,
    /// `m_openedCalendar` (`0xc207f1`).
    opened: bool,
    /// `m_calendarReceived` (`0xc207f2`).
    received: bool,
    /// `m_numInitialPending` (`0xc207f4`), the server's count before the calendar lands.
    num_initial_pending: i32,
    /// `m_actionPending` (`0xc207f3`).
    pub(crate) action_pending: bool,
    /// `m_lastEventTime` and `m_lastInviteTime` (`0xc207fc`, `0xc207f8`): `GetTime` seconds.
    last_event: Option<f64>,
    last_invite: Option<f64>,
    /// `m_absMonth`, `m_absYear`, `m_absMonthDays` (`0xacff8c`, `0xacff90`, `0xc20800`): the shown
    /// month; no days is "not yet chosen".
    abs_month: i32,
    abs_year: i32,
    abs_month_days: i32,
    selected: Selection,
    context: Selection,
    /// `m_contextEventInfo` (`0xc207e8`): the day event the context menu was opened on.
    context_event: Option<usize>,
    /// `s_eventClipboard` (`0xad0350`); an event id of 0 is empty.
    clipboard: DayEvent,
    /// The day events, slotted; the tables and day lists hold indices.
    slab: Vec<Option<DayEvent>>,
    free: Vec<usize>,
    tables: [HashMap<u32, Vec<usize>>; TABLES],
    /// `s_days` (`0xc20808`).
    days: [[Vec<usize>; DAYS]; MONTHS],
    holidays: Vec<HolidayNode>,
    resets: Vec<ResetNode>,
    pending: Vec<PendingInvite>,
    dynamic: HashMap<u32, DynamicHoliday>,
    /// `s_eventTextures` (`0xc20f60`): the dungeon, then the raid `LfgDungeons.dbc` rows.
    textures: [Vec<i32>; 2],
    /// `m_curEvent` (`0xc207ec`).
    pub(crate) current: Option<CurrentEvent>,
    /// `g_clientGameTime`.
    pub(crate) now: WowTime,
    catalog: CalendarCatalogView,
    holiday_index: HashMap<u32, usize>,
    /// The client's Region (`s_region`).
    region: i32,
    pub(crate) player: Option<CalendarPlayerView>,
    pub(crate) names: HashMap<u64, CalendarName>,
    /// Names a day event or the event view asked for (`NameCacheCallback` `0x5b78d0`).
    pub(crate) wanted_names: HashSet<u64>,
    /// Creators the calendar's arrival asks for with no callback (`0x5c30af`).
    prefetch_names: HashSet<u64>,
    ignored: HashSet<u64>,
    friends: HashSet<u64>,
    pub(crate) raid_members: HashSet<u64>,
    /// `CGChat::m_complaintsSystemStatus`, off until `SMSG_FEATURE_SYSTEM_STATUS`, which is not
    /// modelled.
    complaints_enabled: bool,
    pub(crate) requests: Vec<CalendarRequest>,
    pub(crate) signals: Vec<Signal>,
}

impl Default for CalendarModel {
    /// `CGCalendar::InitializeGame` (`0x5c4240`) on the statics' load-time values.
    fn default() -> Self {
        Self {
            initialize: true,
            opened: false,
            received: false,
            num_initial_pending: 0,
            action_pending: false,
            last_event: None,
            last_invite: None,
            abs_month: -1,
            abs_year: MIN_YEAR,
            abs_month_days: 0,
            selected: Selection::default(),
            context: Selection::default(),
            context_event: None,
            clipboard: DayEvent {
                event_type: 0,
                ..DayEvent::default()
            },
            slab: Vec::new(),
            free: Vec::new(),
            tables: Default::default(),
            days: Default::default(),
            holidays: Vec::new(),
            resets: Vec::new(),
            pending: Vec::new(),
            dynamic: HashMap::new(),
            textures: Default::default(),
            current: None,
            now: WowTime::default(),
            catalog: CalendarCatalogView::default(),
            holiday_index: HashMap::new(),
            region: 0,
            player: None,
            names: HashMap::new(),
            wanted_names: HashSet::new(),
            prefetch_names: HashSet::new(),
            ignored: HashSet::new(),
            friends: HashSet::new(),
            raid_members: HashSet::new(),
            complaints_enabled: false,
            requests: Vec::new(),
            signals: Vec::new(),
        }
    }
}

/// `GetDaysInMonth` (`0x5b7a10`): `(days, weekday of the 1st)`. Its December guard tests the month
/// itself against 12, so December reaches month 12, which `mktime` takes as next January.
pub(crate) fn days_in_month(month: i32, year: i32) -> (i32, i32) {
    let mut date = WowTime::date(month, 0, year);
    date.add_days(0, false);
    let weekday = date.weekday;
    date.month = month + 1;
    date.year = if month == 12 { year + 1 } else { year };
    if month == 12 {
        date.month = 0;
    }
    date.add_days(-1, false);
    (date.month_day + 1, weekday)
}

/// `GetCalendarTypeString` (`0x5b7fd0`).
pub(crate) fn calendar_type(kind_bits: u32) -> &'static str {
    if kind_bits & kind::PLAYER != 0 {
        "PLAYER"
    } else if kind_bits & kind::ANNOUNCEMENT != 0 {
        "GUILD_ANNOUNCEMENT"
    } else if kind_bits & kind::GUILD_EVENT != 0 {
        "GUILD_EVENT"
    } else if kind_bits & kind::SYSTEM != 0 {
        "SYSTEM"
    } else if kind_bits & kind::HOLIDAY != 0 {
        "HOLIDAY"
    } else if kind_bits & kind::RAID_LOCKOUT != 0 {
        "RAID_LOCKOUT"
    } else if kind_bits & kind::RAID_RESET != 0 {
        "RAID_RESET"
    } else if kind_bits & (0x2 | 0x100) != 0 {
        "PLAYER"
    } else {
        ""
    }
}

/// `GetDateKey` (`0x5b9140`): a packed time's date alone.
fn date_key(time: u32) -> u32 {
    let t = WowTime::decode(time);
    WowTime::date(t.month, t.month_day, t.year).encode()
}

/// `ValidateDate` (`0x5b7b30`): from the 24th of November 2004 to the last day of 2030.
pub(crate) fn validate_date(date: &WowTime) -> bool {
    let mut check = *date;
    let mut limit = WowTime::date(MIN_MONTH, 23, MIN_YEAR);
    limit.add_days(0, false);
    limit.set_hour_and_minutes(0, 0);
    check.hour = limit.hour;
    check.minute = limit.minute;
    if check.lt(&limit) {
        return false;
    }
    let mut limit = WowTime::date(0, 0, 31);
    limit.add_days(-1, false);
    limit.set_hour_and_minutes(23, 59);
    check.hour = limit.hour;
    check.minute = limit.minute;
    !check.gt(&limit)
}

/// `GameCalendarGetMaxCreateDate` (`0x5b7a80`): this month's last day a year on, 23:59.
pub(crate) fn max_create_date(now: &WowTime) -> WowTime {
    let mut date = WowTime::date((now.month + 13) % 12, 0, now.year + (now.month + 13) / 12);
    date.add_days(-1, false);
    date.year = date.year.min(31);
    date.set_hour_and_minutes(23, 59);
    date
}

impl CalendarModel {
    // ── Lookups ─────────────────────────────────────────────────────────────────────────────

    pub(crate) fn set_catalog(&mut self, catalog: CalendarCatalogView) {
        self.holiday_index = catalog
            .holidays
            .iter()
            .enumerate()
            .map(|(i, h)| (h.id, i))
            .collect();
        self.catalog = catalog;
    }

    pub(crate) fn set_region(&mut self, region: i32) {
        self.region = region;
    }

    /// The ignore and friend lists already stand as given.
    pub(crate) fn social_is(&self, ignored: &HashSet<u64>, friends: &HashSet<u64>) -> bool {
        self.ignored == *ignored && self.friends == *friends
    }

    pub(crate) fn set_social(&mut self, ignored: HashSet<u64>, friends: HashSet<u64>) -> bool {
        let moved = self.ignored != ignored;
        self.ignored = ignored;
        self.friends = friends;
        moved
    }

    /// `Holidays.dbc`'s row by id (`g_holidaysDB.GetRecord`).
    pub(crate) fn holiday(&self, id: u32) -> Option<&CalendarHolidayView> {
        self.holiday_index
            .get(&id)
            .map(|&i| &self.catalog.holidays[i])
    }

    pub(crate) fn map_name(&self, map: u32) -> Option<&str> {
        self.catalog.map_names.get(&map).map(String::as_str)
    }

    fn is_ignored(&self, guid: u64) -> bool {
        guid != 0 && self.ignored.contains(&guid)
    }

    /// A player's name for a Lua answer, `""` while in flight; the guid is then asked for.
    pub(crate) fn name_or_ask(&mut self, guid: u64) -> String {
        match self.names.get(&guid) {
            Some(n) => n.name.clone(),
            None => {
                if guid != 0 {
                    self.wanted_names.insert(guid);
                }
                String::new()
            }
        }
    }

    fn player_guid(&self) -> u64 {
        self.player.as_ref().map_or(0, |p| p.guid)
    }

    fn event(&self, id: usize) -> &DayEvent {
        self.slab[id].as_ref().expect("a live day event")
    }

    fn event_mut(&mut self, id: usize) -> &mut DayEvent {
        self.slab[id].as_mut().expect("a live day event")
    }

    fn alloc(&mut self, e: DayEvent) -> usize {
        match self.free.pop() {
            Some(i) => {
                self.slab[i] = Some(e);
                i
            }
            None => {
                self.slab.push(Some(e));
                self.slab.len() - 1
            }
        }
    }

    fn release(&mut self, id: usize) {
        if self.slab[id].take().is_some() {
            self.free.push(id);
        }
        if self.context_event == Some(id) {
            self.context_event = None;
        }
    }

    /// Empty one table, its events freed (`TSHashTable::InternalClear`).
    fn clear_table(&mut self, table: usize) {
        let ids: Vec<usize> = self.tables[table].drain().flat_map(|(_, v)| v).collect();
        for id in ids {
            self.release(id);
        }
    }

    // ── Signals ─────────────────────────────────────────────────────────────────────────────

    fn signal(&mut self, event: &'static str, args: Vec<ScriptValue>) {
        self.signals.push(Signal::Event(event, args));
    }

    /// `CalendarError` (`0x5b9960`): the error line, then the action is no longer pending.
    pub(crate) fn error(&mut self, key: &'static str, arg: Option<ErrorArg>) {
        self.signals.push(Signal::Error(key, arg));
        self.action_pending = false;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(false)]);
    }

    /// `SetActionPending` (`0x5b7e20`).
    pub(crate) fn set_action_pending(&mut self, pending: bool) {
        self.action_pending = pending;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(pending)]);
    }

    fn send(&mut self, request: CalendarRequest) {
        self.requests.push(request);
    }

    // ── World entry and the clock ───────────────────────────────────────────────────────────

    /// `CGCalendar::EnterWorld` (`0x5be2a0`): nothing pending; on the first entry since login
    /// the clipboard empties, the pending count is asked for and the texture lists are built
    /// from `LfgDungeons.dbc`, raids and dungeons of the player's side, by expansion then name.
    pub(crate) fn enter_world(&mut self, ctx: &Ctx) {
        self.set_action_pending(false);
        if !self.initialize {
            return;
        }
        let Some(player) = self.player.clone() else {
            return;
        };
        self.clipboard = DayEvent {
            event_type: 0,
            ..DayEvent::default()
        };
        self.send(CalendarRequest::GetNumPending);
        let mut lists: [Vec<i32>; 2] = Default::default();
        for d in &ctx.lfg.view.dungeons {
            if d.faction >= 0 && d.faction != player.faction_group {
                continue;
            }
            match d.type_id {
                1 => lists[0].push(d.id),
                2 => lists[1].push(d.id),
                _ => {}
            }
        }
        // `QSortTextures` (`0x5b8e50`): the newest expansion first, then by name.
        for list in &mut lists {
            list.sort_by(|&a, &b| {
                let (Some(ra), Some(rb)) = (ctx.lfg.dungeon(a), ctx.lfg.dungeon(b)) else {
                    return std::cmp::Ordering::Equal;
                };
                rb.expansion_level
                    .cmp(&ra.expansion_level)
                    .then_with(|| crate::script::who_sort::sort_utf8i(&ra.name, &rb.name))
            });
        }
        self.textures = lists;
        self.initialize = false;
    }

    /// The game clock moved to `now`; an alarm whose minute this is fires for an accepted or
    /// confirmed invite (`FireEventAlarm` `0x5b7910`).
    pub(crate) fn tick_clock(&mut self, now: WowTime) {
        let moved = self.now.hour_and_minutes() != now.hour_and_minutes();
        self.now = now;
        if !moved || now.hour < 0 {
            return;
        }
        let mut fired = Vec::new();
        for slot in self.slab.iter_mut().flatten() {
            if !slot.alarm_armed {
                continue;
            }
            let mut alarm = WowTime::decode(slot.alarm);
            alarm.tz_hours = 0;
            if alarm.hour_and_minutes() != now.hour_and_minutes() {
                continue;
            }
            if now.same(&alarm) && matches!(slot.invite_status, 1 | 3) {
                let start = WowTime::decode(slot.start);
                fired.push((slot.title.clone(), start.hour, start.minute));
                slot.alarm_armed = false;
            }
        }
        for (title, hour, minute) in fired {
            self.signal(
                "CALENDAR_EVENT_ALARM",
                vec![
                    ScriptValue::Str(title),
                    ScriptValue::Int(i64::from(hour)),
                    ScriptValue::Int(i64::from(minute)),
                ],
            );
        }
    }

    /// `CountPendingInvites` (`0x5bec70`): the pending invites from a known, unignored inviter, or
    /// the server's count if that is more.
    pub(crate) fn count_pending(&self) -> u32 {
        let count = self
            .pending
            .iter()
            .filter(|p| p.inviter != 0 && !self.is_ignored(p.inviter))
            .count() as u32;
        count.max(self.num_initial_pending.max(0) as u32)
    }

    fn has_pending_invite(&self, event_id: u64) -> bool {
        self.pending.iter().any(|p| p.event_id == event_id)
    }

    fn delete_pending_invite(&mut self, event_id: u64) -> bool {
        match self.pending.iter().position(|p| p.event_id == event_id) {
            Some(i) => {
                self.pending.remove(i);
                true
            }
            None => false,
        }
    }

    /// `AddPendingInvite` (`0x5be590`).
    fn add_pending_invite(&mut self, id: usize) {
        let e = self.event_mut(id);
        e.flags |= mark::PENDING;
        let invite = PendingInvite {
            event_id: e.event_id,
            invite_id: e.invite_id,
            inviter: e.creator,
            ..PendingInvite::default()
        };
        self.pending.push(invite);
        self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
    }

    // ── The shown month ─────────────────────────────────────────────────────────────────────

    /// `GetMonth` (`0x5b8ec0`): the shown month moved by `offset`, held to November 2004 and
    /// 2030; `(month, year, days, weekday of the 1st)`.
    pub(crate) fn month(&self, offset: i32) -> (i32, i32, i32, i32) {
        let (mut month, mut year) = if self.abs_month_days != 0 {
            (self.abs_month, self.abs_year)
        } else {
            (self.now.month, self.now.year)
        };
        if offset != 0 {
            year += offset / 12;
            month += offset % 12;
            if month >= 12 {
                year += 1;
                month -= 12;
            } else if month < 0 {
                year -= 1;
                month += 12;
            }
        }
        year = year.min(MAX_YEAR);
        if year <= MIN_YEAR {
            year = MIN_YEAR;
            month = month.max(MIN_MONTH);
        }
        let (days, weekday) = days_in_month(month, year);
        (month, year, days, weekday)
    }

    /// The year an absolute-month verb starts from: the shown one, or this year (`0x5c43d5`).
    pub(crate) fn abs_year_or_now(&mut self) -> i32 {
        if self.abs_year < 0 {
            self.abs_year = if self.now.year < 0 {
                MIN_YEAR
            } else {
                self.now.year
            };
        }
        self.abs_year
    }

    /// `ClampAbsMonthDay` (`0x5b8f70`): the year within five of this one and 2004 to 2030, the
    /// month as `abs(month % 12)`.
    pub(crate) fn clamp_abs_month(&self, month: i32, year: i32) -> (i32, i32, i32, i32) {
        let min = self.now.year - 5;
        let year = year.clamp(min, min + 10).clamp(MIN_YEAR, MAX_YEAR);
        let month = (month % 12).abs();
        let (days, weekday) = days_in_month(month, year);
        (month, year, days, weekday)
    }

    /// `CalendarSetMonth` and `CalendarSetAbsMonth` (`0x5bcc00`, `0x5c43b0`): show a month.
    pub(crate) fn show_month(&mut self, ctx: &Ctx, month: i32, year: i32, days: i32) {
        self.abs_month = month;
        self.abs_year = year;
        self.abs_month_days = days;
        self.calculate_events(ctx, true);
    }

    /// `GetDayEvent(offset, day)` (`0x5b91a0`): the offset is -1, 0 or 1 and the day valid.
    fn day_valid(&self, offset: i32, day: u32) -> bool {
        if !(-1..=1).contains(&offset) || day >= DAYS as u32 {
            return false;
        }
        let (month, year, _, _) = self.month(offset);
        let mut date = WowTime::default();
        date.set_date(month as u32, day, year as u32);
        validate_date(&date)
    }

    fn day_list(&self, offset: i32, day: u32) -> Option<&Vec<usize>> {
        self.day_valid(offset, day)
            .then(|| &self.days[(offset + 1) as usize][day as usize])
    }

    /// `GetNumDayEvents` (`0x5b9220`).
    pub(crate) fn num_day_events(&self, offset: i32, day: u32) -> usize {
        self.day_list(offset, day).map_or(0, Vec::len)
    }

    /// `GetFirstPendingInviteIndex` (`0x5b92b0`): 0-based, -1 none.
    pub(crate) fn first_pending(&self, offset: i32, day: u32) -> i32 {
        self.day_list(offset, day)
            .and_then(|l| {
                l.iter()
                    .position(|&id| self.event(id).flags & mark::PENDING != 0)
            })
            .map_or(-1, |i| i as i32)
    }

    /// `GetDayEvent(offset, day, index)` (`0x5b9260`).
    pub(crate) fn day_event_id(&self, offset: i32, day: u32, index: u32) -> Option<usize> {
        self.day_list(offset, day)
            .and_then(|l| l.get(index as usize).copied())
    }

    pub(crate) fn day_event(&self, offset: i32, day: u32, index: u32) -> Option<&DayEvent> {
        self.day_event_id(offset, day, index)
            .map(|id| self.event(id))
    }

    pub(crate) fn context_event(&self) -> Option<&DayEvent> {
        self.context_event.map(|id| self.event(id))
    }

    /// The day event a context verb names, or the context menu's when its three numbers are
    /// absent; `None` is the verb's "no such event".
    pub(crate) fn context_or(&self, at: Option<(i32, u32, u32)>) -> Option<usize> {
        match at {
            Some((offset, day, index)) => self.day_event_id(offset, day, index),
            None => self.context_event,
        }
    }

    pub(crate) fn get(&self, id: usize) -> &DayEvent {
        self.event(id)
    }

    /// `ComputeEventIndex` (`0x5b9000`): `(month offset, day, index)`, 0-based day and index.
    pub(crate) fn event_index(&self, context: bool) -> Option<(i32, u32, u32)> {
        let s = if context {
            &self.context
        } else {
            &self.selected
        };
        if s.event_id == 0 && s.holiday_id == 0 && s.start == 0 {
            return None;
        }
        let date = WowTime::decode(s.start);
        let month = (12 * (date.year - self.abs_year) - self.abs_month + date.month + 1) as u32;
        if month > 2 || date.month_day < 0 {
            return None;
        }
        let list = self.days[month as usize].get(date.month_day as usize)?;
        for (i, &id) in list.iter().enumerate() {
            let e = self.event(id);
            let mut hit = s.event_id != 0 && s.event_id == e.event_id;
            if s.holiday_id != 0 {
                hit = s.holiday_id == e.holiday_id;
                if !hit {
                    continue;
                }
                if (s.kind as i32) < 0 {
                    hit = s.end == e.end;
                }
            }
            if hit {
                return Some((month as i32 - 1, date.month_day as u32, i as u32));
            }
        }
        None
    }

    pub(crate) fn close_selected(&mut self) {
        self.selected = Selection::default();
    }

    /// `SelectContextEvent` (`0x5b97d0`).
    pub(crate) fn select_context(&mut self, offset: i32, day: u32, index: u32) {
        if let Some(id) = self.day_event_id(offset, day, index) {
            self.context = Selection::of(self.event(id));
            self.context_event = Some(id);
        }
    }

    pub(crate) fn deselect_context(&mut self) {
        self.context = Selection::default();
        self.context_event = None;
    }

    pub(crate) fn clipboard_full(&self) -> bool {
        self.clipboard.event_id != 0
    }

    // ── The event tables ────────────────────────────────────────────────────────────────────

    /// `InsertEvent` (`0x5c2030`): one copy per day the event spans, each told its place in the
    /// run; a run's last copy starts at the event's end. The first copy alone keeps the alarm.
    fn insert_event(&mut self, event: DayEvent, table: usize) -> usize {
        let original_end = event.end;
        let mut start = WowTime::decode(event.start);
        let end = if event.end != 0 {
            let mut end = WowTime::decode(event.end);
            if end.year == -1 {
                end.year = 31;
            }
            end
        } else {
            start
        };
        let mut day = start;
        let mut num_days = 0u32;
        // The reference loops while the day is not past the end; an undated day never moves.
        while day.le(&end) && num_days < 2 * 366 * 31 {
            num_days += 1;
            let before = day;
            day.add_days(1, false);
            if day == before {
                break;
            }
        }
        let mut month = start.month;
        let mut sequence = 0;
        let mut first = true;
        let mut current = event;
        let mut last;
        loop {
            let key = WowTime::date(month, start.month_day, start.year).encode();
            if !first {
                current = DayEvent {
                    alarm_armed: false,
                    ..current.clone()
                };
            }
            current.sequence_index = sequence;
            current.sequence_length = num_days;
            sequence += 1;
            let id = self.alloc(current.clone());
            self.tables[table].entry(key).or_default().push(id);
            last = id;
            if first {
                start.minute = -1;
                start.hour = -1;
                start.weekday = -1;
                first = false;
            }
            let before = start;
            start.add_days(1, false);
            if start.gt(&end) || start == before {
                break;
            }
            month = start.month;
            let at_end = start.month == end.month && start.month_day == end.month_day;
            if at_end && end.hour == 0 && end.minute == 0 {
                break;
            }
        }
        if num_days > 1 {
            self.event_mut(last).start = original_end;
        }
        last
    }

    /// `CalendarEventTables::GetEvent` (`0x5bea50`).
    fn table_event(&self, table: usize, date: u32, event_id: u64) -> Option<usize> {
        self.tables[table]
            .get(&date)?
            .iter()
            .copied()
            .find(|&id| self.event(id).event_id == event_id)
    }

    /// `CalendarEventTables::Delete` (`0x5be950`).
    fn table_delete(&mut self, table: usize, date: u32, event_id: u64) -> bool {
        let Some(list) = self.tables[table].get_mut(&date) else {
            return false;
        };
        let Some(pos) = list.iter().position(|&id| {
            self.slab[id]
                .as_ref()
                .is_some_and(|e| e.event_id == event_id)
        }) else {
            return false;
        };
        let id = list.remove(pos);
        self.release(id);
        true
    }

    /// The table a guild-or-not flag word picks: 1 for an announcement or guild event.
    fn table_of(kind_bits: u32) -> usize {
        usize::from(kind_bits & kind::GUILD != 0)
    }

    /// The open event's own day event, found by its date (`0x5bf0de`).
    fn current_day_event(&self) -> Option<usize> {
        let cur = self.current.as_ref()?;
        let key = WowTime::date(cur.date.month, cur.date.month_day, cur.date.year).encode();
        self.table_event(Self::table_of(cur.flags), key, cur.event_id)
    }

    /// The first day event with this id in one table, in table order.
    fn find_in_table(&self, table: usize, event_id: u64) -> Option<usize> {
        let mut keys: Vec<&u32> = self.tables[table].keys().collect();
        keys.sort_unstable();
        keys.into_iter()
            .flat_map(|k| self.tables[table][k].iter().copied())
            .find(|&id| self.event(id).event_id == event_id)
    }

    /// `QSortEvents` (`0x5bfd20`): pending invites, then fetched events, then holidays by
    /// priority; then by run position, start and name.
    fn compare_events(&self, a: &DayEvent, b: &DayEvent) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let pa = a.flags & mark::PENDING != 0;
        let pb = b.flags & mark::PENDING != 0;
        if pa != pb {
            return if pa {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let fa = a.kind & kind::FETCHED != 0;
        let fb = b.kind & kind::FETCHED != 0;
        if fa != fb {
            return if fa {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let ha = a.kind & kind::HOLIDAY != 0;
        let hb = b.kind & kind::HOLIDAY != 0;
        if ha != hb {
            return if ha {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        // A server override's priority wins, and its holiday then keeps the title as its name.
        let rank = |e: &DayEvent| -> (i32, Option<&CalendarHolidayView>) {
            if e.kind & kind::HOLIDAY == 0 {
                return (-1, None);
            }
            if let Some(d) = self.dynamic.get(&(e.event_id as u32)) {
                return (d.priority, None);
            }
            match self.holiday(e.event_id as u32) {
                Some(h) => (h.priority, Some(h)),
                None => (-1, None),
            }
        };
        let (prio_a, rec_a) = rank(a);
        let (prio_b, rec_b) = rank(b);
        if prio_b != prio_a {
            return prio_b.cmp(&prio_a);
        }
        let s = a.sequence_type().cmp(&b.sequence_type());
        if s != Ordering::Equal {
            return s;
        }
        let d = (a.start.wrapping_sub(b.start)) as i32;
        if d != 0 {
            return d.cmp(&0);
        }
        let name = |e: &DayEvent, rec: Option<&CalendarHolidayView>| -> String {
            if let Some(r) = rec {
                if let Some(n) = &r.name {
                    return n.clone();
                }
            } else if e.kind & (kind::RAID_LOCKOUT | kind::RAID_RESET) != 0 {
                if let Some(m) = self.map_name(e.holiday_id) {
                    return m.to_string();
                }
            }
            e.title.clone()
        };
        crate::script::who_sort::cmp_utf8i(&name(a, rec_a), &name(b, rec_b))
    }

    /// `UpdateDisplayList` (`0x5bff30`): rebuild the three months' day lists from the tables
    /// their filters show, dropping an ignored creator's events. Before a month is chosen it
    /// shows this one and computes its events instead.
    pub(crate) fn update_display_list(&mut self, ctx: &Ctx, signal: bool) {
        if self.abs_month_days == 0 {
            let (mut month, mut year) = (self.now.month, self.now.year);
            self.abs_month = month;
            self.abs_year = year;
            if year < MAX_YEAR {
                if year <= MIN_YEAR {
                    year = MIN_YEAR;
                    self.abs_year = MIN_YEAR;
                    if self.now.month <= MIN_MONTH {
                        month = MIN_MONTH;
                        self.abs_month = MIN_MONTH;
                    }
                }
            } else {
                year = MAX_YEAR;
                self.abs_year = MAX_YEAR;
            }
            self.abs_month_days = days_in_month(month, year).0;
            self.calculate_events(ctx, true);
            return;
        }
        self.rebuild_days(&ctx.filters);
        if signal {
            self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
        }
    }

    /// `SetFilter` (`0x5c0240`): a filter CVar's change rebuilds the lists, signalling nothing.
    pub(crate) fn set_filter(&mut self, filters: &[bool; TABLES]) {
        if self.abs_month_days != 0 {
            self.rebuild_days(filters);
        }
    }

    fn rebuild_days(&mut self, filters: &[bool; TABLES]) {
        let mut month = (self.abs_month + 11) % 12;
        let mut year = if month == 11 {
            self.abs_year - 1
        } else {
            self.abs_year
        };
        for m in 0..MONTHS {
            for d in 0..DAYS {
                let key = WowTime::date(month, d as i32, year).encode();
                let mut list = Vec::new();
                for (t, table) in self.tables.iter().enumerate() {
                    if !filters[t] {
                        continue;
                    }
                    if let Some(ids) = table.get(&key) {
                        list.extend(
                            ids.iter()
                                .copied()
                                .filter(|&id| !self.is_ignored(self.event(id).creator)),
                        );
                    }
                }
                list.sort_by(|&x, &y| self.compare_events(self.event(x), self.event(y)));
                self.days[m][d] = list;
            }
            month = (month + 1) % 12;
            if month == 0 {
                year += 1;
                if year >= 31 {
                    break;
                }
            }
        }
    }

    // ── Holidays, resets and lockouts ───────────────────────────────────────────────────────

    /// `InitHolidays` (`0x5c04f0`): a node per date slot the player's side sees, for each
    /// holiday of the client's Region; a looping holiday's node carries its cycle.
    fn init_holidays(&mut self) {
        let side = self.player.as_ref().map_or(-1, |p| p.pvp_faction);
        let side_mask = match side {
            1 => 1,
            0 => 2,
            _ => 0,
        };
        if side_mask & 3 == 0 {
            return;
        }
        let region = self.region as i8 as i32;
        for (i, rec) in self.catalog.holidays.iter().enumerate() {
            let dynamic = self.dynamic.get(&rec.id);
            let region_mask = dynamic.map_or(rec.region, |d| d.region);
            // `shl` takes its count mod 32 (`0x5c05c4`), so Region 0 tests bit 31.
            if region_mask != 0 && 1i32.wrapping_shl((region - 1) as u32) & region_mask == 0 {
                continue;
            }
            let (looping, flags, durations, filter) = match dynamic {
                Some(d) => (d.looping, d.flags, d.durations, d.filter_type),
                None => (
                    rec.looping,
                    rec.calendar_flags,
                    rec.durations,
                    rec.filter_type,
                ),
            };
            for j in 0..10 {
                if side_mask & flags[j] == 0 {
                    continue;
                }
                let mut node = HolidayNode {
                    holiday: i,
                    index: j,
                    table: TABLE_HOLIDAYS,
                    ..HolidayNode::default()
                };
                if looping != 0 {
                    let mut total = if durations[j] != 0 { durations[j] } else { 24 };
                    for (k, &d) in durations.iter().enumerate() {
                        if k != j {
                            total += d;
                        }
                        if k == 0 && durations[0] == 0 {
                            total += 3;
                        }
                    }
                    node.duration = total * 60;
                }
                node.table = match filter {
                    0 => 5,
                    1 => 6,
                    2 => 7,
                    _ => TABLE_HOLIDAYS,
                };
                self.holidays.push(node);
            }
        }
    }

    /// `ComputeEventOccurrence` (`0x5bfab0`): a holiday slot's start and end that overlap the
    /// window, and whether its date is a weekday alone.
    fn occurrence(
        &mut self,
        range_start: &WowTime,
        range_end: &WowTime,
        rec: &CalendarHolidayView,
        index: usize,
        interval: i32,
    ) -> Option<(WowTime, WowTime, bool)> {
        let (durations, dates, looping) = match self.dynamic.get(&rec.id) {
            Some(d) => (d.durations, d.dates, d.looping),
            None => (rec.durations, rec.dates, rec.looping),
        };
        if index > 0 && durations[index] == 0 {
            return None;
        }
        let duration = if durations[index] != 0 {
            durations[index]
        } else {
            24
        };
        let duration_minutes = 60 * duration;
        let now = self.now;
        for &date in dates.iter() {
            if date == 0 {
                continue;
            }
            let mut start = WowTime::decode(date as u32);
            if rec.flags & 0x1 != 0 {
                start.tz_hours = now.tz_hours;
                start.convert_to_local();
            }
            let mut reference = now;
            let mut weekly = false;
            let mut days = 0;
            if start.weekday >= 0 && start.month < 0 && start.month_day < 0 && start.year < 0 {
                reference = *range_start;
                weekly = true;
                let diff = start.weekday - range_start.weekday;
                days = diff + if diff < 0 { 7 } else { 0 };
            }
            if start.minute < 0 {
                start.minute = reference.minute;
            }
            if start.hour < 0 {
                start.hour = reference.hour;
            }
            if start.month_day < 0 {
                start.month_day = reference.month_day;
            }
            if start.month < 0 {
                start.month = reference.month;
            }
            if start.year < 0 {
                start.year = self.abs_year_or_now();
                if self.abs_month == 0 && start.month == 11 {
                    start.year -= 1;
                }
            }
            if days != 0 {
                start.add_days(days, false);
            }
            for &previous in durations.iter().take(index) {
                start.add_holiday_duration(60 * if previous != 0 { previous } else { 24 });
            }
            let after = range_end.lt(&start);
            let mut end;
            if looping != 0 {
                if after {
                    return None;
                }
                let mut guard = 0;
                loop {
                    end = start;
                    end.add_holiday_duration(duration_minutes);
                    if !range_start.ge(&end) {
                        break;
                    }
                    start.add_holiday_duration(interval);
                    guard += 1;
                    if range_end.lt(&start) || interval <= 0 || guard > 100_000 {
                        return None;
                    }
                }
            } else {
                if after {
                    continue;
                }
                end = start;
                end.add_holiday_duration(duration_minutes);
                if end.lt(range_start) {
                    continue;
                }
            }
            if start.hour == 0 && start.minute == 0 {
                start.add_minutes(1);
            }
            if end.hour == 0 && end.minute == 0 {
                end.add_minutes(-1);
            }
            return Some((start, end, weekly));
        }
        None
    }

    /// `CalculateEvents` (`0x5c23c0`): recompute the holidays and resets for the window from six
    /// days before the shown month to 42 days into it, then rebuild the day lists.
    pub(crate) fn calculate_events(&mut self, ctx: &Ctx, signal: bool) {
        for table in [4, 5, 6, 7, TABLE_RESETS] {
            self.clear_table(table);
        }
        let mut range_start = WowTime::date(self.abs_month, 0, self.abs_year);
        range_start.set_hour_and_minutes(0, 0);
        range_start.add_days(-6, false);
        let mut range_end = WowTime::date(self.abs_month, 0, self.abs_year);
        range_end.set_hour_and_minutes(0, 0);
        range_end.add_days(42, false);

        if self.holidays.is_empty() {
            self.init_holidays();
        }
        for n in 0..self.holidays.len() {
            let node = self.holidays[n].clone();
            let rec = self.catalog.holidays[node.holiday].clone();
            match self.occurrence(&range_start, &range_end, &rec, node.index, node.duration) {
                Some((mut start, mut end, weekly)) => {
                    if weekly {
                        self.holidays[n].duration = 10080;
                    } else {
                        start.add_days(0, false);
                        end.add_days(0, false);
                    }
                    self.holidays[n].start = start.encode();
                    self.holidays[n].end = end.encode();
                }
                None => {
                    self.holidays[n].start = 0;
                    self.holidays[n].end = 0;
                }
            }
        }
        for n in 0..self.holidays.len() {
            let node = self.holidays[n].clone();
            if node.start == 0 {
                continue;
            }
            let rec = &self.catalog.holidays[node.holiday];
            if rec.name.is_none() {
                continue;
            }
            let id = u64::from(rec.id);
            let holiday = |start: u32, end: u32| DayEvent {
                event_id: id,
                start,
                end,
                kind: kind::HOLIDAY,
                ..DayEvent::default()
            };
            if node.duration != 0 {
                let mut start = WowTime::decode(node.start);
                let mut end = WowTime::decode(node.end);
                if end.year == -1 {
                    end.year = 31;
                }
                if !start.lt(&range_end) {
                    continue;
                }
                while start.year < 31 {
                    self.insert_event(holiday(start.encode(), end.encode()), node.table);
                    start.add_holiday_duration(node.duration);
                    end.add_holiday_duration(node.duration);
                    if !start.lt(&range_end) {
                        break;
                    }
                }
            } else {
                self.insert_event(holiday(node.start, node.end), node.table);
            }
        }
        for r in self.resets.clone() {
            if r.start == 0 || r.interval == 0 {
                continue;
            }
            let mut reset = WowTime::decode(r.start);
            reset.add_holiday_duration(r.interval);
            while reset.lt(&range_end) && reset.year < 31 {
                if reset.gt(&range_start) {
                    let e = DayEvent {
                        holiday_id: r.map as u32,
                        kind: kind::RAID_RESET,
                        start: reset.encode(),
                        ..DayEvent::default()
                    };
                    self.insert_event(e, TABLE_RESETS);
                }
                let before = reset;
                reset.add_holiday_duration(r.interval);
                if reset == before {
                    break;
                }
            }
        }
        self.update_display_list(ctx, signal);
    }

    /// `InsertLockoutEvent` (`0x5c2350`): the map, difficulty and instance ride in the holiday,
    /// end and event-id fields.
    fn insert_lockout(&mut self, map: i32, difficulty: i32, instance_id: u64, time: u32) {
        let e = DayEvent {
            holiday_id: map as u32,
            end: difficulty as u32,
            event_id: instance_id,
            start: time,
            kind: kind::RAID_LOCKOUT,
            ..DayEvent::default()
        };
        self.insert_event(e, TABLE_LOCKOUTS);
    }

    /// Drop the matching lockouts of one day's lockout list.
    fn drop_lockouts(&mut self, key: Option<u32>, mut hit: impl FnMut(&DayEvent) -> bool) {
        let keys: Vec<u32> = match key {
            Some(k) => vec![k],
            None => self.tables[TABLE_LOCKOUTS].keys().copied().collect(),
        };
        for k in keys {
            let Some(list) = self.tables[TABLE_LOCKOUTS].get(&k).cloned() else {
                continue;
            };
            let (gone, kept): (Vec<usize>, Vec<usize>) = list.into_iter().partition(|&id| {
                self.event(id).kind & kind::RAID_LOCKOUT != 0 && hit(self.event(id))
            });
            for id in gone {
                self.release(id);
            }
            self.tables[TABLE_LOCKOUTS].insert(k, kept);
        }
    }

    // ── Server messages ─────────────────────────────────────────────────────────────────────

    /// One message, as `PH_CALENDAR_GROUP` (`0x5c3fe0`) applies it: each arm, then the action
    /// is no longer pending unless the arm's clear byte said to keep it. The command result is
    /// its own handler (`0x6cd380`) and clears nothing on success.
    pub(crate) fn apply(&mut self, ctx: &Ctx, wire: CalendarWire) {
        use CalendarWire as W;
        let mut clear = true;
        match wire {
            W::SendCalendar(s) => self.send_calendar(ctx, *s),
            W::SendEvent(e) => self.get_event(ctx, *e),
            W::InitialInvites(list) => {
                let player = self.player.clone();
                let names = &self.names;
                if let Some(cur) = self.current.as_mut() {
                    if let Some(signal) = cur.initial_invites(list, player.as_ref(), names) {
                        self.signals.push(signal);
                    }
                }
            }
            W::Invite(i) => {
                clear = i.clear_pending;
                self.invite(i);
            }
            W::InviteRemoved {
                guid,
                event_id,
                flags,
                clear_pending,
            } => {
                clear = clear_pending;
                self.invite_removed(guid, event_id, flags);
            }
            W::Status(s) => {
                clear = s.clear_pending;
                self.invite_status(s);
            }
            W::CommandResult(r) => {
                self.command_result(r);
                return;
            }
            W::LockoutAdded { date, lockout } => {
                let mut at = WowTime::decode(date);
                at.add_minutes(lockout.reset / 60);
                self.insert_lockout(
                    lockout.map,
                    lockout.difficulty,
                    lockout.instance_id,
                    at.encode(),
                );
                self.update_display_list(ctx, true);
            }
            W::LockoutRemoved { map, lockout } => {
                match lockout {
                    None => self.drop_lockouts(None, |_| true),
                    Some(l) => {
                        let mut reset = self.now;
                        reset.add_minutes(l.reset / 60);
                        let key = WowTime::date(reset.month, reset.month_day, reset.year).encode();
                        self.drop_lockouts(Some(key), |e| {
                            e.holiday_id == map as u32
                                && (l.difficulty == -1 || e.end == l.difficulty as u32)
                                && (l.instance_id == u64::MAX || e.event_id == l.instance_id)
                        });
                    }
                }
                self.update_display_list(ctx, true);
            }
            W::LockoutUpdated {
                date,
                map,
                difficulty,
                old_reset,
                new_reset,
            } => self.lockout_updated(ctx, date, map, difficulty, old_reset, new_reset),
            W::InviteAlert(a) => {
                self.invite_alert(ctx, a);
                return;
            }
            W::InviteRemovedAlert {
                event_id,
                date,
                flags,
                status,
            } => {
                self.invite_removed_alert(ctx, event_id, date, flags, status);
                return;
            }
            W::InviteStatusAlert {
                event_id,
                date,
                flags,
                status,
            } => {
                self.invite_status_alert(event_id, date, flags, status);
                return;
            }
            W::EventRemovedAlert {
                clear_pending,
                event_id,
                date,
            } => {
                clear = clear_pending;
                self.event_removed_alert(ctx, event_id, date);
            }
            W::EventUpdatedAlert(u) => {
                clear = u.clear_pending;
                self.event_updated_alert(ctx, *u);
            }
            W::ModeratorStatusAlert {
                guid,
                event_id,
                status,
                clear_pending,
            } => {
                clear = clear_pending;
                let me = self.player_guid();
                if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == event_id) {
                    if cur.update_moderator(guid, status, me) {
                        self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
                    }
                }
                if me != 0 && me == guid {
                    self.set_invite_flag(event_id, mark::MODERATOR, status == 1);
                    self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
                }
            }
            W::NumPending(n) => {
                let was = self.num_initial_pending;
                self.num_initial_pending = n as i32;
                if self.num_initial_pending != was {
                    self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
                }
            }
            W::InviteNotes {
                guid,
                event_id,
                notes,
                clear_pending,
            } => {
                clear = clear_pending;
                if self.player.is_some() {
                    self.update_notes(guid, event_id, notes);
                }
            }
            W::InviteNotesAlert { event_id, notes } => {
                let me = self.player_guid();
                if me != 0 {
                    self.update_notes(me, event_id, notes);
                }
                return;
            }
            W::ClearPendingAction => {}
        }
        if clear {
            self.action_pending = false;
            self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(false)]);
        }
    }

    fn update_notes(&mut self, guid: u64, event_id: u64, notes: String) {
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == event_id) {
            if cur.update_notes(guid, notes) {
                self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
            }
        }
    }

    /// `PH_SMSG_CALENDAR_SEND_CALENDAR` (`0x5c29c0`).
    fn send_calendar(&mut self, ctx: &Ctx, s: super::CalendarSnapshotView) {
        self.num_initial_pending = 0;
        self.pending = s
            .invites
            .iter()
            .map(|i| PendingInvite {
                event_id: i.event_id,
                invite_id: i.invite_id,
                inviter: i.inviter,
                status: i.status,
                rank: i.rank,
                invite_type: i.invite_type,
                valid: false,
            })
            .collect();
        let mut num_invites = s.invites.len() as i64;
        self.clear_table(0);
        self.clear_table(TABLE_GUILD);
        let me = self.player_guid();
        let now = self.now;
        for row in &s.events {
            let mut e = DayEvent {
                event_id: row.event_id,
                title: row.title.clone(),
                event_type: row.event_type,
                start: row.start,
                kind: row.flags,
                texture: row.texture,
                flags: if me != 0 && row.creator == me {
                    mark::CREATOR
                } else {
                    0
                },
                ..DayEvent::default()
            };
            if row.flags & kind::ANNOUNCEMENT != 0 {
                e.creator = row.creator;
                e.schedule_alarm();
                self.insert_event(e, Self::table_of(row.flags));
                continue;
            }
            match self.pending.iter().position(|p| p.event_id == row.event_id) {
                Some(p) => {
                    let invite = self.pending[p];
                    e.invite_status = invite.status;
                    e.invite_id = invite.invite_id;
                    if invite.rank != 0 {
                        e.flags |= mark::MODERATOR;
                        if invite.rank == 2 {
                            e.flags |= mark::CREATOR;
                        }
                    }
                    if invite.invite_type == 1 {
                        e.flags |= mark::SIGN_UP;
                        e.creator = row.creator;
                    } else {
                        e.creator = invite.inviter;
                    }
                    let start = WowTime::decode(row.start);
                    if invite.status == 0 && start.gt(&now) && row.flags & kind::LOCKED == 0 {
                        e.flags |= mark::PENDING;
                        self.pending[p].valid = true;
                    } else {
                        self.pending.remove(p);
                        num_invites -= 1;
                    }
                }
                None => {
                    if row.flags & kind::GUILD_EVENT == 0 {
                        continue;
                    }
                    e.flags |= mark::SIGN_UP;
                    e.creator = row.creator;
                    e.invite_status = 7;
                }
            }
            e.schedule_alarm();
            let creator = e.creator;
            self.insert_event(e, Self::table_of(row.flags));
            if creator != 0 && !self.names.contains_key(&creator) {
                self.prefetch_names.insert(creator);
            }
        }
        let ignored = &self.ignored;
        self.pending.retain(|p| {
            let keep = p.valid && p.inviter != 0;
            if !keep || ignored.contains(&p.inviter) {
                num_invites -= 1;
            }
            keep
        });

        let server_date = WowTime::decode(s.server_date);
        self.clear_table(TABLE_LOCKOUTS);
        for l in &s.lockouts {
            let mut reset = server_date;
            reset.add_minutes(l.reset / 60);
            self.insert_lockout(l.map, l.difficulty, l.instance_id, reset.encode());
        }
        self.resets = s
            .resets
            .iter()
            .filter(|r| r.period != 0)
            .map(|r| {
                let mut reset = server_date;
                let start = s.reset_base.wrapping_add(r.offset);
                let periods = (s.server_now.wrapping_sub(start) as f32 / r.period as f32) as u32;
                let at = start
                    .wrapping_add(periods.wrapping_mul(r.period))
                    .wrapping_sub(s.server_now) as i32;
                reset.add_minutes(at / 60);
                ResetNode {
                    map: r.map,
                    interval: (r.period / 60) as i32,
                    start: reset.encode(),
                }
            })
            .collect();
        if !s.holidays.is_empty() {
            self.dynamic.clear();
            self.holidays.clear();
            for h in s.holidays {
                self.dynamic.insert(
                    h.id,
                    DynamicHoliday {
                        region: h.region,
                        looping: h.looping,
                        priority: h.priority,
                        filter_type: h.filter_type,
                        dates: h.dates,
                        durations: h.durations,
                        flags: h.flags,
                        texture: h.texture,
                    },
                );
            }
        }
        self.calculate_events(ctx, true);
        if num_invites != 0 {
            self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
        }
        self.received = true;
    }

    /// `ProcessGetEvent` (`0x5bed20`): the event opens; a first look at an invite still unanswered
    /// marks it pending.
    fn get_event(&mut self, _ctx: &Ctx, e: super::CalendarEventView) {
        if self.player.is_none() {
            return;
        }
        let send_type = e.send_type;
        let me = self.player_guid();
        let (cur, status) = CurrentEvent::from_wire(e, me, &self.names);
        self.current = Some(cur);
        let cur = self.current.as_ref().expect("just set");
        let event_id = cur.event_id;
        if status == 0 && cur.date.gt(&self.now) && !self.has_pending_invite(event_id) {
            let inviter = self.current_day_event().map(|id| {
                let e = self.event_mut(id);
                e.flags |= mark::PENDING;
                e.creator
            });
            self.pending.push(PendingInvite {
                event_id,
                inviter: inviter.unwrap_or(0),
                ..PendingInvite::default()
            });
            self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
        }
        let cur = self.current.as_mut().expect("just set");
        if send_type == 1 || send_type == 2 {
            if cur.names_pending() {
                cur.signal_type = if send_type == 1 { 2 } else { 3 };
            } else {
                self.signal(
                    "CALENDAR_NEW_EVENT",
                    vec![ScriptValue::Bool(send_type == 2)],
                );
            }
            return;
        }
        self.selected = Selection {
            event_id,
            start: cur.date.encode(),
            ..Selection::default()
        };
        if cur.names_pending() {
            cur.signal_type = 4;
        } else {
            let t = calendar_type(cur.flags);
            self.signal("CALENDAR_OPEN_EVENT", vec![ScriptValue::Str(t.into())]);
        }
    }

    /// `ProcessInvite` (`0x5bf030`).
    fn invite(&mut self, i: super::CalendarInviteAddedView) {
        if i.invite_type == 1 {
            let target = match self.current.as_ref().filter(|c| c.event_id == i.event_id) {
                Some(_) => self.current_day_event(),
                None => self.find_in_table(TABLE_GUILD, i.event_id),
            };
            if let Some(id) = target {
                let e = self.event_mut(id);
                e.invite_id = i.invite_id;
                e.invite_status = i.status;
                self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
                if self
                    .current
                    .as_ref()
                    .is_none_or(|c| c.event_id != i.event_id)
                {
                    return;
                }
            }
        }
        let me = self.player_guid();
        let names = &self.names;
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == i.event_id) {
            let invite = Invite {
                invite_id: i.invite_id,
                guid: i.guid,
                level: i.level,
                status: i.status,
                moderator: u8::from(i.guid == me && i.invite_type != 1) * 2,
                invite_type: i.invite_type,
                response_time: i.response_time,
                notes: String::new(),
            };
            if let Some(signal) = cur.add_invite(invite, names) {
                self.signals.push(signal);
            }
        }
    }

    /// `ProcessInviteRemoved` (`0x5bf250`).
    fn invite_removed(&mut self, guid: u64, event_id: u64, flags: u32) {
        if flags & kind::GUILD_EVENT != 0 {
            let target = match self.current.as_ref().filter(|c| c.event_id == event_id) {
                Some(_) => self.current_day_event(),
                None => self.find_in_table(TABLE_GUILD, event_id),
            };
            if let Some(id) = target {
                let e = self.event_mut(id);
                e.invite_id = 0;
                e.invite_status = 7;
                self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
                if self.current.as_ref().is_none_or(|c| c.event_id != event_id) {
                    return;
                }
            }
        }
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == event_id) {
            cur.remove_invite_guid(guid);
            self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
        }
    }

    /// `ProcessInviteStatus` (`0x5bf420`).
    fn invite_status(&mut self, s: super::CalendarStatusView) {
        let me = self.player_guid();
        if me == 0 {
            return;
        }
        let Some(id) = self.table_event(Self::table_of(s.flags), date_key(s.date), s.event_id)
        else {
            return;
        };
        if s.guid == me {
            self.event_mut(id).invite_status = s.status;
            if s.status != 0 {
                if self.delete_pending_invite(s.event_id) {
                    self.event_mut(id).flags &= !mark::PENDING;
                    self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
                }
            } else {
                self.add_pending_invite(id);
            }
            self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
        }
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == s.event_id) {
            if cur.update_status(s.guid, s.status, s.response_time) {
                self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
            }
        }
    }

    /// `ProcessInviteStatusAlert` (`0x5bf590`).
    fn invite_status_alert(&mut self, event_id: u64, date: u32, flags: u32, status: u8) {
        let me = self.player_guid();
        if me == 0 {
            return;
        }
        let Some(id) = self.table_event(Self::table_of(flags), date_key(date), event_id) else {
            if !self.received {
                self.send(CalendarRequest::GetNumPending);
            }
            return;
        };
        self.event_mut(id).invite_status = status;
        if self.event(id).kind & kind::LOCKED == 0 {
            if status != 0 {
                if self.delete_pending_invite(event_id) {
                    self.event_mut(id).flags &= !mark::PENDING;
                    self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
                }
            } else {
                self.add_pending_invite(id);
            }
        }
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == event_id) {
            if cur.update_status(me, status, 0) {
                self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
            }
        }
        self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
    }

    /// `SetInviteFlag` (`0x5bf6f0`).
    fn set_invite_flag(&mut self, event_id: u64, flag: u32, set: bool) {
        let id = match self.current.as_ref().filter(|c| c.event_id == event_id) {
            Some(_) => self.current_day_event(),
            None => self.find_in_table(0, event_id),
        };
        if let Some(id) = id {
            let e = self.event_mut(id);
            if set {
                e.flags |= flag;
            } else {
                e.flags &= !flag;
            }
        }
    }

    /// `ProcessLockoutUpdated` (`0x5c3880`): an extension moves the lockout to its new date.
    fn lockout_updated(
        &mut self,
        ctx: &Ctx,
        date: u32,
        map: i32,
        difficulty: i32,
        old_reset: i32,
        new_reset: i32,
    ) {
        let mut old_date = WowTime::decode(date);
        let mut new_date = old_date;
        old_date.add_minutes(old_reset / 60);
        new_date.add_minutes(new_reset / 60);
        let key = WowTime::date(old_date.month, old_date.month_day, old_date.year).encode();
        let new_key = WowTime::date(new_date.month, new_date.month_day, new_date.year).encode();
        if let Some(list) = self.tables[TABLE_LOCKOUTS].get(&key).cloned() {
            let mut kept = Vec::new();
            for id in list {
                let e = self.event(id).clone();
                let moves = e.kind & kind::RAID_LOCKOUT != 0
                    && e.holiday_id == map as u32
                    && e.end == difficulty as u32
                    && new_key != key;
                if moves {
                    self.release(id);
                    self.insert_event(
                        DayEvent {
                            start: new_date.encode(),
                            ..e
                        },
                        TABLE_LOCKOUTS,
                    );
                } else {
                    kept.push(id);
                }
            }
            self.tables[TABLE_LOCKOUTS].insert(key, kept);
        }
        self.update_display_list(ctx, true);
    }

    /// `ProcessInviteAlert` (`0x5c3ab0`): an invite lands on the calendar; an unanswered one
    /// that is not an announcement is pending.
    fn invite_alert(&mut self, ctx: &Ctx, a: super::CalendarInviteAlertView) {
        let me = self.player_guid();
        if me == 0 {
            return;
        }
        let mut e = DayEvent {
            event_id: a.event_id,
            title: a.title,
            start: a.start,
            kind: a.flags,
            event_type: a.event_type,
            texture: a.texture,
            invite_id: a.invite_id,
            invite_status: a.status,
            ..DayEvent::default()
        };
        e.schedule_alarm();
        let mut pending = false;
        if matches!(e.invite_status, 6 | 7) {
            e.creator = a.creator;
            e.flags = mark::SIGN_UP;
        } else {
            e.creator = a.inviter;
            if e.invite_status == 0 && e.kind & kind::ANNOUNCEMENT == 0 {
                pending = true;
                e.flags = mark::PENDING;
            }
        }
        if a.rank != 0 {
            e.flags |= mark::MODERATOR;
        }
        if a.creator == me || a.rank == 2 {
            e.flags |= mark::CREATOR;
        }
        let creator = e.creator;
        let event_id = e.event_id;
        let table = Self::table_of(e.kind);
        self.insert_event(e, table);
        if pending {
            self.pending.push(PendingInvite {
                event_id,
                inviter: creator,
                ..PendingInvite::default()
            });
            if !self.is_ignored(creator) {
                self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
            }
        }
        self.update_display_list(ctx, true);
    }

    fn clear_selections_of(&mut self, event_id: u64) {
        if self.selected.event_id == event_id {
            self.selected = Selection::default();
        }
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.event_id == event_id)
        {
            self.current = None;
            self.signal("CALENDAR_CLOSE_EVENT", vec![]);
        }
        if self.context.event_id == event_id {
            self.deselect_context();
        }
    }

    /// `ProcessInviteRemovedAlert` (`0x5c0ae0`): a guild sign-up reverts to "not signed up";
    /// anything else leaves the calendar. A removal with a status mails a notice.
    fn invite_removed_alert(
        &mut self,
        ctx: &Ctx,
        event_id: u64,
        date: u32,
        flags: u32,
        status: u8,
    ) {
        let key = date_key(date);
        let table = Self::table_of(flags);
        let found = self.table_event(table, key, event_id);
        let mut remove = true;
        match found {
            Some(id)
                if self.event(id).kind & kind::GUILD_EVENT != 0
                    && self.event(id).flags & mark::SIGN_UP != 0 =>
            {
                self.event_mut(id).invite_status = 7;
                let me = self.player_guid();
                if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == event_id) {
                    if me != 0 {
                        cur.remove_invite_guid(me);
                        self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
                    }
                }
                self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
                remove = false;
            }
            _ => self.clear_selections_of(event_id),
        }
        if status != 0 && status != 9 {
            self.send(CalendarRequest::ReceivedMail);
        }
        self.delete_pending_invite(event_id);
        self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
        if remove && self.table_delete(table, key, event_id) {
            self.update_display_list(ctx, true);
        }
    }

    /// `ProcessEventRemovedAlert` (`0x5c0cf0`).
    fn event_removed_alert(&mut self, ctx: &Ctx, event_id: u64, date: u32) {
        self.clear_selections_of(event_id);
        if self.clipboard.event_id == event_id {
            self.clipboard = DayEvent {
                event_type: 0,
                ..DayEvent::default()
            };
        }
        let key = date_key(date);
        if !self.table_delete(0, key, event_id) && !self.table_delete(TABLE_GUILD, key, event_id) {
            return;
        }
        self.delete_pending_invite(event_id);
        self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
        self.update_display_list(ctx, true);
    }

    /// `ProcessEventUpdatedAlert` (`0x5c3d30`): the event's day copy moves and takes the new
    /// fields; the open event re-reads the whole tail.
    fn event_updated_alert(&mut self, ctx: &Ctx, u: super::CalendarEventUpdateView) {
        let table = Self::table_of(u.flags);
        let key = date_key(u.old_date);
        match self.table_event(table, key, u.event_id) {
            Some(mut id) => {
                let locked = self.event(id).kind & kind::LOCKED != 0;
                if u.old_date != u.date {
                    let old = self.event(id).clone();
                    let mut moved = DayEvent {
                        event_id: u.event_id,
                        start: u.date,
                        flags: old.flags,
                        invite_status: old.invite_status,
                        invite_id: old.invite_id,
                        creator: old.creator,
                        sequence_index: old.sequence_index,
                        sequence_length: old.sequence_length,
                        ..DayEvent::default()
                    };
                    moved.schedule_alarm();
                    let new_id = self.insert_event(moved, table);
                    let was_context = self.context_event == Some(id);
                    self.table_delete(table, key, u.event_id);
                    if was_context {
                        self.context_event = Some(new_id);
                    }
                    id = new_id;
                }
                let e = self.event_mut(id);
                e.title = u.title.chars().take(127).collect();
                e.kind = u.flags;
                e.event_type = i32::from(u.event_type);
                e.texture = u.texture;
                let status = e.invite_status;
                if locked != (u.flags & kind::LOCKED != 0) && status == 0 {
                    if u.flags & kind::LOCKED != 0 {
                        if self.delete_pending_invite(u.event_id) {
                            self.event_mut(id).flags &= !mark::PENDING;
                            self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
                        }
                    } else {
                        self.add_pending_invite(id);
                    }
                }
                self.update_display_list(ctx, true);
            }
            None => {
                if !self.received {
                    self.send(CalendarRequest::GetNumPending);
                }
            }
        }
        if let Some(cur) = self.current.as_mut().filter(|c| c.event_id == u.event_id) {
            cur.update_from_alert(&u);
            self.signal("CALENDAR_UPDATE_EVENT", vec![]);
        }
    }

    /// `Packet_SMSG_CALENDAR_COMMAND_RESULT` (`0x6cd380`): result 0 is silent; each other shows
    /// its line, several with the count or the name it names.
    fn command_result(&mut self, r: super::CalendarCommandResultView) {
        let name = || Some(ErrorArg::Str(r.name.clone()));
        let int = |n: i64| Some(ErrorArg::Int(n));
        let (key, arg) = match r.result {
            0 => return,
            1 => ("CALENDAR_ERROR_GUILD_EVENTS_EXCEEDED", int(100)),
            2 => ("CALENDAR_ERROR_EVENTS_EXCEEDED", int(30)),
            3 => ("CALENDAR_ERROR_SELF_INVITES_EXCEEDED", None),
            4 => ("CALENDAR_ERROR_OTHER_INVITES_EXCEEDED", name()),
            5 => ("CALENDAR_ERROR_PERMISSIONS", None),
            6 => ("CALENDAR_ERROR_EVENT_INVALID", None),
            7 => ("CALENDAR_ERROR_NOT_INVITED", None),
            8 => ("CALENDAR_ERROR_INTERNAL", None),
            9 => ("ERR_GUILD_PLAYER_NOT_IN_GUILD", None),
            10 => ("CALENDAR_ERROR_ALREADY_INVITED_TO_EVENT_S", name()),
            11 => ("PLAYER_NOT_FOUND", None),
            12 => ("CALENDAR_ERROR_NOT_ALLIED", None),
            13 => ("ERR_IGNORING_YOU_S", name()),
            14 => ("CALENDAR_ERROR_INVITES_EXCEEDED", int(100)),
            16 => ("CALENDAR_ERROR_INVALID_DATE", None),
            17 => ("CALENDAR_ERROR_INVALID_TIME", None),
            19 => ("CALENDAR_ERROR_NEEDS_TITLE", None),
            20 => ("CALENDAR_ERROR_EVENT_PASSED", None),
            21 => ("CALENDAR_ERROR_EVENT_LOCKED", None),
            22 => ("CALENDAR_ERROR_DELETE_CREATOR_FAILED", None),
            24 => ("ERR_SYSTEM_DISABLED", None),
            25 => ("ERR_RESTRICTED_ACCOUNT", None),
            26 => ("CALENDAR_ERROR_ARENA_EVENTS_EXCEEDED", None),
            27 => ("CALENDAR_ERROR_RESTRICTED_LEVEL", None),
            28 => ("ERR_USER_SQUELCHED", None),
            29 => ("CALENDAR_ERROR_NO_INVITE", None),
            36 => ("CALENDAR_ERROR_EVENT_WRONG_SERVER", None),
            37 => ("CALENDAR_ERROR_INVITE_WRONG_SERVER", None),
            38 => ("CALENDAR_ERROR_NO_GUILD_INVITES", None),
            39 => ("CALENDAR_ERROR_INVALID_SIGNUP", None),
            40 => ("CALENDAR_ERROR_NO_MODERATOR", None),
            _ => return,
        };
        self.error(key, arg);
    }

    /// `CGCalendar::GuildChanged` (`0x5c0f40`): joining refetches; leaving drops the guild
    /// table and a guild event on the clipboard.
    pub(crate) fn guild_changed(&mut self, ctx: &Ctx, in_guild: bool) {
        if self.player.is_none() {
            return;
        }
        if in_guild {
            self.get_calendar();
            return;
        }
        self.clear_table(TABLE_GUILD);
        if self.clipboard.kind & kind::GUILD != 0 {
            self.clipboard = DayEvent {
                event_type: 0,
                ..DayEvent::default()
            };
        }
        self.update_display_list(ctx, true);
    }

    /// `CGCalendar::InvalidatePlayer` (`0x5bde60`): a renamed or deleted creator refetches.
    pub(crate) fn invalidate_player(&mut self, guid: u64) {
        if self.player.is_none() {
            return;
        }
        let hit = self.tables[0]
            .values()
            .flatten()
            .any(|&id| self.event(id).creator == guid);
        if hit {
            self.get_calendar();
        }
    }

    /// The ignore list moved (`FriendList::IgnoreAdded` `0x6b7200`, `IgnoreRemoved` `0x6b6490`).
    pub(crate) fn ignores_changed(&mut self, ctx: &Ctx) {
        self.update_display_list(ctx, true);
        self.signal("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
    }

    // ── Verbs' halves ───────────────────────────────────────────────────────────────────────

    /// `CGCalendar::GetCalendar` (`0x5bbb40`).
    pub(crate) fn get_calendar(&mut self) {
        self.send(CalendarRequest::GetCalendar);
        self.opened = true;
    }

    pub(crate) fn opened(&self) -> bool {
        self.opened
    }

    /// `CGCalendar::GetEvent` (`0x5b9310`): a fetched kind asks the server; any other opens
    /// at once.
    pub(crate) fn open_event(&mut self, offset: i32, day: u32, index: u32) {
        if self.action_pending
            || !(-1..=1).contains(&offset)
            || day >= DAYS as u32
            || index as usize >= self.days[(offset + 1) as usize][day as usize].len()
        {
            return;
        }
        let Some(id) = self.day_event_id(offset, day, index) else {
            return;
        };
        let e = self.event(id).clone();
        if e.kind & kind::FETCHED != 0 {
            self.send(CalendarRequest::GetEvent {
                event_id: e.event_id,
            });
            self.set_action_pending(true);
            return;
        }
        self.selected = Selection::of(&e);
        self.signal(
            "CALENDAR_OPEN_EVENT",
            vec![ScriptValue::Str(calendar_type(e.kind).into())],
        );
    }

    /// The editing right on the open event (`CanEditCurrentEvent` `0x5b7d20`): an announcement
    /// needs the guild right, anything else a moderator.
    pub(crate) fn can_edit_current(&self, ctx: &Ctx) -> bool {
        let me = self.player_guid();
        self.current.as_ref().is_some_and(|c| {
            if c.flags & kind::ANNOUNCEMENT != 0 {
                ctx.can_edit_guild_event
            } else {
                c.is_moderator(me)
            }
        })
    }

    /// The editing right on a day event (`0x5baf61`): a guild kind needs the guild right, any
    /// other the creator or a moderator mark.
    pub(crate) fn can_edit_day_event(&self, ctx: &Ctx, e: &DayEvent) -> bool {
        if e.kind & kind::GUILD != 0 {
            ctx.can_edit_guild_event
        } else {
            e.flags & (mark::MODERATOR | mark::CREATOR) != 0
        }
    }

    /// A new event of the player's (`CreatePlayerEvent` `0x5bbe50`), announcement (`0x5bbf20`)
    /// or guild event (`0x5bc030`).
    pub(crate) fn new_event(&mut self, calendar_type: u32) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if calendar_type != 0 && player.guild_id == 0 {
            self.error("ERR_GUILD_PLAYER_NOT_IN_GUILD", None);
            return;
        }
        let (guid, level) = match calendar_type {
            1 => (0, 0),
            _ => (player.guid, player.level),
        };
        self.current = Some(CurrentEvent::new_local(
            player.guid,
            guid,
            level,
            calendar_type,
        ));
        self.selected = Selection::default();
        if calendar_type == 0 {
            self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![ScriptValue::Bool(true)]);
        }
    }

    fn throttled(last: Option<f64>, now: f64, window: f64) -> bool {
        last.is_some_and(|t| now - t < window)
    }

    /// `CalendarCanAddEvent` (`0x5b8ba0`): nothing pending and five seconds since the last add.
    pub(crate) fn can_add_event(&self, ctx: &Ctx) -> bool {
        !self.action_pending && !Self::throttled(self.last_event, ctx.now_secs, 5.0)
    }

    /// `CalendarCanSendInvite` (`0x5b8b30`): two seconds between invites.
    pub(crate) fn can_send_invite(&self, ctx: &Ctx) -> bool {
        !self.action_pending && !Self::throttled(self.last_invite, ctx.now_secs, 2.0)
    }

    /// `CGCalendar::AddEvent` (`0x5bbd60`).
    pub(crate) fn add_event(&mut self, ctx: &Ctx) {
        if self.action_pending {
            return;
        }
        let Some(cur) = self.current.as_ref() else {
            self.error("CALENDAR_ERROR_INTERNAL", None);
            return;
        };
        if Self::throttled(self.last_event, ctx.now_secs, 5.0) {
            self.error("CALENDAR_ERROR_EVENT_THROTTLED", None);
            return;
        }
        if !validate_create_date(&cur.date, &self.now) {
            return;
        }
        let guild_id = self.player.as_ref().map_or(0, |p| p.guild_id);
        match cur.add_request(&self.now, guild_id) {
            Ok(request) => {
                self.send(request);
                self.last_event = Some(ctx.now_secs);
                self.set_action_pending(true);
            }
            Err(Some((key, arg))) => self.error(key, arg),
            Err(None) => {}
        }
    }

    /// `CGCalendar::UpdateEvent` (`0x5bc310`).
    pub(crate) fn update_event(&mut self, ctx: &Ctx) {
        if self.action_pending || self.current.as_ref().is_none_or(|c| c.new) {
            return;
        }
        if !self.can_edit_current(ctx) {
            self.error("CALENDAR_ERROR_PERMISSIONS", None);
            return;
        }
        let now = self.now;
        let cur = self.current.as_mut().expect("checked");
        match cur.update_request(&now) {
            Ok(Some(request)) => {
                self.send(request);
                self.set_action_pending(true);
            }
            Ok(None) => {}
            Err(key) => self.error(key, None),
        }
    }

    /// `CGCalendar::RemoveEvent` (`0x5b96a0`): an editor removes the event, anyone else their
    /// own invite.
    pub(crate) fn remove_event(&mut self, ctx: &Ctx) {
        if self.action_pending || self.current.as_ref().is_none_or(|c| c.new) {
            return;
        }
        let me = self.player_guid();
        let result = if self.can_edit_current(ctx) {
            self.current.as_ref().expect("checked").remove_request(me)
        } else {
            match self.current.as_mut().expect("checked").remove_invite_of(me) {
                Some(r) => r,
                None => return,
            }
        };
        match result {
            Ok(request) => {
                if let Some(request) = request {
                    self.send(request);
                }
                self.set_action_pending(true);
            }
            Err(key) => self.error(key, None),
        }
    }

    /// `CGCalendar::RemoveInvite` (`0x5b9780`).
    pub(crate) fn remove_invite(&mut self, index: u32) {
        if self.action_pending {
            return;
        }
        let me = self.player_guid();
        let Some(cur) = self.current.as_mut() else {
            return;
        };
        let new = cur.new;
        match cur.remove_invite_at(index, me) {
            None => {}
            Some(Err(key)) => self.error(key, None),
            Some(Ok(request)) => {
                if new {
                    self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]);
                } else {
                    if let Some(request) = request {
                        self.send(request);
                    }
                    self.set_action_pending(true);
                }
            }
        }
    }

    /// `CGCalendar::Invite` (`0x5bc360`).
    pub(crate) fn invite_name(&mut self, ctx: &Ctx, name: &str) {
        let me = self.player_guid();
        if self.action_pending || self.current.as_ref().is_none_or(|c| !c.is_moderator(me)) {
            return;
        }
        if Self::throttled(self.last_invite, ctx.now_secs, 2.0) {
            self.error("CALENDAR_ERROR_INVITE_THROTTLED", None);
            return;
        }
        let names = &self.names;
        let result = self
            .current
            .as_ref()
            .expect("checked")
            .invite_request(name, me, names);
        match result {
            Ok(request) => {
                self.send(request);
                self.last_invite = Some(ctx.now_secs);
                self.set_action_pending(true);
            }
            Err((key, arg)) => self.error(key, arg),
        }
    }

    /// `CGCalendar::InviteRSVP` (`0x5bc3e0`): a member's tentative on their own guild's event is a
    /// tentative sign-up.
    pub(crate) fn invite_rsvp(&mut self, status: u32) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if self.action_pending {
            return;
        }
        let Some(cur) = self.current.as_ref() else {
            return;
        };
        let request = if cur.flags & kind::GUILD_EVENT != 0 && cur.guild_id == player.guild_id {
            if status != 8 {
                return;
            }
            cur.sign_up_request(true, player.guid)
        } else {
            cur.rsvp_request(status, player.guid)
        };
        if let Some(request) = request {
            self.send(request);
            self.set_action_pending(true);
        }
    }

    /// `CalendarEventAvailable` and `CalendarEventDecline` (`0x5bd6e0`, `0x5bd760`): not on the
    /// player's own guild's event.
    pub(crate) fn answer_own_invite(&mut self, status: u32) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if self.action_pending {
            return;
        }
        let Some(cur) = self.current.as_ref() else {
            return;
        };
        if cur.flags & kind::GUILD_EVENT != 0 && cur.guild_id == player.guild_id {
            return;
        }
        if let Some(request) = cur.rsvp_request(status, player.guid) {
            self.send(request);
            self.set_action_pending(true);
        }
    }

    /// `CGCalendar::EventSignUp` (`0x5bc460`).
    pub(crate) fn event_sign_up(&mut self) {
        if self.action_pending {
            return;
        }
        let me = self.player_guid();
        let Some(cur) = self
            .current
            .as_ref()
            .filter(|c| c.flags & kind::GUILD_EVENT != 0)
        else {
            return;
        };
        if me != 0 && cur.invite_of(me).is_some() {
            return;
        }
        if let Some(request) = cur.sign_up_request(false, me) {
            self.send(request);
            self.set_action_pending(true);
        }
    }

    /// `CGCalendar::GuildFilter` (`0x5bc160`): a mass invite only for a new player event.
    pub(crate) fn guild_filter(&mut self, min_level: u32, max_level: u32, min_rank: u32) {
        if self.action_pending {
            return;
        }
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if player.guild_id == 0 {
            self.error("ERR_GUILD_PLAYER_NOT_IN_GUILD", None);
            return;
        }
        if self
            .current
            .as_ref()
            .is_none_or(|c| !c.new || c.flags & kind::GUILD != 0)
        {
            return;
        }
        self.send(CalendarRequest::GuildFilter {
            min_level,
            max_level,
            min_rank,
        });
        self.set_action_pending(true);
    }

    /// `CGCalendar::ArenaTeam` (`0x5bc240`).
    pub(crate) fn arena_team(&mut self, index: u32) {
        if self.action_pending || index >= 3 {
            return;
        }
        let Some(team_id) = self
            .player
            .as_ref()
            .map(|p| p.arena_teams[index as usize])
            .filter(|&t| t != 0)
        else {
            return;
        };
        if self
            .current
            .as_ref()
            .is_none_or(|c| !c.new || c.flags & kind::GUILD != 0)
        {
            return;
        }
        self.send(CalendarRequest::ArenaTeam { team_id });
        self.set_action_pending(true);
    }

    /// `CGCalendar::SetEventTextureID` (`0x5b9630`): an index into the open event's type's list,
    /// a dungeon's or a raid's.
    pub(crate) fn set_texture_index(&mut self, ctx: &Ctx, index: u32) {
        if !self.can_edit_current(ctx) {
            return;
        }
        let Some(cur) = self.current.as_mut() else {
            return;
        };
        let list = match cur.event_type {
            1 => &self.textures[0],
            0 => &self.textures[1],
            _ => return,
        };
        if let Some(&texture) = list.get(index as usize) {
            cur.set_texture(texture);
        }
    }

    /// `GetTextureIndex` (`0x5b9700`): 0-based, -1 none.
    pub(crate) fn texture_index(&self, event_type: u32, texture: i32) -> i32 {
        let list = match event_type {
            1 => &self.textures[0],
            0 => &self.textures[1],
            _ => return -1,
        };
        list.iter()
            .position(|&t| t == texture)
            .map_or(-1, |i| i as i32)
    }

    /// `CalendarEventGetTextures`' list: the 1-based event type 1 (raid) reads the raids, 2
    /// (dungeon) the dungeons (`0x5bb970`).
    pub(crate) fn textures_for(&self, lua_type: f64) -> &[i32] {
        if lua_type == 2.0 {
            &self.textures[0]
        } else if lua_type == 1.0 {
            &self.textures[1]
        } else {
            &[]
        }
    }

    pub(crate) fn has_pending(&self, event_id: u64) -> bool {
        self.has_pending_invite(event_id)
    }

    /// `ContextMenuInviteRSVP` (`0x5b9860`): never a guild sign-up.
    pub(crate) fn context_rsvp(&mut self, id: usize, status: u32) {
        let e = self.event(id).clone();
        if self.action_pending
            || (e.kind & kind::GUILD_EVENT != 0 && e.flags & mark::SIGN_UP != 0)
            || !matches!(status, 1 | 2 | 8)
        {
            return;
        }
        self.send(CalendarRequest::Rsvp {
            event_id: e.event_id,
            invite_id: e.invite_id,
            status,
        });
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// The context menu's accept and decline (`0x5bab50`, `0x5bad00`): an RSVP straight out,
    /// never on a guild sign-up, with no check of the status.
    pub(crate) fn direct_rsvp(&mut self, id: usize, status: u32) {
        let e = self.event(id).clone();
        if self.action_pending || (e.kind & kind::GUILD_EVENT != 0 && e.flags & mark::SIGN_UP != 0)
        {
            return;
        }
        self.send(CalendarRequest::Rsvp {
            event_id: e.event_id,
            invite_id: e.invite_id,
            status,
        });
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// A server override's texture (`0x5c0fdb`).
    pub(crate) fn dynamic_texture(&self, id: u32) -> Option<&str> {
        self.dynamic.get(&id).map(|d| d.texture.as_str())
    }

    /// `ContextMenuInviteRemove` (`0x5bc4d0`): the player's own invite, the moderator invite 0.
    pub(crate) fn context_remove_invite(&mut self, id: usize) {
        let me = self.player_guid();
        if self.action_pending || me == 0 {
            return;
        }
        let e = self.event(id);
        let request = CalendarRequest::RemoveInvite {
            guid: me,
            invite_id: e.invite_id,
            moderator_invite: 0,
            event_id: e.event_id,
        };
        self.send(request);
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// `ContextMenuEventSignUp` (`0x5bc5e0`): a guild event only while not signed up.
    pub(crate) fn context_sign_up(&mut self, id: usize, tentative: bool) {
        let e = self.event(id);
        if self.action_pending
            || (e.kind & kind::GUILD_EVENT != 0 && e.invite_status != 7)
            || self.player.is_none()
        {
            return;
        }
        let event_id = e.event_id;
        self.send(CalendarRequest::SignUp {
            event_id,
            tentative,
        });
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// `ContextMenuEventRemove` (`0x5bc6d0`).
    pub(crate) fn context_remove_event(&mut self, id: usize) {
        if self.action_pending || self.player.is_none() {
            return;
        }
        let e = self.event(id);
        let request = CalendarRequest::RemoveDayEvent {
            event_id: e.event_id,
            invite_id: e.invite_id,
            flags: e.kind,
        };
        self.send(request);
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// `ContextMenuEventCopy` (`0x5bc7c0`).
    pub(crate) fn context_copy(&mut self, ctx: &Ctx, id: usize) {
        let e = self.event(id).clone();
        if !self.can_edit_day_event(ctx, &e) {
            self.clipboard = DayEvent {
                event_type: 0,
                ..DayEvent::default()
            };
            self.error("CALENDAR_ERROR_PERMISSIONS", None);
            return;
        }
        self.clipboard = e;
    }

    /// `ContextMenuEventPaste` (`0x5bc870`): the clipboard's event copied to a day of a shown
    /// month, at its own time of day.
    pub(crate) fn context_paste(&mut self, ctx: &Ctx, offset: i32, day: u32) {
        if self.action_pending || self.clipboard.event_id == 0 {
            return;
        }
        if Self::throttled(self.last_event, ctx.now_secs, 5.0) {
            self.error("CALENDAR_ERROR_EVENT_THROTTLED", None);
            return;
        }
        let (month, year, days, _) = self.month(offset);
        if day >= days as u32 {
            return;
        }
        let mut date = WowTime::decode(self.clipboard.start);
        date.tz_hours = 0;
        date.month = month;
        date.month_day = day as i32;
        date.year = year;
        date.add_days(0, false);
        if !validate_create_date(&date, &self.now) {
            return;
        }
        self.send(CalendarRequest::CopyEvent {
            event_id: self.clipboard.event_id,
            invite_id: self.clipboard.invite_id,
            date: date.encode(),
        });
        self.last_event = Some(ctx.now_secs);
        self.set_action_pending(true);
    }

    /// `CanComplainAboutEvent` (`0x5bc9d0`).
    pub(crate) fn can_complain(&self, e: &DayEvent) -> bool {
        let me = self.player_guid();
        me != 0
            && self.complaints_enabled
            && e.kind & 0x6cc == 0
            && e.creator != me
            && !self.friends.contains(&e.creator)
    }

    /// `CGCalendar::Complain` (`0x5bca50`). With the complaint system off, never sent.
    pub(crate) fn complain(&mut self, id: usize) {
        let e = self.event(id).clone();
        if self.action_pending || !self.can_complain(&e) {
            return;
        }
        self.send(CalendarRequest::Complain {
            creator: e.creator,
            event_id: e.event_id,
            invite_id: e.invite_id,
        });
        self.action_pending = true;
        self.signal("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(true)]);
    }

    /// `CGCalendar::SetInviteStatus` and `SetModeratorStatus` (`0x5b7d60`, `0x5b7dc0`): on a new
    /// event, or with nothing pending.
    pub(crate) fn set_invite_field(&mut self, ctx: &Ctx, index: u32, status: u32, moderator: bool) {
        let me = self.player_guid();
        let allowed = self.current.as_ref().is_some_and(|c| {
            if c.flags & kind::ANNOUNCEMENT != 0 {
                ctx.can_edit_guild_event
            } else {
                c.is_moderator(me)
            }
        });
        if !allowed {
            return;
        }
        let pending = self.action_pending;
        let cur = self.current.as_mut().expect("checked");
        if !cur.new && pending {
            return;
        }
        let edit = if moderator {
            cur.set_moderator(index, status, me)
        } else {
            cur.set_status(index, status, me)
        };
        match edit {
            InviteEdit::Nothing => {}
            InviteEdit::Local => self.signal("CALENDAR_UPDATE_INVITE_LIST", vec![]),
            InviteEdit::Send(request) => self.send(request),
            InviteEdit::Error(key) => self.error(key, None),
        }
    }

    /// The names the app should resolve: day-event creators and the open event's invitees.
    pub(crate) fn names_wanted(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self
            .wanted_names
            .iter()
            .chain(&self.prefetch_names)
            .copied()
            .collect();
        if let Some(cur) = &self.current {
            out.extend(cur.invites.iter().map(|i| i.guid));
            out.push(cur.creator);
        }
        out.retain(|&g| g != 0);
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Names landed: an awaited creator repaints the list once (`0x5b78d0`), and the open event's
    /// last awaited invitee sorts and fires its deferred signal (`InviteNameCacheCallback`
    /// `0x5f1b10`).
    pub(crate) fn names_landed(&mut self, names: HashMap<u64, CalendarName>) {
        let arrived_wanted = self
            .wanted_names
            .iter()
            .any(|g| names.contains_key(g) && !self.names.contains_key(g));
        self.names.extend(names);
        let known = &self.names;
        self.prefetch_names.retain(|g| !known.contains_key(g));
        if arrived_wanted {
            let names = &self.names;
            self.wanted_names.retain(|g| !names.contains_key(g));
            if !self
                .signals
                .iter()
                .any(|s| *s == Signal::Event("CALENDAR_UPDATE_EVENT_LIST", vec![]))
            {
                self.signal("CALENDAR_UPDATE_EVENT_LIST", vec![]);
            }
        }
        let names = &self.names;
        let raid = &self.raid_members;
        if let Some(cur) = self.current.as_mut() {
            if let Some(signal) = cur.names_landed(names, raid) {
                self.signals.push(signal);
            }
        }
    }
}

/// `ValidateCreateDate` (`0x5b7c00`): a valid date from today to the maximum create date.
pub(crate) fn validate_create_date(date: &WowTime, now: &WowTime) -> bool {
    if !validate_date(date) {
        return false;
    }
    let mut check = *date;
    check.hour = now.hour;
    check.minute = now.minute;
    if check.lt(now) {
        return false;
    }
    let limit = max_create_date(now);
    check.hour = limit.hour;
    check.minute = limit.minute;
    !check.gt(&limit)
}
