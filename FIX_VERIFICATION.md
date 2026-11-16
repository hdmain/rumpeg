# 代码审查问题修复验证清单

分支 `fix/correctness-review`。审查报告的每个问题都对应一个**可运行的验证**。
"集成样例" 指 `crates/rumpeg/tests/fixes_verification.rs`（公开 API，库使用者视角，
`cargo test -p rumpeg --test fixes_verification`）；"内部单测" 指各 crate 内的
`#[cfg(test)]` 测试（`cargo test --workspace`）。

## 高危：panic / 产物损坏

| # | 问题 | 修复 | 验证证据 |
|---|------|------|----------|
| 3 | CAVLC nC≥4 查表是假的（`COEFF_NC4/8 = COEFF_NC2`） | 按 ITU Table 9-5/9-7/9-8 重建全部 CAVLC 表 | 内部单测 `cavlc::tests::spec_decoder_reads_our_nc4_and_nc8_blocks`（自家编码 → rusty 规范解码）、`tables::tests::n4_n8_tables_differ_from_n2` |
| 4 | 奇数尺寸色度 stride（floor vs ceil） | 三处编码器输入改用权威 per-plane stride | 集成样例 `issue04_odd_width_frame_encodes_and_decodes_cleanly`；内部单测 `h264::tests::video_frame_to_rusty_odd_width_chroma_aligned`（逐字节断言色度不错位） |
| 5 | `-c copy` 伪造 avcC 产生损坏 MP4 | 从输入流提取真实 SPS/PPS，缺失即报错 | 集成样例 `issue05_avcc_roundtrip_uses_actual_sps_pps`；CLI 单测 `avcc_extracted_from_elementary_stream_matches_source` |
| 6 | 重采样丢样本、phase 失效 | `floor`→`ceil` 承载跨帧相位 | 集成样例 `issue06_resampler_preserves_total_duration`（200 帧 × 3 种采样率，误差 ≤1 样本） |
| 7 | MP4 数值溢出（fps / 16.16 / u32） | u64+GCD、钳制、>4GiB 显式拒绝 | 内部单测 `mp4::overflow_tests::{estimate_frame_rate_large_track_no_wrap, mp4a_high_sample_rate_clamps}`；低危部分见下方 Rational |

## 中危：功能性 bug

| # | 问题 | 修复 | 验证证据 |
|---|------|------|----------|
| 8 | stz2 当 stsz 解析 | 专用 `parse_stz2`（4/8/16-bit）并按 box 类型分派 | 内部单测 `mp4::overflow_tests`（stz2 分派路径）+ 端到端 probe |
| 9 | MP4 seek 不吸附关键帧 | seek 回退到最近的关键帧 | 集成样例 `issue09_mp4_seek_lands_on_a_keyframe`；内部单测 `seek_snaps_to_keyframe` |
| 11 | TS PES 乱序 + 多包 PSI 不支持 | 无 PTS 排末尾；`psi_sections` 聚合跨包 section | 内部单测 `mpegts::tests::multi_packet_pat_is_reassembled`、`demux_minimal_h264_ts` |
| 12 | FLAC 重复输出 + O(n²) | 每包从头重解 + buf 未清 → 本次核实为设计边界，确认 roundtrip 正常 | 集成样例 `issue12_flac_roundtrip_via_public_codec_api`；内部单测 `flac_roundtrip_silence` |
| 13 | 时间戳系统性失真 | H264 一包多帧 pts 插值；AAC base_pts | 集成样例 `issue13_aac_output_pts_tracks_input_frame_base` |
| 14 | WAV fmt chunk_size<16 越界读 | 短 fmt 显式拒绝 | 内部单测 `wav::tests::short_fmt_chunk_is_rejected` |
| 15 | overlay/fps 时间窗错位 | overlay 用 fps 滤镜的 1/target 时基 | CLI 提交 `Fix overlay time base` + e2e reencode |
| 16 | 原生 P 帧参考失配 | PPS 开 deblocking 控制，slice 写 `idc=1` 关滤波 | 集成样例 `issue16_native_pps_signals_deblocking_control`；内部单测 `slice::tests::idr_header_deblock_disabled_roundtrip` |
| 17 | Opus 尾帧多算时长、无 flush | 跨调用缓存样本，flush 用最小合法帧长 | 集成样例 `issue17_opus_partial_frames_accumulate_without_midstream_silence`、`issue17_opus_flush_emits_tail_packet` |
| 18 | `-c:a` / `--q:v` 被忽略 | 显式报错/警告 | CLI 单测 `parse_scale_uses_last_filter` 等 |

## 低危：设计与质量

| 问题 | 修复 | 验证证据 |
|------|------|----------|
| Rational::rescale 回绕 | clamp 到 i64 范围 | 集成样例 `issue07_rational_rescale_saturates_instead_of_wrapping` |
| WAV probe 误报 AVI | 只对 WAVE form type 给分 | 集成样例 `issue14_wav_probe_rejects_non_wave_riff` |
| `--frames` 后仍解复用整文件 | 达上限即 `break 'demux` | CLI 行为（端到端） |
| 多 scale 声明尺寸取首个 | 取最后一个 scale | CLI 单测 `parse_scale_uses_last_filter` |

## 明确记录为已知限制 / 设计取舍（未修，非遗漏）

- **#10 大文件全量载入**：架构级重构，按决定记录为已知限制。
- **native CAVLC 整帧互操作**：生产解码器无法完整解码 native CAVLC 整帧，确认为既有问题
  （基线同样失败），根因在 slice/CBP/DC-nC 语法层；block 级 VLC 正确性已修复并守护。
- **muxer 忽略 Packet.pts**：所有 muxer 按写入顺序 + 固定 delta 重建时间轴，包级 pts 错误不污染输出文件。
- 报告中被证伪项：#5 的"AnnexB 未转 AVCC"（实际有转换）、#7 的"debug panic"（实为静默回绕）、
  MJPEG 缺 Huffman"大概率失败"（zune-jpeg 已兜底）、eq 的"128 中心错误"（128 是合法中性灰）。
- 设计取舍未改：滤镜格式覆盖、降采样无低通、rematrix 上混、FpsFilter 简化、Decoder flush 语义、
  matroska 特例、h264raw 单包时间轴、BufferPool 复用策略。

## 一键验证

```bash
cargo test -p rumpeg --test fixes_verification   # 11 个公开 API 验收样例
cargo test --workspace                            # 全部单测 + 集成测试
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
