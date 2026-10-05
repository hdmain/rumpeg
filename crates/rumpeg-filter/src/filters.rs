//! Built-in filters.

use crate::graph::Filter;
use rumpeg_resample::convert_samples;
use rumpeg_scale::{transform, FilterMode};
use rumpeg_util::{
    AudioFrame, AudioParams, ChannelLayout, Error, Frame, PixelFormat, Result, SampleFormat,
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
    width: u32,
    height: u32,
}

impl ScaleFilter {
    /// Parse size.
    pub fn parse(args: &str) -> Result<Self> {
        let args = args.replace('x', ":");
        let (w, h) = args
            .split_once(':')
            .ok_or_else(|| Error::invalid_data("scale expects WIDTHxHEIGHT"))?;
        Ok(Self {
            width: w
                .parse()
                .map_err(|_| Error::invalid_data("bad scale width"))?,
            height: h
                .parse()
                .map_err(|_| Error::invalid_data("bad scale height"))?,
        })
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
        let out = transform(
            &video,
            video.format,
            self.width,
            self.height,
            FilterMode::Bilinear,
        )?;
        Ok(vec![Frame::Video(out)])
    }
}
