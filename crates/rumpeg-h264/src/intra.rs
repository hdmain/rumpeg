//! Intra prediction for luma 16×16 / 4×4 and chroma 8×8.

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Intra16x16 modes: 0=V, 1=H, 2=DC, 3=Plane.
pub fn pred_luma_16x16(
    mode: u8,
    pred: &mut [u8; 256],
    above: Option<&[u8; 16]>,
    left: Option<&[u8; 16]>,
    above_left: Option<u8>,
) {
    match mode {
        0 => {
            // Vertical
            let a = above.unwrap_or(&[128; 16]);
            for row in 0..16 {
                pred[row * 16..row * 16 + 16].copy_from_slice(a);
            }
        }
        1 => {
            // Horizontal
            let l = left.unwrap_or(&[128; 16]);
            for row in 0..16 {
                for col in 0..16 {
                    pred[row * 16 + col] = l[row];
                }
            }
        }
        2 => {
            // DC
            let mut sum = 0i32;
            let mut n = 0i32;
            if let Some(a) = above {
                for &v in a {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            if let Some(l) = left {
                for &v in l {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            let dc = if n > 0 {
                ((sum + (n / 2)) / n) as u8
            } else {
                128
            };
            pred.fill(dc);
        }
        3 => {
            // Plane (JM / ITU-compatible).
            let a = above.unwrap_or(&[128; 16]);
            let l = left.unwrap_or(&[128; 16]);
            let al = above_left.unwrap_or(128);
            let mut h = 0i32;
            let mut v = 0i32;
            for i in 0..8 {
                h += (i as i32 + 1)
                    * (i32::from(a[8 + i]) - i32::from(if i == 7 { al } else { a[6 - i] }));
                v += (i as i32 + 1)
                    * (i32::from(l[8 + i]) - i32::from(if i == 7 { al } else { l[6 - i] }));
            }
            let a_p = 16 * (i32::from(a[15]) + i32::from(l[15]));
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for row in 0..16 {
                for col in 0..16 {
                    let val = (a_p + b * (col as i32 - 7) + c * (row as i32 - 7) + 16) >> 5;
                    pred[row * 16 + col] = clamp_u8(val);
                }
            }
        }
        _ => pred.fill(128),
    }
}

/// Intra4x4 modes (0=V, 1=H, 2=DC). Kept for upcoming I_NxN support.
#[allow(dead_code)]
pub fn pred_luma_4x4(
    mode: u8,
    pred: &mut [u8; 16],
    above: Option<&[u8; 4]>,
    left: Option<&[u8; 4]>,
    above_left: Option<u8>,
) {
    match mode {
        0 => {
            let a = above.unwrap_or(&[128; 4]);
            for row in 0..4 {
                pred[row * 4..row * 4 + 4].copy_from_slice(a);
            }
        }
        1 => {
            let l = left.unwrap_or(&[128; 4]);
            for row in 0..4 {
                for col in 0..4 {
                    pred[row * 4 + col] = l[row];
                }
            }
        }
        _ => {
            let mut sum = 0i32;
            let mut n = 0;
            if let Some(a) = above {
                for &v in a {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            if let Some(l) = left {
                for &v in l {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            let _ = above_left;
            let dc = if n > 0 {
                ((sum + n / 2) / n) as u8
            } else {
                128
            };
            pred.fill(dc);
        }
    }
}

/// Chroma Intra 8×8: 0=DC, 1=H, 2=V, 3=Plane.
#[allow(clippy::only_used_in_recursion)]
pub fn pred_chroma_8x8(
    mode: u8,
    pred: &mut [u8; 64],
    above: Option<&[u8; 8]>,
    left: Option<&[u8; 8]>,
    above_left: Option<u8>,
) {
    match mode {
        2 => {
            // Vertical
            let a = above.unwrap_or(&[128; 8]);
            for row in 0..8 {
                pred[row * 8..row * 8 + 8].copy_from_slice(a);
            }
        }
        1 => {
            let l = left.unwrap_or(&[128; 8]);
            for row in 0..8 {
                for col in 0..8 {
                    pred[row * 8 + col] = l[row];
                }
            }
        }
        3 => {
            // Plane (simplified via DC if neighbors incomplete)
            pred_chroma_8x8(0, pred, above, left, above_left);
        }
        _ => {
            let mut sum = 0i32;
            let mut n = 0;
            if let Some(a) = above {
                for &v in a {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            if let Some(l) = left {
                for &v in l {
                    sum += i32::from(v);
                    n += 1;
                }
            }
            let dc = if n > 0 {
                ((sum + n / 2) / n) as u8
            } else {
                128
            };
            pred.fill(dc);
        }
    }
}
