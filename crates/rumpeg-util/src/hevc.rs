//! HEVC configuration records and NAL framing shared by codecs and containers.

use crate::{Error, Result};

/// Parsed HEVC decoder configuration (`hvcC`).
pub struct HevcConfig {
    /// Number of bytes in a sample's NAL length field.
    pub length_size: usize,
    /// Parameter sets with Annex-B start codes.
    pub annexb: Vec<u8>,
}

impl HevcConfig {
    /// Parse and validate the complete configuration record, including NAL arrays.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 23 || data[0] != 1 {
            return Err(Error::invalid_data("invalid hvcC header"));
        }
        let mut pos = 23;
        let mut annexb = Vec::new();
        for _ in 0..data[22] {
            if pos + 3 > data.len() {
                return Err(Error::invalid_data("truncated hvcC array"));
            }
            let count = u16::from_be_bytes([data[pos + 1], data[pos + 2]]);
            pos += 3;
            for _ in 0..count {
                if pos + 2 > data.len() {
                    return Err(Error::invalid_data("truncated hvcC NAL length"));
                }
                let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
                pos += 2;
                if len < 2 || len > data.len() - pos {
                    return Err(Error::invalid_data("invalid hvcC NAL length"));
                }
                annexb.extend_from_slice(&[0, 0, 0, 1]);
                annexb.extend_from_slice(&data[pos..pos + len]);
                pos += len;
            }
        }
        if pos != data.len() {
            return Err(Error::invalid_data("trailing hvcC data"));
        }
        Ok(Self {
            length_size: (data[21] as usize & 3) + 1,
            annexb,
        })
    }
}

/// Convert a length-prefixed HEVC sample to Annex-B, rejecting truncated NALs.
pub fn sample_to_annexb(data: &[u8], length_size: usize) -> Result<Vec<u8>> {
    if !(1..=4).contains(&length_size) {
        return Err(Error::invalid_data("invalid HEVC length size"));
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        if data.len() - pos < length_size {
            return Err(Error::invalid_data("truncated HEVC sample length"));
        }
        let len = data[pos..pos + length_size]
            .iter()
            .fold(0usize, |n, b| (n << 8) | usize::from(*b));
        pos += length_size;
        if len < 2 || len > data.len() - pos {
            return Err(Error::invalid_data("invalid HEVC sample NAL length"));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_lengths_and_truncation() {
        for size in 1..=4 {
            let mut data = vec![0; size];
            data[size - 1] = 3;
            data.extend_from_slice(&[0x26, 1, 0x80]);
            assert_eq!(
                sample_to_annexb(&data, size).unwrap(),
                [0, 0, 0, 1, 0x26, 1, 0x80]
            );
            data.pop();
            assert!(sample_to_annexb(&data, size).is_err());
        }
    }

    #[test]
    fn config_rejects_truncated_arrays() {
        let mut data = vec![0; 23];
        data[0] = 1;
        data[21] = 3;
        data[22] = 1;
        data.extend_from_slice(&[0xa0, 0, 1, 0, 3, 0x40, 1, 0x80]);
        let cfg = HevcConfig::parse(&data).unwrap();
        assert_eq!(cfg.length_size, 4);
        assert_eq!(cfg.annexb, [0, 0, 0, 1, 0x40, 1, 0x80]);
        for n in 0..data.len() {
            assert!(HevcConfig::parse(&data[..n]).is_err());
        }
    }
}
