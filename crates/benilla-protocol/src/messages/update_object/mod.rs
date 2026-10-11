//! `SMSG_UPDATE_OBJECT` body decode: the object list, each entry's [`MovementBlock`] and its
//! sparse [`ObjectFields`].

use std::io::{self, Read};

mod fields;
mod movement;
mod wotlk;

pub use fields::field;
pub use fields::{
    power_display_scale, quest_slot_state, CorpseLook, ObjectFields, OwnerFallback,
    PlayerSkillSlot, QuestLogSlot, UnitAuraSlot, AURA_FLAG_CANCELABLE, AURA_FLAG_EFF_INDEX_MASK,
    FIELD_PLAYER_SKILL_INFO_1_1, PLAYER_EXPLORED_ZONES_SLOTS, PLAYER_QUEST_LOG_SLOTS,
    PLAYER_SKILL_SLOTS, UNIT_AURA_POSITIVE_SLOTS, UNIT_AURA_SLOTS,
};
pub use movement::{CreateSpline, MovementBlock, MoverState, ObjectType};

/// One entry in an `SMSG_UPDATE_OBJECT` object list.
pub enum Object {
    Values {
        guid: u64,
        mask: ObjectFields,
    },
    Movement {
        guid: u64,
        movement: MovementBlock,
    },
    /// `CREATE_OBJECT` or `CREATE_OBJECT2`, which share one wire shape.
    Create {
        guid: u64,
        object_type: ObjectType,
        movement: MovementBlock,
        mask: ObjectFields,
    },
    OutOfRange {
        guids: Vec<u64>,
    },
    Near {
        guids: Vec<u64>,
    },
}

/// Parse an `SMSG_UPDATE_OBJECT` body.
pub(super) fn read_update_object(r: &mut impl Read) -> io::Result<Vec<Object>> {
    wotlk::read_update_object(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quest_log_slot_unpacks_counters_and_state() {
        // Slot 2 (198 + 6 = 204): counters 5/10/63/0 at 6-bit strides, state COMPLETE, a timer
        // (`Player.h:1100-1106`).
        let count_state =
            5u32 | (10 << 6) | (63 << 12) | ((quest_slot_state::COMPLETE as u32) << 24);
        let f = ObjectFields::from_pairs(&[(204, 783), (205, count_state), (206, 4242)]);
        assert_eq!(
            f.player_quest_log(2),
            Some(QuestLogSlot {
                quest_id: 783,
                counters: [5, 10, 63, 0],
                state: quest_slot_state::COMPLETE,
                timer: 4242,
            })
        );
        assert_eq!(f.player_quest_log(1), None);
        assert_eq!(f.player_quest_log(3), None);
        assert_eq!(f.player_quest_log(PLAYER_QUEST_LOG_SLOTS), None);
    }

    #[test]
    fn quest_log_cleared_slot_reads_zero_id() {
        // An abandoned slot: the server zeroes the id and the pair.
        let f = ObjectFields::from_pairs(&[(198, 0), (199, 0), (200, 0)]);
        assert_eq!(f.player_quest_log(0).map(|s| s.quest_id), Some(0));
    }
}
