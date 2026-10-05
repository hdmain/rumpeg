# Rumpeg

High-performance multimedia framework in **Rust**, inspired by [FFmpeg](https://github.com/FFmpeg/FFmpeg).

Rumpeg is designed first as a **library crate**: use it directly from Rust projects for demuxing, decoding, filtering, encoding, and muxing. A thin CLI (`rumpeg`) exercises the same APIs.

## Why Rumpeg?

| Goal | Approach |
|------|----------|
| Library-first | Idiomatic Rust APIs, not a C FFI wrapper |
| Performance | `Arc`-backed zero-copy buffers, buffer pools, release LTO, autovectorizable kernels |
| Clear layering | Same separation as FFmpeg: util → codec / scale / resample → format → filter |
| Safe by default | Memory-safe Rust; H.264 is **pure Rust** (`rumpeg-h264`), not Cisco OpenH264 FFI |

## Crate map

| Crate | FFmpeg analogue | Role |
|-------|-----------------|------|
| `rumpeg-util` | libavutil | Buffers, packets, frames, formats, time bases |
| `rumpeg-h264` | (OpenH264 analogue) | Pure-Rust Baseline H.264 `I_PCM` encode/decode |
| `rumpeg-codec` | libavcodec | Decoder/encoder traits, registry, PCM / rawvideo / H.264 / JPEG |
| `rumpeg-format` | libavformat | Probe, demux, mux (WAV, MP4, Annex-B H.264, JPEG image2) |
| `rumpeg-scale` | libswscale | Pixel format conversion & scaling |
| `rumpeg-resample` | libswresample | Sample format / rate / channel conversion |
| `rumpeg-filter` | libavfilter | Filter graph (`volume`, `aformat`, `format`, `scale`) |
| `rumpeg` | — | Umbrella facade + prelude |
| `rumpeg-cli` | ffmpeg/ffprobe | Command-line front-end |

## Thumbnail (first frame → JPEG)

Library:

```rust
use rumpeg::extract_first_frame_jpeg;

fn main() -> rumpeg::Result<()> {
    rumpeg::init();
    extract_first_frame_jpeg("input.mp4", "thumb.jpg", Some((16, 16)))?;
    Ok(())
}
```

CLI:

```bash
rumpeg convert -i input.mp4 -o thumb.jpg --frames 1 --vf scale=16:16
rumpeg convert -i clip.h264 -o thumb.jpg --frames 1 --vf scale=16:16
```

## H.264 notes (`rumpeg-h264`)

This is Rumpeg’s **own pure-Rust OpenH264-style codec** — no vendored C/C++ and no Cisco OpenH264 FFI.

**v0.1 scope:** Baseline-compatible **Intra `I_PCM`** frames (raw PCM macroblocks). That is enough for synthetic clips, first-frame extract, and thumbnails, and produces bitstreams standard decoders can play. Inter/CAVLC residual coding is intentionally out of scope for now.

## CLI

```bash
cargo run -p rumpeg-cli -- probe input.mp4
cargo run -p rumpeg-cli -- convert -i input.mp4 -o thumb.jpg --frames 1 --vf scale=16:16
cargo run -p rumpeg-cli -- convert -i input.wav -o out.wav --ar 16000 --sample-fmt s16
cargo run -p rumpeg-cli -- codecs
cargo run -p rumpeg-cli -- formats
```

## Examples

```bash
cargo run -p rumpeg --example generate_wav -- sine.wav
cargo run -p rumpeg --example extract_thumb -- thumb.jpg
```

## Current status (v0.1)

**Working today**
- WAV demux / mux for PCM
- **MP4 demux / mux** for `avc1` H.264 (single track)
- **Annex-B `.h264` demux**
- **Pure-Rust H.264** encode/decode (`I_PCM` IDR) via `rumpeg-h264`
- **JPEG encode** + `image2` mux (`.jpg`)
- First-frame extract + scale (library + CLI)
- PCM + rawvideo codecs
- Pixel convert / bilinear scale / audio resample / filters
- `rumpeg probe` / `convert` / `codecs` / `formats`

**Roadmap**
- Broader H.264 (CAVLC Intra/Inter), HEVC/AV1
- More containers (MKV, MPEG-TS)
- Hardware frames, richer filter graphs, SIMD kernels

## License

MIT OR Apache-2.0
