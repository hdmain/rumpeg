#![allow(dead_code)]
//! Bitstream reader / writer for H.264 syntax elements.

use crate::error::{Error, Result};

/// Big-endian bit reader over a byte slice.
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position from start.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bits_left(&self) -> usize {
        self.data.len().saturating_mul(8).saturating_sub(self.pos)
    }

    pub fn byte_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    pub fn read_bit(&mut self) -> Result<u8> {
        if self.pos / 8 >= self.data.len() {
            return Err(Error::truncated("read_bit"));
        }
        let byte = self.data[self.pos / 8];
        let bit = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Ok(bit)
    }

    pub fn read_bits(&mut self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(Error::invalid("read_bits > 32"));
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.read_bit()?);
        }
        Ok(v)
    }

    pub fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bit()? != 0)
    }

    /// Unsigned Exp-Golomb `ue(v)`.
    pub fn read_ue(&mut self) -> Result<u32> {
        let mut zeros = 0u32;
        while self.read_bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(Error::invalid("ue(v) too long"));
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(zeros)?;
        Ok((1u32 << zeros) - 1 + suffix)
    }

    /// Signed Exp-Golomb `se(v)`.
    #[allow(dead_code)]
    pub fn read_se(&mut self) -> Result<i32> {
        let code = self.read_ue()?;
        let val = ((code + 1) >> 1) as i32;
        if code & 1 == 0 {
            Ok(-val)
        } else {
            Ok(val)
        }
    }

    /// Skip to next byte boundary (rbsp trailing bits already consumed separately).
    pub fn align_byte(&mut self) {
        let rem = self.pos % 8;
        if rem != 0 {
            self.pos += 8 - rem;
        }
    }

    pub fn byte_pos(&self) -> usize {
        self.pos / 8
    }

    pub fn remaining_bytes(&self) -> &'a [u8] {
        let start = self.pos.div_ceil(8);
        if start >= self.data.len() {
            &[]
        } else {
            &self.data[start..]
        }
    }

    /// Consume remaining bytes as a raw slice (must be byte-aligned).
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if !self.byte_aligned() {
            return Err(Error::invalid("read_bytes requires byte alignment"));
        }
        let start = self.pos / 8;
        let end = start + n;
        if end > self.data.len() {
            return Err(Error::truncated(format!("need {n} bytes")));
        }
        self.pos = end * 8;
        Ok(&self.data[start..end])
    }
}

/// Big-endian bit writer.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    /// Bits filled in the current trailing byte (0..=7). When 0, buf is fully flushed.
    bit_pos: u8,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_bit(&mut self, bit: u8) {
        if self.bit_pos == 0 {
            self.buf.push(0);
        }
        let last = self.buf.len() - 1;
        if bit != 0 {
            self.buf[last] |= 1 << (7 - self.bit_pos);
        }
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
        }
    }

    pub fn write_bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bit(((value >> i) & 1) as u8);
        }
    }

    pub fn write_flag(&mut self, v: bool) {
        self.write_bit(u8::from(v));
    }

    pub fn write_ue(&mut self, value: u32) {
        let mut x = value + 1;
        let mut zeros = 0u32;
        while x > 1 {
            x >>= 1;
            zeros += 1;
        }
        for _ in 0..zeros {
            self.write_bit(0);
        }
        self.write_bit(1);
        if zeros > 0 {
            let mask = (1u32 << zeros) - 1;
            self.write_bits(value + 1 - (1u32 << zeros), zeros);
            let _ = mask;
        }
    }

    #[allow(dead_code)]
    pub fn write_se(&mut self, value: i32) {
        let code = if value <= 0 {
            (-2 * value) as u32
        } else {
            (2 * value - 1) as u32
        };
        self.write_ue(code);
    }

    /// RBSP stop bit + zero alignment (cabac_alignment / rbsp_trailing_bits).
    pub fn write_rbsp_trailing_bits(&mut self) {
        self.write_bit(1);
        while self.bit_pos != 0 {
            self.write_bit(0);
        }
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        assert_eq!(self.bit_pos, 0, "write_bytes requires alignment");
        self.buf.extend_from_slice(data);
    }

    /// Pad with zero bits until the next byte boundary (PCM alignment).
    pub fn byte_align_zeros(&mut self) {
        while self.bit_pos != 0 {
            self.write_bit(0);
        }
    }

    /// `true` when at a byte boundary.
    pub fn is_byte_aligned(&self) -> bool {
        self.bit_pos == 0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        assert_eq!(self.bit_pos, 0, "bitstream not byte-aligned");
        self.buf
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
}

/// Remove emulation prevention bytes (`00 00 03 xx` → `00 00 xx`).
pub fn rbsp_from_ebsp(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut i = 0;
    while i < ebsp.len() {
        if i + 2 < ebsp.len() && ebsp[i] == 0 && ebsp[i + 1] == 0 && ebsp[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(ebsp[i]);
            i += 1;
        }
    }
    out
}

/// Insert emulation prevention bytes.
pub fn ebsp_from_rbsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 128 + 1);
    let mut zero_count = 0u8;
    for &b in rbsp {
        if zero_count == 2 && b <= 3 {
            out.push(0x03);
            zero_count = 0;
        }
        out.push(b);
        if b == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ue_roundtrip() {
        for v in [0u32, 1, 2, 3, 4, 5, 13, 128, 1000] {
            let mut w = BitWriter::new();
            w.write_ue(v);
            w.write_rbsp_trailing_bits();
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_ue().unwrap(), v);
        }
    }
}
