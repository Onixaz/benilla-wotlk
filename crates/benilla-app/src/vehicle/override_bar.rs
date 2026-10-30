//! The override bar (`CGActionBar::UpdateOverrideSpellBar`, `0x5abb40`): an `OverrideSpellData`
//! row named by our `PLAYER_FIELD_BYTES2` low `i16` with any spell takes the temporary bar, slots
//! 121-130 its ten spells and 131-132 empty (`0x5ab800` with `0x5a7cf0`), under flag `0x4`. It
//! runs when the field moves (`0x6d1c30`) and at world entry (`0x5ac0e0`); a row with no spell, or
//! none, gives the flag back and leaves the slots as written.

use bevy::prelude::*;

use benilla_formats::OverrideSpellsRec;

use crate::net::{EnteredWorldMessage, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, TempActionBar, TEMP_BAR_FIRST_SLOT};

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

/// Re-run the override bar on a move of our override id or a world entry.
pub(super) fn update_override_bar(
    me: Query<&ObjectStore, With<SelfPlayer>>,
    mut entered: MessageReader<EnteredWorldMessage>,
    tables: Res<VehicleTables>,
    actions: Option<ResMut<PlayerActions>>,
    temp: Option<ResMut<TempActionBar>>,
    mut last: Local<Option<i16>>,
) {
    let entry = entered.read().count() > 0;
    let (Some(mut actions), Some(mut temp)) = (actions, temp) else {
        return;
    };
    let Ok(store) = me.single() else {
        return;
    };
    let id = store.0.player_override_spells_id().unwrap_or(0);
    if !entry && *last == Some(id) {
        return;
    }
    *last = Some(id);
    // `GetRecord((int16)id)`: a negative id names no row.
    let rec = u32::try_from(id)
        .ok()
        .and_then(|id| tables.catalog.override_spells.get(&id))
        .filter(|r| has_spells(r));
    match rec {
        Some(rec) => {
            for (i, spell) in override_slots(rec).into_iter().enumerate() {
                actions.set_local(TEMP_BAR_FIRST_SLOT + i as u8, spell);
            }
            temp.flags |= TEMP_BAR_OVERRIDE;
        }
        None => temp.flags &= !TEMP_BAR_OVERRIDE,
    }
    debug!("vehicle: override bar {id} ({})", rec.is_some());
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
