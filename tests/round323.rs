//! r323 — 4 bpp × 4 planes composite-index mode.
//!
//! The EGFF canonical PCX video-mode matrix
//! (`docs/image/pcx/pcx-egff-fileformat-info.html`, "PCX Image Data
//! Format") lists six hardware video modes — Monochrome (1×1), CGA
//! (1×2), EGA (3×1), EGA/VGA (4×1), Extended VGA (1×8), Extended
//! VGA/XGA (3×8) — and does NOT list `4 bpp × 4 planes` among them.
//! It is nonetheless *structurally* reachable: the same summary defines
//! a PCX's maximum colour count as
//!
//! ```text
//! MaxNumberOfColors = (1 << (BitsPerPixel * NumBitPlanes));
//! ```
//!
//! so `4 bpp × 4 planes` describes `1 << (4 * 4) = 65536` composite
//! values. The on-disk layout is the standard plane-oriented PCX form
//! (spec §"Image File (.PCX) Format": "each line of the image is stored
//! by color plane"): each scanline carries plane 0..plane 3 one after
//! another, each holding 4 bits/pixel (2 px/byte, high nibble first).
//! The nibble at the same x-position across the four planes stacks into a
//! 16-bit composite index (`p0 | p1<<4 | p2<<8 | p3<<12`).
//!
//! r323 closes the one remaining `(bpp, planes)` slot the README "Lacks"
//! tail named:
//!
//! * [`oxideav_pcx::encode_pcx_4bpp_4planes`] — writes the plane-oriented
//!   16-bit composite indices.
//! * [`oxideav_pcx::parse_pcx_indexed_4bpp_4planes`] — surfaces them back
//!   (no palette: the spec defines no 65536-entry palette geometry).
//!
//! This file tests:
//!
//! 1. A round-trip of a deterministic composite-index pattern preserves
//!    every pixel exactly.
//! 2. The header advertises `(bits_per_pixel, n_planes) = (4, 4)` and an
//!    even `bytes_per_line` per spec §1.
//! 3. The plane-stacking ordering is exactly `p0 | p1<<4 | p2<<8 | p3<<12`
//!    (verified by hand-built single-pixel cases on the nibble seams).
//! 4. The composite index spans the full 16-bit range (0..=0xFFFF), which
//!    is the whole point of the mode.
//! 5. The typed accessor rejects every other `(bpp, planes)` combination,
//!    and `parse_pcx` rejects `(4, 4)` (no spec-defined RGB mapping).
//! 6. Odd widths (where the last byte's low nibble is padding) round-trip.

use oxideav_pcx::{
    encode_pcx_4bpp_4planes, encode_pcx_8bpp_indexed, parse_header, parse_pcx,
    parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_8bpp, PCX_HEADER_SIZE,
};

/// Deterministic composite-index fill that exercises all four nibble
/// chunks independently across the image.
fn fill(width: u16, height: u16) -> Vec<u16> {
    let mut v = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height as usize {
        for x in 0..width as usize {
            // Each plane nibble varies on a different stride so a swapped
            // plane order would be caught.
            let p0 = (x as u16) & 0x0F;
            let p1 = (y as u16) & 0x0F;
            let p2 = ((x + y) as u16) & 0x0F;
            let p3 = ((x.wrapping_mul(3) + y) as u16) & 0x0F;
            v.push(p0 | (p1 << 4) | (p2 << 8) | (p3 << 12));
        }
    }
    v
}

#[test]
fn roundtrip_4bpp_4planes_even_width() {
    let (w, h) = (16u16, 12u16);
    let indices = fill(w, h);
    let pcx = encode_pcx_4bpp_4planes(w, h, &indices).expect("encode");
    let view = parse_pcx_indexed_4bpp_4planes(&pcx).expect("decode");

    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.stride(), w as usize);
    assert_eq!(view.indices.len(), w as usize * h as usize);
    assert_eq!(view.indices, indices);
}

#[test]
fn roundtrip_4bpp_4planes_odd_width() {
    // Odd width → the last on-disk byte of each plane carries a padding
    // low nibble; the accessor must strip it and still round-trip.
    let (w, h) = (15u16, 9u16);
    let indices = fill(w, h);
    let pcx = encode_pcx_4bpp_4planes(w, h, &indices).expect("encode");
    let view = parse_pcx_indexed_4bpp_4planes(&pcx).expect("decode");
    assert_eq!(view.indices.len(), w as usize * h as usize);
    assert_eq!(view.indices, indices);
}

#[test]
fn header_advertises_4bpp_4planes_even_bytes_per_line() {
    let pcx = encode_pcx_4bpp_4planes(7, 3, &[0u16; 7 * 3]).expect("encode");
    let header = parse_header(&pcx).expect("header");
    assert_eq!(header.bits_per_pixel, 4);
    assert_eq!(header.n_planes, 4);
    // 7 px @ 4 bpp = ceil(7/2) = 4 bytes → already even.
    assert_eq!(header.bytes_per_line, 4);
    assert_eq!(
        header.bytes_per_line % 2,
        0,
        "spec §1 requires even BytesPerLine"
    );
    assert!(pcx.len() > PCX_HEADER_SIZE);
}

#[test]
fn plane_stacking_order_is_p0_p1_p2_p3() {
    // Each single-pixel image sets exactly one plane's nibble so the
    // composite must equal that nibble shifted into its chunk position.
    for plane in 0u16..4 {
        let nib = 0x0Du16;
        let composite = nib << (plane * 4);
        let pcx = encode_pcx_4bpp_4planes(1, 1, &[composite]).expect("encode");
        let view = parse_pcx_indexed_4bpp_4planes(&pcx).expect("decode");
        assert_eq!(
            view.indices[0], composite,
            "plane {plane} nibble must occupy chunk {plane}"
        );
    }
}

#[test]
fn full_16bit_range_roundtrips() {
    // The defining property of the mode: composite indices use all 16
    // bits. Walk a spread of representative values including the extremes.
    let samples: Vec<u16> = vec![
        0x0000, 0xFFFF, 0x1234, 0xABCD, 0x8000, 0x0001, 0xF0F0, 0x0F0F, 0xCAFE, 0xBEEF,
    ];
    let w = samples.len() as u16;
    let pcx = encode_pcx_4bpp_4planes(w, 1, &samples).expect("encode");
    let view = parse_pcx_indexed_4bpp_4planes(&pcx).expect("decode");
    assert_eq!(view.indices, samples);
}

#[test]
fn accessor_rejects_non_4x4_modes() {
    // An 8 bpp × 1 plane file must be rejected by the 4×4 accessor.
    let palette: Vec<u8> = (0..256).flat_map(|i| [i as u8, 0, 0]).collect();
    let pcx8 = encode_pcx_8bpp_indexed(4, 4, &[0u8; 16], &palette).expect("encode 8bpp");
    let err = parse_pcx_indexed_4bpp_4planes(&pcx8).expect_err("must reject (8,1)");
    let msg = format!("{err}");
    assert!(msg.contains("4 bpp × 4 planes"), "unexpected error: {msg}");

    // Sanity: the 8 bpp accessor still accepts it.
    assert!(parse_pcx_indexed_8bpp(&pcx8).is_ok());
}

#[test]
fn parse_pcx_flatten_rejects_4x4() {
    // The canonical Rgba flatten path has no spec-defined 65536-colour
    // mapping, so it must refuse `(4, 4)` rather than invent one.
    let pcx = encode_pcx_4bpp_4planes(4, 2, &[0u16; 8]).expect("encode");
    assert!(
        parse_pcx(&pcx).is_err(),
        "parse_pcx must reject the palette-less 4×4 mode"
    );
}

#[test]
fn encode_rejects_zero_dim_and_short_input() {
    assert!(encode_pcx_4bpp_4planes(0, 4, &[]).is_err());
    assert!(encode_pcx_4bpp_4planes(4, 0, &[]).is_err());
    assert!(encode_pcx_4bpp_4planes(4, 4, &[0u16; 4]).is_err());
}
