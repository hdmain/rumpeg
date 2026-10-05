//! Rumpeg CLI — ffmpeg/ffprobe-inspired front-end over the Rumpeg library.

use clap::{Parser, Subcommand};
use rumpeg::prelude::*;
use std::path::PathBuf;
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
    /// Copy/transcode media (currently WAV PCM remux / format convert).
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
            ar,
            sample_fmt,
        } => cmd_convert(
            &input,
            &output,
            filter.as_deref(),
            ar,
            sample_fmt.as_deref(),
        ),
        Commands::Codecs => cmd_codecs(),
        Commands::Formats => {
            println!("Demuxers:");
            println!("  wav");
            println!("Muxers:");
            println!("  wav");
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

fn cmd_convert(
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
