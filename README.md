# Rumpeg

High-performance multimedia framework in **Rust**, inspired by [FFmpeg](https://github.com/FFmpeg/FFmpeg).

Rumpeg is designed first as a **library crate**: use it directly from Rust projects for demuxing, decoding, filtering, encoding, and muxing. A thin CLI (`rumpeg`) exercises the same APIs.

## Why Rumpeg?

| Goal | Approach |
|------|----------|
| Library-first | Idiomatic Rust APIs, not a C FFI wrapper |
| Performance | `Arc`-backed zero-copy buffers, buffer pools, release LTO, autovectorizable kernels |
| Clear layering | Same separation as FFmpeg: util → codec / scale / resample → format → filter |
| Safe by default | Memory-safe Rust; codecs below are **pure Rust** (no C/FFI) unless noted |

## Crate map

| Crate | FFmpeg analogue | Role |
|-------|-----------------|------|
| `rumpeg-util` | libavutil | Buffers, packets, frames, formats, time bases |
| `rumpeg-h264` | (OpenH264 analogue) | H.264: CABAC/CAVLC decode (`rusty_h264-decoder`) + Intra/P encode |
| `rumpeg-codec` | libavcodec | Registry + PCM / H.264 / HEVC / VP9 / FLAC / Opus / AAC / MP3 / JPEG / PNG |
| `rumpeg-format` | libavformat | Probe, demux, mux (WAV, MP4, MKV/WebM, MPEG-TS, Annex-B, image2) |
| `rumpeg-scale` | libswscale | Pixel format conversion & scaling |
| `rumpeg-resample` | libswresample | Sample format / rate / channel conversion |
| `rumpeg-filter` | libavfilter | Filter graph (scale, crop, pad, flip, fps, eq, volume, …) |
| `rumpeg` | — | Umbrella facade + prelude |
| `rumpeg-cli` | ffmpeg/ffprobe | Command-line front-end |

Requires Rust **1.85+**.

## Thumbnail (first frame → JPEG)

```bash
rumpeg convert -i input.mp4 -o thumb.jpg --frames 1 --vf scale=16:16
rumpeg convert -i clip.h264 -o thumb.png --frames 1 --vf scale=16:16,hflip
```

## Re-encode: FPS + bitrate / quality (+ audio)

```bash
# Fast path (recommended): downscale + bitrate, keeps AAC audio in MP4
rumpeg convert -i input.mp4 -o out.mp4 --fast
rumpeg convert -i input.mp4 -o out.mp4 --vf scale=640:-2 -b:v 800k -b:a 128k

# Change frame rate
rumpeg convert -i input.mp4 -o out.mp4 --fps 24 --crf 28

# Explicit quality knobs
rumpeg convert -i input.mp4 -o out.mp4 --qp 32 --gop 60
```

| Flag | Meaning |
|------|---------|
| `--fast` | Inject `scale=640:-2` + `-b:v 800k` when unset (much faster encode) |
| `--fps` / `-r` | Target frame rate (injects `fps=N` into `--vf`) |
| `--vf fps=N` / `scale=W:H` | Filters; `scale=640:-2` keeps aspect (even height) |
| `-b:v` / `--bitrate` | Video bitrate → QP heuristic |
| `-b:a` | AAC audio bitrate (default `128k` when muxing audio) |
| `--crf` / `--qp` / `--gop` | H.264 quality / GOP |

**MP4 output** re-encodes **H.264 video + AAC audio** when the source has an audio track. Large sources without scale/bitrate auto-apply `scale=640:-2 -b:v 800k` for speed.

**Encoder limits:** pure-Rust Baseline IDR+P (SKIP/Intra-refresh). No CABAC encode, no real motion search, no 2-pass.

## H.264 notes (`rumpeg-h264`)

**Decode** uses [`rusty_h264-decoder`](https://crates.io/crates/rusty_h264-decoder) (pure Rust). Rumpeg disables the optional `asm` feature (OpenH264 SIMD kernels) so there is **no Cisco OpenH264 C/C++ FFI**.

**Decode supports (practical Progressive 8-bit 4:2:0)**
- **CABAC and CAVLC**
- Intra (`I_16x16` / `I_4x4` / `I_8x8` / `I_PCM` on CAVLC paths), **P** and **B** slices
- Baseline / Main / much of High profile
- MP4 `avc1` + `avcC` and Annex-B `.h264`

**Encode (Rumpeg-native)**
- `I_PCM` and `I_16x16` + CAVLC
- **P frames**: SKIP (copy) or Intra-refresh MBs in P slices; GOP via `--gop`
- QP / CRF / bitrate-heuristic knobs
- **Not:** CABAC encode, real motion estimation, B frames, 2-pass ABR

**Not supported / limited**
- MBAFF / field coding, 10-bit, 4:2:2 / 4:4:4
- Hardware decode/encode
- Some exotic High-profile edge cases (upstream gaps)
- CABAC `I_PCM` (upstream note)

## CLI

```bash
cargo run -p rumpeg-cli -- probe input.mkv
cargo run -p rumpeg-cli -- convert -i input.mp4 -o thumb.jpg --frames 1 --vf scale=16:16
cargo run -p rumpeg-cli -- convert -i input.mp4 -o out.mp4 --fps 24 --crf 28
cargo run -p rumpeg-cli -- convert -i input.wav -o out.wav --ar 16000 --sample-fmt s16
cargo run -p rumpeg-cli -- convert -i in.mp4 -o out.mp4 -c copy
cargo run -p rumpeg-cli -- convert -i in.mp4 -o out.wav --map 0:a:0 -c:a pcm_s16le
cargo run -p rumpeg-cli -- codecs
cargo run -p rumpeg-cli -- formats
```

Useful convert knobs: `--map`, `-c` / `--c:v` / `--c:a` (including `copy`), `--fps`/`-r`, `-b:v`/`--crf`/`--qp`/`--gop`, `--vf` / `-f`.

## Current status (v0.1+)

**Containers (demux)**
- WAV (PCM)
- MP4 / ISOBMFF: `avc1` H.264 + optional `mp4a` AAC (multi-track); frame rate from sample timing
- Matroska / WebM (via `matroska-demuxer`)
- MPEG-TS (minimal PAT/PMT/PES; H.264 + AAC)
- Annex-B `.h264`
- image2 demux: `.jpg` / `.png` as a single video packet

**Containers (mux)**
- WAV, MP4 (`avc1` H.264 + optional `mp4a` AAC), image2 (JPEG/PNG)
- MKV / WebM / MPEG-TS mux: **not implemented** (demux only)

**Video codecs**
| Codec | Decode | Encode |
|-------|--------|--------|
| H.264 | yes (CABAC+CAVLC) | Baseline Intra + simple P (SKIP/Intra-refresh); QP/CRF/bitrate |
| HEVC | yes (`rusty_h265`, Progressive 8-bit path) | no |
| VP9 | yes (`rusty_vp9`) | no |
| VP8 | **no** (not registered) | no |
| AV1 | **no** (not registered; `rav1d` has C-ABI only) | no |
| JPEG / PNG | yes | yes |

**Audio codecs**
| Codec | Decode | Encode |
|-------|--------|--------|
| PCM | yes | yes |
| FLAC | yes (`claxon`) | yes (`flacenc`) |
| MP3 | yes | yes (`rusty_mp3`) |
| AAC | yes | LC encode (`rusty_aac`) |
| Opus | yes | yes (`rusty-opus`) |

**Filters** (`filter=a,b,c` / `--vf`): `volume`, `aformat`, `aresample`, `format`, `scale`, `hflip`, `vflip`, `crop`, `pad`, `transpose`, `fps` (real source rate + PTS rewrite), `framestep`, `eq`, `hue`, `overlay` (passthrough until dual-input graphs exist), `null` / `anull`.

**Honest gaps**
- No AV1 / VP8 registration until a clean pure-Rust decode wrap exists
- No H.264 CABAC encode / real ME / 2-pass / VBV
- No MKV/TS mux
- MPEG-TS: single-packet PSI only (no multi-section reassembly yet)
- Hardware / exotic chroma / MBAFF out of scope
- Audio bitrate knobs are accepted but not deeply applied beyond PCM paths

## License

MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
