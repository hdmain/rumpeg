//! Minimal MPEG-TS demuxer (188-byte packets, PAT/PMT, PES reassembly).

use crate::demuxer::Demuxer;
use crate::io::IoReader;
use crate::stream::Stream;
use rumpeg_util::{
    Buffer, ChannelLayout, CodecId, CodecParams, Error, Packet, PacketFlags, PixelFormat, Rational,
    Result, SampleFormat, Timestamp, VideoParams,
};
use std::collections::HashMap;
use std::io::SeekFrom;

const TS_PACKET_SIZE: usize = 188;
const TS_SYNC: u8 = 0x47;

const STREAM_TYPE_H264: u8 = 0x1B;
const STREAM_TYPE_AAC: u8 = 0x0F;
const STREAM_TYPE_PRIVATE: u8 = 0x06;

/// Probe: sync byte every 188 bytes for several consecutive packets.
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() < TS_PACKET_SIZE * 3 {
        return 0;
    }
    let packets = (buf.len() / TS_PACKET_SIZE).min(8);
    for i in 0..packets {
        if buf[i * TS_PACKET_SIZE] != TS_SYNC {
            return 0;
        }
    }
    90
}

#[derive(Clone, Debug)]
struct ElementaryStream {
    pid: u16,
    stream_type: u8,
}

#[derive(Clone, Debug)]
struct PesPacket {
    stream_index: usize,
    pts: Option<i64>,
    data: Vec<u8>,
    is_key: bool,
}

/// MPEG-TS demuxer.
pub struct MpegTsDemuxer {
    streams: Vec<Stream>,
    packets: Vec<PesPacket>,
    index: usize,
}

impl MpegTsDemuxer {
    /// Scan the transport stream and build PES packet index.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let (streams, packets) = demux_ts(&data)?;
        Ok(Self {
            streams,
            packets,
            index: 0,
        })
    }
}

impl Demuxer for MpegTsDemuxer {
    fn format_name(&self) -> &'static str {
        "mpegts"
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, _reader: &mut dyn IoReader) -> Result<Packet> {
        let pes = self.packets.get(self.index).ok_or(Error::Eof)?.clone();
        self.index += 1;
        let mut pkt = Packet::new(Buffer::from_vec(pes.data));
        pkt.stream_index = pes.stream_index;
        let ts = pes.pts.unwrap_or(0);
        pkt.pts = Timestamp::new(ts);
        pkt.dts = pkt.pts;
        if pes.is_key {
            pkt.flags.insert(PacketFlags::KEY);
        }
        Ok(pkt)
    }

    fn seek(&mut self, _reader: &mut dyn IoReader, timestamp: i64) -> Result<()> {
        self.index = self
            .packets
            .iter()
            .position(|p| p.pts.unwrap_or(0) >= timestamp)
            .unwrap_or(0);
        Ok(())
    }
}

fn demux_ts(data: &[u8]) -> Result<(Vec<Stream>, Vec<PesPacket>)> {
    if data.len() < TS_PACKET_SIZE || data[0] != TS_SYNC {
        return Err(Error::invalid_data("mpegts: missing sync byte"));
    }
    if !validate_sync(data) {
        return Err(Error::invalid_data(
            "mpegts: sync not at 188-byte boundaries",
        ));
    }

    let pmt_pid = find_pmt_pid(data)?;
    let elementary = parse_pmt(data, pmt_pid)?;
    if elementary.is_empty() {
        return Err(Error::invalid_data(
            "mpegts: no supported elementary streams",
        ));
    }

    let mut streams = Vec::new();
    for (idx, es) in elementary.iter().enumerate() {
        let params = es_to_codec_params(es.stream_type)?;
        let mut stream = Stream::new(idx, params);
        stream.time_base = Rational::new(1, 90_000);
        streams.push(stream);
    }

    let pid_to_index: HashMap<u16, usize> = elementary
        .iter()
        .enumerate()
        .map(|(i, es)| (es.pid, i))
        .collect();

    let pes_packets = reassemble_pes(data, &pid_to_index)?;
    if pes_packets.is_empty() {
        return Err(Error::invalid_data("mpegts: no PES payloads"));
    }

    Ok((streams, pes_packets))
}

fn validate_sync(data: &[u8]) -> bool {
    let n = data.len() / TS_PACKET_SIZE;
    (0..n).all(|i| data[i * TS_PACKET_SIZE] == TS_SYNC)
}

fn es_to_codec_params(stream_type: u8) -> Result<CodecParams> {
    Ok(match stream_type {
        STREAM_TYPE_H264 => CodecParams::video_codec(
            CodecId::H264,
            VideoParams {
                pix_fmt: PixelFormat::Yuv420p,
                width: 0,
                height: 0,
                frame_rate: Rational::new(25, 1),
                sample_aspect_ratio: Rational::one(),
            },
        ),
        STREAM_TYPE_AAC => CodecParams::audio_codec(
            CodecId::Aac,
            rumpeg_util::AudioParams {
                sample_fmt: SampleFormat::S16,
                sample_rate: 48_000,
                layout: ChannelLayout::STEREO,
                frame_size: 0,
            },
        ),
        STREAM_TYPE_PRIVATE => CodecParams::audio_codec(
            CodecId::Opus,
            rumpeg_util::AudioParams {
                sample_fmt: SampleFormat::S16,
                sample_rate: 48_000,
                layout: ChannelLayout::STEREO,
                frame_size: 0,
            },
        ),
        other => {
            return Err(Error::unsupported(format!(
                "mpegts stream type 0x{other:02X}"
            )));
        }
    })
}

fn find_pmt_pid(data: &[u8]) -> Result<u16> {
    for pkt in iter_ts_packets(data) {
        if pkt.pid != 0 {
            continue;
        }
        if let Some(section) = packet_payload(pkt) {
            if let Some(pmt) = parse_pat(&section) {
                return Ok(pmt);
            }
        }
    }
    Err(Error::invalid_data("mpegts: PAT not found"))
}

fn parse_pat(section: &[u8]) -> Option<u16> {
    if section.len() < 8 || section[0] != 0 {
        return None;
    }
    let section_length = u16::from_be_bytes([section[1], section[2]]) as usize & 0x0FFF;
    if section.len() < 3 + section_length {
        return None;
    }
    let mut i = 8;
    let end = 3 + section_length - 1;
    while i + 4 <= end {
        let program_number = u16::from_be_bytes([section[i], section[i + 1]]);
        let pid = u16::from_be_bytes([section[i + 2] & 0x1F, section[i + 3]]);
        if program_number != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

fn parse_pmt(data: &[u8], pmt_pid: u16) -> Result<Vec<ElementaryStream>> {
    for pkt in iter_ts_packets(data) {
        if pkt.pid != pmt_pid {
            continue;
        }
        if let Some(section) = packet_payload(pkt) {
            if let Some(list) = parse_pmt_section(&section) {
                return Ok(list);
            }
        }
    }
    Err(Error::invalid_data("mpegts: PMT not found"))
}

fn parse_pmt_section(section: &[u8]) -> Option<Vec<ElementaryStream>> {
    if section.len() < 12 || section[0] != 0x02 {
        return None;
    }
    let section_length = u16::from_be_bytes([section[1], section[2]]) as usize & 0x0FFF;
    if section.len() < 3 + section_length {
        return None;
    }
    let program_info_length = u16::from_be_bytes([section[10], section[11]]) as usize & 0x0FFF;
    let mut i = 12 + program_info_length;
    let end = 3 + section_length - 1;
    let mut out = Vec::new();
    while i + 5 <= end {
        let stream_type = section[i];
        let elementary_pid = u16::from_be_bytes([section[i + 1] & 0x1F, section[i + 2]]);
        let es_info_length = u16::from_be_bytes([section[i + 3], section[i + 4]]) as usize & 0x0FFF;
        i += 5 + es_info_length;
        if matches!(
            stream_type,
            STREAM_TYPE_H264 | STREAM_TYPE_AAC | STREAM_TYPE_PRIVATE
        ) {
            out.push(ElementaryStream {
                pid: elementary_pid,
                stream_type,
            });
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

struct TsPacket<'a> {
    pid: u16,
    payload_unit_start: bool,
    payload: &'a [u8],
}

fn iter_ts_packets(data: &[u8]) -> impl Iterator<Item = TsPacket<'_>> {
    let n = data.len() / TS_PACKET_SIZE;
    (0..n).filter_map(move |i| {
        let off = i * TS_PACKET_SIZE;
        let pkt = &data[off..off + TS_PACKET_SIZE];
        parse_ts_packet(pkt)
    })
}

fn parse_ts_packet(pkt: &[u8]) -> Option<TsPacket<'_>> {
    if pkt[0] != TS_SYNC {
        return None;
    }
    let pid = u16::from_be_bytes([pkt[1] & 0x1F, pkt[2]]);
    let payload_unit_start = pkt[1] & 0x40 != 0;
    let adaptation_field_control = (pkt[3] >> 4) & 0x3;
    let mut idx = 4usize;
    if adaptation_field_control == 0x2 || adaptation_field_control == 0x3 {
        let adapt_len = pkt[idx] as usize;
        idx += 1 + adapt_len;
    }
    if adaptation_field_control == 0x2 {
        return Some(TsPacket {
            pid,
            payload_unit_start,
            payload: &[],
        });
    }
    if idx >= pkt.len() {
        return Some(TsPacket {
            pid,
            payload_unit_start,
            payload: &[],
        });
    }
    Some(TsPacket {
        pid,
        payload_unit_start,
        payload: &pkt[idx..],
    })
}

fn packet_payload(pkt: TsPacket<'_>) -> Option<Vec<u8>> {
    if !pkt.payload_unit_start || pkt.payload.is_empty() {
        return None;
    }
    let pointer = pkt.payload[0] as usize;
    if 1 + pointer > pkt.payload.len() {
        return None;
    }
    let section = &pkt.payload[1 + pointer..];
    if section.len() < 3 {
        return None;
    }
    // Truncate stuffing bytes after the PSI section.
    let section_length = u16::from_be_bytes([section[1], section[2]]) as usize & 0x0FFF;
    let total = 3 + section_length;
    if section.len() < total {
        // Incomplete multi-packet section — not supported yet.
        return None;
    }
    Some(section[..total].to_vec())
}

struct PesBuilder {
    stream_index: usize,
    pts: Option<i64>,
    data: Vec<u8>,
}

fn reassemble_pes(data: &[u8], pid_to_index: &HashMap<u16, usize>) -> Result<Vec<PesPacket>> {
    let mut builders: HashMap<u16, PesBuilder> = HashMap::new();
    let mut completed = Vec::new();

    for pkt in iter_ts_packets(data) {
        let Some(&stream_index) = pid_to_index.get(&pkt.pid) else {
            continue;
        };
        if pkt.payload.is_empty() {
            continue;
        }
        if pkt.payload_unit_start {
            if let Some(prev) = builders.remove(&pkt.pid) {
                if !prev.data.is_empty() {
                    completed.push(finish_pes(prev));
                }
            }
            let payload = if pkt.payload.len() >= 3
                && pkt.payload[0] == 0
                && pkt.payload[1] == 0
                && pkt.payload[2] == 1
            {
                pkt.payload
            } else if !pkt.payload.is_empty() {
                let skip = pkt.payload[0] as usize + 1;
                if skip <= pkt.payload.len() {
                    &pkt.payload[skip..]
                } else {
                    continue;
                }
            } else {
                continue;
            };
            if payload.len() >= 6 && payload[0] == 0 && payload[1] == 0 && payload[2] == 1 {
                let (body, pts) = parse_pes_header(payload)?;
                builders.insert(
                    pkt.pid,
                    PesBuilder {
                        stream_index,
                        pts,
                        data: body.to_vec(),
                    },
                );
            }
        } else if let Some(b) = builders.get_mut(&pkt.pid) {
            b.data.extend_from_slice(pkt.payload);
        }
    }
    for (_, b) in builders {
        if !b.data.is_empty() {
            completed.push(finish_pes(b));
        }
    }
    completed.sort_by_key(|p| p.pts.unwrap_or(0));
    Ok(completed)
}

fn parse_pes_header(payload: &[u8]) -> Result<(&[u8], Option<i64>)> {
    if payload.len() < 9 {
        return Err(Error::invalid_data("mpegts: truncated PES header"));
    }
    let _stream_id = payload[3];
    let pes_packet_length = u16::from_be_bytes([payload[4], payload[5]]) as usize;
    let mut idx = 6usize;
    if idx >= payload.len() {
        return Ok((&[], None));
    }
    let flags = payload[idx];
    idx += 1;
    if idx >= payload.len() {
        return Ok((&[], None));
    }
    let pes_header_data_length = payload[idx] as usize;
    idx += 1;
    let mut pts = None;
    if (flags & 0xC0) != 0 && idx + 5 <= payload.len() {
        pts = Some(parse_pes_timestamp(&payload[idx..idx + 5]));
        idx += 5;
        if (flags & 0x40) != 0 && idx + 5 <= payload.len() {
            idx += 5;
        }
    } else {
        idx += pes_header_data_length;
    }
    let body_end = if pes_packet_length == 0 {
        payload.len()
    } else {
        6 + pes_packet_length.min(payload.len().saturating_sub(6))
    };
    let body = &payload[idx.min(body_end)..body_end.min(payload.len())];
    Ok((body, pts))
}

fn parse_pes_timestamp(data: &[u8]) -> i64 {
    let b0 = data[0];
    let val = ((data[1] as u64) << 22)
        | (((data[2] as u64) >> 1) << 15)
        | (((data[3] as u64) >> 1) << 8)
        | ((data[4] as u64) >> 1);
    let _marker = b0 >> 6;
    val as i64
}

fn finish_pes(b: PesBuilder) -> PesPacket {
    let is_key = h264_access_unit_is_key(&b.data);
    PesPacket {
        stream_index: b.stream_index,
        pts: b.pts,
        data: b.data,
        is_key,
    }
}

fn h264_access_unit_is_key(data: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i..i + 3] == [0, 0, 1] {
            let ntype = data.get(i + 3).copied().unwrap_or(0) & 0x1F;
            if ntype == 5 {
                return true;
            }
            i += 4;
        } else if i + 4 < data.len() && data[i..i + 4] == [0, 0, 0, 1] {
            let ntype = data.get(i + 4).copied().unwrap_or(0) & 0x1F;
            if ntype == 5 {
                return true;
            }
            i += 5;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn crc8_section(mut section: Vec<u8>) -> Vec<u8> {
        let len = section.len() - 3;
        section[1] = ((len >> 8) as u8 & 0x0F) | 0xB0;
        section[2] = (len & 0xFF) as u8;
        let crc = mpegts_crc32(&section[..section.len() - 4]);
        let tail = section.len() - 4;
        section[tail..].copy_from_slice(&crc.to_be_bytes());
        section
    }

    fn mpegts_crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= (b as u32) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    fn wrap_section(pid: u16, section: &[u8], continuity: u8) -> [u8; TS_PACKET_SIZE] {
        let mut pkt = [0xFFu8; TS_PACKET_SIZE];
        pkt[0] = TS_SYNC;
        pkt[1] = 0x40 | ((pid >> 8) as u8 & 0x1F);
        pkt[2] = (pid & 0xFF) as u8;
        pkt[3] = 0x10 | (continuity & 0x0F);
        pkt[4] = 0;
        pkt[5..5 + section.len()].copy_from_slice(section);
        pkt
    }

    fn wrap_pes(pid: u16, pes: &[u8], continuity: u8) -> [u8; TS_PACKET_SIZE] {
        let mut pkt = [0xFFu8; TS_PACKET_SIZE];
        pkt[0] = TS_SYNC;
        pkt[1] = 0x40 | ((pid >> 8) as u8 & 0x1F);
        pkt[2] = (pid & 0xFF) as u8;
        pkt[3] = 0x10 | (continuity & 0x0F);
        let copy_len = (TS_PACKET_SIZE - 4).min(pes.len());
        pkt[4..4 + copy_len].copy_from_slice(&pes[..copy_len]);
        pkt
    }

    #[test]
    fn probe_ts_sync() {
        let mut buf = vec![0u8; TS_PACKET_SIZE * 4];
        for (i, chunk) in buf.chunks_mut(TS_PACKET_SIZE).enumerate() {
            chunk[0] = TS_SYNC;
            chunk[1] = i as u8;
        }
        assert_eq!(probe_score(&buf), 90);
    }

    #[test]
    fn demux_minimal_h264_ts() {
        let pmt_pid: u16 = 0x1000;
        let video_pid: u16 = 0x0100;
        let mut pat = vec![0u8; 16];
        pat[5] = 0x00;
        pat[6] = 0x00;
        pat[7] = 0x00;
        pat[8] = 0x00;
        pat[9] = 0x01;
        pat[10] = 0xE0 | ((pmt_pid >> 8) as u8 & 0x1F);
        pat[11] = (pmt_pid & 0xFF) as u8;
        let pat = crc8_section(pat);

        let mut pmt = vec![0u8; 21];
        pmt[0] = 0x02;
        pmt[3] = 0x00;
        pmt[4] = 0x01;
        pmt[5] = 0xC1;
        pmt[6] = 0x00;
        pmt[7] = 0x00;
        pmt[8] = 0xE0 | ((video_pid >> 8) as u8 & 0x1F);
        pmt[9] = (video_pid & 0xFF) as u8;
        pmt[10] = 0xF0;
        pmt[11] = 0x00;
        pmt[12] = STREAM_TYPE_H264;
        pmt[13] = 0xE0 | ((video_pid >> 8) as u8 & 0x1F);
        pmt[14] = (video_pid & 0xFF) as u8;
        pmt[15] = 0xF0;
        pmt[16] = 0x00;
        let pmt = crc8_section(pmt);

        let nal = vec![0, 0, 0, 1, 0x65, 0x88, 0x84];
        let payload_len = nal.len() + 2;
        let mut pes = vec![0, 0, 1, 0xE0];
        pes.push((payload_len >> 8) as u8);
        pes.push((payload_len & 0xFF) as u8);
        pes.push(0);
        pes.push(0);
        pes.extend_from_slice(&nal);

        let mut ts = Vec::new();
        ts.extend_from_slice(&wrap_section(0, &pat, 0));
        ts.extend_from_slice(&wrap_section(pmt_pid, &pmt, 0));
        ts.extend_from_slice(&wrap_pes(video_pid, &pes, 0));

        let (streams, packets) = demux_ts(&ts).expect("demux");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec_params.codec_id, CodecId::H264);
        assert_eq!(packets.len(), 1);
        assert!(packets[0].data.windows(4).any(|w| w == [0, 0, 0, 1]));

        let mut cursor = Cursor::new(ts);
        let demux = MpegTsDemuxer::open(&mut cursor).expect("open");
        assert_eq!(demux.streams().len(), 1);
    }
}
