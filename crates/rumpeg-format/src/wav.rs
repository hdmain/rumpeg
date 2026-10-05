//! WAV (RIFF) demuxer and muxer — first-class PCM container support.

use crate::demuxer::Demuxer;
use crate::io::{read_exact_vec, IoReader, IoWriter};
use crate::muxer::Muxer;
use crate::stream::Stream;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use rumpeg_util::{
    AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, Error, Packet, PacketFlags, Rational,
    Result, SampleFormat, Timestamp,
};
use std::io::SeekFrom;

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Parsed WAV format details.
#[derive(Clone, Debug)]
pub struct WavInfo {
    /// Codec params for the single audio stream.
    pub params: CodecParams,
    /// Data chunk file offset.
    pub data_offset: u64,
    /// Data chunk size in bytes.
    pub data_size: u64,
    /// Block align (bytes per sample frame).
    pub block_align: u16,
}

/// Probe and parse a WAV header from the current position (must be start of RIFF).
pub fn parse_header(r: &mut dyn IoReader) -> Result<WavInfo> {
    let mut riff = [0u8; 4];
    r.read_exact(&mut riff)?;
    if &riff != b"RIFF" {
        return Err(Error::invalid_data("not a RIFF file"));
    }
    let _riff_size = r.read_u32::<LittleEndian>()?;
    let mut wave = [0u8; 4];
    r.read_exact(&mut wave)?;
    if &wave != b"WAVE" {
        return Err(Error::invalid_data("not a WAVE file"));
    }

    let mut fmt = None;
    let mut data_offset = None;
    let mut data_size = None;

    loop {
        let mut chunk_id = [0u8; 4];
        match r.read_exact(&mut chunk_id) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(Error::Io(e)),
        }
        let chunk_size = r.read_u32::<LittleEndian>()? as u64;
        let chunk_pos = r.stream_position()?;

        match &chunk_id {
            b"fmt " => {
                fmt = Some(parse_fmt(r, chunk_size)?);
            }
            b"data" => {
                data_offset = Some(chunk_pos);
                data_size = Some(chunk_size);
                // Don't consume data; leave cursor at start of PCM for demuxer.
                break;
            }
            _ => {
                r.seek(SeekFrom::Current(chunk_size as i64))?;
            }
        }

        // Chunks are word-aligned.
        if chunk_size % 2 == 1 {
            let _ = r.seek(SeekFrom::Current(1));
        }
    }

    let (params, block_align) = fmt.ok_or_else(|| Error::invalid_data("missing fmt chunk"))?;
    let data_offset = data_offset.ok_or_else(|| Error::invalid_data("missing data chunk"))?;
    let data_size = data_size.unwrap_or(0);

    Ok(WavInfo {
        params,
        data_offset,
        data_size,
        block_align,
    })
}

fn parse_fmt(r: &mut dyn IoReader, chunk_size: u64) -> Result<(CodecParams, u16)> {
    let format_tag = r.read_u16::<LittleEndian>()?;
    let channels = r.read_u16::<LittleEndian>()?;
    let sample_rate = r.read_u32::<LittleEndian>()?;
    let _byte_rate = r.read_u32::<LittleEndian>()?;
    let block_align = r.read_u16::<LittleEndian>()?;
    let bits_per_sample = r.read_u16::<LittleEndian>()?;

    let mut consumed = 16u64;
    let mut effective_tag = format_tag;
    if format_tag == WAVE_FORMAT_EXTENSIBLE && chunk_size >= 40 {
        let _cb = r.read_u16::<LittleEndian>()?;
        let _valid = r.read_u16::<LittleEndian>()?;
        let _mask = r.read_u32::<LittleEndian>()?;
        // First two bytes of SubFormat GUID are the actual format tag.
        effective_tag = r.read_u16::<LittleEndian>()?;
        let mut skip = [0u8; 14];
        r.read_exact(&mut skip)?;
        consumed = 40;
    }

    if consumed < chunk_size {
        r.seek(SeekFrom::Current((chunk_size - consumed) as i64))?;
    }

    let (codec_id, sample_fmt) = match (effective_tag, bits_per_sample) {
        (WAVE_FORMAT_PCM, 8) => (CodecId::PcmU8, SampleFormat::U8),
        (WAVE_FORMAT_PCM, 16) => (CodecId::PcmS16Le, SampleFormat::S16),
        (WAVE_FORMAT_PCM, 24) => (CodecId::PcmS24Le, SampleFormat::S24),
        (WAVE_FORMAT_PCM, 32) => (CodecId::PcmS32Le, SampleFormat::S32),
        (WAVE_FORMAT_IEEE_FLOAT, 32) => (CodecId::PcmF32Le, SampleFormat::F32),
        _ => {
            return Err(Error::unsupported(format!(
                "WAV format tag=0x{effective_tag:04x} bits={bits_per_sample}"
            )));
        }
    };

    let audio = AudioParams {
        sample_fmt,
        sample_rate,
        layout: ChannelLayout::new(channels),
        frame_size: 0,
    };
    Ok((CodecParams::audio_codec(codec_id, audio), block_align))
}

/// Score a buffer for WAV detection (0–100).
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() >= 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"WAVE" {
        100
    } else if buf.len() >= 4 && &buf[0..4] == b"RIFF" {
        25
    } else {
        0
    }
}

/// WAV demuxer.
pub struct WavDemuxer {
    info: WavInfo,
    streams: Vec<Stream>,
    bytes_read: u64,
    eof: bool,
}

impl WavDemuxer {
    /// Create from an already-parsed header (reader positioned at data).
    pub fn from_info(info: WavInfo) -> Self {
        let mut stream = Stream::new(0, info.params.clone());
        if let Some(audio) = info.params.audio() {
            stream.time_base = Rational::new(1, audio.sample_rate as i32);
            if info.block_align > 0 {
                let frames = info.data_size / info.block_align as u64;
                stream.duration = Some(frames as i64);
                stream.nb_frames = Some(frames);
            }
        }
        Self {
            info,
            streams: vec![stream],
            bytes_read: 0,
            eof: false,
        }
    }

    /// Open by parsing the header from `reader`.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        let info = parse_header(reader)?;
        Ok(Self::from_info(info))
    }
}

impl Demuxer for WavDemuxer {
    fn format_name(&self) -> &'static str {
        "wav"
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, reader: &mut dyn IoReader) -> Result<Packet> {
        if self.eof {
            return Err(Error::Eof);
        }
        let remaining = self.info.data_size.saturating_sub(self.bytes_read);
        if remaining == 0 {
            self.eof = true;
            return Err(Error::Eof);
        }

        // Read ~100ms worth of audio per packet for reasonable latency.
        let target_frames = self
            .info
            .params
            .audio()
            .map(|a| (a.sample_rate / 10).max(1))
            .unwrap_or(4096) as u64;
        let want = (target_frames * self.info.block_align as u64).min(remaining) as usize;
        // Align to block.
        let want = want - (want % self.info.block_align.max(1) as usize);
        if want == 0 {
            self.eof = true;
            return Err(Error::Eof);
        }

        let data = read_exact_vec(reader, want)?;
        let frames = want as u64 / self.info.block_align as u64;
        let sample_pos = self.bytes_read / self.info.block_align as u64;
        self.bytes_read += want as u64;

        let mut pkt = Packet::new(Buffer::from_vec(data));
        pkt.stream_index = 0;
        pkt.pts = Timestamp::new(sample_pos as i64);
        pkt.dts = pkt.pts;
        pkt.duration = frames as i64;
        pkt.flags.insert(PacketFlags::KEY);
        pkt.pos = (self.info.data_offset + self.bytes_read - want as u64) as i64;
        Ok(pkt)
    }

    fn seek(&mut self, reader: &mut dyn IoReader, timestamp: i64) -> Result<()> {
        let align = self.info.block_align.max(1) as u64;
        let byte = (timestamp.max(0) as u64)
            .saturating_mul(align)
            .min(self.info.data_size);
        let byte = byte - (byte % align);
        reader.seek(SeekFrom::Start(self.info.data_offset + byte))?;
        self.bytes_read = byte;
        self.eof = false;
        Ok(())
    }
}

/// WAV muxer state.
pub struct WavMuxer {
    streams: Vec<Stream>,
    data_size: u32,
    header_written: bool,
    data_size_pos: u64,
    riff_size_pos: u64,
}

impl WavMuxer {
    /// Create an empty WAV muxer.
    pub fn new() -> Self {
        Self {
            streams: Vec::new(),
            data_size: 0,
            header_written: false,
            data_size_pos: 0,
            riff_size_pos: 0,
        }
    }
}

impl Default for WavMuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Muxer for WavMuxer {
    fn format_name(&self) -> &'static str {
        "wav"
    }

    fn add_stream(&mut self, params: CodecParams) -> Result<usize> {
        if !self.streams.is_empty() {
            return Err(Error::invalid_data("WAV supports a single audio stream"));
        }
        match params.codec_id {
            CodecId::PcmS16Le
            | CodecId::PcmS24Le
            | CodecId::PcmS32Le
            | CodecId::PcmF32Le
            | CodecId::PcmU8 => {}
            other => {
                return Err(Error::unsupported(format!(
                    "WAV muxer cannot store codec {other}"
                )));
            }
        }
        let index = 0;
        let mut stream = Stream::new(index, params);
        if let Some(audio) = stream.codec_params.audio() {
            stream.time_base = Rational::new(1, audio.sample_rate as i32);
        }
        self.streams.push(stream);
        Ok(index)
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn write_header(&mut self, writer: &mut dyn IoWriter) -> Result<()> {
        let stream = self
            .streams
            .first()
            .ok_or_else(|| Error::invalid_data("no streams added"))?;
        let audio = stream
            .codec_params
            .audio()
            .ok_or_else(|| Error::invalid_data("expected audio params"))?;

        let (format_tag, bits) = match stream.codec_params.codec_id {
            CodecId::PcmU8 => (WAVE_FORMAT_PCM, 8u16),
            CodecId::PcmS16Le => (WAVE_FORMAT_PCM, 16),
            CodecId::PcmS24Le => (WAVE_FORMAT_PCM, 24),
            CodecId::PcmS32Le => (WAVE_FORMAT_PCM, 32),
            CodecId::PcmF32Le => (WAVE_FORMAT_IEEE_FLOAT, 32),
            _ => unreachable!(),
        };
        let channels = audio.layout.channels;
        let block_align = (channels as u32 * bits as u32 / 8) as u16;
        let byte_rate = audio.sample_rate * block_align as u32;

        writer.write_all(b"RIFF")?;
        self.riff_size_pos = writer.stream_position()?;
        writer.write_u32::<LittleEndian>(0)?; // placeholder
        writer.write_all(b"WAVE")?;

        writer.write_all(b"fmt ")?;
        writer.write_u32::<LittleEndian>(16)?;
        writer.write_u16::<LittleEndian>(format_tag)?;
        writer.write_u16::<LittleEndian>(channels)?;
        writer.write_u32::<LittleEndian>(audio.sample_rate)?;
        writer.write_u32::<LittleEndian>(byte_rate)?;
        writer.write_u16::<LittleEndian>(block_align)?;
        writer.write_u16::<LittleEndian>(bits)?;

        writer.write_all(b"data")?;
        self.data_size_pos = writer.stream_position()?;
        writer.write_u32::<LittleEndian>(0)?; // placeholder

        self.header_written = true;
        Ok(())
    }

    fn write_packet(&mut self, writer: &mut dyn IoWriter, packet: &Packet) -> Result<()> {
        if !self.header_written {
            return Err(Error::invalid_data("write_header must be called first"));
        }
        writer.write_all(packet.data.as_slice())?;
        self.data_size = self.data_size.saturating_add(packet.data.len() as u32);
        Ok(())
    }

    fn write_trailer(&mut self, writer: &mut dyn IoWriter) -> Result<()> {
        let end = writer.stream_position()?;
        writer.seek(SeekFrom::Start(self.data_size_pos))?;
        writer.write_u32::<LittleEndian>(self.data_size)?;
        // RIFF size = file size - 8
        let riff_size = (end as u32).saturating_sub(8);
        writer.seek(SeekFrom::Start(self.riff_size_pos))?;
        writer.write_u32::<LittleEndian>(riff_size)?;
        writer.seek(SeekFrom::Start(end))?;
        writer.flush()?;
        Ok(())
    }
}
