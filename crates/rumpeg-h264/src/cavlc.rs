//! CAVLC residual encode/decode (paired tables for round-trip fidelity).

#![allow(clippy::needless_range_loop)]

use crate::bitstream::{BitReader, BitWriter};
use crate::error::{Error, Result};
use crate::tables::{
    coeff_token_table, nc_category, run_before_table, total_zeros_table, CoeffTokenCode, ZIGZAG4,
};

/// Decode one residual block into zigzag-ordered levels (`max_coeff` is 16, 15, or 4).
pub fn cavlc_decode_block(
    r: &mut BitReader<'_>,
    n_c: i32,
    max_coeff: usize,
) -> Result<(usize, [i32; 16])> {
    let (total_coeff, trailing_ones) = read_coeff_token(r, n_c, max_coeff)?;
    let mut levels = [0i32; 16];
    if total_coeff == 0 {
        return Ok((0, levels));
    }

    let mut coeff_level = [0i32; 16];
    for i in 0..trailing_ones {
        let sign = r.read_bit()?;
        coeff_level[i] = if sign == 0 { 1 } else { -1 };
    }

    let mut suffix_length = if total_coeff > 10 && trailing_ones < 3 {
        1
    } else {
        0
    };
    for i in trailing_ones..total_coeff {
        let mut level = read_level(r, suffix_length)?;
        // First remaining level cannot be ±1 when TrailingOnes < 3.
        if i == trailing_ones && trailing_ones < 3 {
            level += if level > 0 { 1 } else { -1 };
        }
        coeff_level[i] = level;
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if level.abs() > (3 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }

    let mut zeros_left = if total_coeff < max_coeff {
        if max_coeff == 4 {
            read_total_zeros_chroma_dc(r, total_coeff)?
        } else {
            read_vlc_u8(r, total_zeros_table(total_coeff))? as usize
        }
    } else {
        0
    };

    let mut coeff_num = total_coeff as isize - 1 + zeros_left as isize;
    for i in 0..total_coeff {
        if !(0..max_coeff as isize).contains(&coeff_num) {
            return Err(Error::invalid("CAVLC coeff index OOB"));
        }
        levels[coeff_num as usize] = coeff_level[i];
        if i + 1 == total_coeff {
            break;
        }
        let run = if zeros_left > 0 {
            let rb = if zeros_left > 6 {
                read_vlc_u8(r, run_before_table(7))?
            } else {
                read_vlc_u8(r, run_before_table(zeros_left))?
            };
            rb as usize
        } else {
            0
        };
        zeros_left -= run;
        coeff_num -= 1 + run as isize;
    }

    Ok((total_coeff, levels))
}

/// Encode residual levels already in zigzag order (length `max_coeff`).
pub fn cavlc_encode_block(
    w: &mut BitWriter,
    n_c: i32,
    levels_zz: &[i32],
    max_coeff: usize,
) -> Result<usize> {
    let mut nz: Vec<(usize, i32)> = Vec::new();
    for (i, &lvl) in levels_zz.iter().take(max_coeff).enumerate() {
        if lvl != 0 {
            nz.push((i, lvl));
        }
    }
    let total_coeff = nz.len();
    if total_coeff == 0 {
        write_coeff_token(w, n_c, 0, 0, max_coeff)?;
        return Ok(0);
    }

    let mut trailing_ones = 0usize;
    for &(_, lvl) in nz.iter().rev() {
        if lvl.abs() == 1 && trailing_ones < 3 {
            trailing_ones += 1;
        } else {
            break;
        }
    }

    write_coeff_token(w, n_c, total_coeff, trailing_ones, max_coeff)?;

    for i in 0..trailing_ones {
        let lvl = nz[total_coeff - 1 - i].1;
        w.write_bit(if lvl < 0 { 1 } else { 0 });
    }

    let mut suffix_length = if total_coeff > 10 && trailing_ones < 3 {
        1
    } else {
        0
    };
    for i in trailing_ones..total_coeff {
        let mut lvl = nz[total_coeff - 1 - i].1;
        if i == trailing_ones && trailing_ones < 3 {
            // Encode |level|-1 (first remaining cannot be ±1).
            lvl += if lvl > 0 { -1 } else { 1 };
        }
        write_level(w, lvl, suffix_length);
        // suffix adapt uses the *original* level magnitude
        let orig = nz[total_coeff - 1 - i].1;
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if orig.abs() > (3 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }

    let highest = nz[total_coeff - 1].0;
    let total_zeros = highest - (total_coeff - 1);
    if total_coeff < max_coeff {
        if max_coeff == 4 {
            write_total_zeros_chroma_dc(w, total_coeff, total_zeros)?;
        } else {
            write_vlc_u8(w, total_zeros_table(total_coeff), total_zeros as u8)?;
        }
    }

    let mut zeros_left = total_zeros;
    for i in (1..total_coeff).rev() {
        let run = nz[i].0 - nz[i - 1].0 - 1;
        if zeros_left > 0 {
            if zeros_left > 6 {
                write_vlc_u8(w, run_before_table(7), run as u8)?;
            } else {
                write_vlc_u8(w, run_before_table(zeros_left), run as u8)?;
            }
        }
        zeros_left = zeros_left.saturating_sub(run);
    }
    Ok(total_coeff)
}

fn read_coeff_token(r: &mut BitReader<'_>, n_c: i32, max_coeff: usize) -> Result<(usize, usize)> {
    if max_coeff == 4 {
        return read_coeff_token_from_table(r, coeff_token_table(4));
    }
    if n_c >= 8 {
        let code = r.read_bits(6)? as usize;
        return Ok((code >> 2, code & 3));
    }
    read_coeff_token_from_table(r, coeff_token_table(nc_category(n_c)))
}

fn write_coeff_token(
    w: &mut BitWriter,
    n_c: i32,
    total_coeff: usize,
    trailing_ones: usize,
    max_coeff: usize,
) -> Result<()> {
    if max_coeff == 4 {
        for e in coeff_token_table(4) {
            if e.total_coeff as usize == total_coeff && e.trailing_ones as usize == trailing_ones {
                w.write_bits(e.bits, u32::from(e.len));
                return Ok(());
            }
        }
        return Err(Error::invalid("chroma DC coeff_token"));
    }
    if n_c >= 8 {
        let code = ((total_coeff as u32) << 2) | (trailing_ones as u32);
        w.write_bits(code, 6);
        return Ok(());
    }
    let table = coeff_token_table(nc_category(n_c));
    for e in table {
        if e.total_coeff as usize == total_coeff && e.trailing_ones as usize == trailing_ones {
            w.write_bits(e.bits, u32::from(e.len));
            return Ok(());
        }
    }
    Err(Error::invalid(format!(
        "no coeff_token for tc={total_coeff} t1={trailing_ones} nC={n_c}"
    )))
}

fn read_coeff_token_from_table(
    r: &mut BitReader<'_>,
    table: &[CoeffTokenCode],
) -> Result<(usize, usize)> {
    let mut acc = 0u32;
    for len in 1..=17u8 {
        let bit = u32::from(r.read_bit()?);
        acc = (acc << 1) | bit;
        for e in table {
            if e.len == len && e.bits == acc {
                return Ok((e.total_coeff as usize, e.trailing_ones as usize));
            }
        }
    }
    Err(Error::invalid("coeff_token not found"))
}

fn read_level(r: &mut BitReader<'_>, suffix_length: i32) -> Result<i32> {
    let mut level_prefix = 0u32;
    while r.read_bit()? == 0 {
        level_prefix += 1;
        if level_prefix > 15 {
            return Err(Error::invalid("level_prefix too long"));
        }
    }

    let (level_suffix_size, level_code_base) = if level_prefix < 14 {
        (suffix_length as u32, (level_prefix as i32) << suffix_length)
    } else if level_prefix == 14 {
        if suffix_length == 0 {
            (4, 14)
        } else {
            (suffix_length as u32, (14i32) << suffix_length)
        }
    } else {
        // level_prefix == 15
        (12, 30) // 15 escaped; + level_suffix
    };

    let level_suffix = if level_suffix_size > 0 {
        r.read_bits(level_suffix_size)? as i32
    } else {
        0
    };
    let level_code = level_code_base + level_suffix;

    let level = if (level_code & 1) == 0 {
        (level_code + 2) >> 1
    } else {
        -((level_code + 1) >> 1)
    };
    Ok(level)
}

fn write_level(w: &mut BitWriter, level: i32, suffix_length: i32) {
    let level_code = if level > 0 {
        (level << 1) - 2
    } else {
        (-level << 1) - 1
    };

    if suffix_length == 0 {
        if level_code < 14 {
            for _ in 0..level_code {
                w.write_bit(0);
            }
            w.write_bit(1);
        } else if level_code < 30 {
            for _ in 0..14 {
                w.write_bit(0);
            }
            w.write_bit(1);
            w.write_bits((level_code - 14) as u32, 4);
        } else {
            for _ in 0..15 {
                w.write_bit(0);
            }
            w.write_bit(1);
            w.write_bits((level_code - 30) as u32, 12);
        }
        return;
    }

    let level_prefix = level_code >> suffix_length;
    let level_suffix = level_code & ((1 << suffix_length) - 1);
    if level_prefix < 15 {
        for _ in 0..level_prefix {
            w.write_bit(0);
        }
        w.write_bit(1);
        w.write_bits(level_suffix as u32, suffix_length as u32);
    } else {
        for _ in 0..15 {
            w.write_bit(0);
        }
        w.write_bit(1);
        w.write_bits((level_code - (15 << suffix_length)) as u32, 12);
    }
}

fn read_vlc_u8(r: &mut BitReader<'_>, table: &[(u32, u8, u8)]) -> Result<u8> {
    let mut acc = 0u32;
    for len in 1..=16u8 {
        acc = (acc << 1) | u32::from(r.read_bit()?);
        for &(bits, l, val) in table {
            if l == len && bits == acc {
                return Ok(val);
            }
        }
    }
    Err(Error::invalid("VLC not found"))
}

fn write_vlc_u8(w: &mut BitWriter, table: &[(u32, u8, u8)], value: u8) -> Result<()> {
    for &(bits, len, val) in table {
        if val == value {
            w.write_bits(bits, u32::from(len));
            return Ok(());
        }
    }
    Err(Error::invalid(format!("VLC missing value {value}")))
}

fn read_total_zeros_chroma_dc(r: &mut BitReader<'_>, total_coeff: usize) -> Result<usize> {
    static T: [&[(u32, u8, u8)]; 4] = [
        &[],
        &[(0b1, 1, 0), (0b01, 2, 1), (0b001, 3, 2), (0b000, 3, 3)],
        &[(0b1, 1, 0), (0b01, 2, 1), (0b00, 2, 2)],
        &[(0b1, 1, 0), (0b0, 1, 1)],
    ];
    if !(1..=3).contains(&total_coeff) {
        return Ok(0);
    }
    Ok(read_vlc_u8(r, T[total_coeff])? as usize)
}

fn write_total_zeros_chroma_dc(w: &mut BitWriter, total_coeff: usize, zeros: usize) -> Result<()> {
    static T: [&[(u32, u8, u8)]; 4] = [
        &[],
        &[(0b1, 1, 0), (0b01, 2, 1), (0b001, 3, 2), (0b000, 3, 3)],
        &[(0b1, 1, 0), (0b01, 2, 1), (0b00, 2, 2)],
        &[(0b1, 1, 0), (0b0, 1, 1)],
    ];
    write_vlc_u8(w, T[total_coeff], zeros as u8)
}

/// Convert zigzag levels to raster 4×4.
pub fn zigzag_to_raster(levels: &[i32; 16]) -> [i32; 16] {
    let mut out = [0i32; 16];
    for i in 0..16 {
        out[ZIGZAG4[i]] = levels[i];
    }
    out
}

/// Convert raster 4×4 to zigzag levels.
pub fn raster_to_zigzag(block: &[i32; 16]) -> [i32; 16] {
    let mut out = [0i32; 16];
    for i in 0..16 {
        out[i] = block[ZIGZAG4[i]];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cavlc_roundtrip_sparse() {
        let patterns: [[i32; 16]; 4] = [
            [0; 16],
            {
                let mut a = [0; 16];
                a[0] = 5;
                a
            },
            {
                let mut a = [0; 16];
                a[0] = 3;
                a[1] = -1;
                a[4] = 2;
                a
            },
            {
                let mut a = [0; 16];
                a[0] = 1;
                a[1] = -1;
                a[2] = 1;
                a
            },
        ];
        for levels in patterns {
            let mut w = BitWriter::new();
            cavlc_encode_block(&mut w, 0, &levels, 16).unwrap();
            w.write_rbsp_trailing_bits();
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            let (tc, out) = cavlc_decode_block(&mut r, 0, 16).unwrap();
            assert_eq!(out, levels, "tc={tc}");
        }
    }

    #[test]
    fn cavlc_roundtrip_ac15_and_nc() {
        for n_c in [0, 2, 5, 8] {
            for seed in 0..40i32 {
                let mut levels = [0i32; 16];
                // Pseudo-random sparse AC (max_coeff=15 packing).
                for i in 0..15 {
                    let v = ((seed * 17 + i * 3) % 11) - 5;
                    if v.abs() <= 2 || (seed + i) % 3 == 0 {
                        levels[i as usize] = v;
                    }
                }
                let mut w = BitWriter::new();
                cavlc_encode_block(&mut w, n_c, &levels, 15).unwrap();
                w.write_rbsp_trailing_bits();
                let bytes = w.into_bytes();
                let mut r = BitReader::new(&bytes);
                let (_, out) = cavlc_decode_block(&mut r, n_c, 15).unwrap();
                assert_eq!(out, levels, "nC={n_c} seed={seed}");
            }
        }
    }
}
