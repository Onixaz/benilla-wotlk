//! Print screen: the capture, the file writer and the "Screen Captured" status, which must never
//! appear in the shot it announces. The binding runs `TakeScreenshot()` (`WorldFrame.lua:46`),
//! which hides `ScreenshotStatus` and calls `Screenshot()`; the engine later answers
//! `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED`.
//!
//! Our UI quads are built at the top of `UiInput` and the binding runs at its bottom, so a capture
//! on the press frame would still show a second press's fading status; [`ask_for_captures`]
//! therefore holds each ask exactly one frame.
//!
//! The file is `CaptureScreen`'s (`0x4a8570`): `Screenshots\WoWScrnShot_<%m%d%y_%H%M%S>` in local
//! time, a run-length Targa with no alpha when `screenshotFormat` is `tga` in any case, else a
//! JPEG at `screenshotQuality` ([`jpeg_quality`]). Deviation: the JPEG carries no hidden client
//! stamp, the reference's watermark (`ImageWriterEmbedData`, `0x4a8633`).
//!
//! The reference fires `SCREENSHOT_SUCCEEDED` even when the write fails, a bug not copied:
//! [`report_captures`] answers `SCREENSHOT_FAILED` and logs the reason.
//!
//! As in the reference, a raw `Screenshot()` from Lua skips the `Hide()`. The glue screens'
//! capture keys are not built.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;

use benilla_ui::script::UiScript;

use crate::ui_script::{UiFeed, UiInput};

/// The reference's file stem.
const STEM: &str = "WoWScrnShot";

/// The file's format, from `screenshotFormat`: `tga` in any case is a Targa (`0x9edc98`), any other
/// value a JPEG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Tga,
    Jpeg(u8),
}

impl Format {
    fn from_cvars(cvars: Option<&crate::cvars::Cvars>) -> Self {
        let format = cvars
            .and_then(|c| c.get("screenshotFormat"))
            .unwrap_or("jpeg");
        if format.eq_ignore_ascii_case("tga") {
            return Self::Tga;
        }
        let quality = cvars
            .and_then(|c| c.get("screenshotQuality"))
            .unwrap_or("3");
        Self::Jpeg(jpeg_quality(quality))
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Tga => "tga",
            Self::Jpeg(_) => "jpg",
        }
    }
}

/// `ScrnSetScreenshotQuality` (`0x4a84a0`): the CVar's integer clamped unsigned to 1..10, then
/// `n * 5.5 + 45` truncated, so the default 3 is 61.
fn jpeg_quality(value: &str) -> u8 {
    let n = atoi(value) as u32;
    (n.clamp(1, 10) as f32 * 5.5 + 45.0) as u8
}

/// C `atoi`: the leading signed integer, 0 when there is none.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0)
}

/// The writer thread's answer.
enum Outcome {
    Saved(std::path::PathBuf),
    Failed(String),
}

/// The writer threads' channel and the naming clock.
#[derive(Resource)]
struct ScreenshotState {
    tx: crossbeam_channel::Sender<Outcome>,
    rx: crossbeam_channel::Receiver<Outcome>,
    /// The last name's stamp and its count; counted here, not probed on disk, because the worker
    /// may not have created the previous file yet.
    last_stamp: String,
    within_second: u32,
    /// Captures asked for on the previous frame (module doc).
    pending: u32,
}

impl Default for ScreenshotState {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            last_stamp: String::new(),
            within_second: 0,
            pending: 0,
        }
    }
}

impl ScreenshotState {
    /// `WoWScrnShot_MMDDYY_HHMMSS.<ext>` (`OsGetTimeStamp` `0x86d490`, `"%m%d%y_%H%M%S"`).
    ///
    /// A second capture in the same second gets `_2`, `_3`: the reference overwrites, so a burst
    /// leaves one file, a data loss not copied.
    fn next_name(&mut self, t: crate::ui_chat::logging::LocalTime, ext: &str) -> String {
        let stamp = format!(
            "{:02}{:02}{:02}_{:02}{:02}{:02}",
            t.month,
            t.day,
            t.year % 100,
            t.hour,
            t.min,
            t.sec
        );
        if stamp == self.last_stamp {
            self.within_second += 1;
        } else {
            self.last_stamp = stamp;
            self.within_second = 0;
        }
        match self.within_second {
            0 => format!("{STEM}_{}.{ext}", self.last_stamp),
            n => format!("{STEM}_{}_{}.{ext}", self.last_stamp, n + 1),
        }
    }
}

/// Captures each `Screenshot()` one frame after the call. Spawn last frame's asks before draining
/// this frame's: that order is the one-frame hold (module doc).
fn ask_for_captures(
    mut commands: Commands,
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<ScreenshotState>,
) {
    for _ in 0..std::mem::take(&mut state.pending) {
        commands
            .spawn(Screenshot::primary_window())
            .observe(write_capture);
    }
    if let Some(mut script) = script {
        state.pending += script.take_screenshot_asks();
    }
}

/// Names the file and hands the encode and write to a worker, off the frame.
fn write_capture(
    captured: On<ScreenshotCaptured>,
    mut state: ResMut<ScreenshotState>,
    cvars: Option<Res<crate::cvars::Cvars>>,
) {
    // No folder under a `$WOW_CAPTURE` run or without an exe directory: report it as a failure.
    let Some(dir) = crate::local_state::screenshots_dir() else {
        let _ = state
            .tx
            .send(Outcome::Failed("no client folder to write into".into()));
        return;
    };
    let format = Format::from_cvars(cvars.as_deref());
    let name = state.next_name(crate::ui_chat::logging::local_now(), format.extension());
    let path = dir.join(name);

    // Drop alpha: on an HDR target it carries brightness, not opacity.
    let dynamic = match captured.image.clone().try_into_dynamic() {
        Ok(img) => img.to_rgb8(),
        Err(e) => {
            let _ = state
                .tx
                .send(Outcome::Failed(format!("unreadable frame: {e}")));
            return;
        }
    };

    let tx = state.tx.clone();
    IoTaskPool::get()
        .spawn(async move {
            let outcome = std::fs::create_dir_all(&dir)
                .and_then(|()| encode(&dynamic, format, &path))
                .map(|()| Outcome::Saved(path.clone()))
                .unwrap_or_else(|e| Outcome::Failed(format!("{}: {e}", path.display())));
            let _ = tx.send(outcome);
        })
        .detach();
}

/// Write the frame as `format`, the file created afresh as the reference's writers do.
fn encode(rgb: &image::RgbImage, format: Format, path: &std::path::Path) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    match format {
        Format::Tga => rgb.write_with_encoder(image::codecs::tga::TgaEncoder::new(&mut out)),
        Format::Jpeg(q) => rgb.write_with_encoder(
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q),
        ),
    }
    .map_err(std::io::Error::other)?;
    std::io::Write::flush(&mut out)
}

/// Fires `SCREENSHOT_SUCCEEDED` or `SCREENSHOT_FAILED` for finished writes, always a frame after
/// the capture, so the status cannot be in the picture.
fn report_captures(script: Option<NonSendMut<UiScript>>, state: Res<ScreenshotState>) {
    let Some(mut script) = script else {
        return;
    };
    while let Ok(outcome) = state.rx.try_recv() {
        match outcome {
            Outcome::Saved(path) => {
                info!("screenshot: wrote {}", path.display());
                script.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
            }
            Outcome::Failed(why) => {
                warn!("screenshot: FAILED — {why}");
                script.fire_event("SCREENSHOT_FAILED", Vec::new());
            }
        }
    }
}

pub(crate) struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenshotState>().add_systems(
            Update,
            (
                report_captures.in_set(UiFeed),
                ask_for_captures
                    .after(UiInput)
                    .after(crate::bindings::BindingSet),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_chat::logging::LocalTime;

    fn at(year: u32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> LocalTime {
        LocalTime {
            year,
            month,
            day,
            hour,
            min,
            sec,
            milli: 0,
        }
    }

    #[test]
    fn names_follow_the_reference_and_never_collide() {
        let mut s = ScreenshotState::default();
        let t = at(2026, 8, 21, 13, 45, 7);
        assert_eq!(s.next_name(t, "jpg"), "WoWScrnShot_082126_134507.jpg");
        assert_eq!(s.next_name(t, "jpg"), "WoWScrnShot_082126_134507_2.jpg");
        assert_eq!(s.next_name(t, "tga"), "WoWScrnShot_082126_134507_3.tga");
        assert_eq!(
            s.next_name(at(2026, 8, 21, 13, 45, 8), "jpg"),
            "WoWScrnShot_082126_134508.jpg"
        );
        // Every field needs its `%02d` zero.
        assert_eq!(
            s.next_name(at(2001, 1, 2, 3, 4, 5), "jpg"),
            "WoWScrnShot_010201_030405.jpg"
        );
    }

    #[test]
    fn the_quality_is_the_references() {
        assert_eq!(jpeg_quality("3"), 61, "the default");
        assert_eq!(jpeg_quality("10"), 100);
        assert_eq!(jpeg_quality("0"), 50, "clamped up to 1");
        assert_eq!(jpeg_quality("-1"), 100, "an unsigned clamp");
        assert_eq!(jpeg_quality("junk"), 50);
        assert_eq!(Format::from_cvars(None), Format::Jpeg(61));
    }

    #[test]
    fn both_writers_produce_files_their_decoders_read() {
        let dir = std::env::temp_dir().join(format!("benilla-shot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgb =
            image::RgbImage::from_fn(8, 4, |x, y| image::Rgb([x as u8 * 30, y as u8 * 60, 7]));
        for (format, name) in [(Format::Tga, "a.tga"), (Format::Jpeg(61), "a.jpg")] {
            let path = dir.join(name);
            encode(&rgb, format, &path).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            match format {
                // Image type 10: run-length true colour; 24 bits, no alpha.
                Format::Tga => assert_eq!((bytes[2], bytes[16]), (10, 24)),
                Format::Jpeg(_) => assert_eq!(&bytes[..2], [0xff, 0xd8]),
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An ask made during frame N is spawned on frame N+1.
    #[test]
    fn an_ask_is_held_for_one_frame_before_the_shutter() {
        let mut app = App::new();
        app.init_resource::<ScreenshotState>();
        // No VM: the asks are pushed by hand.
        app.add_systems(Update, spawn_pending_only);

        // Frame N: the binding asked.
        app.world_mut().resource_mut::<ScreenshotState>().pending = 0;
        app.world_mut().resource_mut::<ScreenshotState>().pending += 1;
        let staged = app.world().resource::<ScreenshotState>().pending;
        assert_eq!(staged, 1, "the ask is staged, not yet a capture");

        // Frame N+1.
        app.update();
        assert_eq!(
            app.world().resource::<ScreenshotState>().pending,
            0,
            "the held ask is consumed on the next frame"
        );
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1,
            "and exactly one capture is requested"
        );

        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count(),
            1
        );
    }

    /// The real [`ask_for_captures`] with no VM, as at character select.
    fn spawn_pending_only(commands: Commands, state: ResMut<ScreenshotState>) {
        ask_for_captures(commands, None, state);
    }
}
