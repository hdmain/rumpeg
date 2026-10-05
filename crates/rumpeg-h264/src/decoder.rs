//! Pure-Rust H.264 Baseline I_PCM decoder.

use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::nal::{
    avcc_sample_to_annexb, extract_annexb_nals, nal_rbsp, nal_unit_type, AvcDecoderConfig, NAL_IDR,
    NAL_PPS, NAL_SPS,
};
use crate::pps::Pps;
use crate::slice::{parse_slice_header, MB_TYPE_I_PCM};
use crate::sps::Sps;
use crate::yuv::Yuv420Planar;
use std::collections::VecDeque;

/// Decodes Baseline `I_PCM` IDR frames (and stores SPS/PPS from the bitstream).
pub struct Decoder {
    sps: Option<Sps>,
    pps: Option<Pps>,
    /// AVCC NAL length size when feeding length-prefixed samples.
    length_size: usize,
    pending: VecDeque<Yuv420Planar>,
    eof: bool,
}

impl Decoder {
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
            // Maybe a raw NAL without start code.
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
                // Ignore AUD, SEI, non-IDR for this subset.
                if other == 1 {
                    return Err(Error::unsupported(
                        "non-IDR slices are not supported by rumpeg-h264 v0.1 (I_PCM IDR only)",
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
            return Err(Error::unsupported("CABAC not supported"));
        }

        let rbsp = nal_rbsp(nal)?;
        let mut r = BitReader::new(&rbsp);
        let _hdr = parse_slice_header(&mut r, sps, pps, true)?;

        let coded_w = sps.coded_width();
        let coded_h = sps.coded_height();
        let mb_w = coded_w / 16;
        let mb_h = coded_h / 16;
        let mut frame = Yuv420Planar::zeroed(coded_w, coded_h);

        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                let mb_type = r.read_ue()?;
                if mb_type != MB_TYPE_I_PCM {
                    return Err(Error::unsupported(format!(
                        "only I_PCM macroblocks supported (got mb_type={mb_type})"
                    )));
                }
                while !r.byte_aligned() {
                    if r.read_bit()? != 0 {
                        return Err(Error::invalid("pcm_alignment_zero_bit must be 0"));
                    }
                }
                read_pcm_mb(&mut r, &mut frame, mb_x, mb_y)?;
            }
        }

        Ok(frame.crop(sps.width(), sps.height()))
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

fn read_pcm_mb(
    r: &mut BitReader<'_>,
    frame: &mut Yuv420Planar,
    mb_x: u32,
    mb_y: u32,
) -> Result<()> {
    let stride = frame.width as usize;
    let origin = (mb_y * 16 * frame.width + mb_x * 16) as usize;
    for row in 0..16 {
        let bytes = r.read_bytes(16)?;
        let start = origin + row * stride;
        frame.y[start..start + 16].copy_from_slice(bytes);
    }
    let cstride = (frame.width / 2) as usize;
    let corigin = (mb_y * 8 * (frame.width / 2) + mb_x * 8) as usize;
    for row in 0..8 {
        let bytes = r.read_bytes(8)?;
        let start = corigin + row * cstride;
        frame.u[start..start + 8].copy_from_slice(bytes);
    }
    for row in 0..8 {
        let bytes = r.read_bytes(8)?;
        let start = corigin + row * cstride;
        frame.v[start..start + 8].copy_from_slice(bytes);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Encoder;

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

        let enc = Encoder::new(w, h).unwrap();
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = Decoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        assert_eq!(out.width, w);
        assert_eq!(out.height, h);
        assert_eq!(out.y, src.y);
        assert_eq!(out.u, src.u);
        assert_eq!(out.v, src.v);
    }
}
