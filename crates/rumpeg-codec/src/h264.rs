//! H.264 decoder/encoder wrappers.
//!
//! **Decode** uses [`rumpeg_h264`] → `rusty_h264-decoder` (CABAC + CAVLC), with
//! **no Cisco OpenH264 C/C++ FFI**.
//!
//! **Encode backends** (select via [`CodecParams::encoder_name`] / CLI `-c:v`):
//! - **default / `h264`**: [`rusty_h264-encoder`](https://crates.io/crates/rusty_h264-encoder)
//!   — pure-Rust ME, CABAC, ABR, portable SIMD
//! - **`native`**: Rumpeg Baseline Intra + SKIP/Intra-refresh P (debug / tiny deps)
//! - **`libx264`**: system libx264 (`encode-x264` feature)
//! - **`h264_nvenc`**: NVIDIA NVENC (`encode-nvenc` feature, Windows)

use crate::decoder::Decoder;
use crate::encoder::Encoder;
use rumpeg_util::{
    Buffer, CodecId, CodecParams, CodecSpecific, Error, Frame, Packet, PacketFlags, PixelFormat,
    Result, Timestamp, VideoFrame, VideoParams,
};
use std::collections::VecDeque;

pub use rumpeg_h264::IntraMode;

#[cfg(all(feature = "encode-nvenc", windows))]
#[path = "h264_nvenc.rs"]
mod h264_nvenc;
#[cfg(feature = "encode-x264")]
#[path = "h264_x264.rs"]
mod h264_x264;

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

fn even_dim(n: u32) -> usize {
    let n = n.max(2) as usize;
    n + (n % 2)
}

/// Build `avcC` from Annex-B SPS/PPS NALs without a full High-profile SPS parse.
pub(crate) fn avcc_from_sps_pps_bytes(sps: &[u8], pps: &[u8]) -> Result<Vec<u8>> {
    if sps.len() < 4 || pps.is_empty() {
        return Err(Error::invalid_data("missing SPS/PPS for avcC"));
    }
    let cfg = rumpeg_h264::AvcDecoderConfig {
        profile_idc: sps[1],
        profile_compat: sps[2],
        level_idc: sps[3],
        length_size_minus_one: 3,
        sps_list: vec![sps.to_vec()],
        pps_list: vec![pps.to_vec()],
    };
    Ok(cfg.to_avcc())
}

pub(crate) fn avcc_from_annexb(data: &[u8]) -> Result<Vec<u8>> {
    let nals = rumpeg_h264::extract_annexb_nals(data);
    let sps = nals
        .iter()
        .find(|n| rumpeg_h264::nal_unit_type(n).ok() == Some(7))
        .ok_or_else(|| Error::invalid_data("SPS not found in encoder headers"))?;
    let pps = nals
        .iter()
        .find(|n| rumpeg_h264::nal_unit_type(n).ok() == Some(8))
        .ok_or_else(|| Error::invalid_data("PPS not found in encoder headers"))?;
    avcc_from_sps_pps_bytes(sps, pps)
}

pub(crate) fn annexb_is_keyframe(data: &[u8]) -> bool {
    rumpeg_h264::extract_annexb_nals(data)
        .iter()
        .any(|n| rumpeg_h264::nal_unit_type(n).ok() == Some(rumpeg_h264::NAL_IDR))
}

fn split_annexb_aus(data: &[u8]) -> Vec<(Vec<u8>, bool)> {
    if data.is_empty() {
        return Vec::new();
    }
    let nals = rumpeg_h264::extract_annexb_nals(data);
    if nals.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut is_key = false;
    let mut cur_has_vcl = false;
    for nal in nals {
        let ty = rumpeg_h264::nal_unit_type(nal).unwrap_or(0);
        let is_vcl = matches!(ty, 1 | 5);
        // Start a new AU only when the current one already has a VCL NAL.
        if is_vcl && cur_has_vcl {
            out.push((std::mem::take(&mut cur), is_key));
            is_key = false;
            cur_has_vcl = false;
        }
        cur.extend_from_slice(&[0, 0, 0, 1]);
        cur.extend_from_slice(nal);
        if is_vcl {
            cur_has_vcl = true;
        }
        if ty == rumpeg_h264::NAL_IDR {
            is_key = true;
        }
    }
    if !cur.is_empty() {
        out.push((cur, is_key));
    }
    out
}

fn video_frame_to_rusty(frame: &VideoFrame) -> Result<rusty_h264_common::YuvFrame> {
    let yuv = video_frame_to_yuv(frame)?;
    let w = even_dim(yuv.width);
    let h = even_dim(yuv.height);
    if w != yuv.width as usize || h != yuv.height as usize {
        // Pad to even dims expected by rusty_h264.
        let mut y = vec![0u8; w * h];
        let mut u = vec![128u8; (w / 2) * (h / 2)];
        let mut v = vec![128u8; (w / 2) * (h / 2)];
        for row in 0..yuv.height as usize {
            let src = row * yuv.width as usize;
            let dst = row * w;
            y[dst..dst + yuv.width as usize]
                .copy_from_slice(&yuv.y[src..src + yuv.width as usize]);
        }
        let cw = yuv.width as usize / 2;
        let ch = yuv.height as usize / 2;
        for row in 0..ch {
            let src = row * cw;
            let dst = row * (w / 2);
            u[dst..dst + cw].copy_from_slice(&yuv.u[src..src + cw]);
            v[dst..dst + cw].copy_from_slice(&yuv.v[src..src + cw]);
        }
        return Ok(rusty_h264_common::YuvFrame {
            width: w,
            height: h,
            y,
            u,
            v,
        });
    }
    Ok(rusty_h264_common::YuvFrame {
        width: w,
        height: h,
        y: yuv.y,
        u: yuv.u,
        v: yuv.v,
    })
}

fn rusty_preset(name: &str) -> rusty_h264_encoder::Preset {
    match name.to_ascii_lowercase().as_str() {
        "ultrafast" | "superfast" | "veryfast" | "faster" | "fast" => {
            rusty_h264_encoder::Preset::Fast
        }
        "slow" | "slower" | "veryslow" | "placebo" | "quality" => {
            rusty_h264_encoder::Preset::Quality
        }
        _ => rusty_h264_encoder::Preset::Balanced,
    }
}

fn build_rusty_config(params: &CodecParams, width: usize, height: usize) -> rusty_h264_encoder::EncoderConfig {
    let mut cfg = rusty_h264_encoder::EncoderConfig::new(width, height);
    // Streaming convert path: one AU per input frame (no lookahead delay).
    cfg.lookahead = 0;
    cfg.mbtree = false;
    cfg.scenecut = 0;
    cfg.bframes = 0;
    cfg.cabac = true;
    cfg.preset = rusty_preset(&params.encode_preset);
    cfg.framerate = fps_from_params(params) as f32;
    if params.gop_size > 0 {
        cfg.gop_size = params.gop_size;
        cfg.min_keyint = (params.gop_size / 10).max(1);
    }
    if params.quality >= 0 {
        cfg.qp = params.quality.clamp(0, 51) as u8;
        cfg.bitrate = 0;
    } else if params.bit_rate > 0 {
        cfg.bitrate = params.bit_rate.min(u32::MAX as u64) as u32;
    }
    cfg
}

fn probe_rusty_avcc(cfg: &rusty_h264_encoder::EncoderConfig) -> Result<Vec<u8>> {
    let mut enc = rusty_h264_encoder::Encoder::new(cfg.clone())
        .map_err(|e| Error::invalid_data(format!("rusty_h264 open: {e}")))?;
    let black = rusty_h264_common::YuvFrame::black(cfg.width, cfg.height);
    let mut au = enc.encode(&black);
    if au.is_empty() {
        au = enc.flush();
    }
    avcc_from_annexb(&au)
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

enum Backend {
    Rusty {
        enc: rusty_h264_encoder::Encoder,
        pending_pts: VecDeque<Timestamp>,
    },
    Native {
        enc: rumpeg_h264::Encoder,
    },
    #[cfg(feature = "encode-x264")]
    X264(h264_x264::X264Backend),
    #[cfg(all(feature = "encode-nvenc", windows))]
    Nvenc(h264_nvenc::NvencBackend),
}

/// H.264 encoder with selectable backend (default: rusty_h264 ME/CABAC/ABR).
pub struct H264Encoder {
    backend: Backend,
    pending: VecDeque<Packet>,
    avcc: Vec<u8>,
    eof: bool,
}

impl H264Encoder {
    /// Open encoder; default backend is rusty_h264 (ME + CABAC + ABR).
    pub fn new(params: &CodecParams) -> Result<Self> {
        let name = params.encoder_name.to_ascii_lowercase();
        match name.as_str() {
            "native" | "rumpeg" | "librumpeg" => Self::open_native(params),
            "libx264" | "x264" => Self::open_x264(params),
            "h264_nvenc" | "nvenc" => Self::open_nvenc(params),
            _ => Self::open_rusty(params),
        }
    }

    /// Open Rumpeg-native Baseline encoder (Intra + simple P).
    pub fn with_mode(params: &CodecParams, mode: IntraMode) -> Result<Self> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let mut enc =
            rumpeg_h264::Encoder::with_mode(video.width, video.height, mode).map_err(map_h264)?;
        if params.quality >= 0 {
            enc.set_qp(params.quality);
        } else if params.bit_rate > 0 {
            enc.set_bitrate(params.bit_rate, fps_from_params(params));
        }
        if params.gop_size > 0 {
            enc.set_gop_size(params.gop_size);
        }
        let avcc = enc.avcc_extradata().map_err(map_h264)?;
        Ok(Self {
            backend: Backend::Native { enc },
            pending: VecDeque::new(),
            avcc,
            eof: false,
        })
    }

    fn open_native(params: &CodecParams) -> Result<Self> {
        Self::with_mode(params, IntraMode::I16x16Cavlc)
    }

    fn open_rusty(params: &CodecParams) -> Result<Self> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let w = even_dim(video.width);
        let h = even_dim(video.height);
        let cfg = build_rusty_config(params, w, h);
        let avcc = probe_rusty_avcc(&cfg)?;
        let enc = rusty_h264_encoder::Encoder::new(cfg)
            .map_err(|e| Error::invalid_data(format!("rusty_h264 open: {e}")))?;
        Ok(Self {
            backend: Backend::Rusty {
                enc,
                pending_pts: VecDeque::new(),
            },
            pending: VecDeque::new(),
            avcc,
            eof: false,
        })
    }

    fn open_x264(params: &CodecParams) -> Result<Self> {
        #[cfg(feature = "encode-x264")]
        {
            let (backend, avcc) = h264_x264::X264Backend::open(params)?;
            return Ok(Self {
                backend: Backend::X264(backend),
                pending: VecDeque::new(),
                avcc,
                eof: false,
            });
        }
        #[cfg(not(feature = "encode-x264"))]
        {
            eprintln!(
                "rumpeg: libx264 requested but encode-x264 feature is off — using rusty_h264"
            );
            Self::open_rusty(params)
        }
    }

    fn open_nvenc(params: &CodecParams) -> Result<Self> {
        #[cfg(all(feature = "encode-nvenc", windows))]
        {
            let (backend, avcc) = h264_nvenc::NvencBackend::open(params)?;
            return Ok(Self {
                backend: Backend::Nvenc(backend),
                pending: VecDeque::new(),
                avcc,
                eof: false,
            });
        }
        #[cfg(not(all(feature = "encode-nvenc", windows)))]
        {
            eprintln!(
                "rumpeg: h264_nvenc requested but encode-nvenc is unavailable — using rusty_h264"
            );
            Self::open_rusty(params)
        }
    }

    /// `avcC` extradata for MP4 muxing.
    pub fn extradata(&self) -> Result<Vec<u8>> {
        if self.avcc.is_empty() {
            Err(Error::invalid_data("H.264 encoder has no avcC yet"))
        } else {
            Ok(self.avcc.clone())
        }
    }

    fn push_annexb(&mut self, bytes: Vec<u8>, pts: Timestamp) {
        for (au, is_key) in split_annexb_aus(&bytes) {
            if au.is_empty() {
                continue;
            }
            let key = is_key || annexb_is_keyframe(&au);
            let mut pkt = Packet::new(Buffer::from_vec(au));
            pkt.pts = pts;
            pkt.dts = pts;
            if key {
                pkt.flags.insert(PacketFlags::KEY);
            }
            self.pending.push_back(pkt);
        }
    }
}

impl Encoder for H264Encoder {
    fn codec_id(&self) -> CodecId {
        CodecId::H264
    }

    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            let mut flushed_packets: Vec<(Vec<u8>, Timestamp, bool)> = Vec::new();
            match &mut self.backend {
                Backend::Rusty {
                    enc,
                    pending_pts,
                } => {
                    let flushed = enc.flush();
                    let pts = pending_pts.pop_front().unwrap_or(Timestamp::NONE);
                    pending_pts.clear();
                    for (au, is_key) in split_annexb_aus(&flushed) {
                        if !au.is_empty() {
                            let key = is_key || annexb_is_keyframe(&au);
                            flushed_packets.push((au, pts, key));
                        }
                    }
                }
                Backend::Native { .. } => {}
                #[cfg(feature = "encode-x264")]
                Backend::X264(b) => {
                    flushed_packets.extend(b.flush()?);
                }
                #[cfg(all(feature = "encode-nvenc", windows))]
                Backend::Nvenc(b) => {
                    flushed_packets.extend(b.flush()?);
                }
            }
            for (bytes, pts, key) in flushed_packets {
                let mut pkt = Packet::new(Buffer::from_vec(bytes));
                pkt.pts = pts;
                pkt.dts = pts;
                if key {
                    pkt.flags.insert(PacketFlags::KEY);
                }
                self.pending.push_back(pkt);
            }
            return Ok(());
        };
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("H.264 encoder expects video"));
        };

        match &mut self.backend {
            Backend::Rusty {
                enc,
                pending_pts,
            } => {
                let yuv = video_frame_to_rusty(video)?;
                pending_pts.push_back(video.pts);
                let bytes = enc.encode(&yuv);
                if !bytes.is_empty() {
                    let pts = pending_pts.pop_front().unwrap_or(video.pts);
                    self.push_annexb(bytes, pts);
                }
            }
            Backend::Native { enc } => {
                let yuv = video_frame_to_yuv(video)?;
                let (bytes, is_key) = enc.encode_access_unit(&yuv).map_err(map_h264)?;
                let mut pkt = Packet::new(Buffer::from_vec(bytes));
                pkt.pts = video.pts;
                pkt.dts = video.pts;
                if is_key {
                    pkt.flags.insert(PacketFlags::KEY);
                }
                self.pending.push_back(pkt);
            }
            #[cfg(feature = "encode-x264")]
            Backend::X264(b) => {
                if let Some((bytes, pts, key)) = b.encode(video)? {
                    let mut pkt = Packet::new(Buffer::from_vec(bytes));
                    pkt.pts = pts;
                    pkt.dts = pts;
                    if key {
                        pkt.flags.insert(PacketFlags::KEY);
                    }
                    self.pending.push_back(pkt);
                }
            }
            #[cfg(all(feature = "encode-nvenc", windows))]
            Backend::Nvenc(b) => {
                if let Some((bytes, pts, key)) = b.encode(video)? {
                    let mut pkt = Packet::new(Buffer::from_vec(bytes));
                    pkt.pts = pts;
                    pkt.dts = pts;
                    if key {
                        pkt.flags.insert(PacketFlags::KEY);
                    }
                    self.pending.push_back(pkt);
                }
            }
        }
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
