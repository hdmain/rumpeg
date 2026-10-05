//! # Rumpeg
//!
//! A high-performance multimedia framework written in Rust, inspired by
//! [FFmpeg](https://ffmpeg.org/)'s library architecture.
//!
//! Rumpeg is designed **primarily as a Rust library**: link the crates you need
//! (or this umbrella crate) and drive demux → decode → filter → encode → mux
//! pipelines from idiomatic Rust.
//!
//! ## Crate map (FFmpeg analogues)
//!
//! | Rumpeg | FFmpeg |
//! |--------|--------|
//! | [`rumpeg_util`] | libavutil |
//! | [`rumpeg_codec`] | libavcodec |
//! | [`rumpeg_format`] | libavformat |
//! | [`rumpeg_scale`] | libswscale |
//! | [`rumpeg_resample`] | libswresample |
//! | [`rumpeg_filter`] | libavfilter |
//!
//! ## Quick start
//!
//! ```no_run
//! use rumpeg::prelude::*;
//!
//! fn main() -> rumpeg::Result<()> {
//!     rumpeg::init();
//!     let mut input = format::open_input("input.wav")?;
//!     let stream = input
//!         .best_stream(MediaType::Audio)
//!         .ok_or_else(|| Error::not_found("no audio stream"))?;
//!     let mut decoder = codec::open_decoder(&stream.codec_params)?;
//!
//!     while let Ok(packet) = input.read_packet() {
//!         for frame in decoder.decode(&packet)? {
//!             // process decoded Frame::Audio / Frame::Video
//!             let _ = frame;
//!         }
//!     }
//!     Ok(())
//! }
//! ```

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub use rumpeg_codec as codec;
pub use rumpeg_filter as filter;
pub use rumpeg_format as format;
pub use rumpeg_resample as resample;
pub use rumpeg_scale as scale;
pub use rumpeg_util as util;

pub use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, BufferPool, ChannelLayout, CodecId, CodecParams, Duration,
    Error, Frame, MediaType, Packet, PacketFlags, PixelFormat, Rational, Result, SampleFormat,
    Timestamp, VideoFrame, VideoParams, TIME_BASE,
};

/// Convenient prelude for application code.
pub mod prelude {
    pub use crate::codec::{self, DecoderContext, EncoderContext};
    pub use crate::filter::{self, Graph};
    pub use crate::format::{self, FormatContext, OutputContext};
    pub use crate::resample;
    pub use crate::scale::{self, FilterMode};
    pub use crate::util;
    pub use crate::{
        AudioFrame, AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, Error, Frame,
        MediaType, Packet, PixelFormat, Rational, Result, SampleFormat, Timestamp, VideoFrame,
        VideoParams,
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
