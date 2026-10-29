//! Group messages: invites, leadership, loot method, roster, member stats, minimap pings,
//! subgroups, raid target icons, ready checks and lockouts (vmangos `Server/Packets/Group.cpp`).
//! Every guid here is a full `u64` except the party member stats subject, which is packed.

use std::io::{self, Read};

use crate::messages::update_object::power_display_scale;
use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_packed_guid, read_u16_le, read_u32_le,
    read_u64_le, read_u8,
};

/// The raid-assistant bit of a member's flags byte; bits 0-2 are the subgroup
/// (`Group/Group.cpp:1382,1394`).
pub const GROUP_MEMBER_ASSISTANT: u8 = 0x80;
/// A member's main-tank assignment, folded from the row's flags.
pub const GROUP_MEMBER_MAIN_TANK: u8 = 0x08;
/// A member's main-assist assignment, folded from the row's flags.
pub const GROUP_MEMBER_MAIN_ASSIST: u8 = 0x10;

/// The member status bits: an `SMSG_GROUP_LIST` row's byte and the low byte of a member record's
/// `u16` (`[rec+8]`, written by `0x6cfa96` and the member-state fill `0x6e12e0`).
pub mod member_status {
    pub const OFFLINE: u8 = 0x00;
    pub const ONLINE: u8 = 0x01;
    pub const PVP: u8 = 0x02;
    pub const DEAD: u8 = 0x04;
    pub const GHOST: u8 = 0x08;
    pub const PVP_FFA: u8 = 0x10;
    pub const AFK: u8 = 0x40;
    pub const DND: u8 = 0x80;
}

/// vmangos `PartyOperation` (`WorldSession.h:94-98`): which command a party result answers.
/// 12340 also tells `SWAP` apart (`0x6cc228`).
pub mod party_operation {
    pub const INVITE: u32 = 0;
    pub const LEAVE: u32 = 2;
    pub const SWAP: u32 = 4;
}

/// The verdict of `SMSG_PARTY_COMMAND_RESULT`, numbered as 12340's handler `0x6cbec0` switches on
/// it; 1.12.1 had no `TARGET_NOT_IN_INSTANCE`, so every code past 2 moved up one.
pub mod party_result {
    pub const OK: u32 = 0;
    pub const BAD_PLAYER_NAME: u32 = 1;
    pub const TARGET_NOT_IN_GROUP: u32 = 2;
    pub const TARGET_NOT_IN_INSTANCE: u32 = 3;
    pub const GROUP_FULL: u32 = 4;
    pub const ALREADY_IN_GROUP: u32 = 5;
    pub const NOT_IN_GROUP: u32 = 6;
    pub const NOT_LEADER: u32 = 7;
    pub const WRONG_FACTION: u32 = 8;
    pub const IGNORING_YOU: u32 = 9;
    pub const LFG_PENDING: u32 = 12;
    pub const INVITE_RESTRICTED: u32 = 13;
    /// `ERR_GROUP_SWAP_FAILED` under [`super::party_operation::SWAP`], `ERR_INVITE_IN_COMBAT`
    /// otherwise.
    pub const IN_COMBAT_OR_SWAP_FAILED: u32 = 14;
    pub const INVITE_UNKNOWN_REALM: u32 = 15;
    pub const INVITE_NO_PARTY_SERVER: u32 = 16;
    pub const INVITE_PARTY_BUSY: u32 = 17;
    pub const TARGET_AMBIGUOUS: u32 = 18;
    pub const LFG_INVITE_RAID_LOCKED: u32 = 19;
    pub const LFG_BOOT_LIMIT: u32 = 20;
    pub const LFG_BOOT_COOLDOWN: u32 = 21;
    pub const LFG_BOOT_IN_PROGRESS: u32 = 22;
    pub const LFG_BOOT_TOO_FEW_PLAYERS: u32 = 23;
    pub const LFG_BOOT_NOT_ELIGIBLE: u32 = 24;
    pub const RAID_DISALLOWED_BY_LEVEL: u32 = 25;
    pub const LFG_BOOT_IN_COMBAT: u32 = 26;
    pub const VOTE_KICK_REASON_NEEDED: u32 = 27;
    pub const LFG_BOOT_DUNGEON_COMPLETE: u32 = 28;
    pub const LFG_BOOT_LOOT_ROLLS: u32 = 29;
    pub const LFG_TELEPORT_IN_COMBAT: u32 = 30;
}

/// One `SMSG_GROUP_LIST` member row (`Group.cpp:161-167`); the recipient has no row, their flags
/// ride the list's own-flags byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMemberEntry {
    pub name: String,
    pub guid: u64,
    /// [`member_status`] bits (`GetGroupMemberStatus`, `Group/Group.cpp:45-63`).
    pub status: u8,
    /// Subgroup in bits 0-2, plus [`GROUP_MEMBER_ASSISTANT`].
    pub flags: u8,
    /// The LFG role bits, tank `0x2`, healer `0x4`, damage `0x8`, which `UnitGroupRolesAssigned`
    /// reads (`0x60c810`).
    pub roles: u8,
}

impl GroupMemberEntry {
    /// Whether the row's status byte marks the member online for the record write: a non-zero byte,
    /// the client's own test for a party slot and a raid row, where bit 0 alone is not asked.
    /// vmangos sends 0 for an absent member and a byte with bit 0 set for a present one
    /// (`Group/Group.cpp:45-63`), so the two tests agree on its lists.
    pub fn listed_online(&self) -> bool {
        self.status != 0
    }
}

/// The loot tail `SMSG_GROUP_LIST` carries only when it lists members (`Group.cpp:170-179`);
/// `threshold` is an item quality, 2 to 4 in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupLootInfo {
    /// 0 free-for-all, 1 round-robin, 2 master, 3 group, 4 need before greed (`LootMethod`).
    pub method: u8,
    /// The master looter, 0 unless `method` is 2.
    pub master: u64,
    pub threshold: u8,
}

/// The group's instance difficulties from the list's loot tail, each 0-based (`0x6d8fc3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupDifficulties {
    /// `[0xbd1978]`, the group's dungeon difficulty.
    pub dungeon: u8,
    /// `[0xbd197c]`, the group's raid difficulty.
    pub raid: u8,
    /// `[0xbd1980]`, the dynamic difficulty of a flexible raid map.
    pub dynamic: u8,
}

/// One `MSG_SET_DUNGEON_DIFFICULTY` or `MSG_SET_RAID_DIFFICULTY`: a 0-based difficulty and which of
/// the player's own and the group's it sets (`0x525530`, `0x5255a0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifficultySet {
    pub difficulty: u32,
    pub set_player: bool,
    pub set_group: bool,
}

/// What `SMSG_GROUP_LIST` carries, as [`read_group_list`] folds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GroupList {
    pub group_type: u8,
    pub own_flags: u8,
    pub own_roles: u8,
    pub members: Vec<GroupMemberEntry>,
    pub leader: u64,
    pub loot: Option<GroupLootInfo>,
    pub difficulties: Option<GroupDifficulties>,
    pub lfg_restricted: bool,
    pub lfg_status: u8,
    pub lfg_slot: u32,
    pub battleground: bool,
}

/// 3.3.5a's group-type battleground bit (`0x6d8870` tests `& 1`).
const GROUP_TYPE_BATTLEGROUND: u8 = 0x01;
/// 3.3.5a's group-type raid bit (`0x6d8870` tests `& 2`); `& 1` is a battleground group.
const GROUP_TYPE_RAID: u8 = 0x02;
/// 3.3.5a's group-type LFG-restricted bit, the argument of the LFG-restricted set (`0x6d8da2`).
const GROUP_TYPE_LFG_RESTRICTED: u8 = 0x04;
/// 3.3.5a's group-type LFG bit: an LFG list carries a status byte and the dungeon slot.
const GROUP_TYPE_LFG: u8 = 0x08;
/// 3.3.5a's member-flags assistant bit (`GetRaidRosterInfo` tests `& 1`).
const MEMBER_FLAG_ASSISTANT: u8 = 0x01;
/// The main-tank and main-assist assignments, which the combat log's flags read (`0x74dcb0`).
const MEMBER_FLAG_MAIN_TANK: u8 = 0x02;
const MEMBER_FLAG_MAIN_ASSIST: u8 = 0x04;

/// A 3.3.5a subgroup and flags byte folded into one: subgroup in bits 0-2, then
/// [`GROUP_MEMBER_MAIN_TANK`], [`GROUP_MEMBER_MAIN_ASSIST`] and [`GROUP_MEMBER_ASSISTANT`].
fn fold_member_flags(subgroup: u8, flags: u8) -> u8 {
    let bit = |mask: u8, folded: u8| if flags & mask != 0 { folded } else { 0 };
    (subgroup & 0x07)
        | bit(MEMBER_FLAG_ASSISTANT, GROUP_MEMBER_ASSISTANT)
        | bit(MEMBER_FLAG_MAIN_TANK, GROUP_MEMBER_MAIN_TANK)
        | bit(MEMBER_FLAG_MAIN_ASSIST, GROUP_MEMBER_MAIN_ASSIST)
}

/// Read `SMSG_GROUP_LIST` as `0x6d8870` does; `group_type` is 1 for a raid, flags [`fold_member_flags`].
pub(super) fn read_group_list(r: &mut &[u8]) -> io::Result<GroupList> {
    let raw_type = read_u8(r)?;
    let own_subgroup = read_u8(r)?;
    let own_flags = fold_member_flags(own_subgroup, read_u8(r)?);
    let own_roles = read_u8(r)?;
    let (lfg_status, lfg_slot) = if raw_type & GROUP_TYPE_LFG != 0 {
        (read_u8(r)?, read_u32_le(r)?)
    } else {
        (0, 0)
    };
    let _group_guid = read_u64_le(r)?;
    let _counter = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    // A raid is the widest list this carries, 40 (`raidGuids[40]` in `0x6d8870`).
    let mut members = Vec::with_capacity(capacity_hint(count, 40));
    for _ in 0..count {
        let name = read_cstring(r)?;
        let guid = read_u64_le(r)?;
        let status = read_u8(r)?;
        let subgroup = read_u8(r)?;
        let flags = fold_member_flags(subgroup, read_u8(r)?);
        let roles = read_u8(r)?;
        members.push(GroupMemberEntry {
            name,
            guid,
            status,
            flags,
            roles,
        });
    }
    let leader = read_u64_le(r)?;
    let (loot, difficulties) = if count > 0 {
        let method = read_u8(r)?;
        let master = read_u64_le(r)?;
        let threshold = read_u8(r)?;
        let difficulties = GroupDifficulties {
            dungeon: read_u8(r)?,
            raid: read_u8(r)?,
            dynamic: read_u8(r)?,
        };
        let loot = GroupLootInfo {
            method,
            master,
            threshold,
        };
        (Some(loot), Some(difficulties))
    } else {
        (None, None)
    };
    Ok(GroupList {
        group_type: u8::from(raw_type & GROUP_TYPE_RAID != 0),
        own_flags,
        own_roles,
        members,
        leader,
        loot,
        difficulties,
        lfg_restricted: raw_type & GROUP_TYPE_LFG_RESTRICTED != 0,
        lfg_status,
        lfg_slot,
        battleground: raw_type & GROUP_TYPE_BATTLEGROUND != 0,
    })
}

/// Read `SMSG_GROUP_INVITE` as `0x6df1a0` does: can-accept, inviter, `u32`, `u32` dungeon list, `u32`.
pub(super) fn read_group_invite(r: &mut impl Read) -> io::Result<(bool, String)> {
    let can_accept = read_u8(r)? != 0;
    let inviter = read_cstring(r)?;
    let _dungeon_type = read_u32_le(r)?;
    let count = read_u8(r)?;
    for _ in 0..count {
        let _dungeon = read_u32_le(r)?;
    }
    let _unknown = read_u32_le(r)?;
    Ok((can_accept, inviter))
}

/// Read `SMSG_GROUP_DECLINE`: the decliner's name (`Group.cpp:112-115`).
pub(super) fn read_group_decline(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_GROUP_SET_LEADER`: the new leader's name (`Group.cpp:150-153`).
pub(super) fn read_group_set_leader(r: &mut impl Read) -> io::Result<String> {
    read_cstring(r)
}

/// Read `SMSG_PARTY_COMMAND_RESULT` as `0x6cbec0` does: operation, name, result, LFG boot duration.
pub(super) fn read_party_command_result(r: &mut impl Read) -> io::Result<(u32, String, u32)> {
    let operation = read_u32_le(r)?;
    let member = read_cstring(r)?;
    let result = read_u32_le(r)?;
    let _value = read_u32_le(r)?;
    Ok((operation, member, result))
}

/// 12340's `SMSG_PARTY_MEMBER_STATS` mask (`0x6cf9b0`): the member fields that follow, in bit
/// order. Bit 31 is no field: it sets the record's status `0x20` (`0x6cfad2`), which only the
/// minimap and world map read, and is not kept.
pub mod party_member_mask {
    pub const STATUS: u32 = 0x0000_0001;
    pub const CUR_HP: u32 = 0x0000_0002;
    pub const MAX_HP: u32 = 0x0000_0004;
    pub const POWER_TYPE: u32 = 0x0000_0008;
    pub const CUR_POWER: u32 = 0x0000_0010;
    pub const MAX_POWER: u32 = 0x0000_0020;
    pub const LEVEL: u32 = 0x0000_0040;
    pub const ZONE: u32 = 0x0000_0080;
    pub const POSITION: u32 = 0x0000_0100;
    pub const AURAS: u32 = 0x0000_0200;
    pub const PET_GUID: u32 = 0x0000_0400;
    pub const PET_NAME: u32 = 0x0000_0800;
    pub const PET_MODEL_ID: u32 = 0x0000_1000;
    pub const PET_CUR_HP: u32 = 0x0000_2000;
    pub const PET_MAX_HP: u32 = 0x0000_4000;
    pub const PET_POWER_TYPE: u32 = 0x0000_8000;
    pub const PET_CUR_POWER: u32 = 0x0001_0000;
    pub const PET_MAX_POWER: u32 = 0x0002_0000;
    pub const PET_AURAS: u32 = 0x0004_0000;
    pub const VEHICLE_SEAT: u32 = 0x0008_0000;
}

/// One aura of a record's block (`[rec+0x20]` ids, `[rec+0x120]` flags; the pet's at `+0x1c8`
/// and `+0x2c8`): `slot` is the wire mask's bit and `flags` the aura's `AFLAG_*` byte, whose
/// `0x80` marks it harmful (`0x614a8c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordAura {
    pub slot: u8,
    pub spell_id: u32,
    pub flags: u8,
}

/// One `SMSG_PARTY_MEMBER_STATS` or `_FULL` payload (`0x6cf9b0`): only the masked fields are
/// present. The plain opcode carries what changed; `_FULL` everything the server has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartyMemberStatsInfo {
    /// [`member_status`] bits; the record holds a `u16` (`0x6cfa96`).
    pub status: Option<u16>,
    pub cur_hp: Option<u32>,
    pub max_hp: Option<u32>,
    pub power_type: Option<u8>,
    pub cur_power: Option<u16>,
    pub max_power: Option<u16>,
    pub level: Option<u16>,
    pub zone: Option<u16>,
    /// Raw WoW x and y, truncated to `i16` (`[rec+0x1c]`, `[rec+0x1e]`).
    pub position: Option<(i16, i16)>,
    /// The aura block, one entry per set bit of the wire's `u64` slot mask, in slot order. A
    /// delta's mask names the slots that changed, a 0 id clearing one, so the record is patched
    /// slot by slot ([`patch_auras`]).
    pub auras: Option<Vec<RecordAura>>,
    pub pet_guid: Option<u64>,
    pub pet_name: Option<String>,
    pub pet_model_id: Option<u16>,
    pub pet_cur_hp: Option<u32>,
    pub pet_max_hp: Option<u32>,
    pub pet_power_type: Option<u8>,
    pub pet_cur_power: Option<u16>,
    pub pet_max_power: Option<u16>,
    /// The pet's aura block, as [`Self::auras`].
    pub pet_auras: Option<Vec<RecordAura>>,
    /// The `VehicleSeat.dbc` id of the seat the member rides in (`[rec+0x308]`).
    pub vehicle_seat: Option<u32>,
}

/// Apply one aura block to a record's (`0x6cfbd4`-`0x6cfbe6`): each named slot takes its id and
/// flags, the other slots keep theirs, and a 0 id empties the slot. The record keeps the occupied
/// slots in ascending order, the order `UnitBuff`'s record walk reads them in (`0x614a80`).
pub fn patch_auras(record: &mut Option<Vec<RecordAura>>, patch: &[RecordAura]) {
    let slots = record.get_or_insert_with(Vec::new);
    for &aura in patch {
        match slots.binary_search_by_key(&aura.slot, |a| a.slot) {
            Ok(i) if aura.spell_id == 0 => {
                slots.remove(i);
            }
            Ok(i) => slots[i] = aura,
            Err(i) if aura.spell_id != 0 => slots.insert(i, aura),
            Err(_) => {}
        }
    }
}

impl PartyMemberStatsInfo {
    /// The record of a member new to the roster: 1/1 health and power, so an unseen member shows
    /// full bars until stats arrive, and the roster's online bit.
    pub fn placeholder(online: bool) -> Self {
        Self {
            status: Some(u16::from(online)),
            cur_hp: Some(1),
            max_hp: Some(1),
            cur_power: Some(1),
            max_power: Some(1),
            ..Self::default()
        }
    }

    /// The record's online bit (`[rec+8] & 1`), the bit the readers test (`UnitIsConnected`'s). A
    /// record with no status yet reads as the zeroed word of a fresh one, offline.
    pub fn is_online(&self) -> bool {
        self.has_status(member_status::ONLINE)
    }

    /// Whether the record's status has any of `bits` ([`member_status`]); a record with no status
    /// yet has none.
    pub fn has_status(&self, bits: u8) -> bool {
        self.status.unwrap_or(0) & u16::from(bits) != 0
    }

    /// Whether the record's status has the vehicle bit `0x200`, which `UnitInVehicle` reads for a
    /// member out of view (`0x61347b`).
    pub fn in_vehicle(&self) -> bool {
        self.status.unwrap_or(0) & 0x200 != 0
    }

    /// Set or clear the record's online bit (`[rec+8] & 1`), the other bits of its status as they
    /// were: the write `SMSG_GROUP_LIST` makes to a member it lists, from that row's status. A
    /// record with no status yet reads as the zeroed word of a fresh one.
    pub fn set_online(&mut self, online: bool) {
        let status = self.status.unwrap_or(0);
        let bit = u16::from(member_status::ONLINE);
        self.status = Some(if online { status | bit } else { status & !bit });
    }

    /// Fill the record from a member's live descriptor as the member-state fill `0x6e12e0` does,
    /// when the member's object leaves view and just before the stats request, so the frame never
    /// reads 0/0. Values are raw; [`Self::shown_power`] divides at the read. `auras` are the
    /// member's live `(spell id, flags)` in slot order: each with an effect bit (`flags & 0x7`) is
    /// kept, packed from slot 0, up to 64 (`0x6e14a8`-`0x6e14fa`). Of the pet block only the guid
    /// is written, `CHARM` else `SUMMON` (`0x6e14fc`-`0x6e151e`).
    ///
    /// Deviation: zone and position are kept, not set to the viewer's zone and the object's
    /// position, because nothing in benilla reads them; the refer-a-friend (`0x100`) and vehicle
    /// (`0x200`) status bits are not kept, for the same reason.
    pub fn snapshot_descriptor(
        &mut self,
        fields: &crate::messages::update_object::ObjectFields,
        auras: impl IntoIterator<Item = (u32, u8)>,
    ) {
        let mut status = member_status::ONLINE;
        if fields.player_is_ghost() {
            status |= member_status::GHOST;
        }
        // `UNIT_FIELD_BYTES_2` byte 1: PvP `0x1`, FFA PvP `0x4` (`0x6e1310`, `0x6e1324`).
        let byte2 = fields.unit_byte2_flags();
        if byte2 & 0x1 != 0 {
            status |= member_status::PVP;
        }
        if byte2 & 0x4 != 0 {
            status |= member_status::PVP_FFA;
        }
        // Raw health, not `unit_reads_dead`: a feigning member reads alive (`0x6e133b`).
        if fields.unit_health().unwrap_or(0) == 0 {
            status |= member_status::DEAD;
        }
        // `PLAYER_FLAGS` AFK `0x2` and DND `0x4` (`0x6e1354`, `0x6e136a`).
        if fields.player_flags() & 0x2 != 0 {
            status |= member_status::AFK;
        }
        if fields.player_flags() & 0x4 != 0 {
            status |= member_status::DND;
        }
        self.status = Some(u16::from(status));
        let power_type = fields.unit_power_type();
        self.cur_hp = Some(fields.unit_health().unwrap_or(0));
        self.max_hp = Some(fields.unit_max_health().unwrap_or(0));
        self.power_type = Some(power_type);
        // The record holds `u16` power and level, each dword's low word (`0x6e13d8`-`0x6e1404`).
        self.cur_power = Some(fields.unit_power(power_type).unwrap_or(0) as u16);
        self.max_power = Some(fields.unit_max_power(power_type).unwrap_or(0) as u16);
        self.level = Some(fields.unit_level().unwrap_or(0) as u16);
        self.auras = Some(
            auras
                .into_iter()
                .filter(|&(spell_id, flags)| spell_id != 0 && flags & 0x7 != 0)
                .take(64)
                .zip(0u8..)
                .map(|((spell_id, flags), slot)| RecordAura {
                    slot,
                    spell_id,
                    flags,
                })
                .collect(),
        );
        self.pet_guid = fields.unit_pet_guid();
    }

    /// `UnitPowerType` from the record, 0 when it has none (the binding's miss value).
    pub fn shown_power_type(&self) -> u8 {
        self.power_type.unwrap_or(0)
    }

    /// `UnitMana` from the record: the stored power divided by [`power_display_scale`], as for a
    /// live unit.
    pub fn shown_power(&self) -> u32 {
        u32::from(self.cur_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }

    /// `UnitManaMax` from the record, the same divide.
    pub fn shown_max_power(&self) -> u32 {
        u32::from(self.max_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }

    /// `UnitPowerType` from the pet block (`+0xd8`), 0 when it has none.
    pub fn shown_pet_power_type(&self) -> u8 {
        self.pet_power_type.unwrap_or(0)
    }

    /// `UnitMana` from the pet block (`+0xe0`): the stored power divided by
    /// [`power_display_scale`] of the pet's own power type, as for a live unit.
    pub fn shown_pet_power(&self) -> u32 {
        u32::from(self.pet_cur_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_pet_power_type()))
    }

    /// `UnitManaMax` from the pet block (`+0xe2`), the same divide.
    pub fn shown_pet_max_power(&self) -> u32 {
        u32::from(self.pet_max_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_pet_power_type()))
    }
}

/// One aura block (`0x6cfba7`-`0x6cfbfb`): a `u64` slot mask, then a `u32` spell id and a `u8`
/// flags byte per set bit, in bit order.
fn read_aura_block(r: &mut impl Read) -> io::Result<Vec<RecordAura>> {
    let mask = read_u64_le(r)?;
    let mut auras = Vec::with_capacity(mask.count_ones() as usize);
    for slot in 0..64u8 {
        if mask & (1 << slot) != 0 {
            auras.push(RecordAura {
                slot,
                spell_id: read_u32_le(r)?,
                flags: read_u8(r)?,
            });
        }
    }
    Ok(auras)
}

/// Read `SMSG_PARTY_MEMBER_STATS`, or `_FULL` when `full`, as `0x6cf9b0` does: `_FULL` leads with
/// a byte that makes the member an arena opponent (`0x6cf9d2`), then both read the packed guid and
/// the `u32` mask.
pub(super) fn read_party_member_stats(
    r: &mut impl Read,
    full: bool,
) -> io::Result<(u64, PartyMemberStatsInfo)> {
    if full {
        // benilla keeps no arena opponent list for the byte to add to (`0x6cf9fa`).
        let _arena = read_u8(r)?;
    }
    let guid = read_packed_guid(r)?;
    let mask = read_u32_le(r)?;
    let mut info = PartyMemberStatsInfo::default();

    if mask & party_member_mask::STATUS != 0 {
        info.status = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::CUR_HP != 0 {
        info.cur_hp = Some(read_u32_le(r)?);
    }
    if mask & party_member_mask::MAX_HP != 0 {
        info.max_hp = Some(read_u32_le(r)?);
    }
    if mask & party_member_mask::POWER_TYPE != 0 {
        info.power_type = Some(read_u8(r)?);
    }
    if mask & party_member_mask::CUR_POWER != 0 {
        info.cur_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::MAX_POWER != 0 {
        info.max_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::LEVEL != 0 {
        info.level = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::ZONE != 0 {
        info.zone = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::POSITION != 0 {
        let x = read_u16_le(r)? as i16;
        let y = read_u16_le(r)? as i16;
        info.position = Some((x, y));
    }
    if mask & party_member_mask::AURAS != 0 {
        info.auras = Some(read_aura_block(r)?);
    }
    if mask & party_member_mask::PET_GUID != 0 {
        info.pet_guid = Some(read_u64_le(r)?);
    }
    if mask & party_member_mask::PET_NAME != 0 {
        info.pet_name = Some(read_cstring(r)?);
    }
    if mask & party_member_mask::PET_MODEL_ID != 0 {
        info.pet_model_id = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_CUR_HP != 0 {
        info.pet_cur_hp = Some(read_u32_le(r)?);
    }
    if mask & party_member_mask::PET_MAX_HP != 0 {
        info.pet_max_hp = Some(read_u32_le(r)?);
    }
    if mask & party_member_mask::PET_POWER_TYPE != 0 {
        info.pet_power_type = Some(read_u8(r)?);
    }
    if mask & party_member_mask::PET_CUR_POWER != 0 {
        info.pet_cur_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_MAX_POWER != 0 {
        info.pet_max_power = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_AURAS != 0 {
        info.pet_auras = Some(read_aura_block(r)?);
    }
    if mask & party_member_mask::VEHICLE_SEAT != 0 {
        info.vehicle_seat = Some(read_u32_le(r)?);
    }
    Ok((guid, info))
}

/// Read the relayed `MSG_MINIMAP_PING`: the pinger's guid, which the server adds, then x and y
/// (`GroupHandler.cpp:382-391`).
pub(super) fn read_minimap_ping(r: &mut impl Read) -> io::Result<(u64, f32, f32)> {
    Ok((read_u64_le(r)?, read_f32_le(r)?, read_f32_le(r)?))
}

/// A server `MSG_RAID_TARGET_UPDATE` (`Group.cpp:132-147`): mode 0 is one changed icon, mode 1
/// every set icon, an empty list when none is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaidTargetUpdate {
    Delta { icon: u8, guid: u64 },
    List(Vec<(u8, u64)>),
}

pub(super) fn read_raid_target_update(r: &mut &[u8]) -> io::Result<RaidTargetUpdate> {
    let mode = read_u8(r)?;
    if mode == 0 {
        Ok(RaidTargetUpdate::Delta {
            icon: read_u8(r)?,
            guid: read_u64_le(r)?,
        })
    } else {
        let mut entries = Vec::new();
        while !r.is_empty() {
            entries.push((read_u8(r)?, read_u64_le(r)?));
        }
        Ok(RaidTargetUpdate::List(entries))
    }
}

/// A server `MSG_RAID_READY_CHECK`: empty when the leader starts a check, else one member's answer,
/// sent to the leader only (`Group.cpp:94-96`, `126-130`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyCheck {
    Started,
    Answer { guid: u64, ready: u8 },
}

pub(super) fn read_ready_check(r: &mut &[u8]) -> io::Result<ReadyCheck> {
    if r.is_empty() {
        return Ok(ReadyCheck::Started);
    }
    Ok(ReadyCheck::Answer {
        guid: read_u64_le(r)?,
        ready: read_u8(r)?,
    })
}

/// `CMSG_GROUP_INVITE` body (`0x6d4300`): the name, then `InviteUnit`'s `u32` flags.
pub fn group_invite(member_name: &str, flags: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(member_name.len() + 5);
    body.extend_from_slice(member_name.as_bytes());
    body.push(0);
    body.extend_from_slice(&flags.to_le_bytes());
    body
}

/// `CMSG_GROUP_ACCEPT` body (`0x6d4450`): the `u32` role bits `AcceptGroup`'s arguments set.
pub fn group_accept(roles: u32) -> Vec<u8> {
    roles.to_le_bytes().to_vec()
}

/// `CMSG_GROUP_DECLINE` body: empty; the inviter gets `SMSG_GROUP_DECLINE`.
pub fn group_decline() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_GROUP_UNINVITE` body: the kicked player's name (`Group.cpp:9-12`).
pub fn group_uninvite(member_name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(member_name.len() + 1);
    body.extend_from_slice(member_name.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_UNINVITE_GUID` body (`0x6d43c0`): a full guid, then the reason cstring.
pub fn group_uninvite_guid(guid: u64, reason: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + reason.len());
    body.extend_from_slice(&guid.to_le_bytes());
    body.extend_from_slice(reason.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_SET_LEADER` body: a full guid (`Group.cpp:57-64`, vmangos's post-1.11.2 branch).
pub fn group_set_leader(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_LOOT_METHOD` body (`Group.cpp:26-31`): `u32` method, the master looter's full guid
/// (ignored unless the method is 2), `u32` quality threshold.
pub fn loot_method(method: u32, loot_master: u64, threshold: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&method.to_le_bytes());
    body.extend_from_slice(&loot_master.to_le_bytes());
    body.extend_from_slice(&threshold.to_le_bytes());
    body
}

/// `CMSG_GROUP_DISBAND` body: empty.
pub fn group_disband() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_GROUP_RAID_CONVERT` body: empty; turns the sender's party into a raid.
pub fn group_raid_convert() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_REQUEST_PARTY_MEMBER_STATS` body: the member's full guid (`Group.cpp:20-23`); answered
/// by `SMSG_PARTY_MEMBER_STATS_FULL`.
pub fn request_party_member_stats(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_GROUP_CHANGE_SUB_GROUP` body: a member's name, then the subgroup (`Group.cpp:45-49`).
pub fn group_change_sub_group(name: &str, group_nr: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(name.len() + 2);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.push(group_nr);
    body
}

/// `CMSG_GROUP_SWAP_SUB_GROUP` body: the two members' names, mover first (`Group.cpp:51-55`).
pub fn group_swap_sub_group(name: &str, swap_with: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(name.len() + swap_with.len() + 2);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.extend_from_slice(swap_with.as_bytes());
    body.push(0);
    body
}

/// `CMSG_GROUP_ASSISTANT_LEADER` body: a full guid, then 1 to grant raid assistant or 0 to revoke
/// it (`Group.cpp:66-74`).
pub fn group_assistant_leader(guid: u64, grant: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&guid.to_le_bytes());
    body.push(u8::from(grant));
    body
}

/// Our `MSG_MINIMAP_PING` body: x and y, no guid (`Group.cpp:33-37`).
pub fn minimap_ping(x: f32, y: f32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&x.to_le_bytes());
    body.extend_from_slice(&y.to_le_bytes());
    body
}

/// `MSG_RAID_TARGET_UPDATE` body: icon 0 to 7, then a full guid, 0 to clear; the client sends no
/// mode byte (`Group.cpp:77-82`).
pub fn raid_target_set(icon: u8, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.push(icon);
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// `MSG_RAID_TARGET_UPDATE` body asking for every icon: `0xFF` alone, no guid (`Group.cpp:80-81`).
pub fn raid_target_request() -> Vec<u8> {
    vec![0xFF]
}

/// `MSG_RAID_READY_CHECK` body starting a check: empty (`Group.cpp:84-92`).
pub fn ready_check_start() -> Vec<u8> {
    Vec::new()
}

/// `MSG_RAID_READY_CHECK` body answering one: 1 ready or 0 not, no guid (`Group.cpp:84-92`).
pub fn ready_check_answer(ready: bool) -> Vec<u8> {
    vec![u8::from(ready)]
}

/// One `SMSG_RAID_INSTANCE_INFO` row, a saved lockout (`OnRaidInstanceInfo` `0x501030`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaidInstanceEntry {
    /// `Map.dbc` id; the name shown is the client's own lookup.
    pub map: u32,
    /// The difficulty, 0-based.
    pub difficulty: u32,
    /// The 64-bit instance id; `GetSavedInstanceInfo` returns its low and high halves.
    pub instance: u64,
    pub locked: bool,
    pub extended: bool,
    /// Seconds until the lockout resets.
    pub reset: u32,
}

/// Read `SMSG_RAID_INSTANCE_INFO` (`0x501030`): a `u32` count, then per row `u32` map, `u32`
/// difficulty, `u64` instance, `u8` locked, `u8` extended, `u32` reset. The count is
/// authoritative, so a short body is an error.
pub(super) fn read_raid_instance_info(r: &mut &[u8]) -> io::Result<Vec<RaidInstanceEntry>> {
    let count = read_u32_le(r)?;
    // 1024 caps the preallocation far above any real lockout list.
    let mut out = Vec::with_capacity(capacity_hint(count, 1024));
    for _ in 0..count {
        out.push(RaidInstanceEntry {
            map: read_u32_le(r)?,
            difficulty: read_u32_le(r)?,
            instance: read_u64_le(r)?,
            locked: read_u8(r)? != 0,
            extended: read_u8(r)? != 0,
            reset: read_u32_le(r)?,
        });
    }
    Ok(out)
}

/// `CMSG_SET_SAVED_INSTANCE_EXTEND` body (`0x50dc10`): `u32` map, `u32` difficulty, `u8` extend.
pub fn set_saved_instance_extend(map: u32, difficulty: u32, extend: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&map.to_le_bytes());
    body.extend_from_slice(&difficulty.to_le_bytes());
    body.push(u8::from(extend));
    body
}

/// `MSG_PARTY_ASSIGNMENT` body (`0x6d7200`): `u8` assignment (0 main tank, 1 main assist), `u8`
/// set, `u64` member.
pub fn party_assignment(assignment: u8, set: bool, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(10);
    body.push(assignment);
    body.push(u8::from(set));
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// `CMSG_REQUEST_RAID_INFO` body: empty (vmangos `HandleRequestRaidInfoOpcode`).
pub fn request_raid_info() -> Vec<u8> {
    Vec::new()
}
