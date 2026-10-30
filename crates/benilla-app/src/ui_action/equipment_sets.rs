//! The equipment manager's wire: the server's list and replies into the VM, which owns the sets as
//! 12340's equipment manager does, and the VM's saves, deletes and swaps out. The action bar's set
//! slots read the VM's list back through [`PlayerActions::equipment_sets`].

use benilla_protocol::messages::{EquipmentSet, EquipmentSetUseSlot};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{EquipmentSetRequest, EquipmentSetView, UiScript};
use bevy::prelude::*;

use super::PlayerActions;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_script::VmMemo;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::EquipmentSetList, on_equipment_set_list)
        .net_handler(SessionEventKind::EquipmentSetSaved, on_equipment_set_saved)
        .net_handler(
            SessionEventKind::EquipmentSetUseResult,
            on_equipment_set_use_result,
        );
}

fn view(set: &EquipmentSet) -> EquipmentSetView {
    EquipmentSetView {
        guid: set.guid,
        id: set.id,
        name: set.name.clone(),
        icon: set.icon.clone(),
        items: set.items,
    }
}

fn wire(set: EquipmentSetView) -> EquipmentSet {
    EquipmentSet {
        guid: set.guid,
        id: set.id,
        name: set.name,
        icon: set.icon,
        items: set.items,
    }
}

/// The bar's copy follows the VM's list.
fn sync_bar(script: &UiScript, actions: &mut PlayerActions) {
    let sets: Vec<EquipmentSet> = script.equipment_sets().into_iter().map(wire).collect();
    if sets != actions.equipment_sets {
        actions.equipment_sets = sets;
        actions.dirty = true;
    }
}

/// `SMSG_EQUIPMENT_SET_LIST` (`0x5af490`); with no VM the bar takes the list as sent.
fn on_equipment_set_list(
    In(ev): In<SessionEvent>,
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
) {
    let SessionEvent::EquipmentSetList(sets) = ev else {
        return;
    };
    debug!("net: {} equipment set(s)", sets.len());
    actions.equipment_sets_listed = true;
    match script {
        Some(mut script) => {
            script.set_equipment_set_list(sets.iter().map(view).collect());
            sync_bar(&script, &mut actions);
        }
        None => {
            actions.equipment_sets = sets;
            actions.dirty = true;
        }
    }
}

/// `SMSG_EQUIPMENT_SET_SAVED` (`0x5af6c0`): the server's guid for a new set, 0 for a refused one.
fn on_equipment_set_saved(
    In(ev): In<SessionEvent>,
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
) {
    let (SessionEvent::EquipmentSetSaved { id, guid }, Some(mut script)) = (ev, script) else {
        return;
    };
    debug!("net: equipment set {id} saved as {guid:#x}");
    script.equipment_set_saved(id, guid);
    sync_bar(&script, &mut actions);
}

/// `SMSG_EQUIPMENT_SET_USE_RESULT` (`0x5af710`).
fn on_equipment_set_use_result(In(ev): In<SessionEvent>, script: Option<NonSendMut<UiScript>>) {
    let (SessionEvent::EquipmentSetUseResult { result }, Some(mut script)) = (ev, script) else {
        return;
    };
    debug!("net: equipment set swap result {result}");
    script.equipment_set_use_result(result);
}

/// A new VM starts from the held list; then the VM's sends go out and the bar follows its edits.
pub(super) fn drain_equipment_sets(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    commands: Res<NetCommands>,
    mut memory: Local<VmMemo<()>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (_, vm_reset) = memory.get_reset(&script);
    if vm_reset && actions.equipment_sets_listed {
        script.set_equipment_set_list(actions.equipment_sets.iter().map(view).collect());
    }
    let requests = script.take_equipment_set_requests();
    if requests.is_empty() {
        return;
    }
    for request in requests {
        let command = match request {
            EquipmentSetRequest::Save(set) => {
                debug!("ui_action: save equipment set {} ({})", set.id, set.name);
                ClientCommand::EquipmentSetSave(wire(set))
            }
            EquipmentSetRequest::Delete(guid) => ClientCommand::EquipmentSetDelete { guid },
            EquipmentSetRequest::Use(slots) => ClientCommand::EquipmentSetUse {
                slots: slots.map(|(item, bag, slot)| EquipmentSetUseSlot { item, bag, slot }),
            },
        };
        let _ = commands.0.send(command);
    }
    sync_bar(&script, &mut actions);
}
