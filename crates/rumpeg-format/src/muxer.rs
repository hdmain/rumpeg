//! Muxer trait and output format context.

use crate::io::{IoWriter, MediaIo};
use crate::stream::Stream;
use crate::wav::WavMuxer;
use rumpeg_util::{CodecParams, Error, Packet, Result};
use std::path::Path;

/// Trait implemented by container muxers.
pub trait Muxer: Send {
    /// Short format name.
    fn format_name(&self) -> &'static str;

    /// Add a stream; returns its index.
    fn add_stream(&mut self, params: CodecParams) -> Result<usize>;

    /// Current streams.
    fn streams(&self) -> &[Stream];

    /// Write container header.
    fn write_header(&mut self, writer: &mut dyn IoWriter) -> Result<()>;

    /// Write a compressed packet.
    fn write_packet(&mut self, writer: &mut dyn IoWriter, packet: &Packet) -> Result<()>;

    /// Finalize the file (update sizes, indexes, …).
    fn write_trailer(&mut self, writer: &mut dyn IoWriter) -> Result<()>;
}

/// Opened output — owns I/O and the muxer.
pub struct OutputContext {
    io: MediaIo,
    muxer: Box<dyn Muxer>,
    header_written: bool,
    trailer_written: bool,
}

impl OutputContext {
    /// Open an output path. Format is taken from `format_name` or the extension.
    pub fn open(path: impl AsRef<Path>, format_name: Option<&str>) -> Result<Self> {
        let path = path.as_ref();
        let name = format_name
            .map(|s| s.to_string())
            .or_else(|| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
            })
            .unwrap_or_default();
        let io = MediaIo::open_write(path)?;
        Self::open_io(io, &name)
    }

    /// Open with an existing writer.
    pub fn open_io(io: MediaIo, format_name: &str) -> Result<Self> {
        let muxer: Box<dyn Muxer> = match format_name {
            "wav" => Box::new(WavMuxer::new()),
            other => {
                return Err(Error::unsupported(format!("no muxer for format '{other}'")));
            }
        };
        Ok(Self {
            io,
            muxer,
            header_written: false,
            trailer_written: false,
        })
    }

    /// Format name.
    pub fn format_name(&self) -> &str {
        self.muxer.format_name()
    }

    /// Add an output stream.
    pub fn add_stream(&mut self, params: CodecParams) -> Result<usize> {
        self.muxer.add_stream(params)
    }

    /// Streams.
    pub fn streams(&self) -> &[Stream] {
        self.muxer.streams()
    }

    /// Write the header (call after adding streams, before packets).
    pub fn write_header(&mut self) -> Result<()> {
        self.muxer.write_header(&mut self.io)?;
        self.header_written = true;
        Ok(())
    }

    /// Write a packet.
    pub fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        if !self.header_written {
            self.write_header()?;
        }
        self.muxer.write_packet(&mut self.io, packet)
    }

    /// Write trailer / finalize.
    pub fn write_trailer(&mut self) -> Result<()> {
        if !self.trailer_written {
            self.muxer.write_trailer(&mut self.io)?;
            self.trailer_written = true;
        }
        Ok(())
    }
}

impl Drop for OutputContext {
    fn drop(&mut self) {
        if self.header_written && !self.trailer_written {
            let _ = self.muxer.write_trailer(&mut self.io);
        }
    }
}
