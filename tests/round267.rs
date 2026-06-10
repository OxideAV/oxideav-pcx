//! r267 — typed 1 bpp × 3 planes 8-colour EGA RGB paletted accessor.
//!
//! Spec §4 describes the 8-colour EGA RGB PCX mode as three 1-bit planes
//! laid out one after another within each scanline (plane order R, G, B).
//! The three bits at the same x-position stack into a 3-bit colour index
//! (`r | g << 1 | b << 2`), and each plane bit toggles its channel
//! between `0x00` and `0xFF`. Unlike the 16-colour EGA / 256-colour VGA /
//! CGA modes, no on-disk palette is consulted — the eight colours are the
//! on/off primary combinations enumerated by the plane bits themselves.
//!
//! The pre-r267 [`oxideav_pcx::parse_pcx`] entry point always flattens
//! the on-disk image to packed `Rgba`, which is convenient for display
//! pipelines but discards the resolved 3-bit colour index. r267 adds
//! [`oxideav_pcx::parse_pcx_indexed_1bpp_3planes`] — the fifth (and
//! final) paletted typed view — which surfaces the `width × height`
//! resolved-index buffer (one byte per pixel, low three bits = colour
//! index `0..=7`, top-down) alongside the fixed 8-entry RGB palette and a
//! [`oxideav_pcx::Pcx1bpp3PlanesPaletteSource`] tag.
//!
//! This file tests:
//!
//! 1. Round-trip an [`oxideav_pcx::encode_pcx_1bpp_3planes_ega_rgb`]
//!    output through the new accessor and check indices / palette /
//!    source are surfaced exactly.
//! 2. The accessor consistency check: flattening the typed view's
//!    indices through its surfaced palette must reproduce the byte
//!    stream [`oxideav_pcx::parse_pcx`] produces — i.e. the typed view
//!    does NOT diverge from the canonical RGBA flattener.
//! 3. All eight colour indices `0..=7` are reachable (the palette covers
//!    the full on/off-primary cube).
//! 4. The accessor rejects every non-(1, 3) (depth, planes) combination
//!    with [`oxideav_pcx::PcxError::Unsupported`].
//! 5. Per-row padding (widths that don't fall on an 8-pixel byte
//!    boundary, where `bytes_per_line` is rounded up to even per spec §1)
//!    is stripped — the typed view surfaces exactly `width × height`
//!    indices.
//! 6. The accessor shares its validation surface with
//!    [`oxideav_pcx::parse_pcx`]: a malformed file rejected by one is
//!    rejected by the other with a matching error class.

use oxideav_pcx::{
    encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono,
    encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale,
    parse_pcx, parse_pcx_indexed_1bpp_3planes, Pcx1bpp3PlanesPaletteSource, PcxError,
    PcxIndexed1x3,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Fixed 8-entry on/off-primary palette the decoder resolves to. Entry
/// `i` has channel `c` set to `0xFF` iff the matching plane bit is set:
/// `r = i & 1`, `g = i & 2`, `b = i & 4`.
const PRIMARIES: [[u8; 3]; 8] = [
    [0x00, 0x00, 0x00],
    [0xFF, 0x00, 0x00],
    [0x00, 0xFF, 0x00],
    [0xFF, 0xFF, 0x00],
    [0x00, 0x00, 0xFF],
    [0xFF, 0x00, 0xFF],
    [0x00, 0xFF, 0xFF],
    [0xFF, 0xFF, 0xFF],
];

/// Build a `width × height × 3` RGB buffer whose every channel is already
/// `0x00` / `0xFF` (so the encoder's 0x80 threshold round-trips exactly)
/// from a 3-bit colour index per pixel. `idx_at(x, y)` chooses the index.
fn rgb_from_indices(w: usize, h: usize, idx_at: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let c = PRIMARIES[(idx_at(x, y) & 0x07) as usize];
            rgb.extend_from_slice(&c);
        }
    }
    rgb
}

/// Deterministic index grid that walks the full `0..=7` cube while
/// keeping bytes mixed enough to exercise the RLE encoder.
fn index_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x.wrapping_add(y * 3)) & 0x07) as u8);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip an `encode_pcx_1bpp_3planes_ega_rgb` output through the new
/// accessor and verify indices, palette, and source tag.
#[test]
fn ega_rgb_roundtrips_through_typed_accessor() {
    let (w, h) = (16usize, 8usize);
    let indices = index_grid(w, h);
    let rgb = rgb_from_indices(w, h, |x, y| indices[y * w + x]);

    let pcx = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).expect("encode");
    let view = parse_pcx_indexed_1bpp_3planes(&pcx).expect("parse_pcx_indexed_1bpp_3planes");

    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.indices, indices);
    assert_eq!(view.palette, PRIMARIES);
    assert_eq!(
        view.palette_source,
        Pcx1bpp3PlanesPaletteSource::FixedPrimaries
    );
    assert_eq!(view.stride(), w);
}

/// Flattening the typed view's indices through its surfaced palette must
/// produce the same RGBA bytes [`parse_pcx`] does. Pins the typed view as
/// a strict rearrangement (NOT a divergence) of the canonical flattener.
#[test]
fn typed_view_agrees_with_canonical_flattener() {
    let (w, h) = (24usize, 6usize);
    let indices = index_grid(w, h);
    let rgb = rgb_from_indices(w, h, |x, y| indices[y * w + x]);

    let pcx = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).expect("encode");
    let img = parse_pcx(&pcx).expect("parse_pcx");
    let view = parse_pcx_indexed_1bpp_3planes(&pcx).expect("parse_pcx_indexed_1bpp_3planes");

    assert_indices_flatten_to_rgba(&view, &img.data);
}

fn assert_indices_flatten_to_rgba(view: &PcxIndexed1x3, rgba: &[u8]) {
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

/// Every colour index `0..=7` must be reachable and resolve to its
/// matching on/off-primary RGB triplet. Lays a single row of the eight
/// indices and checks each surfaced palette entry.
#[test]
fn all_eight_indices_reachable() {
    let (w, h) = (8usize, 1usize);
    let indices: Vec<u8> = (0..8u8).collect();
    let rgb = rgb_from_indices(w, h, |x, _| indices[x]);

    let pcx = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).expect("encode");
    let view = parse_pcx_indexed_1bpp_3planes(&pcx).expect("parse_pcx_indexed_1bpp_3planes");

    assert_eq!(view.indices, indices);
    for (i, entry) in view.palette.iter().enumerate() {
        let expected = [
            if i & 1 != 0 { 0xFF } else { 0x00 },
            if i & 2 != 0 { 0xFF } else { 0x00 },
            if i & 4 != 0 { 0xFF } else { 0x00 },
        ];
        assert_eq!(*entry, expected, "palette entry {i} mismatch");
    }
}

/// Reject every non-(1, 3) depth/planes combination with
/// [`PcxError::Unsupported`] — the typed accessor's scope is the
/// 1 bpp × 3 planes EGA RGB path, not the other paletted / truecolour
/// modes.
#[test]
fn rejects_non_1bpp_3planes_inputs() {
    let (w, h) = (8u16, 4u16);

    // 24-bit RGB (8 bpp × 3 planes)
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let pcx_24 = encode_pcx_24bpp(w, h, &rgb).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_24) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 24-bit input, got {other:?}"),
    }

    // 1 bpp × 1 plane mono
    let bits = vec![0u8; w as usize * h as usize];
    let pcx_1 = encode_pcx_1bpp_mono(w, h, &bits).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_1) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1-bit mono, got {other:?}"),
    }

    // 8 bpp grayscale
    let pixels = vec![0u8; w as usize * h as usize];
    let pcx_8 = encode_pcx_8bpp_grayscale(w, h, &pixels).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_8) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 8-bit grayscale, got {other:?}"),
    }

    // 4 bpp × 1 plane
    let nib_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x0F) as u8)
        .collect();
    let palette: Vec<u8> = (0..48).map(|i| ((i + 1) & 0xFF) as u8).collect();
    let pcx_4 = encode_pcx_4bpp_packed(w, h, &nib_indices, &palette).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_4) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 4-bit packed, got {other:?}"),
    }

    // 1 bpp × 4 planes (16-colour EGA bit-plane)
    let pcx_1x4 = encode_pcx_1bpp_4planes_ega(w, h, &nib_indices, &palette).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_1x4) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 1bpp×4-planes input, got {other:?}"),
    }

    // 2 bpp × 1 plane CGA
    let cga_indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let pcx_cga = encode_pcx_2bpp_cga(w, h, &cga_indices, 0x80, 4).expect("encode");
    match parse_pcx_indexed_1bpp_3planes(&pcx_cga) {
        Err(PcxError::Unsupported(_)) => {}
        other => panic!("expected Unsupported for 2bpp CGA input, got {other:?}"),
    }
}

/// Widths that don't fall on an 8-pixel byte boundary: spec §1 forces
/// `bytes_per_line` up to an even number, so the on-disk scanline can
/// carry trailing padding bits beyond the visible width. The typed view
/// must strip the padding so the index buffer is exactly
/// `width × height`.
#[test]
fn strips_per_row_padding_for_non_byte_aligned_width() {
    // width = 13 → ceil(13/8) = 2 → rounded up to 2 (already even); the
    // on-disk row carries 16 1-bit slots per plane but only the first 13
    // are visible.
    let (w, h) = (13usize, 5usize);
    let indices = index_grid(w, h);
    let rgb = rgb_from_indices(w, h, |x, y| indices[y * w + x]);

    let pcx = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).expect("encode");

    // Sanity: the on-disk bytes_per_line is 2.
    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 2, "spec §1 rounds bytes_per_line up to even");

    let view = parse_pcx_indexed_1bpp_3planes(&pcx).expect("parse_pcx_indexed_1bpp_3planes");
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.indices.len(), w * h, "padding bits must be stripped");
    assert_eq!(view.indices, indices);
}

/// A truncated header is rejected by both [`parse_pcx`] AND
/// [`parse_pcx_indexed_1bpp_3planes`] — confirms the typed accessor
/// shares the canonical validation surface (no skipped guard rails).
#[test]
fn shares_validation_surface_with_parse_pcx() {
    // Truncated below 128-byte header.
    let truncated = vec![0u8; 40];
    assert!(matches!(
        parse_pcx(&truncated),
        Err(PcxError::InvalidData(_))
    ));
    assert!(matches!(
        parse_pcx_indexed_1bpp_3planes(&truncated),
        Err(PcxError::InvalidData(_))
    ));

    // Bad manufacturer byte.
    let (w, h) = (8usize, 4usize);
    let indices = index_grid(w, h);
    let rgb = rgb_from_indices(w, h, |x, y| indices[y * w + x]);
    let mut pcx = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).expect("encode");
    pcx[0] = 0xFF;
    assert!(matches!(parse_pcx(&pcx), Err(PcxError::InvalidData(_))));
    assert!(matches!(
        parse_pcx_indexed_1bpp_3planes(&pcx),
        Err(PcxError::InvalidData(_))
    ));
}
