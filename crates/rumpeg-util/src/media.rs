//! Media type descriptors: codecs, pixel/sample formats, stream parameters.

use crate::rational::Rational;
use std::fmt;

/// Broad media kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    /// Video elementary stream.
    Video,
    /// Audio elementary stream.
    Audio,
    /// Text / bitmap subtitles.
    Subtitle,
    /// Attachments, timed metadata, etc.
    Data,
    /// Unknown / unset.
    Unknown,
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Subtitle => "subtitle",
            Self::Data => "data",
            Self::Unknown => "unknown",
        })
    }
}

/// Stable codec identifiers (extend as codecs are added).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodecId {
    /// Unknown / unset codec.
    None,
    // Audio
    /// Linear PCM signed 16-bit little-endian.
    PcmS16Le,
    /// Linear PCM signed 24-bit little-endian (packed).
    PcmS24Le,
    /// Linear PCM signed 32-bit little-endian.
    PcmS32Le,
    /// Linear PCM 32-bit float little-endian.
    PcmF32Le,
    /// Linear PCM unsigned 8-bit.
    PcmU8,
    /// FLAC lossless audio.
    Flac,
    /// Opus audio.
    Opus,
    /// AAC audio.
    Aac,
    /// MP3 audio.
    Mp3,
    // Video
    /// Uncompressed packed RGB / planar YUV (rawvideo).
    RawVideo,
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    Hevc,
    /// AV1.
    Av1,
    /// VP9.
    Vp9,
    /// VP8.
    Vp8,
    /// MJPEG.
    Mjpeg,
    /// PNG still image.
    Png,
}

impl CodecId {
    /// Media type associated with this codec.
    pub fn media_type(self) -> MediaType {
        match self {
            Self::None => MediaType::Unknown,
            Self::PcmS16Le
            | Self::PcmS24Le
            | Self::PcmS32Le
            | Self::PcmF32Le
            | Self::PcmU8
            | Self::Flac
            | Self::Opus
            | Self::Aac
            | Self::Mp3 => MediaType::Audio,
            Self::RawVideo
            | Self::H264
            | Self::Hevc
            | Self::Av1
            | Self::Vp9
            | Self::Vp8
            | Self::Mjpeg
            | Self::Png => MediaType::Video,
        }
    }

    /// Short canonical name (FFmpeg-style).
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PcmS16Le => "pcm_s16le",
            Self::PcmS24Le => "pcm_s24le",
            Self::PcmS32Le => "pcm_s32le",
            Self::PcmF32Le => "pcm_f32le",
            Self::PcmU8 => "pcm_u8",
            Self::Flac => "flac",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Mp3 => "mp3",
            Self::RawVideo => "rawvideo",
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp9 => "vp9",
            Self::Vp8 => "vp8",
            Self::Mjpeg => "mjpeg",
            Self::Png => "png",
        }
    }

    /// Parse a codec name string.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pcm_s16le" => Self::PcmS16Le,
            "pcm_s24le" => Self::PcmS24Le,
            "pcm_s32le" => Self::PcmS32Le,
            "pcm_f32le" => Self::PcmF32Le,
            "pcm_u8" => Self::PcmU8,
            "flac" => Self::Flac,
            "opus" => Self::Opus,
            "aac" => Self::Aac,
            "mp3" => Self::Mp3,
            "rawvideo" => Self::RawVideo,
            "h264" | "avc" => Self::H264,
            "hevc" | "h265" => Self::Hevc,
            "av1" => Self::Av1,
            "vp9" => Self::Vp9,
            "vp8" => Self::Vp8,
            "mjpeg" => Self::Mjpeg,
            "png" => Self::Png,
            _ => return None,
        })
    }
}

impl fmt::Display for CodecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Audio sample format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SampleFormat {
    /// Unsigned 8-bit.
    U8,
    /// Signed 16-bit.
    S16,
    /// Signed 24-bit packed (3 bytes/sample).
    S24,
    /// Signed 32-bit.
    S32,
    /// 32-bit IEEE float.
    F32,
    /// 64-bit IEEE float.
    F64,
    /// Planar variants (channel-per-plane).
    U8P,
    /// Planar S16.
    S16P,
    /// Planar S32.
    S32P,
    /// Planar F32.
    F32P,
    /// Planar F64.
    F64P,
}

impl SampleFormat {
    /// Bytes per sample (per channel).
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::U8 | Self::U8P => 1,
            Self::S16 | Self::S16P => 2,
            Self::S24 => 3,
            Self::S32 | Self::S32P | Self::F32 | Self::F32P => 4,
            Self::F64 | Self::F64P => 8,
        }
    }

    /// `true` if samples are planar (one plane per channel).
    pub fn is_planar(self) -> bool {
        matches!(
            self,
            Self::U8P | Self::S16P | Self::S32P | Self::F32P | Self::F64P
        )
    }

    /// Short name.
    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::S16 => "s16",
            Self::S24 => "s24",
            Self::S32 => "s32",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::U8P => "u8p",
            Self::S16P => "s16p",
            Self::S32P => "s32p",
            Self::F32P => "f32p",
            Self::F64P => "f64p",
        }
    }
}

/// Channel layout description (simplified vs FFmpeg channel masks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelLayout {
    /// Number of channels.
    pub channels: u16,
}

impl ChannelLayout {
    /// Mono.
    pub const MONO: Self = Self { channels: 1 };
    /// Stereo.
    pub const STEREO: Self = Self { channels: 2 };

    /// Create from channel count.
    pub const fn new(channels: u16) -> Self {
        Self { channels }
    }
}

/// Pixel format for video frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Packed 24-bit RGB (R,G,B).
    Rgb24,
    /// Packed 32-bit RGBA.
    Rgba,
    /// Packed 24-bit BGR.
    Bgr24,
    /// Packed 32-bit BGRA.
    Bgra,
    /// Planar YUV 4:2:0 (I420).
    Yuv420p,
    /// Planar YUV 4:2:2.
    Yuv422p,
    /// Planar YUV 4:4:4.
    Yuv444p,
    /// Planar grayscale 8-bit.
    Gray8,
    /// NV12 (Y plane + interleaved UV).
    Nv12,
}

impl PixelFormat {
    /// Short name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Bgr24 => "bgr24",
            Self::Bgra => "bgra",
            Self::Yuv420p => "yuv420p",
            Self::Yuv422p => "yuv422p",
            Self::Yuv444p => "yuv444p",
            Self::Gray8 => "gray8",
            Self::Nv12 => "nv12",
        }
    }

    /// Number of planes.
    pub fn plane_count(self) -> usize {
        match self {
            Self::Rgb24 | Self::Rgba | Self::Bgr24 | Self::Bgra | Self::Gray8 => 1,
            Self::Nv12 => 2,
            Self::Yuv420p | Self::Yuv422p | Self::Yuv444p => 3,
        }
    }

    /// Bytes per pixel for packed formats; for planar Y plane, 1.
    pub fn bits_per_pixel(self) -> u32 {
        match self {
            Self::Gray8 => 8,
            Self::Rgb24 | Self::Bgr24 => 24,
            Self::Rgba | Self::Bgra => 32,
            Self::Yuv420p | Self::Nv12 => 12,
            Self::Yuv422p => 16,
            Self::Yuv444p => 24,
        }
    }

    /// Horizontal chroma subsample shift (0 = full res).
    pub fn chroma_x_shift(self) -> u8 {
        match self {
            Self::Yuv420p | Self::Nv12 | Self::Yuv422p => 1,
            _ => 0,
        }
    }

    /// Vertical chroma subsample shift.
    pub fn chroma_y_shift(self) -> u8 {
        match self {
            Self::Yuv420p | Self::Nv12 => 1,
            _ => 0,
        }
    }

    /// Parse pixel format name.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "rgb24" => Self::Rgb24,
            "rgba" => Self::Rgba,
            "bgr24" => Self::Bgr24,
            "bgra" => Self::Bgra,
            "yuv420p" => Self::Yuv420p,
            "yuv422p" => Self::Yuv422p,
            "yuv444p" => Self::Yuv444p,
            "gray" | "gray8" => Self::Gray8,
            "nv12" => Self::Nv12,
            _ => return None,
        })
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Audio stream / codec parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioParams {
    /// Sample format.
    pub sample_fmt: SampleFormat,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel layout.
    pub layout: ChannelLayout,
    /// Preferred frame size in samples/channel (0 = codec default).
    pub frame_size: u32,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            sample_fmt: SampleFormat::S16,
            sample_rate: 48_000,
            layout: ChannelLayout::STEREO,
            frame_size: 0,
        }
    }
}

impl AudioParams {
    /// Bytes for one interleaved sample frame across all channels.
    pub fn bytes_per_frame(&self) -> usize {
        self.sample_fmt.bytes_per_sample() * self.layout.channels as usize
    }
}

/// Video stream / codec parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoParams {
    /// Pixel format.
    pub pix_fmt: PixelFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame rate as a rational (`num/den` fps), if known.
    pub frame_rate: Rational,
    /// Sample aspect ratio.
    pub sample_aspect_ratio: Rational,
}

impl Default for VideoParams {
    fn default() -> Self {
        Self {
            pix_fmt: PixelFormat::Yuv420p,
            width: 0,
            height: 0,
            frame_rate: Rational::new(0, 1),
            sample_aspect_ratio: Rational::one(),
        }
    }
}

/// Codec parameters carried on streams and used to open encoders/decoders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecParams {
    /// Codec identifier.
    pub codec_id: CodecId,
    /// Extra codec configuration data (e.g. AudioSpecificConfig, SPS/PPS).
    pub extradata: Vec<u8>,
    /// Bit rate hint in bits/second (0 = unknown).
    pub bit_rate: u64,
    /// Encoder quality / QP / CRF hint (`-1` = unset). For H.264 this maps to QP.
    pub quality: i32,
    /// GOP / IDR interval hint (`0` = codec default).
    pub gop_size: u32,
    /// Type-specific parameters.
    pub specific: CodecSpecific,
}

/// Type-specific codec parameter payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecSpecific {
    /// No type-specific data.
    None,
    /// Audio parameters.
    Audio(AudioParams),
    /// Video parameters.
    Video(VideoParams),
}

impl Default for CodecParams {
    fn default() -> Self {
        Self {
            codec_id: CodecId::None,
            extradata: Vec::new(),
            bit_rate: 0,
            quality: -1,
            gop_size: 0,
            specific: CodecSpecific::None,
        }
    }
}

impl CodecParams {
    /// Media type derived from the codec id / specific payload.
    pub fn media_type(&self) -> MediaType {
        match &self.specific {
            CodecSpecific::Audio(_) => MediaType::Audio,
            CodecSpecific::Video(_) => MediaType::Video,
            CodecSpecific::None => self.codec_id.media_type(),
        }
    }

    /// Borrow audio params if present.
    pub fn audio(&self) -> Option<&AudioParams> {
        match &self.specific {
            CodecSpecific::Audio(a) => Some(a),
            _ => None,
        }
    }

    /// Borrow video params if present.
    pub fn video(&self) -> Option<&VideoParams> {
        match &self.specific {
            CodecSpecific::Video(v) => Some(v),
            _ => None,
        }
    }

    /// Mutably borrow video params if present.
    pub fn video_mut(&mut self) -> Option<&mut VideoParams> {
        match &mut self.specific {
            CodecSpecific::Video(v) => Some(v),
            _ => None,
        }
    }

    /// Construct audio codec params.
    pub fn audio_codec(codec_id: CodecId, audio: AudioParams) -> Self {
        Self {
            codec_id,
            extradata: Vec::new(),
            bit_rate: 0,
            quality: -1,
            gop_size: 0,
            specific: CodecSpecific::Audio(audio),
        }
    }

    /// Construct video codec params.
    pub fn video_codec(codec_id: CodecId, video: VideoParams) -> Self {
        Self {
            codec_id,
            extradata: Vec::new(),
            bit_rate: 0,
            quality: -1,
            gop_size: 0,
            specific: CodecSpecific::Video(video),
        }
    }
}
