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

/// vmangos `GroupMemberStatus` (`Group/Group.h:102-111`); bit `0x20` is never set.
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
}

impl GroupMemberEntry {
    /// Whether the row's status byte marks the member online for the record write: a non-zero byte,
    /// the client's own test for a party slot (`0x5e6c3b`) and a raid row (`0x4ba951`), where bit 0
    /// alone is not asked. vmangos sends 0 for an absent member and a byte with bit 0 set for a
    /// present one (`Group/Group.cpp:45-63`), so the two tests agree on its lists.
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
    pub members: Vec<GroupMemberEntry>,
    pub leader: u64,
    pub loot: Option<GroupLootInfo>,
    pub difficulties: Option<GroupDifficulties>,
    pub lfg_restricted: bool,
}

/// 3.3.5a's group-type raid bit (`0x6d8870` tests `& 2`); `& 1` is a battleground group.
const GROUP_TYPE_RAID: u8 = 0x02;
/// 3.3.5a's group-type LFG-restricted bit, `SetLfgRestricted`'s argument (`0x6d8da2`).
const GROUP_TYPE_LFG_RESTRICTED: u8 = 0x04;
/// 3.3.5a's group-type LFG bit: an LFG list carries a status byte and the dungeon slot.
const GROUP_TYPE_LFG: u8 = 0x08;
/// 3.3.5a's member-flags assistant bit (`GetRaidRosterInfo` tests `& 1`).
const MEMBER_FLAG_ASSISTANT: u8 = 0x01;

/// A 3.3.5a subgroup and flags byte folded into one: subgroup in bits 0-2, [`GROUP_MEMBER_ASSISTANT`].
fn fold_member_flags(subgroup: u8, flags: u8) -> u8 {
    let assistant = if flags & MEMBER_FLAG_ASSISTANT != 0 {
        GROUP_MEMBER_ASSISTANT
    } else {
        0
    };
    (subgroup & 0x07) | assistant
}

/// Read `SMSG_GROUP_LIST` as `0x6d8870` does; `group_type` is 1 for a raid, flags [`fold_member_flags`].
pub(super) fn read_group_list(r: &mut &[u8]) -> io::Result<GroupList> {
    let raw_type = read_u8(r)?;
    let own_subgroup = read_u8(r)?;
    let own_flags = fold_member_flags(own_subgroup, read_u8(r)?);
    let _roles = read_u8(r)?;
    if raw_type & GROUP_TYPE_LFG != 0 {
        let _lfg_status = read_u8(r)?;
        let _lfg_slot = read_u32_le(r)?;
    }
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
        let _roles = read_u8(r)?;
        members.push(GroupMemberEntry {
            name,
            guid,
            status,
            flags,
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
        members,
        leader,
        loot,
        difficulties,
        lfg_restricted: raw_type & GROUP_TYPE_LFG_RESTRICTED != 0,
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

/// vmangos `GROUP_UPDATE_FLAG_*` (`Group.h:124-151`): the member stats that follow, in bit order.
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
    pub const AURAS_NEGATIVE: u32 = 0x0000_0400;
    pub const PET_GUID: u32 = 0x0000_0800;
    pub const PET_NAME: u32 = 0x0000_1000;
    pub const PET_MODEL_ID: u32 = 0x0000_2000;
    pub const PET_CUR_HP: u32 = 0x0000_4000;
    pub const PET_MAX_HP: u32 = 0x0000_8000;
    pub const PET_POWER_TYPE: u32 = 0x0001_0000;
    pub const PET_CUR_POWER: u32 = 0x0002_0000;
    pub const PET_MAX_POWER: u32 = 0x0004_0000;
    pub const PET_AURAS: u32 = 0x0008_0000;
    pub const PET_AURAS_NEGATIVE: u32 = 0x0010_0000;
}

/// One `SMSG_PARTY_MEMBER_STATS` or `_FULL` payload (`GroupHandler.cpp:590-742`): only the masked
/// fields are present. The plain opcode carries what changed; `_FULL` everything the server has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartyMemberStatsInfo {
    pub status: Option<u8>,
    pub cur_hp: Option<u16>,
    pub max_hp: Option<u16>,
    pub power_type: Option<u8>,
    pub cur_power: Option<u16>,
    pub max_power: Option<u16>,
    pub level: Option<u16>,
    pub zone: Option<u16>,
    /// Raw WoW x and y, truncated to `i16` by the server (`GroupHandler.cpp:625`).
    pub position: Option<(i16, i16)>,
    /// Buff slots 0..31 as `(slot, spell id)`, one per set bit of the wire's `u32` slot mask, in
    /// slot order. A delta's mask names the slots that changed, a 0 id clearing one
    /// (`Player::GetAuraUpdateMask`), so the record is patched slot by slot ([`patch_auras`]).
    pub auras: Option<Vec<(u8, u16)>>,
    /// Debuff slots, likewise, from a `u16` mask; the slot is absolute, 32..47.
    pub auras_negative: Option<Vec<(u8, u16)>>,
    pub pet_guid: Option<u64>,
    pub pet_name: Option<String>,
    pub pet_model_id: Option<u16>,
    pub pet_cur_hp: Option<u16>,
    pub pet_max_hp: Option<u16>,
    pub pet_power_type: Option<u8>,
    pub pet_cur_power: Option<u16>,
    pub pet_max_power: Option<u16>,
    /// The pet's buff and debuff slots, as [`Self::auras`] and [`Self::auras_negative`].
    pub pet_auras: Option<Vec<(u8, u16)>>,
    pub pet_auras_negative: Option<Vec<(u8, u16)>>,
}

/// Apply one aura-block field to a record's (`0x5e52d0`-`0x5e52f8`): each named slot takes its
/// id, the other slots keep theirs, and a 0 empties the slot. The record keeps the occupied slots
/// in ascending order, the order `UnitBuff`'s roster walk reads them in (`0x519780`).
pub fn patch_auras(record: &mut Option<Vec<(u8, u16)>>, patch: &[(u8, u16)]) {
    let slots = record.get_or_insert_with(Vec::new);
    for &(slot, spell) in patch {
        match slots.binary_search_by_key(&slot, |&(s, _)| s) {
            Ok(i) if spell == 0 => {
                slots.remove(i);
            }
            Ok(i) => slots[i].1 = spell,
            Err(i) if spell != 0 => slots.insert(i, (slot, spell)),
            Err(_) => {}
        }
    }
}

impl PartyMemberStatsInfo {
    /// The record of a member new to the roster: 1/1 health and power, so an unseen member shows
    /// full bars until stats arrive (`0x4e833d`), and the roster's online bit.
    pub fn placeholder(online: bool) -> Self {
        Self {
            status: Some(u8::from(online)),
            cur_hp: Some(1),
            max_hp: Some(1),
            cur_power: Some(1),
            max_power: Some(1),
            ..Self::default()
        }
    }

    /// The record's online bit (`[rec+8] & 1`), the bit the readers test (`0x4e8227`, `0x4e816b`,
    /// `0x4e8884`, `0x519741`, `UnitIsConnected`'s `0x517dd3`). A record with no status yet reads
    /// as the zeroed byte of a fresh one, offline.
    pub fn is_online(&self) -> bool {
        self.status.unwrap_or(0) & member_status::ONLINE != 0
    }

    /// Set or clear the record's online bit (`[rec+8] & 1`), the other bits of its status byte as
    /// they were: the write `SMSG_GROUP_LIST` makes to a member it lists, from that row's status
    /// (`0x4e8361`-`0x4e837b`, raid `0x4ba947`-`0x4ba960`). A record with no status yet reads as
    /// the zeroed byte of a fresh one.
    pub fn set_online(&mut self, online: bool) {
        let status = self.status.unwrap_or(0);
        self.status = Some(if online {
            status | member_status::ONLINE
        } else {
            status & !member_status::ONLINE
        });
    }

    /// Snapshot a member's live descriptor as the 1.12 client does (`0x5f0880`) when the member's
    /// object leaves view, just before the stats request, so the frame never reads 0/0. Values are
    /// raw; [`Self::shown_power`] divides at the read. AFK and DND are cleared, as in the client.
    /// `pet` is the object of the member's pet guid ([`ObjectFields::unit_pet_guid`]) when held:
    /// its block is copied, and without one the pet block is emptied (`0x5f0a1f`-`0x5f0b72`).
    /// `pet_name` is that pet's cached name (`0x609210`, copied into `+0x88` at `0x5f0a70`); a
    /// pet whose name the cache does not hold yet keeps the name the record had.
    ///
    /// Deviation: zone and position are kept, not set to the viewer's zone and the object's
    /// position, because nothing in benilla reads them.
    ///
    /// [`ObjectFields::unit_pet_guid`]: crate::messages::update_object::ObjectFields::unit_pet_guid
    pub fn snapshot_descriptor(
        &mut self,
        fields: &crate::messages::update_object::ObjectFields,
        pet: Option<&crate::messages::update_object::ObjectFields>,
        pet_name: Option<&str>,
    ) {
        let mut status = member_status::ONLINE;
        if fields.player_is_ghost() {
            status |= member_status::GHOST;
        }
        // `UNIT_FIELD_FLAGS` PvP bit.
        if fields.unit_flags() & 0x1000 != 0 {
            status |= member_status::PVP;
        }
        // `PLAYER_FLAGS` bit 7 (`0x5f08be`).
        if fields.player_flags() & 0x80 != 0 {
            status |= member_status::PVP_FFA;
        }
        // Raw health, not `unit_reads_dead`: a feigning member reads alive (`0x5f08d3`).
        if fields.unit_health().unwrap_or(0) == 0 {
            status |= member_status::DEAD;
        }
        self.status = Some(status);
        // The record holds `u16`s; the 1.12 client keeps each dword's low word (`0x5f08ea`).
        let power_type = fields.unit_power_type();
        self.cur_hp = Some(fields.unit_health().unwrap_or(0) as u16);
        self.max_hp = Some(fields.unit_max_health().unwrap_or(0) as u16);
        self.power_type = Some(power_type);
        self.cur_power = Some(fields.unit_power(power_type).unwrap_or(0) as u16);
        self.max_power = Some(fields.unit_max_power(power_type).unwrap_or(0) as u16);
        self.level = Some(fields.unit_level().unwrap_or(0) as u16);
        let (buffs, debuffs) = snapshot_aura_block(fields);
        self.auras = Some(buffs);
        self.auras_negative = Some(debuffs);

        let pet_guid = fields.unit_pet_guid();
        match pet_guid.zip(pet) {
            Some((guid, pet)) => {
                let power_type = pet.unit_power_type();
                self.pet_guid = Some(guid);
                if let Some(name) = pet_name {
                    self.pet_name = Some(name.to_string());
                }
                self.pet_model_id = Some(pet.unit_displayid().unwrap_or(0) as u16);
                self.pet_cur_hp = Some(pet.unit_health().unwrap_or(0) as u16);
                self.pet_max_hp = Some(pet.unit_max_health().unwrap_or(0) as u16);
                self.pet_power_type = Some(power_type);
                self.pet_cur_power = Some(pet.unit_power(power_type).unwrap_or(0) as u16);
                self.pet_max_power = Some(pet.unit_max_power(power_type).unwrap_or(0) as u16);
                let (buffs, debuffs) = snapshot_aura_block(pet);
                self.pet_auras = Some(buffs);
                self.pet_auras_negative = Some(debuffs);
            }
            // No pet object: the block is zero-filled (`0x5f0a0f`), its guid included.
            None => {
                self.pet_guid = None;
                self.pet_name = None;
                self.pet_model_id = None;
                self.pet_cur_hp = None;
                self.pet_max_hp = None;
                self.pet_power_type = None;
                self.pet_cur_power = None;
                self.pet_max_power = None;
                self.pet_auras = None;
                self.pet_auras_negative = None;
            }
        }
    }

    /// `UnitPowerType` from the record, 0 when it has none (the binding's miss value).
    pub fn shown_power_type(&self) -> u8 {
        self.power_type.unwrap_or(0)
    }

    /// `UnitMana` from the record (`0x517744`): the stored power divided by
    /// [`power_display_scale`], as for a live unit.
    pub fn shown_power(&self) -> u32 {
        u32::from(self.cur_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }

    /// `UnitManaMax` from the record (`0x5178af`), the same divide.
    pub fn shown_max_power(&self) -> u32 {
        u32::from(self.max_power.unwrap_or(0))
            / power_display_scale(u32::from(self.shown_power_type()))
    }

    /// `UnitPowerType` from the pet block (`+0xd8`, `0x5179d2`'s pet leg), 0 when it has none.
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

/// `(slot, spell id)` pairs of one aura block, as [`PartyMemberStatsInfo::auras`] holds them.
type AuraSlots = Vec<(u8, u16)>;

/// A live descriptor's aura block as the snapshot copies it (`0x5f098d`-`0x5f09f2`): a slot is
/// kept when its id is set and its `AURAFLAGS` nibble has an effect bit (`test al,0xe`), the test
/// [`ObjectFields::unit_auras`] makes; the record holds each id's low word.
///
/// [`ObjectFields::unit_auras`]: crate::messages::update_object::ObjectFields::unit_auras
fn snapshot_aura_block(
    fields: &crate::messages::update_object::ObjectFields,
) -> (AuraSlots, AuraSlots) {
    use crate::messages::update_object::UNIT_AURA_POSITIVE_SLOTS;
    fields
        .unit_auras()
        .map(|a| (a.slot, a.spell_id as u16))
        .filter(|&(_, id)| id != 0)
        .partition(|&(slot, _)| slot < UNIT_AURA_POSITIVE_SLOTS)
}

/// One `(slot, spell id)` per set bit of `mask`, in bit order, the slot counted from `first`
/// (`GroupHandler.cpp:627-648`; the client's reads, `0x5e52d0`-`0x5e52f8`).
fn read_aura_slots(
    r: &mut impl Read,
    mask: u32,
    first: u8,
    bits: u8,
) -> io::Result<Vec<(u8, u16)>> {
    let mut slots = Vec::new();
    for bit in 0..bits {
        if mask & (1 << bit) != 0 {
            slots.push((first + bit, read_u16_le(r)?));
        }
    }
    Ok(slots)
}

/// Read `SMSG_PARTY_MEMBER_STATS` or `_FULL`; the guid is packed (`GroupHandler.cpp:593`).
pub(super) fn read_party_member_stats(
    r: &mut impl Read,
) -> io::Result<(u64, PartyMemberStatsInfo)> {
    let guid = read_packed_guid(r)?;
    let mask = read_u32_le(r)?;
    let mut info = PartyMemberStatsInfo::default();

    if mask & party_member_mask::STATUS != 0 {
        info.status = Some(read_u8(r)?);
    }
    if mask & party_member_mask::CUR_HP != 0 {
        info.cur_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::MAX_HP != 0 {
        info.max_hp = Some(read_u16_le(r)?);
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
        let pos_mask = read_u32_le(r)?;
        info.auras = Some(read_aura_slots(r, pos_mask, 0, 32)?);
    }
    if mask & party_member_mask::AURAS_NEGATIVE != 0 {
        let neg_mask = u32::from(read_u16_le(r)?);
        info.auras_negative = Some(read_aura_slots(r, neg_mask, 32, 16)?);
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
        info.pet_cur_hp = Some(read_u16_le(r)?);
    }
    if mask & party_member_mask::PET_MAX_HP != 0 {
        info.pet_max_hp = Some(read_u16_le(r)?);
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
        let pos_mask = read_u32_le(r)?;
        info.pet_auras = Some(read_aura_slots(r, pos_mask, 0, 32)?);
    }
    if mask & party_member_mask::PET_AURAS_NEGATIVE != 0 {
        let neg_mask = u32::from(read_u16_le(r)?);
        info.pet_auras_negative = Some(read_aura_slots(r, neg_mask, 32, 16)?);
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

/// One `SMSG_RAID_INSTANCE_INFO` row, a permanent raid lockout (vmangos `Player::SendRaidInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaidInstanceEntry {
    /// `Map.dbc` id; the name shown is the client's own lookup.
    pub map: u32,
    /// Seconds until the lockout resets.
    pub reset: u32,
    /// The instance id, `GetSavedInstanceInfo`'s second return.
    pub instance: u32,
}

/// Read `SMSG_RAID_INSTANCE_INFO`; the count is authoritative, so a short body is an error.
pub(super) fn read_raid_instance_info(r: &mut &[u8]) -> io::Result<Vec<RaidInstanceEntry>> {
    let count = read_u32_le(r)?;
    // 1024 caps the preallocation far above any real lockout list.
    let mut out = Vec::with_capacity(capacity_hint(count, 1024));
    for _ in 0..count {
        out.push(RaidInstanceEntry {
            map: read_u32_le(r)?,
            reset: read_u32_le(r)?,
            instance: read_u32_le(r)?,
        });
    }
    Ok(out)
}

/// `CMSG_REQUEST_RAID_INFO` body: empty (vmangos `HandleRequestRaidInfoOpcode`).
pub fn request_raid_info() -> Vec<u8> {
    Vec::new()
}
