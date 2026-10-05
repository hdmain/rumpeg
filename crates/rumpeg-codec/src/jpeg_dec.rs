//! JPEG image decode (input `.jpg` → RGB frames) via the pure-Rust `image` crate.

use crate::decoder::Decoder;
use rumpeg_util::{CodecId, CodecParams, Error, Frame, Packet, PixelFormat, Result, VideoFrame};
use std::collections::VecDeque;

/// JPEG / MJPEG decoder — one compressed image packet → one RGB24 frame.
pub struct JpegDecoderCodec {
    pending: VecDeque<Frame>,
    eof: bool,
}

impl JpegDecoderCodec {
    /// Open a JPEG decoder.
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Decoder for JpegDecoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Mjpeg
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                let img = image::load_from_memory(p.data.as_slice())
                    .map_err(|e| Error::invalid_data(format!("JPEG decode: {e}")))?
                    .to_rgb8();
                let (w, h) = img.dimensions();
                let mut frame = VideoFrame::alloc(PixelFormat::Rgb24, w, h);
                frame.pts = p.pts;
                frame.key_frame = true;
                frame
                    .plane_mut(0)
                    .ok_or_else(|| Error::invalid_data("missing plane"))?
                    .copy_from_slice(img.as_raw());
                self.pending.push_back(Frame::Video(frame));
                Ok(())
            }
            None => {
                self.eof = true;
                Ok(())
            }
        }
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
