//! 4×4 residual transform (ITU-T H.264 integer DCT / IDCT / Hadamard).

/// Forward 4×4 core transform (encoder), in-place on raster 4×4 (`[row*4+col]`).
pub fn dct4x4(block: &mut [i32; 16]) {
    // Horizontal
    for i in 0..4 {
        let i0 = block[i * 4];
        let i1 = block[i * 4 + 1];
        let i2 = block[i * 4 + 2];
        let i3 = block[i * 4 + 3];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i * 4] = z0 + z1;
        block[i * 4 + 1] = (z3 << 1) + z2;
        block[i * 4 + 2] = z0 - z1;
        block[i * 4 + 3] = z3 - (z2 << 1);
    }
    // Vertical
    for i in 0..4 {
        let i0 = block[i];
        let i1 = block[4 + i];
        let i2 = block[8 + i];
        let i3 = block[12 + i];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i] = z0 + z1;
        block[4 + i] = (z3 << 1) + z2;
        block[8 + i] = z0 - z1;
        block[12 + i] = z3 - (z2 << 1);
    }
}

/// Inverse 4×4 residual transform (decoder).
pub fn idct4x4(block: &mut [i32; 16]) {
    // Horizontal
    for i in 0..4 {
        let i0 = block[i * 4];
        let i1 = block[i * 4 + 1];
        let i2 = block[i * 4 + 2];
        let i3 = block[i * 4 + 3];
        let z0 = i0 + i2;
        let z1 = i0 - i2;
        let z2 = (i1 >> 1) - i3;
        let z3 = i1 + (i3 >> 1);
        block[i * 4] = z0 + z3;
        block[i * 4 + 1] = z1 + z2;
        block[i * 4 + 2] = z1 - z2;
        block[i * 4 + 3] = z0 - z3;
    }
    // Vertical + final shift
    for i in 0..4 {
        let i0 = block[i];
        let i1 = block[4 + i];
        let i2 = block[8 + i];
        let i3 = block[12 + i];
        let z0 = i0 + i2;
        let z1 = i0 - i2;
        let z2 = (i1 >> 1) - i3;
        let z3 = i1 + (i3 >> 1);
        block[i] = (z0 + z3 + 32) >> 6;
        block[4 + i] = (z1 + z2 + 32) >> 6;
        block[8 + i] = (z1 - z2 + 32) >> 6;
        block[12 + i] = (z0 - z3 + 32) >> 6;
    }
}

/// 4×4 Hadamard (luma DC for Intra16x16), in-place.
pub fn hadamard4x4(block: &mut [i32; 16]) {
    for i in 0..4 {
        let i0 = block[i * 4];
        let i1 = block[i * 4 + 1];
        let i2 = block[i * 4 + 2];
        let i3 = block[i * 4 + 3];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i * 4] = z0 + z1;
        block[i * 4 + 1] = z3 + z2;
        block[i * 4 + 2] = z0 - z1;
        block[i * 4 + 3] = z3 - z2;
    }
    for i in 0..4 {
        let i0 = block[i];
        let i1 = block[4 + i];
        let i2 = block[8 + i];
        let i3 = block[12 + i];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i] = (z0 + z1) >> 1;
        block[4 + i] = (z3 + z2) >> 1;
        block[8 + i] = (z0 - z1) >> 1;
        block[12 + i] = (z3 - z2) >> 1;
    }
}

/// Inverse 4×4 Hadamard for luma DC.
pub fn ihadamard4x4(block: &mut [i32; 16]) {
    for i in 0..4 {
        let i0 = block[i * 4];
        let i1 = block[i * 4 + 1];
        let i2 = block[i * 4 + 2];
        let i3 = block[i * 4 + 3];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i * 4] = z0 + z1;
        block[i * 4 + 1] = z3 + z2;
        block[i * 4 + 2] = z0 - z1;
        block[i * 4 + 3] = z3 - z2;
    }
    for i in 0..4 {
        let i0 = block[i];
        let i1 = block[4 + i];
        let i2 = block[8 + i];
        let i3 = block[12 + i];
        let z0 = i0 + i3;
        let z1 = i1 + i2;
        let z2 = i1 - i2;
        let z3 = i0 - i3;
        block[i] = z0 + z1;
        block[4 + i] = z3 + z2;
        block[8 + i] = z0 - z1;
        block[12 + i] = z3 - z2;
    }
}

/// 2×2 Hadamard for chroma DC.
pub fn hadamard2x2(block: &mut [i32; 4]) {
    let t0 = block[0] + block[1];
    let t1 = block[2] + block[3];
    let t2 = block[0] - block[1];
    let t3 = block[2] - block[3];
    block[0] = t0 + t1;
    block[1] = t2 + t3;
    block[2] = t0 - t1;
    block[3] = t2 - t3;
}

/// Inverse 2×2 Hadamard.
pub fn ihadamard2x2(block: &mut [i32; 4]) {
    hadamard2x2(block); // self-inverse up to scale; decoder applies LevelScale separately
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::{forward_quant_4x4, forward_quant_shift, level_scale_4x4};

    #[test]
    fn transform_quant_roundtrip_constant() {
        let qp = 12;
        let mut block = [12i32; 16];
        dct4x4(&mut block);
        let mf = forward_quant_4x4(qp);
        let qbits = forward_quant_shift(qp);
        let f = 1 << (qbits - 1);
        let mut q = [0i32; 16];
        for i in 0..16 {
            let sign = if block[i] < 0 { -1 } else { 1 };
            q[i] = sign * ((block[i].abs() * mf[i] + f) >> qbits);
        }
        let scale = level_scale_4x4(qp);
        let mut r = [0i32; 16];
        for i in 0..16 {
            r[i] = q[i] * scale[i];
        }
        idct4x4(&mut r);
        let mean: i32 = r.iter().sum::<i32>() / 16;
        assert!(
            (mean - 12).abs() <= 2,
            "constant residual mean={mean}, block={r:?}"
        );
    }

    #[test]
    fn transform_quant_roundtrip_ramp() {
        let qp = 12;
        let mut block = [0i32; 16];
        for row in 0..4 {
            for col in 0..4 {
                block[row * 4 + col] = (col + row) as i32 - 8;
            }
        }
        let original = block;
        dct4x4(&mut block);
        let mf = forward_quant_4x4(qp);
        let qbits = forward_quant_shift(qp);
        let f = 1 << (qbits - 1);
        let mut q = [0i32; 16];
        for i in 0..16 {
            let sign = if block[i] < 0 { -1 } else { 1 };
            q[i] = sign * ((block[i].abs() * mf[i] + f) >> qbits);
        }
        let scale = level_scale_4x4(qp);
        for i in 0..16 {
            block[i] = q[i] * scale[i];
        }
        idct4x4(&mut block);
        let mut err = 0i32;
        for i in 0..16 {
            err += (block[i] - original[i]).abs();
        }
        assert!(
            err / 16 <= 3,
            "ramp mean abs err={}, recon={block:?} orig={original:?}",
            err / 16
        );
    }
}
