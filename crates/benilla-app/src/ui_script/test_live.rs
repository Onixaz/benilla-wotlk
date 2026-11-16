//! A VM with a fixed live view lent for its whole length, for the dialog tests: the death and
//! summon verbs read what the test sets, as the app's verbs read the `World`.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

use benilla_ui::script::{LiveLoan, LiveState, UiScript};

/// The death answers a test sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct DeathUiState {
    /// Seconds left on the release timer; `None` is the no-timer flag (-1).
    pub release_remaining: Option<f32>,
    /// Seconds until the corpse is reclaimable.
    pub recovery_delay: f32,
    pub resurrect_sickness: bool,
    pub resurrect_has_timer: bool,
    pub spirit_healer_in_range: bool,
    pub sickness_minutes: Option<u32>,
    pub self_res_label: Option<String>,
    pub falling: bool,
    pub out_of_bounds: bool,
    pub cannot_be_resurrected: bool,
}

/// The summon answers a test sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SummonConfirmUiState {
    pub summoner: String,
    pub area: String,
    pub time_left_ms: u32,
}

thread_local! {
    static DEATH: RefCell<DeathUiState> = RefCell::default();
    static SUMMON: RefCell<SummonConfirmUiState> = RefCell::default();
}

/// The view over this thread's set answers.
pub(super) struct View;

impl LiveState for View {
    fn unit_flags(&self, _: u64) -> Option<u32> {
        None
    }

    fn unit_health(&self, _: u64) -> Option<u32> {
        None
    }

    fn active_player(&self) -> Option<u64> {
        None
    }

    fn has_control(&self) -> bool {
        true
    }

    fn far_sight(&self) -> bool {
        false
    }

    fn release_time_left_ms(&self) -> Option<u32> {
        DEATH.with(|d| {
            d.borrow()
                .release_remaining
                .map(|s| (s.max(0.0) * 1000.0) as u32)
        })
    }

    fn corpse_recovery_delay_ms(&self) -> u32 {
        DEATH.with(|d| (d.borrow().recovery_delay.max(0.0) * 1000.0) as u32)
    }

    fn resurrect_sickness(&self) -> bool {
        DEATH.with(|d| d.borrow().resurrect_sickness)
    }

    fn resurrect_has_timer(&self) -> bool {
        DEATH.with(|d| d.borrow().resurrect_has_timer)
    }

    fn spirit_healer_in_range(&self) -> bool {
        DEATH.with(|d| d.borrow().spirit_healer_in_range)
    }

    fn res_sickness_minutes(&self) -> Option<u32> {
        DEATH.with(|d| d.borrow().sickness_minutes)
    }

    fn self_res_label(&self) -> Option<String> {
        DEATH.with(|d| d.borrow().self_res_label.clone())
    }

    fn falling(&self) -> bool {
        DEATH.with(|d| d.borrow().falling)
    }

    fn out_of_bounds(&self) -> bool {
        DEATH.with(|d| d.borrow().out_of_bounds)
    }

    fn cannot_be_resurrected(&self) -> bool {
        DEATH.with(|d| d.borrow().cannot_be_resurrected)
    }

    fn summon_summoner(&self) -> String {
        SUMMON.with(|s| s.borrow().summoner.clone())
    }

    fn summon_area(&self) -> String {
        SUMMON.with(|s| s.borrow().area.clone())
    }

    fn summon_time_left_ms(&self) -> u32 {
        SUMMON.with(|s| s.borrow().time_left_ms)
    }
}

/// A VM with [`View`] lent until it drops.
pub(super) struct Lent {
    script: UiScript,
    _loan: LiveLoan<View>,
}

impl Lent {
    pub(super) fn new(script: UiScript) -> Self {
        DEATH.with(|d| *d.borrow_mut() = DeathUiState::default());
        SUMMON.with(|s| *s.borrow_mut() = SummonConfirmUiState::default());
        Self {
            script,
            // SAFETY: dropped with the wrapper, on this test's thread, and never leaked.
            _loan: unsafe { LiveLoan::new(View) },
        }
    }

    pub(super) fn set_death(&mut self, state: DeathUiState) {
        DEATH.with(|d| *d.borrow_mut() = state);
    }

    pub(super) fn set_summon_confirm(&mut self, state: SummonConfirmUiState) {
        SUMMON.with(|s| *s.borrow_mut() = state);
    }
}

impl Deref for Lent {
    type Target = UiScript;

    fn deref(&self) -> &UiScript {
        &self.script
    }
}

impl DerefMut for Lent {
    fn deref_mut(&mut self) -> &mut UiScript {
        &mut self.script
    }
}
