//! Round 362 — byte-exact bit-packing regression suite for the
//! 1-bit-per-plane encoders.
//!
//! The four 1-bpp-per-plane encode paths (`encode_pcx_1bpp_mono`,
//! `encode_pcx_1bpp_2planes_cga`, `encode_pcx_1bpp_3planes_ega_rgb`,
//! `encode_pcx_1bpp_4planes_ega`) were the documented encoder hotspot
//! (`BENCHMARKS.md` rank #2/#3: the per-pixel
//! `row[plane·bpl + x/8] |= 1 << (7 − x%8)` scatter — one branch-guarded
//! indexed read-modify-write store per set bit). Round 362 replaced the
//! scatter with a whole-byte packer (`pack_1bpp_plane_row`, eight pixels
//! folded into one accumulator and written once).
//!
//! The packer must be **byte-identical** to the scatter form. The risky
//! dimension is the scanline tail: when `width` is not a multiple of 8,
//! the last output byte carries fewer than eight pixels, and the
//! `bytes_per_line`-is-even padding leaves whole trailing bytes that
//! must stay zero. This suite re-encodes through every affected path at
//! a width sweep that hits all eight `width % 8` residues plus widths
//! that force odd `width.div_ceil(8)` (so the even-stride padding byte
//! is exercised) and asserts the produced PCX decodes back to the exact
//! source indices via the matching typed accessor.
//!
//! The spec layout under test: each pixel `x` occupies bit `7 − (x % 8)`
//! of byte `x / 8` within its plane's `bytes_per_line` slice
//! (`docs/image/pcx/zsoft-pcx-technical-reference-rev5.md`, §"Image File
//! (.PCX) Format" — "each line of the image is stored by color plane").
//!
//! Payloads are an in-file xorshift so the suite is deterministic and
//! dependency-free (clean-room: no external crate).

use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_mono, parse_pcx, parse_pcx_indexed_1bpp_2planes_cga,
    parse_pcx_indexed_1bpp_4planes,
};

struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }
    fn next_u8(&mut self) -> u8 {
        // xorshift64* — deterministic, no external dependency.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
    }
}

/// Independent reference implementation of one plane's MSB-first
/// scatter, deliberately written in the naïve per-pixel form the
/// optimisation replaced. Used to pin the encoder's *exact bytes*, not
/// merely a successful round-trip.
fn scatter_plane(dst: &mut [u8], width: usize, get_bit: impl Fn(usize) -> bool) {
    for x in 0..width {
        if get_bit(x) {
            dst[x / 8] |= 1 << (7 - (x % 8));
        }
    }
}

/// Width sweep: every `% 8` residue (1..=8), the two-byte-and-tail
/// cases (9..=16), and a couple of wider widths so a multi-byte plane is
/// exercised. Odd `div_ceil(8)` widths (e.g. 1..=8 → 1 byte → padded to
/// 2) drive the even-stride padding byte.
const WIDTHS: &[u16] = &[1, 2, 3, 5, 7, 8, 9, 15, 16, 17, 23, 24, 31, 33, 64, 100];
const HEIGHTS: &[u16] = &[1, 3, 7];

#[test]
fn mono_roundtrips_every_width_residue() {
    let mut rng = Lcg::new(0x4D4F_4E4F ^ 0x9E37_79B9);
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let n = w as usize * h as usize;
            // Mono: any non-zero pixel sets the bit; mix 0 / non-0.
            let pixels: Vec<u8> = (0..n).map(|_| rng.next_u8() & 1).collect();
            let bytes = encode_pcx_1bpp_mono(w, h, &pixels).expect("mono encode");
            let img = parse_pcx(&bytes).expect("mono decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            // parse_pcx flattens 1bpp mono to 0x00 / 0xFF grayscale RGBA.
            for (i, &p) in pixels.iter().enumerate() {
                let r = img.data[i * 4];
                let want = if p != 0 { 0xFF } else { 0x00 };
                assert_eq!(r, want, "mono w={w} h={h} pixel {i}");
            }
        }
    }
}

#[test]
fn ega_4planes_roundtrips_every_width_residue() {
    let mut rng = Lcg::new(0xE6A4_u64 ^ 0x1234_5678);
    let palette: Vec<u8> = (0..48).map(|i| (i * 5) as u8).collect();
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.next_u8() & 0x0F).collect();
            let bytes = encode_pcx_1bpp_4planes_ega(w, h, &indices, &palette).expect("ega4 encode");
            let img = parse_pcx_indexed_1bpp_4planes(&bytes).expect("ega4 decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            assert_eq!(img.indices, indices, "ega4 w={w} h={h}");
        }
    }
}

#[test]
fn cga_2planes_roundtrips_every_width_residue() {
    let mut rng = Lcg::new(0xC6A2_u64 ^ 0xDEAD_BEEF);
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.next_u8() & 0b11).collect();
            let bytes = encode_pcx_1bpp_2planes_cga(w, h, &indices, 0, 0).expect("cga2 encode");
            let img = parse_pcx_indexed_1bpp_2planes_cga(&bytes).expect("cga2 decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            assert_eq!(img.indices, indices, "cga2 w={w} h={h}");
        }
    }
}

#[test]
fn ega_rgb_3planes_roundtrips_every_width_residue() {
    let mut rng = Lcg::new(0xE6A3_u64 ^ 0x0F0F_0F0F);
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let n = w as usize * h as usize;
            // 3 channels; encoder thresholds each at >= 0x80, decoder
            // emits 0x00 / 0xFF, so seed only those two values per
            // channel to get an exact round-trip.
            let rgb: Vec<u8> = (0..n * 3)
                .map(|_| if rng.next_u8() & 1 != 0 { 0xFF } else { 0x00 })
                .collect();
            let bytes = encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).expect("ega-rgb encode");
            let img = parse_pcx(&bytes).expect("ega-rgb decode");
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            for i in 0..n {
                for c in 0..3 {
                    assert_eq!(
                        img.data[i * 4 + c],
                        rgb[i * 3 + c],
                        "ega-rgb w={w} h={h} pixel {i} ch {c}"
                    );
                }
            }
        }
    }
}

/// Pin the *exact encoded bytes* against an independent scatter
/// reference for the 4-plane EGA path — the packer is byte-identical, so
/// the whole PCX (header + RLE planar data) must match a file built by
/// the naïve per-pixel scatter for the same input. This catches a
/// tail-bit or padding-byte divergence that a round-trip alone could
/// mask (decode strips padding, so a stray padding bit could survive a
/// round-trip but corrupt a strict reader).
#[test]
fn ega_4planes_bytes_match_scatter_reference() {
    let mut rng = Lcg::new(0xBEEF_u64 ^ 0xABCD);
    let palette: Vec<u8> = (0..48).map(|i| (i * 3 + 1) as u8).collect();
    for &w in WIDTHS {
        for &h in HEIGHTS {
            let n = w as usize * h as usize;
            let indices: Vec<u8> = (0..n).map(|_| rng.next_u8() & 0x0F).collect();
            let produced = encode_pcx_1bpp_4planes_ega(w, h, &indices, &palette).expect("encode");

            // Rebuild the planar RLE region independently via the scatter
            // reference and compare the whole file byte-for-byte.
            let bpl = {
                let raw = w.div_ceil(8);
                if raw % 2 == 0 {
                    raw as usize
                } else {
                    raw as usize + 1
                }
            };
            let mut expected = produced[..128].to_vec(); // header is path-shared
            let mut row = vec![0u8; bpl * 4];
            for y in 0..h as usize {
                row.iter_mut().for_each(|v| *v = 0);
                for plane in 0..4 {
                    let dst = &mut row[plane * bpl..plane * bpl + bpl];
                    let line = &indices[y * w as usize..];
                    scatter_plane(dst, w as usize, |x| (line[x] >> plane) & 1 != 0);
                }
                // RLE-encode the row exactly as the encoder does (per-row).
                rle_encode_row(&row, &mut expected);
            }
            assert_eq!(produced, expected, "ega4 byte-exact w={w} h={h}");
        }
    }
}

/// In-file copy of the PCX RLE row encoder (spec §3.2) so the byte-exact
/// reference does not reach into the crate's private `rle` internals
/// beyond the public surface.
fn rle_encode_row(row: &[u8], out: &mut Vec<u8>) {
    let mut i = 0usize;
    let n = row.len();
    while i < n {
        let b = row[i];
        let mut run = 1usize;
        while i + run < n && row[i + run] == b && run < 63 {
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
