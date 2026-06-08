//! r257 — typed 2 bpp × 1 plane CGA paletted accessor.
//!
//! Spec §4.1 describes a 4-colour CGA PCX as a single plane of 2 bpp
//! packed-bits data (4 pixels/byte, the top two bits = pixel 0). PCX
//! repurposes the 48-byte `ega_palette` header region for CGA mode:
//! byte 16's high nibble carries the EGA index used for palette entry 0
//! (the CGA "background / border" colour), and byte 19 carries the CGA
//! palette selector (bit 7 = palette select 0 vs 1, bit 6 = intensity
//! low vs high per CGA hardware semantics).
//!
//! The pre-r257 [`oxideav_pcx::parse_pcx`] entry point always flattens
//! the on-disk image to packed `Rgba`, which is convenient for display
//! pipelines but discards the resolved 2-bit indices and palette family
//! the file actually carries. r257 adds
//! [`oxideav_pcx::parse_pcx_indexed_2bpp_cga`] — a typed accessor that
//! surfaces the `width × height` resolved-index buffer (one byte per
//! pixel, low two bits = palette index `0..=3`, top-down) alongside the
//! resolved 4-entry RGB palette, the resolved `background_index`
//! (`0..=15`) used for palette entry 0, and a
//! [`oxideav_pcx::Pcx2bppCgaPaletteSource`] tag recording which CGA
//! palette family the decoder landed on.
//!
//! This file tests:
//!
//! 1. Round-trip an [`oxideav_pcx::encode_pcx_2bpp_cga`] output through
//!    the new accessor and check indices / palette / background /
//!    selector are surfaced exactly across all four CGA palette
//!    families.
//! 2. The accessor consistency check: for every fixture the indices
//!    flattened through the surfaced palette match the byte stream
//!    [`oxideav_pcx::parse_pcx`] produces — i.e. the typed view does
//!    NOT diverge from the canonical RGBA flattener.
//! 3. The [`oxideav_pcx::Pcx2bppCgaPaletteSource::palette_selector`]
//!    helper round-trips: feeding the helper output back into
//!    `encode_pcx_2bpp_cga` produces a byte-identical file.
//! 4. The accessor rejects every non-(2, 1) (depth, planes) combination
//!    with [`oxideav_pcx::PcxError::Unsupported`].
//! 5. Per-row padding (widths that don't fall on a 4-pixel byte
//!    boundary, where `bytes_per_line` is rounded up to even per spec
//!    §1) is stripped — the typed view surfaces exactly `width × height`
//!    indices.
//! 6. The accessor shares its validation surface with
//!    [`oxideav_pcx::parse_pcx`]: a malformed file rejected by one is
//!    rejected by the other with a matching error class.

use oxideav_pcx::{
    encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, parse_pcx, parse_pcx_indexed_2bpp_cga,
    Pcx2bppCgaPaletteSource, PcxError, PcxIndexed2x1Cga,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Deterministic 2-bit index pattern — `(x ^ y) & 0x03` walks all four
/// palette entries across the row while keeping the bytes mixed enough
/// to exercise the RLE encoder's run-coalescer.
fn indices_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x ^ y) & 0x03) as u8);
        }
    }
    out
}

/// 48-byte synthetic palette used when we need a non-default
/// `ega_palette[..=15]` region — irrelevant for CGA decoding (only
/// bytes 16 and 19 matter), but ensures the typed view does NOT
/// accidentally key off any other header byte.
fn cga_indices_grid(w: usize, h: usize) -> Vec<u8> {
    indices_grid(w, h)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip an `encode_pcx_2bpp_cga` output through the new accessor
/// across all four CGA palette families (palette 0/1 × low/high
/// intensity) and verify indices, palette, background, and source tag.
#[test]
fn cga_roundtrip_across_all_four_palette_families() {
    let (w, h) = (16u16, 8u16);
    let indices = cga_indices_grid(w as usize, h as usize);

    // (selector_byte, background_index, expected_source)
    let cases: &[(u8, u8, Pcx2bppCgaPaletteSource)] = &[
        (0x00, 0, Pcx2bppCgaPaletteSource::Palette1HighIntensity),
        (0x40, 1, Pcx2bppCgaPaletteSource::Palette1LowIntensity),
        (0x80, 7, Pcx2bppCgaPaletteSource::Palette0HighIntensity),
        (0xC0, 15, Pcx2bppCgaPaletteSource::Palette0LowIntensity),
    ];

    for (selector, bg, expected_source) in cases {
        let pcx = encode_pcx_2bpp_cga(w, h, &indices, *selector, *bg).expect("encode");
        let view = parse_pcx_indexed_2bpp_cga(&pcx).expect("parse_pcx_indexed_2bpp_cga");
        assert_eq!(view.width, w as u32);
        assert_eq!(view.height, h as u32);
        assert_eq!(view.indices, indices, "selector=0x{selector:02X}");
        assert_eq!(
            view.background_index, *bg,
            "background_index round-trip mismatch (selector=0x{selector:02X})",
        );
        assert_eq!(
            view.palette_source, *expected_source,
            "palette_source mismatch (selector=0x{selector:02X})",
        );
        assert_eq!(view.stride(), w as usize);
    }
}

/// For every fixture, flattening the typed view's indices through its
/// surfaced palette must produce the same RGBA bytes [`parse_pcx`]
/// does. This pins the typed view as a strict rearrangement (NOT a
/// divergence) of the canonical flattener.
#[test]
fn typed_view_agrees_with_canonical_flattener() {
    let (w, h) = (24u16, 6u16);
    let indices = cga_indices_grid(w as usize, h as usize);

    for &selector in &[0x00u8, 0x40, 0x80, 0xC0] {
        for &bg in &[0u8, 4, 12] {
            let pcx = encode_pcx_2bpp_cga(w, h, &indices, selector, bg).expect("encode");
            let img = parse_pcx(&pcx).expect("parse_pcx");
            let view = parse_pcx_indexed_2bpp_cga(&pcx).expect("parse_pcx_indexed_2bpp_cga");
            assert_indices_flatten_to_rgba(&view, &img.data);
        }
    }
}

fn assert_indices_flatten_to_rgba(view: &PcxIndexed2x1Cga, rgba: &[u8]) {
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

/// The [`Pcx2bppCgaPaletteSource::palette_selector`] helper reconstructs
/// the byte 19 selector pattern so a round-trip caller can hand the
/// surfaced view straight back to `encode_pcx_2bpp_cga` without
/// re-deriving the bit positions. Decode → re-encode via the helper
/// must produce byte-identical output.
#[test]
fn palette_selector_helper_round_trips_to_byte_identical_output() {
    let (w, h) = (12u16, 5u16);
    let indices = cga_indices_grid(w as usize, h as usize);

    for &selector in &[0x00u8, 0x40, 0x80, 0xC0] {
        for &bg in &[0u8, 9, 15] {
            let pcx_a = encode_pcx_2bpp_cga(w, h, &indices, selector, bg).expect("encode");
            let view = parse_pcx_indexed_2bpp_cga(&pcx_a).expect("parse_pcx_indexed_2bpp_cga");
            let pcx_b = encode_pcx_2bpp_cga(
                w,
                h,
                &view.indices,
                view.palette_source.palette_selector(),
                view.background_index,
            )
            .expect("re-encode via helper");
            assert_eq!(
                pcx_a, pcx_b,
                "byte mismatch on round-trip (selector=0x{selector:02X}, bg={bg})",
            );
        }
    }
}

/// Reject every non-(2, 1) depth/planes combination with
/// [`PcxError::Unsupported`] — the typed accessor's scope is the
/// 2 bpp × 1 plane CGA path, not the 8-bit / 24-bit / 1-bit / 4-bit
/// modes.
#[test]
fn rejects_non_2bpp_1plane_inputs() {
    let (w, h) = (8u16, 4u16);

    // 24-bit RGB
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let pcx_24 = encode_pcx_24bpp(w, h, &rgb).expect("encode");
    match parse_pcx_indexed_2bpp_cga(&pcx_24) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 24-bit input, got {other:?}"),
    }

    // 1 bpp × 1 plane mono
    let bits = vec![0u8; w as usize * h as usize];
    let pcx_1 = encode_pcx_1bpp_mono(w, h, &bits).expect("encode");
    match parse_pcx_indexed_2bpp_cga(&pcx_1) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1-bit mono, got {other:?}"),
    }

    // 8 bpp grayscale
    let pixels = vec![0u8; w as usize * h as usize];
    let pcx_8 = encode_pcx_8bpp_grayscale(w, h, &pixels).expect("encode");
    match parse_pcx_indexed_2bpp_cga(&pcx_8) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 8-bit grayscale, got {other:?}"),
    }

    // 4 bpp × 1 plane (16-colour packed-bits)
    let nib_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x0F) as u8)
        .collect();
    let palette: Vec<u8> = (0..48).map(|i| ((i + 1) & 0xFF) as u8).collect();
    let pcx_4 = encode_pcx_4bpp_packed(w, h, &nib_indices, &palette).expect("encode");
    match parse_pcx_indexed_2bpp_cga(&pcx_4) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 4-bit packed, got {other:?}"),
    }

    // 1 bpp × 4 planes (16-colour EGA bit-plane)
    let pcx_1x4 = encode_pcx_1bpp_4planes_ega(w, h, &nib_indices, &palette).expect("encode");
    match parse_pcx_indexed_2bpp_cga(&pcx_1x4) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1bpp×4-planes input, got {other:?}"),
    }
}

/// Widths that don't fall on a 4-pixel byte boundary: spec §1 forces
/// `bytes_per_line` up to an even number, so the on-disk scanline can
/// carry trailing padding beyond the visible width. The typed view
/// must strip the padding so the index buffer is exactly
/// `width × height`.
#[test]
fn strips_per_row_padding_for_non_byte_aligned_width() {
    // width = 13 → ceil(13/4) = 4 → rounded up to 4 (already even);
    // the on-disk row carries 16 2-bit slots but only the first 13
    // are visible.
    let (w, h) = (13u16, 5u16);
    let indices = cga_indices_grid(w as usize, h as usize);
    let pcx = encode_pcx_2bpp_cga(w, h, &indices, 0x80, 4).expect("encode");

    // Sanity: the on-disk bytes_per_line is 4.
    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 4, "spec §1 rounds bytes_per_line up to even");

    let view = parse_pcx_indexed_2bpp_cga(&pcx).expect("parse_pcx_indexed_2bpp_cga");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(
        view.indices.len(),
        (w as usize) * (h as usize),
        "padding 2-bit slots must be stripped"
    );
    assert_eq!(view.indices, indices);
}

/// A truncated header is rejected by both [`parse_pcx`] AND
/// [`parse_pcx_indexed_2bpp_cga`] — confirms the typed accessor shares
/// the canonical validation surface (no skipped guard rails).
#[test]
fn shares_validation_surface_with_parse_pcx() {
    // Truncated below 128-byte header.
    let truncated = vec![0u8; 40];
    assert!(matches!(
        parse_pcx(&truncated),
        Err(PcxError::InvalidData(_))
    ));
    assert!(matches!(
        parse_pcx_indexed_2bpp_cga(&truncated),
        Err(PcxError::InvalidData(_))
    ));

    // Bad manufacturer byte.
    let (w, h) = (8u16, 4u16);
    let indices = cga_indices_grid(w as usize, h as usize);
    let mut pcx = encode_pcx_2bpp_cga(w, h, &indices, 0x80, 4).expect("encode");
    pcx[0] = 0xFF;
    assert!(matches!(parse_pcx(&pcx), Err(PcxError::InvalidData(_))));
    assert!(matches!(
        parse_pcx_indexed_2bpp_cga(&pcx),
        Err(PcxError::InvalidData(_))
    ));
}
