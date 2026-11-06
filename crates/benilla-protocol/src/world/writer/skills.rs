//! The skills pane's one send, the abandon; skills are granted server-side as field updates.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_UNLEARN_SKILL`: no ack; the removal comes back as a `PLAYER_SKILL_INFO` update.
    pub fn unlearn_skill(&mut self, skill_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_UNLEARN_SKILL,
            &messages::unlearn_skill(skill_id),
        )
    }

    /// `CMSG_SKILL_BUY_RANK`: the pending points per skill line (`AcceptSkillUps`, `0x5ce5d0`).
    pub fn skill_buy_rank(&mut self, lines: &[(u32, u32)]) -> Result<()> {
        let mut body = Vec::with_capacity(4 + lines.len() * 8);
        body.extend_from_slice(&(lines.len() as u32).to_le_bytes());
        for (line, points) in lines {
            body.extend_from_slice(&line.to_le_bytes());
            body.extend_from_slice(&points.to_le_bytes());
        }
        self.send(opcode::CMSG_SKILL_BUY_RANK, &body)
    }

    /// `CMSG_SKILL_BUY_STEP`: buy a skill line's next tier (`BuySkillTier`, `0x5ce6f0`).
    pub fn skill_buy_step(&mut self, line: u32) -> Result<()> {
        self.send(opcode::CMSG_SKILL_BUY_STEP, &line.to_le_bytes())
    }
}
