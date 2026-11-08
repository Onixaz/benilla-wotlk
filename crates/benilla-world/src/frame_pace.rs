//! Frame pacing: under a vsync present mode `Time<Virtual>` advances on the presentation cadence,
//! not the CPU loop. Deviates from 12340, whose frame delta is the plain difference of millisecond
//! timestamps (`0x47f0ef`), for a measurement artifact of ours: under `PresentMode::Fifo` frames
//! are shown on refresh boundaries, but `Time<Real>` is stamped where the main thread waits on the
//! render thread, so two frames shown 16.67 ms apart can measure 26.4 and 8.0 ms, and every bone
//! of a standing model jitters at once. When recent deltas look vsync'd, each is snapped to a
//! whole number of refresh periods and the remainder carried, so elapsed time still matches the
//! wall clock; a real stall passes through. An uncapped stream has no refresh to snap to and is
//! the reference's delta exactly, however steady it is.
//!
//! The frame clock has no clamp (`0x47f0ef`, `0x77ecb0`): a long frame reaches every timer whole.
//! `Time<Real>` stays unpaced because probes read it and a stall must stay visible there.
//! `WOW_NO_FRAME_PACE=1` turns pacing off, and a manual `TimeUpdateStrategy` is never paced.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::prelude::*;
use bevy::time::{Real, TimeSystems, TimeUpdateStrategy, Virtual};
use bevy::window::{PresentMode, PrimaryWindow};

/// Recent raw deltas the refresh-period median is taken over, about a third of a second at 60 Hz.
const WINDOW: usize = 20;
/// A sample counts as "on cadence" when it is within this fraction of the median.
const TIGHT: f64 = 0.15;
/// Pacing engages only when at least this share of the window is on cadence.
const TIGHT_SHARE: f64 = 0.6;
/// The largest remainder, in periods, still worked off in whole refreshes; beyond it the frame is
/// a stall and passes through raw.
const CARRY_MAX: f64 = 1.5;
/// The largest multiple snapped to; a longer frame is a stall, not a mis-split.
const MAX_MULT: f64 = 4.0;

#[derive(Resource)]
pub(crate) struct FramePacer {
    /// Recent raw deltas (seconds), for the refresh-period median.
    hist: VecDeque<f64>,
    /// Time measured but not yet handed out, within [`CARRY_MAX`] periods.
    carry: f64,
    /// What `Time<Virtual>::elapsed` is rewritten to; `None` until the first paced frame adopts
    /// Bevy's own, so the clock never jumps.
    total: Option<Duration>,
    /// `WOW_NO_FRAME_PACE=1`.
    off: bool,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self {
            hist: VecDeque::with_capacity(WINDOW),
            carry: 0.0,
            total: None,
            off: std::env::var("WOW_NO_FRAME_PACE").as_deref() == Ok("1"),
        }
    }
}

impl FramePacer {
    /// The refresh period this stream looks like, or `None` when it does not look vsync'd.
    fn cadence(&self) -> Option<f64> {
        if self.hist.len() < WINDOW {
            return None;
        }
        let mut v: Vec<f64> = self.hist.iter().copied().collect();
        v.sort_by(f64::total_cmp);
        let period = v[v.len() / 2];
        if period <= 0.0 {
            return None;
        }
        let tight = v
            .iter()
            .filter(|d| (*d - period).abs() < TIGHT * period)
            .count();
        (tight as f64 >= TIGHT_SHARE * v.len() as f64).then_some(period)
    }

    /// Snaps `raw` to the presentation cadence, carrying the remainder, or returns it unchanged
    /// for a stream that is not vsync'd and for a stall. One call is one presented frame, so the
    /// multiple is 1 until `raw` plus the carry reaches 1.5 periods, then a floor: rounding would
    /// pace the 26.4 ms long half of a mis-split pair (1.58 periods) as two frames.
    fn pace(&mut self, raw: f64) -> f64 {
        if self.hist.len() == WINDOW {
            self.hist.pop_front();
        }
        self.hist.push_back(raw);
        let Some(period) = self.cadence() else {
            self.carry = 0.0;
            return raw;
        };
        let want = raw + self.carry;
        if want < 0.0 {
            // Ahead of the wall clock by more than this frame is worth: stop inventing time.
            self.carry = 0.0;
            return raw;
        }
        let mult = if want >= 1.5 * period {
            (want / period).floor().min(MAX_MULT)
        } else {
            1.0
        };
        let snapped = mult * period;
        let carry = want - snapped;
        if carry.abs() > CARRY_MAX * period {
            // A real stall, not a mis-split: report it as it happened.
            self.carry = 0.0;
            return raw;
        }
        self.carry = carry;
        snapped
    }
}

/// Whether `mode` holds each present to a refresh, the only stream with a cadence to restore.
/// `Mailbox` replaces the queued frame without waiting, so its loop runs uncapped.
fn holds_to_refresh(mode: PresentMode) -> bool {
    matches!(
        mode,
        PresentMode::Fifo | PresentMode::FifoRelaxed | PresentMode::AutoVsync
    )
}

/// Rewrites `Time<Virtual>` and the generic `Time` so this frame's delta is the paced one; runs
/// right after Bevy's clock tick. Bevy has no delta setter, so the clock is rebuilt with its
/// context and wrap period, and two `advance_by` calls place `elapsed` and `delta`. Paused or
/// scaled virtual time passes through, since `Virtual`'s `effective_speed` is private.
pub(crate) fn pace_virtual_time(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    mut generic: ResMut<Time>,
    mut pacer: ResMut<FramePacer>,
    strategy: Option<Res<TimeUpdateStrategy>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    // A manual clock (the capture harness, deterministic tests) must reproduce frame for frame.
    let manual = matches!(
        strategy.as_deref(),
        Some(TimeUpdateStrategy::ManualInstant(_) | TimeUpdateStrategy::ManualDuration(_))
    );
    // The window's own mode, which a `RestartGx()` and the capture probes both write.
    let vsync = window
        .single()
        .is_ok_and(|w| holds_to_refresh(w.present_mode));
    if pacer.off || manual || !vsync || virt.is_paused() || virt.relative_speed_f64() != 1.0 {
        pacer.total = None;
        pacer.carry = 0.0;
        // Deltas measured off the cadence must not vote on the next period.
        pacer.hist.clear();
        return;
    }
    let paced = Duration::from_secs_f64(pacer.pace(real.delta().as_secs_f64()));
    let total = pacer.total.unwrap_or(virt.elapsed()) + paced;
    pacer.total = Some(total);

    let ctx = *virt.context();
    let wrap = virt.wrap_period();
    let mut t = Time::<Virtual>::default();
    *t.context_mut() = ctx;
    t.set_wrap_period(wrap);
    t.advance_by(total.saturating_sub(paced));
    t.advance_by(paced);
    *virt = t;
    *generic = virt.as_generic();
}

/// The most physics catch-up one frame runs: the collision step has no reference clock to follow.
const MAX_FIXED_CATCH_UP: Duration = Duration::from_millis(250);

/// Takes Bevy's 250 ms cap off the virtual clock.
fn lift_max_delta(mut virt: ResMut<Time<Virtual>>) {
    virt.set_max_delta(Duration::MAX);
}

/// Drops the fixed-step backlog past [`MAX_FIXED_CATCH_UP`], so a long frame bursts no steps.
fn cap_fixed_catch_up(mut fixed: ResMut<Time<Fixed>>) {
    let over = fixed.overstep().saturating_sub(MAX_FIXED_CATCH_UP);
    if !over.is_zero() {
        fixed.discard_overstep(over);
    }
}

/// `WOW_FIXED_DT`'s pending arm (see [`plugin`]).
#[derive(Resource)]
struct FixedDt {
    dt: Duration,
    at: std::time::Instant,
    armed: bool,
}

/// Installs the manual strategy once the wall clock passes the arm time, and logs it.
fn arm_fixed_dt(mut cfg: ResMut<FixedDt>, mut commands: Commands) {
    if cfg.armed || std::time::Instant::now() < cfg.at {
        return;
    }
    cfg.armed = true;
    commands.insert_resource(TimeUpdateStrategy::ManualDuration(cfg.dt));
    info!(
        "frame_pace: WOW_FIXED_DT armed — animation clock pinned to {:.4} ms/frame",
        cfg.dt.as_secs_f64() * 1000.0
    );
}

pub fn plugin(app: &mut App) {
    // `WOW_FIXED_DT=<ms>[,<arm_after_wall_seconds>]` (default 40 s) pins the animation step, so a
    // per-frame screenshot burst, whose PNG saves stall every frame, measures the render's
    // evenness and not the clock's; [`pace_virtual_time`] stands down under it. The arm delay is
    // required: a manual strategy also drives `Time<Real>`, which probe triggers read, and during
    // an unthrottled load it would race far ahead of the wall before the character streams in.
    if let Some((ms, after)) = std::env::var("WOW_FIXED_DT").ok().and_then(|v| {
        let (ms, after) = match v.split_once(',') {
            Some((a, b)) => (a, b.trim().parse::<f64>().ok()?),
            None => (v.as_str(), 40.0),
        };
        Some((ms.trim().parse::<f64>().ok().filter(|m| *m > 0.0)?, after))
    }) {
        app.insert_resource(FixedDt {
            dt: Duration::from_secs_f64(ms / 1000.0),
            at: std::time::Instant::now() + Duration::from_secs_f64(after.max(0.0)),
            armed: false,
        })
        .add_systems(First, arm_fixed_dt.before(TimeSystems));
    }
    app.init_resource::<FramePacer>()
        .add_systems(Startup, lift_max_delta)
        .add_systems(First, pace_virtual_time.after(TimeSystems))
        .add_systems(FixedFirst, cap_fixed_catch_up);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pacer() -> FramePacer {
        FramePacer {
            off: false,
            ..Default::default()
        }
    }

    #[derive(Resource, Default)]
    struct FixedSteps(u32);

    /// One 4 s frame after a normal one, as the login UI load stalls.
    fn long_frame() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .init_resource::<FixedSteps>()
            .add_systems(FixedUpdate, |mut n: ResMut<FixedSteps>| n.0 += 1);
        plugin(&mut app);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            10,
        )));
        app.update();
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(4)));
        app.world_mut().resource_mut::<FixedSteps>().0 = 0;
        app.update();
        app
    }

    #[test]
    fn a_long_frame_reaches_the_frame_clock_whole() {
        let app = long_frame();
        assert_eq!(
            app.world().resource::<Time<Virtual>>().delta(),
            Duration::from_secs(4)
        );
        assert_eq!(
            app.world().resource::<Time>().delta(),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn a_long_frame_runs_no_burst_of_fixed_steps() {
        let app = long_frame();
        let steps = app.world().resource::<FixedSteps>().0;
        let per_catch_up = (MAX_FIXED_CATCH_UP.as_secs_f64() * 64.0).ceil() as u32;
        assert!(
            steps <= per_catch_up + 2,
            "{steps} steps, the catch-up is {per_catch_up}"
        );
    }

    #[test]
    fn a_mis_split_pair_is_restored_to_the_cadence_the_display_showed() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        // A measured split of two on-cadence frames.
        let a = p.pace(0.0264);
        let b = p.pace(0.0080);
        assert!(
            (a - period).abs() < 1.0e-6,
            "long half snapped to one period: {a}"
        );
        assert!(
            (b - period).abs() < 1.0e-6,
            "short half snapped to one period: {b}"
        );
        assert!(
            ((a + b) - (0.0264 + 0.0080)).abs() <= period,
            "no time invented or lost beyond one period of carry"
        );
    }

    #[test]
    fn snapping_redistributes_time_it_never_creates_or_destroys_it() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        let mut raw_total = 0.0;
        let mut paced_total = 0.0;
        // Long/short jitter around the cadence, the measured shape.
        for i in 0..1000 {
            let raw = match i % 4 {
                0 => period * 1.55,
                1 => period * 0.45,
                2 => period * 0.80,
                _ => period * 1.20,
            };
            raw_total += raw;
            paced_total += p.pace(raw);
        }
        assert!(
            (raw_total - paced_total).abs() < period,
            "drift {:.6}s over 1000 frames must stay under one period",
            raw_total - paced_total
        );
    }

    #[test]
    fn a_dropped_frame_is_paced_as_two_periods_not_one() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        let d = p.pace(2.0 * period);
        assert!(
            (d - 2.0 * period).abs() < 1.0e-6,
            "a dropped frame keeps both refreshes: {d}"
        );
    }

    #[test]
    fn a_genuine_stall_passes_through_unpaced() {
        let mut p = pacer();
        let period = 1.0 / 60.0;
        for _ in 0..WINDOW {
            p.pace(period);
        }
        let stall = 1.82; // a measured streaming stall
        assert_eq!(p.pace(stall), stall, "a stall is reported as it happened");
    }

    #[test]
    fn a_stream_with_no_cadence_is_left_alone() {
        let mut p = pacer();
        // Too uneven for any period to hold 60% of the window.
        let raws = [0.004, 0.011, 0.0031, 0.019, 0.0072, 0.0155, 0.0028, 0.0093];
        for i in 0..WINDOW * 2 {
            let raw = raws[i % raws.len()];
            assert_eq!(p.pace(raw), raw, "no cadence ⇒ no snapping");
        }
    }

    use bevy::ecs::system::RunSystemOnce;

    /// Steady enough for [`FramePacer::cadence`] (7 of 8 within 15% of 4 ms), and skewed long as
    /// real frame times are: the stream the pacer used to mistake for vsync.
    const STEADY_US: [u64; 8] = [4000, 4100, 3900, 4000, 4200, 3800, 4000, 5500];
    /// A 60 Hz refresh, whole microseconds so a delta compares exactly.
    const REFRESH_US: u64 = 16667;

    /// The three clocks and a primary window in `mode`, past the real clock's zero first update.
    fn clocks(mode: PresentMode) -> World {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<Time<Virtual>>();
        world.init_resource::<Time>();
        world.insert_resource(pacer());
        world.spawn((
            Window {
                present_mode: mode,
                ..default()
            },
            PrimaryWindow,
        ));
        world
            .resource_mut::<Time<Real>>()
            .update_with_duration(Duration::ZERO);
        world
    }

    /// One frame of `us` microseconds through Bevy's clocks and the pacer; the delta it left.
    fn frame(world: &mut World, us: u64) -> Duration {
        let raw = Duration::from_micros(us);
        world.resource_mut::<Time<Real>>().update_with_duration(raw);
        world.resource_mut::<Time<Virtual>>().advance_by(raw);
        // Bevy's own tick ends with the generic clock on the virtual one.
        let generic = world.resource::<Time<Virtual>>().as_generic();
        *world.resource_mut::<Time>() = generic;
        world.run_system_once(pace_virtual_time).unwrap();
        let delta = world.resource::<Time<Virtual>>().delta();
        assert_eq!(world.resource::<Time>().delta(), delta, "generic follows");
        delta
    }

    fn set_mode(world: &mut World, mode: PresentMode) {
        world
            .query::<&mut Window>()
            .single_mut(world)
            .unwrap()
            .present_mode = mode;
    }

    #[test]
    fn a_steady_uncapped_stream_reaches_the_clock_exactly_as_measured() {
        for mode in [
            PresentMode::AutoNoVsync,
            PresentMode::Immediate,
            PresentMode::Mailbox,
        ] {
            let mut world = clocks(mode);
            for i in 0..WINDOW * 10 {
                let us = STEADY_US[i % STEADY_US.len()];
                assert_eq!(frame(&mut world, us), Duration::from_micros(us), "{mode:?}");
            }
        }
    }

    #[test]
    fn the_same_stream_is_snapped_under_vsync() {
        for mode in [
            PresentMode::Fifo,
            PresentMode::FifoRelaxed,
            PresentMode::AutoVsync,
        ] {
            let mut world = clocks(mode);
            let changed = (0..WINDOW * 10)
                .filter(|i| {
                    let us = STEADY_US[i % STEADY_US.len()];
                    frame(&mut world, us) != Duration::from_micros(us)
                })
                .count();
            assert!(changed > WINDOW, "{mode:?} paced {changed} frames");
        }
    }

    #[test]
    fn no_window_is_no_vsync() {
        let mut world = clocks(PresentMode::Fifo);
        let window = world
            .query_filtered::<Entity, With<Window>>()
            .single(&world);
        world.despawn(window.unwrap());
        for i in 0..WINDOW * 2 {
            let us = STEADY_US[i % STEADY_US.len()];
            assert_eq!(frame(&mut world, us), Duration::from_micros(us));
        }
    }

    /// A world under `Fifo` that has settled on [`REFRESH_US`].
    fn settled_on_refresh() -> World {
        let mut world = clocks(PresentMode::Fifo);
        for _ in 0..=WINDOW {
            frame(&mut world, REFRESH_US);
        }
        world
    }

    #[test]
    fn vsync_restores_a_mis_split_pair_through_the_clock() {
        let mut world = settled_on_refresh();
        let period = Duration::from_micros(REFRESH_US);
        assert_eq!(frame(&mut world, 26400), period, "long half");
        assert_eq!(frame(&mut world, 8000), period, "short half");
    }

    #[test]
    fn vsync_keeps_both_refreshes_of_a_dropped_frame() {
        let mut world = settled_on_refresh();
        assert_eq!(
            frame(&mut world, 2 * REFRESH_US),
            Duration::from_micros(2 * REFRESH_US)
        );
    }

    #[test]
    fn vsync_reports_a_stall_as_it_happened() {
        let mut world = settled_on_refresh();
        assert_eq!(
            frame(&mut world, 1_820_000),
            Duration::from_micros(1_820_000)
        );
    }

    #[test]
    fn turning_vsync_off_and_on_starts_the_cadence_over() {
        let mut world = clocks(PresentMode::Fifo);
        for i in 0..WINDOW * 2 {
            frame(&mut world, STEADY_US[i % STEADY_US.len()]);
        }
        set_mode(&mut world, PresentMode::AutoNoVsync);
        let us = STEADY_US[1];
        assert_eq!(frame(&mut world, us), Duration::from_micros(us), "off");
        set_mode(&mut world, PresentMode::Fifo);
        // The window refills from scratch; its last sample is the one that engages.
        for i in 0..WINDOW - 1 {
            let us = STEADY_US[i % STEADY_US.len()];
            assert_eq!(
                frame(&mut world, us),
                Duration::from_micros(us),
                "frame {i}"
            );
        }
    }
}
