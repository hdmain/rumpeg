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
        // `phase` is the input-sample position of the next output, relative to
        // this frame, carried over from previous frames. Emit every output
        // whose source position falls inside this frame; using ceil (not floor)
        // lets an output straddle a frame boundary instead of silently dropping
        // the tail samples. With out = ceil((in - phase)/ratio), the new phase
        // is always in [0, ratio) and no input sample is lost.
        let out_samples = (((in_samples as f64 - self.phase) / ratio).ceil() as isize).max(0)
            as usize;
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
        self.phase = self.phase + out_samples as f64 * ratio - in_samples as f64;
        // Normalise against float rounding: phase must stay in [0, ratio).
        if self.phase < 0.0 {
            self.phase = 0.0;
        } else if self.phase >= ratio {
            self.phase -= ratio;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeding many frames through one stateful Resampler must preserve total
    /// duration: sample count out ~= sample count in * dst/src, with error well
    /// under one frame. The old floor() formula dropped the fractional tail of
    /// every frame, so long streams progressively shrank.
    #[test]
    fn phase_continuity_preserves_total_samples() {
        for &(src_rate, dst_rate) in &[(48_000u32, 44_100u32), (44_100, 48_000), (48_000, 22_050)] {
            let mut r = Resampler::new(src_rate, dst_rate, 1).unwrap();
            let frame_len = 1024usize;
            let frames = 200usize;
            let input = vec![0.5f32; frame_len];
            let mut total_in = 0usize;
            let mut total_out = 0usize;
            for _ in 0..frames {
                total_out += r.process_f32(&input).unwrap().len();
                total_in += frame_len;
            }
            let expected = (total_in as f64 * dst_rate as f64 / src_rate as f64).round() as usize;
            let diff = total_out.abs_diff(expected);
            assert!(
                diff <= 1,
                "{src_rate}->{dst_rate}: expected ~{expected} samples, got {total_out} (diff {diff})"
            );
        }
    }

    /// Same-rate passthrough must be exact and reset phase.
    #[test]
    fn same_rate_is_passthrough() {
        let mut r = Resampler::new(48_000, 48_000, 2).unwrap();
        let input = [1.0f32, 2.0, 3.0, 4.0];
        assert_eq!(r.process_f32(&input).unwrap(), input);
    }
}
