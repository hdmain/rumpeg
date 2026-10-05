//! Linear PCM codecs — essentially zero-copy pack/unpack between packets and frames.

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    AudioFrame, AudioParams, Buffer, CodecId, CodecParams, CodecSpecific, Error, Frame, Packet,
    Result, SampleFormat,
};
use std::collections::VecDeque;

fn sample_format_for(codec: CodecId) -> Result<SampleFormat> {
    Ok(match codec {
        CodecId::PcmU8 => SampleFormat::U8,
        CodecId::PcmS16Le => SampleFormat::S16,
        CodecId::PcmS24Le => SampleFormat::S24,
        CodecId::PcmS32Le => SampleFormat::S32,
        CodecId::PcmF32Le => SampleFormat::F32,
        other => return Err(Error::unsupported(format!("not a PCM codec: {other}"))),
    })
}

fn audio_params(params: &CodecParams) -> Result<&AudioParams> {
    match &params.specific {
        CodecSpecific::Audio(a) => Ok(a),
        _ => Err(Error::invalid_data("PCM codec requires audio parameters")),
    }
}

/// PCM decoder: packet bytes become an interleaved audio frame.
pub struct PcmDecoder {
    codec_id: CodecId,
    params: AudioParams,
    pending: VecDeque<Frame>,
    eof: bool,
}

impl PcmDecoder {
    /// Create a PCM decoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let audio = audio_params(params)?.clone();
        let expected = sample_format_for(params.codec_id)?;
        if audio.sample_fmt != expected {
            return Err(Error::invalid_data(format!(
                "sample format {:?} does not match codec {}",
                audio.sample_fmt, params.codec_id
            )));
        }
        Ok(Self {
            codec_id: params.codec_id,
            params: audio,
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Decoder for PcmDecoder {
    fn codec_id(&self) -> CodecId {
        self.codec_id
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        let Some(packet) = packet else {
            self.eof = true;
            return Ok(());
        };
        let bpf = self.params.bytes_per_frame();
        if bpf == 0 || packet.data.len() % bpf != 0 {
            return Err(Error::invalid_data(format!(
                "PCM packet size {} not aligned to frame size {}",
                packet.data.len(),
                bpf
            )));
        }
        let samples = (packet.data.len() / bpf) as u32;
        let mut frame = AudioFrame::alloc_interleaved(
            self.params.sample_fmt,
            self.params.sample_rate,
            self.params.layout,
            samples,
        );
        frame.pts = packet.pts;
        // Zero-copy share when possible: reuse packet buffer directly.
        frame.planes = vec![packet.data.clone()];
        self.pending.push_back(Frame::Audio(frame));
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Frame> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        if self.eof {
            Err(Error::Eof)
        } else {
            Err(Error::NeedMoreData)
        }
    }
}

/// PCM encoder: audio frame bytes become a packet.
pub struct PcmEncoder {
    codec_id: CodecId,
    params: AudioParams,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl PcmEncoder {
    /// Create a PCM encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let audio = audio_params(params)?.clone();
        let expected = sample_format_for(params.codec_id)?;
        if audio.sample_fmt != expected {
            return Err(Error::invalid_data(format!(
                "sample format {:?} does not match codec {}",
                audio.sample_fmt, params.codec_id
            )));
        }
        Ok(Self {
            codec_id: params.codec_id,
            params: audio,
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Encoder for PcmEncoder {
    fn codec_id(&self) -> CodecId {
        self.codec_id
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        let Frame::Audio(audio) = frame else {
            return Err(Error::invalid_data("PCM encoder expects an audio frame"));
        };
        if audio.format != self.params.sample_fmt {
            return Err(Error::invalid_data("sample format mismatch"));
        }
        let data = if audio.planes.len() == 1 {
            audio.planes[0].clone()
        } else {
            // Flatten planar into interleaved for PCM packetization.
            Buffer::from_vec(planar_to_interleaved(audio)?)
        };
        let mut pkt = Packet::new(data);
        pkt.pts = audio.pts;
        pkt.dts = audio.pts;
        pkt.duration = audio.samples as i64;
        pkt.flags.insert(rumpeg_util::PacketFlags::KEY);
        self.pending.push_back(pkt);
        Ok(())
    }

    fn receive_packet(&mut self) -> Result<Packet> {
        if let Some(pkt) = self.pending.pop_front() {
            return Ok(pkt);
        }
        if self.eof {
            Err(Error::Eof)
        } else {
            Err(Error::NeedMoreData)
        }
    }
}

fn planar_to_interleaved(audio: &AudioFrame) -> Result<Vec<u8>> {
    let bps = audio.format.bytes_per_sample();
    let ch = audio.layout.channels as usize;
    let samples = audio.samples as usize;
    if audio.planes.len() != ch {
        return Err(Error::invalid_data("planar channel count mismatch"));
    }
    let mut out = vec![0u8; bps * ch * samples];
    for s in 0..samples {
        for c in 0..ch {
            let src = &audio.planes[c].as_slice()[s * bps..(s + 1) * bps];
            let dst_off = (s * ch + c) * bps;
            out[dst_off..dst_off + bps].copy_from_slice(src);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumpeg_util::{ChannelLayout, Timestamp};

    #[test]
    fn pcm_roundtrip() {
        let params = CodecParams::audio_codec(
            CodecId::PcmS16Le,
            AudioParams {
                sample_fmt: SampleFormat::S16,
                sample_rate: 48_000,
                layout: ChannelLayout::STEREO,
                frame_size: 0,
            },
        );
        let mut enc = PcmEncoder::new(&params).unwrap();
        let mut dec = PcmDecoder::new(&params).unwrap();

        let mut frame =
            AudioFrame::alloc_interleaved(SampleFormat::S16, 48_000, ChannelLayout::STEREO, 128);
        frame.pts = Timestamp::new(0);
        for (i, chunk) in frame.data_mut().chunks_exact_mut(2).enumerate() {
            let v = (i as i16).wrapping_mul(13);
            chunk.copy_from_slice(&v.to_le_bytes());
        }

        enc.send_frame(Some(&Frame::Audio(frame.clone()))).unwrap();
        let pkt = enc.receive_packet().unwrap();
        dec.send_packet(Some(&pkt)).unwrap();
        let out = dec.receive_frame().unwrap();
        match out {
            Frame::Audio(a) => assert_eq!(a.data(), frame.data()),
            _ => panic!("expected audio"),
        }
    }
}
