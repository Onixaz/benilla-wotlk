//! The stock Video window's Resolution and Effects rows as the window, the presentation path and
//! the world see them: the change callbacks of the reference's registration blocks
//! (`RegisterGxCVars 0x76a630`, `CWorldParam::Initialize 0x78e400`, `Client.cpp`'s `0x4047e0`),
//! each arm writing its own knob.
//!
//! `gxWindow "0"` mode-sets the display to `gxResolution` at `gxRefresh`, as the reference's device
//! does (`ISetPresentParms 0x68e250`). Deviation: where the platform has no client mode-set
//! (Wayland, where winit ignores `Fullscreen::Exclusive`, and macOS) the window is borderless over
//! the monitor at its own mode, and the refresh dropdown shows the stock panel's disabled state.
//!
//! The `gx*` rows are latched until `RestartGx()`, as in the reference; a restart re-asserts them
//! against the window rather than re-creating the device, since wgpu reconfigures the surface live.
//!
//! Uncapped is `AutoNoVsync`, never `Immediate`: on Metal `Immediate` takes ~1 s `nextDrawable`
//! stalls ([`crate::capture::probe_uncap_mode`]).

use benilla_ui::script::{ScreenMode, ScreenResolution};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{
    MonitorSelection, PresentMode, PrimaryMonitor, PrimaryWindow, VideoMode, VideoModeSelection,
    WindowMode, WindowResolution,
};

/// `$WOW_NOVSYNC=1`: uncap for this session only; it never reaches `Config.wtf` (the
/// `session_owned` set in [`crate::cvars`]).
pub(crate) fn novsync_env() -> bool {
    std::env::var("WOW_NOVSYNC").as_deref() == Ok("1")
}

/// Whether this run sizes its own window (a capture, a background run) and so stays windowed.
/// Session-only: `gxWindow`/`gxResolution` are env-overridden while it holds, never saved over.
pub(crate) fn windowed_env() -> bool {
    std::env::var_os("WOW_WIN").is_some()
        || std::env::var_os("WOW_CAPTURE").is_some()
        || std::env::var_os("WOW_CAPTURE_UI").is_some()
        || benilla_world::bgwin::background_run()
}

/// `$WOW_WIN=WxH` in logical px; the one parser, so the size asked for and the size checked agree.
pub(crate) fn requested_window_size() -> Option<UVec2> {
    let v = std::env::var("WOW_WIN").ok()?;
    let (w, h) = v.split_once('x')?;
    Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
}

/// `$WOW_DPI=<f32>`: render at another display's pixel grid, since text rasterizing and snapping
/// quantize in device pixels. It overrides the window's scale factor, so `WOW_WIN` then means
/// physical pixels and a capture is the framebuffer that display would show.
pub(crate) fn requested_dpi() -> Option<f32> {
    let v: f32 = std::env::var("WOW_DPI").ok()?.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v)
}

/// Apply [`requested_dpi`] to a window resolution; the one place the knob is spent.
pub(crate) fn at_requested_dpi(res: WindowResolution) -> WindowResolution {
    match requested_dpi() {
        Some(dpi) => res.with_scale_factor_override(dpi),
        None => res,
    }
}

/// Whether this platform can mode-set the display: Windows, and X11 (XRandR). Wayland and macOS
/// cannot, and there fullscreen is borderless.
pub(crate) fn exclusive_supported() -> bool {
    #[cfg(target_os = "windows")]
    {
        true
    }
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
    }
    #[cfg(not(any(
        target_os = "windows",
        all(unix, not(any(target_os = "macos", target_os = "android")))
    )))]
    {
        false
    }
}

/// The two display modes: the `gxWindow` row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum DisplayMode {
    /// `gxWindow "0"`, the reference's default: the display mode-set to `gxResolution`.
    #[default]
    Fullscreen,
    /// A window at `gxResolution`. `gxWindow "1"`.
    Windowed,
}

/// The windowed size while `gxResolution` is unpicked ("").
pub(crate) const DEFAULT_WINDOWED: UVec2 = UVec2::new(1600, 900);

/// `gxWindow`'s value to the mode, by the reference's `SStrToInt(v) != 0` (`0x769550`); shared
/// by [`crate::cvars`]'s arm and the boot read.
pub(crate) fn display_from_flag(v: f32) -> DisplayMode {
    if v != 0.0 {
        DisplayMode::Windowed
    } else {
        DisplayMode::Fullscreen
    }
}

/// `gxResolution`'s value (`"1280x800"`, parsed with `sscanf("%d%c%d")`, `0x76a220`) to a size in
/// physical pixels. A zero extent is refused rather than handed to the windowing system.
pub(crate) fn parse_resolution(value: &str) -> Option<UVec2> {
    let (w, h) = value.split_once(['x', 'X'])?;
    let size = UVec2::new(w.trim().parse().ok()?, h.trim().parse().ok()?);
    (size.x > 0 && size.y > 0).then_some(size)
}

/// The `WindowMode` a configuration means. `maximize` is `gxMaximize`, which counts only while
/// windowed: the reference's window rebuild (`0x58cf10`) gives a windowed, maximized window the
/// popup style `0x90000000`, no caption and no border, sized to the screen, which is a borderless
/// window over the monitor. `exclusive` is the mode to set, `None` keeping the monitor's own.
pub(crate) fn window_mode(
    display: DisplayMode,
    maximize: bool,
    exclusive: Option<Option<VideoMode>>,
    monitor: MonitorSelection,
) -> WindowMode {
    match (display, maximize, exclusive) {
        (DisplayMode::Fullscreen, _, Some(mode)) => WindowMode::Fullscreen(
            monitor,
            mode.map_or(VideoModeSelection::Current, VideoModeSelection::Specific),
        ),
        (DisplayMode::Fullscreen, _, None) | (DisplayMode::Windowed, true, _) => {
            WindowMode::BorderlessFullscreen(monitor)
        }
        (DisplayMode::Windowed, false, _) => WindowMode::Windowed,
    }
}

/// The mode the primary window is born in, resolved before the `App` exists so a fullscreen launch
/// does not flash windowed until `Startup`. `MonitorSelection::Primary`, since `Current` has no
/// answer before the window exists (`bevy_winit::select_monitor`); a picked `gxResolution` is set
/// at the first frame that sees the monitor's modes ([`apply_window_mode`]).
pub(crate) fn boot_window_mode() -> WindowMode {
    let flag = |name| crate::cvars::boot_cvar(name).and_then(|v| v.parse::<f32>().ok());
    // A run that sizes its own window owns both rows for the session.
    let (display, maximize) = if windowed_env() {
        (DisplayMode::Windowed, false)
    } else {
        (
            flag("gxWindow").map_or_else(DisplayMode::default, display_from_flag),
            flag("gxMaximize").is_some_and(|v| v != 0.0),
        )
    };
    let exclusive = exclusive_supported().then_some(None);
    window_mode(display, maximize, exclusive, MonitorSelection::Primary)
}

/// The windowed size the primary window is born at, `gxResolution` (physical px), read as
/// [`boot_window_mode`] is; `bevy_winit` ignores it while fullscreen.
pub(crate) fn boot_windowed_size() -> UVec2 {
    crate::cvars::boot_cvar("gxResolution")
        .and_then(|v| parse_resolution(&v))
        .unwrap_or(DEFAULT_WINDOWED)
}

/// The swapchain's frames in flight: one more with `gxTripleBuffer` under vsync, the reference's
/// second back buffer (`ISetPresentParms 0x68e250` counts it only with vsync); `None` is wgpu's 2.
/// The patched `bevy_render` (`third_party/bevy_render`) reconfigures the surface when it moves.
pub(crate) fn frame_latency(vsync: bool, triple_buffer: bool) -> Option<std::num::NonZero<u32>> {
    (vsync && triple_buffer).then(|| std::num::NonZero::new(3).expect("non-zero"))
}

/// [`frame_latency`] for the primary window's birth, read off `Config.wtf` as
/// [`boot_window_mode`] is.
pub(crate) fn boot_frame_latency() -> Option<std::num::NonZero<u32>> {
    let flag = |name| crate::cvars::boot_cvar(name).and_then(|v| v.parse::<f32>().ok());
    let vsync = !novsync_env() && flag("gxVSync").is_none_or(|v| v != 0.0);
    frame_latency(vsync, flag("gxTripleBuffer").is_some_and(|v| v != 0.0))
}

/// The video knobs a CVar write lands on. Defaults read only the environment; `load_config`
/// applies the file at `Startup`, which the boot window already matches.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VideoConfig {
    pub(crate) vsync: bool,
    pub(crate) display: DisplayMode,
    /// `gxMaximize`: a windowed window fills the monitor, borderless ([`window_mode`]).
    pub(crate) maximize: bool,
    /// `gxResolution`, physical px: the fullscreen mode and the window's size; `None` is unpicked.
    pub(crate) resolution: Option<UVec2>,
    /// `gxRefresh` in Hz, the fullscreen mode's rate.
    pub(crate) refresh: u32,
    /// `gxTripleBuffer` ([`frame_latency`]).
    pub(crate) triple_buffer: bool,
    /// `gxFixLag`: the frame waits for the GPU to drain after its present ([`fix_input_lag`]).
    pub(crate) fix_lag: bool,
    /// `gxCursor`: the OS cursor, else the client draws its own over the frame ([`crate::cursor`]).
    pub(crate) hw_cursor: bool,
    /// `windowResizeLock`: the window's border drag is refused (`ResizeDragWindow 0x869970`).
    pub(crate) resize_lock: bool,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            vsync: !novsync_env(),
            display: if windowed_env() {
                DisplayMode::Windowed
            } else {
                DisplayMode::default()
            },
            maximize: false,
            resolution: None,
            refresh: 75,
            triple_buffer: false,
            fix_lag: false,
            hw_cursor: true,
            resize_lock: false,
        }
    }
}

impl VideoConfig {
    /// The window's size while windowed.
    pub(crate) fn windowed(&self) -> UVec2 {
        self.resolution.unwrap_or(DEFAULT_WINDOWED)
    }
}

/// The present mode a vsync setting means. On is `PresentMode::default()` (`Fifo`, never tears),
/// not `AutoVsync`, which resolves to `FifoRelaxed` and can tear on a late frame.
pub(crate) fn present_mode(vsync: bool) -> PresentMode {
    if vsync {
        PresentMode::default()
    } else {
        PresentMode::AutoNoVsync
    }
}

/// `componentTextureLevel`, the composited body texture's side as `1 << level`
/// (`CCharacterComponent::Initialize`, `0x401ff0` then `0x4f1a20`, together clamping to [7, 9]);
/// 8 is 256², 9 is 512².
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ComponentTextureLevel(pub(crate) u8);

impl Default for ComponentTextureLevel {
    /// The registered "8".
    fn default() -> Self {
        Self(8)
    }
}

/// The world-side knobs the Effects rows write, beside the window's [`VideoConfig`].
#[derive(SystemParam)]
pub(crate) struct EffectKnobs<'w> {
    view: ResMut<'w, benilla_world::view::ViewDistance>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    msaa: ResMut<'w, benilla_world::view::MsaaSetting>,
    msaa_formats: Res<'w, benilla_world::view::MsaaFormats>,
    tex_filter: ResMut<'w, benilla_assets::TexFilterSetting>,
    clutter: ResMut<'w, benilla_world::clutter::ClutterConfig>,
    weather: ResMut<'w, benilla_world::weather::WeatherState>,
    particles: Option<ResMut<'w, benilla_world::particles::ParticleTuning>>,
    spell_effect_level: ResMut<'w, SpellEffectLevel>,
    ffx: ResMut<'w, benilla_world::ffx_glow::FfxSwitches>,
    terrain: ResMut<'w, benilla_world::terrain_stream::TerrainOptions>,
    projected: ResMut<'w, benilla_world::view::ProjectedTextures>,
    component_level: ResMut<'w, ComponentTextureLevel>,
}

pub(crate) struct VideoPlugin;

/// A write a reference callback refuses: put the row back without firing, and echo the
/// callback's line to the console.
fn refuse(
    cvars: &mut crate::cvars::Cvars,
    console: &mut Option<ResMut<crate::console::ConsoleEcho>>,
    ev: &crate::cvars::CvarChanged,
    line: &str,
) {
    cvars.mirror(&ev.name, &ev.old);
    if let Some(console) = console.as_mut() {
        console.print(line.to_string());
    }
    warn!("cvar {}: '{}' refused ({line})", ev.name, ev.new);
}

/// The change callbacks; each arm writes only its own knob, and a write the reference's callback
/// refuses is put back ([`refuse`]).
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut cfg: ResMut<VideoConfig>,
    mut fx: EffectKnobs,
    mut console: Option<ResMut<crate::console::ConsoleEcho>>,
    mut cvars: ResMut<crate::cvars::Cvars>,
) {
    use benilla_world::view::MSAA_RANGE;
    let v = ev.num();
    let int = benilla_ui::script::sstr_to_int(&ev.new);
    match ev.key().as_str() {
        // "" is benilla's unpicked size; any other value that is not a size is ignored.
        "gxresolution" => match (ev.new.trim().is_empty(), parse_resolution(&ev.new)) {
            (true, _) => cfg.resolution = None,
            (false, Some(size)) => cfg.resolution = Some(size),
            (false, None) => warn!("cvar gxResolution: unparseable value '{}' ignored", ev.new),
        },
        "gxrefresh" => cfg.refresh = int.max(0) as u32,
        "gxvsync" => cfg.vsync = int != 0,
        "gxwindow" => cfg.display = display_from_flag(int as f32),
        "gxmaximize" => cfg.maximize = int != 0,
        // `0x769360`: "TripleBuffer must be 0 or 1".
        "gxtriplebuffer" if !(0..=1).contains(&int) => {
            refuse(&mut cvars, &mut console, &ev, "TripleBuffer must be 0 or 1")
        }
        "gxtriplebuffer" => cfg.triple_buffer = int != 0,
        "gxfixlag" => cfg.fix_lag = int != 0,
        "gxcursor" => cfg.hw_cursor = int != 0,
        "windowresizelock" => cfg.resize_lock = int != 0,
        // The FFX pass's switches, read every frame, so a write shows on the next frame.
        "ffx" | "ffxglow" | "ffxdeath" => {
            match ev.key().as_str() {
                "ffx" => fx.ffx.master = ev.flag(),
                "ffxglow" => fx.ffx.glow = ev.flag(),
                _ => fx.ffx.death = ev.flag(),
            }
            info!("video: full-screen effects {:?}", *fx.ffx);
        }
        // `World::SetFarClip`, validated for the map the player is on.
        "farclip" => {
            let map = fx.map.as_ref().map_or(0, |m| m.0);
            fx.view.set_farclip(v, map);
        }
        "nearclip" => fx.view.set_nearclip(v),
        // `0x769610`: clamped to [1, 16], then this GPU's ceiling, since an unoffered count is a
        // wgpu validation error; the camera takes it at the `RestartGx` commit.
        "gxmultisample" => {
            let asked = (int.max(0) as u32).clamp(*MSAA_RANGE.start(), *MSAA_RANGE.end());
            let granted = fx.msaa_formats.clamp(asked);
            if granted != asked {
                warn!("cvar gxMultisample: this GPU does not offer {asked}x multisampling — using {granted}x");
            }
            fx.msaa.samples = granted;
        }
        // `0x4023c0`: refused outside [0, 5]; the policy reloads the world's textures.
        "texturefilteringmode" => match u8::try_from(int)
            .ok()
            .filter(|m| benilla_assets::TEXTURE_FILTERING_RANGE.contains(m))
        {
            Some(mode) => fx.tex_filter.mode = mode,
            None => refuse(
                &mut cvars,
                &mut console,
                &ev,
                "Texture filtering mode must be in range 0 to 6.",
            ),
        },
        // `0x78df00`: refused above 1, else the device's base mip, every texture reloaded.
        "basemip" => match int {
            0 | 1 => fx.tex_filter.base_mip = int as u8,
            _ => refuse(&mut cvars, &mut console, &ev, "BaseMip must be 0 or 1"),
        },
        // `0x78d860`: refused outside [0.1, 1.0].
        "particledensity" => {
            if (0.1..=1.0).contains(&v) {
                if let Some(particles) = fx.particles.as_mut() {
                    particles.set_density(v);
                }
            } else {
                refuse(
                    &mut cvars,
                    &mut console,
                    &ev,
                    "Value must be between 0.1 and 1.0.",
                );
            }
        }
        // `0x78dab0`: refused outside [16, 256]; the loaded tiles re-scatter off the change.
        "groundeffectdensity" => {
            let cells = int as f32;
            if benilla_world::clutter::GROUND_EFFECT_DENSITY_RANGE.contains(&cells) {
                fx.clutter.set_cells(cells);
            } else {
                refuse(
                    &mut cvars,
                    &mut console,
                    &ev,
                    "Ground effect density must be in range 16 to 256.",
                );
            }
        }
        // `0x78db10`: refused outside [0, 140].
        "groundeffectdist" => {
            if benilla_world::clutter::GROUND_EFFECT_DIST_RANGE.contains(&v) {
                fx.clutter.fade_far = v;
            } else {
                refuse(
                    &mut cvars,
                    &mut console,
                    &ev,
                    "Ground effect distance must be in range 0.000000 to 140.000000.",
                );
            }
        }
        // `0x78dc60`: clamped, then `ScaleFadeDist`. Marking the view distance changed wakes every
        // cached draw-set verdict, as the bands they rest on moved.
        "environmentdetail" => {
            benilla_world::model_fade::set_fade_scale(v);
            fx.view.set_changed();
        }
        // `0x78d6f0`: refused above 1. Applied live, where the reference reads it at world load.
        "shadowlevel" if int > 1 => refuse(
            &mut cvars,
            &mut console,
            &ev,
            "Shadow mip level must be in range 0 - 1.",
        ),
        "shadowlevel" => fx.terrain.low_mip = int != 0,
        // `0x78de60`. Applied live, where the reference reads it at map load.
        "specular" => fx.terrain.specular = int != 0,
        // `0x78dcf0`: `CWorld::enables2` bit 4, read per projected batch.
        "projectedtextures" => fx.projected.0 = int != 0,
        // Read once at character-component init in the reference; here every body re-composites.
        "componenttexturelevel" => fx.component_level.0 = int.clamp(7, 9) as u8,
        // Weather Intensity 0..3 (`MapWeather.cpp`'s handler), the precipitation spawn rate.
        "weatherdensity" => fx.weather.weather_density = v.trunc().clamp(0.0, 3.0) as u8,
        // No callback (`0x78e400`): the record's integer, read at each shard emitter's spawn.
        "spelleffectlevel" => fx.spell_effect_level.0 = int,
        _ => {}
    }
}

/// The `spellEffectLevel` record's integer (`SStrToInt` of the value): what the blizzard shard
/// emitter reads at spawn (`BlizzardObject::Init 0x7fb5b0`).
#[derive(Resource)]
pub(crate) struct SpellEffectLevel(pub(crate) i32);

impl Default for SpellEffectLevel {
    /// The registered "9".
    fn default() -> Self {
        Self(9)
    }
}

/// `/console detailDoodadAlpha [0..255]`, a console command, not a CVar, so it never persists: the
/// ground-clutter cutout that `texel.a x distance_ramp` is tested against. Out of range is rejected
/// with a readout; a bare command prints the readout.
fn detail_doodad_alpha(world: &mut World, args: &str) -> Vec<String> {
    let Some(mut clutter) = world.get_resource_mut::<benilla_world::clutter::ClutterConfig>()
    else {
        return vec!["detailDoodadAlpha: this run has no ground clutter".to_string()];
    };
    match args
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<u8>().ok())
    {
        Some(v) => {
            clutter.alpha_ref = f32::from(v) / 255.0;
            vec![format!("detailDoodadAlpha set to {v}")]
        }
        None => vec![format!(
            "detailDoodadAlpha is {} (usage: /console detailDoodadAlpha 0-255)",
            (clutter.alpha_ref * 255.0).round() as u32
        )],
    }
}

impl Plugin for VideoPlugin {
    fn build(&self, app: &mut App) {
        use crate::console::ConsoleCommandApp;
        app.init_resource::<SpellEffectLevel>()
            .init_resource::<ComponentTextureLevel>();
        app.add_observer(on_cvar);
        app.console_command(
            "detailDoodadAlpha",
            "The ground-clutter cutout reference, 0-255 (128 = the default).",
            detail_doodad_alpha,
        );
        app.init_resource::<VideoConfig>()
            .init_resource::<GxRestarts>()
            .add_systems(Startup, (log_display_session, check_window_pinned).chain())
            // At the frame's end, after the tick and the CVar sync: the stock Okay handler calls
            // `SetCVar` per row, then `RestartGx()`, so the staged rows are registered before the
            // commit, and no reader of the window sees it change mid-frame.
            .add_systems(
                Last,
                (
                    drain_restart_gx,
                    apply_present_mode,
                    apply_window_mode,
                    apply_resize_lock,
                    apply_multisample,
                )
                    .chain(),
            )
            // A push the tick may read (`GetScreenResolutions`): the feed phase.
            .add_systems(
                Update,
                publish_display_modes.in_set(crate::ui_script::UiFeed),
            );
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app
                .init_resource::<FixLag>()
                .add_systems(bevy::render::ExtractSchedule, extract_fix_lag)
                .add_systems(
                    bevy::render::Render,
                    fix_input_lag.in_set(bevy::render::RenderSystems::Cleanup),
                );
        }
    }
}

/// How many `RestartGx()` calls the interface has made. A counter, not a flag: each applier keeps
/// its own last value, so one bump forces one re-assertion in each, in any order.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GxRestarts(pub(crate) u32);

/// Carry `RestartGx()` calls into [`GxRestarts`] and commit the latched `gx*` rows (flags `3`),
/// as the reference's restart runs `UpdateGxCVars` (`0x7698b0`); `GetCVar` answers the applied
/// value until then.
fn drain_restart_gx(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut restarts: ResMut<GxRestarts>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    mut commands: Commands,
) {
    let Some(mut script) = script else {
        return;
    };
    let asks = script.take_restart_gx_asks();
    if asks == 0 {
        // A `ResMut` deref-mut is a change signal, so a quiet frame touches nothing.
        return;
    }
    restarts.0 = restarts.0.wrapping_add(asks);
    let committed = cvars.commit_latched();
    if cvars.has_events() {
        for event in cvars.take_events() {
            commands.trigger(event);
        }
    }
    info!("video: RestartGx — {committed} staged setting(s) committed; re-asserting the display");
}

/// The reference's filters on the monitor's modes (`SetupFormats 0x54f050`): keep when
/// `w/h >= 1.248` (just under 5:4, so square and portrait fail), `w > 640` and `h > 480`.
fn offerable(size: UVec2) -> bool {
    size.x > 640 && size.y > 480 && size.x as f32 / size.y as f32 >= 1.248
}

/// `SetupFormats`' four hardcoded sizes, used when no monitor mode survives [`offerable`]. The
/// reference also takes this path when `widescreen` (default `"1"`) is 0; benilla does not
/// register that CVar.
const SCREEN_FALLBACK: [UVec2; 4] = [
    UVec2::new(800, 600),
    UVec2::new(1024, 768),
    UVec2::new(1280, 1024),
    UVec2::new(1600, 1200),
];

/// A mode's refresh rate in whole Hz, as `CGxMonitorMode::m_refreshRate` holds it.
fn mode_hz(mode: &VideoMode) -> u32 {
    (mode.refresh_rate_millihertz + 500) / 1000
}

/// The fullscreen mode for `size` at `refresh` Hz on `monitor`: an exact rate at the deepest bit
/// depth, else the size's fastest rate. Deviation: the reference's device fails over to its own
/// format when the pair is not offered; ours keeps the size.
pub(crate) fn pick_mode(
    monitor: &bevy::window::Monitor,
    size: UVec2,
    refresh: u32,
) -> Option<VideoMode> {
    let at_size = monitor
        .video_modes
        .iter()
        .filter(|m| m.physical_size == size);
    at_size
        .clone()
        .filter(|m| mode_hz(m) == refresh)
        .max_by_key(|m| m.bit_depth)
        .or_else(|| at_size.max_by_key(|m| (m.refresh_rate_millihertz, m.bit_depth)))
        .copied()
}

/// The dropdown's modes from the monitor the client mode-sets, the primary one (the reference's
/// `D3DADAPTER_DEFAULT`): its sizes through [`offerable`], each with its rates, else
/// [`SCREEN_FALLBACK`] with none. Where the platform cannot mode-set, each size carries the single
/// rate 0, the stock panel's disabled-dropdown sentinel.
fn offered_modes(monitor: Option<&bevy::window::Monitor>) -> Vec<ScreenMode> {
    let exclusive = exclusive_supported();
    let mut offered: Vec<ScreenMode> = Vec::new();
    for mode in monitor.iter().flat_map(|m| m.video_modes.iter()) {
        if !offerable(mode.physical_size) {
            continue;
        }
        offered.push(ScreenMode {
            size: ScreenResolution {
                width: mode.physical_size.x,
                height: mode.physical_size.y,
            },
            refresh_rates: vec![if exclusive { mode_hz(mode) } else { 0 }],
        });
    }
    if offered.is_empty() {
        offered.extend(SCREEN_FALLBACK.iter().map(|s| ScreenMode {
            size: ScreenResolution {
                width: s.x,
                height: s.y,
            },
            refresh_rates: Vec::new(),
        }));
    }
    offered
}

/// The dropdown's current entry: the picked `gxResolution`, else the size in use.
fn current_mode(cfg: &VideoConfig, window: &Window) -> Option<ScreenResolution> {
    let size = cfg.resolution.unwrap_or(UVec2::new(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
    ));
    Some(ScreenResolution {
        width: size.x,
        height: size.y,
    })
    .filter(|r| r.width > 0 && r.height > 0)
}

/// The list [`publish_display_modes`] last pushed and the current entry.
type PublishedModes = Option<(Vec<ScreenMode>, Option<ScreenResolution>)>;

/// The host half of `GetScreenResolutions`, `GetCurrentResolution` and `GetRefreshRates`,
/// recomputed on change; the reference builds its list once and never invalidates it.
fn publish_display_modes(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    cfg: Res<VideoConfig>,
    windows: Query<&Window, With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor, With<PrimaryMonitor>>,
    mut last: Local<crate::ui_script::VmMemo<PublishedModes>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let (offered, current) = (
        offered_modes(monitors.iter().next()),
        current_mode(&cfg, window),
    );
    // Keyed by VM: a `ReloadUI` VM has been pushed nothing, so a plain `Local` would skip it.
    let memo = last.get(&script);
    if memo.as_ref() == Some(&(offered.clone(), current)) {
        return;
    }
    *memo = Some((offered.clone(), current));
    script.set_screen_modes(offered, current);
}

/// [`publish_display_modes`]' push for a VM about to load, which the reference has before
/// `UI_Init`: `UpdateMenuBarTop` divides the current entry's sides at load (`UIParent.lua:1171`).
pub(crate) fn seed_display_modes_for_vm(
    world: &mut World,
    script: &mut benilla_ui::script::UiScript,
) {
    let cfg = world
        .get_resource::<VideoConfig>()
        .copied()
        .unwrap_or_default();
    let mut windows = world.query_filtered::<&Window, With<PrimaryWindow>>();
    let Ok(current) = windows.single(world).map(|w| current_mode(&cfg, w)) else {
        return;
    };
    let mut monitors = world.query_filtered::<&bevy::window::Monitor, With<PrimaryMonitor>>();
    let offered = offered_modes(monitors.iter(world).next());
    script.set_screen_modes(offered, current);
}

/// Check the window got the size `$WOW_WIN` asked for: the window manager may clamp it to the
/// display (macOS does). Fatal under a capture, whose diffs assume the scenario's size; a warning
/// otherwise.
fn check_window_pinned(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(want) = requested_window_size() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    // Under `$WOW_DPI`, `$WOW_WIN` is physical px, so compare the physical size.
    let res = &window.resolution;
    let got = match requested_dpi() {
        Some(_) => UVec2::new(res.physical_width(), res.physical_height()),
        None => UVec2::new(res.width() as u32, res.height() as u32),
    };
    if got == want {
        return;
    }
    let unit = if requested_dpi().is_some() {
        "physical"
    } else {
        "logical"
    };
    let capturing =
        std::env::var_os("WOW_CAPTURE").is_some() || std::env::var_os("WOW_CAPTURE_UI").is_some();
    if !capturing {
        warn!(
            "window: asked for {}x{} {unit}, got {}x{} — the window manager clamped it to the \
             display. Harmless here; it would invalidate a capture.",
            want.x, want.y, got.x, got.y
        );
        return;
    }
    error!(
        "window: REFUSING this capture — asked for {}x{} {unit}, got {}x{}. The window manager \
         clamped the request to the display this window opened on, so the image would not be the \
         size the scenario is denominated in and any diff against it would be meaningless. Use a \
         size that fits the current display (or move the window to a bigger one) and re-run.",
        want.x, want.y, got.x, got.y
    );
    exit.write(AppExit::error());
}

/// One boot line naming what the window got and, on Linux, the backend and any nested compositor
/// (mouse-look's `CursorGrabMode::Locked` is rejected on X11). Not dev-only: players paste it.
fn log_display_session(windows: Query<&Window, With<PrimaryWindow>>) {
    let Ok(window) = windows.single() else {
        return;
    };
    let res = &window.resolution;
    info!(
        "video: {:?}, {}x{} logical / {}x{} physical (scale {}){}",
        window.mode,
        res.width(),
        res.height(),
        res.physical_width(),
        res.physical_height(),
        res.scale_factor(),
        display_session(),
    );
}

/// The display-server facts as a trailing clause; empty except on Linux/BSD, where the windowing
/// backend is chosen at runtime.
fn display_session() -> String {
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        let set = |k: &str| std::env::var_os(k).is_some();
        // winit prefers Wayland when `WAYLAND_DISPLAY` is set, else X11 (XWayland under a
        // Wayland compositor); `bevy`'s `default_platform` builds both.
        let backend = match (set("WAYLAND_DISPLAY"), set("DISPLAY")) {
            (true, _) => "wayland",
            (false, true) => "x11",
            (false, false) => "none",
        };
        // gamescope exports its socket name to children; the SteamOS session stamps the Deck.
        let nested = if set("GAMESCOPE_WAYLAND_DISPLAY") {
            ", gamescope"
        } else {
            ""
        };
        let deck = if set("SteamDeck") { ", steamdeck" } else { "" };
        format!(
            " [{backend}{nested}{deck}, XDG_SESSION_TYPE={}]",
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unset".into()),
        )
    }
    #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
    String::new()
}

/// Push vsync and the frames in flight to the window only when the setting moves or a
/// `RestartGx()` asks: the capture probes write `present_mode` directly, and their override must
/// stick.
fn apply_present_mode(
    cfg: Res<VideoConfig>,
    restarts: Res<GxRestarts>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_restart: Local<u32>,
) {
    // A `RestartGx()` re-asserts even when nothing moved.
    let forced = std::mem::replace(&mut *last_restart, restarts.0) != restarts.0;
    if !cfg.is_changed() && !forced {
        return;
    }
    let want = present_mode(cfg.vsync);
    let latency = frame_latency(cfg.vsync, cfg.triple_buffer);
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    // A spurious change to `Window` is a surface reconfigure.
    if window.present_mode != want {
        window.present_mode = want;
        info!(
            "video: vsync {} ({want:?})",
            if cfg.vsync { "on" } else { "off" }
        );
    }
    if window.desired_maximum_frame_latency != latency {
        window.desired_maximum_frame_latency = latency;
        info!("video: frames in flight {}", latency.map_or(2, |n| n.get()));
    }
}

/// The `WindowMode` the configuration means on the primary monitor: an exclusive mode at the
/// picked size and rate where the platform mode-sets, its current mode while unpicked.
fn wanted_mode(cfg: &VideoConfig, monitor: Option<(Entity, &bevy::window::Monitor)>) -> WindowMode {
    let selection = monitor.map_or(MonitorSelection::Primary, |(e, _)| {
        MonitorSelection::Entity(e)
    });
    let exclusive = exclusive_supported().then(|| {
        cfg.resolution
            .zip(monitor)
            .and_then(|(size, (_, m))| pick_mode(m, size, cfg.refresh))
    });
    window_mode(cfg.display, cfg.maximize, exclusive, selection)
}

/// Push the display mode to the window, gated as [`apply_present_mode`] is, and again the first
/// frame the primary monitor's modes are known (the boot window cannot name a mode). An unchanged
/// mode is not re-asserted, so a restart that moved nothing leaves the display alone.
fn apply_window_mode(
    cfg: Res<VideoConfig>,
    restarts: Res<GxRestarts>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<(Entity, &bevy::window::Monitor), With<PrimaryMonitor>>,
    mut last_restart: Local<u32>,
    mut saw_monitor: Local<bool>,
) {
    let forced = std::mem::replace(&mut *last_restart, restarts.0) != restarts.0;
    let monitor = monitors.iter().next();
    let first_monitor = monitor.is_some() && !std::mem::replace(&mut *saw_monitor, true);
    if !cfg.is_changed() && !forced && !first_monitor {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let want = wanted_mode(&cfg, monitor);
    // The boot mode names `Primary` and a live one the monitor entity: compare what they set.
    let same = match (&window.mode, &want) {
        (WindowMode::Windowed, WindowMode::Windowed)
        | (WindowMode::BorderlessFullscreen(_), WindowMode::BorderlessFullscreen(_)) => true,
        (WindowMode::Fullscreen(_, a), WindowMode::Fullscreen(_, b)) => a == b,
        _ => false,
    };
    let windowed_size = cfg.windowed();
    let size_moved = want == WindowMode::Windowed
        && (
            window.resolution.physical_width(),
            window.resolution.physical_height(),
        ) != (windowed_size.x, windowed_size.y);
    if same && !size_moved {
        return;
    }
    // Leaving fullscreen hands the size back: entering it overwrote `window.resolution` with the
    // monitor's (`bevy_window`'s documented behaviour). Both writes land in one frame, as
    // `bevy_winit::changed_windows` applies `mode` before `resolution`.
    if want == WindowMode::Windowed {
        window
            .resolution
            .set_physical_resolution(windowed_size.x, windowed_size.y);
    }
    window.mode = want;
    info!(
        "video: display mode {:?}{} ({want:?})",
        cfg.display,
        if cfg.maximize && cfg.display == DisplayMode::Windowed {
            ", maximized"
        } else {
            ""
        }
    );
}

/// `gxMultisample` onto the world camera: the observer moves [`MsaaSetting`] only at the
/// `RestartGx` commit, where the reference rebuilds its device with the new sample count.
///
/// [`MsaaSetting`]: benilla_world::view::MsaaSetting
fn apply_multisample(
    msaa: Res<benilla_world::view::MsaaSetting>,
    mut cams: Query<&mut bevy::render::view::Msaa, With<benilla_world::view::WorldCamera>>,
) {
    if !msaa.is_changed() {
        return;
    }
    let want = msaa.level();
    for mut level in &mut cams {
        if *level != want {
            *level = want;
            info!("video: multisampling {}x", want.samples());
        }
    }
}

/// `windowResizeLock`, live as its callback is: the window refuses the border drag.
fn apply_resize_lock(cfg: Res<VideoConfig>, mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    if !cfg.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let resizable = !cfg.resize_lock;
    if window.resizable != resizable {
        window.resizable = resizable;
    }
}

/// `gxFixLag` as the render world reads it.
#[derive(Resource, Default)]
struct FixLag(bool);

fn extract_fix_lag(
    cfg: bevy::render::Extract<Option<Res<VideoConfig>>>,
    mut fix_lag: ResMut<FixLag>,
) {
    let on = cfg.as_ref().is_some_and(|c| c.fix_lag);
    if fix_lag.0 != on {
        fix_lag.0 = on;
    }
}

/// Reduce Input Lag: after the frame's present, wait for the GPU to finish it, as the reference
/// spins on an event query after `Present` (`CGxDeviceD3d::ScenePresent 0x6a3450`), so the CPU
/// never runs a frame ahead and the next frame's input is sampled against a drained queue.
fn fix_input_lag(fix_lag: Res<FixLag>, device: Res<bevy::render::renderer::RenderDevice>) {
    if !fix_lag.0 {
        return;
    }
    if let Err(e) = device.poll(bevy::render::render_resource::PollType::wait_indefinitely()) {
        warn_once!("video: gxFixLag wait failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synced is `Fifo`, not `AutoVsync` (`FifoRelaxed`, which tears on a late frame).
    #[test]
    fn the_default_is_synced_like_the_window_literal() {
        assert!(VideoConfig::default().vsync);
        assert_eq!(present_mode(true), PresentMode::default());
        assert_eq!(present_mode(true), PresentMode::Fifo);
    }

    /// On Metal `Immediate` takes ~1 s `nextDrawable` stalls.
    #[test]
    fn uncapped_is_autonovsync_not_immediate() {
        assert_eq!(present_mode(false), PresentMode::AutoNoVsync);
    }

    /// The reference's polarity: the row is "Windowed Mode".
    #[test]
    fn gxwindow_one_is_windowed() {
        assert_eq!(display_from_flag(0.0), DisplayMode::Fullscreen);
        assert_eq!(display_from_flag(1.0), DisplayMode::Windowed);
    }

    /// Fullscreen mode-sets where the platform can and is borderless where it cannot; a windowed,
    /// maximized window is the reference's popup over the screen (`0x58cf10`).
    #[test]
    fn the_window_mode_of_each_configuration() {
        let m = MonitorSelection::Primary;
        let mode = VideoMode {
            physical_size: UVec2::new(1920, 1080),
            bit_depth: 32,
            refresh_rate_millihertz: 60_000,
        };
        assert_eq!(
            window_mode(DisplayMode::Fullscreen, false, Some(Some(mode)), m),
            WindowMode::Fullscreen(m, VideoModeSelection::Specific(mode))
        );
        assert_eq!(
            window_mode(DisplayMode::Fullscreen, false, Some(None), m),
            WindowMode::Fullscreen(m, VideoModeSelection::Current),
            "an unpicked size keeps the desktop's mode"
        );
        assert_eq!(
            window_mode(DisplayMode::Fullscreen, false, None, m),
            WindowMode::BorderlessFullscreen(m),
            "no mode-set on this platform"
        );
        assert_eq!(
            window_mode(DisplayMode::Windowed, true, Some(None), m),
            WindowMode::BorderlessFullscreen(m)
        );
        assert_eq!(
            window_mode(DisplayMode::Windowed, false, Some(None), m),
            WindowMode::Windowed
        );
        assert_eq!(
            window_mode(DisplayMode::Fullscreen, true, None, m),
            WindowMode::BorderlessFullscreen(m),
            "fullscreen ignores maximize"
        );
    }

    fn monitor(modes: &[(u32, u32, u32, u16)]) -> bevy::window::Monitor {
        bevy::window::Monitor {
            name: None,
            physical_height: 1440,
            physical_width: 2560,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(144_000),
            scale_factor: 1.0,
            video_modes: modes
                .iter()
                .map(|&(w, h, mhz, bits)| VideoMode {
                    physical_size: UVec2::new(w, h),
                    bit_depth: bits,
                    refresh_rate_millihertz: mhz,
                })
                .collect(),
        }
    }

    /// An exact rate at the deepest bit depth, else the size's fastest; no mode at the size is
    /// none.
    #[test]
    fn the_fullscreen_mode_takes_the_picked_rate_or_the_fastest() {
        let m = monitor(&[
            (1920, 1080, 59_940, 32),
            (1920, 1080, 60_000, 16),
            (1920, 1080, 60_000, 32),
            (1920, 1080, 143_998, 32),
            (2560, 1440, 144_000, 32),
        ]);
        let size = UVec2::new(1920, 1080);
        let at = |hz| pick_mode(&m, size, hz).map(|v| (v.refresh_rate_millihertz, v.bit_depth));
        assert_eq!(
            at(60),
            Some((60_000, 32)),
            "59.94 also rounds to 60, but 60.0 is deeper"
        );
        assert_eq!(
            at(144),
            Some((143_998, 32)),
            "millihertz round to the whole Hz"
        );
        assert_eq!(
            at(75),
            Some((143_998, 32)),
            "an unoffered rate takes the fastest"
        );
        assert_eq!(pick_mode(&m, UVec2::new(800, 600), 60), None);
    }

    /// `SetupFormats`' filters: strictly over 640x480 and at least 1.248 wide; with none left, the
    /// four fallback sizes.
    #[test]
    fn the_offered_sizes_are_the_references_filter() {
        let m = monitor(&[
            (640, 480, 60_000, 32),
            (1280, 1024, 60_000, 32),
            (1024, 768, 60_000, 32),
            (1080, 1920, 60_000, 32),
            (1920, 1080, 60_000, 32),
            (1920, 1080, 144_000, 32),
        ]);
        let offered = offered_modes(Some(&m));
        let sizes: Vec<(u32, u32)> = offered
            .iter()
            .map(|o| (o.size.width, o.size.height))
            .collect();
        assert!(
            sizes.contains(&(1280, 1024)),
            "5:4 is 1.25, just over 1.248"
        );
        assert!(sizes.contains(&(1024, 768)));
        assert!(!sizes.contains(&(640, 480)), "the bounds are strict");
        assert!(!sizes.contains(&(1080, 1920)), "portrait fails the aspect");
        let fallback = offered_modes(Some(&monitor(&[(640, 480, 60_000, 32)])));
        assert_eq!(fallback.len(), SCREEN_FALLBACK.len());
        assert!(fallback.iter().all(|m| m.refresh_rates.is_empty()));
    }

    /// Triple buffering counts only under vsync, as `ISetPresentParms` does.
    #[test]
    fn triple_buffering_is_one_more_frame_in_flight_under_vsync() {
        assert_eq!(frame_latency(true, true).map(|n| n.get()), Some(3));
        assert_eq!(frame_latency(false, true), None);
        assert_eq!(frame_latency(true, false), None);
    }

    fn window_app(cfg: VideoConfig) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(cfg)
            .init_resource::<GxRestarts>()
            .add_systems(
                Update,
                (apply_present_mode, apply_window_mode, apply_resize_lock),
            );
        let win = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        (app, win)
    }

    /// Leaving fullscreen hands `gxResolution` back rather than the monitor's size, and the window
    /// is sized in physical pixels, the mode list's unit.
    #[test]
    fn the_fullscreen_round_trip_keeps_the_monitor_out_of_the_windowed_size() {
        let (mut app, win) = window_app(VideoConfig {
            resolution: Some(UVec2::new(1024, 768)),
            ..VideoConfig::default()
        });
        app.update();
        // Stand in for the compositor: fullscreen makes the resolution the monitor's. Then leave.
        app.world_mut()
            .entity_mut(win)
            .get_mut::<Window>()
            .unwrap()
            .resolution
            .set_physical_resolution(3440, 1440);
        app.world_mut().resource_mut::<VideoConfig>().display = DisplayMode::Windowed;
        app.update();
        let w = app.world().entity(win).get::<Window>().unwrap();
        assert_eq!(w.mode, WindowMode::Windowed);
        assert_eq!(
            (
                w.resolution.physical_width(),
                w.resolution.physical_height()
            ),
            (1024, 768),
            "leaving fullscreen restores gxResolution, never the monitor's size"
        );
    }

    /// The `RestartGx` commit applies a picked size to a window that is already windowed, and an
    /// unpicked one opens at the default.
    #[test]
    fn a_restart_resizes_a_window_to_the_committed_size() {
        let (mut app, win) = window_app(VideoConfig {
            display: DisplayMode::Windowed,
            ..VideoConfig::default()
        });
        app.update();
        let size = |app: &App| {
            let w = app.world().entity(win).get::<Window>().unwrap();
            (
                w.resolution.physical_width(),
                w.resolution.physical_height(),
            )
        };
        assert_eq!(size(&app), (DEFAULT_WINDOWED.x, DEFAULT_WINDOWED.y));
        app.world_mut().resource_mut::<VideoConfig>().resolution = Some(UVec2::new(1280, 720));
        app.world_mut().resource_mut::<GxRestarts>().0 += 1;
        app.update();
        assert_eq!(size(&app), (1280, 720));
    }

    /// The `RestartGx` rebuild applies a committed `gxMaximize`, and un-maximizing hands the
    /// windowed size back.
    #[test]
    fn the_restart_rebuild_applies_a_committed_maximize() {
        let (mut app, win) = window_app(VideoConfig {
            display: DisplayMode::Windowed,
            resolution: Some(UVec2::new(1024, 768)),
            ..VideoConfig::default()
        });
        app.update();
        app.world_mut().resource_mut::<VideoConfig>().maximize = true;
        app.world_mut().resource_mut::<GxRestarts>().0 += 1;
        app.update();
        assert!(matches!(
            app.world().entity(win).get::<Window>().unwrap().mode,
            WindowMode::BorderlessFullscreen(_)
        ));
        app.world_mut().resource_mut::<VideoConfig>().maximize = false;
        app.world_mut().resource_mut::<GxRestarts>().0 += 1;
        app.update();
        let w = app.world().entity(win).get::<Window>().unwrap();
        assert_eq!(w.mode, WindowMode::Windowed);
        assert_eq!(
            (
                w.resolution.physical_width(),
                w.resolution.physical_height()
            ),
            (1024, 768)
        );
    }

    /// Vsync, the frames in flight and the resize lock reach the window.
    #[test]
    fn the_presentation_rows_reach_the_window() {
        let (mut app, win) = window_app(VideoConfig::default());
        app.update();
        let window = |app: &App| app.world().entity(win).get::<Window>().unwrap().clone();
        assert!(window(&app).resizable);
        assert_eq!(window(&app).desired_maximum_frame_latency, None);
        {
            let mut cfg = app.world_mut().resource_mut::<VideoConfig>();
            cfg.vsync = true;
            cfg.triple_buffer = true;
            cfg.resize_lock = true;
        }
        app.world_mut().resource_mut::<GxRestarts>().0 += 1;
        app.update();
        assert!(
            !window(&app).resizable,
            "windowResizeLock refuses the border drag"
        );
        assert_eq!(
            window(&app).desired_maximum_frame_latency.map(|n| n.get()),
            Some(3)
        );
        app.world_mut().resource_mut::<VideoConfig>().vsync = false;
        app.update();
        assert_eq!(window(&app).present_mode, PresentMode::AutoNoVsync);
        assert_eq!(
            window(&app).desired_maximum_frame_latency,
            None,
            "no second back buffer without vsync"
        );
    }

    #[test]
    fn gxresolution_parses_the_reference_spelling() {
        assert_eq!(parse_resolution("1280x800"), Some(UVec2::new(1280, 800)));
        assert_eq!(parse_resolution("1600X900"), Some(UVec2::new(1600, 900)));
        assert_eq!(parse_resolution("1024 x 768"), Some(UVec2::new(1024, 768)));
        assert_eq!(parse_resolution("0x600"), None);
        assert_eq!(parse_resolution("1280x"), None);
        assert_eq!(parse_resolution(""), None);
    }
}
