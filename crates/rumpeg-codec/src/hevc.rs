//! Pure-Rust HEVC decoding and low-delay I/P encoding.

use crate::{decoder::Decoder, encoder::Encoder};
use oxideav_h265::encoder::{
    inter::{LowDelayPEncoder, YuvFrame},
    rate::RateControlCfg,
};
use rumpeg_util::{
    hevc::{sample_to_annexb, HevcConfig},
    Buffer, CodecId, CodecParams, Error, Frame, Packet, PacketFlags, PixelFormat, Result,
    Timestamp, VideoFrame,
};
use std::collections::VecDeque;

/// HEVC decoder for progressive 8-bit 4:2:0 video.
pub struct HevcDecoder {
    inner: rusty_h265::Decoder,
    pending: VecDeque<Frame>,
    length_size: Option<usize>,
    eof: bool,
}

impl HevcDecoder {
    /// Open with hvcC or Annex-B parameter sets. Packets use the corresponding framing.
    pub fn new(params: &CodecParams) -> Result<Self> {
        let mut inner = rusty_h265::Decoder::new();
        let mut length_size = None;
        if !params.extradata.is_empty() {
            let headers = if params.extradata[0] == 1 {
                let cfg = HevcConfig::parse(&params.extradata)?;
                length_size = Some(cfg.length_size);
                cfg.annexb
            } else {
                params.extradata.clone()
            };
            inner.push_annexb(&headers, None).map_err(decode_error)?;
        }
        Ok(Self {
            inner,
            pending: VecDeque::new(),
            length_size,
            eof: false,
        })
    }

    fn pull(&mut self) -> Result<()> {
        loop {
            match self.inner.next_frame() {
                Ok(frame) => self
                    .pending
                    .push_back(Frame::Video(hevc_frame_to_video(frame)?)),
                Err(rusty_h265::Error::Again | rusty_h265::Error::Eof) => break,
                Err(e) => return Err(decode_error(e)),
            }
        }
        Ok(())
    }
}

fn decode_error(e: rusty_h265::Error) -> Error {
    Error::invalid_data(format!("HEVC: {e:?}"))
}

fn hevc_frame_to_video(frame: rusty_h265::Frame) -> Result<VideoFrame> {
    if frame.bit_depth() != 8 || frame.picture.chroma_format_idc != 1 {
        return Err(Error::unsupported("HEVC output requires 8-bit 4:2:0"));
    }
    let mut yuv = Vec::new();
    frame.write_yuv(&mut yuv);
    let mut vf = VideoFrame::alloc(
        PixelFormat::Yuv420p,
        frame.width as u32,
        frame.height as u32,
    );
    vf.key_frame = frame.poc == 0;
    vf.pts = frame.pts.map(Timestamp::new).unwrap_or(Timestamp::NONE);
    let mut pos = 0;
    for plane in &mut vf.planes {
        let len = plane.len();
        let src = yuv
            .get(pos..pos + len)
            .ok_or_else(|| Error::invalid_data("short HEVC frame"))?;
        plane.make_mut().copy_from_slice(src);
        pos += len;
    }
    Ok(vf)
}

impl Decoder for HevcDecoder {
    fn codec_id(&self) -> CodecId {
        CodecId::Hevc
    }
    fn send_packet(&mut self, packet: Option<&Packet>) -> Result<()> {
        match packet {
            Some(p) => {
                if self.eof {
                    return Err(Error::Eof);
                }
                let pts = if p.pts.is_none() { None } else { Some(p.pts.0) };
                let data = match self.length_size {
                    Some(n) => sample_to_annexb(p.data.as_slice(), n)?,
                    None => p.data.as_slice().to_vec(),
                };
                self.inner.push_annexb(&data, pts).map_err(decode_error)?;
            }
            None => {
                self.eof = true;
                self.inner.flush();
            }
        }
        self.pull()
    }
    fn receive_frame(&mut self) -> Result<Frame> {
        self.pending.pop_front().ok_or(if self.eof {
            Error::Eof
        } else {
            Error::NeedMoreData
        })
    }
}

/// Software HEVC encoder: 8-bit YUV420, I/P frames, QP or average bitrate.
pub struct HevcEncoder {
    inner: LowDelayPEncoder,
    width: usize,
    height: usize,
    coded_width: usize,
    coded_height: usize,
    extradata: Vec<u8>,
    pending: VecDeque<Packet>,
    eof: bool,
}

impl HevcEncoder {
    /// Open a pure-Rust encoder. Dimensions must be positive and even.
    pub fn new(params: &CodecParams) -> Result<Self> {
        if !matches!(params.encoder_name.as_str(), "" | "hevc" | "h265") {
            return Err(Error::unsupported(format!(
                "HEVC encoder '{}'",
                params.encoder_name
            )));
        }
        let v = params
            .video()
            .ok_or_else(|| Error::invalid_data("HEVC video params required"))?;
        let (w, h) = (v.width as usize, v.height as usize);
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || v.pix_fmt != PixelFormat::Yuv420p {
            return Err(Error::invalid_data(
                "HEVC encoder expects positive even dimensions and yuv420p",
            ));
        }
        let (cw, ch) = (w.div_ceil(16) * 16, h.div_ceil(16) * 16);
        let fps = v.frame_rate;
        let (num, den) = if fps.num > 0 && fps.den > 0 {
            (fps.num as u32, fps.den as u32)
        } else {
            (25, 1)
        };
        let create = || -> Result<LowDelayPEncoder> {
            let mut enc = LowDelayPEncoder::new(
                cw,
                ch,
                if params.quality >= 0 {
                    params.quality.clamp(0, 51)
                } else {
                    26
                },
                if params.gop_size == 0 {
                    30
                } else {
                    params.gop_size as usize
                },
            )
            .map_err(|e| Error::invalid_data(format!("HEVC encoder: {e}")))?
            .with_frame_rate(num, den);
            if cw != w || ch != h {
                enc = enc.with_conformance_window(((cw - w) / 2) as u32, ((ch - h) / 2) as u32);
            }
            if params.bit_rate > 0 {
                enc = enc.with_rate_control(&RateControlCfg::new(params.bit_rate, num, den));
            }
            Ok(enc)
        };
        let mut probe = create()?;
        let y = vec![16; cw * ch];
        let uv = vec![128; cw * ch / 4];
        let headers = probe
            .encode_frame(&YuvFrame {
                y: &y,
                cb: &uv,
                cr: &uv,
            })
            .map_err(|e| Error::invalid_data(format!("HEVC headers: {e}")))?;
        let extradata = hvcc_from_annexb(&headers.au)?;
        Ok(Self {
            inner: create()?,
            width: w,
            height: h,
            coded_width: cw,
            coded_height: ch,
            extradata,
            pending: VecDeque::new(),
            eof: false,
        })
    }
    /// HEVC decoder configuration record for MP4 / Matroska output.
    pub fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

/// Build hvcC from an 8-bit Main-profile encoder's Annex-B parameter sets.
fn hvcc_from_annexb(data: &[u8]) -> Result<Vec<u8>> {
    let nals = rumpeg_h264::extract_annexb_nals(data);
    let mut sets = Vec::new();
    for ty in [32, 33, 34] {
        sets.push(
            nals.iter()
                .find(|n| n.len() >= 2 && (n[0] >> 1) & 63 == ty)
                .ok_or_else(|| Error::invalid_data("missing HEVC parameter set"))?,
        );
    }
    let mut rbsp = Vec::new();
    let mut zeros = 0;
    for &b in &sets[1][2..] {
        if zeros == 2 && b == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    if rbsp.len() < 13 {
        return Err(Error::invalid_data("short HEVC SPS"));
    }
    let mut out = vec![0; 23];
    out[0] = 1;
    out[1..13].copy_from_slice(&rbsp[1..13]);
    out[13] = 0xf0;
    out[15] = 0xfc;
    out[16] = 0xfd;
    out[17] = 0xf8;
    out[18] = 0xf8;
    out[21] = ((((rbsp[0] >> 1) & 7) + 1) << 3) | ((rbsp[0] & 1) << 2) | 3;
    out[22] = 3;
    for (ty, nal) in [32, 33, 34].into_iter().zip(sets) {
        let len = u16::try_from(nal.len())
            .map_err(|_| Error::invalid_data("large HEVC parameter set"))?;
        out.extend_from_slice(&[0x80 | ty, 0, 1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    HevcConfig::parse(&out)?;
    Ok(out)
}

impl Encoder for HevcEncoder {
    fn codec_id(&self) -> CodecId {
        CodecId::Hevc
    }
    fn send_frame(&mut self, frame: Option<&Frame>) -> Result<()> {
        let Some(frame) = frame else {
            self.eof = true;
            return Ok(());
        };
        if self.eof {
            return Err(Error::Eof);
        }
        let Frame::Video(v) = frame else {
            return Err(Error::invalid_data("HEVC requires video"));
        };
        if v.width as usize != self.width
            || v.height as usize != self.height
            || v.format != PixelFormat::Yuv420p
        {
            return Err(Error::invalid_data("HEVC frame dimensions/format changed"));
        }
        let mut planes = Vec::new();
        for i in 0..3 {
            let div = if i == 0 { 1 } else { 2 };
            let (w, h, cw, ch) = (
                self.width / div,
                self.height / div,
                self.coded_width / div,
                self.coded_height / div,
            );
            let src = v
                .plane(i)
                .ok_or_else(|| Error::invalid_data("missing HEVC plane"))?;
            let stride = *v
                .strides
                .get(i)
                .ok_or_else(|| Error::invalid_data("missing HEVC stride"))?;
            if stride < w || src.len() < stride.saturating_mul(h - 1).saturating_add(w) {
                return Err(Error::invalid_data("short HEVC plane"));
            }
            let mut dst = Vec::with_capacity(cw * ch);
            for row in 0..ch {
                let line = &src[row.min(h - 1) * stride..row.min(h - 1) * stride + w];
                dst.extend_from_slice(line);
                dst.resize(dst.len() + cw - w, line[w - 1]);
            }
            planes.push(dst);
        }
        let out = self
            .inner
            .encode_frame(&YuvFrame {
                y: &planes[0],
                cb: &planes[1],
                cr: &planes[2],
            })
            .map_err(|e| Error::invalid_data(format!("HEVC encode: {e}")))?;
        let mut packet = Packet::new(Buffer::from_vec(out.au));
        packet.pts = v.pts;
        packet.dts = v.pts;
        if out.keyframe {
            packet.flags.insert(PacketFlags::KEY);
        }
        self.pending.push_back(packet);
        Ok(())
    }
    fn receive_packet(&mut self) -> Result<Packet> {
        self.pending.pop_front().ok_or(if self.eof {
            Error::Eof
        } else {
            Error::NeedMoreData
        })
    }
}
