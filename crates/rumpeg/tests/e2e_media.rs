//! Image decode → encode and FLAC roundtrip smoke tests.

use rumpeg::prelude::*;

#[test]
fn jpeg_image2_roundtrip_scale() {
    rumpeg::init();
    let dir = std::env::temp_dir();
    let src = dir.join("rumpeg_img_src.jpg");
    let dst = dir.join("rumpeg_img_out.png");

    // Build a tiny JPEG via the encoder path.
    let mut frame = VideoFrame::alloc(PixelFormat::Rgb24, 8, 8);
    {
        let p = frame.plane_mut(0).unwrap();
        for (i, b) in p.iter_mut().enumerate() {
            *b = (i % 200) as u8;
        }
    }
    let params = CodecParams::video_codec(
        CodecId::Mjpeg,
        VideoParams {
            pix_fmt: PixelFormat::Rgb24,
            width: 8,
            height: 8,
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
        },
    );
    let mut enc = codec::open_encoder(&params).unwrap();
    let pkts = enc.encode(&Frame::Video(frame)).unwrap();
    assert!(!pkts.is_empty());
    std::fs::write(&src, pkts[0].data.as_slice()).unwrap();

    // Demux image2 → decode JPEG → scale → encode PNG.
    let mut demux = format::open_input(&src).unwrap();
    assert_eq!(demux.format_name(), "image2");
    let stream = demux.best_stream(MediaType::Video).unwrap().clone();
    let mut dec = codec::open_decoder(&stream.codec_params).unwrap();
    let pkt = demux.read_packet().unwrap();
    let frames = dec.decode(&pkt).unwrap();
    assert_eq!(frames.len(), 1);

    let mut graph = Graph::parse("scale=4:4").unwrap();
    let scaled = graph.run(frames.into_iter().next().unwrap()).unwrap();

    let out_params = CodecParams::video_codec(
        CodecId::Png,
        VideoParams {
            pix_fmt: PixelFormat::Rgb24,
            width: 4,
            height: 4,
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
        },
    );
    let mut enc = codec::open_encoder(&out_params).unwrap();
    let mut mux = format::open_output(&dst, None).unwrap();
    mux.add_stream(out_params).unwrap();
    mux.write_header().unwrap();
    for mut p in enc.encode(&scaled).unwrap() {
        p.stream_index = 0;
        mux.write_packet(&p).unwrap();
    }
    enc.send_eof().unwrap();
    while let Ok(mut p) = enc.receive_packet() {
        p.stream_index = 0;
        mux.write_packet(&p).unwrap();
    }
    mux.write_trailer().unwrap();

    let bytes = std::fs::read(&dst).unwrap();
    assert!(bytes.len() > 8);
    assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn flac_encode_decode_via_codec_api() {
    rumpeg::init();
    let params = CodecParams::audio_codec(
        CodecId::Flac,
        AudioParams {
            sample_fmt: SampleFormat::S16,
            sample_rate: 8_000,
            layout: ChannelLayout::MONO,
            frame_size: 0,
        },
    );
    let mut enc = codec::open_encoder(&params).unwrap();
    let mut frame =
        AudioFrame::alloc_interleaved(SampleFormat::S16, 8_000, ChannelLayout::MONO, 4096);
    for (i, c) in frame.data_mut().chunks_exact_mut(2).enumerate() {
        let s = ((i as f32 * 0.05).sin() * 2000.0) as i16;
        c.copy_from_slice(&s.to_le_bytes());
    }
    enc.send_frame(&Frame::Audio(frame)).unwrap();
    enc.send_eof().unwrap();
    let pkt = enc.receive_packet().unwrap();
    assert_eq!(&pkt.data.as_slice()[0..4], b"fLaC");

    let mut dec = codec::open_decoder(&params).unwrap();
    let frames = dec.decode(&pkt).unwrap();
    assert!(!frames.is_empty());
}
