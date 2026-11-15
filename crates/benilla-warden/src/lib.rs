//! Warden, the anticheat channel inside `SMSG_WARDEN_DATA` and `CMSG_WARDEN_DATA`, answered
//! natively: no module is run, and no module code or client code is shipped.
//!
//! 12340 hands each `SMSG_WARDEN_DATA` body whole to the module it runs (`0x7da850`): first one
//! built into the client (`0x7da8c0`), then the one the server names, loaded from the module cache
//! (`0x7da930`) or downloaded and cached (`0x7da9f0`). A module's replies leave as
//! `CMSG_WARDEN_DATA` (`0x7daab0`). Both module kinds speak the same commands, one opcode byte
//! each, several to a body:
//!
//! - the bodies run through two RC4 streams, keyed from the session key's two halves by the
//!   SHA-1 generator the server uses, the first 16 bytes for what the client sends;
//! - `MODULE_USE` names a module (MD5 id, key, size): the client answers `1` when it holds it,
//!   else `0` and the server sends it in `MODULE_CACHE` chunks; a complete download whose MD5 is
//!   the id is cached and answered `1`, a wrong one is asked for again (`0`);
//! - `HASH_REQUEST` carries a seed the module turns into two new stream keys; the reply is the
//!   SHA-1 of the client's, and both streams switch. The derivation is the module's own code, so
//!   benilla knows the keys only for the module and seed pairs listed in `known`;
//! - `MODULE_INITIALIZE` binds client functions by address for the checks to call;
//! - `CHEAT_CHECKS_REQUEST` lists checks; the reply answers each (`checks`).
//!
//! Deviation: a module or seed benilla has no keys for ends the session ([`WardenRefused`]),
//! where the reference would run whatever module the server sends.
//!
//! This crate is the exchange alone: `benilla-protocol` feeds it each body where the session
//! reads and sends what it queues, and the app supplies the [`WardenHost`].

mod checks;
mod known;

use std::sync::{Arc, Mutex, OnceLock};

use benilla_srp::wotlk_header::Arc4;
use md5::{Digest as _, Md5};
use sha1::Sha1;

use known::{KnownModule, KNOWN};

/// Client-to-server Warden opcodes.
const CMSG_MODULE_MISSING: u8 = 0;
const CMSG_MODULE_OK: u8 = 1;
const CMSG_CHEAT_CHECKS_RESULT: u8 = 2;
const CMSG_HASH_RESULT: u8 = 4;
const CMSG_MODULE_FAILED: u8 = 5;

/// Server-to-client Warden opcodes; the module's command table ends at 6.
const SMSG_MODULE_USE: u8 = 0;
const SMSG_MODULE_CACHE: u8 = 1;
const SMSG_CHEAT_CHECKS_REQUEST: u8 = 2;
const SMSG_MODULE_INITIALIZE: u8 = 3;
const SMSG_MEM_CHECKS_REQUEST: u8 = 4;
const SMSG_HASH_REQUEST: u8 = 5;

/// The image base 12340 loads at; module-relative bindings name the main module from here.
const CLIENT_IMAGE_BASE: u32 = 0x40_0000;

/// What the checks read from the client's side: its executable's memory, its archives, its clocks,
/// and the module cache (12340 keeps modules in `wowcache.wdb`).
pub trait WardenHost: Send + Sync {
    /// Whether the module cache holds module `id`.
    fn module_cached(&self, id: &[u8; 16]) -> bool;
    /// A downloaded module, its bytes as the server sent them, into the cache.
    fn cache_module(&self, id: &[u8; 16], data: &[u8]);
    /// `len` bytes of the client's memory at `va`, as 12340 holds them once in the world; `None`
    /// where it cannot be read.
    fn read_memory(&self, va: u32, len: usize) -> Option<Vec<u8>>;
    /// A file's bytes read through the archives, as the client's archive reader returns them.
    fn read_file(&self, path: &str) -> Option<Vec<u8>>;
    /// The client's async clock, in ms (`0x86ae20`).
    fn async_time_ms(&self) -> u32;
    /// The system tick count, in ms.
    fn tick_count(&self) -> u32;
    /// One line on what a command asked and how it was answered, for the host's log.
    fn note(&self, _line: &str) {}
}

/// The host with nothing behind it: no cache, no memory, no archives.
struct NoHost;

impl WardenHost for NoHost {
    fn module_cached(&self, _: &[u8; 16]) -> bool {
        false
    }
    fn cache_module(&self, _: &[u8; 16], _: &[u8]) {}
    fn read_memory(&self, _: u32, _: usize) -> Option<Vec<u8>> {
        None
    }
    fn read_file(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn async_time_ms(&self) -> u32 {
        since_first_use_ms()
    }
    fn tick_count(&self) -> u32 {
        since_first_use_ms()
    }
}

/// Milliseconds since this clock was first read, for a process with no host.
fn since_first_use_ms() -> u32 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u32
}

static HOST: OnceLock<Box<dyn WardenHost>> = OnceLock::new();

/// The host every connection's checks read from; the first one set is kept for the process. With
/// none set, every check that reads the client answers as unreadable.
pub fn set_host(host: Box<dyn WardenHost>) {
    let _ = HOST.set(host);
}

fn host() -> &'static dyn WardenHost {
    HOST.get().map_or(&NoHost, |h| h.as_ref())
}

/// A Warden exchange benilla cannot answer: the session ends with `reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WardenRefused {
    pub reason: String,
}

impl std::fmt::Display for WardenRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for WardenRefused {}

/// What one `SMSG_WARDEN_DATA` asked of the client beyond its replies: the Lua chunks a check runs
/// through the bound script executor, in order, before the replies leave.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Received {
    pub scripts: Vec<String>,
}

/// The SHA-1 generator both ends key the Warden streams with (the server's `SessionKeyGenerator`):
/// SHA-1 of each half of the seed material, then blocks of `SHA-1(h1 | prev | h2)`.
struct KeyGenerator {
    h1: [u8; 20],
    h2: [u8; 20],
    block: [u8; 20],
    taken: usize,
}

impl KeyGenerator {
    fn new(material: &[u8]) -> Self {
        let half = material.len() / 2;
        let mut generator = Self {
            h1: Sha1::digest(&material[..half]).into(),
            h2: Sha1::digest(&material[half..]).into(),
            block: [0; 20],
            taken: 0,
        };
        generator.refill();
        generator
    }

    fn refill(&mut self) {
        self.block = Sha1::new()
            .chain_update(self.h1)
            .chain_update(self.block)
            .chain_update(self.h2)
            .finalize()
            .into();
        self.taken = 0;
    }

    fn key(&mut self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for b in &mut out {
            if self.taken == self.block.len() {
                self.refill();
            }
            *b = self.block[self.taken];
            self.taken += 1;
        }
        out
    }
}

/// The two stream keys a session key starts with: the client's sending key, then its receiving.
fn session_keys(session_key: &[u8]) -> ([u8; 16], [u8; 16]) {
    let mut generator = KeyGenerator::new(session_key);
    let send = generator.key();
    (send, generator.key())
}

/// The fold a reply's checksum and a `MODULE_INITIALIZE` block's are: the five little-endian words
/// of the bytes' SHA-1, XORed.
fn checksum(bytes: &[u8]) -> u32 {
    let digest: [u8; 20] = Sha1::digest(bytes).into();
    digest
        .as_chunks::<4>()
        .0
        .iter()
        .fold(0, |acc, w| acc ^ u32::from_le_bytes(*w))
}

/// A reply waiting to be sealed: its plain bytes, and the client's new sending key when the reply
/// is the hash result, which still goes out under the old one.
struct Outgoing {
    plain: Vec<u8>,
    rekey_after: Option<[u8; 16]>,
}

/// The module the server named and the download under way.
struct Download {
    id: [u8; 16],
    size: usize,
    data: Vec<u8>,
}

/// The client functions a `MODULE_INITIALIZE` bound, by the virtual address it resolved them to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Bindings {
    /// The archive reader: kind 1 binds open, size, read and close; kind 2 another set of four.
    pub(crate) file_kind: u8,
    pub(crate) file: [u32; 4],
    /// The script check's function: slot 0 takes the name alone, slot 1 `(text, 0, 0)`.
    pub(crate) script: [u32; 2],
    /// The timing check's clock.
    pub(crate) clock: u32,
}

/// One connection's Warden: both streams, the module in use and what its commands bound.
pub(crate) struct Warden {
    /// Server to client.
    incoming: Arc4,
    /// Client to server.
    outgoing: Arc4,
    known: &'static [KnownModule],
    download: Option<Download>,
    /// The module the client runs, once used.
    module: Option<&'static KnownModule>,
    /// The check-type key: the first byte of the client's sending key since the hash request.
    xor: u8,
    bindings: Bindings,
    outbox: Vec<Outgoing>,
}

/// Why a command stopped the body's walk.
enum Stop {
    /// Short or malformed: the rest of the body is dropped, as the module's walk breaks.
    Short,
    Refused(WardenRefused),
}

impl From<WardenRefused> for Stop {
    fn from(r: WardenRefused) -> Self {
        Stop::Refused(r)
    }
}

/// A forward reader over a command body; every read past the end is [`Stop::Short`].
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.take(2)
            .map(|b| u16::from_le_bytes(b.try_into().expect("two bytes")))
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N).map(|b| b.try_into().expect("N bytes"))
    }

    /// A `u8`-length string, as the module's tables carry names.
    pub(crate) fn string(&mut self) -> Option<String> {
        let len = usize::from(self.u8()?);
        self.take(len)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

impl Warden {
    /// The streams a session key starts, answering for `known`.
    fn new(session_key: &[u8], known: &'static [KnownModule]) -> Self {
        let (send, receive) = session_keys(session_key);
        Self {
            incoming: Arc4::keyed(&receive),
            outgoing: Arc4::keyed(&send),
            known,
            download: None,
            module: None,
            xor: 0,
            bindings: Bindings::default(),
            outbox: Vec::new(),
        }
    }

    /// One `SMSG_WARDEN_DATA` body: decrypted, then each command in turn while the opcode is one
    /// the module knows. Replies wait in the outbox for [`Self::seal_outgoing`].
    fn receive(&mut self, host: &dyn WardenHost, body: &[u8]) -> Result<Received, WardenRefused> {
        let mut plain = body.to_vec();
        self.incoming.apply(&mut plain);
        let mut received = Received::default();
        let mut at = 0;
        while at < plain.len() {
            let mut cursor = Cursor::new(&plain[at..]);
            let Some(opcode) = cursor.u8() else { break };
            let rekeyed = match self.command(host, opcode, &mut cursor, &mut received) {
                Ok(rekeyed) => rekeyed,
                Err(Stop::Short) => {
                    host.note(&format!(
                        "command {opcode} at byte {at} of {} read nothing more",
                        plain.len()
                    ));
                    break;
                }
                Err(Stop::Refused(r)) => return Err(r),
            };
            at += cursor.at;
            // The rest of the body is read again under the new key, as the module re-decrypts it.
            if rekeyed && at < plain.len() {
                let mut rest = body[at..].to_vec();
                self.incoming.apply(&mut rest);
                plain.truncate(at);
                plain.extend(rest);
            }
        }
        Ok(received)
    }

    /// One command; `true` when it switched the streams.
    fn command(
        &mut self,
        host: &dyn WardenHost,
        opcode: u8,
        cursor: &mut Cursor,
        received: &mut Received,
    ) -> Result<bool, Stop> {
        match opcode {
            SMSG_MODULE_USE => self.module_use(host, cursor).map(|()| false),
            SMSG_MODULE_CACHE => self.module_cache(host, cursor).map(|()| false),
            SMSG_CHEAT_CHECKS_REQUEST => {
                // The check types are the module's own; with none in use nothing is listed.
                let types = self.module.map(|m| &m.check_types).ok_or(Stop::Short)?;
                let reply = checks::answer(host, types, &self.bindings, self.xor, cursor, received)
                    .ok_or(Stop::Short)?;
                self.reply(reply, None);
                Ok(false)
            }
            SMSG_MODULE_INITIALIZE => {
                self.initialize(cursor)?;
                host.note(&format!("initialize: {:?}", self.bindings));
                Ok(false)
            }
            SMSG_MEM_CHECKS_REQUEST => {
                // A count and that many bytes the module keeps; it answers nothing.
                let n = cursor.u8().ok_or(Stop::Short)?;
                cursor.take(usize::from(n)).ok_or(Stop::Short)?;
                host.note(&format!("memory-checks request of {n} bytes kept"));
                Ok(false)
            }
            SMSG_HASH_REQUEST => self.hash_request(host, cursor).map(|()| true),
            _ => Err(Stop::Short),
        }
    }

    fn reply(&mut self, plain: Vec<u8>, rekey_after: Option<[u8; 16]>) {
        self.outbox.push(Outgoing { plain, rekey_after });
    }

    fn known_module(&self, id: &[u8; 16]) -> Option<&'static KnownModule> {
        self.known.iter().find(|m| &m.id == id)
    }

    /// `MODULE_USE`: id, key, size. A cached module the client can run is answered `1`; otherwise
    /// `0` asks for it, and its chunks are expected.
    fn module_use(&mut self, host: &dyn WardenHost, cursor: &mut Cursor) -> Result<(), Stop> {
        let id = cursor.array::<16>().ok_or(Stop::Short)?;
        let _key = cursor.array::<16>().ok_or(Stop::Short)?;
        let size = cursor.u32().ok_or(Stop::Short)? as usize;
        let Some(module) = self.known_module(&id) else {
            return Err(WardenRefused {
                reason: format!(
                    "this server's Warden module ({}) is not one benilla can answer",
                    hex(&id)
                ),
            }
            .into());
        };
        self.download = None;
        if host.module_cached(&id) {
            self.module = Some(module);
            self.reply(vec![CMSG_MODULE_OK], None);
            host.note(&format!("module use {}: cached, answered ok", hex(&id)));
        } else {
            self.download = Some(Download {
                id,
                size,
                data: Vec::with_capacity(size),
            });
            self.reply(vec![CMSG_MODULE_MISSING], None);
            host.note(&format!(
                "module use {}: not cached, asked for its {size} bytes",
                hex(&id)
            ));
        }
        Ok(())
    }

    /// `MODULE_CACHE`: a `u16`-sized chunk. The last one checks the download's MD5 against its id:
    /// a match is cached and used, a mismatch asked for again.
    fn module_cache(&mut self, host: &dyn WardenHost, cursor: &mut Cursor) -> Result<(), Stop> {
        let len = usize::from(cursor.u16().ok_or(Stop::Short)?);
        let chunk = cursor.take(len).ok_or(Stop::Short)?;
        let Some(download) = self.download.as_mut() else {
            return Ok(());
        };
        if download.data.len() + len > download.size {
            return Err(Stop::Short);
        }
        download.data.extend_from_slice(chunk);
        if download.data.len() < download.size {
            return Ok(());
        }
        let download = self.download.take().expect("checked above");
        let digest: [u8; 16] = Md5::digest(&download.data).into();
        host.note(&format!(
            "module download complete, its MD5 {} its id",
            if digest == download.id {
                "matches"
            } else {
                "differs from"
            }
        ));
        if digest != download.id {
            self.download = Some(Download {
                data: Vec::with_capacity(download.size),
                ..download
            });
            self.reply(vec![CMSG_MODULE_MISSING], None);
            return Ok(());
        }
        host.cache_module(&download.id, &download.data);
        match self.known_module(&download.id) {
            Some(module) => {
                self.module = Some(module);
                self.reply(vec![CMSG_MODULE_OK], None);
            }
            None => self.reply(vec![CMSG_MODULE_FAILED], None),
        }
        Ok(())
    }

    /// `HASH_REQUEST`: the seed. The reply is the SHA-1 of the client's new sending key; the
    /// receiving stream switches now, the sending one after the reply.
    fn hash_request(&mut self, host: &dyn WardenHost, cursor: &mut Cursor) -> Result<(), Stop> {
        let seed = cursor.array::<16>().ok_or(Stop::Short)?;
        let keys = self
            .module
            .and_then(|m| m.seeds.iter().find(|s| s.seed == seed));
        let Some(keys) = keys else {
            return Err(WardenRefused {
                reason: format!(
                    "this server's Warden hash seed ({}) is not one benilla can answer",
                    hex(&seed)
                ),
            }
            .into());
        };
        let mut reply = vec![CMSG_HASH_RESULT];
        reply.extend_from_slice(&Sha1::digest(keys.client_key));
        self.reply(reply, Some(keys.client_key));
        self.incoming = Arc4::keyed(&keys.server_key);
        self.xor = keys.client_key[0];
        host.note(&format!(
            "hash request {}: answered, streams switched",
            hex(&seed)
        ));
        Ok(())
    }

    /// `MODULE_INITIALIZE`: `u16` size, `u32` checksum of the block, then the block: a kind pair
    /// and the functions it binds, each as a library name and an offset into it.
    fn initialize(&mut self, cursor: &mut Cursor) -> Result<(), Stop> {
        let size = usize::from(cursor.u16().ok_or(Stop::Short)?);
        let sum = cursor.u32().ok_or(Stop::Short)?;
        let block = cursor.take(size).ok_or(Stop::Short)?;
        if checksum(block) != sum {
            return Err(Stop::Short);
        }
        let mut b = Cursor::new(block);
        let pair = (b.u8().ok_or(Stop::Short)?, b.u8().ok_or(Stop::Short)?);
        match pair {
            // The archive reader.
            (1, 0) => {
                let kind = b.u8().ok_or(Stop::Short)?;
                if kind == 0 {
                    self.bindings.file_kind = 0;
                    return Ok(());
                }
                let library = b.string().ok_or(Stop::Short)?;
                let mut file = [0u32; 4];
                for f in &mut file {
                    *f = b.u32().ok_or(Stop::Short)?;
                }
                match resolve_all(&library, file) {
                    Some(file) if kind == 1 || kind == 2 => {
                        self.bindings.file_kind = kind;
                        self.bindings.file = file;
                    }
                    _ => self.bindings.file_kind = 0,
                }
            }
            // The timing check's clock, bound only when its flag is 1.
            (1, 1) => {
                let (va, flag) = binding(&mut b).ok_or(Stop::Short)?;
                self.bindings.clock = if flag == 1 { va } else { 0 };
            }
            // The script check's function, into the slot its flag names.
            (4, 0) => {
                let (va, flag) = binding(&mut b).ok_or(Stop::Short)?;
                if va != 0 && flag <= 1 {
                    self.bindings.script = [0, 0];
                    self.bindings.script[usize::from(flag)] = va;
                } else {
                    self.bindings.script = [0, 0];
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The outbox sealed in order under the sending stream, each hash result switching it after.
    fn seal_outgoing(&mut self) -> Vec<Vec<u8>> {
        self.outbox
            .drain(..)
            .map(
                |Outgoing {
                     mut plain,
                     rekey_after,
                 }| {
                    self.outgoing.apply(&mut plain);
                    if let Some(key) = rekey_after {
                        self.outgoing = Arc4::keyed(&key);
                    }
                    plain
                },
            )
            .collect()
    }
}

/// A library name and an offset resolved to a virtual address: the main module's from 12340's
/// image base; another library's cannot be resolved here.
fn resolve(library: &str, offset: u32) -> Option<u32> {
    (library.is_empty() || library.eq_ignore_ascii_case("wow.exe"))
        .then(|| CLIENT_IMAGE_BASE.wrapping_add(offset))
}

fn resolve_all(library: &str, offsets: [u32; 4]) -> Option<[u32; 4]> {
    let mut out = [0u32; 4];
    for (o, offset) in out.iter_mut().zip(offsets) {
        *o = resolve(library, offset)?;
    }
    Some(out)
}

/// One bound function: library name, `u32` offset, `u8` flag. An unresolvable one binds 0.
fn binding(b: &mut Cursor) -> Option<(u32, u8)> {
    let library = b.string()?;
    let offset = b.u32()?;
    let flag = b.u8()?;
    let va = if offset == 0 {
        0
    } else {
        resolve(&library, offset).unwrap_or(0)
    };
    Some((va, flag))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// A connection's Warden, created at its first `SMSG_WARDEN_DATA`, shared by the session and both
/// halves of a split one, as the reference keeps one module per connection.
pub struct WardenLink {
    session_key: Vec<u8>,
    warden: Option<Warden>,
}

/// [`WardenLink`] behind the lock every holder takes.
pub type SharedWarden = Arc<Mutex<WardenLink>>;

impl WardenLink {
    pub fn shared(session_key: &[u8]) -> SharedWarden {
        Arc::new(Mutex::new(Self {
            session_key: session_key.to_vec(),
            warden: None,
        }))
    }

    /// One `SMSG_WARDEN_DATA` body, read with the process's host.
    pub fn receive(&mut self, body: &[u8]) -> Result<Received, WardenRefused> {
        let key = &self.session_key;
        self.warden
            .get_or_insert_with(|| Warden::new(key, KNOWN))
            .receive(host(), body)
    }

    /// The replies waiting, sealed, each a `CMSG_WARDEN_DATA` body in order.
    pub fn seal_outgoing(&mut self) -> Vec<Vec<u8>> {
        self.warden
            .as_mut()
            .map(Warden::seal_outgoing)
            .unwrap_or_default()
    }
}

/// The server's end of the Warden streams, for a test server: it seals what the server sends and
/// opens what the client sends, keyed as [`WardenLink`] is from the same session key.
pub struct WardenServerStreams {
    to_client: Arc4,
    from_client: Arc4,
}

impl WardenServerStreams {
    pub fn new(session_key: &[u8]) -> Self {
        let (client_send, client_receive) = session_keys(session_key);
        Self {
            to_client: Arc4::keyed(&client_receive),
            from_client: Arc4::keyed(&client_send),
        }
    }

    /// A body the server sends.
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let mut out = plain.to_vec();
        self.to_client.apply(&mut out);
        out
    }

    /// A body the client sent.
    pub fn open(&mut self, sealed: &[u8]) -> Vec<u8> {
        let mut out = sealed.to_vec();
        self.from_client.apply(&mut out);
        out
    }

    /// The switch a verified hash result makes: the client's new sending key and the server's.
    pub fn switch(&mut self, client_key: &[u8; 16], server_key: &[u8; 16]) {
        self.from_client = Arc4::keyed(client_key);
        self.to_client = Arc4::keyed(server_key);
    }
}

#[cfg(test)]
mod tests;
