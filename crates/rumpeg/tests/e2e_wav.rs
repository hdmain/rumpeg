//! End-to-end WAV generate → demux → decode.

use rumpeg::prelude::*;
use std::f32::consts::PI;

fn write_sine(path: &std::path::Path) -> Result<()> {
    let sample_rate = 8_000u32;
    let total = 800u32;
    let params = CodecParams::audio_codec(
        CodecId::PcmS16Le,
        AudioParams {
            sample_fmt: SampleFormat::S16,
            sample_rate,
            layout: ChannelLayout::MONO,
            frame_size: 0,
        },
    );

    let mut encoder = codec::open_encoder(&params)?;
    let mut output = format::open_output(path, Some("wav"))?;
    let idx = output.add_stream(params)?;
    output.write_header()?;

    let mut frame =
        AudioFrame::alloc_interleaved(SampleFormat::S16, sample_rate, ChannelLayout::MONO, total);
    frame.pts = Timestamp::new(0);
    for i in 0..total as usize {
        let t = i as f32 / sample_rate as f32;
        let s = (0.2 * (2.0 * PI * 440.0 * t).sin() * 32767.0) as i16;
        let off = i * 2;
        frame.data_mut()[off..off + 2].copy_from_slice(&s.to_le_bytes());
    }
    for mut pkt in encoder.encode(&Frame::Audio(frame))? {
        pkt.stream_index = idx;
        output.write_packet(&pkt)?;
    }
    encoder.send_eof()?;
    loop {
        match encoder.receive_packet() {
            Ok(mut pkt) => {
                pkt.stream_index = idx;
                output.write_packet(&pkt)?;
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }
    output.write_trailer()?;
    Ok(())
}

#[test]
fn wav_roundtrip_sample_count() {
    rumpeg::init();
    let path = std::env::temp_dir().join("rumpeg_e2e_sine.wav");
    write_sine(&path).expect("generate");

    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 44);
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");

    let mut input = format::open_input(&path).expect("open");
    let stream = input.best_stream(MediaType::Audio).unwrap().clone();
    let mut decoder = codec::open_decoder(&stream.codec_params).unwrap();
    let mut samples = 0u32;
    loop {
        let pkt = match input.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => panic!("{e}"),
        };
        for frame in decoder.decode(&pkt).unwrap() {
            if let Frame::Audio(a) = frame {
                samples += a.samples;
            }
        }
    }
    assert_eq!(samples, 800);
}

#[test]
fn volume_filter_halves_peak() {
    rumpeg::init();
    let mut frame = AudioFrame::alloc_interleaved(SampleFormat::S16, 48000, ChannelLayout::MONO, 1);
    frame.data_mut().copy_from_slice(&1000i16.to_le_bytes());
    let out = filter::process(Frame::Audio(frame), "volume=0.5").unwrap();
    match out {
        Frame::Audio(a) => {
            let v = i16::from_le_bytes([a.data()[0], a.data()[1]]);
            assert_eq!(v, 500);
        }
        _ => panic!("expected audio"),
    }
}
