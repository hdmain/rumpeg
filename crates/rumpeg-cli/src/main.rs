//! Rumpeg CLI — ffmpeg/ffprobe-inspired front-end over the Rumpeg library.

use clap::{Parser, Subcommand};
use rumpeg::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "rumpeg",
    version,
    about = "High-performance multimedia toolkit (Rust)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Show information about a media file (ffprobe-like).
    Probe {
        /// Input file path.
        input: PathBuf,
    },
    /// Transcode / remux / extract frames.
    Convert {
        /// Input file.
        #[arg(short, long)]
        input: PathBuf,
        /// Output file.
        #[arg(short, long)]
        output: PathBuf,
        /// Optional audio filterchain (e.g. `volume=0.5`).
        #[arg(short = 'f', long = "filter")]
        filter: Option<String>,
        /// Video filterchain (e.g. `scale=16:16`, `fps=24`).
        #[arg(long = "vf")]
        vf: Option<String>,
        /// Target frame rate (alias for injecting `fps=N` into `--vf`).
        #[arg(long = "fps", short = 'r', value_name = "FPS")]
        fps: Option<u32>,
        /// Stop after encoding this many video frames (e.g. `1` for a thumbnail).
        #[arg(long)]
        frames: Option<u64>,
        /// Target sample rate (audio).
        #[arg(long)]
        ar: Option<u32>,
        /// Target sample format name: u8, s16, s32, f32.
        #[arg(long)]
        sample_fmt: Option<String>,
        /// Stream map (e.g. `0:v:0`, `0:a:0`, `0:0`).
        #[arg(long = "map")]
        map: Option<String>,
        /// Codec name (`copy` remuxes without re-encode when possible).
        #[arg(short = 'c')]
        codec: Option<String>,
        /// Video codec name.
        #[arg(long = "c:v")]
        codec_video: Option<String>,
        /// Audio codec name.
        #[arg(long = "c:a")]
        codec_audio: Option<String>,
        /// Video bitrate (e.g. `500k`, `1500k`, `2000000`). Maps to QP heuristically.
        #[arg(long = "b:v", visible_alias = "bitrate")]
        bit_rate_video: Option<String>,
        /// Audio bitrate (e.g. `128k`). Used when muxing AAC into MP4.
        #[arg(long = "b:a")]
        bit_rate_audio: Option<String>,
        /// CRF-like quality for H.264 (0=best … 51=worst); mapped to QP.
        #[arg(long = "crf")]
        crf: Option<f32>,
        /// Explicit H.264 quantizer (0..=51). Overrides `--crf` / `-b:v` when set.
        #[arg(long = "qp")]
        qp: Option<i32>,
        /// GOP size / IDR interval for H.264 (default 30; `1` = all Intra).
        #[arg(long = "gop")]
        gop: Option<u32>,
        /// Faster re-encode: `scale=640:-2` + `-b:v 800k` (unless already set).
        #[arg(long = "fast")]
        fast: bool,
        /// JPEG/MJPEG quality 1–100 (encoding hint; MJPEG encoder uses default today).
        #[arg(long = "q:v")]
        quality_video: Option<u8>,
    },
    /// List built-in codecs.
    Codecs,
    /// List supported formats.
    Formats,
}

fn main() -> ExitCode {
    rumpeg::init();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rumpeg: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Probe { input } => cmd_probe(&input),
        Commands::Convert {
            input,
            output,
            filter,
            vf,
            fps,
            frames,
            ar,
            sample_fmt,
            map,
            codec,
            codec_video,
            codec_audio,
            bit_rate_video,
            bit_rate_audio,
            crf,
            qp,
            gop,
            fast,
            quality_video,
        } => cmd_convert(
            &input,
            &output,
            filter.as_deref(),
            vf.as_deref(),
            fps,
            frames,
            ar,
            sample_fmt.as_deref(),
            map.as_deref(),
            codec.as_deref(),
            codec_video.as_deref(),
            codec_audio.as_deref(),
            bit_rate_video.as_deref(),
            bit_rate_audio.as_deref(),
            crf,
            qp,
            gop,
            fast,
            quality_video,
        ),
        Commands::Codecs => cmd_codecs(),
        Commands::Formats => {
            println!("Demuxers:");
            println!("  wav");
            println!("  mp4");
            println!("  matroska / webm");
            println!("  mpegts");
            println!("  h264");
            println!("  image2 (jpg/jpeg/png)");
            println!("Muxers:");
            println!("  wav");
            println!("  mp4");
            println!("  image2 (jpg/jpeg/png)");
            Ok(())
        }
    }
}

fn cmd_probe(path: &PathBuf) -> Result<()> {
    let input = format::open_input(path)?;
    println!(
        "Input #0, {}, from '{}':",
        input.format_name(),
        path.display()
    );
    for stream in input.streams() {
        print!("  Stream #0:{}: {}", stream.index, stream.media_type);
        print!(": {}", stream.codec_params.codec_id);
        if let Some(a) = stream.codec_params.audio() {
            print!(
                ", {} Hz, {} channels, {}",
                a.sample_rate,
                a.layout.channels,
                a.sample_fmt.name()
            );
        }
        if let Some(v) = stream.codec_params.video() {
            print!(", {}x{}, {}", v.width, v.height, v.pix_fmt.name());
        }
        if let Some(dur) = stream.duration {
            if let Some(secs) = Timestamp::new(dur).as_secs(stream.time_base) {
                print!(", duration {secs:.3}s");
            }
        }
        if stream.codec_params.bit_rate > 0 {
            print!(", {} b/s", stream.codec_params.bit_rate);
        }
        if let Some(n) = stream.nb_frames {
            print!(", {n} frames");
        }
        if !stream.codec_params.extradata.is_empty() {
            print!(", extradata {} bytes", stream.codec_params.extradata.len());
        }
        print!(
            ", time_base {}/{}",
            stream.time_base.num, stream.time_base.den
        );
        println!();
    }
    Ok(())
}

fn is_still_image_output(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()),
        Some(ref e) if e == "jpg" || e == "jpeg" || e == "png"
    )
}

fn still_image_codec(path: &Path) -> Option<CodecId> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => Some(CodecId::Mjpeg),
        Some("png") => Some(CodecId::Png),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_convert(
    input: &PathBuf,
    output: &PathBuf,
    audio_filter: Option<&str>,
    video_filter: Option<&str>,
    fps: Option<u32>,
    frames: Option<u64>,
    ar: Option<u32>,
    sample_fmt: Option<&str>,
    map: Option<&str>,
    codec: Option<&str>,
    codec_video: Option<&str>,
    codec_audio: Option<&str>,
    bit_rate_video: Option<&str>,
    bit_rate_audio: Option<&str>,
    crf: Option<f32>,
    qp: Option<i32>,
    gop: Option<u32>,
    fast: bool,
    quality_video: Option<u8>,
) -> Result<()> {
    let video_filter = merge_vf_fps(video_filter, fps)?;
    let mut video_filter = video_filter;
    let mut bit_rate_video = bit_rate_video.map(|s| s.to_string());

    if fast {
        if !vf_has_scale(video_filter.as_deref()) {
            video_filter = Some(match video_filter {
                Some(v) => format!("scale=640:-2,{v}"),
                None => "scale=640:-2".to_string(),
            });
        }
        if bit_rate_video.is_none() && crf.is_none() && qp.is_none() {
            bit_rate_video = Some("800k".to_string());
        }
        eprintln!(
            "rumpeg: --fast → scale≤640 + bitrate heuristic (override with --vf / -b:v / --crf)"
        );
    }

    // Auto-speed for large MP4 re-encodes when no quality knobs / scale given.
    if is_mp4_output(output)
        && !is_still_image_output(output)
        && !vf_has_scale(video_filter.as_deref())
        && bit_rate_video.is_none()
        && crf.is_none()
        && qp.is_none()
    {
        if let Ok(probe) = format::open_input(input) {
            if let Some(v) = probe
                .best_stream(MediaType::Video)
                .and_then(|s| s.codec_params.video())
            {
                if v.width > 640 {
                    video_filter = Some(match video_filter {
                        Some(vf) => format!("scale=640:-2,{vf}"),
                        None => "scale=640:-2".to_string(),
                    });
                    bit_rate_video = Some("800k".to_string());
                    eprintln!(
                        "rumpeg: auto scale=640:-2 -b:v 800k for speed (use --vf / -b:v to override)"
                    );
                }
            }
        }
    }

    let video_filter = video_filter.as_deref();
    let bit_rate_video = bit_rate_video.as_deref();

    let reencode_video = fps.is_some()
        || bit_rate_video.is_some()
        || crf.is_some()
        || qp.is_some()
        || gop.is_some()
        || fast;

    if let Some(()) = try_remux_copy(
        input,
        output,
        map,
        codec,
        codec_video,
        codec_audio,
        audio_filter,
        video_filter,
        frames,
        ar,
        sample_fmt,
        reencode_video,
    )? {
        return Ok(());
    }

    let still = is_still_image_output(output);
    let prefer_video = still
        || frames.is_some()
        || video_filter.is_some()
        || reencode_video
        || is_mp4_output(output);
    if prefer_video {
        let frame_limit = frames.or(if still { Some(1) } else { None });
        return try_convert_video(
            input,
            output,
            video_filter,
            frame_limit,
            map,
            codec_video.or(codec),
            bit_rate_video,
            bit_rate_audio,
            crf,
            qp,
            gop,
            quality_video,
        );
    }

    let probe = format::open_input(input)?;
    if probe.best_stream(MediaType::Video).is_some()
        && probe.best_stream(MediaType::Audio).is_none()
    {
        drop(probe);
        return try_convert_video(
            input,
            output,
            video_filter,
            frames,
            map,
            codec_video.or(codec),
            bit_rate_video,
            bit_rate_audio,
            crf,
            qp,
            gop,
            quality_video,
        );
    }
    drop(probe);
    convert_audio(
        input,
        output,
        audio_filter,
        ar,
        sample_fmt,
        map,
        codec_audio.or(codec),
        bit_rate_audio,
    )
}

fn is_mp4_output(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp4") | Some("m4v") | Some("mov")
    )
}

fn vf_has_scale(vf: Option<&str>) -> bool {
    vf.map(|v| {
        v.split(',').any(|p| {
            let name = p.trim().split('=').next().unwrap_or("").trim();
            name == "scale"
        })
    })
    .unwrap_or(false)
}

/// Combine `--vf` with `--fps`/`-r`. Returns owned string when either is set.
fn merge_vf_fps(vf: Option<&str>, fps: Option<u32>) -> Result<Option<String>> {
    match (vf, fps) {
        (None, None) => Ok(None),
        (Some(v), None) => Ok(Some(v.to_string())),
        (None, Some(n)) => {
            if n == 0 {
                return Err(Error::invalid_data("--fps must be > 0"));
            }
            Ok(Some(format!("fps={n}")))
        }
        (Some(v), Some(n)) => {
            if n == 0 {
                return Err(Error::invalid_data("--fps must be > 0"));
            }
            if vf_has_fps(v) {
                Ok(Some(format!("{v},fps={n}")))
            } else {
                Ok(Some(format!("fps={n},{v}")))
            }
        }
    }
}

fn vf_has_fps(vf: &str) -> bool {
    vf.split(',').any(|p| {
        let name = p.trim().split('=').next().unwrap_or("").trim();
        name == "fps"
    })
}

fn parse_scale_from_vf(vf: &str, src_w: u32, src_h: u32) -> Option<(u32, u32)> {
    for part in vf.split(',') {
        let part = part.trim();
        if let Some(args) = part.strip_prefix("scale=") {
            let args = args.replace('x', ":");
            let (w, h) = args.split_once(':')?;
            let tw: i32 = w.parse().ok()?;
            let th: i32 = h.parse().ok()?;
            return Some(rumpeg::filter::ScaleFilter::resolve(src_w, src_h, tw, th));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn try_convert_video(
    input: &PathBuf,
    output: &PathBuf,
    video_filter: Option<&str>,
    frames: Option<u64>,
    map: Option<&str>,
    codec_name: Option<&str>,
    bit_rate_video: Option<&str>,
    bit_rate_audio: Option<&str>,
    crf: Option<f32>,
    qp: Option<i32>,
    gop: Option<u32>,
    quality_video: Option<u8>,
) -> Result<()> {
    let mut demuxer = format::open_input(input)?;
    let in_stream = stream_from_map(&demuxer, map, MediaType::Video)?.clone();
    if in_stream.media_type != MediaType::Video {
        return Err(Error::invalid_data("-map must select a video stream"));
    }
    let audio_in = if map.is_none() && is_mp4_output(output) && still_image_codec(output).is_none()
    {
        demuxer.best_stream(MediaType::Audio).cloned()
    } else {
        None
    };

    let mut decoder = codec::open_decoder(&in_stream.codec_params)?;
    let mut audio_decoder = if let Some(ref a) = audio_in {
        Some(codec::open_decoder(&a.codec_params)?)
    } else {
        None
    };

    let still_codec = still_image_codec(output);
    let max_frames = frames.unwrap_or(if still_codec.is_some() { 1 } else { u64::MAX });

    let source_fps = in_stream
        .codec_params
        .video()
        .map(|v| {
            let f = v.frame_rate.as_f64();
            if f > 0.0 {
                f.round().max(1.0) as u32
            } else {
                25
            }
        })
        .unwrap_or(25);

    let src_w = in_stream
        .codec_params
        .video()
        .map(|v| v.width.max(1))
        .unwrap_or(1);
    let src_h = in_stream
        .codec_params
        .video()
        .map(|v| v.height.max(1))
        .unwrap_or(1);

    let mut out_params = if let Some(codec_id) = still_codec {
        CodecParams::video_codec(
            codec_id,
            VideoParams {
                pix_fmt: PixelFormat::Rgb24,
                width: src_w,
                height: src_h,
                frame_rate: Default::default(),
                sample_aspect_ratio: Default::default(),
            },
        )
    } else if let Some(name) = codec_name.filter(|c| !is_copy_codec(c)) {
        let video = in_stream.codec_params.video().cloned().unwrap_or_default();
        CodecParams::video_codec(parse_video_codec_name(name)?, video)
    } else {
        let mut video = in_stream.codec_params.video().cloned().unwrap_or_default();
        let codec_id = if is_mp4_output(output)
            || matches!(
                output
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref(),
                Some("h264") | Some("264")
            ) {
            CodecId::H264
        } else {
            in_stream.codec_params.codec_id
        };
        if codec_id == CodecId::H264 {
            video.pix_fmt = PixelFormat::Yuv420p;
        }
        CodecParams::video_codec(codec_id, video)
    };

    if let (Some(vf), Some(v)) = (video_filter, out_params.video_mut()) {
        if let Some((w, h)) = parse_scale_from_vf(vf, src_w, src_h) {
            v.width = w;
            v.height = h;
        }
    }

    if let Some(br) = bit_rate_video {
        out_params.bit_rate = parse_bit_rate(br)?;
    }
    if let Some(q) = qp {
        out_params.quality = q.clamp(0, 51);
    } else if let Some(c) = crf {
        out_params.quality = c.round() as i32;
        out_params.quality = out_params.quality.clamp(0, 51);
    }
    if let Some(g) = gop {
        out_params.gop_size = g.max(1);
    } else if out_params.codec_id == CodecId::H264 && out_params.gop_size == 0 {
        out_params.gop_size = 30;
    }
    let _ = quality_video;

    let mut graph = video_filter
        .map(Graph::parse)
        .transpose()?
        .unwrap_or_default();
    graph.set_fps_source(source_fps);

    if let Some(target) = graph.target_fps() {
        if let Some(v) = out_params.video_mut() {
            v.frame_rate = Rational::new(target as i32, 1);
        }
    }

    if out_params.codec_id == CodecId::H264 && out_params.extradata.is_empty() {
        if let Some(v) = out_params.video() {
            if let Ok(enc) = rumpeg::h264::Encoder::with_mode(
                v.width.max(16),
                v.height.max(16),
                rumpeg::h264::IntraMode::I16x16Cavlc,
            ) {
                out_params.extradata = enc.avcc_extradata().unwrap_or_default();
            }
        }
    }

    let mut encoder = codec::open_encoder(&out_params)?;
    let format_hint = if still_codec.is_some() {
        Some("image2")
    } else {
        None
    };
    let mut muxer = format::open_output(output, format_hint)?;
    let out_v = muxer.add_stream(out_params)?;

    let mut audio_encoder = None;
    let mut out_a = None;
    if let Some(ref ain) = audio_in {
        let mut audio = ain.codec_params.audio().cloned().unwrap_or_default();
        audio.sample_fmt = SampleFormat::S16;
        let mut aparams = CodecParams::audio_codec(CodecId::Aac, audio);
        aparams.bit_rate = match bit_rate_audio {
            Some(br) => parse_bit_rate(br)?,
            None => 128_000,
        };
        let sr = aparams.audio().map(|a| a.sample_rate).unwrap_or(48_000);
        let ch = aparams.audio().map(|a| a.layout.channels).unwrap_or(2);
        aparams.extradata = rumpeg::codec::aac::AacEncoderCodec::audio_specific_config(sr, ch);
        audio_encoder = Some(codec::open_encoder(&aparams)?);
        out_a = Some(muxer.add_stream(aparams)?);
    }

    muxer.write_header()?;

    let mut encoded_frames = 0u64;
    let audio_index = audio_in.as_ref().map(|s| s.index);

    loop {
        let packet = match demuxer.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => return Err(e),
        };
        if packet.stream_index == in_stream.index {
            if encoded_frames >= max_frames {
                continue;
            }
            for frame in decoder.decode(&packet)? {
                for frame in graph.run_all(frame)? {
                    for mut pkt in encoder.encode(&frame)? {
                        pkt.stream_index = out_v;
                        muxer.write_packet(&pkt)?;
                    }
                    encoded_frames += 1;
                    if encoded_frames >= max_frames {
                        break;
                    }
                }
            }
        } else if Some(packet.stream_index) == audio_index {
            if let (Some(adec), Some(aenc), Some(ai)) =
                (audio_decoder.as_mut(), audio_encoder.as_mut(), out_a)
            {
                for frame in adec.decode(&packet)? {
                    for mut pkt in aenc.encode(&frame)? {
                        pkt.stream_index = ai;
                        muxer.write_packet(&pkt)?;
                    }
                }
            }
        }
    }

    decoder.send_eof()?;
    loop {
        if encoded_frames >= max_frames {
            break;
        }
        match decoder.receive_frame() {
            Ok(frame) => {
                for frame in graph.run_all(frame)? {
                    for mut pkt in encoder.encode(&frame)? {
                        pkt.stream_index = out_v;
                        muxer.write_packet(&pkt)?;
                    }
                    encoded_frames += 1;
                    if encoded_frames >= max_frames {
                        break;
                    }
                }
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }

    if let Some(adec) = audio_decoder.as_mut() {
        adec.send_eof()?;
        if let (Some(aenc), Some(ai)) = (audio_encoder.as_mut(), out_a) {
            loop {
                match adec.receive_frame() {
                    Ok(frame) => {
                        for mut pkt in aenc.encode(&frame)? {
                            pkt.stream_index = ai;
                            muxer.write_packet(&pkt)?;
                        }
                    }
                    Err(Error::Eof) | Err(Error::NeedMoreData) => break,
                    Err(e) => return Err(e),
                }
            }
        }
    }

    encoder.send_eof()?;
    loop {
        match encoder.receive_packet() {
            Ok(mut pkt) => {
                pkt.stream_index = out_v;
                muxer.write_packet(&pkt)?;
            }
            Err(Error::Eof) | Err(Error::NeedMoreData) => break,
            Err(e) => return Err(e),
        }
    }
    if let (Some(aenc), Some(ai)) = (audio_encoder.as_mut(), out_a) {
        aenc.send_eof()?;
        loop {
            match aenc.receive_packet() {
                Ok(mut pkt) => {
                    pkt.stream_index = ai;
                    muxer.write_packet(&pkt)?;
                }
                Err(Error::Eof) | Err(Error::NeedMoreData) => break,
                Err(e) => return Err(e),
            }
        }
    }

    muxer.write_trailer()?;
    let audio_note = if out_a.is_some() { " + audio" } else { "" };
    eprintln!(
        "Wrote {} ({encoded_frames} frame(s){audio_note})",
        output.display()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn convert_audio(
    input: &PathBuf,
    output: &PathBuf,
    filter_spec: Option<&str>,
    ar: Option<u32>,
    sample_fmt: Option<&str>,
    map: Option<&str>,
    codec_name: Option<&str>,
    bit_rate_audio: Option<&str>,
) -> Result<()> {
    let mut demuxer = format::open_input(input)?;
    let in_stream = stream_from_map(&demuxer, map, MediaType::Audio)?.clone();
    if in_stream.media_type != MediaType::Audio {
        return Err(Error::invalid_data("-map must select an audio stream"));
    }

    let mut decoder = codec::open_decoder(&in_stream.codec_params)?;

    let mut out_params = in_stream.codec_params.clone();
    if let Some(name) = codec_name.filter(|c| !is_copy_codec(c)) {
        let audio = out_params.audio().cloned().unwrap_or_default();
        out_params = CodecParams::audio_codec(parse_audio_codec_name(name)?, audio);
    }
    if let Some(audio) = out_params.audio().cloned() {
        let mut audio = audio;
        if let Some(rate) = ar {
            audio.sample_rate = rate;
        }
        if let Some(name) = sample_fmt {
            audio.sample_fmt = match name {
                "u8" => SampleFormat::U8,
                "s16" => SampleFormat::S16,
                "s32" => SampleFormat::S32,
                "f32" | "flt" => SampleFormat::F32,
                other => return Err(Error::unsupported(format!("sample format {other}"))),
            };
            out_params.codec_id = match audio.sample_fmt {
                SampleFormat::U8 => CodecId::PcmU8,
                SampleFormat::S16 => CodecId::PcmS16Le,
                SampleFormat::S24 => CodecId::PcmS24Le,
                SampleFormat::S32 => CodecId::PcmS32Le,
                SampleFormat::F32 => CodecId::PcmF32Le,
                _ => out_params.codec_id,
            };
        }
        out_params = CodecParams::audio_codec(out_params.codec_id, audio);
    }
    if let Some(br) = bit_rate_audio {
        out_params.bit_rate = parse_bit_rate(br)?;
    }

    let mut encoder = codec::open_encoder(&out_params)?;
    let mut muxer = format::open_output(output, None)?;
    let out_index = muxer.add_stream(out_params.clone())?;
    muxer.write_header()?;

    let mut graph = filter_spec
        .map(Graph::parse)
        .transpose()?
        .unwrap_or_default();

    let target_audio = out_params.audio().cloned();

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
            let frame = graph.run(frame)?;
            let frame = if let (Frame::Audio(ref audio), Some(ref target)) = (&frame, &target_audio)
            {
                if audio.sample_rate != target.sample_rate
                    || audio.format != target.sample_fmt
                    || audio.layout.channels != target.layout.channels
                {
                    Frame::Audio(resample::transform(audio, target)?)
                } else {
                    frame
                }
            } else {
                frame
            };

            for mut pkt in encoder.encode(&frame)? {
                pkt.stream_index = out_index;
                muxer.write_packet(&pkt)?;
            }
        }
    }

    decoder.send_eof()?;
    loop {
        match decoder.receive_frame() {
            Ok(frame) => {
                let frame = graph.run(frame)?;
                for mut pkt in encoder.encode(&frame)? {
                    pkt.stream_index = out_index;
                    muxer.write_packet(&pkt)?;
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
    eprintln!("Wrote {}", output.display());
    Ok(())
}

fn is_copy_codec(name: &str) -> bool {
    name.eq_ignore_ascii_case("copy")
}

fn parse_bit_rate(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('k').or_else(|| s.strip_suffix('K')) {
        Ok(num
            .parse::<u64>()
            .map_err(|_| Error::invalid_data(format!("bad bitrate '{s}'")))?
            * 1000)
    } else if let Some(num) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        Ok(num
            .parse::<u64>()
            .map_err(|_| Error::invalid_data(format!("bad bitrate '{s}'")))?
            * 1_000_000)
    } else {
        s.parse()
            .map_err(|_| Error::invalid_data(format!("bad bitrate '{s}'")))
    }
}

fn parse_video_codec_name(name: &str) -> Result<CodecId> {
    match name.to_ascii_lowercase().as_str() {
        "h264" | "libx264" | "avc" => Ok(CodecId::H264),
        "mjpeg" | "jpeg" => Ok(CodecId::Mjpeg),
        "png" => Ok(CodecId::Png),
        "rawvideo" => Ok(CodecId::RawVideo),
        other => Err(Error::not_found(format!("video codec '{other}'"))),
    }
}

fn parse_audio_codec_name(name: &str) -> Result<CodecId> {
    match name.to_ascii_lowercase().as_str() {
        "pcm_s16le" | "s16" => Ok(CodecId::PcmS16Le),
        "pcm_s24le" => Ok(CodecId::PcmS24Le),
        "pcm_s32le" | "s32" => Ok(CodecId::PcmS32Le),
        "pcm_f32le" | "f32" | "flt" => Ok(CodecId::PcmF32Le),
        "pcm_u8" | "u8" => Ok(CodecId::PcmU8),
        "aac" | "libfdk_aac" => Ok(CodecId::Aac),
        other => Err(Error::not_found(format!("audio codec '{other}'"))),
    }
}

fn stream_from_map<'a>(
    ctx: &'a FormatContext,
    map: Option<&str>,
    fallback: MediaType,
) -> Result<&'a rumpeg::format::Stream> {
    if let Some(spec) = map {
        return resolve_map_spec(ctx, spec);
    }
    ctx.best_stream(fallback)
        .ok_or_else(|| Error::not_found(format!("no {:?} stream", fallback)))
}

fn resolve_map_spec<'a>(ctx: &'a FormatContext, spec: &str) -> Result<&'a rumpeg::format::Stream> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.as_slice() {
        [file, idx] if *file == "0" => {
            let i: usize = idx
                .parse()
                .map_err(|_| Error::invalid_data(format!("bad stream index in '{spec}'")))?;
            ctx.streams()
                .get(i)
                .ok_or_else(|| Error::not_found(format!("stream index {i}")))
        }
        [file, ty, idx] if *file == "0" => {
            let i: usize = idx
                .parse()
                .map_err(|_| Error::invalid_data(format!("bad stream index in '{spec}'")))?;
            let media = match *ty {
                "v" | "V" => MediaType::Video,
                "a" | "A" => MediaType::Audio,
                other => {
                    return Err(Error::invalid_data(format!(
                        "unknown stream type '{other}' in '{spec}'"
                    )));
                }
            };
            ctx.streams()
                .iter()
                .filter(|s| s.media_type == media)
                .nth(i)
                .ok_or_else(|| Error::not_found(format!("no stream for map '{spec}'")))
        }
        _ => Err(Error::invalid_data(format!(
            "unsupported -map specifier '{spec}' (use 0:v:0, 0:a:0, or 0:0)"
        ))),
    }
}

fn output_supports_copy(media: MediaType, path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match media {
        MediaType::Video => matches!(ext.as_deref(), Some("mp4") | Some("mov")),
        MediaType::Audio => matches!(ext.as_deref(), Some("wav")),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn try_remux_copy(
    input: &PathBuf,
    output: &PathBuf,
    map: Option<&str>,
    codec: Option<&str>,
    codec_video: Option<&str>,
    codec_audio: Option<&str>,
    audio_filter: Option<&str>,
    video_filter: Option<&str>,
    frames: Option<u64>,
    ar: Option<u32>,
    sample_fmt: Option<&str>,
    reencode_video: bool,
) -> Result<Option<()>> {
    if audio_filter.is_some()
        || video_filter.is_some()
        || ar.is_some()
        || sample_fmt.is_some()
        || reencode_video
    {
        return Ok(None);
    }

    let copy_v = codec_video.or(codec).map(is_copy_codec).unwrap_or(false);
    let copy_a = codec_audio.or(codec).map(is_copy_codec).unwrap_or(false);
    if !copy_v && !copy_a {
        return Ok(None);
    }

    let mut demuxer = format::open_input(input)?;
    let in_stream = if let Some(spec) = map {
        resolve_map_spec(&demuxer, spec)?.clone()
    } else if copy_v && !copy_a {
        demuxer
            .best_stream(MediaType::Video)
            .ok_or_else(|| Error::not_found("no video stream"))?
            .clone()
    } else if copy_a && !copy_v {
        demuxer
            .best_stream(MediaType::Audio)
            .ok_or_else(|| Error::not_found("no audio stream"))?
            .clone()
    } else {
        return Ok(None);
    };

    let want_copy = match in_stream.media_type {
        MediaType::Video => copy_v,
        MediaType::Audio => copy_a,
        _ => false,
    };
    if !want_copy || !output_supports_copy(in_stream.media_type, output) {
        return Ok(None);
    }

    let mut out_params = in_stream.codec_params.clone();
    if out_params.codec_id == CodecId::H264 && out_params.extradata.is_empty() {
        if let Some(v) = out_params.video() {
            if let Ok(enc) = rumpeg::h264::Encoder::new(v.width.max(16), v.height.max(16)) {
                out_params.extradata = enc.avcc_extradata().unwrap_or_default();
            }
        }
    }

    let format_hint = match in_stream.media_type {
        MediaType::Video => None,
        MediaType::Audio => Some("wav"),
        _ => None,
    };
    let mut muxer = format::open_output(output, format_hint)?;
    let out_index = muxer.add_stream(out_params)?;
    muxer.write_header()?;

    let max_packets = frames.unwrap_or(u64::MAX);
    let mut copied = 0u64;
    loop {
        if copied >= max_packets {
            break;
        }
        let packet = match demuxer.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => return Err(e),
        };
        if packet.stream_index != in_stream.index {
            continue;
        }
        let mut pkt = packet;
        pkt.stream_index = out_index;
        muxer.write_packet(&pkt)?;
        copied += 1;
    }

    muxer.write_trailer()?;
    eprintln!(
        "Wrote {} ({copied} packet(s), stream copy)",
        output.display()
    );
    Ok(Some(()))
}

fn cmd_codecs() -> Result<()> {
    let list = rumpeg::codec::registry::global().list();
    for c in list {
        let kind = match c.kind {
            rumpeg::codec::CodecKind::Decoder => "D",
            rumpeg::codec::CodecKind::Encoder => "E",
            rumpeg::codec::CodecKind::Both => "DE",
        };
        println!("{kind:>2} {:<12} {}", c.name, c.long_name);
    }
    Ok(())
}
