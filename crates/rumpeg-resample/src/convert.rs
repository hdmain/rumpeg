//! Sample format conversion.

use rumpeg_util::{AudioFrame, Error, Result, SampleFormat};

/// Convert an interleaved audio frame to another sample format.
pub fn convert_samples(src: &AudioFrame, dst_fmt: SampleFormat) -> Result<AudioFrame> {
    if src.format == dst_fmt {
        return Ok(src.clone());
    }
    if src.format.is_planar() || dst_fmt.is_planar() {
        return Err(Error::unsupported(
            "planar sample conversion not yet implemented",
        ));
    }

    // Prefer a direct path; otherwise go through f32.
    let bytes = if let Some(out) = try_direct(src, dst_fmt)? {
        out
    } else {
        let mid = if src.format == SampleFormat::F32 {
            src.clone()
        } else {
            let mid_bytes = try_direct(src, SampleFormat::F32)?
                .ok_or_else(|| Error::unsupported(format!("to f32 from {:?}", src.format)))?;
            let mut mid = AudioFrame::alloc_interleaved(
                SampleFormat::F32,
                src.sample_rate,
                src.layout,
                src.samples,
            );
            mid.pts = src.pts;
            mid.data_mut().copy_from_slice(&mid_bytes);
            mid
        };
        if dst_fmt == SampleFormat::F32 {
            return Ok(mid);
        }
        try_direct(&mid, dst_fmt)?.ok_or_else(|| {
            Error::unsupported(format!(
                "sample conversion {:?} -> {:?}",
                src.format, dst_fmt
            ))
        })?
    };

    let mut dst = AudioFrame::alloc_interleaved(dst_fmt, src.sample_rate, src.layout, src.samples);
    dst.pts = src.pts;
    if bytes.len() != dst.data_len() {
        return Err(Error::invalid_data("converted buffer size mismatch"));
    }
    dst.data_mut().copy_from_slice(&bytes);
    Ok(dst)
}

fn try_direct(src: &AudioFrame, dst_fmt: SampleFormat) -> Result<Option<Vec<u8>>> {
    Ok(Some(match (src.format, dst_fmt) {
        (SampleFormat::S16, SampleFormat::F32) => f32_bytes(
            &as_i16(src.data())?
                .iter()
                .map(|&x| x as f32 / 32768.0)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::F32, SampleFormat::S16) => i16_bytes(
            &as_f32(src.data())?
                .iter()
                .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::U8, SampleFormat::S16) => i16_bytes(
            &src.data()
                .iter()
                .map(|&x| (i16::from(x) - 128) << 8)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::S16, SampleFormat::U8) => as_i16(src.data())?
            .iter()
            .map(|&x| ((x >> 8) + 128).clamp(0, 255) as u8)
            .collect(),
        (SampleFormat::S16, SampleFormat::S32) => i32_bytes(
            &as_i16(src.data())?
                .iter()
                .map(|&x| i32::from(x) << 16)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::S32, SampleFormat::S16) => i16_bytes(
            &as_i32(src.data())?
                .iter()
                .map(|&x| (x >> 16) as i16)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::F32, SampleFormat::S32) => i32_bytes(
            &as_f32(src.data())?
                .iter()
                .map(|&x| (x.clamp(-1.0, 1.0) * 2147483647.0).round() as i32)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::S32, SampleFormat::F32) => f32_bytes(
            &as_i32(src.data())?
                .iter()
                .map(|&x| x as f32 / 2147483648.0)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::U8, SampleFormat::F32) => f32_bytes(
            &src.data()
                .iter()
                .map(|&x| (x as f32 - 128.0) / 128.0)
                .collect::<Vec<_>>(),
        ),
        (SampleFormat::F32, SampleFormat::U8) => as_f32(src.data())?
            .iter()
            .map(|&x| {
                ((x.clamp(-1.0, 1.0) * 128.0) + 128.0)
                    .round()
                    .clamp(0.0, 255.0) as u8
            })
            .collect(),
        _ => return Ok(None),
    }))
}

fn as_i16(data: &[u8]) -> Result<Vec<i16>> {
    if data.len() % 2 != 0 {
        return Err(Error::invalid_data("odd s16 buffer"));
    }
    Ok(data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

fn as_i32(data: &[u8]) -> Result<Vec<i32>> {
    if data.len() % 4 != 0 {
        return Err(Error::invalid_data("bad s32 buffer"));
    }
    Ok(data
        .chunks_exact(4)
        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

pub(crate) fn as_f32(data: &[u8]) -> Result<Vec<f32>> {
    if data.len() % 4 != 0 {
        return Err(Error::invalid_data("bad f32 buffer"));
    }
    Ok(data
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

pub(crate) fn i16_bytes(v: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn i32_bytes(v: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub(crate) fn f32_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumpeg_util::ChannelLayout;

    #[test]
    fn s16_f32_roundtrip() {
        let mut src =
            AudioFrame::alloc_interleaved(SampleFormat::S16, 48000, ChannelLayout::MONO, 4);
        let mut bytes = Vec::new();
        for v in [0i16, 16384, -16384, 32767] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        src.data_mut().copy_from_slice(&bytes);
        let f = convert_samples(&src, SampleFormat::F32).unwrap();
        let back = convert_samples(&f, SampleFormat::S16).unwrap();
        for (a, b) in as_i16(src.data())
            .unwrap()
            .iter()
            .zip(as_i16(back.data()).unwrap())
        {
            assert!((*a as i32 - i32::from(b)).abs() <= 1);
        }
    }
}
