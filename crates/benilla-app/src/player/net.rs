//! The player's packet handlers: `SMSG_STANDSTATE_UPDATE` commits its state here and hands its
//! weapon stow to [`super::posture`] as a [`ServerStandState`], run in `control`, which holds the
//! body query and sheath queue;
//! `SMSG_TIME_SYNC_REQ` is answered here.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Player, ServerStandState};
use crate::creature_anim::move_flags;
use crate::entities::CollisionHeight;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, SelfGuid, SelfPlayer};

/// Registers the handlers; called from [`super::PlayerPlugin`].
pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::StandStateUpdate, on_stand_state_update)
        .net_handler(SessionEventKind::TimeSyncReq, on_time_sync_req)
        .net_handler(
            SessionEventKind::MoveSetCollisionHeight,
            on_set_collision_height,
        )
        .net_handler(
            SessionEventKind::CollisionHeightChanged,
            on_collision_height_changed,
        );
}

/// `SMSG_TIME_SYNC_REQ` (`0x6dc010`): with our player held, the counter and the movement clock
/// at handling (`0x6ef370`) go back as `CMSG_TIME_SYNC_RESP` (`0x6e9bb0`). The reference queues
/// the two as a move event its next movement update sends; benilla has no move queue, and a
/// request's event is due on arrival, so the answer goes at once with the same stamp. Not applied:
/// the queue's arm (`0x6ef860`) first drops move flag `0x200` when set, which no live run has
/// tied to a state benilla's mover holds.
fn on_time_sync_req(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    commands: Res<NetCommands>,
) {
    let SessionEvent::TimeSyncReq { counter } = ev else {
        return;
    };
    // The active player's object in the index (`0x6dc028`, `0x6dc03b`), held from its create's step, not a later frame's tag; none answers nothing (`0x6dc047`).
    if !self_guid.0.is_some_and(|g| index.0.contains_key(&g)) {
        return;
    }
    let client_ms = benilla_protocol::client_uptime_ms();
    super::move_trace::time_sync(counter, client_ms);
    let _ = commands
        .0
        .send(ClientCommand::TimeSyncResp { counter, client_ms });
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

/// `MSG_MOVE_SET_COLLISION_HGT`, no ack: any unit's collision height becomes the server's until its
/// model sets it again. The active mover takes the height alone (`0x740833`), another unit with the
/// pose its relay carries (`0x74099f`).
fn on_collision_height_changed(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut heights: Query<&mut CollisionHeight>,
    mut commands: Commands,
) {
    let SessionEvent::CollisionHeightChanged { guid, height } = ev else {
        return;
    };
    let Some(&e) = index.0.get(&guid) else {
        return;
    };
    match heights.get_mut(e) {
        Ok(mut h) => h.0 = height,
        // Not stamped yet: the stamp skips a unit that already has one.
        Err(_) => {
            commands.entity(e).insert(CollisionHeight(height));
        }
    }
}

fn on_stand_state_update(
    In(ev): In<SessionEvent>,
    mut out: MessageWriter<ServerStandState>,
    // `Option`: a client built without the player plugin holds no mover.
    player: Option<ResMut<Player>>,
    me: Query<&crate::net::ObjectStore, With<SelfPlayer>>,
    mut barber: crate::ui_barber::BarberStand,
    mut asks: MessageWriter<super::StandStateRequest>,
) {
    if let SessionEvent::StandStateUpdate { state } = ev {
        // With our player held (`0x73f571`).
        let (Some(mut player), Ok(store)) = (player, me.single()) else {
            return;
        };
        super::posture::server_stand_state(
            &mut player,
            state,
            store.0.unit_stand_state(),
            &mut barber,
            &mut asks,
        );
        out.write(ServerStandState { state });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Guid;

    const ME: u64 = 0x42;

    /// The request's answers after `index_me` (our create's step) and the request in one drain.
    fn answers(index_me: bool) -> Vec<u32> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<GuidIndex>();
        register(&mut app);
        if index_me {
            // What our create's step writes; no tag yet, since that runs a frame later.
            let e = app.world_mut().spawn(Guid(ME)).id();
            app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, e);
        }
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::TimeSyncReq { counter: 7 }],
        );
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::TimeSyncResp { counter, .. } => Some(counter),
                _ => None,
            })
            .collect()
    }

    /// `0x6dc047` tests the active player's object, held from its create's step: a request in the
    /// same drain as our create is answered, one with no object is not.
    #[test]
    fn a_time_sync_behind_our_create_is_answered_before_the_tag() {
        assert_eq!(answers(true), vec![7]);
        assert!(answers(false).is_empty());
    }
}
