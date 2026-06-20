//! Round 354 — decode robustness against over-padded `bytes_per_line`
//! (foreign-encoder scanline padding).
//!
//! The crate's own encoders always write the *minimal* even
//! `bytes_per_line` for a given width, so every existing round-trip
//! sweep (round345.rs and the per-mode fixtures) only exercises the
//! decoder against the tightest possible scanline stride. But the PCX
//! format explicitly permits a scanline to carry **arbitrary trailing
//! padding** beyond the minimum: the ZSoft Technical Reference Manual
//! Rev 5 states `BytesPerLine` is the on-disk per-plane stride and
//! "Do NOT calculate from Xmax-Xmin", and the cross-reference summary
//! (`docs/image/pcx/pcx-egff-fileformat-info.html`) is even more
//! explicit:
//!
//!   * "The planar data in a scan line is often padded with extra data
//!     to align the scan line on an even byte boundary [...] programs
//!     use this value to find where in a scan line pixel data stops and
//!     extra padding begins."
//!   * "Any PCX image may contain extra bytes of padding at the end of
//!     each scan line [...] only the image data within the picture
//!     dimension window coordinates is displayed."
//!   * `LinePaddingSize = ((BytesPerLine * NumBitPlanes) *
//!     (8 / BitsPerPixel)) - ((XEnd - XStart) + 1);`
//!
//! A real-world PCX written by another tool can therefore declare a
//! `bytes_per_line` substantially larger than `ceil(width × bpp / 8)`,
//! with the surplus bytes being padding the decoder MUST skip. This
//! file builds raw PCX bitstreams with a *deliberately over-padded*
//! per-plane stride for every `(bits_per_pixel, n_planes)` mode the
//! crate decodes, and asserts that:
//!
//!   1. the high-level `parse_pcx` (packed RGBA) is byte-identical to
//!      the same picture written with the minimal stride; and
//!   2. every typed accessor recovers exactly the `width × height`
//!      picture-window indices with the padding stripped.
//!
//! All fixtures are hand-built in-file (clean-room: no external crate,
//! no fixture files); the pixel payloads come from a tiny deterministic
//! LCG so the expectations are reproducible.

use oxideav_pcx::{
    parse_pcx, parse_pcx_indexed_1bpp_2planes_cga, parse_pcx_indexed_1bpp_3planes,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_8bpp, PCX_HEADER_SIZE, PCX_MANUFACTURER,
    PCX_VGA_PALETTE_MARKER,
};

/// Deterministic, dependency-free pseudo-random byte source (a 64-bit
/// xorshift). Used only to synthesise test payloads — never any codec
/// logic.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
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
    /// A value masked to `0..(1 << bits)`.
    fn index(&mut self, bits: u32) -> u32 {
        let mask = (1u32 << bits) - 1;
        self.next_u64() as u32 & mask
    }
}

/// The minimal per-plane stride for `width` pixels at `bpp` bits/pixel.
fn min_bpl(width: u16, bpp: u8) -> u16 {
    let w = width as u32;
    let raw = match bpp {
        1 => w.div_ceil(8),
        2 => w.div_ceil(4),
        4 => w.div_ceil(2),
        8 => w,
        _ => unreachable!(),
    };
    // Round up to even (spec: BytesPerLine MUST be even).
    (raw + (raw & 1)) as u16
}

/// PCX RLE-encode a flat byte buffer (whole-image, continuous stream).
/// A literal `>= 0xC0` is escaped as a length-1 packet so the decoder
/// can't mistake it for a run header; runs coalesce up to length 63.
fn rle_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        let mut run = 1usize;
        while i + run < data.len() && data[i + run] == b && run < 63 {
            run += 1;
        }
        if run >= 2 || (b & 0xC0) == 0xC0 {
            out.push(0xC0 | run as u8);
            out.push(b);
        } else {
            out.push(b);
        }
        i += run;
    }
    out
}

/// Build a raw 128-byte PCX header with the given geometry. `bpl` is the
/// *on-disk per-plane* stride, which the caller deliberately over-pads.
#[allow(clippy::too_many_arguments)]
fn build_header(
    version: u8,
    bpp: u8,
    width: u16,
    height: u16,
    n_planes: u8,
    bpl: u16,
    palette_info: u16,
    ega_palette: &[u8; 48],
) -> Vec<u8> {
    let mut h = vec![0u8; PCX_HEADER_SIZE];
    h[0] = PCX_MANUFACTURER;
    h[1] = version;
    h[2] = 1; // RLE
    h[3] = bpp;
    // x_min/y_min = 0; x_max = width-1, y_max = height-1.
    h[8..10].copy_from_slice(&(width - 1).to_le_bytes());
    h[10..12].copy_from_slice(&(height - 1).to_le_bytes());
    h[16..64].copy_from_slice(ega_palette);
    h[65] = n_planes;
    h[66..68].copy_from_slice(&bpl.to_le_bytes());
    h[68..70].copy_from_slice(&palette_info.to_le_bytes());
    h
}

/// Build a complete PCX file: header + planar scanlines (each plane
/// `bpl` bytes, surplus beyond the real per-plane data filled with a
/// non-zero padding sentinel so a decoder that fails to skip it would
/// produce visibly wrong pixels) + optional VGA tail palette.
#[allow(clippy::too_many_arguments)]
fn build_pcx(
    version: u8,
    bpp: u8,
    width: u16,
    height: u16,
    n_planes: u8,
    bpl: u16,
    palette_info: u16,
    ega_palette: &[u8; 48],
    plane_rows: &[Vec<Vec<u8>>], // [row][plane] -> packed plane bytes (len <= bpl)
    vga_tail: Option<&[u8; 768]>,
) -> Vec<u8> {
    let mut planar = Vec::new();
    for row in plane_rows.iter() {
        assert_eq!(row.len(), n_planes as usize);
        for plane in row.iter() {
            assert!(plane.len() <= bpl as usize, "plane data exceeds stride");
            planar.extend_from_slice(plane);
            // Pad the rest of the on-disk plane with 0xAA — a sentinel
            // that, if not stripped, would corrupt the decoded picture.
            planar.resize(planar.len() + (bpl as usize - plane.len()), 0xAA);
        }
    }
    let mut out = build_header(
        version,
        bpp,
        width,
        height,
        n_planes,
        bpl,
        palette_info,
        ega_palette,
    );
    out.extend_from_slice(&rle_encode(&planar));
    if let Some(pal) = vga_tail {
        out.push(PCX_VGA_PALETTE_MARKER);
        out.extend_from_slice(pal);
    }
    out
}

fn ega_palette_48() -> [u8; 48] {
    let mut p = [0u8; 48];
    for (i, c) in p.chunks_exact_mut(3).enumerate() {
        c[0] = (i as u8).wrapping_mul(17);
        c[1] = (i as u8).wrapping_mul(5).wrapping_add(3);
        c[2] = (i as u8).wrapping_mul(11).wrapping_add(7);
    }
    p
}

fn vga_palette_768() -> [u8; 768] {
    let mut p = [0u8; 768];
    for (i, c) in p.chunks_exact_mut(3).enumerate() {
        c[0] = i as u8;
        c[1] = (255 - i) as u8;
        c[2] = (i as u8).wrapping_mul(3);
    }
    p
}

// A representative sweep of widths/heights that straddle the sub-byte
// chunk boundaries AND force odd minimal strides (so the over-pad delta
// from the minimum varies).
const DIMS: &[(u16, u16)] = &[
    (1, 1),
    (3, 2),
    (7, 3),
    (8, 1),
    (9, 5),
    (13, 4),
    (17, 3),
    (31, 2),
];

// How much surplus padding to add on top of the minimal even stride.
// Includes 0 (control = minimal), and several even/odd deltas (the
// final stride is forced even).
const SURPLUS: &[u16] = &[0, 2, 4, 6, 16];

/// For a chosen surplus, return an over-padded stride (still even).
fn padded_bpl(width: u16, bpp: u8, surplus: u16) -> u16 {
    let base = min_bpl(width, bpp);
    let bpl = base + surplus;
    bpl + (bpl & 1) // keep even
}

// ---------------------------------------------------------------------
// 8 bpp × 1 plane — 256-colour indexed (VGA tail palette).
// ---------------------------------------------------------------------
#[test]
fn overpadded_8bpp_indexed_strips_padding() {
    let pal = vga_palette_768();
    for &(w, h) in DIMS {
        // Reference indices: width × height bytes, row-major.
        let mut lcg = Lcg::new(0xA8B1_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.byte();
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 8, surplus);
            // 8 bpp × 1 plane: one plane row per scanline = the w real
            // index bytes, then bpl-w padding bytes.
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| vec![idx[y * w as usize..(y + 1) * w as usize].to_vec()])
                .collect();
            let pcx = build_pcx(5, 8, w, h, 1, bpl, 1, &[0u8; 48], &rows, Some(&pal));
            let view = parse_pcx_indexed_8bpp(&pcx).expect("typed decode");
            assert_eq!(view.width, w as u32);
            assert_eq!(view.height, h as u32);
            assert_eq!(view.indices, idx, "8bpp w={w} h={h} surplus={surplus}");
            // Palette must come from the tail, not be corrupted by padding.
            for (i, c) in pal.chunks_exact(3).enumerate() {
                assert_eq!(view.palette[i], [c[0], c[1], c[2]]);
            }
            // parse_pcx packed RGBA must match the palette-mapped indices.
            let img = parse_pcx(&pcx).expect("packed decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            for (p, &i) in img.data.chunks_exact(4).zip(idx.iter()) {
                let c = &pal[i as usize * 3..i as usize * 3 + 3];
                assert_eq!([p[0], p[1], p[2], p[3]], [c[0], c[1], c[2], 0xFF]);
            }
        }
    }
}

// ---------------------------------------------------------------------
// 8 bpp × 3 planes — 24-bit true colour (no palette).
// ---------------------------------------------------------------------
#[test]
fn overpadded_24bpp_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x24B1_0000 ^ ((w as u64) << 16) ^ h as u64);
        // Reference RGB samples, planar by row.
        let mut rgb = vec![[0u8; 3]; w as usize * h as usize];
        for px in rgb.iter_mut() {
            *px = [lcg.byte(), lcg.byte(), lcg.byte()];
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 8, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    let r: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][0]).collect();
                    let g: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][1]).collect();
                    let b: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][2]).collect();
                    vec![r, g, b]
                })
                .collect();
            let pcx = build_pcx(5, 8, w, h, 3, bpl, 1, &[0u8; 48], &rows, None);
            let img = parse_pcx(&pcx).expect("24bpp decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            for (p, px) in img.data.chunks_exact(4).zip(rgb.iter()) {
                assert_eq!(
                    [p[0], p[1], p[2], p[3]],
                    [px[0], px[1], px[2], 0xFF],
                    "24bpp w={w} h={h} surplus={surplus}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// 4 bpp × 1 plane — 16-colour packed (2 px/byte, high nibble first).
// ---------------------------------------------------------------------
#[test]
fn overpadded_4bpp_packed_strips_padding() {
    let ega = ega_palette_48();
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x4B11_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(4) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 4, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    let mut plane = vec![0u8; min_bpl(w, 4) as usize];
                    for x in 0..w as usize {
                        let nib = idx[base + x] & 0x0F;
                        if x & 1 == 0 {
                            plane[x >> 1] |= nib << 4;
                        } else {
                            plane[x >> 1] |= nib;
                        }
                    }
                    vec![plane]
                })
                .collect();
            let pcx = build_pcx(5, 4, w, h, 1, bpl, 1, &ega, &rows, None);
            let view = parse_pcx_indexed_4bpp(&pcx).expect("4bpp decode");
            assert_eq!(view.indices, idx, "4bpp w={w} h={h} surplus={surplus}");
            // Also confirm the packed RGBA path strips padding.
            let img = parse_pcx(&pcx).expect("4bpp packed decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
        }
    }
}

// ---------------------------------------------------------------------
// 2 bpp × 1 plane — 4-colour CGA packed (4 px/byte).
// ---------------------------------------------------------------------
#[test]
fn overpadded_2bpp_cga_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x2B11_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(2) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 2, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    let mut plane = vec![0u8; min_bpl(w, 2) as usize];
                    for x in 0..w as usize {
                        let v = idx[base + x] & 0x03;
                        let shift = 6 - 2 * (x & 3);
                        plane[x >> 2] |= v << shift;
                    }
                    vec![plane]
                })
                .collect();
            let mut ega = [0u8; 48];
            // byte 16 background nibble + byte 19 selector — arbitrary valid.
            ega[0] = 0x10;
            ega[3] = 0xE0;
            let pcx = build_pcx(5, 2, w, h, 1, bpl, 1, &ega, &rows, None);
            let view = parse_pcx_indexed_2bpp_cga(&pcx).expect("2bpp decode");
            assert_eq!(view.indices, idx, "2bpp w={w} h={h} surplus={surplus}");
            let img = parse_pcx(&pcx).expect("2bpp packed decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 1 plane — monochrome.
// ---------------------------------------------------------------------
#[test]
fn overpadded_1bpp_mono_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x1B11_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut bit = vec![0u8; w as usize * h as usize];
        for v in bit.iter_mut() {
            *v = (lcg.index(1)) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 1, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    let mut plane = vec![0u8; min_bpl(w, 1) as usize];
                    for x in 0..w as usize {
                        if bit[base + x] != 0 {
                            plane[x >> 3] |= 1 << (7 - (x & 7));
                        }
                    }
                    vec![plane]
                })
                .collect();
            let pcx = build_pcx(5, 1, w, h, 1, bpl, 1, &[0u8; 48], &rows, None);
            let img = parse_pcx(&pcx).expect("mono decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
            for (p, &b) in img.data.chunks_exact(4).zip(bit.iter()) {
                let v = if b != 0 { 0xFF } else { 0x00 };
                assert_eq!(
                    [p[0], p[1], p[2], p[3]],
                    [v, v, v, 0xFF],
                    "mono w={w} h={h} surplus={surplus}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 4 planes — 16-colour EGA (plane k supplies index bit k).
// ---------------------------------------------------------------------
#[test]
fn overpadded_1bpp_4planes_ega_strips_padding() {
    let ega = ega_palette_48();
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x1B41_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(4) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 1, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    (0..4)
                        .map(|k| {
                            let mut plane = vec![0u8; min_bpl(w, 1) as usize];
                            for x in 0..w as usize {
                                if (idx[base + x] >> k) & 1 != 0 {
                                    plane[x >> 3] |= 1 << (7 - (x & 7));
                                }
                            }
                            plane
                        })
                        .collect()
                })
                .collect();
            let pcx = build_pcx(5, 1, w, h, 4, bpl, 1, &ega, &rows, None);
            let view = parse_pcx_indexed_1bpp_4planes(&pcx).expect("ega4 decode");
            assert_eq!(view.indices, idx, "ega4 w={w} h={h} surplus={surplus}");
            let img = parse_pcx(&pcx).expect("ega4 packed decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 3 planes — 8-colour EGA RGB (plane k supplies index bit k).
// ---------------------------------------------------------------------
#[test]
fn overpadded_1bpp_3planes_ega_rgb_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x1B31_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(3) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 1, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    (0..3)
                        .map(|k| {
                            let mut plane = vec![0u8; min_bpl(w, 1) as usize];
                            for x in 0..w as usize {
                                if (idx[base + x] >> k) & 1 != 0 {
                                    plane[x >> 3] |= 1 << (7 - (x & 7));
                                }
                            }
                            plane
                        })
                        .collect()
                })
                .collect();
            let pcx = build_pcx(5, 1, w, h, 3, bpl, 1, &[0u8; 48], &rows, None);
            let view = parse_pcx_indexed_1bpp_3planes(&pcx).expect("ega3 decode");
            assert_eq!(view.indices, idx, "ega3 w={w} h={h} surplus={surplus}");
            let img = parse_pcx(&pcx).expect("ega3 packed decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
        }
    }
}

// ---------------------------------------------------------------------
// 1 bpp × 2 planes — 4-colour CGA planar (p0 | p1<<1).
// ---------------------------------------------------------------------
#[test]
fn overpadded_1bpp_2planes_cga_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x1B21_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u8; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(2) as u8;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 1, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    (0..2)
                        .map(|k| {
                            let mut plane = vec![0u8; min_bpl(w, 1) as usize];
                            for x in 0..w as usize {
                                if (idx[base + x] >> k) & 1 != 0 {
                                    plane[x >> 3] |= 1 << (7 - (x & 7));
                                }
                            }
                            plane
                        })
                        .collect()
                })
                .collect();
            let mut ega = [0u8; 48];
            ega[0] = 0x10;
            ega[3] = 0xE0;
            let pcx = build_pcx(5, 1, w, h, 2, bpl, 1, &ega, &rows, None);
            let view = parse_pcx_indexed_1bpp_2planes_cga(&pcx).expect("cga2 decode");
            assert_eq!(view.indices, idx, "cga2 w={w} h={h} surplus={surplus}");
            let img = parse_pcx(&pcx).expect("cga2 packed decode");
            assert_eq!((img.width, img.height), (w as u32, h as u32));
        }
    }
}

// ---------------------------------------------------------------------
// 4 bpp × 4 planes — composite-index mode (p0 | p1<<4 | p2<<8 | p3<<12).
// ---------------------------------------------------------------------
#[test]
fn overpadded_4bpp_4planes_strips_padding() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0x4B41_0000 ^ ((w as u64) << 16) ^ h as u64);
        let mut idx = vec![0u16; w as usize * h as usize];
        for v in idx.iter_mut() {
            *v = lcg.index(16) as u16;
        }
        for &surplus in SURPLUS {
            let bpl = padded_bpl(w, 4, surplus);
            let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
                .map(|y| {
                    let base = y * w as usize;
                    (0..4)
                        .map(|k| {
                            // plane k carries nibble k of each composite index.
                            let mut plane = vec![0u8; min_bpl(w, 4) as usize];
                            for x in 0..w as usize {
                                let nib = ((idx[base + x] >> (4 * k)) & 0x0F) as u8;
                                if x & 1 == 0 {
                                    plane[x >> 1] |= nib << 4;
                                } else {
                                    plane[x >> 1] |= nib;
                                }
                            }
                            plane
                        })
                        .collect()
                })
                .collect();
            let pcx = build_pcx(5, 4, w, h, 4, bpl, 1, &[0u8; 48], &rows, None);
            let view = parse_pcx_indexed_4bpp_4planes(&pcx).expect("4x4 decode");
            assert_eq!(view.indices, idx, "4x4 w={w} h={h} surplus={surplus}");
        }
    }
}

// ---------------------------------------------------------------------
// Equivalence: an over-padded file and its minimal-stride twin decode to
// a byte-identical packed RGBA image. The surplus padding is purely an
// on-disk-layout artefact; the picture-window pixels must be invariant
// under the choice of `bytes_per_line`.
// ---------------------------------------------------------------------
#[test]
fn overpadded_24bpp_equals_minimal_stride() {
    for &(w, h) in DIMS {
        let mut lcg = Lcg::new(0xE0_24B1 ^ ((w as u64) << 16) ^ h as u64);
        let mut rgb = vec![[0u8; 3]; w as usize * h as usize];
        for px in rgb.iter_mut() {
            *px = [lcg.byte(), lcg.byte(), lcg.byte()];
        }
        let rows: Vec<Vec<Vec<u8>>> = (0..h as usize)
            .map(|y| {
                let base = y * w as usize;
                let r: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][0]).collect();
                let g: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][1]).collect();
                let b: Vec<u8> = (0..w as usize).map(|x| rgb[base + x][2]).collect();
                vec![r, g, b]
            })
            .collect();
        let minimal = build_pcx(5, 8, w, h, 3, min_bpl(w, 8), 1, &[0u8; 48], &rows, None);
        let ref_img = parse_pcx(&minimal).expect("minimal decode");
        for &surplus in &[2u16, 8, 32] {
            let bpl = padded_bpl(w, 8, surplus);
            let padded = build_pcx(5, 8, w, h, 3, bpl, 1, &[0u8; 48], &rows, None);
            let img = parse_pcx(&padded).expect("padded decode");
            assert_eq!(
                img.data, ref_img.data,
                "24bpp padded != minimal (w={w} h={h} surplus={surplus})"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Rejection: a `bytes_per_line` so large that the claimed planar buffer
// (`n_planes × bytes_per_line × height`) cannot possibly be backed by
// the RLE bytes present must be rejected, not OOM'd or panicked. This is
// the decompression-bomb guard: a hostile header can claim a multi-MiB
// stride behind a handful of pixel bytes. The decoder caps the claimed
// output at `available_rle_bytes × 63` (the max a run packet can expand)
// and returns an error below that.
// ---------------------------------------------------------------------
#[test]
fn pathological_overpad_is_rejected_not_bombed() {
    // width 4, height 4, 8 bpp × 1 plane, but bytes_per_line claims
    // 60000 bytes/row — 240000 planar bytes claimed behind a tiny RLE
    // payload that can decode only a few bytes.
    let mut pcx = build_header(5, 8, 4, 4, 1, 60000, 1, &[0u8; 48]);
    // A single short RLE run — nowhere near enough to back 240000 bytes.
    pcx.push(0xC0 | 4); // run of 4
    pcx.push(0x11);
    let err = parse_pcx(&pcx);
    assert!(err.is_err(), "pathological over-pad must be rejected");
    // The typed accessor shares the same guard.
    let err2 = parse_pcx_indexed_8bpp(&pcx);
    assert!(err2.is_err(), "typed accessor must reject too");
}

// ---------------------------------------------------------------------
// Boundary: a stride one byte short of the minimum (mis-framing) must be
// rejected, confirming the over-pad acceptance is bounded below by the
// real picture-window requirement and never silently truncates pixels.
// ---------------------------------------------------------------------
#[test]
fn under_minimum_stride_is_rejected() {
    // width 16 at 8 bpp needs >= 16 bytes/plane; declare 15.
    let mut pcx = build_header(5, 8, 16, 1, 1, 15, 1, &[0u8; 48]);
    // Enough RLE to fill 15 bytes so only the stride guard can reject it.
    pcx.extend(std::iter::repeat_n(0x22u8, 15));
    assert!(
        parse_pcx(&pcx).is_err(),
        "bytes_per_line below the picture-window minimum must be rejected"
    );
}
