//! H.264 decoder/encoder wrappers around the pure-Rust [`rumpeg_h264`] crate.
//!
//! **No Cisco OpenH264 C/C++ FFI** — decoding uses [`rusty_h264_decoder`]
//! (CAVLC + CABAC) via `rumpeg-h264`; encoding uses Rumpeg’s Baseline
//! `I_PCM` / `I_16x16` CAVLC encoder with optional P (SKIP / Intra-refresh).

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, CodecSpecific, Error, Frame, Packet, PacketFlags, PixelFormat,
    Result, Timestamp, VideoFrame, VideoParams,
};
use std::collections::VecDeque;

pub use rumpeg_h264::IntraMode;

fn video_params(params: &CodecParams) -> Result<VideoParams> {
    match &params.specific {
        CodecSpecific::Video(v) => Ok(v.clone()),
        _ => Ok(VideoParams {
            pix_fmt: PixelFormat::Yuv420p,
            width: 0,
            height: 0,
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
        }),
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

fn fps_from_params(params: &CodecParams) -> f64 {
    params
        .video()
        .map(|v| {
            let r = v.frame_rate.as_f64();
            if r > 0.0 {
                r
            } else {
                25.0
            }
        })
        .unwrap_or(25.0)
}

/// Pure-Rust H.264 decoder (CABAC + CAVLC via `rusty_h264-decoder`).
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

/// Pure-Rust H.264 encoder. Defaults to `I_16x16` CAVLC + P when dimensions known;
/// use [`IntraMode::Ipcm`] for lossless-ish Intra.
pub struct H264Encoder {
    inner: rumpeg_h264::Encoder,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl H264Encoder {
    /// Open encoder; prefers CAVLC + Inter for practical re-encode size.
    pub fn new(params: &CodecParams) -> Result<Self> {
        // Default to CAVLC (lossy) for file-size usefulness; I_PCM only when requested.
        Self::with_mode(params, IntraMode::I16x16Cavlc)
    }

    /// Open encoder with an explicit Intra mode.
    pub fn with_mode(params: &CodecParams, mode: IntraMode) -> Result<Self> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let mut inner =
            rumpeg_h264::Encoder::with_mode(video.width, video.height, mode).map_err(map_h264)?;
        if params.quality >= 0 {
            inner.set_qp(params.quality);
        } else if params.bit_rate > 0 {
            inner.set_bitrate(params.bit_rate, fps_from_params(params));
        }
        if params.gop_size > 0 {
            inner.set_gop_size(params.gop_size);
        }
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            eof: false,
        })
    }

    /// Set QP (0..=51).
    pub fn set_qp(&mut self, qp: i32) {
        self.inner.set_qp(qp);
    }

    /// Set CRF-like quality (mapped to QP).
    pub fn set_crf(&mut self, crf: f32) {
        self.inner.set_crf(crf);
    }

    /// Set target bitrate (bits/s) given fps for QP approximation.
    pub fn set_bitrate(&mut self, bit_rate: u64, fps: f64) {
        self.inner.set_bitrate(bit_rate, fps);
    }

    /// Set GOP size (IDR interval).
    pub fn set_gop_size(&mut self, gop: u32) {
        self.inner.set_gop_size(gop);
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
        let (bytes, is_key) = self.inner.encode_access_unit(&yuv).map_err(map_h264)?;
        let mut pkt = Packet::new(Buffer::from_vec(bytes));
        pkt.pts = video.pts;
        pkt.dts = video.pts;
        if is_key {
            pkt.flags.insert(PacketFlags::KEY);
        }
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
        rumpeg_h264::Error::Truncated(m) | rumpeg_h264::Error::Invalid(m) => Error::invalid_data(m),
        rumpeg_h264::Error::Unsupported(m) => Error::unsupported(m),
    }
}
