//! What kind of run this is and whose: the always-present layer on the player's side of the `dev`
//! seam. Every answer defaults to the player's (nobody drives the camera, the client starts at the
//! login screen, no rig owns the pick); a dev build's instruments only move answers off those
//! defaults. Gameplay reads this module, never `capture`, `debug_panel` or `perf`.
//!
//! [`benilla_world::dev_state`]'s `deterministic_run` reads `$WOW_CAPTURE` on its own, as
//! [`scenario_active`] does here; the two must stay in step.

use bevy::prelude::*;

/// Something other than the player authors the camera and the avatar this run. Only the capture
/// harness inserts it; the type lives here because non-dev run conditions name it.
#[derive(Resource)]
pub(crate) struct CaptureMode;

/// Is a capture running (`$WOW_CAPTURE` set)? Read before any plugin builds: it decides whether
/// the net thread starts, the window size, and whether clutter and anim-LOD randomness is pinned.
pub(crate) fn scenario_active() -> bool {
    std::env::var("WOW_CAPTURE").is_ok()
}

/// Is the player UI opted into this capture? Forwards [`crate::capture::ui_opted_in`] for readers
/// outside the harness; `false` in a player build, which has no capture to opt into.
#[cfg(feature = "dev")]
pub(crate) fn capture_ui_opted_in() -> bool {
    crate::capture::ui_opted_in()
}

#[cfg(not(feature = "dev"))]
pub(crate) fn capture_ui_opted_in() -> bool {
    false
}

/// Do the credentials come from the environment (`$WOW_USER` and `$WOW_PASS`, both, or
/// `$WOW_PROBE_AS` naming a `.probe-identity` entry, [`fast_path`])? That means "log in without
/// typing" and nothing about who is present; [`unattended`] answers that. One credential alone is
/// a typed login, and `$WOW_CHAR` alone is only the roster's fast path.
pub(crate) fn env_login() -> bool {
    ["WOW_USER", "WOW_PASS"]
        .iter()
        .all(|k| std::env::var_os(k).is_some())
        || std::env::var_os("WOW_PROBE_AS").is_some()
}

/// Is nobody at the keyboard? `false` unless the run declares itself with `$WOW_UNATTENDED`, or is
/// a capture (`$WOW_CAPTURE`) or a rig (`$WOW_RIG`), which cannot be a person. Env login
/// credentials alone never make a run unattended: a player may launch with them. The doubt goes
/// one way because a wrong "attended" costs a probe a timeout, while a wrong "unattended" makes a
/// player's client fight another for their account. A scripted run declares itself, as
/// `docs/CONTRIBUTING.md`, "Running it unattended" says.
///
/// Read by what acts instead of a person: [`crate::net::DisconnectedMessage::new`] (the
/// lost-session verdict, taken once at the wire edge) and the `FATAL` exits.
pub(crate) fn unattended() -> bool {
    ["WOW_UNATTENDED", "WOW_CAPTURE", "WOW_RIG"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

/// A run stuck on a dialog: `true` (exit) only when the run is [`unattended`], logging the `FATAL`
/// marker a leg runner greps for; otherwise the dialog stays up, with a hint when the credentials
/// came from the environment. The call sites in [`crate::login`] and [`crate::char_select`] share
/// it so the verdict and the marker cannot drift apart.
pub(crate) fn fatal_when_driverless(why: &str) -> bool {
    if unattended() {
        error!("login: FATAL — {why}; exiting");
        return true;
    }
    if env_login() {
        warn!(
            "login: {why} — leaving the dialog up, because nothing declared this run driverless. \
             Set WOW_UNATTENDED=1 if nobody is here and it should exit \
             non-zero instead."
        );
    }
    false
}

/// The screen the client starts on: a player at the login screen, a capture straight in the world,
/// a glue capture on the screen it photographs. Needed before the plugins build, so it forwards
/// into the dev half rather than reading a resource.
pub(crate) fn start_state() -> crate::char_select::ClientState {
    #[cfg(feature = "dev")]
    {
        crate::capture::start_state()
    }
    #[cfg(not(feature = "dev"))]
    {
        crate::char_select::ClientState::Login
    }
}

/// Does this build offer developer affordances at all? `false` in a player build. A dev affordance
/// may sit in a gameplay module (the world map's Alt-click jump, `/castvis`), so it asks here.
/// Held by [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_affordances() -> bool {
    cfg!(feature = "dev")
}

/// Did a dev chord (`Ctrl`+`Shift`+key) fire? `false` in a player build. The engine's
/// [`benilla_world::modkeys::dev_chord`] is always compiled, so a gameplay module calling it
/// directly would ship a live dev key to players with no build failure; every non-dev reader comes
/// through here instead. Held by [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_chord(keys: &ButtonInput<KeyCode>, key: KeyCode) -> bool {
    dev_affordances() && benilla_world::modkeys::dev_chord(keys, key)
}

/// The take-control line's free-fly hint, spelled from [`benilla_world::modkeys::DEV_CHORD`];
/// empty in a player build, which must not offer a key that does nothing.
pub(crate) fn free_fly_hint() -> String {
    #[cfg(feature = "dev")]
    {
        format!(
            " ({} + F toggles free-fly)",
            benilla_world::modkeys::DEV_CHORD
        )
    }
    #[cfg(not(feature = "dev"))]
    {
        String::new()
    }
}

/// This crate's source directory on a dev build, `None` in a player build. The `cfg` must be real,
/// not a runtime check: `env!("CARGO_MANIFEST_DIR")` names the build machine, and the literal must
/// be absent from a player binary. Read by [`crate::ui_script`] for the live `assets/ui`, which are
/// benilla's own files whoever the launcher is; the launcher's folder is
/// [`benilla_formats::project_folder`]. Held by [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_source_dir() -> Option<&'static std::path::Path> {
    #[cfg(feature = "dev")]
    {
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    }
    #[cfg(not(feature = "dev"))]
    {
        None
    }
}

/// The name of `.probe-identity`'s lines before any `[name]` header.
const DEFAULT_IDENTITY: &str = "default";

/// `$WOW_PROBE_AS`'s word for "any pool entry no other run holds".
const POOL: &str = "pool";

/// One account `.probe-identity` at the project root declares (gitignored, also read by
/// `scripts/probe-identity.sh`): `WOW_USER=`, `WOW_PASS=`, `WOW_CHAR=`, one per line. The lines
/// before any `[name]` header are the checkout's default entry, which is in the pool; a `[name]`
/// section joins it with `pool=1`, and `pool=0` takes the default out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeclaredIdentity {
    pub(crate) name: String,
    pub(crate) user: String,
    pub(crate) character: String,
    pub(crate) pool: bool,
    /// Read only when `$WOW_PROBE_AS` logs in from the file; may be empty.
    pass: String,
}

/// Every declared entry, in file order, read off the launcher's project folder
/// ([`benilla_formats::project_folder`]); a player build has none.
pub(crate) fn declared_identities() -> Vec<DeclaredIdentity> {
    benilla_formats::project_folder()
        .and_then(|root| std::fs::read_to_string(root.join(".probe-identity")).ok())
        .map(|text| parse_identities(&text))
        .unwrap_or_default()
}

/// The file as entries; an entry without both the account and the character is dropped. A key's
/// first non-empty value wins.
fn parse_identities(text: &str) -> Vec<DeclaredIdentity> {
    let mut sections: Vec<(String, Vec<(&str, &str)>)> =
        vec![(DEFAULT_IDENTITY.to_string(), Vec::new())];
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.trim().to_string(), Vec::new()));
        } else if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"');
            if let Some((_, fields)) = sections.last_mut().filter(|_| !value.is_empty()) {
                fields.push((key.trim(), value));
            }
        }
    }
    sections
        .into_iter()
        .filter_map(|(name, fields)| {
            let field = |key: &str| {
                fields
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            };
            Some(DeclaredIdentity {
                user: field("WOW_USER")?,
                character: field("WOW_CHAR")?,
                pass: field("WOW_PASS").unwrap_or_default(),
                pool: field("pool").map_or(name == DEFAULT_IDENTITY, |v| v == "1"),
                name,
            })
        })
        .collect()
}

/// What the env fast path logs in as, decided once per run by [`fast_path`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FastPath {
    pub(crate) user: String,
    pub(crate) pass: String,
    /// `$WOW_CHAR`, else the entry's character when the file supplied the account and no
    /// `$WOW_PROBE_CHARCREATE` is about to make one.
    pub(crate) character: Option<String>,
    /// The entry this run holds the lease on; `None` without a declaration, or for an undeclared
    /// account let through by `WOW_ALLOW_ACCOUNT`.
    pub(crate) identity: Option<DeclaredIdentity>,
}

/// The env fast path's account ([`crate::login`]), `None` unless [`env_login`]. `$WOW_USER` and
/// `$WOW_PASS` name it, else `$WOW_PROBE_AS` names a `.probe-identity` entry, or `pool` for the
/// first pool entry no other run on this machine holds. A login kicks whoever holds the account,
/// so a declared account is leased for the life of the process (an OS file lock,
/// [`crate::local_state::probe_lease_path`], released when the process ends however it ends), and
/// the answer is `Err(explanation)` when another run holds it, or when this checkout declares
/// accounts and the login is for none of them (`WOW_ALLOW_ACCOUNT=1` lets that one through,
/// unleased). Decided once, the lease with it.
pub(crate) fn fast_path() -> Option<Result<FastPath, String>> {
    if !env_login() {
        return None;
    }
    let decide = || {
        resolve(
            |key| std::env::var(key).ok(),
            &declared_identities(),
            |user| crate::local_state::probe_lease_path(user).map_or(Ok(true), |p| take_lease(&p)),
        )
    };
    // A unit test changes the environment between calls, so it decides every time.
    if cfg!(test) {
        return Some(decide());
    }
    static DECIDED: std::sync::OnceLock<Result<FastPath, String>> = std::sync::OnceLock::new();
    Some(DECIDED.get_or_init(decide).clone())
}

/// The character the roster's fast path seats: `$WOW_CHAR`, else [`fast_path`]'s.
pub(crate) fn fast_path_character() -> Option<String> {
    std::env::var("WOW_CHAR")
        .ok()
        .or_else(|| fast_path()?.ok()?.character)
}

/// [`fast_path`]'s decision, with the environment, the declaration and the lease passed in;
/// `lease(user)` is `Ok(false)` when another run holds the account.
fn resolve(
    env: impl Fn(&str) -> Option<String>,
    declared: &[DeclaredIdentity],
    mut lease: impl FnMut(&str) -> Result<bool, String>,
) -> Result<FastPath, String> {
    let held = |user: &str| {
        format!(
            "another run on this machine is logged in as `{user}`, and a second login kicks it. \
             Wait for it to end, or take another entry (WOW_PROBE_AS=pool takes a free one)."
        )
    };
    if let (Some(user), Some(pass)) = (env("WOW_USER"), env("WOW_PASS")) {
        let identity = match guard_for(declared, &user) {
            Ok(id) => id.cloned(),
            Err(why) if env("WOW_ALLOW_ACCOUNT").is_some() => {
                warn!("login: {why} — WOW_ALLOW_ACCOUNT is set, going ahead anyway");
                None
            }
            Err(why) => {
                return Err(format!(
                    "{why} Set WOW_ALLOW_ACCOUNT=1 if the cross-account login is deliberate."
                ))
            }
        };
        if let Some(id) = &identity {
            if !lease(&id.user)? {
                return Err(held(&id.user));
            }
        }
        let character = env("WOW_CHAR");
        return Ok(FastPath {
            user,
            pass,
            character,
            identity,
        });
    }
    let wanted = env("WOW_PROBE_AS").unwrap_or_default();
    let pool = wanted.eq_ignore_ascii_case(POOL);
    let candidates: Vec<&DeclaredIdentity> = if pool {
        declared.iter().filter(|id| id.pool).collect()
    } else {
        declared
            .iter()
            .find(|id| id.name.eq_ignore_ascii_case(&wanted))
            .into_iter()
            .collect()
    };
    let names = declared
        .iter()
        .map(|id| format!("{}{}", id.name, if id.pool { " (pool)" } else { "" }))
        .collect::<Vec<_>>()
        .join(", ");
    if candidates.is_empty() {
        return Err(if declared.is_empty() {
            format!(
                "WOW_PROBE_AS={wanted}, but this checkout's .probe-identity declares no account."
            )
        } else {
            format!("WOW_PROBE_AS={wanted} matches no .probe-identity entry; it declares {names}.")
        });
    }
    for &id in &candidates {
        if !lease(&id.user)? {
            continue;
        }
        if id.pass.is_empty() {
            return Err(format!(
                "the .probe-identity entry `{}` has no WOW_PASS.",
                id.name
            ));
        }
        let character = env("WOW_CHAR").or_else(|| {
            env("WOW_PROBE_CHARCREATE")
                .is_none()
                .then(|| id.character.clone())
        });
        return Ok(FastPath {
            user: id.user.clone(),
            pass: id.pass.clone(),
            character,
            identity: Some(id.clone()),
        });
    }
    Err(if pool {
        format!("other runs on this machine hold every pool entry ({names}); a login kicks them.")
    } else {
        held(&candidates[0].user)
    })
}

/// The account guard: `Ok(Some(entry))` for a declared account, `Ok(None)` without a declaration,
/// `Err(explanation)` for an account the checkout does not declare, because a login kicks whoever
/// holds the account.
fn guard_for<'a>(
    declared: &'a [DeclaredIdentity],
    user: &str,
) -> Result<Option<&'a DeclaredIdentity>, String> {
    if declared.is_empty() {
        return Ok(None);
    }
    if let Some(id) = declared
        .iter()
        .find(|id| user.eq_ignore_ascii_case(&id.user))
    {
        return Ok(Some(id));
    }
    let accounts = declared
        .iter()
        .map(|id| format!("`{}` (WOW_CHAR={})", id.user, id.character))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "the env fast path is about to log in as `{user}`, and this checkout's .probe-identity \
         declares {accounts}. A login kicks whoever holds `{user}` — a player mid-session, or \
         another checkout's probe."
    ))
}

/// Takes the lease at `path` for the rest of the process: `Ok(false)` when another process holds
/// it. The OS drops the lock when the process ends, so a crash strands no lease.
fn take_lease(path: &std::path::Path) -> Result<bool, String> {
    static HELD: std::sync::Mutex<Vec<std::fs::File>> = std::sync::Mutex::new(Vec::new());
    let fail = |e: std::io::Error| format!("cannot take the lease {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(fail)?;
    match file.try_lock() {
        Ok(()) => {
            HELD.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(file);
            Ok(true)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(fail(e)),
    }
}

/// The word a rig appends to this checkout's bodies (`<Race3><Class3><word>[f]`) so checkouts
/// never collide on a name: the character of the entry this run holds, else the default entry's,
/// lowercased, without a leading `probe`. `None` without a declaration; the rig then configures
/// the body it logged in as.
pub(crate) fn rig_suffix() -> Option<String> {
    let id = fast_path()
        .and_then(Result::ok)
        .and_then(|f| f.identity)
        .or_else(|| declared_identities().into_iter().next())?;
    Some(suffix_of(&id.character))
}

fn suffix_of(character: &str) -> String {
    let name = character.to_ascii_lowercase();
    let word = name
        .strip_prefix("probe")
        .filter(|w| !w.is_empty())
        .unwrap_or(&name);
    word.chars()
        .filter(char::is_ascii_alphabetic)
        .take(6)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outside the dev roots, only [`super::dev_chord`] may call the engine's `dev_chord`, and no
    /// file may name the `dev` feature: the player build gate cannot see either. The display
    /// string [`benilla_world::modkeys::DEV_CHORD`] is exempt.
    #[test]
    fn the_dev_plane_has_exactly_one_door() {
        // Compiled out by `--no-default-features`, plus this module, the door itself.
        const DEV_ROOTS: &[&str] = &[
            "capture/",
            "debug_panel/",
            "asset_churn.rs",
            "dev.rs",
            "hover_log.rs",
            "perf/",
            "preflight.rs",
            "probe_shield.rs",
            "lib.rs",
            "run_mode.rs",
        ];
        // Assembled at runtime so the checker does not flag its own source.
        let needle = format!("modkeys::{}(", "dev_chord");
        // `#[cfg(feature = "dev")]` belongs only in this module, `dev.rs` and `lib.rs`.
        let cfg_needle = format!("feature = {}dev{}", '"', '"');

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&src)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if DEV_ROOTS.iter().any(|r| rel.starts_with(r)) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("source is readable");
                for (n, line) in text.lines().enumerate() {
                    // Doc comments name the function constantly; only real calls matter.
                    let t = line.trim();
                    if t.starts_with("//") {
                        continue;
                    }
                    if t.contains(&needle) || t.contains(&cfg_needle) {
                        offenders.push(format!("{rel}:{}  {t}", n + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a dev-chord affordance outside the dev roots must go through `run_mode::dev_chord`, \
             which is `false` in a player build — the engine's `modkeys::dev_chord` is always \
             compiled, so calling it directly ships a live dev key to a player and the build gate \
             cannot see it. And a `#[cfg(feature = \"dev\")]` outside `run_mode`, \
             `dev.rs` and `lib.rs` means a gameplay module has learned the seam exists — ask \
             `run_mode::dev_affordances()` instead. Offenders:\n  {}",
            offenders.join("\n  "),
        );
    }

    /// Env credentials log in without typing but do not make the run unattended.
    #[test]
    fn env_credentials_are_not_a_claim_about_who_is_in_the_room() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _decl = EnvGuard::unset("WOW_UNATTENDED");
        let _cap = EnvGuard::unset("WOW_CAPTURE");
        let _rig = EnvGuard::unset("WOW_RIG");
        let _as = EnvGuard::unset("WOW_PROBE_AS");

        // A player build's defaults: nothing set, nothing assumed.
        {
            let _u = EnvGuard::unset("WOW_USER");
            let _p = EnvGuard::unset("WOW_PASS");
            let _c = EnvGuard::unset("WOW_CHAR");
            assert!(!env_login() && !unattended());
        }
        // `WOW_USER=… WOW_PASS=… cargo play`, `.cargo/config.toml`'s example: a player's launch.
        let _u = EnvGuard::set("WOW_USER", "player");
        let _p = EnvGuard::set("WOW_PASS", "secret");
        assert!(env_login(), "the fast path still submits for them");
        assert!(
            !unattended(),
            "credentials in the environment are not an empty chair",
        );
        // The declaration, and the two runs that are automated by construction.
        for key in ["WOW_UNATTENDED", "WOW_CAPTURE", "WOW_RIG"] {
            let _d = EnvGuard::set(key, "1");
            assert!(unattended(), "{key} declares the run driverless");
        }
        assert!(!unattended(), "and each guard puts it back");
    }

    /// A file with the default entry, a pool entry and a by-name-only entry.
    const FLEET: &str = "WOW_USER=probe4\nWOW_PASS=secret\nWOW_CHAR=Probefour\n\n\
                         [two]\nWOW_USER=probe5\nWOW_PASS=secret5\nWOW_CHAR=Probefive\npool=1\n\n\
                         [ghost]\r\nWOW_USER=tester\r\nWOW_PASS=secret6\r\nWOW_CHAR=Elza\r\n";

    /// [`resolve`] over `vars`, with `held` naming the accounts another run leases.
    fn resolve_with(vars: &[(&str, &str)], held: &[&str]) -> Result<FastPath, String> {
        resolve(
            |key| {
                vars.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            },
            &parse_identities(FLEET),
            |user| Ok(!held.contains(&user)),
        )
    }

    #[test]
    fn the_fast_path_is_a_declared_account_or_refused() {
        let declared = parse_identities(FLEET);
        assert_eq!(
            guard_for(&declared, "probe4").unwrap().unwrap().name,
            "default"
        );
        // vmangos accounts are case-insensitive, and any entry is declared, pool or not.
        assert_eq!(guard_for(&declared, "PROBE5").unwrap().unwrap().name, "two");
        assert_eq!(
            guard_for(&declared, "tester").unwrap().unwrap().name,
            "ghost"
        );

        // Anyone else's account kicks a live session.
        let player = guard_for(&declared, "player").unwrap_err();
        assert!(player.contains("probe4") && player.contains("probe5") && player.contains("kicks"));
        // The override hint belongs to the caller that can act on it, not to the reason.
        assert!(!player.contains("WOW_ALLOW_ACCOUNT"));
        assert!(guard_for(&[], "player").unwrap().is_none());

        let env = [("WOW_USER", "player"), ("WOW_PASS", "x")];
        assert!(resolve_with(&env, &[])
            .unwrap_err()
            .contains("WOW_ALLOW_ACCOUNT=1"));
        let allowed = resolve_with(&[env[0], env[1], ("WOW_ALLOW_ACCOUNT", "1")], &[]).unwrap();
        assert_eq!((allowed.user.as_str(), allowed.identity), ("player", None));
    }

    #[test]
    fn the_file_declares_named_entries_and_who_is_in_the_pool() {
        let fleet = parse_identities(FLEET);
        let summary: Vec<_> = fleet
            .iter()
            .map(|id| {
                (
                    id.name.as_str(),
                    id.user.as_str(),
                    id.character.as_str(),
                    id.pool,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("default", "probe4", "Probefour", true),
                ("two", "probe5", "Probefive", true),
                ("ghost", "tester", "Elza", false),
            ]
        );
        assert_eq!(fleet[2].pass, "secret6", "a CRLF file reads the same");

        // An entry needs the account and the character; the old one-entry file still reads.
        let one = parse_identities("WOW_USER=probe4\nWOW_PASS=x\nWOW_CHAR=\"Probefour\"\n");
        assert_eq!((one.len(), one[0].character.as_str()), (1, "Probefour"));
        assert!(parse_identities("WOW_USER=probe4\nWOW_PASS=x\n").is_empty());
        assert!(parse_identities("WOW_USER=\nWOW_CHAR=Probefour\n").is_empty());
        assert!(parse_identities("").is_empty());
        let out = parse_identities("WOW_USER=a\nWOW_CHAR=A\npool=0\n[b]\nWOW_USER=b\nWOW_CHAR=B\n");
        assert!(
            out.iter().all(|id| !id.pool),
            "pool=0 takes the default out"
        );
    }

    #[test]
    fn probe_as_logs_in_from_the_file_and_the_pool_skips_held_accounts() {
        // By name, the entry's own account and character.
        let ghost = resolve_with(&[("WOW_PROBE_AS", "Ghost")], &[]).unwrap();
        assert_eq!(
            (
                ghost.user.as_str(),
                ghost.pass.as_str(),
                ghost.character.as_deref()
            ),
            ("tester", "secret6", Some("Elza"))
        );
        // `$WOW_CHAR` still picks the body; a create probe makes its own.
        let other = resolve_with(&[("WOW_PROBE_AS", "two"), ("WOW_CHAR", "Alt")], &[]).unwrap();
        assert_eq!(other.character.as_deref(), Some("Alt"));
        let create = resolve_with(
            &[("WOW_PROBE_AS", "two"), ("WOW_PROBE_CHARCREATE", "N")],
            &[],
        );
        assert_eq!(create.unwrap().character, None);

        // The pool, in file order, past whatever another run holds; never a by-name entry.
        let pool = |held: &[&str]| resolve_with(&[("WOW_PROBE_AS", "pool")], held);
        assert_eq!(pool(&[]).unwrap().user, "probe4");
        assert_eq!(pool(&["probe4"]).unwrap().user, "probe5");
        assert!(pool(&["probe4", "probe5"])
            .unwrap_err()
            .contains("every pool entry"));

        // A held account refuses rather than kicks, whichever way it was named.
        assert!(resolve_with(&[("WOW_PROBE_AS", "ghost")], &["tester"])
            .unwrap_err()
            .contains("`tester`"));
        let env = [("WOW_USER", "probe4"), ("WOW_PASS", "secret")];
        assert!(resolve_with(&env, &["probe4"])
            .unwrap_err()
            .contains("kicks it"));
        assert_eq!(
            resolve_with(&env, &[]).unwrap().identity.unwrap().name,
            "default"
        );

        let unknown = resolve_with(&[("WOW_PROBE_AS", "three")], &[]).unwrap_err();
        assert!(
            unknown.contains("default (pool), two (pool), ghost"),
            "{unknown}"
        );
    }

    /// Two handles on one lease file: the second is refused while the first lives, as another
    /// process would be.
    #[test]
    fn a_lease_is_held_until_the_process_lets_go() {
        let dir = std::env::temp_dir().join(format!("benilla-lease-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("probe-leases").join("probe4.lock");
        assert_eq!(take_lease(&path), Ok(true));
        assert_eq!(take_lease(&path), Ok(false));
        assert_eq!(
            take_lease(&dir.join("probe-leases").join("probe5.lock")),
            Ok(true)
        );
    }

    #[test]
    fn the_rig_word_is_the_declared_character_without_its_probe_prefix() {
        // vmangos player names carry no digits, so a probe fleet spells its index.
        assert_eq!(suffix_of("Probefour"), "four");
        assert_eq!(suffix_of("Probezero"), "zero");
        // Any other name is its own word, cut to fit a 12-character name.
        assert_eq!(suffix_of("Tester"), "tester");
        assert_eq!(suffix_of("Longcharname"), "longch");
        assert_eq!(suffix_of("Probe"), "probe");
    }
}
