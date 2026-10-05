//! Single-image JPEG muxer (`image2` / `.jpg`).

use crate::muxer::Muxer;
use crate::stream::Stream;
use rumpeg_util::{CodecId, CodecParams, Error, Packet, Result};

/// Writes one JPEG bitstream packet to the output file.
pub struct Image2Muxer {
    streams: Vec<Stream>,
    wrote: bool,
}

impl Image2Muxer {
    /// Create an empty image2 muxer.
    pub fn new() -> Self {
        Self {
            streams: Vec::new(),
            wrote: false,
        }
    }
}

impl Default for Image2Muxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Muxer for Image2Muxer {
    fn format_name(&self) -> &'static str {
        "image2"
    }

    fn add_stream(&mut self, params: CodecParams) -> Result<usize> {
        if !self.streams.is_empty() {
            return Err(Error::invalid_data("image2 supports a single stream"));
        }
        if params.codec_id != CodecId::Mjpeg {
            return Err(Error::unsupported(
                "image2 muxer expects mjpeg/JPEG packets",
            ));
        }
        self.streams.push(Stream::new(0, params));
        Ok(0)
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn write_header(&mut self, _writer: &mut dyn crate::io::IoWriter) -> Result<()> {
        Ok(())
    }

    fn write_packet(
        &mut self,
        writer: &mut dyn crate::io::IoWriter,
        packet: &Packet,
    ) -> Result<()> {
        if self.wrote {
            return Err(Error::invalid_data(
                "image2 muxer writes a single JPEG frame",
            ));
        }
        writer.write_all(packet.data.as_slice())?;
        writer.flush()?;
        self.wrote = true;
        Ok(())
    }

    fn write_trailer(&mut self, _writer: &mut dyn crate::io::IoWriter) -> Result<()> {
        if !self.wrote {
            return Err(Error::invalid_data("no JPEG packet written"));
        }
        Ok(())
    }
}
