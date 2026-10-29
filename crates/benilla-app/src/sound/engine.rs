//! The sound engine's init-time settings and the sound-system restart.
//!
//! The reference reads the output device, the channel count, the software HRTF and the DSP effects
//! once, in `SI2::Init` (`0x4c82e0`) → `SE2::Init` (`0x87c710`), and again only when the Sound
//! panel calls `Sound_GameSystem_RestartSoundSystem` (`0x985d30`: `SI2::ShutDown(1)`, `Init(1)`).
//! [`EngineSettings`] is that latch: [`boot_settings`] fills it from the stored config before the
//! device opens, and [`restart_sound_system`] refills it from the live CVars. A restart stops every
//! sound (`SE2::StopAllSounds 0x87ded0`), and the zone pump picks its music again.
//!
//! The output device list is the reference's cache (`s_outputDrivers`): index 0 is "System
//! Default", index `i` the host's device `i - 1`, enumerated at init and at each restart. The
//! choice persists by name (`Sound_OutputDriverName`) and is resolved to an index at init
//! ([`find_driver`], `FindDriverIndex`), which is written back to `Sound_OutputDriverIndex`.

use bevy::prelude::*;

use crate::cvars::{CvarChanged, Cvars};

use super::{output, SoundOutput};

/// The cached list's entry 0, the reference's fallback literal for `SYSTEM_DEFAULT`
/// (`SE2::GetOutputDriverName 0x8784a0`).
pub(crate) const SYSTEM_DEFAULT: &str = "System Default";

/// `Sound_NumChannels`'s clamp (`SE2::Init` at `0x87cea3`/`0x87ceb2`).
const CHANNELS_MIN: i32 = 12;
const CHANNELS_MAX: i32 = 128;

/// What the engine was initialised with, read only at boot and at a restart.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EngineSettings {
    /// The resolved `Sound_OutputDriverIndex`: 0 follows the system default.
    pub(crate) driver: usize,
    /// `Sound_NumChannels`, clamped: the software voices the mix runs
    /// (`System::setSoftwareChannels` at `0x87d3bf`).
    pub(crate) voices: usize,
    /// `Sound_EnableSoftwareHRTF` ([`super::hrtf`]).
    pub(crate) hrtf: bool,
    /// `Sound_EnableDSPEffects`: `DSP_Init` builds the chains only when it is on
    /// ([`super::dsp`]).
    pub(crate) dsp: bool,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            driver: 0,
            voices: 32,
            hrtf: false,
            dsp: true,
        }
    }
}

/// The output device list the panel reads, cached at init and at each restart.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub(crate) struct OutputDrivers(pub(crate) Vec<String>);

impl OutputDrivers {
    /// The cached name at `index` (`SE2::GetOutputDriverName_Cached 0x8792b0`).
    pub(crate) fn name(&self, index: usize) -> Option<&str> {
        match index {
            0 => Some(SYSTEM_DEFAULT),
            i => self.0.get(i - 1).map(String::as_str),
        }
    }

    /// The device to open for `index`; `None` follows the system default.
    pub(crate) fn device(&self, index: usize) -> Option<String> {
        index.checked_sub(1).and_then(|i| self.0.get(i)).cloned()
    }
}

/// `FindDriverIndex`: `index` if its entry carries `name`, else the first entry that does, else 0.
/// The comparison is case-sensitive (`SStrCmp`).
pub(crate) fn find_driver(drivers: &OutputDrivers, name: &str, index: i32) -> usize {
    if let Ok(i) = usize::try_from(index) {
        if drivers.name(i) == Some(name) {
            return i;
        }
    }
    (0..=drivers.0.len())
        .find(|&i| drivers.name(i) == Some(name))
        .unwrap_or(0)
}

/// `Sound_NumChannels` as the voice ceiling.
pub(crate) fn voice_ceiling(num_channels: i32) -> usize {
    num_channels.clamp(CHANNELS_MIN, CHANNELS_MAX) as usize
}

/// A CVar value as `CVar::m_intValue` reads it.
fn int(value: Option<&str>, default: i32) -> i32 {
    value.map_or(default, benilla_ui::script::sstr_to_int)
}

/// The settings `values` names, against `drivers`.
fn settings_from(drivers: &OutputDrivers, get: impl Fn(&str) -> Option<String>) -> EngineSettings {
    let name = get("Sound_OutputDriverName").unwrap_or_default();
    let index = int(get("Sound_OutputDriverIndex").as_deref(), 0);
    EngineSettings {
        driver: find_driver(drivers, &name, index),
        voices: voice_ceiling(int(get("Sound_NumChannels").as_deref(), 32)),
        hrtf: int(get("Sound_EnableSoftwareHRTF").as_deref(), 0) != 0,
        dsp: int(get("Sound_EnableDSPEffects").as_deref(), 1) != 0,
    }
}

/// The boot latch, read from the stored config before the registry loads, as the device opens in
/// the plugin's build.
pub(super) fn boot_settings() -> (OutputDrivers, EngineSettings) {
    let drivers = OutputDrivers(output::device_names());
    let settings = settings_from(&drivers, crate::cvars::boot_cvar);
    (drivers, settings)
}

/// A request to restart the sound system, from the panel's `Sound_GameSystem_RestartSoundSystem`
/// or a lost output device. Every stream owner stops its sounds on it.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) struct SoundRestart;

/// A lost device restarted the engine and the VM has not heard `SOUND_DEVICE_UPDATE` yet.
#[derive(Resource, Default)]
pub(crate) struct DeviceChanged(pub(super) bool);

/// The restart's systems; a stream owner stops its sounds after it, in the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RestartSet;

/// `OutputDriverIndexHandler` (`0x4d0dd0`): an index write stores that entry's name in
/// `Sound_OutputDriverName`; an index past the list stores `""`, as FMOD's failed name read leaves
/// the zeroed buffer.
pub(super) fn on_driver_index(
    ev: On<CvarChanged>,
    drivers: Res<OutputDrivers>,
    mut cvars: ResMut<Cvars>,
) {
    if !ev.is("Sound_OutputDriverIndex") {
        return;
    }
    let index = benilla_ui::script::sstr_to_int(&ev.new);
    let name = usize::try_from(index)
        .ok()
        .and_then(|i| drivers.name(i))
        .unwrap_or("")
        .to_string();
    cvars.set("Sound_OutputDriverName", &name);
}

/// Write the boot-resolved index back to the registry once it has loaded, as `SI2::Init` sets
/// `Sound_OutputDriverIndex` after `SE2::Init`.
pub(super) fn write_back_driver(out: NonSend<SoundOutput>, mut cvars: ResMut<Cvars>) {
    cvars.set("Sound_OutputDriverIndex", &out.engine.driver.to_string());
}

/// A lost output device restarts: `SI2::HandleSoundDeviceChange` (`0x4c66b0`) →
/// `ValidateOutputDriver` (`0x4c6560`) resets the index to 0 and restarts; the panel's own restart
/// arrives through [`super::ui`].
pub(super) fn restart_on_lost_device(
    mut out: NonSendMut<SoundOutput>,
    mut restarts: MessageWriter<SoundRestart>,
) {
    if out.mixer.as_mut().is_some_and(super::Mixer::take_fell_back) {
        restarts.write(SoundRestart);
    }
}

/// The restart itself: re-enumerate the devices, re-read the latch from the live CVars, stop every
/// kit channel and reopen the output. The streams' owners stop theirs on the same message.
pub(super) fn restart_sound_system(
    mut restarts: MessageReader<SoundRestart>,
    mut out: NonSendMut<SoundOutput>,
    mut drivers: ResMut<OutputDrivers>,
    mut cvars: ResMut<Cvars>,
    mut device_changed: ResMut<DeviceChanged>,
) {
    if restarts.read().count() == 0 {
        return;
    }
    let lost = out.mixer.is_some()
        && out.engine.driver != 0
        && !output::device_names()
            .iter()
            .any(|n| Some(n.as_str()) == drivers.name(out.engine.driver));
    if lost {
        // `ValidateOutputDriver` sets the index to "0" before it restarts.
        cvars.set("Sound_OutputDriverIndex", "0");
        cvars.set("Sound_OutputDriverName", SYSTEM_DEFAULT);
    }
    drivers.set_if_neq(OutputDrivers(output::device_names()));
    let settings = settings_from(&drivers, |name| cvars.get(name).map(str::to_string));
    cvars.set("Sound_OutputDriverIndex", &settings.driver.to_string());
    super::kit::stop_all(&mut out);
    let device = drivers.device(settings.driver);
    info!(
        "sound: restart — output {}, {} voices, software HRTF {}, DSP effects {}",
        device.as_deref().unwrap_or(SYSTEM_DEFAULT),
        settings.voices,
        if settings.hrtf { "on" } else { "off" },
        if settings.dsp { "on" } else { "off" },
    );
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.select_output(device);
    }
    out.engine = settings;
    if lost {
        device_changed.0 = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drivers() -> OutputDrivers {
        OutputDrivers(vec!["Speakers".into(), "Headphones".into()])
    }

    /// The cache is "System Default" then the host's devices; an index past it has no name.
    #[test]
    fn the_cached_list_puts_the_system_default_first() {
        let d = drivers();
        assert_eq!(d.name(0), Some(SYSTEM_DEFAULT));
        assert_eq!(d.name(2), Some("Headphones"));
        assert_eq!(d.name(3), None);
        assert_eq!(d.device(0), None);
        assert_eq!(d.device(1).as_deref(), Some("Speakers"));
    }

    /// `FindDriverIndex`: the index when its name matches, else a search by name, else 0.
    #[test]
    fn the_saved_name_wins_over_a_stale_index() {
        let d = drivers();
        assert_eq!(find_driver(&d, "Headphones", 2), 2);
        assert_eq!(find_driver(&d, "Headphones", 1), 2);
        assert_eq!(find_driver(&d, "headphones", 2), 0, "case-sensitive");
        // The registered default names a DirectSound device no host here lists.
        assert_eq!(find_driver(&d, "Primary Sound Driver", 0), 0);
        assert_eq!(find_driver(&d, SYSTEM_DEFAULT, 5), 0);
        assert_eq!(find_driver(&d, "Speakers", -1), 1);
    }

    #[test]
    fn the_channel_count_is_clamped_to_12_through_128() {
        assert_eq!(voice_ceiling(32), 32);
        assert_eq!(voice_ceiling(64), 64);
        assert_eq!(voice_ceiling(4), 12);
        assert_eq!(voice_ceiling(1000), 128);
    }

    #[test]
    fn the_latch_reads_the_four_cvars() {
        let d = drivers();
        let s = settings_from(&d, |n| {
            match n {
                "Sound_OutputDriverName" => Some("Headphones"),
                "Sound_OutputDriverIndex" => Some("0"),
                "Sound_NumChannels" => Some("64"),
                "Sound_EnableSoftwareHRTF" => Some("1"),
                "Sound_EnableDSPEffects" => Some("0"),
                _ => None,
            }
            .map(str::to_string)
        });
        assert_eq!(
            s,
            EngineSettings {
                driver: 2,
                voices: 64,
                hrtf: true,
                dsp: false,
            }
        );
        assert_eq!(settings_from(&d, |_| None), EngineSettings::default());
    }
}
