//! FPS convert + bitrate/QP re-encode smoke tests.

use rumpeg::prelude::*;
use rumpeg::PacketFlags;

fn map_h264(e: rumpeg::h264::Error) -> rumpeg::Error {
    match e {
        rumpeg::h264::Error::Truncated(m) | rumpeg::h264::Error::Invalid(m) => {
            rumpeg::Error::invalid_data(m)
        }
        rumpeg::h264::Error::Unsupported(m) => rumpeg::Error::unsupported(m),
    }
}

fn make_yuv(w: u32, h: u32, seed: u8) -> rumpeg::h264::Yuv420Planar {
    let mut yuv = rumpeg::h264::Yuv420Planar::zeroed(w, h);
    for y in 0..h {
        for x in 0..w {
            yuv.y[(y * w + x) as usize] = seed.wrapping_add((x + y) as u8);
        }
    }
    for i in 0..yuv.u.len() {
        yuv.u[i] = 128;
        yuv.v[i] = 128;
    }
    yuv
}

/// Encode `n` frames at `fps` into an MP4 using I_PCM (decodable by rusty_h264).
fn write_mp4_clip(
    path: &std::path::Path,
    n: usize,
    fps: i32,
    _qp: i32,
) -> rumpeg::Result<(u32, u32)> {
    let w = 32u32;
    let h = 32u32;
    let mut enc = rumpeg::h264::Encoder::new(w, h).map_err(map_h264)?;

    let mut params = CodecParams::video_codec(
        CodecId::H264,
        VideoParams {
            pix_fmt: PixelFormat::Yuv420p,
            width: w,
            height: h,
            frame_rate: Rational::new(fps, 1),
            sample_aspect_ratio: Rational::one(),
        },
    );
    params.extradata = enc.avcc_extradata().map_err(map_h264)?;

    let mut muxer = format::open_output(path, Some("mp4"))?;
    let idx = muxer.add_stream(params)?;
    muxer.write_header()?;

    for i in 0..n {
        let yuv = make_yuv(w, h, (i * 17) as u8);
        let (bytes, is_key) = enc.encode_access_unit(&yuv).map_err(map_h264)?;
        let mut pkt = Packet::new(bytes);
        pkt.stream_index = idx;
        if is_key {
            pkt.flags.insert(PacketFlags::KEY);
        }
        muxer.write_packet(&pkt)?;
    }
    muxer.write_trailer()?;
    Ok((w, h))
}

fn reencode(
    input: &std::path::Path,
    output: &std::path::Path,
    vf: Option<&str>,
    quality: i32,
    bit_rate: u64,
    gop: u32,
    target_fps: Option<u32>,
) -> rumpeg::Result<u64> {
    let mut demuxer = format::open_input(input)?;
    let in_stream = demuxer
        .best_stream(MediaType::Video)
        .ok_or_else(|| Error::not_found("no video"))?
        .clone();
    let mut decoder = codec::open_decoder(&in_stream.codec_params)?;

    let source_fps = in_stream
        .codec_params
        .video()
        .map(|v| v.frame_rate.as_f64().round().max(1.0) as u32)
        .unwrap_or(25);

    let mut video = in_stream.codec_params.video().cloned().unwrap_or_default();
    video.pix_fmt = PixelFormat::Yuv420p;
    if let Some(fps) = target_fps {
        video.frame_rate = Rational::new(fps as i32, 1);
    }

    let mut out_params = CodecParams::video_codec(CodecId::H264, video);
    out_params.quality = quality;
    out_params.bit_rate = bit_rate;
    out_params.gop_size = gop;
    {
        let v = out_params.video().unwrap();
        let enc = rumpeg::h264::Encoder::with_mode(
            v.width.max(16),
            v.height.max(16),
            rumpeg::h264::IntraMode::I16x16Cavlc,
        )
        .map_err(map_h264)?;
        out_params.extradata = enc.avcc_extradata().map_err(map_h264)?;
    }

    let mut graph = vf.map(Graph::parse).transpose()?.unwrap_or_default();
    graph.set_fps_source(source_fps);
    if let Some(t) = graph.target_fps() {
        if let Some(v) = out_params.video_mut() {
            v.frame_rate = Rational::new(t as i32, 1);
        }
    }

    let mut encoder = codec::open_encoder(&out_params)?;
    let mut muxer = format::open_output(output, Some("mp4"))?;
    let out_index = muxer.add_stream(out_params)?;
    muxer.write_header()?;

    let mut encoded = 0u64;
    loop {
        let packet = match demuxer.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => return Err(e),
        };
        if packet.stream_index != in_stream.index {
            continue;
        }
        for frame in decoder.decode(&packet)? {
            for frame in graph.run_all(frame)? {
                for mut pkt in encoder.encode(&frame)? {
                    pkt.stream_index = out_index;
                    muxer.write_packet(&pkt)?;
                }
                encoded += 1;
            }
        }
    }
    decoder.send_eof()?;
    loop {
        match decoder.receive_frame() {
            Ok(frame) => {
                for frame in graph.run_all(frame)? {
                    for mut pkt in encoder.encode(&frame)? {
                        pkt.stream_index = out_index;
                        muxer.write_packet(&pkt)?;
                    }
                    encoded += 1;
                }
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }
    encoder.send_eof()?;
    loop {
        match encoder.receive_packet() {
            Ok(mut pkt) => {
                pkt.stream_index = out_index;
                muxer.write_packet(&pkt)?;
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }
    muxer.write_trailer()?;
    Ok(encoded)
}

#[test]
fn fps_downconvert_reduces_sample_count_and_duration() {
    rumpeg::init();
    let dir = std::env::temp_dir();
    let src = dir.join("rumpeg_fps_src.mp4");
    let dst = dir.join("rumpeg_fps_dst.mp4");

    // 30 frames @ 30 fps → 1.0s; down-convert to 15 fps → ~15 frames, ~1.0s.
    write_mp4_clip(&src, 30, 30, 28).expect("src");

    let n = reencode(&src, &dst, Some("fps=15"), 28, 0, 8, Some(15)).expect("reencode");
    assert!(
        (14..=16).contains(&n),
        "expected ~15 frames after 30→15 fps, got {n}"
    );

    let demux = format::open_input(&dst).expect("open out");
    let stream = demux.best_stream(MediaType::Video).expect("v");
    let fps = stream.codec_params.video().unwrap().frame_rate.as_f64();
    assert!(
        (fps - 15.0).abs() < 0.6,
        "reported fps should be ~15, got {fps}"
    );
    assert_eq!(stream.nb_frames, Some(n));

    // Duration ≈ n / fps seconds in stream time_base (timescale = fps num = 15, delta = 1).
    if let Some(dur) = stream.duration {
        let seconds = dur as f64 * stream.time_base.as_f64();
        assert!(
            (seconds - 1.0).abs() < 0.15,
            "duration should be ~1s, got {seconds}s (dur={dur}, tb={:?})",
            stream.time_base
        );
    }
}

#[test]
fn high_qp_or_low_bitrate_smaller_than_high_quality() {
    rumpeg::init();
    let dir = std::env::temp_dir();
    let src = dir.join("rumpeg_br_src.mp4");
    let hi = dir.join("rumpeg_br_hi.mp4");
    let lo = dir.join("rumpeg_br_lo.mp4");

    write_mp4_clip(&src, 16, 25, 18).expect("src");

    reencode(&src, &hi, None, 12, 0, 8, None).expect("hi");
    reencode(&src, &lo, None, 40, 0, 8, None).expect("lo");

    let hi_len = std::fs::metadata(&hi).unwrap().len();
    let lo_len = std::fs::metadata(&lo).unwrap().len();
    assert!(
        lo_len < hi_len,
        "QP40 encode should be smaller than QP12 ({lo_len} vs {hi_len})"
    );

    // Bitrate heuristic path should also shrink vs a low-QP encode.
    let br = dir.join("rumpeg_br_bitrate.mp4");
    reencode(&src, &br, None, -1, 50_000, 8, None).expect("bitrate");
    let br_len = std::fs::metadata(&br).unwrap().len();
    assert!(
        br_len < hi_len,
        "low bitrate encode should be smaller than QP12 ({br_len} vs {hi_len})"
    );
}

#[test]
fn fps_filter_drops_with_real_source_rate() {
    rumpeg::init();
    let mut graph = Graph::parse("fps=10").unwrap();
    graph.set_fps_source(30);
    assert_eq!(graph.target_fps(), Some(10));

    let mut emitted = 0usize;
    for i in 0..30 {
        let mut frame = VideoFrame::alloc(PixelFormat::Yuv420p, 16, 16);
        frame.pts = Timestamp::new(i as i64);
        let out = graph.run_all(Frame::Video(frame)).unwrap();
        emitted += out.len();
    }
    assert!(
        (9..=11).contains(&emitted),
        "30 @30fps → 10fps should emit ~10 frames, got {emitted}"
    );
}
