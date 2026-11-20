//! Destructible buildings (GameObject type 33): the Wintergrasp walls and towers, the Strand of the
//! Ancients and Isle of Conquest gates, the Ulduar towers and storm beacons. A building has four
//! damage states, intact, damaged, destroyed and rebuilding, picked from `GAMEOBJECT_FLAGS` bits
//! 9 and 10, each with its own WMO and doodad sets from `DestructibleModelData.dbc`.
//!
//! The reference builds all four models when the template lands (`0x710be0`) and shows the one
//! the flags pick (`0x70dfa0`), whatever `GAMEOBJECT_DISPLAYID` says: here the picked state's
//! display is the entity's live display, and a change swaps the model as a display move does.

use benilla_formats::{
    DestructibleModel, DestructibleModels, GameObjectCatalog, DESTRUCTIBLE_STATES,
};
use bevy::prelude::*;

use crate::go_templates::GoTemplate;
use crate::net::ObjectStore;

/// `GAMEOBJECT_TYPE_DESTRUCTIBLE_BUILDING`.
pub(crate) const GO_TYPE_DESTRUCTIBLE_BUILDING: i32 = 33;

/// `DestructibleModelData.dbc`, read once at startup.
#[derive(Resource, Default)]
pub(crate) struct Destructibles(pub(crate) DestructibleModels);

/// The damage state `GAMEOBJECT_FLAGS` picks: DAMAGED (`0x200`) is 1, DESTROYED (`0x400`) 2,
/// both 3 (`0x70dfa0`).
pub(crate) fn damage_state(flags: u32) -> usize {
    (((flags >> 8) & 0xE) >> 1) as usize
}

/// The four states' displays (`0x710be0`): state 0 is the template's; with a row, each later
/// state takes the row's WMO when `GameObjectDisplayInfo` has it, else the state before it's.
/// Without a row the later states have no model.
pub(crate) fn state_displays(
    row: Option<&DestructibleModel>,
    template_display: u32,
    catalog: &GameObjectCatalog,
) -> [Option<u32>; DESTRUCTIBLE_STATES] {
    let mut out = [None; DESTRUCTIBLE_STATES];
    out[0] = Some(template_display).filter(|&d| d != 0);
    let Some(row) = row else {
        return out;
    };
    for state in 1..DESTRUCTIBLE_STATES {
        let own = row.wmo[state];
        out[state] = if own != 0 && catalog.model_path(own).is_some() {
            Some(own)
        } else {
            if own != 0 {
                warn_once!(
                    "destructible building: state {state} names display {own}, which \
                     GameObjectDisplayInfo lacks; it keeps the state before it"
                );
            }
            out[state - 1]
        };
    }
    out
}

/// A destructible building's live display: its template's state model for the flags' state.
/// `None` when the object is not one, its template has not landed, or the state has no model, and
/// the caller then keeps `GAMEOBJECT_DISPLAYID`. The reference draws nothing for a stateless
/// model; only AzerothCore's eight row-less test templates could reach that.
pub(crate) fn live_state_display(
    store: &ObjectStore,
    template: Option<&GoTemplate>,
    destructibles: Option<&Destructibles>,
    catalog: &GameObjectCatalog,
) -> Option<u32> {
    if store.0.gameobject_type_id() != GO_TYPE_DESTRUCTIBLE_BUILDING {
        return None;
    }
    let t = template?.destructible?;
    let row = destructibles.and_then(|d| d.0.get(t.model_data));
    state_displays(row, t.display_id, catalog)[damage_state(store.0.gameobject_flags())]
}

/// The mouseover test of a destructible building (`0x70df30`): only with a row whose
/// `DoNotHighlight` is clear. Its other term, refusing the building the player stands inside
/// (the player's model parented into one of its WMO groups), is not built: benilla does not
/// parent a mover into a GameObject's WMO groups.
pub(crate) fn mouseover_allowed(
    template: &GoTemplate,
    destructibles: Option<&Destructibles>,
) -> bool {
    let Some(t) = template.destructible else {
        return template.highlight_column;
    };
    destructibles
        .and_then(|d| d.0.get(t.model_data))
        .is_some_and(|row| !row.do_not_highlight)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::go_templates::DestructibleTemplate;

    fn catalog(ids: &[u32]) -> GameObjectCatalog {
        GameObjectCatalog::from_models(ids.iter().map(|&i| (i, format!("w{i}.wmo"))).collect())
    }

    #[test]
    fn the_flags_pick_the_state_from_bits_9_and_10() {
        assert_eq!(damage_state(0), 0);
        assert_eq!(damage_state(0x100), 0); // the hit bit is not a state
        assert_eq!(damage_state(0x200), 1);
        assert_eq!(damage_state(0x400), 2);
        assert_eq!(damage_state(0x600 | 0x20), 3);
    }

    /// The Wintergrasp fortress wall (row 31): damaged 7897, destroyed 7874, no rebuilding WMO.
    #[test]
    fn a_state_without_its_own_wmo_keeps_the_state_before_it() {
        let row = DestructibleModel {
            wmo: [0, 7897, 7874, 0],
            ..Default::default()
        };
        let shown = state_displays(Some(&row), 7877, &catalog(&[7877, 7897, 7874]));
        assert_eq!(shown, [Some(7877), Some(7897), Some(7874), Some(7874)]);
        // A display the catalog lacks falls back as well.
        let shown = state_displays(Some(&row), 7877, &catalog(&[7877, 7874]));
        assert_eq!(shown, [Some(7877), Some(7877), Some(7874), Some(7874)]);
        // No row: only the intact model.
        assert_eq!(
            state_displays(None, 7877, &catalog(&[7877])),
            [Some(7877), None, None, None]
        );
    }

    fn template(model_data: u32) -> GoTemplate {
        let mut t = crate::go_templates::GameObjectTemplates::default();
        let mut data = [0i32; 24];
        data[18] = model_data as i32;
        t.insert(190219, 33, 7877, "Wintergrasp Fortress Wall".into(), &data);
        let got = t.template_by_entry(190219).cloned().unwrap();
        assert_eq!(
            got.destructible,
            Some(DestructibleTemplate {
                model_data,
                display_id: 7877
            })
        );
        got
    }

    #[test]
    fn mouseover_needs_a_row_that_allows_it() {
        let rows = Destructibles(DestructibleModels::from_rows(HashMap::from([
            (31, DestructibleModel::default()),
            (
                26,
                DestructibleModel {
                    do_not_highlight: true,
                    ..Default::default()
                },
            ),
        ])));
        assert!(mouseover_allowed(&template(31), Some(&rows)));
        assert!(!mouseover_allowed(&template(26), Some(&rows)));
        assert!(!mouseover_allowed(&template(0), Some(&rows)));
        assert!(!mouseover_allowed(&template(31), None));
    }

    /// The mouseover slot reads the row.
    #[test]
    fn a_building_is_a_mouseover_when_its_row_allows() {
        use crate::target::cursor_mode::{mouseover_eligible, GoOverrides};
        let eligible =
            |column| mouseover_eligible(33, 0, 0, column, Some(3), GoOverrides::default());
        assert!(eligible(Some(true)));
        assert!(!eligible(Some(false)));
        // A template not yet answered reads eligible, as GENERIC's column does.
        assert!(eligible(None));
    }
}
