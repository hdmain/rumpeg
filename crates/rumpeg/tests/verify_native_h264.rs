//! Minimal public-API verification for the native H.264 correctness fixes.
//!
//! Run: `cargo test -p rumpeg --test verify_native_h264`
//!
//! Covers the P-frame deblocking fix: the native encoder builds its reference
//! frames without a loop filter, so the PPS must expose
//! `deblocking_filter_control_present_flag` and every slice must carry
//! `disable_deblocking_filter_idc = 1` — otherwise a spec-conformant decoder
//! deblocks its references while the encoder never did, and reconstruction
//! error accumulates across P frames until the next IDR.
//!
//! The CAVLC coeff_token/total_zeros table fix in this branch is verified by
//! the crate-internal differential test
//! `cavlc::tests::spec_decoder_reads_our_nc4_and_nc8_blocks`, which decodes
//! our encoder's blocks with the spec-conformant VLC tables that
//! `rusty_h264-decoder` (the production decoder) is built on. The block
//! encoder is a private API, so that check lives inside `rumpeg-h264`.

#[test]
fn native_pps_signals_deblocking_control() {
    // Pre-fix: deblocking_filter_control_present_flag = false, so the decoder
    // inferred disable_deblocking_filter_idc = 0 (filter ON) while the encoder
    // never deblocked its references.
    // Post-fix: flag = true and slices carry idc = 1 (filter OFF).
    let enc =
        rumpeg::h264::Encoder::with_mode(32, 32, rumpeg::h264::IntraMode::I16x16Cavlc).unwrap();
    let avcc = enc.avcc_extradata().unwrap();
    let cfg = rumpeg::h264::AvcDecoderConfig::from_avcc(&avcc).unwrap();
    let pps = cfg.pps().expect("avcC must contain a PPS");
    assert!(
        pps.deblocking_filter_control_present_flag,
        "PPS must expose deblocking control so slices can disable the filter"
    );
}
