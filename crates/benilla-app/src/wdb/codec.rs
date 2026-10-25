//! The `.wdb` file, byte for byte: 12340's `DBCache<T>` on disk (Load `0x67ba40`, Save
//! `0x66a090`). A 24-byte header of six little-endian u32 (FourCC, build, locale, record size,
//! format version, server cache version), then `key | u32 size | body` entries in insertion order,
//! then a zero key and a zero size.

use std::collections::HashMap;

use benilla_protocol::messages::opcode;

/// The build every header carries and every load demands (`0x66a124`, `0x67bb50`).
const BUILD: u32 = 12340;

/// One persisted cache: the nine `DBCache` instances built with persistence on (the enabled byte
/// at `+0x45`, static constructors `0x9cb680`..`0x9cb920`). Name, guild, pet name, petition, arena
/// team and dance caches are built with it off and never touch disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Kind {
    /// The FourCC as the u32 the header holds ('WMOB' reads `BOMW` on disk).
    pub(crate) signature: u32,
    pub(crate) file: &'static str,
    /// `sizeof(T)` in memory, which the header carries and the load compares; not a body length.
    pub(crate) record_size: u32,
    /// `T::s_cacheVersion`.
    pub(crate) version: u32,
    /// Bytes of the key: u32 entries, the u64 item guid of item text, the 16-byte Warden id.
    pub(crate) key_len: usize,
    /// The response whose answers land here; 0 for the Warden modules, which have no query.
    pub(crate) response: u16,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

/// `SMSG_ITEM_NAME_QUERY_RESPONSE`, which benilla does not ask; its file is kept as found.
const SMSG_ITEM_NAME_QUERY_RESPONSE: u16 = 0x2c5;

pub(crate) const CREATURE: Kind = Kind {
    signature: fourcc(b"WMOB"),
    file: "creaturecache.wdb",
    record_size: 0x6c,
    version: 1,
    key_len: 4,
    response: opcode::SMSG_CREATURE_QUERY_RESPONSE,
};
pub(crate) const GAMEOBJECT: Kind = Kind {
    signature: fourcc(b"WGOB"),
    file: "gameobjectcache.wdb",
    record_size: 0xa0,
    version: 1,
    key_len: 4,
    response: opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE,
};
pub(crate) const ITEM_NAME: Kind = Kind {
    signature: fourcc(b"WNDB"),
    file: "itemnamecache.wdb",
    record_size: 0x08,
    version: 1,
    key_len: 4,
    response: SMSG_ITEM_NAME_QUERY_RESPONSE,
};
pub(crate) const ITEM: Kind = Kind {
    signature: fourcc(b"WIDB"),
    file: "itemcache.wdb",
    record_size: 0x204,
    version: 5,
    key_len: 4,
    response: opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE,
};
pub(crate) const NPC_TEXT: Kind = Kind {
    signature: fourcc(b"WNPC"),
    file: "npccache.wdb",
    record_size: 0x140,
    version: 1,
    key_len: 4,
    response: opcode::SMSG_NPC_TEXT_UPDATE,
};
pub(crate) const QUEST: Kind = Kind {
    signature: fourcc(b"WQST"),
    file: "questcache.wdb",
    record_size: 0x28e4,
    version: 3,
    key_len: 4,
    response: opcode::SMSG_QUEST_QUERY_RESPONSE,
};
pub(crate) const PAGE_TEXT: Kind = Kind {
    signature: fourcc(b"WPTX"),
    file: "pagetextcache.wdb",
    record_size: 0x0c,
    version: 1,
    key_len: 4,
    response: opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE,
};
pub(crate) const ITEM_TEXT: Kind = Kind {
    signature: fourcc(b"WITX"),
    file: "itemtextcache.wdb",
    record_size: 0x1f40,
    version: 1,
    key_len: 8,
    response: opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE,
};
pub(crate) const WARDEN: Kind = Kind {
    signature: fourcc(b"WRDN"),
    file: "wowcache.wdb",
    record_size: 0x08,
    version: 1,
    key_len: 16,
    response: 0,
};

/// Every persisted cache, in the reference's construction order.
pub(crate) const KINDS: [Kind; 9] = [
    CREATURE, GAMEOBJECT, ITEM_NAME, ITEM, NPC_TEXT, QUEST, PAGE_TEXT, ITEM_TEXT, WARDEN,
];

/// The header's locale: the mounted four-letter code packed big-endian (`0x406381`), so `enUS`
/// reads `SUne` on disk.
pub(crate) fn locale_code(locale: &str) -> u32 {
    locale.bytes().take(4).fold(0u32, |acc, c| (acc << 8) | u32::from(c))
}

/// Why a file loaded nothing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Short header, or another signature, build, locale, record size or format version: nothing
    /// loads, and the next save writes over it (`0x67bb2c`).
    Header,
    /// A short entry or no terminator: everything read so far is dropped (`Clear` `0x66cc90`).
    Corrupt,
}

/// One cache's records in insertion order, the order Save walks (`LinkToTail` `0x67af9d`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WdbFile {
    /// The server cache version the header carries (`+0x38`), stored unchecked on load.
    pub(crate) server_version: u32,
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    index: HashMap<Vec<u8>, usize>,
}

impl WdbFile {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The records, in file order.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.entries.iter().map(|(k, b)| (k.as_slice(), b.as_slice()))
    }

    #[cfg(test)]
    pub(crate) fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.index.get(key).map(|&i| self.entries[i].1.as_slice())
    }

    /// An answer landed: a new key goes to the tail, as a fresh hash node does; a key already held
    /// is re-answered only after an invalidation deleted it, so it moves to the tail too.
    pub(crate) fn insert(&mut self, key: Vec<u8>, body: Vec<u8>) {
        self.remove(&key);
        self.index.insert(key.clone(), self.entries.len());
        self.entries.push((key, body));
    }

    pub(crate) fn remove(&mut self, key: &[u8]) -> bool {
        let Some(i) = self.index.remove(key) else {
            return false;
        };
        self.entries.remove(i);
        for slot in self.index.values_mut() {
            if *slot > i {
                *slot -= 1;
            }
        }
        true
    }

    /// `VerifyAndUpdateVersion` (`0x66f940`): another server cache version drops every record.
    /// Whether it did.
    pub(crate) fn set_server_version(&mut self, version: u32) -> bool {
        if self.server_version == version {
            return false;
        }
        self.entries.clear();
        self.index.clear();
        self.server_version = version;
        true
    }

    /// Load: the header must match `kind` and `locale` field for field; a duplicate key overwrites
    /// in place, as `Unpack` into the found node does.
    pub(crate) fn parse(kind: &Kind, locale: u32, bytes: &[u8]) -> Result<Self, Refused> {
        let u32_at = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let header: Option<[u32; 6]> = (|| {
            Some([
                u32_at(0)?,
                u32_at(4)?,
                u32_at(8)?,
                u32_at(12)?,
                u32_at(16)?,
                u32_at(20)?,
            ])
        })();
        let Some([sig, build, loc, size, version, server_version]) = header else {
            return Err(Refused::Header);
        };
        if [sig, build, loc, size, version]
            != [kind.signature, BUILD, locale, kind.record_size, kind.version]
        {
            return Err(Refused::Header);
        }
        let mut file = Self {
            server_version,
            ..Self::default()
        };
        let mut at = 24;
        loop {
            let key = bytes.get(at..at + kind.key_len).ok_or(Refused::Corrupt)?;
            let size = u32_at(at + kind.key_len).ok_or(Refused::Corrupt)? as usize;
            at += kind.key_len + 4;
            if key.iter().all(|&b| b == 0) {
                return Ok(file);
            }
            let body = bytes.get(at..at + size).ok_or(Refused::Corrupt)?;
            at += size;
            match file.index.get(key) {
                Some(&i) => file.entries[i].1 = body.to_vec(),
                None => file.insert(key.to_vec(), body.to_vec()),
            }
        }
    }

    /// Save: the whole file, rewritten (`CREATE_ALWAYS`, `0x66a0dd`).
    pub(crate) fn encode(&self, kind: &Kind, locale: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            32 + self
                .entries
                .iter()
                .map(|(k, b)| k.len() + 4 + b.len())
                .sum::<usize>(),
        );
        for field in [
            kind.signature,
            BUILD,
            locale,
            kind.record_size,
            kind.version,
            self.server_version,
        ] {
            out.extend_from_slice(&field.to_le_bytes());
        }
        for (key, body) in &self.entries {
            let body = pack(kind, body);
            out.extend_from_slice(key);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&body);
        }
        out.extend(std::iter::repeat_n(0u8, kind.key_len + 4));
        out
    }
}

/// The quest response's fixed head before its reward lists: 26 u32, the id first.
const QUEST_REWARDS_AT: usize = 26 * 4;

/// `Pack` where it differs from the answer held: the quest's reward items and choices go out
/// compacted, the non-empty pairs in order and `(0, 0)` after (`0x7f6e9b`..`0x7f6f5a`).
fn pack<'a>(kind: &Kind, body: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
    if kind.signature != QUEST.signature || body.len() < QUEST_REWARDS_AT + 10 * 8 {
        return body.into();
    }
    let mut out = body.to_vec();
    for (start, pairs) in [(QUEST_REWARDS_AT, 4), (QUEST_REWARDS_AT + 4 * 8, 6)] {
        let span = &body[start..start + pairs * 8];
        let mut packed: Vec<u8> = span
            .chunks_exact(8)
            .filter(|pair| pair[..4] != [0; 4])
            .flatten()
            .copied()
            .collect();
        packed.resize(pairs * 8, 0);
        out[start..start + pairs * 8].copy_from_slice(&packed);
    }
    out.into()
}

/// What an answer stores, from the response as it came off the wire: the key and the body `Pack`
/// rebuilds, or `None` for a refusal the reference never caches.
pub(crate) fn record_of(response: u16, wire: &[u8]) -> Option<(Kind, Vec<u8>, Vec<u8>)> {
    let kind = *KINDS.iter().find(|k| k.response == response && k.response != 0)?;
    match response {
        // The whole response, id included; an empty title is refused (`0x635230`). The title is the
        // first string, after the fixed head of 65 u32.
        opcode::SMSG_QUEST_QUERY_RESPONSE => {
            let key = wire.get(..4)?.to_vec();
            (*wire.get(65 * 4)? != 0).then(|| (kind, key, wire.to_vec()))
        }
        // `u8 result, u64 item guid, text`: only a found text is cached, under the guid.
        opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE => {
            let (&result, rest) = wire.split_first()?;
            (result == 0 && rest.len() >= 8)
                .then(|| (kind, rest[..8].to_vec(), rest[8..].to_vec()))
        }
        // The entry, then the body; an entry with the high bit is the server's "unknown"
        // (`0x67b884`), which deletes the pending node and is never written.
        _ => {
            let key = wire.get(..4)?;
            let entry = u32::from_le_bytes(key.try_into().ok()?);
            (entry & 0x8000_0000 == 0 && entry != 0)
                .then(|| (kind, key.to_vec(), wire[4..].to_vec()))
        }
    }
}

/// The response a stored record answers, rebuilt for the handlers it replays through.
pub(crate) fn wire_of(kind: &Kind, key: &[u8], body: &[u8]) -> Vec<u8> {
    match kind.response {
        opcode::SMSG_QUEST_QUERY_RESPONSE => body.to_vec(),
        opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE => [&[0u8][..], key, body].concat(),
        _ => [key, body].concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN_US: &str = "enUS";

    fn header(kind: &Kind, server_version: u32) -> Vec<u8> {
        [
            kind.signature,
            BUILD,
            locale_code(EN_US),
            kind.record_size,
            kind.version,
            server_version,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
    }

    #[test]
    fn the_header_spells_the_fourcc_and_locale_backwards_on_disk() {
        let bytes = WdbFile::default().encode(&CREATURE, locale_code(EN_US));
        assert_eq!(&bytes[..4], b"BOMW");
        assert_eq!(&bytes[8..12], b"SUne");
        assert_eq!(bytes.len(), 24 + 8, "header and the zero terminator");
        assert!(bytes[24..].iter().all(|&b| b == 0));
    }

    #[test]
    fn records_round_trip_in_insertion_order() {
        let mut f = WdbFile {
            server_version: 7,
            ..WdbFile::default()
        };
        f.insert(9u32.to_le_bytes().to_vec(), b"nine\0".to_vec());
        f.insert(2u32.to_le_bytes().to_vec(), b"two\0".to_vec());
        f.insert(9u32.to_le_bytes().to_vec(), b"nine again\0".to_vec());
        let bytes = f.encode(&CREATURE, locale_code(EN_US));
        let back = WdbFile::parse(&CREATURE, locale_code(EN_US), &bytes).unwrap();
        assert_eq!(back, f);
        let keys: Vec<_> = back.entries().map(|(k, _)| k[0]).collect();
        assert_eq!(keys, [2, 9], "a re-answered key moves to the tail");
        assert_eq!(back.server_version, 7);
    }

    #[test]
    fn a_header_off_by_any_field_loads_nothing() {
        let good = WdbFile::default().encode(&ITEM, locale_code(EN_US));
        for field in 0..5 {
            let mut bad = good.clone();
            bad[field * 4] ^= 1;
            assert_eq!(
                WdbFile::parse(&ITEM, locale_code(EN_US), &bad),
                Err(Refused::Header),
                "field {field}"
            );
        }
        let mut other_server = good.clone();
        other_server[20] = 3;
        assert!(WdbFile::parse(&ITEM, locale_code(EN_US), &other_server).is_ok());
        assert_eq!(
            WdbFile::parse(&ITEM, locale_code("deDE"), &good),
            Err(Refused::Header)
        );
        assert_eq!(
            WdbFile::parse(&ITEM, locale_code(EN_US), &good[..20]),
            Err(Refused::Header)
        );
    }

    #[test]
    fn a_file_without_its_terminator_drops_everything() {
        let mut bytes = header(&CREATURE, 0);
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(b"x\0");
        assert_eq!(
            WdbFile::parse(&CREATURE, locale_code(EN_US), &bytes),
            Err(Refused::Corrupt)
        );
        bytes.extend_from_slice(&[0; 8]);
        assert_eq!(
            WdbFile::parse(&CREATURE, locale_code(EN_US), &bytes)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn wide_keys_frame_as_the_reference_writes_them() {
        let mut f = WdbFile::default();
        f.insert(0x1122_3344_5566_7788u64.to_le_bytes().to_vec(), b"hi\0".to_vec());
        let bytes = f.encode(&ITEM_TEXT, locale_code(EN_US));
        assert_eq!(bytes.len(), 24 + (8 + 4 + 3) + (8 + 4));
        assert_eq!(
            WdbFile::parse(&ITEM_TEXT, locale_code(EN_US), &bytes).unwrap(),
            f
        );
        let mut w = WdbFile::default();
        w.insert(vec![7; 16], vec![1, 0, 0, 0, 9]);
        let bytes = w.encode(&WARDEN, locale_code(EN_US));
        assert_eq!(
            WdbFile::parse(&WARDEN, locale_code(EN_US), &bytes).unwrap(),
            w
        );
    }

    #[test]
    fn another_server_cache_version_drops_every_record() {
        let mut f = WdbFile::default();
        f.insert(vec![1, 0, 0, 0], vec![0]);
        assert!(!f.set_server_version(0));
        assert_eq!(f.len(), 1);
        assert!(f.set_server_version(4));
        assert!(f.is_empty());
        assert_eq!(f.server_version, 4);
    }

    #[test]
    fn refusals_are_never_cached_and_answers_rebuild_their_wire() {
        let mut creature = 69u32.to_le_bytes().to_vec();
        creature.extend_from_slice(b"Wolf\0");
        let (kind, key, body) =
            record_of(opcode::SMSG_CREATURE_QUERY_RESPONSE, &creature).unwrap();
        assert_eq!(kind, CREATURE);
        assert_eq!(body, b"Wolf\0");
        assert_eq!(wire_of(&kind, &key, &body), creature);
        assert_eq!(
            record_of(
                opcode::SMSG_CREATURE_QUERY_RESPONSE,
                &(69u32 | 0x8000_0000).to_le_bytes()
            ),
            None
        );

        let mut text = vec![0u8];
        text.extend_from_slice(&42u64.to_le_bytes());
        text.extend_from_slice(b"Dear friend\0");
        let (kind, key, body) = record_of(opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &text).unwrap();
        assert_eq!(key, 42u64.to_le_bytes());
        assert_eq!(wire_of(&kind, &key, &body), text);
        text[0] = 1;
        assert_eq!(record_of(opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE, &text), None);

        let mut quest = vec![0u8; 65 * 4];
        quest[..4].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(
            record_of(opcode::SMSG_QUEST_QUERY_RESPONSE, &[&quest[..], b"\0"].concat()),
            None,
            "an empty title is refused"
        );
        let titled = [&quest[..], b"Kobold Camp Cleanup\0"].concat();
        let (kind, key, body) = record_of(opcode::SMSG_QUEST_QUERY_RESPONSE, &titled).unwrap();
        assert_eq!(key, 7u32.to_le_bytes());
        assert_eq!(wire_of(&kind, &key, &body), titled, "the id stays in the body");
    }

    #[test]
    fn quest_rewards_are_compacted_on_save_only() {
        let mut body = vec![0u8; 65 * 4];
        let pair = |id: u32, n: u32| [id.to_le_bytes(), n.to_le_bytes()].concat();
        // Reward items: a hole with a stray count, then an item.
        body[QUEST_REWARDS_AT..QUEST_REWARDS_AT + 16]
            .copy_from_slice(&[pair(0, 1), pair(25, 2)].concat());
        let mut f = WdbFile::default();
        f.insert(vec![1, 0, 0, 0], body.clone());
        assert_eq!(f.get(&[1, 0, 0, 0]), Some(&body[..]), "held as answered");
        let bytes = f.encode(&QUEST, locale_code(EN_US));
        let back = WdbFile::parse(&QUEST, locale_code(EN_US), &bytes).unwrap();
        let saved = back.get(&[1, 0, 0, 0]).unwrap();
        assert_eq!(
            &saved[QUEST_REWARDS_AT..QUEST_REWARDS_AT + 16],
            &[pair(25, 2), pair(0, 0)].concat()[..]
        );
    }

    /// The files a real 12340 client wrote, where an install holds them: every one parses, and
    /// writes back byte for byte.
    #[test]
    fn the_reference_clients_own_files_round_trip() {
        let data = benilla_formats::wow_data_or_skip!();
        let Some(locale) = benilla_formats::data_locale(&data).ok() else {
            return;
        };
        let dir = data.parent().unwrap().join("Cache/WDB").join(&locale);
        for kind in KINDS {
            let Ok(bytes) = std::fs::read(dir.join(kind.file)) else {
                continue;
            };
            let file = WdbFile::parse(&kind, locale_code(&locale), &bytes)
                .unwrap_or_else(|e| panic!("{}: {e:?}", kind.file));
            assert_eq!(
                file.encode(&kind, locale_code(&locale)),
                bytes,
                "{} writes back as read",
                kind.file
            );
        }
    }
}
