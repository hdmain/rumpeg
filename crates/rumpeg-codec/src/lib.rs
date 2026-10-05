//! Codec framework — Rumpeg's `libavcodec` counterpart.
//!
//! Provides decoder/encoder traits, a codec registry, and built-in PCM /
//! rawvideo / H.264 / JPEG codecs. The send/receive API mirrors FFmpeg's
//! `avcodec_send_packet` / `avcodec_receive_frame` model.
//!
//! H.264 is provided by the pure-Rust [`rumpeg_h264`] crate (Baseline `I_PCM`),
//! not Cisco OpenH264 FFI.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod decoder;
pub mod encoder;
pub mod h264;
pub mod jpeg;
pub mod pcm;
pub mod rawvideo;
pub mod registry;

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
