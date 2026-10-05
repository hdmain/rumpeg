//! NAL unit helpers (Annex-B and AVCC).

use crate::bitstream::{ebsp_from_rbsp, rbsp_from_ebsp};
use crate::error::{Error, Result};
use crate::pps::Pps;
use crate::sps::Sps;

/// NAL unit type: SPS.
pub const NAL_SPS: u8 = 7;
/// NAL unit type: PPS.
pub const NAL_PPS: u8 = 8;
/// NAL unit type: IDR slice.
pub const NAL_IDR: u8 = 5;
/// NAL unit type: non-IDR coded slice.
pub const NAL_NON_IDR: u8 = 1;

/// Return `nal_unit_type` from a NAL header byte (forbidden_zero_bit | nal_ref_idc | type).
pub fn nal_unit_type(nal: &[u8]) -> Result<u8> {
    let hdr = *nal.first().ok_or_else(|| Error::truncated("empty NAL"))?;
    Ok(hdr & 0x1F)
}

/// RBSP payload of a NAL (without the 1-byte header), emulation bytes removed.
pub fn nal_rbsp(nal: &[u8]) -> Result<Vec<u8>> {
    if nal.is_empty() {
        return Err(Error::truncated("empty NAL"));
    }
    Ok(rbsp_from_ebsp(&nal[1..]))
}

/// Build a NAL unit from type + RBSP (adds header + emulation prevention).
pub fn build_nal(nal_ref_idc: u8, nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let header = (nal_ref_idc & 0x3) << 5 | (nal_type & 0x1F);
    let mut out = Vec::with_capacity(1 + rbsp.len() + 8);
    out.push(header);
    out.extend_from_slice(&ebsp_from_rbsp(rbsp));
    out
}

/// Split an Annex-B byte stream into NAL units (without start codes).
pub fn extract_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        // find start code
        let sc = if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            4
        } else {
            i += 1;
            continue;
        };
        let start = i + sc;
        let mut end = start;
        while end + 3 < data.len() {
            if data[end] == 0
                && data[end + 1] == 0
                && (data[end + 2] == 1
                    || (end + 4 <= data.len() && data[end + 2] == 0 && data[end + 3] == 1))
            {
                break;
            }
            end += 1;
        }
        if end + 3 >= data.len() {
            end = data.len();
        }
        if start < end {
            nals.push(&data[start..end]);
        }
        i = end;
    }
    nals
}

/// Pack NAL units into Annex-B with 4-byte start codes.
pub fn pack_annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

/// Convert length-prefixed AVCC sample to Annex-B (using `length_size` 1..4).
pub fn avcc_sample_to_annexb(sample: &[u8], length_size: usize) -> Result<Vec<u8>> {
    if !(1..=4).contains(&length_size) {
        return Err(Error::invalid("bad AVCC length_size"));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + length_size <= sample.len() {
        let mut len = 0usize;
        for _ in 0..length_size {
            len = (len << 8) | sample[i] as usize;
            i += 1;
        }
        if i + len > sample.len() {
            return Err(Error::truncated("AVCC NAL length"));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[i..i + len]);
        i += len;
    }
    Ok(out)
}

/// Convert Annex-B access unit to a single AVCC sample (4-byte lengths).
pub fn annexb_to_avcc_sample(annexb: &[u8]) -> Vec<u8> {
    let nals = extract_annexb_nals(annexb);
    let mut out = Vec::new();
    for nal in nals {
        let len = nal.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// AVCDecoderConfigurationRecord (`avcC`) used as MP4 extradata.
#[derive(Clone, Debug)]
pub struct AvcDecoderConfig {
    /// Profile, e.g. 66 = Baseline.
    pub profile_idc: u8,
    /// Constraint flags.
    pub profile_compat: u8,
    /// Level, e.g. 30 = 3.0.
    pub level_idc: u8,
    /// NAL length size minus one (usually 3 → 4-byte lengths).
    pub length_size_minus_one: u8,
    /// SPS NAL units (with headers).
    pub sps_list: Vec<Vec<u8>>,
    /// PPS NAL units (with headers).
    pub pps_list: Vec<Vec<u8>>,
}

impl AvcDecoderConfig {
    /// Build from a single SPS/PPS pair.
    pub fn from_sps_pps(sps_nal: &[u8], pps_nal: &[u8]) -> Result<Self> {
        let sps = Sps::parse(sps_nal)?;
        Ok(Self {
            profile_idc: sps.profile_idc,
            profile_compat: sps.constraint_set_flags,
            level_idc: sps.level_idc,
            length_size_minus_one: 3,
            sps_list: vec![sps_nal.to_vec()],
            pps_list: vec![pps_nal.to_vec()],
        })
    }

    /// Serialize to `avcC` box payload.
    pub fn to_avcc(&self) -> Vec<u8> {
        let mut out = vec![
            1, // configurationVersion
            self.profile_idc,
            self.profile_compat,
            self.level_idc,
            0xFC | (self.length_size_minus_one & 0x03),
            0xE0 | (self.sps_list.len() as u8 & 0x1F),
        ];
        for sps in &self.sps_list {
            let len = sps.len() as u16;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(sps);
        }
        out.push(self.pps_list.len() as u8);
        for pps in &self.pps_list {
            let len = pps.len() as u16;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(pps);
        }
        out
    }

    /// Parse `avcC` payload.
    pub fn from_avcc(data: &[u8]) -> Result<Self> {
        if data.len() < 7 {
            return Err(Error::truncated("avcC"));
        }
        let profile_idc = data[1];
        let profile_compat = data[2];
        let level_idc = data[3];
        let length_size_minus_one = data[4] & 0x03;
        let num_sps = (data[5] & 0x1F) as usize;
        let mut i = 6;
        let mut sps_list = Vec::new();
        for _ in 0..num_sps {
            if i + 2 > data.len() {
                return Err(Error::truncated("avcC sps len"));
            }
            let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += 2;
            if i + len > data.len() {
                return Err(Error::truncated("avcC sps"));
            }
            sps_list.push(data[i..i + len].to_vec());
            i += len;
        }
        if i >= data.len() {
            return Err(Error::truncated("avcC pps count"));
        }
        let num_pps = data[i] as usize;
        i += 1;
        let mut pps_list = Vec::new();
        for _ in 0..num_pps {
            if i + 2 > data.len() {
                return Err(Error::truncated("avcC pps len"));
            }
            let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += 2;
            if i + len > data.len() {
                return Err(Error::truncated("avcC pps"));
            }
            pps_list.push(data[i..i + len].to_vec());
            i += len;
        }
        let _ = i;
        Ok(Self {
            profile_idc,
            profile_compat,
            level_idc,
            length_size_minus_one,
            sps_list,
            pps_list,
        })
    }

    /// Length size in bytes for AVCC samples.
    pub fn length_size(&self) -> usize {
        self.length_size_minus_one as usize + 1
    }

    /// Parse first SPS.
    pub fn sps(&self) -> Result<Sps> {
        let nal = self
            .sps_list
            .first()
            .ok_or_else(|| Error::invalid("avcC missing SPS"))?;
        Sps::parse(nal)
    }

    /// Parse first PPS.
    pub fn pps(&self) -> Result<Pps> {
        let nal = self
            .pps_list
            .first()
            .ok_or_else(|| Error::invalid("avcC missing PPS"))?;
        Pps::parse(nal)
    }
}
