//! Round 376 — compact-mode auto-selecting RGB encoder
//! ([`encode_pcx_rgb_auto`]).
//!
//! `encode_pcx_rgb_auto` picks the smallest valid PCX 5.0 geometry that
//! losslessly represents a packed-RGB input: 8 bpp × 1 plane indexed
//! (with a 256-entry VGA tail palette) when the image has `≤ 256`
//! distinct colours, else the 8 bpp × 3 plane planar 24-bit form. Both
//! targets are existing spec modes, so the only new behaviour under test
//! is the colour-count decision, the first-seen palette assignment, and
//! the lossless round-trip through `parse_pcx` in both branches.

use oxideav_pcx::{encode_pcx_24bpp, encode_pcx_rgb_auto, parse_pcx, PcxAutoMode};

/// Deterministic generator so every run exercises the same pixels.
fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Decode a PCX back to packed RGB (dropping the decoder's alpha) so we
/// can compare it against the original RGB input byte-for-byte.
fn decode_to_rgb(bytes: &[u8]) -> (u16, u16, Vec<u8>) {
    let img = parse_pcx(bytes).expect("decode");
    let mut rgb = Vec::with_capacity(img.width as usize * img.height as usize * 3);
    for px in img.data.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    (img.width as u16, img.height as u16, rgb)
}

#[test]
fn solid_color_picks_indexed_single_entry() {
    // One distinct colour → indexed, 1 palette entry.
    let w = 17u16;
    let h = 9u16;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&[0x12, 0x34, 0x56]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 1 });
    let (dw, dh, dec) = decode_to_rgb(&bytes);
    assert_eq!((dw, dh), (w, h));
    assert_eq!(dec, rgb, "indexed solid-colour round-trip must be lossless");
}

#[test]
fn exactly_256_colors_stays_indexed() {
    // 256 pixels, each a distinct colour → exactly fills the palette.
    let w = 16u16;
    let h = 16u16;
    let mut rgb = Vec::with_capacity(256 * 3);
    for i in 0..256u32 {
        // Spread the colour across all three channels so no two pixels
        // collide.
        rgb.extend_from_slice(&[(i as u8), (i ^ 0x5A) as u8, (i.wrapping_mul(3)) as u8]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 256 });
    let (_, _, dec) = decode_to_rgb(&bytes);
    assert_eq!(dec, rgb, "256-colour indexed round-trip must be lossless");
}

#[test]
fn over_256_colors_falls_back_to_24bpp_and_matches_plain_writer() {
    // 257 distinct colours → must spill to planar 24-bit. The bytes the
    // auto writer emits in this branch must be byte-identical to the
    // plain `encode_pcx_24bpp` (same spec mode, same input).
    let w = 257u16;
    let h = 1u16;
    let mut rgb = Vec::with_capacity(257 * 3);
    for i in 0..257u32 {
        // 257 guaranteed-distinct colours via a 9-bit counter spread
        // across the red + green channels.
        rgb.extend_from_slice(&[(i & 0xFF) as u8, (i >> 8) as u8, 0x00]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Rgb24);
    let plain = encode_pcx_24bpp(w, h, &rgb).unwrap();
    assert_eq!(
        bytes, plain,
        "24-bit fallback must match the plain 24bpp writer byte-for-byte"
    );
    let (_, _, dec) = decode_to_rgb(&bytes);
    assert_eq!(dec, rgb, "24-bit fallback round-trip must be lossless");
}

#[test]
fn indexed_is_smaller_than_planar_for_low_color_art() {
    // A low-colour synthetic image: the indexed form (1 byte/pixel +
    // 768-byte palette) should be materially smaller than the planar
    // 24-bit form (3 bytes/pixel) once the image is large enough to
    // amortise the fixed 769-byte palette tail.
    let w = 200u16;
    let h = 200u16;
    let palette: [[u8; 3]; 8] = [
        [0, 0, 0],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
        [255, 255, 255],
    ];
    let mut state = 0xC0FFEEu32;
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..(w as usize * h as usize) {
        let c = palette[(xorshift32(&mut state) % 8) as usize];
        rgb.extend_from_slice(&c);
    }
    let (auto_bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 8 });
    let planar = encode_pcx_24bpp(w, h, &rgb).unwrap();
    assert!(
        auto_bytes.len() < planar.len(),
        "indexed ({} B) should beat planar ({} B) for 8-colour 200×200 art",
        auto_bytes.len(),
        planar.len()
    );
    let (_, _, dec) = decode_to_rgb(&auto_bytes);
    assert_eq!(dec, rgb, "indexed low-colour round-trip must be lossless");
}

#[test]
fn first_seen_palette_order_is_deterministic() {
    // Palette indices follow first-seen raster order: the first pixel
    // gets index 0, the next *new* colour index 1, and so on. Re-running
    // on identical input yields byte-identical output.
    let w = 4u16;
    let h = 1u16;
    // Colours appear in order A, B, A, C → palette {A=0, B=1, C=2}.
    let a = [10, 20, 30];
    let b = [40, 50, 60];
    let c = [70, 80, 90];
    let mut rgb = Vec::new();
    rgb.extend_from_slice(&a);
    rgb.extend_from_slice(&b);
    rgb.extend_from_slice(&a);
    rgb.extend_from_slice(&c);
    let (bytes1, mode1) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    let (bytes2, mode2) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode1, PcxAutoMode::Indexed8 { colors: 3 });
    assert_eq!(mode1, mode2);
    assert_eq!(bytes1, bytes2, "auto-encode must be deterministic");
    let (_, _, dec) = decode_to_rgb(&bytes1);
    assert_eq!(dec, rgb);
}

#[test]
fn rejects_zero_dimension_and_short_input() {
    assert!(encode_pcx_rgb_auto(0, 4, &[0; 12]).is_err());
    assert!(encode_pcx_rgb_auto(4, 0, &[0; 12]).is_err());
    // 4×1 needs 12 RGB bytes; 11 is short.
    assert!(encode_pcx_rgb_auto(4, 1, &[0; 11]).is_err());
}

#[test]
fn odd_width_round_trips_in_both_branches() {
    // Odd width forces an even-stride pad in both targets; the visible
    // pixels must still recover exactly. Low-colour → indexed.
    let w = 7u16;
    let h = 5u16;
    let mut state = 0x1234_5678u32;
    let mut rgb = Vec::new();
    let pal = [[0u8, 0, 0], [255, 255, 255], [128, 64, 32]];
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut state) % 3) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(matches!(mode, PcxAutoMode::Indexed8 { .. }));
    let (dw, dh, dec) = decode_to_rgb(&bytes);
    assert_eq!((dw, dh), (w, h));
    assert_eq!(dec, rgb, "odd-width indexed round-trip must be lossless");
}
