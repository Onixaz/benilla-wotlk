//! The empty-bodied toggles that ask the server to flip a bit of our own `PLAYER_FLAGS`. There is
//! no ack, only the next descriptor update, so to reach a given state send only when it differs.

use anyhow::Result;

use crate::messages::opcode;

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_TOGGLE_PVP` empty, `TogglePVP()` (`0x516840`): the server flips the desired bit;
    /// flagging off clears only the wish, and the flag drops after the server's timer.
    pub fn toggle_pvp(&mut self) -> Result<()> {
        self.send(opcode::CMSG_TOGGLE_PVP, &[])
    }

    /// `CMSG_TOGGLE_PVP` with one byte, `SetPVP(state)` (`0x5168b0`): the state requested, the
    /// argument's number truncated to a byte and 0 for a non-number.
    pub fn set_pvp(&mut self, state: u8) -> Result<()> {
        self.send(opcode::CMSG_TOGGLE_PVP, &[state])
    }

    /// `CMSG_TOGGLE_HELM`: answered by the `PLAYER_FLAGS` `HIDE_HELM` bit, which every client in
    /// range dresses our body from.
    pub fn toggle_helm(&mut self) -> Result<()> {
        self.send(opcode::CMSG_TOGGLE_HELM, &[])
    }

    /// `CMSG_TOGGLE_CLOAK`: the cloak half of [`Self::toggle_helm`].
    pub fn toggle_cloak(&mut self) -> Result<()> {
        self.send(opcode::CMSG_TOGGLE_CLOAK, &[])
    }
}
