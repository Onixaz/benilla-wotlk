//! 12340's calendar event (`0x5f1000`-`0x5f32b0`): the event the editor or viewer holds open, its
//! invite list and its sort, and the sends its verbs build.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::engine::{calendar_type, kind, ErrorArg, Signal};
use super::time::WowTime;
use super::{CalendarEventUpdateView, CalendarEventView, CalendarName, CalendarPlayerView};
use super::{CalendarNewInviteView, CalendarRequest};
use crate::script::who_sort::{cmp_utf8i, sort_utf8i};
use crate::script::ScriptValue;

/// The most invites an event takes (`0x5f1c22`).
pub(crate) const MAX_INVITES: u32 = 100;

/// The reference's invite-status order (`0xad16cc`): confirmed, signed up, accepted, standby,
/// tentative, declined, out, not signed up, invited.
const STATUS_ORDER: [i32; 9] = [9, 3, 6, 1, 7, 4, 2, 8, 5];

/// One invitee, as the reference's event keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Invite {
    pub invite_id: u64,
    pub guid: u64,
    pub level: u8,
    pub status: u8,
    /// 0 none, 1 moderator, 2 creator.
    pub moderator: u8,
    pub invite_type: u8,
    pub response_time: u32,
    pub notes: String,
}

/// What an invite edit did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InviteEdit {
    Nothing,
    /// A new event's list changed locally.
    Local,
    Send(CalendarRequest),
    Error(&'static str),
}

/// The reference's calendar event.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CurrentEvent {
    pub event_id: u64,
    pub invites: Vec<Invite>,
    selected_invite: u64,
    pub moderator_invite: u64,
    pub title: String,
    pub description: String,
    pub event_type: u32,
    pub repeat: u32,
    pub max_size: u32,
    pub date: WowTime,
    pub lockout_date: WowTime,
    pub flags: u32,
    /// Not yet on the server.
    pub new: bool,
    pub changed: bool,
    pub creator: u64,
    pub texture: i32,
    /// Each criterion's reversed flag; a criterion's slot is its own number.
    pub reverse: [bool; 6],
    pub sort_type: u32,
    /// Invitees whose names were not cached when they arrived.
    pending_names: Vec<u64>,
    /// What the last awaited name signals (`0x5f1b10`).
    pub signal_type: u32,
    pub guild_id: u32,
}

impl Default for CurrentEvent {
    /// The event clear (`0x5f1060`) on a fresh object: the date unset, the lockout date zeroed.
    fn default() -> Self {
        Self {
            event_id: 0,
            invites: Vec::new(),
            selected_invite: 0,
            moderator_invite: 0,
            title: String::new(),
            description: String::new(),
            event_type: 0,
            repeat: 0,
            max_size: MAX_INVITES,
            date: WowTime::default(),
            lockout_date: WowTime {
                minute: 0,
                hour: 0,
                weekday: 0,
                month_day: 0,
                month: 0,
                year: 0,
                flags: 0,
                tz_hours: 0,
            },
            flags: 0,
            new: false,
            changed: false,
            creator: 0,
            texture: -1,
            reverse: [false; 6],
            sort_type: 3,
            pending_names: Vec::new(),
            signal_type: 1,
            guild_id: 0,
        }
    }
}

/// The string scrub (`0x7e1830`): pipes stripped, cut to the buffer, at a line break when asked,
/// and to one character short of `max_chars`.
pub(crate) fn scrub(source: &str, max_chars: usize, size: usize, cut_lines: bool) -> String {
    let mut out = String::new();
    for c in source.chars().filter(|&c| c != '|') {
        if out.len() + c.len_utf8() > size - 1 {
            break;
        }
        out.push(c);
    }
    if cut_lines {
        // The newline cut (`0x7e1640`): a CR, an LF, or a `\n` escape ends the text.
        let bytes = out.as_bytes();
        let cut = (0..bytes.len()).find(|&i| match bytes[i] {
            b'\r' | b'\n' => true,
            b'\\' => bytes.get(i + 1) == Some(&b'n'),
            _ => false,
        });
        if let Some(i) = cut {
            out.truncate(i);
        }
    }
    match out.char_indices().nth(max_chars.saturating_sub(1)) {
        Some((i, _)) => out[..i].to_string(),
        None => out,
    }
}

/// The name-case fix (`0x7e2040`): the first letter up, the rest down, over Latin-1 and Cyrillic.
pub(crate) fn fix_name_case(name: &str) -> String {
    name.chars()
        .enumerate()
        .map(|(i, c)| {
            let u = u32::from(c);
            let m = if i == 0 {
                match u {
                    0x61..=0x7A | 0xE0..=0xFE | 0x430..=0x44F => u - 32,
                    0x153 => 0x152,
                    0x451 => 0x401,
                    _ => u,
                }
            } else {
                match u {
                    0x41..=0x5A | 0xC0..=0xDE | 0x410..=0x42F => u + 32,
                    0x152 => 0x153,
                    0x401 => 0x451,
                    _ => u,
                }
            };
            char::from_u32(m).unwrap_or(c)
        })
        .collect()
}

impl CurrentEvent {
    /// The editor's new event (`0x5f2a20`): the player creates it, and unless it is an
    /// announcement the creator is its first, confirmed invitee; a guild event's invite is a
    /// sign-up.
    pub(crate) fn new_local(me: u64, guid: u64, level: u8, calendar_type: u32) -> Self {
        let mut e = Self {
            creator: me,
            ..Self::default()
        };
        let mut invite_type = 0;
        match calendar_type {
            0 => e.flags |= kind::PLAYER,
            1 => e.flags |= kind::ANNOUNCEMENT,
            _ => {
                e.flags |= kind::GUILD_EVENT;
                invite_type = 1;
            }
        }
        if guid != 0 {
            e.invites.push(Invite {
                guid,
                level,
                status: 3,
                moderator: 2,
                invite_type,
                ..Invite::default()
            });
        }
        e.moderator_invite = 1;
        e.new = true;
        e
    }

    /// The event fetch's handler (`0x5f2c00`): the event and its invitees, and the player's own
    /// status, -1 when uninvited, 7 for an unjoined guild event.
    pub(crate) fn from_wire(
        w: CalendarEventView,
        me: u64,
        names: &HashMap<u64, CalendarName>,
    ) -> (Self, i32) {
        let mut e = Self {
            creator: w.creator,
            event_id: w.event_id,
            title: truncate(&w.title, 127),
            description: truncate(&w.description, 1023),
            event_type: u32::from(w.event_type),
            repeat: u32::from(w.repeat),
            max_size: w.max_size,
            texture: w.texture,
            flags: w.flags,
            date: WowTime::decode(w.date),
            lockout_date: WowTime::decode(w.lockout_date),
            guild_id: w.guild_id,
            ..Self::default()
        };
        let mut status = -1;
        let mut found = false;
        for i in w.invites {
            if !names.contains_key(&i.guid) {
                e.pending_names.push(i.guid);
            }
            if i.guid == me {
                found = true;
                status = i32::from(i.status);
                if i.moderator != 0 {
                    e.moderator_invite = i.invite_id;
                }
            }
            e.invites.push(Invite {
                invite_id: i.invite_id,
                guid: i.guid,
                level: i.level,
                status: i.status,
                moderator: i.moderator,
                invite_type: i.invite_type,
                response_time: i.response_time,
                notes: truncate(&i.notes, 127),
            });
        }
        if !found && e.flags & kind::GUILD_EVENT != 0 {
            status = 7;
        }
        if e.pending_names.is_empty() {
            e.sort(3, false, names, &HashSet::new());
        }
        (e, status)
    }

    pub(crate) fn names_pending(&self) -> bool {
        !self.pending_names.is_empty()
    }

    /// The moderator test (`0x5f1b30`): a moderator invite, or the creator.
    pub(crate) fn is_moderator(&self, me: u64) -> bool {
        self.moderator_invite != 0 || (me != 0 && me == self.creator)
    }

    pub(crate) fn invite_of(&self, guid: u64) -> Option<&Invite> {
        self.invites.iter().find(|i| i.guid == guid)
    }

    fn index_of(&self, guid: u64) -> Option<usize> {
        self.invites.iter().position(|i| i.guid == guid)
    }

    /// The selected invite's index (`0x5b8e00`): 0-based, -1 none.
    pub(crate) fn selected_index(&self) -> i32 {
        self.invites
            .iter()
            .position(|i| i.invite_id == self.selected_invite)
            .map_or(-1, |i| i as i32)
    }

    /// The invite select (`0x5f1a00`).
    pub(crate) fn select_invite(&mut self, index: u32) {
        if let Some(i) = self.invites.get(index as usize) {
            self.selected_invite = i.invite_id;
        }
    }

    // ── The setters ─────────────────────────────────────────────────────────────────────────

    /// The title set (`0x5f1130`): at most 31 characters, one line.
    pub(crate) fn set_title(&mut self, title: Option<&str>) {
        if let Some(t) = title.filter(|t| *t != self.title) {
            self.title = scrub(t, 32, 128, true);
            self.changed = true;
        }
    }

    /// The description set (`0x5f1180`): at most 255 characters.
    pub(crate) fn set_description(&mut self, description: Option<&str>) {
        if let Some(d) = description.filter(|d| *d != self.description) {
            self.description = scrub(d, 256, 1024, false);
            self.changed = true;
        }
    }

    /// The type set (`0x5f11d0`).
    pub(crate) fn set_type(&mut self, event_type: u32) {
        if event_type < 5 && event_type != self.event_type {
            self.event_type = event_type;
            self.changed = true;
        }
    }

    /// The repeat set (`0x5f1200`).
    pub(crate) fn set_repeat(&mut self, repeat: u32) {
        if repeat < 4 && repeat != self.repeat {
            self.repeat = repeat;
            self.changed = true;
        }
    }

    /// The size set (`0x5f1230`): not on a guild kind.
    pub(crate) fn set_size(&mut self, size: u32) {
        if self.flags & kind::GUILD == 0 && size > 1 && size != self.max_size {
            self.max_size = size;
            self.changed = true;
        }
    }

    /// The date set (`0x5f1260`): any differing field marks the event changed, accepted or not.
    pub(crate) fn set_date(&mut self, month: u32, month_day: u32, year: u32) -> bool {
        if month as i32 != self.date.month
            || month_day as i32 != self.date.month_day
            || year as i32 != self.date.year
        {
            self.changed = true;
        }
        self.date.set_date(month, month_day, year)
    }

    /// The time set (`0x5f12a0`).
    pub(crate) fn set_time(&mut self, hour: u32, minute: u32) -> bool {
        if hour as i32 != self.date.hour || minute as i32 != self.date.minute {
            self.changed = true;
        }
        self.date.set_hour_and_minutes(hour, minute)
    }

    /// The lockout-date set (`0x5f12e0`): not on a guild kind.
    pub(crate) fn set_lockout_date(&mut self, month: u32, month_day: u32, year: u32) -> bool {
        if self.flags & kind::GUILD != 0 {
            return false;
        }
        let d = &self.lockout_date;
        if month as i32 != d.month || month_day as i32 != d.month_day || year as i32 != d.year {
            self.changed = true;
        }
        self.lockout_date.set_date(month, month_day, year)
    }

    /// The lockout-time set (`0x5f1340`).
    pub(crate) fn set_lockout_time(&mut self, hour: u32, minute: u32) -> bool {
        if self.flags & kind::GUILD != 0 {
            return false;
        }
        if hour as i32 != self.lockout_date.hour || minute as i32 != self.lockout_date.minute {
            self.changed = true;
        }
        self.lockout_date.set_hour_and_minutes(hour, minute)
    }

    /// The texture set (`0x5f1390`).
    pub(crate) fn set_texture(&mut self, texture: i32) {
        if texture != self.texture {
            self.texture = texture;
            self.changed = true;
        }
    }

    /// The flag set and clear (`0x5f13b0`, `0x5f13e0`).
    pub(crate) fn set_flag(&mut self, flag: u32, set: bool) {
        if (self.flags & flag != 0) != set {
            self.flags ^= flag;
            self.changed = true;
        }
    }

    // ── The server's changes ────────────────────────────────────────────────────────────────

    /// The event update's handler (`0x5f14b0`).
    pub(crate) fn update_from_alert(&mut self, u: &CalendarEventUpdateView) {
        self.flags = u.flags;
        self.event_type = u32::from(u.event_type);
        self.texture = u.texture;
        self.title = truncate(&u.title, 127);
        self.description = truncate(&u.description, 1023);
        self.repeat = u32::from(u.repeat);
        self.max_size = u.max_size;
        self.date = WowTime::decode(u.date);
        self.lockout_date = WowTime::decode(u.lockout_date);
    }

    /// The invite removal (`0x5f1580`).
    pub(crate) fn remove_invite_guid(&mut self, guid: u64) {
        if let Some(i) = self.index_of(guid) {
            self.invites.remove(i);
        }
    }

    /// The invite-status update (`0x5f1600`): a response time of 0 keeps the old one.
    pub(crate) fn update_status(&mut self, guid: u64, status: u8, response_time: u32) -> bool {
        let Some(i) = self.invites.iter_mut().find(|i| i.guid == guid) else {
            return false;
        };
        i.status = status;
        if response_time != 0 {
            i.response_time = response_time;
        }
        true
    }

    /// The invite-notes update (`0x5f1670`).
    pub(crate) fn update_notes(&mut self, guid: u64, notes: String) -> bool {
        let Some(i) = self.invites.iter_mut().find(|i| i.guid == guid) else {
            return false;
        };
        i.notes = truncate(&notes, 127);
        true
    }

    /// The invite moderator-status update (`0x5f2640`): the player's own row moves its moderator
    /// invite.
    pub(crate) fn update_moderator(&mut self, guid: u64, status: u8, me: u64) -> bool {
        let Some(i) = self.invites.iter_mut().find(|i| i.guid == guid) else {
            return false;
        };
        i.moderator = status;
        if me != 0 && me == guid {
            self.moderator_invite = if status == 1 { i.invite_id } else { 0 };
        }
        true
    }

    /// The invite add (`0x5f32b0`): with every name cached, the list sorts and repaints at once.
    pub(crate) fn add_invite(
        &mut self,
        invite: Invite,
        names: &HashMap<u64, CalendarName>,
    ) -> Option<Signal> {
        if !names.contains_key(&invite.guid) {
            self.pending_names.push(invite.guid);
        }
        self.invites.push(invite);
        if self.names_pending() {
            self.signal_type = 0;
            return None;
        }
        self.sort(self.sort_type, false, names, &HashSet::new());
        Some(Signal::Event("CALENDAR_UPDATE_INVITE_LIST", vec![]))
    }

    /// The initial invite list's handler (`0x5f2f90`): a mass invite's candidates replace the list,
    /// the player confirmed as creator; the event stays new.
    pub(crate) fn initial_invites(
        &mut self,
        list: Vec<(u64, u8)>,
        player: Option<&CalendarPlayerView>,
        names: &HashMap<u64, CalendarName>,
    ) -> Option<Signal> {
        let me = player.map_or(0, |p| p.guid);
        let mut found = false;
        self.invites.clear();
        for (guid, level) in list {
            if !names.contains_key(&guid) {
                self.pending_names.push(guid);
            }
            let own = guid == me;
            found |= own;
            self.invites.push(Invite {
                guid,
                level,
                status: if own { 3 } else { 0 },
                moderator: if own { 2 } else { 0 },
                ..Invite::default()
            });
        }
        if !found {
            self.invites.push(Invite {
                guid: me,
                level: player.map_or(0, |p| p.level),
                status: 3,
                moderator: 2,
                ..Invite::default()
            });
        }
        self.moderator_invite = 1;
        self.new = true;
        self.selected_invite = 0;
        if self.names_pending() {
            self.signal_type = 1;
            return None;
        }
        self.sort(3, false, names, &HashSet::new());
        Some(Signal::Event(
            "CALENDAR_UPDATE_INVITE_LIST",
            vec![ScriptValue::Bool(true)],
        ))
    }

    /// Names landed: the last awaited one sorts the list and fires the deferred signal
    /// (the invite name-cache callback `0x5f1b10`).
    pub(crate) fn names_landed(
        &mut self,
        names: &HashMap<u64, CalendarName>,
        raid: &HashSet<u64>,
    ) -> Option<Signal> {
        if self.pending_names.is_empty() {
            return None;
        }
        self.pending_names.retain(|g| !names.contains_key(g));
        if self.names_pending() {
            return None;
        }
        self.sort(self.sort_type, false, names, raid);
        Some(match self.signal_type {
            0 => Signal::Event("CALENDAR_UPDATE_INVITE_LIST", vec![]),
            1 => Signal::Event("CALENDAR_UPDATE_INVITE_LIST", vec![ScriptValue::Bool(true)]),
            2 => Signal::Event("CALENDAR_NEW_EVENT", vec![ScriptValue::Bool(false)]),
            3 => Signal::Event("CALENDAR_NEW_EVENT", vec![ScriptValue::Bool(true)]),
            _ => Signal::Event(
                "CALENDAR_OPEN_EVENT",
                vec![ScriptValue::Str(calendar_type(self.flags).into())],
            ),
        })
    }

    // ── The sort ────────────────────────────────────────────────────────────────────────────

    /// The invite sort (`0x5f1980`): the criterion becomes the sort, toggling its direction when
    /// asked; every other criterion's direction resets.
    pub(crate) fn sort(
        &mut self,
        criterion: u32,
        toggle: bool,
        names: &HashMap<u64, CalendarName>,
        raid: &HashSet<u64>,
    ) {
        for i in 0..6 {
            if i == criterion as usize {
                if toggle {
                    self.reverse[i] = !self.reverse[i];
                }
                self.sort_type = criterion;
            } else {
                self.reverse[i] = false;
            }
        }
        let sort_type = self.sort_type;
        let reverse = self
            .reverse
            .get(sort_type as usize)
            .copied()
            .unwrap_or(false);
        self.invites
            .sort_by(|a, b| compare_invites(a, b, sort_type, reverse, names, raid));
    }

    // ── The sends ───────────────────────────────────────────────────────────────────────────

    /// The event add (`0x5f1b90`): a new event's checks and its send; `Err(None)` refuses silently.
    pub(crate) fn add_request(
        &self,
        now: &WowTime,
        guild_id: u32,
    ) -> Result<CalendarRequest, Option<(&'static str, Option<ErrorArg>)>> {
        if !self.new {
            return Err(None);
        }
        if self.title.is_empty() {
            return Err(Some(("CALENDAR_ERROR_NEEDS_TITLE", None)));
        }
        if self.flags & kind::GUILD != 0 {
            if guild_id == 0 {
                return Err(Some(("ERR_GUILD_PLAYER_NOT_IN_GUILD", None)));
            }
        } else {
            if self.max_size == 0 {
                return Err(None);
            }
            if self.max_size > MAX_INVITES {
                return Err(Some((
                    "CALENDAR_ERROR_INVITES_EXCEEDED",
                    Some(ErrorArg::Int(i64::from(MAX_INVITES))),
                )));
            }
        }
        if self.date.hour == -1 || self.date.minute == -1 {
            return Err(Some(("CALENDAR_ERROR_INVALID_TIME", None)));
        }
        if self.date.month == -1 || self.date.month_day == -1 || self.date.year == -1 {
            return Err(Some(("CALENDAR_ERROR_INVALID_DATE", None)));
        }
        if self.date.le(now) {
            return Err(Some(("CALENDAR_ERROR_EVENT_TIME_PASSED", None)));
        }
        let invites = (self.flags & kind::ANNOUNCEMENT == 0).then(|| {
            self.invites
                .iter()
                .map(|i| CalendarNewInviteView {
                    guid: i.guid,
                    status: i.status,
                    moderator: i.moderator,
                })
                .collect()
        });
        Ok(CalendarRequest::AddEvent {
            title: self.title.clone(),
            description: self.description.clone(),
            event_type: self.event_type as u8,
            repeat: self.repeat as u8,
            max_size: self.max_size,
            texture: self.texture,
            date: self.date.encode(),
            lockout_date: self.lockout_date.encode(),
            flags: self.flags,
            invites,
        })
    }

    /// The event update (`0x5f1ef0`): only a changed event, and never one already begun.
    pub(crate) fn update_request(
        &mut self,
        now: &WowTime,
    ) -> Result<Option<CalendarRequest>, &'static str> {
        if !self.changed {
            return Ok(None);
        }
        self.changed = false;
        if self.date.le(now) {
            return Err("CALENDAR_ERROR_EVENT_PASSED");
        }
        Ok(Some(CalendarRequest::UpdateEvent {
            event_id: self.event_id,
            moderator_invite: self.moderator_invite,
            title: self.title.clone(),
            description: self.description.clone(),
            event_type: self.event_type as u8,
            repeat: self.repeat as u8,
            max_size: self.max_size,
            texture: self.texture,
            date: self.date.encode(),
            lockout_date: self.lockout_date.encode(),
            flags: self.flags,
        }))
    }

    /// The event remove (`0x5f1e30`).
    pub(crate) fn remove_request(&self, me: u64) -> Result<Option<CalendarRequest>, &'static str> {
        if !self.is_moderator(me) {
            return Err("CALENDAR_ERROR_PERMISSIONS");
        }
        Ok(Some(CalendarRequest::RemoveEvent {
            event_id: self.event_id,
            moderator_invite: self.moderator_invite,
            guild: self.flags & kind::GUILD != 0,
        }))
    }

    /// The invite removal by guid (`0x5f23b0`): `None` when the player holds no invite.
    pub(crate) fn remove_invite_of(
        &mut self,
        me: u64,
    ) -> Option<Result<Option<CalendarRequest>, &'static str>> {
        if me == 0 {
            return None;
        }
        let index = self.index_of(me)?;
        self.remove_invite_at(index as u32, me)
    }

    /// The invite remove (`0x5f2220`): never the creator; another's invite only as a moderator; a
    /// new event's drops at once.
    pub(crate) fn remove_invite_at(
        &mut self,
        index: u32,
        me: u64,
    ) -> Option<Result<Option<CalendarRequest>, &'static str>> {
        let invite = self.invites.get(index as usize)?.clone();
        if invite.moderator == 2 {
            return Some(Err("CALENDAR_DELETE_CREATOR_FAILED"));
        }
        if me != 0 && invite.guid != me && !self.is_moderator(me) {
            return Some(Err("CALENDAR_ERROR_PERMISSIONS"));
        }
        if self.new {
            self.invites.remove(index as usize);
            return Some(Ok(None));
        }
        Some(Ok(Some(CalendarRequest::RemoveInvite {
            guid: invite.guid,
            invite_id: invite.invite_id,
            moderator_invite: self.moderator_invite,
            event_id: self.event_id,
        })))
    }

    /// The invite (`0x5f2070`): not on an announcement, at most 100, never a name already on the
    /// list.
    pub(crate) fn invite_request(
        &self,
        name: &str,
        me: u64,
        names: &HashMap<u64, CalendarName>,
    ) -> Result<CalendarRequest, (&'static str, Option<ErrorArg>)> {
        if self.flags & kind::ANNOUNCEMENT != 0 {
            return Err(("CALENDAR_ERROR_INVITES_DISABLED", None));
        }
        if self.invites.len() as u32 >= MAX_INVITES {
            return Err((
                "CALENDAR_ERROR_INVITES_EXCEEDED",
                Some(ErrorArg::Int(i64::from(MAX_INVITES))),
            ));
        }
        let fixed = fix_name_case(name);
        let fixed: String = fixed.chars().take(47).collect();
        if self
            .invites
            .iter()
            .any(|i| names.get(&i.guid).is_some_and(|n| n.name == fixed))
        {
            return Err((
                "CALENDAR_ERROR_ALREADY_INVITED_TO_EVENT_S",
                Some(ErrorArg::Str(fixed)),
            ));
        }
        if !self.is_moderator(me) {
            return Err(("CALENDAR_ERROR_PERMISSIONS", None));
        }
        Ok(CalendarRequest::Invite {
            event_id: self.event_id,
            moderator_invite: self.moderator_invite,
            name: fixed,
            pre_invite: self.new,
            guild_event: self.flags & kind::GUILD_EVENT != 0,
        })
    }

    /// The invite RSVP (`0x5f23f0`): accept, decline or tentative on the player's own invite, never
    /// a guild sign-up's.
    pub(crate) fn rsvp_request(&self, status: u32, me: u64) -> Option<CalendarRequest> {
        if self.new || !matches!(status, 1 | 2 | 8) {
            return None;
        }
        let invite = self.invite_of(me).filter(|_| me != 0)?;
        if self.flags & kind::GUILD_EVENT != 0 && invite.invite_type == 1 {
            return None;
        }
        Some(CalendarRequest::Rsvp {
            event_id: self.event_id,
            invite_id: invite.invite_id,
            status,
        })
    }

    /// The sign-up (`0x5f2530`): only while the player is on no row.
    pub(crate) fn sign_up_request(&self, tentative: bool, me: u64) -> Option<CalendarRequest> {
        if self.new || (me != 0 && self.invite_of(me).is_some()) {
            return None;
        }
        Some(CalendarRequest::SignUp {
            event_id: self.event_id,
            tentative,
        })
    }

    /// The invite-status check (`0x5f26f0`): a moderator, a real status other than signed up or not
    /// signed up, accept and decline not on a guild event, and a change.
    pub(crate) fn can_set_status(&self, index: u32, status: u32, me: u64) -> bool {
        if !self.is_moderator(me) || matches!(status, 0 | 6 | 7) || status >= 9 {
            return false;
        }
        let Some(invite) = self.invites.get(index as usize) else {
            return false;
        };
        if self.flags & kind::GUILD_EVENT != 0 && matches!(status, 1 | 2) {
            return false;
        }
        status != u32::from(invite.status)
    }

    /// The invite-status set (`0x5f2750`).
    pub(crate) fn set_status(&mut self, index: u32, status: u32, me: u64) -> InviteEdit {
        if !self.can_set_status(index, status, me) {
            return InviteEdit::Nothing;
        }
        if self.new {
            self.invites[index as usize].status = status as u8;
            return InviteEdit::Local;
        }
        let invite = &self.invites[index as usize];
        InviteEdit::Send(CalendarRequest::SetStatus {
            guid: invite.guid,
            event_id: self.event_id,
            invite_id: invite.invite_id,
            moderator_invite: self.moderator_invite,
            status,
        })
    }

    /// The moderate check (`0x5f16e0`): a guild event's member sign-up cannot moderate.
    pub(crate) fn can_moderate(&self, index: u32) -> Result<bool, &'static str> {
        let Some(invite) = self.invites.get(index as usize) else {
            return Ok(false);
        };
        if self.flags & kind::GUILD_EVENT != 0 && invite.invite_type == 0 {
            return Err("CALENDAR_ERROR_NO_MODERATOR");
        }
        Ok(true)
    }

    /// The moderator-status set (`0x5f2860`).
    pub(crate) fn set_moderator(&mut self, index: u32, status: u32, me: u64) -> InviteEdit {
        if !self.is_moderator(me) {
            return InviteEdit::Error("CALENDAR_ERROR_PERMISSIONS");
        }
        if index as usize >= self.invites.len() || status >= 2 {
            return InviteEdit::Nothing;
        }
        match self.can_moderate(index) {
            Err(key) => return InviteEdit::Error(key),
            Ok(false) => return InviteEdit::Nothing,
            Ok(true) => {}
        }
        let invite = &self.invites[index as usize];
        if status == u32::from(invite.moderator) {
            return InviteEdit::Nothing;
        }
        if self.new {
            self.invites[index as usize].moderator = status as u8;
            return InviteEdit::Local;
        }
        InviteEdit::Send(CalendarRequest::SetModerator {
            guid: invite.guid,
            event_id: self.event_id,
            invite_id: invite.invite_id,
            moderator_invite: self.moderator_invite,
            status,
        })
    }
}

/// A string cut to a reference buffer, at a character boundary.
fn truncate(s: &str, max_bytes: usize) -> String {
    let mut end = s.len().min(max_bytes);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The invite sort's compare (`0x5f1720`): by name, level, class, status, raid membership or notes;
/// a tie on any but the name falls to the name.
fn compare_invites(
    a: &Invite,
    b: &Invite,
    sort_type: u32,
    reverse: bool,
    names: &HashMap<u64, CalendarName>,
    raid: &HashSet<u64>,
) -> Ordering {
    let (na, nb) = (names.get(&a.guid), names.get(&b.guid));
    let by_name = || match (na, nb) {
        (Some(x), Some(y)) => cmp_utf8i(&x.name, &y.name),
        _ => Ordering::Equal,
    };
    let order = |s: u8| STATUS_ORDER.get(usize::from(s)).copied().unwrap_or(0);
    let mut result = match sort_type {
        0 => by_name(),
        1 => a.level.cmp(&b.level),
        2 => match (na, nb) {
            (Some(x), Some(y)) if !x.class_name.is_empty() && !y.class_name.is_empty() => {
                sort_utf8i(&x.class_name, &y.class_name)
            }
            _ => Ordering::Equal,
        },
        3 => order(a.status).cmp(&order(b.status)),
        4 => match (raid.contains(&a.guid), raid.contains(&b.guid)) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => Ordering::Equal,
        },
        5 => sort_utf8i(&a.notes, &b.notes),
        _ => Ordering::Equal,
    };
    if reverse {
        result = result.reverse();
    }
    if sort_type != 0 && result == Ordering::Equal {
        return by_name();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A title loses its pipes and its second line, and keeps 31 characters.
    #[test]
    fn a_title_is_scrubbed_as_the_editor_stores_it() {
        assert_eq!(
            scrub("Raid |cffff0000night", 32, 128, true),
            "Raid cffff0000night"
        );
        assert_eq!(scrub("Line one\nline two", 32, 128, true), "Line one");
        assert_eq!(scrub(&"x".repeat(40), 32, 128, true).len(), 31);
        assert_eq!(scrub("a\nb", 256, 1024, false), "a\nb");
    }

    /// A typed name is cased as the client sends it.
    #[test]
    fn an_invite_name_is_cased_like_a_character_name() {
        assert_eq!(fix_name_case("cLAUDE"), "Claude");
        assert_eq!(fix_name_case("éLZA"), "Élza");
    }

    /// The status sort puts the confirmed first and the invited last.
    #[test]
    fn the_status_sort_follows_the_reference_order() {
        let mut e = CurrentEvent::default();
        for (guid, status) in [(1, 0), (2, 3), (3, 1)] {
            e.invites.push(Invite {
                guid,
                status,
                ..Invite::default()
            });
        }
        e.sort(3, false, &HashMap::new(), &HashSet::new());
        let order: Vec<u64> = e.invites.iter().map(|i| i.guid).collect();
        assert_eq!(order, [2, 3, 1]);
        e.sort(3, true, &HashMap::new(), &HashSet::new());
        let order: Vec<u64> = e.invites.iter().map(|i| i.guid).collect();
        assert_eq!(order, [1, 3, 2]);
    }
}
