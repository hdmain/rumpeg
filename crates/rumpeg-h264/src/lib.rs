//! Pure-Rust H.264 / AVC helper codec for Rumpeg.
//!
//! This crate is an intentional **OpenH264 analogue written entirely in Rust**
//! (no Cisco OpenH264 C/C++ FFI). The v0.1 scope is Baseline-compatible
//! **Intra `I_PCM` frames**: each macroblock carries raw 8-bit YUV samples.
//! That is enough for reliable first-frame extract / thumbnail pipelines and
//! produces bitstreams any standard H.264 decoder can play.
//!
//! ## Features
//! - Annex-B NAL packing / splitting
//! - AVCC (`avcC`) extradata build + parse for MP4
//! - SPS / PPS encode & decode
//! - IDR slice encode & decode with `I_PCM` macroblocks
//!
//! ## Non-goals (yet)
//! Inter prediction, CAVLC/CABAC residual coding, B-frames, high profiles.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

mod bitstream;
mod decoder;
mod encoder;
mod error;
mod nal;
mod pps;
mod slice;
mod sps;
mod yuv;

pub use decoder::Decoder;
pub use encoder::Encoder;
pub use error::{Error, Result};
pub use nal::{
    annexb_to_avcc_sample, avcc_sample_to_annexb, extract_annexb_nals, nal_unit_type,
    AvcDecoderConfig, NAL_IDR, NAL_NON_IDR, NAL_PPS, NAL_SPS,
};
pub use pps::Pps;
pub use sps::Sps;
pub use yuv::Yuv420Planar;
