//! The `benilla` launcher: a shim that carries the build stamp, so a new commit recompiles these
//! few lines and relinks instead of rebuilding `benilla-app`.

use benilla_app::BuildId;

// A dev build reads the source tree's install and keeps its state there; a drop-in reads and
// writes the install it sits in. One binary cannot be both.
#[cfg(all(feature = "dev", feature = "standalone"))]
compile_error!("`standalone` is the player's drop-in build: build it with `--no-default-features`");

fn main() -> benilla_app::AppExit {
    benilla_app::run(BuildId {
        version: env!("CARGO_PKG_VERSION"),
        describe: env!("BENILLA_GIT_DESCRIBE"),
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
        project_dir: env!("BENILLA_PROJECT_DIR"),
        standalone: cfg!(feature = "standalone"),
        ..Default::default()
    })
}
