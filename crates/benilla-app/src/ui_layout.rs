//! The placed frames: the windows the player has moved or resized (the userPlaced bit,
//! `frame+0xb4 & 0x1000`), kept in 12340's `layout-local.txt`, account-data slot 6, which stays
//! local: never uploaded, never downloaded. The engine half (snapshot, restore) is
//! `benilla_ui::script`'s `layout_cache`; this is the file and when it is read and written.
//!
//! The grammar, LF, integers (writer `0x5234a0`, per-frame callback `0x517e00`):
//!
//! ```text
//! Version: 1
//! Frame: WorldMapScreenAnchor
//! FrameLevel: 0
//! Anchor: TOPLEFT
//! X: 10
//! Y: -118
//! W: 1
//! H: 1
//! ```
//!
//! The reader (`0x5183a0`) matches each line's prefix in any case, starts a block at `Frame:` with
//! level -1, anchor `TOPLEFT` and zero offsets and size, and ignores the version and any other
//! line. The file is read after the UI loads and written at its shutdown, on logout, quit,
//! disconnect and `/reload`, and at no other time.

use bevy::prelude::*;

use benilla_ui::script::{PlacedFrame, UiScript};

use crate::ui_script::VmMemo;

/// Whether this VM has had the file seated, which is also what owes the shutdown a write: a VM
/// that never read it must not write its authored tree over the player's file.
#[derive(Resource, Default)]
pub(crate) struct LayoutFile {
    restored: VmMemo<bool>,
}

/// The file as the writer composes it.
pub(crate) fn render(frames: &[PlacedFrame]) -> String {
    let mut out = String::from("Version: 1\n");
    for f in frames {
        out.push_str(&format!(
            "Frame: {}\nFrameLevel: {}\nAnchor: {}\nX: {}\nY: {}\nW: {}\nH: {}\n",
            f.name, f.level, f.anchor, f.x, f.y, f.w, f.h
        ));
    }
    out
}

/// The client's string-to-integer parse: the leading integer, 0 when there is none.
fn int(s: &str) -> i32 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0)
}

/// The file as its rows, each block flushed at the next `Frame:` and at the end.
pub(crate) fn parse(text: &str) -> Vec<PlacedFrame> {
    let mut out = Vec::new();
    let mut open: Option<PlacedFrame> = None;
    let strip = |line: &str, key: &str| -> Option<String> {
        let head = line.get(..key.len())?;
        head.eq_ignore_ascii_case(key)
            .then(|| line[key.len()..].to_string())
    };
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some(name) = strip(line, "Frame: ") {
            out.extend(open.take());
            open = Some(PlacedFrame {
                name,
                level: -1,
                anchor: "TOPLEFT".into(),
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            });
            continue;
        }
        let Some(f) = open.as_mut() else {
            continue;
        };
        if let Some(v) = strip(line, "FrameLevel: ") {
            f.level = int(&v);
        } else if let Some(v) = strip(line, "Anchor: ") {
            f.anchor = v.trim().to_string();
        } else if let Some(v) = strip(line, "X: ") {
            f.x = int(&v);
        } else if let Some(v) = strip(line, "Y: ") {
            f.y = int(&v);
        } else if let Some(v) = strip(line, "W: ") {
            f.w = int(&v);
        } else if let Some(v) = strip(line, "H: ") {
            f.h = int(&v);
        }
    }
    out.extend(open);
    out
}

/// Seat the saved geometry into the VM once, when the slot has heard from the server. It runs
/// after the UI tree is built and the load-time `UIParent_ManageFramePositions()` pass has run,
/// which skips user-placed frames on every later pass (`UIParent.lua:1692`).
pub(crate) fn load_layout(
    script: Option<NonSendMut<UiScript>>,
    data: Res<crate::account_data::AccountData>,
    mut file: ResMut<LayoutFile>,
) {
    let Some(mut script) = script else { return };
    if *file.restored.get(&script) {
        return;
    }
    let Some(bytes) = data.read(crate::account_data::store::LAYOUT) else {
        return; // waiting for the server's times
    };
    *file.restored.get(&script) = true;
    let frames = parse(&String::from_utf8_lossy(&bytes));
    if !frames.is_empty() {
        info!("layout: {} placed frames restored", frames.len());
        script.place_frames(frames);
    }
}

/// The account-data save of slot 6 with the placed-frames writer at the UI shutdown: the whole
/// file, from the live tree.
pub(crate) fn save_to_account_data(world: &mut World) {
    let Some(mut file) = world.remove_resource::<LayoutFile>() else {
        return;
    };
    let body = world
        .get_non_send_resource_mut::<UiScript>()
        .and_then(|mut script| {
            (*file.restored.get(&script)).then(|| render(&script.placed_frames()))
        });
    world.insert_resource(file);
    let Some(body) = body else {
        return;
    };
    crate::account_data::save_slots(
        world,
        &[(crate::account_data::store::LAYOUT, body.into_bytes())],
    );
}

/// The placed frames' restore; their write is the UI shutdown's ([`save_to_account_data`]).
pub(crate) struct UiLayoutPlugin;

impl Plugin for UiLayoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LayoutFile>().add_systems(
            Update,
            // In the feed phase, so the tick sees the restore the frame it lands.
            load_layout
                .in_set(crate::ui_script::UiFeed)
                .in_set(crate::char_select::InWorldGated),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, level: i32, anchor: &str, x: i32, y: i32, w: i32, h: i32) -> PlacedFrame {
        PlacedFrame {
            name: name.into(),
            level,
            anchor: anchor.into(),
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn the_file_is_the_references_and_round_trips() {
        let frames = vec![
            row("WorldMapScreenAnchor", 0, "TOPLEFT", 10, -118, 1, 1),
            row("ChatFrame1", 2, "BOTTOMLEFT", 32, 95, 430, 120),
        ];
        let text = render(&frames);
        assert!(text.starts_with(
            "Version: 1\nFrame: WorldMapScreenAnchor\nFrameLevel: 0\nAnchor: TOPLEFT\nX: 10\n\
             Y: -118\nW: 1\nH: 1\nFrame: ChatFrame1\n"
        ));
        assert_eq!(parse(&text), frames);
        assert_eq!(
            render(&[]),
            "Version: 1\n",
            "nothing placed: the 11-byte file"
        );
    }

    #[test]
    fn the_reader_takes_what_the_reference_takes() {
        let text = "version: 9\r\nX: 5\r\nframe: A\r\nanchor: CENTER\r\nx: -3junk\r\nW: \r\n\
                    Frame: B\r\nH: 40\r\n";
        assert_eq!(
            parse(text),
            [
                row("A", -1, "CENTER", -3, 0, 0, 0),
                row("B", -1, "TOPLEFT", 0, 0, 0, 40),
            ],
            "keys any case, a line outside a block ignored, defaults per block"
        );
    }
}
