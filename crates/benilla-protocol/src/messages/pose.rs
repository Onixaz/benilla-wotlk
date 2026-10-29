//! The client-volunteered pose bodies, sheath and stand state: the server stores what we send
//! unvalidated and echoes it through our unit fields, which every observer's body reads.

use std::io;

use crate::wire::read_u8;

/// Body of `CMSG_SETSHEATHED`: a `u32` sheath state (0 stowed, 1 melee, 2 ranged), stored
/// unvalidated into `UNIT_FIELD_BYTES_2` (vmangos `CombatHandler.cpp:80-87`).
pub fn set_sheathed(state: u32) -> Vec<u8> {
    state.to_le_bytes().to_vec()
}

/// Body of `CMSG_STANDSTATECHANGE`: a `u32` stand state, of which the server accepts only
/// 0 stand, 1 sit, 3 sleep and 8 kneel (`MiscHandler.cpp:437`), into `UNIT_FIELD_BYTES_1` byte 0.
pub fn stand_state_change(state: u32) -> Vec<u8> {
    state.to_le_bytes().to_vec()
}

/// Read `SMSG_STANDSTATE_UPDATE`: one `u8` stand state (vmangos `Unit.cpp:9541`), with no guid; the
/// reference applies it to the local player whatever unit the server meant.
pub(super) fn read_stand_state_update(r: &mut &[u8]) -> io::Result<u8> {
    read_u8(r)
}

#[cfg(test)]
mod inbound_tests {
    use crate::messages::{opcode, parse_server, ServerPacket};

    #[test]
    fn stand_state_update_decodes() {
        // One byte; a drink's sit is 1.
        match parse_server(opcode::SMSG_STANDSTATE_UPDATE, &[1]).unwrap() {
            ServerPacket::StandStateUpdate { state } => assert_eq!(state, 1),
            other => panic!("expected StandStateUpdate, got {}", other.name()),
        }
        assert!(
            parse_server(opcode::SMSG_STANDSTATE_UPDATE, &[]).is_err(),
            "a short body is an error"
        );
    }
}
