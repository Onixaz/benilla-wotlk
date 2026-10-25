//! Where benilla keeps local state, and the only place that computes a path into it; each path
//! fn's doc says what it holds.
//!
//! What the reference writes, benilla writes in the reference's format at the reference's path
//! (`WTF\`, `Cache\WDB\`, `Logs\`, `Screenshots\`, addons read from `Interface\AddOns\`), under
//! one of two roots ([`client_root`]):
//! - **the install**, for a drop-in `Wow.exe` (the `standalone` launcher, [`drop_in_install`]),
//!   so the reference client and benilla take turns on one folder;
//! - **`benilla-config/`** ([`home`]) in every other build, which never writes into the install.
//!
//! What the reference has no file for (benilla's diagnostics, the probe leases) lives in
//! [`home`] itself, `<install>/benilla-config/` for the drop-in. [`home`] resolves, in order, the
//! same shape as [`benilla_formats::wow_data`]:
//! 1. `$BENILLA_HOME`.
//! 2. `<project folder>/benilla-config/`, dev builds only ([`benilla_formats::project_folder`],
//!    the launcher's): a shipped binary must not carry the build machine's source tree.
//! 3. `<exe dir>/benilla-config/`.
//!
//! With `$WOW_CAPTURE` set every path resolves to `None`: a capture neither reads nor writes
//! player state.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The folder's name: not `benilla`, which beside the binary is the executable itself, and not
/// `WTF`, which a real install already uses.
const STATE_DIR: &str = "benilla-config";

/// The state folder, or `None` when persistence is off (a capture run, or no executable path).
/// It may not exist yet: [`write_atomic`] creates it, and a reader treats a missing file as
/// defaults.
pub(crate) fn home() -> Option<PathBuf> {
    if std::env::var_os("WOW_CAPTURE").is_some() {
        return None; // hermetic: captures neither read nor write player state
    }
    if let Some(over) = std::env::var_os("BENILLA_HOME") {
        return Some(PathBuf::from(over));
    }
    if cfg!(test) {
        return None; // a unit test reaches no real state folder unless it pins `$BENILLA_HOME`
    }
    resident_home()
}

/// The install a drop-in `Wow.exe` replaces the reference in, which it writes into as the reference
/// does; `None` in every other build, whatever folder it runs from, and on a capture run. A unit
/// test names one with `$BENILLA_TEST_DROP_IN`.
pub(crate) fn drop_in_install() -> Option<PathBuf> {
    if std::env::var_os("WOW_CAPTURE").is_some() {
        return None;
    }
    if cfg!(test) {
        return std::env::var_os("BENILLA_TEST_DROP_IN").map(PathBuf::from);
    }
    benilla_formats::drop_in_install().map(Path::to_path_buf)
}

/// The root the reference's own files hang off: a drop-in's install, else [`home`]. `None` when
/// persistence is off.
pub(crate) fn client_root() -> Option<PathBuf> {
    drop_in_install().or_else(home)
}

/// Steps 2 and 3 of [`home`]: the folder the build itself lives in.
fn resident_home() -> Option<PathBuf> {
    // 2 · the project folder, dev builds only.
    if let Some(root) = dev_project_root() {
        return Some(root.join(STATE_DIR));
    }
    // 3 · beside the binary; a dev build never reaches here.
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(STATE_DIR))
}

/// The project folder a dev build keeps its state in ([`benilla_formats::project_folder`]), through
/// [`shared_root`].
fn dev_project_root() -> Option<PathBuf> {
    Some(shared_root(benilla_formats::project_folder()?))
}

/// `here`, or for a linked worktree its primary checkout, whichever worktree built the binary, so
/// every worktree shares one settings folder. Anything unexpected keeps `here`.
fn shared_root(here: &Path) -> PathBuf {
    let dot_git = here.join(".git");
    // A linked worktree: `.git` is a file pointing at `<primary>/.git/worktrees/<name>`.
    if dot_git.is_file() {
        if let Some(gitdir) = std::fs::read_to_string(&dot_git).ok().and_then(|s| {
            s.trim()
                .strip_prefix("gitdir:")
                .map(|p| p.trim().to_owned())
        }) {
            // …/.git/worktrees/<name> → …/.git → the primary checkout.
            if let Some(primary) = Path::new(&gitdir)
                .ancestors()
                .nth(2)
                .and_then(|d| d.parent())
            {
                if primary.is_dir() {
                    return primary.to_path_buf();
                }
            }
        }
    }
    here.to_path_buf()
}

/// `WTF\Config.wtf`: the CVars archived outside any account (`CVar::SaveAll` `0x767100`).
pub(crate) fn config_path() -> Option<PathBuf> {
    client_root().map(|r| r.join("WTF").join("Config.wtf"))
}

/// `WTF\RunOnce*.wtf`, names sorted case-blind (`ForEveryRunOnceWTF` `0x406740`): run once after
/// `Config.wtf` at startup, deleted at exit.
pub(crate) fn run_once_files() -> Vec<PathBuf> {
    let Some(dir) = client_root().map(|r| r.join("WTF")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    let n = n.to_ascii_lowercase();
                    n.starts_with("runonce") && n.ends_with(".wtf")
                })
        })
        .collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files
}

/// `WTF\Account\<ACCOUNT>\` (`OpenAccountFile` `0x6b8b90`): the account's account-data files
/// ([`crate::account_data`]) and saved variables. `account` is the upper-cased login name.
pub(crate) fn account_dir(account: &str) -> Option<PathBuf> {
    Some(
        client_root()?
            .join("WTF")
            .join("Account")
            .join(wtf_component(account)),
    )
}

/// `WTF\Account\<ACCOUNT>\<Realm>\<Character>\`, the character's (`0x9f6270`), its realm and name
/// raw as the reference's folders are.
pub(crate) fn character_dir(account: &str, realm: &str, character: &str) -> Option<PathBuf> {
    Some(
        account_dir(account)?
            .join(wtf_component(realm))
            .join(wtf_component(character)),
    )
}

/// `benilla-config/saved-variables.lua`: the flat channel our FrameXML saves through
/// `RegisterForSave` (the `SavedVariables.lua` analog), written whole at logout, run at UI load.
pub(crate) fn saved_variables_path() -> Option<PathBuf> {
    home().map(|h| h.join("saved-variables.lua"))
}

/// `benilla-config/addons/<Realm>-<Character>.txt`: the AddOn enable state, per character in the
/// reference's `AddOns.txt` format (`<AddOnName>: enabled|disabled`), which it writes at the tail
/// of its UI shutdown (`0x490bd0`). An addon absent from the file is enabled.
pub(crate) fn addons_state_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("addons").join(format!("{key}.txt")))
}

/// `benilla-config/saved/`: per-addon saved variables, account scope, one `<Addon>.lua` per addon
/// declaring `## SavedVariables` (the reference's `WTF/Account/<ACC>/SavedVariables/<Addon>.lua`).
pub(crate) fn addon_saved_account_dir() -> Option<PathBuf> {
    home().map(|h| h.join("saved"))
}

/// `benilla-config/saved/<Realm>-<Character>/`: per-addon saved variables, character scope
/// (`## SavedVariablesPerCharacter`), loaded after the account file so it wins.
pub(crate) fn addon_saved_character_dir(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("saved").join(key))
}

/// `Cache\WDB\<locale>\` (`GetCacheDirectory` `0x667ef0`): the query caches' `.wdb` files
/// ([`crate::wdb`]), one set per install as the reference keeps them.
pub(crate) fn wdb_dir(locale: &str) -> Option<PathBuf> {
    client_root().map(|r| r.join("Cache").join("WDB").join(file_token(locale)))
}

/// `Logs\` (`0x9fa4e4`, `0xa02ef8`): `WoWChatLog.txt`, `WoWCombatLog.txt` and `FrameXML.log`.
pub(crate) fn logs_dir() -> Option<PathBuf> {
    client_root().map(|r| r.join("Logs"))
}

/// `benilla-config/shots.txt`: the framing instrument's camera poses (`/shot`, dev builds).
pub(crate) fn shots_path() -> Option<PathBuf> {
    home().map(|h| h.join("shots.txt"))
}

/// `Screenshots\` (`CaptureScreen` `0x4a85c1`): where the print-screen key writes, each image once,
/// not through [`write_atomic`].
pub(crate) fn screenshots_dir() -> Option<PathBuf> {
    client_root().map(|r| r.join("Screenshots"))
}

/// `Interface\AddOns\`: where third-party addons are read from, with their art and fonts.
pub(crate) fn addons_dir() -> Option<PathBuf> {
    client_root().map(|r| r.join("Interface").join("AddOns"))
}

/// `benilla-config/Diagnostics/`: the stuck-thread sampler's profiles ([`crate::perf::stall`]) and
/// the FPS journal; `None` on a capture run like every path here, so a capture has no sampler.
pub(crate) fn diagnostics_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Diagnostics"))
}

/// `benilla-config/Diagnostics/fps-journal.csv`: the FPS journal's rows while the `fpsJournal`
/// CVar is on; a capture names its own path through `WOW_FPS_JOURNAL`.
pub(crate) fn fps_journal_path() -> Option<PathBuf> {
    diagnostics_dir().map(|d| d.join("fps-journal.csv"))
}

/// `benilla-config/probe-leases/<account>.lock`: the lock a run holds on a declared account while
/// it lives ([`crate::run_mode::fast_path`]). Always the shared project folder's, past
/// `$BENILLA_HOME` and a capture, because the lease guards an account on the server, which every
/// worktree's runs share. `None` in a player build and in a unit test.
pub(crate) fn probe_lease_path(user: &str) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let file = format!("{}.lock", file_token(&user.to_ascii_lowercase()));
    Some(
        dev_project_root()?
            .join(STATE_DIR)
            .join("probe-leases")
            .join(file),
    )
}

/// A realm or character name as one path component: anything not a letter or digit becomes `_`.
/// Letters of any script stay, as in the reference's raw-name folders: vmangos accepts non-Latin
/// names (`StrictPlayerNames = 0`) and a character name is letters only, so no two share a file.
fn file_token(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if t.is_empty() {
        "unknown".into()
    } else {
        t
    }
}

/// A name as the one path component the reference makes of it, kept as typed but for what no
/// folder name can hold: a path separator, a reserved Windows character or a control byte.
fn wtf_component(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if t.trim_matches(['.', ' ']).is_empty() {
        "unknown".into()
    } else {
        t
    }
}

/// Write a state file atomically, through `<path>.tmp` and a rename, so a crash leaves the old
/// file intact.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes())
}

/// [`write_atomic`] for bytes: the saved-variables files carry Lua byte strings as they are.
pub(crate) fn write_atomic_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Test plumbing for the persistence env vars: `set_var` is process-global, so every such test
/// takes [`ENV_LOCK`] and scopes its overrides in [`EnvGuard`]s.
#[cfg(test)]
pub(crate) mod test_env {
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) struct EnvGuard(&'static str, Option<std::ffi::OsString>);
    impl EnvGuard {
        pub(crate) fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self(key, old)
        }
        pub(crate) fn unset(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            std::env::remove_var(key);
            Self(key, old)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.1 {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_env::{EnvGuard, ENV_LOCK};
    use super::*;

    /// Every per-character file keys on this token; ASCII names map unchanged.
    #[test]
    fn distinct_names_never_share_a_token() {
        assert_ne!(file_token("Вася"), file_token("Петя"));
        assert_ne!(file_token("Zoë"), file_token("Zoé"));
        assert_eq!(file_token("Zoë"), "Zoë");
        assert_eq!(file_token("Onehunter"), "Onehunter");
        assert_eq!(file_token("Hydraxian Waterlords"), "Hydraxian_Waterlords");
        assert_eq!(
            file_token("a/b\\c:d"),
            "a_b_c_d",
            "separators never survive"
        );
        assert_eq!(
            file_token("realm-name"),
            "realm_name",
            "the key's own `-` stays unforgeable"
        );
    }

    /// The layout under the override, the one step of [`home`] whose answer a test can state.
    #[test]
    fn the_home_law_override_then_the_residents_and_hermetic_captures() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-ls-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // 1 · the explicit override.
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.join(STATE_DIR).to_str().unwrap());
        assert_eq!(home(), Some(tmp.join(STATE_DIR)));
        assert_eq!(
            config_path(),
            Some(tmp.join("benilla-config").join("WTF").join("Config.wtf"))
        );
        assert_eq!(
            character_dir("PROBE1", "Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/WTF/Account/PROBE1/Hydraxian Waterlords/Probeone")),
            "the realm's and the character's raw names, as the reference's folders"
        );
        assert_eq!(
            account_dir("../X"),
            Some(tmp.join("benilla-config/WTF/Account/.._X")),
            "no name can leave the folder"
        );
        assert_eq!(
            saved_variables_path(),
            Some(tmp.join("benilla-config/saved-variables.lua"))
        );

        assert_eq!(shots_path(), Some(tmp.join("benilla-config/shots.txt")));
        assert_eq!(
            screenshots_dir(),
            Some(tmp.join("benilla-config/Screenshots"))
        );

        // 0 · a capture run resolves nothing, even with an override set.
        let _c2 = EnvGuard::set("WOW_CAPTURE", "ui-options");
        assert_eq!(home(), None);
        assert_eq!(shots_path(), None);
        assert_eq!(
            screenshots_dir(),
            None,
            "a capture writes no player screenshots"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// One layout, two roots: the drop-in writes the reference's folders inside its install, every
    /// other build the same folders in `benilla-config/`, and a capture neither.
    #[test]
    fn only_the_drop_in_writes_into_the_install() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-dropin-{}", std::process::id()));
        let state = tmp.join(STATE_DIR);
        let install = tmp.join("wow");
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", state.to_str().unwrap());

        let _d = EnvGuard::unset("BENILLA_TEST_DROP_IN");
        assert_eq!(drop_in_install(), None);
        assert_eq!(
            benilla_formats::drop_in_install(),
            None,
            "only a standalone launcher records an install to write"
        );
        assert_eq!(client_root(), Some(state.clone()));
        assert_eq!(logs_dir(), Some(state.join("Logs")));
        assert_eq!(screenshots_dir(), Some(state.join("Screenshots")));
        assert_eq!(addons_dir(), Some(state.join("Interface").join("AddOns")));

        let _d2 = EnvGuard::set("BENILLA_TEST_DROP_IN", install.to_str().unwrap());
        assert_eq!(drop_in_install(), Some(install.clone()));
        assert_eq!(client_root(), Some(install.clone()));
        assert_eq!(logs_dir(), Some(install.join("Logs")));
        assert_eq!(screenshots_dir(), Some(install.join("Screenshots")));
        assert_eq!(addons_dir(), Some(install.join("Interface").join("AddOns")));
        assert_eq!(
            home(),
            Some(state.clone()),
            "what the reference has no file for stays in benilla's folder"
        );

        let _c2 = EnvGuard::set("WOW_CAPTURE", "ui-options");
        assert_eq!(drop_in_install(), None, "a capture writes no install");
        assert_eq!(client_root(), None);
        assert_eq!(logs_dir(), None);
        assert_eq!(screenshots_dir(), None);
        assert_eq!(addons_dir(), None);
    }

    /// A unit test that does not pin `$BENILLA_HOME` resolves no state folder, so no test can write
    /// the real one.
    #[test]
    fn a_test_without_an_override_has_no_state_folder() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        assert_eq!(home(), None);
        assert_eq!(config_path(), None);
    }

    /// A missing or wrong `$WOW_DATA` does not move or lose the state folder.
    #[test]
    fn the_state_folder_no_longer_depends_on_finding_the_install() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let _d = EnvGuard::set("WOW_DATA", "/nonexistent/benilla-test/Data");
        let h =
            resident_home().expect("a broken install path must not cost the player their config");
        assert!(
            !h.starts_with("/nonexistent"),
            "home() still reads $WOW_DATA: {}",
            h.display()
        );
        assert!(h.ends_with(STATE_DIR), "{}", h.display());
    }

    /// A player build resolves beside the binary; a dev build to the primary checkout, even from a
    /// linked worktree (whose `.git` is a file).
    #[test]
    fn the_state_folder_lands_where_the_build_says() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let h = resident_home().expect("the resident folder always resolves");
        assert!(h.ends_with(STATE_DIR), "{}", h.display());

        let Some(here) = benilla_formats::project_folder() else {
            // Player build: beside the binary.
            let exe_dir = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            assert_eq!(h, exe_dir.join(STATE_DIR));
            return;
        };

        if here.join(".git").is_file() {
            assert_ne!(
                h,
                here.join(STATE_DIR),
                "a linked worktree must not get its own settings folder — the worktrees share one"
            );
            assert!(
                h.parent().unwrap().join(".git").is_dir(),
                "resolved to {}, which is not a primary checkout",
                h.display()
            );
        } else {
            assert_eq!(h, here.join(STATE_DIR));
        }
    }

    /// A project folder that is a linked worktree keeps its state in the primary checkout, so every
    /// worktree shares one; any other folder, a crate on top of benilla's included, keeps its own.
    #[test]
    fn the_state_root_is_the_folder_or_its_primary_checkout() {
        let tmp = std::env::temp_dir().join(format!("benilla-root-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let primary = tmp.join("benilla");
        std::fs::create_dir_all(primary.join(".git/worktrees/pool-3")).unwrap();
        let slot = tmp.join("pool-3");
        std::fs::create_dir_all(&slot).unwrap();
        let gitdir = primary.join(".git/worktrees/pool-3");
        std::fs::write(slot.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(shared_root(&slot), primary);
        assert_eq!(shared_root(&primary), primary);
        let hello_mod = tmp.join("hello-mod");
        std::fs::create_dir_all(hello_mod.join(".git")).unwrap();
        assert_eq!(shared_root(&hello_mod), hello_mod);
        let plain = tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(shared_root(&plain), plain);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A launcher's recorded folder is where a dev build keeps its state and reads its probe
    /// identity; a player build resolves neither from it. Recording is process-wide, so the parent
    /// runs this same test in a child process of the test binary, which records and resolves.
    #[test]
    fn a_recorded_launcher_folder_holds_the_state_and_the_identity() {
        const CHILD: &str = "BENILLA_TEST_LAUNCHER_FOLDER";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            benilla_formats::set_project_folder(dir.to_str().unwrap());
            if crate::run_mode::dev_affordances() {
                assert_eq!(resident_home(), Some(dir.join(STATE_DIR)));
                let declared = crate::run_mode::declared_identities();
                let summary: Vec<_> = declared
                    .iter()
                    .map(|id| (id.user.as_str(), id.character.as_str()))
                    .collect();
                assert_eq!(summary, [("probe9", "Modchar")]);
            } else {
                let exe_dir = std::env::current_exe().unwrap();
                assert_eq!(
                    resident_home(),
                    Some(exe_dir.parent().unwrap().join(STATE_DIR))
                );
                assert!(crate::run_mode::declared_identities().is_empty());
            }
            return;
        }
        let tmp = std::env::temp_dir().join(format!("benilla-launcher-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join(".probe-identity"),
            "WOW_USER=probe9\nWOW_PASS=unused\nWOW_CHAR=Modchar\n",
        )
        .unwrap();
        let name =
            "local_state::tests::a_recorded_launcher_folder_holds_the_state_and_the_identity";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, &tmp)
            .env_remove("BENILLA_HOME")
            .env_remove("WOW_CAPTURE")
            .output()
            .unwrap();
        let report = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success() && report.contains("1 passed"),
            "the child run:\n{report}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn write_atomic_creates_dirs_and_replaces_whole_files() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-wa-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let path = tmp.join("nested/config.toml");
        write_atomic(&path, "a = \"1\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = \"1\"\n");
        write_atomic(&path, "a = \"2\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = \"2\"\n");
        assert!(!path.with_extension("tmp").exists(), "tmp renamed away");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
