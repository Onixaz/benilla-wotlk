//! The realmd (login) wire protocol, version 3 in 1.12.1 and 8 in 3.3.5a: logon challenge, logon
//! proof and realm list, the three exchanges [`crate::logon`] performs. Login packets are not
//! header-encrypted and each is self-delimiting, so they are read in request/response order.

use std::io::{Read, Write};
use std::net::Ipv4Addr;

use anyhow::{bail, Result};
use benilla_expansions::Expansion;
use sha1::{Digest, Sha1};

use crate::wire::{read_array, read_cstring, read_f32_le, read_u16_le, read_u32_le, read_u8};
use crate::RealmInfo;

const CMD_AUTH_LOGON_CHALLENGE: u8 = 0x00;
const CMD_AUTH_LOGON_PROOF: u8 = 0x01;
const CMD_REALM_LIST: u8 = 0x10;

const GAME_NAME_WOW: u32 = 0x0057_6f57; // "WoW\0" little-endian
const PLATFORM_X86: u32 = 0x0078_3836; // "x86\0"
                                       // Tags are little-endian u32s, so they reach the
                                       // wire reversed (`0x0057696e` is `n i W \0`) and
                                       // realmd reverses them back. vmangos fails a session
                                       // on anything but "Win" or "OSX".
const OS_WINDOWS: u32 = 0x0057_696e; // "Win\0"
const OS_MACOS: u32 = 0x004F_5358; // "OSX\0"

/// The host's OS tag: `OSX` on macOS, else `Win`, as there was no 1.12 Linux client. Servers act
/// on it: vmangos picks `WardenWin` or `WardenMac`, and realmd checks the build (`FindBuildInfo`).
const fn client_os() -> u32 {
    if cfg!(target_os = "macos") {
        OS_MACOS
    } else {
        OS_WINDOWS
    }
}
const LOCALE_EN_US: u32 = 0x656e_5553; // "enUS"

/// A non-success auth result byte, typed so the app can map it to its `AUTH_*` glue string; it
/// survives [`crate::logon`]'s `anyhow` contexts via `downcast_ref`. vmangos answers an unknown
/// account and a wrong password alike with 0x04 (`WOW_FAIL_UNKNOWN_ACCOUNT`, `AuthCodes.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthReject {
    /// The grunt result byte (`WOW_FAIL_*`).
    pub code: u8,
}

impl std::fmt::Display for AuthReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server rejected logon: result {:#04x}", self.code)
    }
}

impl std::error::Error for AuthReject {}

/// The SRP6 inputs from a successful `CMD_AUTH_LOGON_CHALLENGE_Server`.
pub struct ChallengeReply {
    pub server_public_key: [u8; 32],
    pub generator: u8,
    pub large_safe_prime: [u8; 32],
    pub salt: [u8; 32],
    /// The version challenge answered by `crc_hash` ([`version_proof`]); not an SRP6 input.
    pub crc_salt: [u8; 16],
}

/// Send `CMD_AUTH_LOGON_CHALLENGE_Client` as `expansion`'s client does: its protocol version,
/// version and build. `account_name` must be uppercased, as the SRP6 hashes use it.
pub fn write_logon_challenge(
    w: &mut impl Write,
    account_name: &str,
    expansion: Expansion,
) -> std::io::Result<()> {
    let profile = expansion.profile();
    // Everything after the 2-byte size field, assembled first so we can prefix its length.
    let mut body = Vec::with_capacity(34 + account_name.len());
    body.extend_from_slice(&GAME_NAME_WOW.to_le_bytes());
    body.extend_from_slice(&profile.logon.version);
    body.extend_from_slice(&profile.build.to_le_bytes());
    body.extend_from_slice(&PLATFORM_X86.to_le_bytes());
    body.extend_from_slice(&client_os().to_le_bytes());
    body.extend_from_slice(&LOCALE_EN_US.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // utc_timezone_offset
    body.extend_from_slice(&Ipv4Addr::LOCALHOST.octets()); // client_ip_address (network order)
    body.push(account_name.len() as u8);
    body.extend_from_slice(account_name.as_bytes());

    let mut packet = Vec::with_capacity(4 + body.len());
    packet.push(CMD_AUTH_LOGON_CHALLENGE);
    packet.push(profile.logon.protocol_version);
    packet.extend_from_slice(&(body.len() as u16).to_le_bytes());
    packet.extend_from_slice(&body);
    w.write_all(&packet)
}

/// Read the challenge reply; a non-success result is an [`AuthReject`].
pub fn read_challenge_reply(r: &mut impl Read) -> Result<ChallengeReply> {
    let opcode = read_u8(r)?;
    if opcode != CMD_AUTH_LOGON_CHALLENGE {
        bail!("expected CMD_AUTH_LOGON_CHALLENGE (0x00), got {opcode:#x}");
    }
    let _protocol_version = read_u8(r)?;
    let result = read_u8(r)?;
    if result != 0 {
        return Err(AuthReject { code: result }.into());
    }
    let server_public_key = read_array::<32>(r)?;
    let generator_len = read_u8(r)?;
    let mut generator = vec![0u8; generator_len as usize];
    r.read_exact(&mut generator)?;
    let generator = match generator.first() {
        Some(&g) => g,
        None => bail!("server sent an empty generator"),
    };
    let prime_len = read_u8(r)? as usize;
    let mut prime = vec![0u8; prime_len];
    r.read_exact(&mut prime)?;
    let large_safe_prime: [u8; 32] = prime
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("large safe prime was {prime_len} bytes, expected 32"))?;
    let salt = read_array::<32>(r)?;
    // Read to the end: login packets are not length-framed, so leftover bytes desync the next
    // read. `security_flag` and the PIN block are unused but must be consumed.
    let crc_salt = read_array::<16>(r)?;
    let security_flag = read_u8(r)?;
    if security_flag & 0x01 != 0 {
        // PIN: pin_grid_seed (u32) + pin_salt[16]; vmangos always sends security_flag 0.
        let _pin_grid_seed = read_u32_le(r)?;
        let _pin_salt = read_array::<16>(r)?;
    }
    Ok(ChallengeReply {
        server_public_key,
        generator,
        large_safe_prime,
        salt,
        crc_salt,
    })
}

// --- the version (client-integrity) proof --------------------------------------------------------
//
// The proof's `crc_hash` answers the challenge's `crc_salt`: the 1.12 client hashes its own
// binaries under that salt into a 20-byte digest `H` and sends `SHA1(A ‖ H)`. realmd recomputes it
// from a stored `H` (`AuthSocket::VerifyVersion`) and, under `StrictVersionCheck = 1` (the shipped
// `realmd.conf.example` value), refuses a mismatch with `WOW_FAIL_VERSION_INVALID` (0x09).

/// The `crc_salt` every mangos-family realmd sends: a constant, not a nonce (vmangos
/// `AuthSocket.cpp:65`, cmangos `AuthSocket.cpp:187`), so the answer is a per-build constant.
const MANGOS_VERSION_CHALLENGE: [u8; 16] = [
    0xba, 0xa3, 0x1e, 0x99, 0xa0, 0x0b, 0x21, 0x57, 0xfc, 0x37, 0x3f, 0xb3, 0x69, 0xcd, 0xd2, 0xf1,
];

/// The integrity digest `H` for `crc_salt`, known only for [`MANGOS_VERSION_CHALLENGE`]: the
/// expansion profile's, the Mac client's on macOS because servers pick `H` by the OS tag.
/// Deviation: a stored per-OS constant, not the reference's HMAC over its own executables
/// (`0x5b1170`), because every mangos-family realmd issues this one salt.
fn integrity_hash(crc_salt: &[u8; 16], expansion: Expansion) -> Option<[u8; 20]> {
    if *crc_salt != MANGOS_VERSION_CHALLENGE {
        return None;
    }
    let logon = &expansion.profile().logon;
    Some(if client_os() == OS_MACOS {
        logon.integrity_macos
    } else {
        logon.integrity_windows
    })
}

/// The proof's `crc_hash`: `SHA1(A ‖ H)` over `A`'s wire bytes (realmd hashes `lp->A` as
/// received), or twenty zeros for an unknown salt, which only a strict server refuses.
pub fn version_proof(
    crc_salt: &[u8; 16],
    client_public_key: &[u8; 32],
    expansion: Expansion,
) -> [u8; 20] {
    match integrity_hash(crc_salt, expansion) {
        Some(h) => {
            let mut sha = Sha1::new();
            sha.update(client_public_key);
            sha.update(h);
            sha.finalize().into()
        }
        None => [0u8; 20],
    }
}

/// Send `CMD_AUTH_LOGON_PROOF_Client`, computing `crc_hash` from the challenge's `crc_salt`.
pub fn write_logon_proof(
    w: &mut impl Write,
    client_public_key: &[u8; 32],
    client_proof: &[u8; 20],
    crc_salt: &[u8; 16],
    expansion: Expansion,
) -> std::io::Result<()> {
    let mut packet = Vec::with_capacity(1 + 32 + 20 + 20 + 1 + 1);
    packet.push(CMD_AUTH_LOGON_PROOF);
    packet.extend_from_slice(client_public_key);
    packet.extend_from_slice(client_proof);
    packet.extend_from_slice(&version_proof(crc_salt, client_public_key, expansion));
    packet.push(0); // number_of_telemetry_keys
    packet.push(0); // security_flag = None
    w.write_all(&packet)
}

/// Read the proof reply: the server's `M2`, or an [`AuthReject`]. After `M2`, 1.12.1's success
/// reply carries a `u32` hardware survey id; 3.3.5a's `u32 account_flags, u32 hardware_survey_id,
/// u16 login_flags`.
pub fn read_proof_reply(r: &mut impl Read, expansion: Expansion) -> Result<[u8; 20]> {
    let opcode = read_u8(r)?;
    if opcode != CMD_AUTH_LOGON_PROOF {
        bail!("expected CMD_AUTH_LOGON_PROOF (0x01), got {opcode:#x}");
    }
    let result = read_u8(r)?;
    if result != 0 {
        // A wrong password fails here, where `M1` does not verify: vmangos answers 0x04.
        return Err(AuthReject { code: result }.into());
    }
    let server_proof = read_array::<20>(r)?;
    match expansion {
        Expansion::Classic => {
            let _hardware_survey_id = read_u32_le(r)?;
        }
        Expansion::Wotlk => {
            let _account_flags = read_u32_le(r)?;
            let _hardware_survey_id = read_u32_le(r)?;
            let _login_flags = read_u16_le(r)?;
        }
    }
    Ok(server_proof)
}

/// Send `CMD_REALM_LIST_Client` (opcode + a `u32` padding of 0).
pub fn write_realm_list_request(w: &mut impl Write) -> std::io::Result<()> {
    let mut packet = [0u8; 5];
    packet[0] = CMD_REALM_LIST;
    w.write_all(&packet)
}

/// The magic populations the 1.12 realm-list parser (`0x5b2230`) rewrites, as `(population sent,
/// population rewritten, flag OR'd)`: Recommended, New and Full travel as these exact float bit
/// patterns, not as wire flags.
pub(crate) const MAGIC_POPULATIONS: [(u32, u32, u8); 3] = [
    (0x4416_0000, 0x0000_0000, 0x20), // 600.0 → 0.0,   Recommended
    (0x4348_0000, 0x3a83_126f, 0x40), // 200.0 → 0.001, New
    (0x43c8_0000, 0x4100_0000, 0x80), // 400.0 → 8.0,   Full
];

/// The 3.3.5a realm flag that appends the realm's version and build to its record.
const REALM_FLAG_SPECIFY_BUILD: u8 = 0x04;

/// Read `CMD_REALM_LIST_Server` into the advertised realms. 1.12.1 counts realms in a `u8` and
/// types each with a `u32`; 3.3.5a counts in a `u16`, types with a `u8` followed by a `u8` lock,
/// and appends `u8 major, minor, patch, u16 build` to a realm flagged
/// [`REALM_FLAG_SPECIFY_BUILD`]. For 1.12.1, [`MAGIC_POPULATIONS`] are rewritten here as its
/// parser does: the realm-list screen averages every realm's population, and an unswapped
/// Recommended `600.0` would skew that mean for every row.
pub fn read_realm_list(r: &mut impl Read, expansion: Expansion) -> Result<Vec<RealmInfo>> {
    let opcode = read_u8(r)?;
    if opcode != CMD_REALM_LIST {
        bail!("expected CMD_REALM_LIST (0x10), got {opcode:#x}");
    }
    let _size = read_u16_le(r)?;
    let _header_padding = read_u32_le(r)?;
    let number_of_realms = match expansion {
        Expansion::Classic => u16::from(read_u8(r)?),
        Expansion::Wotlk => read_u16_le(r)?,
    };
    let mut realms = Vec::with_capacity(usize::from(number_of_realms.min(256)));
    for _ in 0..number_of_realms {
        let realm_type = match expansion {
            Expansion::Classic => read_u32_le(r)?,
            Expansion::Wotlk => {
                let realm_type = u32::from(read_u8(r)?);
                let _locked = read_u8(r)?;
                realm_type
            }
        };
        let mut flags = read_u8(r)?;
        let name = read_cstring(r)?;
        let address = read_cstring(r)?;
        let mut population = read_f32_le(r)?;
        let characters = read_u8(r)?;
        let category = read_u8(r)?;
        let id = read_u8(r)?;
        match expansion {
            Expansion::Classic => {
                // The sentinel swap, before anyone can average this number.
                if let Some(&(_, rewritten, bit)) = MAGIC_POPULATIONS
                    .iter()
                    .find(|(magic, _, _)| *magic == population.to_bits())
                {
                    population = f32::from_bits(rewritten);
                    flags |= bit;
                }
            }
            Expansion::Wotlk => {
                if flags & REALM_FLAG_SPECIFY_BUILD != 0 {
                    let _version = read_array::<3>(r)?;
                    let _build = read_u16_le(r)?;
                }
            }
        }
        realms.push(RealmInfo {
            name,
            address,
            population,
            characters,
            realm_type,
            flags,
            category,
            id,
        });
    }
    let _footer_padding = read_u16_le(r)?; // consume so the stream stays aligned
    Ok(realms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_tags_reverse_to_the_names_servers_match_on() {
        let spelled = |tag: u32| {
            let mut b = tag.to_le_bytes();
            b.reverse();
            String::from_utf8(b.iter().copied().filter(|&c| c != 0).collect()).unwrap()
        };
        assert_eq!(spelled(OS_WINDOWS), "Win");
        assert_eq!(spelled(OS_MACOS), "OSX");
        assert_eq!(spelled(PLATFORM_X86), "x86");
    }

    #[test]
    fn client_os_follows_the_host() {
        if cfg!(target_os = "macos") {
            assert_eq!(client_os(), OS_MACOS);
        } else {
            assert_eq!(client_os(), OS_WINDOWS);
        }
    }

    /// An arbitrary but fixed `A` for the version-proof vectors below.
    fn test_public_key() -> [u8; 32] {
        std::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(9))
    }

    /// Expected digests computed outside this crate (Python `hashlib`) as `SHA1(A ‖ H)`.
    #[test]
    fn version_proof_matches_the_realmd_expression() {
        let expected = if cfg!(target_os = "macos") {
            "dccd0e67946fae221451513b1a59724090ff6096"
        } else {
            "9b95cd41edd719fddf237294b8aca17010e71703"
        };
        let got = version_proof(
            &MANGOS_VERSION_CHALLENGE,
            &test_public_key(),
            Expansion::Classic,
        );
        assert_eq!(
            got.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            expected
        );
    }

    #[test]
    fn an_unknown_version_challenge_is_answered_with_zeros() {
        let mut salt = MANGOS_VERSION_CHALLENGE;
        salt[0] ^= 0xff;
        for expansion in Expansion::ALL {
            assert_eq!(
                version_proof(&salt, &test_public_key(), expansion),
                [0u8; 20]
            );
        }
    }

    /// `opcode · A[32] · M1[20] · crc_hash[20] · num_keys · security_flag`, 75 bytes.
    #[test]
    fn the_proof_packet_carries_the_version_proof() {
        let a = test_public_key();
        let m1: [u8; 20] = std::array::from_fn(|i| (i as u8).wrapping_mul(13).wrapping_add(4));
        let mut packet = Vec::new();
        write_logon_proof(
            &mut packet,
            &a,
            &m1,
            &MANGOS_VERSION_CHALLENGE,
            Expansion::Classic,
        )
        .unwrap();

        assert_eq!(packet.len(), 1 + 32 + 20 + 20 + 1 + 1);
        assert_eq!(packet[0], CMD_AUTH_LOGON_PROOF);
        assert_eq!(&packet[1..33], &a);
        assert_eq!(&packet[33..53], &m1);
        assert_eq!(
            &packet[53..73],
            &version_proof(&MANGOS_VERSION_CHALLENGE, &a, Expansion::Classic)
        );
        assert_eq!(&packet[73..], &[0, 0]);
    }

    #[test]
    fn the_realm_list_keeps_every_field_the_wire_carries() {
        let mut body = Vec::new();
        body.extend_from_slice(&8u32.to_le_bytes()); // realm_type: RPPVP
        body.push(0x42); // flags: 0x02 offline | 0x40 sentinel
        body.extend_from_slice(b"Onyxia\0");
        body.extend_from_slice(b"127.0.0.1:8085\0");
        body.extend_from_slice(&1.75f32.to_le_bytes()); // population
        body.push(3); // characters
        body.push(2); // category
        body.push(9); // realm id

        let mut packet = vec![CMD_REALM_LIST];
        packet.extend_from_slice(&(body.len() as u16 + 3).to_le_bytes()); // size
        packet.extend_from_slice(&0u32.to_le_bytes()); // header padding
        packet.push(1); // number_of_realms
        packet.extend_from_slice(&body);
        packet.extend_from_slice(&0u16.to_le_bytes()); // footer padding

        let realms = read_realm_list(&mut packet.as_slice(), Expansion::Classic).unwrap();
        assert_eq!(realms.len(), 1);
        let r = &realms[0];
        assert_eq!(r.name, "Onyxia");
        assert_eq!(r.address, "127.0.0.1:8085");
        assert_eq!(r.realm_type, 8);
        assert_eq!(r.flags, 0x42);
        assert_eq!(r.population, 1.75);
        assert_eq!(r.characters, 3);
        assert_eq!(r.category, 2);
        assert_eq!(r.id, 9);
    }

    /// The rewritten values are the reference parser's own, bit for bit.
    #[test]
    fn the_three_magic_populations_become_flags_and_are_rewritten() {
        let one = |pop: f32| {
            let mut body = Vec::new();
            body.extend_from_slice(&0u32.to_le_bytes());
            body.push(0x02); // a real wire flag, which must survive the OR
            body.extend_from_slice(b"R\0");
            body.extend_from_slice(b"h:1\0");
            body.extend_from_slice(&pop.to_le_bytes());
            body.extend_from_slice(&[0, 1, 0]);
            let mut packet = vec![CMD_REALM_LIST];
            packet.extend_from_slice(&(body.len() as u16 + 3).to_le_bytes());
            packet.extend_from_slice(&0u32.to_le_bytes());
            packet.push(1);
            packet.extend_from_slice(&body);
            packet.extend_from_slice(&0u16.to_le_bytes());
            read_realm_list(&mut packet.as_slice(), Expansion::Classic)
                .unwrap()
                .remove(0)
        };

        let recommended = one(600.0);
        assert_eq!(recommended.flags, 0x02 | 0x20, "the wire flag survives");
        assert_eq!(recommended.population, 0.0);

        let new = one(200.0);
        assert_eq!(new.flags, 0x02 | 0x40);
        assert_eq!(new.population.to_bits(), 0x3a83_126f, "0.001f exactly");

        let full = one(400.0);
        assert_eq!(full.flags, 0x02 | 0x80);
        assert_eq!(full.population, 8.0);

        // An ordinary population is left alone, flags and all.
        let plain = one(1.5);
        assert_eq!(plain.flags, 0x02);
        assert_eq!(plain.population, 1.5);
    }
}
