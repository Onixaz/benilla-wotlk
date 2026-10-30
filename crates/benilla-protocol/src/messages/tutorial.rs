//! Tutorial wire: one inbound bank of bits and the client's three tutorial sends. The 1.12
//! client sizes its banks from every remaining byte of the packet; vmangos sends 32 bytes.

use std::io::{self, Read};

/// `SMSG_TUTORIAL_FLAGS`: bit `id` is `bytes[id >> 3] & (1 << (id & 7))`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TutorialFlags {
    pub bytes: Vec<u8>,
}

pub(super) fn read_tutorial_flags(r: &mut impl Read) -> io::Result<TutorialFlags> {
    let mut bytes = Vec::new();
    r.read_to_end(&mut bytes)?;
    Ok(TutorialFlags { bytes })
}

/// `CMSG_TUTORIAL_FLAG`: the 0-based id as a `u32`.
pub fn tutorial_flag(id: u32) -> Vec<u8> {
    id.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bank_is_every_remaining_byte_and_the_flag_is_one_dword() {
        let body = [0x01u8, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        let f = read_tutorial_flags(&mut body.as_slice()).unwrap();
        assert_eq!(f.bytes, body.to_vec());
        let f = read_tutorial_flags(&mut [].as_slice()).unwrap();
        assert!(f.bytes.is_empty(), "an empty body is an empty bank");
        assert_eq!(tutorial_flag(41), vec![41, 0, 0, 0]);
    }
}
