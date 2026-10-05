//! Container format layer — Rumpeg's `libavformat` counterpart.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod demuxer;
pub mod h264raw;
pub mod image2;
pub mod io;
pub mod mp4;
pub mod muxer;
pub mod probe;
pub mod stream;
pub mod wav;

pub use demuxer::{Demuxer, FormatContext};
pub use io::{IoReader, IoWriter, MediaIo};
pub use muxer::{Muxer, OutputContext};
pub use probe::{probe, ProbeResult};
use rumpeg_util::Result;
pub use stream::Stream;

/// Open an input for demuxing (path or URL-like string).
pub fn open_input(path: impl AsRef<std::path::Path>) -> Result<FormatContext> {
    FormatContext::open(path)
}

/// Open an output for muxing. Format is inferred from the path extension when
/// `format_name` is `None`.
pub fn open_output(
    path: impl AsRef<std::path::Path>,
    format_name: Option<&str>,
) -> Result<OutputContext> {
    OutputContext::open(path, format_name)
}
