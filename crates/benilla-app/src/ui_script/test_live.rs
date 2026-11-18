//! A VM with a fixed live view lent for its whole length, for the dialog tests: the death and
//! summon verbs read what the test sets, as the app's verbs read the `World`.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

use benilla_ui::script::{
    CompanionState, CompanionType, CompanionView, LiveLoan, LiveState, PetActionView, PetBarLive,
    PetBookState, PetStats, PossessView, StableCallback, StableFamily, StableList, UiScript,
};

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

/// The pet and companion answers a test sets.
#[derive(Clone, Debug, Default)]
struct PetAnswers {
    stats: (bool, PetStats),
    menu: Option<(bool, bool, bool)>,
    companions: CompanionState,
    possess: Option<PossessView>,
    stable: StableList,
    /// The stable's creature records, by entry.
    stable_families: std::collections::HashMap<u32, StableFamily>,
    /// The pet bar's bar-wide state, `None` for no pet info.
    pet_bar: Option<PetBarLive>,
    /// The pet bar's slots, which the drag writes.
    pet_slots: Vec<PetActionView>,
    /// The pet's book.
    pet_book: Option<PetBookState>,
    /// The verbs' writes inside their calls, in order.
    pet_writes: PetWrites,
}

/// What the pet verbs wrote inside their calls, as the live view sends it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PetWrites {
    /// The drag's packets, one or two `(0-based slot, word)` pairs each.
    pub placed: Vec<Vec<(u32, u32)>>,
    /// The 0-based slots the bar toggle reached past its gate.
    pub toggled: Vec<u32>,
    /// The book's sets that flipped, `(spell, state)`.
    pub spell_autocasts: Vec<(u32, bool)>,
    /// `PetStopAttack` calls.
    pub stops: u32,
}

thread_local! {
    static DEATH: RefCell<DeathUiState> = RefCell::default();
    static SUMMON: RefCell<SummonConfirmUiState> = RefCell::default();
    static PET: RefCell<PetAnswers> = RefCell::default();
    /// The view lent for the rest of the thread once a pet answer is set.
    static STICKY: RefCell<Option<LiveLoan<View>>> = const { RefCell::new(None) };
}

/// Set pet and companion answers on a bare VM, as the app's live view answers them: the first call lends [`View`] for the rest of the test's thread.
pub(crate) trait PetAnswersExt {
    /// The pet bar as the live view answers it: `has_bar`, `actions_usable`, `pickup_allowed`, and the slots, the attacking global raised by a slot showing it.
    fn set_pet_actions(
        &mut self,
        has_bar: bool,
        actions_usable: bool,
        pickup_allowed: bool,
        slots: Vec<PetActionView>,
    );
    fn set_pet_stats(&mut self, has_ui: bool, stats: PetStats);
    fn set_pet_menu(&mut self, abandon: bool, rename: bool, dismiss: bool);
    fn set_companions(&mut self, state: CompanionState);
    fn set_possess_spell(&mut self, possess: Option<PossessView>);
    fn set_stable(&mut self, list: StableList, families: Vec<(u32, StableFamily)>);
    fn set_pet_book(&mut self, book: PetBookState);
    /// What the pet verbs wrote inside their calls since the last take.
    fn take_pet_writes(&mut self) -> PetWrites;
}

/// Lend [`View`] for the rest of the thread; the first loan must enclose every later one, so no other loan may be up when it is made.
fn sticky(script: &UiScript) {
    STICKY.with(|s| {
        if s.borrow().is_none() {
            assert!(
                !script.has_live(),
                "the sticky loan must be made outside every other loan"
            );
            // SAFETY: held until the thread ends, made before any loan a later call nests in it.
            *s.borrow_mut() = Some(unsafe { LiveLoan::new(View) });
        }
    });
}

impl PetAnswersExt for UiScript {
    fn set_pet_actions(
        &mut self,
        has_bar: bool,
        actions_usable: bool,
        pickup_allowed: bool,
        slots: Vec<PetActionView>,
    ) {
        sticky(self);
        PET.with(|p| {
            let mut p = p.borrow_mut();
            p.pet_bar = Some(PetBarLive {
                has_bar,
                actions_usable,
                pickup_allowed,
                attacking: slots.iter().any(|s| s.attack_active),
                time_remaining_ms: None,
            });
            p.pet_slots = if has_bar { slots } else { Vec::new() };
        });
    }

    fn set_pet_book(&mut self, book: PetBookState) {
        sticky(self);
        PET.with(|p| p.borrow_mut().pet_book = Some(book));
    }

    fn take_pet_writes(&mut self) -> PetWrites {
        PET.with(|p| std::mem::take(&mut p.borrow_mut().pet_writes))
    }

    fn set_pet_stats(&mut self, has_ui: bool, stats: PetStats) {
        sticky(self);
        PET.with(|p| p.borrow_mut().stats = (has_ui, stats));
    }

    fn set_pet_menu(&mut self, abandon: bool, rename: bool, dismiss: bool) {
        sticky(self);
        PET.with(|p| p.borrow_mut().menu = Some((abandon, rename, dismiss)));
    }

    fn set_companions(&mut self, state: CompanionState) {
        sticky(self);
        PET.with(|p| p.borrow_mut().companions = state);
    }

    fn set_possess_spell(&mut self, possess: Option<PossessView>) {
        sticky(self);
        PET.with(|p| p.borrow_mut().possess = possess);
    }

    fn set_stable(&mut self, list: StableList, families: Vec<(u32, StableFamily)>) {
        sticky(self);
        PET.with(|p| {
            let mut p = p.borrow_mut();
            p.stable = list;
            p.stable_families = families.into_iter().collect();
        });
    }
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

    fn companion_spells(&self, ty: CompanionType) -> Vec<u32> {
        PET.with(|p| {
            let p = p.borrow();
            p.companions.list(ty).iter().map(|c| c.spell_id).collect()
        })
    }

    fn companion(&self, ty: CompanionType, index: usize) -> Option<CompanionView> {
        PET.with(|p| p.borrow().companions.list(ty).get(index).cloned())
    }

    fn companion_random_pool(&self) -> Vec<u32> {
        PET.with(|p| p.borrow().companions.random_pool.clone())
    }

    fn pet_stats(&self) -> (bool, PetStats) {
        PET.with(|p| p.borrow().stats.clone())
    }

    fn pet_menu(&self) -> (bool, bool, bool) {
        PET.with(|p| p.borrow().menu.unwrap_or((false, false, true)))
    }

    /// Any unit asked answers the pet's family, as the tests name only the pet.
    fn creature_family(&self, _: u64) -> Option<String> {
        PET.with(|p| p.borrow().stats.1.family.clone())
    }

    fn summon_time_left_ms(&self) -> u32 {
        SUMMON.with(|s| s.borrow().time_left_ms)
    }

    fn possess(&self) -> Option<PossessView> {
        PET.with(|p| p.borrow().possess.clone())
    }

    fn stable(&self) -> StableList {
        PET.with(|p| p.borrow().stable.clone())
    }

    fn stable_family(&mut self, entry: u32, _: Option<StableCallback>) -> Option<StableFamily> {
        PET.with(|p| p.borrow().stable_families.get(&entry).cloned())
    }

    fn pet_bar(&self) -> Option<PetBarLive> {
        PET.with(|p| p.borrow().pet_bar.clone())
    }

    fn pet_action(
        &self,
        index: usize,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<PetActionView> {
        PET.with(|p| p.borrow().pet_slots.get(index).cloned())
    }

    fn pet_place_action(&mut self, entries: &[(u32, u32)]) -> bool {
        PET.with(|p| {
            let mut p = p.borrow_mut();
            p.pet_writes.placed.push(entries.to_vec());
            for &(i, word) in entries {
                if let Some(s) = p.pet_slots.get_mut(i as usize) {
                    s.packed = word;
                }
            }
        });
        true
    }

    fn pet_stop_attack(&mut self) {
        PET.with(|p| p.borrow_mut().pet_writes.stops += 1);
    }

    fn pet_toggle_autocast(&mut self, index: u32) -> bool {
        PET.with(|p| {
            let mut p = p.borrow_mut();
            if !p.pet_bar.as_ref().is_some_and(|b| b.actions_usable) {
                return false;
            }
            p.pet_writes.toggled.push(index);
            if let Some(s) = p.pet_slots.get_mut(index as usize) {
                if s.autocast_allowed {
                    s.autocast_enabled = !s.autocast_enabled;
                }
            }
            true
        })
    }

    fn pet_set_spell_autocast(&mut self, spell_id: u32, state: Option<bool>) -> bool {
        PET.with(|p| {
            let mut p = p.borrow_mut();
            let Some(slot) = p
                .pet_book
                .as_mut()
                .and_then(|b| b.slots.iter_mut().rev().find(|s| s.spell_id == spell_id))
            else {
                return false;
            };
            let Some((true, on)) = slot.autocast else {
                return false;
            };
            let on = state.unwrap_or(!on);
            slot.autocast = Some((true, on));
            p.pet_writes.spell_autocasts.push((spell_id, on));
            true
        })
    }

    fn pet_book(&self, _: std::time::Instant, _: f64) -> Option<PetBookState> {
        PET.with(|p| p.borrow().pet_book.clone())
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
