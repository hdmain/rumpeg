//! High-level helpers for common pipelines.

use rumpeg_util::{
    CodecId, CodecParams, Error, Frame, MediaType, PixelFormat, Result, VideoParams,
};

/// Extract the first video frame from `input`, optionally scale, and write a JPEG to `output`.
///
/// `scale_to` is `(width, height)` for the output JPEG, or `None` to keep the source size.
pub fn extract_first_frame_jpeg(
    input: impl AsRef<std::path::Path>,
    output: impl AsRef<std::path::Path>,
    scale_to: Option<(u32, u32)>,
) -> Result<()> {
    crate::init();
    let mut demuxer = crate::format::open_input(input)?;
    let stream = demuxer
        .best_stream(MediaType::Video)
        .ok_or_else(|| Error::not_found("no video stream"))?
        .clone();
    let mut decoder = crate::codec::open_decoder(&stream.codec_params)?;

    let mut video_frame = None;
    loop {
        let packet = match demuxer.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => return Err(e),
        };
        if packet.stream_index != stream.index {
            continue;
        }
        for frame in decoder.decode(&packet)? {
            if let Frame::Video(v) = frame {
                video_frame = Some(v);
                break;
            }
        }
        if video_frame.is_some() {
            break;
        }
    }
    decoder.send_eof()?;
    if video_frame.is_none() {
        loop {
            match decoder.receive_frame() {
                Ok(Frame::Video(v)) => {
                    video_frame = Some(v);
                    break;
                }
                Ok(_) => continue,
                Err(Error::Eof) | Err(Error::NeedMoreData) => break,
                Err(e) => return Err(e),
            }
        }
    }
    let mut frame = video_frame.ok_or_else(|| Error::not_found("no video frame decoded"))?;

    if let Some((w, h)) = scale_to {
        frame = crate::scale::transform(
            &frame,
            PixelFormat::Rgb24,
            w,
            h,
            crate::scale::FilterMode::Bilinear,
        )?;
    }

    let out_params = CodecParams::video_codec(
        CodecId::Mjpeg,
        VideoParams {
            pix_fmt: PixelFormat::Rgb24,
            width: frame.width,
            height: frame.height,
            frame_rate: Default::default(),
            sample_aspect_ratio: Default::default(),
        },
    );
    let mut encoder = crate::codec::open_encoder(&out_params)?;
    let mut muxer = crate::format::open_output(output, Some("image2"))?;
    let idx = muxer.add_stream(out_params)?;
    muxer.write_header()?;
    for mut pkt in encoder.encode(&Frame::Video(frame))? {
        pkt.stream_index = idx;
        muxer.write_packet(&pkt)?;
    }
    encoder.send_eof()?;
    while let Ok(mut pkt) = encoder.receive_packet() {
        pkt.stream_index = idx;
        muxer.write_packet(&pkt)?;
    }
    muxer.write_trailer()?;
    Ok(())
}
