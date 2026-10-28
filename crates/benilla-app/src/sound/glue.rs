//! Glue-screen music: `GlueParent.lua`'s `CurrentGlueMusic`, a SoundEntries kit by name
//! (`GS_LichKing`), which the stock screens' music verbs stream and keep across the glue screens
//! (`SI2::StartGlueMusic 0x9861c0`), starting a new variation each time one ends
//! (`SI2::GlueMusicUpdate 0x986080`). Entering the world does not end it: it plays through the map
//! load and fades over 3.0 s once the load drains, still behind the loading screen; the ambience is
//! cut there.

use bevy::prelude::*;

use crate::char_select::ClientState;
use benilla_assets::{LockRecover, WorldAssets};

use super::kit::{SoundCategory, SoundKits};
use super::{mixer, SoundConfig, SoundOutput};

/// The theme's fade when the world's load drains: `StopGlueMusic(3.0f)` at `0x4db91f`
/// (`[0x9ebbc4]`), linear in amplitude.
const GLUE_MUSIC_FADE_OUT_MS: u64 = 3000;

/// The stock glue screens' music verbs, which drive the theme.
#[derive(Message, Clone, Debug, PartialEq)]
pub(crate) enum GlueMusicRequest {
    /// `PlayGlueMusic(name)` (`0x4dc070` → `0x9861c0`): the same kit still streaming plays on; the
    /// remembered name is kept (its poll restarts it); another name cuts the old kit and starts.
    Play(String),
    /// `StopGlueMusic()` (`0x4dc1a0` → `0x9860e0(−1)`): forgets the name, so nothing restarts it,
    /// and cuts the stream.
    Stop,
    /// `PlayGlueAmbience(name [, fade])` (`0x4dc0c0` → `SI2::StartGlueAmbience 0x985fb0`): the
    /// same kit still playing plays on; else the old one stops over 3.0 s and the kit starts
    /// looping on the ambience slider, faded in over `fade` seconds.
    Ambience { name: String, fade: Option<f32> },
    /// `StopGlueAmbience()` (`0x4dc130`): a 1.0 s fade (`0x985f70(1.0)`).
    StopAmbience,
}

/// `StartGlueAmbience`'s stop of the kit it replaces, `3.0f` (`0x985fb0`).
const GLUE_AMBIENCE_SWITCH_FADE_MS: u64 = 3000;

/// `StopGlueAmbience`'s fade, `1.0f` (`0x4dc130`).
const STOP_GLUE_AMBIENCE_FADE_MS: u64 = 1000;

/// The held glue-music stream and its starvation watch. The handle is kept through the stop-fade
/// so the watch still sees the stream during the world-entry load burst.
struct GlueMusic {
    handle: Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>,
    /// The kit name `PlayGlueMusic` last started (`s_glueMusicName`, `0xdce968`); empty once
    /// stopped.
    track: String,
    /// The streaming kit's id, which a repeated `PlayGlueMusic` compares against.
    kit: Option<u32>,
    /// The streaming variation's SoundEntries volume, under the Music slider.
    volume: f32,
    watch: mixer::StreamWatch,
    /// The stop-fade is running; the per-frame slider feed stands off so it cannot undo the ramp.
    fading: bool,
    /// The glue ambience (`s_glueAmbience`, `0xdcea90`): its looping stream, kit and volume.
    ambience: Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>,
    ambience_kit: Option<u32>,
    ambience_volume: f32,
}

impl Default for GlueMusic {
    fn default() -> Self {
        Self {
            handle: None,
            track: String::new(),
            kit: None,
            volume: 1.0,
            watch: mixer::StreamWatch::new("glue music"),
            fading: false,
            ambience: None,
            ambience_kit: None,
            ambience_volume: 1.0,
        }
    }
}

/// Stream a picked variation of the kit named `name` as the held theme, at its SoundEntries volume
/// on the Music slider (the glue kits play in sound group 1, music).
fn open_glue_music(
    music: &mut GlueMusic,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    kits: Option<&mut SoundKits>,
    config: &SoundConfig,
    name: &str,
) {
    let (Some(assets), Some(kits), Some(mixer_ref)) = (assets, kits, out.mixer.as_mut()) else {
        return;
    };
    let Some(kit) = kits.catalog().by_name(name).map(|k| k.id) else {
        warn_once!("glue music: no SoundEntries kit named {name:?}");
        return;
    };
    let Some((file, volume)) = kits.pick_stream(kit) else {
        return;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(&file)
    };
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            debug!("glue music: {file} — {e:#}");
            return;
        }
    };
    let amp = config.category_amp(SoundCategory::Music) * volume;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(amp))) {
        Ok(h) => {
            info!("glue music: {name} — {file}");
            music.handle = Some(h);
            music.kit = Some(kit);
            music.volume = volume;
            music.fading = false;
            music.watch.reset();
        }
        Err(e) => debug!("glue music: {e:#}"),
    }
}

/// Start the kit named `name` as the glue ambience, looping, faded in over `fade` seconds
/// (`0x985fb0`: loop mode 1, sound group 2).
fn open_glue_ambience(
    music: &mut GlueMusic,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    kits: Option<&mut SoundKits>,
    config: &SoundConfig,
    name: &str,
    fade: Option<f32>,
) {
    let (Some(assets), Some(kits), Some(mixer_ref)) = (assets, kits, out.mixer.as_mut()) else {
        return;
    };
    let Some(kit) = kits.catalog().by_name(name).map(|k| k.id) else {
        warn_once!("glue ambience: no SoundEntries kit named {name:?}");
        return;
    };
    let Some((file, volume)) = kits.pick_stream(kit) else {
        return;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(&file)
    };
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            debug!("glue ambience: {file} — {e:#}");
            return;
        }
    };
    let amp = config.category_amp(SoundCategory::Ambience) * volume;
    let fade_in = fade
        .filter(|s| *s > 0.0)
        .map(|s| mixer::fade((s * 1000.0) as u64));
    let data = data
        .loop_region(..)
        .fade_in_tween(fade_in)
        .volume(mixer::amp_to_db(amp));
    match mixer_ref.play_stream(data) {
        Ok(h) => {
            info!("glue ambience: {name} — {file}");
            music.ambience = Some(h);
            music.ambience_kit = Some(kit);
            music.ambience_volume = volume;
        }
        Err(e) => debug!("glue ambience: {e:#}"),
    }
}

/// A stop passed −1, which keeps the sound's own fade-out (`SESound::Stop 0x879840`): the glue
/// kits play with the default 0 (`0x4c6a40`), so the channel stops at once, here behind the
/// de-click ramp every force-stop takes.
fn glue_cut() -> kira::Tween {
    mixer::declick()
}

/// Stop the glue ambience over `fade`.
fn fade_glue_ambience(music: &mut GlueMusic, fade: kira::Tween) {
    if let Some(mut h) = music.ambience.take() {
        h.stop(fade);
    }
    music.ambience_kit = None;
}

/// Stop the held theme over `fade`; the handle drops once the watch sees it stopped.
fn fade_glue_music(music: &mut GlueMusic, fade: kira::Tween) {
    if let Some(h) = music.handle.as_mut() {
        h.stop(fade);
        music.fading = true;
    }
}

/// What a `PlayGlueMusic(name)` does (`0x9861c0`): nothing while that kit streams, nothing for the
/// remembered name (whatever streams), else switch.
#[derive(Debug, PartialEq, Eq)]
enum PlayAction {
    Nothing,
    /// Stop what streams, remember `name` and start it.
    Switch,
}

fn play_action(name: &str, remembered: &str, same_kit_streaming: bool) -> PlayAction {
    if same_kit_streaming || name.eq_ignore_ascii_case(remembered) {
        PlayAction::Nothing
    } else {
        PlayAction::Switch
    }
}

/// The stock glue screens' music verbs.
fn drive_glue_music_requests(
    mut requests: MessageReader<GlueMusicRequest>,
    mut music: NonSendMut<GlueMusic>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    mut kits: Option<ResMut<SoundKits>>,
    config: Res<SoundConfig>,
) {
    for request in requests.read() {
        match request {
            GlueMusicRequest::Play(name) => {
                let asked = kits
                    .as_deref()
                    .and_then(|k| k.catalog().by_name(name))
                    .map(|k| k.id);
                let streaming = music.handle.is_some() && !music.fading;
                let same = streaming && asked.is_some() && asked == music.kit;
                if play_action(name, &music.track, same) == PlayAction::Switch {
                    fade_glue_music(&mut music, glue_cut());
                    music.track = name.clone();
                    open_glue_music(
                        &mut music,
                        &mut out,
                        assets.as_deref(),
                        kits.as_deref_mut(),
                        &config,
                        name,
                    );
                }
            }
            GlueMusicRequest::Stop => {
                music.track.clear();
                fade_glue_music(&mut music, glue_cut());
            }
            GlueMusicRequest::Ambience { name, fade } => {
                let asked = kits
                    .as_deref()
                    .and_then(|k| k.catalog().by_name(name))
                    .map(|k| k.id);
                if asked.is_some() && asked == music.ambience_kit && music.ambience.is_some() {
                    continue;
                }
                fade_glue_ambience(&mut music, mixer::fade(GLUE_AMBIENCE_SWITCH_FADE_MS));
                open_glue_ambience(
                    &mut music,
                    &mut out,
                    assets.as_deref(),
                    kits.as_deref_mut(),
                    &config,
                    name,
                    *fade,
                );
            }
            GlueMusicRequest::StopAmbience => {
                fade_glue_ambience(&mut music, mixer::fade(STOP_GLUE_AMBIENCE_FADE_MS));
            }
        }
    }
}

/// Arm the theme's 3.0 s fade and cut the ambience when the world's load drains.
///
/// The reference's trigger: `CGlueMgr::Idle` (`0x4dab40`) in its enter-world state waits for the
/// loading screen, and on the pass after the glue suspends it calls `StopGlueMusic(3.0f)` and
/// `StopGlueAmbience(−1.0f)` (`0x4db91f`, `0x4db92d`), then sends `CMSG_PLAYER_LOGIN`. benilla's
/// analogue is `world_hold`'s falling edge ([`crate::loading_screen`]), acted on only in the world
/// so the logout blackout's drop does not fade the theme the glue just restarted.
fn hand_off_glue_music(
    config: Res<SoundConfig>,
    state: Res<State<ClientState>>,
    mut music: NonSendMut<GlueMusic>,
    mut was_covered: Local<bool>,
) {
    let covered = config.world_hold;
    let fell = std::mem::replace(&mut *was_covered, covered) && !covered;
    if !fell || *state.get() != ClientState::InWorld {
        return;
    }
    if let Some(h) = music.handle.as_mut() {
        info!("glue music: the world's load drained — {GLUE_MUSIC_FADE_OUT_MS} ms fade");
        h.stop(mixer::fade(GLUE_MUSIC_FADE_OUT_MS));
        music.fading = true;
    }
    fade_glue_ambience(&mut music, glue_cut());
}

/// The starvation watch over the held theme, and the handle's drop once it reaches `Stopped`.
/// On the glue screens the remembered kit then starts again (`0x986080`); in the world it runs out
/// and stays out.
fn watch_glue_music(
    mut music: NonSendMut<GlueMusic>,
    // Wall time: the watch compares against the stream's own position, and the paced virtual
    // clock is snapped and capped by `frame_pace`.
    time: Res<Time<bevy::time::Real>>,
    config: Res<SoundConfig>,
    state: Option<Res<State<ClientState>>>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    mut kits: Option<ResMut<SoundKits>>,
) {
    let music = &mut *music;
    let on_glue = state.is_none_or(|s| *s.get() != ClientState::InWorld);
    if music.handle.is_none() && on_glue && !music.track.is_empty() {
        let track = music.track.clone();
        open_glue_music(
            music,
            &mut out,
            assets.as_deref(),
            kits.as_deref_mut(),
            &config,
            &track,
        );
    }
    if let Some(h) = music.ambience.as_mut() {
        h.set_volume(
            mixer::amp_to_db(config.category_amp(SoundCategory::Ambience) * music.ambience_volume),
            mixer::glide(),
        );
    }
    // The Music slider rescales the playing theme in place (the `MusicVolume` re-apply walker
    // `0x7a6660(ecx=2)` matches the glue stream), except while the stop-fade runs.
    if !music.fading {
        if let Some(h) = music.handle.as_mut() {
            h.set_volume(
                mixer::amp_to_db(config.category_amp(SoundCategory::Music) * music.volume),
                mixer::glide(),
            );
        }
    }
    let Some(h) = music.handle.as_ref() else {
        return;
    };
    if h.state() == kira::sound::PlaybackState::Stopped {
        // Logged: the theme outlives world entry, and nothing else reports when it stops.
        info!("glue music: stream ended");
        music.handle = None;
        music.fading = false;
        music.watch.reset();
        return;
    }
    music.watch.feed(h, f64::from(time.delta_secs()));
}

/// A sound-system restart stops the theme and the ambience (`SE2::StopAllSounds 0x87ded0`); on the
/// glue screens [`watch_glue_music`] starts the remembered theme again.
fn stop_on_restart(
    mut restarts: MessageReader<super::engine::SoundRestart>,
    mut music: NonSendMut<GlueMusic>,
) {
    if restarts.read().count() == 0 {
        return;
    }
    fade_glue_music(&mut music, glue_cut());
    fade_glue_ambience(&mut music, glue_cut());
}

/// Report the glue theme's voice into the global budget; it plays into `InWorld` too.
fn report_stream_voices(music: NonSend<GlueMusic>, mut out: NonSendMut<super::SoundOutput>) {
    let live = |h: &Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>| {
        usize::from(
            h.as_ref()
                .is_some_and(|h| h.state() != kira::sound::PlaybackState::Stopped),
        )
    };
    out.glue_streams = live(&music.handle) + live(&music.ambience);
}

pub(super) fn plugin(app: &mut App) {
    app.add_message::<GlueMusicRequest>()
        .insert_non_send_resource(GlueMusic::default())
        .add_systems(
            Update,
            (
                // The stock screens' `PlayGlueMusic`/`StopGlueMusic`.
                drive_glue_music_requests
                    .after(crate::ui_script::glue_mgr::drive_glue_mgr)
                    .before(watch_glue_music),
                // No run condition: the theme plays into `InWorld`, and the entry load burst is
                // the window the watch exists for.
                hand_off_glue_music,
                stop_on_restart
                    .after(super::engine::RestartSet)
                    .before(watch_glue_music),
                watch_glue_music.after(hand_off_glue_music),
                report_stream_voices,
            ),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x9861c0`'s legs: the asked kit still streaming plays on, the remembered name is kept even
    /// when nothing streams (the poll restarts it), any other name switches.
    #[test]
    fn play_glue_music_keeps_the_kit_or_the_name_else_switches() {
        assert_eq!(play_action("GS_LichKing", "", true), PlayAction::Nothing);
        assert_eq!(
            play_action("GS_LichKing", "gs_lichking", false),
            PlayAction::Nothing
        );
        assert_eq!(play_action("GS_LichKing", "", false), PlayAction::Switch);
        assert_eq!(
            play_action("GS_BurningCrusade", "GS_LichKing", false),
            PlayAction::Switch
        );
    }

    /// `GlueParent.lua`'s `CurrentGlueMusic` and the login's `GlueAmbienceTracks["DARKPORTAL"]`
    /// name SoundEntries kits, not files: every variation reads off the chain and opens as a stream.
    #[test]
    fn the_stock_glue_music_and_ambience_are_kits_whose_files_stream() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_sound_kit_catalog(&mut chain).expect("SoundEntries");
        for name in ["GS_LichKing", "GlueScreenIntro"] {
            let kit = catalog.by_name(name).expect("the glue kit");
            assert!(!kit.files.is_empty(), "{name} has variations");
            for (file, _) in &kit.files {
                let bytes = chain.read(file).expect("the variation reads");
                assert!(
                    mixer::stream_from_bytes(bytes).is_ok(),
                    "{file} opens as a stream"
                );
            }
        }
    }
}
