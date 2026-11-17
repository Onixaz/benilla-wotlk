//! Prints the Adler-32 of every frame the decoder outputs, one per line in output order, over
//! the cropped Y, U and V planes in turn: the hash `ffmpeg -f framehash -hash adler32` prints,
//! so the two decoders can be diffed frame by frame (`scripts/movie-oracle.sh`).
//!
//! `cargo run -p benilla-cinematics --example frame_hashes -- <file.avi> [frames] [dump-dir]`
//! With a dump dir, each frame is also written as raw I420 (`ffmpeg -f rawvideo` layout).

use std::io::Write;

use benilla_cinematics::{avi::Avi, Decoder, Frame};

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

fn dump(dir: &str, n: usize, frame: &Frame) {
    let mut out = Vec::new();
    for p in 0..3 {
        let (data, stride, w, h) = frame.plane(p);
        for row in data.chunks(stride).take(h) {
            out.extend_from_slice(&row[..w]);
        }
    }
    std::fs::write(format!("{dir}/{n:05}.yuv"), out).expect("write frame");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: frame_hashes <file.avi> [frames] [dump-dir]");
    let limit: usize = args
        .get(2)
        .map_or(usize::MAX, |n| n.parse().expect("frames"));
    let dir = args.get(3);
    let bytes = std::fs::read(path).expect("read file");
    let avi = Avi::parse(&bytes).expect("parse avi");
    let mut dec = Decoder::new();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut n = 0;
    let mut emit = |f: Frame, n: &mut usize| {
        if let Some(d) = dir {
            dump(d, *n, &f);
        }
        writeln!(out, "{:08x}", adler(&f)).expect("stdout");
        *n += 1;
    };
    for (i, packet) in avi.video_packets(&bytes).enumerate() {
        if n >= limit {
            return;
        }
        match dec.decode(packet) {
            Ok(Some(f)) => emit(f, &mut n),
            Ok(None) => {}
            Err(e) => {
                eprintln!("chunk {i}: {e}");
                std::process::exit(1);
            }
        }
    }
    if n < limit {
        if let Some(f) = dec.flush() {
            emit(f, &mut n);
        }
    }
}
