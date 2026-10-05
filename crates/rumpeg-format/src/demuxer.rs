//! Demuxer trait and input format context.

use crate::io::{IoReader, MediaIo};
use crate::probe;
use crate::stream::Stream;
use crate::wav::WavDemuxer;
use rumpeg_util::{Error, MediaType, Packet, Result};
use std::io::Seek;
use std::path::Path;

/// Trait implemented by container demuxers.
pub trait Demuxer: Send {
    /// Short format name (e.g. `"wav"`).
    fn format_name(&self) -> &'static str;

    /// Streams discovered in the container.
    fn streams(&self) -> &[Stream];

    /// Read the next compressed packet.
    fn read_packet(&mut self, reader: &mut dyn IoReader) -> Result<Packet>;

    /// Seek to an approximate timestamp in the default stream's time base.
    fn seek(&mut self, reader: &mut dyn IoReader, timestamp: i64) -> Result<()>;
}

/// Opened input — owns I/O and the demuxer.
pub struct FormatContext {
    io: MediaIo,
    demuxer: Box<dyn Demuxer>,
}

impl FormatContext {
    /// Open a path, probe the format, and construct a demuxer.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let io = MediaIo::open_read(path)?;
        Self::open_io(io)
    }

    /// Open from an existing I/O handle.
    pub fn open_io(mut io: MediaIo) -> Result<Self> {
        let probed = probe::probe(&mut io)?;
        io.seek(std::io::SeekFrom::Start(0))?;
        let demuxer: Box<dyn Demuxer> = match probed.format_name {
            "wav" => Box::new(WavDemuxer::open(&mut io)?),
            other => {
                return Err(Error::unsupported(format!(
                    "no demuxer for format '{other}'"
                )));
            }
        };
        Ok(Self { io, demuxer })
    }

    /// Format short name.
    pub fn format_name(&self) -> &str {
        self.demuxer.format_name()
    }

    /// All streams.
    pub fn streams(&self) -> &[Stream] {
        self.demuxer.streams()
    }

    /// Best stream of the given media type (first match).
    pub fn best_stream(&self, media_type: MediaType) -> Option<&Stream> {
        self.streams().iter().find(|s| s.media_type == media_type)
    }

    /// Read next packet.
    pub fn read_packet(&mut self) -> Result<Packet> {
        self.demuxer.read_packet(&mut self.io)
    }

    /// Seek on the first stream.
    pub fn seek(&mut self, timestamp: i64) -> Result<()> {
        self.demuxer.seek(&mut self.io, timestamp)
    }
}
