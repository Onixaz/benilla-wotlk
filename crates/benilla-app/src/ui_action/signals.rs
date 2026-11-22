//! The spellbook's cooldown signals (`0x53bac0`): `ACTIONBAR_UPDATE_COOLDOWN` (`0x5a7cc0`),
//! `UPDATE_SHAPESHIFT_COOLDOWN` while the stance bar lists a form (`0x53bac5`), then
//! `SPELL_UPDATE_COOLDOWN`. Every site that writes a cooldown list runs them, then the usable
//! update when the write was on hold, then its list's own signal: the bags' `BAG_UPDATE_COOLDOWN`
//! (`0x5d6f10`) after the player's list, the pet bar's `PET_BAR_UPDATE_COOLDOWN` (`0x5d3090`)
//! after the pet's.
//!
//! The selection update (`0x53b480`) is the bar's `ACTIONBAR_UPDATE_STATE` (`0x5a7cb0`), then
//! `CURRENT_SPELL_CAST_CHANGED`: our cast's start visuals (`0x80b573`, `0x806961`) and its finish
//! (`0x8054f0`), the targeting cursor's arm (`0x80d806`), and the form, override-spell and init
//! sites (`0x6da389`, `0x6d1c58`, `0x6e805e`) run it.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::ui_script::live::UiEvents;

use super::usable_cache::{self, Pass, UsableCaches};
use super::UiError;

/// The caller's ordered stream: a signal or line fires in turn with the write or refusal that raised it, as the display (`0x5216f0`) runs inside the refusing code; a press's edges fire at their step with the `World` lent.
pub(crate) trait SignalSink {
    fn signal(&mut self, event: &'static str);

    /// Show `line` here: its sound, then its event or its chat line.
    fn line(&mut self, line: UiError);
}

impl SignalSink for UiEvents {
    fn signal(&mut self, event: &'static str) {
        self.fire(event, Vec::new());
    }

    fn line(&mut self, line: UiError) {
        super::show_in_turn(self, "ui_lines", line);
    }
}

impl SignalSink for UiScript {
    fn signal(&mut self, event: &'static str) {
        self.fire_event(event, Vec::new());
    }

    fn line(&mut self, line: UiError) {
        // The sound goes first whatever the text (`0x52172d`), so an unresolved line still sounds.
        let text = super::ui_error_text(&line, &|key| self.lua().globals().get::<String>(key).ok())
            .unwrap_or_default();
        self.show_text(line.key, text);
    }
}

/// The sink of a system that holds no VM: its signals join the handler queue in order and fire once
/// the system's commands apply ([`Self::flush`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct QueuedSignals<'w> {
    ui: Option<ResMut<'w, UiEvents>>,
}

impl SignalSink for QueuedSignals<'_> {
    fn signal(&mut self, event: &'static str) {
        if let Some(ui) = self.ui.as_mut() {
            ui.fire(event, Vec::new());
        }
    }

    fn line(&mut self, line: UiError) {
        if let Some(ui) = self.ui.as_mut() {
            ui.line(line);
        }
    }
}

impl QueuedSignals<'_> {
    /// Fire what the system signalled once its commands apply.
    pub(crate) fn flush(&self, ecs: &mut Commands) {
        if self.ui.as_ref().is_some_and(|ui| !ui.is_empty()) {
            ecs.queue(crate::ui_script::live::fire_handler_events);
        }
    }
}

/// A test's record of what fired, in order: a line by its key.
impl SignalSink for Vec<&'static str> {
    fn signal(&mut self, event: &'static str) {
        self.push(event);
    }

    fn line(&mut self, line: UiError) {
        self.push(line.key);
    }
}

/// A test's record of the lines shown, in order; signals are not recorded.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct LinesSeen(pub(crate) Vec<UiError>);

#[cfg(test)]
impl SignalSink for LinesSeen {
    fn signal(&mut self, _: &'static str) {}

    fn line(&mut self, line: UiError) {
        self.0.push(line);
    }
}

/// The list signal that ends a cooldown site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tail {
    /// None: the start cooldown's clear (`0x80476d`), the ranged reset (`0x807364`), the player's
    /// own start (`0x805ed6`).
    None,
    /// The player's list: `BAG_UPDATE_COOLDOWN`.
    Bags,
    /// The pet's list: `PET_BAR_UPDATE_COOLDOWN`.
    Pet,
}

impl Tail {
    /// The tail of a site that names its list by index: the pet's for 1, else the bags'.
    pub(crate) fn of_list(pet: bool) -> Self {
        if pet {
            Tail::Pet
        } else {
            Tail::Bags
        }
    }

    fn event(self) -> Option<&'static str> {
        match self {
            Tail::None => None,
            Tail::Bags => Some("BAG_UPDATE_COOLDOWN"),
            Tail::Pet => Some("PET_BAR_UPDATE_COOLDOWN"),
        }
    }
}

/// The selection update's events (`0x53b480`).
pub(crate) const SELECTION: [&str; 2] = ["ACTIONBAR_UPDATE_STATE", "CURRENT_SPELL_CAST_CHANGED"];

/// The selection update in a handler's turn.
pub(crate) fn selection(ui: &mut UiEvents) {
    for event in SELECTION {
        ui.fire(event, Vec::new());
    }
}

/// The selection update fired into a VM a press holds.
pub(crate) fn selection_now(script: &mut UiScript) {
    for event in SELECTION {
        script.fire_event(event, Vec::new());
    }
}

/// `0x53bac0`'s events, `stances` whether the stance bar lists a form (`[0xbe8e28]`).
pub(crate) fn spellbook_events(stances: bool) -> impl Iterator<Item = &'static str> {
    [
        Some("ACTIONBAR_UPDATE_COOLDOWN"),
        stances.then_some("UPDATE_SHAPESHIFT_COOLDOWN"),
        Some("SPELL_UPDATE_COOLDOWN"),
    ]
    .into_iter()
    .flatten()
}

/// Whether the stance bar lists a form, as the cooldown signals read it.
fn stances_listed(world: &World) -> bool {
    world
        .get_resource::<UsableCaches>()
        .is_some_and(UsableCaches::lists_forms)
}

/// `0x53bac0` alone, in a handler's turn.
pub(crate) fn spellbook(ui: &mut UiEvents) {
    ui.world(|world: &mut World| {
        let stances = stances_listed(world);
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            for event in spellbook_events(stances) {
                ui.fire(event, Vec::new());
            }
        }
    });
}

/// One cooldown site in a handler's turn: the spellbook's signals, the usable update when
/// `usable`, then `tail`.
pub(crate) fn cooldown_site(ui: &mut UiEvents, usable: bool, tail: Tail) {
    spellbook(ui);
    if usable {
        ui.world(usable_cache::step(Pass::Full));
    }
    if let Some(event) = tail.event() {
        ui.fire(event, Vec::new());
    }
}

/// One cooldown site fired into a VM a press holds, `run` the press's usable update.
pub(crate) fn cooldown_site_now(
    script: &mut UiScript,
    run: Option<&mut usable_cache::UsableRun>,
    usable: bool,
    tail: Tail,
) {
    let stances = run.as_ref().is_some_and(|r| r.lists_forms());
    for event in spellbook_events(stances) {
        script.fire_event(event, Vec::new());
    }
    if let (true, Some(run)) = (usable, run) {
        run.fire(script, Pass::Full);
    }
    if let Some(event) = tail.event() {
        script.fire_event(event, Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The auto-repeating spell's setter (`0x800a00`): the same spell is a no-op, a new one signals
    /// the start, even over a running one; the cancel (`0x807560`) signals the stop only while one
    /// runs.
    #[test]
    fn the_auto_repeat_spell_signals_each_new_spell_and_its_clear() {
        let mut cell = crate::spell::AutoRepeatActive(None);
        let mut seen: Vec<&'static str> = Vec::new();
        cell.set(5019, &mut seen);
        cell.set(5019, &mut seen);
        cell.set(75, &mut seen);
        assert!(cell.clear(&mut seen));
        assert!(!cell.clear(&mut seen));
        assert_eq!(
            seen,
            [
                "START_AUTOREPEAT_SPELL",
                "START_AUTOREPEAT_SPELL",
                "STOP_AUTOREPEAT_SPELL"
            ]
        );
    }

    #[test]
    fn the_shapeshift_signal_sits_between_the_two_while_the_bar_lists_a_form() {
        assert_eq!(
            spellbook_events(false).collect::<Vec<_>>(),
            ["ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN"]
        );
        assert_eq!(
            spellbook_events(true).collect::<Vec<_>>(),
            [
                "ACTIONBAR_UPDATE_COOLDOWN",
                "UPDATE_SHAPESHIFT_COOLDOWN",
                "SPELL_UPDATE_COOLDOWN"
            ]
        );
    }
}
