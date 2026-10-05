//! Filter graph engine — Rumpeg's `libavfilter` counterpart.
//!
//! Filters are chained into a [`Graph`] that processes [`Frame`]s.
//!
//! Supported names in a comma-separated chain (`a,b=c,d`):
//! `volume`, `aformat`, `aresample`, `format`, `scale`, `hflip`, `vflip`,
//! `crop`, `pad`, `transpose`, `fps`, `framestep`, `eq`, `hue`, `overlay`,
//! `null`, `anull`.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod filters;
pub mod graph;

pub use graph::{Filter, Graph};
use rumpeg_util::{Frame, Result};

/// Apply a simple linear filter chain described by an FFmpeg-like filter string.
pub fn process(frame: Frame, filter_spec: &str) -> Result<Frame> {
    let mut graph = Graph::parse(filter_spec)?;
    graph.run(frame)
}
