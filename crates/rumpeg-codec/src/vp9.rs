//! VP9 decode via pure-Rust [`rusty_vp9`].

use crate::decoder::Decoder;
use rumpeg_util::{
    CodecId, CodecParams, Error, Frame, Packet, PixelFormat, Result, Timestamp, VideoFrame,
};
use std::collections::VecDeque;

/// VP9 decoder (8-bit 4:2:0 practical path).
pub struct Vp9DecoderCodec {
    inner: rusty_vp9::Vp9Decoder,
    pending: VecDeque<Frame>,
    eof: bool,
}

impl Vp9DecoderCodec {
    /// Open a VP9 decoder.
    pub fn new(_params: &CodecParams) -> Result<Self> {
        Ok(Self {
            inner: rusty_vp9::Vp9Decoder::new(),
            pending: VecDeque::new(),
            eof: false,
        })
    }

    fn pull(&mut self) {
        while let Ok(frame) = self.inner.next_frame() {
            if let Some(vf) = vp9_to_video(frame) {
                self.pending.push_back(Frame::Video(vf));
            }
        }
    }
}

fn vp9_to_video(frame: rusty_vp9::DecodedFrame) -> Option<VideoFrame> {
    if frame.bit_depth != 8 || frame.planes.len() < 3 {
        return None;
    }
    let mut vf = VideoFrame::alloc(PixelFormat::Yuv420p, frame.width, frame.height);
    vf.pts = frame.pts.map(Timestamp::new).unwrap_or(Timestamp::NONE);
    vf.key_frame = true;
    for p in 0..3 {
        let src = &frame.planes[p];
        let stride = frame.strides.get(p).copied().unwrap_or(0);
        let dst = vf.plane_mut(p)?;
        let h = if p == 0 {
            frame.height
        } else {
            frame.height / 2
        } as usize;
        let w = if p == 0 { frame.width } else { frame.width / 2 } as usize;
        let stride = if stride == 0 { w } else { stride };
        if src.len() == dst.len() && stride == w {
            dst.copy_from_slice(src);
        } else {
            for y in 0..h {
                let s = &src[y * stride..y * stride + w.min(stride)];
                dst[y * w..y * w + s.len()].copy_from_slice(s);
            }
        }
    }
    Some(vf)
}

impl Decoder for Vp9DecoderCodec {
    fn codec_id(&self) -> CodecId {
        CodecId::Vp9
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                let pts = if p.pts.is_none() { None } else { Some(p.pts.0) };
                self.inner
                    .push(p.data.as_slice(), pts)
                    .map_err(|e| Error::invalid_data(format!("VP9: {e:?}")))?;
                self.pull();
            }
            None => {
                self.eof = true;
                self.inner.flush();
                self.pull();
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
