//! Decoder trait and context.

use crate::h264::H264Decoder;
use crate::pcm::PcmDecoder;
use crate::rawvideo::RawVideoDecoder;
use rumpeg_util::{CodecId, CodecParams, Error, Frame, Packet, Result};

/// Trait implemented by all decoders.
pub trait Decoder: Send {
    /// Codec this decoder implements.
    fn codec_id(&self) -> CodecId;

    /// Feed a compressed packet. `None` flushes the decoder.
    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()>;

    /// Pull a decoded frame, or [`Error::NeedMoreData`] / [`Error::Eof`].
    fn receive_frame(&mut self) -> Result<Frame>;

    /// Flush internal state.
    fn flush(&mut self) {
        let _ = self.send_packet(None);
    }
}

/// Owned decoder instance bound to stream parameters.
pub struct DecoderContext {
    inner: Box<dyn Decoder>,
    params: CodecParams,
}

impl DecoderContext {
    /// Open a decoder matching `params.codec_id`.
    pub fn open(params: &CodecParams) -> Result<Self> {
        let inner: Box<dyn Decoder> = match params.codec_id {
            CodecId::PcmS16Le
            | CodecId::PcmS24Le
            | CodecId::PcmS32Le
            | CodecId::PcmF32Le
            | CodecId::PcmU8 => Box::new(PcmDecoder::new(params)?),
            CodecId::RawVideo => Box::new(RawVideoDecoder::new(params)?),
            CodecId::H264 => Box::new(H264Decoder::new(params)?),
            other => return Err(Error::not_found(format!("decoder for {other}"))),
        };
        Ok(Self {
            inner,
            params: params.clone(),
        })
    }

    /// Parameters used to open this decoder.
    pub fn params(&self) -> &CodecParams {
        &self.params
    }

    /// Send a packet for decoding.
    pub fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        self.inner.send_packet(Some(packet))
    }

    /// Signal end of stream / flush.
    pub fn send_eof(&mut self) -> Result<()> {
        self.inner.send_packet(None)
    }

    /// Receive a decoded frame.
    pub fn receive_frame(&mut self) -> Result<Frame> {
        self.inner.receive_frame()
    }

    /// Decode all frames currently available after sending `packet`.
    pub fn decode(&mut self, packet: &Packet) -> Result<Vec<Frame>> {
        self.send_packet(packet)?;
        let mut frames = Vec::new();
        loop {
            match self.receive_frame() {
                Ok(frame) => frames.push(frame),
                Err(Error::NeedMoreData) | Err(Error::TryAgain) => break,
                Err(Error::Eof) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(frames)
    }
}
