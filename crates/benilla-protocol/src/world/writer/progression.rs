//! The talent and achievement sends: spending points, confirming a respec, inspecting a list.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Ask for a player's achievements (`CMSG_QUERY_INSPECT_ACHIEVEMENTS`); the server answers
    /// `SMSG_RESPOND_INSPECT_ACHIEVEMENTS`.
    pub fn query_inspect_achievements(&mut self, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_QUERY_INSPECT_ACHIEVEMENTS,
            &messages::query_inspect_achievements(guid),
        )
    }

    /// `CMSG_LEARN_TALENT`: a `Talent.dbc` row and the 0-based rank to learn up to. No reply:
    /// success is the rank spell's learn plus a fresh `PLAYER_CHARACTER_POINTS1`.
    pub fn learn_talent(&mut self, talent_id: u32, requested_rank: u32) -> Result<()> {
        self.send(
            opcode::CMSG_LEARN_TALENT,
            &messages::learn_talent(talent_id, requested_rank),
        )
    }

    /// `CMSG_LEARN_PREVIEW_TALENTS`: the preview mode's Learn, each `(talentId, rank)` whose
    /// preview rank passes its learned one. The server answers with a fresh `SMSG_TALENTS_INFO`.
    pub fn learn_preview_talents(&mut self, talents: &[(u32, u32)]) -> Result<()> {
        self.send(
            opcode::CMSG_LEARN_PREVIEW_TALENTS,
            &messages::learn_preview_talents(talents),
        )
    }

    /// `CMSG_REMOVE_GLYPH`: empty a glyph socket, 0-based.
    pub fn remove_glyph(&mut self, socket: u32) -> Result<()> {
        self.send(opcode::CMSG_REMOVE_GLYPH, &messages::remove_glyph(socket))
    }

    /// `CMSG_PET_LEARN_TALENT`: the pet's next rank of a talent; the server answers with the
    /// pet's `SMSG_TALENTS_INFO`.
    pub fn pet_learn_talent(&mut self, pet_guid: u64, talent_id: u32, rank: u32) -> Result<()> {
        self.send(
            opcode::CMSG_PET_LEARN_TALENT,
            &messages::pet_learn_talent(pet_guid, talent_id, rank),
        )
    }

    /// `CMSG_LEARN_PREVIEW_TALENTS_PET`: the pet's preview Learn.
    pub fn learn_preview_talents_pet(
        &mut self,
        pet_guid: u64,
        talents: &[(u32, u32)],
    ) -> Result<()> {
        self.send(
            opcode::CMSG_LEARN_PREVIEW_TALENTS_PET,
            &messages::learn_preview_talents_pet(pet_guid, talents),
        )
    }

    /// `MSG_TALENT_WIPE_CONFIRM`: the `CONFIRM_TALENT_WIPE` dialog's Accept; declining sends
    /// nothing. The server resets the talents and has the trainer cast spell 14867.
    pub fn talent_wipe_confirm(&mut self, trainer_guid: u64) -> Result<()> {
        self.send(
            opcode::MSG_TALENT_WIPE_CONFIRM,
            &messages::talent_wipe_confirm(trainer_guid),
        )
    }
}
