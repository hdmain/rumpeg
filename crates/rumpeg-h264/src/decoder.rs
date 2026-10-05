//! Production H.264 decoder powered by [`rusty_h264_decoder`] (CAVLC + CABAC).
//!
//! This is a pure-Rust decoder (`forbid(unsafe_code)` core). We intentionally
//! disable the optional `asm` feature (OpenH264 SIMD kernels) so Rumpeg stays
//! free of Cisco OpenH264 C/C++ / FFI.

use crate::error::{Error, Result};
use crate::nal::{avcc_sample_to_annexb, AvcDecoderConfig};
use crate::sps::Sps;
use crate::yuv::Yuv420Planar;
use rusty_h264_decoder::Decoder as RustyDecoder;
use std::collections::VecDeque;

/// Decodes Progressive 8-bit 4:2:0 H.264 (Baseline / Main / much of High),
/// including **CABAC** Intra and Inter slices.
pub struct Decoder {
    inner: RustyDecoder,
    /// AVCC NAL length size when feeding length-prefixed samples.
    length_size: usize,
    /// Annex-B SPS+PPS from `avcC`, injected once before the first VCL sample.
    param_sets: Vec<u8>,
    params_fed: bool,
    dims: Option<(u32, u32)>,
    pending: VecDeque<Yuv420Planar>,
    eof: bool,
}

impl Decoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self {
            inner: RustyDecoder::new(),
            length_size: 4,
            param_sets: Vec::new(),
            params_fed: false,
            dims: None,
            pending: VecDeque::new(),
            eof: false,
        }
    }

    /// Configure from MP4 `avcC` extradata (stores SPS/PPS for later injection).
    pub fn with_avcc(extradata: &[u8]) -> Result<Self> {
        let cfg = AvcDecoderConfig::from_avcc(extradata)?;
        let mut dec = Self::new();
        dec.length_size = cfg.length_size();
        dec.param_sets = pack_param_sets(&cfg);
        if let Some(sps_nal) = cfg.sps_list.first() {
            if let Ok(sps) = Sps::parse(sps_nal) {
                dec.dims = Some((sps.width(), sps.height()));
            }
        }
        Ok(dec)
    }

    /// Display size from `avcC` SPS, if known.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.dims
    }

    /// Feed an Annex-B blob (may contain multiple NALs / access units).
    pub fn decode_annexb(&mut self, data: &[u8]) -> Result<()> {
        self.feed_annexb(data)
    }

    /// Feed one AVCC length-prefixed sample (MP4).
    pub fn decode_avcc_sample(&mut self, sample: &[u8]) -> Result<()> {
        self.feed_params_if_needed()?;
        let annexb = avcc_sample_to_annexb(sample, self.length_size)?;
        self.feed_annexb(&annexb)
    }

    /// Decode a single NAL unit (with header, without start code) by wrapping
    /// it as a one-NAL Annex-B access unit.
    pub fn decode_nal(&mut self, nal: &[u8]) -> Result<()> {
        if nal.is_empty() {
            return Ok(());
        }
        let mut annexb = Vec::with_capacity(4 + nal.len());
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(nal);
        self.feed_annexb(&annexb)
    }

    /// Pull the next decoded frame.
    pub fn receive(&mut self) -> Result<Option<Yuv420Planar>> {
        Ok(self.pending.pop_front())
    }

    /// Signal end of stream.
    pub fn flush(&mut self) {
        self.eof = true;
    }

    fn feed_params_if_needed(&mut self) -> Result<()> {
        if self.params_fed || self.param_sets.is_empty() {
            return Ok(());
        }
        // SPS/PPS only — no picture expected.
        match self.inner.decode(&self.param_sets) {
            Ok(None) => {}
            Ok(Some(frame)) => {
                // Unusual but harmless if extradata somehow included VCL.
                self.push_yuv(frame.width, frame.height, frame.y, frame.u, frame.v);
            }
            Err(e) => return Err(map_rusty(e)),
        }
        self.params_fed = true;
        Ok(())
    }

    fn feed_annexb(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self.inner.decode(data) {
            Ok(None) => Ok(()),
            Ok(Some(frame)) => {
                self.push_yuv(frame.width, frame.height, frame.y, frame.u, frame.v);
                Ok(())
            }
            Err(e) => Err(map_rusty(e)),
        }
    }

    fn push_yuv(&mut self, width: usize, height: usize, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) {
        let width = width as u32;
        let height = height as u32;
        self.dims = Some((width, height));
        self.pending.push_back(Yuv420Planar {
            width,
            height,
            y,
            u,
            v,
        });
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

fn pack_param_sets(cfg: &AvcDecoderConfig) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in cfg.sps_list.iter().chain(cfg.pps_list.iter()) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

fn map_rusty(err: rusty_h264_decoder::DecodeError) -> Error {
    // DecodeError is typically displayed via Debug/Display; keep message short.
    Error::invalid(format!("H.264 decode failed: {err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Encoder;

    #[test]
    fn rusty_decodes_ipcm_annexb() {
        let w = 32u32;
        let h = 32u32;
        let mut src = Yuv420Planar::zeroed(w, h);
        for (i, y) in src.y.iter_mut().enumerate() {
            *y = (i % 220) as u8 + 16;
        }
        src.u.fill(128);
        src.v.fill(128);

        let mut enc = Encoder::new(w, h).unwrap();
        let annexb = enc.encode_annexb(&src).unwrap();
        let mut dec = Decoder::new();
        dec.decode_annexb(&annexb).unwrap();
        let out = dec.receive().unwrap().expect("frame");
        assert_eq!(out.width, w);
        assert_eq!(out.height, h);
        // I_PCM should be near-exact; allow tiny deblock / reconstruction deltas.
        let mut err = 0i64;
        for (a, b) in src.y.iter().zip(out.y.iter()) {
            err += (i32::from(*a) - i32::from(*b)).abs() as i64;
        }
        let mean = err as f64 / src.y.len() as f64;
        assert!(mean < 2.0, "I_PCM via rusty_h264 mean abs error {mean}");
    }
}
