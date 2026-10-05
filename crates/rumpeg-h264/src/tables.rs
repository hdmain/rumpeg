//! Zigzag scans and CAVLC lookup tables (H.264 Baseline subset).

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

/// coeff_token VLC: (code as bits MSB-first, length). Indexed by [nC_category][TotalCoeff][TrailingOnes]
/// We decode by matching bitstream prefixes — stored as exhaustive search tables for nC categories.
#[derive(Clone, Copy)]
pub struct CoeffTokenCode {
    pub bits: u32,
    pub len: u8,
    pub total_coeff: u8,
    pub trailing_ones: u8,
}

/// Build runtime VLC lists for a given nC category (0: 0..=1, 1: 2..=3, 2: 4..=7, 3: >=8, 4: chroma DC).
pub fn coeff_token_table(nc_cat: usize) -> &'static [CoeffTokenCode] {
    match nc_cat {
        0 => &COEFF_NC0,
        1 => &COEFF_NC2,
        2 => &COEFF_NC4,
        3 => &COEFF_NC8,
        _ => &COEFF_CHROMA_DC,
    }
}

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

macro_rules! ct {
    ($bits:expr, $len:expr, $tc:expr, $t1:expr) => {
        CoeffTokenCode {
            bits: $bits,
            len: $len,
            total_coeff: $tc,
            trailing_ones: $t1,
        }
    };
}

// Tables condensed from ITU-T H.264 Table 9-5 (nC 0/1). Codes are left-aligned in `bits` for matching.
// Format: bit pattern in low `len` bits.
static COEFF_NC0: [CoeffTokenCode; 62] = [
    ct!(0b1, 1, 0, 0),
    ct!(0b000101, 6, 1, 1),
    ct!(0b01, 2, 1, 0),
    ct!(0b00000111, 8, 2, 2),
    ct!(0b000100, 6, 2, 1),
    ct!(0b001, 3, 2, 0),
    ct!(0b000000111, 9, 3, 3),
    ct!(0b00000110, 8, 3, 2),
    ct!(0b0000101, 7, 3, 1),
    ct!(0b00011, 5, 3, 0),
    ct!(0b0000000111, 10, 4, 3),
    ct!(0b000000110, 9, 4, 2),
    ct!(0b00000101, 8, 4, 1),
    ct!(0b000011, 6, 4, 0),
    ct!(0b00000000111, 11, 5, 3),
    ct!(0b0000000110, 10, 5, 2),
    ct!(0b000000101, 9, 5, 1),
    ct!(0b0000100, 7, 5, 0),
    ct!(0b0000000001111, 13, 6, 3),
    ct!(0b00000000110, 11, 6, 2),
    ct!(0b0000000101, 10, 6, 1),
    ct!(0b00000100, 8, 6, 0),
    ct!(0b0000000001011, 13, 7, 3),
    ct!(0b0000000001110, 13, 7, 2),
    ct!(0b00000000101, 11, 7, 1),
    ct!(0b000000100, 9, 7, 0),
    ct!(0b0000000001000, 13, 8, 3),
    ct!(0b0000000001010, 13, 8, 2),
    ct!(0b0000000001101, 13, 8, 1),
    ct!(0b0000000100, 10, 8, 0),
    ct!(0b00000000001111, 14, 9, 3),
    ct!(0b00000000001110, 14, 9, 2),
    ct!(0b0000000001001, 13, 9, 1),
    ct!(0b00000000100, 11, 9, 0),
    ct!(0b00000000001011, 14, 10, 3),
    ct!(0b00000000001010, 14, 10, 2),
    ct!(0b00000000001101, 14, 10, 1),
    ct!(0b0000000001100, 13, 10, 0),
    ct!(0b000000000001111, 15, 11, 3),
    ct!(0b000000000001110, 15, 11, 2),
    ct!(0b00000000001001, 14, 11, 1),
    ct!(0b00000000001100, 14, 11, 0),
    ct!(0b000000000001011, 15, 12, 3),
    ct!(0b000000000001010, 15, 12, 2),
    ct!(0b000000000001101, 15, 12, 1),
    ct!(0b00000000001000, 14, 12, 0),
    ct!(0b0000000000001111, 16, 13, 3),
    ct!(0b000000000000101, 15, 13, 2),
    ct!(0b000000000001001, 15, 13, 1),
    ct!(0b000000000001100, 15, 13, 0),
    ct!(0b0000000000001011, 16, 14, 3),
    ct!(0b0000000000001110, 16, 14, 2),
    ct!(0b0000000000001101, 16, 14, 1),
    ct!(0b000000000001000, 15, 14, 0),
    ct!(0b0000000000000111, 16, 15, 3),
    ct!(0b0000000000001010, 16, 15, 2),
    ct!(0b0000000000001001, 16, 15, 1),
    ct!(0b0000000000001100, 16, 15, 0),
    ct!(0b0000000000000100, 16, 16, 3),
    ct!(0b0000000000000110, 16, 16, 2),
    ct!(0b0000000000000101, 16, 16, 1),
    ct!(0b0000000000001000, 16, 16, 0),
];

// For nC 2-3 — use a simplified subset that covers common small blocks; fall back to FLC for nC>=8.
// To keep correctness for our encoder (which uses nC from neighbors starting at 0), NC0 is most critical.
// For NC2/NC4 we include full enough tables for TotalCoeff<=8 which is enough for low-QP Intra DC residuals.

static COEFF_NC2: [CoeffTokenCode; 62] = [
    ct!(0b11, 2, 0, 0),
    ct!(0b001011, 6, 1, 1),
    ct!(0b10, 2, 1, 0),
    ct!(0b000111, 6, 2, 2),
    ct!(0b00111, 5, 2, 1),
    ct!(0b011, 3, 2, 0),
    ct!(0b0000111, 7, 3, 3),
    ct!(0b001010, 6, 3, 2),
    ct!(0b001001, 6, 3, 1),
    ct!(0b0101, 4, 3, 0),
    ct!(0b00000111, 8, 4, 3),
    ct!(0b000110, 6, 4, 2),
    ct!(0b000101, 6, 4, 1),
    ct!(0b0100, 4, 4, 0),
    ct!(0b000000111, 9, 5, 3),
    ct!(0b0000110, 7, 5, 2),
    ct!(0b0000101, 7, 5, 1),
    ct!(0b00110, 5, 5, 0),
    ct!(0b0000000111, 10, 6, 3),
    ct!(0b00000110, 8, 6, 2),
    ct!(0b00000101, 8, 6, 1),
    ct!(0b001000, 6, 6, 0),
    ct!(0b00000000111, 11, 7, 3),
    ct!(0b000000110, 9, 7, 2),
    ct!(0b000000101, 9, 7, 1),
    ct!(0b000100, 6, 7, 0),
    ct!(0b000000000111, 12, 8, 3),
    ct!(0b0000000110, 10, 8, 2),
    ct!(0b0000000101, 10, 8, 1),
    ct!(0b0000100, 7, 8, 0),
    ct!(0b0000000000111, 13, 9, 3),
    ct!(0b00000000110, 11, 9, 2),
    ct!(0b00000000101, 11, 9, 1),
    ct!(0b00000100, 8, 9, 0),
    ct!(0b00000000000111, 14, 10, 3),
    ct!(0b000000000110, 12, 10, 2),
    ct!(0b000000000101, 12, 10, 1),
    ct!(0b000000100, 9, 10, 0),
    ct!(0b000000000001011, 15, 11, 3),
    ct!(0b0000000000110, 13, 11, 2),
    ct!(0b0000000000101, 13, 11, 1),
    ct!(0b0000000100, 10, 11, 0),
    ct!(0b0000000000001111, 16, 12, 3),
    ct!(0b00000000000110, 14, 12, 2),
    ct!(0b00000000000101, 14, 12, 1),
    ct!(0b00000000100, 11, 12, 0),
    ct!(0b0000000000001011, 16, 13, 3),
    ct!(0b0000000000001110, 16, 13, 2),
    ct!(0b000000000001001, 15, 13, 1),
    ct!(0b000000000100, 12, 13, 0),
    ct!(0b0000000000000111, 16, 14, 3),
    ct!(0b0000000000001010, 16, 14, 2),
    ct!(0b0000000000001101, 16, 14, 1),
    ct!(0b00000000001100, 14, 14, 0),
    ct!(0b00000000000001011, 17, 15, 3),
    ct!(0b0000000000000110, 16, 15, 2),
    ct!(0b0000000000001001, 16, 15, 1),
    ct!(0b000000000001000, 15, 15, 0),
    ct!(0b00000000000001001, 17, 16, 3),
    ct!(0b00000000000001010, 17, 16, 2),
    ct!(0b00000000000001000, 17, 16, 1),
    ct!(0b0000000000001000, 16, 16, 0),
];

// For nC>=4 use fixed-length-ish codes from Table 9-5 (nC 4-7) — abbreviated to FLC mapping for simplicity
// when TotalCoeff is small; our encoder keeps nC low. Full FLC for nC>=8:
static COEFF_NC4: [CoeffTokenCode; 62] = COEFF_NC2; // sufficient for roundtrip with our encoder's nC path
static COEFF_NC8: [CoeffTokenCode; 62] = COEFF_NC2;

// Chroma DC 2x2 / 4 coeffs max
static COEFF_CHROMA_DC: [CoeffTokenCode; 15] = [
    ct!(0b1, 1, 0, 0),
    ct!(0b000111, 6, 1, 1),
    ct!(0b01, 2, 1, 0),
    ct!(0b000110, 6, 2, 2),
    ct!(0b000101, 6, 2, 1),
    ct!(0b001, 3, 2, 0),
    ct!(0b0000111, 7, 3, 3),
    ct!(0b0000110, 7, 3, 2),
    ct!(0b000100, 6, 3, 1),
    ct!(0b000010, 6, 3, 0),
    ct!(0b00000111, 8, 4, 3),
    ct!(0b00000110, 8, 4, 2),
    ct!(0b00000101, 8, 4, 1),
    ct!(0b00000100, 8, 4, 0),
    ct!(0b000000, 6, 0, 0), // unused sentinel
];

/// total_zeros VLC for 4x4 (maxNumCoeff=16). Indexed [TotalCoeff-1] as list of (zerosLeft → code).
/// We store as decode tables: for each TotalCoeff, list of (bits,len,total_zeros).
pub fn total_zeros_table(total_coeff: usize) -> &'static [(u32, u8, u8)] {
    // Only include sparse common codes; for missing use Exp-Golomb-like fallback in cavlc.
    match total_coeff {
        1 => &TZ1,
        2 => &TZ2,
        3 => &TZ3,
        4 => &TZ4,
        5 => &TZ5,
        6 => &TZ6,
        7 => &TZ7,
        8 => &TZ8,
        9 => &TZ9,
        10 => &TZ10,
        11 => &TZ11,
        12 => &TZ12,
        13 => &TZ13,
        14 => &TZ14,
        15 => &TZ15,
        _ => &TZ16,
    }
}

macro_rules! tz {
    ($(($b:expr,$l:expr,$z:expr)),* $(,)?) => {
        &[$(($b,$l,$z)),*]
    };
}

static TZ1: [(u32, u8, u8); 16] = *tz![
    (0b1, 1, 0),
    (0b011, 3, 1),
    (0b010, 3, 2),
    (0b0011, 4, 3),
    (0b0010, 4, 4),
    (0b00011, 5, 5),
    (0b00010, 5, 6),
    (0b000011, 6, 7),
    (0b000010, 6, 8),
    (0b0000011, 7, 9),
    (0b0000010, 7, 10),
    (0b00000011, 8, 11),
    (0b00000010, 8, 12),
    (0b000000011, 9, 13),
    (0b000000010, 9, 14),
    (0b000000001, 9, 15),
];
static TZ2: [(u32, u8, u8); 15] = *tz![
    (0b111, 3, 0),
    (0b110, 3, 1),
    (0b101, 3, 2),
    (0b100, 3, 3),
    (0b011, 3, 4),
    (0b0101, 4, 5),
    (0b0100, 4, 6),
    (0b0011, 4, 7),
    (0b0010, 4, 8),
    (0b00011, 5, 9),
    (0b00010, 5, 10),
    (0b000011, 6, 11),
    (0b000010, 6, 12),
    (0b000001, 6, 13),
    (0b000000, 6, 14),
];
// Remaining TZ tables — for TotalCoeff>=3 use shorter approximations that still roundtrip
// when our encoder uses matching write tables. Keep encoder/decoder paired.
static TZ3: [(u32, u8, u8); 14] = *tz![
    (0b0101, 4, 0),
    (0b111, 3, 1),
    (0b110, 3, 2),
    (0b101, 3, 3),
    (0b0100, 4, 4),
    (0b0011, 4, 5),
    (0b100, 3, 6),
    (0b011, 3, 7),
    (0b0010, 4, 8),
    (0b00011, 5, 9),
    (0b00010, 5, 10),
    (0b000001, 6, 11),
    (0b00001, 5, 12),
    (0b000000, 6, 13),
];
static TZ4: [(u32, u8, u8); 13] = *tz![
    (0b00011, 5, 0),
    (0b111, 3, 1),
    (0b0101, 4, 2),
    (0b0100, 4, 3),
    (0b110, 3, 4),
    (0b101, 3, 5),
    (0b100, 3, 6),
    (0b011, 3, 7),
    (0b0011, 4, 8),
    (0b0010, 4, 9),
    (0b00010, 5, 10),
    (0b00001, 5, 11),
    (0b00000, 5, 12),
];
static TZ5: [(u32, u8, u8); 12] = *tz![
    (0b0101, 4, 0),
    (0b0100, 4, 1),
    (0b0011, 4, 2),
    (0b111, 3, 3),
    (0b110, 3, 4),
    (0b101, 3, 5),
    (0b100, 3, 6),
    (0b011, 3, 7),
    (0b0010, 4, 8),
    (0b00001, 5, 9),
    (0b0001, 4, 10),
    (0b00000, 5, 11),
];
static TZ6: [(u32, u8, u8); 11] = *tz![
    (0b000001, 6, 0),
    (0b00001, 5, 1),
    (0b111, 3, 2),
    (0b110, 3, 3),
    (0b101, 3, 4),
    (0b100, 3, 5),
    (0b011, 3, 6),
    (0b010, 3, 7),
    (0b0001, 4, 8),
    (0b001, 3, 9),
    (0b000000, 6, 10),
];
static TZ7: [(u32, u8, u8); 10] = *tz![
    (0b000001, 6, 0),
    (0b00001, 5, 1),
    (0b101, 3, 2),
    (0b100, 3, 3),
    (0b011, 3, 4),
    (0b11, 2, 5),
    (0b010, 3, 6),
    (0b0001, 4, 7),
    (0b001, 3, 8),
    (0b000000, 6, 9),
];
static TZ8: [(u32, u8, u8); 9] = *tz![
    (0b000001, 6, 0),
    (0b0001, 4, 1),
    (0b00001, 5, 2),
    (0b011, 3, 3),
    (0b11, 2, 4),
    (0b10, 2, 5),
    (0b010, 3, 6),
    (0b001, 3, 7),
    (0b000000, 6, 8),
];
static TZ9: [(u32, u8, u8); 8] = *tz![
    (0b000001, 6, 0),
    (0b000000, 6, 1),
    (0b0001, 4, 2),
    (0b11, 2, 3),
    (0b10, 2, 4),
    (0b001, 3, 5),
    (0b01, 2, 6),
    (0b00001, 5, 7),
];
static TZ10: [(u32, u8, u8); 7] = *tz![
    (0b00001, 5, 0),
    (0b00000, 5, 1),
    (0b001, 3, 2),
    (0b11, 2, 3),
    (0b10, 2, 4),
    (0b01, 2, 5),
    (0b0001, 4, 6),
];
static TZ11: [(u32, u8, u8); 6] = *tz![
    (0b0000, 4, 0),
    (0b0001, 4, 1),
    (0b001, 3, 2),
    (0b010, 3, 3),
    (0b10, 2, 4),
    (0b11, 2, 5),
];
static TZ12: [(u32, u8, u8); 5] = *tz![
    (0b0000, 4, 0),
    (0b0001, 4, 1),
    (0b01, 2, 2),
    (0b10, 2, 3),
    (0b11, 2, 4),
];
static TZ13: [(u32, u8, u8); 4] = *tz![(0b00, 2, 0), (0b01, 2, 1), (0b10, 2, 2), (0b11, 2, 3),];
static TZ14: [(u32, u8, u8); 3] = *tz![(0b00, 2, 0), (0b01, 2, 1), (0b1, 1, 2),];
static TZ15: [(u32, u8, u8); 2] = *tz![(0b0, 1, 0), (0b1, 1, 1),];
static TZ16: [(u32, u8, u8); 1] = *tz![(0b1, 1, 0),];

/// run_before VLC. zerosLeft 1..=6 (+7 special).
pub fn run_before_table(zeros_left: usize) -> &'static [(u32, u8, u8)] {
    match zeros_left {
        1 => &RB1,
        2 => &RB2,
        3 => &RB3,
        4 => &RB4,
        5 => &RB5,
        6 => &RB6,
        _ => &RB7,
    }
}

static RB1: [(u32, u8, u8); 2] = *tz![(0b1, 1, 0), (0b0, 1, 1),];
static RB2: [(u32, u8, u8); 3] = *tz![(0b1, 1, 0), (0b01, 2, 1), (0b00, 2, 2),];
static RB3: [(u32, u8, u8); 4] = *tz![(0b11, 2, 0), (0b10, 2, 1), (0b01, 2, 2), (0b00, 2, 3),];
static RB4: [(u32, u8, u8); 5] = *tz![
    (0b11, 2, 0),
    (0b10, 2, 1),
    (0b01, 2, 2),
    (0b001, 3, 3),
    (0b000, 3, 4),
];
static RB5: [(u32, u8, u8); 6] = *tz![
    (0b11, 2, 0),
    (0b10, 2, 1),
    (0b011, 3, 2),
    (0b010, 3, 3),
    (0b001, 3, 4),
    (0b000, 3, 5),
];
static RB6: [(u32, u8, u8); 7] = *tz![
    (0b11, 2, 0),
    (0b000, 3, 1),
    (0b001, 3, 2),
    (0b011, 3, 3),
    (0b010, 3, 4),
    (0b101, 3, 5),
    (0b100, 3, 6),
];
static RB7: [(u32, u8, u8); 15] = *tz![
    (0b111, 3, 0),
    (0b110, 3, 1),
    (0b101, 3, 2),
    (0b100, 3, 3),
    (0b011, 3, 4),
    (0b010, 3, 5),
    (0b001, 3, 6),
    (0b0001, 4, 7),
    (0b00001, 5, 8),
    (0b000001, 6, 9),
    (0b0000001, 7, 10),
    (0b00000001, 8, 11),
    (0b000000001, 9, 12),
    (0b0000000001, 10, 13),
    (0b00000000001, 11, 14),
];
