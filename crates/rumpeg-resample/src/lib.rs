//! Audio resampling and sample-format conversion — `libswresample` counterpart.

#![deny(missing_docs)]
#![warn(rust_2018_idioms)]

mod convert;
mod resample;

pub use convert::convert_samples;
pub use resample::{resample_frame, Resampler};

use rumpeg_util::{AudioFrame, AudioParams, ChannelLayout, Error, Result, SampleFormat};

/// Full audio transform: format + rate + channel layout.
pub fn transform(src: &AudioFrame, dst: &AudioParams) -> Result<AudioFrame> {
    if src.samples == 0 {
        return Err(Error::invalid_data("empty audio frame"));
    }

    let mut frame = if src.format == dst.sample_fmt {
        src.clone()
    } else {
        convert_samples(src, dst.sample_fmt)?
    };

    if frame.layout.channels != dst.layout.channels {
        frame = rematrix(&frame, dst.layout)?;
    }

    if frame.sample_rate != dst.sample_rate {
        frame = resample_frame(&frame, dst.sample_rate)?;
    }

    Ok(frame)
}

fn rematrix(src: &AudioFrame, layout: ChannelLayout) -> Result<AudioFrame> {
    if src.layout.channels == layout.channels {
        return Ok(src.clone());
    }

    let f32_frame = if src.format == SampleFormat::F32 {
        src.clone()
    } else {
        convert_samples(src, SampleFormat::F32)?
    };

    let samples = f32_frame.samples as usize;
    let src_ch = f32_frame.layout.channels as usize;
    let dst_ch = layout.channels as usize;
    let src_data = convert::as_f32(f32_frame.data())?;

    let mut out_samples = vec![0.0f32; samples * dst_ch];
    match (src_ch, dst_ch) {
        (1, 2) => {
            for i in 0..samples {
                let s = src_data[i];
                out_samples[i * 2] = s;
                out_samples[i * 2 + 1] = s;
            }
        }
        (2, 1) => {
            for i in 0..samples {
                out_samples[i] = 0.5 * (src_data[i * 2] + src_data[i * 2 + 1]);
            }
        }
        _ => {
            for i in 0..samples {
                for c in 0..dst_ch {
                    out_samples[i * dst_ch + c] = if c < src_ch {
                        src_data[i * src_ch + c]
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    let mut out = AudioFrame::alloc_interleaved(
        SampleFormat::F32,
        f32_frame.sample_rate,
        layout,
        f32_frame.samples,
    );
    out.pts = src.pts;
    out.data_mut()
        .copy_from_slice(&convert::f32_bytes(&out_samples));

    if src.format == SampleFormat::F32 {
        Ok(out)
    } else {
        convert_samples(&out, src.format)
    }
}
