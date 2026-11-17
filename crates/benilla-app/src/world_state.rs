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
#[derive(Resource)]
pub(crate) struct WorldStates {
    values: HashMap<u32, u32>,
    scope: Option<(u32, u32, u32)>,
    phase_mask: u32,
}

impl Default for WorldStates {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            scope: None,
            phase_mask: DEFAULT_PHASE_MASK,
        }
    }
}

/// The phase mask a game starts in, written by the game's init (`0x52b8f5`).
const DEFAULT_PHASE_MASK: u32 = 1;

impl WorldStates {
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

    /// The player's phase mask (`[0xac80a4]`), which gates the world-state UI's phased rows.
    pub(crate) fn phase_mask(&self) -> u32 {
        self.phase_mask
    }

    /// `SMSG_SET_PHASE_SHIFT`'s setter (`0x513960`): true when the mask moved, which rebuilds the world-state UI and fires `UPDATE_WORLD_STATES` (`0x51397f`); the same mask again does nothing.
    pub(crate) fn set_phase_mask(&mut self, mask: u32) -> bool {
        let moved = mask != self.phase_mask;
        if moved {
            self.phase_mask = mask;
        }
        moved
    }

    /// The next game starts in phase 1 (`0x52b8f5`), with no event: no UI is up to hear it.
    pub(crate) fn reset_phase_mask(&mut self) {
        self.phase_mask = DEFAULT_PHASE_MASK;
    }

    /// An init's leading half (`0x548970`): records the filter and keeps every entry. The caller
    /// applies the pairs after. True when the filter moved, the only case the reference rebuilds.
    pub(crate) fn init_scope(&mut self, map: u32, zone: u32, area: u32) -> bool {
        let moved = self.scope != Some((map, zone, area));
        self.scope = Some((map, zone, area));
        moved
    }

    /// Writes `(id, value)` pairs, the reference's setter for both opcodes. Init's trailing
    /// `(0, 0)` is written as data; a zero value is stored, not deleted (`edx` is never tested); a
    /// repeated key overwrites in place. `moved` when any value moved (`0x549440` returns 1 only then; a new key starts at 0); `seasons` counts the arena season's moves, each of which the setter signals as it writes (`0x5494db`).
    pub(crate) fn write(&mut self, states: &[(u32, u32)]) -> Written {
        let mut written = Written::default();
        for &(key, value) in states {
            if self.values.insert(key, value).unwrap_or(0) != value {
                written.moved = true;
                written.seasons += usize::from(key == ARENA_SEASON);
            }
        }
        written
    }
}

/// What one write moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Written {
    /// Any value moved.
    pub(crate) moved: bool,
    /// How many times the arena season moved, each firing `ARENA_SEASON_WORLD_STATE`.
    pub(crate) seasons: usize,
}

/// The arena season's world state, `0xc77`, whose change fires `ARENA_SEASON_WORLD_STATE`.
const ARENA_SEASON: u32 = 0xc77;

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
    fn a_phase_shift_rebuilds_and_fires_only_when_it_moves() {
        let mut table = WorldStates::default();
        assert_eq!(table.phase_mask(), 1, "the game's init");
        assert!(!table.set_phase_mask(1), "the same mask: no event");
        assert!(
            table.set_phase_mask(0x41),
            "the UI list rebuilds and the event fires"
        );
        assert_eq!(table.phase_mask(), 0x41);
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
