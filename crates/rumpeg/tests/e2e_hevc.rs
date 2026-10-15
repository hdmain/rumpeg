//! HEVC software encode, MP4/raw framing, and decoder interoperability.
use rumpeg::format::{
    hevcraw::{HevcRawDemuxer, HevcRawMuxer},
    mp4::{Mp4Demuxer, Mp4Muxer},
    Demuxer, Muxer,
};
use rumpeg::prelude::*;
use rumpeg::PacketFlags;
use std::io::Cursor;

fn encode(bitrate: u64) -> (CodecParams, Vec<Packet>) {
    let mut params = CodecParams::video_codec(
        CodecId::Hevc,
        VideoParams {
            pix_fmt: PixelFormat::Yuv420p,
            width: 30,
            height: 18,
            frame_rate: Rational::new(25, 1),
            sample_aspect_ratio: Rational::one(),
        },
    );
    params.gop_size = 3;
    params.quality = 24;
    params.bit_rate = bitrate;
    let mut enc = codec::open_encoder(&params).unwrap();
    params = enc.params().clone();
    assert_eq!(
        rumpeg::util::hevc::HevcConfig::parse(&params.extradata)
            .unwrap()
            .length_size,
        4
    );
    let mut packets = Vec::new();
    for i in 0..4 {
        let mut f = VideoFrame::alloc(PixelFormat::Yuv420p, 30, 18);
        f.plane_mut(0).unwrap().fill(70 + i as u8 * 10);
        f.plane_mut(1).unwrap().fill(110);
        f.plane_mut(2).unwrap().fill(140);
        f.pts = Timestamp::new(i);
        packets.extend(enc.encode(&Frame::Video(f)).unwrap());
    }
    enc.send_eof().unwrap();
    assert!(matches!(enc.receive_packet(), Err(Error::Eof)));
    assert_eq!(packets.len(), 4);
    assert!(packets[0].flags.contains(PacketFlags::KEY));
    assert!(!packets[1].flags.contains(PacketFlags::KEY));
    assert!(packets[3].flags.contains(PacketFlags::KEY));
    (params, packets)
}

fn decode(mut demux: impl Demuxer, io: &mut Cursor<Vec<u8>>) {
    let stream = demux.streams()[0].clone();
    assert_eq!(stream.codec_params.codec_id, CodecId::Hevc);
    let mut dec = codec::open_decoder(&stream.codec_params).unwrap();
    let mut frames = Vec::new();
    while let Ok(p) = demux.read_packet(io) {
        frames.extend(dec.decode(&p).unwrap());
    }
    dec.send_eof().unwrap();
    while let Ok(f) = dec.receive_frame() {
        frames.push(f);
    }
    assert_eq!(frames.len(), 4);
    for (i, f) in frames.iter().enumerate() {
        let Frame::Video(v) = f else {
            panic!("video expected")
        };
        assert_eq!((v.width, v.height), (30, 18));
        assert_eq!(v.pts, Timestamp::new(i as i64));
        assert!((i32::from(v.plane(0).unwrap()[0]) - (70 + i as i32 * 10)).abs() < 15);
    }
}

#[test]
fn hevc_mp4_encode_demux_decode() {
    let (params, packets) = encode(0);
    let mut mux = Mp4Muxer::new();
    mux.add_stream(params).unwrap();
    let mut io = Cursor::new(Vec::new());
    mux.write_header(&mut io).unwrap();
    for p in &packets {
        mux.write_packet(&mut io, p).unwrap();
    }
    mux.write_trailer(&mut io).unwrap();
    let demux = Mp4Demuxer::open(&mut io).unwrap();
    decode(demux, &mut io);
    // Both HEVC sample entry types use hvcC and length-prefixed samples.
    let mut bytes = io.into_inner();
    let pos = bytes.windows(4).position(|s| s == b"hev1").unwrap();
    bytes[pos..pos + 4].copy_from_slice(b"hvc1");
    let mut io = Cursor::new(bytes);
    let demux = Mp4Demuxer::open(&mut io).unwrap();
    decode(demux, &mut io);
}

#[test]
fn hevc_annexb_abr_encode_demux_decode() {
    let (params, packets) = encode(100_000);
    let mut mux = HevcRawMuxer::default();
    mux.add_stream(params).unwrap();
    let mut io = Cursor::new(Vec::new());
    mux.write_header(&mut io).unwrap();
    for p in &packets {
        mux.write_packet(&mut io, p).unwrap();
    }
    mux.write_trailer(&mut io).unwrap();
    let mut media = format::MediaIo::from_bytes(io.get_ref().clone());
    assert_eq!(format::probe(&mut media).unwrap().format_name, "hevc");
    let demux = HevcRawDemuxer::open(&mut io).unwrap();
    decode(demux, &mut io);
}
