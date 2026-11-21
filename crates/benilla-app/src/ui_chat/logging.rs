//! The `LoggingChat`/`LoggingCombat` files, `WoWChatLog.txt` and `WoWCombatLog.txt`: the stock
//! `/chatlog` and `/combatlog` flip the VM's flag and print the notice (`ChatFrame.lua:675-695`);
//! this appends to `Logs\` each chat line as the window shows it, every `COMBAT_` kind included,
//! and each combat-log entry as `0x74e880` packs it (`0x4fb3c0`), as SLog lines: stamped
//! `M/D HH:MM:SS.mmm` in local time, two spaces, the text, CRLF. A file that fails to open clears
//! its flag (`0x4fbcba`), after Lua already printed "enabled".

use std::io::Write as _;

use bevy::prelude::*;

/// One `Logs\` file, open while its flag is on.
pub(crate) struct LogFile {
    name: &'static str,
    file: Option<std::fs::File>,
}

impl LogFile {
    const fn new(name: &'static str) -> Self {
        Self { name, file: None }
    }

    /// Append a line's bytes as they are, when the file is open.
    pub(crate) fn record(&mut self, line: &[u8]) {
        if let Some(file) = self.file.as_mut() {
            let mut out = format!("{}  ", stamp()).into_bytes();
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
            if file.write_all(&out).is_err() {
                self.file = None;
            }
        }
    }

    /// Open or close the file; false when it should be open and cannot be. Without a state folder
    /// (a hermetic capture) it stays closed and the flag stands.
    fn set(&mut self, on: bool) -> bool {
        if !on {
            self.file = None;
            return true;
        }
        if self.file.is_some() {
            return true;
        }
        let Some(dir) = crate::local_state::logs_dir() else {
            return true;
        };
        let path = dir.join(self.name);
        let opened = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        });
        match opened {
            Ok(file) => {
                info!("chat: logging to {}", path.display());
                self.file = Some(file);
                true
            }
            Err(e) => {
                warn!("chat: cannot open {}: {e}", path.display());
                false
            }
        }
    }
}

#[cfg(test)]
impl LogFile {
    /// A log already open on `path`, for tests that read back what was written.
    pub(crate) fn at(path: &std::path::Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        Self {
            name: "test.txt",
            file: Some(file),
        }
    }
}

/// `WoWChatLog.txt`, kept on the chat windows since every line passes [`super::frames::route`].
pub(crate) struct ChatLogFile(pub(crate) LogFile);

impl Default for ChatLogFile {
    fn default() -> Self {
        Self(LogFile::new("WoWChatLog.txt"))
    }
}

/// `WoWCombatLog.txt`, where each combat-log entry is written as it fires (`0x74f910`).
#[derive(Resource)]
pub(crate) struct CombatLogFile(pub(crate) LogFile);

impl CombatLogFile {
    /// Append an entry's line, opening the file first when the flag moved this frame ahead of
    /// [`sync_chat_logging`], as `LoggingCombat` opens it in the call (`0x4fbcb1`).
    pub(crate) fn record(&mut self, line: &[u8]) {
        if self.0.set(true) {
            self.0.record(line);
        }
    }
}

impl Default for CombatLogFile {
    fn default() -> Self {
        Self(LogFile::new("WoWCombatLog.txt"))
    }
}

/// A local wall-clock reading, the fields of the reference's `GetLocalTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalTime {
    pub(crate) year: u32,
    /// 1–12.
    pub(crate) month: u32,
    /// 1–31.
    pub(crate) day: u32,
    pub(crate) hour: u32,
    pub(crate) min: u32,
    pub(crate) sec: u32,
    pub(crate) milli: u32,
}

/// `M/D HH:MM:SS.mmm` of now, local time: the reference's log-line stamp
/// (`"%u/%u %02u:%02u:%02u.%03u  "`, from `GetLocalTime`), which `crate::ui_script::load_log`
/// shares.
pub(crate) fn stamp() -> String {
    let t = local_now();
    format!(
        "{}/{} {:02}:{:02}:{:02}.{:03}",
        t.month, t.day, t.hour, t.min, t.sec, t.milli
    )
}

/// Now, from the reference's `GetLocalTime`.
#[cfg(windows)]
pub(crate) fn local_now() -> LocalTime {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    // SAFETY: `GetLocalTime` fully writes the out-param and reads nothing from it; zeroed is
    // valid for a struct of plain integers.
    let t = unsafe {
        let mut t = std::mem::zeroed::<SYSTEMTIME>();
        GetLocalTime(&mut t);
        t
    };
    LocalTime {
        year: t.wYear.into(),
        month: t.wMonth.into(),
        day: t.wDay.into(),
        hour: t.wHour.into(),
        min: t.wMinute.into(),
        sec: t.wSecond.into(),
        milli: t.wMilliseconds.into(),
    }
}

/// Now, in the process's time zone.
#[cfg(unix)]
pub(crate) fn local_now() -> LocalTime {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    LocalTime {
        milli: now.subsec_millis(),
        ..local_fields(now.as_secs() as i64)
    }
}

/// An epoch second's local fields, through `localtime_r`; UTC when that fails, which POSIX allows
/// only on `EOVERFLOW`, a year past `int`.
#[cfg(unix)]
fn local_fields(secs: i64) -> LocalTime {
    let time = secs as libc::time_t;
    // SAFETY: `localtime_r` fully writes the out-param and reads nothing from it; zeroed is valid
    // for its integers and its nullable zone-name pointer.
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        let c = benilla_ui::civil::from_unix(secs);
        return LocalTime {
            year: c.year as u32,
            month: c.month,
            day: c.day,
            hour: c.hour,
            min: c.min,
            sec: c.sec,
            milli: 0,
        };
    }
    LocalTime {
        year: (tm.tm_year + 1900) as u32,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
        milli: 0,
    }
}

/// The VM's two flags → the two files, on the frame either flag moves or a new session's VM
/// starts with both off, as a logout closes them (`0x5072b0`).
pub(super) fn sync_chat_logging(
    mut script: crate::ui_script::InTurn,
    mut windows: ResMut<super::frames::ChatWindows>,
    mut combat_file: ResMut<CombatLogFile>,
) {
    let added = script.is_added();
    let Some(mut script) = script.turn() else {
        return;
    };
    if !script.take_logging_changes() && !added {
        return;
    }
    let (chat, combat) = script.logging_flags();
    let opened = (windows.logs.0.set(chat), combat_file.0.set(combat));
    if opened != (true, true) {
        script.set_logging_flags(chat && opened.0, combat && opened.1);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CombatLogFile>();
    app.add_systems(
        Update,
        // After the UI tick, so a flag Lua moved is read the same frame.
        sync_chat_logging
            .in_set(crate::ui_script::UiDrain)
            .in_set(crate::char_select::InWorldGated),
    );
}

#[cfg(all(test, unix))]
mod tests {
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    extern "C" {
        fn tzset();
    }

    /// Under a zone 5:45 east of UTC, the epoch reads 05:45, and 20:00 UTC is past local midnight.
    #[test]
    fn the_stamp_fields_follow_the_local_zone() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tz = EnvGuard::set("TZ", "NPT-05:45");
        // SAFETY: `tzset` rereads `TZ`; the lock keeps other env tests out.
        unsafe { tzset() };
        let fields = [super::local_fields(0), super::local_fields(72_000)]
            .map(|t| [t.year, t.month, t.day, t.hour, t.min, t.sec]);
        drop(tz);
        unsafe { tzset() };
        assert_eq!(fields, [[1970, 1, 1, 5, 45, 0], [1970, 1, 2, 1, 45, 0]]);
    }
}
