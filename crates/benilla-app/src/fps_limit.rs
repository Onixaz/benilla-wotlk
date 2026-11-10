//! The graphics CVar frame limiter (12340, 0x6836d0): one sleep per frame, no spin wait.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::cvars::CvarChanged;

#[derive(Resource)]
pub(crate) struct FrameCaps {
    foreground: u32,
    background: u32,
}

impl Default for FrameCaps {
    fn default() -> Self {
        Self {
            foreground: 200,
            background: 30,
        }
    }
}

impl FrameCaps {
    fn interval(&self, focused: bool) -> Option<Duration> {
        // Zero becomes UINT_MAX before the unsigned minimum and the lower bound of eight.
        let unlimited = |n| if n == 0 { u32::MAX } else { n };
        let mut fps = unlimited(self.foreground);
        if !focused {
            fps = fps.min(unlimited(self.background));
        }
        (fps != u32::MAX).then(|| Duration::from_millis(u64::from(1000 / fps.max(8))))
    }
}

pub(crate) fn on_cvar(event: On<CvarChanged>, mut caps: ResMut<FrameCaps>) {
    // The reference reads the CVar's cached integer, with an unsigned comparison in the limiter.
    let value = event.num() as i32 as u32;
    match event.key().as_str() {
        "maxfps" => caps.foreground = value,
        "maxfpsbk" => caps.background = value,
        _ => return,
    }
    info!("frame limiter: {} = {}", event.name, event.new);
}

#[derive(Default)]
struct FrameDeadline(Option<Instant>);

impl FrameDeadline {
    fn remaining(&self, now: Instant) -> Duration {
        self.0
            .map_or(Duration::ZERO, |due| due.saturating_duration_since(now))
    }

    fn advance(&mut self, now: Instant, interval: Duration) {
        // Schedule from the wake time: a late frame never creates a catch-up burst (0x68374a).
        self.0 = Some(now + interval);
    }
}

/// Deviation: an instrumented run measures the client, not the cap, and its window opens
/// unfocused ([`benilla_world::bgwin`]), so the limiter stands down there.
#[derive(Resource)]
struct Instrumented(bool);

pub(crate) struct FpsLimitPlugin;

impl Plugin for FpsLimitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameCaps>()
            .insert_resource(Instrumented(benilla_world::bgwin::background_run()))
            // Winit's default background 60 Hz rail must not override maxFPSBk, including zero.
            .insert_resource(bevy::winit::WinitSettings::continuous())
            // NonSend keeps the sleep on the main thread, including with a parallel executor.
            .insert_non_send_resource(FrameDeadline::default())
            .add_observer(on_cvar)
            .add_systems(Last, limit_frame);
    }
}

fn limit_frame(
    windows: Query<&Window, With<PrimaryWindow>>,
    caps: Res<FrameCaps>,
    instrumented: Res<Instrumented>,
    mut deadline: NonSendMut<FrameDeadline>,
) {
    let interval = windows
        .single()
        .ok()
        .filter(|_| !instrumented.0)
        .and_then(|w| caps.interval(w.focused));
    let Some(interval) = interval else {
        deadline.0 = None;
        return;
    };
    let wait = deadline.remaining(Instant::now());
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
    deadline.advance(Instant::now(), interval);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_defaults_and_background_selection() {
        let mut caps = FrameCaps::default();
        assert_eq!(caps.interval(true), Some(Duration::from_millis(5)));
        assert_eq!(caps.interval(false), Some(Duration::from_millis(33)));
        caps.foreground = 20;
        assert_eq!(caps.interval(false), Some(Duration::from_millis(50)));
        caps.background = 60;
        assert_eq!(caps.interval(false), caps.interval(true));
    }

    #[test]
    fn uncapped_and_minimum_rate_are_applied_after_selection() {
        let mut caps = FrameCaps {
            foreground: 0,
            background: 30,
        };
        assert_eq!(caps.interval(true), None);
        assert_eq!(caps.interval(false), Some(Duration::from_millis(33)));
        caps.background = 0;
        assert_eq!(caps.interval(false), None);
        caps.foreground = 60;
        assert_eq!(caps.interval(false), Some(Duration::from_millis(16)));
        for value in 1..=8 {
            caps.foreground = value;
            assert_eq!(caps.interval(true), Some(Duration::from_millis(125)));
        }
        caps.foreground = 200;
        caps.background = 1;
        assert_eq!(caps.interval(false), Some(Duration::from_millis(125)));
    }

    #[test]
    fn deadlines_sleep_only_the_remainder_and_do_not_catch_up_after_a_stall() {
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        let mut deadline = FrameDeadline::default();
        assert_eq!(deadline.remaining(start), Duration::ZERO);
        deadline.advance(start, interval);
        assert_eq!(
            deadline.remaining(start + Duration::from_millis(6)),
            Duration::from_millis(10)
        );
        let late = start + Duration::from_secs(1);
        assert_eq!(deadline.remaining(late), Duration::ZERO);
        deadline.advance(late, interval);
        assert_eq!(deadline.remaining(late), interval);
    }

    #[test]
    fn cvar_changes_reach_the_cached_caps_and_archive() {
        use crate::cvars::{Cvars, Scope};
        let mut app = App::new();
        app.init_resource::<Cvars>().add_plugins(FpsLimitPlugin);
        let mut cvars = app.world_mut().resource_mut::<Cvars>();
        assert_eq!(cvars.get("maxFPS"), Some("200"));
        assert_eq!(cvars.get("maxFPSBk"), Some("30"));
        cvars.set("maxFPS", "60");
        cvars.set("maxFPSBk", "15");
        let events = cvars.take_events();
        for event in events {
            app.world_mut().trigger(event);
        }
        assert_eq!(app.world().resource::<FrameCaps>().foreground, 60);
        assert_eq!(app.world().resource::<FrameCaps>().background, 15);
        assert!(crate::console::execute(app.world_mut(), "maxfps 0").is_empty());
        assert_eq!(app.world().resource::<FrameCaps>().interval(true), None);
        let archive = app.world().resource::<Cvars>().compose_scope(Scope::Config);
        assert!(archive.contains("SET maxFPS \"0\""));
        assert!(archive.contains("SET maxFPSBk \"15\""));
    }

    #[test]
    fn plugin_paces_frames_and_releases_the_deadline_when_uncapped() {
        let mut app = App::new();
        app.add_plugins(FpsLimitPlugin);
        assert_eq!(
            app.world()
                .resource::<bevy::winit::WinitSettings>()
                .unfocused_mode,
            bevy::winit::UpdateMode::Continuous,
        );
        // The test runner's own environment may name an instrumented run.
        app.insert_resource(Instrumented(false));
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.world_mut().resource_mut::<FrameCaps>().foreground = 50;
        let start = Instant::now();
        app.update();
        for _ in 0..3 {
            app.update();
        }
        assert!(start.elapsed() >= Duration::from_millis(60));
        app.world_mut().resource_mut::<FrameCaps>().foreground = 0;
        app.update();
        assert!(app.world().non_send_resource::<FrameDeadline>().0.is_none());
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        app.update();
        assert!(app.world().non_send_resource::<FrameDeadline>().0.is_some());
        app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        app.update();
        assert!(app.world().non_send_resource::<FrameDeadline>().0.is_none());
    }

    #[test]
    fn an_instrumented_run_is_never_capped() {
        let mut app = App::new();
        app.add_plugins(FpsLimitPlugin)
            .insert_resource(Instrumented(true));
        // A probe window: on top, never focused, so the stock default would be maxFPSBk.
        app.world_mut().spawn((
            Window {
                focused: false,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut().resource_mut::<FrameCaps>().background = 8;
        for _ in 0..4 {
            app.update();
            assert!(app.world().non_send_resource::<FrameDeadline>().0.is_none());
        }
    }
}
