//! Zigzag scans and CAVLC lookup tables (ITU-T H.264 Table 9-5 / 9-7 / 9-8).
//!
//! The coeff_token / total_zeros tables below are transcribed from the exact
//! H.264 VLC tables (the same source `rusty_h264-common` uses for production
//! decoding). They used to be simplified aliases that only round-tripped
//! against our own decoder; aligning them with the spec is what lets a
//! spec-conformant decoder (`rusty_h264-decoder`, ffmpeg, ...) read our CAVLC
//! bitstreams when nC >= 4.

/// 4×4 zigzag scan (frame).
pub const ZIGZAG4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Inverse zigzag: raster index → scan position.
#[allow(dead_code)]
pub const UNZIGZAG4: [usize; 16] = {
    let mut t = [0usize; 16];
    let mut i = 0;
    while i < 16 {
        t[ZIGZAG4[i]] = i;
        i += 1;
    }
    t
};

/// Chroma DC 2×2 zigzag.
#[allow(dead_code)]
pub const ZIGZAG_CHROMA_DC: [usize; 4] = [0, 1, 2, 3];

/// Default 4×4 quantization scaling (flat).
#[allow(dead_code)]
pub fn quant_scale(qp: i32) -> i32 {
    // MF[qp%6] * 2^(qp/6) style — use ITU table for LevelScale with Flat scaling.
    const V: [[i32; 6]; 6] = [
        [10, 13, 16, 13, 10, 13],
        [11, 14, 18, 14, 11, 14],
        [13, 16, 20, 16, 13, 16],
        [14, 18, 23, 18, 14, 18],
        [16, 20, 25, 20, 16, 20],
        [18, 23, 29, 23, 18, 23],
    ];
    let q = qp.clamp(0, 51) as usize;
    let m = q % 6;
    let s = q / 6;
    // Return scale for (0,0) position; callers use full matrix.
    V[0][m] << s
}

/// LevelScale4x4(qP) for flat scaling list (ITU Table 7-3 / 8-13 style).
pub fn level_scale_4x4(qp: i32) -> [i32; 16] {
    // v = LevelScale = normAdjust * Mf * 2^floor(qp/6)
    // Flat: normAdjust positions use {10,13,16} pattern.
    const MF: [[i32; 16]; 6] = [
        [
            10, 13, 10, 13, 13, 16, 13, 16, 10, 13, 10, 13, 13, 16, 13, 16,
        ],
        [
            11, 14, 11, 14, 14, 18, 14, 18, 11, 14, 11, 14, 14, 18, 14, 18,
        ],
        [
            13, 16, 13, 16, 16, 20, 16, 20, 13, 16, 13, 16, 16, 20, 16, 20,
        ],
        [
            14, 18, 14, 18, 18, 23, 18, 23, 14, 18, 14, 18, 18, 23, 18, 23,
        ],
        [
            16, 20, 16, 20, 20, 25, 20, 25, 16, 20, 16, 20, 20, 25, 20, 25,
        ],
        [
            18, 23, 18, 23, 23, 29, 23, 29, 18, 23, 18, 23, 23, 29, 23, 29,
        ],
    ];
    let q = qp.clamp(0, 51) as usize;
    let mut out = [0i32; 16];
    let shift = q / 6;
    for i in 0..16 {
        out[i] = MF[q % 6][i] << shift;
    }
    out
}

/// Forward quant multipliers for encoder (approx inverse of LevelScale).
pub fn forward_quant_4x4(qp: i32) -> [i32; 16] {
    // Use MF from JM-style forward quant: MF[q%6] then >> (15+q/6) with rounding.
    const MF: [[i32; 16]; 6] = [
        [
            13107, 8066, 13107, 8066, 8066, 5243, 8066, 5243, 13107, 8066, 13107, 8066, 8066, 5243,
            8066, 5243,
        ],
        [
            11916, 7490, 11916, 7490, 7490, 4660, 7490, 4660, 11916, 7490, 11916, 7490, 7490, 4660,
            7490, 4660,
        ],
        [
            10082, 6554, 10082, 6554, 6554, 4194, 6554, 4194, 10082, 6554, 10082, 6554, 6554, 4194,
            6554, 4194,
        ],
        [
            9362, 5825, 9362, 5825, 5825, 3647, 5825, 3647, 9362, 5825, 9362, 5825, 5825, 3647,
            5825, 3647,
        ],
        [
            8192, 5243, 8192, 5243, 5243, 3355, 5243, 3355, 8192, 5243, 8192, 5243, 5243, 3355,
            5243, 3355,
        ],
        [
            7282, 4559, 7282, 4559, 4559, 2893, 4559, 2893, 7282, 4559, 7282, 4559, 4559, 2893,
            4559, 2893,
        ],
    ];
    let q = qp.clamp(0, 51) as usize;
    MF[q % 6]
}

pub fn forward_quant_shift(qp: i32) -> i32 {
    15 + qp.clamp(0, 51) / 6
}

/// coeff_token VLC entry: codeword in the low `len` bits of `bits`.
#[derive(Clone, Copy)]
pub struct CoeffTokenCode {
    pub bits: u32,
    pub len: u8,
    pub total_coeff: u8,
    pub trailing_ones: u8,
}

/// Build a coeff_token list from an ITU Table 9-5 column.
///
/// Index into the source tables is `TotalCoeff * 4 + TrailingOnes`; a zero
/// length means an undefined codeword (e.g. TrailingOnes > TotalCoeff). Exactly
/// 62 combinations are defined (1 + 2 + 3 + 14*4).
const fn build_coeff_tokens(cat: usize) -> [CoeffTokenCode; 62] {
    let mut out = [CoeffTokenCode {
        bits: 0,
        len: 0,
        total_coeff: 0,
        trailing_ones: 0,
    }; 62];
    let mut k = 0usize;
    let mut i = 0usize;
    while i < 68 {
        let len = COEFF_TOKEN_LEN[cat][i];
        if len != 0 {
            out[k] = CoeffTokenCode {
                bits: COEFF_TOKEN_BITS[cat][i] as u32,
                len,
                total_coeff: (i / 4) as u8,
                trailing_ones: (i % 4) as u8,
            };
            k += 1;
        }
        i += 1;
    }
    out
}

/// Build a chroma-DC coeff_token list (max 4 coefficients, 14 defined codes).
const fn build_chroma_dc_coeff_tokens() -> [CoeffTokenCode; 14] {
    let mut out = [CoeffTokenCode {
        bits: 0,
        len: 0,
        total_coeff: 0,
        trailing_ones: 0,
    }; 14];
    let mut k = 0usize;
    let mut i = 0usize;
    while i < 20 {
        let len = CHROMA_DC_COEFF_TOKEN_LEN[i];
        if len != 0 {
            out[k] = CoeffTokenCode {
                bits: CHROMA_DC_COEFF_TOKEN_BITS[i] as u32,
                len,
                total_coeff: (i / 4) as u8,
                trailing_ones: (i % 4) as u8,
            };
            k += 1;
        }
        i += 1;
    }
    out
}

// ITU-T H.264 Table 9-5 coeff_token codeword lengths, indexed
// [nC table][TotalCoeff * 4 + TrailingOnes]. 0 = undefined.
#[rustfmt::skip]
const COEFF_TOKEN_LEN: [[u8; 68]; 4] = [
    [
        1,0,0,0,
        6,2,0,0,  8,6,3,0,  9,8,7,5,  10,9,8,6,
        11,10,9,7, 13,11,10,8, 13,13,11,9, 13,13,13,10,
        14,14,13,11, 14,14,14,13, 15,15,14,14, 15,15,15,14,
        16,15,15,15, 16,16,16,15, 16,16,16,16, 16,16,16,16,
    ],
    [
        2,0,0,0,
        6,2,0,0,  6,5,3,0,  7,6,6,4,  8,6,6,4,
        8,7,7,5,  9,8,8,6,  11,9,9,6,  11,11,11,7,
        12,11,11,9, 12,12,12,11, 12,12,12,11, 13,13,13,12,
        13,13,13,13, 13,14,13,13, 14,14,14,13, 14,14,14,14,
    ],
    [
        4,0,0,0,
        6,4,0,0,  6,5,4,0,  6,5,5,4,  7,5,5,4,
        7,5,5,4,  7,6,6,4,  7,6,6,4,  8,7,7,5,
        8,8,7,6,  9,8,8,7,  9,9,8,8,  9,9,9,8,
        10,9,9,9, 10,10,10,10, 10,10,10,10, 10,10,10,10,
    ],
    [
        6,0,0,0,
        6,6,0,0,  6,6,6,0,  6,6,6,6,  6,6,6,6,
        6,6,6,6,  6,6,6,6,  6,6,6,6,  6,6,6,6,
        6,6,6,6,  6,6,6,6,  6,6,6,6,  6,6,6,6,
        6,6,6,6,  6,6,6,6,  6,6,6,6,  6,6,6,6,
    ],
];

// ITU-T H.264 Table 9-5 coeff_token codeword values.
#[rustfmt::skip]
const COEFF_TOKEN_BITS: [[u8; 68]; 4] = [
    [
        1,0,0,0,
        5,1,0,0,  7,4,1,0,  7,6,5,3,  7,6,5,3,
        7,6,5,4,  15,6,5,4,  11,14,5,4, 8,10,13,4,
        15,14,9,4, 11,10,13,12, 15,14,9,12, 11,10,13,8,
        15,1,9,12, 11,14,13,8,  7,10,9,12, 4,6,5,8,
    ],
    [
        3,0,0,0,
        11,2,0,0,  7,7,3,0,  7,10,9,5,  7,6,5,4,
        4,6,5,6,  7,6,5,8,  15,6,5,4,  11,14,13,4,
        15,10,9,4, 11,14,13,12, 8,10,9,8, 15,14,13,12,
        11,10,9,12, 7,11,6,8,  9,8,10,1,  7,6,5,4,
    ],
    [
        15,0,0,0,
        15,14,0,0, 11,15,13,0, 8,12,14,12, 15,10,11,11,
        11,8,9,10, 9,14,13,9,  8,10,9,8, 15,14,13,13,
        11,14,10,12, 15,10,13,12, 11,14,9,12, 8,10,13,8,
        13,7,9,12,  9,12,11,10, 5,8,7,6,  1,4,3,2,
    ],
    [
        3,0,0,0,
        0,1,0,0,  4,5,6,0,  8,9,10,11, 12,13,14,15,
        16,17,18,19, 20,21,22,23, 24,25,26,27, 28,29,30,31,
        32,33,34,35, 36,37,38,39, 40,41,42,43, 44,45,46,47,
        48,49,50,51, 52,53,54,55, 56,57,58,59, 60,61,62,63,
    ],
];

// Chroma DC (2x2) coeff_token, index = TotalCoeff*4 + TrailingOnes.
#[rustfmt::skip]
const CHROMA_DC_COEFF_TOKEN_LEN: [u8; 20] = [
    2,0,0,0,  6,1,0,0,  6,6,3,0,  6,7,7,6,  6,8,8,7,
];
#[rustfmt::skip]
const CHROMA_DC_COEFF_TOKEN_BITS: [u8; 20] = [
    1,0,0,0,  7,1,0,0,  4,6,1,0,  3,3,2,5,  2,3,2,0,
];

static COEFF_NC0: [CoeffTokenCode; 62] = build_coeff_tokens(0);
static COEFF_NC2: [CoeffTokenCode; 62] = build_coeff_tokens(1);
static COEFF_NC4: [CoeffTokenCode; 62] = build_coeff_tokens(2);
static COEFF_NC8: [CoeffTokenCode; 62] = build_coeff_tokens(3);
static COEFF_CHROMA_DC: [CoeffTokenCode; 14] = build_chroma_dc_coeff_tokens();

/// nC category: 0 for nC<2, 1 for 2..3, 2 for 4..7, 3 for >=8.
pub fn nc_category(n_c: i32) -> usize {
    if n_c < 2 {
        0
    } else if n_c < 4 {
        1
    } else if n_c < 8 {
        2
    } else {
        3
    }
}

/// Runtime VLC list for a coeff_token nC category.
/// Category 4 selects the chroma-DC table.
pub fn coeff_token_table(nc_cat: usize) -> &'static [CoeffTokenCode] {
    match nc_cat {
        0 => &COEFF_NC0,
        1 => &COEFF_NC2,
        2 => &COEFF_NC4,
        3 => &COEFF_NC8,
        _ => &COEFF_CHROMA_DC,
    }
}

// --- total_zeros (ITU Table 9-7), indexed [TotalCoeff-1][total_zeros] ---

#[rustfmt::skip]
const TOTAL_ZEROS_LEN: [[u8; 16]; 15] = [
    [1,3,3,4,4,5,5,6,6,7,7,8,8,9,9,9],
    [3,3,3,3,3,4,4,4,4,5,5,6,6,6,6,0],
    [4,3,3,3,4,4,3,3,4,5,5,6,5,6,0,0],
    [5,3,4,4,3,3,3,4,3,4,5,5,5,0,0,0],
    [4,4,4,3,3,3,3,3,4,5,4,5,0,0,0,0],
    [6,5,3,3,3,3,3,3,4,3,6,0,0,0,0,0],
    [6,5,3,3,3,2,3,4,3,6,0,0,0,0,0,0],
    [6,4,5,3,2,2,3,3,6,0,0,0,0,0,0,0],
    [6,6,4,2,2,3,2,5,0,0,0,0,0,0,0,0],
    [5,5,3,2,2,2,4,0,0,0,0,0,0,0,0,0],
    [4,4,3,3,1,3,0,0,0,0,0,0,0,0,0,0],
    [4,4,2,1,3,0,0,0,0,0,0,0,0,0,0,0],
    [3,3,1,2,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,1,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [1,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
];
#[rustfmt::skip]
const TOTAL_ZEROS_BITS: [[u8; 16]; 15] = [
    [1,3,2,3,2,3,2,3,2,3,2,3,2,3,2,1],
    [7,6,5,4,3,5,4,3,2,3,2,3,2,1,0,0],
    [5,7,6,5,4,3,4,3,2,3,2,1,1,0,0,0],
    [3,7,5,4,6,5,4,3,3,2,2,1,0,0,0,0],
    [5,4,3,7,6,5,4,3,2,1,1,0,0,0,0,0],
    [1,1,7,6,5,4,3,2,1,1,0,0,0,0,0,0],
    [1,1,5,4,3,3,2,1,1,0,0,0,0,0,0,0],
    [1,1,1,3,3,2,2,1,0,0,0,0,0,0,0,0],
    [1,0,1,3,2,1,1,1,0,0,0,0,0,0,0,0],
    [1,0,1,3,2,1,1,0,0,0,0,0,0,0,0,0],
    [0,1,1,2,1,3,0,0,0,0,0,0,0,0,0,0],
    [0,1,1,1,1,0,0,0,0,0,0,0,0,0,0,0],
    [0,1,1,1,0,0,0,0,0,0,0,0,0,0,0,0],
    [0,1,1,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
];

/// Build a `total_zeros` VLC list for one TotalCoeff. Undefined entries get a
/// sentinel value (255) so `write_vlc_u8` can never match them.
const fn build_total_zeros(tc: usize) -> [(u32, u8, u8); 16] {
    let mut out = [(0u32, 0u8, 255u8); 16];
    let mut i = 0usize;
    while i < 16 {
        let len = TOTAL_ZEROS_LEN[tc][i];
        if len != 0 {
            out[i] = (TOTAL_ZEROS_BITS[tc][i] as u32, len, i as u8);
        }
        i += 1;
    }
    out
}

static TOTAL_ZEROS_TABLES: [[(u32, u8, u8); 16]; 15] = [
    build_total_zeros(0),
    build_total_zeros(1),
    build_total_zeros(2),
    build_total_zeros(3),
    build_total_zeros(4),
    build_total_zeros(5),
    build_total_zeros(6),
    build_total_zeros(7),
    build_total_zeros(8),
    build_total_zeros(9),
    build_total_zeros(10),
    build_total_zeros(11),
    build_total_zeros(12),
    build_total_zeros(13),
    build_total_zeros(14),
];
// TotalCoeff == 16 has no zeros to code; only value 0 (len 1, bit 1) is legal.
static TOTAL_ZEROS_EMPTY: [(u32, u8, u8); 1] = [(1, 1, 0)];

/// total_zeros VLC for a 4x4 block (maxNumCoeff=16), for TotalCoeff 1..15.
pub fn total_zeros_table(total_coeff: usize) -> &'static [(u32, u8, u8)] {
    if (1..=15).contains(&total_coeff) {
        &TOTAL_ZEROS_TABLES[total_coeff - 1]
    } else {
        &TOTAL_ZEROS_EMPTY
    }
}

// --- run_before (ITU Table 9-8), indexed [min(zerosLeft,7)-1][run_before] ---

#[rustfmt::skip]
const RUN_LEN: [[u8; 15]; 7] = [
    [1,1,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [1,2,2,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,2,2,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,2,3,3,0,0,0,0,0,0,0,0,0,0],
    [2,2,3,3,3,3,0,0,0,0,0,0,0,0,0],
    [2,3,3,3,3,3,3,0,0,0,0,0,0,0,0],
    [3,3,3,3,3,3,3,4,5,6,7,8,9,10,11],
];
#[rustfmt::skip]
const RUN_BITS: [[u8; 15]; 7] = [
    [1,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [1,1,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [3,2,1,0,0,0,0,0,0,0,0,0,0,0,0],
    [3,2,1,1,0,0,0,0,0,0,0,0,0,0,0],
    [3,2,3,2,1,0,0,0,0,0,0,0,0,0,0],
    [3,0,1,3,2,5,4,0,0,0,0,0,0,0,0],
    [7,6,5,4,3,2,1,1,1,1,1,1,1,1,1],
];

/// Build a `run_before` VLC list for one zerosLeft bucket.
const fn build_run_before(z: usize) -> [(u32, u8, u8); 15] {
    let mut out = [(0u32, 0u8, 255u8); 15];
    let mut i = 0usize;
    while i < 15 {
        let len = RUN_LEN[z][i];
        if len != 0 {
            out[i] = (RUN_BITS[z][i] as u32, len, i as u8);
        }
        i += 1;
    }
    out
}

static RUN_BEFORE_TABLES: [[(u32, u8, u8); 15]; 7] = [
    build_run_before(0),
    build_run_before(1),
    build_run_before(2),
    build_run_before(3),
    build_run_before(4),
    build_run_before(5),
    build_run_before(6),
];

/// run_before VLC for `zeros_left` (buckets 1..=6, 7 for >=7).
pub fn run_before_table(zeros_left: usize) -> &'static [(u32, u8, u8)] {
    let z = zeros_left.clamp(1, 7) - 1;
    &RUN_BEFORE_TABLES[z]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coeff_token_tables_have_expected_entries() {
        // Each luma table defines exactly 62 codewords; chroma DC defines 14.
        assert_eq!(coeff_token_table(0).len(), 62);
        assert_eq!(coeff_token_table(1).len(), 62);
        assert_eq!(coeff_token_table(2).len(), 62);
        assert_eq!(coeff_token_table(3).len(), 62);
        assert_eq!(coeff_token_table(4).len(), 14);
    }

    #[test]
    fn n4_n8_tables_differ_from_n2() {
        // Regression for the "COEFF_NC4/NC8 = COEFF_NC2" alias bug: TotalCoeff=0
        // is 2 bits (0b11) in nC2 but 4 bits (0b1111) in nC4 and 6 bits in nC8.
        let find = |cat: usize, tc: u8, t1: u8| -> (u32, u8) {
            coeff_token_table(cat)
                .iter()
                .find(|e| e.total_coeff == tc && e.trailing_ones == t1)
                .map(|e| (e.bits, e.len))
                .unwrap()
        };
        assert_eq!(find(1, 0, 0), (0b11, 2));
        assert_eq!(find(2, 0, 0), (0b1111, 4));
        assert_eq!(find(3, 0, 0), (0b11, 6));
    }
}
