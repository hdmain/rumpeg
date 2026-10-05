//! Pure-Rust H.264 (AVC) for Rumpeg.
//!
//! **Decode** is powered by [`rusty_h264_decoder`](https://crates.io/crates/rusty_h264-decoder)
//! (CAVLC + CABAC). Optional OpenH264 `asm` / FFI is disabled.
//!
//! **Encode** (Rumpeg-native Baseline):
//! - [`IntraMode::Ipcm`] — exact sample round-trip
//! - [`IntraMode::I16x16Cavlc`] — I_16x16 + CAVLC, with optional **P** frames
//!   (SKIP / Intra-refresh GOP) for practical bitrate reduction
//!
//! Also provides SPS/PPS helpers, Annex-B / AVCC helpers, and
//! [`BaselineIntraDecoder`] for encoder round-trip tests.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

mod bitstream;
mod cavlc;
mod decoder;
mod encoder;
mod error;
mod intra;
mod mb;
mod nal;
mod native_decoder;
mod pps;
mod slice;
mod sps;
mod tables;
mod transform;
mod yuv;

pub use decoder::Decoder;
pub use encoder::{Encoder, IntraMode};
pub use error::{Error, Result};
pub use nal::{
    annexb_to_avcc_sample, avcc_sample_to_annexb, extract_annexb_nals, nal_unit_type,
    AvcDecoderConfig, NAL_IDR, NAL_NON_IDR, NAL_PPS, NAL_SPS,
};
pub use native_decoder::BaselineIntraDecoder;
pub use yuv::Yuv420Planar;
