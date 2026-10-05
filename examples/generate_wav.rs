//! Generate a sine-wave WAV using the Rumpeg muxer/encoder APIs.

use rumpeg::prelude::*;
use std::env;
use std::f32::consts::PI;

fn main() -> Result<()> {
    rumpeg::init();
    let path = env::args().nth(1).unwrap_or_else(|| "sine.wav".into());

    let sample_rate = 48_000u32;
    let duration_secs = 1.0f32;
    let freq = 440.0f32;
    let total_samples = (sample_rate as f32 * duration_secs) as u32;

    let params = CodecParams::audio_codec(
        CodecId::PcmS16Le,
        AudioParams {
            sample_fmt: SampleFormat::S16,
            sample_rate,
            layout: ChannelLayout::STEREO,
            frame_size: 0,
        },
    );

    let mut encoder = codec::open_encoder(&params)?;
    let mut output = format::open_output(&path, Some("wav"))?;
    let stream_index = output.add_stream(params)?;
    output.write_header()?;

    const CHUNK: u32 = 1024;
    let mut written = 0u32;
    while written < total_samples {
        let n = (total_samples - written).min(CHUNK);
        let mut frame =
            AudioFrame::alloc_interleaved(SampleFormat::S16, sample_rate, ChannelLayout::STEREO, n);
        frame.pts = Timestamp::new(i64::from(written));

        let data = frame.data_mut();
        for i in 0..n as usize {
            let t = (written as usize + i) as f32 / sample_rate as f32;
            let sample = (0.3 * (2.0 * PI * freq * t).sin() * 32767.0) as i16;
            let bytes = sample.to_le_bytes();
            let off = i * 4;
            data[off..off + 2].copy_from_slice(&bytes);
            data[off + 2..off + 4].copy_from_slice(&bytes);
        }

        for mut pkt in encoder.encode(&Frame::Audio(frame))? {
            pkt.stream_index = stream_index;
            output.write_packet(&pkt)?;
        }
        written += n;
    }

    encoder.send_eof()?;
    loop {
        match encoder.receive_packet() {
            Ok(mut pkt) => {
                pkt.stream_index = stream_index;
                output.write_packet(&pkt)?;
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }
    output.write_trailer()?;
    println!("wrote {path} ({total_samples} samples @ {sample_rate} Hz)");
    Ok(())
}
