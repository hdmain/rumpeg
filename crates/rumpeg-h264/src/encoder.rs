//! Pure-Rust H.264 Baseline I_PCM encoder.

use crate::bitstream::BitWriter;
use crate::error::{Error, Result};
use crate::nal::{build_nal, pack_annexb, AvcDecoderConfig, NAL_IDR};
use crate::pps::Pps;
use crate::slice::{write_idr_slice_header, SliceHeader, MB_TYPE_I_PCM};
use crate::sps::Sps;
use crate::yuv::Yuv420Planar;

/// Encodes planar YUV 4:2:0 pictures as Baseline IDR frames using `I_PCM`.
pub struct Encoder {
    sps: Sps,
    pps: Pps,
    sps_nal: Vec<u8>,
    pps_nal: Vec<u8>,
    display_width: u32,
    display_height: u32,
}

impl Encoder {
    /// Create an encoder for `width`×`height` display pictures (padded to MBs internally).
    pub fn new(width: u32, height: u32) -> Result<Self> {
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
        })
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

    /// Encode one IDR access unit as Annex-B (SPS + PPS + IDR).
    pub fn encode_annexb(&self, frame: &Yuv420Planar) -> Result<Vec<u8>> {
        let idr = self.encode_idr_nal(frame)?;
        Ok(pack_annexb(&[
            self.sps_nal.as_slice(),
            self.pps_nal.as_slice(),
            idr.as_slice(),
        ]))
    }

    /// Encode only the IDR VCL NAL (caller already has SPS/PPS / avcC).
    pub fn encode_idr_nal(&self, frame: &Yuv420Planar) -> Result<Vec<u8>> {
        if frame.width != self.display_width || frame.height != self.display_height {
            return Err(Error::invalid(format!(
                "frame size {}x{} != encoder {}x{}",
                frame.width, frame.height, self.display_width, self.display_height
            )));
        }
        let padded = Yuv420Planar::pad_to_mbs(frame);
        let mb_w = self.sps.coded_width() / 16;
        let mb_h = self.sps.coded_height() / 16;

        let mut w = BitWriter::new();
        let hdr = SliceHeader::idr_i(self.pps.pps_id);
        write_idr_slice_header(&mut w, &hdr, &self.sps, &self.pps);

        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                w.write_ue(MB_TYPE_I_PCM);
                w.byte_align_zeros();
                write_pcm_mb(&mut w, &padded, mb_x, mb_y);
            }
        }
        w.write_rbsp_trailing_bits();
        Ok(build_nal(3, NAL_IDR, w.as_slice()))
    }
}

fn write_pcm_mb(w: &mut BitWriter, frame: &Yuv420Planar, mb_x: u32, mb_y: u32) {
    let stride = frame.width as usize;
    let origin = (mb_y * 16 * frame.width + mb_x * 16) as usize;
    for row in 0..16 {
        let start = origin + row * stride;
        w.write_bytes(&frame.y[start..start + 16]);
    }
    let cstride = (frame.width / 2) as usize;
    let corigin = (mb_y * 8 * (frame.width / 2) + mb_x * 8) as usize;
    for row in 0..8 {
        let start = corigin + row * cstride;
        w.write_bytes(&frame.u[start..start + 8]);
    }
    for row in 0..8 {
        let start = corigin + row * cstride;
        w.write_bytes(&frame.v[start..start + 8]);
    }
}
