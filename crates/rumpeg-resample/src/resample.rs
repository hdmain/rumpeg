//! Sample-rate conversion.

use crate::convert::{as_f32, convert_samples, f32_bytes};
use rumpeg_util::{AudioFrame, Error, Result, SampleFormat};

/// Stateful resampler that can process successive frames with phase continuity.
pub struct Resampler {
    src_rate: u32,
    dst_rate: u32,
    channels: u16,
    /// Fractional input position carried across frames.
    phase: f64,
}

impl Resampler {
    /// Create a resampler between two rates.
    pub fn new(src_rate: u32, dst_rate: u32, channels: u16) -> Result<Self> {
        if src_rate == 0 || dst_rate == 0 || channels == 0 {
            return Err(Error::invalid_data("invalid resampler rates/channels"));
        }
        Ok(Self {
            src_rate,
            dst_rate,
            channels,
            phase: 0.0,
        })
    }

    /// Resample one interleaved f32 frame; returns output samples per channel.
    pub fn process_f32(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        let ch = self.channels as usize;
        if input.len() % ch != 0 {
            return Err(Error::invalid_data("input not aligned to channels"));
        }
        let in_samples = input.len() / ch;
        if in_samples == 0 {
            return Ok(Vec::new());
        }
        if self.src_rate == self.dst_rate {
            self.phase = 0.0;
            return Ok(input.to_vec());
        }

        let ratio = self.src_rate as f64 / self.dst_rate as f64;
        let out_samples = ((in_samples as f64) / ratio).floor() as usize;
        let mut out = vec![0.0f32; out_samples * ch];

        for i in 0..out_samples {
            let src_pos = self.phase + i as f64 * ratio;
            let i0 = src_pos.floor() as usize;
            let i1 = (i0 + 1).min(in_samples - 1);
            let frac = (src_pos - i0 as f64) as f32;
            for c in 0..ch {
                let a = input[i0 * ch + c];
                let b = input[i1 * ch + c];
                out[i * ch + c] = a + (b - a) * frac;
            }
        }
        self.phase = (self.phase + out_samples as f64 * ratio) - in_samples as f64;
        if self.phase < 0.0 {
            self.phase = 0.0;
        }
        Ok(out)
    }
}

/// Resample a single frame to `dst_rate` (stateless; phase starts at 0).
pub fn resample_frame(src: &AudioFrame, dst_rate: u32) -> Result<AudioFrame> {
    if src.sample_rate == dst_rate {
        return Ok(src.clone());
    }
    let mut resampler = Resampler::new(src.sample_rate, dst_rate, src.layout.channels)?;

    let f32_frame = if src.format == SampleFormat::F32 {
        src.clone()
    } else {
        convert_samples(src, SampleFormat::F32)?
    };
    let input = as_f32(f32_frame.data())?;
    let output = resampler.process_f32(&input)?;
    let out_samples = (output.len() / src.layout.channels as usize) as u32;

    let mut out =
        AudioFrame::alloc_interleaved(SampleFormat::F32, dst_rate, src.layout, out_samples);
    out.pts = src.pts;
    out.data_mut().copy_from_slice(&f32_bytes(&output));

    if src.format == SampleFormat::F32 {
        Ok(out)
    } else {
        convert_samples(&out, src.format)
    }
}
