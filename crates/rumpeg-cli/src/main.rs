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
        /// Video filterchain (e.g. `scale=16:16`).
        #[arg(long = "vf")]
        vf: Option<String>,
        /// Stop after encoding this many video frames (e.g. `1` for a thumbnail).
        #[arg(long)]
        frames: Option<u64>,
        /// Target sample rate (audio).
        #[arg(long)]
        ar: Option<u32>,
        /// Target sample format name: u8, s16, s32, f32.
        #[arg(long)]
        sample_fmt: Option<String>,
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
            frames,
            ar,
            sample_fmt,
        } => cmd_convert(
            &input,
            &output,
            filter.as_deref(),
            vf.as_deref(),
            frames,
            ar,
            sample_fmt.as_deref(),
        ),
        Commands::Codecs => cmd_codecs(),
        Commands::Formats => {
            println!("Demuxers:");
            println!("  wav");
            println!("  mp4");
            println!("  h264");
            println!("Muxers:");
            println!("  wav");
            println!("  mp4");
            println!("  image2 (jpg/jpeg)");
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
        println!();
    }
    Ok(())
}

fn is_jpeg_output(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()),
        Some(ref e) if e == "jpg" || e == "jpeg"
    )
}

fn cmd_convert(
    input: &PathBuf,
    output: &PathBuf,
    audio_filter: Option<&str>,
    video_filter: Option<&str>,
    frames: Option<u64>,
    ar: Option<u32>,
    sample_fmt: Option<&str>,
) -> Result<()> {
    let prefer_video = is_jpeg_output(output) || frames.is_some() || video_filter.is_some();
    if prefer_video {
        return try_convert_video(input, output, video_filter, frames.or(Some(1)));
    }

    // Auto-select based on available streams.
    let probe = format::open_input(input)?;
    if probe.best_stream(MediaType::Video).is_some()
        && probe.best_stream(MediaType::Audio).is_none()
    {
        drop(probe);
        return try_convert_video(input, output, video_filter, frames);
    }
    drop(probe);
    convert_audio(input, output, audio_filter, ar, sample_fmt)
}

fn try_convert_video(
    input: &PathBuf,
    output: &PathBuf,
    video_filter: Option<&str>,
    frames: Option<u64>,
) -> Result<()> {
    let mut demuxer = format::open_input(input)?;
    let in_stream = demuxer
        .best_stream(MediaType::Video)
        .ok_or_else(|| Error::not_found("no video stream"))?
        .clone();
    let mut decoder = codec::open_decoder(&in_stream.codec_params)?;

    let jpeg_out = is_jpeg_output(output);
    let max_frames = frames.unwrap_or(if jpeg_out { 1 } else { u64::MAX });

    let mut out_params = if jpeg_out {
        let (w, h) = in_stream
            .codec_params
            .video()
            .map(|v| (v.width.max(1), v.height.max(1)))
            .unwrap_or((1, 1));
        CodecParams::video_codec(
            CodecId::Mjpeg,
            VideoParams {
                pix_fmt: PixelFormat::Rgb24,
                width: w,
                height: h,
                frame_rate: Default::default(),
                sample_aspect_ratio: Default::default(),
            },
        )
    } else {
        in_stream.codec_params.clone()
    };

    // For MP4 output of H.264, ensure avcC extradata is present.
    if out_params.codec_id == CodecId::H264 && out_params.extradata.is_empty() {
        if let Some(v) = out_params.video() {
            if let Ok(enc) = rumpeg::h264::Encoder::new(v.width.max(16), v.height.max(16)) {
                out_params.extradata = enc.avcc_extradata().unwrap_or_default();
            }
        }
    }

    let mut encoder = codec::open_encoder(&out_params)?;
    let format_hint = if jpeg_out { Some("image2") } else { None };
    let mut muxer = format::open_output(output, format_hint)?;
    let out_index = muxer.add_stream(out_params)?;
    muxer.write_header()?;

    let mut graph = video_filter
        .map(Graph::parse)
        .transpose()?
        .unwrap_or_default();

    let mut encoded_frames = 0u64;
    loop {
        if encoded_frames >= max_frames {
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
        for frame in decoder.decode(&packet)? {
            let frame = graph.run(frame)?;
            for mut pkt in encoder.encode(&frame)? {
                pkt.stream_index = out_index;
                muxer.write_packet(&pkt)?;
            }
            encoded_frames += 1;
            if encoded_frames >= max_frames {
                break;
            }
        }
    }

    if encoded_frames < max_frames {
        decoder.send_eof()?;
        loop {
            if encoded_frames >= max_frames {
                break;
            }
            match decoder.receive_frame() {
                Ok(frame) => {
                    let frame = graph.run(frame)?;
                    for mut pkt in encoder.encode(&frame)? {
                        pkt.stream_index = out_index;
                        muxer.write_packet(&pkt)?;
                    }
                    encoded_frames += 1;
                }
                Err(Error::Eof) | Err(Error::NeedMoreData) => break,
                Err(e) => return Err(e),
            }
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
    eprintln!("Wrote {} ({encoded_frames} frame(s))", output.display());
    Ok(())
}

fn convert_audio(
    input: &PathBuf,
    output: &PathBuf,
    filter_spec: Option<&str>,
    ar: Option<u32>,
    sample_fmt: Option<&str>,
) -> Result<()> {
    let mut demuxer = format::open_input(input)?;
    let in_stream = demuxer
        .best_stream(MediaType::Audio)
        .ok_or_else(|| Error::not_found("no audio stream"))?
        .clone();

    let mut decoder = codec::open_decoder(&in_stream.codec_params)?;

    let mut out_params = in_stream.codec_params.clone();
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
