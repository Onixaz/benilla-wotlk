//! The calendar behind the stock `Blizzard_Calendar` and the minimap clock's invite badge:
//! 12340's `CGCalendar` and `CGCalendarEvent`. The app pushes the holiday tables once
//! ([`UiScript::set_calendar_catalog`]), the player's facts, the clock and the names it is asked
//! for, and applies each server message through [`UiScript::calendar_packet`]; the verbs read the
//! state and queue the sends ([`UiScript::take_calendar_requests`]). The state outlives a
//! `ReloadUI` as the reference's does: the app moves it into the rebuilt VM
//! ([`UiScript::take_calendar`]).

mod api;
mod engine;
mod event;
mod time;

#[cfg(test)]
mod tests;

pub(super) use api::install;
pub use engine::CalendarModel;
pub(super) use event::scrub;
pub use time::WowTime;

use std::collections::{HashMap, HashSet};

use engine::{Ctx, ErrorArg, Signal, FILTERS, TABLES};

use super::{Model, ScriptValue};

/// One `Holidays.dbc` row, its name and description resolved through `HolidayNames.dbc` and
/// `HolidayDescriptions.dbc`; `None` where that row is missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarHolidayView {
    pub id: u32,
    pub durations: [i32; 10],
    pub dates: [i32; 26],
    pub region: i32,
    pub looping: i32,
    pub calendar_flags: [i32; 10],
    pub name: Option<String>,
    pub description: Option<String>,
    pub texture: String,
    pub priority: i32,
    pub filter_type: i32,
    /// `0x1`: the dates are in the server's zone (`0x5bfb4e`).
    pub flags: i32,
}

/// The tables the calendar reads, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarCatalogView {
    pub holidays: Vec<CalendarHolidayView>,
    /// `Map.dbc`'s names, a lockout's and a reset's title.
    pub map_names: HashMap<u32, String>,
}

/// The player's facts the calendar reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarPlayerView {
    pub guid: u64,
    pub level: u8,
    /// `PLAYER_GUILDID`, 0 guildless.
    pub guild_id: u32,
    /// `PLAYER_FIELD_ARENA_TEAM_INFO_1_1`'s three team ids.
    pub arena_teams: [u32; 3],
    /// `GetPVPFactionIndex` (`0x6d6e90`): the race's side, 0 Horde, 1 Alliance, -1 neither.
    pub pvp_faction: i32,
    /// The same of the player's faction template, which `LfgDungeons.dbc`'s side is matched to.
    pub faction_group: i32,
}

/// A name the app resolved for a guid, with its class's `ChrClasses.dbc` name and file token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarName {
    pub name: String,
    pub class_name: String,
    pub class_file: String,
}

/// `SMSG_CALENDAR_SEND_CALENDAR`'s pending invite.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteRefView {
    pub event_id: u64,
    pub invite_id: u64,
    pub status: u8,
    pub rank: u8,
    pub invite_type: u8,
    pub inviter: u64,
}

/// `SMSG_CALENDAR_SEND_CALENDAR`'s event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventRowView {
    pub event_id: u64,
    pub title: String,
    pub event_type: i32,
    pub start: u32,
    pub flags: u32,
    pub texture: i32,
    pub creator: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarLockoutView {
    pub map: i32,
    pub difficulty: i32,
    /// Seconds from the packet's base time.
    pub reset: i32,
    pub instance_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarResetView {
    pub map: i32,
    pub period: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarHolidayRowView {
    pub id: u32,
    pub region: i32,
    pub looping: i32,
    pub priority: i32,
    pub filter_type: i32,
    pub dates: [i32; 26],
    pub durations: [i32; 10],
    pub flags: [i32; 10],
    pub texture: String,
}

/// `SMSG_CALENDAR_SEND_CALENDAR`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarSnapshotView {
    pub invites: Vec<CalendarInviteRefView>,
    pub events: Vec<CalendarEventRowView>,
    pub server_now: u32,
    pub server_date: u32,
    pub lockouts: Vec<CalendarLockoutView>,
    pub reset_base: u32,
    pub resets: Vec<CalendarResetView>,
    pub holidays: Vec<CalendarHolidayRowView>,
}

/// `SMSG_CALENDAR_SEND_EVENT`'s invitee.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteeView {
    pub guid: u64,
    pub level: u8,
    pub status: u8,
    pub moderator: u8,
    pub invite_type: u8,
    pub invite_id: u64,
    pub response_time: u32,
    pub notes: String,
}

/// `SMSG_CALENDAR_SEND_EVENT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventView {
    pub send_type: u8,
    pub creator: u64,
    pub event_id: u64,
    pub title: String,
    pub description: String,
    pub event_type: u8,
    pub repeat: u8,
    pub max_size: u32,
    pub texture: i32,
    pub flags: u32,
    pub date: u32,
    pub lockout_date: u32,
    pub guild_id: u32,
    pub invites: Vec<CalendarInviteeView>,
}

/// `SMSG_CALENDAR_EVENT_INVITE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteAddedView {
    pub guid: u64,
    pub event_id: u64,
    pub invite_id: u64,
    pub level: u8,
    pub status: u8,
    pub invite_type: u8,
    pub response_time: u32,
    pub clear_pending: bool,
}

/// `SMSG_CALENDAR_EVENT_STATUS`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarStatusView {
    pub guid: u64,
    pub event_id: u64,
    pub date: u32,
    pub flags: u32,
    pub status: u8,
    pub clear_pending: bool,
    pub response_time: u32,
}

/// `SMSG_CALENDAR_EVENT_INVITE_ALERT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteAlertView {
    pub event_id: u64,
    pub title: String,
    pub start: u32,
    pub flags: u32,
    pub event_type: i32,
    pub texture: i32,
    pub invite_id: u64,
    pub status: u8,
    pub rank: u8,
    pub creator: u64,
    pub inviter: u64,
}

/// `SMSG_CALENDAR_EVENT_UPDATED_ALERT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventUpdateView {
    pub clear_pending: bool,
    pub event_id: u64,
    pub old_date: u32,
    pub flags: u32,
    pub date: u32,
    pub event_type: u8,
    pub texture: i32,
    pub title: String,
    pub description: String,
    pub repeat: u8,
    pub max_size: u32,
    pub lockout_date: u32,
}

/// `SMSG_CALENDAR_COMMAND_RESULT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarCommandResultView {
    pub command: u32,
    pub event_name: String,
    pub name: String,
    pub result: u32,
}

/// Every calendar message, as the engine applies it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarWire {
    SendCalendar(Box<CalendarSnapshotView>),
    SendEvent(Box<CalendarEventView>),
    InitialInvites(Vec<(u64, u8)>),
    Invite(CalendarInviteAddedView),
    InviteRemoved {
        guid: u64,
        event_id: u64,
        flags: u32,
        clear_pending: bool,
    },
    Status(CalendarStatusView),
    CommandResult(CalendarCommandResultView),
    LockoutAdded {
        date: u32,
        lockout: CalendarLockoutView,
    },
    LockoutRemoved {
        map: i32,
        lockout: Option<CalendarLockoutView>,
    },
    LockoutUpdated {
        date: u32,
        map: i32,
        difficulty: i32,
        old_reset: i32,
        new_reset: i32,
    },
    InviteAlert(CalendarInviteAlertView),
    InviteRemovedAlert {
        event_id: u64,
        date: u32,
        flags: u32,
        status: u8,
    },
    InviteStatusAlert {
        event_id: u64,
        date: u32,
        flags: u32,
        status: u8,
    },
    EventRemovedAlert {
        clear_pending: bool,
        event_id: u64,
        date: u32,
    },
    EventUpdatedAlert(Box<CalendarEventUpdateView>),
    ModeratorStatusAlert {
        guid: u64,
        event_id: u64,
        status: u8,
        clear_pending: bool,
    },
    NumPending(u32),
    InviteNotes {
        guid: u64,
        event_id: u64,
        notes: String,
        clear_pending: bool,
    },
    InviteNotesAlert {
        event_id: u64,
        notes: String,
    },
    ClearPendingAction,
}

/// A new event's invitee, as `CMSG_CALENDAR_ADD_EVENT` carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarNewInviteView {
    pub guid: u64,
    pub status: u8,
    pub moderator: u8,
}

/// What the calendar asks the app for: a send, or a mailbox re-query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarRequest {
    GetCalendar,
    GetNumPending,
    GetEvent {
        event_id: u64,
    },
    GuildFilter {
        min_level: u32,
        max_level: u32,
        min_rank: u32,
    },
    ArenaTeam {
        team_id: u32,
    },
    AddEvent {
        title: String,
        description: String,
        event_type: u8,
        repeat: u8,
        max_size: u32,
        texture: i32,
        date: u32,
        lockout_date: u32,
        flags: u32,
        /// `None` for an announcement, which sends no list.
        invites: Option<Vec<CalendarNewInviteView>>,
    },
    UpdateEvent {
        event_id: u64,
        moderator_invite: u64,
        title: String,
        description: String,
        event_type: u8,
        repeat: u8,
        max_size: u32,
        texture: i32,
        date: u32,
        lockout_date: u32,
        flags: u32,
    },
    RemoveEvent {
        event_id: u64,
        moderator_invite: u64,
        guild: bool,
    },
    RemoveDayEvent {
        event_id: u64,
        invite_id: u64,
        flags: u32,
    },
    CopyEvent {
        event_id: u64,
        invite_id: u64,
        date: u32,
    },
    Invite {
        event_id: u64,
        moderator_invite: u64,
        name: String,
        pre_invite: bool,
        guild_event: bool,
    },
    Rsvp {
        event_id: u64,
        invite_id: u64,
        status: u32,
    },
    RemoveInvite {
        guid: u64,
        invite_id: u64,
        moderator_invite: u64,
        event_id: u64,
    },
    SetStatus {
        guid: u64,
        event_id: u64,
        invite_id: u64,
        moderator_invite: u64,
        status: u32,
    },
    SetModerator {
        guid: u64,
        event_id: u64,
        invite_id: u64,
        moderator_invite: u64,
        status: u32,
    },
    Complain {
        creator: u64,
        event_id: u64,
        invite_id: u64,
    },
    SignUp {
        event_id: u64,
        tentative: bool,
    },
    /// `CGMailInfo::ReceivedMail(0)` (`0x5c0c8f`): a removal mailed its notice.
    ReceivedMail,
}

/// `CGGuildInfo::CanEditGuildEvent` (`0x5cbca0`): the rank's right bit 20.
pub(crate) const RIGHT_EDIT_GUILD_EVENT: u32 = 0x10_0000;

/// The filter CVars' values, a missing one off (`0x5c003a`).
fn filters(model: &Model) -> [bool; TABLES] {
    let mut out = [true; TABLES];
    for (shown, name) in out.iter_mut().zip(FILTERS) {
        if let Some(name) = name {
            *shown = model
                .cvars
                .get(&name.to_ascii_lowercase())
                .is_some_and(|s| super::cvars::sstr_to_int(&s.value) != 0);
        }
    }
    out
}

/// Run `f` on the calendar with the model's other facts it reads.
pub(crate) fn with_calendar<R>(
    model: &mut Model,
    f: impl FnOnce(&mut CalendarModel, &Ctx) -> R,
) -> R {
    let filters = filters(model);
    let Model {
        calendar,
        lfg,
        guild,
        now,
        ..
    } = model;
    let ctx = Ctx {
        lfg: &lfg.catalog,
        filters,
        can_edit_guild_event: guild.in_guild && guild.rights & RIGHT_EDIT_GUILD_EVENT != 0,
        now_secs: *now,
    };
    f(calendar, &ctx)
}

/// A filter CVar was written: the day lists rebuild with its value (`SetCalendarFilterCVarCallback`
/// `0x5127a0`).
pub(crate) fn filter_cvar_written(model: &mut Model, key: &str) {
    if FILTERS
        .iter()
        .flatten()
        .any(|n| n.eq_ignore_ascii_case(key))
    {
        let filters = filters(model);
        model.calendar.set_filter(&filters);
    }
}

/// Fire the calendar's queued signals, in order; an error's line is its key's text filled with
/// the argument.
pub(crate) fn fire_signals(lua: &mlua::Lua) {
    let signals = std::mem::take(&mut crate::script::model_mut_of(lua).calendar.signals);
    for s in signals {
        match s {
            Signal::Event(event, args) => super::tick::fire_event_into(lua, event, args),
            Signal::Error(key, arg) => {
                let template = super::get_text::global_text(lua, key);
                let text = match &arg {
                    None => crate::strings::fill(&template, &[]),
                    Some(ErrorArg::Int(n)) => {
                        crate::strings::fill(&template, &[crate::strings::Arg::D(*n)])
                    }
                    Some(ErrorArg::Str(s)) => {
                        crate::strings::fill(&template, &[crate::strings::Arg::S(s)])
                    }
                };
                super::tick::fire_event_into(
                    lua,
                    "CALENDAR_UPDATE_ERROR",
                    vec![ScriptValue::Str(text)],
                );
            }
        }
    }
}

/// `CGCalendar::GetDateTimeText` (`0x5b7e50`): `FULLDATE_AND_TIME` of the date and the clock,
/// either left empty where the packed time leaves it unset; the clock in 24 hours under
/// `timeMgrUseMilitaryTime`.
pub fn date_time_text(time: u32, military: bool, get: &dyn Fn(&str) -> String) -> String {
    use crate::strings::{fill, Arg};
    const WEEKDAYS: [&str; 7] = [
        "WEEKDAY_SUNDAY",
        "WEEKDAY_MONDAY",
        "WEEKDAY_TUESDAY",
        "WEEKDAY_WEDNESDAY",
        "WEEKDAY_THURSDAY",
        "WEEKDAY_FRIDAY",
        "WEEKDAY_SATURDAY",
    ];
    const MONTHS: [&str; 12] = [
        "FULLDATE_MONTH_JANUARY",
        "FULLDATE_MONTH_FEBRUARY",
        "FULLDATE_MONTH_MARCH",
        "FULLDATE_MONTH_APRIL",
        "FULLDATE_MONTH_MAY",
        "FULLDATE_MONTH_JUNE",
        "FULLDATE_MONTH_JULY",
        "FULLDATE_MONTH_AUGUST",
        "FULLDATE_MONTH_SEPTEMBER",
        "FULLDATE_MONTH_OCTOBER",
        "FULLDATE_MONTH_NOVEMBER",
        "FULLDATE_MONTH_DECEMBER",
    ];
    let t = WowTime::decode(time);
    let date = if t.year >= 0 && t.month >= 0 && t.month_day >= 0 && t.weekday >= 0 {
        let weekday = get(WEEKDAYS[(t.weekday as usize).min(6)]);
        let month = get(MONTHS[(t.month as usize).min(11)]);
        fill(
            &get("FULLDATE"),
            &[
                Arg::S(&weekday),
                Arg::S(&month),
                Arg::D(i64::from(t.month_day + 1)),
                Arg::D(i64::from(t.year + 2000)),
                Arg::D(i64::from(t.month + 1)),
            ],
        )
    } else {
        String::new()
    };
    let clock = if t.hour >= 0 && t.minute >= 0 {
        if military {
            fill(
                &get("TIME_TWENTYFOURHOURS"),
                &[Arg::D(i64::from(t.hour)), Arg::D(i64::from(t.minute))],
            )
        } else {
            let (hour, token) = match t.hour {
                0 => (12, "TIME_TWELVEHOURAM"),
                12 => (12, "TIME_TWELVEHOURPM"),
                h if h > 12 => (h - 12, "TIME_TWELVEHOURPM"),
                h => (h, "TIME_TWELVEHOURAM"),
            };
            fill(
                &get(token),
                &[Arg::D(i64::from(hour)), Arg::D(i64::from(t.minute))],
            )
        }
    } else {
        String::new()
    };
    fill(&get("FULLDATE_AND_TIME"), &[Arg::S(&date), Arg::S(&clock)])
}

impl super::UiScript {
    fn calendar_run<R>(&mut self, f: impl FnOnce(&mut CalendarModel, &Ctx) -> R) -> R {
        let r = with_calendar(&mut self.model_mut(), f);
        fire_signals(self.lua());
        r
    }

    /// Push the holiday and map tables.
    pub fn set_calendar_catalog(&mut self, catalog: CalendarCatalogView) {
        self.model_mut().calendar.set_catalog(catalog);
    }

    /// Push the client's Region (`[WoW Config] Region`), the holidays' region mask's bit.
    pub fn set_calendar_region(&mut self, region: i32) {
        self.model_mut().calendar.set_region(region);
    }

    /// Push the player's facts; `None` out of world.
    pub fn set_calendar_player(&mut self, player: Option<CalendarPlayerView>) {
        if self.model_ref().calendar.player != player {
            self.model_mut().calendar.player = player;
        }
    }

    /// Push the game clock; each new minute may fire an event's alarm.
    pub fn set_calendar_clock(&mut self, now: WowTime) {
        if self.model_ref().calendar.now == now {
            return;
        }
        self.calendar_run(|c, _| c.tick_clock(now));
    }

    /// Push the ignore and friend lists; an ignore change rebuilds the day lists.
    pub fn set_calendar_social(&mut self, ignored: HashSet<u64>, friends: HashSet<u64>) {
        // Read first: the feed pushes every frame, and a `&mut` borrow alone moves the count.
        if self.model_ref().calendar.social_is(&ignored, &friends) {
            return;
        }
        let moved = self.model_mut().calendar.set_social(ignored, friends);
        if moved {
            self.calendar_run(|c, ctx| c.ignores_changed(ctx));
        }
    }

    /// Push the raid roster's guids, the invite list's "party" sort key.
    pub fn set_calendar_raid_members(&mut self, raid: HashSet<u64>) {
        if self.model_ref().calendar.raid_members != raid {
            self.model_mut().calendar.raid_members = raid;
        }
    }

    /// The guids the calendar wants named.
    pub fn calendar_names_wanted(&self) -> Vec<u64> {
        self.model_ref().calendar.names_wanted()
    }

    /// Push resolved names; awaited ones fire what was waiting on them.
    pub fn set_calendar_names(&mut self, names: HashMap<u64, CalendarName>) {
        let fresh: HashMap<u64, CalendarName> = {
            let model = self.model_ref();
            names
                .into_iter()
                .filter(|(g, n)| model.calendar.names.get(g) != Some(n))
                .collect()
        };
        if fresh.is_empty() {
            return;
        }
        self.calendar_run(|c, _| c.names_landed(fresh));
    }

    /// `CGCalendar::EnterWorld` (`0x5be2a0`), at each world-enter cascade.
    pub fn calendar_enter_world(&mut self) {
        self.calendar_run(|c, ctx| c.enter_world(ctx));
    }

    /// One server message, applied as 12340 applies it.
    pub fn calendar_packet(&mut self, wire: CalendarWire) {
        self.calendar_run(|c, ctx| c.apply(ctx, wire));
    }

    /// `CGCalendar::GuildChanged` (`0x5c0f40`): the player's own guild field moved.
    pub fn calendar_guild_changed(&mut self, in_guild: bool) {
        self.calendar_run(|c, ctx| c.guild_changed(ctx, in_guild));
    }

    /// `CGCalendar::InvalidatePlayer` (`0x5bde60`).
    pub fn calendar_invalidate_player(&mut self, guid: u64) {
        self.calendar_run(|c, _| c.invalidate_player(guid));
    }

    /// Drain the calendar's queued requests, in call order.
    pub fn take_calendar_requests(&mut self) -> Vec<CalendarRequest> {
        drain_field!(self, calendar.requests)
    }

    /// Take the calendar out of this VM, for the one a `ReloadUI` builds.
    pub fn take_calendar(&mut self) -> Box<CalendarModel> {
        Box::new(std::mem::take(&mut self.model_mut().calendar))
    }

    /// Seat a calendar taken from the VM a `ReloadUI` replaced.
    pub fn restore_calendar(&mut self, calendar: Box<CalendarModel>) {
        self.model_mut().calendar = *calendar;
    }
}
