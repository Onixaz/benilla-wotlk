//! The pet frame's right-click menu. `UnitPopup.lua:796-810` shows the paper doll and Abandon when
//! `PetCanBeAbandoned()`, Rename when `PetCanBeRenamed()` too, and Dismiss when the pet cannot be
//! abandoned and `PetCanBeDismissed()`. `PetAbandon` (`0x5d4900`) sends `CMSG_PET_ABANDON`;
//! `PetDismiss` (`0x5d46f0`) sends the Dismiss order as `CMSG_PET_ACTION`, past the usability
//! predicate, and fires `PET_DISMISS_START`.

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SummonPropertiesCatalog};
use benilla_protocol::messages::{PET_ACT_COMMAND, PET_COMMAND_DISMISS};
use benilla_ui::script::{ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::ui_action::{UiError, UiErrorKeys};

use super::{PetBar, PetUnit};

/// The pet-flags byte's can-be-renamed bit (`0x5d38c0`): set on a tamed pet not yet renamed.
const PET_FLAG_RENAME: u8 = 0x01;
/// The pet-flags byte's can-be-abandoned bit (`0x5d3780`): set on a hunter pet, clear on a summon.
const PET_FLAG_ABANDON: u8 = 0x02;
/// `SummonProperties.dbc` flag that keeps a summon from being dismissed (`0x5d3881`, `0x5d484c`).
const SUMMON_FLAG_NO_DISMISS: u32 = 0x20;
/// `PET_DISMISS_START`'s argument, the dismiss cast in ms (`0x5d48c3`).
const PET_DISMISS_MS: i64 = 10_000;

/// `(PetCanBeAbandoned, PetCanBeRenamed)` off `UNIT_FIELD_BYTES_2` byte 2.
pub(super) fn menu_predicates(pet_flags: u8) -> (bool, bool) {
    (
        pet_flags & PET_FLAG_ABANDON != 0,
        pet_flags & PET_FLAG_RENAME != 0,
    )
}

/// Whether the unit's summon row bars a dismiss: the unit's summon-properties lookup (`0x71d1f0`)
/// through its `UNIT_CREATED_BY_SPELL`, flag `0x20`.
pub(super) fn summon_bars_dismiss(
    store: &ObjectStore,
    spells: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
) -> bool {
    store
        .0
        .unit_created_by_spell()
        .and_then(|spell| spells.get(spell)?.summon_properties_row())
        .and_then(|row| summons.flags(row))
        .is_some_and(|flags| flags & SUMMON_FLAG_NO_DISMISS != 0)
}

/// Push the menu's predicates. Abandon and Rename need us as `UNIT_FIELD_SUMMONEDBY` (`0x5d37cb`),
/// Dismiss's refusal needs us as `UNIT_FIELD_CREATEDBY` (`0x5d386b`); with no pet it answers 1.
pub(super) fn feed_pet_menu(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    pet: PetUnit,
    spells: Option<Res<crate::ui_action::Spells>>,
    summons: Option<Res<crate::spell::SummonSlots>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let store = (bar.spells.pet_guid != 0)
        .then(|| pet.store(bar.spells.pet_guid))
        .flatten();
    let flags = store
        .filter(|store| store.0.unit_summoned_by() == pet.self_guid.0)
        .map_or(0, |store| store.0.unit_pet_flags());
    let (can_be_abandoned, can_be_renamed) = menu_predicates(flags);
    let can_be_dismissed = !store
        .filter(|store| store.0.unit_created_by() == pet.self_guid.0)
        .zip(spells.as_deref().zip(summons.as_deref()))
        .is_some_and(|(store, (spells, summons))| {
            summon_bars_dismiss(store, &spells.catalog, &summons.0)
        });
    script.set_pet_menu(can_be_abandoned, can_be_renamed, can_be_dismissed);
}

/// Send the menu's verbs under [`PetBar`]'s pet guid. Nothing is applied locally: the server
/// answers by removing the pet or moving its name timestamp.
pub(super) fn drain_pet_menu(
    script: Option<NonSendMut<UiScript>>,
    bar: Res<PetBar>,
    commands: Res<NetCommands>,
    mut ui_errors: ResMut<UiErrorKeys>,
    pet: PetUnit,
    spells: Option<Res<crate::ui_action::Spells>>,
    summons: Option<Res<crate::spell::SummonSlots>>,
    stable: Option<Res<crate::ui_stable::StableOpen>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (abandons, dismisses) = script.take_pet_gives_up();
    let renames = script.take_pet_renames();
    if abandons == 0 && dismisses == 0 && renames.is_empty() {
        return;
    }
    let pet_guid = bar.spells.pet_guid;
    if pet_guid == 0 {
        // `PetRename 0x5d5670`'s first check; the other verbs drop silently.
        for _ in &renames {
            ui_errors.0.push(UiError::key("ERR_NO_PET"));
        }
        debug!("ui_pet: dropping a queued menu verb — there is no pet");
        return;
    }

    // Counted, not deduped: the reference sends one per press.
    for _ in 0..abandons {
        debug!("ui_pet: abandoning pet {pet_guid:#x}");
        let _ = commands.0.send(ClientCommand::PetAbandon { pet_guid });
        // `0x5d4900`: with a stable master held, the stable is listed again.
        if let Some(npc) = stable.as_ref().and_then(|s| s.npc) {
            let _ = commands.0.send(ClientCommand::ListStabledPets { npc });
        }
    }
    // `0x5d46f0`: the usability predicate first, then a summon whose row bars it sends nothing;
    // otherwise the Dismiss order at target 0, then `PET_DISMISS_START`.
    let dismiss_word = PET_COMMAND_DISMISS | (u32::from(PET_ACT_COMMAND) << 24);
    for _ in 0..dismisses {
        let Some(store) = pet.usable_pet(&bar) else {
            debug!("ui_pet: dismiss refused, the pet's actions are not usable — no packet");
            continue;
        };
        if let (Some(spells), Some(summons)) = (spells.as_deref(), summons.as_deref()) {
            if summon_bars_dismiss(store, &spells.catalog, &summons.0) {
                debug!("ui_pet: dismiss refused, the summon cannot be dismissed — no packet");
                continue;
            }
        }
        debug!("ui_pet: dismissing pet {pet_guid:#x} ({dismiss_word:#010x})");
        let _ = commands.0.send(ClientCommand::PetAction {
            pet_guid,
            packed: dismiss_word,
            target_guid: 0,
        });
        script.fire_event("PET_DISMISS_START", vec![ScriptValue::Int(PET_DISMISS_MS)]);
    }
    for name in renames {
        // `PetRename 0x5d5670`: a refused name shows its name error and sends nothing.
        if let Err(code) = benilla_ui::names::check_pet_name(&name) {
            ui_errors.0.push(UiError::key(code.key()));
            continue;
        }
        debug!("ui_pet: renaming pet {pet_guid:#x} to {name:?}");
        let _ = commands.0.send(ClientCommand::PetRename { pet_guid, name });
    }
}
