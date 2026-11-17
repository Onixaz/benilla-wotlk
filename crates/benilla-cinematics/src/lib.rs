//! The 3.3.5a pre-rendered movies: the RIFF AVIs under `Data\<locale>\Interface\Cinematics\`.
//!
//! Each holds one DivX 5.03 video stream (MPEG-4 Part 2, Advanced Simple Profile) and one MP3
//! stream. [`avi`] splits the file into its streams; [`Decoder`] turns the video chunks into
//! frames; [`captions`] reads the `.sbt` caption files; the MP3 bytes go to the client's mixer as
//! they are. The decoder builds the subset these twelve files use (I/P/B-VOPs, H.263
//! quantisation, half-pel motion, DivX packed B-frames) and refuses a stream that needs more.

pub mod avi;
mod bits;
pub mod captions;
mod coef;
mod decoder;
mod idct;
mod mc;
mod picture;
mod tables;

pub use decoder::{DecodeError, Decoder};
pub use picture::Frame;
