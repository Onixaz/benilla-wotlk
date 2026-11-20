//! The mount and dismount result packets, onto the red error line.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::errors::mount_result_key;
use super::{show_in_turn, UiError};
use crate::net::NetHandlerApp;
use crate::ui_script::live::UiEvents;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::MountResult, on_mount_result);
}

fn on_mount_result(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::MountResult { mount, code } = ev {
        if let Some(line) = mount_result(mount, code) {
            show_in_turn(&mut ui, "ui_action", line);
        }
    }
}

/// `SMSG_MOUNTRESULT`/`SMSG_DISMOUNTRESULT`: OK (`MOUNTRESULT_OK` 10, `DISMOUNTRESULT_OK` 3) is
/// silent in the reference; a failure shows its line in the handler's turn. None of these
/// strings takes arguments.
fn mount_result(mount: bool, code: u32) -> Option<UiError> {
    let ok = if mount { code == 10 } else { code == 3 };
    if ok {
        return None;
    }
    info!(
        "net: {}mount refused (code {code})",
        if mount { "" } else { "dis" }
    );
    mount_result_key(mount, code).map(UiError::key)
}
