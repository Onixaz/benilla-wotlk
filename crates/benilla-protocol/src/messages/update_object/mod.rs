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
    PLAYER_SKILL_SLOTS, QUEST_LOG_SLOT_FIELDS, UNIT_AURA_POSITIVE_SLOTS, UNIT_AURA_SLOTS,
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
        // Slot 2: counters 5/10 and 63/0 as `u16` pairs, state COMPLETE, a timer.
        let base = field::FIELD_PLAYER_QUEST_LOG_1_1 + 2 * QUEST_LOG_SLOT_FIELDS;
        let f = ObjectFields::from_pairs(&[
            (base, 783),
            (base + 1, u32::from(quest_slot_state::COMPLETE)),
            (base + 2, 5 | 10 << 16),
            (base + 3, 63),
            (base + 4, 4242),
        ]);
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
        // An abandoned slot: the server zeroes the id and the rest.
        let base = field::FIELD_PLAYER_QUEST_LOG_1_1;
        let f = ObjectFields::from_pairs(&[(base, 0), (base + 1, 0), (base + 4, 0)]);
        assert_eq!(f.player_quest_log(0).map(|s| s.quest_id), Some(0));
    }
}
