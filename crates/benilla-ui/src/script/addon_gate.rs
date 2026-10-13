//! `AddOn_CanLoad` (`0x5f7e90`) as a pure function: the one load law every surface consults.
//! The first check that fires decides, in the reference's order: missing, the visiting guard (a
//! cycle is loadable), enabled, banned, corrupt, version, dependencies, demand.
//!
//! The version arm (`0x5f7f17`) has two steps: an `## Interface` below 20000 is `INCOMPATIBLE`,
//! which nothing overrides; anything else but exactly 30300 is `INTERFACE_VERSION`, and with
//! `checkAddonVersion` off that reason is reset to none (`0x5f7f56`), so force-load falls through
//! to the dependency and demand checks. A missing `## Interface` compares as 0.
//!
//! The demand arm (`0x5f7fc8`) refuses a LoadOnDemand addon as `DEMAND_LOADED`, and only for the
//! callers that pass `0`: the startup walk (`0x5f7ff0`) and the glue's `GetAddOnInfo`
//! (`0x4dc66b`). `LoadAddOn` (`0x528a6d`), the in-game `GetAddOnInfo` (`0x51138b`) and the
//! dependency recursion (`0x5f7f9c`) pass `1` and skip it.
//!
//! The reason renders once (`0x5f4c40`, names at `0xad17b4`): the addon's own token, else `DEP_`
//! and the deepest failing dependency's token, else nil. `BANNED`, `CORRUPT` and `INSECURE` read
//! the server's `SMSG_ADDON_INFO` signature state, which is not modelled; they are never produced.

/// The `## Interface` this client implements, the reference's hard-coded `0x765c` (`0x5f7f32`).
pub const CLIENT_INTERFACE: u32 = 30300;

/// Below this `## Interface` an addon is `INCOMPATIBLE`, whatever `checkAddonVersion` says
/// (`0x5f7f1a`, `0x4e20`).
pub const INCOMPATIBLE_BELOW: u32 = 20000;

/// The version arm alone (`0x5f7f17`): `INCOMPATIBLE`, `INTERFACE_VERSION` (only while the check
/// is on), or none.
pub fn version_reason(interface: u32, version_check: bool) -> Option<&'static str> {
    if interface < INCOMPATIBLE_BELOW {
        Some("INCOMPATIBLE")
    } else if interface != CLIENT_INTERFACE && version_check {
        Some("INTERFACE_VERSION")
    } else {
        None
    }
}

/// One addon as the gate reads it; every addon registry lowers into this.
pub struct GateRow<'a> {
    pub name: &'a str,
    /// This character's enable state (`AddOns.txt`; an addon nobody disabled is enabled).
    pub enabled: bool,
    /// `## Interface` as the client parses it ([`crate::toc::Toc::interface_version`]):
    /// the leading integer, `0` when absent.
    pub interface: u32,
    /// `## LoadOnDemand`, the record's `+0x2c` byte (`0x5f8d50`).
    pub load_on_demand: bool,
    /// Loaded this session, the record's `+0x18` byte. A loaded dependency satisfies the
    /// recursion outright (`0x5f7f7f`, `0x5f7630`).
    pub loaded: bool,
    /// `## Dependencies` / `## RequiredDeps` / any `## Dep*` (one list in the reference).
    pub dependencies: Vec<&'a str>,
}

/// The arbiter's answer: loadable, or the two out-params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Loadable,
    Refused {
        /// The addon's own reason token (`DISABLED`, `INTERFACE_VERSION`, …).
        reason: Option<&'static str>,
        /// A failing dependency's raw token, rendered as `DEP_<token>`.
        dep: Option<&'static str>,
    },
}

impl Verdict {
    pub fn loadable(self) -> bool {
        self == Verdict::Loadable
    }

    /// The formatter (`0x5f4c40`) with its callers' nil guard, so `LOADABLE` never renders.
    pub fn token(self) -> Option<String> {
        match self {
            Verdict::Loadable => None,
            Verdict::Refused { reason, dep } => reason
                .map(str::to_owned)
                .or_else(|| dep.map(|d| format!("DEP_{d}"))),
        }
    }
}

/// `AddOn_CanLoad` over a registry: can `rows[index]` load right now?
///
/// `demand_gate` is the caller's second argument inverted: true for the startup walk and the
/// glue's list, where a LoadOnDemand addon reports `DEMAND_LOADED`; false for `LoadAddOn` and the
/// in-game `GetAddOnInfo`. `version_check` is the live `checkAddonVersion`, read per query.
pub fn can_load(rows: &[GateRow], index: usize, demand_gate: bool, version_check: bool) -> Verdict {
    let mut visiting = vec![false; rows.len()];
    walk(rows, index, demand_gate, version_check, &mut visiting)
}

/// One level of the arbiter, its checks in the reference's order.
fn walk(
    rows: &[GateRow],
    i: usize,
    demand_gate: bool,
    version_check: bool,
    visiting: &mut [bool],
) -> Verdict {
    // Check 2: the visiting guard (`+0x2d`, `0x5f7ebb`); a cycle resolves as loadable.
    if visiting[i] {
        return Verdict::Loadable;
    }
    let row = &rows[i];
    // Check 3: enabled (`0x5f7ecc`).
    if !row.enabled {
        return Verdict::Refused {
            reason: Some("DISABLED"),
            dep: None,
        };
    }
    // Checks 4 and 5, banned and corrupt: no signature state, never produced.
    // Check 6: the version arm (`0x5f7f17`).
    if let Some(reason) = version_reason(row.interface, version_check) {
        return Verdict::Refused {
            reason: Some(reason),
            dep: None,
        };
    }
    // Check 7: the dependencies, recursively with the demand arm skipped (`0x5f7f9c` pushes 1);
    // the dependency's reason lands in the caller's `dep` out-param, so the deepest failure's raw
    // token is the one `DEP_` wraps.
    visiting[i] = true;
    for dep in &row.dependencies {
        let found = rows.iter().position(|r| r.name.eq_ignore_ascii_case(dep));
        let verdict = match found {
            // Check 1, in the recursion: a name not in the registry is `MISSING`.
            None => Verdict::Refused {
                reason: Some("MISSING"),
                dep: None,
            },
            // A loaded dependency satisfies the walk before any recursion (`0x5f7f87`).
            Some(d) if rows[d].loaded => Verdict::Loadable,
            Some(d) => walk(rows, d, false, version_check, visiting),
        };
        if let Verdict::Refused { reason, dep } = verdict {
            visiting[i] = false;
            return Verdict::Refused {
                reason: None,
                dep: reason.or(dep),
            };
        }
    }
    visiting[i] = false;
    // Check 8: the demand arm (`0x5f7fc8`), for the callers that pass 0.
    if demand_gate && row.load_on_demand {
        return Verdict::Refused {
            reason: Some("DEMAND_LOADED"),
            dep: None,
        };
    }
    Verdict::Loadable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(name: &'a str, deps: Vec<&'a str>) -> GateRow<'a> {
        GateRow {
            name,
            enabled: true,
            interface: CLIENT_INTERFACE,
            load_on_demand: false,
            loaded: false,
            dependencies: deps,
        }
    }

    /// Enabled comes before every later check, so the glue shows a disabled row grey, never red.
    #[test]
    fn a_disabled_addon_reports_only_disabled() {
        let mut a = row("A", vec!["Ghost"]);
        a.enabled = false;
        a.interface = 0; // incompatible too
        let rows = vec![a];
        assert_eq!(
            can_load(&rows, 0, true, true).token().as_deref(),
            Some("DISABLED")
        );
    }

    #[test]
    fn the_version_gate_is_exact_and_force_load_erases_it() {
        for interface in [30200, 30301, 40000] {
            let mut a = row("A", vec![]);
            a.interface = interface;
            let rows = vec![a];
            assert_eq!(
                can_load(&rows, 0, false, true).token().as_deref(),
                Some("INTERFACE_VERSION"),
                "{interface}"
            );
            assert_eq!(can_load(&rows, 0, false, false), Verdict::Loadable);
        }
    }

    /// Below 20000 is `INCOMPATIBLE` before the version check is read: a 1.12 addon (11200) and a
    /// manifest with no `## Interface` stay refused with force-load on.
    #[test]
    fn an_addon_below_20000_is_incompatible_even_with_force_load() {
        for interface in [0, 11200, 19999] {
            let mut a = row("A", vec![]);
            a.interface = interface;
            let rows = vec![a];
            for version_check in [true, false] {
                assert_eq!(
                    can_load(&rows, 0, false, version_check).token().as_deref(),
                    Some("INCOMPATIBLE"),
                    "{interface}, check {version_check}"
                );
            }
        }
        let mut a = row("A", vec![]);
        a.interface = 20000;
        let rows = vec![a];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("INTERFACE_VERSION"),
            "20000 is past the incompatible arm (`jae`)"
        );
        // A dependent reads its dependency's raw token.
        let mut lib = row("Lib", vec![]);
        lib.interface = 11200;
        let rows = vec![row("Use", vec!["Lib"]), lib];
        assert_eq!(
            can_load(&rows, 0, false, false).token().as_deref(),
            Some("DEP_INCOMPATIBLE")
        );
    }

    #[test]
    fn dep_prefix_applies_once_at_any_depth() {
        let a = row("A", vec!["B"]);
        let b = row("B", vec!["C"]);
        let mut c = row("C", vec![]);
        c.enabled = false;
        let rows = vec![a, b, c];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DEP_DISABLED"),
            "not DEP_DEP_DISABLED — the shared out-param collapses the nesting"
        );
        // A dependency not installed fails the recursion's check 1.
        let rows = vec![row("A", vec!["Ghost"])];
        assert_eq!(
            can_load(&rows, 0, false, true).token().as_deref(),
            Some("DEP_MISSING")
        );
    }

    #[test]
    fn a_cycle_is_loadable_to_the_query() {
        let a = row("Ping", vec!["Pong"]);
        let b = row("Pong", vec!["Ping"]);
        let rows = vec![a, b];
        assert_eq!(can_load(&rows, 0, true, true), Verdict::Loadable);
    }

    /// The startup walk and the glue refuse a LoadOnDemand addon; `LoadAddOn` does not, and
    /// loads an ordinary addon too. Force-load does not lift the demand arm.
    #[test]
    fn the_demand_arm_refuses_load_on_demand_for_the_walk_and_the_glue_only() {
        let mut lod = row("Lod", vec![]);
        lod.load_on_demand = true;
        let plain = row("Plain", vec![]);
        let rows = vec![lod, plain];
        for version_check in [true, false] {
            assert_eq!(
                can_load(&rows, 0, true, version_check).token().as_deref(),
                Some("DEMAND_LOADED")
            );
        }
        assert_eq!(can_load(&rows, 0, false, true), Verdict::Loadable);
        assert_eq!(can_load(&rows, 1, true, true), Verdict::Loadable);
        assert_eq!(can_load(&rows, 1, false, true), Verdict::Loadable);
    }

    /// The recursion skips the demand arm (`0x5f7f9c` pushes 1), so a LoadOnDemand dependency
    /// never reports `DEP_DEMAND_LOADED`, loaded or not.
    #[test]
    fn a_load_on_demand_dependency_does_not_refuse_its_dependent() {
        let a = row("A", vec!["B"]);
        let mut b = row("B", vec![]);
        b.load_on_demand = true;
        let rows = vec![a, b];
        assert_eq!(can_load(&rows, 0, true, true), Verdict::Loadable);
        assert_eq!(can_load(&rows, 0, false, true), Verdict::Loadable);
    }
}
