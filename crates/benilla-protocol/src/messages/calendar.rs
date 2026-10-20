//! Calendar: the whole-state request/response pair (`CMSG_CALENDAR_GET_CALENDAR` /
//! `SMSG_CALENDAR_SEND_CALENDAR`, [`super::opcode::CMSG_CALENDAR_GET_CALENDAR`]/
//! [`super::opcode::SMSG_CALENDAR_SEND_CALENDAR`]). The field order and widths are transcribed
//! from AzerothCore's `HandleCalendarGetCalendar` (`Handlers/CalendarHandler.cpp`). Event
//! creation, invites and RSVPs are separate opcodes and not read here.

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_packed_guid, read_packed_time, read_u32_le, read_u64_le,
    read_u8, PackedTime,
};

/// One row of the invite section — an event we've been invited to (which may or may not still be
/// one of our own [`CalendarEvent`]s if we're also its creator/attendee).
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarInvite {
    pub event_id: u64,
    pub invite_id: u64,
    /// `CalendarInviteStatus` (VERIFIED AzerothCore `Calendar.h`): 0 invited, 1 accepted, 2
    /// declined, 3 confirmed, 4 out (removed), 5 signed up, 6 not signed up, 7 tentative, 8 (not
    /// used on the wire; native's own reply, see `calendar.md`'s "observed status values").
    pub status: u8,
    /// The invitee's role/rank flags for a raid-signup event (`CalendarModRank`-adjacent) — 0 for
    /// an ordinary invite.
    pub rank: u8,
    /// `true` only when the event this invite names is a guild event AND we can currently resolve
    /// it (the server folds "not found" into `false` too, same as [`Self::creator_or_sender`]
    /// falling back to the invite's sender).
    pub is_guild_event: bool,
    /// The event's creator guid if the event resolves server-side, else the invite's own sender —
    /// the two are mutually exclusive on the wire (AzerothCore's own `if (event) {…} else {…}`),
    /// folded into one field since nothing here can tell which branch fired after the fact and the
    /// UI only ever wants "who to show".
    pub creator_or_sender: u64,
}

/// One row of the event section — an event WE hold (created, or a guild event our own guild
/// holds), summary-only: title/type/time/flags/dungeon/creator, no invite list or description
/// (those need the per-event `CMSG_CALENDAR_GET_EVENT` round trip this pass does not implement).
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEvent {
    pub event_id: u64,
    pub title: String,
    /// `CalendarEventType` (VERIFIED AzerothCore `Calendar.h`): 0 raid, 1 dungeon, 2 pvp, 3
    /// meeting, 4 other, 5 heroic.
    pub event_type: u32,
    pub event_time: PackedTime,
    /// `CalendarFlags` bits — `CALENDAR_FLAG_ALL_ALLOWED`(0x1), `_INVITES_LOCKED`(0x10),
    /// `_WITHOUT_INVITES`(0x20), `_GUILD_EVENT`(0x40) among them (VERIFIED AzerothCore
    /// `Calendar.h`).
    pub flags: u32,
    /// `-1` when the event names no dungeon.
    pub dungeon_id: i32,
    pub creator_guid: u64,
}

/// One bound-instance row — a raid/dungeon save this character personally holds, with its reset
/// countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarBoundInstance {
    pub map_id: u32,
    pub difficulty: u32,
    /// Seconds until reset from the server's `currTime` at send time — `0` already-resettable
    /// (AzerothCore clamps a past reset time to `0` rather than sending a negative).
    pub reset_in_secs: u32,
    /// The instance save's own guid (`HighGuid::Instance`) — a RAW 8-byte value on this row
    /// specifically (AzerothCore's plain `data << guid`, not `WriteAsPacked()`; verified against
    /// `ObjectGuid.cpp:70-74`'s own operator, unlike every other guid in this packet).
    pub instance_guid: u64,
}

/// One weekly-reset-rule row — the realm's own reset schedule for a raid map, independent of
/// whether this character has ever entered it (used to compute "next Tuesday" style countdowns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarResetRule {
    pub map_id: i32,
    pub period_secs: i32,
    /// Always `0` on AzerothCore (`Handlers/CalendarHandler.cpp`'s own comment: "needed for other
    /// than 7-day periods if not aligned with relationTime" — never populated).
    pub offset_secs: i32,
}

/// One active-holiday row (`Holidays.dbc`) — a realm-wide calendar entry (Darkmoon Faire, a
/// world event) rather than anything player-specific.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarHoliday {
    pub id: u32,
    pub region: u32,
    pub looping: u32,
    pub priority: u32,
    pub calendar_filter_type: u32,
    /// `Holidays.dbc` `Date[26]` — up to 26 occurrence start times (packed-time-shaped u32s, per
    /// the DBC's own field type, but read raw here: nothing in this pass decodes them, the day
    /// grid only needs to know a holiday is active, not enumerate every future occurrence).
    pub dates: [u32; 26],
    pub durations: [u32; 10],
    pub calendar_flags: [u32; 10],
    /// `Holidays.dbc`'s texture filename doubles as the holiday's display name on this wire
    /// (AzerothCore's own comment: "m_textureFilename (holiday name)").
    pub name: String,
}

/// The whole `SMSG_CALENDAR_SEND_CALENDAR` payload — everything [`super::opcode::
/// CMSG_CALENDAR_GET_CALENDAR`] asks for, in one packet.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarSendCalendar {
    pub invites: Vec<CalendarInvite>,
    pub events: Vec<CalendarEvent>,
    /// Raw server unix time at send — the anchor [`crate::wire`]'s own `unix`-based callers can
    /// use directly, no packed-time unwrap needed for "what time is it right now".
    pub server_time: u32,
    pub zone_time: PackedTime,
    pub bound_instances: Vec<CalendarBoundInstance>,
    /// The base epoch (plus the realm's configured reset hour) every weekly [`CalendarResetRule`]
    /// period aligns to — AzerothCore's `relationTime`, default 2005-12-29 + `CONFIG_INSTANCE_
    /// RESET_TIME_HOUR`.
    pub relation_time: u32,
    pub reset_rules: Vec<CalendarResetRule>,
    pub holidays: Vec<CalendarHoliday>,
}

/// Body of [`super::opcode::CMSG_CALENDAR_GET_CALENDAR`] — empty (VERIFIED AzerothCore
/// `HandleCalendarGetCalendar(WorldPacket& /*recvData*/)`'s own unused parameter; a real client
/// sends nothing else, per `calendar.md`'s native trace of `OpenCalendar`).
pub fn calendar_get_calendar() -> Vec<u8> {
    Vec::new()
}

/// Read [`super::opcode::SMSG_CALENDAR_SEND_CALENDAR`] — see this module's own doc for the
/// verification trail. Section order, verbatim: invite count + invites, event count + events,
/// server time (`u32`) + zone time (packed `u32`), bound-instance count + rows, `relationTime`
/// (`u32`), reset-rule count + rows, holiday count + rows.
pub fn read_send_calendar(r: &mut &[u8]) -> io::Result<CalendarSendCalendar> {
    let invite_count = read_u32_le(r)?;
    let mut invites = Vec::with_capacity(capacity_hint(invite_count, 1024));
    for _ in 0..invite_count {
        let event_id = read_u64_le(r)?;
        let invite_id = read_u64_le(r)?;
        let status = read_u8(r)?;
        let rank = read_u8(r)?;
        let is_guild_event = read_u8(r)? != 0;
        let creator_or_sender = read_packed_guid(r)?;
        invites.push(CalendarInvite {
            event_id,
            invite_id,
            status,
            rank,
            is_guild_event,
            creator_or_sender,
        });
    }

    let event_count = read_u32_le(r)?;
    let mut events = Vec::with_capacity(capacity_hint(event_count, 1024));
    for _ in 0..event_count {
        events.push(CalendarEvent {
            event_id: read_u64_le(r)?,
            title: read_cstring(r)?,
            event_type: read_u32_le(r)?,
            event_time: read_packed_time(r)?,
            flags: read_u32_le(r)?,
            dungeon_id: read_u32_le(r)? as i32,
            creator_guid: read_packed_guid(r)?,
        });
    }

    let server_time = read_u32_le(r)?;
    let zone_time = read_packed_time(r)?;

    let bound_count = read_u32_le(r)?;
    let mut bound_instances = Vec::with_capacity(capacity_hint(bound_count, 1024));
    for _ in 0..bound_count {
        bound_instances.push(CalendarBoundInstance {
            map_id: read_u32_le(r)?,
            difficulty: read_u32_le(r)?,
            reset_in_secs: read_u32_le(r)?,
            instance_guid: read_u64_le(r)?, // RAW, not packed — see the field's own doc.
        });
    }

    let relation_time = read_u32_le(r)?;

    let reset_rule_count = read_u32_le(r)?;
    let mut reset_rules = Vec::with_capacity(capacity_hint(reset_rule_count, 1024));
    for _ in 0..reset_rule_count {
        reset_rules.push(CalendarResetRule {
            map_id: read_u32_le(r)? as i32,
            period_secs: read_u32_le(r)? as i32,
            offset_secs: read_u32_le(r)? as i32,
        });
    }

    let holiday_count = read_u32_le(r)?;
    let mut holidays = Vec::with_capacity(capacity_hint(holiday_count, 1024));
    for _ in 0..holiday_count {
        let id = read_u32_le(r)?;
        let region = read_u32_le(r)?;
        let looping = read_u32_le(r)?;
        let priority = read_u32_le(r)?;
        let calendar_filter_type = read_u32_le(r)?;
        let mut dates = [0u32; 26];
        for d in &mut dates {
            *d = read_u32_le(r)?;
        }
        let mut durations = [0u32; 10];
        for d in &mut durations {
            *d = read_u32_le(r)?;
        }
        let mut calendar_flags = [0u32; 10];
        for f in &mut calendar_flags {
            *f = read_u32_le(r)?;
        }
        let name = read_cstring(r)?;
        holidays.push(CalendarHoliday {
            id,
            region,
            looping,
            priority,
            calendar_filter_type,
            dates,
            durations,
            calendar_flags,
            name,
        });
    }

    Ok(CalendarSendCalendar {
        invites,
        events,
        server_time,
        zone_time,
        bound_instances,
        relation_time,
        reset_rules,
        holidays,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built wire buffer, section by section, matching `HandleCalendarGetCalendar`'s own
    /// write order exactly — the byte-shape oracle for [`read_send_calendar`].
    #[test]
    fn reads_every_section_in_order() {
        let mut w = Vec::new();
        // 1 invite: event 10, invite 20, status 1 (accepted), rank 0, guild event, creator guid
        // 0x1 (packed: mask 0x01, one byte 0x01).
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&10u64.to_le_bytes());
        w.extend_from_slice(&20u64.to_le_bytes());
        w.push(1);
        w.push(0);
        w.push(1); // is_guild_event
        w.push(0x01); // packed guid mask: byte 0 present
        w.push(0x01); // that byte's value

        // 1 event: id 99, title "Naxx", type 0 (raid), packed time, flags 0x40, dungeon -1,
        // creator guid 0x2 (packed).
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&99u64.to_le_bytes());
        w.extend_from_slice(b"Naxx\0");
        w.extend_from_slice(&0u32.to_le_bytes());
        // packed time: 2026-09-29 (Tue=2) 14:37 → year-2000=26, month-1=8, day-1=28.
        let packed =
            (26u32 << 24) | (8u32 << 20) | (28u32 << 14) | (2u32 << 11) | (14u32 << 6) | 37;
        w.extend_from_slice(&packed.to_le_bytes());
        w.extend_from_slice(&0x40u32.to_le_bytes());
        w.extend_from_slice(&(-1i32).to_le_bytes());
        w.push(0x01);
        w.push(0x02);

        // server time + zone time (same packed value reused).
        w.extend_from_slice(&1_700_000_000u32.to_le_bytes());
        w.extend_from_slice(&packed.to_le_bytes());

        // 1 bound instance: map 533, difficulty 0, 3600s left, RAW guid 0xABCD.
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&533u32.to_le_bytes());
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(&3600u32.to_le_bytes());
        w.extend_from_slice(&0xABCDu64.to_le_bytes());

        // relation time.
        w.extend_from_slice(&1_135_814_400u32.to_le_bytes());

        // 1 reset rule: map 533, period 604800 (7 days), offset 0.
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&533i32.to_le_bytes());
        w.extend_from_slice(&604_800i32.to_le_bytes());
        w.extend_from_slice(&0i32.to_le_bytes());

        // 1 holiday: id 141 (Darkmoon Faire), region 0, looping 1, priority 1, filter 1,
        // 26 dates + 10 durations + 10 flags (all 0), name "Darkmoon".
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&141u32.to_le_bytes());
        for v in [0u32, 1, 1, 1] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..(26 + 10 + 10) {
            w.extend_from_slice(&0u32.to_le_bytes());
        }
        w.extend_from_slice(b"Darkmoon\0");

        let mut slice = w.as_slice();
        let cal = read_send_calendar(&mut slice).unwrap();
        assert!(
            slice.is_empty(),
            "the whole buffer was consumed, no trailing bytes"
        );

        assert_eq!(cal.invites.len(), 1);
        assert_eq!(cal.invites[0].event_id, 10);
        assert_eq!(cal.invites[0].invite_id, 20);
        assert_eq!(cal.invites[0].status, 1);
        assert!(cal.invites[0].is_guild_event);
        assert_eq!(cal.invites[0].creator_or_sender, 1);

        assert_eq!(cal.events.len(), 1);
        assert_eq!(cal.events[0].title, "Naxx");
        assert_eq!(cal.events[0].dungeon_id, -1);
        assert_eq!(cal.events[0].creator_guid, 2);
        assert_eq!(cal.events[0].event_time.year, 2026);
        assert_eq!(cal.events[0].event_time.day, 29);

        assert_eq!(cal.server_time, 1_700_000_000);
        assert_eq!(cal.zone_time.hour, 14);

        assert_eq!(cal.bound_instances.len(), 1);
        assert_eq!(cal.bound_instances[0].map_id, 533);
        assert_eq!(cal.bound_instances[0].reset_in_secs, 3600);
        assert_eq!(cal.bound_instances[0].instance_guid, 0xABCD);

        assert_eq!(cal.relation_time, 1_135_814_400);

        assert_eq!(cal.reset_rules.len(), 1);
        assert_eq!(cal.reset_rules[0].period_secs, 604_800);

        assert_eq!(cal.holidays.len(), 1);
        assert_eq!(cal.holidays[0].name, "Darkmoon");
        assert_eq!(cal.holidays[0].id, 141);
    }

    #[test]
    fn get_calendar_body_is_empty() {
        assert!(calendar_get_calendar().is_empty());
    }

    /// An empty calendar (no invites, no events, no saves, no holidays) reads clean — the shape
    /// every fresh account/character hits first.
    #[test]
    fn reads_a_fully_empty_calendar() {
        let mut w = Vec::new();
        w.extend_from_slice(&0u32.to_le_bytes()); // invite count
        w.extend_from_slice(&0u32.to_le_bytes()); // event count
        w.extend_from_slice(&1_700_000_000u32.to_le_bytes()); // server time
        w.extend_from_slice(&0u32.to_le_bytes()); // zone time (midnight 2000-01-01, Sat)
        w.extend_from_slice(&0u32.to_le_bytes()); // bound-instance count
        w.extend_from_slice(&0u32.to_le_bytes()); // relation time
        w.extend_from_slice(&0u32.to_le_bytes()); // reset-rule count
        w.extend_from_slice(&0u32.to_le_bytes()); // holiday count

        let mut slice = w.as_slice();
        let cal = read_send_calendar(&mut slice).unwrap();
        assert!(slice.is_empty());
        assert!(cal.invites.is_empty());
        assert!(cal.events.is_empty());
        assert!(cal.bound_instances.is_empty());
        assert!(cal.reset_rules.is_empty());
        assert!(cal.holidays.is_empty());
    }
}
