//! The install's movies decode to the frames ffmpeg's decoder makes of them, bit for bit
//! (`scripts/movie-oracle.sh` checks every frame of every file against ffmpeg; these digests pin
//! two of those runs where ffmpeg is absent).

use std::path::PathBuf;

use benilla_cinematics::{avi::Avi, Decoder, Frame};

/// `Data/<locale>/Interface/Cinematics/<name>`, found without regard to case.
fn movie(data: &std::path::Path, name: &str) -> Option<PathBuf> {
    let child = |dir: &std::path::Path, want: &str| -> Option<PathBuf> {
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(want))
            })
    };
    std::fs::read_dir(data).ok()?.flatten().find_map(|locale| {
        let dir = child(&locale.path(), "Interface")?;
        let dir = child(&dir, "Cinematics")?;
        child(&dir, name)
    })
}

fn adler(frame: &Frame) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for p in 0..3 {
        let (data, stride, w, h) = frame.plane(p);
        for row in data.chunks(stride).take(h) {
            for &v in &row[..w] {
                a = (a + u32::from(v)) % 65521;
                b = (b + a) % 65521;
            }
        }
    }
    (b << 16) | a
}

/// The frame count and an FNV-1a fold of the per-frame Adler-32s of a movie's first `limit`
/// output frames.
fn digest(path: &std::path::Path, limit: usize) -> (usize, u64) {
    let bytes = std::fs::read(path).unwrap();
    let avi = Avi::parse(&bytes).unwrap();
    let mut dec = Decoder::new();
    let mut frames = Vec::new();
    for packet in avi.video_packets(&bytes) {
        if frames.len() == limit {
            break;
        }
        if let Some(f) = dec.decode(packet).unwrap() {
            frames.push(adler(&f));
        }
    }
    if frames.len() < limit {
        frames.extend(dec.flush().map(|f| adler(&f)));
    }
    let d = frames.iter().fold(0xcbf2_9ce4_8422_2325u64, |d, &h| {
        (d ^ u64::from(h)).wrapping_mul(0x0100_0000_01b3)
    });
    (frames.len(), d)
}

/// The logo: I- and P-VOPs only, every frame.
#[test]
fn the_logo_decodes_as_the_reference_decoder_does() {
    let data = benilla_formats::wow_data_or_skip!();
    let Some(path) = movie(&data, "Logo_800.avi") else {
        benilla_formats::skipped("no Logo_800.avi in the install", &[data]);
        return;
    };
    assert_eq!(digest(&path, usize::MAX), (648, 0xe574_2a71_8388_e8a6));
}

/// The Lich King intro: packed B-VOPs with direct mode, the first 400 frames.
#[test]
fn packed_b_frames_decode_as_the_reference_decoder_does() {
    let data = benilla_formats::wow_data_or_skip!();
    let Some(path) = movie(&data, "WOW_Intro_LK_800.avi") else {
        benilla_formats::skipped("no WOW_Intro_LK_800.avi in the install", &[data]);
        return;
    };
    assert_eq!(digest(&path, 400), (400, 0xb8ca_a6c0_59d7_f9ff));
}
