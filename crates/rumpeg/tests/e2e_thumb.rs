//! Video → JPEG thumbnail pipeline using pure-Rust H.264.

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

fn write_demo_h264(path: &std::path::Path) -> rumpeg::Result<(u32, u32)> {
    let w = 32u32;
    let h = 32u32;
    let mut yuv = rumpeg::h264::Yuv420Planar::zeroed(w, h);
    for y in 0..h {
        for x in 0..w {
            yuv.y[(y * w + x) as usize] = if (x / 8 + y / 8) % 2 == 0 { 40 } else { 200 };
        }
    }
    for i in 0..yuv.u.len() {
        yuv.u[i] = 100;
        yuv.v[i] = 160;
    }
    let enc = rumpeg::h264::Encoder::new(w, h).map_err(map_h264)?;
    std::fs::write(path, enc.encode_annexb(&yuv).map_err(map_h264)?)?;
    Ok((w, h))
}

#[test]
fn extract_scaled_jpeg_from_h264() {
    rumpeg::init();
    let dir = std::env::temp_dir();
    let h264 = dir.join("rumpeg_thumb_src.h264");
    let jpg = dir.join("rumpeg_thumb_16.jpg");
    write_demo_h264(&h264).expect("encode h264");

    rumpeg::extract_first_frame_jpeg(&h264, &jpg, Some((16, 16))).expect("extract");

    let bytes = std::fs::read(&jpg).expect("read jpg");
    assert!(bytes.len() > 20);
    assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "SOI marker");
    assert_eq!(bytes[bytes.len() - 2..], [0xFF, 0xD9], "EOI marker");
}

#[test]
fn mp4_roundtrip_then_jpeg() {
    rumpeg::init();
    let dir = std::env::temp_dir();
    let h264 = dir.join("rumpeg_mp4_src.h264");
    let mp4 = dir.join("rumpeg_mp4_src.mp4");
    let jpg = dir.join("rumpeg_mp4_thumb.jpg");
    let (w, h) = write_demo_h264(&h264).expect("h264");

    // Remux Annex-B → MP4 via library APIs.
    let enc = rumpeg::h264::Encoder::new(w, h).unwrap();
    let mut params = CodecParams::video_codec(
        CodecId::H264,
        VideoParams {
            pix_fmt: PixelFormat::Yuv420p,
            width: w,
            height: h,
            frame_rate: Rational::new(25, 1),
            sample_aspect_ratio: Rational::one(),
        },
    );
    params.extradata = enc.avcc_extradata().unwrap();

    let annexb = std::fs::read(&h264).unwrap();
    // Build an AVCC sample containing only the IDR NAL.
    let nals = rumpeg::h264::extract_annexb_nals(&annexb);
    let idr = nals
        .iter()
        .find(|n| rumpeg::h264::nal_unit_type(n).unwrap() == rumpeg::h264::NAL_IDR)
        .expect("idr");
    let mut sample = Vec::new();
    sample.extend_from_slice(&(idr.len() as u32).to_be_bytes());
    sample.extend_from_slice(idr);

    let mut muxer = format::open_output(&mp4, Some("mp4")).unwrap();
    let idx = muxer.add_stream(params).unwrap();
    muxer.write_header().unwrap();
    let mut pkt = Packet::new(sample);
    pkt.stream_index = idx;
    pkt.flags.insert(PacketFlags::KEY);
    muxer.write_packet(&pkt).unwrap();
    muxer.write_trailer().unwrap();

    rumpeg::extract_first_frame_jpeg(&mp4, &jpg, Some((16, 16))).expect("mp4 extract");
    let bytes = std::fs::read(&jpg).unwrap();
    assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
}
