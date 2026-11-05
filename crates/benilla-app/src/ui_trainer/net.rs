//! The trainer window's packet handlers, filling [`TrainerOpen`] and [`TrainerErrors`].

use benilla_protocol::messages::TrainerSpell;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{TrainerAsked, TrainerErrors, TrainerOpen};
use crate::net::NetHandlerApp;
use crate::ui_gossip::GossipState;

/// Register the trainer handlers, one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TrainerList, on_list)
        .net_handler(K::TrainerBuySucceeded, on_buy_succeeded)
        .net_handler(K::TrainerBuyFailed, on_buy_failed)
        // Re-evaluation triggers, as second handlers on their kinds; the feed re-derives after
        // every handler has run, so registration order does not matter.
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::PetSpells, on_pet_spells)
        .net_handler(K::SpellModifier, on_spell_modifier)
        .net_handler(K::UnlearnSpells, on_unlearn_spells)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_SET_FLAT/PCT_SPELL_MODIFIER`, the two talent triggers.
fn on_spell_modifier(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellModifier { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell entered the book: `SMSG_LEARNED_SPELL`'s handler (`0x6e7d60`) re-evaluates through
/// the learn registrar (`0x6e7d2a`).
fn on_spell_learned(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellLearned { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell left the book: `SMSG_REMOVED_SPELL`'s handler (`0x6e7840`) re-evaluates through the
/// unlearn (`0x6e7376`).
fn on_spell_removed(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellRemoved { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A rank replaced another, so the known-at-a-higher-rank answers may have moved (`0x6e7e97`).
fn on_spell_superceded(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellSuperceded { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// The pet's spellbook arrived or changed (`0x5d6d78`).
fn on_pet_spells(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::PetSpells(_)) {
        trainer_open.trigger_re_derive();
    }
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut trainer_open: ResMut<TrainerOpen>,
    asked: Res<TrainerAsked>,
    gossip: Res<GossipState>,
) {
    if let SessionEvent::TrainerList {
        trainer,
        trainer_type,
        services,
        greeting,
    } = ev
    {
        let expected = [asked.0, gossip.npc];
        trainer_list(
            trainer,
            trainer_type,
            services,
            greeting,
            &expected,
            &mut trainer_open,
        );
    }
}

fn on_buy_succeeded(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuySucceeded { trainer, spell_id } = ev {
        trainer_buy_succeeded(trainer, spell_id);
    }
}

fn on_buy_failed(In(ev): In<SessionEvent>, mut errors: ResMut<TrainerErrors>) {
    if let SessionEvent::TrainerBuyFailed { error, .. } = ev {
        trainer_buy_failed(error, &mut errors);
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

/// `SMSG_TRAINER_LIST`: open the window on the trainer's services, when the list is for the
/// trainer last asked or the gossip NPC (`0x6d14b6`, `0x6d14d5`); any other list is dropped.
fn trainer_list(
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
    expected: &[Option<u64>],
    trainer_open: &mut TrainerOpen,
) {
    if !expected.contains(&Some(trainer)) {
        debug!("net: trainer list for {trainer:#x}, neither asked nor the gossip npc — dropped");
        return;
    }
    debug!(
        "net: trainer {trainer:#x} (type {trainer_type}) listed {} services",
        services.len()
    );
    trainer_open.open(trainer, trainer_type, services, greeting);
}

/// `SMSG_TRAINER_BUY_SUCCEEDED`, logged only: the reference registers no handler for it (absent
/// from `reference/3.3.5a-opcodes.tsv`), the bought row repaints on `SMSG_LEARNED_SPELL`'s
/// re-evaluation, and AzerothCore sends no fresh list (`HandleTrainerBuySpellOpcode`).
fn trainer_buy_succeeded(trainer: u64, spell_id: u32) {
    debug!("net: trainer {trainer:#x} taught spell {spell_id}");
}

/// `SMSG_TRAINER_BUY_FAILED`: queue the code for the feed to log.
fn trainer_buy_failed(error: u32, errors: &mut TrainerErrors) {
    debug!("net: trainer buy failed (code {error})");
    errors.0.push(error);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_opens_only_for_the_asked_trainer_or_the_gossip_npc() {
        let mut open = TrainerOpen::default();
        let list = |guid, expected: &[Option<u64>], open: &mut TrainerOpen| {
            trainer_list(guid, 0, Vec::new(), String::new(), expected, open);
        };
        list(0x99, &[Some(0x42), None], &mut open);
        assert_eq!(open.trainer, None, "a stranger's list is dropped");
        list(0x42, &[Some(0x42), None], &mut open);
        assert_eq!(open.trainer, Some(0x42), "the asked trainer");
        list(0x77, &[Some(0x42), Some(0x77)], &mut open);
        assert_eq!(open.trainer, Some(0x77), "the gossip npc");
    }
}
