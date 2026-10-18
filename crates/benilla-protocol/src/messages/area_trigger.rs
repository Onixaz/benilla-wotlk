//! The area-trigger pair: the client's report of entering an `AreaTrigger.dbc` volume, and the
//! server's refusal text. The server decides what a trigger does; a teleport arrives as
//! `SMSG_TRANSFER_PENDING` and `SMSG_NEW_WORLD`, or a same-map `MSG_MOVE_TELEPORT_ACK`.

use std::io;

use crate::wire::{read_cstring, read_u32_le};

/// `CMSG_AREATRIGGER` (180, as `0x6dbe30` sends it): the `u32` `AreaTrigger.dbc` id. The server
/// obeys only a real row with our last reported position inside the volume, with no slop but an
/// inn's 5 yd, never while taxi-flying (AzerothCore `Handlers/MiscHandler.cpp:729`).
pub fn area_trigger(trigger_id: u32) -> Vec<u8> {
    trigger_id.to_le_bytes().to_vec()
}

/// `SMSG_AREA_TRIGGER_MESSAGE` (`0x2b8`, `Server/WorldSession.cpp:882-898`): why a trigger did
/// not fire, as a `u32` length (terminator included), which the reference ignores, then the text.
pub(super) fn read_area_trigger_message(r: &mut &[u8]) -> io::Result<String> {
    let _length = read_u32_le(r)?;
    read_cstring(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_is_the_bare_id() {
        assert_eq!(area_trigger(542), 542u32.to_le_bytes().to_vec());
    }

    #[test]
    fn message_reads_past_its_length_prefix() {
        let text = "You must be at least level 58 to enter.";
        let mut body = Vec::new();
        body.extend_from_slice(&(text.len() as u32 + 1).to_le_bytes());
        body.extend_from_slice(text.as_bytes());
        body.push(0);
        let mut r = body.as_slice();
        assert_eq!(read_area_trigger_message(&mut r).unwrap(), text);
        assert!(r.is_empty(), "the whole body is consumed");
    }
}
