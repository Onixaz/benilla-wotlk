//! The action bar sends. Its contents are client-authoritative: the server stores the slots,
//! hands them back at login and never edits them in play.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Set one action-bar slot, or clear it with `packed == 0` (`CMSG_SET_ACTION_BUTTON`). A
    /// drag-swap is two sends, never atomic; the server never echoes our own edit.
    pub fn set_action_button(&mut self, button: u8, packed: u32) -> Result<()> {
        self.send(
            opcode::CMSG_SET_ACTION_BUTTON,
            &messages::set_action_button(button, packed),
        )
    }

    /// Dismiss the totem in `slot` (`CMSG_TOTEM_DESTROYED`); the server answers with nothing.
    pub fn totem_destroyed(&mut self, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_TOTEM_DESTROYED,
            &messages::totem_destroyed(slot),
        )
    }

    /// Post the extra bars' visibility byte, `PLAYER_FIELD_BYTES` byte 2. The server owns it: the
    /// reference never writes it locally, it holds once `SMSG_UPDATE_OBJECT` echoes it, and a
    /// disconnected send is dropped silently (`0x5ab637`).
    pub fn set_actionbar_toggles(&mut self, toggles: u8) -> Result<()> {
        self.send(
            opcode::CMSG_SET_ACTIONBAR_TOGGLES,
            &messages::set_actionbar_toggles(toggles),
        )
    }
}
