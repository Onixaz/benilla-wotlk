//! The party quest-share wire (411-413, 630). The sharer gets one `MSG_QUEST_PUSH_RESULT` per
//! member; an eligible member gets an ordinary `SMSG_QUESTGIVER_QUEST_DETAILS` with the sharer's
//! player guid as `npcGuid`, and Accept sends `CMSG_QUESTGIVER_ACCEPT_QUEST` to that guid
//! (`QuestHandler.cpp:111-114, 403-459`). The push-result guid names a different player each way.

use std::io;

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

/// The `u8` verdict of `MSG_QUEST_PUSH_RESULT`, numbered as 12340 shows it: byte `v` in 0..=11 is
/// message `0x19b + v` (`0x6e3a72`, table at `0x6e43f8`). 1.12.1's `TOO_FAR` (4) is gone, so
/// every later value moved down one. A newtype, not an enum, so an unknown value survives the
/// parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuestShareMsg(pub u8);

impl QuestShareMsg {
    /// The push went out to this member.
    pub const SHARING_QUEST: Self = Self(0);
    /// The member fails `CanTakeQuest` (level, race, prereqs).
    pub const CANT_TAKE_QUEST: Self = Self(1);
    /// Sent by the server when the member accepts.
    pub const ACCEPT_QUEST: Self = Self(2);
    /// The client's no (`0x58cb0e`).
    pub const DECLINE_QUEST: Self = Self(3);
    /// The member already has a share open; the client's answer to a second (`0x6d07bf`).
    pub const BUSY: Self = Self(4);
    /// The member's quest log is full.
    pub const LOG_FULL: Self = Self(5);
    /// The member is already on the quest.
    pub const HAVE_QUEST: Self = Self(6);
    /// The member has already completed the quest.
    pub const FINISH_QUEST: Self = Self(7);
    /// A daily the member cannot take today.
    pub const CANT_BE_SHARED_TODAY: Self = Self(8);
    /// The quest's timer ran out before the share.
    pub const SHARING_TIMER_EXPIRED: Self = Self(9);
    /// The member is not in the sharer's party.
    pub const NOT_IN_PARTY: Self = Self(10);
    /// A daily the member is not eligible for today.
    pub const NOT_ELIGIBLE_TODAY: Self = Self(11);
}

/// `MSG_QUEST_PUSH_RESULT` from the server (`Quest.cpp:81-85`). Its guid is the member the verdict
/// is about, never the sharer, and fills the `%s` of `ERR_QUEST_PUSH_*` (`Player.cpp:14596-14608`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestPushResult {
    pub member: u64,
    pub msg: QuestShareMsg,
}

/// `SMSG_QUEST_CONFIRM_ACCEPT` (`Quest.cpp:131-136`): the escort confirm, sent to the other
/// eligible members when one accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestConfirmAccept {
    /// Echoed back in [`quest_confirm_accept`].
    pub quest_id: u32,
    /// Server-localized (`Player.cpp:14575-14586`); the receiver may not have the quest cached.
    pub title: String,
    /// The member who accepted: the first `%s` of `QUEST_ACCEPT`.
    pub sender: u64,
}

// ── CMSG encoders ────────────────────────────────────────────────────────────────────────────────

/// Body of `CMSG_PUSHQUESTTOPARTY` (`Quest.cpp:68-71`): Share Quest; the server walks the group.
pub fn push_quest_to_party(quest_id: u32) -> Vec<u8> {
    quest_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_QUEST_CONFIRM_ACCEPT` (`Quest.cpp:63-66`): the escort confirm's yes. A no sends
/// nothing (`QuestHandler.cpp:172-193`).
pub fn quest_confirm_accept(quest_id: u32) -> Vec<u8> {
    quest_id.to_le_bytes().to_vec()
}

/// Body of `MSG_QUEST_PUSH_RESULT` from the client (`Quest.cpp:73-77`): the sharer being answered,
/// the `npcGuid` the shared DETAILS came under. vmangos ignores it (`QuestHandler.cpp:461-467`).
pub fn quest_push_result(sharer: u64, msg: QuestShareMsg) -> Vec<u8> {
    let mut b = Vec::with_capacity(9);
    b.extend_from_slice(&sharer.to_le_bytes());
    b.push(msg.0);
    b
}

// ── SMSG readers ─────────────────────────────────────────────────────────────────────────────────

/// Read `MSG_QUEST_PUSH_RESULT` from the server.
pub(in crate::messages) fn read_quest_push_result(r: &mut &[u8]) -> io::Result<QuestPushResult> {
    Ok(QuestPushResult {
        member: read_u64_le(r)?,
        msg: QuestShareMsg(read_u8(r)?),
    })
}

/// Read `SMSG_QUEST_CONFIRM_ACCEPT`.
pub(in crate::messages) fn read_quest_confirm_accept(
    r: &mut &[u8],
) -> io::Result<QuestConfirmAccept> {
    Ok(QuestConfirmAccept {
        quest_id: read_u32_le(r)?,
        title: read_cstring(r)?,
        sender: read_u64_le(r)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_confirm_bodies_are_bare_quest_ids() {
        assert_eq!(
            push_quest_to_party(0x0123_4567),
            vec![0x67, 0x45, 0x23, 0x01]
        );
        assert_eq!(quest_confirm_accept(9), vec![9, 0, 0, 0]);
    }

    #[test]
    fn push_result_body_is_guid_then_msg() {
        let b = quest_push_result(0x0000_0000_0000_002A, QuestShareMsg::DECLINE_QUEST);
        assert_eq!(b, vec![0x2A, 0, 0, 0, 0, 0, 0, 0, 3]);
    }

    #[test]
    fn push_result_reads_member_then_msg() {
        let mut b = 0x0000_0000_0000_00AAu64.to_le_bytes().to_vec();
        b.push(7);
        let r = read_quest_push_result(&mut b.as_slice()).unwrap();
        assert_eq!(
            r,
            QuestPushResult {
                member: 0xAA,
                msg: QuestShareMsg::FINISH_QUEST,
            }
        );
    }

    #[test]
    fn unknown_verdict_byte_parses() {
        let mut b = 1u64.to_le_bytes().to_vec();
        b.push(0x7F);
        let r = read_quest_push_result(&mut b.as_slice()).unwrap();
        assert_eq!(r.msg, QuestShareMsg(0x7F));
    }

    #[test]
    fn confirm_accept_reads_id_title_sender() {
        let mut b = 1234u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"Escort Duty\0");
        b.extend_from_slice(&0xF130_0000_0000_0001u64.to_le_bytes());
        let c = read_quest_confirm_accept(&mut b.as_slice()).unwrap();
        assert_eq!(c.quest_id, 1234);
        assert_eq!(c.title, "Escort Duty");
        assert_eq!(c.sender, 0xF130_0000_0000_0001);
    }
}
