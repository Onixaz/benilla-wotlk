//! The calendar's messages, as 12340's `PH_CALENDAR_GROUP` (`0x5c3fe0`) and
//! `Packet_SMSG_CALENDAR_COMMAND_RESULT` (`0x6cd380`) read them, and the bodies the calendar's
//! verbs send. A date on the wire is a packed `WowTime` word; `server_now` is unix seconds.

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_i32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8,
    write_packed_guid,
};

/// A pending invite of `SMSG_CALENDAR_SEND_CALENDAR`'s first list (`0x5c29c0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteRef {
    pub event_id: u64,
    pub invite_id: u64,
    pub status: u8,
    pub rank: u8,
    /// 1 for a guild event's sign-up.
    pub invite_type: u8,
    pub inviter: u64,
}

/// One event of `SMSG_CALENDAR_SEND_CALENDAR`'s event list (`0x5c29c0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventRow {
    pub event_id: u64,
    pub title: String,
    /// Raid, dungeon, PvP, meeting or other, 0-based.
    pub event_type: i32,
    pub start: u32,
    /// The calendar flags: `0x1` player, `0x10` locked, `0x40` announcement, `0x400` guild event.
    pub flags: u32,
    /// The `LfgDungeons.dbc` row whose texture the event shows, -1 none.
    pub texture: i32,
    pub creator: u64,
}

/// One raid lockout: `reset` is seconds from the packet's base time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarLockout {
    pub map: i32,
    pub difficulty: i32,
    pub reset: i32,
    pub instance_id: u64,
}

/// One raid's reset cycle: every `period` seconds from the base time plus `offset`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarReset {
    pub map: i32,
    pub period: u32,
    pub offset: u32,
}

/// A server-side `Holidays.dbc` row that replaces the client's (`0x5c29c0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarHolidayRow {
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

/// `SMSG_CALENDAR_SEND_CALENDAR` (`0x5c29c0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarSnapshot {
    pub invites: Vec<CalendarInviteRef>,
    pub events: Vec<CalendarEventRow>,
    /// The server's unix time.
    pub server_now: u32,
    /// The server's packed date, the lockouts' base.
    pub server_date: u32,
    pub lockouts: Vec<CalendarLockout>,
    /// The unix time the reset cycles count from.
    pub reset_base: u32,
    pub resets: Vec<CalendarReset>,
    pub holidays: Vec<CalendarHolidayRow>,
}

/// One invitee of `SMSG_CALENDAR_SEND_EVENT` (`0x5f2c00`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInvitee {
    pub guid: u64,
    pub level: u8,
    pub status: u8,
    /// 0 none, 1 moderator, 2 creator.
    pub moderator: u8,
    pub invite_type: u8,
    pub invite_id: u64,
    pub response_time: u32,
    pub notes: String,
}

/// `SMSG_CALENDAR_SEND_EVENT` (`0x5bed20`, `0x5f2c00`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventDetail {
    /// 0 a fetch, 1 an add, 2 a copy.
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
    pub invites: Vec<CalendarInvitee>,
}

/// `SMSG_CALENDAR_EVENT_INVITE` (`0x5bf030`): the third byte is 1 when a response time follows,
/// and the client keeps it as the invite type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteAdded {
    pub guid: u64,
    pub event_id: u64,
    pub invite_id: u64,
    pub level: u8,
    pub status: u8,
    pub invite_type: u8,
    pub response_time: u32,
    pub clear_pending: bool,
}

/// `SMSG_CALENDAR_EVENT_STATUS` (`0x5bf420`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarStatusUpdate {
    pub guid: u64,
    pub event_id: u64,
    pub date: u32,
    pub flags: u32,
    pub status: u8,
    pub clear_pending: bool,
    pub response_time: u32,
}

/// `SMSG_CALENDAR_EVENT_INVITE_ALERT` (`0x5c3ab0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarInviteAlert {
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

/// `SMSG_CALENDAR_EVENT_UPDATED_ALERT` (`0x5c3d30`, its tail `0x5f14b0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventUpdate {
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

/// `SMSG_CALENDAR_COMMAND_RESULT` (`0x6cd380`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarCommandResult {
    pub command: u32,
    pub event_name: String,
    pub name: String,
    pub result: u32,
}

/// Every calendar message, in the shapes the reference reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarPacket {
    SendCalendar(Box<CalendarSnapshot>),
    SendEvent(Box<CalendarEventDetail>),
    /// `SMSG_CALENDAR_FILTER_GUILD` or `SMSG_CALENDAR_ARENA_TEAM` (`0x5f2f90`): `(guid, level)`.
    InitialInvites(Vec<(u64, u8)>),
    Invite(CalendarInviteAdded),
    /// `SMSG_CALENDAR_EVENT_INVITE_REMOVED` (`0x5bf250`).
    InviteRemoved {
        guid: u64,
        event_id: u64,
        flags: u32,
        clear_pending: bool,
    },
    Status(CalendarStatusUpdate),
    CommandResult(CalendarCommandResult),
    /// `SMSG_CALENDAR_RAID_LOCKOUT_ADDED` (`0x5c37d0`): the base date leads.
    LockoutAdded {
        date: u32,
        lockout: CalendarLockout,
    },
    /// `SMSG_CALENDAR_RAID_LOCKOUT_REMOVED` (`0x5c07e0`): a negative map clears every lockout and
    /// carries nothing more.
    LockoutRemoved {
        map: i32,
        lockout: Option<CalendarLockout>,
    },
    /// `SMSG_CALENDAR_RAID_LOCKOUT_UPDATED` (`0x5c3880`): an extended lockout's move.
    LockoutUpdated {
        date: u32,
        map: i32,
        difficulty: i32,
        old_reset: i32,
        new_reset: i32,
    },
    InviteAlert(CalendarInviteAlert),
    /// `SMSG_CALENDAR_EVENT_INVITE_REMOVED_ALERT` (`0x5c0ae0`).
    InviteRemovedAlert {
        event_id: u64,
        date: u32,
        flags: u32,
        status: u8,
    },
    /// `SMSG_CALENDAR_EVENT_INVITE_STATUS_ALERT` (`0x5bf590`).
    InviteStatusAlert {
        event_id: u64,
        date: u32,
        flags: u32,
        status: u8,
    },
    /// `SMSG_CALENDAR_EVENT_REMOVED_ALERT` (`0x5c0cf0`).
    EventRemovedAlert {
        clear_pending: bool,
        event_id: u64,
        date: u32,
    },
    EventUpdatedAlert(Box<CalendarEventUpdate>),
    /// `SMSG_CALENDAR_EVENT_MODERATOR_STATUS_ALERT` (`0x5c0e70`).
    ModeratorStatusAlert {
        guid: u64,
        event_id: u64,
        status: u8,
        clear_pending: bool,
    },
    /// `SMSG_CALENDAR_SEND_NUM_PENDING` (`0x5b7ce0`).
    NumPending(u32),
    /// `SMSG_CALENDAR_EVENT_INVITE_NOTES` (`0x5bbc20`).
    InviteNotes {
        guid: u64,
        event_id: u64,
        notes: String,
        clear_pending: bool,
    },
    /// `SMSG_CALENDAR_EVENT_INVITE_NOTES_ALERT` (`0x5bbcd0`): the player's own notes.
    InviteNotesAlert {
        event_id: u64,
        notes: String,
    },
    /// `SMSG_CALENDAR_CLEAR_PENDING_ACTION`, empty.
    ClearPendingAction,
}

impl CalendarPacket {
    /// The opcode's name, for the packet trace.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SendCalendar(_) => "SMSG_CALENDAR_SEND_CALENDAR",
            Self::SendEvent(_) => "SMSG_CALENDAR_SEND_EVENT",
            Self::InitialInvites(_) => "SMSG_CALENDAR_FILTER_GUILD",
            Self::Invite(_) => "SMSG_CALENDAR_EVENT_INVITE",
            Self::InviteRemoved { .. } => "SMSG_CALENDAR_EVENT_INVITE_REMOVED",
            Self::Status(_) => "SMSG_CALENDAR_EVENT_STATUS",
            Self::CommandResult(_) => "SMSG_CALENDAR_COMMAND_RESULT",
            Self::LockoutAdded { .. } => "SMSG_CALENDAR_RAID_LOCKOUT_ADDED",
            Self::LockoutRemoved { .. } => "SMSG_CALENDAR_RAID_LOCKOUT_REMOVED",
            Self::LockoutUpdated { .. } => "SMSG_CALENDAR_RAID_LOCKOUT_UPDATED",
            Self::InviteAlert(_) => "SMSG_CALENDAR_EVENT_INVITE_ALERT",
            Self::InviteRemovedAlert { .. } => "SMSG_CALENDAR_EVENT_INVITE_REMOVED_ALERT",
            Self::InviteStatusAlert { .. } => "SMSG_CALENDAR_EVENT_INVITE_STATUS_ALERT",
            Self::EventRemovedAlert { .. } => "SMSG_CALENDAR_EVENT_REMOVED_ALERT",
            Self::EventUpdatedAlert(_) => "SMSG_CALENDAR_EVENT_UPDATED_ALERT",
            Self::ModeratorStatusAlert { .. } => "SMSG_CALENDAR_EVENT_MODERATOR_STATUS_ALERT",
            Self::NumPending(_) => "SMSG_CALENDAR_SEND_NUM_PENDING",
            Self::InviteNotes { .. } => "SMSG_CALENDAR_EVENT_INVITE_NOTES",
            Self::InviteNotesAlert { .. } => "SMSG_CALENDAR_EVENT_INVITE_NOTES_ALERT",
            Self::ClearPendingAction => "SMSG_CALENDAR_CLEAR_PENDING_ACTION",
        }
    }
}

/// The most rows a calendar list preallocates for; the counts are the server's.
const LIST_HINT: usize = 256;

fn flag(r: &mut &[u8]) -> io::Result<bool> {
    Ok(read_u8(r)? != 0)
}

fn read_lockout(r: &mut &[u8]) -> io::Result<CalendarLockout> {
    Ok(CalendarLockout {
        map: read_i32_le(r)?,
        difficulty: read_i32_le(r)?,
        reset: read_i32_le(r)?,
        instance_id: read_u64_le(r)?,
    })
}

fn read_i32s<const N: usize>(r: &mut &[u8]) -> io::Result<[i32; N]> {
    let mut out = [0; N];
    for v in &mut out {
        *v = read_i32_le(r)?;
    }
    Ok(out)
}

/// `SMSG_CALENDAR_SEND_CALENDAR` (`0x5c29c0`): the invites, the events, the two clocks, the
/// lockouts, the reset cycles and the holiday overrides, in that order.
pub(super) fn read_send_calendar(r: &mut &[u8]) -> io::Result<CalendarSnapshot> {
    let n = read_u32_le(r)?;
    let mut invites = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        invites.push(CalendarInviteRef {
            event_id: read_u64_le(r)?,
            invite_id: read_u64_le(r)?,
            status: read_u8(r)?,
            rank: read_u8(r)?,
            invite_type: read_u8(r)?,
            inviter: read_packed_guid(r)?,
        });
    }
    let n = read_u32_le(r)?;
    let mut events = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        events.push(CalendarEventRow {
            event_id: read_u64_le(r)?,
            title: read_cstring(r)?,
            event_type: read_i32_le(r)?,
            start: read_u32_le(r)?,
            flags: read_u32_le(r)?,
            texture: read_i32_le(r)?,
            creator: read_packed_guid(r)?,
        });
    }
    let server_now = read_u32_le(r)?;
    let server_date = read_u32_le(r)?;
    let n = read_u32_le(r)?;
    let mut lockouts = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        lockouts.push(read_lockout(r)?);
    }
    let reset_base = read_u32_le(r)?;
    let n = read_u32_le(r)?;
    let mut resets = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        resets.push(CalendarReset {
            map: read_i32_le(r)?,
            period: read_u32_le(r)?,
            offset: read_u32_le(r)?,
        });
    }
    let n = read_u32_le(r)?;
    let mut holidays = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        holidays.push(CalendarHolidayRow {
            id: read_u32_le(r)?,
            region: read_i32_le(r)?,
            looping: read_i32_le(r)?,
            priority: read_i32_le(r)?,
            filter_type: read_i32_le(r)?,
            dates: read_i32s(r)?,
            durations: read_i32s(r)?,
            flags: read_i32s(r)?,
            texture: read_cstring(r)?,
        });
    }
    Ok(CalendarSnapshot {
        invites,
        events,
        server_now,
        server_date,
        lockouts,
        reset_base,
        resets,
        holidays,
    })
}

/// `SMSG_CALENDAR_SEND_EVENT` (`0x5bed20`): the send type, then the event (`0x5f2c00`).
pub(super) fn read_send_event(r: &mut &[u8]) -> io::Result<CalendarEventDetail> {
    let send_type = read_u8(r)?;
    let creator = read_packed_guid(r)?;
    let event_id = read_u64_le(r)?;
    let title = read_cstring(r)?;
    let description = read_cstring(r)?;
    let event_type = read_u8(r)?;
    let repeat = read_u8(r)?;
    let max_size = read_u32_le(r)?;
    let texture = read_i32_le(r)?;
    let flags = read_u32_le(r)?;
    let date = read_u32_le(r)?;
    let lockout_date = read_u32_le(r)?;
    let guild_id = read_u32_le(r)?;
    let n = read_u32_le(r)?;
    let mut invites = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        invites.push(CalendarInvitee {
            guid: read_packed_guid(r)?,
            level: read_u8(r)?,
            status: read_u8(r)?,
            moderator: read_u8(r)?,
            invite_type: read_u8(r)?,
            invite_id: read_u64_le(r)?,
            response_time: read_u32_le(r)?,
            notes: read_cstring(r)?,
        });
    }
    Ok(CalendarEventDetail {
        send_type,
        creator,
        event_id,
        title,
        description,
        event_type,
        repeat,
        max_size,
        texture,
        flags,
        date,
        lockout_date,
        guild_id,
        invites,
    })
}

/// `SMSG_CALENDAR_FILTER_GUILD` and `SMSG_CALENDAR_ARENA_TEAM` (`0x5f2f90`): `u32` count, then
/// `(packed guid, u8 level)` each.
pub(super) fn read_initial_invites(r: &mut &[u8]) -> io::Result<Vec<(u64, u8)>> {
    let n = read_u32_le(r)?;
    let mut out = Vec::with_capacity(capacity_hint(n, LIST_HINT));
    for _ in 0..n {
        out.push((read_packed_guid(r)?, read_u8(r)?));
    }
    Ok(out)
}

/// `SMSG_CALENDAR_EVENT_INVITE` (`0x5bf030`).
pub(super) fn read_invite(r: &mut &[u8]) -> io::Result<CalendarInviteAdded> {
    let guid = read_packed_guid(r)?;
    let event_id = read_u64_le(r)?;
    let invite_id = read_u64_le(r)?;
    let level = read_u8(r)?;
    let status = read_u8(r)?;
    let invite_type = read_u8(r)?;
    let response_time = if invite_type == 1 { read_u32_le(r)? } else { 0 };
    Ok(CalendarInviteAdded {
        guid,
        event_id,
        invite_id,
        level,
        status,
        invite_type,
        response_time,
        clear_pending: flag(r)?,
    })
}

pub(super) fn read_invite_removed(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::InviteRemoved {
        guid: read_packed_guid(r)?,
        event_id: read_u64_le(r)?,
        flags: read_u32_le(r)?,
        clear_pending: flag(r)?,
    })
}

/// `SMSG_CALENDAR_EVENT_STATUS` (`0x5bf420`).
pub(super) fn read_status(r: &mut &[u8]) -> io::Result<CalendarStatusUpdate> {
    Ok(CalendarStatusUpdate {
        guid: read_packed_guid(r)?,
        event_id: read_u64_le(r)?,
        date: read_u32_le(r)?,
        flags: read_u32_le(r)?,
        status: read_u8(r)?,
        clear_pending: flag(r)?,
        response_time: read_u32_le(r)?,
    })
}

/// `SMSG_CALENDAR_COMMAND_RESULT` (`0x6cd380`): `u32` command, the event's and a player's name,
/// `u32` result.
pub(super) fn read_command_result(r: &mut &[u8]) -> io::Result<CalendarCommandResult> {
    Ok(CalendarCommandResult {
        command: read_u32_le(r)?,
        event_name: read_cstring(r)?,
        name: read_cstring(r)?,
        result: read_u32_le(r)?,
    })
}

pub(super) fn read_lockout_added(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::LockoutAdded {
        date: read_u32_le(r)?,
        lockout: read_lockout(r)?,
    })
}

pub(super) fn read_lockout_removed(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    let map = read_i32_le(r)?;
    let lockout = if map < 0 {
        None
    } else {
        Some(CalendarLockout {
            map,
            difficulty: read_i32_le(r)?,
            reset: read_i32_le(r)?,
            instance_id: read_u64_le(r)?,
        })
    };
    Ok(CalendarPacket::LockoutRemoved { map, lockout })
}

pub(super) fn read_lockout_updated(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::LockoutUpdated {
        date: read_u32_le(r)?,
        map: read_i32_le(r)?,
        difficulty: read_i32_le(r)?,
        old_reset: read_i32_le(r)?,
        new_reset: read_i32_le(r)?,
    })
}

/// `SMSG_CALENDAR_EVENT_INVITE_ALERT` (`0x5c3ab0`).
pub(super) fn read_invite_alert(r: &mut &[u8]) -> io::Result<CalendarInviteAlert> {
    Ok(CalendarInviteAlert {
        event_id: read_u64_le(r)?,
        title: read_cstring(r)?,
        start: read_u32_le(r)?,
        flags: read_u32_le(r)?,
        event_type: read_i32_le(r)?,
        texture: read_i32_le(r)?,
        invite_id: read_u64_le(r)?,
        status: read_u8(r)?,
        rank: read_u8(r)?,
        creator: read_packed_guid(r)?,
        inviter: read_packed_guid(r)?,
    })
}

pub(super) fn read_invite_removed_alert(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::InviteRemovedAlert {
        event_id: read_u64_le(r)?,
        date: read_u32_le(r)?,
        flags: read_u32_le(r)?,
        status: read_u8(r)?,
    })
}

pub(super) fn read_invite_status_alert(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::InviteStatusAlert {
        event_id: read_u64_le(r)?,
        date: read_u32_le(r)?,
        flags: read_u32_le(r)?,
        status: read_u8(r)?,
    })
}

pub(super) fn read_event_removed_alert(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::EventRemovedAlert {
        clear_pending: flag(r)?,
        event_id: read_u64_le(r)?,
        date: read_u32_le(r)?,
    })
}

/// `SMSG_CALENDAR_EVENT_UPDATED_ALERT` (`0x5c3d30`): the client reads the head, and re-reads
/// from the flags on for the event it holds open (`0x5f14b0`).
pub(super) fn read_event_updated_alert(r: &mut &[u8]) -> io::Result<CalendarEventUpdate> {
    Ok(CalendarEventUpdate {
        clear_pending: flag(r)?,
        event_id: read_u64_le(r)?,
        old_date: read_u32_le(r)?,
        flags: read_u32_le(r)?,
        date: read_u32_le(r)?,
        event_type: read_u8(r)?,
        texture: read_i32_le(r)?,
        title: read_cstring(r)?,
        description: read_cstring(r)?,
        repeat: read_u8(r)?,
        max_size: read_u32_le(r)?,
        lockout_date: read_u32_le(r)?,
    })
}

pub(super) fn read_moderator_status_alert(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::ModeratorStatusAlert {
        guid: read_packed_guid(r)?,
        event_id: read_u64_le(r)?,
        status: read_u8(r)?,
        clear_pending: flag(r)?,
    })
}

pub(super) fn read_invite_notes(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::InviteNotes {
        guid: read_packed_guid(r)?,
        event_id: read_u64_le(r)?,
        notes: read_cstring(r)?,
        clear_pending: flag(r)?,
    })
}

pub(super) fn read_invite_notes_alert(r: &mut &[u8]) -> io::Result<CalendarPacket> {
    Ok(CalendarPacket::InviteNotesAlert {
        event_id: read_u64_le(r)?,
        notes: read_cstring(r)?,
    })
}

/// A new event's invitee, as `CMSG_CALENDAR_ADD_EVENT` carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarNewInvite {
    pub guid: u64,
    pub status: u8,
    pub moderator: u8,
}

/// The event an add or an update sends, `CGCalendarEvent`'s fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarEventFields {
    pub title: String,
    pub description: String,
    pub event_type: u8,
    pub repeat: u8,
    pub max_size: u32,
    pub texture: i32,
    pub date: u32,
    pub lockout_date: u32,
    pub flags: u32,
}

/// One calendar send, each as its 12340 sender writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarCommand {
    /// `CMSG_CALENDAR_GET_CALENDAR` (`0x5bbb40`), empty.
    GetCalendar,
    /// `CMSG_CALENDAR_GET_NUM_PENDING` (`0x5bbbb0`), empty.
    GetNumPending,
    /// `CMSG_CALENDAR_GET_EVENT` (`0x5b9310`).
    GetEvent { event_id: u64 },
    /// `CMSG_CALENDAR_GUILD_FILTER` (`0x5bc160`).
    GuildFilter {
        min_level: u32,
        max_level: u32,
        min_rank: u32,
    },
    /// `CMSG_CALENDAR_ARENA_TEAM` (`0x5bc240`).
    ArenaTeam { team_id: u32 },
    /// `CMSG_CALENDAR_ADD_EVENT` (`0x5f1b90`): the invitee list goes unless the event is an
    /// announcement.
    AddEvent {
        event: CalendarEventFields,
        invites: Option<Vec<CalendarNewInvite>>,
    },
    /// `CMSG_CALENDAR_UPDATE_EVENT` (`0x5f1ef0`).
    UpdateEvent {
        event_id: u64,
        moderator_invite: u64,
        event: CalendarEventFields,
    },
    /// `CMSG_CALENDAR_REMOVE_EVENT` from the editor (`0x5f1e30`): a `u8` guild flag trails.
    RemoveEvent {
        event_id: u64,
        moderator_invite: u64,
        guild: bool,
    },
    /// `CMSG_CALENDAR_REMOVE_EVENT` from the day menu (`0x5bc6d0`): the `u32` flags trail.
    RemoveDayEvent {
        event_id: u64,
        invite_id: u64,
        flags: u32,
    },
    /// `CMSG_CALENDAR_COPY_EVENT` (`0x5bc870`).
    CopyEvent {
        event_id: u64,
        invite_id: u64,
        date: u32,
    },
    /// `CMSG_CALENDAR_EVENT_INVITE` (`0x5f2070`).
    Invite {
        event_id: u64,
        moderator_invite: u64,
        name: String,
        pre_invite: bool,
        guild_event: bool,
    },
    /// `CMSG_CALENDAR_EVENT_RSVP` (`0x5f23f0`, `0x5b9860`).
    Rsvp {
        event_id: u64,
        invite_id: u64,
        status: u32,
    },
    /// `CMSG_CALENDAR_EVENT_REMOVE_INVITE` (`0x5f2220`, `0x5bc4d0`).
    RemoveInvite {
        guid: u64,
        invite_id: u64,
        moderator_invite: u64,
        event_id: u64,
    },
    /// `CMSG_CALENDAR_EVENT_STATUS` (`0x5f2750`).
    SetStatus {
        guid: u64,
        event_id: u64,
        invite_id: u64,
        moderator_invite: u64,
        status: u32,
    },
    /// `CMSG_CALENDAR_EVENT_MODERATOR_STATUS` (`0x5f2860`).
    SetModerator {
        guid: u64,
        event_id: u64,
        invite_id: u64,
        moderator_invite: u64,
        status: u32,
    },
    /// `CMSG_CALENDAR_COMPLAIN` (`0x5bca50`).
    Complain {
        creator: u64,
        event_id: u64,
        invite_id: u64,
    },
    /// `CMSG_CALENDAR_EVENT_SIGN_UP` (`0x5f2530`, `0x5bc5e0`).
    SignUp { event_id: u64, tentative: bool },
}

fn put_cstring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn put_packed_guid(out: &mut Vec<u8>, guid: u64) {
    write_packed_guid(guid, out).expect("a Vec takes every write");
}

fn put_event(out: &mut Vec<u8>, e: &CalendarEventFields) {
    put_cstring(out, &e.title);
    put_cstring(out, &e.description);
    out.push(e.event_type);
    out.push(e.repeat);
    out.extend_from_slice(&e.max_size.to_le_bytes());
    out.extend_from_slice(&e.texture.to_le_bytes());
    out.extend_from_slice(&e.date.to_le_bytes());
    out.extend_from_slice(&e.lockout_date.to_le_bytes());
    out.extend_from_slice(&e.flags.to_le_bytes());
}

impl CalendarCommand {
    /// The opcode and body.
    pub fn encode(&self) -> (u16, Vec<u8>) {
        use super::opcode as op;
        let mut b = Vec::with_capacity(32);
        let opcode = match self {
            Self::GetCalendar => op::CMSG_CALENDAR_GET_CALENDAR,
            Self::GetNumPending => op::CMSG_CALENDAR_GET_NUM_PENDING,
            Self::GetEvent { event_id } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                op::CMSG_CALENDAR_GET_EVENT
            }
            Self::GuildFilter {
                min_level,
                max_level,
                min_rank,
            } => {
                for v in [min_level, max_level, min_rank] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                op::CMSG_CALENDAR_GUILD_FILTER
            }
            Self::ArenaTeam { team_id } => {
                b.extend_from_slice(&team_id.to_le_bytes());
                op::CMSG_CALENDAR_ARENA_TEAM
            }
            Self::AddEvent { event, invites } => {
                put_event(&mut b, event);
                if let Some(invites) = invites {
                    b.extend_from_slice(&(invites.len() as u32).to_le_bytes());
                    for i in invites {
                        put_packed_guid(&mut b, i.guid);
                        b.push(i.status);
                        b.push(i.moderator);
                    }
                }
                op::CMSG_CALENDAR_ADD_EVENT
            }
            Self::UpdateEvent {
                event_id,
                moderator_invite,
                event,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&moderator_invite.to_le_bytes());
                put_event(&mut b, event);
                op::CMSG_CALENDAR_UPDATE_EVENT
            }
            Self::RemoveEvent {
                event_id,
                moderator_invite,
                guild,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&moderator_invite.to_le_bytes());
                b.push(u8::from(*guild));
                op::CMSG_CALENDAR_REMOVE_EVENT
            }
            Self::RemoveDayEvent {
                event_id,
                invite_id,
                flags,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&invite_id.to_le_bytes());
                b.extend_from_slice(&flags.to_le_bytes());
                op::CMSG_CALENDAR_REMOVE_EVENT
            }
            Self::CopyEvent {
                event_id,
                invite_id,
                date,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&invite_id.to_le_bytes());
                b.extend_from_slice(&date.to_le_bytes());
                op::CMSG_CALENDAR_COPY_EVENT
            }
            Self::Invite {
                event_id,
                moderator_invite,
                name,
                pre_invite,
                guild_event,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&moderator_invite.to_le_bytes());
                put_cstring(&mut b, name);
                b.push(u8::from(*pre_invite));
                b.push(u8::from(*guild_event));
                op::CMSG_CALENDAR_EVENT_INVITE
            }
            Self::Rsvp {
                event_id,
                invite_id,
                status,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.extend_from_slice(&invite_id.to_le_bytes());
                b.extend_from_slice(&status.to_le_bytes());
                op::CMSG_CALENDAR_EVENT_RSVP
            }
            Self::RemoveInvite {
                guid,
                invite_id,
                moderator_invite,
                event_id,
            } => {
                put_packed_guid(&mut b, *guid);
                for v in [invite_id, moderator_invite, event_id] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                op::CMSG_CALENDAR_EVENT_REMOVE_INVITE
            }
            Self::SetStatus {
                guid,
                event_id,
                invite_id,
                moderator_invite,
                status,
            }
            | Self::SetModerator {
                guid,
                event_id,
                invite_id,
                moderator_invite,
                status,
            } => {
                put_packed_guid(&mut b, *guid);
                for v in [event_id, invite_id, moderator_invite] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                b.extend_from_slice(&status.to_le_bytes());
                if matches!(self, Self::SetStatus { .. }) {
                    op::CMSG_CALENDAR_EVENT_STATUS
                } else {
                    op::CMSG_CALENDAR_EVENT_MODERATOR_STATUS
                }
            }
            Self::Complain {
                creator,
                event_id,
                invite_id,
            } => {
                for v in [creator, event_id, invite_id] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                op::CMSG_CALENDAR_COMPLAIN
            }
            Self::SignUp {
                event_id,
                tentative,
            } => {
                b.extend_from_slice(&event_id.to_le_bytes());
                b.push(u8::from(*tentative));
                op::CMSG_CALENDAR_EVENT_SIGN_UP
            }
        };
        (opcode, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(guid: u64) -> Vec<u8> {
        let mut v = Vec::new();
        put_packed_guid(&mut v, guid);
        v
    }

    /// A calendar with one invite, one event, one lockout, one reset and one holiday reads back
    /// field for field, consuming the whole body.
    #[test]
    fn send_calendar_reads_every_list_in_order() {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&9u64.to_le_bytes());
        b.extend_from_slice(&[0, 1, 0]);
        b.extend(packed(0x42));
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(b"Raid\0");
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0x1234u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.extend(packed(0x42));
        b.extend_from_slice(&1_000u32.to_le_bytes());
        b.extend_from_slice(&0x5678u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [533i32, 1, 3600] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&5u64.to_le_bytes());
        b.extend_from_slice(&500u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in [533u32, 604_800, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u32.to_le_bytes());
        for v in 0..(5 + 26 + 10 + 10) {
            b.extend_from_slice(&(v as u32).to_le_bytes());
        }
        b.extend_from_slice(b"Calendar_Fair\0");
        let mut r = &b[..];
        let c = read_send_calendar(&mut r).unwrap();
        assert!(r.is_empty(), "the whole body is read");
        assert_eq!(c.invites[0].inviter, 0x42);
        assert_eq!(c.invites[0].rank, 1);
        assert_eq!(c.events[0].title, "Raid");
        assert_eq!(c.events[0].texture, -1);
        assert_eq!((c.server_now, c.server_date), (1_000, 0x5678));
        assert_eq!(c.lockouts[0].instance_id, 5);
        assert_eq!(c.resets[0].period, 604_800);
        let h = &c.holidays[0];
        assert_eq!((h.id, h.filter_type, h.dates[0], h.flags[9]), (0, 4, 5, 50));
        assert_eq!(h.texture, "Calendar_Fair");
    }

    /// The invite's response time is present only when its third byte is 1 (`0x5bf087`).
    #[test]
    fn an_invite_reads_its_response_time_only_on_type_one() {
        let mut head = packed(0x10);
        head.extend_from_slice(&3u64.to_le_bytes());
        head.extend_from_slice(&4u64.to_le_bytes());
        head.extend_from_slice(&[80, 0]);
        let mut with = head.clone();
        with.push(1);
        with.extend_from_slice(&77u32.to_le_bytes());
        with.push(1);
        let i = read_invite(&mut &with[..]).unwrap();
        assert_eq!((i.response_time, i.clear_pending), (77, true));
        let mut without = head;
        without.extend_from_slice(&[0, 0]);
        let i = read_invite(&mut &without[..]).unwrap();
        assert_eq!((i.response_time, i.clear_pending), (0, false));
    }

    /// A negative map on the lockout removal is the whole-table clear and carries no more.
    #[test]
    fn a_negative_lockout_removal_carries_nothing_more() {
        let b = (-1i32).to_le_bytes();
        let p = read_lockout_removed(&mut &b[..]).unwrap();
        assert_eq!(
            p,
            CalendarPacket::LockoutRemoved {
                map: -1,
                lockout: None
            }
        );
    }

    /// The two removal senders differ in their tail: a `u8` from the editor, the `u32` flags
    /// from the day menu (`0x5f1eb2`, `0x5bc76a`).
    #[test]
    fn the_two_remove_event_tails_differ_in_width() {
        let (op, editor) = CalendarCommand::RemoveEvent {
            event_id: 1,
            moderator_invite: 2,
            guild: true,
        }
        .encode();
        assert_eq!(op, super::super::opcode::CMSG_CALENDAR_REMOVE_EVENT);
        assert_eq!(editor.len(), 17);
        let (_, menu) = CalendarCommand::RemoveDayEvent {
            event_id: 1,
            invite_id: 2,
            flags: 0x400,
        }
        .encode();
        assert_eq!(menu.len(), 20);
    }

    /// An announcement's add carries no invitee count (`0x5f1d64`).
    #[test]
    fn an_announcement_add_carries_no_invitee_list() {
        let event = CalendarEventFields {
            title: "T".into(),
            flags: 0x40,
            ..Default::default()
        };
        let (_, bare) = CalendarCommand::AddEvent {
            event: event.clone(),
            invites: None,
        }
        .encode();
        let (_, listed) = CalendarCommand::AddEvent {
            event,
            invites: Some(vec![CalendarNewInvite {
                guid: 0x2a,
                status: 3,
                moderator: 2,
            }]),
        }
        .encode();
        assert_eq!(listed.len(), bare.len() + 4 + 2 + 2);
    }
}
