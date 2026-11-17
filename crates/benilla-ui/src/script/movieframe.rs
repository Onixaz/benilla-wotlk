//! The `MovieFrame` methods (method table `0xb2cd30`): `StartMovie` (`0x970660`), `StopMovie`
//! (`0x970730`) and `EnableSubtitles` (`0x970770`), and the frame's per-frame caption pass.
//!
//! The pictures and sound are the app's, reached through [`MovieHost`]; the VM keeps what the
//! reference's frame keeps for its scripts: the playing flag `+0x2a0`, the subtitle switch
//! `+0x2a8` and the captions. `StartMovie` (`0x95eb30`) opens `<file>.avi` and answers 1, or nil
//! when the file does not open; `StopMovie` (`0x95eba0`) closes the video and audio, hides a
//! shown caption (`0x95ea90`), clears the flag, then runs `OnMovieFinished`, so a movie stopped
//! from Lua finishes exactly as one that ran out.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::bool_or_default;
use super::object::frame_handle_of;
use crate::widget::{FrameHandle, FrameKind, KindState, MovieCaption};

pub(super) const REG_MOVIEFRAME_METHODS: &str = "__benilla_movieframe_methods";

/// The app's movie player, as the MovieFrame verbs reach it.
pub trait MovieHost {
    /// Open `<file>.avi` for `frame` and start its video and its audio at `volume` (0 to 1), and
    /// read `<file>.sbt`: its captions, or `None` when the movie does not open (`0x95dc80`,
    /// `0x95e350`).
    fn open(&self, frame: FrameHandle, file: &str, volume: f32) -> Option<Vec<MovieCaption>>;
    /// Close `frame`'s video and audio (`0x95e2d0`, `0x95e170`).
    fn close(&self, frame: FrameHandle);
}

/// The app's report of one rendered movie frame (`0x95ebf0`), the input to the caption pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovieTick {
    /// Milliseconds into the movie: the sound's position while it plays, else the wall clock.
    pub elapsed_ms: u32,
    /// The movie's sound is playing.
    pub sound_playing: bool,
    /// The movie has run out: past its last frame, its sound ended, or a frame failed to decode.
    pub finished: bool,
    /// A new frame was shown this tick; captions move only then.
    pub advanced: bool,
}

/// Raises unless `this` is a live MovieFrame, since a caller can lift the method table onto
/// another frame.
fn check_movie_frame(lua: &Lua, this: &Table) -> mlua::Result<FrameHandle> {
    let h = frame_handle_of(lua, this)?;
    let is_movie = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::MovieFrame);
    if is_movie {
        Ok(h)
    } else {
        Err(mlua::Error::runtime(
            "Wrong object type for member function",
        ))
    }
}

/// Run `f` on `h`'s movie state, if `h` is a live MovieFrame.
fn with_state<R>(
    lua: &Lua,
    h: FrameHandle,
    f: impl FnOnce(&mut crate::widget::MovieState) -> R,
) -> Option<R> {
    let mut model = super::model_mut_of(lua);
    match model.arena.frame_mut(h).map(|fr| &mut fr.kind_state) {
        Some(KindState::Movie(m)) => Some(f(m)),
        _ => None,
    }
}

fn host(lua: &Lua) -> Option<std::rc::Rc<dyn MovieHost>> {
    super::model_mut_of(lua).movie_host.clone()
}

fn fire(lua: &Lua, h: FrameHandle, script: &str, args: Vec<Value>) {
    let id = super::model_mut_of(lua).frame_id(h);
    if let Err(e) = super::event::fire_widget_handler(lua, id, script, args) {
        super::model_mut_of(lua).record_lua_error(&e);
    }
}

/// `0x95eba0`: a playing movie closes, a shown caption hides, then `OnMovieFinished` runs.
pub(crate) fn stop(lua: &Lua, h: FrameHandle) {
    let Some(Some(had_caption)) = with_state(lua, h, |m| {
        if !m.playing {
            return None;
        }
        let shown = m.shown.take().is_some();
        m.captions.clear();
        m.playing = false;
        Some(shown)
    }) else {
        return;
    };
    if let Some(host) = host(lua) {
        host.close(h);
    }
    if had_caption {
        fire(lua, h, "OnMovieHideSubtitle", Vec::new());
    }
    fire(lua, h, "OnMovieFinished", Vec::new());
}

/// `0x95ebf0` past its timing: a finished movie stops; else, on a new frame, a caption whose end
/// has passed hides, and with none shown, while subtitles are on or no sound plays, the first
/// caption covering the moment shows.
pub(crate) fn tick(lua: &Lua, h: FrameHandle, t: MovieTick) {
    let playing = with_state(lua, h, |m| m.playing).unwrap_or(false);
    if !playing {
        return;
    }
    if t.finished {
        stop(lua, h);
        return;
    }
    if !t.advanced {
        return;
    }
    let hide = with_state(lua, h, |m| {
        let ended = m
            .shown
            .is_some_and(|i| m.captions.get(i).is_none_or(|c| c.end_ms <= t.elapsed_ms));
        if ended {
            m.shown = None;
        }
        ended
    })
    .unwrap_or(false);
    if hide {
        fire(lua, h, "OnMovieHideSubtitle", Vec::new());
    }
    let show = with_state(lua, h, |m| {
        if m.shown.is_some() || !(m.subtitles || !t.sound_playing) {
            return None;
        }
        let i = m
            .captions
            .iter()
            .position(|c| c.start_ms <= t.elapsed_ms && c.end_ms > t.elapsed_ms)?;
        m.shown = Some(i);
        Some(m.captions[i].text.clone())
    })
    .flatten();
    if let Some(text) = show {
        match lua.create_string(&text) {
            Ok(s) => fire(lua, h, "OnMovieShowSubtitle", vec![Value::String(s)]),
            Err(e) => super::model_mut_of(lua).record_lua_error(&e),
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `lua_isstring` on the file and `lua_isnumber` on the volume, else the usage error
    // (`0xaa1218`); one value back, 1 for a started movie (`0x9706ce`) and nil for one that is not.
    m.set(
        "StartMovie",
        lua.create_function(|lua, (this, file, volume): (Table, Value, Value)| {
            let h = check_movie_frame(lua, &this)?;
            let file = match file {
                Value::String(_) | Value::Integer(_) | Value::Number(_) => lua
                    .coerce_string(file)
                    .ok()
                    .flatten()
                    .map(|s| s.to_string_lossy()),
                _ => None,
            };
            let volume = lua.coerce_number(volume).ok().flatten();
            let (Some(file), Some(volume)) = (file, volume) else {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:StartMovie(\"filename\", volume_0_to_255)",
                    who.as_deref()
                        .unwrap_or(super::object::anchor_args::UNNAMED)
                )));
            };
            // `vol · (1/255)`, clamped into the sound's 0..1 (`0x95e600`).
            let gain = ((volume as i32) as f32 / 255.0).clamp(0.0, 1.0);
            let Some(captions) = host(lua).and_then(|host| host.open(h, &file, gain)) else {
                return Ok(Value::Nil);
            };
            with_state(lua, h, |m| {
                m.captions = captions;
                m.shown = None;
                m.playing = true;
            });
            Ok(Value::Integer(1))
        })?,
    )?;
    // No arguments, no returns (`0x970762`).
    m.set(
        "StopMovie",
        lua.create_function(|lua, this: Table| {
            let h = check_movie_frame(lua, &this)?;
            stop(lua, h);
            Ok(())
        })?,
    )?;
    // `Lua_GetBoolOrDefault(2, true)` into `+0x2a8`; no returns.
    m.set(
        "EnableSubtitles",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let h = check_movie_frame(lua, &this)?;
            let enabled = bool_or_default(args.front(), true);
            with_state(lua, h, |m| m.subtitles = enabled);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_MOVIEFRAME_METHODS, m)?;
    Ok(())
}

impl super::UiScript {
    /// Seat the app's movie player, which `StartMovie` and `StopMovie` reach.
    pub fn set_movie_host(&mut self, host: Box<dyn MovieHost>) {
        self.model_mut().movie_host = Some(std::rc::Rc::from(host));
    }

    /// The app's report of a frame of `frame`'s movie: the caption pass, or the stop of a movie
    /// that ran out ([`MovieTick`]).
    pub fn movie_tick(&mut self, frame: FrameHandle, t: MovieTick) {
        tick(&self.lua, frame, t);
    }

    /// The MovieFrame with a movie open, if any: a walk over every frame, for a caller that did
    /// not see the open.
    pub fn playing_movie(&self) -> Option<FrameHandle> {
        let model = self.model_ref();
        let found = model
            .arena
            .iter_frames()
            .find(|(_, f)| matches!(&f.kind_state, KindState::Movie(m) if m.playing))
            .map(|(h, _)| h);
        found
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::script::UiScript;

    /// A player that opens any file not named `missing`, with two captions, and logs its calls.
    struct Host {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl MovieHost for Host {
        fn open(&self, _: FrameHandle, file: &str, volume: f32) -> Option<Vec<MovieCaption>> {
            self.log
                .borrow_mut()
                .push(format!("open {file} {volume:.3}"));
            (file != "missing").then(|| {
                vec![
                    MovieCaption {
                        start_ms: 1000,
                        end_ms: 2000,
                        text: "one".into(),
                    },
                    MovieCaption {
                        start_ms: 2000,
                        end_ms: 3000,
                        text: "two".into(),
                    },
                ]
            })
        }
        fn close(&self, _: FrameHandle) {
            self.log.borrow_mut().push("close".into());
        }
    }

    fn vm() -> (UiScript, Rc<RefCell<Vec<String>>>) {
        let mut s = UiScript::new_glue().unwrap();
        let log = Rc::new(RefCell::new(Vec::new()));
        s.set_movie_host(Box::new(Host { log: log.clone() }));
        s.run(
            r#"M = CreateFrame("MovieFrame", "MovieProbe"); Seen = {}
               M:SetScript("OnMovieFinished", function(self) table.insert(Seen, "finished") end)
               M:SetScript("OnMovieShowSubtitle", function(self, text) table.insert(Seen, "show " .. text) end)
               M:SetScript("OnMovieHideSubtitle", function(self) table.insert(Seen, "hide") end)"#,
        )
        .unwrap();
        (s, log)
    }

    fn seen(s: &UiScript) -> String {
        s.eval::<String>("return table.concat(Seen, ', ')").unwrap()
    }

    fn at(ms: u32) -> MovieTick {
        MovieTick {
            elapsed_ms: ms,
            sound_playing: true,
            finished: false,
            advanced: true,
        }
    }

    /// `StartMovie` raises its usage on anything less than a file and a volume, answers nil for a
    /// file that does not open and 1 for one that does; a plain frame has none of the methods.
    #[test]
    fn the_movie_frame_opens_through_the_host() {
        let (s, log) = vm();
        assert!(s
            .eval::<bool>(r##"return select("#", M:StartMovie("missing", 255)) == 1 and M:StartMovie("missing", 255) == nil"##)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return M:StartMovie("Interface\\Cinematics\\X", "150") == 1"#)
            .unwrap());
        assert_eq!(
            log.borrow()[2],
            r"open Interface\Cinematics\X 0.588",
            "{:?}",
            log.borrow()
        );
        assert!(s
            .eval::<bool>(r##"return select("#", M:EnableSubtitles(1)) + select("#", M:EnableSubtitles()) == 0"##)
            .unwrap());
        for bad in [
            "M:StartMovie()",
            r#"M:StartMovie("a")"#,
            r#"M:StartMovie("a", "loud")"#,
            "M:StartMovie({}, 1)",
            "M:StartMovie(true, 1)",
        ] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains(r#"Usage: MovieProbe:StartMovie("filename", volume_0_to_255)"#),
                "{bad}: {err}"
            );
        }
        s.run(r#"P = CreateFrame("Frame", "NotAMovie")"#).unwrap();
        assert!(s.eval::<bool>("return P.StartMovie == nil").unwrap());
        let err = s.run("M.StopMovie(P)").unwrap_err().to_string();
        assert!(
            err.contains("Wrong object type for member function"),
            "{err}"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// With subtitles on, captions show and hide on new frames as their times pass; `StopMovie`
    /// closes the movie, hides the shown caption and runs `OnMovieFinished`, once.
    #[test]
    fn captions_follow_the_clock_and_stop_finishes_once() {
        let (mut s, log) = vm();
        s.run(r#"M:EnableSubtitles(true); M:StartMovie("x", 255)"#)
            .unwrap();
        let h = s.playing_movie().expect("playing");
        s.movie_tick(h, at(500));
        s.movie_tick(h, at(1500));
        s.movie_tick(
            h,
            MovieTick {
                advanced: false,
                ..at(2500)
            },
        );
        assert_eq!(seen(&s), "show one");
        s.movie_tick(h, at(2500));
        assert_eq!(seen(&s), "show one, hide, show two");
        s.run("M:StopMovie(); M:StopMovie()").unwrap();
        assert_eq!(seen(&s), "show one, hide, show two, hide, finished");
        assert_eq!(log.borrow().last().unwrap(), "close");
        assert!(s.playing_movie().is_none());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// Subtitles off: no caption shows while the sound plays, and one does without sound. A movie
    /// that runs out stops as `StopMovie` does.
    #[test]
    fn subtitles_off_show_only_without_sound_and_a_finished_movie_stops() {
        let (mut s, _) = vm();
        s.run(r#"M:StartMovie("x", 255)"#).unwrap();
        let h = s.playing_movie().expect("playing");
        s.movie_tick(h, at(1500));
        assert_eq!(seen(&s), "");
        s.movie_tick(
            h,
            MovieTick {
                sound_playing: false,
                ..at(1600)
            },
        );
        assert_eq!(seen(&s), "show one");
        s.movie_tick(
            h,
            MovieTick {
                finished: true,
                ..at(1700)
            },
        );
        assert_eq!(seen(&s), "show one, hide, finished");
        assert!(s.playing_movie().is_none());
    }
}
