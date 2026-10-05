//! Minimal ISOBMFF / MP4 demuxer + single-frame muxer for `avc1` / H.264.

use crate::demuxer::Demuxer;
use crate::io::{IoReader, IoWriter};
use crate::muxer::Muxer;
use crate::stream::Stream;
use byteorder::{BigEndian, ReadBytesExt};
use rumpeg_util::{
    AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, Error, Packet, PacketFlags,
    PixelFormat, Rational, Result, SampleFormat, Timestamp, VideoParams,
};
use std::io::{Cursor, Read, Seek, SeekFrom};

/// Probe score for MP4 (`ftyp`).
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() >= 8 && &buf[4..8] == b"ftyp" {
        100
    } else {
        0
    }
}

#[derive(Clone, Debug)]
struct SampleEntry {
    stream_index: usize,
    offset: u64,
    size: u32,
    cts: i64,
    is_key: bool,
}

/// MP4 demuxer (`avc1` video and optional `mp4a` AAC).
pub struct Mp4Demuxer {
    streams: Vec<Stream>,
    samples: Vec<SampleEntry>,
    index: usize,
}

impl Mp4Demuxer {
    /// Parse `moov` / sample tables from the start of the file.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut file = Vec::new();
        reader.read_to_end(&mut file)?;
        let parsed = parse_mp4(&file)?;
        Ok(Self {
            streams: parsed.streams,
            samples: parsed.samples,
            index: 0,
        })
    }
}

impl Demuxer for Mp4Demuxer {
    fn format_name(&self) -> &'static str {
        "mp4"
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, reader: &mut dyn IoReader) -> Result<Packet> {
        let sample = self.samples.get(self.index).ok_or(Error::Eof)?.clone();
        reader.seek(SeekFrom::Start(sample.offset))?;
        let mut buf = vec![0u8; sample.size as usize];
        reader.read_exact(&mut buf)?;
        let mut pkt = Packet::new(Buffer::from_vec(buf));
        pkt.stream_index = sample.stream_index;
        pkt.pts = Timestamp::new(sample.cts);
        pkt.dts = pkt.pts;
        if sample.is_key {
            pkt.flags.insert(PacketFlags::KEY);
        }
        self.index += 1;
        Ok(pkt)
    }

    fn seek(&mut self, _reader: &mut dyn IoReader, timestamp: i64) -> Result<()> {
        self.index = self
            .samples
            .iter()
            .position(|s| s.cts >= timestamp)
            .unwrap_or(0);
        Ok(())
    }
}

struct ParsedMp4 {
    streams: Vec<Stream>,
    samples: Vec<SampleEntry>,
}

#[derive(Clone, Debug)]
enum TrackKind {
    Video {
        width: u32,
        height: u32,
        avcc: Vec<u8>,
        #[allow(dead_code)]
        length_size: usize,
    },
    Audio {
        sample_rate: u32,
        channels: u16,
        extradata: Vec<u8>,
    },
}

#[derive(Clone, Debug)]
struct TrackInfo {
    kind: TrackKind,
    timescale: u32,
    samples: Vec<SampleEntry>,
}

fn parse_mp4(data: &[u8]) -> Result<ParsedMp4> {
    let mut cursor = Cursor::new(data);
    let mut moov = None;
    while (cursor.position() as usize) + 8 <= data.len() {
        let start = cursor.position();
        let size = cursor.read_u32::<BigEndian>()? as u64;
        let mut typ = [0u8; 4];
        cursor.read_exact(&mut typ)?;
        let header = 8u64;
        let total = if size == 1 {
            cursor.read_u64::<BigEndian>()?
        } else if size == 0 {
            data.len() as u64 - start
        } else {
            size
        };
        if total < header {
            return Err(Error::invalid_data("bad box size"));
        }
        let payload_end = start + total;
        if typ == *b"moov" {
            let payload = &data[start as usize + header as usize..payload_end as usize];
            moov = Some(payload.to_vec());
        }
        cursor.seek(SeekFrom::Start(payload_end))?;
    }
    let moov = moov.ok_or_else(|| Error::invalid_data("MP4 missing moov"))?;
    parse_moov(&moov, data)
}

fn parse_moov(moov: &[u8], file: &[u8]) -> Result<ParsedMp4> {
    let mut timescale = 1000u32;
    let mut tracks = Vec::new();
    let mut i = 0;
    while i + 8 <= moov.len() {
        let size = u32::from_be_bytes(moov[i..i + 4].try_into().unwrap()) as usize;
        let typ = &moov[i + 4..i + 8];
        if size < 8 || i + size > moov.len() {
            break;
        }
        let payload = &moov[i + 8..i + size];
        if typ == b"mvhd" {
            timescale = parse_mvhd_timescale(payload)?;
        } else if typ == b"trak" {
            if let Ok(t) = parse_trak(payload, file) {
                tracks.push(t);
            }
        }
        i += size;
    }
    if tracks.is_empty() {
        return Err(Error::not_found("no supported tracks in MP4"));
    }

    let mut streams = Vec::new();
    let mut samples = Vec::new();
    for track in tracks {
        let stream_index = streams.len();
        let tb = Rational::new(
            1,
            if track.timescale != 0 {
                track.timescale
            } else {
                timescale
            }
            .max(1) as i32,
        );
        let stream = track_to_stream(stream_index, &track, tb);
        streams.push(stream);
        for mut sample in track.samples {
            sample.stream_index = stream_index;
            samples.push(sample);
        }
    }
    samples.sort_by_key(|s| s.offset);
    Ok(ParsedMp4 { streams, samples })
}

fn track_to_stream(index: usize, track: &TrackInfo, time_base: Rational) -> Stream {
    let mut stream = match &track.kind {
        TrackKind::Video {
            width,
            height,
            avcc,
            ..
        } => {
            let frame_rate = estimate_frame_rate(track);
            let mut params = CodecParams::video_codec(
                CodecId::H264,
                VideoParams {
                    pix_fmt: PixelFormat::Yuv420p,
                    width: *width,
                    height: *height,
                    frame_rate,
                    sample_aspect_ratio: Rational::one(),
                },
            );
            params.extradata = avcc.clone();
            Stream::new(index, params)
        }
        TrackKind::Audio {
            sample_rate,
            channels,
            extradata,
        } => {
            let mut params = CodecParams::audio_codec(
                CodecId::Aac,
                AudioParams {
                    sample_fmt: SampleFormat::S16,
                    sample_rate: *sample_rate,
                    layout: ChannelLayout::new(*channels),
                    frame_size: 0,
                },
            );
            params.extradata = extradata.clone();
            Stream::new(index, params)
        }
    };
    stream.time_base = time_base;
    stream.nb_frames = Some(track.samples.len() as u64);
    if !track.samples.is_empty() {
        // Media duration ≈ last sample CTS + one average delta.
        let first = track.samples.first().unwrap().cts;
        let last = track.samples.last().unwrap().cts;
        let n = track.samples.len() as i64;
        let avg_delta = if n > 1 {
            ((last - first) / (n - 1)).max(1)
        } else {
            1
        };
        stream.duration = Some(last + avg_delta);
    }
    stream
}

/// Estimate fps from media timescale and average sample duration (from CTS spacing).
fn estimate_frame_rate(track: &TrackInfo) -> Rational {
    if track.timescale == 0 || track.samples.len() < 2 {
        return Rational::new(25, 1);
    }
    let last = track.samples.last().unwrap().cts;
    let first = track.samples.first().unwrap().cts;
    let span = (last - first).max(1) as u64;
    let n = (track.samples.len() - 1) as u64;
    // fps ≈ n * timescale / span
    let num = (n * u64::from(track.timescale)) as i32;
    let den = span as i32;
    if den <= 0 || num <= 0 {
        Rational::new(25, 1)
    } else {
        Rational::new(num, den).reduce()
    }
}

fn parse_mvhd_timescale(payload: &[u8]) -> Result<u32> {
    if payload.is_empty() {
        return Err(Error::invalid_data("empty mvhd"));
    }
    let version = payload[0];
    if version == 1 {
        if payload.len() < 28 {
            return Err(Error::invalid_data("mvhd v1"));
        }
        Ok(u32::from_be_bytes(payload[20..24].try_into().unwrap()))
    } else {
        if payload.len() < 20 {
            return Err(Error::invalid_data("mvhd v0"));
        }
        Ok(u32::from_be_bytes(payload[12..16].try_into().unwrap()))
    }
}

fn parse_trak(trak: &[u8], file: &[u8]) -> Result<TrackInfo> {
    let mut mdia = None;
    let mut i = 0;
    while i + 8 <= trak.len() {
        let size = u32::from_be_bytes(trak[i..i + 4].try_into().unwrap()) as usize;
        let typ = &trak[i + 4..i + 8];
        if size < 8 || i + size > trak.len() {
            break;
        }
        if typ == b"mdia" {
            mdia = Some(&trak[i + 8..i + size]);
        }
        i += size;
    }
    let mdia = mdia.ok_or_else(|| Error::invalid_data("trak without mdia"))?;
    parse_mdia(mdia, file)
}

fn parse_mdia(mdia: &[u8], file: &[u8]) -> Result<TrackInfo> {
    let mut timescale = 0u32;
    let mut hdlr = None;
    let mut minf = None;
    let mut i = 0;
    while i + 8 <= mdia.len() {
        let size = u32::from_be_bytes(mdia[i..i + 4].try_into().unwrap()) as usize;
        let typ = &mdia[i + 4..i + 8];
        if size < 8 || i + size > mdia.len() {
            break;
        }
        let payload = &mdia[i + 8..i + size];
        match typ {
            b"mdhd" => timescale = parse_mdhd_timescale(payload)?,
            b"hdlr" => hdlr = Some(payload),
            b"minf" => minf = Some(payload),
            _ => {}
        }
        i += size;
    }
    let hdlr = hdlr.ok_or_else(|| Error::invalid_data("missing hdlr"))?;
    if hdlr.len() < 12 {
        return Err(Error::invalid_data("hdlr too short"));
    }
    let handler = &hdlr[8..12];
    let minf = minf.ok_or_else(|| Error::invalid_data("missing minf"))?;
    let mut stbl = None;
    let mut j = 0;
    while j + 8 <= minf.len() {
        let size = u32::from_be_bytes(minf[j..j + 4].try_into().unwrap()) as usize;
        let typ = &minf[j + 4..j + 8];
        if size < 8 || j + size > minf.len() {
            break;
        }
        if typ == b"stbl" {
            stbl = Some(&minf[j + 8..j + size]);
        }
        j += size;
    }
    let stbl = stbl.ok_or_else(|| Error::invalid_data("missing stbl"))?;
    if handler == b"vide" {
        parse_stbl_video(stbl, file, timescale)
    } else if handler == b"soun" {
        parse_stbl_audio(stbl, file, timescale)
    } else {
        Err(Error::unsupported(format!(
            "MP4 track handler {}",
            String::from_utf8_lossy(handler)
        )))
    }
}

fn parse_mdhd_timescale(payload: &[u8]) -> Result<u32> {
    if payload.is_empty() {
        return Err(Error::invalid_data("empty mdhd"));
    }
    if payload[0] == 1 {
        if payload.len() < 28 {
            return Err(Error::invalid_data("mdhd v1"));
        }
        Ok(u32::from_be_bytes(payload[20..24].try_into().unwrap()))
    } else {
        if payload.len() < 20 {
            return Err(Error::invalid_data("mdhd v0"));
        }
        Ok(u32::from_be_bytes(payload[12..16].try_into().unwrap()))
    }
}

fn parse_stbl_video(stbl: &[u8], file: &[u8], timescale: u32) -> Result<TrackInfo> {
    let mut stsd = None;
    let mut stts = None;
    let mut stsc = None;
    let mut stsz = None;
    let mut stco = None;
    let mut co64 = None;
    let mut stss = None;
    let mut i = 0;
    while i + 8 <= stbl.len() {
        let size = u32::from_be_bytes(stbl[i..i + 4].try_into().unwrap()) as usize;
        let typ = &stbl[i + 4..i + 8];
        if size < 8 || i + size > stbl.len() {
            break;
        }
        let payload = &stbl[i + 8..i + size];
        match typ {
            b"stsd" => stsd = Some(payload),
            b"stts" => stts = Some(payload),
            b"stsc" => stsc = Some(payload),
            b"stsz" | b"stz2" => stsz = Some(payload),
            b"stco" => stco = Some(payload),
            b"co64" => co64 = Some(payload),
            b"stss" => stss = Some(payload),
            _ => {}
        }
        i += size;
    }
    let stsd = stsd.ok_or_else(|| Error::invalid_data("missing stsd"))?;
    let (width, height, avcc, length_size) = parse_stsd_avc1(stsd)?;
    let sizes = parse_stsz(stsz.ok_or_else(|| Error::invalid_data("missing stsz"))?)?;
    let chunk_offsets = if let Some(stco) = stco {
        parse_stco(stco)?
    } else if let Some(co64) = co64 {
        parse_co64(co64)?
    } else {
        return Err(Error::invalid_data("missing stco/co64"));
    };
    let stsc = parse_stsc(stsc.ok_or_else(|| Error::invalid_data("missing stsc"))?)?;
    let stts = parse_stts(stts.ok_or_else(|| Error::invalid_data("missing stts"))?)?;
    let keyframes = parse_stss(stss);

    let sample_offsets = expand_sample_offsets(&chunk_offsets, &stsc, &sizes)?;
    if sample_offsets.len() != sizes.len() {
        return Err(Error::invalid_data("sample table size mismatch"));
    }
    let mut cts = 0i64;
    let mut samples = Vec::with_capacity(sizes.len());
    let mut stts_iter = expand_stts(&stts);
    for (idx, (&offset, &size)) in sample_offsets.iter().zip(sizes.iter()).enumerate() {
        let dur = stts_iter.next().unwrap_or(1);
        let is_key = keyframes
            .as_ref()
            .map(|k| k.contains(&(idx as u32 + 1)))
            .unwrap_or(idx == 0);
        samples.push(SampleEntry {
            stream_index: 0,
            offset,
            size,
            cts,
            is_key,
        });
        cts += i64::from(dur);
    }
    // Ensure file offsets valid
    for s in &samples {
        let end = s.offset as usize + s.size as usize;
        if end > file.len() {
            return Err(Error::invalid_data("sample extends past EOF"));
        }
    }
    Ok(TrackInfo {
        kind: TrackKind::Video {
            width,
            height,
            avcc,
            length_size,
        },
        timescale,
        samples,
    })
}

fn parse_stbl_audio(stbl: &[u8], file: &[u8], timescale: u32) -> Result<TrackInfo> {
    let mut stsd = None;
    let mut stts = None;
    let mut stsc = None;
    let mut stsz = None;
    let mut stco = None;
    let mut co64 = None;
    let mut i = 0;
    while i + 8 <= stbl.len() {
        let size = u32::from_be_bytes(stbl[i..i + 4].try_into().unwrap()) as usize;
        let typ = &stbl[i + 4..i + 8];
        if size < 8 || i + size > stbl.len() {
            break;
        }
        let payload = &stbl[i + 8..i + size];
        match typ {
            b"stsd" => stsd = Some(payload),
            b"stts" => stts = Some(payload),
            b"stsc" => stsc = Some(payload),
            b"stsz" | b"stz2" => stsz = Some(payload),
            b"stco" => stco = Some(payload),
            b"co64" => co64 = Some(payload),
            _ => {}
        }
        i += size;
    }
    let stsd = stsd.ok_or_else(|| Error::invalid_data("missing stsd"))?;
    let (sample_rate, channels, extradata) = parse_stsd_mp4a(stsd)?;
    let sizes = parse_stsz(stsz.ok_or_else(|| Error::invalid_data("missing stsz"))?)?;
    let chunk_offsets = if let Some(stco) = stco {
        parse_stco(stco)?
    } else if let Some(co64) = co64 {
        parse_co64(co64)?
    } else {
        return Err(Error::invalid_data("missing stco/co64"));
    };
    let stsc = parse_stsc(stsc.ok_or_else(|| Error::invalid_data("missing stsc"))?)?;
    let stts = parse_stts(stts.ok_or_else(|| Error::invalid_data("missing stts"))?)?;

    let sample_offsets = expand_sample_offsets(&chunk_offsets, &stsc, &sizes)?;
    if sample_offsets.len() != sizes.len() {
        return Err(Error::invalid_data("sample table size mismatch"));
    }
    let mut cts = 0i64;
    let mut samples = Vec::with_capacity(sizes.len());
    let mut stts_iter = expand_stts(&stts);
    for (idx, (&offset, &size)) in sample_offsets.iter().zip(sizes.iter()).enumerate() {
        let dur = stts_iter.next().unwrap_or(1);
        samples.push(SampleEntry {
            stream_index: 0,
            offset,
            size,
            cts,
            is_key: idx == 0,
        });
        cts += i64::from(dur);
    }
    for s in &samples {
        let end = s.offset as usize + s.size as usize;
        if end > file.len() {
            return Err(Error::invalid_data("sample extends past EOF"));
        }
    }
    Ok(TrackInfo {
        kind: TrackKind::Audio {
            sample_rate,
            channels,
            extradata,
        },
        timescale,
        samples,
    })
}

fn parse_stsd_mp4a(stsd: &[u8]) -> Result<(u32, u16, Vec<u8>)> {
    if stsd.len() < 8 {
        return Err(Error::invalid_data("stsd"));
    }
    let entry_count = u32::from_be_bytes(stsd[4..8].try_into().unwrap());
    if entry_count == 0 {
        return Err(Error::invalid_data("empty stsd"));
    }
    let i = 8;
    let size = u32::from_be_bytes(stsd[i..i + 4].try_into().unwrap()) as usize;
    let typ = &stsd[i + 4..i + 8];
    if typ != b"mp4a" {
        return Err(Error::unsupported(format!(
            "audio sample entry {} (only mp4a)",
            String::from_utf8_lossy(typ)
        )));
    }
    if size < 36 || i + size > stsd.len() {
        return Err(Error::invalid_data("bad mp4a size"));
    }
    let entry = &stsd[i..i + size];
    let channels = u16::from_be_bytes(entry[16..18].try_into().unwrap());
    let sample_rate = u32::from_be_bytes([entry[24], entry[25], entry[26], entry[27]]);

    let mut j = 36;
    while j + 8 <= entry.len() {
        let bsize = u32::from_be_bytes(entry[j..j + 4].try_into().unwrap()) as usize;
        let btyp = &entry[j + 4..j + 8];
        if bsize < 8 || j + bsize > entry.len() {
            break;
        }
        if btyp == b"esds" {
            let esds = &entry[j + 8..j + bsize];
            let asc = parse_esds_audio_specific_config(esds)?;
            return Ok((sample_rate, channels.max(1), asc));
        }
        j += bsize;
    }
    Ok((sample_rate, channels.max(1), Vec::new()))
}

fn parse_esds_audio_specific_config(esds: &[u8]) -> Result<Vec<u8>> {
    let mut i = 0usize;
    while i + 2 < esds.len() {
        let tag = esds[i];
        i += 1;
        let mut len = 0usize;
        loop {
            if i >= esds.len() {
                return Err(Error::invalid_data("truncated esds"));
            }
            let b = esds[i];
            i += 1;
            len = (len << 7) | (b & 0x7F) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        if i + len > esds.len() {
            return Err(Error::invalid_data("bad esds length"));
        }
        if tag == 0x05 {
            return Ok(esds[i..i + len].to_vec());
        }
        i += len;
    }
    Err(Error::invalid_data("esds missing ASC"))
}

fn parse_stsd_avc1(stsd: &[u8]) -> Result<(u32, u32, Vec<u8>, usize)> {
    // version(1)+flags(3)+entry_count(4)
    if stsd.len() < 8 {
        return Err(Error::invalid_data("stsd"));
    }
    let entry_count = u32::from_be_bytes(stsd[4..8].try_into().unwrap());
    if entry_count == 0 {
        return Err(Error::invalid_data("empty stsd"));
    }
    let i = 8;
    let size = u32::from_be_bytes(stsd[i..i + 4].try_into().unwrap()) as usize;
    let typ = &stsd[i + 4..i + 8];
    if typ != b"avc1" && typ != b"avc3" {
        return Err(Error::unsupported(format!(
            "sample entry {} (only avc1)",
            String::from_utf8_lossy(typ)
        )));
    }
    if size < 86 || i + size > stsd.len() {
        return Err(Error::invalid_data("bad avc1 size"));
    }
    let entry = &stsd[i..i + size];
    // VisualSampleEntry: 6 reserved + data_ref(2) + pre_defined/reserved + width/height at offset 32 from entry start?
    // entry layout: size(4)+type(4)+reserved(6)+data_ref_index(2)+pre_defined(2)+reserved(2)+pre_defined(3*4)+width(2)+height(2)...
    // From start of sample entry (including size/type): width at byte 32, height at 34.
    let width = u16::from_be_bytes(entry[32..34].try_into().unwrap()) as u32;
    let height = u16::from_be_bytes(entry[34..36].try_into().unwrap()) as u32;
    // Find avcC box inside remaining bytes after VisualSampleEntry fixed part (78 bytes from type? )
    // Fixed VisualSampleEntry = 8 (size+type) + 78 = 86 bytes before extensions.
    let mut j = 86;
    while j + 8 <= entry.len() {
        let bsize = u32::from_be_bytes(entry[j..j + 4].try_into().unwrap()) as usize;
        let btyp = &entry[j + 4..j + 8];
        if bsize < 8 || j + bsize > entry.len() {
            break;
        }
        if btyp == b"avcC" {
            let avcc = entry[j + 8..j + bsize].to_vec();
            let cfg = rumpeg_h264::AvcDecoderConfig::from_avcc(&avcc)
                .map_err(|e| Error::invalid_data(e.to_string()))?;
            return Ok((width, height, avcc, cfg.length_size()));
        }
        j += bsize;
    }
    let _ = i;
    Err(Error::invalid_data("avc1 missing avcC"))
}

fn parse_stsz(payload: &[u8]) -> Result<Vec<u32>> {
    if payload.len() < 12 {
        return Err(Error::invalid_data("stsz"));
    }
    let sample_size = u32::from_be_bytes(payload[4..8].try_into().unwrap());
    let count = u32::from_be_bytes(payload[8..12].try_into().unwrap()) as usize;
    if sample_size != 0 {
        return Ok(vec![sample_size; count]);
    }
    if payload.len() < 12 + count * 4 {
        return Err(Error::invalid_data("stsz table"));
    }
    let mut sizes = Vec::with_capacity(count);
    for n in 0..count {
        let o = 12 + n * 4;
        sizes.push(u32::from_be_bytes(payload[o..o + 4].try_into().unwrap()));
    }
    Ok(sizes)
}

fn parse_stco(payload: &[u8]) -> Result<Vec<u64>> {
    if payload.len() < 8 {
        return Err(Error::invalid_data("stco"));
    }
    let count = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
    if payload.len() < 8 + count * 4 {
        return Err(Error::invalid_data("stco table"));
    }
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        let o = 8 + n * 4;
        out.push(u32::from_be_bytes(payload[o..o + 4].try_into().unwrap()) as u64);
    }
    Ok(out)
}

fn parse_co64(payload: &[u8]) -> Result<Vec<u64>> {
    if payload.len() < 8 {
        return Err(Error::invalid_data("co64"));
    }
    let count = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
    if payload.len() < 8 + count * 8 {
        return Err(Error::invalid_data("co64 table"));
    }
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        let o = 8 + n * 8;
        out.push(u64::from_be_bytes(payload[o..o + 8].try_into().unwrap()));
    }
    Ok(out)
}

fn parse_stsc(payload: &[u8]) -> Result<Vec<(u32, u32, u32)>> {
    if payload.len() < 8 {
        return Err(Error::invalid_data("stsc"));
    }
    let count = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
    if payload.len() < 8 + count * 12 {
        return Err(Error::invalid_data("stsc table"));
    }
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        let o = 8 + n * 12;
        let first = u32::from_be_bytes(payload[o..o + 4].try_into().unwrap());
        let spc = u32::from_be_bytes(payload[o + 4..o + 8].try_into().unwrap());
        let desc = u32::from_be_bytes(payload[o + 8..o + 12].try_into().unwrap());
        out.push((first, spc, desc));
    }
    Ok(out)
}

fn parse_stts(payload: &[u8]) -> Result<Vec<(u32, u32)>> {
    if payload.len() < 8 {
        return Err(Error::invalid_data("stts"));
    }
    let count = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
    if payload.len() < 8 + count * 8 {
        return Err(Error::invalid_data("stts table"));
    }
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        let o = 8 + n * 8;
        let sample_count = u32::from_be_bytes(payload[o..o + 4].try_into().unwrap());
        let sample_delta = u32::from_be_bytes(payload[o + 4..o + 8].try_into().unwrap());
        out.push((sample_count, sample_delta));
    }
    Ok(out)
}

fn parse_stss(payload: Option<&[u8]>) -> Option<Vec<u32>> {
    let payload = payload?;
    if payload.len() < 8 {
        return None;
    }
    let count = u32::from_be_bytes(payload[4..8].try_into().unwrap()) as usize;
    if payload.len() < 8 + count * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        let o = 8 + n * 4;
        out.push(u32::from_be_bytes(payload[o..o + 4].try_into().unwrap()));
    }
    Some(out)
}

fn expand_stts(stts: &[(u32, u32)]) -> impl Iterator<Item = u32> + '_ {
    stts.iter()
        .flat_map(|&(count, delta)| std::iter::repeat_n(delta, count as usize))
}

fn expand_sample_offsets(
    chunk_offsets: &[u64],
    stsc: &[(u32, u32, u32)],
    sizes: &[u32],
) -> Result<Vec<u64>> {
    if stsc.is_empty() || chunk_offsets.is_empty() {
        return Err(Error::invalid_data("empty chunk map"));
    }
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut sample_idx = 0usize;
    for (chunk_idx, &chunk_start) in chunk_offsets.iter().enumerate() {
        let chunk_no = (chunk_idx as u32) + 1;
        let mut spc = stsc[0].1;
        for &(first, samples_per_chunk, _) in stsc {
            if first <= chunk_no {
                spc = samples_per_chunk;
            }
        }
        let mut off = chunk_start;
        for _ in 0..spc {
            if sample_idx >= sizes.len() {
                return Ok(offsets);
            }
            offsets.push(off);
            off += u64::from(sizes[sample_idx]);
            sample_idx += 1;
        }
    }
    Ok(offsets)
}

/// Minimal MP4 muxer for a single H.264 track (one or more AVCC samples).
pub struct Mp4Muxer {
    streams: Vec<Stream>,
    extradata: Vec<u8>,
    width: u16,
    height: u16,
    samples: Vec<Vec<u8>>,
    sample_is_key: Vec<bool>,
    timescale: u32,
    /// Duration of each sample in `timescale` ticks.
    sample_delta: u32,
}

impl Mp4Muxer {
    /// Create an empty MP4 muxer.
    pub fn new() -> Self {
        Self {
            streams: Vec::new(),
            extradata: Vec::new(),
            width: 0,
            height: 0,
            samples: Vec::new(),
            sample_is_key: Vec::new(),
            timescale: 1000,
            sample_delta: 40, // 25 fps default at 1000 Hz
        }
    }
}

impl Default for Mp4Muxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Muxer for Mp4Muxer {
    fn format_name(&self) -> &'static str {
        "mp4"
    }

    fn add_stream(&mut self, params: CodecParams) -> Result<usize> {
        if !self.streams.is_empty() {
            return Err(Error::invalid_data("MP4 muxer supports one video stream"));
        }
        if params.codec_id != CodecId::H264 {
            return Err(Error::unsupported(
                "MP4 muxer currently writes avc1/H.264 only",
            ));
        }
        let video = params
            .video()
            .ok_or_else(|| Error::invalid_data("video params required"))?;
        self.width = video.width as u16;
        self.height = video.height as u16;
        self.extradata = params.extradata.clone();
        if self.extradata.is_empty() {
            return Err(Error::invalid_data(
                "H.264 MP4 mux requires avcC extradata on the stream",
            ));
        }
        // Derive timescale + sample delta from frame_rate.
        let fps = video.frame_rate.as_f64();
        if fps > 0.0 && video.frame_rate.num > 0 && video.frame_rate.den > 0 {
            // Prefer exact integer: timescale = fps.num, delta = fps.den when fps = num/den.
            // Use a comfortable timescale (≥ fps) so delta ≥ 1.
            let num = video.frame_rate.num.unsigned_abs().max(1);
            let den = video.frame_rate.den.unsigned_abs().max(1);
            // timescale ticks/sec such that delta = timescale * den / num is integer.
            self.timescale = num;
            self.sample_delta = den;
            if self.sample_delta == 0 {
                self.sample_delta = 1;
            }
        }
        self.streams.push(Stream::new(0, params));
        Ok(0)
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn write_header(&mut self, _writer: &mut dyn IoWriter) -> Result<()> {
        Ok(())
    }

    fn write_packet(&mut self, _writer: &mut dyn IoWriter, packet: &Packet) -> Result<()> {
        // Convert Annex-B → AVCC if needed.
        let data = if packet.data.windows(4).any(|w| w == [0, 0, 0, 1])
            || packet.data.windows(3).any(|w| w == [0, 0, 1])
        {
            rumpeg_h264::annexb_to_avcc_sample(packet.data.as_slice())
        } else {
            packet.data.as_slice().to_vec()
        };
        let filtered = filter_avcc_vcl(&data)?;
        self.samples.push(filtered);
        self.sample_is_key
            .push(packet.flags.contains(PacketFlags::KEY));
        Ok(())
    }

    fn write_trailer(&mut self, writer: &mut dyn IoWriter) -> Result<()> {
        if self.samples.is_empty() {
            return Err(Error::invalid_data("no samples to mux"));
        }
        let bytes = build_mp4(
            self.width,
            self.height,
            &self.extradata,
            &self.samples,
            &self.sample_is_key,
            self.timescale,
            self.sample_delta,
        )?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
    }
}

fn filter_avcc_vcl(sample: &[u8]) -> Result<Vec<u8>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i + 4 <= sample.len() {
        let len = u32::from_be_bytes(sample[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if i + len > sample.len() {
            return Err(Error::invalid_data("bad AVCC length"));
        }
        let nal = &sample[i..i + len];
        let ntype = nal.first().copied().unwrap_or(0) & 0x1F;
        // Keep non-parameter NALs; if only params present, keep all.
        if ntype != 7 && ntype != 8 {
            out.extend_from_slice(&(len as u32).to_be_bytes());
            out.extend_from_slice(nal);
        }
        i += len;
    }
    if out.is_empty() {
        Ok(sample.to_vec())
    } else {
        Ok(out)
    }
}

fn build_mp4(
    width: u16,
    height: u16,
    avcc: &[u8],
    samples: &[Vec<u8>],
    sample_is_key: &[bool],
    timescale: u32,
    sample_delta: u32,
) -> Result<Vec<u8>> {
    let mut mdat_payload = Vec::new();
    let mut sample_sizes = Vec::new();
    for s in samples {
        sample_sizes.push(s.len() as u32);
        mdat_payload.extend_from_slice(s);
    }

    let ftyp = box_of(b"ftyp", {
        let mut b = Vec::new();
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(b"isom");
        b.extend_from_slice(b"iso2");
        b.extend_from_slice(b"avc1");
        b.extend_from_slice(b"mp41");
        b
    });

    let mdat_header = 8u32;
    let mdat_box_size = mdat_header + mdat_payload.len() as u32;
    let data_offset = ftyp.len() as u32 + mdat_header;

    let moov = build_moov(
        width,
        height,
        avcc,
        &sample_sizes,
        sample_is_key,
        data_offset,
        timescale,
        sample_delta,
        samples.len() as u32,
    )?;

    let mut out = Vec::new();
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&(mdat_box_size).to_be_bytes());
    out.extend_from_slice(b"mdat");
    out.extend_from_slice(&mdat_payload);
    out.extend_from_slice(&moov);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn build_moov(
    width: u16,
    height: u16,
    avcc: &[u8],
    sample_sizes: &[u32],
    sample_is_key: &[bool],
    chunk_offset: u32,
    timescale: u32,
    sample_delta: u32,
    sample_count: u32,
) -> Result<Vec<u8>> {
    let delta = sample_delta.max(1);
    let duration = sample_count.saturating_mul(delta);
    let mvhd = box_of(b"mvhd", {
        let mut b = vec![0u8; 100];
        b[0] = 0; // version
                  // timescale at 12
        b[12..16].copy_from_slice(&timescale.to_be_bytes());
        b[16..20].copy_from_slice(&duration.to_be_bytes());
        b[20..24].copy_from_slice(&0x00010000u32.to_be_bytes()); // rate 1.0
        b[24..26].copy_from_slice(&0x0100u16.to_be_bytes()); // volume
                                                             // matrix identity
        b[36..40].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[52..56].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[68..72].copy_from_slice(&0x40000000u32.to_be_bytes());
        b[96..100].copy_from_slice(&2u32.to_be_bytes()); // next_track_ID
        b
    });

    let tkhd = box_of(b"tkhd", {
        let mut b = vec![0u8; 84];
        b[0] = 0;
        b[3] = 0x07; // flags: enabled, in movie, in preview
        b[12..16].copy_from_slice(&1u32.to_be_bytes()); // track_ID
        b[20..24].copy_from_slice(&duration.to_be_bytes());
        b[40..44].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[56..60].copy_from_slice(&0x00010000u32.to_be_bytes());
        b[72..76].copy_from_slice(&0x40000000u32.to_be_bytes());
        b[76..80].copy_from_slice(&(u32::from(width) << 16).to_be_bytes());
        b[80..84].copy_from_slice(&(u32::from(height) << 16).to_be_bytes());
        b
    });

    let mdhd = box_of(b"mdhd", {
        let mut b = vec![0u8; 24];
        b[12..16].copy_from_slice(&timescale.to_be_bytes());
        b[16..20].copy_from_slice(&duration.to_be_bytes());
        b[20..22].copy_from_slice(&0x55c4u16.to_be_bytes()); // und
        b
    });

    let hdlr = box_of(b"hdlr", {
        let mut b = vec![0u8; 24];
        b[8..12].copy_from_slice(b"vide");
        b.extend_from_slice(b"VideoHandler");
        b.push(0);
        b
    });

    let vmhd = box_of(b"vmhd", {
        let mut b = vec![0u8; 12];
        b[3] = 1; // flags
        b
    });

    let dref = box_of(b"dref", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes()); // version/flags
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&box_of(b"url ", {
            let mut u = vec![0u8; 4];
            u[3] = 1; // self-contained
            u
        }));
        b
    });
    let dinf = box_of(b"dinf", dref);

    let avc1 = build_avc1(width, height, avcc)?;
    let stsd = box_of(b"stsd", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&avc1);
        b
    });
    let stts = box_of(b"stts", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&sample_count.to_be_bytes());
        b.extend_from_slice(&delta.to_be_bytes());
        b
    });
    let stsc = box_of(b"stsc", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        b.extend_from_slice(&sample_count.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        b
    });
    let stsz = box_of(b"stsz", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // sample_size
        b.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
        for s in sample_sizes {
            b.extend_from_slice(&s.to_be_bytes());
        }
        b
    });
    let stco = box_of(b"stco", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&chunk_offset.to_be_bytes());
        b
    });
    // Sync samples: all KEY frames (1-based indices). Ensure at least sample 1.
    let mut key_indices: Vec<u32> = sample_is_key
        .iter()
        .enumerate()
        .filter_map(|(i, k)| if *k { Some((i + 1) as u32) } else { None })
        .collect();
    if key_indices.is_empty() {
        key_indices.push(1);
    }
    let stss = box_of(b"stss", {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&(key_indices.len() as u32).to_be_bytes());
        for idx in &key_indices {
            b.extend_from_slice(&idx.to_be_bytes());
        }
        b
    });
    let stbl = box_of(
        b"stbl",
        [
            stsd.as_slice(),
            stts.as_slice(),
            stsc.as_slice(),
            stsz.as_slice(),
            stco.as_slice(),
            stss.as_slice(),
        ]
        .concat(),
    );
    let minf = box_of(
        b"minf",
        [vmhd.as_slice(), dinf.as_slice(), stbl.as_slice()].concat(),
    );
    let mdia = box_of(
        b"mdia",
        [mdhd.as_slice(), hdlr.as_slice(), minf.as_slice()].concat(),
    );
    let trak = box_of(b"trak", [tkhd.as_slice(), mdia.as_slice()].concat());
    Ok(box_of(b"moov", [mvhd.as_slice(), trak.as_slice()].concat()))
}

fn build_avc1(width: u16, height: u16, avcc: &[u8]) -> Result<Vec<u8>> {
    let mut body = vec![0u8; 78];
    // after size+type added by box_of -- VisualSampleEntry fields start at 0 of body:
    // reserved 6, data_ref 2, predef/reserved 16, width/height at 24/26 of body...
    // SampleEntry: 6 bytes reserved + 2 data_reference_index
    body[6..8].copy_from_slice(&1u16.to_be_bytes());
    // VisualSampleEntry continues: pre_defined(2)+reserved(2)+pre_defined[3](12) = 16 -> width at 24
    body[24..26].copy_from_slice(&width.to_be_bytes());
    body[26..28].copy_from_slice(&height.to_be_bytes());
    body[28..32].copy_from_slice(&0x00480000u32.to_be_bytes()); // horiz resolution 72 dpi
    body[32..36].copy_from_slice(&0x00480000u32.to_be_bytes());
    body[40..42].copy_from_slice(&1u16.to_be_bytes()); // frame_count
                                                       // compressor name (32 bytes) already zero
    body[74..76].copy_from_slice(&0x0018u16.to_be_bytes()); // depth
    body[76..78].copy_from_slice(&0xffffu16.to_be_bytes());
    let avcc_box = box_of(b"avcC", avcc.to_vec());
    body.extend_from_slice(&avcc_box);
    Ok(box_of(b"avc1", body))
}

fn box_of(typ: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
    let size = (8 + payload.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(&payload);
    out
}
