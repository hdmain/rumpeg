//! # Rumpeg
//!
//! A high-performance multimedia framework written in Rust, inspired by
//! [FFmpeg](https://ffmpeg.org/)'s library architecture.
//!
//! H.264 support is provided by the pure-Rust [`rumpeg_h264`] crate (Baseline
//! `I_PCM`) — **not** Cisco OpenH264 C/C++ FFI.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub use rumpeg_codec as codec;
pub use rumpeg_filter as filter;
pub use rumpeg_format as format;
pub use rumpeg_h264 as h264;
pub use rumpeg_resample as resample;
pub use rumpeg_scale as scale;
pub use rumpeg_util as util;

pub mod thumbnail;

pub use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, BufferPool, ChannelLayout, CodecId, CodecParams, Duration,
    Error, Frame, MediaType, Packet, PacketFlags, PixelFormat, Rational, Result, SampleFormat,
    Timestamp, VideoFrame, VideoParams, TIME_BASE,
};
pub use thumbnail::extract_first_frame_jpeg;

/// Convenient prelude for application code.
pub mod prelude {
    pub use crate::codec::{self, DecoderContext, EncoderContext};
    pub use crate::filter::{self, Graph};
    pub use crate::format::{self, FormatContext, OutputContext};
    pub use crate::resample;
    pub use crate::scale::{self, FilterMode};
    pub use crate::util;
    pub use crate::{
        extract_first_frame_jpeg, AudioFrame, AudioParams, Buffer, ChannelLayout, CodecId,
        CodecParams, Error, Frame, MediaType, Packet, PixelFormat, Rational, Result, SampleFormat,
        Timestamp, VideoFrame, VideoParams,
    };
}

/// Initialize global registries (codecs, etc.). Safe to call multiple times.
pub fn init() {
    codec::init();
}

/// Library version string.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
