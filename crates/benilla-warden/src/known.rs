//! The modules benilla answers for. A module turns the hash request's seed into the two new stream
//! keys with code of its own, which benilla does not run; the keys a module derives from a seed
//! are listed here instead, as the servers that send it list them.

/// A module, by the MD5 of its bytes as the server sends them, the id its `MODULE_USE` names.
pub(crate) struct KnownModule {
    pub(crate) id: [u8; 16],
    /// The byte each check type is listed under, which every module assigns anew.
    pub(crate) check_types: CheckTypes,
    /// The seeds whose keys are known.
    pub(crate) seeds: &'static [KnownSeed],
}

/// A module's check-type bytes, before the request XORs them with the check-type key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CheckTypes {
    pub(crate) memory: u8,
    pub(crate) module: u8,
    pub(crate) page_b: u8,
    pub(crate) page_a: u8,
    pub(crate) file: u8,
    pub(crate) script: u8,
    pub(crate) proc: u8,
    pub(crate) driver: u8,
    pub(crate) timing: u8,
}

/// What a module derives from one hash-request seed: the client's new sending key, whose SHA-1 is
/// the reply, and the server's.
pub(crate) struct KnownSeed {
    pub(crate) seed: [u8; 16],
    pub(crate) client_key: [u8; 16],
    pub(crate) server_key: [u8; 16],
}

/// The Windows module AzerothCore and TrinityCore send to a 3.3.5a client, with the one seed they
/// request its hash for (their `WardenModuleWin.h`).
const WIN_79C0768D: KnownModule = KnownModule {
    id: [
        0x79, 0xC0, 0x76, 0x8D, 0x65, 0x79, 0x77, 0xD6, 0x97, 0xE1, 0x0B, 0xAD, 0x95, 0x6C, 0xCE,
        0xD1,
    ],
    check_types: CheckTypes {
        memory: 0xf3,
        module: 0xd9,
        page_b: 0xbf,
        page_a: 0xb2,
        file: 0x98,
        script: 0x8b,
        proc: 0x7e,
        driver: 0x71,
        timing: 0x57,
    },
    seeds: &[KnownSeed {
        seed: [
            0x4D, 0x80, 0x8D, 0x2C, 0x77, 0xD9, 0x05, 0xC4, 0x1A, 0x63, 0x80, 0xEC, 0x08, 0x58,
            0x6A, 0xFE,
        ],
        client_key: [
            0x7F, 0x96, 0xEE, 0xFD, 0xA5, 0xB6, 0x3D, 0x20, 0xA4, 0xDF, 0x8E, 0x00, 0xCB, 0xF4,
            0x83, 0x04,
        ],
        server_key: [
            0xC2, 0xB7, 0xAD, 0xED, 0xFC, 0xCC, 0xA9, 0xC2, 0xBF, 0xB3, 0xF8, 0x56, 0x02, 0xBA,
            0x80, 0x9B,
        ],
    }],
};

/// Every module benilla answers for.
pub(crate) const KNOWN: &[KnownModule] = &[WIN_79C0768D];
