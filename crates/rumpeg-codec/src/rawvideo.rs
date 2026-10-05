//! Uncompressed raw video codec (identity encode/decode).

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, CodecSpecific, Error, Frame, Packet, PacketFlags, Result,
    VideoFrame, VideoParams,
};
use std::collections::VecDeque;

fn video_params(params: &CodecParams) -> Result<&VideoParams> {
    match &params.specific {
        CodecSpecific::Video(v) => Ok(v),
        _ => Err(Error::invalid_data(
            "rawvideo codec requires video parameters",
        )),
    }
}

fn frame_byte_size(v: &VideoParams) -> usize {
    let f = VideoFrame::alloc(v.pix_fmt, v.width, v.height);
    f.total_bytes()
}

fn pack_frame(frame: &VideoFrame) -> Buffer {
    if frame.planes.len() == 1 {
        return frame.planes[0].clone();
    }
    let mut out = Vec::with_capacity(frame.total_bytes());
    for plane in &frame.planes {
        out.extend_from_slice(plane.as_slice());
    }
    Buffer::from_vec(out)
}

fn unpack_frame(
    params: &VideoParams,
    data: &Buffer,
    pts: rumpeg_util::Timestamp,
) -> Result<VideoFrame> {
    let mut frame = VideoFrame::alloc(params.pix_fmt, params.width, params.height);
    frame.pts = pts;
    frame.key_frame = true;
    let need = frame.total_bytes();
    if data.len() < need {
        return Err(Error::BufferTooSmall {
            need,
            have: data.len(),
        });
    }
    let mut offset = 0;
    for plane in &mut frame.planes {
        let len = plane.len();
        plane
            .make_mut()
            .copy_from_slice(&data[offset..offset + len]);
        offset += len;
    }
    Ok(frame)
}

/// Raw video decoder.
pub struct RawVideoDecoder {
    params: VideoParams,
    pending: VecDeque<Frame>,
    eof: bool,
}

impl RawVideoDecoder {
    /// Create a rawvideo decoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        Ok(Self {
            params: video_params(params)?.clone(),
            pending: VecDeque::new(),
            eof: false,
        })
    }
}

impl Decoder for RawVideoDecoder {
    fn codec_id(&self) -> CodecId {
        CodecId::RawVideo
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        let Some(packet) = packet else {
            self.eof = true;
            return Ok(());
        };
        let frame = unpack_frame(&self.params, &packet.data, packet.pts)?;
        self.pending.push_back(Frame::Video(frame));
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

/// Raw video encoder.
pub struct RawVideoEncoder {
    params: VideoParams,
    pending: VecDeque<Packet>,
    eof: bool,
    #[allow(dead_code)]
    frame_bytes: usize,
}

impl RawVideoEncoder {
    /// Create a rawvideo encoder.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let video = video_params(params)?.clone();
        let frame_bytes = frame_byte_size(&video);
        Ok(Self {
            params: video,
            pending: VecDeque::new(),
            eof: false,
            frame_bytes,
        })
    }
}

impl Encoder for RawVideoEncoder {
    fn codec_id(&self) -> CodecId {
        CodecId::RawVideo
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data(
                "rawvideo encoder expects a video frame",
            ));
        };
        if video.width != self.params.width || video.height != self.params.height {
            return Err(Error::invalid_data("frame size mismatch"));
        }
        if video.format != self.params.pix_fmt {
            return Err(Error::invalid_data("pixel format mismatch"));
        }
        let mut pkt = Packet::new(pack_frame(video));
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
