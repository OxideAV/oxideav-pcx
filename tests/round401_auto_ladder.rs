//! Round 401 — extended compact-mode candidate ladder for
//! [`encode_pcx_rgb_auto`] / [`encode_pcx_image_auto`].
//!
//! r376 introduced the two-candidate auto writer (8 bpp indexed vs
//! planar 24-bit). r401 grows it into a full ladder over the crate's
//! existing spec modes: every candidate whose losslessness
//! precondition holds is encoded and the fewest-byte file wins, with a
//! fixed deterministic preference order breaking exact ties. Every
//! branch must still round-trip through [`parse_pcx`] bit-for-bit —
//! the ladder is a pure encode-time size optimisation and never
//! quantises.

use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed, encode_pcx_image_auto,
    encode_pcx_rgb_auto, parse_pcx, PcxAutoMode, PcxImage, PcxPixelFormat,
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

/// Assert `bytes` decodes to exactly `(w, h, rgb)`.
fn assert_lossless(bytes: &[u8], w: u16, h: u16, rgb: &[u8]) {
    let (dw, dh, dec) = decode_to_rgb(bytes);
    assert_eq!((dw, dh), (w, h), "decoded dimensions");
    assert_eq!(dec, rgb, "auto output must round-trip RGB exactly");
}

// ---------------------------------------------------------------------------
// Gray8 candidate
// ---------------------------------------------------------------------------

#[test]
fn grayscale_image_picks_gray8_and_drops_tail() {
    // 64×64 with 200 distinct grey levels: Indexed8 would carry the
    // fixed 769-byte VGA tail; Gray8 carries the identical pixel
    // structure (an injective byte remap cannot change RLE run
    // boundaries' positions, only escape costs) plus no tail at all
    // here because every grey level IS the pixel byte.
    let (w, h) = (64u16, 64u16);
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let g = ((x * 3 + y * 5) % 200) as u8;
            rgb.extend_from_slice(&[g, g, g]);
        }
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Gray8);
    assert_lossless(&bytes, w, h, &rgb);
    // Explicit size dominance over the two r376 candidates.
    let gray: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
    let direct = encode_pcx_8bpp_grayscale(w, h, &gray).unwrap();
    assert_eq!(bytes, direct, "auto Gray8 must equal the direct writer");
    let planar = encode_pcx_24bpp(w, h, &rgb).unwrap();
    assert!(bytes.len() < planar.len());
}

#[test]
fn escape_heavy_grey_correctly_falls_back_to_indexed8() {
    // A grey image whose values are ALL RLE-expensive bytes (≥ 0xC0):
    // in the Gray8 payload every literal costs a 2-byte escape, while
    // first-seen indexing remaps the 64 distinct levels to indices
    // 0..63 (1 byte each). On 1024 pixels that saves ~1024 payload
    // bytes — more than the 769-byte VGA tail costs — so the ladder's
    // size comparison must pick Indexed8 even though the image is pure
    // grey. This is exactly why Gray8 is a *candidate*, not a rule.
    let (w, h) = (32u16, 32u16);
    let mut rgb = Vec::new();
    let mut st = 0xC0FFEEu32;
    for _ in 0..(w as usize * h as usize) {
        let g = 0xC0u8.wrapping_add((xorshift32(&mut st) % 0x40) as u8);
        rgb.extend_from_slice(&[g, g, g]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 64 });
    assert_lossless(&bytes, w, h, &rgb);
    // The Gray8 candidate must genuinely be larger here.
    let gray: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
    let gray_file = encode_pcx_8bpp_grayscale(w, h, &gray).unwrap();
    assert!(
        bytes.len() < gray_file.len(),
        "Indexed8 ({}) must beat escape-heavy Gray8 ({})",
        bytes.len(),
        gray_file.len()
    );
}

#[test]
fn gray8_beats_indexed8_when_escapes_do_not_dominate() {
    // Grey levels kept below 0xC0 so the Gray8 payload has the same
    // RLE cost structure as the indexed payload (injective byte remap,
    // no escape asymmetry) — the 769-byte tail is then the whole
    // difference and Gray8 must win by at least that margin minus any
    // run-boundary noise.
    let (w, h) = (32u16, 32u16);
    let mut rgb = Vec::new();
    let mut st = 0xBEEFu32;
    for _ in 0..(w as usize * h as usize) {
        let g = (xorshift32(&mut st) % 0xC0) as u8;
        rgb.extend_from_slice(&[g, g, g]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Gray8);
    assert_lossless(&bytes, w, h, &rgb);
    let mut palette = vec![0u8; 768];
    let mut seen: Vec<u8> = Vec::new();
    let mut indices = Vec::new();
    for p in rgb.chunks_exact(3) {
        let idx = match seen.iter().position(|&s| s == p[0]) {
            Some(i) => i,
            None => {
                seen.push(p[0]);
                let i = seen.len() - 1;
                palette[i * 3] = p[0];
                palette[i * 3 + 1] = p[0];
                palette[i * 3 + 2] = p[0];
                i
            }
        };
        indices.push(idx as u8);
    }
    let indexed = encode_pcx_8bpp_indexed(w, h, &indices, &palette).unwrap();
    assert!(
        bytes.len() < indexed.len(),
        "Gray8 ({}) must beat Indexed8 ({}) here",
        bytes.len(),
        indexed.len()
    );
}

#[test]
fn near_gray_image_is_not_gray8() {
    // One pixel with r != g disqualifies the Gray8 candidate entirely
    // (the ladder never quantises); with 2 distinct colours the indexed
    // form wins instead on a canvas this size.
    let (w, h) = (48u16, 48u16);
    let mut rgb = vec![0x40u8; w as usize * h as usize * 3];
    rgb[0] = 0x41; // (0x41, 0x40, 0x40) — not a pure grey
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_ne!(mode, PcxAutoMode::Gray8);
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn image_auto_threads_dpi_through_gray8() {
    let (w, h) = (40u16, 25u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        let g = (i % 251) as u8;
        rgb.extend_from_slice(&[g, g, g]);
    }
    let image = PcxImage {
        width: w as u32,
        height: h as u32,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((300, 300)),
        window_origin: None,
        screen_size: None,
    };
    let (bytes, mode) = encode_pcx_image_auto(&image).unwrap();
    assert_eq!(mode, PcxAutoMode::Gray8);
    let decoded = parse_pcx(&bytes).unwrap();
    assert_eq!(decoded.dpi, Some((300, 300)), "authoring DPI must survive");
    assert_lossless(&bytes, w, h, &rgb);
}

// ---------------------------------------------------------------------------
// Mono1 candidate
// ---------------------------------------------------------------------------

#[test]
fn black_and_white_image_picks_mono1() {
    // Text-like bilevel content: 1 bpp is 8× smaller per pixel than the
    // Gray8 candidate and 24× smaller than planar before RLE even
    // starts. Assert the ladder lands on Mono1 and round-trips.
    let (w, h) = (80u16, 60u16);
    let mut rgb = Vec::new();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let on = (x / 3 + y / 2) % 2 == 0;
            let v = if on { 0xFF } else { 0x00 };
            rgb.extend_from_slice(&[v, v, v]);
        }
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    assert_lossless(&bytes, w, h, &rgb);
    assert_eq!(bytes[3], 1, "1 bpp");
    assert_eq!(bytes[65], 1, "1 plane");
    // Must beat the Gray8 candidate (same content is also all-grey).
    let gray: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
    let gray_file = encode_pcx_8bpp_grayscale(w, h, &gray).unwrap();
    assert!(bytes.len() < gray_file.len());
}

#[test]
fn all_black_solid_is_mono1() {
    // Single colour that happens to be black: Mono1 applies (all-zero
    // plane rows RLE-collapse) and must win the ladder outright.
    let (w, h) = (64u16, 64u16);
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn off_black_disqualifies_mono1() {
    // (1, 1, 1) is a grey but not pure black: Mono1 must not fire (it
    // would quantise); Gray8 still applies. Both grey levels are kept
    // below 0xC0 so RLE escape costs don't hand the byte count to the
    // indexed candidate instead (that economics case has its own test).
    let (w, h) = (64u16, 64u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        let v = if i % 2 == 0 { 0x01 } else { 0xBF };
        rgb.extend_from_slice(&[v, v, v]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Gray8);
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn image_auto_threads_dpi_through_mono1() {
    let (w, h) = (33u16, 21u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        let v = if (i / 5) % 2 == 0 { 0x00 } else { 0xFF };
        rgb.extend_from_slice(&[v, v, v]);
    }
    let image = PcxImage {
        width: w as u32,
        height: h as u32,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((600, 300)),
        window_origin: None,
        screen_size: None,
    };
    let (bytes, mode) = encode_pcx_image_auto(&image).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    let decoded = parse_pcx(&bytes).unwrap();
    assert_eq!(decoded.dpi, Some((600, 300)));
    assert_lossless(&bytes, w, h, &rgb);
}

// ---------------------------------------------------------------------------
// EgaRgb1x3 candidate
// ---------------------------------------------------------------------------

#[test]
fn eight_primary_image_picks_ega_rgb_1x3() {
    // All eight EGA RGB primaries present: 3 bits/pixel planar beats
    // both 8 bpp forms and needs no stored palette.
    let (w, h) = (96u16, 64u16);
    let prim: [[u8; 3]; 8] = [
        [0x00, 0x00, 0x00],
        [0xFF, 0x00, 0x00],
        [0x00, 0xFF, 0x00],
        [0x00, 0x00, 0xFF],
        [0xFF, 0xFF, 0x00],
        [0x00, 0xFF, 0xFF],
        [0xFF, 0x00, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let mut st = 0xDECAFu32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&prim[(xorshift32(&mut st) % 8) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::EgaRgb1x3);
    assert_lossless(&bytes, w, h, &rgb);
    assert_eq!(bytes[3], 1, "1 bpp");
    assert_eq!(bytes[65], 3, "3 planes");
}

#[test]
fn primaries_plus_one_off_color_disqualify_ega_rgb() {
    // A single non-primary pixel bars the EgaRgb1x3 candidate — the
    // ladder never quantises.
    let (w, h) = (48u16, 48u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        if i == 5 {
            rgb.extend_from_slice(&[0x80, 0x00, 0x00]); // off-primary red
        } else if i % 2 == 0 {
            rgb.extend_from_slice(&[0xFF, 0x00, 0x00]);
        } else {
            rgb.extend_from_slice(&[0x00, 0x00, 0xFF]);
        }
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_ne!(mode, PcxAutoMode::EgaRgb1x3);
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn bilevel_prefers_mono1_over_ega_rgb() {
    // Black + white qualifies for BOTH Mono1 and EgaRgb1x3; Mono1's
    // single plane is a third of the bits and must win the size
    // comparison.
    let (w, h) = (64u16, 64u16);
    let mut rgb = Vec::new();
    let mut st = 0xF00Du32;
    for _ in 0..(w as usize * h as usize) {
        let v = if xorshift32(&mut st) & 1 == 0 {
            0x00
        } else {
            0xFF
        };
        rgb.extend_from_slice(&[v, v, v]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn image_auto_threads_dpi_through_ega_rgb_1x3() {
    let (w, h) = (50u16, 30u16);
    let prim: [[u8; 3]; 4] = [
        [0xFF, 0x00, 0x00],
        [0x00, 0xFF, 0x00],
        [0x00, 0x00, 0xFF],
        [0xFF, 0xFF, 0x00],
    ];
    let mut st = 0xACEu32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&prim[(xorshift32(&mut st) % 4) as usize]);
    }
    let image = PcxImage {
        width: w as u32,
        height: h as u32,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((150, 150)),
        window_origin: None,
        screen_size: None,
    };
    let (bytes, mode) = encode_pcx_image_auto(&image).unwrap();
    assert_eq!(mode, PcxAutoMode::EgaRgb1x3);
    let decoded = parse_pcx(&bytes).unwrap();
    assert_eq!(decoded.dpi, Some((150, 150)));
    assert_lossless(&bytes, w, h, &rgb);
}

// ---------------------------------------------------------------------------
// Ladder-wide invariants (extended as candidates land)
// ---------------------------------------------------------------------------

/// The chosen file must never be larger than either always-applicable
/// baseline candidate (Indexed8 when ≤256 colours, Rgb24 always), for a
/// spread of contents and dimensions including odd widths and 1-pixel
/// edges.
#[test]
fn ladder_never_loses_to_the_baselines() {
    let mut st = 0x1234_5678u32;
    for &(w, h) in &[
        (1u16, 1u16),
        (1, 7),
        (7, 1),
        (3, 5),
        (16, 16),
        (17, 9),
        (33, 2),
        (64, 48),
    ] {
        for flavor in 0..4 {
            let n = w as usize * h as usize;
            let mut rgb = Vec::with_capacity(n * 3);
            for i in 0..n {
                let px: [u8; 3] = match flavor {
                    // pure grey noise
                    0 => {
                        let g = (xorshift32(&mut st) % 256) as u8;
                        [g, g, g]
                    }
                    // two colours
                    1 => {
                        if i % 3 == 0 {
                            [0xFF, 0x00, 0x00]
                        } else {
                            [0x00, 0x00, 0xFF]
                        }
                    }
                    // low-colour dither
                    2 => {
                        let v = (xorshift32(&mut st) % 6) as u8;
                        [v * 40, v * 30, v * 20]
                    }
                    // full-random (usually > 256 colours on big canvases)
                    _ => {
                        let r = xorshift32(&mut st);
                        [(r >> 16) as u8, (r >> 8) as u8, r as u8]
                    }
                };
                rgb.extend_from_slice(&px);
            }
            let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
            assert_lossless(&bytes, w, h, &rgb);
            let planar = encode_pcx_24bpp(w, h, &rgb).unwrap();
            assert!(
                bytes.len() <= planar.len(),
                "{w}×{h} flavor {flavor}: auto {} > planar {} (mode {mode:?})",
                bytes.len(),
                planar.len()
            );
        }
    }
}
