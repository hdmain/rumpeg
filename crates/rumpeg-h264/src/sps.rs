//! Sequence parameter set.

use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use crate::nal::{build_nal, nal_rbsp, nal_unit_type, NAL_SPS};

/// Decoded SPS (subset used by this codec).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sps {
    /// Profile IDC (66 = Baseline).
    pub profile_idc: u8,
    /// Constraint flags byte.
    pub constraint_set_flags: u8,
    /// Level IDC (e.g. 30 = 3.0).
    pub level_idc: u8,
    /// `seq_parameter_set_id`.
    pub sps_id: u32,
    /// `log2_max_frame_num_minus4`.
    pub log2_max_frame_num_minus4: u32,
    /// `pic_order_cnt_type`.
    pub pic_order_cnt_type: u32,
    /// `log2_max_pic_order_cnt_lsb_minus4` when poc type is 0.
    pub log2_max_pic_order_cnt_lsb_minus4: u32,
    /// `max_num_ref_frames`.
    pub max_num_ref_frames: u32,
    /// `pic_width_in_mbs_minus1`.
    pub pic_width_in_mbs_minus1: u32,
    /// `pic_height_in_map_units_minus1`.
    pub pic_height_in_map_units_minus1: u32,
    /// `frame_mbs_only_flag`.
    pub frame_mbs_only_flag: bool,
    /// Cropping rectangle.
    pub frame_cropping_flag: bool,
    /// Left crop in chroma samples / 2 units (see H.264).
    pub frame_crop_left: u32,
    /// Right crop.
    pub frame_crop_right: u32,
    /// Top crop.
    pub frame_crop_top: u32,
    /// Bottom crop.
    pub frame_crop_bottom: u32,
}

impl Sps {
    /// Create a Baseline SPS for a luma size (will be MB-aligned).
    pub fn baseline(width: u32, height: u32) -> Self {
        let aw = width.div_ceil(16);
        let ah = height.div_ceil(16);
        let crop_right = (aw * 16 - width) / 2; // in chroma / 2 units for 4:2:0 → luma/2
        let crop_bottom = (ah * 16 - height) / 2;
        // frame_crop_* are in units of 2 luma samples for frame_mbs_only 4:2:0
        Self {
            profile_idc: 66,
            constraint_set_flags: 0x40, // constraint_set1_flag (Baseline)
            level_idc: 30,
            sps_id: 0,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            max_num_ref_frames: 0,
            pic_width_in_mbs_minus1: aw.saturating_sub(1),
            pic_height_in_map_units_minus1: ah.saturating_sub(1),
            frame_mbs_only_flag: true,
            frame_cropping_flag: crop_right > 0 || crop_bottom > 0,
            frame_crop_left: 0,
            frame_crop_right: crop_right,
            frame_crop_top: 0,
            frame_crop_bottom: crop_bottom,
        }
    }

    /// Coded width in pixels (before cropping).
    pub fn coded_width(&self) -> u32 {
        (self.pic_width_in_mbs_minus1 + 1) * 16
    }

    /// Coded height in pixels (before cropping).
    pub fn coded_height(&self) -> u32 {
        (self.pic_height_in_map_units_minus1 + 1) * 16
    }

    /// Display width after cropping.
    pub fn width(&self) -> u32 {
        let mut w = self.coded_width();
        if self.frame_cropping_flag {
            w -= (self.frame_crop_left + self.frame_crop_right) * 2;
        }
        w
    }

    /// Display height after cropping.
    pub fn height(&self) -> u32 {
        let mut h = self.coded_height();
        if self.frame_cropping_flag {
            h -= (self.frame_crop_top + self.frame_crop_bottom) * 2;
        }
        h
    }

    /// Encode to a complete SPS NAL unit.
    pub fn to_nal(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_bits(u32::from(self.profile_idc), 8);
        w.write_bits(u32::from(self.constraint_set_flags), 8);
        w.write_bits(u32::from(self.level_idc), 8);
        w.write_ue(self.sps_id);
        // Baseline: no chroma_format_idc high-profile fields
        w.write_ue(self.log2_max_frame_num_minus4);
        w.write_ue(self.pic_order_cnt_type);
        if self.pic_order_cnt_type == 0 {
            w.write_ue(self.log2_max_pic_order_cnt_lsb_minus4);
        }
        w.write_ue(self.max_num_ref_frames);
        w.write_flag(false); // gaps_in_frame_num_value_allowed_flag
        w.write_ue(self.pic_width_in_mbs_minus1);
        w.write_ue(self.pic_height_in_map_units_minus1);
        w.write_flag(self.frame_mbs_only_flag);
        if !self.frame_mbs_only_flag {
            w.write_flag(false); // mb_adaptive_frame_field_flag
        }
        w.write_flag(false); // direct_8x8_inference_flag
        w.write_flag(self.frame_cropping_flag);
        if self.frame_cropping_flag {
            w.write_ue(self.frame_crop_left);
            w.write_ue(self.frame_crop_right);
            w.write_ue(self.frame_crop_top);
            w.write_ue(self.frame_crop_bottom);
        }
        w.write_flag(false); // vui_parameters_present_flag
        w.write_rbsp_trailing_bits();
        build_nal(3, NAL_SPS, w.as_slice())
    }

    /// Parse an SPS NAL unit.
    pub fn parse(nal: &[u8]) -> Result<Self> {
        if nal_unit_type(nal)? != NAL_SPS {
            return Err(Error::invalid("not an SPS NAL"));
        }
        let rbsp = nal_rbsp(nal)?;
        let mut r = BitReader::new(&rbsp);
        let profile_idc = r.read_bits(8)? as u8;
        let constraint_set_flags = r.read_bits(8)? as u8;
        let level_idc = r.read_bits(8)? as u8;
        let sps_id = r.read_ue()?;

        // High profiles carry extra fields — skip if present.
        if matches!(profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
            let chroma_format_idc = r.read_ue()?;
            if chroma_format_idc == 3 {
                let _ = r.read_flag()?; // separate_colour_plane_flag
            }
            let _ = r.read_ue()?; // bit_depth_luma_minus8
            let _ = r.read_ue()?; // bit_depth_chroma_minus8
            let _ = r.read_flag()?; // qpprime_y_zero_transform_bypass_flag
            if r.read_flag()? {
                // scaling matrix — skip 8 or 12 lists of 16 coeffs (simplified reject)
                return Err(Error::unsupported("SPS scaling matrix"));
            }
        }

        let log2_max_frame_num_minus4 = r.read_ue()?;
        let pic_order_cnt_type = r.read_ue()?;
        let mut log2_max_pic_order_cnt_lsb_minus4 = 0;
        if pic_order_cnt_type == 0 {
            log2_max_pic_order_cnt_lsb_minus4 = r.read_ue()?;
        } else if pic_order_cnt_type == 1 {
            let _ = r.read_flag()?;
            let _ = r.read_se()?;
            let _ = r.read_se()?;
            let n = r.read_ue()?;
            for _ in 0..n {
                let _ = r.read_se()?;
            }
        }
        let max_num_ref_frames = r.read_ue()?;
        let _gaps = r.read_flag()?;
        let pic_width_in_mbs_minus1 = r.read_ue()?;
        let pic_height_in_map_units_minus1 = r.read_ue()?;
        let frame_mbs_only_flag = r.read_flag()?;
        if !frame_mbs_only_flag {
            let _ = r.read_flag()?;
        }
        let _direct = r.read_flag()?;
        let frame_cropping_flag = r.read_flag()?;
        let (mut l, mut rr, mut t, mut b) = (0, 0, 0, 0);
        if frame_cropping_flag {
            l = r.read_ue()?;
            rr = r.read_ue()?;
            t = r.read_ue()?;
            b = r.read_ue()?;
        }
        let _vui = r.read_flag()?;

        Ok(Self {
            profile_idc,
            constraint_set_flags,
            level_idc,
            sps_id,
            log2_max_frame_num_minus4,
            pic_order_cnt_type,
            log2_max_pic_order_cnt_lsb_minus4,
            max_num_ref_frames,
            pic_width_in_mbs_minus1,
            pic_height_in_map_units_minus1,
            frame_mbs_only_flag,
            frame_cropping_flag,
            frame_crop_left: l,
            frame_crop_right: rr,
            frame_crop_top: t,
            frame_crop_bottom: b,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sps_roundtrip() {
        let sps = Sps::baseline(320, 240);
        let nal = sps.to_nal();
        let parsed = Sps::parse(&nal).unwrap();
        assert_eq!(parsed.width(), 320);
        assert_eq!(parsed.height(), 240);
        assert_eq!(parsed.profile_idc, 66);
    }
}
