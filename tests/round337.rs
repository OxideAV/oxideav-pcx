//! r337 — encoder no-panic + lossless-roundtrip property sweep.
//!
//! Depth-mode round on a saturated crate: every documented `(bpp,
//! planes)` mode is already covered on both decode and encode, so this
//! round hardens the *encoder* surface the way r327's CHANGELOG hardened
//! the decoder's fuzz surface — but in the CI-run `tests/` harness so the
//! contract is proven green on every push, not only under a manual fuzz
//! session.
//!
//! Two properties are swept across a dimension matrix that spans the
//! packing edge cases (odd vs even widths exercising the spec §1 even-
//! `bytes_per_line` padding; widths straddling the 2 / 4 / 8 pixels-per-
//! byte sub-byte chunk boundaries; single-row, single-column, and 1×1
//! degenerate shapes):
//!
//!  1. **No panic / clean reject.** Every public index-based encoder
//!     either returns `Ok` for a correctly-sized input or a typed `Err`
//!     for a deliberately-undersized one — never a panic, out-of-bounds,
//!     or debug overflow. Dimensions and buffer lengths are chosen to
//!     drive both arms.
//!
//!  2. **Lossless round-trip through the typed accessor.** For each
//!     index-based mode, `parse_*indexed*(encode(indices)).indices ==
//!     indices` after masking each input index to the bit width the mode
//!     can carry (2-bit CGA, 4-bit EGA, 8-bit VGA, 16-bit composite). The
//!     ZSoft rev-5 manual
//!     (`docs/image/pcx/zsoft-pcx-technical-reference-rev5.md`) defines
//!     each plane / packed layout the encoder writes and the decoder
//!     reads back; a round-trip that loses an index would be a defect in
//!     one of the two paths.
//!
//! All inputs are deterministic (a fixed LCG over the index range) so the
//! test is reproducible; this is not a fuzz target (that lives in
//! `fuzz/fuzz_targets/encode_pcx.rs`) but a fixed property sweep.

use oxideav_pcx::{
    encode_pcx_1bpp_4planes_ega, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_4planes,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed, parse_pcx,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_8bpp,
};

/// Deterministic, mask-bounded pseudo-random index generator. A small
/// xorshift keeps the sweep reproducible while still producing a varied
/// index pattern (so a run-length or padding bug doesn't hide behind a
/// constant fill).
fn indices_u8(n: usize, mask: u8, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s as u8) & mask
        })
        .collect()
}

fn indices_u16(n: usize, seed: u64) -> Vec<u16> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u16
        })
        .collect()
}

/// The dimension matrix: small enough to keep the sweep fast, chosen so
/// every packing geometry is hit. Odd widths (1, 3, 5, 7, 9) exercise the
/// even-`bytes_per_line` padding; 2 / 4 / 8 exercise the exact sub-byte
/// chunk boundaries; 17 sits just over the 16-pixel two-byte 8-bpp
/// boundary; 1-tall / 1-wide / 1×1 cover the degenerate shapes.
const DIMS: &[(u16, u16)] = &[
    (1, 1),
    (1, 5),
    (5, 1),
    (2, 2),
    (3, 4),
    (4, 3),
    (7, 7),
    (8, 8),
    (9, 2),
    (16, 2),
    (17, 3),
    (31, 5),
];

#[test]
fn sweep_8bpp_indexed_roundtrip() {
    let palette: Vec<u8> = (0..256u16)
        .flat_map(|i| [i as u8, i as u8, i as u8])
        .collect();
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let idx = indices_u8(n, 0xFF, 0xA53F + n as u64);
        let bytes = encode_pcx_8bpp_indexed(w, h, &idx, &palette).expect("encode 8bpp");
        // Generic flatten path must not panic on the result.
        parse_pcx(&bytes).expect("parse_pcx of 8bpp output");
        let view = parse_pcx_indexed_8bpp(&bytes).expect("indexed 8bpp");
        assert_eq!(view.indices, idx, "8bpp roundtrip {w}x{h}");
        assert_eq!(view.indices.len(), n);
    }
}

#[test]
fn sweep_8bpp_grayscale_roundtrip() {
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let samples = indices_u8(n, 0xFF, 0x1234 + n as u64);
        let bytes = encode_pcx_8bpp_grayscale(w, h, &samples).expect("encode gray");
        // Grayscale flattens to Rgba with R==G==B==sample.
        let img = parse_pcx(&bytes).expect("parse gray");
        let rgba = &img.data;
        for (i, s) in samples.iter().enumerate() {
            let px = &rgba[i * 4..i * 4 + 4];
            assert_eq!((px[0], px[1], px[2]), (*s, *s, *s), "gray {w}x{h} px {i}");
        }
    }
}

#[test]
fn sweep_4bpp_packed_roundtrip() {
    let palette: Vec<u8> = (0..16u8).flat_map(|i| [i << 4, i << 4, i << 4]).collect();
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let idx = indices_u8(n, 0x0F, 0x77AA + n as u64);
        let bytes = encode_pcx_4bpp_packed(w, h, &idx, &palette).expect("encode 4bpp");
        parse_pcx(&bytes).expect("parse_pcx of 4bpp output");
        let view = parse_pcx_indexed_4bpp(&bytes).expect("indexed 4bpp");
        assert_eq!(view.indices, idx, "4bpp packed roundtrip {w}x{h}");
    }
}

#[test]
fn sweep_1bpp_4planes_ega_roundtrip() {
    let palette: Vec<u8> = (0..16u8).flat_map(|i| [i << 4, i << 4, i << 4]).collect();
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let idx = indices_u8(n, 0x0F, 0x55CC + n as u64);
        let bytes = encode_pcx_1bpp_4planes_ega(w, h, &idx, &palette).expect("encode 1bpp4");
        parse_pcx(&bytes).expect("parse_pcx of 1bpp4 output");
        let view = parse_pcx_indexed_1bpp_4planes(&bytes).expect("indexed 1bpp4");
        assert_eq!(view.indices, idx, "1bpp×4 EGA roundtrip {w}x{h}");
    }
}

#[test]
fn sweep_2bpp_cga_roundtrip() {
    // Selector 0x00 = palette 1 high-intensity (the decoder's default
    // assumption), background 0.
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let idx = indices_u8(n, 0b11, 0x9911 + n as u64);
        let bytes = encode_pcx_2bpp_cga(w, h, &idx, 0x00, 0).expect("encode cga");
        parse_pcx(&bytes).expect("parse_pcx of cga output");
        let view = parse_pcx_indexed_2bpp_cga(&bytes).expect("indexed cga");
        assert_eq!(view.indices, idx, "2bpp CGA roundtrip {w}x{h}");
    }
}

#[test]
fn sweep_4bpp_4planes_composite_roundtrip() {
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let idx = indices_u16(n, 0x3C3C + n as u64);
        let bytes = encode_pcx_4bpp_4planes(w, h, &idx).expect("encode 4x4");
        let view = parse_pcx_indexed_4bpp_4planes(&bytes).expect("indexed 4x4");
        assert_eq!(view.indices, idx, "4bpp×4 composite roundtrip {w}x{h}");
    }
}

#[test]
fn sweep_24bpp_roundtrip() {
    for &(w, h) in DIMS {
        let n = w as usize * h as usize;
        let rgb = indices_u8(n * 3, 0xFF, 0x4242 + n as u64);
        let bytes = encode_pcx_24bpp(w, h, &rgb).expect("encode 24bpp");
        let img = parse_pcx(&bytes).expect("parse 24bpp");
        let rgba = &img.data;
        for i in 0..n {
            let src = &rgb[i * 3..i * 3 + 3];
            let px = &rgba[i * 4..i * 4 + 4];
            assert_eq!(
                (px[0], px[1], px[2]),
                (src[0], src[1], src[2]),
                "24bpp {w}x{h} px {i}"
            );
        }
    }
}

/// Every index-based encoder rejects an input buffer shorter than
/// `width × height` with a typed `Err`, never a panic. This is the clean-
/// reject arm of property 1.
#[test]
fn undersized_inputs_are_rejected_not_panicked() {
    let pal768: Vec<u8> = vec![0u8; 768];
    let pal48: Vec<u8> = vec![0u8; 48];
    let short = [0u8; 1];
    let short16 = [0u16; 1];
    // 4×4 = 16 pixels claimed, 1-element buffer supplied.
    assert!(encode_pcx_8bpp_indexed(4, 4, &short, &pal768).is_err());
    assert!(encode_pcx_8bpp_grayscale(4, 4, &short).is_err());
    assert!(encode_pcx_4bpp_packed(4, 4, &short, &pal48).is_err());
    assert!(encode_pcx_1bpp_4planes_ega(4, 4, &short, &pal48).is_err());
    assert!(encode_pcx_2bpp_cga(4, 4, &short, 0x00, 0).is_err());
    assert!(encode_pcx_4bpp_4planes(4, 4, &short16).is_err());
    assert!(encode_pcx_24bpp(4, 4, &short).is_err());

    // Zero dimensions are rejected everywhere.
    assert!(encode_pcx_8bpp_indexed(0, 4, &[0u8; 16], &pal768).is_err());
    assert!(encode_pcx_24bpp(4, 0, &[0u8; 16 * 3]).is_err());

    // Wrong-length palette argument is rejected (not silently padded).
    let pal_wrong = vec![0u8; 47];
    assert!(encode_pcx_4bpp_packed(2, 2, &[0u8; 4], &pal_wrong).is_err());
    let pal768_wrong = vec![0u8; 767];
    assert!(encode_pcx_8bpp_indexed(2, 2, &[0u8; 4], &pal768_wrong).is_err());

    // CGA background index out of the documented 0..=15 range is rejected.
    assert!(encode_pcx_2bpp_cga(2, 2, &[0u8; 4], 0x00, 0x10).is_err());
}
