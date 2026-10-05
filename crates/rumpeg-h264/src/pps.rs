//! Picture parameter set.

use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use crate::nal::{build_nal, nal_rbsp, nal_unit_type, NAL_PPS};

/// Decoded PPS (subset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pps {
    /// `pic_parameter_set_id`.
    pub pps_id: u32,
    /// `seq_parameter_set_id`.
    pub sps_id: u32,
    /// `entropy_coding_mode_flag` (0 = CAVLC).
    pub entropy_coding_mode_flag: bool,
    /// `num_ref_idx_l0_default_active_minus1`.
    pub num_ref_idx_l0_default_active_minus1: u32,
    /// `weighted_pred_flag`.
    pub weighted_pred_flag: bool,
    /// `pic_init_qp_minus26`.
    pub pic_init_qp_minus26: i32,
    /// `chroma_qp_index_offset`.
    pub chroma_qp_index_offset: i32,
    /// `deblocking_filter_control_present_flag`.
    pub deblocking_filter_control_present_flag: bool,
    /// `constrained_intra_pred_flag`.
    pub constrained_intra_pred_flag: bool,
    /// `redundant_pic_cnt_present_flag`.
    pub redundant_pic_cnt_present_flag: bool,
}

impl Pps {
    /// Default PPS referencing SPS 0, CAVLC, QP 26.
    pub fn baseline() -> Self {
        Self {
            pps_id: 0,
            sps_id: 0,
            entropy_coding_mode_flag: false,
            num_ref_idx_l0_default_active_minus1: 0,
            weighted_pred_flag: false,
            pic_init_qp_minus26: 0,
            chroma_qp_index_offset: 0,
            deblocking_filter_control_present_flag: false,
            constrained_intra_pred_flag: false,
            redundant_pic_cnt_present_flag: false,
        }
    }

    /// Encode to PPS NAL.
    pub fn to_nal(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_ue(self.pps_id);
        w.write_ue(self.sps_id);
        w.write_flag(self.entropy_coding_mode_flag);
        w.write_flag(false); // bottom_field_pic_order_in_frame_present_flag
        w.write_ue(0); // num_slice_groups_minus1
        w.write_ue(self.num_ref_idx_l0_default_active_minus1);
        w.write_ue(0); // num_ref_idx_l1_default_active_minus1
        w.write_flag(self.weighted_pred_flag);
        w.write_bits(0, 2); // weighted_bipred_idc
        w.write_se(self.pic_init_qp_minus26);
        w.write_se(0); // pic_init_qs_minus26
        w.write_se(self.chroma_qp_index_offset);
        w.write_flag(self.deblocking_filter_control_present_flag);
        w.write_flag(self.constrained_intra_pred_flag);
        w.write_flag(self.redundant_pic_cnt_present_flag);
        w.write_rbsp_trailing_bits();
        build_nal(3, NAL_PPS, w.as_slice())
    }

    /// Parse PPS NAL.
    pub fn parse(nal: &[u8]) -> Result<Self> {
        if nal_unit_type(nal)? != NAL_PPS {
            return Err(Error::invalid("not a PPS NAL"));
        }
        let rbsp = nal_rbsp(nal)?;
        let mut r = BitReader::new(&rbsp);
        let pps_id = r.read_ue()?;
        let sps_id = r.read_ue()?;
        let entropy_coding_mode_flag = r.read_flag()?;
        let _bottom = r.read_flag()?;
        let num_slice_groups_minus1 = r.read_ue()?;
        if num_slice_groups_minus1 > 0 {
            return Err(Error::unsupported("slice groups"));
        }
        let num_ref_idx_l0_default_active_minus1 = r.read_ue()?;
        let _l1 = r.read_ue()?;
        let weighted_pred_flag = r.read_flag()?;
        let _weighted_bipred_idc = r.read_bits(2)?;
        let pic_init_qp_minus26 = r.read_se()?;
        let _qs = r.read_se()?;
        let chroma_qp_index_offset = r.read_se()?;
        let deblocking_filter_control_present_flag = r.read_flag()?;
        let constrained_intra_pred_flag = r.read_flag()?;
        let redundant_pic_cnt_present_flag = r.read_flag()?;
        Ok(Self {
            pps_id,
            sps_id,
            entropy_coding_mode_flag,
            num_ref_idx_l0_default_active_minus1,
            weighted_pred_flag,
            pic_init_qp_minus26,
            chroma_qp_index_offset,
            deblocking_filter_control_present_flag,
            constrained_intra_pred_flag,
            redundant_pic_cnt_present_flag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pps_roundtrip() {
        let pps = Pps::baseline();
        let nal = pps.to_nal();
        let parsed = Pps::parse(&nal).unwrap();
        assert_eq!(parsed, pps);
    }
}
