//! The stable master's packet handlers: they fill [`StableOpen`] and [`StableErrors`].

use benilla_protocol::messages::StabledPet;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StableErrors, StableOpen};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfPlayer};
use crate::ui_action::{PetTameFailures, Spells};
use crate::unit_auras::UnitAuras;

/// `SPELL_AURA_OPEN_STABLE`: while one is up, a stable success re-lists with no master held
/// (`0x5a1872`).
const SPELL_AURA_OPEN_STABLE: u32 = 292;

/// Register the stable handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ListStabledPets, on_list)
        .net_handler(K::StableResult, on_result)
        .net_handler(K::Disconnected, on_session_end);
}

/// The stable closes with the connection; the range guard cannot close it, as it has no self
/// player to measure from after the drop.
fn on_session_end(In(_): In<SessionEvent>, mut stable_open: ResMut<StableOpen>) {
    stable_open.clear();
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut names: ResMut<NameCache>,
) {
    if let SessionEvent::ListStabledPets {
        npc,
        num_stable_slots,
        pets,
    } = ev
    {
        list_stabled_pets(npc, num_stable_slots, pets, &mut stable_open, &mut names);
    }
}

fn on_result(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut errors: ResMut<StableErrors>,
    mut tame_failures: ResMut<PetTameFailures>,
    commands: Res<NetCommands>,
    self_auras: Query<&UnitAuras, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
) {
    if let SessionEvent::StableResult { result } = ev {
        let open_stable_aura = || {
            let catalog = spells.as_ref().map(|s| &s.catalog);
            self_auras
                .single()
                .ok()
                .zip(catalog)
                .is_some_and(|(auras, catalog)| {
                    auras.iter().any(|a| {
                        catalog.get(a.spell_id).is_some_and(|rec| {
                            rec.effect_apply_aura.contains(&SPELL_AURA_OPEN_STABLE)
                        })
                    })
                })
        };
        stable_result(
            result,
            &mut stable_open,
            &mut errors,
            &mut tame_failures,
            &commands,
            open_stable_aura,
        );
    }
}

/// `MSG_LIST_STABLED_PETS`, sent unprompted for the gossip stable option, which opens the window,
/// and in answer to a refresh.
fn list_stabled_pets(
    npc: u64,
    num_stable_slots: u8,
    pets: Vec<StabledPet>,
    stable_open: &mut StableOpen,
    names: &mut NameCache,
) {
    debug!(
        "net: stable master {npc:#x} listed {} pets ({num_stable_slots} slots bought)",
        pets.len()
    );
    // Seed the pet-name cache from the rows, the pair `SMSG_PET_NAME_QUERY_RESPONSE` would carry:
    // stock `PetStable.lua:163` hands `UnitName("pet")` to a tooltip whose `SetText` raises on nil,
    // and this cache starts each world session empty, as 12340's pet-name cache does (persistence
    // off, cleared at world entry, `0x52b7c4`).
    for pet in &pets {
        if !pet.name.is_empty() {
            names.insert_pet(pet.pet_number, pet.name.clone());
        }
    }
    stable_open.open(npc, num_stable_slots, pets);
}

/// `SMSG_STABLE_RESULT`, handled as the reference's jump table does (`0x5a181a`): 1 shows
/// `ERR_NOT_ENOUGH_MONEY`, 8 and 9 re-request the list, as no success carries one, 10 counts a
/// bought slot and re-requests, 11 fires `PET_STABLE_UPDATE`, 12 shows the exotic tame failure,
/// and 2-7 show nothing. With no stable master held, the re-request goes out only under an
/// open-stable aura, aimed at no master (`0x5a1879`).
fn stable_result(
    result: u8,
    stable_open: &mut StableOpen,
    errors: &mut StableErrors,
    tame_failures: &mut PetTameFailures,
    net_commands: &NetCommands,
    open_stable_aura: impl FnOnce() -> bool,
) {
    use benilla_protocol::messages::stable_result as code;
    // Before the guid test, as in the reference: the count moves even with no stable open.
    if result == code::SUCCESS_BUY_SLOT {
        stable_open.num_stable_slots = stable_open.num_stable_slots.saturating_add(1);
    }
    match result {
        code::ERR_MONEY => {
            debug!("net: stable purchase refused — not enough money");
            errors.0.push("ERR_NOT_ENOUGH_MONEY");
        }
        code::SUCCESS_STABLE | code::SUCCESS_UNSTABLE | code::SUCCESS_BUY_SLOT => {
            let npc = match stable_open.npc {
                Some(npc) => npc,
                None if open_stable_aura() => 0,
                None => {
                    debug!("net: stable success {result} with no stable master — no re-list");
                    return;
                }
            };
            debug!("net: stable action succeeded (code {result}) — re-listing");
            let _ = net_commands.0.send(ClientCommand::ListStabledPets { npc });
        }
        code::UPDATE => stable_open.update_owed = true,
        code::ERR_EXOTIC => tame_failures.0.push(result),
        _ => debug!("net: stable result {result} — no client-visible effect"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::stable_result as code;

    fn run(
        result: u8,
        open: &mut StableOpen,
        aura: bool,
    ) -> (StableErrors, PetTameFailures, Vec<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut errors = StableErrors::default();
        let mut tame = PetTameFailures::default();
        stable_result(result, open, &mut errors, &mut tame, &commands, || aura);
        (errors, tame, rx.try_iter().collect())
    }

    #[test]
    fn a_success_re_lists_with_the_master_or_under_the_open_stable_aura() {
        let mut open = StableOpen::default();
        let (_, _, sent) = run(code::SUCCESS_STABLE, &mut open, false);
        assert!(sent.is_empty(), "no master, no aura");
        let (_, _, sent) = run(code::SUCCESS_UNSTABLE, &mut open, true);
        assert!(matches!(
            sent[..],
            [ClientCommand::ListStabledPets { npc: 0 }]
        ));
        open.npc = Some(0x42);
        let (_, _, sent) = run(code::SUCCESS_BUY_SLOT, &mut open, false);
        assert!(matches!(
            sent[..],
            [ClientCommand::ListStabledPets { npc: 0x42 }]
        ));
        assert_eq!(open.num_stable_slots, 1);
    }

    #[test]
    fn eleven_owes_the_update_and_twelve_is_the_exotic_tame_failure() {
        let mut open = StableOpen::default();
        let (_, tame, sent) = run(code::UPDATE, &mut open, false);
        assert!(open.update_owed && tame.0.is_empty() && sent.is_empty());
        let (errors, tame, _) = run(code::ERR_EXOTIC, &mut open, false);
        assert_eq!(tame.0, vec![12]);
        assert!(errors.0.is_empty());
    }
}
