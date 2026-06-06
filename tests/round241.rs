//! r241 — typed 4 bpp × 1 plane paletted accessor.
//!
//! Spec §4.1 / EGFF video-mode table line "4 bpp / 1 plane / 16 colours
//! / EGA and VGA" describes a 16-colour packed-bits PCX where each byte
//! of the on-disk scanline holds two pixels (high nibble = even-x
//! pixel, low nibble = odd-x pixel). The 16-entry palette is recorded
//! in the header's 48-byte `ega_palette` field; per the rev-5 manual
//! a PCX 3.0+ writer that leaves the field at all-zeros expects the
//! decoder to fall back to the standard EGA hardware palette listed in
//! spec table §3.1.
//!
//! The pre-r241 [`oxideav_pcx::parse_pcx`] entry point always flattens
//! the on-disk image to packed `Rgba`, which is convenient for display
//! pipelines but discards the nibble indices the file actually carries.
//! r241 adds [`oxideav_pcx::parse_pcx_indexed_4bpp`] — a typed accessor
//! that surfaces the `width × height` unpacked index buffer (one byte
//! per pixel, low-nibble = palette index `0..=15`, top-down) alongside
//! the resolved 16-entry RGB palette and a
//! [`oxideav_pcx::Pcx4bppPaletteSource`] tag recording which spec §3
//! branch produced the palette.
//!
//! This file tests:
//!
//! 1. Round-trip an [`oxideav_pcx::encode_pcx_4bpp_packed`] output
//!    through the new accessor and check indices / palette /
//!    `Pcx4bppPaletteSource::Ega16InHeader` are surfaced exactly.
//! 2. A hand-built 4 bpp × 1 plane PCX with an all-zero `ega_palette`
//!    header field surfaces `Pcx4bppPaletteSource::Ega16Default` and
//!    the spec table §3.1 hardware palette.
//! 3. The accessor consistency check: for every fixture the indices
//!    flattened through the surfaced palette match the byte stream
//!    [`oxideav_pcx::parse_pcx`] produces — i.e. the typed view does
//!    NOT diverge from the canonical RGBA flattener.
//! 4. The accessor rejects every non-(4, 1) (depth, planes) combination
//!    with [`oxideav_pcx::PcxError::Unsupported`].
//! 5. Per-row padding (odd-width fixtures where `bytes_per_line` is
//!    rounded up to even per spec §1) is stripped — the typed view
//!    surfaces exactly `width × height` indices, not `bytes_per_line ×
//!    2 × height` raw nibbles.
//! 6. The accessor shares its validation surface with
//!    [`oxideav_pcx::parse_pcx`]: a malformed file rejected by one is
//!    rejected by the other with a matching error class.

use oxideav_pcx::{
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_packed,
    encode_pcx_8bpp_grayscale, parse_pcx, parse_pcx_indexed_4bpp, Pcx4bppPaletteSource, PcxError,
    PcxIndexed4,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Deterministic 4-bit index pattern: per-pixel `(x * 5 ^ y * 3) & 0x0F`
/// covers all 16 palette entries while keeping the bytes byte-mixed
/// enough that the RLE encoder's run-coalescer is exercised.
fn indices_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x.wrapping_mul(5) ^ y.wrapping_mul(3)) & 0x0F) as u8);
        }
    }
    out
}

/// Synthetic 16-entry RGB palette (48 bytes) used by the in-header
/// round-trip test. The triplets are chosen to be obviously non-default
/// so a mistake that surfaces the EGA hardware palette instead would
/// be caught.
fn palette_48() -> Vec<u8> {
    let mut p = Vec::with_capacity(48);
    for i in 0..16u16 {
        let r = ((i.wrapping_mul(17)) & 0xFF) as u8;
        let g = ((i.wrapping_mul(29)) & 0xFF) as u8;
        let b = ((i.wrapping_mul(43)) & 0xFF) as u8;
        // Force non-zero in every triplet so the "any non-zero byte"
        // branch in the decoder fires deterministically.
        let r = if r == 0 { 1 } else { r };
        let g = if g == 0 { 2 } else { g };
        let b = if b == 0 { 3 } else { b };
        p.push(r);
        p.push(g);
        p.push(b);
    }
    p
}

/// Spec table §3.1 standard EGA hardware palette — the fallback the
/// decoder substitutes when the header `ega_palette` is all-zeros.
const EGA_DEFAULT: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xAA],
    [0x00, 0xAA, 0x00],
    [0x00, 0xAA, 0xAA],
    [0xAA, 0x00, 0x00],
    [0xAA, 0x00, 0xAA],
    [0xAA, 0x55, 0x00],
    [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55],
    [0x55, 0x55, 0xFF],
    [0x55, 0xFF, 0x55],
    [0x55, 0xFF, 0xFF],
    [0xFF, 0x55, 0x55],
    [0xFF, 0x55, 0xFF],
    [0xFF, 0xFF, 0x55],
    [0xFF, 0xFF, 0xFF],
];

/// Hand-built 4 bpp × 1 plane PCX with an all-zero `ega_palette`
/// header field. Exercises the `Ega16Default` source branch (the
/// decoder substitutes the spec table §3.1 hardware palette).
fn build_4bpp_no_palette(w: u16, h: u16, indices: &[u8]) -> Vec<u8> {
    assert_eq!(indices.len(), w as usize * h as usize);
    // bytes_per_line = ceil(w / 2), rounded up to even per spec §1
    let bpl_nat = (w as usize).div_ceil(2);
    let bpl = (bpl_nat + 1) & !1;
    let mut out = Vec::with_capacity(128 + bpl * h as usize * 2);
    out.resize(128, 0);
    out[0] = 0x0A; // manufacturer
    out[1] = 5; // version 5
    out[2] = 1; // RLE
    out[3] = 4; // bits_per_pixel
    let x_max = w - 1;
    let y_max = h - 1;
    out[8..10].copy_from_slice(&x_max.to_le_bytes());
    out[10..12].copy_from_slice(&y_max.to_le_bytes());
    // h_dpi / v_dpi at offsets 12 / 14 stay at zero per the default.
    // ega_palette at 16..64 stays at all-zeros — the "fallback" branch
    // we want to exercise.
    out[65] = 1; // n_planes
    out[66..68].copy_from_slice(&(bpl as u16).to_le_bytes());
    // palette_info at offset 68 stays at zero (spec §3 says 1 = colour
    // / BW, 2 = grayscale; 0 isn't formally defined but pre-PB-IV
    // writers commonly leave it that way and the decoder treats it as
    // "not the grayscale flag", which is what we want here).
    for y in 0..h as usize {
        // Pack `indices` into 4 bpp per pixel: even-x → high nibble,
        // odd-x → low nibble.
        let mut row = vec![0u8; bpl];
        for x in 0..w as usize {
            let v = indices[y * w as usize + x] & 0x0F;
            let byte_off = x / 2;
            if x % 2 == 0 {
                row[byte_off] |= v << 4;
            } else {
                row[byte_off] |= v;
            }
        }
        rle_encode_into(&row, &mut out);
    }
    out
}

fn rle_encode_into(input: &[u8], out: &mut Vec<u8>) {
    let mut i = 0usize;
    let n = input.len();
    while i < n {
        let b = input[i];
        let mut run = 1usize;
        while i + run < n && input[i + run] == b && run < 63 {
            run += 1;
        }
        if run >= 2 || (b & 0xC0) == 0xC0 {
            out.push(0xC0 | (run as u8));
            out.push(b);
        } else {
            out.push(b);
        }
        i += run;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip an [`encode_pcx_4bpp_packed`] output through
/// [`parse_pcx_indexed_4bpp`] and verify all three components.
#[test]
fn ega_in_header_roundtrip_surfaces_indices_and_palette() {
    let (w, h) = (32u16, 16u16);
    let indices = indices_grid(w as usize, h as usize);
    let palette = palette_48();
    let pcx = encode_pcx_4bpp_packed(w, h, &indices, &palette).expect("encode");
    let view = parse_pcx_indexed_4bpp(&pcx).expect("parse_pcx_indexed_4bpp");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.indices, indices);
    assert_eq!(view.palette_source, Pcx4bppPaletteSource::Ega16InHeader);
    for i in 0..16 {
        assert_eq!(
            view.palette[i],
            [palette[i * 3], palette[i * 3 + 1], palette[i * 3 + 2]],
            "palette entry {i}"
        );
    }
    assert_eq!(view.stride(), w as usize);
}

/// A 4 bpp × 1 plane PCX with an all-zero `ega_palette` header field
/// surfaces `Ega16Default` and the spec table §3.1 hardware palette.
#[test]
fn no_palette_surfaces_default_ega_hardware_palette() {
    let (w, h) = (24u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);
    let pcx = build_4bpp_no_palette(w, h, &indices);
    let view = parse_pcx_indexed_4bpp(&pcx).expect("parse_pcx_indexed_4bpp");
    assert_eq!(view.indices, indices);
    assert_eq!(view.palette_source, Pcx4bppPaletteSource::Ega16Default);
    assert_eq!(view.palette, EGA_DEFAULT);
}

/// For every fixture, flattening the typed view's indices through its
/// surfaced palette must produce the same RGBA bytes [`parse_pcx`]
/// does. This pins the typed view as a strict rearrangement (NOT a
/// divergence) of the canonical flattener.
#[test]
fn typed_view_agrees_with_canonical_flattener() {
    let (w, h) = (40u16, 12u16);
    let indices = indices_grid(w as usize, h as usize);
    let palette = palette_48();

    // In-header palette case.
    let pcx_pal = encode_pcx_4bpp_packed(w, h, &indices, &palette).expect("encode");
    let img = parse_pcx(&pcx_pal).expect("parse_pcx");
    let view = parse_pcx_indexed_4bpp(&pcx_pal).expect("parse_pcx_indexed_4bpp");
    assert_indices_flatten_to_rgba(&view, &img.data);

    // Default-palette fallback case.
    let pcx_fb = build_4bpp_no_palette(w, h, &indices);
    let img_fb = parse_pcx(&pcx_fb).expect("parse_pcx");
    let view_fb = parse_pcx_indexed_4bpp(&pcx_fb).expect("parse_pcx_indexed_4bpp");
    assert_indices_flatten_to_rgba(&view_fb, &img_fb.data);
}

fn assert_indices_flatten_to_rgba(view: &PcxIndexed4, rgba: &[u8]) {
    assert_eq!(view.indices.len() * 4, rgba.len());
    for (i, &idx) in view.indices.iter().enumerate() {
        let p = view.palette[idx as usize];
        let off = i * 4;
        assert_eq!(rgba[off], p[0], "R mismatch at pixel {i} index {idx}");
        assert_eq!(rgba[off + 1], p[1], "G mismatch at pixel {i} index {idx}");
        assert_eq!(rgba[off + 2], p[2], "B mismatch at pixel {i} index {idx}");
        assert_eq!(rgba[off + 3], 0xFF, "alpha mismatch at pixel {i}");
    }
}

/// Reject every non-(4, 1) depth/planes combination with
/// [`PcxError::Unsupported`] — the typed accessor's scope is the 4 bpp
/// × 1 plane EGA path, not the 8-bit / 24-bit / 1 bpp / 2 bpp paths.
#[test]
fn rejects_non_4bpp_single_plane_inputs() {
    let (w, h) = (8u16, 4u16);

    // 24-bit RGB
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let pcx_24 = encode_pcx_24bpp(w, h, &rgb).expect("encode");
    match parse_pcx_indexed_4bpp(&pcx_24) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 24-bit input, got {other:?}"),
    }

    // 1 bpp mono
    let bits = vec![0u8; w as usize * h as usize];
    let pcx_1 = encode_pcx_1bpp_mono(w, h, &bits).expect("encode");
    match parse_pcx_indexed_4bpp(&pcx_1) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1-bit mono, got {other:?}"),
    }

    // 8 bpp grayscale
    let pixels = vec![0u8; w as usize * h as usize];
    let pcx_8 = encode_pcx_8bpp_grayscale(w, h, &pixels).expect("encode");
    match parse_pcx_indexed_4bpp(&pcx_8) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 8-bit grayscale, got {other:?}"),
    }

    // 2 bpp CGA
    let two_bit_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let pcx_2 = encode_pcx_2bpp_cga(w, h, &two_bit_indices, 0, 0).expect("encode");
    match parse_pcx_indexed_4bpp(&pcx_2) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 2-bit CGA, got {other:?}"),
    }
}

/// Odd-width fixtures: spec §1 forces `bytes_per_line` up to an even
/// number, so the on-disk scanline can carry trailing padding beyond
/// the visible width. The typed view must strip the padding so the
/// index buffer is exactly `width × height`.
#[test]
fn strips_per_row_padding_for_odd_width() {
    // width = 5 → ceil(5/2) = 3 → rounds up to 4 bytes per line; the
    // on-disk row carries 8 nibbles but only the first 5 are visible.
    let (w, h) = (5u16, 3u16);
    let indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x0F) as u8)
        .collect();
    let palette = palette_48();
    let pcx = encode_pcx_4bpp_packed(w, h, &indices, &palette).expect("encode");

    // Sanity: the on-disk bytes_per_line is 4, not 3.
    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 4, "spec §1 rounds bytes_per_line up to even");

    let view = parse_pcx_indexed_4bpp(&pcx).expect("parse_pcx_indexed_4bpp");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(
        view.indices.len(),
        (w as usize) * (h as usize),
        "padding nibbles must be stripped"
    );
    assert_eq!(view.indices, indices);
}

/// A truncated header is rejected by both [`parse_pcx`] AND
/// [`parse_pcx_indexed_4bpp`] — confirms the typed accessor shares the
/// canonical validation surface (no skipped guard rails).
#[test]
fn shares_validation_surface_with_parse_pcx() {
    // Truncated below 128-byte header.
    let truncated = vec![0u8; 40];
    assert!(matches!(
        parse_pcx(&truncated),
        Err(PcxError::InvalidData(_))
    ));
    assert!(matches!(
        parse_pcx_indexed_4bpp(&truncated),
        Err(PcxError::InvalidData(_))
    ));

    // Bad manufacturer byte.
    let (w, h) = (8u16, 4u16);
    let indices = vec![0u8; w as usize * h as usize];
    let mut pcx = encode_pcx_4bpp_packed(w, h, &indices, &palette_48()).expect("encode");
    pcx[0] = 0xFF;
    assert!(matches!(parse_pcx(&pcx), Err(PcxError::InvalidData(_))));
    assert!(matches!(
        parse_pcx_indexed_4bpp(&pcx),
        Err(PcxError::InvalidData(_))
    ));
}
