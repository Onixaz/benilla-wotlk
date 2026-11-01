//! Sound: the 1.12 client's own selection and scheduling ([`kit`], the triggers) over kira behind
//! an FMOD-shaped seam ([`mixer`]). This module holds the output, the listener and the config.

use bevy::prelude::*;

use crate::net::Embodied;
use crate::player::{head_height, CameraPivot, Player};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

mod anim_events;
mod cinematic;
mod combat;
mod creature;
mod death_thud;
mod dsp;
mod emitter_pool;
mod emote;
pub(crate) mod engine;
pub(crate) mod footsteps;
mod gameobject;
mod glue;
mod greeting;
mod hrtf;
pub(crate) mod interior;
mod kit;
mod limiter;
mod liquid_loop;
mod math;
mod message;
mod meter;
mod missile;
mod mix_tap;
mod mixer;
mod money;
mod mount;
mod net;
// Crate-visible for the dev stall watchdog, which asks `output::device_open`.
pub(crate) mod output;
mod probe;
mod reverb;
mod sheathe;
mod spell;
mod ui;
mod vocal;
mod water;
mod weather;
mod zone;
pub(crate) use emote::EmoteSounds;
/// First-play kit decodes so far, for the probe's tail line.
pub(crate) fn kit_decodes() -> u32 {
    kit::DECODES.load(std::sync::atomic::Ordering::Relaxed)
}
pub(crate) use glue::GlueMusicRequest;
pub(crate) use greeting::NpcGreetingRequest;
/// Named by the schedule tests' class table (`game_plugins::schedule_tests::Classes`).
#[cfg(test)]
pub(crate) use kit::SoundKits;
pub(crate) use message::MessageSounds;
pub(crate) use mixer::Mixer;
pub(crate) use ui::{AutoEquipSound, LootPickupSound};
pub(crate) use zone::AreaSounds;
pub(crate) use zone::ExplorationSounds;

/// The sound CVars. Defaults are the reference's registrations: `MusicVolume` 0.4 and
/// `AmbienceVolume` 0.6, every other volume 1.0.
#[derive(Resource)]
pub(crate) struct SoundConfig {
    /// `Sound_EnableAllSound` (default "1", `0x4d1710`). The reference pauses the engine; benilla
    /// zeroes every category in [`Self::category_amp`] and channels run on silently, which sounds
    /// the same.
    pub enabled: bool,
    /// The dev chord + `M` mute: zeroes the main track only, so selection and channels run on and
    /// unmute is instant. Starts `false`; unattended runs open no device instead ([`SoundPlugin`]).
    pub muted: bool,
    /// Master volume, linear `[0,1]`, applied on the main track.
    pub master: f32,
    /// The category sliders `[0,1]`, the pump's category factor.
    pub sfx: f32,
    pub music: f32,
    pub ambience: f32,
    /// `Sound_EnableSFX` (default "1", `0x4d1681`): the SFX category's own switch.
    pub sfx_enabled: bool,
    /// `Sound_EnableMusic`/`Sound_EnableAmbience` (default "1", `0x4d16ed`/`0x4d16a7`). The
    /// reference's music callback stops and re-picks the stream; benilla keeps it alive at zero
    /// and resumes mid-track.
    pub music_enabled: bool,
    pub ambience_enabled: bool,
    /// `Sound_EnableErrorSpeech` (default "1", `0x4d16c7`): gates only your own character's refusal
    /// lines ([`vocal`]), read before any escalation state moves.
    pub error_speech: bool,
    /// `Sound_EnableSoundWhenGameIsInBG` (default "0", `0x4d18c1`), sound while the window is
    /// inactive. `false` is the reference: on `WM_ACTIVATE` alone it calls
    /// `FSOUND_SetMute(-3, !active)`, muting every channel, music included. It is a mute, not a
    /// pause: playback cursors keep moving.
    ///
    /// Deviation: the reference also refuses to open a new non-music stream while inactive; benilla
    /// starts everything and gates the output, because a refusal at kit start would make an
    /// unattended capture record silence.
    pub background_sound: bool,
    /// `Sound_EnableReverb` (default "0", `0x4d125b`): gates both the zone preset and the
    /// per-channel wet send. Read by [`reverb::zone_reverb`].
    pub reverb: bool,
    /// Not a CVar: set while the loading cover is up ([`feed_world_hold`]). No new sound starts
    /// and the zone beds stay down, since the reference blocks on its world load; sounds already
    /// playing, such as the glue theme's fade tail, play out as the reference's do.
    pub world_hold: bool,
    /// Not a CVar: set while a cinematic plays ([`feed_music_suppression`]), the reference's music
    /// suppression flag. The cinematic start reaches the same setter as `EnableMusic 0`, which
    /// stop-and-destroys the track, a cut, never a fade, and the music pump idles while it is set.
    /// Ambience is untouched: the cinematic never writes its flag.
    ///
    /// Kept apart from [`Self::music_enabled`]: the CVar persists to `Config.wtf`, so asserting it
    /// would save music off for a player who quits mid-cinematic. So the reference's restore latch,
    /// needed there because its flag is the CVar itself, is not carried.
    pub music_suppressed: bool,
    /// Deviation: `SoundOutputLimiter`, not a 1.12 CVar, default on, so overlapping full-scale
    /// kits are limited rather than clipped. The reference has no headroom mechanism and clips at
    /// full scale, as kira's hard clamp would (see [`limiter`]).
    pub limiter: bool,
    /// `Sound_ListenerAtCharacter` (default "1", `0x4d17c5`), read once per frame. `1` puts the
    /// listener on the mover with the character's facing, `0` at the camera eye with its basis;
    /// position and orientation always move together, with no velocity. A cinematic overrides it
    /// (checked first).
    pub listener_at_character: bool,
    /// `Sound_EnableEmoteSounds` (default "1", `0x4d17e5`): gates only the received text-emote
    /// voice line ([`emote::emote_sounds`]); a zero fetches no kit at all. The reference's check
    /// sits on the received text-emote leg or on the shared leg, which `$CSD` also enters; which
    /// one is untraced, and benilla gates the received path alone.
    pub emote_sounds: bool,
    /// `Sound_EnablePetSounds` (default "1"): off, a unit with a pet number makes no aggro, order or
    /// attack bark and no emote voice (`0x7474b0`, `0x746d60`).
    pub pet_sounds: bool,
    /// `Sound_ZoneMusicNoDelay` (default "0", `0x4d1808`), the panel's "Loop Music": drops the
    /// `ZoneMusic.dbc` silence interval between plays of one zone's track
    /// ([`zone::next_track_time`]). A zone change is immediate either way.
    pub zone_music_no_delay: bool,
}

impl SoundConfig {
    /// The category slider a channel multiplies in (the reference's category pick).
    pub(crate) fn category_amp(&self, cat: kit::SoundCategory) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        match cat {
            kit::SoundCategory::Sfx if self.sfx_enabled => self.sfx,
            kit::SoundCategory::Music if self.music_enabled => self.music,
            kit::SoundCategory::Ambience if self.ambience_enabled => self.ambience,
            _ => 0.0,
        }
    }
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            muted: false,
            master: 1.0,
            sfx: 1.0,
            sfx_enabled: true,
            music: 0.4,
            ambience: 0.6,
            music_enabled: true,
            ambience_enabled: true,
            error_speech: true,
            background_sound: false,
            reverb: false,
            world_hold: false,
            music_suppressed: false,
            limiter: true,
            // The reference's registered defaults: "1", "1", "0".
            listener_at_character: true,
            emote_sounds: true,
            pet_sounds: true,
            zone_music_no_delay: false,
        }
    }
}

/// Copy the loading cover into [`SoundConfig::world_hold`] in `PreUpdate`, before every trigger.
/// The one-frame lag behind the raise is what keeps the enter-world click audible.
fn feed_world_hold(
    mut config: ResMut<SoundConfig>,
    screen: Res<crate::loading_screen::LoadingScreen>,
) {
    let hold = screen.covering();
    if config.world_hold != hold {
        config.world_hold = hold;
    }
}

/// Shut the output while the window is unfocused, unless [`SoundConfig::background_sound`].
/// A missing window opens the gate. The `Local` makes it fire on the edge only: a tween re-issued
/// every frame would restart the 16 ms ramp forever.
fn apply_focus_gate(
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut last: Local<Option<bool>>,
) {
    let open = config.background_sound || windows.single().map_or(true, |w| w.focused);
    if *last == Some(open) {
        return;
    }
    *last = Some(open);
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_output_gate(open);
    }
}

/// Copy the cinematic's music stop into [`SoundConfig::music_suppressed`].
///
/// Runs in `WorldStage::Stream`, not `PreUpdate`: the cinematic starts in `Input`, and
/// [`zone::zone_audio`] in `Present` must see the flag that same frame, or it starts a track (and
/// consumes the zone's intro fanfare) on the cinematic's first frame.
fn feed_music_suppression(
    mut config: ResMut<SoundConfig>,
    cinematic: Option<Res<crate::cinematic::Cinematic>>,
) {
    let playing = cinematic
        .as_deref()
        .is_some_and(crate::cinematic::Cinematic::is_playing);
    if config.music_suppressed != playing {
        config.music_suppressed = playing;
    }
}

/// The backend output; `mixer` is `None` with no device or under `$WOW_NOSOUND`. Non-Send: the
/// device stream is not `Send` on every platform, so audio systems run on the main thread.
pub(crate) struct SoundOutput {
    pub(crate) mixer: Option<Mixer>,
    /// Live kit channels, pumped by [`kit::pump_channels`].
    pub(crate) channels: Vec<kit::ActiveChannel>,
    /// The `$WOW_SOUND_PROBE` recorder, here so the kit player can stamp every play.
    pub(crate) probe: Option<probe::Probe>,
    /// Live stream voices, rewritten each frame by their owners ([`zone`], [`glue`],
    /// [`cinematic`]). They count against the voice ceiling like any sound: the
    /// reference's music and ambience occupy FMOD channels too. One field per owner, since a
    /// shared counter with several writers drifts.
    pub(crate) zone_streams: usize,
    pub(crate) glue_streams: usize,
    pub(crate) cinematic_streams: usize,
    /// One-shots that lost their slot to a louder newcomer, and plays refused because nothing
    /// live was quieter.
    pub(crate) voices_stolen: u64,
    pub(crate) voices_denied: u64,
    /// Same-kit copies dropped by [`kit::SAME_KIT_MAX`].
    pub(crate) copies_dropped: u64,
    /// What the engine was initialised with ([`engine`]): the voice ceiling and the software
    /// HRTF apply from here.
    pub(crate) engine: engine::EngineSettings,
    /// The `SoundFilter.dbc` voice chains ([`dsp`]), used while [`engine::EngineSettings::dsp`].
    pub(crate) voice_filters: dsp::VoiceFilters,
}

impl SoundOutput {
    /// Everything the device is mixing, the number [`engine::EngineSettings::voices`] bounds.
    pub(crate) fn live_voices(&self) -> usize {
        self.channels.len() + self.zone_streams + self.glue_streams + self.cinematic_streams
    }
}

/// Build the Death Knight voice chains from `SoundFilter.dbc` and `ChrRaces`; a missing table
/// leaves them empty and every voice dry.
fn load_voice_filters(
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
    races: Option<Res<crate::entities::CharCreate>>,
) {
    use benilla_assets::LockRecover;
    let (Some(assets), Some(races)) = (assets, races) else {
        return;
    };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_sound_filter_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            out.voice_filters = dsp::VoiceFilters::build(&catalog, |race| {
                races.0.race_file(race).map(str::to_string)
            });
            info!("sound: {} voice filter chains", catalog.len());
        }
        Err(e) => warn!("sound: voice filters failed to load: {e:#}"),
    }
}

/// `Material.dbc`, shared by the armor foley ([`footsteps`]) and the weapon impacts ([`combat`]).
#[derive(Resource)]
pub(crate) struct Materials(pub(crate) benilla_formats::MaterialCatalog);

fn load_materials(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_material_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} material rows", cat.len());
            commands.insert_resource(Materials(cat));
        }
        Err(e) => warn!("sound: materials failed to load: {e:#}"),
    }
}

/// Your worn chest item's `Material` id, read as the reference reads it, element 4 of
/// `[player+0x1d38]`, for the armor foley and a player victim's impact. Self-only in the reference
/// and here, since the inventory fields reach only their owner; `store` must be the player's own.
pub(super) fn worn_chest_material(
    store: Option<&crate::net::ObjectStore>,
    objects: &crate::net::Objects,
    items: &crate::items::Items,
    net: &crate::net::NetCommands,
) -> Option<u32> {
    /// The fifth 8-byte guid of the inv-slot array.
    const EQUIPMENT_SLOT_CHEST: u8 = 4;
    let guid = store?.0.player_inv_slot(EQUIPMENT_SLOT_CHEST)?;
    let entry = objects.object(guid)?.object_entry()?;
    Some(items.held(entry, net)?.material)
}

/// This frame's listener pose in Bevy space, set by [`update_audio_listener`] and read by every
/// sound system.
#[derive(Resource)]
pub(crate) struct AudioListener {
    pub(crate) pos: Vec3,
    pub(crate) rot: Quat,
}

impl Default for AudioListener {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
        }
    }
}

/// The world soundscape is live: in the world and seated on the avatar. The seat check matters
/// after a logout, when the camera still sits at the old spot until the next take-control.
fn world_audio_live(
    state: Res<State<crate::char_select::ClientState>>,
    player: Res<Player>,
) -> bool {
    *state.get() == crate::char_select::ClientState::InWorld && player.active
}

pub(crate) struct SoundPlugin;

/// The sound CVars' change callback: volumes clamp to `[0, 1]`, enables are `!= 0` as the reference
/// parses them (`setne al`).
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut sound: ResMut<SoundConfig>) {
    let v = ev.num();
    match ev.key().as_str() {
        "sound_mastervolume" => sound.master = v.clamp(0.0, 1.0),
        "sound_sfxvolume" => sound.sfx = v.clamp(0.0, 1.0),
        "sound_musicvolume" => sound.music = v.clamp(0.0, 1.0),
        "sound_ambiencevolume" => sound.ambience = v.clamp(0.0, 1.0),
        "sound_enableallsound" => sound.enabled = v != 0.0,
        "sound_enablesfx" => sound.sfx_enabled = v != 0.0,
        "sound_enablemusic" => sound.music_enabled = v != 0.0,
        "sound_enableambience" => sound.ambience_enabled = v != 0.0,
        "sound_enableerrorspeech" => sound.error_speech = v != 0.0,
        "sound_enablesoundwhengameisinbg" => sound.background_sound = v != 0.0,
        "sound_enablereverb" => sound.reverb = v != 0.0,
        "soundoutputlimiter" => sound.limiter = v != 0.0,
        "sound_listeneratcharacter" => sound.listener_at_character = v != 0.0,
        "sound_enableemotesounds" => sound.emote_sounds = v != 0.0,
        "sound_enablepetsounds" => sound.pet_sounds = v != 0.0,
        "sound_zonemusicnodelay" => sound.zone_music_no_delay = v != 0.0,
        _ => {}
    }
}

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.add_observer(on_cvar);
        // `$WOW_NOSOUND` (a silent unattended run) and `$WOW_CAPTURE` open no device at all, so
        // they never hold the audio hardware against a session someone is listening to.
        let silent = ["WOW_NOSOUND", "WOW_CAPTURE"]
            .into_iter()
            .find(|v| std::env::var_os(v).is_some());
        // Before the mixer: the probe's taps are main-track effects, fixed when kira builds it.
        let probe_dir = if silent.is_some() {
            None
        } else {
            probe::output_dir()
        };
        let (drivers, settings) = engine::boot_settings();
        let mixer = if let Some(var) = silent {
            info!("${var} set — audio disabled");
            None
        } else {
            match Mixer::new(probe_dir.as_deref(), drivers.device(settings.driver)) {
                Ok(m) => Some(m),
                Err(e) => {
                    warn!("no audio device — running silent: {e:#}");
                    None
                }
            }
        };
        let probe = probe_dir.zip(mixer.as_ref()).and_then(|(dir, m)| {
            let Some(rate) = m.sample_rate() else {
                warn!("sound probe: device sample rate unknown — not recording");
                return None;
            };
            Some(probe::Probe::start(dir, rate, m.audio_pos()))
        });
        app.insert_non_send_resource(SoundOutput {
            mixer,
            channels: Vec::new(),
            probe,
            zone_streams: 0,
            glue_streams: 0,
            cinematic_streams: 0,
            voices_stolen: 0,
            voices_denied: 0,
            copies_dropped: 0,
            engine: settings,
            voice_filters: dsp::VoiceFilters::default(),
        })
        .insert_resource(drivers)
        .init_resource::<engine::DeviceChanged>()
        .add_message::<engine::SoundRestart>()
        .add_observer(engine::on_driver_index)
        .init_resource::<SoundConfig>()
        .init_resource::<AudioListener>()
        .add_systems(
            Startup,
            (
                load_materials.after(benilla_assets::AssetSet::Open),
                engine::write_back_driver.after(crate::cvars::CvarLoad),
                // `ChrRaces` arrives with the entity tables.
                load_voice_filters.after(crate::entities::setup_entities),
            ),
        )
        .add_systems(
            Update,
            (
                ui::drain_restart_request,
                engine::restart_on_lost_device,
                engine::restart_sound_system,
                ui::feed_drivers,
            )
                .chain()
                // After the registry drains the panel's `SetCVar`, which the same click makes.
                .after(crate::ui_script::UiInput)
                .after(crate::cvars::sync_cvars)
                .in_set(engine::RestartSet),
        )
        .add_systems(PreUpdate, feed_world_hold)
        .add_systems(
            Update,
            feed_music_suppression.in_set(benilla_world::schedule::WorldStage::Stream),
        )
        .add_systems(
            Update,
            (
                // After Input writes the pose and camera, before Present's consumers read it.
                update_audio_listener.in_set(WorldStage::Stream),
                toggle_mute,
                apply_master_volume.after(toggle_mute),
                apply_focus_gate,
                poll_mix_health,
            ),
        );
        probe::plugin(app);
        kit::plugin(app);
        liquid_loop::plugin(app);
        zone::plugin(app);
        cinematic::plugin(app);
        gameobject::plugin(app);
        anim_events::plugin(app);
        emitter_pool::plugin(app);
        spell::plugin(app);
        missile::plugin(app);
        creature::plugin(app);
        combat::plugin(app);
        footsteps::plugin(app);
        death_thud::plugin(app);
        mount::plugin(app);
        water::plugin(app);
        weather::plugin(app);
        emote::plugin(app);
        greeting::plugin(app);
        ui::plugin(app);
        vocal::plugin(app);
        message::plugin(app);
        glue::plugin(app);
        money::plugin(app);
        reverb::plugin(app);
        interior::plugin(app);
        sheathe::plugin(app);
    }
}

/// Compute this frame's [`AudioListener`] and feed it to the backend: at the character's head
/// with its facing ([`SoundConfig::listener_at_character`]), else at the camera, which is also
/// the fallback when there is no seated body.
fn update_audio_listener(
    mut listener: ResMut<AudioListener>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    player: Res<Player>,
    cinematic: Option<Res<crate::cinematic::Cinematic>>,
    self_av: Query<(&Transform, Option<&CameraPivot>), With<Embodied>>,
    cam: Query<&Transform, (With<WorldCamera>, Without<Embodied>)>,
) {
    // A cinematic puts the listener on the camera ahead of the CVar (`camera+0x50`, armed by the
    // cinematic start).
    let flying = cinematic
        .as_deref()
        .is_some_and(crate::cinematic::Cinematic::is_playing);
    // `player.pos` is the feet; the facing is the yaw about world +Y.
    if config.listener_at_character && player.active && !player.detached && !flying {
        if let Ok((t, pivot)) = self_av.single() {
            listener.pos = player.pos + Vec3::Y * head_height(pivot, t.scale.x);
            listener.rot = Quat::from_rotation_y(player.facing());
            if let Some(mixer) = out.mixer.as_mut() {
                mixer.set_listener(listener.pos, listener.rot);
            }
            return;
        }
    }
    // Deviation: with no character the reference leaves the listener where it was; benilla seats
    // it on the camera, where the view is.
    if let Ok(t) = cam.single() {
        listener.pos = t.translation;
        listener.rot = t.rotation;
        if let Some(mixer) = out.mixer.as_mut() {
            mixer.set_listener(listener.pos, listener.rot);
        }
    }
}

/// The dev chord + `M` flips [`SoundConfig::muted`]; the chord never collides with a game
/// binding.
fn toggle_mute(keys: Res<ButtonInput<KeyCode>>, mut config: ResMut<SoundConfig>) {
    if crate::run_mode::dev_chord(&keys, KeyCode::KeyM) {
        config.muted = !config.muted;
        info!("sound {}", if config.muted { "muted" } else { "unmuted" });
    }
}

/// Apply the master enable, volume and limiter on change; a `Local`, since the panel dirties
/// the resource every open frame.
fn apply_master_volume(
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    mut last: Local<Option<(bool, f32, bool)>>,
) {
    let cur = (
        config.enabled && !config.muted,
        config.master,
        config.limiter,
    );
    if *last == Some(cur) {
        return;
    }
    *last = Some(cur);
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_master(if cur.0 { cur.1 } else { 0.0 });
        mixer.set_limiter(cur.2);
    }
}

/// The window each mix-health report summarises.
const MIX_HEALTH_REPORT: std::time::Duration = std::time::Duration::from_secs(5);

/// Drain the backend's mix-health queues every frame (they are bounded rings) and report
/// deadline misses, voice refusals and the output level. A quiet report during a crackle points
/// upstream: [`mixer::StreamWatch`] meters stream decoders, and `$WOW_MIX_TAP` records the mix.
fn poll_mix_health(
    mut out: NonSendMut<SoundOutput>,
    time: Res<Time>,
    mut exit: MessageReader<bevy::app::AppExit>,
    mut since_report: Local<std::time::Duration>,
    mut last_refused: Local<u64>,
    mut peak_voices: Local<usize>,
) {
    // A recording probe owns the meters: [`meter::MixLevel::take`] resets on read.
    if out.probe.is_some() {
        return;
    }
    *peak_voices = (*peak_voices).max(out.channels.len());
    let Some(mixer) = out.mixer.as_mut() else {
        return;
    };
    let health = mixer.poll_health();
    *since_report += time.delta();
    // App exit forces a report, so the misses since the last boundary are not lost.
    let exiting = exit.read().next().is_some();
    if *since_report < MIX_HEALTH_REPORT && !exiting {
        return;
    }
    *since_report = std::time::Duration::ZERO;
    let peak = mixer.take_health_peak();
    let level = mixer.take_level();
    let rate = mixer.sample_rate();
    let window = mixer.take_output_window();
    let voices = std::mem::take(&mut *peak_voices);
    report_output(window, peak);
    let new_refused = health.voices_refused - *last_refused;
    *last_refused = health.voices_refused;
    if new_refused > 0 {
        warn!(
            "audio: {new_refused} 3D sound(s) never played — the spatial-voice arena was full. \
             These are sounds the player should have heard; the ceiling is ours to raise \
             (`SPATIAL_VOICE_CAPACITY`), not the game's to work around.",
        );
    }
    report_level(level, voices, rate);
}

/// One report window's output side: every layer between the mix and the speaker, each with its
/// own number, so a crackle names its layer.
fn report_output(w: mixer::OutputWindow, peak_load: f32) {
    if w.cycles == 0 {
        // No stream ran this window; `Mixer::poll_health` already said why, per event.
        return;
    }
    let lead = w
        .lead_min_ms
        .map_or_else(|| "n/a".to_string(), |v| format!("{v:.1} ms"));
    let ring = w
        .ring_min_ms
        .map_or_else(|| "n/a".to_string(), |v| format!("{v:.0} ms"));
    let shape = format!(
        "{} cycles: lead ≥ {lead}, IO copy ≤ {:.2} ms, cycle gap ≤ {:.1} ms (nominal {:.1}), \
         render ≤ {:.2} ms per {:.1} ms chunk (peak load {:.0}%), ring ≥ {ring}",
        w.cycles,
        w.io_wall_max_ms,
        w.gap_max_ms,
        w.cycle_ms,
        w.render_wall_max_ms,
        w.chunk_ms,
        peak_load * 100.0,
    );
    if w.underruns > 0 {
        warn!(
            "audio: {} output underrun(s) — ~{:.0} ms of silence reached the device: the render \
             thread fell the whole mix-ahead behind. This is what a crackle sounds like. {shape}",
            w.underruns, w.underrun_ms,
        );
    }
    if w.overloads > 0 {
        let ago = w
            .last_overload_ago_ms
            .map_or_else(String::new, |v| format!(", last {v:.0} ms ago"));
        warn!(
            "audio: {} HAL processor overload(s){ago} — the OS says our IO cycle ran past its \
             deadline. The copy's own wall time and the lead say which: a long copy is the IO \
             thread parked inside the callback (page-in, preemption), a short lead is it woken \
             late. {shape}",
            w.overloads,
        );
    } else if w.lead_min_ms.is_some_and(|l| l < 0.0)
        || w.io_wall_max_ms > w.cycle_ms * 0.5
        || w.gap_max_ms > w.cycle_ms * 1.5
    {
        warn!("audio: IO cycle strain without an overload — {shape}");
    } else {
        debug!("audio: output steady — {shape}");
    }
}

/// One report window's level side: the peak asked for, how long it was over full scale, what the
/// limiter pulled and how many voices were live.
fn report_level(level: meter::LevelReading, voices: usize, rate: Option<u32>) {
    // First: a NaN passes the limiter's `peak > CEILING` test straight into the driver.
    if level.nonfinite > 0 {
        error!(
            "audio: {} non-finite (NaN/inf) sample(s) reached the mix. This is a defect upstream \
             of the output — the limiter cannot catch it, and it is broadband noise at whatever \
             the hardware makes of the bits.",
            level.nonfinite,
        );
    }
    if level.over == 0 {
        debug!(
            "audio: mix peak {:.2} of full scale, {voices} voice(s) at most",
            level.peak,
        );
        return;
    }
    // Two samples per frame; with no known rate, a count rather than a guessed duration.
    let over = match rate {
        Some(r) => format!(
            "for ~{:.0} ms",
            level.over as f64 / 2.0 / f64::from(r) * 1000.0
        ),
        None => format!("across {} samples", level.over),
    };
    warn!(
        "audio: the mix asked for {:.2}x full scale ({:+.1} dBFS) {over} of the last {}s, with \
         {voices} voice(s) live at most; the limiter pulled up to {:.1} dB to hold it under. \
         Without it that is hard clipping — the \"dirty, like a speaker breaking\" report.",
        level.peak,
        20.0 * level.peak.max(1e-6).log10(),
        MIX_HEALTH_REPORT.as_secs(),
        20.0 * level.reduction.max(1e-6).log10(),
    );
}
