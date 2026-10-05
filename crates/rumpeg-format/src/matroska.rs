//! Matroska / WebM demuxer (`matroska-demuxer`).

use crate::demuxer::Demuxer;
use crate::io::IoReader;
use crate::stream::Stream;
use matroska_demuxer::{DemuxError, MatroskaFile, TrackEntry, TrackType};
use rumpeg_util::{
    Buffer, ChannelLayout, CodecId, CodecParams, Error, Packet, PacketFlags, PixelFormat, Rational,
    Result, SampleFormat, Timestamp, VideoParams,
};
use std::collections::HashMap;
use std::io::{Cursor, SeekFrom};

const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Probe score for Matroska / WebM (EBML header).
pub fn probe_score(buf: &[u8]) -> u32 {
    if buf.len() >= 4 && buf[0..4] == EBML_MAGIC {
        100
    } else {
        0
    }
}

/// Matroska / WebM demuxer.
pub struct MatroskaDemuxer {
    mkv: MatroskaFile<Cursor<Vec<u8>>>,
    streams: Vec<Stream>,
    track_to_stream: HashMap<u64, usize>,
    format_name: &'static str,
}

impl MatroskaDemuxer {
    /// Parse tracks and prepare for packet reads.
    pub fn open(reader: &mut dyn IoReader) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut file_data = Vec::new();
        reader.read_to_end(&mut file_data)?;

        let mkv = MatroskaFile::open(Cursor::new(file_data)).map_err(map_demux_err)?;
        let doc = mkv.ebml_header().doc_type();
        let format_name = if doc.eq_ignore_ascii_case("webm") {
            "webm"
        } else {
            "matroska"
        };

        let ts_scale = mkv.info().timestamp_scale().get();
        let time_base = matroska_time_base(ts_scale);

        let mut streams = Vec::new();
        let mut track_to_stream = HashMap::new();

        for track in mkv.tracks() {
            if !track.flag_enabled() {
                continue;
            }
            let Some(params) = track_to_codec_params(track) else {
                continue;
            };
            let stream_index = streams.len();
            track_to_stream.insert(track.track_number().get(), stream_index);
            let mut stream = Stream::new(stream_index, params);
            stream.time_base = time_base;
            streams.push(stream);
        }

        if streams.is_empty() {
            return Err(Error::invalid_data("matroska: no supported enabled tracks"));
        }

        Ok(Self {
            mkv,
            streams,
            track_to_stream,
            format_name,
        })
    }
}

impl Demuxer for MatroskaDemuxer {
    fn format_name(&self) -> &'static str {
        self.format_name
    }

    fn streams(&self) -> &[Stream] {
        &self.streams
    }

    fn read_packet(&mut self, _reader: &mut dyn IoReader) -> Result<Packet> {
        let mut frame = matroska_demuxer::Frame::default();
        loop {
            let ok = self.mkv.next_frame(&mut frame).map_err(map_demux_err)?;
            if !ok {
                return Err(Error::Eof);
            }
            if frame.is_invisible {
                continue;
            }
            let Some(&stream_index) = self.track_to_stream.get(&frame.track) else {
                continue;
            };
            let mut pkt = Packet::new(Buffer::from_vec(frame.data.clone()));
            pkt.stream_index = stream_index;
            let pts = i64::try_from(frame.timestamp).unwrap_or(i64::MAX);
            pkt.pts = Timestamp::new(pts);
            pkt.dts = pkt.pts;
            if frame.is_keyframe == Some(true) {
                pkt.flags.insert(PacketFlags::KEY);
            }
            return Ok(pkt);
        }
    }

    fn seek(&mut self, _reader: &mut dyn IoReader, timestamp: i64) -> Result<()> {
        let ts = u64::try_from(timestamp.max(0)).unwrap_or(0);
        self.mkv.seek(ts).map_err(map_demux_err)
    }
}

fn matroska_time_base(timestamp_scale: u64) -> Rational {
    // TimestampScale is nanoseconds per Matroska timecode tick.
    let den = (1_000_000_000u64 / timestamp_scale.max(1)).max(1);
    Rational::new(1, den.min(i32::MAX as u64) as i32)
}

fn track_to_codec_params(track: &TrackEntry) -> Option<CodecParams> {
    let codec_id = track.codec_id();
    match track.track_type() {
        TrackType::Video => {
            let video = track.video()?;
            let mapped = map_video_codec(codec_id)?;
            let mut params = CodecParams::video_codec(
                mapped,
                VideoParams {
                    pix_fmt: PixelFormat::Yuv420p,
                    width: video.pixel_width().get() as u32,
                    height: video.pixel_height().get() as u32,
                    frame_rate: Rational::new(25, 1),
                    sample_aspect_ratio: Rational::one(),
                },
            );
            if mapped == CodecId::H264 || mapped == CodecId::Hevc {
                if let Some(priv_data) = track.codec_private() {
                    params.extradata = priv_data.to_vec();
                }
            }
            Some(params)
        }
        TrackType::Audio => {
            let audio = track.audio()?;
            let mapped = map_audio_codec(codec_id, track)?;
            let channels = audio.channels().get().min(u16::MAX as u64) as u16;
            let sample_rate = audio.sampling_frequency().round().max(1.0) as u32;
            let sample_fmt = SampleFormat::S16;
            let mut params = CodecParams::audio_codec(
                mapped,
                rumpeg_util::AudioParams {
                    sample_fmt,
                    sample_rate,
                    layout: ChannelLayout::new(channels.max(1)),
                    frame_size: 0,
                },
            );
            if let Some(priv_data) = track.codec_private() {
                params.extradata = priv_data.to_vec();
            }
            Some(params)
        }
        _ => None,
    }
}

fn map_video_codec(codec_id: &str) -> Option<CodecId> {
    Some(match codec_id {
        "V_MPEG4/ISO/AVC" => CodecId::H264,
        "V_MPEGH/ISO/HEVC" => CodecId::Hevc,
        "V_VP9" => CodecId::Vp9,
        "V_VP8" => CodecId::Vp8,
        "V_AV1" => CodecId::Av1,
        _ => return None,
    })
}

fn map_audio_codec(codec_id: &str, track: &TrackEntry) -> Option<CodecId> {
    Some(match codec_id {
        "A_FLAC" => CodecId::Flac,
        "A_OPUS" => CodecId::Opus,
        "A_AAC" | "A_AAC/MPEG2/Main" | "A_AAC/MPEG2/LC" | "A_AAC/MPEG4/Main" | "A_AAC/MPEG4/LC" => {
            CodecId::Aac
        }
        "A_MPEG/L3" => CodecId::Mp3,
        "A_PCM/INT/LIT" => {
            let depth = track.audio()?.bit_depth()?.get();
            if depth == 16 {
                CodecId::PcmS16Le
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

fn map_demux_err(err: DemuxError) -> Error {
    Error::invalid_data(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_matroska_magic() {
        assert_eq!(probe_score(&EBML_MAGIC), 100);
        assert_eq!(probe_score(b"ftyp"), 0);
    }

    #[test]
    fn map_video_codec_ids() {
        assert_eq!(map_video_codec("V_MPEG4/ISO/AVC"), Some(CodecId::H264));
        assert_eq!(map_video_codec("V_VP9"), Some(CodecId::Vp9));
        assert_eq!(map_video_codec("V_UNKNOWN"), None);
    }
}
