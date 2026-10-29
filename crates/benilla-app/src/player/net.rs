//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` goes to [`super::posture`] as a
//! [`ServerStandState`], applied in `control`, which holds the body query and sheath queue.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Player, ServerStandState};
use crate::creature_anim::move_flags;
use crate::entities::CollisionHeight;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid, SelfPlayer};

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(
            SessionEventKind::MoveSetCollisionHeight,
            on_set_collision_height,
        );
}

/// `SMSG_MOVE_SET_COLLISION_HGT` on our mover (`0x7307a0`, then `0x6e9600` at the event): the
/// body's collision height becomes the server's until the model sets it again, and the ack
/// carries our pose and the height. The depth lines read it through [`Player::collision_height`].
fn on_set_collision_height(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    player: Option<Res<Player>>,
    mut body: Query<&mut CollisionHeight, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::MoveSetCollisionHeight {
        guid,
        counter,
        height,
    } = ev
    else {
        return;
    };
    // A build with no mover has no body to resize.
    let Some(player) = player.filter(|_| self_guid.0 == Some(guid)) else {
        return;
    };
    if let Ok(mut h) = body.single_mut() {
        h.0 = height;
    }
    let flying = if player.flying { move_flags::FLYING } else { 0 };
    let _ = commands.0.send(ClientCommand::CollisionHeightAck {
        guid,
        counter,
        height,
        flags: player.modes.wire_flags() | flying,
        pos: benilla_assets::coords::bevy_to_wow(player.pos),
        orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
    });
}

fn on_stand_state_update(In(ev): In<SessionEvent>, mut out: MessageWriter<ServerStandState>) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        out.write(ServerStandState { state });
    }
}
