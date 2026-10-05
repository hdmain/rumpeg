//! Stream metadata within a container.

use rumpeg_util::{CodecParams, MediaType, Rational};

/// One elementary stream inside a container.
#[derive(Clone, Debug)]
pub struct Stream {
    /// Zero-based index within the parent format context.
    pub index: usize,
    /// Stream media type.
    pub media_type: MediaType,
    /// Codec parameters needed to open a decoder.
    pub codec_params: CodecParams,
    /// Stream time base.
    pub time_base: Rational,
    /// Stream duration in `time_base` ticks (`None` if unknown).
    pub duration: Option<i64>,
    /// Approximate number of frames (`None` if unknown).
    pub nb_frames: Option<u64>,
    /// Stream metadata (language, title, …).
    pub metadata: Vec<(String, String)>,
}

impl Stream {
    /// Create a stream with the given index and codec params.
    pub fn new(index: usize, codec_params: CodecParams) -> Self {
        let media_type = codec_params.media_type();
        Self {
            index,
            media_type,
            codec_params,
            time_base: Rational::one(),
            duration: None,
            nb_frames: None,
            metadata: Vec::new(),
        }
    }
}
