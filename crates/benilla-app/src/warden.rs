//! Warden's client side in the app: the host its checks read ([`AppHost`]) and the handler that
//! sends the replies; a check's script runs in [`crate::ui_warden`]. The exchange is
//! `benilla-warden`'s.
//!
//! The host answers from the player's own install, read-only: the executable's memory from
//! `Wow.exe` mapped as the loader maps it, files through the archive chain, and the module cache
//! from `wowcache.wdb` ([`crate::wdb`]), which 12340 keeps downloaded modules in.

use std::sync::{Mutex, OnceLock};

use benilla_formats::ExeImage;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

pub(crate) struct WardenPlugin;

impl Plugin for WardenPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::WardenReplies, on_replies);
    }
}

/// The replies the read queued leave through the writer, after the scripts ahead of them ran.
fn on_replies(In(ev): In<SessionEvent>, commands: Option<Res<NetCommands>>) {
    let SessionEvent::WardenReplies = ev else {
        return;
    };
    if let Some(commands) = commands {
        let _ = commands.0.send(ClientCommand::WardenReplies);
    }
}

/// The host every connection's checks read, set once before the first connect.
pub(crate) fn install_host() {
    benilla_protocol::warden::set_host(Box::new(AppHost::default()));
}

/// The module cache: `wowcache.wdb`'s records, each a module's id and its bytes as the server sent
/// them, in the order the file keeps.
#[derive(Default)]
struct ModuleCache(Vec<([u8; 16], Vec<u8>)>);

impl ModuleCache {
    /// The file's records: each body is 12340's packed form (`0x7da220`), the size then the bytes.
    fn seed<'a>(&mut self, records: impl Iterator<Item = (&'a [u8], &'a [u8])>) {
        self.0.clear();
        for (key, body) in records {
            let (Ok(id), Some(size)) = (
                <[u8; 16]>::try_from(key),
                body.get(..4)
                    .map(|s| u32::from_le_bytes(s.try_into().expect("four bytes")) as usize),
            ) else {
                continue;
            };
            if let Some(data) = body.get(4..4 + size) {
                self.0.push((id, data.to_vec()));
            }
        }
    }

    /// The records in [`Self::seed`]'s form.
    fn records(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.0
            .iter()
            .map(|(id, data)| {
                let mut body = (data.len() as u32).to_le_bytes().to_vec();
                body.extend_from_slice(data);
                (id.to_vec(), body)
            })
            .collect()
    }

    fn contains(&self, id: &[u8; 16]) -> bool {
        self.0.iter().any(|(m, _)| m == id)
    }

    /// A download replaces the same id's record and goes to the tail, as a fresh node does.
    fn insert(&mut self, id: &[u8; 16], data: &[u8]) {
        self.0.retain(|(m, _)| m != id);
        self.0.push((*id, data.to_vec()));
    }
}

/// The cache, shared by the IO thread, which caches a download, and the cache file's load and save.
static MODULES: Mutex<ModuleCache> = Mutex::new(ModuleCache(Vec::new()));

fn modules() -> std::sync::MutexGuard<'static, ModuleCache> {
    MODULES.lock().unwrap_or_else(|e| e.into_inner())
}

/// The modules the cache file loaded.
pub(crate) fn seed_modules<'a>(records: impl Iterator<Item = (&'a [u8], &'a [u8])>) {
    modules().seed(records);
}

/// The records the cache file saves.
pub(crate) fn module_records() -> Vec<(Vec<u8>, Vec<u8>)> {
    modules().records()
}

/// A moved server cache version drops the modules with every other cached answer.
pub(crate) fn drop_modules() {
    modules().0.clear();
}

/// The two login-status globals the glue's status display keeps (`0x4d80c0`): the last result
/// shown (`[0xac3da8]`) and the last state (`[0xac3dac]`). In the world they hold what the realm
/// login left: `LOGIN_OK` (0) and `LOGIN_STATE_AUTHENTICATED` (4).
const LOGIN_STATUS: [(u32, u32); 2] = [(0xac_3da8, 0), (0xac_3dac, 4)];

/// What the checks read from benilla's side of the connection.
#[derive(Default)]
pub(crate) struct AppHost {
    exe: OnceLock<Option<ExeImage>>,
    chain: Mutex<Option<benilla_formats::Chain>>,
}

impl AppHost {
    fn exe(&self) -> Option<&ExeImage> {
        self.exe
            .get_or_init(|| {
                let path = benilla_formats::wow_exe()?;
                ExeImage::open(&path)
                    .inspect_err(|e| warn!("warden: {e:#}"))
                    .ok()
            })
            .as_ref()
    }
}

impl benilla_protocol::warden::WardenHost for AppHost {
    fn module_cached(&self, id: &[u8; 16]) -> bool {
        modules().contains(id)
    }

    fn cache_module(&self, id: &[u8; 16], data: &[u8]) {
        modules().insert(id, data);
    }

    fn read_memory(&self, va: u32, len: usize) -> Option<Vec<u8>> {
        let mut bytes = self.exe()?.read(va, len)?;
        overlay_runtime(va, &mut bytes);
        Some(bytes)
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let mut chain = self.chain.lock().unwrap_or_else(|e| e.into_inner());
        if chain.is_none() {
            *chain = benilla_formats::open_chain(&benilla_formats::wow_data()?).ok();
        }
        chain.as_mut()?.read_file(path).ok()
    }

    fn async_time_ms(&self) -> u32 {
        benilla_protocol::client_uptime_ms()
    }

    fn tick_count(&self) -> u32 {
        benilla_protocol::client_uptime_ms()
    }

    fn note(&self, line: &str) {
        debug!("warden: {line}");
    }
}

/// The globals 12340 has written by the time it is in the world, over the file's bytes at `va`.
fn overlay_runtime(va: u32, bytes: &mut [u8]) {
    let end = u64::from(va) + bytes.len() as u64;
    for (at, value) in LOGIN_STATUS {
        for (i, b) in value.to_le_bytes().into_iter().enumerate() {
            let a = u64::from(at) + i as u64;
            if a >= u64::from(va) && a < end {
                bytes[(a - u64::from(va)) as usize] = b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::warden::WardenHost;

    #[test]
    fn the_login_status_reads_as_the_world_holds_it_over_the_files_bytes() {
        let mut bytes = [0xffu8; 12];
        overlay_runtime(0xac_3da4, &mut bytes);
        assert_eq!(bytes, [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 4, 0, 0, 0]);
        let mut tail = [0xffu8; 2];
        overlay_runtime(0xac_3dad, &mut tail);
        assert_eq!(tail, [0, 0]);
    }

    /// The module cache round-trips through the file's packed records.
    #[test]
    fn a_cached_module_saves_and_loads_as_its_packed_record() {
        let mut cache = ModuleCache::default();
        cache.insert(&[9; 16], b"module");
        cache.insert(&[8; 16], b"other");
        cache.insert(&[9; 16], b"module");
        let records = cache.records();
        assert_eq!(records[1].0, [9; 16], "a re-download moves to the tail");
        assert_eq!(records[1].1[..4], 6u32.to_le_bytes());
        assert_eq!(&records[1].1[4..], b"module");
        let mut loaded = ModuleCache::default();
        loaded.seed(records.iter().map(|(k, b)| (k.as_slice(), b.as_slice())));
        assert!(loaded.contains(&[9; 16]) && loaded.contains(&[8; 16]));
        assert_eq!(loaded.records(), records);
    }

    /// The install's executable reads at its code addresses; the login status reads as in the world.
    #[test]
    fn the_installs_executable_answers_memory_checks() {
        benilla_formats::wow_exe_or_skip!();
        let host = AppHost::default();
        let image_base = host.exe().unwrap().image_base();
        assert_eq!(
            host.read_memory(image_base, 2).as_deref(),
            Some(b"MZ".as_slice())
        );
        assert_eq!(
            host.read_memory(0xac_3dac, 4).as_deref(),
            Some(4u32.to_le_bytes().as_slice())
        );
    }
}
