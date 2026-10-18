//! The mount and dismount result packets, onto the red error line, and the equipment-set list a
//! set slot reads.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{MountErrors, PlayerActions};
use crate::net::NetHandlerApp;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::MountResult, on_mount_result)
        .net_handler(SessionEventKind::EquipmentSetList, on_equipment_set_list);
}

/// `SMSG_EQUIPMENT_SET_LIST` (`0x5af490`): the list replaces the held one whole; the bar's set
/// slots re-resolve their name and icon.
fn on_equipment_set_list(In(ev): In<SessionEvent>, mut actions: ResMut<PlayerActions>) {
    if let SessionEvent::EquipmentSetList(sets) = ev {
        debug!("net: {} equipment set(s)", sets.len());
        actions.equipment_sets = sets;
        actions.dirty = true;
    }
}

fn on_mount_result(In(ev): In<SessionEvent>, mut errors: ResMut<MountErrors>) {
    if let SessionEvent::MountResult { mount, code } = ev {
        mount_result(mount, code, &mut errors);
    }
}

/// `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT`: OK (`MOUNTRESULT_OK` 10, `DISMOUNTRESULT_OK` 3) is
/// silent in the reference; a failure queues the red error line.
fn mount_result(mount: bool, code: u32, errors: &mut MountErrors) {
    let ok = if mount { code == 10 } else { code == 3 };
    if !ok {
        info!(
            "net: {}mount refused (code {code})",
            if mount { "" } else { "dis" }
        );
        errors.0.push((mount, code));
    }
}
