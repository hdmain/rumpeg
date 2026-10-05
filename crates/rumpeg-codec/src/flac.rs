//! FLAC decode ([`claxon`]) and encode ([`flacenc`]) — both pure Rust.

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, CodecSpecific, Error,
    Frame, Packet, PacketFlags, Result, SampleFormat,
};
use std::collections::VecDeque;
use std::io::Cursor;

fn audio_params(params: &CodecParams) -> Result<AudioParams> {
    match &params.specific {
        CodecSpecific::Audio(a) => Ok(a.clone()),
        _ => Ok(AudioParams::default()),
    }
}

fn i32_planar_to_s16_interleaved(channels: &[Vec<i32>], bits: u32) -> Result<Vec<u8>> {
    let ch = channels.len();
    if ch == 0 {
        return Ok(Vec::new());
    }
    let n = channels[0].len();
    let shift = bits.saturating_sub(16);
    let mut out = Vec::with_capacity(n * ch * 2);
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        for c in 0..ch {
            let mut s = channels[c][i];
            if shift > 0 {
                s >>= shift as i32;
            }
            let s16 = s.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            out.extend_from_slice(&s16.to_le_bytes());
        }
    }
    Ok(out)
}

/// FLAC decoder (claxon). Accepts a complete FLAC bitstream in one or more packets;
/// typical usage feeds the whole `.flac` file as a single packet.
pub struct FlacDecoder {
    pending: VecDeque<Frame>,
    eof: bool,
    /// Accumulated compressed bytes (FLAC is often one contiguous stream).
    buf: Vec<u8>,
}

impl FlacDecoder {
    /// Open a FLAC decoder.
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            pending: VecDeque::new(),
            eof: false,
            buf: Vec::new(),
        })
    }

    fn drain_buf(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let cursor = Cursor::new(self.buf.as_slice());
        let mut reader = match claxon::FlacReader::new(cursor) {
            Ok(r) => r,
            Err(e) => {
                // Incomplete stream — wait for more data unless flushing.
                if !self.eof {
                    return Ok(());
                }
                return Err(Error::invalid_data(format!("FLAC open failed: {e}")));
            }
        };
        let info = reader.streaminfo();
        let channels = info.channels as u16;
        let sample_rate = info.sample_rate;
        let bits = info.bits_per_sample;
        let mut blocks = reader.blocks();
        let mut scratch = Vec::new();
        loop {
            match blocks.read_next_or_eof(scratch) {
                Ok(Some(block)) => {
                    let chans: Vec<Vec<i32>> = (0..channels as u32)
                        .map(|c| block.channel(c).to_vec())
                        .collect();
                    let pcm = i32_planar_to_s16_interleaved(&chans, bits)?;
                    let samples = (pcm.len() / (2 * channels.max(1) as usize)) as u32;
                    let mut frame = AudioFrame::alloc_interleaved(
                        SampleFormat::S16,
                        sample_rate,
                        ChannelLayout::new(channels),
                        samples,
                    );
                    frame.data_mut().copy_from_slice(&pcm);
                    scratch = block.into_buffer();
                    self.pending.push_back(Frame::Audio(frame));
                }
                Ok(None) => break,
                Err(e) => {
                    if self.eof {
                        return Err(Error::invalid_data(format!("FLAC decode: {e}")));
                    }
                    break;
                }
            }
        }
        if self.eof {
            self.buf.clear();
        }
        Ok(())
    }
}

impl Decoder for FlacDecoder {
    fn codec_id(&self) -> CodecId {
        CodecId::Flac
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                self.buf.extend_from_slice(p.data.as_slice());
                self.drain_buf()?;
            }
            None => {
                self.eof = true;
                self.drain_buf()?;
            }
        }
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Frame> {
        if let Some(f) = self.pending.pop_front() {
            return Ok(f);
        }
        if self.eof {
            Err(Error::Eof)
        } else {
            Err(Error::NeedMoreData)
        }
    }
}

/// FLAC encoder via flacenc (produces a complete FLAC stream on flush).
pub struct FlacEncoder {
    params: AudioParams,
    pcm_s16: Vec<i16>,
    pending: VecDeque<Packet>,
    eof: bool,
    finished: bool,
}

impl FlacEncoder {
    /// Open a FLAC encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let audio = audio_params(params)?;
        Ok(Self {
            params: audio,
            pcm_s16: Vec::new(),
            pending: VecDeque::new(),
            eof: false,
            finished: false,
        })
    }

    fn encode_all(&mut self) -> Result<()> {
        if self.finished || self.pcm_s16.is_empty() {
            return Ok(());
        }
        let channels = self.params.layout.channels.max(1) as usize;
        let sample_rate = self.params.sample_rate as usize;
        let samples_per_ch = self.pcm_s16.len() / channels;
        if samples_per_ch == 0 {
            return Ok(());
        }

        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let samples_i32: Vec<i32> = self.pcm_s16.iter().map(|&s| i32::from(s)).collect();
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|e| Error::invalid_data(format!("FLAC config: {e:?}")))?;
        let source =
            flacenc::source::MemSource::from_samples(&samples_i32, channels, 16, sample_rate);
        let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
            .map_err(|e| Error::Other(format!("FLAC encode: {e}")))?;
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream
            .write(&mut sink)
            .map_err(|e| Error::Other(format!("FLAC write: {e}")))?;
        let bytes = sink.as_slice().to_vec();

        let mut pkt = Packet::new(Buffer::from_vec(bytes));
        pkt.flags.insert(PacketFlags::KEY);
        self.pending.push_back(pkt);
        self.finished = true;
        self.pcm_s16.clear();
        Ok(())
    }
}

impl Encoder for FlacEncoder {
    fn codec_id(&self) -> CodecId {
        CodecId::Flac
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        match frame {
            Some(Frame::Audio(audio)) => {
                let s16 = if audio.format == SampleFormat::S16 {
                    let data = audio.data();
                    let mut v = Vec::with_capacity(data.len() / 2);
                    for c in data.chunks_exact(2) {
                        v.push(i16::from_le_bytes([c[0], c[1]]));
                    }
                    v
                } else {
                    // Convert via f32 path is heavy; require s16 for now.
                    return Err(Error::unsupported(
                        "FLAC encoder currently expects s16 interleaved PCM",
                    ));
                };
                self.pcm_s16.extend_from_slice(&s16);
                Ok(())
            }
            Some(_) => Err(Error::invalid_data("FLAC encoder expects audio")),
            None => {
                self.eof = true;
                self.encode_all()
            }
        }
    }

    fn receive_packet(&mut self) -> Result<Packet> {
        if let Some(p) = self.pending.pop_front() {
            return Ok(p);
        }
        if self.eof {
            Err(Error::Eof)
        } else {
            Err(Error::NeedMoreData)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_roundtrip_silence() {
        let params = CodecParams::audio_codec(
            CodecId::Flac,
            AudioParams {
                sample_fmt: SampleFormat::S16,
                sample_rate: 16_000,
                layout: ChannelLayout::MONO,
                frame_size: 0,
            },
        );
        let mut enc = FlacEncoder::new(&params).unwrap();
        let mut frame =
            AudioFrame::alloc_interleaved(SampleFormat::S16, 16_000, ChannelLayout::MONO, 2048);
        // Soft tone so encoder has content.
        let plane = frame.data_mut();
        for (i, chunk) in plane.chunks_exact_mut(2).enumerate() {
            let s = ((i as f32 * 0.1).sin() * 1000.0) as i16;
            chunk.copy_from_slice(&s.to_le_bytes());
        }
        enc.send_frame(Some(&Frame::Audio(frame))).unwrap();
        enc.send_frame(None).unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert!(pkt.data.len() > 42);
        assert_eq!(&pkt.data.as_slice()[0..4], b"fLaC");

        let mut dec = FlacDecoder::new(&params).unwrap();
        dec.send_packet(Some(&pkt)).unwrap();
        dec.send_packet(None).unwrap();
        let out = dec.receive_frame().unwrap();
        let Frame::Audio(a) = out else {
            panic!("audio")
        };
        assert_eq!(a.sample_rate, 16_000);
        assert!(a.samples > 0);
    }
}
