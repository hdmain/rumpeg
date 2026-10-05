//! MP3 decode (+ optional encode) via pure-Rust [`rusty_mp3`].

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

fn f32_to_s16_frame(decoded: rusty_mp3::DecodedAudio, pts: Timestamp) -> Frame {
    let samples_per_ch = decoded.samples.len() / decoded.channels.max(1) as usize;
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

/// MP3 decoder.
pub struct Mp3DecoderCodec {
    inner: rusty_mp3::Mp3Decoder,
    pending: VecDeque<Frame>,
    last_pts: Timestamp,
    eof: bool,
}

impl Mp3DecoderCodec {
    /// Open an MP3 decoder.
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            inner: rusty_mp3::Mp3Decoder::new(),
            pending: VecDeque::new(),
            last_pts: Timestamp::NONE,
            eof: false,
        })
    }

    fn drain(&mut self) {
        loop {
            match self.inner.next_frame() {
                Ok(audio) => self
                    .pending
                    .push_back(f32_to_s16_frame(audio, self.last_pts)),
                Err(rusty_mp3::Error::Again) => break,
                Err(rusty_mp3::Error::Eof) => break,
                Err(_) => break,
            }
        }
    }
}

impl Decoder for Mp3DecoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Mp3
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                self.last_pts = p.pts;
                self.inner.push(p.data.as_slice());
                self.drain();
            }
            None => {
                self.eof = true;
                self.inner.flush();
                self.drain();
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

/// MP3 encoder (CBR via rusty_mp3).
pub struct Mp3EncoderCodec {
    inner: rusty_mp3::Mp3Encoder,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl Mp3EncoderCodec {
    /// Open an MP3 encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let _ = audio_params(params)?;
        let kbps = if params.bit_rate > 0 {
            (params.bit_rate / 1000).max(32) as u32
        } else {
            128
        };
        let config = rusty_mp3::Mp3EncoderConfig {
            bitrate_kbps: kbps,
            vbr_quality: None,
        };
        Ok(Self {
            inner: rusty_mp3::Mp3Encoder::new(config),
            pending: VecDeque::new(),
            eof: false,
        })
    }

    fn drain(&mut self) {
        loop {
            match self.inner.next_packet() {
                Ok(bytes) => {
                    let mut pkt = Packet::new(Buffer::from_vec(bytes));
                    pkt.flags.insert(PacketFlags::KEY);
                    self.pending.push_back(pkt);
                }
                Err(rusty_mp3::Error::Again) => break,
                Err(rusty_mp3::Error::Eof) => break,
                Err(_) => break,
            }
        }
    }
}

impl Encoder for Mp3EncoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Mp3
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        match frame {
            Some(Frame::Audio(audio)) => {
                if audio.format != SampleFormat::S16 {
                    return Err(Error::unsupported("MP3 encoder expects s16 PCM"));
                }
                let data = audio.data();
                self.inner
                    .push_pcm_s16le(data, audio.layout.channels, audio.sample_rate)
                    .map_err(|e| Error::invalid_data(format!("MP3 encode: {e:?}")))?;
                self.drain();
                Ok(())
            }
            Some(_) => Err(Error::invalid_data("MP3 encoder expects audio")),
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
