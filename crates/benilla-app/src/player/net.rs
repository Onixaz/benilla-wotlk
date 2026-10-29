//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue;
//! `SMSG_TIME_SYNC_REQ` is answered here.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::ServerStandState;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfPlayer};

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::TimeSyncReq, on_time_sync_req);
}

/// `SMSG_TIME_SYNC_REQ` (`0x6dc010`): with our player held, the counter and the movement clock
/// at handling (`0x6ef370`) go back as `CMSG_TIME_SYNC_RESP` (`0x6e9bb0`). The reference queues
/// the two as a move event its next movement update sends; benilla has no move queue, and a
/// request's event is due on arrival, so the answer goes at once with the same stamp. Not applied:
/// the queue's arm (`0x6ef860`) first drops move flag `0x200` when set, which no live run has
/// tied to a state benilla's mover holds.
fn on_time_sync_req(
    In(ev): In<SessionEvent>,
    me: Query<(), With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::TimeSyncReq { counter } = ev else {
        return;
    };
    if me.is_empty() {
        return;
    }
    let client_ms = benilla_protocol::client_uptime_ms();
    super::move_trace::time_sync(counter, client_ms);
    let _ = commands
        .0
        .send(ClientCommand::TimeSyncResp { counter, client_ms });
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}
