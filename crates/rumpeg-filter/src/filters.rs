//! Built-in filters.

use crate::graph::Filter;
use rumpeg_resample::convert_samples;
use rumpeg_scale::{transform, FilterMode};
use rumpeg_util::{
    AudioFrame, AudioParams, ChannelLayout, Error, Frame, PixelFormat, Result, SampleFormat,
    Timestamp, VideoFrame,
};

/// Multiply audio amplitude by a constant factor.
pub struct VolumeFilter {
    factor: f32,
}

impl VolumeFilter {
    /// Parse `volume=1.5` style args.
    pub fn parse(args: &str) -> Result<Self> {
        let factor: f32 = args
            .parse()
            .map_err(|_| Error::invalid_data(format!("bad volume factor '{args}'")))?;
        Ok(Self { factor })
    }
}

impl Filter for VolumeFilter {
    fn name(&self) -> &str {
        "volume"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Audio(mut audio) = frame else {
            return Err(Error::invalid_data("volume filter expects audio"));
        };
        apply_volume(&mut audio, self.factor)?;
        Ok(vec![Frame::Audio(audio)])
    }
}

fn apply_volume(audio: &mut AudioFrame, factor: f32) -> Result<()> {
    match audio.format {
        SampleFormat::F32 => {
            let data = audio.data_mut();
            for chunk in data.chunks_exact_mut(4) {
                let mut v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                v *= factor;
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
        SampleFormat::S16 => {
            let data = audio.data_mut();
            for chunk in data.chunks_exact_mut(2) {
                let mut v = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
                v = (v * factor).clamp(-32768.0, 32767.0);
                chunk.copy_from_slice(&(v as i16).to_le_bytes());
            }
        }
        other => {
            // Convert to f32, apply, convert back.
            let fmt = other;
            let mut f = convert_samples(audio, SampleFormat::F32)?;
            apply_volume(&mut f, factor)?;
            *audio = convert_samples(&f, fmt)?;
        }
    }
    Ok(())
}

/// Audio format conversion filter (`aformat=`).
pub struct AFormatFilter {
    sample_fmt: Option<SampleFormat>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
}

impl AFormatFilter {
    /// Parse `aformat=sample_fmts=s16:sample_rates=48000:channel_layouts=stereo`.
    pub fn parse(args: &str) -> Result<Self> {
        let mut f = Self {
            sample_fmt: None,
            sample_rate: None,
            channels: None,
        };
        for part in args.split(':') {
            if part.is_empty() {
                continue;
            }
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| Error::invalid_data(format!("bad aformat arg '{part}'")))?;
            match k {
                "sample_fmts" => {
                    f.sample_fmt = Some(match v {
                        "u8" => SampleFormat::U8,
                        "s16" => SampleFormat::S16,
                        "s32" => SampleFormat::S32,
                        "flt" | "f32" => SampleFormat::F32,
                        other => {
                            return Err(Error::unsupported(format!("sample format {other}")));
                        }
                    });
                }
                "sample_rates" => {
                    f.sample_rate = Some(
                        v.parse()
                            .map_err(|_| Error::invalid_data("bad sample_rates"))?,
                    );
                }
                "channel_layouts" => {
                    f.channels = Some(match v {
                        "mono" => 1,
                        "stereo" => 2,
                        other => other
                            .parse()
                            .map_err(|_| Error::invalid_data("bad channel_layouts"))?,
                    });
                }
                _ => {}
            }
        }
        Ok(f)
    }
}

impl Filter for AFormatFilter {
    fn name(&self) -> &str {
        "aformat"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Audio(audio) = frame else {
            return Err(Error::invalid_data("aformat expects audio"));
        };
        let params = AudioParams {
            sample_fmt: self.sample_fmt.unwrap_or(audio.format),
            sample_rate: self.sample_rate.unwrap_or(audio.sample_rate),
            layout: ChannelLayout::new(self.channels.unwrap_or(audio.layout.channels)),
            frame_size: 0,
        };
        let out = rumpeg_resample::transform(&audio, &params)?;
        Ok(vec![Frame::Audio(out)])
    }
}

/// Video pixel format filter (`format=pix_fmts=rgb24`).
pub struct FormatFilter {
    pix_fmt: PixelFormat,
}

impl FormatFilter {
    /// Parse args.
    pub fn parse(args: &str) -> Result<Self> {
        let name = args
            .strip_prefix("pix_fmts=")
            .or_else(|| args.split('=').next_back())
            .unwrap_or(args);
        let pix_fmt = PixelFormat::from_name(name)
            .ok_or_else(|| Error::not_found(format!("pixel format '{name}'")))?;
        Ok(Self { pix_fmt })
    }
}

impl Filter for FormatFilter {
    fn name(&self) -> &str {
        "format"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("format filter expects video"));
        };
        let out = transform(
            &video,
            self.pix_fmt,
            video.width,
            video.height,
            FilterMode::Bilinear,
        )?;
        Ok(vec![Frame::Video(out)])
    }
}

/// Video scale filter (`scale=640x360` or `scale=640:360`).
pub struct ScaleFilter {
    /// Target width; `0` or negative specials resolved at filter time (`-1`/`-2` = keep aspect).
    width: i32,
    /// Target height; same rules as width.
    height: i32,
}

impl ScaleFilter {
    /// Parse `scale=W:H` / `WxH`. Negative dims keep aspect (`-2` = even).
    pub fn parse(args: &str) -> Result<Self> {
        let args = args.replace('x', ":");
        let (w, h) = args
            .split_once(':')
            .ok_or_else(|| Error::invalid_data("scale expects WIDTHxHEIGHT"))?;
        let width: i32 = w
            .parse()
            .map_err(|_| Error::invalid_data("bad scale width"))?;
        let height: i32 = h
            .parse()
            .map_err(|_| Error::invalid_data("bad scale height"))?;
        if width == 0 && height == 0 {
            return Err(Error::invalid_data("scale width/height cannot both be 0"));
        }
        Ok(Self { width, height })
    }

    /// Resolved output size for a source frame.
    pub fn resolve(src_w: u32, src_h: u32, tw: i32, th: i32) -> (u32, u32) {
        let sw = src_w.max(1) as i64;
        let sh = src_h.max(1) as i64;
        let (mut ow, mut oh) = match (tw > 0, th > 0) {
            (true, true) => (i64::from(tw), i64::from(th)),
            (true, false) => {
                let h = (sh * i64::from(tw) + sw / 2) / sw;
                (i64::from(tw), h)
            }
            (false, true) => {
                let w = (sw * i64::from(th) + sh / 2) / sh;
                (w, i64::from(th))
            }
            (false, false) => (sw, sh),
        };
        ow -= ow % 2;
        oh -= oh % 2;
        (ow.max(2) as u32, oh.max(2) as u32)
    }
}

impl Filter for ScaleFilter {
    fn name(&self) -> &str {
        "scale"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("scale expects video"));
        };
        let (w, h) = Self::resolve(video.width, video.height, self.width, self.height);
        let out = transform(&video, video.format, w, h, FilterMode::Bilinear)?;
        Ok(vec![Frame::Video(out)])
    }
}

/// Horizontal flip (`hflip`).
pub struct HFlipFilter;

impl HFlipFilter {
    /// No args.
    pub fn parse(_args: &str) -> Result<Self> {
        Ok(Self)
    }
}

impl Filter for HFlipFilter {
    fn name(&self) -> &str {
        "hflip"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("hflip expects video"));
        };
        Ok(vec![Frame::Video(hflip_video(&video)?)])
    }
}

fn hflip_video(src: &VideoFrame) -> Result<VideoFrame> {
    let mut out = VideoFrame::alloc(src.format, src.width, src.height);
    out.pts = src.pts;
    out.key_frame = src.key_frame;
    let planes = match src.format {
        PixelFormat::Yuv420p => 3,
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => 1,
        PixelFormat::Rgba | PixelFormat::Bgra => 1,
        other => {
            return Err(Error::unsupported(format!(
                "hflip does not support pixel format {other}"
            )));
        }
    };
    for p in 0..planes {
        let (sw, sh, bpp) = plane_geom(src, p);
        let src_plane = src
            .plane(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let dst_plane = out
            .plane_mut(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let stride = sw * bpp;
        for row in 0..sh {
            let srow = &src_plane[row * stride..(row + 1) * stride];
            let drow = &mut dst_plane[row * stride..(row + 1) * stride];
            for x in 0..sw {
                let sx = (sw - 1 - x) * bpp;
                let dx = x * bpp;
                drow[dx..dx + bpp].copy_from_slice(&srow[sx..sx + bpp]);
            }
        }
    }
    Ok(out)
}

fn plane_geom(frame: &VideoFrame, plane: usize) -> (usize, usize, usize) {
    match frame.format {
        PixelFormat::Yuv420p => {
            if plane == 0 {
                (frame.width as usize, frame.height as usize, 1)
            } else {
                ((frame.width as usize) / 2, (frame.height as usize) / 2, 1)
            }
        }
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => (frame.width as usize, frame.height as usize, 3),
        PixelFormat::Rgba | PixelFormat::Bgra => (frame.width as usize, frame.height as usize, 4),
        _ => (frame.width as usize, frame.height as usize, 1),
    }
}

/// Crop to `W:H` with optional `:X:Y` offset (default 0:0).
pub struct CropFilter {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
}

impl CropFilter {
    /// Parse `crop=W:H` or `crop=W:H:X:Y`.
    pub fn parse(args: &str) -> Result<Self> {
        let parts: Vec<&str> = args.split(':').collect();
        if parts.len() < 2 || parts.len() > 4 {
            return Err(Error::invalid_data("crop expects W:H or W:H:X:Y"));
        }
        let width: u32 = parts[0]
            .parse()
            .map_err(|_| Error::invalid_data("bad crop width"))?;
        let height: u32 = parts[1]
            .parse()
            .map_err(|_| Error::invalid_data("bad crop height"))?;
        let x = if parts.len() > 2 {
            parts[2]
                .parse()
                .map_err(|_| Error::invalid_data("bad crop x"))?
        } else {
            0
        };
        let y = if parts.len() > 3 {
            parts[3]
                .parse()
                .map_err(|_| Error::invalid_data("bad crop y"))?
        } else {
            0
        };
        Ok(Self {
            width,
            height,
            x,
            y,
        })
    }
}

impl Filter for CropFilter {
    fn name(&self) -> &str {
        "crop"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("crop expects video"));
        };
        Ok(vec![Frame::Video(crop_video(
            &video,
            self.width,
            self.height,
            self.x,
            self.y,
        )?)])
    }
}

fn crop_video(src: &VideoFrame, w: u32, h: u32, x: u32, y: u32) -> Result<VideoFrame> {
    if x + w > src.width || y + h > src.height {
        return Err(Error::invalid_data("crop rectangle out of bounds"));
    }
    let mut out = VideoFrame::alloc(src.format, w, h);
    out.pts = src.pts;
    out.key_frame = src.key_frame;
    let planes = match src.format {
        PixelFormat::Yuv420p => 3,
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => 1,
        other => {
            return Err(Error::unsupported(format!(
                "crop does not support pixel format {other}"
            )));
        }
    };
    for p in 0..planes {
        let (sw, _sh, bpp) = plane_geom(src, p);
        let (dw, dh, _) = plane_geom(&out, p);
        let x_off = if p == 0 { x as usize } else { (x / 2) as usize };
        let y_off = if p == 0 { y as usize } else { (y / 2) as usize };
        let src_plane = src
            .plane(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let dst_plane = out
            .plane_mut(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let stride = sw * bpp;
        let dst_stride = dw * bpp;
        for row in 0..dh {
            let srow = &src_plane[(y_off + row) * stride + x_off * bpp..];
            let drow = &mut dst_plane[row * dst_stride..(row + 1) * dst_stride];
            drow.copy_from_slice(&srow[..dst_stride]);
        }
    }
    Ok(out)
}

/// Pad to `W:H` placing input at `:X:Y` with optional `:color` (default black).
pub struct PadFilter {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    color: PadColor,
}

#[derive(Clone, Copy)]
struct PadColor {
    y: u8,
    u: u8,
    v: u8,
    rgb: [u8; 3],
}

impl PadColor {
    fn black() -> Self {
        Self {
            y: 16,
            u: 128,
            v: 128,
            rgb: [0, 0, 0],
        }
    }

    fn parse(name: &str) -> Result<Self> {
        if name.eq_ignore_ascii_case("black") {
            return Ok(Self::black());
        }
        let hex = name.strip_prefix("0x").unwrap_or(name);
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16)
                .map_err(|_| Error::invalid_data("bad pad color"))?;
            let g = u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| Error::invalid_data("bad pad color"))?;
            let b = u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| Error::invalid_data("bad pad color"))?;
            return Ok(Self {
                y: ((66 * r as u32 + 129 * g as u32 + 25 * b as u32 + 128) >> 8) as u8,
                u: ((-38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128) >> 8) as u8,
                v: ((112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128) >> 8) as u8,
                rgb: [r, g, b],
            });
        }
        Err(Error::invalid_data(format!("unknown pad color '{name}'")))
    }
}

impl PadFilter {
    /// Parse `pad=W:H:X:Y` or with trailing `:color`.
    pub fn parse(args: &str) -> Result<Self> {
        let parts: Vec<&str> = args.split(':').collect();
        if parts.len() < 4 {
            return Err(Error::invalid_data("pad expects W:H:X:Y[:color]"));
        }
        let width: u32 = parts[0]
            .parse()
            .map_err(|_| Error::invalid_data("bad pad width"))?;
        let height: u32 = parts[1]
            .parse()
            .map_err(|_| Error::invalid_data("bad pad height"))?;
        let x: u32 = parts[2]
            .parse()
            .map_err(|_| Error::invalid_data("bad pad x"))?;
        let y: u32 = parts[3]
            .parse()
            .map_err(|_| Error::invalid_data("bad pad y"))?;
        let color = if parts.len() > 4 {
            PadColor::parse(parts[4])?
        } else {
            PadColor::black()
        };
        Ok(Self {
            width,
            height,
            x,
            y,
            color,
        })
    }
}

impl Filter for PadFilter {
    fn name(&self) -> &str {
        "pad"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("pad expects video"));
        };
        Ok(vec![Frame::Video(pad_video(
            &video,
            self.width,
            self.height,
            self.x,
            self.y,
            self.color,
        )?)])
    }
}

fn pad_video(
    src: &VideoFrame,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
    color: PadColor,
) -> Result<VideoFrame> {
    if x + src.width > w || y + src.height > h {
        return Err(Error::invalid_data(
            "pad canvas too small for input at offset",
        ));
    }
    let mut out = VideoFrame::alloc(src.format, w, h);
    out.pts = src.pts;
    out.key_frame = src.key_frame;
    match src.format {
        PixelFormat::Yuv420p => {
            fill_plane(
                out.plane_mut(0).unwrap(),
                w as usize,
                h as usize,
                1,
                color.y,
            );
            fill_plane(
                out.plane_mut(1).unwrap(),
                (w / 2) as usize,
                (h / 2) as usize,
                1,
                color.u,
            );
            fill_plane(
                out.plane_mut(2).unwrap(),
                (w / 2) as usize,
                (h / 2) as usize,
                1,
                color.v,
            );
            blit_plane(
                src.plane(0).unwrap(),
                src.width as usize,
                src.height as usize,
                1,
                out.plane_mut(0).unwrap(),
                w as usize,
                x as usize,
                y as usize,
            );
            blit_plane(
                src.plane(1).unwrap(),
                (src.width / 2) as usize,
                (src.height / 2) as usize,
                1,
                out.plane_mut(1).unwrap(),
                (w / 2) as usize,
                (x / 2) as usize,
                (y / 2) as usize,
            );
            blit_plane(
                src.plane(2).unwrap(),
                (src.width / 2) as usize,
                (src.height / 2) as usize,
                1,
                out.plane_mut(2).unwrap(),
                (w / 2) as usize,
                (x / 2) as usize,
                (y / 2) as usize,
            );
        }
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
            let bpp = 3;
            fill_rgb(
                out.plane_mut(0).unwrap(),
                w as usize,
                h as usize,
                bpp,
                color.rgb,
            );
            blit_plane(
                src.plane(0).unwrap(),
                src.width as usize,
                src.height as usize,
                bpp,
                out.plane_mut(0).unwrap(),
                w as usize,
                x as usize,
                y as usize,
            );
        }
        other => {
            return Err(Error::unsupported(format!(
                "pad does not support pixel format {other}"
            )));
        }
    }
    Ok(out)
}

fn fill_plane(data: &mut [u8], w: usize, h: usize, bpp: usize, value: u8) {
    for row in 0..h {
        for col in 0..w {
            let i = row * w * bpp + col * bpp;
            for b in 0..bpp {
                data[i + b] = value;
            }
        }
    }
}

fn fill_rgb(data: &mut [u8], w: usize, h: usize, bpp: usize, rgb: [u8; 3]) {
    for row in 0..h {
        for col in 0..w {
            let i = row * w * bpp + col * bpp;
            data[i..i + bpp].copy_from_slice(&rgb[..bpp]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_plane(
    src: &[u8],
    sw: usize,
    sh: usize,
    bpp: usize,
    dst: &mut [u8],
    dw: usize,
    dx: usize,
    dy: usize,
) {
    let stride_s = sw * bpp;
    let stride_d = dw * bpp;
    for row in 0..sh {
        let srow = &src[row * stride_s..(row + 1) * stride_s];
        let drow = &mut dst[(dy + row) * stride_d + dx * bpp..];
        drow[..stride_s].copy_from_slice(srow);
    }
}

/// Vertical flip (`vflip`).
pub struct VFlipFilter;

impl VFlipFilter {
    /// No args.
    pub fn parse(_args: &str) -> Result<Self> {
        Ok(Self)
    }
}

impl Filter for VFlipFilter {
    fn name(&self) -> &str {
        "vflip"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("vflip expects video"));
        };
        Ok(vec![Frame::Video(vflip_video(&video)?)])
    }
}

fn vflip_video(src: &VideoFrame) -> Result<VideoFrame> {
    let mut out = VideoFrame::alloc(src.format, src.width, src.height);
    out.pts = src.pts;
    out.key_frame = src.key_frame;
    let planes = match src.format {
        PixelFormat::Yuv420p => 3,
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => 1,
        other => {
            return Err(Error::unsupported(format!(
                "vflip does not support pixel format {other}"
            )));
        }
    };
    for p in 0..planes {
        let (sw, sh, bpp) = plane_geom(src, p);
        let src_plane = src
            .plane(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let dst_plane = out
            .plane_mut(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let stride = sw * bpp;
        for row in 0..sh {
            let srow = &src_plane[row * stride..(row + 1) * stride];
            let drow = &mut dst_plane[(sh - 1 - row) * stride..(sh - row) * stride];
            drow.copy_from_slice(srow);
        }
    }
    Ok(out)
}

/// Transpose / rotate by 90° (`transpose=clock|cclock|…`).
pub struct TransposeFilter {
    mode: TransposeMode,
}

#[derive(Clone, Copy)]
enum TransposeMode {
    Clock,
    CounterClock,
}

impl TransposeFilter {
    /// Parse transpose mode name or FFmpeg numeric aliases.
    pub fn parse(args: &str) -> Result<Self> {
        let mode = match args {
            "clock" | "1" | "clock_flip" => TransposeMode::Clock,
            "cclock" | "2" | "cclock_flip" => TransposeMode::CounterClock,
            other => {
                return Err(Error::unsupported(format!(
                    "transpose mode '{other}' (use clock or cclock)"
                )));
            }
        };
        Ok(Self { mode })
    }
}

impl Filter for TransposeFilter {
    fn name(&self) -> &str {
        "transpose"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("transpose expects video"));
        };
        Ok(vec![Frame::Video(transpose_video(&video, self.mode)?)])
    }
}

fn transpose_video(src: &VideoFrame, mode: TransposeMode) -> Result<VideoFrame> {
    let (out_w, out_h) = (src.height, src.width);
    let mut out = VideoFrame::alloc(src.format, out_w, out_h);
    out.pts = src.pts;
    out.key_frame = src.key_frame;
    let planes = match src.format {
        PixelFormat::Yuv420p => 3,
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => 1,
        other => {
            return Err(Error::unsupported(format!(
                "transpose does not support pixel format {other}"
            )));
        }
    };
    for p in 0..planes {
        let (sw, sh, bpp) = plane_geom(src, p);
        let (dw, _dh, _) = plane_geom(&out, p);
        let src_plane = src
            .plane(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        let dst_plane = out
            .plane_mut(p)
            .ok_or_else(|| Error::invalid_data("missing plane"))?;
        for sy in 0..sh {
            for sx in 0..sw {
                let (dx, dy) = match mode {
                    TransposeMode::Clock => (sy, sw - 1 - sx),
                    TransposeMode::CounterClock => (sh - 1 - sy, sx),
                };
                copy_pixel(src_plane, sw, bpp, sx, sy, dst_plane, dw, bpp, dx, dy);
            }
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn copy_pixel(
    src: &[u8],
    sw: usize,
    sbpp: usize,
    sx: usize,
    sy: usize,
    dst: &mut [u8],
    dw: usize,
    dbpp: usize,
    dx: usize,
    dy: usize,
) {
    let si = (sy * sw + sx) * sbpp;
    let di = (dy * dw + dx) * dbpp;
    dst[di..di + dbpp].copy_from_slice(&src[si..si + sbpp]);
}

/// Change frame rate by dropping or duplicating frames.
///
/// Accumulator uses integer ratios of `target` / `source` fps (Bresenham-style).
/// Output frames get rewritten PTS assuming `time_base = 1/target` ticks when
/// `rewrite_pts` is true (default).
pub struct FpsFilter {
    target: u32,
    source: u32,
    acc: i64,
    out_index: u64,
    rewrite_pts: bool,
}

impl FpsFilter {
    /// Parse `fps=N` or `fps=N:source=M`.
    pub fn parse(args: &str) -> Result<Self> {
        let mut target: Option<u32> = None;
        let mut source: Option<u32> = None;
        if args.contains('=') && args.contains(':') {
            for part in args.split(':') {
                if let Some((k, v)) = part.split_once('=') {
                    match k {
                        "fps" | "rate" => {
                            target = Some(
                                v.parse()
                                    .map_err(|_| Error::invalid_data(format!("bad fps '{v}'")))?,
                            );
                        }
                        "source" | "src" => {
                            source = Some(v.parse().map_err(|_| {
                                Error::invalid_data(format!("bad source fps '{v}'"))
                            })?);
                        }
                        _ => {}
                    }
                } else if target.is_none() {
                    target = Some(
                        part.parse()
                            .map_err(|_| Error::invalid_data(format!("bad fps '{part}'")))?,
                    );
                }
            }
        } else if let Some((t, s)) = args.split_once(':') {
            // `fps=30:25` → target:source
            target = Some(
                t.parse()
                    .map_err(|_| Error::invalid_data(format!("bad fps '{t}'")))?,
            );
            source = Some(
                s.parse()
                    .map_err(|_| Error::invalid_data(format!("bad source fps '{s}'")))?,
            );
        } else {
            target = Some(
                args.parse()
                    .map_err(|_| Error::invalid_data(format!("bad fps value '{args}'")))?,
            );
        }
        let target = target.ok_or_else(|| Error::invalid_data("fps requires a target rate"))?;
        if target == 0 {
            return Err(Error::invalid_data("fps must be > 0"));
        }
        let source = source.unwrap_or(0);
        Ok(Self {
            target,
            source,
            acc: 0,
            out_index: 0,
            rewrite_pts: true,
        })
    }

    /// Create with explicit target and source rates.
    pub fn new(target: u32, source: u32) -> Result<Self> {
        if target == 0 {
            return Err(Error::invalid_data("fps must be > 0"));
        }
        Ok(Self {
            target,
            source: if source == 0 { target } else { source },
            acc: 0,
            out_index: 0,
            rewrite_pts: true,
        })
    }

    /// Override source fps (from demuxer stream metadata). `0` keeps current.
    pub fn with_source_fps(&mut self, source: u32) {
        if source > 0 {
            self.source = source;
        }
    }

    /// Target fps.
    pub fn target_fps(&self) -> u32 {
        self.target
    }
}

impl Filter for FpsFilter {
    fn name(&self) -> &str {
        "fps"
    }

    fn set_source_fps(&mut self, source_fps: u32) {
        self.with_source_fps(source_fps);
    }

    fn target_fps(&self) -> Option<u32> {
        Some(self.target)
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(mut video) = frame else {
            return Err(Error::invalid_data("fps expects video"));
        };
        let source = if self.source == 0 { 25 } else { self.source };
        // Emit floor((acc + target) / source) copies; for down-convert usually 0 or 1.
        self.acc += i64::from(self.target);
        let mut out = Vec::new();
        while self.acc >= i64::from(source) {
            self.acc -= i64::from(source);
            let mut f = video.clone();
            if self.rewrite_pts {
                // PTS in 1/target seconds.
                f.pts = Timestamp::new(self.out_index as i64);
            }
            self.out_index += 1;
            out.push(Frame::Video(f));
        }
        // Avoid cloning the last unused frame when nothing emitted.
        let _ = &mut video;
        Ok(out)
    }
}

/// Keep every Nth frame (`framestep=N`, 1-based stride).
pub struct FramestepFilter {
    step: u64,
    index: u64,
}

impl FramestepFilter {
    /// Parse `framestep=N`.
    pub fn parse(args: &str) -> Result<Self> {
        let step: u64 = args
            .parse()
            .map_err(|_| Error::invalid_data(format!("bad framestep '{args}'")))?;
        if step == 0 {
            return Err(Error::invalid_data("framestep must be >= 1"));
        }
        Ok(Self { step, index: 0 })
    }
}

impl Filter for FramestepFilter {
    fn name(&self) -> &str {
        "framestep"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(_) = frame else {
            return Err(Error::invalid_data("framestep expects video"));
        };
        self.index += 1;
        if (self.index - 1) % self.step == 0 {
            Ok(vec![frame])
        } else {
            Ok(vec![])
        }
    }
}

/// Linear contrast / brightness (`eq=…`).
pub struct EqFilter {
    contrast: f32,
    brightness: f32,
}

impl EqFilter {
    /// Parse `eq=c:b` or `eq=contrast=c:brightness=b`.
    pub fn parse(args: &str) -> Result<Self> {
        let mut contrast = 1.0f32;
        let mut brightness = 0.0f32;
        if args.contains('=') {
            for part in args.split(':') {
                let (k, v) = part
                    .split_once('=')
                    .ok_or_else(|| Error::invalid_data(format!("bad eq arg '{part}'")))?;
                match k {
                    "contrast" => {
                        contrast = v.parse().map_err(|_| Error::invalid_data("bad contrast"))?;
                    }
                    "brightness" => {
                        brightness = v
                            .parse()
                            .map_err(|_| Error::invalid_data("bad brightness"))?;
                    }
                    _ => {}
                }
            }
        } else {
            let parts: Vec<&str> = args.split(':').collect();
            if parts.is_empty() {
                return Err(Error::invalid_data("eq expects contrast:brightness"));
            }
            contrast = parts[0]
                .parse()
                .map_err(|_| Error::invalid_data("bad contrast"))?;
            if parts.len() > 1 {
                brightness = parts[1]
                    .parse()
                    .map_err(|_| Error::invalid_data("bad brightness"))?;
            }
        }
        Ok(Self {
            contrast,
            brightness,
        })
    }
}

impl Filter for EqFilter {
    fn name(&self) -> &str {
        "eq"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("eq expects video"));
        };
        Ok(vec![Frame::Video(apply_eq(
            &video,
            self.contrast,
            self.brightness,
        )?)])
    }
}

fn apply_eq(src: &VideoFrame, contrast: f32, brightness: f32) -> Result<VideoFrame> {
    let mut out = src.clone();
    match src.format {
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
            let plane = out.plane_mut(0).unwrap();
            for chunk in plane.chunks_exact_mut(3) {
                for c in chunk.iter_mut() {
                    let v = *c as f32;
                    *c = ((v - 128.0) * contrast + 128.0 + brightness).clamp(0.0, 255.0) as u8;
                }
            }
        }
        PixelFormat::Yuv420p => {
            let y = out.plane_mut(0).unwrap();
            for p in y.iter_mut() {
                let v = *p as f32;
                *p = ((v - 128.0) * contrast + 128.0 + brightness).clamp(0.0, 255.0) as u8;
            }
        }
        other => {
            return Err(Error::unsupported(format!(
                "eq does not support pixel format {other}"
            )));
        }
    }
    Ok(out)
}

/// Hue rotation in degrees (RGB approximation).
pub struct HueFilter {
    degrees: f32,
}

impl HueFilter {
    /// Parse `hue=h` or `hue=h=degrees`.
    pub fn parse(args: &str) -> Result<Self> {
        let degrees = if let Some((_, v)) = args.split_once('=') {
            v.parse()
                .map_err(|_| Error::invalid_data("bad hue degrees"))?
        } else {
            args.parse()
                .map_err(|_| Error::invalid_data(format!("bad hue '{args}'")))?
        };
        Ok(Self { degrees })
    }
}

impl Filter for HueFilter {
    fn name(&self) -> &str {
        "hue"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let Frame::Video(video) = frame else {
            return Err(Error::invalid_data("hue expects video"));
        };
        Ok(vec![Frame::Video(apply_hue(&video, self.degrees)?)])
    }
}

fn apply_hue(src: &VideoFrame, degrees: f32) -> Result<VideoFrame> {
    let rgb = if src.format == PixelFormat::Rgb24 {
        src.clone()
    } else if src.format == PixelFormat::Yuv420p {
        transform(
            src,
            PixelFormat::Rgb24,
            src.width,
            src.height,
            FilterMode::Bilinear,
        )?
    } else {
        return Err(Error::unsupported(format!(
            "hue does not support pixel format {}",
            src.format
        )));
    };
    let mut out = rgb.clone();
    let plane = out.plane_mut(0).unwrap();
    let rad = degrees.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();
    for chunk in plane.chunks_exact_mut(3) {
        let r = chunk[0] as f32 / 255.0;
        let g = chunk[1] as f32 / 255.0;
        let b = chunk[2] as f32 / 255.0;
        let nr = (0.299 + 0.701 * cos + 0.168 * sin) * r
            + (0.587 - 0.587 * cos + 0.330 * sin) * g
            + (0.114 - 0.114 * cos - 0.497 * sin) * b;
        let ng = (0.299 - 0.299 * cos - 0.328 * sin) * r
            + (0.587 + 0.413 * cos + 0.035 * sin) * g
            + (0.114 - 0.114 * cos + 0.292 * sin) * b;
        let nb = (0.299 - 0.300 * cos + 1.250 * sin) * r
            + (0.587 - 0.588 * cos - 1.050 * sin) * g
            + (0.114 + 0.886 * cos - 0.203 * sin) * b;
        chunk[0] = (nr.clamp(0.0, 1.0) * 255.0) as u8;
        chunk[1] = (ng.clamp(0.0, 1.0) * 255.0) as u8;
        chunk[2] = (nb.clamp(0.0, 1.0) * 255.0) as u8;
    }
    if src.format == PixelFormat::Yuv420p {
        return transform(
            &out,
            PixelFormat::Yuv420p,
            src.width,
            src.height,
            FilterMode::Bilinear,
        );
    }
    Ok(out)
}

/// Resample audio to a target rate (`aresample=RATE`, alias for `aformat`).
pub struct AresampleFilter {
    inner: AFormatFilter,
}

impl AresampleFilter {
    /// Parse sample rate.
    pub fn parse(args: &str) -> Result<Self> {
        let rate_str = args
            .strip_prefix("rate=")
            .or_else(|| args.strip_prefix("sample_rates="))
            .unwrap_or(args);
        let rate: u32 = rate_str
            .parse()
            .map_err(|_| Error::invalid_data(format!("bad aresample rate '{args}'")))?;
        Ok(Self {
            inner: AFormatFilter {
                sample_fmt: None,
                sample_rate: Some(rate),
                channels: None,
            },
        })
    }
}

impl Filter for AresampleFilter {
    fn name(&self) -> &str {
        "aresample"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        self.inner.filter(frame)
    }
}

/// Overlay position parser — dual-input not wired; passthrough with accepted `x:y`.
pub struct OverlayFilter {
    _x: i32,
    _y: i32,
}

impl OverlayFilter {
    /// Parse `overlay=x:y` (requires dual-input; currently passthrough).
    pub fn parse(args: &str) -> Result<Self> {
        let (x, y) = args
            .split_once(':')
            .ok_or_else(|| Error::invalid_data("overlay expects x:y"))?;
        Ok(Self {
            _x: x
                .parse()
                .map_err(|_| Error::invalid_data("bad overlay x"))?,
            _y: y
                .parse()
                .map_err(|_| Error::invalid_data("bad overlay y"))?,
        })
    }
}

impl Filter for OverlayFilter {
    fn name(&self) -> &str {
        "overlay"
    }

    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        Ok(vec![frame])
    }
}
