//! Self-roundtrip on every (depth, n_planes) combo round 1 ships
//! writers for, plus parse-only smoke tests for the read-only combos
//! (1 bpp × 1 plane mono, 1 bpp × 4 plane EGA — write paths land in
//! round 2).
//!
//! Each parse-only test builds the PCX byte stream by hand (header
//! field-by-field per spec §3, then a tiny RLE-encoded scanline) so a
//! bug in our encoder can't mask the same bug in our decoder.

use oxideav_pcx::rle;
use oxideav_pcx::types::{PCX_HEADER_SIZE, PCX_VGA_PALETTE_BLOCK_BYTES, PCX_VGA_PALETTE_MARKER};
use oxideav_pcx::{encode_pcx_24bpp, encode_pcx_8bpp_indexed, parse_pcx, PcxError, PcxPixelFormat};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn checker_rgb(w: u16, h: u16) -> Vec<u8> {
    // 4-colour 2×2-tiled checker: red / green / blue / white.
    let palette: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let q = (x & 1) + 2 * (y & 1);
            data.extend_from_slice(&palette[q]);
        }
    }
    data
}

fn rgba_from_rgb(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for c in rgb.chunks_exact(3) {
        out.extend_from_slice(&[c[0], c[1], c[2], 0xFF]);
    }
    out
}

fn rainbow_palette() -> Vec<u8> {
    // 256-entry RGB palette: a "hue ramp" (R = i, G = 255-i, B = i^0xAA).
    let mut p = Vec::with_capacity(768);
    for i in 0..256u32 {
        p.push(i as u8);
        p.push((255 - i) as u8);
        p.push((i ^ 0xAA) as u8);
    }
    p
}

// ---------------------------------------------------------------------------
// 24-bpp 3-plane self-roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_24bpp_3planes() {
    let rgb = checker_rgb(8, 6);
    let bytes = encode_pcx_24bpp(8, 6, &rgb).unwrap();
    // Header sanity: byte 0 = 0x0A, byte 1 = 5 (PCX 5.0), byte 2 = 1
    // (RLE), byte 3 = 8 (bits/pixel), byte 65 = 3 (n_planes).
    assert_eq!(bytes[0], 0x0A);
    assert_eq!(bytes[1], 5);
    assert_eq!(bytes[2], 1);
    assert_eq!(bytes[3], 8);
    assert_eq!(bytes[65], 3);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 6);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

#[test]
fn roundtrip_24bpp_solid_compresses_well() {
    // 100×100 solid colour → RLE should crush it to a small handful of
    // packets per scanline (max-63 runs).
    let mut rgb = vec![0u8; 100 * 100 * 3];
    for px in rgb.chunks_exact_mut(3) {
        px.copy_from_slice(&[123, 45, 67]);
    }
    let bytes = encode_pcx_24bpp(100, 100, &rgb).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.data, rgba_from_rgb(&rgb));
    // way smaller than 100*100*3 + header
    assert!(
        bytes.len() < 100 * 100 * 3 / 4,
        "solid 100×100 should crush, got {} bytes",
        bytes.len()
    );
}

#[test]
fn roundtrip_24bpp_uncompressible() {
    // Garbage pixels — every byte different from its neighbour. RLE
    // shouldn't help but must remain bit-exact.
    let w = 32u16;
    let h = 32u16;
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for i in 0..(w as usize * h as usize) {
        rgb.push((i.wrapping_mul(31)) as u8);
        rgb.push((i.wrapping_mul(53)) as u8);
        rgb.push((i.wrapping_mul(97)) as u8);
    }
    let bytes = encode_pcx_24bpp(w, h, &rgb).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

#[test]
fn roundtrip_24bpp_odd_width_pads_scanline() {
    // Odd width forces bytes_per_line to round up to even per spec §1.
    // Round-trip must remain bit-exact in the visible w×h region.
    let w = 7u16;
    let h = 5u16;
    let rgb = checker_rgb(w, h);
    let bytes = encode_pcx_24bpp(w, h, &rgb).unwrap();
    // bytes_per_line is at offset 66 (u16 LE).
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 8, "expected bytes_per_line padded to even");
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

// ---------------------------------------------------------------------------
// 8-bpp single-plane indexed self-roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_8bpp_indexed_with_vga_palette() {
    let palette = rainbow_palette();
    // 16×8 picture: each row is a horizontal index ramp `x % 256`.
    let w = 16u16;
    let h = 8u16;
    let mut indices = Vec::with_capacity(w as usize * h as usize);
    for _y in 0..h {
        for x in 0..w {
            indices.push((x as u8) * 13);
        }
    }
    let bytes = encode_pcx_8bpp_indexed(w, h, &indices, &palette).unwrap();
    // Header sanity.
    assert_eq!(bytes[3], 8); // bits per pixel
    assert_eq!(bytes[65], 1); // planes
                              // VGA palette block sits at the tail.
    assert!(
        bytes.len() >= PCX_HEADER_SIZE + PCX_VGA_PALETTE_BLOCK_BYTES,
        "encoded file too short to hold a VGA palette"
    );
    assert_eq!(
        bytes[bytes.len() - PCX_VGA_PALETTE_BLOCK_BYTES],
        PCX_VGA_PALETTE_MARKER
    );
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 8);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    // Spot-check: pixel (3, 0) → index 39 → palette[39] = (39, 216, 39^0xAA).
    let idx39 = 3u8 * 13;
    assert_eq!(
        img.data[(0 * 16 + 3) * 4..][..4],
        [idx39, 255 - idx39, idx39 ^ 0xAA, 0xFF]
    );
}

#[test]
fn roundtrip_8bpp_solid_compresses_hard() {
    let palette = rainbow_palette();
    let mut indices = vec![0u8; 100 * 100];
    for v in indices.iter_mut() {
        *v = 42;
    }
    let bytes = encode_pcx_8bpp_indexed(100, 100, &indices, &palette).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 100);
    assert_eq!(img.height, 100);
    // Every pixel decoded to palette[42].
    let want = [42u8, 255 - 42, 42 ^ 0xAA, 0xFF];
    for px in img.data.chunks_exact(4) {
        assert_eq!(px, &want);
    }
    // RLE compression check — solid-colour data should be small.
    assert!(
        bytes.len() < 100 * 100 / 4 + PCX_VGA_PALETTE_BLOCK_BYTES + PCX_HEADER_SIZE,
        "solid-colour 100×100 too large at {} bytes",
        bytes.len()
    );
}

#[test]
fn roundtrip_8bpp_indexes_above_192_get_rle_escaped() {
    // Index 200 has top two bits set; a singleton must become a
    // length-1 RLE packet so the decoder doesn't read it as a run.
    let palette = rainbow_palette();
    // Width 4 to hit a guaranteed singleton in the middle.
    let indices = vec![10u8, 20, 200, 30];
    let bytes = encode_pcx_8bpp_indexed(4, 1, &indices, &palette).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    let want = palette;
    let p = |i: u8| {
        [
            want[i as usize * 3],
            want[i as usize * 3 + 1],
            want[i as usize * 3 + 2],
            0xFF,
        ]
    };
    let mut expected = Vec::new();
    for &i in &indices {
        expected.extend_from_slice(&p(i));
    }
    assert_eq!(img.data, expected);
}

// ---------------------------------------------------------------------------
// 1 bpp × 1 plane (monochrome) — read-only smoke test built by hand
// ---------------------------------------------------------------------------

fn write_pcx_header_raw(
    out: &mut Vec<u8>,
    width: u16,
    height: u16,
    bpp: u8,
    planes: u8,
    bytes_per_line: u16,
    ega_palette: &[u8; 48],
) {
    out.push(0x0A); // manufacturer
    out.push(5); // version
    out.push(1); // encoding RLE
    out.push(bpp);
    out.extend_from_slice(&0u16.to_le_bytes()); // x_min
    out.extend_from_slice(&0u16.to_le_bytes()); // y_min
    out.extend_from_slice(&(width - 1).to_le_bytes()); // x_max
    out.extend_from_slice(&(height - 1).to_le_bytes()); // y_max
    out.extend_from_slice(&72u16.to_le_bytes()); // h_dpi
    out.extend_from_slice(&72u16.to_le_bytes()); // v_dpi
    out.extend_from_slice(ega_palette);
    out.push(0); // reserved
    out.push(planes);
    out.extend_from_slice(&bytes_per_line.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // palette_info
    out.extend_from_slice(&0u16.to_le_bytes()); // h_screen_size
    out.extend_from_slice(&0u16.to_le_bytes()); // v_screen_size
    out.extend_from_slice(&[0u8; 54]); // filler
    debug_assert_eq!(out.len(), PCX_HEADER_SIZE);
}

#[test]
fn parse_1bpp_monochrome_8x2() {
    // 8×2 monochrome: row 0 = 0b10101010, row 1 = 0b11110000.
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, 8, 2, 1, 1, 2, &[0u8; 48]);
    // Each row is 2 bytes (bytes_per_line) but only the top byte is
    // pixel-bearing (8 pixels = 8 bits = 1 byte). Pad with 0.
    rle::encode(&[0xAA, 0x00], &mut bytes); // row 0
    rle::encode(&[0xF0, 0x00], &mut bytes); // row 1
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 2);
    // Row 0: alternating black/white starting from bit-7 (white).
    for x in 0..8usize {
        let exp = if (0xAAu8 >> (7 - x)) & 1 != 0 {
            0xFF
        } else {
            0x00
        };
        assert_eq!(img.data[(0 * 8 + x) * 4 + 0], exp);
    }
    // Row 1: 4 white then 4 black.
    for x in 0..4usize {
        assert_eq!(img.data[(1 * 8 + x) * 4 + 0], 0xFF);
    }
    for x in 4..8usize {
        assert_eq!(img.data[(1 * 8 + x) * 4 + 0], 0x00);
    }
}

// ---------------------------------------------------------------------------
// 1 bpp × 4 planes (16-colour EGA) — read-only smoke test built by hand
// ---------------------------------------------------------------------------

#[test]
fn parse_1bpp_4planes_ega_palette_in_header() {
    // 8×1 image: index 5 (red+blue) for all 8 pixels. With our default
    // EGA fallback table that's [0xAA, 0x00, 0xAA] (magenta).
    let mut bytes = Vec::new();
    // Use an explicit palette (not all zeros) so we exercise the
    // header-palette path. Set entry 5 to a sentinel red.
    let mut ega = [0u8; 48];
    ega[5 * 3] = 0xCC;
    ega[5 * 3 + 1] = 0x33;
    ega[5 * 3 + 2] = 0x77;
    write_pcx_header_raw(&mut bytes, 8, 1, 1, 4, 1, &ega);
    // 4 planes, 1 byte each; plane bits → bit position in the 4-bit
    // index. Index 5 = 0b0101 → planes 0 and 2 set.
    let row = [0xFFu8, 0x00, 0xFF, 0x00];
    rle::encode(&row, &mut bytes);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 8);
    for x in 0..8usize {
        assert_eq!(img.data[x * 4..x * 4 + 4], [0xCC, 0x33, 0x77, 0xFF]);
    }
}

// ---------------------------------------------------------------------------
// VGA palette absence falls back to grayscale ramp
// ---------------------------------------------------------------------------

#[test]
fn parse_8bpp_no_vga_palette_uses_grayscale_ramp() {
    let w = 4u16;
    let h = 1u16;
    let indices: Vec<u8> = vec![0, 64, 128, 255];
    // Build a PCX manually with no tail palette block.
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, w, h, 8, 1, 4, &[0u8; 48]);
    rle::encode(&indices, &mut bytes);
    let img = parse_pcx(&bytes).unwrap();
    for (x, &i) in indices.iter().enumerate() {
        assert_eq!(
            &img.data[x * 4..x * 4 + 4],
            &[i, i, i, 0xFF],
            "x={x} idx={i} should map to grayscale ramp"
        );
    }
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn rejects_truncated_header() {
    assert!(matches!(
        parse_pcx(&[0u8; 64]),
        Err(PcxError::InvalidData(_))
    ));
}

#[test]
fn rejects_bad_manufacturer() {
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, 1, 1, 8, 1, 2, &[0u8; 48]);
    bytes[0] = 0xFF;
    assert!(matches!(parse_pcx(&bytes), Err(PcxError::InvalidData(_))));
}

#[test]
fn rejects_unknown_version() {
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, 1, 1, 8, 1, 2, &[0u8; 48]);
    bytes[1] = 99;
    assert!(matches!(parse_pcx(&bytes), Err(PcxError::InvalidData(_))));
}

#[test]
fn rejects_unknown_encoding() {
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, 1, 1, 8, 1, 2, &[0u8; 48]);
    bytes[2] = 2;
    assert!(matches!(parse_pcx(&bytes), Err(PcxError::Unsupported(_))));
}

#[test]
fn rejects_unsupported_combo() {
    // 4 bpp × 1 plane is not in the round-1 set.
    let mut bytes = Vec::new();
    write_pcx_header_raw(&mut bytes, 1, 1, 4, 1, 2, &[0u8; 48]);
    rle::encode(&[0u8, 0u8], &mut bytes);
    assert!(matches!(parse_pcx(&bytes), Err(PcxError::Unsupported(_))));
}

// ---------------------------------------------------------------------------
// RLE round-trip unit test (covers the 0xC0 escape + run cap = 63).
// ---------------------------------------------------------------------------

#[test]
fn rle_roundtrip_with_high_byte_escape() {
    let cases: &[&[u8]] = &[
        &[0x00],
        &[0xC0],                         // singleton high-bit byte → must be RLE-escaped
        &[0xFF; 1],                      // singleton 0xFF
        &[0x42; 5],                      // short run
        &[0x42; 63],                     // exactly the cap
        &[0x42; 64],                     // one over the cap → two packets
        &[0x42; 200],                    // multi-packet run
        &[0xC0, 0xC1, 0xC2, 0xC3, 0xC4], // every byte needs escaping
        &[1, 1, 2, 2, 2, 3, 4, 5, 5],    // mix of runs + literals
    ];
    for case in cases {
        let mut enc = Vec::new();
        rle::encode(case, &mut enc);
        let mut dec = Vec::new();
        let consumed = rle::decode(&enc, &mut dec, case.len()).unwrap();
        assert_eq!(consumed, enc.len(), "decoder should consume entire stream");
        assert_eq!(dec, case.to_vec(), "input case = {:?}", case);
    }
}

// ---------------------------------------------------------------------------
// Hard-asserted self-roundtrip on every (depth, n_planes) writer combo
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_all_writer_combos() {
    // Sizes exercise the even / odd boundary for bytes_per_line.
    let cases: &[(u16, u16)] = &[(1, 1), (2, 2), (7, 5), (16, 16), (33, 9), (100, 100)];
    let palette = rainbow_palette();
    for &(w, h) in cases {
        // 24-bit
        let rgb = checker_rgb(w, h);
        let bytes = encode_pcx_24bpp(w, h, &rgb).unwrap();
        let img = parse_pcx(&bytes).unwrap();
        assert_eq!(img.data, rgba_from_rgb(&rgb), "24-bit roundtrip {w}×{h}");
        // 8-bit indexed
        let mut indices = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            for x in 0..w {
                let v = ((y as usize * w as usize + x as usize) % 256) as u8;
                indices.push(v);
            }
        }
        let bytes = encode_pcx_8bpp_indexed(w, h, &indices, &palette).unwrap();
        let img = parse_pcx(&bytes).unwrap();
        // Recompute expected packed-RGBA from indices + palette.
        let mut expected = Vec::with_capacity(indices.len() * 4);
        for &i in &indices {
            let off = i as usize * 3;
            expected.extend_from_slice(&[palette[off], palette[off + 1], palette[off + 2], 0xFF]);
        }
        assert_eq!(img.data, expected, "8-bit indexed roundtrip {w}×{h}");
    }
}
