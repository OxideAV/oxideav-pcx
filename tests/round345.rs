//! Round 345 — exhaustive cross-dimensional round-trip property tests.
//!
//! The crate's spec coverage (every `(bits_per_pixel, n_planes)` mode
//! the ZSoft Technical Reference Manual Rev 5 defines) is complete; the
//! hand-picked fixtures in the earlier `roundXX.rs` files each pin one
//! representative size per mode. This file is the *systematic* sweep:
//! for every encode→decode path it walks a Cartesian product of widths
//! and heights chosen to straddle every packing seam the format has —
//!
//!   * the **even-`bytes_per_line`** padding (spec §"ZSoft .PCX File
//!     Header Format": `BytesPerLine` "MUST be EVEN"), exercised by odd
//!     per-plane byte counts that round up;
//!   * the **sub-byte chunk boundary** for the bit-packed modes — 8
//!     pixels/byte (1 bpp), 4 pixels/byte (2 bpp), 2 pixels/byte (4 bpp)
//!     — exercised by widths that land mid-chunk so the final partial
//!     byte's unused low bits are padding the round-trip must ignore;
//!   * the **planar-vs-packed** reconstruction for the multi-plane modes
//!     (24-bit 8 bpp × 3, EGA 1 bpp × {3,4}, CGA 1 bpp × 2, composite
//!     4 bpp × 4).
//!
//! Every assertion is bit-exact on the recovered index / sample buffer
//! (not a tolerance), because PCX RLE is lossless by construction. The
//! pixel payloads are produced by a tiny in-file LCG so the tests are
//! deterministic and dependency-free (clean-room: no external crate, no
//! fixture files).

use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_4planes,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed, parse_pcx,
    parse_pcx_indexed_1bpp_2planes_cga, parse_pcx_indexed_1bpp_3planes,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_8bpp,
};

/// Deterministic, dependency-free pseudo-random byte source (a 64-bit
/// xorshift). Used only to synthesise test payloads — never any codec
/// logic. A fixed seed per call site keeps the sweep reproducible.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        // Avoid the xorshift fixed point at 0.
        Lcg(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }
    /// A byte masked to `0..(1 << bits)` — an index that fits the mode.
    fn index(&mut self, bits: u32) -> u8 {
        let mask = (1u16 << bits) - 1;
        (self.byte() as u16 & mask) as u8
    }
}

/// The width/height sweep. These deliberately include:
///   * widths 1 and 2 (degenerate single-/double-column),
///   * odd widths (7, 13, 17) that force `bytes_per_line` to round up,
///   * widths landing exactly on and one-past each sub-byte chunk
///     boundary (8, 9 for 1 bpp; 4, 5 for 2 bpp; 2, 3 for 4 bpp),
///   * a couple of larger sizes (31, 64) to span multiple RLE runs.
const WIDTHS: &[u16] = &[1, 2, 3, 4, 5, 7, 8, 9, 13, 16, 17, 31, 64];
const HEIGHTS: &[u16] = &[1, 2, 3, 5, 8, 17];

fn gray_ramp_768() -> Vec<u8> {
    let mut p = vec![0u8; 768];
    for (i, c) in p.chunks_exact_mut(3).enumerate() {
        let v = i as u8;
        c[0] = v;
        c[1] = v;
        c[2] = v;
    }
    p
}

fn ega_palette_48() -> Vec<u8> {
    // 16 distinct triples so the palette round-trips observably; values
    // are arbitrary for a no-loss index round-trip.
    let mut p = vec![0u8; 48];
    for (i, c) in p.chunks_exact_mut(3).enumerate() {
        c[0] = (i as u8).wrapping_mul(17);
        c[1] = (i as u8).wrapping_mul(5).wrapping_add(3);
        c[2] = (i as u8).wrapping_mul(11).wrapping_add(7);
    }
    p
}

// ---------------------------------------------------------------------
// 8 bpp × 1 plane — 256-colour indexed (VGA tail palette).
// ---------------------------------------------------------------------
#[test]
fn sweep_8bpp_indexed_roundtrip_bit_exact() {
    let pal = gray_ramp_768();
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x8b00 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
            let bytes = encode_pcx_8bpp_indexed(w, h, &indices, &pal).unwrap();

            // Canonical RGBA decode produces width*height*4 bytes.
            let img = parse_pcx(&bytes).unwrap();
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            assert_eq!(img.data.len(), n * 4);

            // Typed indexed accessor must recover the exact indices.
            let idx = parse_pcx_indexed_8bpp(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "8bpp index mismatch w={w} h={h}");
        }
    }
}

// ---------------------------------------------------------------------
// 8 bpp × 1 plane — grayscale (palette_info = 2, no tail palette).
// ---------------------------------------------------------------------
#[test]
fn sweep_8bpp_grayscale_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x6a00 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let pixels: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
            let bytes = encode_pcx_8bpp_grayscale(w, h, &pixels).unwrap();
            let img = parse_pcx(&bytes).unwrap();
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            // Grayscale: each sample becomes (g, g, g, 0xFF).
            for (i, px) in img.data.chunks_exact(4).enumerate() {
                let g = pixels[i];
                assert_eq!(px, [g, g, g, 0xFF], "gray w={w} h={h} i={i}");
            }
        }
    }
}

// ---------------------------------------------------------------------
// 4 bpp × 1 plane — 16-colour packed (2 pixels/byte).
// ---------------------------------------------------------------------
#[test]
fn sweep_4bpp_packed_roundtrip_bit_exact() {
    let pal = ega_palette_48();
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x4b00 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.index(4)).collect();
            let bytes = encode_pcx_4bpp_packed(w, h, &indices, &pal).unwrap();
            let idx = parse_pcx_indexed_4bpp(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "4bpp packed mismatch w={w} h={h}");
            // Palette must survive verbatim.
            for (i, c) in pal.chunks_exact(3).enumerate() {
                assert_eq!(idx.palette[i], [c[0], c[1], c[2]], "pal[{i}] w={w} h={h}");
            }
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 4 planes — 16-colour EGA bit-plane layout.
// ---------------------------------------------------------------------
#[test]
fn sweep_1bpp_4planes_ega_roundtrip_bit_exact() {
    let pal = ega_palette_48();
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x14b0 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.index(4)).collect();
            let bytes = encode_pcx_1bpp_4planes_ega(w, h, &indices, &pal).unwrap();
            let idx = parse_pcx_indexed_1bpp_4planes(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "1bpp×4 mismatch w={w} h={h}");
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 3 planes — 8-colour EGA RGB (no header palette).
// ---------------------------------------------------------------------
#[test]
fn sweep_1bpp_3planes_ega_rgb_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x13b0 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            // Each channel is thresholded at 0x80; feed pure 0x00/0xFF
            // so the round-trip is exact (per README §encode).
            let rgb: Vec<u8> = (0..n * 3)
                .map(|_| if rng.byte() & 1 == 1 { 0xFF } else { 0x00 })
                .collect();
            let bytes = encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap();
            let idx = parse_pcx_indexed_1bpp_3planes(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            // The 3-bit index per pixel is (R?1) | (G?2) | (B?4) — verify
            // each pixel's recovered index matches its source bits.
            for i in 0..n {
                let r = (rgb[i * 3] >= 0x80) as u8;
                let g = (rgb[i * 3 + 1] >= 0x80) as u8;
                let b = (rgb[i * 3 + 2] >= 0x80) as u8;
                let expect = r | (g << 1) | (b << 2);
                assert_eq!(idx.indices[i], expect, "1bpp×3 idx w={w} h={h} i={i}");
            }
        }
    }
}

// ---------------------------------------------------------------------
// 2 bpp × 1 plane — 4-colour CGA packed (4 pixels/byte).
// ---------------------------------------------------------------------
#[test]
fn sweep_2bpp_cga_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x2cb0 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.index(2)).collect();
            // selector bit7 = palette select, bit6 = intensity; sweep all
            // four meaningful (selector, background) combos lightly.
            let selector = rng.byte() & 0xC0;
            let background = rng.index(4);
            let bytes = encode_pcx_2bpp_cga(w, h, &indices, selector, background).unwrap();
            let idx = parse_pcx_indexed_2bpp_cga(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "2bpp CGA mismatch w={w} h={h}");
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 2 planes — 4-colour CGA plane-oriented layout.
// ---------------------------------------------------------------------
#[test]
fn sweep_1bpp_2planes_cga_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x12b0 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.index(2)).collect();
            let selector = rng.byte() & 0xC0;
            let background = rng.index(4);
            let bytes = encode_pcx_1bpp_2planes_cga(w, h, &indices, selector, background).unwrap();
            let idx = parse_pcx_indexed_1bpp_2planes_cga(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "1bpp×2 CGA mismatch w={w} h={h}");
        }
    }
}

// ---------------------------------------------------------------------
// 8 bpp × 3 planes — 24-bit RGB (planar).
// ---------------------------------------------------------------------
#[test]
fn sweep_24bpp_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x2400 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            let rgb: Vec<u8> = (0..n * 3).map(|_| rng.byte()).collect();
            let bytes = encode_pcx_24bpp(w, h, &rgb).unwrap();
            let img = parse_pcx(&bytes).unwrap();
            assert_eq!(img.width, w as u32, "w={w} h={h}");
            assert_eq!(img.height, h as u32, "w={w} h={h}");
            // RGBA out: each pixel (r, g, b, 0xFF).
            for i in 0..n {
                let px = &img.data[i * 4..i * 4 + 4];
                assert_eq!(
                    px,
                    [rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2], 0xFF],
                    "24bpp px w={w} h={h} i={i}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// 4 bpp × 4 planes — composite-index mode (u16 index per pixel).
// ---------------------------------------------------------------------
#[test]
fn sweep_4bpp_4planes_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x4400 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            // 4 planes × 4 bits = 16-bit composite index.
            let indices: Vec<u16> = (0..n).map(|_| (rng.next_u64() & 0xFFFF) as u16).collect();
            let bytes = encode_pcx_4bpp_4planes(w, h, &indices).unwrap();
            let idx = parse_pcx_indexed_4bpp_4planes(&bytes).unwrap();
            assert_eq!(idx.width, w as u32, "w={w} h={h}");
            assert_eq!(idx.height, h as u32, "w={w} h={h}");
            assert_eq!(idx.indices, indices, "4bpp×4 mismatch w={w} h={h}");
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 1 plane — monochrome.
// ---------------------------------------------------------------------
#[test]
fn sweep_1bpp_mono_roundtrip_bit_exact() {
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let mut rng = Lcg::new(0x1100 ^ ((w as u64) << 16) ^ h as u64);
            let n = w as usize * h as usize;
            // One byte per pixel, 0 or 1.
            let pixels: Vec<u8> = (0..n).map(|_| rng.byte() & 1).collect();
            let bytes = encode_pcx_1bpp_mono(w, h, &pixels).unwrap();
            let img = parse_pcx(&bytes).unwrap();
            assert_eq!(img.width, w as u32, "w={w} h={h}");
            assert_eq!(img.height, h as u32, "w={w} h={h}");
            // bit 1 = white (0xFF), bit 0 = black (0x00).
            for (i, &p) in pixels.iter().enumerate() {
                let want = if p & 1 == 1 { 0xFF } else { 0x00 };
                let px = &img.data[i * 4..i * 4 + 4];
                assert_eq!(px, [want, want, want, 0xFF], "mono w={w} h={h} i={i}");
            }
        }
    }
}
