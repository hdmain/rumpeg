//! H.264 decoder/encoder wrappers around the pure-Rust [`rumpeg_h264`] crate.
//!
//! **No Cisco OpenH264 C/C++ FFI** — decoding/encoding uses Rumpeg's own
//! Baseline `I_PCM` implementation.

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, CodecSpecific, Error, Frame, Packet, PacketFlags, PixelFormat,
    Result, Timestamp, VideoFrame, VideoParams,
};
use std::collections::VecDeque;

fn video_params(params: &CodecParams) -> Result<VideoParams> {
    match &params.specific {
        CodecSpecific::Video(v) => Ok(v.clone()),
        _ => {
            // Dimensions may be unknown until SPS; allow zeros.
            Ok(VideoParams {
                pix_fmt: PixelFormat::Yuv420p,
                width: 0,
                height: 0,
                frame_rate: Default::default(),
                sample_aspect_ratio: Default::default(),
            })
        }
    }
}

fn yuv_to_video_frame(yuv: rumpeg_h264::Yuv420Planar, pts: Timestamp) -> VideoFrame {
    let mut frame = VideoFrame::alloc(PixelFormat::Yuv420p, yuv.width, yuv.height);
    frame.pts = pts;
    frame.key_frame = true;
    frame.plane_mut(0).unwrap().copy_from_slice(&yuv.y);
    frame.plane_mut(1).unwrap().copy_from_slice(&yuv.u);
    frame.plane_mut(2).unwrap().copy_from_slice(&yuv.v);
    frame
}

fn video_frame_to_yuv(frame: &VideoFrame) -> Result<rumpeg_h264::Yuv420Planar> {
    if frame.format != PixelFormat::Yuv420p {
        return Err(Error::invalid_data(
            "H.264 encoder expects yuv420p (convert first)",
        ));
    }
    let y = frame
        .plane(0)
        .ok_or_else(|| Error::invalid_data("missing Y"))?
        .to_vec();
    let u = frame
        .plane(1)
        .ok_or_else(|| Error::invalid_data("missing U"))?
        .to_vec();
    let v = frame
        .plane(2)
        .ok_or_else(|| Error::invalid_data("missing V"))?
        .to_vec();
    Ok(rumpeg_h264::Yuv420Planar {
        width: frame.width,
        height: frame.height,
        y,
        u,
        v,
    })
}

/// Pure-Rust H.264 decoder (`I_PCM` IDR subset).
pub struct H264Decoder {
    inner: rumpeg_h264::Decoder,
    pending: VecDeque<Frame>,
    last_pts: Timestamp,
    eof: bool,
    /// True when packets are AVCC (MP4) rather than Annex-B.
    avcc: bool,
}

impl H264Decoder {
    /// Open from codec parameters (`extradata` may be `avcC`).
    pub fn new(params: &CodecParams) -> Result<Self> {
        let _ = video_params(params)?;
        let (inner, avcc) = if !params.extradata.is_empty() {
            (
                rumpeg_h264::Decoder::with_avcc(&params.extradata).map_err(map_h264)?,
                true,
            )
        } else {
            (rumpeg_h264::Decoder::new(), false)
        };
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            last_pts: Timestamp::NONE,
            eof: false,
            avcc,
        })
    }
}

impl Decoder for H264Decoder {
    fn codec_id(&self) -> CodecId {
        CodecId::H264
    }

    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        let Some(packet) = packet else {
            self.eof = true;
            self.inner.flush();
            return Ok(());
        };
        self.last_pts = packet.pts;
        if self.avcc {
            self.inner
                .decode_avcc_sample(packet.data.as_slice())
                .map_err(map_h264)?;
        } else {
            self.inner
                .decode_annexb(packet.data.as_slice())
                .map_err(map_h264)?;
        }
        while let Some(yuv) = self.inner.receive().map_err(map_h264)? {
            self.pending
                .push_back(Frame::Video(yuv_to_video_frame(yuv, packet.pts)));
        }
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

/// Pure-Rust H.264 encoder (`I_PCM` IDR).
pub struct H264Encoder {
    inner: rumpeg_h264::Encoder,
    pending: VecDeque<Packet>,
    eof: bool,
    /// Emit Annex-B access units including SPS/PPS each frame (simple for .h264).
    annexb_with_params: bool,
}

impl H264Encoder {
    /// Open encoder; requires known video width/height.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let inner =
            rumpeg_h264::Encoder::new(video.width, video.height).map_err(map_h264)?;
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            eof: false,
            annexb_with_params: true,
        })
    }

    /// `avcC` extradata for MP4 muxing.
    pub fn extradata(&self) -> Result<Vec<u8>> {
        self.inner.avcc_extradata().map_err(map_h264)
    }
}

impl Encoder for H264Encoder {
    fn codec_id(&self) -> CodecId {
        CodecId::H264
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("H.264 encoder expects video"));
        };
        let yuv = video_frame_to_yuv(video)?;
        let bytes = if self.annexb_with_params {
            self.inner.encode_annexb(&yuv).map_err(map_h264)?
        } else {
            let nal = self.inner.encode_idr_nal(&yuv).map_err(map_h264)?;
            rumpeg_h264::annexb_to_avcc_sample(&{
                let mut a = Vec::new();
                a.extend_from_slice(&[0, 0, 0, 1]);
                a.extend_from_slice(&nal);
                a
            })
        };
        let mut pkt = Packet::new(Buffer::from_vec(bytes));
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

fn map_h264(err: rumpeg_h264::Error) -> Error {
    match err {
        rumpeg_h264::Error::Truncated(m) | rumpeg_h264::Error::Invalid(m) => {
            Error::invalid_data(m)
        }
        rumpeg_h264::Error::Unsupported(m) => Error::unsupported(m),
    }
}
