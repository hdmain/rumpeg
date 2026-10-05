//! Core utilities for Rumpeg — the `libavutil` counterpart.
//!
//! Provides reference-counted buffers, packets, frames, media type
//! descriptors, and shared error types used across the Rumpeg stack.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod buffer;
pub mod error;
pub mod frame;
pub mod media;
pub mod packet;
pub mod rational;
pub mod time;

pub use buffer::{Buffer, BufferPool};
pub use error::{Error, Result};
pub use frame::{AudioFrame, Frame, VideoFrame};
pub use media::{
    AudioParams, ChannelLayout, CodecId, CodecParams, CodecSpecific, MediaType, PixelFormat,
    SampleFormat, VideoParams,
};
pub use packet::{Packet, PacketFlags};
pub use rational::Rational;
pub use time::{Duration, Timestamp, TIME_BASE};
