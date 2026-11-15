//! `CHEAT_CHECKS_REQUEST` and its reply. The request is a table of `u8`-length strings ended by an
//! empty one, then checks, each a type byte XORed with the check-type key and its arguments, ended
//! by a byte whose type is none of them. The reply is opcode `2`, the results' `u16` length and
//! checksum, then one result per check in order.
//!
//! Each check is answered from what benilla holds, as 12340 with nothing injected answers it:
//! memory from the client's executable, files through the archives, scripts by the bound executor.
//! The scans for foreign code (page, driver, module, API hooks) find nothing.

use sha1::{Digest as _, Sha1};

use super::known::CheckTypes;
use super::{checksum, Bindings, Cursor, Received, WardenHost, CMSG_CHEAT_CHECKS_RESULT};

/// A scan's result when it found nothing; a find answers `0x4a`.
const NOT_FOUND: u8 = 0xe9;

/// The script executor 12340 exports (`0x819210`): it runs a chunk and returns nothing, which the
/// check reads as no value found.
const EXECUTE_SCRIPT: u32 = 0x81_9210;

/// The archive reader's open, size, read and close (`0x424f80`, `0x4218c0`, `0x422530`,
/// `0x422910`).
const FILE_READER: [u32; 4] = [0x42_4f80, 0x42_18c0, 0x42_2530, 0x42_2910];

/// The async clock (`0x86ae20`).
const ASYNC_CLOCK: u32 = 0x86_ae20;

/// One check as the request lists it.
enum Check {
    Timing,
    Driver { name: u8 },
    Proc { module: u8 },
    Script { text: u8 },
    File { path: u8 },
    Page,
    Module,
    Memory { module: u8, offset: u32, len: u8 },
}

/// The reply to the request at `cursor`, or `None` when the request is cut short, which the module
/// answers with nothing. Script checks' chunks land in `received`.
pub(super) fn answer(
    host: &dyn WardenHost,
    types: &CheckTypes,
    bindings: &Bindings,
    xor: u8,
    cursor: &mut Cursor,
    received: &mut Received,
) -> Option<Vec<u8>> {
    let mut strings = Vec::new();
    loop {
        let s = cursor.string()?;
        if s.is_empty() {
            break;
        }
        strings.push(s);
    }
    let mut checks = Vec::new();
    while let Some(byte) = cursor.u8() {
        let check = match byte ^ xor {
            t if t == types.timing => Check::Timing,
            t if t == types.driver => {
                cursor.take(4 + 20)?;
                Check::Driver { name: cursor.u8()? }
            }
            t if t == types.proc => {
                cursor.take(4 + 20)?;
                let module = cursor.u8()?;
                cursor.take(1 + 4 + 1)?;
                Check::Proc { module }
            }
            t if t == types.script => Check::Script { text: cursor.u8()? },
            t if t == types.file => Check::File { path: cursor.u8()? },
            t if t == types.page_a || t == types.page_b => {
                cursor.take(4 + 20 + 4 + 1)?;
                Check::Page
            }
            t if t == types.module => {
                cursor.take(4 + 20)?;
                Check::Module
            }
            t if t == types.memory => Check::Memory {
                module: cursor.u8()?,
                offset: cursor.u32()?,
                len: cursor.u8()?,
            },
            _ => break,
        };
        checks.push(check);
    }

    // Index 0 is the empty name; a later one names a table string. A check naming none is skipped.
    let string = |i: u8| -> Option<&str> {
        match i {
            0 => Some(""),
            i => strings.get(usize::from(i) - 1).map(String::as_str),
        }
    };
    let mut results = Vec::new();
    let mut notes = Vec::new();
    for check in checks {
        match check {
            Check::Timing => {
                // The tick count read twice around the clock agrees with it: no detour.
                results.push(1);
                let now = if bindings.clock == ASYNC_CLOCK {
                    host.async_time_ms()
                } else {
                    host.tick_count()
                };
                results.extend_from_slice(&now.to_le_bytes());
                notes.push(format!("timing {now}"));
            }
            Check::Driver { name } | Check::Proc { module: name } => {
                if let Some(name) = string(name) {
                    results.push(NOT_FOUND);
                    notes.push(format!("scan {name:?} not found"));
                }
            }
            Check::Page | Check::Module => {
                results.push(NOT_FOUND);
                notes.push("scan not found".to_owned());
            }
            Check::Script { text } => {
                let Some(text) = string(text) else { continue };
                // Slot 0 is called first when bound; only the executor in slot 1 runs the text.
                if bindings.script[0] == 0 && bindings.script[1] == EXECUTE_SCRIPT {
                    received.scripts.push(text.to_owned());
                }
                results.push(1);
                notes.push(format!("script {text:?}"));
            }
            Check::File { path } => {
                let Some(path) = string(path) else { continue };
                // The whole file's SHA-1, read through the bound reader.
                let sha: Option<[u8; 20]> = (bindings.file_kind == 1
                    && bindings.file == FILE_READER)
                    .then(|| host.read_file(path))
                    .flatten()
                    .map(|bytes| Sha1::digest(&bytes).into());
                notes.push(format!(
                    "file {path:?} {}",
                    if sha.is_some() { "hashed" } else { "unread" }
                ));
                match sha {
                    Some(sha) => {
                        results.push(0);
                        results.extend_from_slice(&sha);
                    }
                    None => results.push(1),
                }
            }
            Check::Memory {
                module,
                offset,
                len,
            } => {
                let Some(name) = string(module) else { continue };
                // No name reads the address as given; a module name reads from its base.
                let va = if name.is_empty() {
                    Some(offset)
                } else {
                    super::resolve(name, 0).map(|base| base.wrapping_add(offset))
                };
                let read = va.and_then(|va| host.read_memory(va, usize::from(len)));
                notes.push(format!(
                    "memory {name:?}+{offset:#x}/{len} {}",
                    if read.is_some() { "read" } else { "unread" }
                ));
                match read {
                    Some(bytes) => {
                        results.push(0);
                        results.extend_from_slice(&bytes);
                    }
                    None => results.push(1),
                }
            }
        }
    }

    host.note(&format!("checks ({}): {}", notes.len(), notes.join("; ")));
    let mut reply = vec![CMSG_CHEAT_CHECKS_RESULT];
    reply.extend_from_slice(&(results.len() as u16).to_le_bytes());
    reply.extend_from_slice(&checksum(&results).to_le_bytes());
    reply.extend_from_slice(&results);
    Some(reply)
}
