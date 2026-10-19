//! The dungeon finder's messages, as 12340's `LFGPacketHandler` (`0x55bdc0`) and
//! `Packet_SMSG_LFG_OPEN_FROM_GOSSIP` (`0x5528d0`) read them, and the bodies its verbs send. A
//! dungeon on the wire is a slot: the `LfgDungeons.dbc` id in the low 24 bits, its type above.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// One `SMSG_LFG_UPDATE_PLAYER` or `SMSG_LFG_UPDATE_PARTY`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgUpdate {
    /// What moved: 7 and 15 are a leave, 10 a proposal that formed the group.
    pub update_type: u8,
    pub joined: bool,
    /// The party's own "LFG joined" byte; the player update reuses `joined`.
    pub lfg_joined: bool,
    pub queued: bool,
    /// The join's two option bytes, echoed back.
    pub no_partial_clear: bool,
    pub achievements: bool,
    /// The party update's three needs bytes; zero for the player.
    pub needs: [u8; 3],
    pub slots: Vec<u32>,
    pub comment: String,
}

/// `SMSG_LFG_QUEUE_STATUS`: the five waits land in `[0xbeaa2c..0xbeaa3c]` in wire order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgQueueStatus {
    pub dungeon: u32,
    /// `[0xbeaa2c]`, `GetLFGQueueStats`' `myWait`.
    pub my_wait: i32,
    /// `[0xbeaa30]`, its `averageWait`.
    pub average_wait: i32,
    pub tank_wait: i32,
    pub healer_wait: i32,
    pub damage_wait: i32,
    pub tanks_needed: u8,
    pub healers_needed: u8,
    pub damage_needed: u8,
    /// Seconds already queued.
    pub queued_secs: u32,
}

/// One member row of `SMSG_LFG_PROPOSAL_UPDATE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgProposalMember {
    pub roles: u32,
    pub is_local: bool,
    pub in_dungeon: bool,
    pub same_group: bool,
    pub answered: bool,
    pub accepted: bool,
}

/// `SMSG_LFG_PROPOSAL_UPDATE`: a group offer and its members' answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgProposal {
    pub slot: u32,
    pub state: u8,
    pub id: u32,
    pub completed_mask: u32,
    /// Do not show the offer window: the group is already in the dungeon.
    pub silent: bool,
    pub members: Vec<LfgProposalMember>,
}

/// One member row of `SMSG_LFG_ROLE_CHECK_UPDATE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgRoleCheckMember {
    pub guid: u64,
    pub confirmed: bool,
    pub roles: u32,
    pub level: u8,
}

/// `SMSG_LFG_ROLE_CHECK_UPDATE`: the party leader's role poll.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgRoleCheck {
    /// 2 in progress, 5 aborted, 6 failed; the rest end it quietly.
    pub state: u32,
    pub beginning: bool,
    pub slots: Vec<u32>,
    pub members: Vec<LfgRoleCheckMember>,
}

/// `SMSG_LFG_BOOT_PROPOSAL_UPDATE`: a vote to kick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgBootProposal {
    pub in_progress: bool,
    pub did_vote: bool,
    pub my_vote: bool,
    pub target: u64,
    pub total_votes: u32,
    pub boot_votes: u32,
    pub time_left: u32,
    pub votes_needed: u32,
    pub reason: String,
}

/// One reward item: `(item entry, ItemDisplayInfo id, count)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfgRewardItem {
    pub item: u32,
    pub display: u32,
    pub count: u32,
}

/// A random or seasonal dungeon's rewards in `SMSG_LFG_PLAYER_INFO`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeonReward {
    pub slot: u32,
    /// Today's first-run reward is taken.
    pub done: bool,
    pub base_money: i32,
    pub base_xp: i32,
    pub variable_money: i32,
    pub variable_xp: i32,
    pub items: Vec<LfgRewardItem>,
}

/// One player's locked dungeons, `(slot, reason)` (`ParseBlackList 0x559b10`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgMemberLocks {
    pub guid: u64,
    pub locks: Vec<(u32, u32)>,
}

/// `SMSG_LFG_PLAYER_REWARD`: the dungeon's completion reward.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgCompletionReward {
    pub random_slot: u32,
    pub actual_slot: u32,
    pub first: bool,
    pub strangers: i32,
    pub base_money: i32,
    pub base_xp: i32,
    pub variable_money: i32,
    pub variable_xp: i32,
    pub items: Vec<LfgRewardItem>,
}

/// Every dungeon finder server message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LfgPacket {
    UpdatePlayer(LfgUpdate),
    UpdateParty(LfgUpdate),
    /// `SMSG_LFG_UPDATE_SEARCH`: whether the raid browser search is still joined.
    UpdateSearch {
        joined: bool,
    },
    QueueStatus(LfgQueueStatus),
    /// `SMSG_LFG_DISABLED`, empty.
    Disabled,
    Proposal(LfgProposal),
    RoleCheck(LfgRoleCheck),
    RoleChosen {
        guid: u64,
        accepted: bool,
        roles: u32,
    },
    /// `SMSG_LFG_JOIN_RESULT`; result 6 carries the party's locks.
    JoinResult {
        result: u32,
        reason: u32,
        party_locks: Option<Vec<LfgMemberLocks>>,
    },
    Boot(LfgBootProposal),
    PlayerInfo {
        rewards: Vec<LfgDungeonReward>,
        locks: Vec<(u32, u32)>,
    },
    PartyInfo(Vec<LfgMemberLocks>),
    Reward(LfgCompletionReward),
    TeleportDenied(u32),
    OfferContinue(u32),
    /// `SMSG_OPEN_LFG_DUNGEON_FINDER`: a gossip option opens the window on a dungeon.
    OpenFromGossip(u32),
}

impl LfgPacket {
    /// The opcode's name, for the packet trace.
    pub fn name(&self) -> &'static str {
        match self {
            Self::UpdatePlayer(_) => "SMSG_LFG_UPDATE_PLAYER",
            Self::UpdateParty(_) => "SMSG_LFG_UPDATE_PARTY",
            Self::UpdateSearch { .. } => "SMSG_LFG_UPDATE_SEARCH",
            Self::QueueStatus(_) => "SMSG_LFG_QUEUE_STATUS",
            Self::Disabled => "SMSG_LFG_DISABLED",
            Self::Proposal(_) => "SMSG_LFG_PROPOSAL_UPDATE",
            Self::RoleCheck(_) => "SMSG_LFG_ROLE_CHECK_UPDATE",
            Self::RoleChosen { .. } => "SMSG_LFG_ROLE_CHOSEN",
            Self::JoinResult { .. } => "SMSG_LFG_JOIN_RESULT",
            Self::Boot(_) => "SMSG_LFG_BOOT_PROPOSAL_UPDATE",
            Self::PlayerInfo { .. } => "SMSG_LFG_PLAYER_INFO",
            Self::PartyInfo(_) => "SMSG_LFG_PARTY_INFO",
            Self::Reward(_) => "SMSG_LFG_PLAYER_REWARD",
            Self::TeleportDenied(_) => "SMSG_LFG_TELEPORT_DENIED",
            Self::OfferContinue(_) => "SMSG_LFG_OFFER_CONTINUE",
            Self::OpenFromGossip(_) => "SMSG_OPEN_LFG_DUNGEON_FINDER",
        }
    }
}

fn flag(r: &mut impl Read) -> io::Result<bool> {
    Ok(read_u8(r)? != 0)
}

fn i32_le(r: &mut impl Read) -> io::Result<i32> {
    Ok(read_u32_le(r)? as i32)
}

fn read_slots(r: &mut impl Read) -> io::Result<Vec<u32>> {
    let n = read_u8(r)?;
    (0..n).map(|_| read_u32_le(r)).collect()
}

/// `SMSG_LFG_UPDATE_PLAYER`: the extra block only behind the joined byte.
pub(super) fn read_update_player(r: &mut impl Read) -> io::Result<LfgUpdate> {
    let update_type = read_u8(r)?;
    let joined = flag(r)?;
    let mut u = LfgUpdate {
        update_type,
        joined,
        lfg_joined: joined,
        ..Default::default()
    };
    if joined {
        u.queued = flag(r)?;
        u.no_partial_clear = flag(r)?;
        u.achievements = flag(r)?;
        u.slots = read_slots(r)?;
        u.comment = read_cstring(r)?;
    }
    Ok(u)
}

/// `SMSG_LFG_UPDATE_PARTY`: the player's layout plus the party's joined byte and
/// three needs bytes.
pub(super) fn read_update_party(r: &mut impl Read) -> io::Result<LfgUpdate> {
    let update_type = read_u8(r)?;
    let joined = flag(r)?;
    let mut u = LfgUpdate {
        update_type,
        joined,
        ..Default::default()
    };
    if joined {
        u.lfg_joined = flag(r)?;
        u.queued = flag(r)?;
        u.no_partial_clear = flag(r)?;
        u.achievements = flag(r)?;
        for need in &mut u.needs {
            *need = read_u8(r)?;
        }
        u.slots = read_slots(r)?;
        u.comment = read_cstring(r)?;
    }
    Ok(u)
}

pub(super) fn read_queue_status(r: &mut impl Read) -> io::Result<LfgQueueStatus> {
    Ok(LfgQueueStatus {
        dungeon: read_u32_le(r)?,
        my_wait: i32_le(r)?,
        average_wait: i32_le(r)?,
        tank_wait: i32_le(r)?,
        healer_wait: i32_le(r)?,
        damage_wait: i32_le(r)?,
        tanks_needed: read_u8(r)?,
        healers_needed: read_u8(r)?,
        damage_needed: read_u8(r)?,
        queued_secs: read_u32_le(r)?,
    })
}

/// `SMSG_LFG_PROPOSAL_UPDATE`; members past the fifth are not read.
pub(super) fn read_proposal(r: &mut impl Read) -> io::Result<LfgProposal> {
    let mut p = LfgProposal {
        slot: read_u32_le(r)?,
        state: read_u8(r)?,
        id: read_u32_le(r)?,
        completed_mask: read_u32_le(r)?,
        silent: flag(r)?,
        members: Vec::new(),
    };
    let n = read_u8(r)?.min(5);
    for _ in 0..n {
        p.members.push(LfgProposalMember {
            roles: read_u32_le(r)?,
            is_local: flag(r)?,
            in_dungeon: flag(r)?,
            same_group: flag(r)?,
            answered: flag(r)?,
            accepted: flag(r)?,
        });
    }
    Ok(p)
}

pub(super) fn read_role_check(r: &mut impl Read) -> io::Result<LfgRoleCheck> {
    let state = read_u32_le(r)?;
    let beginning = flag(r)?;
    let slots = read_slots(r)?;
    let n = read_u8(r)?;
    let mut members = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        members.push(LfgRoleCheckMember {
            guid: read_u64_le(r)?,
            confirmed: flag(r)?,
            roles: read_u32_le(r)?,
            level: read_u8(r)?,
        });
    }
    Ok(LfgRoleCheck {
        state,
        beginning,
        slots,
        members,
    })
}

/// A lock block: `u32` count, then `(slot, reason)` pairs (`ParseBlackList 0x559b10`).
fn read_locks(r: &mut impl Read) -> io::Result<Vec<(u32, u32)>> {
    let n = read_u32_le(r)?;
    let mut locks = Vec::with_capacity(capacity_hint(n, 256));
    for _ in 0..n {
        locks.push((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok(locks)
}

/// A party lock block: `u8` count of `(guid, lock block)` (`ParsePartyInfo 0x55b080`).
fn read_party_locks(r: &mut impl Read) -> io::Result<Vec<LfgMemberLocks>> {
    let n = read_u8(r)?;
    (0..n)
        .map(|_| {
            Ok(LfgMemberLocks {
                guid: read_u64_le(r)?,
                locks: read_locks(r)?,
            })
        })
        .collect()
}

/// `SMSG_LFG_JOIN_RESULT`: only result 6 reads the party lock block (`cmp 6` at `0x55cca1`); a
/// server that sends it with another result leaves it unread, and one that sends result 6 with
/// no locks ends the body there, which reads as no block.
pub(super) fn read_join_result(r: &mut &[u8]) -> io::Result<LfgPacket> {
    let result = read_u32_le(r)?;
    let reason = read_u32_le(r)?;
    let party_locks = if result == 6 && !r.is_empty() {
        Some(read_party_locks(r)?)
    } else {
        None
    };
    Ok(LfgPacket::JoinResult {
        result,
        reason,
        party_locks,
    })
}

pub(super) fn read_boot(r: &mut impl Read) -> io::Result<LfgBootProposal> {
    Ok(LfgBootProposal {
        in_progress: flag(r)?,
        did_vote: flag(r)?,
        my_vote: flag(r)?,
        target: read_u64_le(r)?,
        total_votes: read_u32_le(r)?,
        boot_votes: read_u32_le(r)?,
        time_left: read_u32_le(r)?,
        votes_needed: read_u32_le(r)?,
        reason: read_cstring(r)?,
    })
}

fn read_items(r: &mut impl Read) -> io::Result<Vec<LfgRewardItem>> {
    let n = read_u8(r)?;
    (0..n)
        .map(|_| {
            Ok(LfgRewardItem {
                item: read_u32_le(r)?,
                display: read_u32_le(r)?,
                count: read_u32_le(r)?,
            })
        })
        .collect()
}

/// `SMSG_LFG_PLAYER_INFO`: the random dungeons' rewards, then the lock block.
pub(super) fn read_player_info(r: &mut impl Read) -> io::Result<LfgPacket> {
    let n = read_u8(r)?;
    let mut rewards = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        rewards.push(LfgDungeonReward {
            slot: read_u32_le(r)?,
            done: flag(r)?,
            base_money: i32_le(r)?,
            base_xp: i32_le(r)?,
            variable_money: i32_le(r)?,
            variable_xp: i32_le(r)?,
            items: read_items(r)?,
        });
    }
    let locks = read_locks(r)?;
    Ok(LfgPacket::PlayerInfo { rewards, locks })
}

pub(super) fn read_party_info(r: &mut impl Read) -> io::Result<LfgPacket> {
    Ok(LfgPacket::PartyInfo(read_party_locks(r)?))
}

pub(super) fn read_reward(r: &mut impl Read) -> io::Result<LfgCompletionReward> {
    Ok(LfgCompletionReward {
        random_slot: read_u32_le(r)?,
        actual_slot: read_u32_le(r)?,
        first: flag(r)?,
        strangers: i32_le(r)?,
        base_money: i32_le(r)?,
        base_xp: i32_le(r)?,
        variable_money: i32_le(r)?,
        variable_xp: i32_le(r)?,
        items: read_items(r)?,
    })
}

pub(super) fn read_role_chosen(r: &mut impl Read) -> io::Result<LfgPacket> {
    Ok(LfgPacket::RoleChosen {
        guid: read_u64_le(r)?,
        accepted: flag(r)?,
        roles: read_u32_le(r)?,
    })
}

/// What `JoinLFG` sends (`0x5572d0`): the roles, the two option bytes, the slots, the three
/// needs bytes behind their count, and the comment.
pub struct LfgJoin<'a> {
    pub roles: u32,
    pub no_partial_clear: bool,
    pub achievements: bool,
    pub slots: &'a [u32],
    pub needs: [u8; 3],
    pub comment: &'a str,
}

/// `CMSG_LFG_JOIN`'s body.
pub fn lfg_join(join: &LfgJoin<'_>) -> Vec<u8> {
    let mut body = join.roles.to_le_bytes().to_vec();
    body.push(u8::from(join.no_partial_clear));
    body.push(u8::from(join.achievements));
    body.push(join.slots.len().min(255) as u8);
    for slot in join.slots.iter().take(255) {
        body.extend_from_slice(&slot.to_le_bytes());
    }
    body.push(3);
    body.extend_from_slice(&join.needs);
    body.extend_from_slice(join.comment.as_bytes());
    body.push(0);
    body
}

/// `CMSG_LFG_PROPOSAL_RESULT`'s body (`0x554450`): the proposal id and the answer.
pub fn lfg_proposal_result(id: u32, accept: bool) -> Vec<u8> {
    let mut body = id.to_le_bytes().to_vec();
    body.push(u8::from(accept));
    body
}

/// A cstring body: `CMSG_SET_LFG_COMMENT`.
pub fn lfg_comment(comment: &str) -> Vec<u8> {
    let mut body = comment.as_bytes().to_vec();
    body.push(0);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// A joined player update reads the block; a left one stops after its two bytes.
    #[test]
    fn the_player_update_reads_its_block_only_when_joined() {
        let mut body = vec![12, 1, 1, 0, 0, 2];
        body.extend_from_slice(&le(261 | 6 << 24));
        body.extend_from_slice(&le(262 | 6 << 24));
        body.extend_from_slice(b"hi\0");
        let mut r = &body[..];
        let u = read_update_player(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(u.update_type, 12);
        assert!(u.joined && u.lfg_joined && u.queued);
        assert_eq!(u.slots, [261 | 6 << 24, 262 | 6 << 24]);
        assert_eq!(u.comment, "hi");

        let body = [7u8, 0];
        let mut r = &body[..];
        let u = read_update_player(&mut r).unwrap();
        assert!(r.is_empty() && !u.joined && u.slots.is_empty());
    }

    /// The party layout adds its joined byte and the three needs before the slots.
    #[test]
    fn the_party_update_reads_its_needs_bytes() {
        let mut body = vec![13, 1, 1, 0, 0, 0, 1, 2, 3, 1];
        body.extend_from_slice(&le(261));
        body.push(0);
        let mut r = &body[..];
        let u = read_update_party(&mut r).unwrap();
        assert!(r.is_empty());
        assert!(u.joined && u.lfg_joined && !u.queued);
        assert_eq!(u.needs, [1, 2, 3]);
        assert_eq!(u.slots, [261]);
    }

    /// The five waits come in `[0xbeaa2c..0xbeaa3c]` order, then the needs and the time.
    #[test]
    fn the_queue_status_reads_its_waits_in_wire_order() {
        let mut body = le(261).to_vec();
        for w in [10, 20, 30, 40, 50] {
            body.extend_from_slice(&le(w));
        }
        body.extend_from_slice(&[1, 0, 3]);
        body.extend_from_slice(&le(99));
        let mut r = &body[..];
        let s = read_queue_status(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            (
                s.my_wait,
                s.average_wait,
                s.tank_wait,
                s.healer_wait,
                s.damage_wait
            ),
            (10, 20, 30, 40, 50)
        );
        assert_eq!((s.tanks_needed, s.damage_needed, s.queued_secs), (1, 3, 99));
    }

    /// Five members are the most a proposal holds; each row is a `u32` and five bytes.
    #[test]
    fn a_proposal_reads_each_member_row() {
        let mut body = le(261).to_vec();
        body.push(1);
        body.extend_from_slice(&le(7));
        body.extend_from_slice(&le(0b11));
        body.extend_from_slice(&[0, 2]);
        body.extend_from_slice(&le(8));
        body.extend_from_slice(&[1, 0, 1, 1, 1]);
        body.extend_from_slice(&le(2));
        body.extend_from_slice(&[0, 0, 0, 1, 0]);
        let mut r = &body[..];
        let p = read_proposal(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((p.slot, p.state, p.id, p.completed_mask), (261, 1, 7, 3));
        assert!(!p.silent);
        assert!(p.members[0].is_local && p.members[0].answered && p.members[0].accepted);
        assert!(p.members[1].answered && !p.members[1].accepted);
    }

    /// The rewards come first, then a `u32`-counted lock block.
    #[test]
    fn the_player_info_reads_rewards_then_locks() {
        let mut body = vec![1];
        body.extend_from_slice(&le(261 | 6 << 24));
        body.push(1);
        for v in [500, 1000, 0, 0] {
            body.extend_from_slice(&le(v));
        }
        body.push(1);
        for v in [29434, 7, 2] {
            body.extend_from_slice(&le(v));
        }
        body.extend_from_slice(&le(1));
        body.extend_from_slice(&le(262 | 6 << 24));
        body.extend_from_slice(&le(2));
        let mut r = &body[..];
        let LfgPacket::PlayerInfo { rewards, locks } = read_player_info(&mut r).unwrap() else {
            panic!("player info");
        };
        assert!(r.is_empty());
        assert!(rewards[0].done);
        assert_eq!((rewards[0].base_money, rewards[0].base_xp), (500, 1000));
        assert_eq!(rewards[0].items[0].count, 2);
        assert_eq!(locks, [(262 | 6 << 24, 2)]);
    }

    /// Result 6 carries the party's locks; any other result ends at the reason.
    #[test]
    fn the_join_result_reads_party_locks_only_for_result_six() {
        let mut body = le(6).to_vec();
        body.extend_from_slice(&le(0));
        body.push(1);
        body.extend_from_slice(&5u64.to_le_bytes());
        body.extend_from_slice(&le(1));
        body.extend_from_slice(&le(261));
        body.extend_from_slice(&le(3));
        let mut r = &body[..];
        let LfgPacket::JoinResult { party_locks, .. } = read_join_result(&mut r).unwrap() else {
            panic!("join result");
        };
        assert!(r.is_empty());
        assert_eq!(party_locks.unwrap()[0].locks, [(261, 3)]);

        let body = [le(1), le(3)].concat();
        let mut r = &body[..];
        let LfgPacket::JoinResult {
            result,
            reason,
            party_locks,
        } = read_join_result(&mut r).unwrap()
        else {
            panic!("join result");
        };
        assert_eq!((result, reason, party_locks), (1, 3, None));
    }

    /// The join body: roles, two options, the slots, `3` and the needs, then the comment.
    #[test]
    fn the_join_body_follows_the_client_layout() {
        let body = lfg_join(&LfgJoin {
            roles: 0x8,
            no_partial_clear: false,
            achievements: true,
            slots: &[261 | 6 << 24],
            needs: [0, 0, 0],
            comment: "go",
        });
        let mut want = le(8).to_vec();
        want.extend_from_slice(&[0, 1, 1]);
        want.extend_from_slice(&le(261 | 6 << 24));
        want.extend_from_slice(&[3, 0, 0, 0]);
        want.extend_from_slice(b"go\0");
        assert_eq!(body, want);
    }
}
