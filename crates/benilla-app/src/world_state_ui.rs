//! The always-up world-state readout behind stock `WorldStateFrame.xml`: joins the rows
//! `WorldStateUI.dbc` displays with the states the server sent ([`crate::world_state`]).
//!
//! The list builder (`ResetWorldStateUI`, `0x5487d0`) walks the whole DBC and admits a row when:
//! 1. `MapID` is -1 or the scope's map;
//! 2. `AreaID` is 0, the scope's zone or the scope's area;
//! 3. `PhaseShift` is 0 or shares a bit with the player's phase mask;
//! 4. `Type` is 0, 1 or 3; 2 (battleground scoreboard columns) never.
//!
//! The scope is the server's last `SMSG_INIT_WORLD_STATES`, not the player's position.
//!
//! The phase mask (`[0xac80a4]`) is 1 outside phased areas and `SMSG_SET_PHASE_SHIFT` is not
//! read, so it is held at 1.

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};
use benilla_ui::script::{UiScript, WorldStateUiView};

use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use crate::world_state::WorldStates;

/// The shared `WorldStateUI.dbc` catalog; absent, the readout is empty.
#[derive(Resource)]
pub(crate) struct WorldStateUiRes(pub(crate) WorldStateUiCatalog);

fn load_world_state_ui(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_world_state_ui_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("world-state UI: {} rows in WorldStateUI.dbc", cat.len());
            commands.insert_resource(WorldStateUiRes(cat));
        }
        Err(e) => warn!("world-state UI: WorldStateUI.dbc failed — no readout: {e:#}"),
    }
}

/// Expands a `WorldStateUI` label, a different expander from [`crate::npc_text`]'s, sharing only
/// the world-state getter. The grammar is `%<digits>W|w` alone, printed `%d`; any other `%` emits a
/// literal `%`, consumes its digits and leaves the next character as text. Output is truncated at
/// the reference's 256-byte limit.
fn expand(text: &str, states: &WorldStates) -> String {
    /// The `strncat` bound: the reference's caller passes `0x100`.
    const LIMIT: usize = 0x100 - 1;

    fn push(s: &str, out: &mut String) {
        for ch in s.chars() {
            if out.len() + ch.len_utf8() > LIMIT {
                return;
            }
            out.push(ch);
        }
    }

    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            let ch = text[i..].chars().next().expect("char boundary");
            push(ch.encode_utf8(&mut [0u8; 4]), &mut out);
            i += ch.len_utf8();
            continue;
        }
        let digits_at = i + 1;
        let mut j = digits_at;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        match bytes.get(j) {
            Some(b'w' | b'W') => {
                // `atoi`: an empty run is key 0.
                let id: u32 = text[digits_at..j].parse().unwrap_or(0);
                push(&states.get(id).to_string(), &mut out);
                i = j + 1;
            }
            _ => {
                push("%", &mut out);
                i = j;
            }
        }
    }
    out
}

/// The player's phase mask, held at the reference's default.
const PHASE_MASK: u32 = 1;

/// The admitted rows for the current scope, resolved as `GetWorldStateUIInfo` answers them.
fn build(catalog: &WorldStateUiCatalog, states: &WorldStates) -> Vec<WorldStateUiView> {
    let Some(scope) = states.scope() else {
        // Before an init the reference never rebuilds.
        return Vec::new();
    };
    catalog
        .rows()
        .filter(|(_, row)| admits(row, scope))
        .map(|(_, row)| resolve(row, states))
        .collect()
}

fn admits(row: &WorldStateUiRow, (map, zone, area): (u32, u32, u32)) -> bool {
    let map_ok = row.map_id == u32::MAX || row.map_id == map;
    let area_ok = row.area_id == 0 || row.area_id == zone || row.area_id == area;
    let phase_ok = row.phase_shift == 0 || row.phase_shift & PHASE_MASK != 0;
    map_ok && area_ok && phase_ok && matches!(row.ui_type, 0 | 1 | 3)
}

/// `GetWorldStateUIInfo` for one row (`0x548d40`): only the text is expanded, and the extended
/// ids answer their states' values.
fn resolve(row: &WorldStateUiRow, states: &WorldStates) -> WorldStateUiView {
    WorldStateUiView {
        ui_type: row.ui_type,
        // No `StateVariable` answers 1.
        ui_state: match row.state_variable {
            0 => 1,
            id => states.get(id),
        },
        text: expand(&row.text, states),
        icon: row.icon.clone(),
        dynamic_icon: row.dynamic_icon.clone(),
        tooltip: row.tooltip.clone(),
        dynamic_tooltip: row.dynamic_tooltip.clone(),
        extended_ui: row.extended_ui.clone(),
        extended_ui_state: row.extended_ui_state.map(|id| states.get(id)),
    }
}

/// Pushes the readout when a world-state packet arrives, the reference's rebuild trigger.
fn feed_world_state_ui(
    script: Option<NonSendMut<UiScript>>,
    catalog: Option<Res<WorldStateUiRes>>,
    states: Res<WorldStates>,
    mut last: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let (Some(mut script), Some(catalog)) = (script, catalog) else {
        return;
    };
    let key = states.generation();
    if *last.get(&script) == Some(key) {
        return;
    }
    *last.get(&script) = Some(key);
    script.set_world_state_ui(build(&catalog.0, &states));
}

pub(crate) struct WorldStateUiPlugin;

impl Plugin for WorldStateUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_world_state_ui.after(AssetSet::Open))
            .add_systems(Update, feed_world_state_ui.after(crate::ui_script::UiInput));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(map_id: u32, area_id: u32, ui_type: u32) -> WorldStateUiRow {
        WorldStateUiRow {
            map_id,
            area_id,
            phase_shift: 0,
            icon: String::new(),
            text: String::new(),
            tooltip: String::new(),
            state_variable: 0,
            ui_type,
            dynamic_icon: String::new(),
            dynamic_tooltip: String::new(),
            extended_ui: String::new(),
            extended_ui_state: [0; 3],
        }
    }

    #[test]
    fn the_macro_expands_world_state_values() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 3), (2328, 1)]);
        assert_eq!(
            expand("Towers Controlled: %2327w", &states),
            "Towers Controlled: 3"
        );
        assert_eq!(expand("%2327W/%2328w", &states), "3/1");
        assert_eq!(
            expand("Bases: %1779w  Resources: %1776w/%1780w", &states),
            "Bases: 0  Resources: 0/0",
            "an un-received state reads 0, never blank"
        );
        assert_eq!(
            expand("Graveyards Assaulted", &states),
            "Graveyards Assaulted"
        );
        assert_eq!(expand("", &states), "");
    }

    #[test]
    fn a_top_bit_value_prints_negative() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 0xFFFF_FFFF)]);
        assert_eq!(expand("%2327w", &states), "-1");
    }

    /// Unreachable on shipped data.
    #[test]
    fn a_malformed_macro_emits_a_literal_percent() {
        let states = WorldStates::default();
        assert_eq!(expand("100% done", &states), "100% done");
        assert_eq!(expand("%d", &states), "%d");
        assert_eq!(expand("%", &states), "%");
        assert_eq!(
            expand("%123x", &states),
            "%x",
            "the digits scanned ahead of the bad letter are consumed either way"
        );
    }

    #[test]
    fn the_output_is_capped_at_the_reference_buffer() {
        let states = WorldStates::default();
        let long = "a".repeat(600);
        assert_eq!(expand(&long, &states).len(), 0xFF);
    }

    #[test]
    fn the_builder_gates_admit_the_right_rows() {
        // Eastern Plaguelands: map 0, zone 139.
        let epl = (0, 139, 139);
        assert!(admits(&row(0, 139, 0), epl));
        assert!(!admits(&row(0, 139, 0), (0, 1377, 1377)), "wrong area");
        assert!(!admits(&row(0, 139, 0), (1, 139, 139)), "wrong map");
        assert!(admits(&row(0, 0, 0), epl), "area 0 is a wildcard");
        assert!(
            admits(&row(u32::MAX, 139, 0), (5, 139, 139)),
            "map -1 is a wildcard"
        );
        // A row may name the zone or the area inside it.
        assert!(admits(&row(1, 976, 0), (1, 440, 976)), "the area");
        assert!(admits(&row(1, 440, 0), (1, 440, 976)), "the zone");
        assert!(
            admits(&row(0, 139, 1), epl),
            "world PvP has no channel gate"
        );
        assert!(admits(&row(0, 139, 3), epl));
        assert!(
            !admits(&row(0, 139, 2), epl),
            "a scoreboard column is never an always-up row"
        );
        let mut phased = row(0, 139, 0);
        phased.phase_shift = 64;
        assert!(!admits(&phased, epl), "a phased row outside its phase");
        phased.phase_shift = 65;
        assert!(admits(&phased, epl));
    }

    #[test]
    fn nothing_shows_before_the_first_init() {
        let catalog = WorldStateUiCatalog::from_rows(vec![(136, row(0, 139, 0))]);
        let states = WorldStates::default();
        assert!(build(&catalog, &states).is_empty());
    }

    #[test]
    fn a_row_without_a_state_variable_reads_one() {
        let states = WorldStates::default();
        assert_eq!(resolve(&row(0, 0, 0), &states).ui_state, 1);

        let mut with_state = row(0, 0, 0);
        with_state.state_variable = 2339;
        assert_eq!(
            resolve(&with_state, &states).ui_state,
            0,
            "a state that HAS an id and reads 0 is 0"
        );
        let mut states = WorldStates::default();
        states.write(&[(2339, 1)]);
        assert_eq!(resolve(&with_state, &states).ui_state, 1);
    }

    #[test]
    fn the_extended_ui_ids_are_resolved_to_values() {
        let mut states = WorldStates::default();
        states.write(&[(2427, 42), (2428, 7)]);
        let mut r = row(0, 139, 1);
        r.extended_ui = "CAPTUREPOINT".into();
        r.extended_ui_state = [2427, 2428, 0];
        assert_eq!(resolve(&r, &states).extended_ui_state, [42, 7, 0]);
    }

    /// Against the install's table: Warsong Gulch's always-up flag-score rows.
    #[test]
    fn the_real_table_builds_the_warsong_gulch_readout() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let catalog =
            benilla_formats::load_world_state_ui_catalog(&mut chain).expect("WorldStateUI");
        let mut wsg = WorldStates::default();
        wsg.init_scope(489, 3277, 3277);
        wsg.write(&[(1581, 2), (1582, 1), (1601, 3)]);
        let rows = build(&catalog, &wsg);
        let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert!(
            texts.contains(&"2/3") && texts.contains(&"1/3"),
            "{texts:?}"
        );

        let mut elsewhere = WorldStates::default();
        elsewhere.init_scope(0, 12, 12); // Elwynn Forest
        assert!(build(&catalog, &elsewhere).is_empty());
    }

    /// An init keeps the table and moves only the filter.
    #[test]
    fn an_init_keeps_the_previous_zones_values() {
        let mut states = WorldStates::default();
        states.init_scope(0, 139, 139);
        states.write(&[(2327, 3)]);
        states.init_scope(0, 12, 87);
        assert_eq!(states.get(2327), 3, "the value outlives the zone");
        assert_eq!(states.scope(), Some((0, 12, 87)));
    }
}
