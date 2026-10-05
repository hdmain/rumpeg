//! Filter graph engine — Rumpeg's `libavfilter` counterpart.
//!
//! Filters are chained into a [`Graph`] that processes [`Frame`]s. Built-ins
//! cover volume, aformat, scale, and format conversion.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod filters;
pub mod graph;

pub use graph::{Filter, Graph};
use rumpeg_util::{Frame, Result};

/// Apply a simple linear filter chain described by an FFmpeg-like filter string.
///
/// Currently supports comma-separated filters:
/// - `volume=N` — multiply PCM by N (f32 factor)
/// - `aformat=sample_fmts=s16:sample_rates=48000` — audio format convert
/// - `format=pix_fmts=rgb24` — video pixel format convert
/// - `scale=WIDTHxHEIGHT` — video scale
pub fn process(frame: Frame, filter_spec: &str) -> Result<Frame> {
    let mut graph = Graph::parse(filter_spec)?;
    graph.run(frame)
}
