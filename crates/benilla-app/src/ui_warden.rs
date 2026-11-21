//! A Warden script check's chunk, run in the UI's Lua state ([`crate::warden`] has the rest).

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::NetHandlerApp;

pub(crate) struct UiWardenPlugin;

impl Plugin for UiWardenPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::WardenScript, on_script);
    }
}

/// A check's chunk, through the executor it bound (`0x819210`), in whichever UI state is up: glue
/// or game.
fn on_script(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::WardenScript { chunk } = ev else {
        return;
    };
    crate::ui_script::safe_world::lend(world, |script| script.execute_secure(&chunk));
}
