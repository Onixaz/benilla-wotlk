//! Meeting-stone wire messages: the queue state, the right-click join and the leave request.

use std::io::{self, Read};

use crate::wire::{read_u32_le, read_u8};

/// `SMSG 0x295` (reference handler `0x4ca230`): the queued area and a status byte, which the
/// client turns into one of five messages before firing `MEETINGSTONE_CHANGED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingStoneSetQueue {
    pub area: u32,
    pub status: u8,
}

pub(super) fn read_meeting_stone_set_queue(r: &mut impl Read) -> io::Result<MeetingStoneSetQueue> {
    Ok(MeetingStoneSetQueue {
        area: read_u32_le(r)?,
        status: read_u8(r)?,
    })
}

/// `CMSG 0x292` (reference builder `0x4c9ff0`): the right-clicked stone's guid (GO type 23) and
/// nothing else; the server resolves the area from the stone's `gameobject_template.data[2]`.
pub fn meeting_stone_join(go_guid: u64) -> Vec<u8> {
    go_guid.to_le_bytes().to_vec()
}

/// `CMSG 0x293` (reference `0x4ca120`, `CancelMeetingStoneRequest`): an empty body.
pub fn meeting_stone_leave() -> Vec<u8> {
    Vec::new()
}

/// Display-only replies (reference handler `0x4ca3c0`): none changes the queue or fires an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetingStoneNotice {
    /// `0x297`, empty.
    Success,
    /// `0x298`, empty.
    InProgress,
    /// `0x299`, `u64 guid`; the line waits for the name cache.
    MemberAdded { guid: u64 },
}

pub(super) fn read_meeting_stone_member_added(r: &mut impl Read) -> io::Result<MeetingStoneNotice> {
    Ok(MeetingStoneNotice::MemberAdded {
        guid: crate::wire::read_u64_le(r)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference builder `0x4c9ff0` writes one 8-byte guid (`0x418370`) after the opcode.
    #[test]
    fn cmsg_meetingstone_join_body_golden() {
        assert_eq!(
            meeting_stone_join(0x1234_5678_9abc_def0),
            vec![0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12],
            "CMSG_MEETINGSTONE_JOIN body"
        );
    }

    #[test]
    fn the_leave_body_is_empty() {
        assert!(meeting_stone_leave().is_empty());
    }
}
