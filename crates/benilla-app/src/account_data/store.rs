//! The account data's rules, without Bevy: 12340's eight slots (`0xad94e0`), each a file under
//! `WTF\Account\<ACCOUNT>\` or `WTF\Account\<ACCOUNT>\<Realm>\<Character>\`, its MD5 and time in
//! that folder's `cache.md5`, and the server's copy kept in step through the times packet, a
//! download, and an upload 30 seconds after a save or at the logout flush.

use std::path::{Path, PathBuf};

use benilla_protocol::messages::{AccountDataTimes, AccountDataUpdate};
use md5::{Digest, Md5};

/// One slot of the table: its scope, its files and the CVar that gates its sync.
#[derive(Debug)]
pub(crate) struct SlotDef {
    /// `WTF\Account\<ACCOUNT>\` (`+0x01`); else the character's folder.
    pub(crate) global: bool,
    /// The file the server keeps a copy of (`+0x08`).
    pub(crate) cache: Option<&'static str>,
    /// The one-time backup of it (`+0x0c`).
    pub(crate) old: Option<&'static str>,
    /// A file read and written but never synced (`+0x04`); a slot with one is local only (`+0x14`).
    pub(crate) local: Option<&'static str>,
    /// `synchronizeConfig` and its kin (`+0x10`); `None` syncs whenever `synchronizeSettings` does.
    pub(crate) gate: Option<&'static str>,
}

impl SlotDef {
    pub(crate) fn local_only(&self) -> bool {
        self.local.is_some()
    }
}

pub(crate) const CONFIG_ACCOUNT: usize = 0;
pub(crate) const CONFIG_CHARACTER: usize = 1;
pub(crate) const BINDINGS_ACCOUNT: usize = 2;
pub(crate) const BINDINGS_CHARACTER: usize = 3;
pub(crate) const MACROS_ACCOUNT: usize = 4;
pub(crate) const MACROS_CHARACTER: usize = 5;
pub(crate) const LAYOUT: usize = 6;
pub(crate) const CHAT: usize = 7;

const fn synced(
    global: bool,
    cache: &'static str,
    old: &'static str,
    gate: &'static str,
) -> SlotDef {
    SlotDef {
        global,
        cache: Some(cache),
        old: Some(old),
        local: None,
        gate: Some(gate),
    }
}

/// The table, dumped from the binary (`0xad94e0`, stride `0x40`): the global mask is `0x15`, the
/// character mask `0xea`.
pub(crate) const SLOTS: [SlotDef; 8] = [
    synced(
        true,
        "config-cache.wtf",
        "config-cache.old",
        "synchronizeConfig",
    ),
    synced(
        false,
        "config-cache.wtf",
        "config-cache.old",
        "synchronizeConfig",
    ),
    synced(
        true,
        "bindings-cache.wtf",
        "bindings-cache.old",
        "synchronizeBindings",
    ),
    synced(
        false,
        "bindings-cache.wtf",
        "bindings-cache.old",
        "synchronizeBindings",
    ),
    synced(
        true,
        "macros-cache.txt",
        "macros-cache.old",
        "synchronizeMacros",
    ),
    synced(
        false,
        "macros-cache.txt",
        "macros-cache.old",
        "synchronizeMacros",
    ),
    SlotDef {
        global: false,
        cache: None,
        old: None,
        local: Some("layout-local.txt"),
        gate: None,
    },
    SlotDef {
        global: false,
        cache: Some("chat-cache.txt"),
        old: Some("chat-cache.old"),
        local: None,
        gate: None,
    },
];

/// The checksums' file in each scope folder (`0xa31394`).
const CHECKSUMS: &str = "cache.md5";

/// A save's upload waits this long for the next edit (`0x6b9440`).
pub(crate) const SAVE_DELAY_SECS: f32 = 30.0;

/// A slot's live state (`+0x15`..`+0x3c`).
#[derive(Clone, Debug, Default, PartialEq)]
struct SlotState {
    active: bool,
    /// Its loads wait and its saves are refused until the server's times or bytes arrive.
    downloading: bool,
    /// The cache file's MD5 as of the last sync, all zero for a missing or empty file.
    md5: [u8; 16],
    /// Saved since the last sync.
    changed: bool,
    time: i32,
    server_time: i32,
    /// Seconds until the save's upload check.
    timer: Option<f32>,
}

/// What goes to the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wire {
    /// `CMSG_REQUEST_ACCOUNT_DATA`.
    Request { slot: u32 },
    /// `CMSG_UPDATE_ACCOUNT_DATA`, the file's exact bytes.
    Upload {
        slot: u32,
        time: u32,
        bytes: Vec<u8>,
    },
}

/// Which slots may sync now: `synchronizeSettings` and the slot's gate CVar (`0x6b9440`, `0x6b94c0`).
pub(crate) trait SyncGate {
    fn allows(&self, slot: &SlotDef) -> bool;
}

impl<F: Fn(&SlotDef) -> bool> SyncGate for F {
    fn allows(&self, slot: &SlotDef) -> bool {
        self(slot)
    }
}

/// The eight slots and the two folders they live in.
#[derive(Debug, Default, bevy::prelude::Resource)]
pub(crate) struct AccountData {
    slots: [SlotState; 8],
    /// The server's clock (`0xc7bff0`): its value from the last times packet, one step on per
    /// upload.
    clock: u32,
    account_dir: Option<PathBuf>,
    character_dir: Option<PathBuf>,
    outbox: Vec<Wire>,
}

/// The MD5 of a file's bytes; a missing or empty file is all zero (`0x6b8ca0`).
fn md5_of(path: Option<&Path>) -> [u8; 16] {
    match path.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) if !bytes.is_empty() => Md5::digest(&bytes).into(),
        _ => [0; 16],
    }
}

impl AccountData {
    fn dir(&self, slot: usize) -> Option<&Path> {
        if SLOTS[slot].global {
            self.account_dir.as_deref()
        } else {
            self.character_dir.as_deref()
        }
    }

    /// The file the slot's loads read and its saves write: the local one for the layout, else the
    /// cache file.
    pub(crate) fn file(&self, slot: usize) -> Option<PathBuf> {
        let def = &SLOTS[slot];
        Some(self.dir(slot)?.join(def.local.or(def.cache)?))
    }

    fn cache_file(&self, slot: usize) -> Option<PathBuf> {
        Some(self.dir(slot)?.join(SLOTS[slot].cache?))
    }

    fn scope(global: bool) -> impl Iterator<Item = usize> {
        (0..SLOTS.len()).filter(move |&s| SLOTS[s].global == global)
    }

    /// The scope's opening (`0x6b9970`): its slots go live and wait for the server, their checksums
    /// are read back, a file edited since is marked changed, and a cache file with no backup yet
    /// gets one (`0x6b8ab0`). `dir` is `None` when nothing persists.
    pub(crate) fn initialize(&mut self, global: bool, dir: Option<PathBuf>) {
        if global {
            self.account_dir = dir;
        } else {
            self.character_dir = dir;
        }
        let stored = self.load_checksums(global);
        for (i, slot) in Self::scope(global).enumerate() {
            let (md5, time) = stored.get(i).copied().unwrap_or_default();
            let changed = md5_of(self.cache_file(slot).as_deref()) != md5;
            self.slots[slot] = SlotState {
                active: true,
                downloading: true,
                md5,
                changed,
                time,
                server_time: 0,
                timer: None,
            };
            if let (Some(cache), Some(old)) = (self.cache_file(slot), SLOTS[slot].old) {
                let old = cache.with_file_name(old);
                if !old.exists() && cache.exists() {
                    let _ = std::fs::copy(&cache, &old);
                }
            }
        }
    }

    /// The character scope's close after a logout: the character's slots go dark.
    pub(crate) fn release_character(&mut self) {
        for slot in Self::scope(false) {
            self.slots[slot] = SlotState::default();
        }
        self.character_dir = None;
    }

    /// `cache.md5`: per scope slot in slot order, the MD5 then an `i32` time; any other size is
    /// ignored (`0x6b8d40`).
    fn load_checksums(&self, global: bool) -> Vec<([u8; 16], i32)> {
        let count = Self::scope(global).count();
        let dir = if global {
            &self.account_dir
        } else {
            &self.character_dir
        };
        let Some(bytes) = dir
            .as_ref()
            .and_then(|d| std::fs::read(d.join(CHECKSUMS)).ok())
        else {
            return Vec::new();
        };
        if bytes.len() != 20 * count {
            return Vec::new();
        }
        bytes
            .as_chunks::<20>()
            .0
            .iter()
            .map(|r| {
                let mut md5 = [0; 16];
                md5.copy_from_slice(&r[..16]);
                (md5, i32::from_le_bytes([r[16], r[17], r[18], r[19]]))
            })
            .collect()
    }

    /// The checksums written back (`0x6b8ee0`).
    fn save_checksums(&self, global: bool) {
        let dir = if global {
            &self.account_dir
        } else {
            &self.character_dir
        };
        let Some(dir) = dir else {
            return;
        };
        let mut out = Vec::with_capacity(100);
        for slot in Self::scope(global) {
            out.extend_from_slice(&self.slots[slot].md5);
            out.extend_from_slice(&self.slots[slot].time.to_le_bytes());
        }
        if let Err(e) = crate::local_state::write_atomic_bytes(&dir.join(CHECKSUMS), &out) {
            bevy::log::warn!("account data: cannot write {}: {e}", dir.display());
        }
    }

    /// The checksum refresh (`0x6b8f50`): after an upload or a download the file is in sync.
    fn mark_synced(&mut self, slot: usize, time: i32) {
        let md5 = md5_of(self.cache_file(slot).as_deref());
        let s = &mut self.slots[slot];
        s.md5 = md5;
        s.changed = false;
        s.time = time;
        s.server_time = time;
        self.save_checksums(SLOTS[slot].global);
    }

    /// Whether the slot's file may be read: live, and not waiting on the server (the load at
    /// `0x6b9050` defers until then).
    pub(crate) fn ready(&self, slot: usize) -> bool {
        self.slots[slot].active && !self.slots[slot].downloading
    }

    /// No live slot of the scope still waits for the server; a scope never opened waits for nothing.
    pub(crate) fn scope_ready(&self, global: bool) -> bool {
        Self::scope(global).all(|s| !self.slots[s].active || !self.slots[s].downloading)
    }

    /// The slot's file, once ready; empty when there is none.
    pub(crate) fn read(&self, slot: usize) -> Option<Vec<u8>> {
        if !self.ready(slot) {
            return None;
        }
        Some(
            self.file(slot)
                .and_then(|p| std::fs::read(p).ok())
                .unwrap_or_default(),
        )
    }

    /// The slot save (`0x6b9be0`) then its upload scheduled (`0x6b9440`): refused while the server
    /// has not answered; else the file is rewritten whole and, where sync is on, the upload check
    /// runs in 30 seconds. Whether it wrote.
    pub(crate) fn save(&mut self, slot: usize, bytes: &[u8], gate: &impl SyncGate) -> bool {
        if !self.slots[slot].active || self.slots[slot].downloading {
            return false;
        }
        let Some(path) = self.file(slot) else {
            return false;
        };
        if let Err(e) = crate::local_state::write_atomic_bytes(&path, bytes) {
            bevy::log::warn!("account data: cannot write {}: {e}", path.display());
            return false;
        }
        let s = &mut self.slots[slot];
        s.changed = true;
        if gate.allows(&SLOTS[slot]) && !SLOTS[slot].local_only() {
            s.timer = Some(SAVE_DELAY_SECS);
        }
        true
    }

    /// `SMSG_ACCOUNT_DATA_TIMES` (`0x6b94c0`): the server's times; each live slot stops waiting and
    /// goes up if it changed or is newer here, down if the server's is newer.
    pub(crate) fn on_times(&mut self, times: &AccountDataTimes, gate: &impl SyncGate) {
        self.clock = times.server_time;
        for (slot, def) in SLOTS.iter().enumerate() {
            let sent = times.times[slot];
            if !def.local_only() {
                match sent {
                    Some(t) => self.slots[slot].server_time = t as i32,
                    // A slot the server did not name keeps waiting.
                    None => continue,
                }
            }
            let s = &mut self.slots[slot];
            if !s.active {
                continue;
            }
            if times.flag == 0 && def.global && s.server_time < s.time {
                s.server_time = s.time;
            }
            s.downloading = false;
            if def.local_only() || !gate.allows(def) {
                continue;
            }
            if s.changed || s.time > s.server_time {
                self.upload(slot);
            } else if s.time < s.server_time {
                self.slots[slot].downloading = true;
                self.outbox.push(Wire::Request { slot: slot as u32 });
            }
        }
    }

    /// `SMSG_UPDATE_ACCOUNT_DATA` (`0x6b9730`): the server's bytes for a slot this side asked for,
    /// a global one or the active character's, written over the file unless it was edited here
    /// since and is not older.
    pub(crate) fn on_update(&mut self, update: &AccountDataUpdate, active_guid: u64) {
        let slot = update.slot as usize;
        let Some(def) = SLOTS.get(slot) else {
            return;
        };
        if !(update.guid == active_guid || def.global) || !self.slots[slot].downloading {
            return;
        }
        // An unedited file at least as new as the server's stays (`0x6b97c4`-`0x6b97d6`); a body
        // that does not inflate leaves the file empty.
        let time = update.time as i32;
        let s = &self.slots[slot];
        if s.changed || s.time < time {
            if let Some(path) = self.cache_file(slot) {
                let bytes = update.inflate().unwrap_or_default();
                if let Err(e) = crate::local_state::write_atomic_bytes(&path, &bytes) {
                    bevy::log::warn!("account data: cannot write {}: {e}", path.display());
                }
                self.mark_synced(slot, time);
            }
        }
        self.slots[slot].downloading = false;
    }

    /// The upload (`0x6b9100`): the file's bytes under the next clock step.
    fn upload(&mut self, slot: usize) {
        let bytes = self
            .cache_file(slot)
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default();
        let time = self.clock;
        self.clock = self.clock.wrapping_add(1);
        self.outbox.push(Wire::Upload {
            slot: slot as u32,
            time,
            bytes,
        });
        self.mark_synced(slot, time as i32);
    }

    /// The save timer (`0x6b9340`): an unchanged file only refreshes the checksums; a changed one
    /// goes up.
    fn fire(&mut self, slot: usize) {
        self.slots[slot].timer = None;
        let md5 = md5_of(self.cache_file(slot).as_deref());
        if md5 == self.slots[slot].md5 {
            let s = &mut self.slots[slot];
            s.changed = false;
            s.time = s.server_time;
            self.save_checksums(SLOTS[slot].global);
        } else {
            self.upload(slot);
        }
    }

    /// The save timers, `dt` seconds on.
    pub(crate) fn tick(&mut self, dt: f32) {
        for slot in 0..SLOTS.len() {
            if let Some(t) = self.slots[slot].timer.as_mut() {
                *t -= dt;
                if *t <= 0.0 {
                    self.fire(slot);
                }
            }
        }
    }

    /// The flush (`0x6b9b90`): every pending save's check now, at the UI shutdown.
    pub(crate) fn flush(&mut self) {
        for slot in 0..SLOTS.len() {
            if self.slots[slot].timer.is_some() {
                self.fire(slot);
            }
        }
    }

    pub(crate) fn has_outbox(&self) -> bool {
        !self.outbox.is_empty()
    }

    pub(crate) fn take_outbox(&mut self) -> Vec<Wire> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always(_: &SlotDef) -> bool {
        true
    }

    fn times(server: u32, mask: u32, at: &[(usize, u32)]) -> AccountDataTimes {
        let mut t = [None; 8];
        for &(slot, time) in at {
            t[slot] = Some(time);
        }
        AccountDataTimes {
            server_time: server,
            flag: 1,
            mask,
            times: t,
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("benilla-acct-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_table_is_the_references() {
        let global: u32 = (0..8).filter(|&s| SLOTS[s].global).map(|s| 1 << s).sum();
        assert_eq!(global, 0x15);
        assert_eq!(0xff & !global, 0xea);
        assert!(SLOTS[LAYOUT].local_only());
        assert_eq!(SLOTS[CHAT].gate, None);
    }

    #[test]
    fn a_slot_waits_for_the_server_and_a_fresh_one_neither_sends_nor_fetches() {
        let dir = tmp("fresh");
        let mut ad = AccountData::default();
        ad.initialize(true, Some(dir.clone()));
        assert!(!ad.ready(CONFIG_ACCOUNT));
        assert!(
            !ad.save(CONFIG_ACCOUNT, b"SET a \"1\"\n", &always),
            "no save while waiting"
        );
        ad.on_times(&times(1000, 0x15, &[(0, 0), (2, 0), (4, 0)]), &always);
        assert!(ad.scope_ready(true));
        assert!(!ad.has_outbox(), "equal times move nothing");
        assert_eq!(ad.read(CONFIG_ACCOUNT), Some(Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_goes_up_thirty_seconds_later_or_at_the_flush() {
        let dir = tmp("save");
        let mut ad = AccountData::default();
        ad.initialize(true, Some(dir.clone()));
        ad.on_times(&times(5000, 0x15, &[(0, 0), (2, 0), (4, 0)]), &always);
        assert!(ad.save(CONFIG_ACCOUNT, b"SET a \"1\"\n", &always));
        ad.tick(29.0);
        assert!(!ad.has_outbox());
        ad.tick(1.5);
        let sent = ad.take_outbox();
        assert_eq!(
            sent,
            [Wire::Upload {
                slot: 0,
                time: 5000,
                bytes: b"SET a \"1\"\n".to_vec()
            }]
        );
        // The checksum file now names the upload: MD5 then the time, per global slot.
        let sums = std::fs::read(dir.join(CHECKSUMS)).unwrap();
        assert_eq!(sums.len(), 60);
        assert_eq!(&sums[..16], &Md5::digest(b"SET a \"1\"\n")[..]);
        assert_eq!(&sums[16..20], &5000i32.to_le_bytes());

        // An unchanged rewrite only refreshes the sums; a changed one goes up at the flush.
        assert!(ad.save(CONFIG_ACCOUNT, b"SET a \"1\"\n", &always));
        ad.flush();
        assert!(!ad.has_outbox());
        assert!(ad.save(CONFIG_ACCOUNT, b"SET a \"2\"\n", &always));
        ad.flush();
        assert!(matches!(
            &ad.take_outbox()[..],
            [Wire::Upload { time: 5001, .. }]
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_newer_side_wins_at_the_times_packet() {
        let dir = tmp("newer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("macros-cache.txt"), b"MACRO 1 \"x\" y\r\nEND\r\n").unwrap();
        let mut ad = AccountData::default();
        // No checksums yet: the file on disk counts as changed and goes up.
        ad.initialize(true, Some(dir.clone()));
        ad.on_times(&times(9, 0x15, &[(0, 3), (2, 0), (4, 0)]), &always);
        let sent = ad.take_outbox();
        assert!(sent.contains(&Wire::Request { slot: 0 }), "{sent:?}");
        assert!(
            sent.iter()
                .any(|w| matches!(w, Wire::Upload { slot: 4, .. })),
            "{sent:?}"
        );
        assert!(
            !ad.ready(CONFIG_ACCOUNT),
            "a download in flight keeps the slot waiting"
        );

        // The server's bytes land over the file.
        let body = benilla_protocol::messages::update_account_data(0, 3, b"SET b \"7\"\n");
        let mut wire = 0u64.to_le_bytes().to_vec();
        wire.extend_from_slice(&body);
        let update =
            benilla_protocol::messages::read_update_account_data(&mut wire.as_slice()).unwrap();
        ad.on_update(&update, 0);
        assert!(ad.ready(CONFIG_ACCOUNT));
        assert_eq!(ad.read(CONFIG_ACCOUNT).unwrap(), b"SET b \"7\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slot_the_server_never_names_keeps_waiting_and_the_layout_does_not() {
        let dir = tmp("unnamed");
        let mut ad = AccountData::default();
        ad.initialize(false, Some(dir.clone()));
        ad.on_times(&times(1, 0x02, &[(1, 0)]), &always);
        assert!(ad.ready(CONFIG_CHARACTER));
        assert!(!ad.ready(CHAT));
        assert!(ad.ready(LAYOUT), "local only: any times packet frees it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_off_writes_locally_and_sends_nothing() {
        let dir = tmp("off");
        let mut ad = AccountData::default();
        ad.initialize(true, Some(dir.clone()));
        let off = |_: &SlotDef| false;
        ad.on_times(&times(1, 0x15, &[(0, 0), (2, 5), (4, 0)]), &off);
        assert!(ad.ready(BINDINGS_ACCOUNT));
        assert!(ad.save(BINDINGS_ACCOUNT, b"bind A NONE\r\n", &off));
        ad.flush();
        assert!(!ad.has_outbox());
        assert_eq!(
            std::fs::read(dir.join("bindings-cache.wtf")).unwrap(),
            b"bind A NONE\r\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cache_file_gets_its_backup_once() {
        let dir = tmp("old");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config-cache.wtf"), b"SET a \"1\"\n").unwrap();
        let mut ad = AccountData::default();
        ad.initialize(true, Some(dir.clone()));
        assert_eq!(
            std::fs::read(dir.join("config-cache.old")).unwrap(),
            b"SET a \"1\"\n"
        );
        std::fs::write(dir.join("config-cache.wtf"), b"SET a \"2\"\n").unwrap();
        ad.initialize(true, Some(dir.clone()));
        assert_eq!(
            std::fs::read(dir.join("config-cache.old")).unwrap(),
            b"SET a \"1\"\n",
            "never refreshed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
