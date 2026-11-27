//! 12340's `Logs\` files written through its log layer (`0x775740`, `0x775a90`): each line is
//! stamped `M/D HH:MM:SS.mmm  <text>\r\n` in local time, the first write of a run truncates
//! (`CREATE_ALWAYS`) and subsequent writes append and flush.
//!
//! - `Logs\connection.log` (`0x4d7cd0`, `0x4da3d0`): login (`0x9f427c`) and realm/character
//!   connection (`0xa30870`) transitions.
//! - `Logs\cpu.log` (`0x86b0d0`): CPU vendor, features, topology and brand string at startup.
//! - `Logs\gx.log` (`0x683670`, `0x684ba0`): graphics adapter and capabilities (2 MiB cap).
//! - `Logs\SESound.log` (`0x87c710`): sound engine startup (and deletes stale `Logs\Sound.log`).
//! - `Logs\taint.log` (`0x518920`, `0x52a930`): taint events while `taintLog` >= 1.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use bevy::prelude::*;

/// 12340's 2 MiB byte cap for `Logs\gx.log` (`0x684ba0`).
const GX_LOG_CAP: usize = 0x20_0000;

/// One session-scoped log file: first write truncates (`CREATE_ALWAYS`), later writes append.
struct SessionLog {
    file_name: &'static str,
    opened_path: Option<PathBuf>,
    written_bytes: usize,
    cap: Option<usize>,
}

impl SessionLog {
    const fn new(file_name: &'static str, cap: Option<usize>) -> Self {
        Self {
            file_name,
            opened_path: None,
            written_bytes: 0,
            cap,
        }
    }

    fn write_line(&mut self, dir: &Path, text: &str) {
        if let Some(cap) = self.cap {
            if self.written_bytes >= cap {
                return;
            }
        }
        let path = dir.join(self.file_name);
        let truncate = self.opened_path.as_deref() != Some(path.as_path());
        if truncate {
            self.written_bytes = 0;
        }
        let line = format!("{}  {text}\r\n", crate::ui_chat::logging::stamp());
        let written = std::fs::create_dir_all(dir).and_then(|()| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(truncate)
                .append(!truncate)
                .open(&path)?;
            file.write_all(line.as_bytes())?;
            file.flush()
        });
        match written {
            Ok(()) => {
                self.opened_path = Some(path);
                self.written_bytes = self.written_bytes.saturating_add(text.len());
            }
            Err(e) => warn!("logs: cannot write {}: {e}", path.display()),
        }
    }
}

static CONNECTION_LOG: Mutex<SessionLog> = Mutex::new(SessionLog::new("connection.log", None));
static GX_LOG: Mutex<SessionLog> = Mutex::new(SessionLog::new("gx.log", Some(GX_LOG_CAP)));
static SOUND_LOG: Mutex<SessionLog> = Mutex::new(SessionLog::new("SESound.log", None));
static TAINT_LOG: Mutex<SessionLog> = Mutex::new(SessionLog::new("taint.log", None));

/// Whether `taintLog` >= 1 (`0x52a930`).
static TAINT_LOG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Log directory for session logs. Under `cfg(test)`, unrelated parallel tests (such as
/// `glue_mgr` unit tests) must not scribble into another test's `$BENILLA_HOME`, so writing
/// through `logs_dir()` is gated on `$BENILLA_TEST_CLIENT_LOGS` in unit tests.
fn session_logs_dir() -> Option<PathBuf> {
    if cfg!(test) && std::env::var_os("BENILLA_TEST_CLIENT_LOGS").is_none() {
        return None;
    }
    crate::local_state::logs_dir()
}

/// Write one `GRUNT: state: %s result: %s %s` line to `Logs\connection.log` (`0x4da4b0`,
/// `0x9f427c`).
pub(crate) fn log_grunt(state: &str, result: &str, detail: &str) {
    let Some(dir) = session_logs_dir() else {
        return;
    };
    let mut log = CONNECTION_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    log.write_line(
        &dir,
        &format!("GRUNT: state: {state} result: {result} {detail}"),
    );
}

/// Write one `ClientConnection %s: %s code=%s %s` line to `Logs\connection.log` (`0x6b06e3`,
/// `0xa30870`).
pub(crate) fn log_client_connection(phase: &str, op: &str, code: &str, result: &str) {
    let Some(dir) = session_logs_dir() else {
        return;
    };
    let mut log = CONNECTION_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    log.write_line(
        &dir,
        &format!("ClientConnection {phase}: {op} code={code} {result}"),
    );
}

/// Write `Logs\cpu.log` once at startup (`0x86b0d0`).
pub(crate) fn write_cpu_log() {
    let Some(dir) = session_logs_dir() else {
        return;
    };
    write_cpu_log_under(&dir);
}

fn write_cpu_log_under(dir: &Path) {
    let mut log = SessionLog::new("cpu.log", None);
    for line in cpu_log_lines() {
        log.write_line(dir, &line);
    }
}

fn cpu_log_lines() -> Vec<String> {
    let processors = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        #[cfg(target_arch = "x86")]
        use core::arch::x86::{__cpuid, __cpuid_count, CpuidResult};
        #[cfg(target_arch = "x86_64")]
        use core::arch::x86_64::{__cpuid, __cpuid_count, CpuidResult};
        let zero = CpuidResult {
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
        };

        let leaf0 = __cpuid(0);
        let mut vendor_bytes = [0u8; 12];
        vendor_bytes[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
        vendor_bytes[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
        vendor_bytes[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
        let vendor_str = String::from_utf8_lossy(&vendor_bytes).into_owned();
        let vendor_id = match vendor_str.as_str() {
            "GenuineIntel" => 1,
            "AuthenticAMD" => 2,
            _ => 0,
        };

        let leaf1 = if leaf0.eax >= 1 { __cpuid(1) } else { zero };
        let leaf4 = if leaf0.eax >= 4 {
            __cpuid_count(4, 0)
        } else {
            zero
        };
        let ext0 = __cpuid(0x8000_0000);
        let ext1 = if ext0.eax >= 0x8000_0001 {
            __cpuid(0x8000_0001)
        } else {
            zero
        };
        let ext8 = if ext0.eax >= 0x8000_0008 {
            __cpuid(0x8000_0008)
        } else {
            zero
        };
        let brand = if ext0.eax >= 0x8000_0004 {
            let mut raw = [0u8; 48];
            for (i, leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004]
                .into_iter()
                .enumerate()
            {
                let r = __cpuid(leaf);
                let o = i * 16;
                raw[o..o + 4].copy_from_slice(&r.eax.to_le_bytes());
                raw[o + 4..o + 8].copy_from_slice(&r.ebx.to_le_bytes());
                raw[o + 8..o + 12].copy_from_slice(&r.ecx.to_le_bytes());
                raw[o + 12..o + 16].copy_from_slice(&r.edx.to_le_bytes());
            }
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim_start().to_owned()
        } else {
            String::new()
        };

        vec![
            format!("vendor: {vendor_id}"),
            format!("features: {:08X}", leaf1.edx & 0x7fff_ffff),
            "sockets: 1".to_owned(),
            format!("cores: {processors}"),
            format!("processors: {processors}"),
            format!("vendor id string= {vendor_str}"),
            format!(
                "standard ({}): 1b={:08X} 1d={:08x} 4a={:08X}",
                leaf0.eax, leaf1.ebx, leaf1.edx, leaf4.eax
            ),
            format!(
                "extended ({}): 1c={:08X} 1d={:08x} 8c={:08X}",
                ext0.eax & 0x7fff_ffff,
                ext1.ecx,
                ext1.edx,
                ext8.ecx
            ),
            format!("processor brand string= {brand}"),
        ]
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        vec![
            "vendor: 0".to_owned(),
            "features: 00000000".to_owned(),
            "sockets: 1".to_owned(),
            format!("cores: {processors}"),
            format!("processors: {processors}"),
            format!("vendor id string= {}", std::env::consts::ARCH),
            format!("processor brand string= {}", std::env::consts::ARCH),
        ]
    }
}

/// Write `Logs\gx.log` from the active render adapter and limits (`0x683670`, `0x684c40`).
pub(crate) fn log_gx_init(
    adapter: Option<&bevy::render::renderer::RenderAdapterInfo>,
    device: Option<&bevy::render::renderer::RenderDevice>,
) {
    let Some(dir) = session_logs_dir() else {
        return;
    };
    let mut lines = Vec::new();
    if let Some(info) = adapter {
        lines.push(format!(
            "Adapter: {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        ));
        if !info.driver.is_empty() || !info.driver_info.is_empty() {
            lines.push(format!("Driver: {} {}", info.driver, info.driver_info));
        }
    } else {
        lines.push("Adapter: headless".to_owned());
    }
    lines.push("Caps:".to_owned());
    if let Some(dev) = device {
        let limits = dev.limits();
        lines.push(format!(
            "\tnumTmus: {}",
            limits.max_sampled_textures_per_shader_stage
        ));
        lines.push("\tgenerateMipMaps: 1".to_owned());
        lines.push("\ttexFilterAnisotropic: 1, 16".to_owned());
        lines.push(format!(
            "\tmaxTextureDimension2D: {}",
            limits.max_texture_dimension_2d
        ));
        lines.push(format!("\tnumStreams: {}", limits.max_vertex_buffers));
    }
    let mut log = GX_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for line in lines {
        log.write_line(&dir, &line);
    }
}

/// Write `Logs\SESound.log` on sound init and delete any legacy `Logs\Sound.log` (`0x87c710`,
/// `0x87c7a4`).
pub(crate) fn log_sound_init(detail: &str) {
    let Some(dir) = session_logs_dir() else {
        return;
    };
    log_sound_init_under(&dir, detail);
}

fn log_sound_init_under(dir: &Path, detail: &str) {
    let _ = std::fs::remove_file(dir.join("Sound.log"));
    let mut log = SOUND_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    log.write_line(dir, "=> Version 3.3.5 (12340) Jun 24 2010");
    log.write_line(dir, "");
    log.write_line(dir, "=> Setting up Game Sound:");
    log.write_line(dir, " - SESound Engine Init");
    log.write_line(dir, &format!(" - {detail}"));
}

/// Update `taintLog` (`0x52a930`): values >= 1 enable `Logs\taint.log`.
pub(crate) fn set_taint_log(value: &str) {
    let enabled = value.trim().parse::<i32>().unwrap_or(0) >= 1;
    TAINT_LOG_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether `taintLog` >= 1.
pub(crate) fn taint_log_enabled() -> bool {
    TAINT_LOG_ENABLED.load(Ordering::Relaxed)
}

/// Write one taint event to `Logs\taint.log` in 12340's format (`0x518920`) when `taintLog` >= 1.
pub(crate) fn log_taint_event(ev: &benilla_ui::script::TaintLogEvent) {
    if !taint_log_enabled() {
        return;
    }
    let Some(dir) = session_logs_dir() else {
        return;
    };
    log_taint_event_under(&dir, ev);
}

fn log_taint_event_under(dir: &Path, ev: &benilla_ui::script::TaintLogEvent) {
    let ctx = ev.stack.first().map_or("UNKNOWN", String::as_str);
    let head = match ev.kind {
        benilla_ui::script::TaintLogKind::Global => {
            format!(
                "Global variable {} tainted by {} - {ctx}",
                ev.target, ev.taint
            )
        }
        benilla_ui::script::TaintLogKind::Execution => {
            format!(
                "Execution tainted by {} while reading {} - {ctx}",
                ev.taint, ev.target
            )
        }
        benilla_ui::script::TaintLogKind::Blocked => {
            format!(
                "An action was blocked because of taint from {} - {ctx}",
                ev.taint
            )
        }
        benilla_ui::script::TaintLogKind::BlockedInCombat => {
            format!(
                "An action was blocked in combat because of taint from {} - {ctx}",
                ev.taint
            )
        }
    };
    let mut log = TAINT_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    log.write_line(dir, &head);
    for frame in ev.stack.iter().skip(1) {
        if !frame.is_empty() {
            log.write_line(dir, &format!("    {frame}"));
        }
    }
}

/// Bevy plugin that writes `Logs\gx.log` once the render device is ready and keeps `taintLog`
/// synced with the CVar.
pub(crate) struct ClientLogsPlugin;

impl Plugin for ClientLogsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_taint_log_cvar)
            .add_systems(Startup, startup_gx_log);
    }
}

fn startup_gx_log(
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
    device: Option<Res<bevy::render::renderer::RenderDevice>>,
) {
    log_gx_init(adapter.as_deref(), device.as_deref());
}

fn on_taint_log_cvar(ev: On<crate::cvars::CvarChanged>) {
    if ev.is("taintLog") {
        set_taint_log(&ev.new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    #[test]
    fn cpu_and_sound_logs_write_stamped_crlf_lines_and_sound_deletes_legacy_file() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-clogs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let logs = tmp.join("Logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("Sound.log"), "legacy\r\n").unwrap();

        write_cpu_log_under(&logs);
        let cpu = std::fs::read_to_string(logs.join("cpu.log")).unwrap();
        assert!(cpu.contains("  vendor: "), "{cpu}");
        assert!(cpu.contains("  processors: "), "{cpu}");
        assert!(cpu.contains("\r\n"), "{cpu}");

        log_sound_init_under(&logs, "Output: System Default");
        assert!(
            !logs.join("Sound.log").exists(),
            "SESound init deletes legacy Logs\\Sound.log"
        );
        let sound = std::fs::read_to_string(logs.join("SESound.log")).unwrap();
        assert!(
            sound.contains("=> Version 3.3.5 (12340) Jun 24 2010\r\n"),
            "{sound}"
        );
        assert!(sound.contains("=> Setting up Game Sound:\r\n"), "{sound}");
        assert!(sound.contains(" - Output: System Default\r\n"), "{sound}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn connection_and_taint_logs_follow_12340_format() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-clogs2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _e = EnvGuard::set("BENILLA_TEST_CLIENT_LOGS", "1");

        log_grunt("LOGIN_STATE_CONNECTING", "LOGIN_OK", "");
        log_client_connection("Initiating", "COP_CONNECT", "CSTATUS_CONNECTING", "");
        let conn = std::fs::read_to_string(tmp.join("Logs/connection.log")).unwrap();
        assert!(
            conn.contains("  GRUNT: state: LOGIN_STATE_CONNECTING result: LOGIN_OK \r\n"),
            "{conn}"
        );
        assert!(
            conn.contains(
                "  ClientConnection Initiating: COP_CONNECT code=CSTATUS_CONNECTING \r\n"
            ),
            "{conn}"
        );

        let ev = benilla_ui::script::TaintLogEvent {
            kind: benilla_ui::script::TaintLogKind::BlockedInCombat,
            target: "CastSpellByName()".to_owned(),
            taint: "BadAddon".to_owned(),
            stack: vec!["CastSpellByName()".to_owned(), "BadAddon.lua:12".to_owned()],
        };
        set_taint_log("0");
        log_taint_event(&ev);
        assert!(!tmp.join("Logs/taint.log").exists());

        set_taint_log("1");
        log_taint_event(&ev);
        let taint = std::fs::read_to_string(tmp.join("Logs/taint.log")).unwrap();
        assert!(
            taint.contains(
                "  An action was blocked in combat because of taint from BadAddon - CastSpellByName()\r\n"
            ),
            "{taint}"
        );
        assert!(taint.contains("      BadAddon.lua:12\r\n"), "{taint}");
        set_taint_log("0");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
