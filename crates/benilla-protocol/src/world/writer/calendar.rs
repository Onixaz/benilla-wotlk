//! The calendar's sends, `CGCalendar`'s and `CGCalendarEvent`'s (`0x5b7820`-`0x5f32b0`).

use anyhow::Result;

use crate::messages::CalendarCommand;

use super::WorldWriter;

impl WorldWriter {
    /// One calendar send, encoded as its 12340 sender writes it.
    pub fn calendar(&mut self, command: &CalendarCommand) -> Result<()> {
        let (opcode, body) = command.encode();
        self.send(opcode, &body)
    }
}
