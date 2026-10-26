//! The sound bindings: `PlaySound`, `PlaySoundFile`, `PlayMusic` and `StopMusic` each queue a
//! plain request for the app to drain, since the engine does not touch the app's mixer.
//!
//! `PlaySound` takes a kit name (`0x4586d0` reaches only `PlaySoundByName`, `0x458030`); a number
//! is `lua_tostring`'s decimal name, which no `SoundEntries` row carries, so it plays nothing (the
//! client plays a kit id internally, through `0x458850`). It answers nothing; `PlaySoundFile`
//! answers one value (`0x458780`).
//!
//! The music pair drives its own slot: the reference gives Lua a stream of its own (`[0xb06ccc]`,
//! opened in `0x460450` at `0x4604a7`) beside the zone track's (`[0xb06cc4]`), one of the client's
//! two endlessly looping streams (`0x7a5592`). `StopMusic` (`0x458770`) is `0x460450` with a null
//! name, so a [`MusicRequest`] is the argument, not the verb. Neither answers anything.

use mlua::{Lua, Value};

/// One queued sound for the app's kit player.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRequest {
    /// `PlaySound("name")`: a kit name, a number's decimal spelling included.
    KitName(String),
    /// `PlaySoundFile("path")`: a file path, no kit.
    File(String),
}

/// One queued `PlayMusic` or `StopMusic`, for the app's Lua music slot.
#[derive(Clone, Debug, PartialEq)]
pub enum MusicRequest {
    /// `PlayMusic("path")`: start the caller's looping stream.
    Play(String),
    /// `StopMusic()`: `0x460450` with no name.
    Stop,
}

impl super::UiScript {
    /// Queue a kit play from the app side, for the UI sounds the client fires from C++
    /// (`QUESTCOMPLETED` on the turn-in) that no Lua handler or message row owns.
    pub fn queue_sound_kit(&mut self, name: &str) {
        self.model_mut()
            .sound_queue
            .push(SoundRequest::KitName(name.to_string()));
    }

    /// Drain the queued sounds; the app plays each in 2D, as UI sounds have no world position.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        drain_field!(self, sound_queue)
    }

    /// Drain the music intents in call order: a play and a stop in one frame are a track or
    /// silence by their order.
    pub fn take_music(&mut self) -> Vec<MusicRequest> {
        drain_field!(self, music_queue)
    }
}

/// Register the sound and music globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    install_drivers(lua)?;
    lua.globals().set(
        "PlaySoundFile",
        // `0x458780`: `lua_isstring`, so a number is a file name, else the literal at `0x835f98`.
        // It answers 1 when the play started and nil when `0x7a5450` refused it. Deviation: the
        // app plays the queued file after the call, so the refusals (the bus-3 cap, a muted SFX
        // channel, a missing file) come too late to answer, and a queued file answers 1.
        lua.create_function(|lua, args: mlua::MultiValue| {
            let path = super::binding_abi::string_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: PlaySoundFile(\"soundfile\")",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            model.sound_queue.push(SoundRequest::File(path));
            Ok(1)
        })?,
    )?;
    lua.globals().set(
        "PlaySound",
        // `0x4586d0`: `lua_isstring`, then `lua_tostring`, so a number becomes its decimal name,
        // else the literal at `0x835f60`; no return value on any path.
        lua.create_function(|lua, args: mlua::MultiValue| {
            let name = super::binding_abi::string_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: PlaySound(\"sound\")",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            // During the UI load a by-name play is dropped before any other gate (`0x458046`,
            // `0x45804d`).
            if model.sound_suppression == 0 {
                model.sound_queue.push(SoundRequest::KitName(name));
            }
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "PlayMusic",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `0x458720`: `lua_isstring` (`0x6f3510`), then `lua_tostring` (`0x6f3690`), so a
            // number becomes a file name and anything else raises this literal (`0x835f7c`,
            // `0x458758`); the path goes to the file layer verbatim.
            let path = super::binding_abi::string_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: PlayMusic(\"music\")",
            )?;
            crate::script::model_mut_of(lua)
                .music_queue
                .push(MusicRequest::Play(path));
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "StopMusic",
        // Reads no argument (`0x458770`), so an extra one is ignored.
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua)
                .music_queue
                .push(MusicRequest::Stop);
            Ok(())
        })?,
    )
}

/// The sound table's driver and voice verbs (table `0xb2d928`). benilla plays through the system's
/// default output and lists no drivers of its own, and it has no voice chat: every driver count is
/// 0, every name the empty string, and the voice loopback never runs.
fn install_drivers(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // `Sound_GameSystem_GetNumOutputDrivers` (`0x985c70`) and its siblings: the counts.
    for name in [
        "Sound_GameSystem_GetNumOutputDrivers",
        "Sound_GameSystem_GetNumInputDrivers",
        "Sound_ChatSystem_GetNumOutputDrivers",
        "Sound_ChatSystem_GetNumInputDrivers",
    ] {
        g.set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(0.0))?)?;
    }
    // The names by index (`0x985ca0` and siblings): a number or the usage error, then the name.
    for (name, usage) in [
        (
            "Sound_GameSystem_GetOutputDriverNameByIndex",
            "Usage: Sound_GetOutputDriverNameByIndex(OutputDriverIndex)",
        ),
        (
            "Sound_GameSystem_GetInputDriverNameByIndex",
            "Usage: Sound_GameSystem_GetInputDriverNameByIndex(InputDriverIndex)",
        ),
        (
            "Sound_ChatSystem_GetOutputDriverNameByIndex",
            "Usage: Sound_ChatSystem_GetOutputDriverNameByIndex(OutputDriverIndex)",
        ),
        (
            "Sound_ChatSystem_GetInputDriverNameByIndex",
            "Usage: Sound_ChatSystem_GetInputDriverNameByIndex(InputDriverIndex)",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                if lua.coerce_number(index)?.is_none() {
                    return Err(mlua::Error::runtime(usage));
                }
                lua.create_string("")
            })?,
        )?;
    }
    // `Sound_GameSystem_RestartSoundSystem` (`0x985d30`): the sound system is the host's and
    // follows its CVars as they change, so there is nothing to restart.
    g.set(
        "Sound_GameSystem_RestartSoundSystem",
        lua.create_function(|_, _: mlua::MultiValue| Ok(()))?,
    )?;
    // The voice loopback (`0x985d50`-`0x985ef0`): with no voice chat nothing records or plays.
    for name in [
        "VoiceChat_StopCapture",
        "VoiceChat_StartCapture",
        "VoiceChat_StopRecordingLoopbackSound",
        "VoiceChat_PlayLoopbackSound",
        "VoiceChat_StopPlayingLoopbackSound",
        "VoiceChat_ActivatePrimaryCaptureCallback",
    ] {
        g.set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(()))?)?;
    }
    g.set(
        "VoiceChat_RecordLoopbackSound",
        lua.create_function(|lua, seconds: Value| {
            if lua.coerce_number(seconds)?.is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: VoiceChat_RecordLoopbackSound(MaxRecordTimeInSeconds)",
                ));
            }
            Ok(())
        })?,
    )?;
    for name in [
        "VoiceChat_IsRecordingLoopbackSound",
        "VoiceChat_IsPlayingLoopbackSound",
        "VoiceChat_GetCurrentMicrophoneSignalLevel",
    ] {
        g.set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(0.0))?)?;
    }
    Ok(())
}

/// The in-game voice verbs (`ChatFrame` and `ComSatClient` tables), which the glue does not have.
pub(super) fn install_voice(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // `IsVoiceChatAllowedByServer` (`0x4fccb0`, the server's flag `[0xbcf004]`),
    // `IsVoiceChatEnabled` (`0x4fcbf0`, that flag and a voice session) and
    // `VoiceIsDisabledByClient` (`0x7dc910`): nil, voice off, as on a server that offers none.
    for name in [
        "IsVoiceChatAllowedByServer",
        "IsVoiceChatEnabled",
        "VoiceIsDisabledByClient",
    ] {
        g.set(
            name,
            lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?,
        )?;
    }
    // The device verbs (`0x7dab80`, `0x7dac40`, `0x7dacf0`, `0x7dae40`) return nothing while no
    // voice session `[0xd37d24]` exists.
    for name in [
        "VoiceEnumerateOutputDevices",
        "VoiceEnumerateCaptureDevices",
        "VoiceSelectOutputDevice",
        "VoiceSelectCaptureDevice",
    ] {
        g.set(
            name,
            lua.create_function(|_, _: mlua::MultiValue| Ok(mlua::MultiValue::new()))?,
        )?;
    }
    // `GetNumVoiceSessions` (`0x7db030`) counts the live voice sessions: none.
    g.set(
        "GetNumVoiceSessions",
        lua.create_function(|_, _: mlua::MultiValue| Ok(0))?,
    )?;
    // `GetVoiceSessionInfo(id)` (`0x7db0b0`): one nil, for a non-number and for an id no session
    // holds.
    g.set(
        "GetVoiceSessionInfo",
        lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?,
    )?;
    // `GetNumVoiceSessionMembersBySessionID(id)` (`0x7db370`) and
    // `GetVoiceSessionMemberInfoBySessionID(id, member)` (`0x7deef0`): zero values for an id no
    // session holds.
    for name in [
        "GetNumVoiceSessionMembersBySessionID",
        "GetVoiceSessionMemberInfoBySessionID",
    ] {
        g.set(
            name,
            lua.create_function(|_, _: mlua::MultiValue| Ok(mlua::MultiValue::new()))?,
        )?;
    }
    // `GetActiveVoiceChannel` (`0x4fbd40`) and `GetVoiceCurrentSessionID` (`0x7db1f0`): nil while
    // no channel carries voice and no session is current.
    for name in ["GetActiveVoiceChannel", "GetVoiceCurrentSessionID"] {
        g.set(
            name,
            lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MusicRequest, SoundRequest};
    use crate::script::UiScript;

    /// `PlaySound` names a kit and answers nothing (`0x4586d0`); a number is its decimal name,
    /// never a `SoundEntries` id. `PlaySoundFile` answers one value, 1 for a play (`0x458780`).
    #[test]
    fn playsound_queues_by_name_and_answers_nothing() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("PlaySound(1234)").unwrap(), 0);
        assert_eq!(s.arity(r#"PlaySound("GAMEOBJECT_DOOROPEN")"#).unwrap(), 0);
        // Further arguments are never read.
        s.run(r#"PlaySound(841, "SFX", true)"#).unwrap();
        assert_eq!(
            s.eval::<(i64, bool)>(
                r#"local n = table.getn({PlaySoundFile("Sound\\Doodad\\BellTollHorde.wav")})
                   return n, PlaySoundFile(12) == 1"#
            )
            .unwrap(),
            (1, true)
        );

        assert_eq!(
            s.take_sounds(),
            vec![
                SoundRequest::KitName("1234".into()),
                SoundRequest::KitName("GAMEOBJECT_DOOROPEN".into()),
                SoundRequest::KitName("841".into()),
                SoundRequest::File("Sound\\Doodad\\BellTollHorde.wav".into()),
                SoundRequest::File("12".into()),
            ]
        );
        assert!(s.take_sounds().is_empty());

        for (call, usage) in [
            ("PlaySound()", r#"Usage: PlaySound("sound")"#),
            ("PlaySound({})", r#"Usage: PlaySound("sound")"#),
            ("PlaySound(true)", r#"Usage: PlaySound("sound")"#),
            ("PlaySoundFile()", r#"Usage: PlaySoundFile("soundfile")"#),
            ("PlaySoundFile(nil)", r#"Usage: PlaySoundFile("soundfile")"#),
        ] {
            let e = s.run(call).expect_err(call).to_string();
            assert!(e.contains(usage), "{call}: {e}");
        }
        assert!(s.take_sounds().is_empty(), "a raise queues nothing");
    }

    #[test]
    fn the_music_pair_queues_in_call_order_and_answers_nothing() {
        let mut s = UiScript::new().unwrap();
        // Counted with `table.getn`: Lua 5.0 has no `select`.
        let (played, stopped): (usize, usize) = s
            .eval(
                r#"return table.getn({PlayMusic("Sound\\Music\\x.mp3")}),
                          table.getn({StopMusic()})"#,
            )
            .unwrap();
        assert_eq!((played, stopped), (0, 0));
        s.run(r#"PlayMusic("Sound\\Music\\ZoneMusic\\Elwynn\\DayElwynn01.mp3")"#)
            .unwrap();
        s.run("StopMusic()").unwrap();
        assert_eq!(
            s.take_music(),
            vec![
                MusicRequest::Play("Sound\\Music\\x.mp3".into()),
                MusicRequest::Stop,
                MusicRequest::Play("Sound\\Music\\ZoneMusic\\Elwynn\\DayElwynn01.mp3".into()),
                MusicRequest::Stop,
            ]
        );
        assert!(s.take_music().is_empty());
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn playmusic_takes_a_string_or_a_number_and_stopmusic_takes_anything() {
        let mut s = UiScript::new().unwrap();
        assert!(s.run("PlayMusic()").is_err());
        assert!(s.run("PlayMusic(nil)").is_err());
        assert!(s.run("PlayMusic(true)").is_err());
        assert!(s.run("PlayMusic({})").is_err());
        assert!(s.take_music().is_empty(), "a raise queues nothing");

        // The reference asks the file layer for `"42"` and plays nothing.
        s.run("PlayMusic(42)").unwrap();
        s.run("StopMusic(1, 2)").unwrap();
        assert_eq!(
            s.take_music(),
            vec![MusicRequest::Play("42".into()), MusicRequest::Stop]
        );
    }

    /// With no voice session the stock voice menu lists no rows and no channel carries voice.
    #[test]
    fn no_voice_session_lists_nothing() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumVoiceSessions()").unwrap(), 0);
        for call in ["GetVoiceSessionInfo(1)", "GetVoiceSessionInfo()"] {
            assert_eq!(s.arity(call).unwrap(), 1, "{call}: one nil (`0x7db0b0`)");
            assert!(s
                .eval::<Option<String>>(&format!("return {call}"))
                .unwrap()
                .is_none());
        }
        for call in [
            "GetNumVoiceSessionMembersBySessionID(1)",
            "GetNumVoiceSessionMembersBySessionID(nil)",
            "GetVoiceSessionMemberInfoBySessionID(1, 1)",
        ] {
            assert_eq!(s.arity(call).unwrap(), 0, "{call}: zero values");
        }
        // The stock pull-out roster's read (`ChannelFrame.lua:1020`).
        assert_eq!(
            s.eval::<i64>("return GetNumVoiceSessionMembersBySessionID(nil) or 0")
                .unwrap(),
            0
        );
        for verb in ["GetActiveVoiceChannel", "GetVoiceCurrentSessionID"] {
            assert_eq!(s.arity(&format!("{verb}()")).unwrap(), 1, "{verb}");
            assert!(s
                .eval::<Option<i64>>(&format!("return {verb}()"))
                .unwrap()
                .is_none());
        }
    }
}
