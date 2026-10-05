//! Pure-Rust H.264 Baseline encoder: Intra (`I_PCM` / `I_16x16`) and minimal Inter (P).
//!
//! Inter path uses a GOP of IDR + P frames. P MBs are either **SKIP** (copy from
//! previous reconstructed frame when SAD is low) or **Intra refresh** (`I_16x16`
//! coded inside the P slice). This is not full motion estimation, but it yields
//! practical file-size reduction for typical Progressive content.

use crate::bitstream::BitWriter;
use crate::error::{Error, Result};
use crate::mb::{
    copy_mb, encode_i16x16_dc_mb, encode_i16x16_dc_mb_in_p, encode_pcm_mb, mb_luma_sad, NcCache,
};
use crate::nal::{build_nal, pack_annexb, AvcDecoderConfig, NAL_IDR, NAL_NON_IDR};
use crate::pps::Pps;
use crate::slice::{write_idr_slice_header, write_p_slice_header, SliceHeader};
use crate::sps::Sps;
use crate::yuv::Yuv420Planar;

/// Intra coding mode for IDR / Intra frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IntraMode {
    /// Lossless-ish raw samples per macroblock (exact round-trip).
    #[default]
    Ipcm,
    /// Baseline CAVLC `I_16x16` DC prediction + residual (lossy, much smaller).
    I16x16Cavlc,
}

/// Encodes planar YUV 4:2:0 pictures as Baseline H.264.
pub struct Encoder {
    sps: Sps,
    pps: Pps,
    sps_nal: Vec<u8>,
    pps_nal: Vec<u8>,
    display_width: u32,
    display_height: u32,
    mode: IntraMode,
    qp: i32,
    /// Frames between IDRs (1 = all Intra). Default 30.
    gop_size: u32,
    /// Enable P frames when `gop_size > 1` and mode is CAVLC.
    enable_p: bool,
    /// Luma SAD / MB below which a P MB is SKIP'd.
    skip_sad: u32,
    frame_index: u32,
    frame_num: u32,
    idr_pic_id: u32,
    prev: Option<Yuv420Planar>,
}

impl Encoder {
    /// Create an `I_PCM` encoder for `width`×`height` display pictures.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Self::with_mode(width, height, IntraMode::Ipcm)
    }

    /// Create an encoder with an explicit Intra mode (P enabled for CAVLC).
    pub fn with_mode(width: u32, height: u32, mode: IntraMode) -> Result<Self> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::invalid("width/height must be non-zero even numbers"));
        }
        let sps = Sps::baseline(width, height);
        let pps = Pps::baseline();
        let sps_nal = sps.to_nal();
        let pps_nal = pps.to_nal();
        Ok(Self {
            sps,
            pps,
            sps_nal,
            pps_nal,
            display_width: width,
            display_height: height,
            mode,
            qp: 28,
            gop_size: 30,
            enable_p: matches!(mode, IntraMode::I16x16Cavlc),
            skip_sad: 16 * 16 * 6, // ~avg abs diff of 6 per luma sample
            frame_index: 0,
            frame_num: 0,
            idr_pic_id: 0,
            prev: None,
        })
    }

    /// Set quantizer (0..=51). Ignored for `I_PCM`.
    pub fn set_qp(&mut self, qp: i32) {
        self.qp = qp.clamp(0, 51);
    }

    /// Current QP.
    pub fn qp(&self) -> i32 {
        self.qp
    }

    /// Map a CRF-like quality (0=best … 51=worst) onto QP.
    pub fn set_crf(&mut self, crf: f32) {
        self.set_qp(crf.round() as i32);
    }

    /// Approximate QP from a target video bitrate (bits/s) and frame rate.
    ///
    /// Rough rule of thumb for our Intra/P hybrid — not VBV/2-pass accurate.
    pub fn set_bitrate(&mut self, bit_rate: u64, fps: f64) {
        let pixels = (self.display_width as u64).saturating_mul(self.display_height as u64);
        let fps = if fps <= 0.0 { 25.0 } else { fps };
        let bpp = if pixels == 0 || bit_rate == 0 {
            0.05
        } else {
            (bit_rate as f64) / (pixels as f64 * fps)
        };
        // Map bits-per-pixel to QP (tuned for I_16x16 + SKIP P).
        let qp = if bpp >= 0.20 {
            18
        } else if bpp >= 0.12 {
            23
        } else if bpp >= 0.08 {
            28
        } else if bpp >= 0.05 {
            32
        } else if bpp >= 0.03 {
            36
        } else if bpp >= 0.02 {
            40
        } else {
            45
        };
        self.set_qp(qp);
    }

    /// Set GOP size (IDR interval). `1` forces all-Intra.
    pub fn set_gop_size(&mut self, gop: u32) {
        self.gop_size = gop.max(1);
        self.enable_p = self.gop_size > 1 && matches!(self.mode, IntraMode::I16x16Cavlc);
    }

    /// Active Intra mode.
    pub fn mode(&self) -> IntraMode {
        self.mode
    }

    /// SPS NAL (with header).
    pub fn sps_nal(&self) -> &[u8] {
        &self.sps_nal
    }

    /// PPS NAL (with header).
    pub fn pps_nal(&self) -> &[u8] {
        &self.pps_nal
    }

    /// Parsed SPS.
    pub fn sps(&self) -> &Sps {
        &self.sps
    }

    /// `avcC` extradata for MP4.
    pub fn avcc_extradata(&self) -> Result<Vec<u8>> {
        Ok(AvcDecoderConfig::from_sps_pps(&self.sps_nal, &self.pps_nal)?.to_avcc())
    }

    /// Encode one access unit as Annex-B (SPS+PPS on IDR; VCL only otherwise).
    pub fn encode_annexb(&mut self, frame: &Yuv420Planar) -> Result<Vec<u8>> {
        let (nal, is_idr) = self.encode_vcl_nal(frame)?;
        if is_idr {
            Ok(pack_annexb(&[
                self.sps_nal.as_slice(),
                self.pps_nal.as_slice(),
                nal.as_slice(),
            ]))
        } else {
            Ok(pack_annexb(&[nal.as_slice()]))
        }
    }

    /// Encode one access unit; returns `(annexb_or_nal_bytes, is_keyframe)`.
    pub fn encode_access_unit(&mut self, frame: &Yuv420Planar) -> Result<(Vec<u8>, bool)> {
        let (nal, is_idr) = self.encode_vcl_nal(frame)?;
        let bytes = if is_idr {
            pack_annexb(&[
                self.sps_nal.as_slice(),
                self.pps_nal.as_slice(),
                nal.as_slice(),
            ])
        } else {
            pack_annexb(&[nal.as_slice()])
        };
        Ok((bytes, is_idr))
    }

    /// Encode only the VCL NAL (IDR or non-IDR).
    pub fn encode_idr_nal(&mut self, frame: &Yuv420Planar) -> Result<Vec<u8>> {
        let (nal, _) = self.encode_vcl_nal(frame)?;
        Ok(nal)
    }

    fn encode_vcl_nal(&mut self, frame: &Yuv420Planar) -> Result<(Vec<u8>, bool)> {
        if frame.width != self.display_width || frame.height != self.display_height {
            return Err(Error::invalid(format!(
                "frame size {}x{} != encoder {}x{}",
                frame.width, frame.height, self.display_width, self.display_height
            )));
        }
        let force_idr = self.frame_index == 0
            || !self.enable_p
            || self.gop_size <= 1
            || (self.frame_index % self.gop_size == 0)
            || self.prev.is_none()
            || matches!(self.mode, IntraMode::Ipcm);

        let result = if force_idr {
            self.encode_idr(frame)
        } else {
            self.encode_p(frame)
        };
        self.frame_index = self.frame_index.wrapping_add(1);
        result
    }

    fn encode_idr(&mut self, frame: &Yuv420Planar) -> Result<(Vec<u8>, bool)> {
        let padded = Yuv420Planar::pad_to_mbs(frame);
        let mb_w = self.sps.coded_width() / 16;
        let mb_h = self.sps.coded_height() / 16;

        let mut w = BitWriter::new();
        let mut hdr = SliceHeader::idr_i(self.pps.pps_id);
        hdr.frame_num = 0;
        hdr.idr_pic_id = self.idr_pic_id;
        hdr.pic_order_cnt_lsb = 0;
        if self.mode == IntraMode::I16x16Cavlc {
            hdr.slice_qp_delta = self.qp - 26;
        }
        write_idr_slice_header(&mut w, &hdr, &self.sps, &self.pps);

        let mut working = padded.clone();
        match self.mode {
            IntraMode::Ipcm => {
                for mb_y in 0..mb_h {
                    for mb_x in 0..mb_w {
                        encode_pcm_mb(&mut w, &padded, mb_x, mb_y);
                    }
                }
            }
            IntraMode::I16x16Cavlc => {
                let mut nc = NcCache::new(mb_w, mb_h);
                for mb_y in 0..mb_h {
                    for mb_x in 0..mb_w {
                        encode_i16x16_dc_mb(&mut w, &mut working, &mut nc, mb_x, mb_y, self.qp)?;
                    }
                }
            }
        }
        w.write_rbsp_trailing_bits();
        let nal = build_nal(3, NAL_IDR, w.as_slice());

        self.frame_num = 1;
        self.idr_pic_id = self.idr_pic_id.wrapping_add(1);
        self.prev = Some(match self.mode {
            IntraMode::Ipcm => padded,
            IntraMode::I16x16Cavlc => working,
        });
        Ok((nal, true))
    }

    fn encode_p(&mut self, frame: &Yuv420Planar) -> Result<(Vec<u8>, bool)> {
        let padded = Yuv420Planar::pad_to_mbs(frame);
        let reference = self
            .prev
            .as_ref()
            .ok_or_else(|| Error::invalid("P frame without reference"))?
            .clone();
        let mb_w = self.sps.coded_width() / 16;
        let mb_h = self.sps.coded_height() / 16;
        let total_mb = mb_w * mb_h;

        let mut w = BitWriter::new();
        let mut hdr = SliceHeader::p(self.pps.pps_id, self.frame_num, self.frame_index * 2);
        hdr.slice_qp_delta = self.qp - 26;
        write_p_slice_header(&mut w, &hdr, &self.sps, &self.pps);

        let mut nc = NcCache::new(mb_w, mb_h);
        let mut working = padded.clone();
        let mut skip_run = 0u32;
        let mut mb_i = 0u32;

        while mb_i < total_mb {
            let mb_x = mb_i % mb_w;
            let mb_y = mb_i / mb_w;
            let sad = mb_luma_sad(&padded, &reference, mb_x, mb_y);
            if sad <= self.skip_sad {
                skip_run += 1;
                copy_mb(&mut working, &reference, mb_x, mb_y);
                mb_i += 1;
                continue;
            }
            // Flush pending skips, then code an Intra-refresh MB in the P slice.
            w.write_ue(skip_run);
            skip_run = 0;
            encode_i16x16_dc_mb_in_p(&mut w, &mut working, &mut nc, mb_x, mb_y, self.qp)?;
            mb_i += 1;
        }
        // Trailing skip run covers remaining MBs (or 0).
        w.write_ue(skip_run);
        w.write_rbsp_trailing_bits();
        let nal = build_nal(2, NAL_NON_IDR, w.as_slice());

        self.frame_num =
            (self.frame_num + 1) & ((1u32 << (self.sps.log2_max_frame_num_minus4 + 4)) - 1);
        self.prev = Some(working);
        Ok((nal, false))
    }
}
