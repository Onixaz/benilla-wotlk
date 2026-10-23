//! The repo's tasks that a cargo alias cannot express on its own.
//!
//! `cargo play` builds the player binary on `play` (thin LTO), or with `BENILLA_PGO_LTO=fat` on
//! `release` (fat LTO), with the PGO profile `scripts/pgo-train.sh` records for that mode when
//! there is one, and runs it.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The folder `scripts/pgo-train.sh` writes the profiles to, under the primary checkout so every
/// worktree shares them and `cargo clean` keeps them.
const PROFILES: [&str; 2] = ["benilla-config", "pgo"];

/// `BENILLA_PGO_LTO`'s build: the cargo profile and the profile file's mode, one file per mode as
/// a profile matches only the LTO mode it was recorded on. Thin LTO by default: fat measures 3–7%
/// less frame time at Acherus for a build three times as long.
fn lto_mode() -> Option<(&'static str, &'static str)> {
    match std::env::var("BENILLA_PGO_LTO").as_deref() {
        Err(_) | Ok("" | "thin") => Some(("play", "thin")),
        Ok("fat") => Some(("release", "fat")),
        Ok(other) => {
            eprintln!("play: BENILLA_PGO_LTO={other}: `thin` or `fat`");
            None
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("play") => match build() {
            Some(exe) => run(&exe, args.collect()),
            None => ExitCode::FAILURE,
        },
        _ => {
            eprintln!("usage: cargo play [-- <client arguments>]");
            ExitCode::FAILURE
        }
    }
}

/// Builds the player binary, profile-guided when its mode has a profile, and answers its path, or
/// `None` after saying why not.
fn build() -> Option<PathBuf> {
    let root = workspace_root();
    let (cargo_profile, mode) = lto_mode()?;
    let profile: PathBuf = PROFILES
        .iter()
        .fold(primary_checkout(&root), |p, c| p.join(c))
        .join(format!("benilla-{mode}.profdata"));
    let Some(host) = host_triple() else {
        eprintln!("play: `rustc -vV` named no host triple");
        return None;
    };
    // The same cargo profile, features and target as the training build, which the profile's
    // function names hash; the explicit target keeps the flag off build scripts and proc macros,
    // and the encoded form carries a profile path with spaces in it.
    let mut flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let guided = profile.is_file();
    if guided {
        eprintln!(
            "play: {mode} LTO with the PGO profile {}",
            profile.display()
        );
        if !flags.is_empty() {
            flags.push('\u{1f}');
        }
        flags.push_str(&format!("-Cprofile-use={}", profile.display()));
    } else {
        eprintln!(
            "play: {mode} LTO without PGO; scripts/pgo-train.sh records a profile for about 20% \
             more frames per second"
        );
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = Command::new(cargo)
        .current_dir(&root)
        .env("CARGO_ENCODED_RUSTFLAGS", flags)
        .args(["build", "--profile", cargo_profile, "-p", "benilla"])
        .args(["--no-default-features", "--target", &host])
        .status();
    match built {
        Ok(s) if s.success() => {}
        Ok(_) => {
            if guided {
                eprintln!(
                    "play: the build failed; after a toolchain bump the profile needs recording \
                     again (scripts/pgo-train.sh)"
                );
            }
            return None;
        }
        Err(e) => {
            eprintln!("play: cannot run cargo: {e}");
            return None;
        }
    }
    Some(
        target_dir(&root)
            .join(&host)
            .join(cargo_profile)
            .join(format!("benilla{}", std::env::consts::EXE_SUFFIX)),
    )
}

/// Runs the built client with its state in `target/play/benilla-config`, whichever LTO mode built
/// it, unless the shell names its own.
fn run(exe: &Path, client_args: Vec<String>) -> ExitCode {
    let mut run = Command::new(exe);
    run.args(&client_args);
    if std::env::var_os("BENILLA_HOME").is_none() {
        let state = target_dir(&workspace_root())
            .join("play")
            .join("benilla-config");
        run.env("BENILLA_HOME", state);
    }
    match run.status() {
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("play: cannot run {}: {e}", exe.display());
            ExitCode::FAILURE
        }
    }
}

/// The workspace this task belongs to: two folders above its own manifest.
fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .map_or_else(|| manifest.to_path_buf(), Path::to_path_buf)
}

/// Cargo's target folder: `CARGO_TARGET_DIR`, else `target` under the workspace.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

/// The primary checkout behind `root`, which a linked worktree shares its state with; `root`
/// itself when git cannot say.
fn primary_checkout(root: &Path) -> PathBuf {
    Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            Path::new(s.trim())
                .parent()
                .map(|p| p.components().collect())
        })
        .unwrap_or_else(|| root.to_path_buf())
}

/// The host target triple, from `rustc -vV`.
fn host_triple() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
}
