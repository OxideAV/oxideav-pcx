//! r295 — authoring DPI override for the EGA / CGA palette-mode writers.
//!
//! Spec §3 records the header's `h_dpi` / `v_dpi` words (offsets 12 / 14,
//! both 16-bit little-endian) as "the resolutions at which the image was
//! created (printer or scanner); e.g. a scan might store 300, 300". The
//! field is a **format-independent** header word: it describes the
//! authoring device, not the pixel encoding, so a 16-colour EGA or a
//! 4-colour CGA image scanned at 300 × 300 is exactly as spec-conformant
//! as a 24-bit one.
//!
//! r219 added a `*_dpi` writer suite, but only for the four formats a
//! consumer typically scans into PCX (24-bit, 8 bpp indexed, 8 bpp
//! grayscale, 1 bpp mono). The EGA / CGA / packed-bits palette writers
//! still hard-coded the historical 72×72 PC Paintbrush "screen DPI"
//! default, so a decode → re-encode of a scanned 16-colour file silently
//! flattened its authoring resolution to 72×72.
//!
//! r295 closes that gap with four new writers —
//! [`oxideav_pcx::encode_pcx_4bpp_packed_dpi`],
//! [`oxideav_pcx::encode_pcx_2bpp_cga_dpi`],
//! [`oxideav_pcx::encode_pcx_1bpp_3planes_ega_rgb_dpi`], and
//! [`oxideav_pcx::encode_pcx_1bpp_4planes_ega_dpi`] — each mirroring its
//! non-DPI sibling byte-for-byte except for the header `h_dpi` / `v_dpi`
//! words. The pixel-data region is unchanged, so the decode path and the
//! typed paletted accessors keep producing identical output.
//!
//! This file tests:
//!
//! 1. The DPI pair lands at header offsets 12 / 14, and the legacy
//!    writers still emit the documented 72×72 default.
//! 2. The decoder surfaces the stamped DPI on [`oxideav_pcx::PcxImage::dpi`].
//! 3. Apart from the DPI words, the `_dpi` output is byte-identical to
//!    the non-DPI writer's output (proving the pixel region is untouched).
//! 4. A 0 in either DPI component is rejected at the writer boundary
//!    (the spec §3 "0 = unset" sentinel — emitting a redundant zero pair
//!    would be indistinguishable from the plain writer).
//! 5. The pixels round-trip through the matching typed paletted accessor
//!    unchanged.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_3planes_ega_rgb_dpi,
    encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_4planes_ega_dpi, encode_pcx_2bpp_cga,
    encode_pcx_2bpp_cga_dpi, encode_pcx_4bpp_packed, encode_pcx_4bpp_packed_dpi, parse_pcx,
    parse_pcx_indexed_1bpp_3planes, parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga,
    parse_pcx_indexed_4bpp, PcxError,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// 48-byte (16 RGB triplets) EGA palette with deterministic, non-zero
/// bytes so the in-header palette path is exercised.
fn ega16_palette() -> Vec<u8> {
    (0..48u32).map(|i| ((i + 1) & 0xFF) as u8).collect()
}

/// 16-colour nibble index grid that visits every `0..=15` index and stays
/// mixed enough to exercise the RLE encoder.
fn nibble_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x.wrapping_add(y * 3)) & 0x0F) as u8);
        }
    }
    out
}

/// 8-colour EGA RGB buffer (every channel already 0x00 / 0xFF so the
/// 0x80 threshold round-trips exactly) from a 3-bit index per pixel.
fn ega_rgb(w: usize, h: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let idx = ((x.wrapping_add(y)) & 0x07) as u8;
            rgb.push(if idx & 1 != 0 { 0xFF } else { 0x00 });
            rgb.push(if idx & 2 != 0 { 0xFF } else { 0x00 });
            rgb.push(if idx & 4 != 0 { 0xFF } else { 0x00 });
        }
    }
    rgb
}

const DPI: (u16, u16) = (300, 600);

// ---------------------------------------------------------------------------
// Header DPI placement + legacy default
// ---------------------------------------------------------------------------

/// Each new `_dpi` writer stamps the requested pair at header offsets
/// 12 / 14, and the matching legacy writer keeps the documented 72×72
/// default.
#[test]
fn dpi_lands_at_header_offsets_12_14() {
    let (w, h) = (12u16, 6u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let cga: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let rgb = ega_rgb(w as usize, h as usize);

    let cases: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (
            encode_pcx_4bpp_packed(w, h, &nibs, &pal).unwrap(),
            encode_pcx_4bpp_packed_dpi(w, h, &nibs, &pal, DPI).unwrap(),
        ),
        (
            encode_pcx_2bpp_cga(w, h, &cga, 0x80, 4).unwrap(),
            encode_pcx_2bpp_cga_dpi(w, h, &cga, 0x80, 4, DPI).unwrap(),
        ),
        (
            encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap(),
            encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, DPI).unwrap(),
        ),
        (
            encode_pcx_1bpp_4planes_ega(w, h, &nibs, &pal).unwrap(),
            encode_pcx_1bpp_4planes_ega_dpi(w, h, &nibs, &pal, DPI).unwrap(),
        ),
    ];

    for (default, dpi) in &cases {
        assert_eq!(read_u16_le(default, 12), 72, "legacy h_dpi default");
        assert_eq!(read_u16_le(default, 14), 72, "legacy v_dpi default");
        assert_eq!(read_u16_le(dpi, 12), DPI.0, "stamped h_dpi");
        assert_eq!(read_u16_le(dpi, 14), DPI.1, "stamped v_dpi");
    }
}

// ---------------------------------------------------------------------------
// Only the DPI words differ
// ---------------------------------------------------------------------------

/// The `_dpi` output must be byte-identical to the non-DPI output apart
/// from the four DPI bytes at offsets 12..16, proving the pixel-data
/// region (and the rest of the header) is left untouched.
#[test]
fn only_dpi_words_differ_from_legacy_writer() {
    let (w, h) = (10u16, 5u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let cga: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let rgb = ega_rgb(w as usize, h as usize);

    let pairs: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (
            encode_pcx_4bpp_packed(w, h, &nibs, &pal).unwrap(),
            encode_pcx_4bpp_packed_dpi(w, h, &nibs, &pal, DPI).unwrap(),
        ),
        (
            encode_pcx_2bpp_cga(w, h, &cga, 0x40, 7).unwrap(),
            encode_pcx_2bpp_cga_dpi(w, h, &cga, 0x40, 7, DPI).unwrap(),
        ),
        (
            encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap(),
            encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, DPI).unwrap(),
        ),
        (
            encode_pcx_1bpp_4planes_ega(w, h, &nibs, &pal).unwrap(),
            encode_pcx_1bpp_4planes_ega_dpi(w, h, &nibs, &pal, DPI).unwrap(),
        ),
    ];

    for (default, dpi) in &pairs {
        assert_eq!(default.len(), dpi.len(), "length must match");
        assert!(default.len() > PCX_HEADER_SIZE);
        for (i, (a, b)) in default.iter().zip(dpi.iter()).enumerate() {
            if (12..16).contains(&i) {
                continue; // DPI words are allowed to differ.
            }
            assert_eq!(a, b, "byte {i} must be identical outside the DPI words");
        }
    }
}

// ---------------------------------------------------------------------------
// Decode surfaces the stamped DPI
// ---------------------------------------------------------------------------

/// The decoder reports the stamped DPI on `PcxImage::dpi` for all four
/// palette modes.
#[test]
fn decoder_surfaces_stamped_dpi() {
    let (w, h) = (16u16, 8u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let cga: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let rgb = ega_rgb(w as usize, h as usize);

    let files = [
        encode_pcx_4bpp_packed_dpi(w, h, &nibs, &pal, DPI).unwrap(),
        encode_pcx_2bpp_cga_dpi(w, h, &cga, 0x00, 0, DPI).unwrap(),
        encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, DPI).unwrap(),
        encode_pcx_1bpp_4planes_ega_dpi(w, h, &nibs, &pal, DPI).unwrap(),
    ];

    for (i, f) in files.iter().enumerate() {
        let img = parse_pcx(f).unwrap_or_else(|e| panic!("decode file {i} failed: {e}"));
        assert_eq!(img.dpi, Some(DPI), "file {i} DPI not surfaced");
        // The legacy writer surfaces the 72×72 default.
    }
}

/// The legacy writers surface the documented 72×72 default on decode.
#[test]
fn legacy_writers_surface_72_default() {
    let (w, h) = (8u16, 4u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let img = parse_pcx(&encode_pcx_4bpp_packed(w, h, &nibs, &pal).unwrap()).unwrap();
    assert_eq!(img.dpi, Some((72, 72)));
}

// ---------------------------------------------------------------------------
// Zero-component rejection (spec §3 "0 = unset" sentinel)
// ---------------------------------------------------------------------------

/// A 0 in either DPI component is rejected up front for every new writer,
/// matching the spec §3 sentinel rule the r219 writers already enforce.
#[test]
fn rejects_zero_dpi_component() {
    let (w, h) = (8u16, 4u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let cga: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let rgb = ega_rgb(w as usize, h as usize);

    for bad in [(0, 300), (300, 0), (0, 0)] {
        assert!(matches!(
            encode_pcx_4bpp_packed_dpi(w, h, &nibs, &pal, bad),
            Err(PcxError::InvalidData(_))
        ));
        assert!(matches!(
            encode_pcx_2bpp_cga_dpi(w, h, &cga, 0x80, 4, bad),
            Err(PcxError::InvalidData(_))
        ));
        assert!(matches!(
            encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, bad),
            Err(PcxError::InvalidData(_))
        ));
        assert!(matches!(
            encode_pcx_1bpp_4planes_ega_dpi(w, h, &nibs, &pal, bad),
            Err(PcxError::InvalidData(_))
        ));
    }
}

// ---------------------------------------------------------------------------
// Pixels round-trip unchanged through the typed accessors
// ---------------------------------------------------------------------------

/// Stamping a DPI must not perturb the pixel data: the typed paletted
/// accessor surfaces the same indices the non-DPI writer would produce.
#[test]
fn pixels_roundtrip_through_typed_accessors() {
    let (w, h) = (13u16, 7u16);
    let pal = ega16_palette();
    let nibs = nibble_grid(w as usize, h as usize);
    let cga: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i & 0x03) as u8)
        .collect();
    let rgb = ega_rgb(w as usize, h as usize);

    // 4 bpp × 1 plane
    let v_dpi =
        parse_pcx_indexed_4bpp(&encode_pcx_4bpp_packed_dpi(w, h, &nibs, &pal, DPI).unwrap())
            .unwrap();
    let v_plain =
        parse_pcx_indexed_4bpp(&encode_pcx_4bpp_packed(w, h, &nibs, &pal).unwrap()).unwrap();
    assert_eq!(v_dpi.indices, v_plain.indices);
    assert_eq!(v_dpi.indices, nibs);

    // 2 bpp × 1 plane CGA
    let c_dpi =
        parse_pcx_indexed_2bpp_cga(&encode_pcx_2bpp_cga_dpi(w, h, &cga, 0x80, 4, DPI).unwrap())
            .unwrap();
    let c_plain =
        parse_pcx_indexed_2bpp_cga(&encode_pcx_2bpp_cga(w, h, &cga, 0x80, 4).unwrap()).unwrap();
    assert_eq!(c_dpi.indices, c_plain.indices);
    assert_eq!(c_dpi.indices, cga);

    // 1 bpp × 3 planes EGA RGB
    let r_dpi = parse_pcx_indexed_1bpp_3planes(
        &encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, DPI).unwrap(),
    )
    .unwrap();
    let r_plain =
        parse_pcx_indexed_1bpp_3planes(&encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap())
            .unwrap();
    assert_eq!(r_dpi.indices, r_plain.indices);

    // 1 bpp × 4 planes EGA
    let e_dpi = parse_pcx_indexed_1bpp_4planes(
        &encode_pcx_1bpp_4planes_ega_dpi(w, h, &nibs, &pal, DPI).unwrap(),
    )
    .unwrap();
    let e_plain =
        parse_pcx_indexed_1bpp_4planes(&encode_pcx_1bpp_4planes_ega(w, h, &nibs, &pal).unwrap())
            .unwrap();
    assert_eq!(e_dpi.indices, e_plain.indices);
    assert_eq!(e_dpi.indices, nibs);
}
