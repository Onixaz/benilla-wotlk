use std::collections::HashMap;
use std::sync::Mutex;

use md5::Md5;
use sha1::Sha1;

use super::known::{CheckTypes, KnownModule, KnownSeed};
use super::*;

const SESSION_KEY: [u8; 40] = [7; 40];
const SEED: [u8; 16] = [0x11; 16];
const CLIENT_KEY: [u8; 16] = [0x5a; 16];
const SERVER_KEY: [u8; 16] = [0xa5; 16];

/// A host with a module cache, a few bytes of memory and one archive file.
#[derive(Default)]
struct TestHost {
    cache: Mutex<HashMap<[u8; 16], Vec<u8>>>,
    memory: HashMap<u32, Vec<u8>>,
}

impl WardenHost for TestHost {
    fn module_cached(&self, id: &[u8; 16]) -> bool {
        self.cache.lock().unwrap().contains_key(id)
    }
    fn cache_module(&self, id: &[u8; 16], data: &[u8]) {
        self.cache.lock().unwrap().insert(*id, data.to_vec());
    }
    fn read_memory(&self, va: u32, len: usize) -> Option<Vec<u8>> {
        self.memory
            .get(&va)
            .filter(|b| b.len() >= len)
            .map(|b| b[..len].to_vec())
    }
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        (path == "Interface\\Test.lua").then(|| b"file".to_vec())
    }
    fn async_time_ms(&self) -> u32 {
        0x1234_5678
    }
    fn tick_count(&self) -> u32 {
        7
    }
}

/// A module of `bytes`, known with one seed's keys.
fn known_for(bytes: &[u8]) -> &'static [KnownModule] {
    let seeds: &'static [KnownSeed] = Box::leak(Box::new([KnownSeed {
        seed: SEED,
        client_key: CLIENT_KEY,
        server_key: SERVER_KEY,
    }]));
    Box::leak(Box::new([KnownModule {
        id: Md5::digest(bytes).into(),
        check_types: TYPES,
        seeds,
    }]))
}

/// A client Warden and the server's end of its streams.
struct Pair {
    client: Warden,
    server: WardenServerStreams,
    host: TestHost,
}

impl Pair {
    fn new(module: &[u8]) -> Self {
        Self {
            client: Warden::new(&SESSION_KEY, known_for(module)),
            server: WardenServerStreams::new(&SESSION_KEY),
            host: TestHost::default(),
        }
    }

    /// The server sends `plain`; the client's replies come back opened.
    fn send(&mut self, plain: &[u8]) -> Result<(Received, Vec<Vec<u8>>), WardenRefused> {
        let sealed = self.server.seal(plain);
        let received = self.client.receive(&self.host, &sealed)?;
        let replies = self
            .client
            .seal_outgoing()
            .iter()
            .map(|r| self.server.open(r))
            .collect();
        Ok((received, replies))
    }
}

fn module_use(module: &[u8]) -> Vec<u8> {
    let mut p = vec![SMSG_MODULE_USE];
    p.extend_from_slice(&Md5::digest(module));
    p.extend_from_slice(&[0x33; 16]);
    p.extend_from_slice(&(module.len() as u32).to_le_bytes());
    p
}

fn chunk(bytes: &[u8]) -> Vec<u8> {
    let mut p = vec![SMSG_MODULE_CACHE];
    p.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    p.extend_from_slice(bytes);
    p
}

fn hash_request(seed: [u8; 16]) -> Vec<u8> {
    let mut p = vec![SMSG_HASH_REQUEST];
    p.extend_from_slice(&seed);
    p
}

/// A `MODULE_INITIALIZE` block as the server frames it.
fn init(block: &[u8]) -> Vec<u8> {
    let mut p = vec![SMSG_MODULE_INITIALIZE];
    p.extend_from_slice(&(block.len() as u16).to_le_bytes());
    p.extend_from_slice(&checksum(block).to_le_bytes());
    p.extend_from_slice(block);
    p
}

/// AzerothCore's three blocks: the archive reader, the script executor in slot 1, the clock.
fn azerothcore_init() -> Vec<u8> {
    let mut reader = vec![1, 0, 1, 0];
    for offset in [0x24F80u32, 0x218C0, 0x22530, 0x22910] {
        reader.extend_from_slice(&offset.to_le_bytes());
    }
    let mut script = vec![4, 0, 0];
    script.extend_from_slice(&0x41_9210u32.to_le_bytes());
    script.push(1);
    let mut clock = vec![1, 1, 0];
    clock.extend_from_slice(&0x46_AE20u32.to_le_bytes());
    clock.push(1);
    [init(&reader), init(&script), init(&clock)].concat()
}

/// Up to the hash: downloaded in two chunks, used, hashed and switched.
fn hashed(module: &[u8]) -> Pair {
    let mut pair = Pair::new(module);
    let (_, r) = pair.send(&module_use(module)).unwrap();
    assert_eq!(
        r,
        [vec![CMSG_MODULE_MISSING]],
        "an uncached module is asked for"
    );
    let (first, rest) = module.split_at(module.len() / 2);
    let (_, r) = pair.send(&chunk(first)).unwrap();
    assert!(r.is_empty(), "a partial download answers nothing");
    let (_, r) = pair.send(&chunk(rest)).unwrap();
    assert_eq!(
        r,
        [vec![CMSG_MODULE_OK]],
        "a complete, matching download is used"
    );
    let (_, r) = pair.send(&hash_request(SEED)).unwrap();
    let mut want = vec![CMSG_HASH_RESULT];
    want.extend_from_slice(&Sha1::digest(CLIENT_KEY));
    assert_eq!(
        r,
        [want],
        "the hash reply is the SHA-1 of the client's new key"
    );
    pair.server.switch(&CLIENT_KEY, &SERVER_KEY);
    pair
}

#[test]
fn a_downloaded_module_is_cached_and_its_hash_switches_both_streams() {
    let module = b"a module's bytes as the server sends them".to_vec();
    let mut pair = hashed(&module);
    assert_eq!(
        pair.host
            .cache
            .lock()
            .unwrap()
            .get(&<[u8; 16]>::from(Md5::digest(&module))),
        Some(&module),
        "the download lands in the module cache"
    );
    // Under the new keys the client still reads and answers: an empty check list.
    let (_, r) = pair
        .send(&[SMSG_CHEAT_CHECKS_REQUEST, 0, CLIENT_KEY[0]])
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(&r[0][..3], &[CMSG_CHEAT_CHECKS_RESULT, 0, 0]);
}

#[test]
fn a_cached_module_is_used_without_a_download() {
    let module = b"cached".to_vec();
    let mut pair = Pair::new(&module);
    pair.host
        .cache_module(&Md5::digest(&module).into(), &module);
    let (_, r) = pair.send(&module_use(&module)).unwrap();
    assert_eq!(r, [vec![CMSG_MODULE_OK]]);
}

#[test]
fn a_download_whose_md5_is_not_its_id_is_asked_for_again() {
    let module = b"the real module".to_vec();
    let mut pair = Pair::new(&module);
    pair.send(&module_use(&module)).unwrap();
    let wrong = vec![0u8; module.len()];
    let (_, r) = pair.send(&chunk(&wrong)).unwrap();
    assert_eq!(r, [vec![CMSG_MODULE_MISSING]]);
    assert!(pair.host.cache.lock().unwrap().is_empty());
    let (_, r) = pair.send(&chunk(&module)).unwrap();
    assert_eq!(r, [vec![CMSG_MODULE_OK]], "the second download is used");
}

#[test]
fn an_unknown_module_or_seed_is_refused() {
    let mut pair = Pair::new(b"known");
    assert!(pair.send(&module_use(b"another")).is_err());

    let mut pair = hashed(b"known");
    // A fresh pair at the hash with another seed.
    let mut other = Pair::new(b"known");
    other
        .host
        .cache_module(&Md5::digest(b"known").into(), b"known");
    other.send(&module_use(b"known")).unwrap();
    let refused = other.send(&hash_request([0x22; 16])).unwrap_err();
    assert!(refused.reason.contains("seed"), "{refused}");
    // The hashed pair is unaffected.
    assert!(pair
        .send(&[SMSG_CHEAT_CHECKS_REQUEST, 0, CLIENT_KEY[0]])
        .is_ok());
}

/// A request in AzerothCore's layout: the string table, the timing check, then one of each kind.
#[test]
fn every_check_kind_is_answered_in_order() {
    let module = b"module".to_vec();
    let mut pair = hashed(&module);
    pair.host
        .memory
        .insert(0x46_8D24, vec![1, 2, 3, 4, 5, 6, 7]);
    pair.send(&azerothcore_init()).unwrap();

    let xor = CLIENT_KEY[0];
    let lua = "return not not PQR_IsMoving";
    let mut p = vec![SMSG_CHEAT_CHECKS_REQUEST];
    for s in [lua, "Interface\\Test.lua", "dbk64.sys"] {
        p.push(s.len() as u8);
        p.extend_from_slice(s.as_bytes());
    }
    p.push(0);
    p.push(TIMING_TYPE ^ xor);
    // Memory: no module name, the address, the length.
    p.extend([0xf3 ^ xor, 0]);
    p.extend_from_slice(&0x46_8D24u32.to_le_bytes());
    p.push(7);
    // An unreadable address.
    p.extend([0xf3 ^ xor, 0]);
    p.extend_from_slice(&0x10u32.to_le_bytes());
    p.push(2);
    p.extend([0x8b ^ xor, 1]);
    p.extend([0x98 ^ xor, 2]);
    p.push(0xb2 ^ xor);
    p.extend([0u8; 4 + 20 + 4 + 1]);
    p.push(0xd9 ^ xor);
    p.extend([0u8; 4 + 20]);
    p.push(0x71 ^ xor);
    p.extend([0u8; 4 + 20]);
    p.push(3);
    p.push(xor);

    let (received, r) = pair.send(&p).unwrap();
    assert_eq!(
        received.scripts,
        [lua],
        "the executor bound in slot 1 runs the text"
    );
    let reply = &r[0];
    assert_eq!(reply[0], CMSG_CHEAT_CHECKS_RESULT);
    let len = usize::from(u16::from_le_bytes([reply[1], reply[2]]));
    let sum = u32::from_le_bytes(reply[3..7].try_into().unwrap());
    let results = &reply[7..];
    assert_eq!(len, results.len());
    assert_eq!(
        sum,
        checksum(results),
        "the checksum folds the results' SHA-1"
    );

    let mut want = vec![1];
    want.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    want.extend([0, 1, 2, 3, 4, 5, 6, 7]);
    want.push(1);
    want.push(1);
    want.push(0);
    want.extend_from_slice(&Sha1::digest(b"file"));
    want.extend([NOT_FOUND_BYTE, NOT_FOUND_BYTE, NOT_FOUND_BYTE]);
    assert_eq!(results, want.as_slice());
}

const TIMING_TYPE: u8 = 0x57;

/// The test module's check types, AzerothCore's module's.
const TYPES: CheckTypes = CheckTypes {
    memory: 0xf3,
    module: 0xd9,
    page_b: 0xbf,
    page_a: 0xb2,
    file: 0x98,
    script: 0x8b,
    proc: 0x7e,
    driver: 0x71,
    timing: 0x57,
};
const NOT_FOUND_BYTE: u8 = 0xe9;

/// The script check runs nothing unless the executor itself is what slot 1 bound.
#[test]
fn a_script_check_runs_only_through_the_bound_executor() {
    let mut pair = hashed(b"module");
    let xor = CLIENT_KEY[0];
    let p = [
        vec![SMSG_CHEAT_CHECKS_REQUEST, 1, b'x', 0],
        vec![0x8b ^ xor, 1, xor],
    ]
    .concat();
    let (received, r) = pair.send(&p).unwrap();
    assert!(received.scripts.is_empty(), "nothing bound, nothing run");
    assert_eq!(&r[0][7..], &[1]);
}

/// Both ends key the streams from the session key alike.
#[test]
fn the_session_key_starts_the_same_streams_at_both_ends() {
    let mut server = WardenServerStreams::new(&SESSION_KEY);
    let mut link = WardenLink {
        session_key: SESSION_KEY.to_vec(),
        warden: Some(Warden::new(&SESSION_KEY, known_for(b"m"))),
    };
    let sealed = server.seal(&module_use(b"m"));
    link.warden
        .as_mut()
        .unwrap()
        .receive(&TestHost::default(), &sealed)
        .unwrap();
    let replies = link.seal_outgoing();
    assert_eq!(server.open(&replies[0]), [CMSG_MODULE_MISSING]);
}
