//! JPEG image encoder (MJPEG / mjpeg codec id) via the pure-Rust `image` crate.

use crate::encoder::Encoder;
use image::codecs::jpeg::JpegEncoder;
use image::ExtendedColorType;
use rumpeg_scale::convert_frame;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, Error, Frame, Packet, PacketFlags, PixelFormat, Result,
};
use std::collections::VecDeque;
use std::io::Cursor;

/// JPEG encoder producing one compressed packet per frame.
pub struct JpegEncoderCodec {
    quality: u8,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl JpegEncoderCodec {
    /// Create a JPEG encoder (`quality` 1–100, default 90).
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            quality: 90,
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Encoder for JpegEncoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Mjpeg
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("JPEG encoder expects a video frame"));
        };
        let rgb = if video.format == PixelFormat::Rgb24 {
            video.clone()
        } else {
            convert_frame(video, PixelFormat::Rgb24)?
        };
        let data = rgb
            .plane(0)
            .ok_or_else(|| Error::invalid_data("missing RGB plane"))?;
        let mut out = Cursor::new(Vec::new());
        let mut enc = JpegEncoder::new_with_quality(&mut out, self.quality);
        enc.encode(data, rgb.width, rgb.height, ExtendedColorType::Rgb8)
            .map_err(|e| Error::Other(format!("JPEG encode failed: {e}")))?;
        let mut pkt = Packet::new(Buffer::from_vec(out.into_inner()));
        pkt.pts = video.pts;
        pkt.dts = video.pts;
        pkt.flags.insert(PacketFlags::KEY);
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
