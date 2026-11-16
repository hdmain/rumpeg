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
    /// Interleaved f32 samples carried across send_frame calls so partial
    /// frames accumulate instead of each call zero-padding its own tail.
    pending_pcm: Vec<f32>,
    eof: bool,
}

/// Smallest legal Opus frame length (in samples/channel) that can hold `n`
/// samples at `rate`: 2.5/5/10/20/40/60/120 ms.
fn min_legal_frame(n: usize, rate: usize) -> usize {
    let sizes = [
        rate / 400,
        rate / 200,
        rate / 100,
        rate / 50,
        rate / 25,
        rate * 3 / 50,
        rate * 3 / 25,
    ];
    sizes.into_iter().find(|&s| s >= n).unwrap_or(rate * 3 / 25)
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
            pending_pcm: Vec::new(),
            eof: false,
        })
    }
}

impl OpusEncoderCodec {
    /// Encode exactly `frame_len` samples/channel from the front of
    /// `pending_pcm` into one Opus packet.
    fn encode_frame(&mut self, frame_len: usize) -> Result<()> {
        let ch = self.channels.max(1);
        let take = frame_len * ch;
        let chunk: Vec<f32> = self
            .pending_pcm
            .drain(..take.min(self.pending_pcm.len()))
            .collect();
        let mut padded = chunk;
        padded.resize(frame_len * ch, 0.0);
        let mut packet = vec![0u8; 4000];
        let len = self
            .inner
            .encode(&padded, frame_len, &mut packet)
            .map_err(|e| Error::invalid_data(format!("Opus encode: {e:?}")))?;
        packet.truncate(len);
        let mut pkt = Packet::new(Buffer::from_vec(packet));
        pkt.flags.insert(PacketFlags::KEY);
        self.pending.push_back(pkt);
        Ok(())
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
                for c in data.chunks_exact(2) {
                    let s = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0;
                    self.pending_pcm.push(s);
                }
                // Emit full frames only; keep any remainder for the next call
                // so no silence is inserted in the middle of the stream.
                let per_frame = self.frame_samples * self.channels.max(1);
                while self.pending_pcm.len() >= per_frame {
                    self.encode_frame(self.frame_samples)?;
                }
                Ok(())
            }
            Some(_) => Err(Error::invalid_data("Opus encoder expects audio")),
            None => {
                // Flush the tail as ONE frame padded to the smallest legal Opus
                // frame length, instead of dropping the residual samples.
                if !self.pending_pcm.is_empty() {
                    let samples_per_ch = self.pending_pcm.len() / self.channels.max(1);
                    let rate = self.frame_samples * 50; // frame_samples = rate/50
                    let frame_len = min_legal_frame(samples_per_ch, rate);
                    self.encode_frame(frame_len)?;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CodecParams {
        CodecParams::audio_codec(
            CodecId::Opus,
            AudioParams {
                sample_fmt: SampleFormat::S16,
                sample_rate: 48_000,
                layout: ChannelLayout::MONO,
                frame_size: 0,
            },
        )
    }

    fn audio_frame(samples: usize) -> AudioFrame {
        let mut f = AudioFrame::alloc_interleaved(
            SampleFormat::S16,
            48_000,
            ChannelLayout::MONO,
            samples as u32,
        );
        for (i, c) in f.data_mut().chunks_exact_mut(2).enumerate() {
            let v = ((i % 100) as f32 * 100.0) as i16;
            c.copy_from_slice(&v.to_le_bytes());
        }
        f
    }

    /// Partial frames must accumulate across send_frame calls instead of each
    /// call emitting a zero-padded 20ms frame (which inserted mid-stream silence).
    #[test]
    fn opus_partial_frames_accumulate() {
        let mut enc = OpusEncoderCodec::new(&params()).unwrap();
        // Half a 20ms frame (480 samples @48k) per call; two calls make one frame.
        enc.send_frame(Some(&Frame::Audio(audio_frame(480))))
            .unwrap();
        assert!(matches!(enc.receive_packet(), Err(Error::NeedMoreData)));
        enc.send_frame(Some(&Frame::Audio(audio_frame(480))))
            .unwrap();
        assert!(enc.receive_packet().is_ok());
    }

    /// A short tail must be flushed as one packet, not dropped.
    #[test]
    fn opus_flush_emits_tail() {
        let mut enc = OpusEncoderCodec::new(&params()).unwrap();
        enc.send_frame(Some(&Frame::Audio(audio_frame(200))))
            .unwrap(); // < 960
        enc.send_frame(None).unwrap();
        assert!(enc.receive_packet().is_ok(), "tail frame must be emitted");
        assert!(matches!(enc.receive_packet(), Err(Error::Eof)));
    }

    #[test]
    fn min_legal_frame_rounds_up() {
        assert_eq!(min_legal_frame(1, 48_000), 120);
        assert_eq!(min_legal_frame(200, 48_000), 240);
        assert_eq!(min_legal_frame(960, 48_000), 960);
    }
}
