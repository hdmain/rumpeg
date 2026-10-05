//! Format probing — identify containers from a small header peek.

use crate::h264raw;
use crate::image2;
use crate::io::IoReader;
use crate::matroska;
use crate::mp4;
use crate::mpegts;
use crate::wav;
use rumpeg_util::{Error, Result};
use std::io::SeekFrom;

/// Result of probing an input.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    /// Detected format short name.
    pub format_name: &'static str,
    /// Confidence score 0–100.
    pub score: u32,
}

/// Probe `reader` without permanently consuming it (restores position).
pub fn probe(reader: &mut dyn IoReader) -> Result<ProbeResult> {
    let start = reader.stream_position()?;
    let mut buf = vec![0u8; 4096];
    let n = match reader.read(&mut buf) {
        Ok(n) => n,
        Err(e) => {
            let _ = reader.seek(SeekFrom::Start(start));
            return Err(Error::Io(e));
        }
    };
    buf.truncate(n);
    let _ = reader.seek(SeekFrom::Start(start));

    let mut best: Option<ProbeResult> = None;
    let candidates = [
        ("mp4", mp4::probe_score(&buf)),
        ("matroska", matroska::probe_score(&buf)),
        ("mpegts", mpegts::probe_score(&buf)),
        ("wav", wav::probe_score(&buf)),
        ("image2", image2::probe_score(&buf)),
        ("h264", h264raw::probe_score(&buf)),
    ];
    for (name, score) in candidates {
        if score == 0 {
            continue;
        }
        if best.as_ref().map(|b| b.score).unwrap_or(0) < score {
            best = Some(ProbeResult {
                format_name: name,
                score,
            });
        }
    }

    best.ok_or_else(|| Error::ProbeFailed("unrecognized format".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MediaIo;

    #[test]
    fn probe_prefers_mp4_ftyp() {
        let data = b"\x00\x00\x00\x20ftypisom\x00\x00\x00\x00";
        let mut io = MediaIo::from_bytes(data.to_vec());
        let r = probe(&mut io).unwrap();
        assert_eq!(r.format_name, "mp4");
        assert_eq!(r.score, 100);
    }

    #[test]
    fn probe_matroska_ebml() {
        let data = [0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00, 0x00, 0x00];
        let mut io = MediaIo::from_bytes(data.to_vec());
        let r = probe(&mut io).unwrap();
        assert_eq!(r.format_name, "matroska");
    }
}
