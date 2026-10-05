# Rumpeg

High-performance multimedia framework in **Rust**, inspired by [FFmpeg](https://github.com/FFmpeg/FFmpeg).

Rumpeg is designed first as a **library crate**: use it directly from Rust projects for demuxing, decoding, filtering, encoding, and muxing. A thin CLI (`rumpeg`) exercises the same APIs.

## Why Rumpeg?

| Goal | Approach |
|------|----------|
| Library-first | Idiomatic Rust APIs, not a C FFI wrapper |
| Performance | `Arc`-backed zero-copy buffers, buffer pools, release LTO, autovectorizable kernels |
| Clear layering | Same separation as FFmpeg: util → codec / scale / resample → format → filter |
| Safe by default | Memory-safe Rust; unsafe only where proven necessary for SIMD/IO later |

## Crate map

| Crate | FFmpeg analogue | Role |
|-------|-----------------|------|
| `rumpeg-util` | libavutil | Buffers, packets, frames, formats, time bases |
| `rumpeg-codec` | libavcodec | Decoder/encoder traits, registry, PCM + rawvideo |
| `rumpeg-format` | libavformat | Probe, demux, mux (WAV today) |
| `rumpeg-scale` | libswscale | Pixel format conversion & scaling |
| `rumpeg-resample` | libswresample | Sample format / rate / channel conversion |
| `rumpeg-filter` | libavfilter | Filter graph (`volume`, `aformat`, `scale`, …) |
| `rumpeg` | — | Umbrella facade + prelude |
| `rumpeg-cli` | ffmpeg/ffprobe | Command-line front-end |

## Quick start (library)

```rust
use rumpeg::prelude::*;

fn main() -> rumpeg::Result<()> {
    rumpeg::init();

    let mut input = format::open_input("input.wav")?;
    let stream = input
        .best_stream(MediaType::Audio)
        .ok_or_else(|| Error::not_found("no audio"))?
        .clone();
    let mut decoder = codec::open_decoder(&stream.codec_params)?;

    while let Ok(packet) = input.read_packet() {
        for frame in decoder.decode(&packet)? {
            // Frame::Audio / Frame::Video
            let _ = frame;
        }
    }
    Ok(())
}
```

Add to your `Cargo.toml`:

```toml
[dependencies]
rumpeg = { path = "crates/rumpeg" }
```

## CLI

```bash
cargo run -p rumpeg-cli -- probe input.wav
cargo run -p rumpeg-cli -- convert -i input.wav -o out.wav --ar 16000 --sample-fmt s16
cargo run -p rumpeg-cli -- convert -i input.wav -o quiet.wav -f volume=0.25
cargo run -p rumpeg-cli -- codecs
```

## Examples

```bash
cargo run -p rumpeg --example generate_wav -- sine.wav
cargo run -p rumpeg --example decode_wav -- sine.wav
```

## Architecture

```
┌────────────┐   packets   ┌────────────┐   frames   ┌────────────┐
│  format    │ ──────────► │   codec    │ ─────────► │  filter    │
│ (demuxer)  │             │ (decoder)  │            │  / scale   │
└────────────┘             └────────────┘            │  /resample │
                                                     └─────┬──────┘
                                                           │ frames
                                                     ┌─────▼──────┐
                                                     │   codec    │
                                                     │ (encoder)  │
                                                     └─────┬──────┘
                                                           │ packets
                                                     ┌─────▼──────┐
                                                     │  format    │
                                                     │  (muxer)   │
                                                     └────────────┘
```

Core types (`Packet`, `Frame`, `Buffer`) live in `rumpeg-util` and are shared everywhere — same idea as FFmpeg’s `AVPacket` / `AVFrame` / `AVBuffer`.

## Current status (v0.1)

**Working today**
- WAV demux / mux for PCM (`u8`, `s16le`, `s24le`, `s32le`, `f32le`)
- PCM + rawvideo encode/decode (zero-copy where possible)
- Pixel convert: YUV420P ↔ RGB24, RGB ↔ RGBA/BGR/Gray
- Bilinear / nearest scaling
- Audio resample, sample-format convert, mono↔stereo
- Filter graph: `volume`, `aformat`, `format`, `scale`
- `rumpeg probe` / `convert` / `codecs` CLI

**Roadmap (FFmpeg-parity direction)**
- Containers: MP4/ISOBMFF, Matroska/WebM, MPEG-TS
- Codecs: FLAC, Opus, AAC, H.264/HEVC/AV1 (pure-Rust and/or hardware)
- Bitstream filters, hardware frames, device capture
- Broader filter set and graph labeling (`[in]…[out]`)
- SIMD kernels (AVX2/NEON) behind feature flags
- FATE-style conformance corpus

## Performance notes

- `Buffer` is `Arc<[u8]>` with clone-on-write — packet→PCM decode can share the allocation.
- `BufferPool` recycles fixed-size buffers for steady streaming.
- Workspace release profile uses thin LTO + single codegen unit.
- Scale/convert loops are written for LLVM autovectorization; explicit SIMD can land without API breaks.

## License

MIT OR Apache-2.0
