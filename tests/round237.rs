//! r237 — typed 8 bpp × 1 plane paletted accessor.
//!
//! Spec §4.1 ("256 colour") + §3 ("Palette Information") describe an
//! 8 bpp × 1 plane PCX as either:
//!
//! * a `palette_info = 2` grayscale (no tail palette honoured per spec
//!   §3 — the flag forces the synthetic 0..=255 ramp interpretation),
//! * a colour image with a 256-entry VGA palette appended after the
//!   pixel block (spec §3 records 769 bytes from EOF starting with
//!   marker `0x0C`, followed by 768 RGB bytes), or
//! * neither (no flag, no tail block) — older 8 bpp PCX files that
//!   omit colour information entirely.
//!
//! The pre-r237 [`oxideav_pcx::parse_pcx`] entry point always flattens
//! the on-disk image to packed `Rgba`, which is convenient for display
//! pipelines but discards the palette indices the file actually
//! carries. r237 adds [`oxideav_pcx::parse_pcx_indexed_8bpp`] — a typed
//! accessor that surfaces the `width × height` index buffer (one byte
//! per pixel, top-down) alongside the resolved 256-entry RGB palette
//! and a [`oxideav_pcx::PcxPaletteSource`] tag recording which spec §3
//! branch produced it.
//!
//! This file tests:
//!
//! 1. Round-trip an [`oxideav_pcx::encode_pcx_8bpp_indexed`] output
//!    through the new accessor and check indices / palette /
//!    `PaletteSource::VgaTail` are surfaced exactly.
//! 2. Same for [`oxideav_pcx::encode_pcx_8bpp_grayscale`] —
//!    `PaletteSource::GrayscaleFlag` + synthetic ramp.
//! 3. A hand-built 8 bpp PCX with neither flag nor tail block surfaces
//!    `PaletteSource::GrayscaleFallback` + the synthetic ramp (and the
//!    indices match the input bytes 1:1).
//! 4. The accessor consistency check: for every fixture the indices
//!    flattened through the surfaced palette match the byte stream
//!    [`oxideav_pcx::parse_pcx`] produces — i.e. the typed view does
//!    NOT diverge from the canonical RGBA flattener.
//! 5. The accessor rejects every non-(8,1) (depth, planes) combination
//!    with [`oxideav_pcx::PcxError::Unsupported`].
//! 6. Per-row padding (odd-width fixtures where `bytes_per_line` is
//!    rounded up to even per spec §1) is stripped — the typed view
//!    surfaces exactly `width × height` indices, not `bytes_per_line ×
//!    height`.
//! 7. The accessor shares its validation surface with
//!    [`oxideav_pcx::parse_pcx`]: a malformed file rejected by one is
//!    rejected by the other with a matching error class.

use oxideav_pcx::{
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_packed,
    encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed, parse_pcx, parse_pcx_indexed_8bpp,
    PcxError, PcxIndexed8, PcxPaletteSource, PcxPixelFormat,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Deterministic 8-bit index pattern: alternating diagonal stripes.
fn indices_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x.wrapping_mul(17) ^ y.wrapping_mul(7)) & 0xFF) as u8);
        }
    }
    out
}

/// Synthetic 256-entry RGB palette (768 bytes) used by the VGA-tail
/// round-trip test.
fn palette_768() -> Vec<u8> {
    let mut p = Vec::with_capacity(768);
    for i in 0..256u16 {
        let r = (i & 0xFF) as u8;
        let g = ((i.wrapping_mul(5)) & 0xFF) as u8;
        let b = ((i.wrapping_mul(13)) & 0xFF) as u8;
        p.push(r);
        p.push(g);
        p.push(b);
    }
    p
}

/// Hand-built 8 bpp × 1 plane PCX with no `palette_info = 2` flag and
/// no VGA tail block — exercises the `GrayscaleFallback` source.
fn build_8bpp_no_palette(w: u16, h: u16, indices: &[u8]) -> Vec<u8> {
    assert_eq!(indices.len(), w as usize * h as usize);
    // bytes_per_line rounded up to even per spec §1
    let bpl = ((w as usize) + 1) & !1;
    let mut out = Vec::with_capacity(128 + bpl * h as usize * 2);
    out.resize(128, 0);
    out[0] = 0x0A; // manufacturer
    out[1] = 5; // version 5
    out[2] = 1; // RLE
    out[3] = 8; // bits_per_pixel
    let x_max = w - 1;
    let y_max = h - 1;
    out[8..10].copy_from_slice(&x_max.to_le_bytes());
    out[10..12].copy_from_slice(&y_max.to_le_bytes());
    // h_dpi / v_dpi at offsets 12 / 14 stay at zero per the default.
    out[65] = 1; // n_planes
    out[66..68].copy_from_slice(&(bpl as u16).to_le_bytes());
    // palette_info left at 0 (NOT 2 — so the grayscale flag does NOT
    // apply); no VGA tail block will follow either, so the fallback
    // ramp is the only deterministic palette the decoder can use.
    // RLE-encode each scanline. The fixture indices are intentionally
    // chosen to include byte values ≥ 0xC0 so the escape path is
    // exercised, and the encoder coalesces identical neighbours.
    for y in 0..h as usize {
        let row = &indices[y * w as usize..(y + 1) * w as usize];
        // Pad to bytes_per_line with zeros.
        let mut padded = row.to_vec();
        padded.resize(bpl, 0);
        rle_encode_into(&padded, &mut out);
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

/// Round-trip an [`encode_pcx_8bpp_indexed`] output through
/// [`parse_pcx_indexed_8bpp`] and verify all three components.
#[test]
fn vga_tail_roundtrip_surfaces_indices_and_palette() {
    let (w, h) = (32u16, 16u16);
    let indices = indices_grid(w as usize, h as usize);
    let palette = palette_768();
    let pcx = encode_pcx_8bpp_indexed(w, h, &indices, &palette).expect("encode");
    let view = parse_pcx_indexed_8bpp(&pcx).expect("parse_pcx_indexed_8bpp");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.indices, indices);
    assert_eq!(view.palette_source, PcxPaletteSource::VgaTail);
    // Palette must match the encoded triplets entry-for-entry.
    for i in 0..256 {
        assert_eq!(
            view.palette[i],
            [palette[i * 3], palette[i * 3 + 1], palette[i * 3 + 2]],
            "palette entry {i}"
        );
    }
    assert_eq!(view.stride(), w as usize);
}

/// A `palette_info = 2` grayscale PCX surfaces the flag-driven palette
/// source plus the synthetic `0..=255` ramp.
#[test]
fn grayscale_flag_surfaces_synthetic_ramp() {
    let (w, h) = (16u16, 8u16);
    let pixels: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0xFF) as u8)
        .collect();
    let pcx = encode_pcx_8bpp_grayscale(w, h, &pixels).expect("encode");
    let view = parse_pcx_indexed_8bpp(&pcx).expect("parse_pcx_indexed_8bpp");
    assert_eq!(view.indices, pixels);
    assert_eq!(view.palette_source, PcxPaletteSource::GrayscaleFlag);
    for i in 0..256 {
        let v = i as u8;
        assert_eq!(view.palette[i], [v, v, v]);
    }
}

/// An 8 bpp PCX with neither `palette_info = 2` nor a VGA tail block
/// surfaces `GrayscaleFallback` + the synthetic ramp.
#[test]
fn no_palette_surfaces_fallback_source() {
    let (w, h) = (24u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);
    let pcx = build_8bpp_no_palette(w, h, &indices);
    let view = parse_pcx_indexed_8bpp(&pcx).expect("parse_pcx_indexed_8bpp");
    assert_eq!(view.indices, indices);
    assert_eq!(view.palette_source, PcxPaletteSource::GrayscaleFallback);
    for i in 0..256 {
        let v = i as u8;
        assert_eq!(view.palette[i], [v, v, v]);
    }
}

/// For every fixture, flattening the typed view's indices through its
/// surfaced palette must produce the same RGBA bytes
/// [`parse_pcx`] does. This pins the typed view as a strict
/// rearrangement (NOT a divergence) of the canonical flattener.
#[test]
fn typed_view_agrees_with_canonical_flattener() {
    let (w, h) = (40u16, 12u16);
    let indices = indices_grid(w as usize, h as usize);
    let palette = palette_768();

    // VGA-tail case.
    let pcx_vga = encode_pcx_8bpp_indexed(w, h, &indices, &palette).expect("encode");
    let img = parse_pcx(&pcx_vga).expect("parse_pcx");
    let view = parse_pcx_indexed_8bpp(&pcx_vga).expect("parse_pcx_indexed_8bpp");
    assert_indices_flatten_to_rgba(&view, &img.data);

    // Grayscale-flag case.
    let pcx_gs =
        encode_pcx_8bpp_grayscale(w, h, &indices_grid(w as usize, h as usize)).expect("encode");
    let img_gs = parse_pcx(&pcx_gs).expect("parse_pcx");
    let view_gs = parse_pcx_indexed_8bpp(&pcx_gs).expect("parse_pcx_indexed_8bpp");
    assert_indices_flatten_to_rgba(&view_gs, &img_gs.data);

    // Fallback case.
    let pcx_fb = build_8bpp_no_palette(w, h, &indices);
    let img_fb = parse_pcx(&pcx_fb).expect("parse_pcx");
    let view_fb = parse_pcx_indexed_8bpp(&pcx_fb).expect("parse_pcx_indexed_8bpp");
    assert_indices_flatten_to_rgba(&view_fb, &img_fb.data);
}

fn assert_indices_flatten_to_rgba(view: &PcxIndexed8, rgba: &[u8]) {
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

/// Reject every non-(8, 1) depth/planes combination with
/// [`PcxError::Unsupported`] — the typed accessor's scope is the 8 bpp
/// × 1 plane VGA path, not the 24-bit or EGA/CGA paths.
#[test]
fn rejects_non_8bpp_single_plane_inputs() {
    let (w, h) = (8u16, 4u16);
    // 24-bit RGB
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let pcx_24 = encode_pcx_24bpp(w, h, &rgb).expect("encode");
    match parse_pcx_indexed_8bpp(&pcx_24) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 24-bit input, got {other:?}"),
    }

    // 1 bpp mono — encode_pcx_1bpp_mono takes one byte per pixel (the
    // writer packs them into the bit-plane stride on its own).
    let bits = vec![0u8; w as usize * h as usize];
    let pcx_1 = encode_pcx_1bpp_mono(w, h, &bits).expect("encode");
    match parse_pcx_indexed_8bpp(&pcx_1) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1-bit mono, got {other:?}"),
    }

    // 4 bpp packed
    let four_bit_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x0F) as u8)
        .collect();
    let ega = vec![0u8; 48];
    let pcx_4 = encode_pcx_4bpp_packed(w, h, &four_bit_indices, &ega).expect("encode");
    match parse_pcx_indexed_8bpp(&pcx_4) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 4-bit packed, got {other:?}"),
    }

    // 2 bpp CGA
    let two_bit_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let pcx_2 = encode_pcx_2bpp_cga(w, h, &two_bit_indices, 0, 0).expect("encode");
    match parse_pcx_indexed_8bpp(&pcx_2) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 2-bit CGA, got {other:?}"),
    }
}

/// Odd-width fixtures: spec §1 forces `bytes_per_line` up to an even
/// number, so the on-disk scanline is one byte wider than the visible
/// width. The typed view must strip the padding so the index buffer is
/// exactly `width × height`, not `bytes_per_line × height`.
#[test]
fn strips_per_row_padding_for_odd_width() {
    let (w, h) = (5u16, 3u16); // odd width — bpl rounds up to 6
    let indices: Vec<u8> = (0..(w as usize * h as usize)).map(|i| i as u8).collect();
    let palette = palette_768();
    let pcx = encode_pcx_8bpp_indexed(w, h, &indices, &palette).expect("encode");

    // Sanity: the on-disk bytes_per_line is 6, not 5.
    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 6, "spec §1 rounds bytes_per_line up to even");

    let view = parse_pcx_indexed_8bpp(&pcx).expect("parse_pcx_indexed_8bpp");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(
        view.indices.len(),
        (w as usize) * (h as usize),
        "padding byte must be stripped"
    );
    assert_eq!(view.indices, indices);
}

/// A truncated header is rejected by both [`parse_pcx`] AND
/// [`parse_pcx_indexed_8bpp`] — confirms the typed accessor shares the
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
        parse_pcx_indexed_8bpp(&truncated),
        Err(PcxError::InvalidData(_))
    ));

    // Bad manufacturer byte.
    let (w, h) = (8u16, 4u16);
    let mut pcx =
        encode_pcx_8bpp_indexed(w, h, &vec![0u8; w as usize * h as usize], &palette_768())
            .expect("encode");
    pcx[0] = 0xFF;
    assert!(matches!(parse_pcx(&pcx), Err(PcxError::InvalidData(_))));
    assert!(matches!(
        parse_pcx_indexed_8bpp(&pcx),
        Err(PcxError::InvalidData(_))
    ));
}

/// Sanity: [`PcxImage::pixel_format`] for a parsed file is still
/// [`PcxPixelFormat::Rgba`] (the canonical entry point hasn't changed
/// shape) — the typed accessor is purely additive.
#[test]
fn parse_pcx_pixel_format_unchanged() {
    let (w, h) = (10u16, 5u16);
    let pcx = encode_pcx_8bpp_indexed(w, h, &vec![42u8; w as usize * h as usize], &palette_768())
        .expect("encode");
    let img = parse_pcx(&pcx).expect("parse_pcx");
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
}
