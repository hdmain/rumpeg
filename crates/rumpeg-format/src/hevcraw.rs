//! Annex-B HEVC elementary stream demuxing and muxing.

use crate::{
    demuxer::Demuxer,
    io::{IoReader, IoWriter},
    muxer::Muxer,
    Stream,
};
use rumpeg_util::{
    Buffer, CodecId, CodecParams, Error, Packet, PacketFlags, PixelFormat, Rational, Result,
    Timestamp, VideoParams,
};
use std::io::SeekFrom;

/// Recognize HEVC parameter sets using the two-byte HEVC NAL header.
pub fn probe_score(data: &[u8]) -> u32 {
    let nals = rumpeg_h264::extract_annexb_nals(data);
    if nals.iter().any(|n| {
        n.len() >= 2 && n[0] & 0x81 == 0 && n[1] & 7 != 0 && matches!((n[0] >> 1) & 63, 32..=34)
    }) {
        80
    } else {
        0
    }
}

/// In-memory HEVC demuxer with one access unit per packet (default 25 fps).
pub struct HevcRawDemuxer {
    streams: Vec<Stream>,
    packets: Vec<Packet>,
    index: usize,
}

impl HevcRawDemuxer {
    /// Split Annex-B NALs at first-slice / access-unit delimiter boundaries.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let mut packets = Vec::new();
        let mut au = Vec::new();
        let mut has_vcl = false;
        let mut key = false;
        for nal in rumpeg_h264::extract_annexb_nals(&data) {
            if nal.len() < 2 || nal[0] & 0x80 != 0 || nal[1] & 7 == 0 {
                return Err(Error::invalid_data("invalid HEVC NAL header"));
            }
            let ty = (nal[0] >> 1) & 63;
            let vcl = ty <= 31;
            if vcl && nal.len() < 3 {
                return Err(Error::invalid_data("short HEVC slice"));
            }
            let first = vcl && nal[2] & 0x80 != 0;
            if has_vcl && (first || matches!(ty, 32..=35 | 39)) {
                packets.push(make_packet(std::mem::take(&mut au), packets.len(), key));
                has_vcl = false;
                key = false;
            }
            au.extend_from_slice(&[0, 0, 0, 1]);
            au.extend_from_slice(nal);
            has_vcl |= vcl;
            key |= matches!(ty, 16..=23);
        }
        if has_vcl {
            packets.push(make_packet(au, packets.len(), key));
        }
        if packets.is_empty() {
            return Err(Error::invalid_data("HEVC stream contains no pictures"));
        }
        let params = CodecParams::video_codec(
            CodecId::Hevc,
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
        stream.nb_frames = Some(packets.len() as u64);
        Ok(Self {
            streams: vec![stream],
            packets,
            index: 0,
        })
    }
}
fn make_packet(data: Vec<u8>, index: usize, key: bool) -> Packet {
    let mut p = Packet::new(Buffer::from_vec(data));
    p.pts = Timestamp::new(index as i64);
    p.dts = p.pts;
    if key {
        p.flags.insert(PacketFlags::KEY);
    }
    p
}
impl Demuxer for HevcRawDemuxer {
    fn format_name(&self) -> &'static str {
        "hevc"
    }
    fn streams(&self) -> &[Stream] {
        &self.streams
    }
    fn read_packet(&mut self, _: &mut dyn IoReader) -> Result<Packet> {
        let p = self.packets.get(self.index).ok_or(Error::Eof)?.clone();
        self.index += 1;
        Ok(p)
    }
    fn seek(&mut self, _: &mut dyn IoReader, timestamp: i64) -> Result<()> {
        let target = timestamp.max(0) as usize;
        self.index = self
            .packets
            .iter()
            .enumerate()
            .take(target.saturating_add(1))
            .filter(|(_, p)| p.flags.contains(PacketFlags::KEY))
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0);
        Ok(())
    }
}

/// Single-stream Annex-B HEVC muxer.
#[derive(Default)]
pub struct HevcRawMuxer {
    streams: Vec<Stream>,
}
impl Muxer for HevcRawMuxer {
    fn format_name(&self) -> &'static str {
        "hevc"
    }
    fn streams(&self) -> &[Stream] {
        &self.streams
    }
    fn add_stream(&mut self, params: CodecParams) -> Result<usize> {
        if params.codec_id != CodecId::Hevc || !self.streams.is_empty() {
            return Err(Error::unsupported("raw HEVC requires one HEVC stream"));
        }
        self.streams.push(Stream::new(0, params));
        Ok(0)
    }
    fn write_header(&mut self, w: &mut dyn IoWriter) -> Result<()> {
        let params = &self
            .streams
            .first()
            .ok_or_else(|| Error::invalid_data("no HEVC stream"))?
            .codec_params;
        if params.extradata.first() == Some(&1) {
            w.write_all(&rumpeg_util::hevc::HevcConfig::parse(&params.extradata)?.annexb)?;
        } else {
            w.write_all(&params.extradata)?;
        }
        Ok(())
    }
    fn write_packet(&mut self, w: &mut dyn IoWriter, p: &Packet) -> Result<()> {
        if p.stream_index != 0 || self.streams.is_empty() {
            return Err(Error::invalid_data("invalid HEVC stream index"));
        }
        // Encoder packets are Annex-B; MP4 packets have length prefixes.
        let data = p.data.as_slice();
        if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
            w.write_all(data)?;
        } else {
            let cfg =
                rumpeg_util::hevc::HevcConfig::parse(&self.streams[0].codec_params.extradata)?;
            w.write_all(&rumpeg_util::hevc::sample_to_annexb(data, cfg.length_size)?)?;
        }
        Ok(())
    }
    fn write_trailer(&mut self, w: &mut dyn IoWriter) -> Result<()> {
        w.flush()?;
        Ok(())
    }
}
