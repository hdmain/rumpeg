//! Pixel format conversion kernels.

use rumpeg_util::{Error, PixelFormat, Result, VideoFrame};

/// Convert `src` to `dst_fmt` at the same resolution.
pub fn convert_frame(src: &VideoFrame, dst_fmt: PixelFormat) -> Result<VideoFrame> {
    if src.format == dst_fmt {
        return Ok(src.clone());
    }
    match (src.format, dst_fmt) {
        (PixelFormat::Yuv420p, PixelFormat::Rgb24) => yuv420_to_rgb24(src),
        (PixelFormat::Rgb24, PixelFormat::Yuv420p) => rgb24_to_yuv420(src),
        (PixelFormat::Rgb24, PixelFormat::Rgba) => rgb24_to_rgba(src),
        (PixelFormat::Rgba, PixelFormat::Rgb24) => rgba_to_rgb24(src),
        (PixelFormat::Rgb24, PixelFormat::Bgr24) => swap_rgb_bgr(src, PixelFormat::Bgr24),
        (PixelFormat::Bgr24, PixelFormat::Rgb24) => swap_rgb_bgr(src, PixelFormat::Rgb24),
        (PixelFormat::Gray8, PixelFormat::Rgb24) => gray_to_rgb24(src),
        (PixelFormat::Rgb24, PixelFormat::Gray8) => rgb24_to_gray(src),
        (a, b) => Err(Error::unsupported(format!(
            "pixel conversion {a} -> {b} not implemented"
        ))),
    }
}

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn yuv420_to_rgb24(src: &VideoFrame) -> Result<VideoFrame> {
    let y = src
        .plane(0)
        .ok_or_else(|| Error::invalid_data("missing Y"))?;
    let u = src
        .plane(1)
        .ok_or_else(|| Error::invalid_data("missing U"))?;
    let v = src
        .plane(2)
        .ok_or_else(|| Error::invalid_data("missing V"))?;
    let ys = src.strides[0];
    let us = src.strides[1];
    let vs = src.strides[2];
    let mut dst = VideoFrame::alloc(PixelFormat::Rgb24, src.width, src.height);
    dst.pts = src.pts;
    let stride = dst.strides[0];
    let out = dst.plane_mut(0).unwrap();
    let w = src.width as usize;
    let h = src.height as usize;

    for row in 0..h {
        let y_row = &y[row * ys..row * ys + w];
        let uv_row = row / 2;
        let u_row = &u[uv_row * us..];
        let v_row = &v[uv_row * vs..];
        let out_row = &mut out[row * stride..row * stride + w * 3];
        for col in 0..w {
            let yy = y_row[col] as i32;
            let uu = u_row[col / 2] as i32 - 128;
            let vv = v_row[col / 2] as i32 - 128;
            // BT.601 limited-range approximation (fast integer form).
            let r = clamp_u8(yy + ((359 * vv) >> 8));
            let g = clamp_u8(yy - ((88 * uu + 183 * vv) >> 8));
            let b = clamp_u8(yy + ((454 * uu) >> 8));
            let o = col * 3;
            out_row[o] = r;
            out_row[o + 1] = g;
            out_row[o + 2] = b;
        }
    }
    Ok(dst)
}

fn rgb24_to_yuv420(src: &VideoFrame) -> Result<VideoFrame> {
    let rgb = src
        .plane(0)
        .ok_or_else(|| Error::invalid_data("missing RGB"))?;
    let rs = src.strides[0];
    let mut dst = VideoFrame::alloc(PixelFormat::Yuv420p, src.width, src.height);
    dst.pts = src.pts;
    let w = src.width as usize;
    let h = src.height as usize;

    // Y plane
    {
        let ys = dst.strides[0];
        let y_plane = dst.plane_mut(0).unwrap();
        for row in 0..h {
            for col in 0..w {
                let i = row * rs + col * 3;
                let r = rgb[i] as i32;
                let g = rgb[i + 1] as i32;
                let b = rgb[i + 2] as i32;
                y_plane[row * ys + col] = clamp_u8(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
            }
        }
    }
    // U/V planes (average 2x2)
    let rgb = src.plane(0).unwrap();
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut u_buf = vec![0u8; cw * ch];
    let mut v_buf = vec![0u8; cw * ch];
    for row in 0..ch {
        for col in 0..cw {
            let mut us = 0i32;
            let mut vs = 0i32;
            let mut n = 0i32;
            for dy in 0..2 {
                let y = row * 2 + dy;
                if y >= h {
                    continue;
                }
                for dx in 0..2 {
                    let x = col * 2 + dx;
                    if x >= w {
                        continue;
                    }
                    let i = y * rs + x * 3;
                    let r = rgb[i] as i32;
                    let g = rgb[i + 1] as i32;
                    let b = rgb[i + 2] as i32;
                    us += ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    vs += ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    n += 1;
                }
            }
            if n > 0 {
                u_buf[row * cw + col] = clamp_u8(us / n);
                v_buf[row * cw + col] = clamp_u8(vs / n);
            }
        }
    }
    dst.plane_mut(1).unwrap().copy_from_slice(&u_buf);
    dst.plane_mut(2).unwrap().copy_from_slice(&v_buf);
    Ok(dst)
}

fn rgb24_to_rgba(src: &VideoFrame) -> Result<VideoFrame> {
    let rgb = src.plane(0).ok_or_else(|| Error::invalid_data("missing"))?;
    let mut dst = VideoFrame::alloc(PixelFormat::Rgba, src.width, src.height);
    dst.pts = src.pts;
    let out = dst.plane_mut(0).unwrap();
    let px = (src.width * src.height) as usize;
    for i in 0..px {
        out[i * 4] = rgb[i * 3];
        out[i * 4 + 1] = rgb[i * 3 + 1];
        out[i * 4 + 2] = rgb[i * 3 + 2];
        out[i * 4 + 3] = 255;
    }
    Ok(dst)
}

fn rgba_to_rgb24(src: &VideoFrame) -> Result<VideoFrame> {
    let rgba = src.plane(0).ok_or_else(|| Error::invalid_data("missing"))?;
    let mut dst = VideoFrame::alloc(PixelFormat::Rgb24, src.width, src.height);
    dst.pts = src.pts;
    let out = dst.plane_mut(0).unwrap();
    let px = (src.width * src.height) as usize;
    for i in 0..px {
        out[i * 3] = rgba[i * 4];
        out[i * 3 + 1] = rgba[i * 4 + 1];
        out[i * 3 + 2] = rgba[i * 4 + 2];
    }
    Ok(dst)
}

fn swap_rgb_bgr(src: &VideoFrame, dst_fmt: PixelFormat) -> Result<VideoFrame> {
    let data = src.plane(0).ok_or_else(|| Error::invalid_data("missing"))?;
    let mut dst = VideoFrame::alloc(dst_fmt, src.width, src.height);
    dst.pts = src.pts;
    let out = dst.plane_mut(0).unwrap();
    for (s, d) in data.chunks_exact(3).zip(out.chunks_exact_mut(3)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
    }
    Ok(dst)
}

fn gray_to_rgb24(src: &VideoFrame) -> Result<VideoFrame> {
    let g = src.plane(0).ok_or_else(|| Error::invalid_data("missing"))?;
    let mut dst = VideoFrame::alloc(PixelFormat::Rgb24, src.width, src.height);
    dst.pts = src.pts;
    let out = dst.plane_mut(0).unwrap();
    for (i, &y) in g.iter().enumerate() {
        out[i * 3] = y;
        out[i * 3 + 1] = y;
        out[i * 3 + 2] = y;
    }
    Ok(dst)
}

fn rgb24_to_gray(src: &VideoFrame) -> Result<VideoFrame> {
    let rgb = src.plane(0).ok_or_else(|| Error::invalid_data("missing"))?;
    let mut dst = VideoFrame::alloc(PixelFormat::Gray8, src.width, src.height);
    dst.pts = src.pts;
    let out = dst.plane_mut(0).unwrap();
    for (i, px) in rgb.chunks_exact(3).enumerate() {
        out[i] = ((px[0] as u16 * 77 + px[1] as u16 * 150 + px[2] as u16 * 29) >> 8) as u8;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_yuv_roundtrip_smoke() {
        let mut src = VideoFrame::alloc(PixelFormat::Rgb24, 16, 16);
        for (i, b) in src.plane_mut(0).unwrap().iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let yuv = rgb24_to_yuv420(&src).unwrap();
        let back = yuv420_to_rgb24(&yuv).unwrap();
        assert_eq!(back.width, 16);
        assert_eq!(back.format, PixelFormat::Rgb24);
    }
}
