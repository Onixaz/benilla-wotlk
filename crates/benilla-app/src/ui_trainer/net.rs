//! The trainer window's packet handlers: the list runs the setter and the list build, and each of
//! the re-evaluator's triggers runs it in its turn.

use benilla_protocol::messages::TrainerSpell;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{TrainerAsked, TrainerOpen};
use crate::net::NetHandlerApp;
use crate::ui_gossip::GossipState;
use crate::ui_script::live::UiEvents;

/// Register the trainer handlers, one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TrainerList, on_list)
        .net_handler(K::TrainerBuySucceeded, on_buy_succeeded)
        .net_handler(K::TrainerBuyFailed, on_buy_failed)
        // The re-evaluator's packet triggers, second handlers on their kinds, each after the
        // book change its packet makes.
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::PetSpells, on_pet_spells)
        .net_handler(K::UnlearnSpells, on_unlearn_spells)
        .net_handler(K::Disconnected, on_session_end);
}

/// A spell entered the book: the learn registrar re-evaluates (`0x6e7d2a`).
fn on_spell_learned(In(ev): In<SessionEvent>, ui: Option<ResMut<UiEvents>>) {
    if let (SessionEvent::SpellLearned { .. }, Some(mut ui)) = (ev, ui) {
        ui.world(super::refresh);
    }
}

/// A spell left the book: the unlearn re-evaluates (`0x6e7376`).
fn on_spell_removed(In(ev): In<SessionEvent>, ui: Option<ResMut<UiEvents>>) {
    if let (SessionEvent::SpellRemoved { .. }, Some(mut ui)) = (ev, ui) {
        ui.world(super::refresh);
    }
}

/// A rank replaced another (`0x6e7e97`).
fn on_spell_superceded(In(ev): In<SessionEvent>, ui: Option<ResMut<UiEvents>>) {
    if let (SessionEvent::SpellSuperceded { .. }, Some(mut ui)) = (ev, ui) {
        ui.world(super::refresh);
    }
}

/// The pet's spellbook arrived or changed (`0x5d6d78`).
fn on_pet_spells(In(ev): In<SessionEvent>, ui: Option<ResMut<UiEvents>>) {
    if let (SessionEvent::PetSpells(_), Some(mut ui)) = (ev, ui) {
        ui.world(super::refresh);
    }
}

/// `SMSG_TRAINER_LIST` (`0x6d12c0`): a list for the trainer last asked or the gossip NPC
/// (`0x6d14b6`, `0x6d14d5`) runs the setter and the list build in turn; any other is dropped.
fn on_list(
    In(ev): In<SessionEvent>,
    asked: Res<TrainerAsked>,
    gossip: Res<GossipState>,
    ui: Option<ResMut<UiEvents>>,
) {
    if let SessionEvent::TrainerList {
        trainer,
        trainer_type,
        services,
        greeting,
    } = ev
    {
        if !lands(trainer, &[asked.0, gossip.npc], &services) {
            return;
        }
        if let Some(mut ui) = ui {
            ui.world(move |w| super::list(w, trainer, trainer_type, services, greeting));
        }
    }
}

/// Whether a list lands: for the trainer last asked or the gossip NPC.
fn lands(trainer: u64, expected: &[Option<u64>], services: &[TrainerSpell]) -> bool {
    if !expected.contains(&Some(trainer)) {
        debug!("net: trainer list for {trainer:#x}, neither asked nor the gossip npc — dropped");
        return false;
    }
    debug!(
        "net: trainer {trainer:#x} listed {} services",
        services.len()
    );
    true
}

/// `SMSG_TRAINER_BUY_SUCCEEDED`, logged only: the reference registers no handler for it (absent
/// from `reference/3.3.5a-opcodes.tsv`), the bought row repaints on `SMSG_LEARNED_SPELL`'s
/// re-evaluation, and AzerothCore sends no fresh list (`HandleTrainerBuySpellOpcode`).
fn on_buy_succeeded(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuySucceeded { trainer, spell_id } = ev {
        debug!("net: trainer {trainer:#x} taught spell {spell_id}");
    }
}

/// `SMSG_TRAINER_BUY_FAILED` (`0x6d8410`): the reference shows the player nothing and only writes
/// its line to the console.
fn on_buy_failed(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuyFailed { error, .. } = ev {
        info!(
            "ui_trainer: {} (code {error})",
            super::train_error_log_line(error)
        );
    }
}

/// An open trainer window ends with the session.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut trainer_open: ResMut<TrainerOpen>,
    mut asked: ResMut<TrainerAsked>,
    mut unlearned: ResMut<super::UnlearnedSpells>,
) {
    trainer_open.clear_session();
    asked.0 = None;
    unlearned.0.clear();
}

/// `SMSG_SEND_UNLEARN_SPELLS` (`0x6e2240`): the list replaces the last one.
fn on_unlearn_spells(In(ev): In<SessionEvent>, mut unlearned: ResMut<super::UnlearnedSpells>) {
    if let SessionEvent::UnlearnSpells(spells) = ev {
        unlearned.0 = spells;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_lands_only_for_the_asked_trainer_or_the_gossip_npc() {
        assert!(!lands(0x99, &[Some(0x42), None], &[]), "a stranger's list");
        assert!(lands(0x42, &[Some(0x42), None], &[]), "the asked trainer");
        assert!(
            lands(0x77, &[Some(0x42), Some(0x77)], &[]),
            "the gossip npc"
        );
    }
}
