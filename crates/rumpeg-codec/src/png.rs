//! PNG image encoder via the pure-Rust `image` crate.

use crate::encoder::Encoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use rumpeg_scale::convert_frame;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, Error, Frame, Packet, PacketFlags, PixelFormat, Result,
};
use std::collections::VecDeque;
use std::io::Cursor;

/// PNG encoder producing one compressed packet per frame.
pub struct PngEncoderCodec {
    pending: VecDeque<Packet>,
    eof: bool,
}

impl PngEncoderCodec {
    /// Create a PNG encoder.
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Encoder for PngEncoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Png
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("PNG encoder expects a video frame"));
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
        let enc = PngEncoder::new(&mut out);
        enc.write_image(data, rgb.width, rgb.height, ExtendedColorType::Rgb8)
            .map_err(|e| Error::Other(format!("PNG encode failed: {e}")))?;
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
