//! The world-state table, the server's key/value scoreboard from `SMSG_INIT_WORLD_STATES` and
//! `SMSG_UPDATE_WORLD_STATE`. Read by the NPC-text `$<n>w`/`$<n>e` tokens ([`crate::npc_text`])
//! and by the world map's landmark pass ([`crate::ui_world_map`]), where an `AreaPOI.dbc` row with
//! a `WorldStateID` shows only while that state is nonzero.
//!
//! The reference keeps an open hash keyed by the raw wire dword (bucket `mask & key`, value at node
//! `+0x18`); keys stay raw because `$<n>e` reads the negated key (`$2077e` looks up `0xFFFFF7E3`).
//!
//! An init first runs `InitWorldStates` (`0x548970`): it stores map, zone and area as the display
//! filter ([`WorldStates::scope`]) and rebuilds the world-state UI list, and only then are the
//! packet's pairs applied. It keeps the table, so values persist across zones. The filter is the
//! server's last init, never the avatar's position.

use std::collections::HashMap;

use bevy::prelude::*;

/// The live world-state table, keyed and valued by raw wire dwords.
#[derive(Resource, Default)]
pub(crate) struct WorldStates {
    values: HashMap<u32, u32>,
    generation: u64,
    scope: Option<(u32, u32, u32)>,
    owed: Vec<WorldStateEvent>,
}

impl WorldStates {
    /// Bumped per write and per init, not per pair, the edge readers rebuild on: the reference's
    /// handler re-runs the world-map landmark builder once per packet.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The reference's getter: a missing key reads 0. Signed, as the reference prints it through
    /// `"%d"`.
    pub(crate) fn get(&self, key: u32) -> i32 {
        self.values.get(&key).copied().unwrap_or(0) as i32
    }

    /// Every pair, unordered, for instruments only (`capture::ProbeBgPlugin`); the reference
    /// never enumerates the table.
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (u32, i32)> + '_ {
        self.values.iter().map(|(&k, &v)| (k, v as i32))
    }

    /// The `(map, zone, area)` of the last init (`[0xbe8f54]`, `[0xbe8f50]`, `[0xbe8f4c]`);
    /// `None` before any init.
    pub(crate) fn scope(&self) -> Option<(u32, u32, u32)> {
        self.scope
    }

    /// An init's leading half (`0x548970`): records the filter and keeps every entry. The caller
    /// applies the pairs after. True when the filter moved, the only case the reference rebuilds.
    pub(crate) fn init_scope(&mut self, map: u32, zone: u32, area: u32) -> bool {
        let moved = self.scope != Some((map, zone, area));
        self.scope = Some((map, zone, area));
        self.generation = self.generation.wrapping_add(1);
        moved
    }

    /// Writes `(id, value)` pairs, the reference's setter for both opcodes. Init's trailing
    /// `(0, 0)` is written as data; a zero value is stored, not deleted (`edx` is never tested); a
    /// repeated key overwrites in place. True when any value moved (`0x549440` returns 1 only
    /// then; a new key starts at 0), and a moved arena season owes its event (`0x5494c9`).
    pub(crate) fn write(&mut self, states: &[(u32, u32)]) -> bool {
        let mut moved = false;
        for &(key, value) in states {
            if self.values.insert(key, value).unwrap_or(0) != value {
                moved = true;
                if key == ARENA_SEASON {
                    self.owed.push(WorldStateEvent::ArenaSeason);
                }
            }
        }
        self.generation = self.generation.wrapping_add(1);
        moved
    }

    /// Owes `UPDATE_WORLD_STATES`, which the packet handler fires after its writes.
    pub(crate) fn owe_update(&mut self) {
        self.owed.push(WorldStateEvent::Update);
    }

    pub(crate) fn has_owed(&self) -> bool {
        !self.owed.is_empty()
    }

    /// The events owed, in packet order.
    pub(crate) fn take_owed(&mut self) -> Vec<WorldStateEvent> {
        std::mem::take(&mut self.owed)
    }
}

/// The arena season's world state, `0xc77`, whose change fires `ARENA_SEASON_WORLD_STATE`.
const ARENA_SEASON: u32 = 0xc77;

/// An event a world-state packet owes the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldStateEvent {
    /// `UPDATE_WORLD_STATES`: every update, and an init that moved anything (`0x526530`).
    Update,
    /// `ARENA_SEASON_WORLD_STATE`, from the setter when state `0xc77` moves.
    ArenaSeason,
}

impl WorldStateEvent {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Update => "UPDATE_WORLD_STATES",
            Self::ArenaSeason => "ARENA_SEASON_WORLD_STATE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_miss_reads_zero_and_a_value_keeps_its_sign() {
        let mut table = WorldStates::default();
        assert_eq!(table.get(2077), 0, "empty table — the reference's `\"0\"`");
        table.write(&[(2077, 7), (2264, 0xFFFF_FFFF)]);
        assert_eq!(table.get(2077), 7);
        assert_eq!(table.get(2264), -1, "printed through `%d`");
        assert_eq!(table.get(9999), 0, "still a miss");
    }

    #[test]
    fn the_negated_key_is_a_separate_entry() {
        let mut table = WorldStates::default();
        table.write(&[(2077, 7)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 0);
        table.write(&[(2077u32.wrapping_neg(), 42)]);
        assert_eq!(table.get(2077u32.wrapping_neg()), 42);
        assert_eq!(table.get(2077), 7, "the positive key is untouched");
    }

    #[test]
    fn a_write_replaces_and_the_terminator_is_just_data() {
        let mut table = WorldStates::default();
        table.write(&[(2264, 1), (0, 0)]);
        table.write(&[(2264, 5)]);
        assert_eq!(table.get(2264), 5);
        assert_eq!(table.get(0), 0);
    }
}
