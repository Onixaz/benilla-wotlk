//! The `"pet"` unit token: our `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON` (`0x60acc3`), pushed ahead of each `UNIT_PET` ([`push_pet_token`]). Its per-field `UNIT_*` fan-out is the units' (#1289).

use bevy::prelude::*;

use benilla_ui::script::{UiScript, UnitState};

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_script::gate;
use crate::ui_unit::{fire_transitions, snapshot};

use super::PetUnit;

/// What the `"pet"` feed last pushed, for its edges.
#[derive(Default)]
pub(crate) struct PetUnitMemory {
    /// The last snapshot pushed under `"pet"`, for [`fire_transitions`]' per-field diff.
    pushed: Option<UnitState>,
    /// The name cache's counter, which catches a `resolve` answer landing frames later.
    names_generation: gate::Watch,
}

/// `UNIT_FIELD_FLAGS` `0x800`, the server-written in-combat flag: the whole trigger for
/// `PET_ATTACK_START`/`PET_ATTACK_STOP`.
pub(super) const UNIT_FLAG_PET_IN_COMBAT: u32 = 0x0000_0800;

/// The `"pet"` token's snapshot: our charm, else our summon (`0x60acc3`-`0x60acd3`), `None` while neither is set or streamed. The reaction is 0: the pet frame reads none.
fn pet_snapshot(
    pet: &PetUnit,
    names: &NameCache,
    commands: &NetCommands,
    spells: Option<&crate::ui_action::Spells>,
) -> Option<UnitState> {
    let pet_guid = pet.token_guid()?;
    let store = pet.store(pet_guid)?;
    let name = names
        .resolve_unit(pet_guid, Some(store), commands)
        .map(str::to_string);
    // No `ChrClasses.dbc`: the reference reads a class only for TYPEMASK_PLAYER.
    let types = CreatureTypeSources::of_resources(names, spells);
    Some(snapshot(store, pet_guid, name, 0, None, types))
}

/// Push the `"pet"` token ahead of `UNIT_PET`, whose handlers read it: the setter's step (`0x5d6aca`) and our charm and summon watches' (event slots 0 and 2).
pub(crate) fn push_pet_token(world: &mut World) {
    if let Err(e) = world.run_system_cached(push_pet_token_system) {
        warn!("ui_pet: the pet token was not pushed: {e}");
    }
}

fn push_pet_token_system(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    pet: PetUnit,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fresh = pet_snapshot(&pet, &names, &commands, spells.as_deref());
    script.set_unit("pet", fresh);
}

/// Feed the `"pet"` token's per-field `UNIT_*` events, the units' fan-out (#1289).
pub(crate) fn feed_pet_unit(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    pet: PetUnit,
    // The per-field edges, for `fire_transitions`' watch-bridge arms.
    mut field_edges: MessageReader<crate::net::FieldChanged>,
    changed_stores: Query<(), Changed<ObjectStore>>,
    mut removed_stores: RemovedComponents<ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    // The form table, the creature-type resolver's first stage.
    spells: Option<Res<crate::ui_action::Spells>>,
    mut memory: Local<crate::ui_script::VmMemo<PetUnitMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    let edges = crate::net::FieldEdges::collect(&mut field_edges);
    let names_moved = memory.names_generation.moved(names.generation());
    let stores_changed = !changed_stores.is_empty();
    let stores_removed = !removed_stores.is_empty();
    gate::trace(
        "feed_pet_unit",
        &[
            ("vm_reset", vm_reset),
            ("names", names_moved),
            ("stores", stores_changed),
            ("removed", stores_removed),
        ],
    );
    let gate = gate::Gate::new(vm_reset || names_moved || stores_changed || stores_removed);
    removed_stores.clear();
    if gate.skip() {
        return;
    }
    // An unstreamed pet pushes nothing: `UnitExists("pet")` reads false and the frame hides.
    let fresh = pet_snapshot(&pet, &names, &commands, spells.as_deref());

    let dirty = match (&fresh, &memory.pushed) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if dirty {
        gate.audit("feed_pet_unit", "the pet snapshot");
        script.set_unit("pet", fresh.clone());
    }
    match &fresh {
        Some(cur) => {
            if memory.pushed.as_ref() != Some(cur) {
                if memory.pushed.is_none() {
                    debug!(
                        "ui_pet: \"pet\" resolved — {} ({}/{} hp)",
                        cur.name.as_deref().unwrap_or("<name pending>"),
                        cur.health,
                        cur.max_health,
                    );
                }
                fire_transitions(&mut script, "pet", memory.pushed.as_ref(), cur, &edges);
                memory.pushed = Some(cur.clone());
            }
        }
        // Clearing the token fires no UNIT_* event; the frame repaints on UNIT_PET.
        None => memory.pushed = None,
    }
}

/// The name watcher's field callback on offset `0x218` (`UNIT_FIELD_PET_NAME_TIMESTAMP`): any unit's rename drops its cached pet name so the next read asks again, and our charm or summon's (`owner_is_us`) fires `LOCALPLAYER_PET_RENAMED`.
pub(crate) fn pet_renamed(
    pet_guid: u64,
    owner_is_us: bool,
    ui: &mut crate::ui_script::live::UiEvents,
) {
    if let Some(pet_number) = benilla_protocol::guid::pet_number(pet_guid) {
        debug!("ui_pet: pet {pet_guid:#x} was renamed — re-asking its name");
        ui.world(move |world| {
            if let Some(mut names) = world.get_resource_mut::<NameCache>() {
                names.forget_pet(pet_number);
            }
        });
    }
    if owner_is_us {
        ui.fire("LOCALPLAYER_PET_RENAMED", Vec::new());
    }
}

/// The in-combat flag as the reference's field callback sees it: `None` unless the unit is ours by
/// CHARMEDBY, else SUMMONEDBY (not the possessed-unit lookup's CREATEDBY fallback); then the flag.
pub(crate) fn pet_combat_flag(store: &ObjectStore, self_guid: Option<u64>) -> Option<bool> {
    (store
        .0
        .unit_owner(benilla_protocol::OwnerFallback::SummonedBy)
        == self_guid)
        .then(|| store.0.unit_flags() & UNIT_FLAG_PET_IN_COMBAT != 0)
}
