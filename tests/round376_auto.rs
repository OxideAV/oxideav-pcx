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

use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_image_auto, encode_pcx_rgb_auto, parse_pcx, PcxAutoMode, PcxImage,
    PcxPixelFormat,
};

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
fn solid_color_large_picks_indexed_single_entry() {
    // One distinct colour over a large canvas: an indexed form with a
    // single meaningful palette entry must win. Since r401 the ladder
    // routes any <= 16-colour image to the 4 bpp header-palette
    // geometry (no VGA tail, half a byte per pixel), so the expected
    // winner is Indexed4 rather than the r376-era Indexed8. Assert the
    // colour count survives in the mode and the round-trip is lossless.
    let w = 256u16;
    let h = 256u16;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&[0x12, 0x34, 0x56]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed4 { colors: 1 });
    let (dw, dh, dec) = decode_to_rgb(&bytes);
    assert_eq!((dw, dh), (w, h));
    assert_eq!(dec, rgb, "indexed solid-colour round-trip must be lossless");
}

#[test]
fn tiny_low_color_image_prefers_planar_when_smaller() {
    // A *tiny* image whose colour count exceeds 16 (so no header-palette
    // candidate applies since r401): the fixed 769-byte VGA tail
    // dominates the Indexed8 candidate, so the planar 24-bit form is
    // genuinely the smaller file and the size-comparing auto writer must
    // return it. Either way the round-trip is lossless.
    let w = 5u16;
    let h = 5u16;
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) as u8 {
        // 25 distinct non-grey colours.
        rgb.extend_from_slice(&[10 + i, 20 + i.wrapping_mul(2), 30 + i.wrapping_mul(3)]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(
        mode,
        PcxAutoMode::Rgb24,
        "the 769-byte palette tail makes planar the smaller file for a 5×5 25-colour image"
    );
    let (_, _, dec) = decode_to_rgb(&bytes);
    assert_eq!(dec, rgb, "tiny planar round-trip must be lossless");
}

#[test]
fn exactly_256_colors_round_trips() {
    // 256 distinct colours spread over a large canvas (each colour
    // repeated) so the indexed candidate's one-byte indices beat planar
    // and the palette is exactly full. Round-trip must be lossless.
    let w = 256u16;
    let h = 64u16;
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..h as usize {
        for i in 0..256u32 {
            rgb.extend_from_slice(&[(i as u8), (i ^ 0x5A) as u8, (i.wrapping_mul(3)) as u8]);
        }
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
    // amortise the fixed 769-byte palette tail. The palette uses 20
    // distinct non-grey, non-primary colours: more than 16 so none of
    // the r401 header-palette / EGA / grayscale rungs apply and the
    // test keeps pinning the Indexed8-vs-planar comparison it was
    // written for.
    let w = 200u16;
    let h = 200u16;
    let mut palette = Vec::new();
    for i in 0..20u8 {
        palette.push([10 + i * 9, 30 + i * 7, 50 + i * 5]);
    }
    let mut state = 0xC0FFEEu32;
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..(w as usize * h as usize) {
        let c = palette[(xorshift32(&mut state) % 20) as usize];
        rgb.extend_from_slice(&c);
    }
    let (auto_bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 20 });
    let planar = encode_pcx_24bpp(w, h, &rgb).unwrap();
    assert!(
        auto_bytes.len() < planar.len(),
        "indexed ({} B) should beat planar ({} B) for 20-colour 200×200 art",
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
    // Large enough that an indexed candidate wins, so the colour count
    // in the mode is observable. With 3 colours both r401 header-palette
    // rungs apply; the period-4 pixel pattern makes every bit-plane row
    // a repeat of one byte, so the plane-oriented 1 bpp × 4 form
    // RLE-collapses far below the packed-nibble form and wins the byte
    // count. Colours first appear in order A, B, C → palette
    // {A=0, B=1, C=2}; repeated to fill a 64×64 canvas.
    let w = 64u16;
    let h = 64u16;
    let a = [10u8, 20, 30];
    let b = [40, 50, 60];
    let c = [70, 80, 90];
    let order = [a, b, a, c];
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&order[i % 4]);
    }
    let (bytes1, mode1) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    let (bytes2, mode2) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode1, PcxAutoMode::Indexed1x4 { colors: 3 });
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
    // pixels must still recover exactly whichever mode the size compare
    // picks. A 201×151 low-colour canvas is large enough that an indexed
    // form wins (3 colours → one of the r401 16-colour header-palette
    // rungs; which of the packed/planar pair is smaller is an RLE
    // accident of the noise, so both are admitted), exercising the
    // odd-width pad on the indexed path.
    let w = 201u16;
    let h = 151u16;
    let mut state = 0x1234_5678u32;
    let mut rgb = Vec::new();
    let pal = [[0u8, 0, 0], [255, 255, 255], [128, 64, 32]];
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut state) % 3) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(matches!(
        mode,
        PcxAutoMode::Indexed4 { .. } | PcxAutoMode::Indexed1x4 { .. }
    ));
    let (dw, dh, dec) = decode_to_rgb(&bytes);
    assert_eq!((dw, dh), (w, h));
    assert_eq!(dec, rgb, "odd-width indexed round-trip must be lossless");
}

#[test]
fn returned_bytes_are_the_smaller_of_the_two_candidates() {
    // The "most compact" contract: for a low-colour input the emitted
    // file is never larger than *either* the plain planar writer or the
    // indexed writer would produce on its own. Check a size where the
    // decision could go either way plus the two clear extremes.
    let pal = [[0u8, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]];
    for (w, h) in [(2u16, 2u16), (40, 40), (256, 256)] {
        let mut state = 0xBEEF_0001u32;
        let mut rgb = Vec::new();
        for _ in 0..(w as usize * h as usize) {
            rgb.extend_from_slice(&pal[(xorshift32(&mut state) % 4) as usize]);
        }
        let (auto, _mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
        let planar = encode_pcx_24bpp(w, h, &rgb).unwrap();
        assert!(
            auto.len() <= planar.len(),
            "{w}×{h}: auto ({}) must not exceed planar ({})",
            auto.len(),
            planar.len()
        );
        let (_, _, dec) = decode_to_rgb(&auto);
        assert_eq!(dec, rgb, "{w}×{h}: round-trip must be lossless");
    }
}

// --- encode_pcx_image_auto (PcxImage-level wrapper) -----------------------

/// Build an `Rgba` `PcxImage` from packed RGB (alpha = 0xFF), with the
/// given optional metadata.
fn rgba_image(
    w: u32,
    h: u32,
    rgb: &[u8],
    dpi: Option<(u16, u16)>,
    window_origin: Option<(u16, u16)>,
    screen_size: Option<(u16, u16)>,
) -> PcxImage {
    let mut data = Vec::with_capacity(w as usize * h as usize * 4);
    for c in rgb.chunks_exact(3) {
        data.extend_from_slice(&[c[0], c[1], c[2], 0xFF]);
    }
    PcxImage {
        width: w,
        height: h,
        pixel_format: PcxPixelFormat::Rgba,
        data,
        pts: None,
        dpi,
        window_origin,
        screen_size,
    }
}

#[test]
fn image_auto_indexed_preserves_dpi() {
    // Large low-colour Rgba image with authoring DPI but no window /
    // screen metadata → indexed branch, DPI threaded into the header.
    let w = 200u32;
    let h = 200u32;
    // More than 16 non-grey colours so no r401 header-palette / EGA /
    // grayscale rung applies: the test pins the Indexed8 DPI path.
    let mut pal = Vec::new();
    for i in 0..20u8 {
        pal.push([15 + i * 8, 40 + i * 6, 70 + i * 4]);
    }
    let mut state = 0xAB_CD_01u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut state) % 20) as usize]);
    }
    let img = rgba_image(w, h, &rgb, Some((300, 300)), None, None);
    let (bytes, mode) = encode_pcx_image_auto(&img).unwrap();
    assert!(matches!(mode, PcxAutoMode::Indexed8 { .. }));
    // Header bits_per_pixel / n_planes confirm the indexed geometry.
    assert_eq!(bytes[3], 8, "indexed → 8 bpp");
    assert_eq!(bytes[65], 1, "indexed → 1 plane");
    // The decoded image must carry the threaded DPI and the original
    // pixels.
    let dec = parse_pcx(&bytes).unwrap();
    assert_eq!(
        dec.dpi,
        Some((300, 300)),
        "DPI must survive the indexed branch"
    );
    let (_, _, dec_rgb) = decode_to_rgb(&bytes);
    assert_eq!(dec_rgb, rgb);
}

#[test]
fn image_auto_window_metadata_forces_planar_to_preserve_it() {
    // A low-colour image that *would* go indexed, but carries a window
    // origin the indexed geometry cannot represent. The wrapper must
    // honour the metadata: fall back to planar and round-trip the origin.
    let w = 200u32;
    let h = 200u32;
    let pal = [[10u8, 10, 10], [200, 100, 50]];
    let mut state = 0x77_77_01u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut state) % 2) as usize]);
    }
    let img = rgba_image(w, h, &rgb, Some((150, 150)), Some((40, 24)), None);
    let (bytes, mode) = encode_pcx_image_auto(&img).unwrap();
    assert_eq!(
        mode,
        PcxAutoMode::Rgb24,
        "window-origin metadata must force the planar branch"
    );
    let dec = parse_pcx(&bytes).unwrap();
    assert_eq!(
        dec.window_origin,
        Some((40, 24)),
        "window origin must survive"
    );
    assert_eq!(
        dec.dpi,
        Some((150, 150)),
        "DPI must survive alongside window"
    );
    let (_, _, dec_rgb) = decode_to_rgb(&bytes);
    assert_eq!(dec_rgb, rgb);
}

#[test]
fn image_auto_true_color_falls_back_to_planar() {
    // > 256 colours → planar regardless of metadata.
    let w = 257u32;
    let h = 2u32;
    let mut rgb = Vec::new();
    for _ in 0..h {
        for i in 0..257u32 {
            rgb.extend_from_slice(&[(i & 0xFF) as u8, (i >> 8) as u8, 0x00]);
        }
    }
    let img = rgba_image(w, h, &rgb, None, None, None);
    let (bytes, mode) = encode_pcx_image_auto(&img).unwrap();
    assert_eq!(mode, PcxAutoMode::Rgb24);
    assert_eq!(bytes[3], 8);
    assert_eq!(bytes[65], 3, "24-bit → 3 planes");
    let (_, _, dec_rgb) = decode_to_rgb(&bytes);
    assert_eq!(dec_rgb, rgb);
}

#[test]
fn image_auto_rejects_indexed8_input() {
    let img = PcxImage {
        width: 4,
        height: 1,
        pixel_format: PcxPixelFormat::Indexed8,
        data: vec![0, 1, 2, 3],
        pts: None,
        dpi: None,
        window_origin: None,
        screen_size: None,
    };
    assert!(encode_pcx_image_auto(&img).is_err());
}
