//! The camera's tuning rows `CameraRegisterCVars` (`0x5fd910`) composes in three loops, per follow
//! style: `cameraSmooth<style><state><param>` (`0x5fde21`), `cameraSmoothViewData<style><field>
//! <param>` (`0x5fdec1`) and `cameraTerrainTilt<style><state><param>` (`0x5fdf61`), all flags 0x10
//! (per account, category 5). The names and the default strings are the binary's own tables. No
//! host reader: benilla's follow rig keeps its own model, so these hold what a player's
//! `config-cache.wtf` sets.

use std::sync::LazyLock;

use super::table::{same, Registered};

/// The follow styles (`0xad1b94`), the order the loops index them.
const STYLES: [&str; 5] = ["Never", "Smart", "Always", "Spline", "Smarter"];
/// The movement states the smoothing rows split by (`0xad1c10`).
const SMOOTH_STATES: [&str; 7] = ["Idle", "Stop", "Track", "Move", "Strafe", "Turn", "Fear"];
/// Each smoothing row's two parameters (`0xad1c08`).
const SMOOTH_PARAMS: [&str; 2] = ["Delay", "Factor"];
/// The view fields (`0xad1b74`), shared with the saved views' names.
const VIEW_DATA: [&str; 3] = ["Distance", "Pitch", "Yaw"];
/// The movement states the terrain-tilt rows split by (`0xad1dc0`).
const TILT_STATES: [&str; 10] = [
    "Fall", "Fear", "Idle", "Jump", "Move", "Strafe", "Swim", "Taxi", "Track", "Turn",
];
/// Each terrain-tilt row's three parameters (`0xad1de8`).
const TILT_PARAMS: [&str; 3] = ["Absorb", "Delay", "Factor"];

/// `[style][state][param]` (`0xad1c30`).
const SMOOTH_STATE_DEFAULTS: [[[&str; 2]; 7]; 5] = [
    [
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.0", "0.0"],
    ],
    [
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.4", "10.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.4", "10.0"],
    ],
    [
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
    ],
    [
        ["0.0", "4.0"],
        ["0.0", "4.0"],
        ["0.0", "4.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "4.0"],
    ],
    [
        ["0.0", "0.0"],
        ["0.0", "0.0"],
        ["0.4", "10.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.0", "1.0"],
        ["0.4", "10.0"],
    ],
];
/// `[style][field][param]` (`0xad1d48`).
const SMOOTH_VIEW_DATA_DEFAULTS: [[[&str; 2]; 3]; 5] = [
    [["0.0", "0.0"], ["0.0", "0.0"], ["0.0", "0.0"]],
    [["0.0", "0.0"], ["0.0", "0.0"], ["0.0", "1.0"]],
    [["0.0", "0.0"], ["0.0", "1.0"], ["0.0", "1.0"]],
    [["0.0", "0.0"], ["0.0", "1.0"], ["0.0", "1.0"]],
    [["0.0", "0.0"], ["0.0", "1.0"], ["0.0", "1.0"]],
];
/// `[style][state][param]` (`0xad1df8`).
const TILT_DEFAULTS: [[[&str; 3]; 10]; 5] = [
    [
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
    ],
    [
        ["1.0", "0.0", "0.75"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
    ],
    [
        ["1.0", "0.0", "0.75"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "-1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
    ],
    [
        ["1.0", "0.0", "0.75"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
    ],
    [
        ["1.0", "0.0", "0.75"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "-1.0"],
        ["0.0", "0.0", "-1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["0.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
        ["1.0", "0.0", "1.0"],
    ],
];

/// The 250 rows in the reference's own registration order.
pub(super) static ROWS: LazyLock<Vec<Registered>> = LazyLock::new(|| {
    let row = |name: String, default: &'static str| {
        same(Box::leak(name.into_boxed_str()), default).per_account()
    };
    let mut rows = Vec::with_capacity(250);
    for (s, style) in STYLES.iter().enumerate() {
        for (t, state) in SMOOTH_STATES.iter().enumerate() {
            for (p, param) in SMOOTH_PARAMS.iter().enumerate() {
                let name = format!("cameraSmooth{style}{state}{param}");
                rows.push(row(name, SMOOTH_STATE_DEFAULTS[s][t][p]));
            }
        }
        for (f, field) in VIEW_DATA.iter().enumerate() {
            for (p, param) in SMOOTH_PARAMS.iter().enumerate() {
                let name = format!("cameraSmoothViewData{style}{field}{param}");
                rows.push(row(name, SMOOTH_VIEW_DATA_DEFAULTS[s][f][p]));
            }
        }
        for (t, state) in TILT_STATES.iter().enumerate() {
            for (p, param) in TILT_PARAMS.iter().enumerate() {
                let name = format!("cameraTerrainTilt{style}{state}{param}");
                rows.push(row(name, TILT_DEFAULTS[s][t][p]));
            }
        }
    }
    rows
});

#[cfg(test)]
mod tests {
    use super::super::table::Scope;
    use super::ROWS;

    /// Five styles of 7 × 2 smoothing, 3 × 2 view-data and 10 × 3 tilt rows, composed in the
    /// loops' own order (style, then state or field, then parameter).
    #[test]
    fn the_three_loops_compose_250_rows_per_account() {
        assert_eq!(ROWS.len(), 5 * (7 * 2 + 3 * 2 + 10 * 3));
        assert!(ROWS.iter().all(|r| r.scope == Scope::Account && !r.hidden));
        let default = |name: &str| ROWS.iter().find(|r| r.name == name).map(|r| r.default);
        assert_eq!(ROWS[0].name, "cameraSmoothNeverIdleDelay");
        assert_eq!(ROWS[249].name, "cameraTerrainTiltSmarterTurnFactor");
        assert_eq!(default("cameraSmoothSmartTrackDelay"), Some("0.4"));
        assert_eq!(default("cameraSmoothViewDataAlwaysYawFactor"), Some("1.0"));
        assert_eq!(default("cameraTerrainTiltSmarterIdleFactor"), Some("-1.0"));
    }
}
