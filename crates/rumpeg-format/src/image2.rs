//! Single-image demuxer/muxer (`image2` / `.jpg` / `.png`).

use crate::demuxer::Demuxer;
use crate::io::IoReader;
use crate::muxer::Muxer;
use crate::stream::Stream;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, Error, Packet, PixelFormat, Rational, Result, Timestamp,
    VideoParams,
};
use std::io::SeekFrom;

/// Probe score for JPEG / PNG still images.
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() >= 2 && buf[0..2] == [0xFF, 0xD8] {
        return 85;
    }
    if buf.len() >= 8 && buf[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return 85;
    }
    0
}

/// Single-frame image demuxer.
pub struct Image2Demuxer {
    streams: Vec<Stream>,
    data: Vec<u8>,
    read: bool,
}

impl Image2Demuxer {
    /// Detect codec and load the full image bitstream.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let codec = detect_image_codec(&data)?;
        let params = CodecParams::video_codec(
            codec,
            VideoParams {
                pix_fmt: PixelFormat::Rgb24,
                width: 0,
                height: 0,
                frame_rate: Rational::one(),
                sample_aspect_ratio: Rational::one(),
            },
        );
        Ok(Self {
            streams: vec![Stream::new(0, params)],
            data,
            read: false,
        })
    }
}

impl Demuxer for Image2Demuxer {
    fn format_name(&self) -> &'static str {
        "image2"
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, _reader: &mut dyn IoReader) -> Result<Packet> {
        if self.read {
            return Err(Error::Eof);
        }
        self.read = true;
        let mut pkt = Packet::new(Buffer::from_vec(self.data.clone()));
        pkt.stream_index = 0;
        pkt.pts = Timestamp::new(0);
        pkt.dts = pkt.pts;
        Ok(pkt)
    }

    fn seek(&mut self, _reader: &mut dyn IoReader, _timestamp: i64) -> Result<()> {
        self.read = false;
        Ok(())
    }
}

fn detect_image_codec(data: &[u8]) -> Result<CodecId> {
    if data.len() >= 2 && data[0..2] == [0xFF, 0xD8] {
        return Ok(CodecId::Mjpeg);
    }
    if data.len() >= 8 && data[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Ok(CodecId::Png);
    }
    Err(Error::invalid_data("image2: not JPEG or PNG"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn probe_jpeg_png() {
        assert_eq!(probe_score(&[0xFF, 0xD8, 0xFF]), 85);
        assert_eq!(
            probe_score(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            85
        );
    }

    #[test]
    fn demux_single_jpeg_packet() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        let mut io = Cursor::new(jpeg.clone());
        let mut demux = Image2Demuxer::open(&mut io).unwrap();
        let pkt = demux.read_packet(&mut io).unwrap();
        assert_eq!(pkt.data.as_slice(), jpeg.as_slice());
        assert!(demux.read_packet(&mut io).is_err());
    }
}

/// Writes one still-image bitstream packet to the output file.
pub struct Image2Muxer {
    streams: Vec<Stream>,
    wrote: bool,
    expected: CodecId,
}

impl Image2Muxer {
    /// Create a JPEG image2 muxer.
    pub fn new() -> Self {
        Self::with_codec(CodecId::Mjpeg)
    }

    /// Create an image2 muxer for a specific still-image codec.
    pub fn with_codec(codec: CodecId) -> Self {
        Self {
            streams: Vec::new(),
            wrote: false,
            expected: codec,
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
        if params.codec_id != self.expected {
            return Err(Error::unsupported(format!(
                "image2 muxer expects {:?} packets",
                self.expected
            )));
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
                "image2 muxer writes a single image frame",
            ));
        }
        writer.write_all(packet.data.as_slice())?;
        writer.flush()?;
        self.wrote = true;
        Ok(())
    }

    fn write_trailer(&mut self, _writer: &mut dyn crate::io::IoWriter) -> Result<()> {
        if !self.wrote {
            return Err(Error::invalid_data("no image packet written"));
        }
        Ok(())
    }
}
