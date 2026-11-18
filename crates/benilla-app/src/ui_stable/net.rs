//! The stable master's packet handlers: they write [`StableOpen`] and fire the window's events once their changes apply.

use benilla_protocol::messages::StabledPet;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{StableOpen, SPELL_AURA_OPEN_STABLE};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfPlayer};
use crate::ui_action::{PetTameFailures, Spells, UiError, UiErrorKeys};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;
use crate::unit_auras::UnitAuras;

/// Register the stable handlers, the creature answer's callbacks and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::ListStabledPets, on_list)
        .net_handler(K::StableResult, on_result)
        .net_handler(K::CreatureName, on_creature_answer)
        .net_handler(K::Disconnected, on_session_end);
}

/// The connection's end leaves the world (`0x528d5a`): a held master closes as the world leave closes it, then the globals and the record asks go with the session.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    ui: Option<ResMut<UiEvents>>,
) {
    let fired = stable_open.close_through_target(false);
    if let Some(mut ui) = ui {
        for _ in 0..fired {
            ui.fire("PET_STABLE_CLOSED", vec![]);
        }
    }
    stable_open.clear();
    stable_open.drop_asks();
}

/// A creature record's answer runs the stable verbs' callbacks queued on it, in order; an unknown entry fails them, which fires nothing (`0x5a0e80`, `0x5a0ea0`).
fn on_creature_answer(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    names: Res<NameCache>,
    ui: Option<ResMut<UiEvents>>,
) {
    let SessionEvent::CreatureName { entry, .. } = ev else {
        return;
    };
    let callbacks = stable_open.take_asks(entry);
    let (Some(mut ui), true) = (ui, names.creature_record(entry).is_some()) else {
        return;
    };
    for callback in callbacks {
        ui.fire(callback.event(), vec![]);
    }
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut stable_open: ResMut<StableOpen>,
    mut names: ResMut<NameCache>,
    ui: Option<ResMut<UiEvents>>,
) {
    if let SessionEvent::ListStabledPets {
        npc,
        num_stable_slots,
        pets,
    } = ev
    {
        // Another master's list moves the interact target, whose clear runs the interaction close's stable arm on the held one before the new master is stored (`0x5a11d9`, `0x51fd1c`, `0x513065`, `0x5a0f4c`): its event fires with none held.
        let switched = stable_open.npc().is_some_and(|held| held != npc) && stable_open.close();
        let Some(mut ui) = ui else {
            list_stabled_pets(npc, num_stable_slots, pets, &mut stable_open, &mut names);
            return;
        };
        if switched {
            ui.fire("PET_STABLE_CLOSED", vec![]);
            ui.world(move |world| {
                world.resource_scope(|world, mut names: Mut<NameCache>| {
                    if let Some(mut open) = world.get_resource_mut::<StableOpen>() {
                        list_stabled_pets(npc, num_stable_slots, pets, &mut open, &mut names);
                    }
                });
            });
        } else {
            list_stabled_pets(npc, num_stable_slots, pets, &mut stable_open, &mut names);
        }
        // The selection resets with the list, then `PET_STABLE_SHOW` (`0x5a11fa`, `0x5a131a`).
        ui.write(benilla_ui::script::UiScript::reset_stable_selection);
        ui.fire("PET_STABLE_SHOW", vec![]);
    }
}

fn on_result(
    In(ev): In<SessionEvent>,
    stable_open: ResMut<StableOpen>,
    mut errors: ResMut<UiErrorKeys>,
    mut tame_failures: ResMut<PetTameFailures>,
    commands: Res<NetCommands>,
    self_auras: Query<&UnitAuras, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    ui: Option<ResMut<UiEvents>>,
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
        let update = stable_result(
            result,
            stable_open,
            &mut errors,
            &mut tame_failures,
            &commands,
            open_stable_aura,
        );
        if let (true, Some(mut ui)) = (update, ui) {
            ui.fire("PET_STABLE_UPDATE", vec![]);
        }
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

/// `SMSG_STABLE_RESULT`, handled as the reference's jump table does (`0x5a181a`): 1 shows `ERR_NOT_ENOUGH_MONEY`, 8 and 9 re-request the list, as no success carries one, 10 counts a bought slot and re-requests, 11 owes `PET_STABLE_UPDATE` (`0x5a1904`), 12 shows the exotic tame failure, and 2-7 show nothing. With no stable master held, the re-request goes out only under an open-stable aura, aimed at no master (`0x5a1879`). Whether the update is owed.
fn stable_result(
    result: u8,
    mut stable_open: impl std::ops::DerefMut<Target = StableOpen>,
    errors: &mut UiErrorKeys,
    tame_failures: &mut PetTameFailures,
    net_commands: &NetCommands,
    open_stable_aura: impl FnOnce() -> bool,
) -> bool {
    use benilla_protocol::messages::stable_result as code;
    // Before the guid test, as in the reference: the count moves even with no stable open.
    if result == code::SUCCESS_BUY_SLOT {
        stable_open.num_stable_slots = stable_open.num_stable_slots.saturating_add(1);
    }
    match result {
        code::ERR_MONEY => {
            debug!("net: stable purchase refused — not enough money");
            errors.0.push(UiError::key("ERR_NOT_ENOUGH_MONEY"));
        }
        code::SUCCESS_STABLE | code::SUCCESS_UNSTABLE | code::SUCCESS_BUY_SLOT => {
            let npc = match stable_open.npc() {
                Some(npc) => npc,
                None if open_stable_aura() => 0,
                None => {
                    debug!("net: stable success {result} with no stable master — no re-list");
                    return false;
                }
            };
            debug!("net: stable action succeeded (code {result}) — re-listing");
            let _ = net_commands.0.send(ClientCommand::ListStabledPets { npc });
        }
        code::UPDATE => return true,
        code::ERR_EXOTIC => tame_failures.0.push(result),
        _ => debug!("net: stable result {result} — no client-visible effect"),
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::stable_result as code;

    fn run(
        result: u8,
        open: &mut StableOpen,
        aura: bool,
    ) -> (bool, UiErrorKeys, PetTameFailures, Vec<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut errors = UiErrorKeys::default();
        let mut tame = PetTameFailures::default();
        let update = stable_result(result, open, &mut errors, &mut tame, &commands, || aura);
        (update, errors, tame, rx.try_iter().collect())
    }

    #[test]
    fn a_success_re_lists_with_the_master_or_under_the_open_stable_aura() {
        let mut open = StableOpen::default();
        let (_, _, _, sent) = run(code::SUCCESS_STABLE, &mut open, false);
        assert!(sent.is_empty(), "no master, no aura");
        let (_, _, _, sent) = run(code::SUCCESS_UNSTABLE, &mut open, true);
        assert!(matches!(
            sent[..],
            [ClientCommand::ListStabledPets { npc: 0 }]
        ));
        open.open(0x42, 0, Vec::new());
        let (_, _, _, sent) = run(code::SUCCESS_BUY_SLOT, &mut open, false);
        assert!(matches!(
            sent[..],
            [ClientCommand::ListStabledPets { npc: 0x42 }]
        ));
        assert_eq!(open.num_stable_slots, 1);
    }

    #[test]
    fn eleven_owes_the_update_and_twelve_is_the_exotic_tame_failure() {
        let mut open = StableOpen::default();
        let (update, _, tame, sent) = run(code::UPDATE, &mut open, false);
        assert!(update && tame.0.is_empty() && sent.is_empty());
        let (update, errors, tame, _) = run(code::ERR_EXOTIC, &mut open, false);
        assert!(!update);
        assert_eq!(tame.0, vec![12]);
        assert!(errors.0.is_empty());
        let (_, errors, _, _) = run(code::ERR_MONEY, &mut open, false);
        assert_eq!(errors.0[0].key, "ERR_NOT_ENOUGH_MONEY");
    }
}
