//! The stock glue screens: the player's own `GlueXML.toc` off the patch chain, run into a glue VM
//! ([`UiScript::new_glue`]) that stands in the world while no character is in it, where the boot
//! VM otherwise stands.
//!
//! The reference's glue builder resets the Lua state on a rebuild (the cold first build, passes 0
//! and skips it), registers the glue functions and the 26 glue events, opens `Logs\GlueXML.log`,
//! then checks `GlueXML.toc`'s signature. Only a valid one loads the toc through the shared runner,
//! and only when the loaded files' MD5 matches the signature's does `FRAMES_LOADED` fire. Signature
//! results 0–2 log their line, missing, corrupt or modified, and load nothing; an MD5 mismatch logs
//! no line, its frames already built, and skips `FRAMES_LOADED`. Either way the client quits with
//! error 9 (then the quit post `ForceQuit` makes with code 0). Deviation: no signature or digest
//! check, so the chain's toc always loads and `FRAMES_LOADED` follows, because the check verifies
//! Blizzard's signature with a key carried in the client binary, which benilla does not ship, and a
//! modified interface file is loaded rather than refused.
//!
//! The screen comes after the build: at startup the reference fires `SET_GLUE_SCREEN` with
//! `"movie"` while the `movie` CVar is 1 (clearing it), else `"login"`; back from the world fires
//! it with `"charselect"` or `"login"`. benilla plays no movie yet (#130), so its boot screen is
//! `"login"`.
//!
//! The screens' `ModelFFX` panes drive the glue booth ([`super::glue_scene`]): its scene rig, the
//! scene M2's own point lights and the `FfxGlow::GLUE_SCENE` pass, never the `ui_models` tile
//! path, where every lit batch draws black.

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use super::reference_ui;

/// The glue screen `SET_GLUE_SCREEN` names, `GlueParent.lua`'s `GlueScreenInfo` key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueScreen {
    Login,
    CharSelect,
}

impl GlueScreen {
    /// The key, as the reference's literals spell it.
    pub(crate) fn key(self) -> &'static str {
        match self {
            GlueScreen::Login => "login",
            GlueScreen::CharSelect => "charselect",
        }
    }
}

/// Load the chain's `GlueXML.toc` into `script`, a glue VM, and fire `FRAMES_LOADED`: the glue
/// builder's steps from the toc on. Returns every failure; a loader error is tagged
/// `"GlueXML/<file>: <error>"`, a handler that raised during the walk is one too. The load's record
/// is left for [`finish_glue_load_log`].
pub(super) fn load_glue(script: &mut UiScript) -> Vec<String> {
    let mut failures = match reference_ui::glue() {
        Some(glue) => {
            let mut toc = benilla_ui::status::Status::default();
            let failures = glue.load_files_into(script, &glue.toc.files, &mut toc);
            let mut log = benilla_ui::status::Status::default();
            let banner = benilla_ui::status::toc_banner(reference_ui::GLUE_TOC);
            toc.close_into(&mut log, script.framexml_debug(), banner);
            script.report_load_status(log);
            failures
        }
        None => {
            // The runner's `"Couldn't open %s"` at severity 2; no glue frame exists.
            let e = format!(
                "{}: not found — the stock glue screens are not built",
                reference_ui::GLUE_TOC
            );
            error!("ui_script: {e}");
            script.report_load_failure(&e);
            let mut log = benilla_ui::status::Status::default();
            log.report(
                benilla_ui::status::FAILURE,
                benilla_ui::status::missing(reference_ui::GLUE_TOC, false),
            );
            script.report_load_status(log);
            vec![e]
        }
    };
    script.fire_event("FRAMES_LOADED", vec![]);
    failures.extend(
        script
            .errors()
            .into_iter()
            .map(|e| format!("a handler raised during the glue load: {e}")),
    );
    failures
}

/// Drain the glue load's record into `Logs\GlueXML.log`, as the builder does after `FRAMES_LOADED`.
pub(super) fn finish_glue_load_log(script: &mut UiScript) {
    script.finish_ui_load_log();
    super::load_log::write_glue(script.take_load_log_writes());
}

/// Put `screen` up: `SET_GLUE_SCREEN` with its key as `arg1` (event 0).
pub(super) fn set_glue_screen(script: &mut UiScript, screen: GlueScreen) {
    script.fire_event(
        "SET_GLUE_SCREEN",
        vec![ScriptValue::Str(screen.key().to_string())],
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The glue functions the stock GlueXML calls that benilla has not built yet: each one's call
    /// raises "attempt to call global `X' (a nil value)", at the load or on the login screen. The
    /// list only shrinks: a name built, or no longer reached, comes off it.
    const UNBUILT_CALLED: &[&str] = &[];

    /// CVars the glue's files read that benilla does not register yet: each read warns, and
    /// `GetCVarMin` raises `Couldn't find CVar named`. The list only shrinks.
    const UNREGISTERED_CVARS: &[&str] = &[];

    /// What the player's own files make the reference report too: their `tos.html` uses `<H4>`,
    /// which 1.12's SimpleHTML does not know (`"Frame %s: Unknown element type: %s"`, and the walk
    /// goes on).
    const INSTALL_WARNINGS: &[&str] = &["Frame TOSText: Unknown element type: H4"];

    /// The movie frame's three handlers, which benilla's `MovieFrame` does not take: it plays no
    /// movie (#130), so nothing would fire them, where the reference takes all three. Each is a
    /// loader warning, `<frame>: SetScript("<handler>") unsupported`; the list only shrinks.
    const UNBUILT_HANDLERS: &[(&str, &str)] = &[
        ("MovieFrame", "OnMovieFinished"),
        ("MovieFrame", "OnMovieHideSubtitle"),
        ("MovieFrame", "OnMovieShowSubtitle"),
    ];

    /// The frames each stock glue screen and dialog is, which the load must build.
    const SCREENS: [&str; 8] = [
        "GlueParent",
        "AccountLogin",
        "CharacterSelect",
        "CharacterCreate",
        "RealmList",
        "GlueDialog",
        "RealmWizard",
        "AddonList",
    ];

    /// The name a "nil value" call error names, in either of Lua's two spellings of it.
    fn nil_call(error: &str) -> Option<&str> {
        let (_, rest) = error.split_once("attempt to call ")?;
        let (kind, rest) = rest.split_once(' ')?;
        if !matches!(kind, "global" | "field" | "method") {
            return None;
        }
        let rest = rest.strip_prefix('`').or_else(|| rest.strip_prefix('\''))?;
        let (name, rest) = rest.split_once('\'')?;
        rest.starts_with(" (a nil value)").then_some(name)
    }

    /// The glue state's functions: the rows of the glue manager's registrar tables in
    /// `reference/3.3.5a-shapes.tsv` (the glue core, character select and create, the realm
    /// list, and the tables it shares with the in-game state).
    fn glue_surface() -> BTreeSet<String> {
        const TABLES: [&str; 9] = [
            "t022", "t025", "t024", "t023", "t093", "t100", "t113", "t038", "t037",
        ];
        let path = format!(
            "{}/../../reference/3.3.5a-shapes.tsv",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {path}: {e}"))
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.split('\t').collect::<Vec<_>>())
            .filter(|f| f.len() > 1 && TABLES.contains(&f[1]))
            .map(|f| f[0].to_string())
            .collect()
    }

    #[test]
    fn a_nil_call_names_its_function() {
        assert_eq!(
            nil_call("x.lua:1: attempt to call global `GetBuildInfo' (a nil value)"),
            Some("GetBuildInfo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to call field 'Foo' (a nil value)"),
            Some("Foo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to index global `X' (a nil value)"),
            None
        );
        assert_eq!(nil_call("x.lua:1: bad argument #1"), None);
    }

    /// The stock GlueXML loads into the glue VM, `FRAMES_LOADED` fires and the login screen goes
    /// up, and a few frames run: every screen's frame exists, and the only Lua errors are calls
    /// of the glue functions [`UNBUILT_CALLED`] names, each of which is still unbuilt and still
    /// called. Any other error, an XML problem or a missing widget method, fails here.
    #[test]
    fn the_stock_glue_loads_with_only_unbuilt_verbs_raising() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        // The CVar registry the app seeds every VM with.
        s.seed_cvars(crate::cvars::Cvars::default().vm_seed());
        let mut errors = load_glue(&mut s);
        // `load_glue` reports the walk's handler errors; the rest are the screen's and the ticks'.
        let _ = s.take_errors();
        set_glue_screen(&mut s, GlueScreen::Login);
        for _ in 0..3 {
            s.tick(0.1);
        }
        s.resolve();
        errors.extend(s.take_errors());
        // Every row the session retained: the load's failures and warnings, and what raised.
        errors.extend(s.diagnostics().into_iter().map(|d| d.message));

        let surface = glue_surface();
        let mut called = BTreeSet::new();
        let mut handlers = BTreeSet::new();
        let other: Vec<&String> = errors
            .iter()
            .filter(|e| {
                if let Some(name) = nil_call(e).filter(|n| UNBUILT_CALLED.contains(n)) {
                    called.insert(name);
                    return false;
                }
                if INSTALL_WARNINGS.contains(&e.as_str()) {
                    return false;
                }
                if UNREGISTERED_CVARS.iter().any(|c| {
                    e.as_str() == format!("unknown CVar '{c}' (not host-registered) — ignored")
                        || e.contains(&format!("Couldn't find CVar named '{c}'"))
                }) {
                    return false;
                }
                if let Some(h) = UNBUILT_HANDLERS.iter().find(|(frame, handler)| {
                    e.contains(&format!("{frame}: SetScript(\"{handler}\") unsupported"))
                }) {
                    handlers.insert(*h);
                    return false;
                }
                true
            })
            .collect();
        assert!(
            other.is_empty(),
            "the glue load raised {} error(s) or warning(s) that are not a call of an unbuilt \
             glue function on UNBUILT_CALLED or a handler on UNBUILT_HANDLERS:\n{}",
            other.len(),
            other
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let listed: BTreeSet<&str> = UNBUILT_CALLED.iter().copied().collect();
        let stale: Vec<&&str> = listed.difference(&called).collect();
        assert!(
            stale.is_empty(),
            "UNBUILT_CALLED names no longer called unbuilt; take them off: {stale:?}"
        );
        let stale: Vec<&(&str, &str)> = UNBUILT_HANDLERS
            .iter()
            .filter(|h| !handlers.contains(*h))
            .collect();
        assert!(
            stale.is_empty(),
            "UNBUILT_HANDLERS entries the load no longer warns about; take them off: {stale:?}"
        );
        for name in UNBUILT_CALLED {
            assert!(
                surface.contains(*name),
                "{name} is not a glue-state function"
            );
            assert!(
                s.eval::<bool>(&format!("return {name} == nil")).unwrap(),
                "{name} is built; take it off UNBUILT_CALLED"
            );
        }

        for name in SCREENS {
            assert!(
                s.eval::<bool>(&format!(
                    "return type({name}) == 'table' and {name}.GetObjectType ~= nil"
                ))
                .unwrap(),
                "the glue load built no frame {name}"
            );
        }
        assert_eq!(
            s.eval::<(bool, bool)>(
                "return AccountLogin:IsShown() == 1, CharacterSelect:IsShown() == 1"
            )
            .unwrap(),
            (true, false),
            "SET_GLUE_SCREEN \"login\" shows the login screen alone"
        );
    }

    /// Each Cinematics button shows `MovieFrame`, whose stock walk tries every movie of its
    /// version; `StartMovie` answers nil, so the walk ends by hiding the frame and its `OnHide`
    /// puts the login screen back, with nothing raised (#301).
    #[test]
    fn the_cinematics_buttons_come_back_to_the_login_screen() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.seed_cvars(crate::cvars::Cvars::default().vm_seed());
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        s.tick(0.1);
        let _ = s.take_errors();
        s.run(
            "Tried = 0; local start = MovieFrame.StartMovie; \
             MovieFrame.StartMovie = function(self, ...) Tried = Tried + 1; return start(self, ...) end",
        )
        .unwrap();
        // Two files per movie with a video, high resolution then the first: version 1 has two
        // movies, versions 2 and 3 one each past an empty first slot.
        for (button, tries) in [(1, 4), (2, 2), (3, 2)] {
            s.run(&format!(
                "Tried = 0; CinematicsFrame:Show(); CinematicsButton{button}:Click()"
            ))
            .unwrap();
            s.tick(0.1);
            s.resolve();
            let errors = s.take_errors();
            assert!(errors.is_empty(), "button {button}: {errors:?}");
            assert_eq!(
                s.eval::<(i64, i64, bool, bool, bool)>(
                    "return MovieFrame.version, Tried, AccountLogin:IsShown() == 1, \
                     MovieFrame:IsShown() == 1, CinematicsFrame:IsShown() == 1"
                )
                .unwrap(),
                (button, tries, true, false, false),
                "button {button}: version, StartMovie calls, login, movie, cinematics shown"
            );
        }
    }

    /// `GlueParent` fills the screen root, 768 tall and `768·a` wide, up to 16:9; past that
    /// `GlueParent_OnLoad` centres it 16:9 wide on the root's height.
    #[test]
    fn glue_parent_fills_the_root_up_to_sixteen_by_nine() {
        let _ = benilla_formats::wow_data_or_skip!();
        let rect = |aspect: f32| {
            let mut s = UiScript::new_glue().unwrap();
            s.set_screen_size(768.0 * aspect, 768.0);
            let _ = load_glue(&mut s);
            s.resolve();
            s.eval::<(f32, f32, f32, f32)>(
                "return GlueParent:GetLeft(), GlueParent:GetRight(), GlueParent:GetBottom(), \
                 GlueParent:GetTop()",
            )
            .unwrap()
        };
        let near = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 0.01)
        };
        for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0] {
            let got = rect(aspect);
            assert!(
                near(got, (0.0, 768.0 * aspect, 0.0, 768.0)),
                "{aspect}: {got:?}"
            );
        }
        let wide = 21.0 / 9.0;
        let bar = (768.0 * wide - 768.0 * 16.0 / 9.0) / 2.0;
        let got = rect(wide);
        assert!(
            near(got, (bar, 768.0 * wide - bar, 0.0, 768.0)),
            "21:9: {got:?}"
        );
    }

    /// The stock login's boxes take their editing keys: Backspace deletes in the focused box and
    /// Tab runs its `OnTabPressed`, with `AccountLogin`'s `OnKeyDown` above them in the key walk.
    /// Both reach the box only through the walk, as the input feed sends them.
    #[test]
    fn the_stock_login_boxes_take_backspace_and_tab() {
        use benilla_ui::script::{EditAction, EditUnit};
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.seed_cvars(crate::cvars::Cvars::default().vm_seed());
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        for _ in 0..3 {
            s.tick(0.1);
        }
        s.resolve();
        // The panel as it stands once the agreements are accepted, which the glue manager reads.
        s.run(
            "AccountLoginUI:Show() AccountLoginAccountEdit:SetText('')              AccountLoginAccountEdit:SetFocus()",
        )
        .unwrap();
        for c in ["a", "b", "c"] {
            s.char_input(c);
        }
        // The feed offers BACKSPACE to the keyboard frames first, then the box's chord.
        assert!(
            !s.frame_key_input("BACKSPACE"),
            "a keyboard frame above the focused box took BACKSPACE"
        );
        s.editbox_action(EditAction::Delete {
            unit: EditUnit::Char,
            back: true,
        });
        assert_eq!(
            s.eval::<String>("return AccountLoginAccountEdit:GetText()")
                .unwrap(),
            "ab"
        );
        assert!(s.key_input("TAB"), "TAB reached nothing");
        assert_eq!(
            s.focused_editbox_name().as_deref(),
            Some("AccountLoginPasswordEdit"),
            "the account box's OnTabPressed moves the focus to the password"
        );
    }
}
