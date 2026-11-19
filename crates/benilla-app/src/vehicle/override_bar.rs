//! The override bar (`0x5abb40`): an `OverrideSpellData` row named by our `PLAYER_FIELD_BYTES2`
//! low `i16` with any spell takes the temporary bar, slots 121-130 its ten spells and 131-132 empty
//! (`0x5ab800` with `0x5a7cf0`), under flag `0x4`. It runs in the turn of the field's watch
//! (`0x6d1c5d`) and of world entry (`0x5ac0ea`); a row with no spell, or none, gives the flag back
//! and leaves the slots as written.

use bevy::prelude::*;

use benilla_formats::OverrideSpellsRec;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::TempActionBar;

use super::VehicleTables;

/// The override bar's temporary-bar flag.
pub(crate) const TEMP_BAR_OVERRIDE: u32 = 0x4;

/// `0x5a7c80`: a row lists at least one spell.
fn has_spells(rec: &OverrideSpellsRec) -> bool {
    rec.spells.iter().any(|&s| s != 0)
}

/// The slot words `0x5a7cf0` gives the temporary bar: the row's spells, then nothing.
pub(super) fn override_slots(rec: &OverrideSpellsRec) -> [u32; 12] {
    std::array::from_fn(|i| rec.spells.get(i).copied().unwrap_or(0))
}

/// `0x5abb40` as a step in its caller's turn.
pub(crate) fn update_override_bar(world: &mut World) {
    let mut me = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
    // No player gives the flag back.
    let id = me
        .iter(world)
        .next()
        .map(|s| s.0.player_override_spells_id().unwrap_or(0));
    // `GetRecord((int16)id)`: a negative id names no row.
    let words = id
        .and_then(|id| u32::try_from(id).ok())
        .and_then(|id| {
            world
                .get_resource::<VehicleTables>()?
                .catalog
                .override_spells
                .get(&id)
        })
        .filter(|r| has_spells(r))
        .map(override_slots);
    if let Some(words) = words {
        crate::ui_action::bonus::write_temp_bar(world, words);
    }
    if let Some(mut temp) = world.get_resource_mut::<TempActionBar>() {
        match words {
            Some(_) => temp.flags |= TEMP_BAR_OVERRIDE,
            None => temp.flags &= !TEMP_BAR_OVERRIDE,
        }
    }
    debug!("vehicle: override bar {id:?} ({})", words.is_some());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_fills_ten_slots_and_empties_two() {
        let rec = OverrideSpellsRec {
            id: 1,
            spells: [10, 0, 30, 0, 0, 0, 0, 0, 0, 100],
            flags: 0,
        };
        assert!(has_spells(&rec));
        assert_eq!(
            override_slots(&rec),
            [10, 0, 30, 0, 0, 0, 0, 0, 0, 100, 0, 0]
        );
        assert!(!has_spells(&OverrideSpellsRec::default()));
    }
}
