//! The instance messages: the lockout lines, the encounter units, the lock warning, and their
//! sends. The 12340 client fills most lockout lines itself as system chat; the raid welcome is the
//! `RAID_INSTANCE_WELCOME` event, which `UIParent.lua` words. Maps travel as `Map.dbc` ids; the
//! reference looks the name up itself and prints the id when the row is missing.

use std::io;

use crate::wire::{read_packed_guid, read_u32_le, read_u8};

/// `SMSG_RAID_INSTANCE_MESSAGE`'s `type`, the jump table at `0x50cae4`; outside 1-5 the reference
/// does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidInstanceWarning {
    /// 1: `RAID_INSTANCE_WARNING_HOURS`, filled with `seconds / 3600`.
    Hours,
    /// 2: `RAID_INSTANCE_WARNING_MIN`, filled with `seconds / 60`.
    Minutes,
    /// 3: `RAID_INSTANCE_WARNING_MIN_SOON`, filled with `seconds / 60`.
    MinutesSoon,
    /// 4: no line; the `RAID_INSTANCE_WELCOME` event with the name, seconds, locked and extended.
    Welcome,
    /// 5: `RAID_INSTANCE_EXPIRED`, the name alone.
    Expired,
}

impl RaidInstanceWarning {
    pub fn from_wire(ty: u32) -> Option<Self> {
        match ty {
            1 => Some(Self::Hours),
            2 => Some(Self::Minutes),
            3 => Some(Self::MinutesSoon),
            4 => Some(Self::Welcome),
            5 => Some(Self::Expired),
            _ => None,
        }
    }

    /// The `GlobalStrings.lua` token of the system line; `None` for the welcome, an event.
    pub fn token(self) -> Option<&'static str> {
        match self {
            Self::Hours => Some("RAID_INSTANCE_WARNING_HOURS"),
            Self::Minutes => Some("RAID_INSTANCE_WARNING_MIN"),
            Self::MinutesSoon => Some("RAID_INSTANCE_WARNING_MIN_SOON"),
            Self::Welcome => None,
            Self::Expired => Some("RAID_INSTANCE_EXPIRED"),
        }
    }
}

/// `SMSG_RAID_INSTANCE_MESSAGE` (`0x50ca80`): a raid lockout's countdown, welcome or expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaidInstanceMessage {
    /// The raw wire type; [`RaidInstanceWarning::from_wire`] resolves it.
    pub message_type: u32,
    pub map: u32,
    /// The 0-based difficulty, which names the map with its `MapDifficulty.dbc` row (`0x4fe070`).
    pub difficulty: u32,
    /// Seconds until the reset, read and divided signed.
    pub seconds: i32,
    /// The welcome's two trailing bytes, `(locked, extended)`; only type 4 reads them.
    pub welcome: Option<(bool, bool)>,
}

/// `SMSG_INSTANCE_RESET_FAILED`'s `reason` (`0x50cda0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceResetFailure {
    /// 0: `INSTANCE_RESET_FAILED`, players still inside.
    General,
    /// 1: `INSTANCE_RESET_FAILED_OFFLINE`, someone in the party is offline.
    Offline,
    /// 2: `INSTANCE_RESET_FAILED_ZONING`, someone in the party is zoning in.
    Zoning,
}

impl InstanceResetFailure {
    /// Deviation: a reason of 3 or more is silent, where the reference prints an uninitialized
    /// stack buffer.
    pub fn from_wire(reason: u32) -> Option<Self> {
        match reason {
            0 => Some(Self::General),
            1 => Some(Self::Offline),
            2 => Some(Self::Zoning),
            _ => None,
        }
    }

    /// The reference's GlobalStrings token.
    pub fn token(self) -> &'static str {
        match self {
            Self::General => "INSTANCE_RESET_FAILED",
            Self::Offline => "INSTANCE_RESET_FAILED_OFFLINE",
            Self::Zoning => "INSTANCE_RESET_FAILED_ZONING",
        }
    }
}

/// `SMSG_INSTANCE_RESET_FAILED`: the group leader's reset was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceResetFailed {
    /// The raw wire reason; [`InstanceResetFailure::from_wire`] resolves it.
    pub reason: u32,
    pub map: u32,
}

/// Read `SMSG_RAID_INSTANCE_MESSAGE`: `u32` type, map, difficulty and seconds, then for type 4
/// `u8` locked and `u8` extended.
pub(super) fn read_raid_instance_message(r: &mut &[u8]) -> io::Result<RaidInstanceMessage> {
    let message_type = read_u32_le(r)?;
    let map = read_u32_le(r)?;
    let difficulty = read_u32_le(r)?;
    let seconds = read_u32_le(r)? as i32;
    let welcome = if message_type == 4 {
        Some((read_u8(r)? != 0, read_u8(r)? != 0))
    } else {
        None
    };
    Ok(RaidInstanceMessage {
        message_type,
        map,
        difficulty,
        seconds,
        welcome,
    })
}

/// Read `SMSG_INSTANCE_RESET_FAILED`: `u32 reason`, `u32 mapId`.
pub(super) fn read_instance_reset_failed(r: &mut &[u8]) -> io::Result<InstanceResetFailed> {
    Ok(InstanceResetFailed {
        reason: read_u32_le(r)?,
        map: read_u32_le(r)?,
    })
}

/// Read a one-`u32` body: `SMSG_INSTANCE_RESET`'s map, `SMSG_UPDATE_LAST_INSTANCE`'s map,
/// `SMSG_UPDATE_INSTANCE_OWNERSHIP`'s bind flag and `SMSG_INSTANCE_SAVE_CREATED`'s flag.
pub(super) fn read_u32_body(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// One `SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT` (`0x5eddd0`), by its `u32` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncounterUnitUpdate {
    /// 0: take a boss slot, unless the unit holds one.
    Engage { guid: u64, priority: u8 },
    /// 1: free the unit's slot; its priority byte is read and dropped.
    Disengage { guid: u64 },
    /// 2: re-rank a held unit.
    Priority { guid: u64, priority: u8 },
    /// 3: an encounter timer of `ms` (`0x5edbc0`), which nothing reads.
    Timer { ms: u8 },
    /// 4: an encounter objective on (`0x5edc50`), which nothing reads.
    EnableObjective { id: u8 },
    /// 5: an objective's progress moved by `delta` (`0x5edd10`).
    UpdateObjective { id: u8, delta: u8 },
    /// 6: an objective off (`0x5edd50`).
    DisableObjective { id: u8 },
    /// 7: re-sort the slots and signal, with no body.
    Resort,
    /// Past 7: nothing read, nothing done.
    Unknown(u32),
}

/// Read `SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT`: `u32` type, then a packed guid and `u8` (0-2), a
/// `u8` (3, 4, 6), two `u8`s (5) or nothing.
pub(super) fn read_encounter_unit(r: &mut &[u8]) -> io::Result<EncounterUnitUpdate> {
    use EncounterUnitUpdate as U;
    Ok(match read_u32_le(r)? {
        ty @ 0..=2 => {
            let guid = read_packed_guid(r)?;
            let priority = read_u8(r)?;
            match ty {
                0 => U::Engage { guid, priority },
                1 => U::Disengage { guid },
                _ => U::Priority { guid, priority },
            }
        }
        3 => U::Timer { ms: read_u8(r)? },
        4 => U::EnableObjective { id: read_u8(r)? },
        5 => U::UpdateObjective {
            id: read_u8(r)?,
            delta: read_u8(r)?,
        },
        6 => U::DisableObjective { id: read_u8(r)? },
        7 => U::Resort,
        other => U::Unknown(other),
    })
}

/// `SMSG_INSTANCE_LOCK_WARNING_QUERY` (the `0x147` arm of `0x6e2e90`): the time left to accept
/// the instance's lock, the encounters already done, and whether the lock extends a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceLockWarning {
    /// Milliseconds; zero or negative (as `i32`) stops the clock (`0x513b20`).
    pub ms: u32,
    /// `DungeonEncounter.dbc` `Bit`s already completed.
    pub completed_mask: u32,
    pub extending: bool,
}

/// Read `SMSG_INSTANCE_LOCK_WARNING_QUERY`: `u32` ms, `u32` completed mask, `u8` extending.
pub(super) fn read_instance_lock_warning(r: &mut &[u8]) -> io::Result<InstanceLockWarning> {
    Ok(InstanceLockWarning {
        ms: read_u32_le(r)?,
        completed_mask: read_u32_le(r)?,
        extending: read_u8(r)? != 0,
    })
}

/// Body of `CMSG_RESET_INSTANCES`: empty (`ResetInstances`, `0x515630`).
pub fn reset_instances() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_INSTANCE_LOCK_RESPONSE` (`RespondInstanceLock`, `0x613ed0`): `u8` accept.
pub fn instance_lock_response(accept: bool) -> Vec<u8> {
    vec![u8::from(accept)]
}

/// Body of a client `MSG_SET_DUNGEON_DIFFICULTY` (`0x526050`) or `MSG_SET_RAID_DIFFICULTY`
/// (`0x5261a0`): the 0-based `u32` difficulty.
pub fn set_difficulty(difficulty: u32) -> Vec<u8> {
    difficulty.to_le_bytes().to_vec()
}

/// `SMSG_RAID_GROUP_ONLY` (the arm before `0x6e3c10`): a positive `delay_ms` arms the
/// instance-boot deadline (`INSTANCE_BOOT_START`), any other clears it (`INSTANCE_BOOT_STOP`);
/// with a zero delay, `reason` 1-4 first shows `ERR_RAID_GROUP_LOWLEVEL`, `_ONLY`, `_FULL` or
/// `_REQUIREMENTS_UNMATCH`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaidGroupOnly {
    pub delay_ms: u32,
    pub reason: u32,
}

/// Parse `SMSG_RAID_GROUP_ONLY`: `u32 delayMs`, `u32 reason`.
pub(super) fn read_raid_group_only(r: &mut impl std::io::Read) -> std::io::Result<RaidGroupOnly> {
    Ok(RaidGroupOnly {
        delay_ms: crate::wire::read_u32_le(r)?,
        reason: crate::wire::read_u32_le(r)?,
    })
}
