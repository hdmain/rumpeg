//! Native Baseline Intra decoder (I_PCM + I_16x16 CAVLC) - used for encoder
//! round-trip tests. Production decode goes through [`crate::decoder::Decoder`]
//! (`rusty_h264-decoder`, CAVLC + CABAC).

use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::mb::{decode_intra_mb, NcCache};
use crate::nal::{
    avcc_sample_to_annexb, extract_annexb_nals, nal_rbsp, nal_unit_type, AvcDecoderConfig, NAL_IDR,
    NAL_PPS, NAL_SPS,
};
use crate::pps::Pps;
use crate::slice::parse_slice_header;
use crate::sps::Sps;
use crate::yuv::Yuv420Planar;
use std::collections::VecDeque;

/// Decodes Baseline Progressive Intra IDR frames (`I_PCM` and `I_16x16` CAVLC).
pub struct BaselineIntraDecoder {
    sps: Option<Sps>,
    pps: Option<Pps>,
    length_size: usize,
    pending: VecDeque<Yuv420Planar>,
    eof: bool,
}

impl BaselineIntraDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self {
            sps: None,
            pps: None,
            length_size: 4,
            pending: VecDeque::new(),
            eof: false,
        }
    }

    /// Configure from MP4 `avcC` extradata.
    pub fn with_avcc(extradata: &[u8]) -> Result<Self> {
        let cfg = AvcDecoderConfig::from_avcc(extradata)?;
        let mut dec = Self::new();
        dec.length_size = cfg.length_size();
        for sps in &cfg.sps_list {
            dec.sps = Some(Sps::parse(sps)?);
        }
        for pps in &cfg.pps_list {
            dec.pps = Some(Pps::parse(pps)?);
        }
        Ok(dec)
    }

    /// Display size from the active SPS, if known.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.sps.as_ref().map(|s| (s.width(), s.height()))
    }

    /// Feed an Annex-B blob (may contain multiple NALs) or a single NAL without start codes.
    pub fn decode_annexb(&mut self, data: &[u8]) -> Result<()> {
        let nals = extract_annexb_nals(data);
        if nals.is_empty() {
            if !data.is_empty() {
                self.decode_nal(data)?;
            }
            return Ok(());
        }
        for nal in nals {
            self.decode_nal(nal)?;
        }
        Ok(())
    }

    /// Feed one AVCC length-prefixed sample (MP4).
    pub fn decode_avcc_sample(&mut self, sample: &[u8]) -> Result<()> {
        let annexb = avcc_sample_to_annexb(sample, self.length_size)?;
        self.decode_annexb(&annexb)
    }

    /// Decode a single NAL unit (with header, without start code).
    pub fn decode_nal(&mut self, nal: &[u8]) -> Result<()> {
        match nal_unit_type(nal)? {
            NAL_SPS => {
                self.sps = Some(Sps::parse(nal)?);
            }
            NAL_PPS => {
                self.pps = Some(Pps::parse(nal)?);
            }
            NAL_IDR => {
                let frame = self.decode_idr_slice(nal)?;
                self.pending.push_back(frame);
            }
            other => {
                if other == 1 {
                    return Err(Error::unsupported(
                        "native BaselineIntraDecoder: non-IDR / Inter slices not supported",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Pull the next decoded frame.
    pub fn receive(&mut self) -> Result<Option<Yuv420Planar>> {
        Ok(self.pending.pop_front())
    }

    /// Signal end of stream.
    pub fn flush(&mut self) {
        self.eof = true;
    }

    fn decode_idr_slice(&self, nal: &[u8]) -> Result<Yuv420Planar> {
        let sps = self
            .sps
            .as_ref()
            .ok_or_else(|| Error::invalid("IDR before SPS"))?;
        let pps = self
            .pps
            .as_ref()
            .ok_or_else(|| Error::invalid("IDR before PPS"))?;
        if pps.entropy_coding_mode_flag {
            return Err(Error::unsupported(
                "native BaselineIntraDecoder does not decode CABAC (use Decoder)",
            ));
        }

        let rbsp = nal_rbsp(nal)?;
        let mut r = BitReader::new(&rbsp);
        let hdr = parse_slice_header(&mut r, sps, pps, true)?;

        let coded_w = sps.coded_width();
        let coded_h = sps.coded_height();
        let mb_w = coded_w / 16;
        let mb_h = coded_h / 16;
        let mut frame = Yuv420Planar::zeroed(coded_w, coded_h);
        let mut nc = NcCache::new(mb_w, mb_h);
        let qp = (pps.pic_init_qp_minus26 + 26 + hdr.slice_qp_delta).clamp(0, 51);

        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                decode_intra_mb(&mut r, &mut frame, &mut nc, mb_x, mb_y, qp)?;
            }
        }

        Ok(frame.crop(sps.width(), sps.height()))
    }
}

impl Default for BaselineIntraDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{Encoder, IntraMode};

    #[test]
    fn ipc_roundtrip_colors() {
        let w = 32u32;
        let h = 32u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        for (i, y) in src.y.iter_mut().enumerate() {
            *y = (i % 220) as u8 + 16;
        }
        for (i, u) in src.u.iter_mut().enumerate() {
            *u = ((80 + i) % 200) as u8;
        }
        for (i, v) in src.v.iter_mut().enumerate() {
            *v = ((40 + i * 3) % 200) as u8;
        }

        let mut enc = Encoder::new(w, h).unwrap();
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = BaselineIntraDecoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        assert_eq!(out.width, w);
        assert_eq!(out.height, h);
        assert_eq!(out.y, src.y);
        assert_eq!(out.u, src.u);
        assert_eq!(out.v, src.v);
    }

    #[test]
    fn cavlc_i16x16_flat_grey() {
        let w = 16u32;
        let h = 16u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        src.y.fill(128);
        src.u.fill(128);
        src.v.fill(128);
        let mut enc = Encoder::with_mode(w, h, IntraMode::I16x16Cavlc).unwrap();
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = BaselineIntraDecoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        let mut err = 0i64;
        for (a, b) in src.y.iter().zip(out.y.iter()) {
            err += (i32::from(*a) - i32::from(*b)).abs() as i64;
        }
        let mean = err as f64 / src.y.len() as f64;
        assert!(mean < 1.0, "flat grey mean abs error {mean}");
    }

    #[test]
    fn cavlc_i16x16_flat_offset() {
        let w = 16u32;
        let h = 16u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        src.y.fill(140);
        src.u.fill(128);
        src.v.fill(128);
        let mut enc = Encoder::with_mode(w, h, IntraMode::I16x16Cavlc).unwrap();
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = BaselineIntraDecoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        let mut err = 0i64;
        for (a, b) in src.y.iter().zip(out.y.iter()) {
            err += (i32::from(*a) - i32::from(*b)).abs() as i64;
        }
        let mean = err as f64 / src.y.len() as f64;
        assert!(mean < 4.0, "flat offset mean abs error {mean}");
    }

    #[test]
    fn cavlc_i16x16_single_mb_ramp() {
        let w = 16u32;
        let h = 16u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        for y in 0..h {
            for x in 0..w {
                src.y[(y * w + x) as usize] = (16 + x * 8).min(235) as u8;
            }
        }
        src.u.fill(128);
        src.v.fill(128);
        let mut enc = Encoder::with_mode(w, h, IntraMode::I16x16Cavlc).unwrap();
        enc.set_qp(6);
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = BaselineIntraDecoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        let mut err = 0i64;
        for (a, b) in src.y.iter().zip(out.y.iter()) {
            err += (i32::from(*a) - i32::from(*b)).abs() as i64;
        }
        let mean = err as f64 / src.y.len() as f64;
        assert!(mean < 5.0, "single-MB ramp mean abs error {mean}");
    }

    #[test]
    fn cavlc_i16x16_roundtrip_low_error() {
        let w = 32u32;
        let h = 32u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        for y in 0..h {
            for x in 0..w {
                src.y[(y * w + x) as usize] = (16 + (x + y) * 3).min(235) as u8;
            }
        }
        for y in 0..(h / 2) {
            for x in 0..(w / 2) {
                src.u[(y * (w / 2) + x) as usize] = (128 + x as i32 - 8).clamp(16, 240) as u8;
                src.v[(y * (w / 2) + x) as usize] = (128 + y as i32 - 8).clamp(16, 240) as u8;
            }
        }

        let mut enc = Encoder::with_mode(w, h, IntraMode::I16x16Cavlc).unwrap();
        enc.set_qp(0);
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut pcm_enc = Encoder::new(w, h).unwrap();
        let pcm = pcm_enc.encode_annexb(&src).unwrap();
        assert!(
            annexb.len() < pcm.len(),
            "CAVLC AU should be smaller than I_PCM ({} vs {})",
            annexb.len(),
            pcm.len()
        );

        let mut dec = BaselineIntraDecoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        assert_eq!(out.width, w);
        assert_eq!(out.height, h);

        let mut err = 0i64;
        for (a, b) in src.y.iter().zip(out.y.iter()) {
            err += (i32::from(*a) - i32::from(*b)).abs() as i64;
        }
        let mean = err as f64 / src.y.len() as f64;
        assert!(
            mean < 12.0,
            "luma mean abs error too high: {mean} (CAVLC Intra decode)"
        );
    }
}
