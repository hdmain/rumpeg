//! Encode a tiny H.264 clip, then extract a scaled JPEG thumbnail.

use rumpeg::prelude::*;
use std::env;
use std::path::PathBuf;

fn map_h264(e: rumpeg::h264::Error) -> Error {
    match e {
        rumpeg::h264::Error::Truncated(m) | rumpeg::h264::Error::Invalid(m) => {
            Error::invalid_data(m)
        }
        rumpeg::h264::Error::Unsupported(m) => Error::unsupported(m),
    }
}

fn main() -> Result<()> {
    rumpeg::init();
    let dir = env::temp_dir();
    let h264_path = dir.join("rumpeg_demo.h264");
    let jpg_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("thumb.jpg"));

    let w = 64u32;
    let h = 48u32;
    let mut yuv = rumpeg::h264::Yuv420Planar::zeroed(w, h);
    for y in 0..h {
        for x in 0..w {
            yuv.y[(y * w + x) as usize] = ((x * 3 + y * 5) % 200) as u8 + 16;
        }
    }
    for i in 0..yuv.u.len() {
        yuv.u[i] = 128;
        yuv.v[i] = 128;
    }
    let enc = rumpeg::h264::Encoder::new(w, h).map_err(map_h264)?;
    std::fs::write(&h264_path, enc.encode_annexb(&yuv).map_err(map_h264)?)?;

    rumpeg::extract_first_frame_jpeg(&h264_path, &jpg_path, Some((16, 16)))?;
    println!(
        "wrote {} (16x16 JPEG from {}x{} H.264)",
        jpg_path.display(),
        w,
        h
    );
    Ok(())
}
