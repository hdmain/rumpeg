//! AAC decode + LC encode via pure-Rust [`rusty_aac`].

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, ChannelLayout, CodecId, CodecParams, CodecSpecific, Error,
    Frame, Packet, PacketFlags, Result, SampleFormat, Timestamp,
};
use std::collections::VecDeque;

fn audio_params(params: &CodecParams) -> Result<AudioParams> {
    match &params.specific {
        CodecSpecific::Audio(a) => Ok(a.clone()),
        _ => Ok(AudioParams::default()),
    }
}

fn decoded_to_frame(decoded: rusty_aac::DecodedAudio, pts: Timestamp) -> Frame {
    let ch = decoded.channels.max(1) as usize;
    let samples_per_ch = decoded.samples.len() / ch;
    let mut frame = AudioFrame::alloc_interleaved(
        SampleFormat::S16,
        decoded.sample_rate,
        ChannelLayout::new(decoded.channels),
        samples_per_ch as u32,
    );
    frame.pts = pts;
    let plane = frame.data_mut();
    for (i, &s) in decoded.samples.iter().enumerate() {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        let o = i * 2;
        if o + 1 < plane.len() {
            plane[o..o + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    Frame::Audio(frame)
}

/// AAC decoder (ADTS or raw with AudioSpecificConfig in extradata).
pub struct AacDecoderCodec {
    inner: rusty_aac::AacDecoder,
    pending: VecDeque<Frame>,
    last_pts: Timestamp,
    eof: bool,
}

impl AacDecoderCodec {
    /// Open an AAC decoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let inner = if !params.extradata.is_empty() {
            rusty_aac::AacDecoder::with_config_bytes(&params.extradata)
                .map_err(|e| Error::invalid_data(format!("AAC ASC: {e:?}")))?
        } else {
            rusty_aac::AacDecoder::new()
        };
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            last_pts: Timestamp::NONE,
            eof: false,
        })
    }
}

impl Decoder for AacDecoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Aac
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                self.last_pts = p.pts;
                let data = p.data.as_slice();
                let payload = if rusty_aac::is_adts(data) {
                    let hdr = if data.len() > 1 && (data[1] & 1) != 0 {
                        7
                    } else {
                        9
                    };
                    if data.len() > hdr {
                        &data[hdr..]
                    } else {
                        data
                    }
                } else {
                    data
                };
                let pts = if p.pts.is_none() { None } else { Some(p.pts.0) };
                match self.inner.decode(payload, pts) {
                    Ok(audio) => self
                        .pending
                        .push_back(decoded_to_frame(audio, self.last_pts)),
                    Err(e) => {
                        let msg = format!("{e:?}");
                        if msg.contains("Again") {
                            return Ok(());
                        }
                        return Err(Error::invalid_data(format!("AAC decode: {e:?}")));
                    }
                }
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

/// AAC-LC encoder (raw AUs; callers may wrap ADTS for file output).
pub struct AacEncoderCodec {
    inner: rusty_aac::AacEncoder,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl AacEncoderCodec {
    /// Open an AAC-LC encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let _ = audio_params(params)?;
        let mut cfg = rusty_aac::AacEncoderConfig::default();
        if params.bit_rate > 0 {
            cfg.bitrate_bps = params.bit_rate as u32;
        }
        Ok(Self {
            inner: rusty_aac::AacEncoder::new(cfg),
            pending: VecDeque::new(),
            eof: false,
        })
    }

    fn drain(&mut self) {
        loop {
            match self.inner.next_packet() {
                Ok(ep) => {
                    let mut pkt = Packet::new(Buffer::from_vec(ep.data));
                    pkt.pts = Timestamp::new(ep.pts);
                    pkt.flags.insert(PacketFlags::KEY);
                    self.pending.push_back(pkt);
                }
                Err(rusty_aac::Error::Again) => break,
                Err(rusty_aac::Error::Eof) => break,
                Err(_) => break,
            }
        }
    }
}

impl Encoder for AacEncoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Aac
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        match frame {
            Some(Frame::Audio(audio)) => {
                let f32s: Vec<f32> = if audio.format == SampleFormat::F32 {
                    let data = audio.data();
                    data.chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect()
                } else if audio.format == SampleFormat::S16 {
                    let data = audio.data();
                    data.chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                        .collect()
                } else {
                    return Err(Error::unsupported("AAC encoder expects s16 or f32 PCM"));
                };
                self.inner
                    .push_pcm(&f32s, audio.layout.channels, audio.sample_rate)
                    .map_err(|e| Error::invalid_data(format!("AAC encode: {e:?}")))?;
                self.drain();
                Ok(())
            }
            Some(_) => Err(Error::invalid_data("AAC encoder expects audio")),
            None => {
                self.eof = true;
                self.inner.finish();
                self.drain();
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
