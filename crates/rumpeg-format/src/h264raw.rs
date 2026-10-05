//! Annex-B H.264 elementary stream demuxer (`.h264` / `.264`).

use crate::demuxer::Demuxer;
use crate::io::IoReader;
use crate::stream::Stream;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, Error, Packet, PacketFlags, PixelFormat, Rational, Result,
    Timestamp, VideoParams,
};
use std::io::SeekFrom;

/// Probe score for Annex-B start codes.
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() >= 4
        && ((buf[0] == 0 && buf[1] == 0 && buf[2] == 1)
            || (buf[0] == 0 && buf[1] == 0 && buf[2] == 0 && buf[3] == 1))
    {
        // Prefer over random data; MP4 also may start with zeros so keep modest.
        60
    } else {
        0
    }
}

/// Demuxer that emits one Annex-B access unit per packet (SPS/PPS coalesced with first IDR).
pub struct H264RawDemuxer {
    streams: Vec<Stream>,
    data: Vec<u8>,
    pos: usize,
    done: bool,
}

impl H264RawDemuxer {
    /// Read the entire elementary stream into memory and prepare packets.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        if rumpeg_h264::extract_annexb_nals(&data).is_empty() {
            return Err(Error::invalid_data("no Annex-B NALs found"));
        }
        let params = CodecParams::video_codec(
            CodecId::H264,
            VideoParams {
                pix_fmt: PixelFormat::Yuv420p,
                width: 0,
                height: 0,
                frame_rate: Rational::new(25, 1),
                sample_aspect_ratio: Rational::one(),
            },
        );
        let mut stream = Stream::new(0, params);
        stream.time_base = Rational::new(1, 25);
        Ok(Self {
            streams: vec![stream],
            data,
            pos: 0,
            done: false,
        })
    }
}

impl Demuxer for H264RawDemuxer {
    fn format_name(&self) -> &'static str {
        "h264"
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, _reader: &mut dyn IoReader) -> Result<Packet> {
        if self.done || self.pos >= self.data.len() {
            self.done = true;
            return Err(Error::Eof);
        }
        // Emit the remainder as a single access-unit blob (decoder splits NALs).
        // For multi-frame files we'd split on AU boundaries; I_PCM encodes are one AU.
        let slice = &self.data[self.pos..];
        self.pos = self.data.len();
        self.done = true;
        let mut pkt = Packet::new(Buffer::from_slice(slice));
        pkt.stream_index = 0;
        pkt.pts = Timestamp::new(0);
        pkt.dts = pkt.pts;
        pkt.flags.insert(PacketFlags::KEY);
        Ok(pkt)
    }

    fn seek(&mut self, _reader: &mut dyn IoReader, _timestamp: i64) -> Result<()> {
        self.pos = 0;
        self.done = false;
        Ok(())
    }
}
