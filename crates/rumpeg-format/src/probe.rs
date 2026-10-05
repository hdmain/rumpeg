//! Format probing — identify containers from a small header peek.

use crate::io::IoReader;
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
    let candidates = [("wav", wav::probe_score(&buf))];
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
