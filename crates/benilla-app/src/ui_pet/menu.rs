//! The pet frame's right-click menu (`UnitPopup.lua:796-810`): its predicates, and its verbs, which act inside their call when no script call waits ahead of them ([`run_menu_verb`]).

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SummonPropertiesCatalog};
use benilla_protocol::messages::{PET_ACT_COMMAND, PET_COMMAND_DISMISS};
use benilla_ui::script::{PetMenuOutcome, PetMenuVerb, ScriptValue};

use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::UiError;

use super::{PetBar, PetUnit};

/// The pet-flags byte's can-be-renamed bit (`0x5d38c0`): set on a tamed pet not yet renamed.
const PET_FLAG_RENAME: u8 = 0x01;
/// The pet-flags byte's can-be-abandoned bit (`0x5d3780`): set on a hunter pet, clear on a summon.
pub(crate) const PET_FLAG_ABANDON: u8 = 0x02;
/// `SummonProperties.dbc` flag that keeps a summon from being dismissed (`0x5d3881`, `0x5d484c`).
const SUMMON_FLAG_NO_DISMISS: u32 = 0x20;
/// `PET_DISMISS_START`'s argument, the dismiss cast in ms (`0x5d48c3`).
const PET_DISMISS_MS: i64 = 10_000;

/// `(PetCanBeAbandoned, PetCanBeRenamed)` off `UNIT_FIELD_BYTES_2` byte 2.
pub(crate) fn menu_predicates(pet_flags: u8) -> (bool, bool) {
    (
        pet_flags & PET_FLAG_ABANDON != 0,
        pet_flags & PET_FLAG_RENAME != 0,
    )
}

/// Whether the unit's summon row bars a dismiss: the unit's summon-properties lookup (`0x71d1f0`)
/// through its `UNIT_CREATED_BY_SPELL`, flag `0x20`.
pub(crate) fn summon_bars_dismiss(
    fields: &benilla_protocol::ObjectFields,
    spells: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
) -> bool {
    fields
        .unit_created_by_spell()
        .and_then(|spell| spells.get(spell)?.summon_properties_row())
        .and_then(|row| summons.flags(row))
        .is_some_and(|flags| flags & SUMMON_FLAG_NO_DISMISS != 0)
}

/// What a menu verb reads and sends through, inside its call or from the queue behind a script call.
pub(crate) struct MenuCx<'a, 'w, 's> {
    pub(crate) bar: &'a PetBar,
    pub(crate) pet: &'a PetUnit<'w, 's>,
    pub(crate) spells: Option<&'a crate::ui_action::Spells>,
    pub(crate) summons: Option<&'a crate::spell::SummonSlots>,
    pub(crate) commands: &'a NetCommands,
    /// The stable master the stable window holds (`[0xc0f6c0]`).
    pub(crate) stable_master: Option<u64>,
    /// The active mover (`[0xca1238]`).
    pub(crate) mover: Option<u64>,
}

/// One menu verb under the pet list's head, as 12340's verb runs it.
pub(crate) fn run_menu_verb(cx: &MenuCx, verb: PetMenuVerb<'_>) -> PetMenuOutcome {
    match verb {
        PetMenuVerb::Abandon => abandon(cx),
        PetMenuVerb::Dismiss => dismiss(cx),
        PetMenuVerb::Rename(name) => rename(cx, name),
    }
}

/// `PetAbandon` (`0x5d4900`): `CMSG_PET_ABANDON` under the head, 0 with none, then the stable listed again while a stable master is held (`0x5d4961`).
fn abandon(cx: &MenuCx) -> PetMenuOutcome {
    let pet_guid = cx.bar.spells.pet_guid;
    debug!("ui_pet: abandoning pet {pet_guid:#x}");
    let _ = cx.commands.0.send(ClientCommand::PetAbandon { pet_guid });
    if let Some(npc) = cx.stable_master {
        let _ = cx.commands.0.send(ClientCommand::ListStabledPets { npc });
    }
    PetMenuOutcome::Done
}

/// `PetDismiss` (`0x5d46f0`): the usability gate (`0x5d46fa`); a head that is the active mover we ride dismisses it as the controlled vehicle (`0x5d4744`, `0x5d47fe`); a summon whose row bars it sends nothing (`0x5d484c`); else the Dismiss order at target 0, then `PET_DISMISS_START` (`0x5d48be`, `0x5d48c3`).
fn dismiss(cx: &MenuCx) -> PetMenuOutcome {
    let Some(store) = cx.pet.usable_pet(cx.bar) else {
        debug!("ui_pet: dismiss refused, the pet's actions are not usable — no packet");
        return PetMenuOutcome::Done;
    };
    let pet_guid = cx.bar.spells.pet_guid;
    let riding = cx.pet.seat.iter().next().map(|p| p.vehicle);
    if cx.mover == Some(pet_guid) && cx.pet.player_store().is_some() && riding == Some(pet_guid) {
        return PetMenuOutcome::VehicleExit;
    }
    if let (Some(spells), Some(summons)) = (cx.spells, cx.summons) {
        if summon_bars_dismiss(&store.0, &spells.catalog, &summons.0) {
            debug!("ui_pet: dismiss refused, the summon cannot be dismissed — no packet");
            return PetMenuOutcome::Done;
        }
    }
    let packed = PET_COMMAND_DISMISS | (u32::from(PET_ACT_COMMAND) << 24);
    debug!("ui_pet: dismissing pet {pet_guid:#x} ({packed:#010x})");
    let _ = cx.commands.0.send(ClientCommand::PetAction {
        pet_guid,
        packed,
        target_guid: 0,
    });
    PetMenuOutcome::DismissStart
}

/// `PetRename` (`0x5d5670`): the head held (`0x5d56c3`), summoned by us (`0x5d56f4`) and renameable (`0x5d5710`), then the name's own check (`0x5d5752`, `0x5d5808`), each refusal its message; else `CMSG_PET_RENAME` (`0x5d57f7`).
fn rename(cx: &MenuCx, name: &str) -> PetMenuOutcome {
    let pet_guid = cx.bar.spells.pet_guid;
    let Some(store) = cx.pet.store(pet_guid) else {
        return PetMenuOutcome::Refused("ERR_NO_PET");
    };
    if store.0.unit_summoned_by() != cx.pet.self_guid.0 {
        return PetMenuOutcome::Refused("ERR_NOTYOURPET");
    }
    if store.0.unit_pet_flags() & PET_FLAG_RENAME == 0 {
        return PetMenuOutcome::Refused("ERR_PET_NOT_RENAMEABLE");
    }
    if let Err(code) = benilla_ui::names::check_pet_name(name) {
        return PetMenuOutcome::Refused(code.key());
    }
    debug!("ui_pet: renaming pet {pet_guid:#x} to {name:?}");
    let _ = cx.commands.0.send(ClientCommand::PetRename {
        pet_guid,
        name: name.to_string(),
    });
    PetMenuOutcome::Done
}

/// The menu verbs a script call queued ahead of them, run in turn once that call applied.
pub(super) fn drain_pet_menu(
    mut vm: crate::ui_script::InTurn,
    stable: Option<Res<crate::ui_stable::StableOpen>>,
    bar: Option<Res<PetBar>>,
    commands: Res<NetCommands>,
    pet: PetUnit,
    spells: Option<Res<crate::ui_action::Spells>>,
    summons: Option<Res<crate::spell::SummonSlots>>,
    mover: Query<&crate::net::Guid, With<crate::net::ActiveMover>>,
) {
    let Some(mut script) = vm.turn() else {
        return;
    };
    let (abandons, dismisses) = script.take_pet_gives_up();
    let renames = script.take_pet_renames();
    if abandons == 0 && dismisses == 0 && renames.is_empty() {
        return;
    }
    let Some(bar) = bar.as_deref() else {
        return;
    };
    let stable_master = stable
        .as_deref()
        .and_then(crate::ui_session::NpcSession::npc);
    let cx = MenuCx {
        bar,
        pet: &pet,
        spells: spells.as_deref(),
        summons: summons.as_deref(),
        commands: &commands,
        stable_master,
        mover: mover.iter().next().map(|g| g.0),
    };
    let verbs = std::iter::repeat_n(PetMenuVerb::Abandon, abandons as usize).chain(
        std::iter::repeat_n(PetMenuVerb::Dismiss, dismisses as usize),
    );
    let mut outcomes: Vec<PetMenuOutcome> = verbs.map(|v| run_menu_verb(&cx, v)).collect();
    outcomes.extend(
        renames
            .iter()
            .map(|n| run_menu_verb(&cx, PetMenuVerb::Rename(n))),
    );
    for outcome in outcomes {
        match outcome {
            PetMenuOutcome::Done => {}
            PetMenuOutcome::DismissStart => {
                script.fire_event("PET_DISMISS_START", vec![ScriptValue::Int(PET_DISMISS_MS)]);
            }
            PetMenuOutcome::VehicleExit => {
                script.push_vehicle_request(benilla_ui::script::VehicleRequest::Exit);
            }
            // The refusal shows inside the verb's turn, in order with the menu's other events.
            PetMenuOutcome::Refused(key) => {
                crate::ui_action::signals::SignalSink::line(&mut *script, UiError::key(key));
            }
        }
    }
}
