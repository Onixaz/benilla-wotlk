//! Loot wire messages: the loot window, group rolls, master loot and item-push results.

use std::io;

use crate::wire::{capacity_hint, read_u32_le, read_u64_le, read_u8};

/// One item row of `SMSG_LOOT_RESPONSE` (vmangos `LootMgr.cpp:837-845,900-912`). `slot` is the
/// 0-based wire slot that [`autostore_loot_item`] sends back; quest items follow at
/// `items.len() + i` (`LootMgr.cpp:970`). The wire's `randomSuffix` is always 0 and is discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootItem {
    pub slot: u8,
    pub item_id: u32,
    pub count: u32,
    /// `item_template.display_id` → icon via `ItemDisplayInfo.dbc`.
    pub display_info_id: u32,
    pub random_property_id: u32,
    /// A [`slot_type`] code; solo looting always writes `ALLOW_LOOT` (`LootMgr.cpp:918-926`).
    pub slot_type: u8,
}

/// `LootSlotType` (vmangos `LootMgr.h:75-83`): the `u8` that ends each [`LootItem`] row.
pub mod slot_type {
    pub const ALLOW_LOOT: u8 = 0;
    /// A group roll is in progress on this item: shown, not yet lootable.
    pub const ROLL_ONGOING: u8 = 1;
    /// Only the loot master can hand this out ([`loot_master_give`]). Under master loot vmangos
    /// sends it on every row, even greys its take handler allows (`LootMgr.cpp:917-941`).
    pub const MASTER: u8 = 2;
    /// Shown red and not lootable: a requirement the viewer fails, in a group.
    pub const LOCKED: u8 = 3;
    /// Owner-permission solo looting, binding checks skipped.
    pub const OWNER: u8 = 4;
}

/// `LootType`, the wire byte after the guid; 12340's response handler names 1-4 and 6-8
/// (`0x6d53fb`-`0x6d541f`).
pub mod loot_type {
    pub const CORPSE: u8 = 1;
    pub const PICKPOCKETING: u8 = 2;
    pub const FISHING: u8 = 3;
    pub const DISENCHANTING: u8 = 4;
    pub const SKINNING: u8 = 6;
    pub const PROSPECTING: u8 = 7;
    pub const MILLING: u8 = 8;
}

/// `LootError` (vmangos `LootMgr.h:85-100`): the code on the error shape of `SMSG_LOOT_RESPONSE`.
/// `CMSG_LOOT` can draw `DIDNT_KILL`, `PLAYER_NOT_FOUND`, `PLAY_TIME_EXCEEDED`, `NOTSTANDING`,
/// `STUNNED` (`LootHandler.cpp:342-373`) and `TOO_FAR` (`Player.cpp:7952-7956`); the `MASTER_*`
/// codes answer a refused [`loot_master_give`], to the master looter only.
pub mod loot_error {
    pub const DIDNT_KILL: u8 = 0;
    pub const TOO_FAR: u8 = 4;
    pub const BAD_FACING: u8 = 5;
    pub const LOCKED: u8 = 6;
    pub const NOTSTANDING: u8 = 8;
    pub const STUNNED: u8 = 9;
    pub const PLAYER_NOT_FOUND: u8 = 10;
    pub const PLAY_TIME_EXCEEDED: u8 = 11;
    pub const MASTER_INV_FULL: u8 = 12;
    pub const MASTER_UNIQUE_ITEM: u8 = 13;
    pub const MASTER_OTHER: u8 = 14;
    pub const ALREADY_PICKPOCKETED: u8 = 15;
    pub const NOT_WHILE_SHAPESHIFTED: u8 = 16;
}

/// The two shapes of `SMSG_LOOT_RESPONSE`: a `lootType` of 0 after the guid is the error shape.
#[derive(Debug, Clone, PartialEq)]
pub enum LootResponseBody {
    /// `u32 gold, u8 itemCount`, then the [`LootItem`] rows (vmangos `LootMgr.cpp:848-873`).
    Items {
        loot_type: u8,
        gold: u32,
        items: Vec<LootItem>,
    },
    /// Only a [`loot_error`] code follows (`Player.cpp:7744-7750`).
    Error { error: u8 },
}

/// `CMSG_LOOT` (`Loot.cpp:8-11`): the guid to open, always answered by `SMSG_LOOT_RESPONSE`.
pub fn loot(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_AUTOSTORE_LOOT_ITEM` (`Loot.cpp:3-6`): a [`LootItem::slot`]; the server picks the bag.
pub fn autostore_loot_item(loot_slot: u8) -> Vec<u8> {
    vec![loot_slot]
}

/// `CMSG_LOOT_MONEY`: an empty body (vmangos `Opcodes_1_12_1.h:351`).
pub fn loot_money() -> Vec<u8> {
    Vec::new()
}

/// `CMSG_LOOT_RELEASE` (`Loot.cpp:13-16`): a guid the server ignores for the loot guid it stored.
pub fn loot_release(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `RollVote`, the `rollType` of `CMSG_LOOT_ROLL` and `RollOnLoot`: 3 is Disenchant, sent only
/// when the start's vote mask has bit `1 << rollType` (`0x6fbc70`).
pub mod roll_vote {
    pub const PASS: u8 = 0;
    pub const NEED: u8 = 1;
    pub const GREED: u8 = 2;
    pub const DISENCHANT: u8 = 3;
}

/// `CMSG_LOOT_ROLL` (`Loot.cpp:18-23`): `u64 lootedTarget, u32 itemSlot, u8 rollType`. The
/// corpse and slot identify the roll; the FrameXML `rollID` never reaches the wire.
pub fn loot_roll(looted_target: u64, item_slot: u32, roll_type: u8) -> Vec<u8> {
    let mut body = looted_target.to_le_bytes().to_vec();
    body.extend_from_slice(&item_slot.to_le_bytes());
    body.push(roll_type);
    body
}

/// `CMSG_LOOT_MASTER_GIVE` (`Loot.cpp:25-30`): `u64 lootGuid, u8 slotId, u64 playerGuid`; the
/// slot is a `u8` here, a `u32` in the roll family. A refusal is a `MASTER_*` [`loot_error`].
pub fn loot_master_give(loot_guid: u64, slot: u8, player_guid: u64) -> Vec<u8> {
    let mut body = loot_guid.to_le_bytes().to_vec();
    body.push(slot);
    body.extend_from_slice(&player_guid.to_le_bytes());
    body
}

/// `SMSG_LOOT_START_ROLL` (`0x6fb2c0`): a group roll opened on one drop. `map_id` is the looted
/// object's map; a roll for another map than ours is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootStartRoll {
    pub looted_target: u64,
    pub map_id: u32,
    /// The wire [`LootItem::slot`], carried as a `u32` here.
    pub item_slot: u32,
    pub item_id: u32,
    pub random_suffix: u32,
    pub random_property_id: u32,
    /// The stack size, `GetLootRollItemInfo`'s `count`.
    pub count: u32,
    /// How long the roll stays open, in ms.
    pub countdown_ms: u32,
    /// The votes this roller may cast, bit `1 << rollType`: 0x1 Pass, 0x2 Need, 0x4 Greed, 0x8
    /// Disenchant.
    pub vote_mask: u8,
}

/// What one `SMSG_LOOT_ROLL` reports, as `0x6fa840` reads `(rollNumber, rollType)`: a signed
/// `rollNumber` below 0 (128 and up on the wire) is a vote, Greed on type 2, Disenchant on 3,
/// else Pass; at 0 and up, type 0 is the Need vote and 1, 2, 3 are the Need, Greed and
/// Disenchant dice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollAnnounce {
    Need,
    Greed,
    Disenchant,
    Pass,
    NeedRoll(u8),
    GreedRoll(u8),
    DisenchantRoll(u8),
}

/// `SMSG_LOOT_ROLL` (`0x6fae40`): one player's vote or dice roll; the last byte is the auto-pass
/// flag, an item the roller cannot loot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootRoll {
    pub looted_target: u64,
    pub item_slot: u32,
    pub roller: u64,
    pub item_id: u32,
    pub random_suffix: u32,
    pub random_property_id: u32,
    pub roll_number: u8,
    pub roll_type: u8,
    pub auto_pass: bool,
}

impl LootRoll {
    /// The announcement this `(rollNumber, rollType)` pair is (`0x6fa840`).
    pub fn announce(&self) -> RollAnnounce {
        let n = self.roll_number;
        match (n >= 128, self.roll_type) {
            (true, 2) => RollAnnounce::Greed,
            (true, 3) => RollAnnounce::Disenchant,
            (true, _) => RollAnnounce::Pass,
            (false, 0) => RollAnnounce::Need,
            (false, 2) => RollAnnounce::GreedRoll(n),
            (false, 3) => RollAnnounce::DisenchantRoll(n),
            (false, _) => RollAnnounce::NeedRoll(n),
        }
    }
}

/// `SMSG_LOOT_ROLL_WON` (`Loot.cpp:65-75`): `winner` took the item. Unlike [`LootRoll`], the
/// winner guid comes after the item fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootRollWon {
    pub looted_target: u64,
    pub item_slot: u32,
    pub item_id: u32,
    pub random_suffix: u32,
    pub random_property_id: u32,
    pub winner: u64,
    /// The winning dice value, or 100 with Need when only one player may roll (`Group.cpp:1110`).
    pub roll_number: u8,
    /// The [`roll_vote`] that won: 1 Need, 2 Greed, 3 Disenchant.
    pub roll_type: u8,
}

/// `SMSG_LOOT_ALL_PASSED` (`Loot.cpp:77-84`): everyone passed; the item stays for normal looting.
/// Unlike the rest of the family, `itemRandomPropId` comes before `randomSuffixId` here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootAllPassed {
    pub looted_target: u64,
    pub item_slot: u32,
    pub item_id: u32,
    pub random_property_id: u32,
    pub random_suffix: u32,
}

/// `SMSG_LOOT_LIST` (`0x71ca50` -> `0x71bc00`): a corpse's master looter and round-robin looter,
/// each a packed guid, 0 for none, shown on the dead unit's tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootList {
    pub creature: u64,
    pub master_looter: u64,
    pub looter: u64,
}

/// `SMSG_LOOT_SLOT_CHANGED` (`0x6d58d0`): one row of the open loot redrawn, by wire slot; the
/// slot type is not on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LootSlotChanged {
    pub looted_target: u64,
    pub slot: u8,
    pub item_id: u32,
    pub display_info_id: u32,
    pub random_suffix: u32,
    pub random_property_id: u32,
    pub count: u32,
}

pub(super) fn read_loot_start_roll(r: &mut &[u8]) -> io::Result<LootStartRoll> {
    Ok(LootStartRoll {
        looted_target: read_u64_le(r)?,
        map_id: read_u32_le(r)?,
        item_slot: read_u32_le(r)?,
        item_id: read_u32_le(r)?,
        random_suffix: read_u32_le(r)?,
        random_property_id: read_u32_le(r)?,
        count: read_u32_le(r)?,
        countdown_ms: read_u32_le(r)?,
        vote_mask: read_u8(r)?,
    })
}

pub(super) fn read_loot_roll(r: &mut &[u8]) -> io::Result<LootRoll> {
    Ok(LootRoll {
        looted_target: read_u64_le(r)?,
        item_slot: read_u32_le(r)?,
        roller: read_u64_le(r)?,
        item_id: read_u32_le(r)?,
        random_suffix: read_u32_le(r)?,
        random_property_id: read_u32_le(r)?,
        roll_number: read_u8(r)?,
        roll_type: read_u8(r)?,
        auto_pass: read_u8(r)? != 0,
    })
}

/// Read `SMSG_LOOT_LIST`.
pub(super) fn read_loot_list(r: &mut &[u8]) -> io::Result<LootList> {
    Ok(LootList {
        creature: read_u64_le(r)?,
        master_looter: crate::wire::read_packed_guid(r)?,
        looter: crate::wire::read_packed_guid(r)?,
    })
}

/// Read `SMSG_LOOT_SLOT_CHANGED`.
pub(super) fn read_loot_slot_changed(r: &mut &[u8]) -> io::Result<LootSlotChanged> {
    Ok(LootSlotChanged {
        looted_target: read_u64_le(r)?,
        slot: read_u8(r)?,
        item_id: read_u32_le(r)?,
        display_info_id: read_u32_le(r)?,
        random_suffix: read_u32_le(r)?,
        random_property_id: read_u32_le(r)?,
        count: read_u32_le(r)?,
    })
}

pub(super) fn read_loot_roll_won(r: &mut &[u8]) -> io::Result<LootRollWon> {
    Ok(LootRollWon {
        looted_target: read_u64_le(r)?,
        item_slot: read_u32_le(r)?,
        item_id: read_u32_le(r)?,
        random_suffix: read_u32_le(r)?,
        random_property_id: read_u32_le(r)?,
        winner: read_u64_le(r)?,
        roll_number: read_u8(r)?,
        roll_type: read_u8(r)?,
    })
}

/// `SMSG_LOOT_ALL_PASSED` (`0x6fb600`): the random property before the suffix, unlike the rest of
/// the family.
pub(super) fn read_loot_all_passed(r: &mut &[u8]) -> io::Result<LootAllPassed> {
    Ok(LootAllPassed {
        looted_target: read_u64_le(r)?,
        item_slot: read_u32_le(r)?,
        item_id: read_u32_le(r)?,
        random_property_id: read_u32_le(r)?,
        random_suffix: read_u32_le(r)?,
    })
}

/// `SMSG_LOOT_MASTER_LIST` (vmangos `Server/Packets/Group.cpp:182-187`): `u8 count`, then the
/// eligible members' guids. Every member who opens the corpse gets it just before
/// `SMSG_LOOT_RESPONSE` (`Player.cpp:8077-8081`), filtered by range (`Group/Group.cpp:914-940`).
pub(super) fn read_loot_master_list(r: &mut &[u8]) -> io::Result<Vec<u64>> {
    let count = read_u8(r)?;
    // At most a raid: vmangos `MAX_RAID_SIZE` is 40 (`Group/Group.h:50`).
    let mut candidates = Vec::with_capacity(capacity_hint(count, 40));
    for _ in 0..count {
        candidates.push(read_u64_le(r)?);
    }
    Ok(candidates)
}

/// `SMSG_LOOT_RESPONSE` (vmangos `Player.cpp:8138-8141`): `u64 guid, u8 lootType`, then the
/// error code when `lootType` is 0, else gold and the [`LootItem`] rows.
pub(super) fn read_loot_response(r: &mut &[u8]) -> io::Result<(u64, LootResponseBody)> {
    let guid = read_u64_le(r)?;
    let loot_type = read_u8(r)?;
    if loot_type == 0 {
        let error = read_u8(r)?;
        return Ok((guid, LootResponseBody::Error { error }));
    }
    let gold = read_u32_le(r)?;
    let count = read_u8(r)?;
    // `MAX_NR_LOOT_ITEMS` 16 + `MAX_NR_QUEST_ITEMS` 32 (vmangos `LootMgr.h:34,36`).
    let mut items = Vec::with_capacity(capacity_hint(count, 16 + 32));
    for _ in 0..count {
        let slot = read_u8(r)?;
        let item_id = read_u32_le(r)?;
        let item_count = read_u32_le(r)?;
        let display_info_id = read_u32_le(r)?;
        let _random_suffix = read_u32_le(r)?; // always a literal 0 on the wire (LootMgr.cpp:842)
        let random_property_id = read_u32_le(r)?;
        let slot_type = read_u8(r)?;
        items.push(LootItem {
            slot,
            item_id,
            count: item_count,
            display_info_id,
            random_property_id,
            slot_type,
        });
    }
    Ok((
        guid,
        LootResponseBody::Items {
            loot_type,
            gold,
            items,
        },
    ))
}

/// `SMSG_LOOT_RELEASE_RESPONSE` (`Loot.h:137-145`): `u64 guid, u8 result`; the result is always 1.
pub(super) fn read_loot_release_response(r: &mut &[u8]) -> io::Result<(u64, u8)> {
    Ok((read_u64_le(r)?, read_u8(r)?))
}

/// `SMSG_LOOT_REMOVED` (`Loot.h:147-154`): the wire slot just taken, sent to every looter.
pub(super) fn read_loot_removed(r: &mut &[u8]) -> io::Result<u8> {
    read_u8(r)
}

/// `SMSG_LOOT_MONEY_NOTIFY` (`Loot.h:69-76`): our share of the coin, split equally among looters.
pub(super) fn read_loot_money_notify(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// `SMSG_ITEM_PUSH_RESULT` (vmangos `Server/Packets/Item.cpp:211-224`, both post-1.10.2 fields
/// present at 5875): the "You receive …" chat line. `item_slot` is `0xFFFF_FFFF` when it stacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemPushResult {
    pub player_guid: u64,
    /// `false` = looted, `true` = received from an NPC (vendor purchase, quest reward).
    pub from_npc: bool,
    /// Crafted or conjured, giving the "You create: …" line; not a stack-merge flag (vmangos
    /// `SendNewItem`: "0=received, 1=created").
    pub created: bool,
    pub show_in_chat: bool,
    /// The bag it went to, numbered as [`super::items::BAG_PLAYER_INVENTORY`] and
    /// [`super::items::SLOT_BAG_FIRST`].
    pub bag_slot: u8,
    pub item_slot: u32,
    pub item_entry: u32,
    pub suffix_factor: u32,
    pub random_property_id: u32,
    pub count: u32,
    /// The bags' count of the entry after the push, which the quest-item toast reads
    /// (`0x6d7de0`).
    pub total_count: u32,
}

pub(super) fn read_item_push_result(r: &mut &[u8]) -> io::Result<ItemPushResult> {
    let player_guid = read_u64_le(r)?;
    let received = read_u32_le(r)?;
    let created = read_u32_le(r)?;
    let show_in_chat = read_u32_le(r)?;
    let bag_slot = read_u8(r)?;
    let item_slot = read_u32_le(r)?;
    let item_entry = read_u32_le(r)?;
    let suffix_factor = read_u32_le(r)?;
    let random_property_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    let total_count = read_u32_le(r)?;
    Ok(ItemPushResult {
        player_guid,
        from_npc: received != 0,
        created: created != 0,
        show_in_chat: show_in_chat != 0,
        bag_slot,
        item_slot,
        item_entry,
        suffix_factor,
        random_property_id,
        count,
        total_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cmsg_bodies_golden() {
        assert_eq!(
            loot(0x1234_5678_9abc_def0),
            hx("f0debc9a78563412"),
            "CMSG_LOOT body"
        );

        assert_eq!(
            autostore_loot_item(3),
            hx("03"),
            "CMSG_AUTOSTORE_LOOT_ITEM body"
        );

        assert_eq!(loot_money(), Vec::<u8>::new(), "CMSG_LOOT_MONEY body");

        assert_eq!(
            loot_release(0x1234_5678_9abc_def0),
            hx("f0debc9a78563412"),
            "CMSG_LOOT_RELEASE body"
        );

        assert_eq!(
            loot_roll(0x1234_5678_9abc_def0, 2, roll_vote::GREED),
            hx("f0debc9a785634120200000002"),
            "CMSG_LOOT_ROLL body"
        );

        let give = loot_master_give(0x1234_5678_9abc_def0, 2, 0x0fed_cba9_8765_4321);
        assert_eq!(
            give,
            hx("f0debc9a785634120221436587a9cbed0f"),
            "CMSG_LOOT_MASTER_GIVE body"
        );
        assert_eq!(give.len(), 17, "8 + 1 + 8, not 8 + 4 + 8");
    }

    /// The roll and master-loot opcodes (vmangos `Opcodes_1_12_1.h:671-677`).
    #[test]
    fn roll_opcode_values() {
        assert_eq!(opcode::SMSG_LOOT_ALL_PASSED, 670);
        assert_eq!(opcode::SMSG_LOOT_ROLL_WON, 671);
        assert_eq!(opcode::CMSG_LOOT_ROLL, 672);
        assert_eq!(opcode::SMSG_LOOT_START_ROLL, 673);
        assert_eq!(opcode::SMSG_LOOT_ROLL, 674);
        assert_eq!(opcode::CMSG_LOOT_MASTER_GIVE, 675);
        assert_eq!(opcode::SMSG_LOOT_MASTER_LIST, 676);
    }

    #[test]
    fn loot_master_list_decodes() {
        let mut body = vec![3u8];
        for guid in [0xAAu64, 0xBB, 0xCC] {
            body.extend_from_slice(&guid.to_le_bytes());
        }

        match parse_server(opcode::SMSG_LOOT_MASTER_LIST, &body).unwrap() {
            ServerPacket::LootMasterList { candidates } => {
                assert_eq!(candidates, vec![0xAA, 0xBB, 0xCC]);
            }
            other => panic!("expected LootMasterList, got {}", other.name()),
        }
    }

    /// `Group::MasterLoot` filters by range (`Group/Group.cpp:925-937`): `count = 0` is legal.
    #[test]
    fn loot_master_list_accepts_an_empty_list() {
        match parse_server(opcode::SMSG_LOOT_MASTER_LIST, &[0u8]).unwrap() {
            ServerPacket::LootMasterList { candidates } => assert!(candidates.is_empty()),
            other => panic!("expected LootMasterList, got {}", other.name()),
        }
    }

    /// The 12340 body `0x6fb2c0` reads, as `Group::SendLootStartRoll` writes it.
    #[test]
    fn loot_start_roll_decodes() {
        let mut body = 0xAAu64.to_le_bytes().to_vec();
        for v in [574u32, 1, 17182, 9, 7, 2, 60_000] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.push(0x0F);
        match parse_server(opcode::SMSG_LOOT_START_ROLL, &body).unwrap() {
            ServerPacket::LootStartRoll(p) => assert_eq!(
                p,
                LootStartRoll {
                    looted_target: 0xAA,
                    map_id: 574,
                    item_slot: 1,
                    item_id: 17182,
                    random_suffix: 9,
                    random_property_id: 7,
                    count: 2,
                    countdown_ms: 60_000,
                    vote_mask: 0x0F,
                }
            ),
            other => panic!("expected LootStartRoll, got {}", other.name()),
        }
    }

    /// `0x6fa840`'s split: a signed roll number below 0 is a vote, keyed by type.
    #[test]
    fn loot_roll_decodes_and_names_every_shape() {
        let announce = |roll_number: u8, roll_type: u8, auto_pass: u8| {
            let mut body = 0xAAu64.to_le_bytes().to_vec();
            body.extend_from_slice(&1u32.to_le_bytes());
            body.extend_from_slice(&0xBBu64.to_le_bytes());
            body.extend_from_slice(&17182u32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&[roll_number, roll_type, auto_pass]);
            match parse_server(opcode::SMSG_LOOT_ROLL, &body).unwrap() {
                ServerPacket::LootRoll(p) => p,
                other => panic!("expected LootRoll, got {}", other.name()),
            }
        };
        let auto = announce(128, 0, 1);
        assert!(auto.auto_pass);
        assert_eq!(auto.announce(), RollAnnounce::Pass);
        for (n, t, want) in [
            (0, 0, RollAnnounce::Need),
            (128, 2, RollAnnounce::Greed),
            (128, 3, RollAnnounce::Disenchant),
            (128, 0, RollAnnounce::Pass),
            (57, 1, RollAnnounce::NeedRoll(57)),
            (57, 2, RollAnnounce::GreedRoll(57)),
            (57, 3, RollAnnounce::DisenchantRoll(57)),
        ] {
            assert_eq!(announce(n, t, 0).announce(), want, "({n}, {t})");
        }
    }

    #[test]
    fn loot_roll_won_decodes() {
        let mut body = 0xAAu64.to_le_bytes().to_vec();
        body.extend_from_slice(&1u32.to_le_bytes()); // itemSlot
        body.extend_from_slice(&17182u32.to_le_bytes()); // itemEntryId
        body.extend_from_slice(&9u32.to_le_bytes()); // randomSuffix
        body.extend_from_slice(&0u32.to_le_bytes()); // itemRandomPropId
        body.extend_from_slice(&0xBBu64.to_le_bytes()); // winnerGuid
        body.push(84); // rollNumber
        body.push(roll_vote::DISENCHANT);

        match parse_server(opcode::SMSG_LOOT_ROLL_WON, &body).unwrap() {
            ServerPacket::LootRollWon(p) => assert_eq!(
                p,
                LootRollWon {
                    looted_target: 0xAA,
                    item_slot: 1,
                    item_id: 17182,
                    random_suffix: 9,
                    random_property_id: 0,
                    winner: 0xBB,
                    roll_number: 84,
                    roll_type: roll_vote::DISENCHANT,
                }
            ),
            other => panic!("expected LootRollWon, got {}", other.name()),
        }
    }

    #[test]
    fn loot_all_passed_decodes() {
        // The property ahead of the suffix (`0x6fb600`).
        let mut body = 0xAAu64.to_le_bytes().to_vec();
        for v in [1u32, 17182, 7, 9] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        match parse_server(opcode::SMSG_LOOT_ALL_PASSED, &body).unwrap() {
            ServerPacket::LootAllPassed(p) => assert_eq!(
                p,
                LootAllPassed {
                    looted_target: 0xAA,
                    item_slot: 1,
                    item_id: 17182,
                    random_property_id: 7,
                    random_suffix: 9,
                }
            ),
            other => panic!("expected LootAllPassed, got {}", other.name()),
        }
    }

    /// `Group::SendLooter`: the corpse, then two packed guids, an empty one a lone 0.
    #[test]
    fn loot_list_decodes() {
        let mut body = 0xF130_0000_0000_00AAu64.to_le_bytes().to_vec();
        body.extend_from_slice(&[0x01, 0x2A]); // packed 0x2A
        body.push(0); // no looter
        match parse_server(opcode::SMSG_LOOT_LIST, &body).unwrap() {
            ServerPacket::LootList(p) => assert_eq!(
                p,
                LootList {
                    creature: 0xF130_0000_0000_00AA,
                    master_looter: 0x2A,
                    looter: 0,
                }
            ),
            other => panic!("expected LootList, got {}", other.name()),
        }
    }

    /// `0x6d58d0`: the guid, a `u8` slot, then five `u32`.
    #[test]
    fn loot_slot_changed_decodes() {
        let mut body = 0xAAu64.to_le_bytes().to_vec();
        body.push(3);
        for v in [2589u32, 7629, 0, 0, 4] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        match parse_server(opcode::SMSG_LOOT_SLOT_CHANGED, &body).unwrap() {
            ServerPacket::LootSlotChanged(p) => assert_eq!(
                p,
                LootSlotChanged {
                    looted_target: 0xAA,
                    slot: 3,
                    item_id: 2589,
                    display_info_id: 7629,
                    random_suffix: 0,
                    random_property_id: 0,
                    count: 4,
                }
            ),
            other => panic!("expected LootSlotChanged, got {}", other.name()),
        }
    }

    #[test]
    fn loot_response_items_shape_decodes() {
        let mut body = 0xAAu64.to_le_bytes().to_vec();
        body.push(loot_type::CORPSE);
        body.extend_from_slice(&1234u32.to_le_bytes()); // gold
        body.push(2); // item count
                      // slot 0: entry 117, count 1, display 123, suffix 0, prop 0, ALLOW_LOOT
        body.push(0);
        body.extend_from_slice(&117u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&123u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(slot_type::ALLOW_LOOT);
        // slot 1: entry 6948 (Hearthstone), count 1, display 4500, suffix 0, prop 0, LOCKED
        body.push(1);
        body.extend_from_slice(&6948u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&4500u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(slot_type::LOCKED);

        match parse_server(opcode::SMSG_LOOT_RESPONSE, &body).unwrap() {
            ServerPacket::LootResponse {
                guid,
                loot_type,
                gold,
                items,
            } => {
                assert_eq!(guid, 0xAA);
                assert_eq!(loot_type, loot_type::CORPSE);
                assert_eq!(gold, 1234);
                assert_eq!(
                    items,
                    vec![
                        LootItem {
                            slot: 0,
                            item_id: 117,
                            count: 1,
                            display_info_id: 123,
                            random_property_id: 0,
                            slot_type: slot_type::ALLOW_LOOT,
                        },
                        LootItem {
                            slot: 1,
                            item_id: 6948,
                            count: 1,
                            display_info_id: 4500,
                            random_property_id: 0,
                            slot_type: slot_type::LOCKED,
                        },
                    ]
                );
            }
            other => panic!("expected LootResponse, got {}", other.name()),
        }
    }

    #[test]
    fn loot_response_error_shape_decodes() {
        let mut body = 0xBBu64.to_le_bytes().to_vec();
        body.push(0); // lootType 0 ⇒ error shape
        body.push(loot_error::TOO_FAR);

        match parse_server(opcode::SMSG_LOOT_RESPONSE, &body).unwrap() {
            ServerPacket::LootError { guid, error } => {
                assert_eq!(guid, 0xBB);
                assert_eq!(error, loot_error::TOO_FAR);
            }
            other => panic!("expected LootError, got {}", other.name()),
        }
    }

    #[test]
    fn loot_release_response_wire() {
        let mut body = 0xCCu64.to_le_bytes().to_vec();
        body.push(1);
        match parse_server(opcode::SMSG_LOOT_RELEASE_RESPONSE, &body).unwrap() {
            ServerPacket::LootReleaseResponse { guid, result } => {
                assert_eq!(guid, 0xCC);
                assert_eq!(result, 1);
            }
            other => panic!("expected LootReleaseResponse, got {}", other.name()),
        }
    }

    #[test]
    fn loot_removed_wire() {
        let body = vec![5u8];
        match parse_server(opcode::SMSG_LOOT_REMOVED, &body).unwrap() {
            ServerPacket::LootRemoved { slot } => assert_eq!(slot, 5),
            other => panic!("expected LootRemoved, got {}", other.name()),
        }
    }

    #[test]
    fn loot_money_notify_wire() {
        let body = 777u32.to_le_bytes().to_vec();
        match parse_server(opcode::SMSG_LOOT_MONEY_NOTIFY, &body).unwrap() {
            ServerPacket::LootMoneyNotify { amount } => assert_eq!(amount, 777),
            other => panic!("expected LootMoneyNotify, got {}", other.name()),
        }
    }

    #[test]
    fn loot_clear_money_wire() {
        match parse_server(opcode::SMSG_LOOT_CLEAR_MONEY, &[]).unwrap() {
            ServerPacket::LootClearMoney => {}
            other => panic!("expected LootClearMoney, got {}", other.name()),
        }
    }

    #[test]
    fn item_push_result_wire() {
        let mut body = 0x42u64.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes()); // received: looted
        body.extend_from_slice(&1u32.to_le_bytes()); // created: new item
        body.extend_from_slice(&1u32.to_le_bytes()); // showInChat
        body.push(255); // bagSlot: player backpack
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // itemSlot: stacked
        body.extend_from_slice(&117u32.to_le_bytes()); // itemEntry
        body.extend_from_slice(&0u32.to_le_bytes()); // suffixFactor
        body.extend_from_slice(&0u32.to_le_bytes()); // randomPropertyId
        body.extend_from_slice(&5u32.to_le_bytes()); // count
        body.extend_from_slice(&12u32.to_le_bytes()); // totalCount

        match parse_server(opcode::SMSG_ITEM_PUSH_RESULT, &body).unwrap() {
            ServerPacket::ItemPushResult(p) => {
                assert_eq!(
                    p,
                    ItemPushResult {
                        player_guid: 0x42,
                        from_npc: false,
                        created: true,
                        show_in_chat: true,
                        bag_slot: 255,
                        item_slot: 0xFFFF_FFFF,
                        item_entry: 117,
                        suffix_factor: 0,
                        random_property_id: 0,
                        count: 5,
                        total_count: 12,
                    }
                );
            }
            other => panic!("expected ItemPushResult, got {}", other.name()),
        }
    }
}
