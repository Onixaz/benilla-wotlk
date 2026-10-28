//! The two sends that set our selection, picking a unit and inspecting a player, both carrying a
//! raw 8-byte guid.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_SET_SELECTION`: a raw `u64`, not a packed guid (`SetSelection::ReadFromWorldPacket`);
    /// 0 clears it. The server stores it in our `UNIT_FIELD_TARGET` for observers.
    pub fn set_selection(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_SET_SELECTION, &messages::full_guid(guid))
    }

    /// `CMSG_GET_MIRRORIMAGE_DATA` (`0x716f10`): the clone's full guid; answered by
    /// `SMSG_MIRRORIMAGE_DATA`.
    pub fn get_mirror_image_data(&mut self, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GET_MIRRORIMAGE_DATA,
            &messages::full_guid(guid),
        )
    }

    /// `CMSG_INSPECT`: it also sets our selection (`MiscHandler.cpp:945`), so the reference sends
    /// it though the window paints from streamed fields; the `SMSG_INSPECT` reply is just the guid.
    pub fn inspect(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_INSPECT, &messages::full_guid(guid))
    }
}
