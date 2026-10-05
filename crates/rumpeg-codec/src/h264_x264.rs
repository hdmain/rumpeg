//! Optional libx264 encoder backend (`encode-x264` feature).

use rumpeg_util::{CodecParams, CodecSpecific, Error, Result, Timestamp, VideoFrame};
use x264::{Colorspace, Image, Plane, Preset, Setup, Tune};

pub struct X264Backend {
    enc: Option<x264::Encoder>,
    width: i32,
    height: i32,
    frame_idx: i64,
    fps_num: u32,
}

impl X264Backend {
    pub fn open(params: &CodecParams) -> Result<(Self, Vec<u8>)> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let width = video.width.max(2) as i32;
        let height = video.height.max(2) as i32;
        let fps = video.frame_rate.as_f64();
        let fps_num = if fps > 0.0 { fps.round().max(1.0) as u32 } else { 25 };

        let preset = match params.encode_preset.to_ascii_lowercase().as_str() {
            "ultrafast" => Preset::Ultrafast,
            "superfast" => Preset::Superfast,
            "veryfast" => Preset::Veryfast,
            "faster" => Preset::Faster,
            "fast" => Preset::Fast,
            "slow" => Preset::Slow,
            "slower" => Preset::Slower,
            "veryslow" => Preset::Veryslow,
            "placebo" => Preset::Placebo,
            _ => Preset::Medium,
        };

        let mut setup = Setup::preset(preset, Tune::None, false, false)
            .fps(fps_num, 1)
            .annexb(true)
            .high();
        if params.gop_size > 0 {
            setup = setup.max_keyframe_interval(params.gop_size as i32);
        }
        if params.bit_rate > 0 {
            // x264 crate bitrate is in metric kbps.
            let kbps = ((params.bit_rate + 500) / 1000).max(1) as i32;
            setup = setup.bitrate(kbps);
        }

        let mut enc = setup
            .build(Colorspace::I420, width, height)
            .map_err(|_| Error::invalid_data("libx264 open failed (is libx264 installed?)"))?;

        let headers = enc
            .headers()
            .map_err(|_| Error::invalid_data("libx264 headers failed"))?;
        let avcc = super::avcc_from_annexb(headers.entirety())?;

        Ok((
            Self {
                enc: Some(enc),
                width,
                height,
                frame_idx: 0,
                fps_num,
            },
            avcc,
        ))
    }

    pub fn encode(&mut self, video: &VideoFrame) -> Result<Option<(Vec<u8>, Timestamp, bool)>> {
        let enc = self
            .enc
            .as_mut()
            .ok_or_else(|| Error::invalid_data("libx264 already flushed"))?;
        if video.format != rumpeg_util::PixelFormat::Yuv420p {
            return Err(Error::invalid_data("libx264 expects yuv420p"));
        }
        let y = video
            .plane(0)
            .ok_or_else(|| Error::invalid_data("missing Y"))?;
        let u = video
            .plane(1)
            .ok_or_else(|| Error::invalid_data("missing U"))?;
        let v = video
            .plane(2)
            .ok_or_else(|| Error::invalid_data("missing V"))?;

        let planes = [
            Plane {
                stride: self.width,
                data: y,
            },
            Plane {
                stride: self.width / 2,
                data: u,
            },
            Plane {
                stride: self.width / 2,
                data: v,
            },
        ];
        let image = Image::new(Colorspace::I420, self.width, self.height, &planes);
        let pts = self.frame_idx;
        self.frame_idx += 1;
        let (data, pic) = enc
            .encode(pts, image)
            .map_err(|_| Error::invalid_data("libx264 encode failed"))?;
        let bytes = data.entirety().to_vec();
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some((bytes, video.pts, pic.keyframe())))
    }

    pub fn flush(&mut self) -> Result<Vec<(Vec<u8>, Timestamp, bool)>> {
        let Some(enc) = self.enc.take() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut flush = enc.flush();
        while let Some(result) = flush.next() {
            let (data, pic) = result.map_err(|_| Error::invalid_data("libx264 flush failed"))?;
            let bytes = data.entirety().to_vec();
            if !bytes.is_empty() {
                out.push((bytes, Timestamp::NONE, pic.keyframe()));
            }
        }
        Ok(out)
    }
}
