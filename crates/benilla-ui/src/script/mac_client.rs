//! The verbs only the Mac build of 12340 registers: the movie recorder's and the music player's,
//! which the stock UI's Mac arms call (`MacOptionsFrame.lua`, `MovieRecordingProgress.xml`, the
//! `platform="mac"` rows of `Bindings.xml`). A VM answering as the Mac build installs them
//! ([`super::ClientOs::Mac`]); the PC build has none.
//!
//! Every address in this file is the Mac 12340 build's (its i386 half), not the Windows one's.
//!
//! Deviation: benilla records no movie and drives no music player (`docs/DEVIATIONS.md`). The
//! recorder verbs answer as the Mac client does on a machine below the recorder's requirements,
//! where its support test (`0x909f40`) is false; the music player verbs do nothing.

use mlua::{Lua, MultiValue, Value};

use super::ScriptValue;

/// The Mac build's own globals, in its registrar's order (table `0xd9b0c0`).
pub(crate) const VERBS: [&str; 23] = [
    "MusicPlayer_PlayPause",
    "MusicPlayer_VolumeUp",
    "MusicPlayer_VolumeDown",
    "MusicPlayer_BackTrack",
    "MusicPlayer_NextTrack",
    "MovieRecording_Toggle",
    "MovieRecording_Cancel",
    "MovieRecording_IsRecording",
    "MovieRecording_IsCompressing",
    "MovieRecording_GetProgress",
    "MovieRecording_GetViewportWidth",
    "MovieRecording_GetAspectRatio",
    "MovieRecording_IsSupported",
    "MovieRecording_IsCodecSupported",
    "MovieRecording_IsCursorRecordingSupported",
    "MovieRecording_MaxLength",
    "MovieRecording_DataRate",
    "MovieRecording_GetTime",
    "MovieRecording_GetMovieFullPath",
    "MovieRecording_SearchUncompressedMovie",
    "MovieRecording_QueueMovieToCompress",
    "MovieRecording_DeleteMovie",
    "MovieRecording_ToggleGUI",
];

/// The viewport in whole pixels, width then height.
fn viewport(lua: &Lua) -> (i32, i32) {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let screen = model.screen;
    (
        (screen.right - screen.left) as i32,
        (screen.top - screen.bottom) as i32,
    )
}

/// The viewport's height over its width (`0x905f10`), a single-precision divide.
fn aspect_ratio(lua: &Lua) -> f32 {
    let (width, height) = viewport(lua);
    height as f32 / width as f32
}

/// The recorder's double-to-unsigned cast: clamped to `0..=4294967295`, then truncated.
fn to_u32(n: f64) -> u32 {
    n.clamp(0.0, 4_294_967_295.0) as u32
}

/// The bytes a second of uncompressed recording takes (`0x9048e0`): both sides rounded down to a
/// multiple of 4, one and a half bytes a pixel, 16 on top. The sound term the reference adds is
/// its capture format's (`[0xda3880]`), zero until a recording opens one.
fn bytes_per_second(width: f64, framerate: f64, ratio: f32) -> u32 {
    let height = to_u32(f64::from(ratio) * width);
    let pixels = (to_u32(width) & !3).wrapping_mul(height & !3);
    to_u32(f64::from(pixels as f32 * framerate as f32) * 1.5 + 16.0)
}

/// `MovieRecording_DataRate`'s text (`0x508f60`), by size.
fn data_rate_text(bytes: u32) -> String {
    let scaled = |by: f32| f64::from(bytes as f32 * by);
    if bytes > 0x4000_0000 {
        format!("{:.3} GB/s", scaled(1.0 / 1_073_741_824.0))
    } else if bytes > 0x10_0000 {
        format!("{:.3} MB/s", scaled(1.0 / 1_048_576.0))
    } else if bytes > 0x400 {
        format!("{:.3} KB/s", scaled(1.0 / 1024.0))
    } else {
        format!("{bytes} B/s")
    }
}

/// The three `is-number` tests `MaxLength` and `DataRate` open with, raising `usage` on a miss.
fn recording_args(lua: &Lua, args: &MultiValue, usage: &'static str) -> mlua::Result<(f64, f64)> {
    let mut numbers = [0.0; 3];
    for (i, slot) in numbers.iter_mut().enumerate() {
        let arg = args.get(i).cloned().unwrap_or(Value::Nil);
        *slot = lua
            .coerce_number(arg)?
            .ok_or_else(|| mlua::Error::RuntimeError(usage.into()))?;
    }
    Ok((numbers[0], numbers[1]))
}

/// The text of a GlobalStrings key, or the key itself where the table has none.
fn global_text(lua: &Lua, key: &str) -> mlua::Result<String> {
    Ok(lua
        .globals()
        .get::<Option<String>>(key)?
        .unwrap_or_else(|| key.to_string()))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The music player's five (`0x51c100` on): secure-only, then one command to the player, no
    // returns. benilla has no taint model and no player to command.
    for name in &VERBS[..5] {
        g.set(
            *name,
            lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
        )?;
    }

    // `MovieRecording_Toggle()` (`0x511840`): without the recorder's requirements it shows
    // `MOVIE_RECORDING_WARNING_REQUIREMENTS` as a UI error and starts nothing.
    g.set(
        "MovieRecording_Toggle",
        lua.create_function(|lua, _: MultiValue| {
            let text = global_text(lua, "MOVIE_RECORDING_WARNING_REQUIREMENTS")?;
            super::tick::fire_event_into(lua, "UI_ERROR_MESSAGE", vec![ScriptValue::Str(text)]);
            Ok(MultiValue::new())
        })?,
    )?;

    // No returns, and nothing to act on with no recording and no movie on disk: `Cancel`
    // (`0x5088e0`) and `SearchUncompressedMovie` (`0x508a50`) stop at the support test;
    // `QueueMovieToCompress` (`0x508a90`) and `DeleteMovie` (`0x508ac0`) take a recorded movie's
    // path.
    for name in [
        "MovieRecording_Cancel",
        "MovieRecording_SearchUncompressedMovie",
        "MovieRecording_QueueMovieToCompress",
        "MovieRecording_DeleteMovie",
    ] {
        g.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
        )?;
    }

    // One boolean each, false: `IsRecording` (`0x508910`) and `IsCompressing` (`0x508940`) read
    // the recorder's idle state, `IsSupported` (`0x508af0`) and `IsCursorRecordingSupported`
    // (`0x508ba0`) the support test. `IsCodecSupported(codec)` (`0x508b20`) asks QuickTime for the
    // codec, which benilla does not have.
    for name in [
        "MovieRecording_IsRecording",
        "MovieRecording_IsCompressing",
        "MovieRecording_IsSupported",
        "MovieRecording_IsCodecSupported",
        "MovieRecording_IsCursorRecordingSupported",
    ] {
        g.set(name, lua.create_function(|_, _: MultiValue| Ok(false))?)?;
    }

    // `MovieRecording_GetProgress()` (`0x5089a0`): whether the compressor is idle, then its
    // progress, `true` and 1 at rest.
    g.set(
        "MovieRecording_GetProgress",
        lua.create_function(|_, _: MultiValue| Ok((true, 1.0)))?,
    )?;

    // `MovieRecording_GetViewportWidth()` (`0x5089f0`): the viewport's width in pixels.
    g.set(
        "MovieRecording_GetViewportWidth",
        lua.create_function(|lua, _: MultiValue| Ok(f64::from(viewport(lua).0)))?,
    )?;

    // `MovieRecording_GetAspectRatio()` (`0x508a20`): height over width, which the stock UI
    // multiplies a movie width by.
    g.set(
        "MovieRecording_GetAspectRatio",
        lua.create_function(|lua, _: MultiValue| Ok(f64::from(aspect_ratio(lua))))?,
    )?;

    // `MovieRecording_MaxLength(width, framerate, capturesound)` (`0x508d00`): the time the free
    // disk space holds as `h:mm:ss`. The estimate (`0x90a340`) is zero below the requirements.
    g.set(
        "MovieRecording_MaxLength",
        lua.create_function(|lua, args: MultiValue| {
            recording_args(
                lua,
                &args,
                "Usage: MovieRecording_MaxLength(width, framerate, capturesound)",
            )?;
            Ok("0:00:00")
        })?,
    )?;

    // `MovieRecording_DataRate(width, framerate, capturesound)` (`0x508f60`): the bytes a second
    // of recording takes at that size, as text.
    g.set(
        "MovieRecording_DataRate",
        lua.create_function(|lua, args: MultiValue| {
            let (width, framerate) = recording_args(
                lua,
                &args,
                "Usage: MovieRecording_DataRate(width, framerate, capturesound)",
            )?;
            Ok(data_rate_text(bytes_per_second(
                width,
                framerate,
                aspect_ratio(lua),
            )))
        })?,
    )?;

    // `MovieRecording_GetTime()` (`0x508c00`): the running recording's length as `h:mm:ss`.
    // Deviation: with none running the reference counts from its unset start time; benilla
    // answers zero.
    g.set(
        "MovieRecording_GetTime",
        lua.create_function(|_, _: MultiValue| Ok("0:00:00"))?,
    )?;

    // `MovieRecording_GetMovieFullPath()` (`0x508bd0`): the current movie's path, empty with none.
    g.set(
        "MovieRecording_GetMovieFullPath",
        lua.create_function(|_, _: MultiValue| Ok(""))?,
    )?;

    // `MovieRecording_ToggleGUI()` (`0x50e910`): flips `MovieRecordingGUI` and prints
    // `MOVIE_RECORDING_GUI_ON` or `MOVIE_RECORDING_GUI_OFF` as a system line. No support test.
    g.set(
        "MovieRecording_ToggleGUI",
        lua.create_function(|lua, _: MultiValue| {
            let on = {
                let mut model = crate::script::model_mut_of(lua);
                let was_on = model
                    .cvars
                    .get("movierecordinggui")
                    .is_some_and(|slot| super::cvars::sstr_to_int(&slot.value) != 0);
                let value = if was_on { "0" } else { "1" };
                super::cvars::set_from_engine(&mut model, "MovieRecordingGUI", value.to_string());
                !was_on
            };
            let text = global_text(
                lua,
                if on {
                    "MOVIE_RECORDING_GUI_ON"
                } else {
                    "MOVIE_RECORDING_GUI_OFF"
                },
            )?;
            crate::script::model_mut_of(lua).party_lines.push(text);
            Ok(MultiValue::new())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{ClientOs, UiScript};

    fn mac() -> UiScript {
        UiScript::new_on(ClientOs::Mac).unwrap()
    }

    /// The Mac build has its 23 verbs and the PC build none of them; nothing else differs in the
    /// two builds' globals.
    #[test]
    fn the_mac_verbs_are_the_mac_builds_alone() {
        let globals = |s: &UiScript| {
            let mut names = s
                .eval::<Vec<String>>(
                    "local out = {} \
                     for k in pairs(getfenv(0)) do if type(k) == 'string' then table.insert(out, k) end end \
                     return out",
                )
                .unwrap();
            names.sort_unstable();
            names
        };
        let pc = globals(&UiScript::new_on(ClientOs::Pc).unwrap());
        let mut only_mac = globals(&mac());
        only_mac.retain(|n| !pc.contains(n));
        let mut verbs = VERBS.map(str::to_string).to_vec();
        verbs.sort_unstable();
        assert_eq!(only_mac, verbs);
        // The glue registers none of them.
        let glue = UiScript::new_glue_on(ClientOs::Mac).unwrap();
        for verb in VERBS {
            assert!(
                glue.eval::<bool>(&format!("return {verb} == nil")).unwrap(),
                "{verb}"
            );
        }
    }

    /// Each verb's return count, and the booleans as real booleans.
    #[test]
    fn the_recorder_answers_as_one_below_its_requirements() {
        let s = mac();
        for (call, count) in [
            ("MusicPlayer_PlayPause()", 0),
            ("MusicPlayer_VolumeUp()", 0),
            ("MusicPlayer_VolumeDown()", 0),
            ("MusicPlayer_BackTrack()", 0),
            ("MusicPlayer_NextTrack()", 0),
            ("MovieRecording_Toggle()", 0),
            ("MovieRecording_Cancel()", 0),
            ("MovieRecording_SearchUncompressedMovie(true)", 0),
            ("MovieRecording_QueueMovieToCompress('a.mov')", 0),
            ("MovieRecording_DeleteMovie('a.mov')", 0),
            ("MovieRecording_ToggleGUI()", 0),
            ("MovieRecording_IsRecording()", 1),
            ("MovieRecording_IsCompressing()", 1),
            ("MovieRecording_IsSupported()", 1),
            ("MovieRecording_IsCodecSupported(1635148593)", 1),
            ("MovieRecording_IsCursorRecordingSupported()", 1),
            ("MovieRecording_GetProgress()", 2),
            ("MovieRecording_GetViewportWidth()", 1),
            ("MovieRecording_GetAspectRatio()", 1),
            ("MovieRecording_MaxLength(640, 29.97, 1)", 1),
            ("MovieRecording_DataRate(640, 29.97, 1)", 1),
            ("MovieRecording_GetTime()", 1),
            ("MovieRecording_GetMovieFullPath()", 1),
        ] {
            assert_eq!(s.arity(call).unwrap(), count, "{call}");
        }
        for verb in [
            "IsRecording",
            "IsCompressing",
            "IsSupported",
            "IsCodecSupported",
            "IsCursorRecordingSupported",
        ] {
            assert!(
                s.eval::<bool>(&format!("return MovieRecording_{verb}(0) == false"))
                    .unwrap(),
                "{verb}"
            );
        }
        assert!(s
            .eval::<bool>(
                "local idle, at = MovieRecording_GetProgress() return idle == true and at == 1"
            )
            .unwrap());
        assert_eq!(
            s.eval::<(String, String, String)>(
                "return MovieRecording_MaxLength(640, 29.97, 1), MovieRecording_GetTime(), \
                 MovieRecording_GetMovieFullPath()"
            )
            .unwrap(),
            ("0:00:00".to_string(), "0:00:00".to_string(), String::new())
        );
    }

    /// The viewport pair follows the screen, the ratio as height over width.
    #[test]
    fn the_viewport_verbs_follow_the_screen() {
        let mut s = mac();
        s.set_screen_size(1680.0, 1050.0);
        assert_eq!(
            s.eval::<(f64, f64)>(
                "return MovieRecording_GetViewportWidth(), MovieRecording_GetAspectRatio()"
            )
            .unwrap(),
            (1680.0, 0.625)
        );
    }

    /// The data rate is one and a half bytes a pixel a frame plus 16, both sides rounded down to a
    /// multiple of 4, in the unit its size picks.
    #[test]
    fn the_data_rate_is_the_frame_size_times_the_framerate() {
        let mut s = mac();
        s.set_screen_size(1024.0, 768.0);
        let rate = |args: &str| {
            s.eval::<String>(&format!("return MovieRecording_DataRate({args})"))
                .unwrap()
        };
        // 640x480 at 29.97: 13810192 bytes.
        assert_eq!(rate("640, 29.97, 1"), "13.170 MB/s");
        assert_eq!(rate("'640', '29.97', 0"), "13.170 MB/s");
        // 4x3 rounds its height down to 0.
        assert_eq!(rate("4, 10, 0"), "16 B/s");
        // 8x4 at 10: 496 bytes; at 30, 1456.
        assert_eq!(rate("8, 10, 0"), "496 B/s");
        assert_eq!(rate("10, 30, 0"), "1.422 KB/s");
        // 4096x3072 at 60: over a gigabyte.
        assert_eq!(rate("4096, 60, 0"), "1.055 GB/s");
        assert_eq!(rate("-5, 30, 0"), "16 B/s");
        assert_eq!(data_rate_text(0x400), "1024 B/s");
        assert_eq!(data_rate_text(0x401), "1.001 KB/s");
        assert_eq!(data_rate_text(0x10_0000), "1024.000 KB/s");
        assert_eq!(data_rate_text(0x4000_0000), "1024.000 MB/s");
    }

    /// Both estimators raise their usage line unless all three arguments read as numbers.
    #[test]
    fn the_estimators_raise_their_usage() {
        let s = mac();
        for verb in ["MaxLength", "DataRate"] {
            let usage = format!("Usage: MovieRecording_{verb}(width, framerate, capturesound)");
            for args in [
                "",
                "640",
                "640, 30",
                "640, 30, true",
                "nil, 30, 1",
                "640, 'x', 1",
            ] {
                let err = s
                    .run(&format!("MovieRecording_{verb}({args})"))
                    .expect_err(args)
                    .to_string();
                assert!(err.contains(&usage), "{verb}({args}): {err}");
            }
        }
    }

    /// `Toggle` shows the requirements warning as a UI error.
    #[test]
    fn toggle_shows_the_requirements_warning() {
        let s = mac();
        s.run(
            "MOVIE_RECORDING_WARNING_REQUIREMENTS = 'needs more' \
             local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE') \
             f:SetScript('OnEvent', function(_, _, text) seen = text end) \
             MovieRecording_Toggle()",
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return seen").unwrap(), "needs more");
    }

    /// `ToggleGUI` flips the CVar and prints the matching system line.
    #[test]
    fn toggle_gui_flips_its_cvar() {
        let mut s = mac();
        s.register_cvars([("MovieRecordingGUI", "1")]);
        s.run("MOVIE_RECORDING_GUI_ON = 'on' MOVIE_RECORDING_GUI_OFF = 'off'")
            .unwrap();
        s.run("MovieRecording_ToggleGUI()").unwrap();
        assert_eq!(s.cvar("MovieRecordingGUI").as_deref(), Some("0"));
        assert_eq!(s.take_party_lines(), ["off"]);
        s.run("MovieRecording_ToggleGUI()").unwrap();
        assert_eq!(s.cvar("MovieRecordingGUI").as_deref(), Some("1"));
        assert_eq!(s.take_party_lines(), ["on"]);
    }
}
