//! Decode a WAV file and print basic audio stats.

use rumpeg::prelude::*;
use std::env;

fn main() -> Result<()> {
    rumpeg::init();
    let path = env::args().nth(1).expect("usage: decode_wav <input.wav>");

    let mut input = format::open_input(&path)?;
    let stream = input
        .best_stream(MediaType::Audio)
        .ok_or_else(|| Error::not_found("no audio"))?
        .clone();
    let mut decoder = codec::open_decoder(&stream.codec_params)?;

    let mut packets = 0u64;
    let mut samples = 0u64;
    loop {
        let packet = match input.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => break,
            Err(e) => return Err(e),
        };
        packets += 1;
        for frame in decoder.decode(&packet)? {
            if let Frame::Audio(a) = frame {
                samples += u64::from(a.samples);
            }
        }
    }

    println!("format      : {}", input.format_name());
    println!("codec       : {}", stream.codec_params.codec_id);
    if let Some(a) = stream.codec_params.audio() {
        println!("sample rate : {} Hz", a.sample_rate);
        println!("channels    : {}", a.layout.channels);
        println!("sample fmt  : {}", a.sample_fmt.name());
    }
    println!("packets     : {packets}");
    println!("samples     : {samples}");
    Ok(())
}
