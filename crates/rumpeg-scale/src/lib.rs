//! Image scaling and pixel-format conversion — `libswscale` counterpart.
//!
//! Hot paths use tight inner loops over contiguous scanlines. Where the
//! compiler can autovectorize (RGB packing / YUV matrix math), release builds
//! with LTO typically emit SIMD without unsafe intrinsics.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

mod convert;
mod scale;

pub use convert::convert_frame;
use rumpeg_util::{Error, PixelFormat, Result, VideoFrame};
pub use scale::{scale_frame, FilterMode};

/// Convert and/or scale a video frame to a target format and size.
pub fn transform(
    src: &VideoFrame,
    dst_fmt: PixelFormat,
    dst_w: u32,
    dst_h: u32,
    filter: FilterMode,
) -> Result<VideoFrame> {
    if src.width == 0 || src.height == 0 || dst_w == 0 || dst_h == 0 {
        return Err(Error::invalid_data("invalid frame dimensions"));
    }

    let same_size = src.width == dst_w && src.height == dst_h;
    let same_fmt = src.format == dst_fmt;

    if same_size && same_fmt {
        return Ok(src.clone());
    }

    if same_size {
        return convert_frame(src, dst_fmt);
    }

    // Scale in a convenient intermediate format when needed.
    let scaled = if src.format == PixelFormat::Rgb24 {
        scale_frame(src, dst_w, dst_h, filter)?
    } else {
        let rgb = convert_frame(src, PixelFormat::Rgb24)?;
        scale_frame(&rgb, dst_w, dst_h, filter)?
    };

    if dst_fmt == PixelFormat::Rgb24 {
        Ok(scaled)
    } else {
        convert_frame(&scaled, dst_fmt)
    }
}
