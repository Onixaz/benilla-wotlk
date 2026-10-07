//! The 3.3.5a world-packet header cipher. After `CMSG_AUTH_SESSION` every header, not the body,
//! runs through RC4, one stream per direction, each keyed by `HMAC-SHA1(seed, K)` under a fixed
//! 16-byte seed, with the first 1024 keystream bytes discarded (RC4-drop1024). The seeds and the
//! drop are the client's; the emulators mirror them (AzerothCore `AuthCrypt.cpp`).
//!
//! Client headers are 6 bytes as in 1.12 (`u16` BE size, `u32` LE opcode). Server headers are 4
//! bytes, or 5 when the size exceeds `0x7FFF`: the first byte then carries `0x80` and the size's
//! top 7 bits, giving a 23-bit big-endian size.

use sha1::{Digest, Sha1};

use crate::SESSION_KEY_LENGTH;

/// The seed of the client-to-server stream (AzerothCore's `ServerDecryptionKey`).
const CLIENT_TO_SERVER_SEED: [u8; 16] = [
    0xC2, 0xB3, 0x72, 0x3C, 0xC6, 0xAE, 0xD9, 0xB5, 0x34, 0x3C, 0x53, 0xEE, 0x2F, 0x43, 0x67, 0xCE,
];

/// The seed of the server-to-client stream (AzerothCore's `ServerEncryptionKey`).
const SERVER_TO_CLIENT_SEED: [u8; 16] = [
    0xCC, 0x98, 0xAE, 0x04, 0xE8, 0x97, 0xEA, 0xCA, 0x12, 0xDD, 0xC0, 0x93, 0x42, 0x91, 0x53, 0x57,
];

/// Keystream bytes discarded after keying.
const DROP: usize = 1024;

/// `HMAC-SHA1(key, message)` (RFC 2104) for a key shorter than SHA-1's 64-byte block.
fn hmac_sha1(key: &[u8; 16], message: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for (i, k) in key.iter().enumerate() {
        ipad[i] ^= k;
        opad[i] ^= k;
    }
    let inner: [u8; 20] = Sha1::new()
        .chain_update(ipad)
        .chain_update(message)
        .finalize()
        .into();
    Sha1::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// One direction's RC4 stream; encrypting and decrypting are the same XOR.
#[derive(Clone)]
pub struct Arc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl std::fmt::Debug for Arc4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Arc4 { .. }")
    }
}

impl Arc4 {
    /// RC4 keyed with `key`.
    fn keyed(key: &[u8]) -> Self {
        let mut state = [0u8; 256];
        for (i, s) in state.iter_mut().enumerate() {
            *s = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
            state.swap(i, j as usize);
        }
        Self { state, i: 0, j: 0 }
    }

    /// RC4 keyed with `key`, `DROP` bytes already discarded.
    fn new_dropped(key: &[u8]) -> Self {
        let mut arc4 = Self::keyed(key);
        arc4.apply(&mut [0u8; DROP]);
        arc4
    }

    /// The stream for `seed` under session key `K`.
    fn for_seed(seed: &[u8; 16], session_key: &[u8; SESSION_KEY_LENGTH]) -> Self {
        Self::new_dropped(&hmac_sha1(seed, session_key))
    }

    /// XOR `data` in place with the next keystream bytes.
    pub fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.state[self.i as usize]);
            self.state.swap(self.i as usize, self.j as usize);
            let k = self.state
                [self.state[self.i as usize].wrapping_add(self.state[self.j as usize]) as usize];
            *byte ^= k;
        }
    }
}

/// Both streams of one end of a connection: `encrypt` for what it sends, `decrypt` for what it
/// receives.
#[derive(Debug, Clone)]
pub struct WotlkHeaderCrypto {
    pub encrypt: Arc4,
    pub decrypt: Arc4,
}

impl WotlkHeaderCrypto {
    /// The client's end.
    pub fn client(session_key: &[u8; SESSION_KEY_LENGTH]) -> Self {
        Self {
            encrypt: Arc4::for_seed(&CLIENT_TO_SERVER_SEED, session_key),
            decrypt: Arc4::for_seed(&SERVER_TO_CLIENT_SEED, session_key),
        }
    }

    /// The server's end, for a fake world server that speaks real headers.
    pub fn server(session_key: &[u8; SESSION_KEY_LENGTH]) -> Self {
        Self {
            encrypt: Arc4::for_seed(&SERVER_TO_CLIENT_SEED, session_key),
            decrypt: Arc4::for_seed(&CLIENT_TO_SERVER_SEED, session_key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// RFC 2202 case 2, its 4-byte key ("Jefe") zero-padded to 16 bytes as HMAC pads it anyway.
    #[test]
    fn hmac_sha1_matches_rfc_2202() {
        let mut key = [0u8; 16];
        key[..4].copy_from_slice(b"Jefe");
        assert_eq!(
            hex(&hmac_sha1(&key, b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    /// RFC 6229's vector for the 40-bit key `0102030405`, at offsets 0 and 1024.
    #[test]
    fn arc4_matches_rfc_6229_before_and_after_the_drop() {
        let mut head = [0u8; 16];
        Arc4::keyed(&[1, 2, 3, 4, 5]).apply(&mut head);
        assert_eq!(hex(&head), "b2396305f03dc027ccc3524a0a1118a8");

        let mut at_1024 = [0u8; 16];
        Arc4::new_dropped(&[1, 2, 3, 4, 5]).apply(&mut at_1024);
        assert_eq!(hex(&at_1024), "30abbcc7c20b01609f23ee2d5f6bb7df");
    }

    #[test]
    fn a_server_header_round_trips_between_the_two_ends() {
        let key = [0x5Au8; SESSION_KEY_LENGTH];
        let mut server = WotlkHeaderCrypto::server(&key);
        let mut client = WotlkHeaderCrypto::client(&key);
        for header in [[0x00, 0x05, 0xEE, 0x01], [0x12, 0x34, 0x56, 0x78]] {
            let mut wire = header;
            server.encrypt.apply(&mut wire);
            assert_ne!(wire, header);
            client.decrypt.apply(&mut wire);
            assert_eq!(wire, header);
        }
        let mut wire = [0x00, 0x08, 0xDC, 0x01, 0x00, 0x00];
        client.encrypt.apply(&mut wire);
        server.decrypt.apply(&mut wire);
        assert_eq!(wire, [0x00, 0x08, 0xDC, 0x01, 0x00, 0x00]);
    }
}
