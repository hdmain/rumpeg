//! Exercise the shipped CLI through HEVC encode, remux, raw decode and overlays.
use rumpeg::prelude::*;
use std::process::Command;

#[test]
fn cli_hevc_pipeline() {
    let dir = std::env::temp_dir().join(format!("rumpeg-hevc-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.png");
    let mp4 = dir.join("encoded.mp4");
    let raw = dir.join("encoded.hevc");
    let thumb = dir.join("thumbnail.png");
    let reencoded = dir.join("reencoded.mp4");
    let params = CodecParams::video_codec(
        CodecId::Png,
        VideoParams {
            pix_fmt: PixelFormat::Rgb24,
            width: 30,
            height: 18,
            frame_rate: Rational::new(25, 1),
            sample_aspect_ratio: Rational::one(),
        },
    );
    let mut encoder = codec::open_encoder(&params).unwrap();
    let mut frame = VideoFrame::alloc(PixelFormat::Rgb24, 30, 18);
    frame.plane_mut(0).unwrap().fill(110);
    let p = encoder.encode(&Frame::Video(frame)).unwrap();
    std::fs::write(&input, p[0].data.as_slice()).unwrap();
    let run = |src: &std::path::Path, dst: &std::path::Path, args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_rumpeg"))
            .args(["convert", "-i"])
            .arg(src)
            .arg("-o")
            .arg(dst)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(
        &input,
        &mp4,
        &[
            "-c:v",
            "hevc",
            "--qp",
            "24",
            "--overlay",
            "kind=rect;x=0;y=0;width=4;height=4;color=#ffffff",
        ],
    );
    run(&mp4, &raw, &["-c", "copy"]);
    run(&raw, &thumb, &["--frames", "1"]);
    run(&raw, &reencoded, &["-c:v", "h265", "-b:v", "100k"]);
    for output in [&mp4, &reencoded] {
        let mut demux = format::open_input(output).unwrap();
        assert_eq!(demux.streams()[0].codec_params.codec_id, CodecId::Hevc);
        let mut decoder = codec::open_decoder(&demux.streams()[0].codec_params).unwrap();
        let packet = demux.read_packet().unwrap();
        decoder.send_packet(&packet).unwrap();
        decoder.send_eof().unwrap();
        let Frame::Video(v) = decoder.receive_frame().unwrap() else {
            panic!("expected video")
        };
        assert_eq!((v.width, v.height), (30, 18));
    }
    assert!(std::fs::metadata(thumb).unwrap().len() > 0);
    for path in [input, mp4, raw, reencoded, dir.join("thumbnail.png")] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(dir).unwrap();
}
