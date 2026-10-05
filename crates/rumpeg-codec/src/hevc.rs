//! HEVC / H.265 decode via pure-Rust [`rusty_h265`].

use crate::decoder::Decoder;
use rumpeg_util::{
    CodecId, CodecParams, Error, Frame, Packet, PixelFormat, Result, Timestamp, VideoFrame,
};
use std::collections::VecDeque;

/// HEVC decoder (Progressive 8-bit 4:2:0 Main / Main Still when upstream allows).
pub struct HevcDecoder {
    inner: rusty_h265::Decoder,
    pending: VecDeque<Frame>,
    eof: bool,
}

impl HevcDecoder {
    /// Open an HEVC decoder. `extradata` may contain hvcC or Annex-B VPS/SPS/PPS.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let mut inner = rusty_h265::Decoder::new();
        if !params.extradata.is_empty() {
            // Feed parameter sets; ignore Again.
            let _ = inner.push_annexb(&params.extradata, None);
        }
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            eof: false,
        })
    }

    fn pull(&mut self) {
        loop {
            match self.inner.next_frame() {
                Ok(frame) => {
                    if let Some(vf) = hevc_frame_to_video(frame) {
                        self.pending.push_back(Frame::Video(vf));
                    }
                }
                Err(rusty_h265::Error::Again) => break,
                Err(rusty_h265::Error::Eof) => break,
                Err(_) => break,
            }
        }
    }
}

fn hevc_frame_to_video(frame: rusty_h265::Frame) -> Option<VideoFrame> {
    if frame.bit_depth() > 8 {
        // Practical Progressive 8-bit path only for now.
        return None;
    }
    let w = frame.width as u32;
    let h = frame.height as u32;
    let mut yuv = Vec::new();
    frame.write_yuv(&mut yuv);
    let y_size = (w * h) as usize;
    let uv_size = ((w / 2) * (h / 2)) as usize;
    if yuv.len() < y_size + 2 * uv_size {
        return None;
    }
    let mut vf = VideoFrame::alloc(PixelFormat::Yuv420p, w, h);
    vf.pts = frame.pts.map(Timestamp::new).unwrap_or(Timestamp::NONE);
    vf.key_frame = true;
    vf.plane_mut(0)?.copy_from_slice(&yuv[..y_size]);
    vf.plane_mut(1)?
        .copy_from_slice(&yuv[y_size..y_size + uv_size]);
    vf.plane_mut(2)?
        .copy_from_slice(&yuv[y_size + uv_size..y_size + 2 * uv_size]);
    Some(vf)
}

impl Decoder for HevcDecoder {
    fn codec_id(&self) -> CodecId {
        CodecId::Hevc
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                let pts = if p.pts.is_none() { None } else { Some(p.pts.0) };
                self.inner
                    .push_annexb(p.data.as_slice(), pts)
                    .map_err(|e| Error::invalid_data(format!("HEVC: {e:?}")))?;
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
