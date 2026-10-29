//! The barber shop's send.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_ALTER_APPEARANCE`: `ApplyBarberShopStyle()`, once the look differs and the purse
    /// covers the price.
    pub fn alter_appearance(&mut self, styles: [u32; 4]) -> Result<()> {
        let [hair_style, hair_color, facial_hair, skin] = styles;
        self.send(
            opcode::CMSG_ALTER_APPEARANCE,
            &messages::alter_appearance(hair_style, hair_color, facial_hair, skin),
        )
    }
}
