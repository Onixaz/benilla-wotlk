//! The chat windows' saved state, the whole `chat-cache.txt` record: the per-type colours and each
//! window's tint, alpha, font size, lock, interactivity, dock, shown flag, saved anchor and size,
//! name, message groups and channels. It lives in the VM, written and read by the stock verbs
//! (`SetChatWindow*`, `ChangeChatColor`, `GetChatWindowInfo` and kin); this module loads it at
//! login and saves it at logout, in the grammar of the reference's
//! `WTF/Account/<ACC>/<REALM>/<CHAR>/chat-cache.txt` (written whole by `0x4fd300`, read by
//! `0x508320`):
//!
//! ```text
//! VERSION 5
//! ADDEDVERSION 13
//! OPTION_GUILD_RECRUITMENT_CHANNEL AUTO
//! CHANNELS … END           ← the custom channels, one per line, re-joined at login
//! ZONECHANNELS 18874371    ← the joined zone channels, as bits 1<<(ChannelID-1)
//! COLORS … END             ← the chat-type registry, one SAY 255 255 255 row per type
//! WINDOW 1                 ← then per window:
//! NAME General             ← only when a name was stored
//! SIZE 0
//! COLOR 0 0 0 40           ← R G B A bytes
//! LOCKED 1
//! UNINTERACTABLE 0
//! DOCKED 1
//! SHOWN 1
//! POSITION BOTTOMLEFT 0.031250 0.130000   ← only once a position was saved, with
//! DIMENSIONS 430.000000 120.000000        ← the size
//! MESSAGES … END           ← the enabled groups of the 46-entry table, in order
//! CHANNELS … END           ← this window's custom channels; its zone channels are bits
//! ZONECHANNELS 18874371    ← this window's zone channels, masked by the joined set
//! END
//! ```
//!
//! The loader zeroes every window's flags first, so a `MESSAGES` block is the whole set; a window
//! the file omits keeps its boot init ([`ChatWindowLook::stock`]), and a file older than
//! `ADDEDVERSION 13` gets the groups added since. `LOCKED` is written as `!= 0`, so only 0 and 1
//! round-trip. Deviation: `set_chat_window_looks` drops an out-of-range window, where the
//! reference's `WINDOW` bound is off by one (`index > 10`), because that write would land past the
//! array.
//!
//! The file is account-data slot 7 ([`crate::account_data`]); its reader is as lenient as the
//! reference's (keys case-insensitive, unknown keys and blank lines skipped).
//! `UPDATE_CHAT_WINDOWS`, then `UPDATE_CHAT_COLOR` per registry entry, fire file or no file. It is
//! written whole at every UI shutdown (`CGChat::SaveChatSettings` `0x4ffda0`), and at no other time.

use bevy::prelude::*;

use benilla_ui::script::{
    frame_point_index, frame_point_name, ChatTypeColor, ChatWindowLook, UiScript,
    CHAT_CACHE_ADDED_VERSION, COMBAT_LOG_FIRST_GROUP, MESSAGE_GROUPS,
};

use super::edit::zone_bit;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::VmMemo;

/// Whether this VM restored the file, which is also what owes the shutdown a write: a VM that
/// never read it must not write its stock table over the player's.
#[derive(Resource, Default)]
pub(super) struct ChatWindowFile {
    restored: VmMemo<bool>,
}

/// A parsed file: its windows (0-based), its `COLORS` rows in file order, its custom channels.
#[derive(Debug, PartialEq)]
pub(super) struct Parsed {
    pub(super) looks: Vec<(usize, ChatWindowLook)>,
    pub(super) colors: Vec<(String, [u8; 3], bool)>,
    pub(super) joined: Vec<super::edit::SavedChannel>,
    /// The header's `ZONECHANNELS` word, the reference's `[0xb6e5e0]`, overwritten from the file
    /// (`0x498d83`); `None` without the line, so the loader seeds from the DBC rather than 0.
    pub(super) zone_mask: Option<u32>,
    /// `OPTION_GUILD_RECRUITMENT_CHANNEL`, the latch `GetGuildRecruitmentMode` returns: `STANDARD`
    /// is 0 and any other word, or none, is 1 (`0x49ea70`, `ecx=0` only for `STANDARD`).
    pub(super) guild_recruitment_auto: bool,
}

impl Default for Parsed {
    fn default() -> Self {
        Self {
            looks: Vec::new(),
            colors: Vec::new(),
            joined: Vec::new(),
            zone_mask: None,
            // The boot value: the reference writes `AUTO` for characters that never set it.
            guild_recruitment_auto: true,
        }
    }
}

/// Render the file as the writer does (`0x4fd300`). `custom` fills the header's `CHANNELS`, custom
/// channels only: a zone channel travels as its bit. `zone_mask` goes raw into the header and is
/// ANDed with each window's bits. Both are the durable lists, never the live roster, which the
/// session end clears before the flush reads it.
fn render(
    looks: &[ChatWindowLook],
    colors: &[ChatTypeColor],
    custom: &[super::edit::SavedChannel],
    zone_mask: u32,
    guild_recruitment_auto: bool,
) -> String {
    let mut out = String::new();
    // The latch `SetGuildRecruitmentMode` writes; this is the one place it persists.
    let recruitment = if guild_recruitment_auto {
        "AUTO"
    } else {
        "STANDARD"
    };
    out.push_str(&format!(
        "VERSION 5\n\nADDEDVERSION {CHAT_CACHE_ADDED_VERSION}\n\n\
         OPTION_GUILD_RECRUITMENT_CHANNEL {recruitment}\n\nCHANNELS\n"
    ));
    for c in custom {
        out.push_str(&format!("{} {} {}\n", c.name, i32::from(c.voice), c.number));
    }
    out.push_str(&format!("END\n\nZONECHANNELS {zone_mask}\n\nCOLORS\n"));
    for c in colors {
        out.push_str(&format!(
            "{} {} {} {} {}\n",
            c.name,
            c.rgb[0],
            c.rgb[1],
            c.rgb[2],
            if c.by_class { 'Y' } else { 'N' }
        ));
    }
    out.push_str("END\n\n");
    for (i, l) in looks.iter().enumerate() {
        out.push_str(&format!("WINDOW {}\n", i + 1));
        if !l.name.is_empty() {
            out.push_str(&format!("NAME {}\n", l.name));
        }
        out.push_str(&format!(
            "SIZE {}\nCOLOR {} {} {} {}\nLOCKED {}\nUNINTERACTABLE {}\nDOCKED {}\nSHOWN {}\n",
            l.font_size,
            l.r,
            l.g,
            l.b,
            l.a,
            i32::from(l.locked),
            i32::from(l.uninteractable),
            l.docked.unwrap_or(0),
            i32::from(l.shown),
        ));
        // `%f`, six decimals, as the writer's `SStrPrintf`; a blank line after the size.
        if l.placed {
            out.push_str(&format!(
                "POSITION {} {:.6} {:.6}\nDIMENSIONS {:.6} {:.6}\n\n",
                frame_point_name(l.point),
                l.offset.0,
                l.offset.1,
                l.dimensions.0,
                l.dimensions.1,
            ));
        }
        out.push_str("MESSAGES\n");
        for m in &l.messages {
            out.push_str(m);
            out.push('\n');
        }
        out.push_str("END\n\nCHANNELS\n");
        // The window's two lists: its names (`+0xb8`), then its zone ids (`+0xc8`) as bits.
        for name in &l.named {
            out.push_str(name);
            out.push('\n');
        }
        let window_mask = l.zone_ids.iter().fold(0, |m, id| m | zone_bit(*id));
        out.push_str(&format!(
            "END\n\nZONECHANNELS {}\n\nEND\n\n",
            window_mask & zone_mask
        ));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// Top level, or inside a `WINDOW` whose keys arrive one per line.
    Top,
    /// The header's `CHANNELS … END`: the custom channels to re-join.
    Joined,
    Colors,
    Messages,
    Channels,
}

/// Parse a file. `rows` is `ChatChannels.dbc` as `(id, Shortcut)`, which the in-window
/// `ZONECHANNELS` arm needs to turn bits back into `(Shortcut, id)` channel rows.
fn parse(text: &str, rows: &[(u32, String)]) -> Parsed {
    let mut out = Parsed::default();
    let mut current: Option<(usize, ChatWindowLook)> = None;
    let mut block = Block::Top;
    let mut added_version: u8 = 0;
    let byte = |s: Option<&str>| -> u8 { s.and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) };
    let flush = |current: &mut Option<(usize, ChatWindowLook)>, out: &mut Parsed| {
        if let Some(w) = current.take() {
            out.looks.push(w);
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let Some(head) = it.next() else { continue };
        let is_end = head.eq_ignore_ascii_case("END");
        match block {
            Block::Colors => {
                if is_end {
                    block = Block::Top;
                } else {
                    let rgb = [byte(it.next()), byte(it.next()), byte(it.next())];
                    let by_class = it.next().is_some_and(|f| f.eq_ignore_ascii_case("Y"));
                    out.colors.push((head.to_ascii_uppercase(), rgb, by_class));
                }
                continue;
            }
            Block::Joined => {
                if is_end {
                    block = Block::Top;
                } else {
                    // `<name> <voice> <number>` (`0x4fd455`); the reader takes `atol`s.
                    let voice = it
                        .next()
                        .is_some_and(|v| v.parse::<i32>().unwrap_or(0) != 0);
                    let number = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                    out.joined.push(super::edit::SavedChannel {
                        name: head.to_string(),
                        voice,
                        number,
                    });
                }
                continue;
            }
            Block::Messages => {
                if is_end {
                    block = Block::Top;
                } else if let Some((_, look)) = current.as_mut() {
                    look.messages.push(head.to_ascii_uppercase());
                }
                continue;
            }
            Block::Channels => {
                if is_end {
                    block = Block::Top;
                } else if let Some((_, look)) = current.as_mut() {
                    // A word that is a `ChatChannels.dbc` Shortcut keeps its id, which
                    // `ChatFrame.lua:1379` matches lines by; it stays listed by name.
                    let id = rows
                        .iter()
                        .find(|(_, shortcut)| shortcut.eq_ignore_ascii_case(head))
                        .map_or(0, |(id, _)| *id);
                    look.channels.push((head.to_string(), id));
                    look.named.push(head.to_string());
                }
                continue;
            }
            Block::Top => {}
        }
        if head.eq_ignore_ascii_case("COLORS") {
            block = Block::Colors;
            continue;
        }
        if head.eq_ignore_ascii_case("WINDOW") {
            flush(&mut current, &mut out);
            let Some(index) = it.next().and_then(|n| n.parse::<usize>().ok()) else {
                warn!("chat cache: WINDOW line with no number ignored: {line}");
                continue;
            };
            if index == 0 {
                continue;
            }
            let mut look = ChatWindowLook::stock(index - 1);
            // benilla's older one-line row: the keys follow on the same line.
            while let Some(key) = it.next() {
                apply_key(&mut look, key, &mut it, rows);
            }
            current = Some((index - 1, look));
            continue;
        }
        let Some((_, look)) = current.as_mut() else {
            if head.eq_ignore_ascii_case("CHANNELS") {
                block = Block::Joined;
            } else if head.eq_ignore_ascii_case("ADDEDVERSION") {
                added_version = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            } else if head.eq_ignore_ascii_case("ZONECHANNELS") {
                // Overwrites `[0xb6e5e0]` (`0x498d83`), never ORs: the file holds the whole mask.
                out.zone_mask = it.next().and_then(|v| v.trim().parse::<u32>().ok());
            } else if head.eq_ignore_ascii_case("OPTION_GUILD_RECRUITMENT_CHANNEL") {
                // The reference's test, whole-word and case-folded: only `STANDARD` means 0.
                out.guild_recruitment_auto = !it
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case("STANDARD"));
            }
            // `VERSION` is not read.
            continue;
        };
        if is_end {
            flush(&mut current, &mut out);
        } else if head.eq_ignore_ascii_case("MESSAGES") {
            look.messages.clear();
            block = Block::Messages;
        } else if head.eq_ignore_ascii_case("CHANNELS") {
            look.channels.retain(|(_, id)| *id != 0);
            look.named.clear();
            block = Block::Channels;
        } else if head.eq_ignore_ascii_case("NAME") {
            look.name = line[4..].trim().to_string();
        } else {
            apply_key(look, head, &mut it, rows);
        }
    }
    flush(&mut current, &mut out);
    // The loader's EOF back-fill: an older file gets the `defaultOn` groups added since, below
    // group 40 into window 1 and the rest into window 2.
    if added_version < CHAT_CACHE_ADDED_VERSION {
        for (i, (name, on, ver)) in MESSAGE_GROUPS.iter().enumerate() {
            if *on && *ver > added_version {
                let target = usize::from(i >= COMBAT_LOG_FIRST_GROUP);
                if let Some((_, look)) = out.looks.iter_mut().find(|(w, _)| *w == target) {
                    if !look.messages.iter().any(|m| m == name) {
                        look.messages.push((*name).to_string());
                    }
                }
            }
        }
    }
    for (_, look) in &mut out.looks {
        look.normalize_messages();
    }
    out
}

/// One `KEY value…` of a window block, whichever line it arrived on.
fn apply_key<'a>(
    look: &mut ChatWindowLook,
    key: &str,
    it: &mut impl Iterator<Item = &'a str>,
    rows: &[(u32, String)],
) {
    let byte = |s: Option<&str>| -> u8 { s.and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) };
    let flag = |s: Option<&str>| -> bool { s.is_none_or(|v| v.trim() != "0") };
    // `SStrToFloat`: 0 for a missing or malformed word.
    let float = |s: Option<&str>| -> f32 { s.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) };
    if key.eq_ignore_ascii_case("SIZE") {
        look.font_size = it
            .next()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0)
            .max(0);
    } else if key.eq_ignore_ascii_case("COLOR") {
        look.r = byte(it.next());
        look.g = byte(it.next());
        look.b = byte(it.next());
        look.a = byte(it.next());
    } else if key.eq_ignore_ascii_case("LOCKED") {
        look.locked = flag(it.next());
    } else if key.eq_ignore_ascii_case("UNINTERACTABLE") {
        look.uninteractable = it.next().is_some_and(|v| v.trim() != "0");
    } else if key.eq_ignore_ascii_case("POSITION") {
        // An unknown point keeps the old one, as `StringToFramePoint` writes only on a match.
        if let Some(point) = it.next().and_then(frame_point_index) {
            look.point = point;
        }
        look.offset = (float(it.next()), float(it.next()));
        look.placed = true;
    } else if key.eq_ignore_ascii_case("DIMENSIONS") {
        look.dimensions = (float(it.next()), float(it.next()));
        look.placed = true;
    } else if key.eq_ignore_ascii_case("DOCKED") {
        look.docked = it
            .next()
            .and_then(|v| v.trim().parse::<u8>().ok())
            .filter(|p| *p > 0);
    } else if key.eq_ignore_ascii_case("SHOWN") {
        look.shown = flag(it.next());
    } else if key.eq_ignore_ascii_case("ZONECHANNELS") {
        // The in-window arm: every DBC row whose bit is set joins the window as (Shortcut, id).
        let mask = it
            .next()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);
        for (id, shortcut) in rows {
            if mask & zone_bit(*id) == 0 {
                continue;
            }
            if !look.zone_ids.contains(id) {
                look.zone_ids.push(*id);
            }
            if !look
                .channels
                .iter()
                .any(|(c, _)| c.eq_ignore_ascii_case(shortcut))
            {
                look.channels.push((shortcut.clone(), *id));
            }
        }
    }
}

/// The `(id, Shortcut)` rows the parser needs, off the loaded `ChatChannels.dbc`.
fn shortcut_rows(channels: &super::edit::ChannelState) -> Vec<(u32, String)> {
    channels
        .channels
        .rows()
        .iter()
        .map(|r| (r.id, r.shortcut.clone()))
        .collect()
}

/// Restore the file into a fresh VM, once per character per VM, and fire the loader's two events,
/// file or no file. Called from the world-entry UI load, not `Update`, for those events:
/// `UPDATE_CHAT_WINDOWS` registers a window for any `CHAT_MSG_*`, so
/// it must precede the server's MOTD after `SMSG_LOGIN_VERIFY_WORLD`, and the `UPDATE_CHAT_COLOR`
/// burst must precede `PLAYER_LOGIN`: its `WHISPER` row also repaints `ChatTypeInfo["REPLY"]`
/// (`ChatFrame.lua:1357-1365`), whose `.id` is 0, and `UpdateColorByID(0, …)` recolours every line
/// printed with no explicit colour, an addon's login `Print` among them.
pub(crate) fn restore_chat_looks(world: &mut World, script: &mut UiScript) {
    // Owned rows up front: `ChatWindowFile` is borrowed mutably below.
    let Some(channels) = world.get_resource::<super::edit::ChannelState>() else {
        return;
    };
    let rows = shortcut_rows(channels);
    let auto_rows: Vec<(String, u32)> = channels
        .channels
        .auto_join_rows()
        .map(|r| (r.shortcut.clone(), r.id))
        .collect();
    let commands = world.get_resource::<NetCommands>().map(|c| c.0.clone());
    // The DBC seed: every `ChatChannels.dbc` row the client joins by itself (`flags & 1`).
    let seed_mask = auto_rows.iter().fold(0, |m, (_, id)| m | zone_bit(*id));
    // The slot's bytes, `None` while it waits for the server; an empty file is no file.
    let text = world
        .get_resource::<crate::account_data::AccountData>()
        .and_then(|d| d.read(crate::account_data::store::CHAT))
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let mut parsed;
    {
        let Some(mut file) = world.get_resource_mut::<ChatWindowFile>() else {
            return;
        };
        if *file.restored.get(script) {
            return; // already restored into this VM
        }
        *file.restored.get(script) = true;
        let had_file = text.is_some();
        parsed = text.map(|t| parse(&t, &rows)).unwrap_or_default();
        if !had_file {
            // The loader's no-file path: window 1 gets each seed row as
            // `(Shortcut, id)`, which `ChatFrame_RegisterForChannels` matches zone speech by.
            let mut general = ChatWindowLook::stock(0);
            general.channels = auto_rows.clone();
            general.zone_ids = auto_rows.iter().map(|(_, id)| *id).collect();
            parsed.looks.push((0, general));
        }
        if !parsed.looks.is_empty() || !parsed.colors.is_empty() {
            info!(
                "chat cache: {} windows, {} colour rows, {} custom channels restored",
                parsed.looks.len(),
                parsed.colors.len(),
                parsed.joined.len()
            );
        }
    }
    // The durable mask: the file's word, else the DBC seed; confirmed joins OR into it after.
    let mask = parsed.zone_mask.unwrap_or(seed_mask);
    if let Some(mut channels) = world.get_resource_mut::<super::edit::ChannelState>() {
        // `Some` is the reference's "chat system ready" flag (`0x499a18`): the zone walk and the
        // guild-recruitment cascade wait for it.
        channels.zone_mask = Some(mask);
        // The custom re-join list, seated here, then moved only by a confirmed join or a leave.
        channels.custom = parsed.joined.clone();
    }
    script.set_guild_recruitment_mode(u8::from(parsed.guild_recruitment_auto));
    script.set_chat_colors(parsed.colors);
    script.set_chat_window_looks(parsed.looks);
    // `UPDATE_CHAT_WINDOWS` once, then `UPDATE_CHAT_COLOR` for every registry entry, file or no
    // file.
    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    let renorm = |b: u8| f64::from(b as f32 * (1.0f32 / 255.0f32));
    for entry in script.chat_colors() {
        script.fire_event(
            "UPDATE_CHAT_COLOR",
            vec![
                benilla_ui::script::ScriptValue::Str(entry.name),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[0])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[1])),
                benilla_ui::script::ScriptValue::Number(renorm(entry.rgb[2])),
            ],
        );
    }
    // The header's `CHANNELS` re-join at login, as in the reference; zone channels are the walk's.
    // The list holds custom channels only, so each carries id 0; 3.3.5a's re-join (`0x50819c`)
    // is the client's own, auto 1.
    if let Some(commands) = commands {
        for c in parsed.joined {
            let _ = commands.send(ClientCommand::JoinChannel {
                name: c.name,
                password: String::new(),
                channel_id: 0,
                auto_joined: true,
                voice: c.voice,
            });
        }
    }
}

/// `CGChat::SaveChatSettings` (`0x4ffda0`) at the UI shutdown: the whole file, unconditionally,
/// from a VM that restored it. Never from an unseated mask: `None` means the file was never read,
/// and writing would zero every block's `ZONECHANNELS`.
pub(crate) fn save_to_account_data(world: &mut World) {
    let Some(mut file) = world.remove_resource::<ChatWindowFile>() else {
        return;
    };
    let body = {
        let script = world.get_non_send_resource::<UiScript>();
        let channels = world.get_resource::<super::edit::ChannelState>();
        match (script, channels) {
            (Some(script), Some(channels)) if *file.restored.get(script) => {
                channels.zone_mask.map(|zone_mask| {
                    render(
                        &script.chat_window_looks(),
                        &script.chat_colors(),
                        &channels.custom,
                        zone_mask,
                        script.guild_recruitment_mode() != 0,
                    )
                })
            }
            _ => None,
        }
    };
    world.insert_resource(file);
    if let Some(body) = body {
        crate::account_data::save_slots(
            world,
            &[(crate::account_data::store::CHAT, body.into_bytes())],
        );
    }
}

pub(super) fn plugin(app: &mut App) {
    // The restore runs from the world-entry UI load: its events must precede the first chat line
    // and `PLAYER_LOGIN`, which no `Update` ordering guarantees; the save is the UI shutdown's.
    app.init_resource::<ChatWindowFile>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window `n`'s (1-based) boot-init record, which differs per window, with the look fields set.
    fn look(n: usize, r: u8, g: u8, b: u8, a: u8, font_size: i32) -> ChatWindowLook {
        ChatWindowLook {
            r,
            g,
            b,
            a,
            font_size,
            locked: true,
            ..ChatWindowLook::stock(n - 1)
        }
    }

    /// A subset of `ChatChannels.dbc` as [`parse`] takes it, with the three `INITIAL` rows.
    fn rows() -> Vec<(u32, String)> {
        vec![
            (1, "General".to_string()),
            (2, "Trade".to_string()),
            (22, "LocalDefense".to_string()),
            (24, "LookingForGroup".to_string()),
        ]
    }

    fn colors(rows: &[(&str, [u8; 3])]) -> Vec<ChatTypeColor> {
        rows.iter()
            .map(|(n, rgb)| ChatTypeColor {
                name: (*n).to_string(),
                rgb: *rgb,
                by_class: false,
            })
            .collect()
    }

    /// `channels` with the window's two lists as a join fills them: a custom name by name, a zone
    /// channel by id.
    fn joined(mut l: ChatWindowLook, channels: Vec<(&str, u32)>) -> ChatWindowLook {
        l.channels = channels
            .iter()
            .map(|(n, id)| (n.to_string(), *id))
            .collect();
        l.named = channels
            .iter()
            .filter(|(_, id)| *id == 0)
            .map(|(n, _)| n.to_string())
            .collect();
        l.zone_ids = channels
            .iter()
            .map(|(_, id)| *id)
            .filter(|id| *id != 0)
            .collect();
        l
    }

    #[test]
    fn the_file_round_trips() {
        let mut named = look(3, 9, 8, 7, 6, 12);
        named.name = "Loot & Trade".into();
        named.shown = true;
        named.uninteractable = true;
        named.point = 2;
        named.offset = (0.25, -0.5);
        named.placed = true;
        named.dimensions = (500.0, 200.5);
        named.messages = vec!["LOOT".into(), "MONEY".into()];
        let named = joined(named, vec![("MyChan", 0), ("Trade", 2)]);
        let looks = vec![
            look(1, 0, 0, 0, 64, 14),
            look(2, 255, 128, 0, 255, 0),
            named,
        ];
        let colors = colors(&[("SAY", [1, 2, 3]), ("CHANNEL7", [4, 5, 6])]);
        // The mask and the custom list are passed in, not derived from the live roster.
        let custom = vec![super::super::edit::SavedChannel {
            name: "MyChan".into(),
            voice: true,
            number: 5,
        }];
        let text = render(&looks, &colors, &custom, 0b11, true);
        assert!(
            text.contains("\nCHANNELS\nMyChan 1 5\nEND\n\nZONECHANNELS 3\n"),
            "{text}"
        );
        assert!(text.starts_with("VERSION 5\n\nADDEDVERSION 13\n"), "{text}");
        assert!(text.contains("SAY 1 2 3 N\n"), "{text}");
        assert!(text.contains("SHOWN 1\nMESSAGES\n"), "{text}");
        assert!(
            text.contains(
                "POSITION TOPRIGHT 0.250000 -0.500000\nDIMENSIONS 500.000000 200.500000\n\nMESSAGES"
            ),
            "{text}"
        );
        assert_eq!(
            text.matches(
                "
POSITION "
            )
            .count(),
            1,
            "only a saved window writes its geometry"
        );
        let parsed = parse(&text, &rows());
        let mut expect = looks.clone();
        // The zone channel comes back by its Shortcut row, after the custom names.
        expect[2].channels = vec![("MyChan".into(), 0), ("Trade".into(), 2)];
        assert_eq!(
            parsed.looks,
            vec![
                (0, expect[0].clone()),
                (1, expect[1].clone()),
                (2, expect[2].clone())
            ],
            "0-based indices, values intact"
        );
        assert_eq!(
            parsed.colors,
            vec![
                ("SAY".to_string(), [1, 2, 3], false),
                ("CHANNEL7".to_string(), [4, 5, 6], false)
            ]
        );
        assert_eq!(parsed.joined, custom);
    }

    /// The stock `ChatFrame_OnEvent` matches channel lines by id (`ChatFrame.lua:1379`), so a
    /// shortcut stored with id 0 would lose its channel's lines.
    #[test]
    fn a_window_channel_named_like_a_dbc_shortcut_regains_its_id() {
        let text = "WINDOW 1\nSIZE 0\n\nMESSAGES\nEND\n\nCHANNELS\nGeneral\nMyChan\n                    LocalDefense\nEND\n\nZONECHANNELS 2\n\nEND\n";
        let parsed = parse(text, &rows());
        assert_eq!(
            parsed.looks[0].1.channels,
            vec![
                ("General".to_string(), 1),
                ("MyChan".to_string(), 0),
                ("LocalDefense".to_string(), 22),
                ("Trade".to_string(), 2),
            ],
            "the two shortcuts come back as their DBC rows; the genuinely custom name keeps id 0, \
             and the ZONECHANNELS bit still contributes Trade"
        );

        // The next save keeps the names the window listed and the bit it carried.
        let out = render(&[parsed.looks[0].1.clone()], &[], &[], 0x0020_0003, false);
        let window = out.split("WINDOW 1").nth(1).unwrap();
        assert!(
            window.contains("CHANNELS\nGeneral\nMyChan\nLocalDefense\nEND"),
            "{out}"
        );
        assert!(window.contains("ZONECHANNELS 2\n"), "{out}");
    }

    #[test]
    fn a_windows_zone_bits_are_masked_by_the_joined_set() {
        let w = joined(look(1, 0, 0, 0, 0, 0), vec![("General", 1), ("Trade", 2)]);
        // The mask holds General alone: Trade was left, so its window bit is ANDed away.
        let text = render(&[w], &[], &[], 1, true);
        let windows: Vec<&str> = text.split("WINDOW 1").collect();
        assert!(windows[1].contains("ZONECHANNELS 1\n"), "{text}");
    }

    /// The header and window words come from the durable mask, not the live roster.
    #[test]
    fn an_empty_roster_does_not_erase_a_windows_zone_channels() {
        let w = joined(look(1, 0, 0, 0, 0, 0), vec![("General", 1), ("Trade", 2)]);
        // In both channels by the mask, with the live roster empty, as at a session-end save.
        let text = render(&[w], &[], &[], 0b11, true);
        assert!(
            text.contains("\nZONECHANNELS 3\n"),
            "the header carries the durable mask, not the roster's shadow: {text}"
        );
        let parsed = parse(&text, &rows());
        assert_eq!(parsed.zone_mask, Some(0b11), "the header word round-trips");
        assert_eq!(
            parsed.looks[0].1.channels,
            vec![("General".to_string(), 1), ("Trade".to_string(), 2)],
            "window 1 keeps both channels, where an unmarked file's came back empty"
        );
    }

    #[test]
    fn the_zone_mask_moves_on_join_and_explicit_leave_only() {
        let mut state = super::super::edit::ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x11,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
                benilla_formats::ChatChannelRow {
                    id: 2,
                    flags: 0x3b,
                    pattern: "Trade - %s".into(),
                    shortcut: "Trade".into(),
                },
            ]),
            ..Default::default()
        };
        // Unseated, a join is dropped.
        state.note_zone_channel_joined("General - Elwynn Forest");
        assert_eq!(state.zone_mask, None, "not seated: nothing to OR into");
        state.zone_mask = Some(0);
        state.note_zone_channel_joined("General - Elwynn Forest");
        state.note_zone_channel_joined("Trade - City");
        assert_eq!(state.zone_mask, Some(0b11));
        state.note_zone_channel_joined("MyChan");
        assert_eq!(state.zone_mask, Some(0b11), "a custom channel has no bit");
        // The clear is keyed on the slot the wire name finds and its id (`0x49f0f4`-`0x49f11a`);
        // a name with no slot clears nothing.
        state.note_zone_channel_left("Trade - City");
        assert_eq!(state.zone_mask, Some(0b11), "no slot carries it yet");
        state.claim_slot("Trade - City");
        state.note_zone_channel_left("Trade - City");
        assert_eq!(
            state.zone_mask,
            Some(0b01),
            "an explicit leave clears one bit"
        );
    }

    #[test]
    fn the_custom_list_moves_on_join_and_explicit_leave_only() {
        let mut state = super::super::edit::ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x11,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
            ]),
            ..Default::default()
        };
        state.note_custom_channel_joined("General - Elwynn Forest");
        assert!(state.custom.is_empty(), "a zone channel travels as its bit");
        state.note_custom_channel_joined("MyChan");
        state.note_custom_channel_joined("mychan");
        assert_eq!(
            state
                .custom
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["MyChan"],
            "once, by name"
        );
        state.note_custom_channel_left("MYCHAN");
        assert!(state.custom.is_empty(), "an explicit leave drops it");
    }

    /// benilla's older one-line rows (no `ADDEDVERSION`), with the back-fill into window 2.
    #[test]
    fn one_line_rows_without_addedversion_still_parse() {
        let got = parse(
            "window 1  size 16  color 10 20 30 40\n\
             WINDOW 2  SHOWN 0  COLOR 1 2 3 4  DOCKED 2  BOGUS 9\n\
             WINDOW 3  SIZE 12\n",
            &rows(),
        );
        let mut w2 = look(2, 1, 2, 3, 4, 0);
        w2.shown = false;
        w2.docked = Some(2);
        assert_eq!(
            got.looks,
            vec![
                (0, look(1, 10, 20, 30, 40, 16)),
                (1, w2),
                (2, look(3, 0, 0, 0, 0x28, 12)),
            ]
        );
        assert!(got.looks[0].1.messages.iter().any(|m| m == "MONEY"));
        assert!(got.looks[1].1.messages.iter().any(|m| m == "OPENING"));
    }

    #[test]
    fn a_stock_reference_file_parses() {
        let text = "VERSION 5\n\nADDEDVERSION 13\n\nOPTION_GUILD_RECRUITMENT_CHANNEL AUTO\n\n\
                    CHANNELS\nMyGuildChat\nEND\n\nZONECHANNELS 8388611\n\n\
                    COLORS\nSAY 255 255 255 N\nSYSTEM 200 200 0 N\nEND\n\n\
                    WINDOW 1\nSIZE 0\nCOLOR 0 0 0 40\nLOCKED 1\nUNINTERACTABLE 0\nDOCKED 1\n\
                    SHOWN 1\nMESSAGES\nSYSTEM\nSAY\nEND\n\nCHANNELS\nMyGuildChat\nEND\n\n\
                    ZONECHANNELS 8388611\n\nEND\n\n\
                    WINDOW 2\nNAME Combat Log\nSIZE 0\nCOLOR 0 0 0 40\nLOCKED 1\nUNINTERACTABLE 1\n\
                    DOCKED 2\nSHOWN 0\nPOSITION TOPLEFT 0.100000 -0.200000\n\
                    DIMENSIONS 300.000000 150.000000\n\n\
                    MESSAGES\nCOMBAT_XP_GAIN\nEND\n\nCHANNELS\nEND\n\nZONECHANNELS 0\n\nEND\n";
        let got = parse(text, &rows());
        assert_eq!(
            got.colors,
            vec![
                ("SAY".to_string(), [255, 255, 255], false),
                ("SYSTEM".to_string(), [200, 200, 0], false)
            ]
        );
        assert_eq!(
            got.joined,
            vec![super::super::edit::SavedChannel {
                name: "MyGuildChat".into(),
                voice: false,
                number: 0,
            }],
            "a bare name reads as no voice and no number"
        );
        assert_eq!(got.looks.len(), 2);
        let (i, w1) = &got.looks[0];
        assert_eq!(*i, 0);
        assert!(w1.shown && w1.name.is_empty());
        assert_eq!(w1.messages, vec!["SYSTEM".to_string(), "SAY".to_string()]);
        assert_eq!(
            w1.channels,
            vec![
                ("MyGuildChat".to_string(), 0),
                ("General".to_string(), 1),
                ("Trade".to_string(), 2),
                ("LookingForGroup".to_string(), 24),
            ],
            "bits 0, 1 and 23 of 8388611 — the shortcut rows, in DBC order"
        );
        let (_, w2) = &got.looks[1];
        assert!(!w2.shown);
        assert_eq!(w2.name, "Combat Log");
        assert_eq!(w2.docked, Some(2));
        assert_eq!(w2.messages, vec!["COMBAT_XP_GAIN".to_string()]);
        assert!(w2.uninteractable && w2.placed);
        assert_eq!((w2.point, w2.offset), (0, (0.1, -0.2)));
        assert_eq!(w2.dimensions, (300.0, 150.0));
    }

    #[test]
    fn the_lock_round_trips_and_an_absent_key_stays_locked() {
        let unlocked = ChatWindowLook {
            locked: false,
            ..look(1, 0, 0, 0, 64, 14)
        };
        let text = render(std::slice::from_ref(&unlocked), &[], &[], 0, true);
        assert!(text.contains("LOCKED 0"));
        assert_eq!(parse(&text, &rows()).looks, vec![(0, unlocked.clone())]);
        assert_eq!(
            parse("WINDOW 1\nSIZE 14\nCOLOR 0 0 0 64\nEND\n", &rows()).looks,
            vec![(0, look(1, 0, 0, 0, 64, 14))],
            "no LOCKED key = the init's LOCKED 1"
        );
    }

    #[test]
    fn dock_positions_round_trip_and_an_absent_key_keeps_the_init() {
        let moved = ChatWindowLook {
            docked: Some(3),
            ..look(1, 0, 0, 0, 0, 0)
        };
        let text = render(std::slice::from_ref(&moved), &[], &[], 0, true);
        assert!(text.contains("DOCKED 3"));
        assert_eq!(parse(&text, &rows()).looks, vec![(0, moved.clone())]);
        // The boot init: window 1 shown at dock index 1 with forty groups, window 2 at 2 with
        // six, window 3 hidden.
        let got = parse(
            "WINDOW 1  SIZE 0\nWINDOW 2  SIZE 0\nWINDOW 3  SIZE 0\n",
            &rows(),
        );
        assert_eq!(
            got.looks
                .iter()
                .map(|(_, l)| (l.docked, l.shown, l.messages.len()))
                .collect::<Vec<_>>(),
            vec![(Some(1), true, 40), (Some(2), true, 6), (None, false, 0)]
        );
    }

    #[test]
    fn junk_costs_only_its_own_line() {
        let got = parse(
            "WINDOW\nnot a window line\nWINDOW 0 SIZE 1\nWINDOW 2 SIZE 18\n",
            &rows(),
        );
        assert_eq!(got.looks, vec![(1, look(2, 0, 0, 0, 0x28, 18))]);
    }

    /// A `MESSAGES` block replaces the init set: the loader zeroes every flag before it reads.
    #[test]
    fn an_empty_messages_block_is_an_empty_set() {
        let got = parse("ADDEDVERSION 13\nWINDOW 1\nMESSAGES\nEND\nEND\n", &rows());
        assert!(got.looks[0].1.messages.is_empty());
    }

    /// The chat cache's account data, opened and heard from (sync off), its character folder in
    /// `tmp`: what a login leaves for the UI load. Returns the file's path.
    fn ready_chat_slot(world: &mut World, tmp: &std::path::Path) -> std::path::PathBuf {
        let dir = tmp.join("ACC/Realm/Char");
        let mut data = crate::account_data::AccountData::default();
        data.initialize(true, Some(tmp.join("ACC")));
        data.initialize(false, Some(dir.clone()));
        let times = benilla_protocol::messages::AccountDataTimes {
            server_time: 1,
            flag: 1,
            mask: 0xff,
            times: [Some(0); 8],
        };
        data.on_times(&times, &|_: &crate::account_data::SlotDef| false);
        world.insert_resource(data);
        world.init_resource::<crate::cvars::Cvars>();
        dir.join("chat-cache.txt")
    }

    /// A rename right before a `/reload`, which never crosses `OnExit(InWorld)`, reaches the file
    /// through the shutdown's save and comes back in the rebuilt VM.
    #[test]
    fn a_window_renamed_just_before_a_reload_reaches_the_file() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-chat-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("benilla-config")).expect("hermetic home");
        let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _home = crate::local_state::test_env::EnvGuard::set(
            "BENILLA_HOME",
            tmp.join("benilla-config")
                .to_str()
                .expect("utf-8 temp path"),
        );

        let mut world = World::new();
        world.init_resource::<crate::ui_script::AddOnIdentity>();
        world.init_resource::<crate::minimap::MinimapZoom>();
        world.init_resource::<crate::ui_script::ReloadUiPending>();
        world.init_resource::<super::super::edit::ChannelState>();
        world.init_resource::<ChatWindowFile>();
        let path = ready_chat_slot(&mut world, &tmp);
        crate::ui_script::setup_script(&mut world);
        world.insert_resource(probe_roster("Reloadprobe"));
        crate::ui_script::load_ingame_ui_on_world_entry(&mut world);

        world
            .non_send_resource_mut::<UiScript>()
            .run(r#"SetChatWindowName(1, "Reloaded")"#)
            .expect("rename");

        // `/reload` at once.
        world.insert_resource(State::new(crate::char_select::ClientState::InWorld));
        world.resource_mut::<crate::ui_script::ReloadUiPending>().0 = true;
        crate::ui_script::run_pending_reload(&mut world);

        let on_disk = std::fs::read_to_string(&path).expect("the shutdown wrote the file");
        assert!(
            on_disk.contains("NAME Reloaded"),
            "the rename must survive the reload; file was:\n{on_disk}"
        );

        // The rebuilt VM reads it back.
        let name = world
            .non_send_resource_mut::<UiScript>()
            .eval::<String>("GetChatWindowInfo(1)")
            .unwrap_or_default();
        assert_eq!(
            name, "Reloaded",
            "the rebuilt VM restored the renamed window, not the stale one"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A roster with one character picked, which keys the chat cache path.
    fn probe_roster(name: &str) -> crate::char_select::Roster {
        crate::char_select::Roster::with_pending_pick(
            vec![benilla_protocol::Character {
                guid: 1,
                name: name.into(),
                race: 1,  // Human → Alliance
                class: 1, // Warrior
                gender: 0,
                level: 60,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
                zone: 0,
                map: 0,
                position: benilla_protocol::wire::Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                flags: 0,
                customize_flags: 0,
                equipment: [benilla_protocol::CharEnumItem::default(); 19],
                bags: [benilla_protocol::CharEnumItem::default(); 4],
                pet_display_id: 0,
                pet_level: 0,
                pet_family: 0,
            }],
            1,
        )
    }

    /// The disconnect clears the live roster a frame before `end_ui_session` flushes, so the
    /// header's re-join list must come from the durable custom list; the same functions run here.
    #[test]
    fn a_custom_channel_survives_a_logout() {
        use bevy::ecs::system::RunSystemOnce;

        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-chat-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("benilla-config")).expect("hermetic home");
        let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _home = crate::local_state::test_env::EnvGuard::set(
            "BENILLA_HOME",
            tmp.join("benilla-config")
                .to_str()
                .expect("utf-8 temp path"),
        );

        let roster = probe_roster("Channelprobe");
        let mut world = World::new();
        let path = ready_chat_slot(&mut world, &tmp);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the character dir");
        // A file listing one custom channel in the header.
        let file = "VERSION 5\n\nCHANNELS\nMyChan 1 1\nEND\n\nZONECHANNELS 3\n\n\
             COLORS\nEND\n\nWINDOW 1\nSIZE 0\nMESSAGES\nSAY\nEND\n\nCHANNELS\nEND\n\n\
             ZONECHANNELS 3\n\nEND\n";
        std::fs::write(&path, file).expect("seed the file");

        world.init_resource::<crate::ui_script::AddOnIdentity>();
        world.init_resource::<crate::minimap::MinimapZoom>();
        world.init_resource::<crate::ui_script::ReloadUiPending>();
        world.init_resource::<super::super::edit::ChannelState>();
        world.init_resource::<super::super::channels::ZoneChannelWalk>();
        world.init_resource::<super::super::recruitment::GuildRecruitmentCascade>();
        world.init_resource::<Messages<crate::net::DisconnectedMessage>>();
        world.init_resource::<ChatWindowFile>();
        crate::ui_script::setup_script(&mut world);
        world.insert_resource(roster);
        crate::ui_script::load_ingame_ui_on_world_entry(&mut world);

        // The server confirms the re-join (`YOU_JOINED`) through the real feed arm.
        world.resource_scope(
            |world, mut channels: Mut<super::super::edit::ChannelState>| {
                let mut script = world.non_send_resource_mut::<UiScript>();
                let mut windows = super::super::frames::ChatWindows::default();
                let mut e = super::super::event::ChatEvent::text_only(
                    super::super::event::ChatEventKind::ChannelNotice,
                    String::new(),
                );
                e.channel = "MyChan".into();
                e.notice = "2".into(); // YOU_JOINED
                super::super::feed::deliver(&mut script, &mut windows, &mut channels, &mut e);
            },
        );
        assert_eq!(
            world
                .resource::<super::super::edit::ChannelState>()
                .number_of("MyChan"),
            Some(1),
            "precondition: the channel is joined and numbered"
        );

        // `/logout`: the disconnect lands in `Update` first…
        world.write_message(crate::net::DisconnectedMessage::new(
            "logged out".into(),
            benilla_protocol::SessionEnd::LoggedOut,
        ));
        world
            .run_system_once(super::super::channels::end_session_channels_on_disconnect)
            .expect("the disconnect twin runs");
        // …and `OnExit(InWorld)` flushes the dying VM's cache a frame later.
        crate::ui_script::end_ui_session(&mut world);

        let on_disk = std::fs::read_to_string(&path).expect("the shutdown wrote the file");
        assert!(
            on_disk.contains("\nCHANNELS\nMyChan 1 1\nEND\n\nZONECHANNELS"),
            "the custom channel is still in the header's re-join list; file was:\n{on_disk}"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Every `chat-cache.txt` the real 12340 client wrote into the install comes back byte for byte
    /// through the parser and the writer, with the install's own `ChatChannels.dbc`.
    #[test]
    fn the_reference_clients_own_files_round_trip() {
        let data = benilla_formats::wow_data_or_skip!();
        let Some(root) = data.parent().map(|r| r.join("WTF").join("Account")) else {
            return;
        };
        let Ok(mut chain) = benilla_formats::Chain::open(&data) else {
            return;
        };
        let Ok(catalog) = benilla_formats::load_chat_channels_catalog(&mut chain) else {
            return;
        };
        let rows: Vec<(u32, String)> = catalog
            .rows()
            .iter()
            .map(|r| (r.id, r.shortcut.clone()))
            .collect();
        let files: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|acc| {
                std::fs::read_dir(acc.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .flat_map(|realm| {
                std::fs::read_dir(realm.path())
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .map(|c| c.path().join("chat-cache.txt"))
            .filter(|p| p.is_file())
            .collect();
        for path in &files {
            let text = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
            let parsed = parse(&text, &rows);
            let looks: Vec<ChatWindowLook> = parsed.looks.iter().map(|(_, l)| l.clone()).collect();
            let colors: Vec<ChatTypeColor> = parsed
                .colors
                .iter()
                .map(|(name, rgb, by_class)| ChatTypeColor {
                    name: name.clone(),
                    rgb: *rgb,
                    by_class: *by_class,
                })
                .collect();
            let back = render(
                &looks,
                &colors,
                &parsed.joined,
                parsed.zone_mask.unwrap_or(0),
                parsed.guild_recruitment_auto,
            );
            assert_eq!(back, text, "{} writes back as read", path.display());
        }
    }
}
