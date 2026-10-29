//! The client-volunteered pose sends: sheath, stand state and the mount flourish. None gets a
//! reply; other clients read the pose from our descriptor.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_SETSHEATHED`: `state` 0 stowed, 1 melee drawn, 2 ranged drawn, stored as our
    /// `UNIT_FIELD_BYTES_2` sheath byte. The reference setter sends it whenever `bFireEvent` is
    /// set.
    pub fn set_sheathed(&mut self, state: u32) -> Result<()> {
        self.send(opcode::CMSG_SETSHEATHED, &messages::set_sheathed(state))
    }

    /// `CMSG_STANDSTATECHANGE`: vmangos accepts only 0 stand, 1 sit, 3 sleep and 8 kneel, stored
    /// as `UNIT_FIELD_BYTES_1` byte 0.
    pub fn stand_state_change(&mut self, state: u32) -> Result<()> {
        self.send(
            opcode::CMSG_STANDSTATECHANGE,
            &messages::stand_state_change(state),
        )
    }

    /// `CMSG_MOUNTSPECIAL_ANIM`, empty: the mounted space-bar flourish. The sender plays
    /// `MountSpecial` (94) itself at send time and ignores the broadcast echo.
    pub fn mount_special(&mut self) -> Result<()> {
        self.send(opcode::CMSG_MOUNTSPECIAL_ANIM, &[])
    }
}
