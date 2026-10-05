//! Slice header encode/decode for IDR I and P slices.

use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use crate::pps::Pps;
use crate::sps::Sps;

/// `I_PCM` mb_type value in an I slice (Table 7-11).
pub const MB_TYPE_I_PCM: u32 = 25;

/// Parsed / constructed slice header fields we care about.
#[derive(Clone, Debug)]
pub struct SliceHeader {
    /// `first_mb_in_slice`.
    pub first_mb_in_slice: u32,
    /// `slice_type` (2 or 7 = I; 0 or 5 = P).
    pub slice_type: u32,
    /// `pic_parameter_set_id`.
    pub pps_id: u32,
    /// `frame_num`.
    pub frame_num: u32,
    /// `idr_pic_id`.
    pub idr_pic_id: u32,
    /// `pic_order_cnt_lsb`.
    pub pic_order_cnt_lsb: u32,
    /// `slice_qp_delta`.
    pub slice_qp_delta: i32,
}

impl SliceHeader {
    /// Default IDR I-slice header.
    pub fn idr_i(pps_id: u32) -> Self {
        Self {
            first_mb_in_slice: 0,
            slice_type: 7, // I
            pps_id,
            frame_num: 0,
            idr_pic_id: 0,
            pic_order_cnt_lsb: 0,
            slice_qp_delta: 0,
        }
    }

    /// Non-IDR P-slice header.
    pub fn p(pps_id: u32, frame_num: u32, poc_lsb: u32) -> Self {
        Self {
            first_mb_in_slice: 0,
            slice_type: 5, // P
            pps_id,
            frame_num,
            idr_pic_id: 0,
            pic_order_cnt_lsb: poc_lsb,
            slice_qp_delta: 0,
        }
    }
}

/// Parse an IDR/non-IDR I-slice header.
pub fn parse_slice_header(
    r: &mut BitReader<'_>,
    sps: &Sps,
    pps: &Pps,
    idr: bool,
) -> Result<SliceHeader> {
    let first_mb_in_slice = r.read_ue()?;
    let slice_type_code = r.read_ue()?;
    let slice_type = slice_type_code % 5;
    if slice_type != 2 {
        return Err(Error::unsupported(format!(
            "only I slices supported in native parser, got slice_type={slice_type_code}"
        )));
    }
    let pps_id = r.read_ue()?;
    if pps_id != pps.pps_id {
        return Err(Error::invalid("pps_id mismatch"));
    }
    let frame_num_bits = sps.log2_max_frame_num_minus4 + 4;
    let frame_num = r.read_bits(frame_num_bits)?;
    if !sps.frame_mbs_only_flag && r.read_flag()? {
        let _ = r.read_flag()?;
    }
    let idr_pic_id = if idr { r.read_ue()? } else { 0 };
    let pic_order_cnt_lsb = if sps.pic_order_cnt_type == 0 {
        r.read_bits(sps.log2_max_pic_order_cnt_lsb_minus4 + 4)?
    } else {
        0
    };
    if pps.redundant_pic_cnt_present_flag {
        let _ = r.read_ue()?;
    }
    if idr {
        let _ = r.read_flag()?; // no_output_of_prior_pics_flag
        let _ = r.read_flag()?; // long_term_reference_flag
    }
    let slice_qp_delta = r.read_se()?;
    if pps.deblocking_filter_control_present_flag {
        let idc = r.read_ue()?;
        if idc != 1 {
            let _ = r.read_se()?;
            let _ = r.read_se()?;
        }
    }
    Ok(SliceHeader {
        first_mb_in_slice,
        slice_type: slice_type_code,
        pps_id,
        frame_num,
        idr_pic_id,
        pic_order_cnt_lsb,
        slice_qp_delta,
    })
}

/// Write IDR I-slice header bits.
pub fn write_idr_slice_header(w: &mut BitWriter, hdr: &SliceHeader, sps: &Sps, pps: &Pps) {
    w.write_ue(hdr.first_mb_in_slice);
    w.write_ue(hdr.slice_type);
    w.write_ue(hdr.pps_id);
    w.write_bits(hdr.frame_num, sps.log2_max_frame_num_minus4 + 4);
    if !sps.frame_mbs_only_flag {
        w.write_flag(false);
    }
    w.write_ue(hdr.idr_pic_id);
    if sps.pic_order_cnt_type == 0 {
        w.write_bits(
            hdr.pic_order_cnt_lsb,
            sps.log2_max_pic_order_cnt_lsb_minus4 + 4,
        );
    }
    if pps.redundant_pic_cnt_present_flag {
        w.write_ue(0);
    }
    w.write_flag(false); // no_output_of_prior_pics_flag
    w.write_flag(false); // long_term_reference_flag
    w.write_se(hdr.slice_qp_delta);
    if pps.deblocking_filter_control_present_flag {
        w.write_ue(0);
    }
}

/// Write non-IDR P-slice header bits (CAVLC Baseline).
pub fn write_p_slice_header(w: &mut BitWriter, hdr: &SliceHeader, sps: &Sps, pps: &Pps) {
    w.write_ue(hdr.first_mb_in_slice);
    w.write_ue(hdr.slice_type);
    w.write_ue(hdr.pps_id);
    w.write_bits(hdr.frame_num, sps.log2_max_frame_num_minus4 + 4);
    if !sps.frame_mbs_only_flag {
        w.write_flag(false);
    }
    if sps.pic_order_cnt_type == 0 {
        w.write_bits(
            hdr.pic_order_cnt_lsb,
            sps.log2_max_pic_order_cnt_lsb_minus4 + 4,
        );
    }
    if pps.redundant_pic_cnt_present_flag {
        w.write_ue(0);
    }
    // num_ref_idx_active_override_flag = 0 → use PPS defaults
    w.write_flag(false);
    // ref_pic_list_modification_flag_l0 = 0
    w.write_flag(false);
    // dec_ref_pic_marking: adaptive_ref_pic_marking_mode_flag = 0
    w.write_flag(false);
    if pps.entropy_coding_mode_flag {
        w.write_ue(0); // cabac_init_idc
    }
    w.write_se(hdr.slice_qp_delta);
    if pps.deblocking_filter_control_present_flag {
        w.write_ue(0);
    }
}
