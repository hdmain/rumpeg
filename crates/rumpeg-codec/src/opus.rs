//! Opus decode (+ encode) via pure-Rust [`rusty_opus`].

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, CodecSpecific, Error,
    Frame, Packet, PacketFlags, Result, SampleFormat, Timestamp,
};
use rusty_opus::{Application, OpusDecoder, OpusEncoder};
use std::collections::VecDeque;

fn audio_params(params: &CodecParams) -> Result<AudioParams> {
    match &params.specific {
        CodecSpecific::Audio(a) => Ok(a.clone()),
        _ => Ok(AudioParams {
            sample_fmt: SampleFormat::S16,
            sample_rate: 48_000,
            layout: ChannelLayout::STEREO,
            frame_size: 0,
        }),
    }
}

/// Opus decoder.
pub struct OpusDecoderCodec {
    inner: OpusDecoder,
    channels: usize,
    sample_rate: i32,
    pending: VecDeque<Frame>,
    last_pts: Timestamp,
    eof: bool,
}

impl OpusDecoderCodec {
    /// Open an Opus decoder (defaults to 48 kHz stereo when params incomplete).
    pub fn new(params: &CodecParams) -> Result<Self> {
        let audio = audio_params(params)?;
        let sample_rate = if audio.sample_rate == 0 {
            48_000
        } else {
            audio.sample_rate as i32
        };
        let channels = audio.layout.channels.max(1) as usize;
        let inner = OpusDecoder::new(sample_rate, channels)
            .map_err(|e| Error::invalid_data(format!("Opus decoder: {e:?}")))?;
        Ok(Self {
            inner,
            channels,
            sample_rate,
            pending: VecDeque::new(),
            last_pts: Timestamp::NONE,
            eof: false,
        })
    }
}

impl Decoder for OpusDecoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Opus
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                self.last_pts = p.pts;
                // Max Opus frame is 120 ms @ 48 kHz = 5760 samples/ch.
                let max_frame = 5760usize;
                let mut out = vec![0.0f32; max_frame * self.channels];
                let n = self
                    .inner
                    .decode(p.data.as_slice(), max_frame, &mut out)
                    .map_err(|e| Error::invalid_data(format!("Opus decode: {e:?}")))?;
                out.truncate(n * self.channels);
                let mut frame = AudioFrame::alloc_interleaved(
                    SampleFormat::S16,
                    self.sample_rate as u32,
                    ChannelLayout::new(self.channels as u16),
                    n as u32,
                );
                frame.pts = self.last_pts;
                let plane = frame.data_mut();
                for (i, &s) in out.iter().enumerate() {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    plane[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
                self.pending.push_back(Frame::Audio(frame));
            }
            None => self.eof = true,
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

/// Opus encoder.
pub struct OpusEncoderCodec {
    inner: OpusEncoder,
    channels: usize,
    frame_samples: usize,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl OpusEncoderCodec {
    /// Open an Opus encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let audio = audio_params(params)?;
        let sample_rate = if audio.sample_rate == 0 {
            48_000
        } else {
            audio.sample_rate as i32
        };
        let channels = audio.layout.channels.max(1) as usize;
        let mut inner = OpusEncoder::new(sample_rate, channels, Application::Audio)
            .map_err(|e| Error::invalid_data(format!("Opus encoder: {e:?}")))?;
        if params.bit_rate > 0 {
            inner.bitrate_bps = params.bit_rate as i32;
        }
        // 20 ms frames at the encoder rate.
        let frame_samples = (sample_rate as usize / 50).max(120);
        Ok(Self {
            inner,
            channels,
            frame_samples,
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Encoder for OpusEncoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Opus
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        match frame {
            Some(Frame::Audio(audio)) => {
                if audio.format != SampleFormat::S16 {
                    return Err(Error::unsupported("Opus encoder expects s16 PCM"));
                }
                let data = audio.data();
                let mut pcm = Vec::with_capacity(data.len() / 2);
                for c in data.chunks_exact(2) {
                    let s = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0;
                    pcm.push(s);
                }
                let samples_per_ch = pcm.len() / self.channels.max(1);
                // Encode in encoder frame-sized chunks; pad last.
                let mut offset = 0;
                while offset < samples_per_ch {
                    let take = (samples_per_ch - offset).min(self.frame_samples);
                    let mut chunk = vec![0.0f32; self.frame_samples * self.channels];
                    for i in 0..take {
                        for c in 0..self.channels {
                            chunk[i * self.channels + c] = pcm[(offset + i) * self.channels + c];
                        }
                    }
                    let mut packet = vec![0u8; 4000];
                    let len = self
                        .inner
                        .encode(&chunk, self.frame_samples, &mut packet)
                        .map_err(|e| Error::invalid_data(format!("Opus encode: {e:?}")))?;
                    packet.truncate(len);
                    let mut pkt = Packet::new(Buffer::from_vec(packet));
                    pkt.flags.insert(PacketFlags::KEY);
                    self.pending.push_back(pkt);
                    offset += take;
                }
                Ok(())
            }
            Some(_) => Err(Error::invalid_data("Opus encoder expects audio")),
            None => {
                self.eof = true;
                Ok(())
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
