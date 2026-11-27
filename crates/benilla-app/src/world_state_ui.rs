//! The always-up world-state readout behind stock `WorldStateFrame.xml`: joins the rows
//! `WorldStateUI.dbc` displays with the states the server sent ([`crate::world_state`]).
//!
//! The list builder (`0x5487d0`) walks the whole DBC and admits a row when:
//! 1. `MapID` is -1 or the scope's map;
//! 2. `AreaID` is 0, the scope's zone or the scope's area;
//! 3. `PhaseShift` is 0 or shares a bit with the player's phase mask;
//! 4. `Type` is 0, 1 or 3; 2 (battleground scoreboard columns) never.
//!
//! The scope is the server's last `SMSG_INIT_WORLD_STATES`, not the player's position; the phase
//! mask is the server's last `SMSG_SET_PHASE_SHIFT` ([`WorldStates::phase_mask`]).

use bevy::prelude::*;

use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};
use benilla_ui::script::WorldStateUiView;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use crate::ui_notices::UiClock;
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

/// Expands a `WorldStateUI` label (`0x5770d0`), a different expander from [`crate::npc_text`]'s,
/// sharing only the world-state getter. `%<digits>W|w` prints the state `%d`, `%<digits>K|k` the
/// countdown to it ([`countdown`]); any other `%` emits a literal `%`, consumes its digits and leaves
/// the next character as text. Output is truncated at the reference's 256-byte limit.
fn expand(text: &str, states: &WorldStates, ui_time: i32) -> String {
    /// The `strncat` bound: the reference expander's caller passes `0x100`.
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
            Some(&kind @ (b'w' | b'W' | b'k' | b'K')) => {
                // `atoi`: an empty run is key 0.
                let id: u32 = text[digits_at..j].parse().unwrap_or(0);
                let value = states.get(id);
                let shown = if matches!(kind, b'k' | b'K') {
                    countdown(value, ui_time)
                } else {
                    value.to_string()
                };
                push(&shown, &mut out);
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

/// The time left until `end`, a Unix time on the server's UI clock (`0x576e50`): clamped at 0,
/// `mm:ss` under an hour, else `h:mm:ss`.
fn countdown(end: i32, ui_time: i32) -> String {
    let left = end.wrapping_sub(ui_time).max(0);
    let (hours, minutes, seconds) = (left / 3600, left % 3600 / 60, left % 60);
    if hours <= 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

/// The admitted rows for the current scope, resolved as `GetWorldStateUIInfo` answers them.
#[cfg(test)]
fn build(
    catalog: &WorldStateUiCatalog,
    states: &WorldStates,
    ui_time: i32,
) -> Vec<WorldStateUiView> {
    let Some(scope) = states.scope() else {
        // Before an init the reference never rebuilds.
        return Vec::new();
    };
    catalog
        .rows()
        .filter(|(_, row)| admits(row, scope, states.phase_mask()))
        .map(|(_, row)| resolve(row, states, ui_time))
        .collect()
}

fn admits(row: &WorldStateUiRow, (map, zone, area): (u32, u32, u32), phase_mask: u32) -> bool {
    let map_ok = row.map_id == u32::MAX || row.map_id == map;
    let area_ok = row.area_id == 0 || row.area_id == zone || row.area_id == area;
    let phase_ok = row.phase_shift == 0 || row.phase_shift & phase_mask != 0;
    map_ok && area_ok && phase_ok && matches!(row.ui_type, 0 | 1 | 3)
}

/// `GetWorldStateUIInfo` for one row (`0x548d40`): only the text is expanded, and the extended
/// ids answer their states' values.
fn resolve(row: &WorldStateUiRow, states: &WorldStates, ui_time: i32) -> WorldStateUiView {
    WorldStateUiView {
        ui_type: row.ui_type,
        // No `StateVariable` answers 1.
        ui_state: match row.state_variable {
            0 => 1,
            id => states.get(id),
        },
        text: expand(&row.text, states, ui_time),
        icon: row.icon.clone(),
        dynamic_icon: row.dynamic_icon.clone(),
        tooltip: row.tooltip.clone(),
        dynamic_tooltip: row.dynamic_tooltip.clone(),
        extended_ui: row.extended_ui.clone(),
        extended_ui_state: row.extended_ui_state.map(|id| states.get(id)),
    }
}

/// Whether any admitted row is a timer (type 3), which drives `WORLD_STATE_UI_TIMER_UPDATE`
/// (`0x5488f0`); none before the first init.
pub(crate) fn has_timer_row(catalog: &WorldStateUiRes, states: &WorldStates) -> bool {
    let Some(scope) = states.scope() else {
        return false;
    };
    catalog
        .0
        .rows()
        .any(|(_, row)| row.ui_type == 3 && admits(row, scope, states.phase_mask()))
}

/// The readout as the verbs read it at the call: `GetNumWorldStateUI` counts the rows the builder admits (`0x548720`) and `GetWorldStateUIInfo` resolves one (`0x548d40`). The builder's inputs, the scope and the phase mask, move only where it reruns (`0x548970`, `0x513973`), so the rows admitted at the call are the ones it would hold.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WorldStateUiLive<'w> {
    catalog: Option<Res<'w, WorldStateUiRes>>,
    states: Option<Res<'w, WorldStates>>,
    clock: Option<Res<'w, UiClock>>,
}

impl WorldStateUiLive<'_> {
    /// The admitted rows' count; `None` without the table or the states.
    pub(crate) fn count(&self) -> Option<usize> {
        let (catalog, states) = (self.catalog.as_ref()?, self.states.as_ref()?);
        let Some(scope) = states.scope() else {
            return Some(0);
        };
        Some(
            catalog
                .0
                .rows()
                .filter(|(_, row)| admits(row, scope, states.phase_mask()))
                .count(),
        )
    }

    /// The `index`th admitted row resolved now; `Some(None)` past the end.
    pub(crate) fn info(&self, index: usize) -> Option<Option<WorldStateUiView>> {
        let (catalog, states) = (self.catalog.as_ref()?, self.states.as_ref()?);
        let Some(scope) = states.scope() else {
            return Some(None);
        };
        Some(
            catalog
                .0
                .rows()
                .filter(|(_, row)| admits(row, scope, states.phase_mask()))
                .nth(index)
                .map(|(_, row)| resolve(row, states, self.ui_time())),
        )
    }

    /// The UI clock the countdowns read (`0x548700`); before the server answers, the local clock.
    fn ui_time(&self) -> i32 {
        self.clock
            .as_deref()
            .map_or_else(|| UiClock::default().now(), UiClock::now)
    }

    /// One world state's value now (`0x549440`'s table), `None` without the states.
    pub(crate) fn value(&self, key: u32) -> Option<i32> {
        Some(self.states.as_ref()?.get(key))
    }
}

pub(crate) struct WorldStateUiPlugin;

impl Plugin for WorldStateUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_world_state_ui.after(AssetSet::Open));
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
            expand("Towers Controlled: %2327w", &states, 0),
            "Towers Controlled: 3"
        );
        assert_eq!(expand("%2327W/%2328w", &states, 0), "3/1");
        assert_eq!(
            expand("Bases: %1779w  Resources: %1776w/%1780w", &states, 0),
            "Bases: 0  Resources: 0/0",
            "an un-received state reads 0, never blank"
        );
        assert_eq!(
            expand("Graveyards Assaulted", &states, 0),
            "Graveyards Assaulted"
        );
        assert_eq!(expand("", &states, 0), "");
    }

    #[test]
    fn a_top_bit_value_prints_negative() {
        let mut states = WorldStates::default();
        states.write(&[(2327, 0xFFFF_FFFF)]);
        assert_eq!(expand("%2327w", &states, 0), "-1");
    }

    /// Unreachable on shipped data.
    #[test]
    fn a_malformed_macro_emits_a_literal_percent() {
        let states = WorldStates::default();
        assert_eq!(expand("100% done", &states, 0), "100% done");
        assert_eq!(expand("%d", &states, 0), "%d");
        assert_eq!(expand("%", &states, 0), "%");
        assert_eq!(
            expand("%123x", &states, 0),
            "%x",
            "the digits scanned ahead of the bad letter are consumed either way"
        );
    }

    #[test]
    fn the_output_is_capped_at_the_reference_buffer() {
        let states = WorldStates::default();
        let long = "a".repeat(600);
        assert_eq!(expand(&long, &states, 0).len(), 0xFF);
    }

    /// `0x576e50`: the time left to the state's Unix time on the UI clock, never negative.
    #[test]
    fn a_countdown_prints_minutes_under_an_hour_and_hours_past_it() {
        assert_eq!(countdown(1000, 1000), "00:00");
        assert_eq!(countdown(1000, 1200), "00:00", "a passed end clamps at 0");
        assert_eq!(countdown(1125, 1000), "02:05");
        assert_eq!(countdown(1000 + 3599, 1000), "59:59");
        assert_eq!(countdown(1000 + 3600, 1000), "1:00:00");
        assert_eq!(countdown(1000 + 36_061, 1000), "10:01:01");
        assert_eq!(
            countdown(0, 1_800_000_000),
            "00:00",
            "an un-received end (0) reads no time left"
        );
    }

    /// `0x5770d0`'s `k`/`K` arm: Wintergrasp's two labels (rows 208, 212).
    #[test]
    fn the_macro_expands_a_countdown() {
        let mut states = WorldStates::default();
        states.write(&[(4354, 1_800_000_600), (3781, 1_800_000_065)]);
        let now = 1_800_000_000;
        assert_eq!(
            expand("Next Battle: %4354k", &states, now),
            "Next Battle: 10:00"
        );
        assert_eq!(
            expand("Time Remaining: %3781K", &states, now),
            "Time Remaining: 01:05"
        );
        assert_eq!(
            expand("%4354w", &states, now),
            "1800000600",
            "w stays the raw value"
        );
    }

    #[test]
    fn the_builder_gates_admit_the_right_rows() {
        // Eastern Plaguelands: map 0, zone 139.
        let epl = (0, 139, 139);
        assert!(admits(&row(0, 139, 0), epl, 1));
        assert!(!admits(&row(0, 139, 0), (0, 1377, 1377), 1), "wrong area");
        assert!(!admits(&row(0, 139, 0), (1, 139, 139), 1), "wrong map");
        assert!(admits(&row(0, 0, 0), epl, 1), "area 0 is a wildcard");
        assert!(
            admits(&row(u32::MAX, 139, 0), (5, 139, 139), 1),
            "map -1 is a wildcard"
        );
        // A row may name the zone or the area inside it.
        assert!(admits(&row(1, 976, 0), (1, 440, 976), 1), "the area");
        assert!(admits(&row(1, 440, 0), (1, 440, 976), 1), "the zone");
        assert!(
            admits(&row(0, 139, 1), epl, 1),
            "world PvP has no channel gate"
        );
        assert!(admits(&row(0, 139, 3), epl, 1));
        assert!(
            !admits(&row(0, 139, 2), epl, 1),
            "a scoreboard column is never an always-up row"
        );
        let mut phased = row(0, 139, 0);
        phased.phase_shift = 64;
        assert!(!admits(&phased, epl, 1), "a phased row outside its phase");
        assert!(
            admits(&phased, epl, 0x41),
            "the server phased the player in"
        );
        phased.phase_shift = 65;
        assert!(admits(&phased, epl, 1));
    }

    #[test]
    fn nothing_shows_before_the_first_init() {
        let catalog = WorldStateUiCatalog::from_rows(vec![(136, row(0, 139, 0))]);
        let states = WorldStates::default();
        assert!(build(&catalog, &states, 0).is_empty());
    }

    #[test]
    fn a_row_without_a_state_variable_reads_one() {
        let states = WorldStates::default();
        assert_eq!(resolve(&row(0, 0, 0), &states, 0).ui_state, 1);

        let mut with_state = row(0, 0, 0);
        with_state.state_variable = 2339;
        assert_eq!(
            resolve(&with_state, &states, 0).ui_state,
            0,
            "a state that HAS an id and reads 0 is 0"
        );
        let mut states = WorldStates::default();
        states.write(&[(2339, 1)]);
        assert_eq!(resolve(&with_state, &states, 0).ui_state, 1);
    }

    #[test]
    fn the_extended_ui_ids_are_resolved_to_values() {
        let mut states = WorldStates::default();
        states.write(&[(2427, 42), (2428, 7)]);
        let mut r = row(0, 139, 1);
        r.extended_ui = "CAPTUREPOINT".into();
        r.extended_ui_state = [2427, 2428, 0];
        assert_eq!(resolve(&r, &states, 0).extended_ui_state, [42, 7, 0]);
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
        let rows = build(&catalog, &wsg, 0);
        let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert!(
            texts.contains(&"2/3") && texts.contains(&"1/3"),
            "{texts:?}"
        );

        let mut elsewhere = WorldStates::default();
        elsewhere.init_scope(0, 12, 12); // Elwynn Forest
        assert!(build(&catalog, &elsewhere, 0).is_empty());
    }

    /// Against the install's table: Wintergrasp between battles, with the states AzerothCore's
    /// Wintergrasp sends at zone entry: the Alliance holds the keep
    /// (defender 3802 = 0, attacker 3803 = 1), no battle (3801 = 1, 3710 = 0), the next in ten
    /// minutes (4354).
    #[test]
    fn the_real_table_builds_the_wintergrasp_readout_between_battles() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let catalog =
            benilla_formats::load_world_state_ui_catalog(&mut chain).expect("WorldStateUI");
        let now = 1_800_000_000;
        let mut wg = WorldStates::default();
        wg.init_scope(571, 4197, 4197);
        wg.write(&[
            (3801, 1),
            (3710, 0),
            (3802, 0),
            (3803, 1),
            (4354, (now + 600) as u32),
            (3781, (now + 600) as u32),
        ]);
        let shown: Vec<(u32, String)> = build(&catalog, &wg, now)
            .into_iter()
            .filter(|r| r.ui_state > 0)
            .map(|r| (r.ui_type, r.text))
            .collect();
        assert!(
            shown.contains(&(0, "Alliance Controlled".into())),
            "{shown:?}"
        );
        assert!(
            shown.contains(&(3, "Next Battle: 10:00".into())),
            "{shown:?}"
        );
        assert_eq!(
            shown.len(),
            2,
            "no vehicles or battle timer outside a battle: {shown:?}"
        );
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
