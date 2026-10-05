//! Intra macroblock encode/decode (I_PCM, I_16x16 CAVLC).

#![allow(
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::manual_memcpy
)]

use crate::bitstream::{BitReader, BitWriter};
use crate::cavlc::{cavlc_decode_block, cavlc_encode_block, raster_to_zigzag, zigzag_to_raster};
use crate::error::{Error, Result};
use crate::intra::{pred_chroma_8x8, pred_luma_16x16};
use crate::slice::MB_TYPE_I_PCM;
use crate::tables::{forward_quant_4x4, forward_quant_shift, level_scale_4x4};
use crate::transform::{dct4x4, hadamard2x2, hadamard4x4, idct4x4, ihadamard2x2, ihadamard4x4};
use crate::yuv::Yuv420Planar;

/// 4×4 block coding order inside a macroblock.
pub const BLK4_ORDER: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

/// Neighbour total-coeff caches for nC (luma 4×4 grid + chroma).
pub struct NcCache {
    /// Per-picture luma 4×4 TotalCoeff, row-major (`(mb_h*4) * (mb_w*4)`).
    pub luma: Vec<i8>,
    pub chroma_u: Vec<i8>,
    pub chroma_v: Vec<i8>,
    pub mb_w: u32,
    #[allow(dead_code)]
    pub mb_h: u32,
}

impl NcCache {
    pub fn new(mb_w: u32, mb_h: u32) -> Self {
        let lw = (mb_w * 4) as usize;
        let lh = (mb_h * 4) as usize;
        let cw = (mb_w * 2) as usize;
        let ch = (mb_h * 2) as usize;
        Self {
            luma: vec![-1; lw * lh],
            chroma_u: vec![-1; cw * ch],
            chroma_v: vec![-1; cw * ch],
            mb_w,
            mb_h,
        }
    }

    fn luma_nc(&self, bx: i32, by: i32) -> i32 {
        let w = (self.mb_w * 4) as i32;
        let left = if bx > 0 {
            let i = (by * w + bx - 1) as usize;
            Some(self.luma[i])
        } else {
            None
        };
        let above = if by > 0 {
            let i = ((by - 1) * w + bx) as usize;
            Some(self.luma[i])
        } else {
            None
        };
        match (left, above) {
            (Some(a), Some(b)) if a >= 0 && b >= 0 => (i32::from(a) + i32::from(b) + 1) >> 1,
            (Some(a), _) if a >= 0 => i32::from(a),
            (_, Some(b)) if b >= 0 => i32::from(b),
            _ => 0,
        }
    }

    fn set_luma(&mut self, bx: i32, by: i32, tc: usize) {
        let w = (self.mb_w * 4) as i32;
        self.luma[(by * w + bx) as usize] = tc as i8;
    }

    fn chroma_nc(&self, plane: usize, bx: i32, by: i32) -> i32 {
        let w = (self.mb_w * 2) as i32;
        let buf = if plane == 0 {
            &self.chroma_u
        } else {
            &self.chroma_v
        };
        let left = if bx > 0 {
            Some(buf[(by * w + bx - 1) as usize])
        } else {
            None
        };
        let above = if by > 0 {
            Some(buf[((by - 1) * w + bx) as usize])
        } else {
            None
        };
        match (left, above) {
            (Some(a), Some(b)) if a >= 0 && b >= 0 => (i32::from(a) + i32::from(b) + 1) >> 1,
            (Some(a), _) if a >= 0 => i32::from(a),
            (_, Some(b)) if b >= 0 => i32::from(b),
            _ => 0,
        }
    }

    fn set_chroma(&mut self, plane: usize, bx: i32, by: i32, tc: usize) {
        let w = (self.mb_w * 2) as i32;
        let buf = if plane == 0 {
            &mut self.chroma_u
        } else {
            &mut self.chroma_v
        };
        buf[(by * w + bx) as usize] = tc as i8;
    }
}

/// Decode one Intra macroblock (I_PCM or I_16x16).
pub fn decode_intra_mb(
    r: &mut BitReader<'_>,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    qp: i32,
) -> Result<()> {
    let mb_type = r.read_ue()?;
    if mb_type == MB_TYPE_I_PCM {
        while !r.byte_aligned() {
            if r.read_bit()? != 0 {
                return Err(Error::invalid("pcm_alignment_zero_bit must be 0"));
            }
        }
        read_pcm_mb(r, frame, mb_x, mb_y)?;
        // Mark neighbours available with synthetic non-zero nC so subsequent MBs work.
        mark_pcm_nc(nc, mb_x, mb_y);
        return Ok(());
    }
    if mb_type == 0 {
        return Err(Error::unsupported(
            "I_NxN (Intra 4×4) not yet supported — use I_16x16 or I_PCM",
        ));
    }
    if mb_type > 24 {
        return Err(Error::unsupported(format!("unknown mb_type={mb_type}")));
    }

    let idx = mb_type - 1;
    let pred_mode = (idx % 4) as u8;
    let cbp_chroma = (idx / 4) % 3;
    let cbp_luma = if idx / 12 != 0 { 15u32 } else { 0 };

    let chroma_mode = r.read_ue()?;
    if chroma_mode > 3 {
        return Err(Error::invalid("intra_chroma_pred_mode"));
    }
    let mb_qp_delta = r.read_se()?;
    let qp_y = (qp + mb_qp_delta).clamp(0, 51);
    let qp_c = qp_chroma(qp_y);

    decode_i16x16(
        r,
        frame,
        nc,
        mb_x,
        mb_y,
        pred_mode,
        chroma_mode as u8,
        cbp_luma,
        cbp_chroma,
        qp_y,
        qp_c,
    )
}

/// Encode one I_16x16 DC macroblock (CAVLC). Writes reconstruction into `frame`.
pub fn encode_i16x16_dc_mb(
    w: &mut BitWriter,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    qp: i32,
) -> Result<()> {
    encode_i16x16_dc_mb_typed(w, frame, nc, mb_x, mb_y, qp, false)
}

/// Encode I_16x16 DC as an Intra MB inside a **P** slice (`mb_type = 5 + I_mb_type`).
pub fn encode_i16x16_dc_mb_in_p(
    w: &mut BitWriter,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    qp: i32,
) -> Result<()> {
    encode_i16x16_dc_mb_typed(w, frame, nc, mb_x, mb_y, qp, true)
}

fn encode_i16x16_dc_mb_typed(
    w: &mut BitWriter,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    qp: i32,
    in_p_slice: bool,
) -> Result<()> {
    let pred_mode: u8 = 2; // DC
    let cbp_chroma: u32 = 2;
    let cbp_luma: u32 = 15;
    let i_mb_type = 1 + u32::from(pred_mode) + 4 * cbp_chroma + 12;
    let mb_type = if in_p_slice { 5 + i_mb_type } else { i_mb_type };
    w.write_ue(mb_type);
    w.write_ue(0); // chroma DC pred
    w.write_se(0); // mb_qp_delta

    encode_i16x16(
        w,
        frame,
        nc,
        mb_x,
        mb_y,
        pred_mode,
        0,
        cbp_luma,
        cbp_chroma,
        qp,
        qp_chroma(qp),
    )
}

/// Sum of absolute differences for one luma macroblock vs a reference frame.
pub fn mb_luma_sad(curr: &Yuv420Planar, reference: &Yuv420Planar, mb_x: u32, mb_y: u32) -> u32 {
    let w = curr.width as usize;
    let ox = (mb_x * 16) as usize;
    let oy = (mb_y * 16) as usize;
    let mut sad = 0u32;
    for row in 0..16usize {
        let cy = oy + row;
        if cy >= curr.height as usize || cy >= reference.height as usize {
            break;
        }
        let c_off = cy * w + ox;
        let r_off = cy * (reference.width as usize) + ox;
        for col in 0..16usize {
            if ox + col >= w || ox + col >= reference.width as usize {
                break;
            }
            sad += (i32::from(curr.y[c_off + col]) - i32::from(reference.y[r_off + col]))
                .unsigned_abs();
        }
    }
    sad
}

/// Copy one macroblock (Y + UV) from `src` into `dst`.
pub fn copy_mb(dst: &mut Yuv420Planar, src: &Yuv420Planar, mb_x: u32, mb_y: u32) {
    let w = dst.width as usize;
    let ox = (mb_x * 16) as usize;
    let oy = (mb_y * 16) as usize;
    for row in 0..16usize {
        let y = oy + row;
        if y >= dst.height as usize || y >= src.height as usize {
            break;
        }
        let d_off = y * w + ox;
        let s_off = y * (src.width as usize) + ox;
        let n = (16).min(w.saturating_sub(ox)).min(src.width as usize - ox);
        dst.y[d_off..d_off + n].copy_from_slice(&src.y[s_off..s_off + n]);
    }
    let cw = (dst.width / 2) as usize;
    let cox = (mb_x * 8) as usize;
    let coy = (mb_y * 8) as usize;
    for plane in 0..2usize {
        let (dp, sp) = if plane == 0 {
            (&mut dst.u, &src.u)
        } else {
            (&mut dst.v, &src.v)
        };
        for row in 0..8usize {
            let y = coy + row;
            if y >= (dst.height / 2) as usize || y >= (src.height / 2) as usize {
                break;
            }
            let d_off = y * cw + cox;
            let s_off = y * ((src.width / 2) as usize) + cox;
            let n = (8)
                .min(cw.saturating_sub(cox))
                .min((src.width / 2) as usize - cox);
            dp[d_off..d_off + n].copy_from_slice(&sp[s_off..s_off + n]);
        }
    }
}

/// Encode I_PCM macroblock.
pub fn encode_pcm_mb(w: &mut BitWriter, frame: &Yuv420Planar, mb_x: u32, mb_y: u32) {
    w.write_ue(MB_TYPE_I_PCM);
    w.byte_align_zeros();
    write_pcm_mb(w, frame, mb_x, mb_y);
}

fn mark_pcm_nc(nc: &mut NcCache, mb_x: u32, mb_y: u32) {
    let bx0 = (mb_x * 4) as i32;
    let by0 = (mb_y * 4) as i32;
    for by in 0..4 {
        for bx in 0..4 {
            nc.set_luma(bx0 + bx, by0 + by, 16);
        }
    }
    let cx0 = (mb_x * 2) as i32;
    let cy0 = (mb_y * 2) as i32;
    for cy in 0..2 {
        for cx in 0..2 {
            nc.set_chroma(0, cx0 + cx, cy0 + cy, 16);
            nc.set_chroma(1, cx0 + cx, cy0 + cy, 16);
        }
    }
}

fn decode_i16x16(
    r: &mut BitReader<'_>,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    pred_mode: u8,
    chroma_mode: u8,
    cbp_luma: u32,
    cbp_chroma: u32,
    qp_y: i32,
    qp_c: i32,
) -> Result<()> {
    let (above, left, above_left) = luma_neighbors(frame, mb_x, mb_y);
    let mut pred = [0u8; 256];
    pred_luma_16x16(
        pred_mode,
        &mut pred,
        above.as_ref(),
        left.as_ref(),
        above_left,
    );

    // Luma DC (16 coeffs via Hadamard).
    let nc_dc = {
        let bx = (mb_x * 4) as i32;
        let by = (mb_y * 4) as i32;
        // Use top-left 4x4 neighbour context for DC block (spec uses special nC).
        nc.luma_nc(bx, by)
    };
    let (tc_dc, dc_zz) = cavlc_decode_block(r, nc_dc, 16)?;
    let mut dc = zigzag_to_raster(&dc_zz);
    ihadamard4x4(&mut dc);
    dequant_dc_luma(&mut dc, qp_y);

    let scale = level_scale_4x4(qp_y);
    let bx0 = (mb_x * 4) as i32;
    let by0 = (mb_y * 4) as i32;
    let mut residuals = [[0i32; 16]; 16];

    for &blk in &BLK4_ORDER {
        let bx = bx0 + (blk % 4) as i32;
        let by = by0 + (blk / 4) as i32;
        let mut block = [0i32; 16];
        let dc_val = dc[blk];
        let tc = if cbp_luma != 0 {
            let n_c = nc.luma_nc(bx, by);
            let (tc_ac, ac_zz) = cavlc_decode_block(r, n_c, 15)?;
            let ac = zigzag_to_raster(&pad15(ac_zz));
            for j in 1..16 {
                block[j] = ac[j];
            }
            nc.set_luma(bx, by, tc_ac + usize::from(dc_val != 0));
            tc_ac
        } else {
            nc.set_luma(bx, by, usize::from(dc_val != 0));
            0
        };
        let _ = tc;
        // Dequant AC only — DC was already inverse-quantized via Hadamard path.
        for j in 1..16 {
            block[j] *= scale[j];
        }
        block[0] = dc_val;
        idct4x4(&mut block);
        residuals[blk] = block;
    }
    // Fix: for Intra16 DC, TotalCoeff for neighbours includes DC — JM stores TotalCoeff of AC+DC.
    // Our set_luma above already approximates.

    // Reconstruct luma
    let stride = frame.width as usize;
    let origin = (mb_y * 16 * frame.width + mb_x * 16) as usize;
    for blk in 0..16 {
        let bx = (blk % 4) * 4;
        let by = (blk / 4) * 4;
        for row in 0..4 {
            for col in 0..4 {
                let p = i32::from(pred[(by + row) * 16 + (bx + col)]);
                let v = p + residuals[blk][row * 4 + col];
                frame.y[origin + (by + row) * stride + (bx + col)] = clamp_u8(v);
            }
        }
    }

    decode_chroma(r, frame, nc, mb_x, mb_y, chroma_mode, cbp_chroma, qp_c)?;
    let _ = tc_dc;
    Ok(())
}

fn encode_i16x16(
    w: &mut BitWriter,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    pred_mode: u8,
    chroma_mode: u8,
    cbp_luma: u32,
    cbp_chroma: u32,
    qp_y: i32,
    qp_c: i32,
) -> Result<()> {
    let (above, left, above_left) = luma_neighbors(frame, mb_x, mb_y);
    let mut pred = [0u8; 256];
    pred_luma_16x16(
        pred_mode,
        &mut pred,
        above.as_ref(),
        left.as_ref(),
        above_left,
    );

    let stride = frame.width as usize;
    let origin = (mb_y * 16 * frame.width + mb_x * 16) as usize;

    // Snapshot source samples before we overwrite with reconstruction.
    let mut src_y = [0u8; 256];
    for row in 0..16 {
        for col in 0..16 {
            src_y[row * 16 + col] = frame.y[origin + row * stride + col];
        }
    }

    let mut dc_raw = [0i32; 16];
    let mut ac_blocks = [[0i32; 16]; 16];
    let mf = forward_quant_4x4(qp_y);
    let qbits = forward_quant_shift(qp_y);
    let f = 1 << (qbits - 1);
    let scale = level_scale_4x4(qp_y);

    for blk in 0..16 {
        let bx = (blk % 4) * 4;
        let by = (blk / 4) * 4;
        let mut block = [0i32; 16];
        for row in 0..4 {
            for col in 0..4 {
                let src = i32::from(src_y[(by + row) * 16 + (bx + col)]);
                let p = i32::from(pred[(by + row) * 16 + (bx + col)]);
                block[row * 4 + col] = src - p;
            }
        }
        dct4x4(&mut block);
        dc_raw[blk] = block[0];
        let mut q = [0i32; 16];
        for i in 1..16 {
            let sign = if block[i] < 0 { -1 } else { 1 };
            q[i] = sign * ((block[i].abs() * mf[i] + f) >> qbits);
        }
        ac_blocks[blk] = q;
    }

    let mut dc_h = dc_raw;
    hadamard4x4(&mut dc_h);
    let mf0 = mf[0];
    let qbits_dc = qbits + 1;
    let f_dc = 1 << (qbits_dc - 1);
    let mut dc_q = [0i32; 16];
    for i in 0..16 {
        let sign = if dc_h[i] < 0 { -1 } else { 1 };
        dc_q[i] = sign * ((dc_h[i].abs() * mf0 + f_dc) >> qbits_dc);
    }

    let bx0 = (mb_x * 4) as i32;
    let by0 = (mb_y * 4) as i32;
    let nc_dc = nc.luma_nc(bx0, by0);
    let dc_zz = raster_to_zigzag(&dc_q);
    cavlc_encode_block(w, nc_dc, &dc_zz, 16)?;

    let mut dc_recon = dc_q;
    ihadamard4x4(&mut dc_recon);
    dequant_dc_luma(&mut dc_recon, qp_y);

    let mut residuals = [[0i32; 16]; 16];
    for &blk in &BLK4_ORDER {
        let bx = bx0 + (blk % 4) as i32;
        let by = by0 + (blk / 4) as i32;
        let n_c = nc.luma_nc(bx, by);
        let ac_zz = raster_to_zigzag(&ac_blocks[blk]);
        let mut ac15 = [0i32; 16];
        for i in 0..15 {
            ac15[i] = ac_zz[i + 1];
        }
        let tc = if cbp_luma != 0 {
            cavlc_encode_block(w, n_c, &ac15, 15)?
        } else {
            0
        };
        nc.set_luma(bx, by, tc + usize::from(dc_recon[blk] != 0));

        let mut block = [0i32; 16];
        for j in 1..16 {
            block[j] = ac_blocks[blk][j] * scale[j];
        }
        block[0] = dc_recon[blk];
        idct4x4(&mut block);
        residuals[blk] = block;
    }

    for blk in 0..16 {
        let bx = (blk % 4) * 4;
        let by = (blk / 4) * 4;
        for row in 0..4 {
            for col in 0..4 {
                let p = i32::from(pred[(by + row) * 16 + (bx + col)]);
                let v = p + residuals[blk][row * 4 + col];
                frame.y[origin + (by + row) * stride + (bx + col)] = clamp_u8(v);
            }
        }
    }

    encode_chroma(w, frame, nc, mb_x, mb_y, chroma_mode, cbp_chroma, qp_c)?;
    Ok(())
}

fn decode_chroma(
    r: &mut BitReader<'_>,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    chroma_mode: u8,
    cbp_chroma: u32,
    qp_c: i32,
) -> Result<()> {
    if cbp_chroma == 0 {
        // Still need prediction with zero residual.
        for plane in 0..2 {
            reconstruct_chroma_plane(frame, mb_x, mb_y, plane, chroma_mode, &[[0i32; 16]; 4])?;
            let cx0 = (mb_x * 2) as i32;
            let cy0 = (mb_y * 2) as i32;
            for cy in 0..2 {
                for cx in 0..2 {
                    nc.set_chroma(plane, cx0 + cx, cy0 + cy, 0);
                }
            }
        }
        return Ok(());
    }

    let scale = level_scale_4x4(qp_c);
    for plane in 0..2 {
        // Chroma DC 2x2
        let (tc_dc, dc_zz) = cavlc_decode_block(r, -1, 4)?; // nC=-1 → chroma DC table
        let mut dc = [dc_zz[0], dc_zz[1], dc_zz[2], dc_zz[3]];
        ihadamard2x2(&mut dc);
        dequant_dc_chroma(&mut dc, qp_c);

        let mut residuals = [[0i32; 16]; 4];
        let cx0 = (mb_x * 2) as i32;
        let cy0 = (mb_y * 2) as i32;
        for blk in 0..4 {
            let cx = cx0 + (blk % 2) as i32;
            let cy = cy0 + (blk / 2) as i32;
            let mut block = [0i32; 16];
            let dc_val = dc[blk];
            let tc = if cbp_chroma == 2 {
                let n_c = nc.chroma_nc(plane, cx, cy);
                let (tc_ac, ac_zz) = cavlc_decode_block(r, n_c, 15)?;
                let ac = zigzag_to_raster(&pad15(ac_zz));
                for j in 1..16 {
                    block[j] = ac[j];
                }
                nc.set_chroma(plane, cx, cy, tc_ac + usize::from(dc_val != 0));
                tc_ac
            } else {
                nc.set_chroma(plane, cx, cy, usize::from(dc_val != 0));
                0
            };
            let _ = tc;
            for j in 1..16 {
                block[j] *= scale[j];
            }
            block[0] = dc_val;
            idct4x4(&mut block);
            residuals[blk] = block;
        }
        let _ = tc_dc;
        reconstruct_chroma_plane(frame, mb_x, mb_y, plane, chroma_mode, &residuals)?;
    }
    Ok(())
}

fn encode_chroma(
    w: &mut BitWriter,
    frame: &mut Yuv420Planar,
    nc: &mut NcCache,
    mb_x: u32,
    mb_y: u32,
    chroma_mode: u8,
    cbp_chroma: u32,
    qp_c: i32,
) -> Result<()> {
    if cbp_chroma == 0 {
        for plane in 0..2 {
            reconstruct_chroma_plane(frame, mb_x, mb_y, plane, chroma_mode, &[[0i32; 16]; 4])?;
            let cx0 = (mb_x * 2) as i32;
            let cy0 = (mb_y * 2) as i32;
            for cy in 0..2 {
                for cx in 0..2 {
                    nc.set_chroma(plane, cx0 + cx, cy0 + cy, 0);
                }
            }
        }
        return Ok(());
    }
    let mf = forward_quant_4x4(qp_c);
    let qbits = forward_quant_shift(qp_c);
    let f = 1 << (qbits - 1);
    let scale = level_scale_4x4(qp_c);
    let cstride = (frame.width / 2) as usize;
    let corigin = (mb_y * 8 * (frame.width / 2) + mb_x * 8) as usize;

    for plane in 0..2 {
        let (above, left, above_left) = chroma_neighbors(frame, mb_x, mb_y, plane);
        let mut pred = [0u8; 64];
        pred_chroma_8x8(
            chroma_mode,
            &mut pred,
            above.as_ref(),
            left.as_ref(),
            above_left,
        );

        let mut src_c = [0u8; 64];
        {
            let plane_data = if plane == 0 { &frame.u } else { &frame.v };
            for row in 0..8 {
                for col in 0..8 {
                    src_c[row * 8 + col] = plane_data[corigin + row * cstride + col];
                }
            }
        }

        let mut dc_raw = [0i32; 4];
        let mut ac_blocks = [[0i32; 16]; 4];
        for blk in 0..4 {
            let bx = (blk % 2) * 4;
            let by = (blk / 2) * 4;
            let mut block = [0i32; 16];
            for row in 0..4 {
                for col in 0..4 {
                    let src = i32::from(src_c[(by + row) * 8 + (bx + col)]);
                    let p = i32::from(pred[(by + row) * 8 + (bx + col)]);
                    block[row * 4 + col] = src - p;
                }
            }
            dct4x4(&mut block);
            dc_raw[blk] = block[0];
            let mut q = [0i32; 16];
            for i in 1..16 {
                let sign = if block[i] < 0 { -1 } else { 1 };
                q[i] = sign * ((block[i].abs() * mf[i] + f) >> qbits);
            }
            ac_blocks[blk] = q;
        }
        let mut dc_h = dc_raw;
        hadamard2x2(&mut dc_h);
        let mf0 = mf[0];
        let qbits_dc = qbits + 1;
        let f_dc = 1 << (qbits_dc - 1);
        let mut dc_q = [0i32; 4];
        for i in 0..4 {
            let sign = if dc_h[i] < 0 { -1 } else { 1 };
            dc_q[i] = sign * ((dc_h[i].abs() * mf0 + f_dc) >> qbits_dc);
        }
        let mut dc_zz = [0i32; 16];
        dc_zz[0] = dc_q[0];
        dc_zz[1] = dc_q[1];
        dc_zz[2] = dc_q[2];
        dc_zz[3] = dc_q[3];
        cavlc_encode_block(w, -1, &dc_zz, 4)?;

        let mut dc_recon = dc_q;
        ihadamard2x2(&mut dc_recon);
        dequant_dc_chroma(&mut dc_recon, qp_c);

        let cx0 = (mb_x * 2) as i32;
        let cy0 = (mb_y * 2) as i32;
        let mut residuals = [[0i32; 16]; 4];
        for blk in 0..4 {
            let cx = cx0 + (blk % 2) as i32;
            let cy = cy0 + (blk / 2) as i32;
            let tc = if cbp_chroma == 2 {
                let n_c = nc.chroma_nc(plane, cx, cy);
                let ac_zz = raster_to_zigzag(&ac_blocks[blk]);
                let mut ac15 = [0i32; 16];
                for i in 0..15 {
                    ac15[i] = ac_zz[i + 1];
                }
                let tc = cavlc_encode_block(w, n_c, &ac15, 15)?;
                nc.set_chroma(plane, cx, cy, tc + usize::from(dc_recon[blk] != 0));
                tc
            } else {
                nc.set_chroma(plane, cx, cy, usize::from(dc_recon[blk] != 0));
                0
            };
            let _ = tc;
            let mut block = [0i32; 16];
            for j in 1..16 {
                block[j] = ac_blocks[blk][j] * scale[j];
            }
            block[0] = dc_recon[blk];
            idct4x4(&mut block);
            residuals[blk] = block;
        }
        reconstruct_chroma_plane(frame, mb_x, mb_y, plane, chroma_mode, &residuals)?;
    }
    Ok(())
}

fn reconstruct_chroma_plane(
    frame: &mut Yuv420Planar,
    mb_x: u32,
    mb_y: u32,
    plane: usize,
    chroma_mode: u8,
    residuals: &[[i32; 16]; 4],
) -> Result<()> {
    let (above, left, above_left) = chroma_neighbors(frame, mb_x, mb_y, plane);
    let mut pred = [0u8; 64];
    pred_chroma_8x8(
        chroma_mode,
        &mut pred,
        above.as_ref(),
        left.as_ref(),
        above_left,
    );
    let cstride = (frame.width / 2) as usize;
    let corigin = (mb_y * 8 * (frame.width / 2) + mb_x * 8) as usize;
    let plane_data = if plane == 0 {
        &mut frame.u
    } else {
        &mut frame.v
    };
    for blk in 0..4 {
        let bx = (blk % 2) * 4;
        let by = (blk / 2) * 4;
        for row in 0..4 {
            for col in 0..4 {
                let p = i32::from(pred[(by + row) * 8 + (bx + col)]);
                let v = p + residuals[blk][row * 4 + col];
                plane_data[corigin + (by + row) * cstride + (bx + col)] = clamp_u8(v);
            }
        }
    }
    Ok(())
}

fn pad15(ac_zz: [i32; 16]) -> [i32; 16] {
    // AC levels occupy zigzag positions 1..15; input is packed as levels[0..14].
    let mut full = [0i32; 16];
    for i in 0..15 {
        full[i + 1] = ac_zz[i];
    }
    full
}

fn dequant_dc_luma(dc: &mut [i32; 16], qp: i32) {
    // LevelScale already includes <<(qp/6); Intra16 DC needs an extra >>2 (ITU 8.5.2).
    let scale = level_scale_4x4(qp)[0];
    let qbits = qp / 6;
    if qp >= 12 {
        for v in dc.iter_mut() {
            *v = (*v * scale) >> 2;
        }
    } else {
        let f = 1 << (1 - qbits);
        let rshift = 2 - qbits;
        for v in dc.iter_mut() {
            // scale = mf << qbits; equivalent to (v * mf + f) >> (2 - qbits) with mf unshifted.
            let mf = scale >> qbits;
            *v = (*v * mf + f) >> rshift;
        }
    }
}

fn dequant_dc_chroma(dc: &mut [i32; 4], qp: i32) {
    // Chroma DC: (c * LevelScale) >> 1 when scale includes <<(qp/6).
    let scale = level_scale_4x4(qp)[0];
    let qbits = qp / 6;
    if qp >= 6 {
        for v in dc.iter_mut() {
            *v = (*v * scale) >> 1;
        }
    } else {
        let mf = scale >> qbits;
        let f = 1 << (0 - qbits); // qp 0..5
        for v in dc.iter_mut() {
            *v = (*v * mf + f) >> (1 - qbits);
        }
    }
}

fn qp_chroma(qp_y: i32) -> i32 {
    // ITU Table 8-15 abbreviated.
    const MAP: [i32; 52] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38,
        39, 39, 39, 39,
    ];
    MAP[qp_y.clamp(0, 51) as usize]
}

fn luma_neighbors(
    frame: &Yuv420Planar,
    mb_x: u32,
    mb_y: u32,
) -> (Option<[u8; 16]>, Option<[u8; 16]>, Option<u8>) {
    let stride = frame.width as usize;
    let above = if mb_y > 0 {
        let mut a = [0u8; 16];
        let start = ((mb_y * 16 - 1) * frame.width + mb_x * 16) as usize;
        a.copy_from_slice(&frame.y[start..start + 16]);
        Some(a)
    } else {
        None
    };
    let left = if mb_x > 0 {
        let mut l = [0u8; 16];
        let ox = (mb_x * 16 - 1) as usize;
        for row in 0..16 {
            l[row] = frame.y[((mb_y * 16 + row as u32) as usize) * stride + ox];
        }
        Some(l)
    } else {
        None
    };
    let above_left = if mb_x > 0 && mb_y > 0 {
        let i = ((mb_y * 16 - 1) * frame.width + (mb_x * 16 - 1)) as usize;
        Some(frame.y[i])
    } else {
        None
    };
    (above, left, above_left)
}

fn chroma_neighbors(
    frame: &Yuv420Planar,
    mb_x: u32,
    mb_y: u32,
    plane: usize,
) -> (Option<[u8; 8]>, Option<[u8; 8]>, Option<u8>) {
    let data = if plane == 0 { &frame.u } else { &frame.v };
    let cw = (frame.width / 2) as usize;
    let above = if mb_y > 0 {
        let mut a = [0u8; 8];
        let start = ((mb_y * 8 - 1) as usize) * cw + (mb_x * 8) as usize;
        a.copy_from_slice(&data[start..start + 8]);
        Some(a)
    } else {
        None
    };
    let left = if mb_x > 0 {
        let mut l = [0u8; 8];
        let ox = (mb_x * 8 - 1) as usize;
        for row in 0..8 {
            l[row] = data[((mb_y * 8 + row as u32) as usize) * cw + ox];
        }
        Some(l)
    } else {
        None
    };
    let above_left = if mb_x > 0 && mb_y > 0 {
        let i = ((mb_y * 8 - 1) as usize) * cw + (mb_x * 8 - 1) as usize;
        Some(data[i])
    } else {
        None
    };
    (above, left, above_left)
}

fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

pub fn read_pcm_mb(
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

pub fn write_pcm_mb(w: &mut BitWriter, frame: &Yuv420Planar, mb_x: u32, mb_y: u32) {
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
