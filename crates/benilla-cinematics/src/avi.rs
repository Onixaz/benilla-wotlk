//! RIFF AVI demuxing, as far as the movies in `Interface\Cinematics\` need it: one `vids`
//! stream and one `auds` stream, chunks read in file order from `movi` (the `idx1` index is not
//! needed to play front to back).

use std::fmt;

/// A malformed or unsupported AVI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AviError(pub String);

impl fmt::Display for AviError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "avi: {}", self.0)
    }
}

impl std::error::Error for AviError {}

fn err<T>(msg: impl Into<String>) -> Result<T, AviError> {
    Err(AviError(msg.into()))
}

/// One `strl`: its `strh` fields and its `strf` bytes.
struct StreamHeader {
    kind: [u8; 4],
    handler: [u8; 4],
    scale: u32,
    rate: u32,
    strf: Vec<u8>,
}

/// The video stream's header fields a player needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    /// `strh.fccHandler`, e.g. `DXGM`.
    pub handler: [u8; 4],
    /// `strf.biCompression`, e.g. `DX50`.
    pub compression: [u8; 4],
    pub width: u32,
    pub height: u32,
    /// Frames per second as `rate / scale` (`strh.dwRate`, `strh.dwScale`).
    pub rate: u32,
    pub scale: u32,
}

/// The audio stream's `WAVEFORMATEX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    /// `wFormatTag`: `0x55` is MPEG layer 3.
    pub format: u16,
    pub channels: u16,
    pub sample_rate: u32,
}

/// One chunk of `movi`: a stream's packet, in file order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// The stream number from the chunk id (`00dc` is 0).
    pub stream: u8,
    /// Byte range of the payload in the file.
    pub offset: usize,
    pub len: usize,
}

/// A parsed AVI: its two streams and every `movi` chunk, borrowing nothing.
#[derive(Debug, Clone)]
pub struct Avi {
    pub video: VideoInfo,
    pub video_stream: u8,
    pub audio: Option<AudioInfo>,
    pub audio_stream: Option<u8>,
    pub chunks: Vec<Chunk>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn fourcc_at(b: &[u8], at: usize) -> Option<[u8; 4]> {
    b.get(at..at + 4)?.try_into().ok()
}

/// Walks the chunks of `b[start..end]`, handing each `(id, payload start, payload len)` to `f`.
fn walk(
    b: &[u8],
    start: usize,
    end: usize,
    f: &mut dyn FnMut([u8; 4], usize, usize) -> Result<(), AviError>,
) -> Result<(), AviError> {
    let mut at = start;
    while at + 8 <= end {
        let id = fourcc_at(b, at).expect("bounded");
        let len = u32_at(b, at + 4).expect("bounded") as usize;
        let body = at + 8;
        // A truncated last chunk ends the walk, as a player would stop reading there.
        if body + len > end {
            break;
        }
        f(id, body, len)?;
        // Chunks are word aligned: an odd length carries one pad byte.
        at = body + len + (len & 1);
    }
    Ok(())
}

/// The stream number of a `NNxx` chunk id.
fn stream_of(id: [u8; 4]) -> Option<u8> {
    let d = |c: u8| c.is_ascii_digit().then(|| c - b'0');
    Some(d(id[0])? * 10 + d(id[1])?)
}

impl Avi {
    pub fn parse(b: &[u8]) -> Result<Avi, AviError> {
        if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"AVI " {
            return err("not a RIFF AVI");
        }
        let riff_end = (8 + u32_at(b, 4).expect("bounded") as usize).min(b.len());
        let mut streams: Vec<StreamHeader> = Vec::new();
        let mut movi = None;
        walk(b, 12, riff_end, &mut |id, at, len| {
            if &id != b"LIST" || len < 4 {
                return Ok(());
            }
            match &b[at..at + 4] {
                b"hdrl" => walk(b, at + 4, at + len, &mut |id, at, len| {
                    if &id != b"LIST" || len < 4 || &b[at..at + 4] != b"strl" {
                        return Ok(());
                    }
                    let mut strh = None;
                    let mut strf = None;
                    walk(b, at + 4, at + len, &mut |id, at, len| {
                        match &id {
                            b"strh" => strh = Some((at, len)),
                            b"strf" => strf = Some(b[at..at + len].to_vec()),
                            _ => {}
                        }
                        Ok(())
                    })?;
                    let Some((h, _)) = strh.filter(|&(_, l)| l >= 28) else {
                        return err("stream without strh");
                    };
                    streams.push(StreamHeader {
                        kind: fourcc_at(b, h).expect("bounded"),
                        handler: fourcc_at(b, h + 4).expect("bounded"),
                        scale: u32_at(b, h + 20).expect("bounded"),
                        rate: u32_at(b, h + 24).expect("bounded"),
                        strf: strf.unwrap_or_default(),
                    });
                    Ok(())
                }),
                b"movi" => {
                    movi = Some((at + 4, at + len));
                    Ok(())
                }
                _ => Ok(()),
            }
        })?;

        let mut video = None;
        let mut audio = None;
        for (n, s) in streams.iter().enumerate() {
            let (n, strf) = (n as u8, &s.strf);
            match &s.kind {
                b"vids" if video.is_none() => {
                    if strf.len() < 20 {
                        return err("video strf too short");
                    }
                    let height = u32_at(strf, 8).expect("bounded") as i32;
                    video = Some((
                        n,
                        VideoInfo {
                            handler: s.handler,
                            compression: fourcc_at(strf, 16).expect("bounded"),
                            width: u32_at(strf, 4).expect("bounded"),
                            height: height.unsigned_abs(),
                            rate: s.rate,
                            scale: s.scale,
                        },
                    ));
                }
                b"auds" if audio.is_none() => {
                    if strf.len() < 8 {
                        return err("audio strf too short");
                    }
                    audio = Some((
                        n,
                        AudioInfo {
                            format: u16_at(strf, 0).expect("bounded"),
                            channels: u16_at(strf, 2).expect("bounded"),
                            sample_rate: u32_at(strf, 4).expect("bounded"),
                        },
                    ));
                }
                _ => {}
            }
        }
        let Some((video_stream, video)) = video else {
            return err("no video stream");
        };
        if video.rate == 0 || video.scale == 0 {
            return err("video stream without a frame rate");
        }
        let Some((start, end)) = movi else {
            return err("no movi list");
        };

        let mut chunks = Vec::new();
        fn collect(
            b: &[u8],
            start: usize,
            end: usize,
            out: &mut Vec<Chunk>,
        ) -> Result<(), AviError> {
            walk(b, start, end, &mut |id, at, len| {
                if &id == b"LIST" {
                    // `rec ` groups interleave chunks; their contents are chunks like any other.
                    if len >= 4 && &b[at..at + 4] == b"rec " {
                        collect(b, at + 4, at + len, out)?;
                    }
                } else if let Some(stream) = stream_of(id) {
                    out.push(Chunk {
                        stream,
                        offset: at,
                        len,
                    });
                }
                Ok(())
            })
        }
        collect(b, start, end, &mut chunks)?;

        Ok(Avi {
            video,
            video_stream,
            audio: audio.as_ref().map(|(_, a)| a.clone()),
            audio_stream: audio.map(|(n, _)| n),
            chunks,
        })
    }

    /// The video packets in decode order, empty chunks included (each is a frame slot).
    pub fn video_packets<'a>(&'a self, b: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.chunks
            .iter()
            .filter(move |c| c.stream == self.video_stream)
            .map(move |c| &b[c.offset..c.offset + c.len])
    }

    /// The audio stream's bytes, concatenated: for format `0x55` an MPEG audio elementary stream.
    pub fn audio_bytes(&self, b: &[u8]) -> Vec<u8> {
        let Some(n) = self.audio_stream else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for c in self.chunks.iter().filter(|c| c.stream == n) {
            out.extend_from_slice(&b[c.offset..c.offset + c.len]);
        }
        out
    }

    /// Seconds per video frame.
    pub fn frame_duration(&self) -> f64 {
        f64::from(self.video.scale) / f64::from(self.video.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() & 1 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = kind.to_vec();
        b.extend_from_slice(body);
        chunk(b"LIST", &b)
    }

    /// A two-stream file built by hand: streams, rates, and the `movi` chunks in order, with an
    /// odd-length chunk's pad byte and a `rec ` group.
    #[test]
    fn parses_streams_and_movi_in_order() {
        let mut strh_v = vec![0u8; 56];
        strh_v[0..4].copy_from_slice(b"vids");
        strh_v[4..8].copy_from_slice(b"DXGM");
        strh_v[20..24].copy_from_slice(&1u32.to_le_bytes());
        strh_v[24..28].copy_from_slice(&24u32.to_le_bytes());
        let mut strf_v = vec![0u8; 40];
        strf_v[4..8].copy_from_slice(&800u32.to_le_bytes());
        strf_v[8..12].copy_from_slice(&448u32.to_le_bytes());
        strf_v[16..20].copy_from_slice(b"DX50");
        let mut strh_a = vec![0u8; 56];
        strh_a[0..4].copy_from_slice(b"auds");
        let mut strf_a = vec![0u8; 18];
        strf_a[0..2].copy_from_slice(&0x55u16.to_le_bytes());
        strf_a[2..4].copy_from_slice(&2u16.to_le_bytes());
        strf_a[4..8].copy_from_slice(&48000u32.to_le_bytes());

        let hdrl = list(
            b"hdrl",
            &[
                chunk(b"avih", &[0; 56]),
                list(
                    b"strl",
                    &[chunk(b"strh", &strh_v), chunk(b"strf", &strf_v)].concat(),
                ),
                list(
                    b"strl",
                    &[chunk(b"strh", &strh_a), chunk(b"strf", &strf_a)].concat(),
                ),
            ]
            .concat(),
        );
        let movi = list(
            b"movi",
            &[
                chunk(b"00dc", &[1, 2, 3]),
                chunk(b"01wb", &[9, 9]),
                list(b"rec ", &chunk(b"00dc", &[])),
                chunk(b"JUNK", &[0; 4]),
                chunk(b"00dc", &[4]),
                chunk(b"01wb", &[8]),
            ]
            .concat(),
        );
        let body = [b"AVI ".to_vec(), hdrl, movi, chunk(b"idx1", &[])].concat();
        let file = chunk(b"RIFF", &body);

        let avi = Avi::parse(&file).unwrap();
        assert_eq!(avi.video.width, 800);
        assert_eq!(avi.video.height, 448);
        assert_eq!(&avi.video.handler, b"DXGM");
        assert_eq!((avi.video.rate, avi.video.scale), (24, 1));
        let a = avi.audio.clone().unwrap();
        assert_eq!((a.format, a.channels, a.sample_rate), (0x55, 2, 48000));
        let v: Vec<&[u8]> = avi.video_packets(&file).collect();
        assert_eq!(v, vec![&[1u8, 2, 3][..], &[][..], &[4][..]]);
        assert_eq!(avi.audio_bytes(&file), vec![9, 9, 8]);
    }

    #[test]
    fn refuses_what_is_not_an_avi() {
        assert!(Avi::parse(b"RIFF\x04\0\0\0WAVE").is_err());
        assert!(Avi::parse(b"").is_err());
    }
}
