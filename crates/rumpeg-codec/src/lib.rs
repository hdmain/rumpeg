//! Codec framework — Rumpeg's `libavcodec` counterpart.
//!
//! Provides decoder/encoder traits, a codec registry, and built-in PCM /
//! rawvideo / H.264 / HEVC / VP9 / FLAC / Opus / AAC / MP3 / JPEG / PNG codecs.
//! The send/receive API mirrors FFmpeg's `avcodec_send_packet` /
//! `avcodec_receive_frame` model.
//!
//! Default H.264 encode uses pure-Rust `rusty_h264-encoder` (ME / CABAC / ABR).
//! Optional Cargo features: `encode-x264`, `encode-nvenc`.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod aac;
pub mod decoder;
pub mod encoder;
pub mod flac;
pub mod h264;
pub mod hevc;
pub mod jpeg;
pub mod jpeg_dec;
pub mod mp3;
pub mod opus;
pub mod pcm;
pub mod png;
pub mod png_dec;
pub mod rawvideo;
pub mod registry;
pub mod vp9;

pub use decoder::{Decoder, DecoderContext};
pub use encoder::{Encoder, EncoderContext};
pub use registry::{CodecDescriptor, CodecKind, Registry};
use rumpeg_util::Result;

/// Initialize built-in codecs into the global registry.
pub fn init() {
    registry::global().register_builtins();
}

/// Find and open a decoder for the given codec parameters.
pub fn open_decoder(params: &rumpeg_util::CodecParams) -> Result<DecoderContext> {
    DecoderContext::open(params)
}

/// Find and open an encoder for the given codec parameters.
pub fn open_encoder(params: &rumpeg_util::CodecParams) -> Result<EncoderContext> {
    EncoderContext::open(params)
}
