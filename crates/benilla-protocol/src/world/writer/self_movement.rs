//! Our own mover's sends: the `MSG_MOVE_*` stream and the acks the server demands of a player
//! mover. Every ack is mandatory: unacked, the server force-resolves the change on a timeout and
//! flags its anticheat, or completes a teleport about 20 s late.

use anyhow::Result;

use crate::messages::{self, opcode, JumpInfo, MoveMode, TransportPose};
use crate::world::movement::{client_uptime_ms, movement_info};

use super::WorldWriter;

impl WorldWriter {
    /// One `MSG_MOVE_*` packet, its opcode picked per movement transition as the reference does.
    /// `flags` may set only bits with a serialized tail: directional, turn and walk bits, and
    /// `JUMPING`, `SWIMMING` and `ON_TRANSPORT` with `jump`, `pitch` and `transport`.
    pub fn send_movement(
        &mut self,
        opcode: u16,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        // The serializer writes each tail only when its flag is set, so flag and value must agree.
        info.pitch = pitch;
        info.fall_time = fall_time;
        info.jump = jump;
        info.transport = transport;
        self.send(opcode, &messages::movement(self.mover, &info))
    }

    /// `CMSG_SET_ACTIVE_MOVER`, full guid: the client's claim after `SMSG_CLIENT_CONTROL_UPDATE`,
    /// sent at login for our own body and again on possession. Every later movement packet
    /// names this unit, as the server drops one naming any other.
    pub fn set_active_mover(&mut self, guid: u64) -> Result<()> {
        self.mover = guid;
        self.send(opcode::CMSG_SET_ACTIVE_MOVER, &messages::full_guid(guid))
    }

    /// `CMSG_FAR_SIGHT`, one `u8`: 1 when the view attaches, 0 when it releases (reference:
    /// `0x5ee290`). It names no object; the server reads its own `PLAYER_FARSIGHT`.
    pub fn far_sight(&mut self, engage: bool) -> Result<()> {
        self.send(opcode::CMSG_FAR_SIGHT, &[u8::from(engage)])
    }

    /// `CMSG_MOVE_NOT_ACTIVE_MOVER`: the released mover's packed guid, then its parting pose,
    /// which the server re-broadcasts to observers as a stop (`HandleMoveNotActiveMover`).
    pub fn move_not_active_mover(
        &mut self,
        guid: u64,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        fall_time: u32,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        info.fall_time = fall_time;
        self.send(
            opcode::CMSG_MOVE_NOT_ACTIVE_MOVER,
            &messages::movement(guid, &info),
        )
    }

    /// `CMSG_MOVE_TIME_SKIPPED`: the server adds `lag_ms` to its copy of our movement clock and,
    /// just after we board a transport, re-sends that transport's create update.
    pub fn move_time_skipped(&mut self, guid: u64, lag_ms: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MOVE_TIME_SKIPPED,
            &messages::move_time_skipped(guid, lag_ms),
        )
    }

    /// `MSG_MOVE_SET_PITCH`, `_START_PITCH_UP`, `_START_PITCH_DOWN` or `_STOP_PITCH`: the mover's
    /// pose with its extra flags and pitch, which an aiming vehicle's `ALWAYS_ALLOW_PITCHING`
    /// carries (`0x6ee460`'s move event).
    #[allow(clippy::too_many_arguments)] // the movement report's fields, one each
    pub fn move_pitch(
        &mut self,
        opcode: u16,
        mover: u64,
        flags: u32,
        flags2: u16,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        info.flags2 = flags2;
        info.pitch = pitch;
        self.send(opcode, &messages::movement(mover, &info))
    }

    /// `CMSG_REQUEST_VEHICLE_EXIT`, `_PREV_SEAT` or `_NEXT_SEAT`: one of the three empty seat
    /// requests for a vehicle we ride and do not control.
    pub fn request_vehicle(&mut self, opcode: u16) -> Result<()> {
        self.send(opcode, &[])
    }

    /// `CMSG_REQUEST_VEHICLE_SWITCH_SEAT`: `vehicle`'s `seat`.
    pub fn request_vehicle_switch_seat(&mut self, vehicle: u64, seat: u8) -> Result<()> {
        self.send(
            opcode::CMSG_REQUEST_VEHICLE_SWITCH_SEAT,
            &messages::request_vehicle_switch_seat(vehicle, seat),
        )
    }

    /// `CMSG_UPDATE_PROJECTILE_POSITION`: where `caster`'s trajectory missile `cast_id` hit, raw
    /// WoW coordinates.
    pub fn update_projectile_position(
        &mut self,
        caster: u64,
        spell: u32,
        cast_id: u8,
        at: [f32; 3],
    ) -> Result<()> {
        let [x, y, z] = at;
        self.send(
            opcode::CMSG_UPDATE_PROJECTILE_POSITION,
            &messages::update_projectile_position(
                caster,
                spell,
                cast_id,
                crate::wire::Vector3d { x, y, z },
            ),
        )
    }

    /// `CMSG_EJECT_PASSENGER` or `CMSG_PLAYER_VEHICLE_ENTER`, each a bare guid.
    pub fn vehicle_guid(&mut self, opcode: u16, guid: u64) -> Result<()> {
        self.send(opcode, &messages::vehicle_guid(guid))
    }

    /// `CMSG_DISMISS_CONTROLLED_VEHICLE` with our pose, or with `seat`
    /// `CMSG_CHANGE_SEATS_ON_CONTROLLED_VEHICLE`.
    pub fn controlled_vehicle(
        &mut self,
        mover: u64,
        flags: u32,
        pose: ([f32; 3], f32),
        seat: Option<(u64, u8)>,
    ) -> Result<()> {
        let info = movement_info(pose.0, pose.1, flags);
        match seat {
            None => self.send(
                opcode::CMSG_DISMISS_CONTROLLED_VEHICLE,
                &messages::dismiss_controlled_vehicle(mover, &info),
            ),
            Some((vehicle, seat)) => self.send(
                opcode::CMSG_CHANGE_SEATS_ON_CONTROLLED_VEHICLE,
                &messages::change_seats_on_controlled_vehicle(mover, &info, vehicle, seat),
            ),
        }
    }

    /// `CMSG_TIME_SYNC_RESP`: answer a time-sync request with our movement clock at `client_ms`.
    pub fn time_sync_resp(&mut self, counter: u32, client_ms: u32) -> Result<()> {
        self.send(
            opcode::CMSG_TIME_SYNC_RESP,
            &messages::time_sync_resp(counter, client_ms),
        )
    }

    /// `CMSG_MOVE_SPLINE_DONE`: owed, at rest, when an `SMSG_MONSTER_MOVE` on our own guid ends.
    /// The server checks `spline_id` against its newest, then relocates us and tells observers.
    pub fn move_spline_done(
        &mut self,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        spline_id: u32,
    ) -> Result<()> {
        let info = movement_info(pos, orientation, flags);
        self.send(
            opcode::CMSG_MOVE_SPLINE_DONE,
            &messages::move_spline_done(self.mover, &info, spline_id),
        )
    }

    /// `MSG_MOVE_WORLDPORT_ACK`, empty: confirms `SMSG_NEW_WORLD` so the object stream resumes.
    pub fn worldport_ack(&mut self) -> Result<()> {
        self.send(opcode::MSG_MOVE_WORLDPORT_ACK, &[])
    }

    /// `CMSG_FORCE_*_SPEED_CHANGE_ACK`: echoes `counter` and the exact `speed` with our live pose,
    /// which the server relocates us to. Unacked, it force-resolves after ~4 s and flags anticheat.
    pub fn force_speed_change_ack(
        &mut self,
        kind: messages::SpeedKind,
        guid: u64,
        counter: u32,
        speed: f32,
        flags: u32,
        pos: [f32; 3],
        orientation: f32,
        pitch: f32,
        fall_time: u32,
        jump: Option<JumpInfo>,
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pos, orientation, flags);
        info.pitch = pitch;
        info.fall_time = fall_time;
        info.jump = jump;
        info.transport = transport;
        self.send(
            kind.ack_opcode(),
            &messages::force_speed_ack(guid, counter, &info, speed),
        )
    }

    /// `MSG_MOVE_TELEPORT_ACK`: completes a near-teleport at once; without it the server finishes
    /// on a ~20 s fallback.
    pub fn teleport_ack(&mut self, guid: u64, counter: u32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_TELEPORT_ACK,
            &messages::teleport_ack(guid, counter, client_uptime_ms()),
        )
    }

    /// `CMSG_MOVE_SET_COLLISION_HGT_ACK`: the grant's counter, our pose and the height it set.
    pub fn collision_height_ack(
        &mut self,
        guid: u64,
        counter: u32,
        height: f32,
        flags: u32,
        pose: ([f32; 3], f32),
    ) -> Result<()> {
        let info = movement_info(pose.0, pose.1, flags);
        self.send(
            opcode::CMSG_MOVE_SET_COLLISION_HGT_ACK,
            &messages::collision_height_ack(guid, counter, &info, height),
        )
    }

    /// Ack a granted root, water-walk, feather-fall, hover or can-fly: guid, echoed counter and
    /// our pose, plus a trailing `f32` apply for all but root. Unacked, the server never applies it.
    /// `flags` must carry the mode's bit (a root apply-ack without it is a kick, vmangos
    /// `MovementHandler.cpp:715-722`) and no moving bit beside `MOVEFLAG_ROOT`; turn bits are fine.
    pub fn move_mode_ack(
        &mut self,
        guid: u64,
        counter: u32,
        mode: MoveMode,
        apply: bool,
        flags: u32,
        pose: ([f32; 3], f32),
    ) -> Result<()> {
        let info = movement_info(pose.0, pose.1, flags);
        let trailing = mode.ack_carries_apply().then_some(apply);
        self.send(
            mode.ack_opcode(apply),
            &messages::move_flag_ack(guid, counter, &info, trailing),
        )
    }

    /// `CMSG_MOVE_KNOCK_BACK_ACK`: our pose at launch, with the server's `launch` echoed as the
    /// jump tail. `flags` must carry `MOVEFLAG_JUMPING` and `launch` must match within 0.01, or
    /// vmangos rejects the ack and observers never see the knockback.
    pub fn knock_back_ack(
        &mut self,
        guid: u64,
        counter: u32,
        launch: JumpInfo,
        flags: u32,
        pose: ([f32; 3], f32),
        transport: Option<TransportPose>,
    ) -> Result<()> {
        let mut info = movement_info(pose.0, pose.1, flags);
        info.fall_time = 0;
        info.jump = Some(launch);
        info.transport = transport;
        self.send(
            opcode::CMSG_MOVE_KNOCK_BACK_ACK,
            &messages::knock_back_ack(guid, counter, &info),
        )
    }
}
