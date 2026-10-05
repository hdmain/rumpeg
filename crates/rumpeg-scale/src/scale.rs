//! Spatial scaling kernels.

use rumpeg_util::{Error, PixelFormat, Result, VideoFrame};

/// Interpolation mode for scaling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterMode {
    /// Nearest neighbor (fastest).
    Nearest,
    /// Bilinear (default quality/speed balance).
    #[default]
    Bilinear,
}

/// Scale an RGB24 (or Gray8) frame to a new size.
pub fn scale_frame(
    src: &VideoFrame,
    dst_w: u32,
    dst_h: u32,
    filter: FilterMode,
) -> Result<VideoFrame> {
    match src.format {
        PixelFormat::Rgb24 => scale_rgb24(src, dst_w, dst_h, filter),
        PixelFormat::Gray8 => scale_gray8(src, dst_w, dst_h, filter),
        other => Err(Error::unsupported(format!(
            "direct scale of {other}; convert to rgb24 first"
        ))),
    }
}

fn scale_rgb24(src: &VideoFrame, dst_w: u32, dst_h: u32, filter: FilterMode) -> Result<VideoFrame> {
    let mut dst = VideoFrame::alloc(PixelFormat::Rgb24, dst_w, dst_h);
    dst.pts = src.pts;
    let src_data = src.plane(0).unwrap();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let ss = src.strides[0];
    let ds = dst.strides[0];
    let dst_data = dst.plane_mut(0).unwrap();

    match filter {
        FilterMode::Nearest => {
            for y in 0..dh {
                let sy = y * sh / dh;
                let src_row = &src_data[sy * ss..];
                let dst_row = &mut dst_data[y * ds..y * ds + dw * 3];
                for x in 0..dw {
                    let sx = x * sw / dw;
                    let s = sx * 3;
                    let d = x * 3;
                    dst_row[d..d + 3].copy_from_slice(&src_row[s..s + 3]);
                }
            }
        }
        FilterMode::Bilinear => {
            let x_ratio = ((sw - 1) as f32) / dw.max(1) as f32;
            let y_ratio = ((sh - 1) as f32) / dh.max(1) as f32;
            for y in 0..dh {
                let fy = y as f32 * y_ratio;
                let y0 = fy as usize;
                let y1 = (y0 + 1).min(sh - 1);
                let wy = fy - y0 as f32;
                let row0 = &src_data[y0 * ss..];
                let row1 = &src_data[y1 * ss..];
                let dst_row = &mut dst_data[y * ds..y * ds + dw * 3];
                for x in 0..dw {
                    let fx = x as f32 * x_ratio;
                    let x0 = fx as usize;
                    let x1 = (x0 + 1).min(sw - 1);
                    let wx = fx - x0 as f32;
                    for c in 0..3 {
                        let p00 = row0[x0 * 3 + c] as f32;
                        let p10 = row0[x1 * 3 + c] as f32;
                        let p01 = row1[x0 * 3 + c] as f32;
                        let p11 = row1[x1 * 3 + c] as f32;
                        let top = p00 + (p10 - p00) * wx;
                        let bot = p01 + (p11 - p01) * wx;
                        dst_row[x * 3 + c] = (top + (bot - top) * wy).round() as u8;
                    }
                }
            }
        }
    }
    Ok(dst)
}

fn scale_gray8(src: &VideoFrame, dst_w: u32, dst_h: u32, filter: FilterMode) -> Result<VideoFrame> {
    // Convert via nearest for simplicity on gray (still useful for thumbnails).
    let _ = filter;
    let mut dst = VideoFrame::alloc(PixelFormat::Gray8, dst_w, dst_h);
    dst.pts = src.pts;
    let src_data = src.plane(0).unwrap();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let ss = src.strides[0];
    let ds = dst.strides[0];
    let dst_data = dst.plane_mut(0).unwrap();
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            dst_data[y * ds + x] = src_data[sy * ss + sx];
        }
    }
    Ok(dst)
}
