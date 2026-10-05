//! Encoder trait and context.

use crate::h264::H264Encoder;
use crate::jpeg::JpegEncoderCodec;
use crate::pcm::PcmEncoder;
use crate::rawvideo::RawVideoEncoder;
use rumpeg_util::{CodecId, CodecParams, Error, Frame, Packet, Result};

/// Trait implemented by all encoders.
pub trait Encoder: Send {
    /// Codec this encoder implements.
    fn codec_id(&self) -> CodecId;

    /// Feed a raw frame. `None` flushes the encoder.
    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()>;

    /// Pull an encoded packet.
    fn receive_packet(&mut self) -> Result<Packet>;

    /// Flush.
    fn flush(&mut self) {
        let _ = self.send_frame(None);
    }
}

/// Owned encoder instance.
pub struct EncoderContext {
    inner: Box<dyn Encoder>,
    params: CodecParams,
}

impl EncoderContext {
    /// Open an encoder matching `params.codec_id`.
    pub fn open(params: &CodecParams) -> Result<Self> {
        let inner: Box<dyn Encoder> = match params.codec_id {
            CodecId::PcmS16Le
            | CodecId::PcmS24Le
            | CodecId::PcmS32Le
            | CodecId::PcmF32Le
            | CodecId::PcmU8 => Box::new(PcmEncoder::new(params)?),
            CodecId::RawVideo => Box::new(RawVideoEncoder::new(params)?),
            CodecId::H264 => Box::new(H264Encoder::new(params)?),
            CodecId::Mjpeg => Box::new(JpegEncoderCodec::new(params)?),
            other => return Err(Error::not_found(format!("encoder for {other}"))),
        };
        Ok(Self {
            inner,
            params: params.clone(),
        })
    }

    /// Parameters used to open this encoder.
    pub fn params(&self) -> &CodecParams {
        &self.params
    }

    /// Send a frame for encoding.
    pub fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.inner.send_frame(Some(frame))
    }

    /// Flush the encoder.
    pub fn send_eof(&mut self) -> Result<()> {
        self.inner.send_frame(None)
    }

    /// Receive an encoded packet.
    pub fn receive_packet(&mut self) -> Result<Packet> {
        self.inner.receive_packet()
    }

    /// Encode a single frame into zero or more packets.
    pub fn encode(&mut self, frame: &Frame) -> Result<Vec<Packet>> {
        self.send_frame(frame)?;
        let mut packets = Vec::new();
        loop {
            match self.receive_packet() {
                Ok(pkt) => packets.push(pkt),
                Err(Error::NeedMoreData) | Err(Error::TryAgain) => break,
                Err(Error::Eof) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(packets)
    }
}
