//! The world connection's header cipher and server-header framing, one per reference client:
//! 1.12.1's byte cipher with 4-byte server headers, 3.3.5a's RC4 with 4- or 5-byte ones.

use std::io::{self, Read};

use benilla_srp::vanilla_header::{DecrypterHalf, EncrypterHalf, HeaderCrypto};
use benilla_srp::wotlk_header::{Arc4, WotlkHeaderCrypto};

/// The cipher of client headers.
pub(crate) enum Encrypter {
    Vanilla(EncrypterHalf),
    Wotlk(Box<Arc4>),
}

impl Encrypter {
    pub(crate) fn encrypt(&mut self, header: &mut [u8]) {
        match self {
            Encrypter::Vanilla(e) => e.encrypt(header),
            Encrypter::Wotlk(e) => e.apply(header),
        }
    }
}

/// The cipher of server headers, which also knows their framing.
pub(crate) enum Decrypter {
    Vanilla(DecrypterHalf),
    Wotlk(Box<Arc4>),
}

impl Decrypter {
    fn decrypt(&mut self, header: &mut [u8]) {
        match self {
            Decrypter::Vanilla(d) => d.decrypt(header),
            Decrypter::Wotlk(d) => d.apply(header),
        }
    }

    /// 3.3.5a marks a size above `0x7FFF` with the first byte's `0x80` and widens the size to 23
    /// bits over three bytes; 1.12.1 has no wider form.
    fn is_large(&self, first: u8) -> bool {
        matches!(self, Decrypter::Wotlk(_)) && first & 0x80 != 0
    }
}

/// Both directions' ciphers.
pub(crate) struct Crypto {
    pub(crate) encrypter: Encrypter,
    pub(crate) decrypter: Decrypter,
}

impl From<HeaderCrypto> for Crypto {
    fn from(crypto: HeaderCrypto) -> Self {
        let (encrypt, decrypt) = crypto.split();
        Crypto {
            encrypter: Encrypter::Vanilla(encrypt),
            decrypter: Decrypter::Vanilla(decrypt),
        }
    }
}

impl From<WotlkHeaderCrypto> for Crypto {
    fn from(crypto: WotlkHeaderCrypto) -> Self {
        Crypto {
            encrypter: Encrypter::Wotlk(Box::new(crypto.encrypt)),
            decrypter: Decrypter::Wotlk(Box::new(crypto.decrypt)),
        }
    }
}

/// Read one server header, decrypting it once `decrypter` is set: the body length, opcode
/// excluded, and the opcode. The size is big-endian and counts the opcode; the opcode is
/// little-endian.
pub(super) fn read_server_header(
    stream: &mut impl Read,
    mut decrypter: Option<&mut Decrypter>,
) -> io::Result<(usize, u16)> {
    let mut first = [0u8; 1];
    stream.read_exact(&mut first)?;
    if let Some(d) = decrypter.as_deref_mut() {
        d.decrypt(&mut first);
    }
    let large = decrypter.as_deref().is_some_and(|d| d.is_large(first[0]));
    let mut rest = [0u8; 4];
    let rest = if large { &mut rest[..] } else { &mut rest[..3] };
    stream.read_exact(rest)?;
    if let Some(d) = decrypter {
        d.decrypt(rest);
    }
    let (size, opcode) = if large {
        let size = u32::from_be_bytes([0, first[0] & 0x7F, rest[0], rest[1]]);
        (size, u16::from_le_bytes([rest[2], rest[3]]))
    } else {
        let size = u32::from(u16::from_be_bytes([first[0], rest[0]]));
        (size, u16::from_le_bytes([rest[1], rest[2]]))
    };
    Ok((size.saturating_sub(2) as usize, opcode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_srp::SESSION_KEY_LENGTH;

    const KEY: [u8; SESSION_KEY_LENGTH] = [0x3C; SESSION_KEY_LENGTH];

    #[test]
    fn a_plain_header_is_size_and_opcode() {
        let wire = [0x00, 0x2A, 0xEC, 0x01];
        assert_eq!(
            read_server_header(&mut &wire[..], None).unwrap(),
            (0x28, 0x01EC)
        );
    }

    #[test]
    fn a_vanilla_header_is_four_bytes_even_with_the_top_bit_set() {
        let mut server = HeaderCrypto::from_session_key(KEY);
        let mut wire = [0x80, 0x02, 0xEE, 0x01];
        server.encrypter().encrypt(&mut wire);
        let mut client = Crypto::from(HeaderCrypto::from_session_key(KEY));
        assert_eq!(
            read_server_header(&mut &wire[..], Some(&mut client.decrypter)).unwrap(),
            (0x8000, 0x01EE)
        );
    }

    #[test]
    fn a_wotlk_header_widens_to_five_bytes_past_0x7fff() {
        let mut server = WotlkHeaderCrypto::server(&KEY);
        let mut client = Crypto::from(WotlkHeaderCrypto::client(&KEY));

        let mut small = [0x00, 0x0D, 0xEE, 0x01];
        server.encrypt.apply(&mut small);
        let mut large = [0x81, 0x23, 0x46, 0xA9, 0x00];
        server.encrypt.apply(&mut large);
        let wire: Vec<u8> = small.iter().chain(&large).copied().collect();

        let mut r = &wire[..];
        assert_eq!(
            read_server_header(&mut r, Some(&mut client.decrypter)).unwrap(),
            (0x0B, 0x01EE)
        );
        assert_eq!(
            read_server_header(&mut r, Some(&mut client.decrypter)).unwrap(),
            (0x01_2344, 0x00A9)
        );
        assert!(r.is_empty());
    }
}
