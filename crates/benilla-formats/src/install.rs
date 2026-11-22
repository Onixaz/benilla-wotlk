//! Where the WoW install is: the one resolver for the `Data` folder and the install root around
//! it. The first `Data` candidate that exists wins:
//!
//! 1. `$WOW_DATA`. Set and empty (`WOW_DATA=`) means no install and ends the search, so the
//!    no-install path runs on a machine that has one.
//! 2. `$WOW_INSTALL/Data`. Set and empty (`WOW_INSTALL=`) with no `$WOW_DATA` is no install too.
//! 3. `<project folder>/WoW/Data`, `dev` builds only ([`project_folder`]).
//! 4. `<exe dir>/Data`, then `<exe dir>/WoW/Data`, the release layouts.
//!
//! The install root ([`wow_install`]), where `Wow.exe` sits, is `$WOW_INSTALL`, else the folder
//! the chosen `Data` sits in.
//!
//! A drop-in `Wow.exe` (the `standalone` launcher, [`set_drop_in_install`]) has one candidate,
//! `<install>/Data`, and never reads `$WOW_DATA`, `$WOW_INSTALL` or a `WoW` link: it is the
//! install's own client.
//!
//! `dev` is a default feature here, so every dependent declares `default-features = false` and
//! re-exports it; otherwise feature unification puts the source-tree path in a player build.
//! `scripts/gates.sh`'s player build and `tests::a_player_build_carries_no_source_tree_path`
//! catch it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The launcher's project folder, handed in by its entry point ([`set_project_folder`]).
static LAUNCHER_FOLDER: OnceLock<PathBuf> = OnceLock::new();

/// The install a drop-in `Wow.exe` sits in, handed in by its entry point ([`set_drop_in_install`]).
static DROP_IN_INSTALL: OnceLock<PathBuf> = OnceLock::new();

/// Record the install root a `standalone` launcher replaces `Wow.exe` in, its own folder: from
/// here [`wow_data`] answers `<install>/Data` or nothing. The entry point calls this once, before
/// anything resolves, and a later call changes nothing.
pub fn set_drop_in_install(root: &Path) {
    let _ = DROP_IN_INSTALL.set(root.to_path_buf());
}

/// The install root a drop-in `Wow.exe` sits in, or `None` in every other build.
pub fn drop_in_install() -> Option<&'static Path> {
    DROP_IN_INSTALL.get().map(PathBuf::as_path)
}

/// Record the launcher's project folder, as its build stamp names it (`BuildId::project_dir`); an
/// empty stamp, a player build's, records nothing. The entry point calls this once, before
/// anything resolves from the folder, and a later call changes nothing.
pub fn set_project_folder(dir: &str) {
    if !dir.is_empty() {
        let _ = LAUNCHER_FOLDER.set(PathBuf::from(dir));
    }
}

/// The folder a dev build keeps its `WoW` link, `benilla-config/` and `.probe-identity` in: the
/// launcher's, when its entry point recorded one, else benilla's own checkout, so a crate built on
/// top of benilla resolves from its folder and not from cargo's checkout of benilla. `None` in a
/// player build, which looks in no source tree.
pub fn project_folder() -> Option<&'static Path> {
    #[cfg(feature = "dev")]
    {
        LAUNCHER_FOLDER
            .get()
            .map(PathBuf::as_path)
            .or_else(benilla_checkout)
    }
    #[cfg(not(feature = "dev"))]
    {
        None
    }
}

/// benilla's own checkout: this crate's manifest dir, two levels up. The addon corpus always
/// resolves here, since its tests are benilla's.
#[cfg(feature = "dev")]
fn benilla_checkout() -> Option<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)
}

/// The vanilla `Data` directory, or `None` when there is no install. Uncached, so a `$WOW_DATA`
/// change is seen.
pub fn wow_data() -> Option<PathBuf> {
    candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`wow_data`] looks, in order, whether or not it exists, for a "looked in" message.
pub fn candidates() -> Vec<PathBuf> {
    if let Some(root) = drop_in_install() {
        return drop_in_candidates(root);
    }
    candidates_from(
        std::env::var_os("WOW_DATA").map(PathBuf::from),
        std::env::var_os("WOW_INSTALL").map(PathBuf::from),
        project_folder(),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    )
}

/// A drop-in's one place: the `Data` folder beside it, whatever the environment says.
fn drop_in_candidates(root: &Path) -> Vec<PathBuf> {
    vec![root.join("Data")]
}

/// [`candidates`] with the environment passed in: a test that set `$WOW_DATA` would change the
/// answer for every test running beside it. `tests/wow_data_env.rs` covers the real read.
fn candidates_from(
    override_dir: Option<PathBuf>,
    install_dir: Option<PathBuf>,
    project: Option<&Path>,
    exe_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(5);

    // 1. The override; set and empty is no install at all, even in a dev build
    // (`WOW_DATA= cargo dev` runs as a player without data).
    let data_named = override_dir.is_some();
    if let Some(over) = override_dir {
        if over.as_os_str().is_empty() {
            return Vec::new();
        }
        out.push(over);
    }

    // 2. The install root's `Data`; set and empty is no install unless `$WOW_DATA` named one.
    if let Some(root) = install_dir {
        if root.as_os_str().is_empty() {
            if !data_named {
                return Vec::new();
            }
        } else {
            out.push(root.join("Data"));
        }
    }

    // 3. The project folder, dev builds only.
    if let Some(root) = project {
        out.push(root.join("WoW/Data"));
    }

    // 4. Beside the binary; a failed `current_exe` adds nothing.
    if let Some(dir) = exe_dir {
        out.push(dir.join("Data"));
        out.push(dir.join("WoW/Data"));
    }

    out
}

/// The install root, the folder `Wow.exe` and `Data` sit in, or `None` with no install: the drop-in's
/// own folder, else `$WOW_INSTALL` (set and empty: none), else the parent of [`wow_data`]'s answer.
pub fn wow_install() -> Option<PathBuf> {
    if let Some(root) = drop_in_install() {
        return Some(root.to_path_buf());
    }
    install_from(
        std::env::var_os("WOW_INSTALL").map(PathBuf::from),
        wow_data(),
    )
}

/// [`wow_install`] with the environment and the resolved `Data` passed in.
fn install_from(install_dir: Option<PathBuf>, data: Option<PathBuf>) -> Option<PathBuf> {
    match install_dir {
        Some(root) if root.as_os_str().is_empty() => None,
        Some(root) if root.is_dir() => Some(root),
        _ => data.and_then(|d| d.parent().map(Path::to_path_buf)),
    }
}

/// The install's `Wow.exe`, or `None` when the install root has none: the reference executable the
/// Warden memory checks read, never written and never run.
pub fn wow_exe() -> Option<PathBuf> {
    let root = wow_install()?;
    ["Wow.exe", "WoW.exe", "wow.exe"]
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())
}

/// Every place [`wow_exe`] looks, for a skip message.
pub fn wow_exe_candidates() -> Vec<PathBuf> {
    wow_install()
        .map(|root| vec![root.join("Wow.exe")])
        .unwrap_or_default()
}

/// The install's `Wow.exe`, or skip this test, spelled and refused as [`wow_data_or_skip`] is.
#[macro_export]
macro_rules! wow_exe_or_skip {
    () => {
        $crate::wow_exe_or_skip!(())
    };
    ($ret:expr) => {
        match $crate::wow_exe() {
            Some(exe) => exe,
            None => {
                $crate::skipped("no Wow.exe in the install", &$crate::wow_exe_candidates());
                return $ret;
            }
        }
    };
}

/// The install, or skip this test: `wow_data_or_skip!()` in a `-> ()` test,
/// `wow_data_or_skip!(None)` in a helper returning `Option`. The skip names every path tried, and
/// under `BENILLA_REQUIRE_DATA=1` it fails instead.
#[macro_export]
macro_rules! wow_data_or_skip {
    () => {
        $crate::wow_data_or_skip!(())
    };
    ($ret:expr) => {
        match $crate::wow_data() {
            Some(data) => data,
            None => {
                $crate::skipped("no WoW install found", &$crate::candidates());
                return $ret;
            }
        }
    };
}

/// An install-gated test's skip, printed; a panic under `BENILLA_REQUIRE_DATA=1`, which the gate
/// sets where the install resolves, since libtest hides a passing test's stderr and a lost install
/// reads green.
#[doc(hidden)]
pub fn skipped(what: &str, looked_in: &[PathBuf]) {
    skipped_by("BENILLA_REQUIRE_DATA", what, looked_in);
}

/// A corpus-gated test's skip, as [`skipped`], refused under `BENILLA_REQUIRE_CORPUS=1`, which the
/// gate sets where the addon corpus resolves: a machine with the install and no corpus still
/// requires every install-gated test.
#[doc(hidden)]
pub fn skipped_corpus(what: &str, looked_in: &[PathBuf]) {
    skipped_by("BENILLA_REQUIRE_CORPUS", what, looked_in);
}

fn skipped_by(switch: &str, what: &str, looked_in: &[PathBuf]) {
    skipped_under(
        std::env::var_os(switch).is_some_and(|v| !v.is_empty() && v != "0"),
        switch,
        what,
        looked_in,
    );
    // `$BENILLA_SKIP_LOG`: libtest hides the line above, so the gate counts skips from this file.
    if let Some(log) = std::env::var_os("BENILLA_SKIP_LOG").filter(|p| !p.is_empty()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            // One write per line: parallel tests append here, and a formatted write can interleave.
            let _ = f.write_all(format!("{what}\n").as_bytes());
        }
    }
}

/// [`skipped`] with the switch passed in, testable without the process environment.
fn skipped_under(required: bool, switch: &str, what: &str, looked_in: &[PathBuf]) {
    let msg = format!("skipping: {what} — looked in {looked_in:?}");
    assert!(
        !required,
        "{msg} — and {switch} is set: the gate resolved this data on this machine, so a skip \
         here is a broken resolver or a missing link, not missing data"
    );
    eprintln!("{msg}");
}

/// The third-party addon corpus the UI engine's real-addon tests run against, or `None`:
/// `$BENILLA_ADDON_CORPUS` (set and empty: none), then `<benilla's checkout>/wow-addons-vanilla`
/// (`dev` only), a gitignored link beside `WoW`. Never a folder beside the checkout, which depends
/// on where the checkout sits.
pub fn addon_corpus() -> Option<PathBuf> {
    addon_corpus_candidates().into_iter().find(|c| c.is_dir())
}

/// Every place [`addon_corpus`] looks, in order, whether or not it exists, for the skip message.
pub fn addon_corpus_candidates() -> Vec<PathBuf> {
    addon_corpus_candidates_from(std::env::var_os("BENILLA_ADDON_CORPUS").map(PathBuf::from))
}

/// [`addon_corpus_candidates`] with the environment passed in, like [`candidates_from`].
fn addon_corpus_candidates_from(override_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    // Set and empty is no corpus at all, as `WOW_DATA=` is no install: the land gates run so.
    if let Some(over) = override_dir {
        if over.as_os_str().is_empty() {
            return Vec::new();
        }
        out.push(over);
    }
    // benilla's checkout, dev builds only, whoever the launcher is: the corpus tests are benilla's.
    #[cfg(feature = "dev")]
    if let Some(root) = benilla_checkout() {
        out.push(root.join("wow-addons-vanilla"));
    }
    out
}

/// The addon corpus, or skip this test, spelled and refused as [`wow_data_or_skip`] is.
#[macro_export]
macro_rules! addon_corpus_or_skip {
    () => {
        $crate::addon_corpus_or_skip!(())
    };
    ($ret:expr) => {
        match $crate::addon_corpus() {
            Some(root) => root,
            None => {
                $crate::skipped_corpus(
                    "no vanilla addon corpus (set $BENILLA_ADDON_CORPUS)",
                    &$crate::addon_corpus_candidates(),
                );
                return $ret;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No test here touches the process environment (see [`candidates_from`]), or records a
    /// launcher's folder, which is process-wide (`tests/project_folder.rs` has a process of its own).
    fn probe(over: Option<&str>, exe: Option<&str>) -> Vec<PathBuf> {
        candidates_from(
            over.map(PathBuf::from),
            None,
            project_folder(),
            exe.map(PathBuf::from),
        )
    }

    #[test]
    fn the_ladder_is_override_then_project_folder_then_beside_the_binary() {
        let c = probe(Some("/opt/wow/Data"), Some("/games/benilla"));
        assert_eq!(c.first(), Some(&PathBuf::from("/opt/wow/Data")), "{c:?}");
        assert!(c.contains(&PathBuf::from("/games/benilla/Data")), "{c:?}");
        assert!(
            c.contains(&PathBuf::from("/games/benilla/WoW/Data")),
            "{c:?}"
        );

        // Either input may be absent without shifting the rest.
        assert_eq!(
            probe(None, Some("/games/benilla")).last(),
            Some(&PathBuf::from("/games/benilla/WoW/Data"))
        );
        assert!(!probe(Some("/opt/wow/Data"), None)
            .iter()
            .any(|p| p.starts_with("/games")));
    }

    #[test]
    fn a_candidate_that_does_not_exist_is_never_chosen() {
        let tmp = std::env::temp_dir().join(format!("benilla-wd-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("Data")).unwrap();
        let ghost = tmp.join("nope");

        // The winner depends on the build (dev puts the project folder first), so assert only
        // that it is not the ghost and exists.
        let chosen = candidates_from(
            Some(ghost.clone()),
            None,
            project_folder(),
            Some(tmp.clone()),
        )
        .into_iter()
        .find(|c| c.is_dir());
        assert_ne!(
            chosen,
            Some(ghost),
            "a $WOW_DATA that does not exist must never be chosen"
        );
        assert!(
            chosen.is_some_and(|c| c.is_dir()),
            "a stale override must fall through to a real install"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A drop-in reads its own install's `Data` and nothing else: no override, no `WoW` link.
    #[test]
    fn a_drop_in_has_one_candidate_its_own_data() {
        assert_eq!(
            drop_in_candidates(Path::new("/games/wotlk")),
            vec![PathBuf::from("/games/wotlk/Data")]
        );
    }

    /// `$WOW_INSTALL` names the root: its `Data` follows `$WOW_DATA`, and set empty it is no
    /// install unless `$WOW_DATA` named one.
    #[test]
    fn the_install_root_adds_its_data_after_the_override() {
        let c = candidates_from(
            Some(PathBuf::from("/opt/wow/Data")),
            Some(PathBuf::from("/games/wotlk")),
            None,
            None,
        );
        assert_eq!(
            c,
            vec![
                PathBuf::from("/opt/wow/Data"),
                PathBuf::from("/games/wotlk/Data")
            ]
        );
        assert_eq!(
            candidates_from(None, Some(PathBuf::new()), None, Some("/x".into())),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            candidates_from(Some("/d".into()), Some(PathBuf::new()), None, None),
            vec![PathBuf::from("/d")]
        );
    }

    /// The root is `$WOW_INSTALL` when it exists, else the folder `Data` sits in; set empty, none.
    #[test]
    fn the_install_root_is_the_override_else_the_data_folders_parent() {
        let tmp = std::env::temp_dir().join(format!("benilla-wi-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(
            install_from(Some(tmp.clone()), Some("/games/wotlk/Data".into())),
            Some(tmp.clone())
        );
        assert_eq!(
            install_from(Some(tmp.join("nope")), Some("/games/wotlk/Data".into())),
            Some(PathBuf::from("/games/wotlk"))
        );
        assert_eq!(
            install_from(None, Some("/games/wotlk/Data".into())),
            Some(PathBuf::from("/games/wotlk"))
        );
        assert_eq!(
            install_from(Some(PathBuf::new()), Some("/g/Data".into())),
            None
        );
        assert_eq!(install_from(None, None), None);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn an_empty_override_means_there_is_no_install() {
        assert_eq!(
            probe(Some(""), Some("/games/benilla")),
            Vec::<PathBuf>::new(),
            "`WOW_DATA=` must leave the ladder with no rungs at all"
        );
    }

    /// A failure here means feature unification put the project-folder rung in a player build.
    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_player_build_carries_no_source_tree_path() {
        assert_eq!(
            project_folder(),
            None,
            "a player build has no project folder, whatever its launcher stamped"
        );
        let root = env!("CARGO_MANIFEST_DIR");
        for c in probe(None, Some("/games/benilla")) {
            assert!(
                !c.starts_with(root),
                "a player build looked inside the source tree at {}",
                c.display()
            );
        }
    }

    /// With no launcher folder recorded (a test, an example, a tool) the rung is benilla's own
    /// checkout; with one, it is that folder's, and benilla's checkout is not looked in.
    #[cfg(feature = "dev")]
    #[test]
    fn a_dev_build_looks_in_the_project_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        assert_eq!(
            project_folder(),
            Some(root),
            "nothing recorded: the checkout"
        );
        let c = probe(None, Some("/games/benilla"));
        assert!(
            c.contains(&root.join("WoW/Data")),
            "the dev build lost its project-folder candidate ({}): {c:?}",
            root.display()
        );

        let launcher = Path::new("/home/player/hello-mod");
        let c = candidates_from(
            None,
            None,
            Some(launcher),
            Some(PathBuf::from("/games/benilla")),
        );
        assert_eq!(
            c,
            vec![
                launcher.join("WoW/Data"),
                PathBuf::from("/games/benilla/Data"),
                PathBuf::from("/games/benilla/WoW/Data"),
            ],
            "a launcher's folder takes the rung, and nothing else looks in benilla's checkout"
        );
    }

    #[cfg(feature = "dev")]
    #[test]
    fn the_corpus_ladder_is_override_then_benillas_checkout_and_nothing_outside_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let c = addon_corpus_candidates_from(Some(PathBuf::from("/opt/corpus")));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/opt/corpus"),
                root.join("wow-addons-vanilla")
            ],
            "{c:?}"
        );
        for c in addon_corpus_candidates_from(None) {
            assert!(
                c.starts_with(root),
                "a corpus candidate outside the checkout ({}) — that is the resolver whose answer \
                 moved when the pool did",
                c.display()
            );
        }
    }

    #[test]
    fn an_empty_corpus_override_is_no_corpus_even_in_a_dev_build() {
        assert_eq!(
            addon_corpus_candidates_from(Some(PathBuf::new())),
            Vec::<PathBuf>::new()
        );
    }

    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_player_build_has_no_corpus_rung_of_its_own() {
        assert_eq!(addon_corpus_candidates_from(None), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_skip_is_a_skip_where_nothing_requires_the_data() {
        skipped_under(
            false,
            "BENILLA_REQUIRE_DATA",
            "no WoW install found",
            &[PathBuf::from("/nowhere")],
        );
    }

    #[test]
    #[should_panic(expected = "BENILLA_REQUIRE_DATA is set")]
    fn a_skip_is_refused_where_the_gate_says_the_data_is_present() {
        skipped_under(
            true,
            "BENILLA_REQUIRE_DATA",
            "no WoW install found",
            &[PathBuf::from("/nowhere")],
        );
    }

    #[test]
    #[should_panic(expected = "BENILLA_REQUIRE_CORPUS is set")]
    fn a_corpus_skip_is_refused_by_its_own_switch() {
        skipped_under(
            true,
            "BENILLA_REQUIRE_CORPUS",
            "no vanilla addon corpus",
            &[PathBuf::from("/nowhere")],
        );
    }
}
