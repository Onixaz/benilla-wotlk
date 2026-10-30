//! The saved-variables host side: FrameXML's two files and when they are read and written. The
//! declaration sets and the serializer are [`benilla_ui::script`]'s.
//!
//! The saved-variables load (`0x512930`) runs after the addons and before `VARIABLES_LOADED`: it
//! creates the account's and the character's folders, then runs the account's `SavedVariables.lua`
//! and the character's, each a chunk, so a saved value overrides a file-scope default before any
//! consumer reads it; a file that is missing or fails logs `Error loading <path>` (`0xa4486c`) to
//! the load's record. The saved-variables save (`0x5271da`) writes both at the UI shutdown, after
//! `PLAYER_LOGOUT`, with the one-time `.bak` ([`crate::local_state::write_saved_file`]); there is
//! no autosave and no Lua call that forces a write.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use benilla_ui::script::UiScript;

pub(crate) struct UiSavedPlugin;

impl Plugin for UiSavedPlugin {
    fn build(&self, _app: &mut App) {
        // No systems: the write is one ordered step of `crate::ui_script::shutdown_ui_state`.
    }
}

/// The account's file, then the character's when one is logged in.
fn files(identity: Option<&(String, String)>) -> Vec<PathBuf> {
    let account = crate::local_state::saved_variables_path();
    let character = identity.and_then(|(realm, character)| {
        crate::local_state::saved_variables_character_path(realm, character)
    });
    account.into_iter().chain(character).collect()
}

/// Run the two saved-variables files into the VM, then `host_settings`, then fire
/// `VARIABLES_LOADED`. A malformed or unreadable file warns and is held
/// ([`UiScript::hold_saved_file`]), so the shutdown write leaves it alone. Read as bytes: the
/// writer keeps a Lua string's raw bytes.
///
/// `host_settings` seats host state after the chunks, so it outranks the files, and before the
/// event, where the consumers run (the minimap's tracked town type, `0x5128d0`).
pub(crate) fn load_saved_variables(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    host_settings: impl FnOnce(&mut UiScript),
) {
    // None in a capture or before a login: nothing is read, and the event still fires.
    for path in files(identity) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let loaded = match std::fs::read(&path) {
            Ok(bytes) => match script.run_chunk(&bytes) {
                Ok(()) => {
                    info!("saved variables: loaded {}", path.display());
                    true
                }
                Err(e) => {
                    warn!(
                        "saved variables: {} did not load ({e}) — running on defaults, and \
                         leaving the file as it is",
                        path.display()
                    );
                    script.hold_saved_file(&path);
                    false
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                warn!("saved variables: cannot read {}: {e}", path.display());
                script.hold_saved_file(&path);
                false
            }
        };
        if !loaded {
            report_error_loading(script, &path);
        }
    }
    host_settings(script);
    // Fired whether or not there was a file, as a step of the reference's load sequence.
    script.fire_event("VARIABLES_LOADED", vec![]);
}

/// `Error loading <path>` into the load's record, the path as the reference prints it.
fn report_error_loading(script: &UiScript, path: &Path) {
    let mut status = benilla_ui::status::Status::default();
    status.report(
        benilla_ui::status::FAILURE,
        format!(
            "Error loading {}",
            crate::local_state::client_relative(path)
        ),
    );
    script.report_load_status(status);
}

/// Write both files from the live globals, one step of [`crate::ui_script::shutdown_ui_state`].
/// No `RegisterForSave` name means the UI never loaded, so nothing is written: that would wipe the
/// files.
pub(crate) fn save(script: &mut UiScript, identity: Option<&(String, String)>) {
    if script.saved_variable_names().is_empty() {
        return;
    }
    let lists = [
        script.saved_variable_names(),
        script.saved_variable_names_per_character(),
    ];
    for (path, names) in files(identity).into_iter().zip(lists) {
        if script.saved_file_held(&path) {
            warn!(
                "saved variables: {} did not load this session — left as it is",
                path.display()
            );
            continue;
        }
        let body = script.saved_variables_bytes_for(&names);
        match crate::local_state::write_saved_file(&path, &body) {
            Ok(()) => info!(
                "saved variables: wrote {} ({} names)",
                path.display(),
                names.len()
            ),
            Err(e) => warn!("saved variables: cannot write {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// A VM with one declared global per scope and a `VARIABLES_LOADED` witness.
    fn script(value: &str) -> UiScript {
        let s = UiScript::new().unwrap();
        s.run(&format!(
            "KEPT = {value} RegisterForSave(\"KEPT\") \
             MINE = 'a' RegisterForSavePerCharacter(\"MINE\") \
             VL_SEEN = 0 \
             local f = CreateFrame(\"Frame\") \
             f:RegisterEvent(\"VARIABLES_LOADED\") \
             f:SetScript(\"OnEvent\", function() VL_SEEN = VL_SEEN + 1 end)"
        ))
        .unwrap();
        s
    }

    fn id() -> (String, String) {
        ("Realm".to_string(), "Char".to_string())
    }

    #[test]
    fn the_files_round_trip_through_the_folder_and_then_fire_variables_loaded() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-sv-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut s = script("7");
        s.run("MINE = 'b'").unwrap();
        save(&mut s, Some(&id()));
        let account = tmp.join("WTF/Account/TESTACCT/SavedVariables.lua");
        let character = tmp.join("WTF/Account/TESTACCT/Realm/Char/SavedVariables.lua");
        assert_eq!(std::fs::read(&account).unwrap(), b"\r\nKEPT = 7\r\n");
        assert_eq!(std::fs::read(&character).unwrap(), b"\r\nMINE = \"b\"\r\n");

        // The restart: this VM's own defaults are 1 and 'a', the files say 7 and 'b'.
        let mut fresh = script("1");
        load_saved_variables(&mut fresh, Some(&id()), |_| {});
        assert_eq!(fresh.eval::<i64>("return KEPT").unwrap(), 7);
        assert_eq!(fresh.eval::<String>("return MINE").unwrap(), "b");
        assert_eq!(
            fresh.eval::<i64>("return VL_SEEN").unwrap(),
            1,
            "VARIABLES_LOADED fires exactly once, after the chunks"
        );

        // A malformed file is left alone and the session runs on defaults.
        std::fs::write(&account, "KEPT = = 3\n").unwrap();
        let mut broken = script("1");
        load_saved_variables(&mut broken, Some(&id()), |_| {});
        assert_eq!(broken.eval::<i64>("return KEPT").unwrap(), 1);
        save(&mut broken, Some(&id()));
        assert_eq!(
            std::fs::read_to_string(&account).unwrap(),
            "KEPT = = 3\n",
            "a file that did not load is not replaced by the defaults the session ran on"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The first save over a file moves it to `.bak`; later saves leave that `.bak` alone.
    #[test]
    fn the_bak_is_the_first_file_ever_replaced() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svbak-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let account = tmp.join("WTF/Account/TESTACCT/SavedVariables.lua");
        let bak = tmp.join("WTF/Account/TESTACCT/SavedVariables.lua.bak");

        save(&mut script("1"), None);
        assert!(!bak.exists(), "nothing to back up on the first write");
        save(&mut script("2"), None);
        assert_eq!(std::fs::read(&bak).unwrap(), b"\r\nKEPT = 1\r\n");
        save(&mut script("3"), None);
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            b"\r\nKEPT = 1\r\n",
            "never refreshed"
        );
        assert_eq!(std::fs::read(&account).unwrap(), b"\r\nKEPT = 3\r\n");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_missing_file_is_logged_as_the_reference_logs_it() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svlog-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut s = script("1");
        load_saved_variables(&mut s, Some(&id()), |_| {});
        s.finish_ui_load_log();
        let writes = format!("{:?}", s.take_load_log_writes());
        assert!(
            writes.contains(r"Error loading WTF\\Account\\TESTACCT\\SavedVariables.lua")
                && writes.contains(
                    r"Error loading WTF\\Account\\TESTACCT\\Realm\\Char\\SavedVariables.lua"
                ),
            "{writes}"
        );
        assert!(
            tmp.join("WTF/Account/TESTACCT/Realm/Char").is_dir(),
            "the load makes the folders"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_byte_string_survives_the_file() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svbytes-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut s = script("string.char(65, 233, 255)");
        save(&mut s, None);
        let mut fresh = script("'x'");
        load_saved_variables(&mut fresh, None, |_| {});
        assert!(fresh
            .eval::<bool>(
                "return string.len(KEPT) == 3 and string.byte(KEPT, 2) == 233 \
                 and string.byte(KEPT, 3) == 255"
            )
            .unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Even with `BENILLA_HOME` writable, a capture neither reads nor writes settings.
    #[test]
    fn a_capture_run_neither_reads_nor_writes() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svcap-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _c = EnvGuard::set("WOW_CAPTURE", "ui-saved");

        let mut s = script("7");
        save(&mut s, Some(&id()));
        assert!(!tmp.exists(), "a capture must not plant a settings file");
        // The load reads nothing, but the event still fires.
        let mut fresh = script("1");
        load_saved_variables(&mut fresh, Some(&id()), |_| {});
        assert_eq!(fresh.eval::<i64>("return KEPT").unwrap(), 1);
        assert_eq!(fresh.eval::<i64>("return VL_SEEN").unwrap(), 1);
    }

    #[test]
    fn an_empty_declaration_set_writes_nothing() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svempty-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut bare = UiScript::new().unwrap();
        save(&mut bare, Some(&id()));
        assert!(!tmp.exists());
    }
}
