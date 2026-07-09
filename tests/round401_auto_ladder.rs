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
    // would quantise). With only two distinct greys the 4 bpp
    // header-palette rung wins the byte count over Gray8 — what matters
    // here is that no bilevel shortcut fires and the round-trip stays
    // exact.
    let (w, h) = (64u16, 64u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        let v = if i % 2 == 0 { 0x01 } else { 0xBF };
        rgb.extend_from_slice(&[v, v, v]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_ne!(mode, PcxAutoMode::Mono1);
    assert_eq!(mode, PcxAutoMode::Indexed4 { colors: 2 });
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
    // All eight primaries so every EGA index bit-plane is busy: a
    // sparser primary set would leave the 16-colour planar rung's top
    // bit-planes all-zero and hand it the byte count instead. Geometry
    // matches eight_primary_image_picks_ega_rgb_1x3 (wide rows) — on a
    // narrow row the noisy 1-bit planes' RLE escape overhead can tip
    // the byte count to the escape-free packed-nibble rung instead.
    let (w, h) = (96u16, 30u16);
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
    let mut st = 0xACEu32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&prim[(xorshift32(&mut st) % 8) as usize]);
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
// Indexed4 / Indexed1x4 candidates (≤ 16 colours, header palette)
// ---------------------------------------------------------------------------

#[test]
fn sixteen_color_image_picks_a_header_palette_form() {
    // Exactly 16 distinct non-grey, non-primary colours: both 4-bit
    // rungs apply, no 8 bpp form can beat half-a-byte-per-pixel plus a
    // free (in-header) palette on a canvas this size. Random noise has
    // no bit-plane periodicity for the planar form to exploit, so the
    // packed-nibble form wins.
    let (w, h) = (100u16, 80u16);
    let mut pal: Vec<[u8; 3]> = Vec::new();
    for i in 0..16u8 {
        pal.push([13 + i * 11, 29 + i * 7, 47 + i * 5]);
    }
    let mut st = 0x16C0_10E5u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 16) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed4 { colors: 16 });
    assert_lossless(&bytes, w, h, &rgb);
    assert_eq!(bytes[3], 4, "4 bpp");
    assert_eq!(bytes[65], 1, "1 plane");
    // The exact palette rides in the 48-byte header Colormap, in
    // first-seen raster order — compare as a set.
    let mut header_pal: Vec<[u8; 3]> = (0..16)
        .map(|i| [bytes[16 + i * 3], bytes[17 + i * 3], bytes[18 + i * 3]])
        .collect();
    let mut want: Vec<[u8; 3]> = pal.clone();
    header_pal.sort_unstable();
    want.sort_unstable();
    assert_eq!(header_pal, want);
}

#[test]
fn seventeen_colors_disqualify_the_header_palette_forms() {
    // One colour over the header palette's capacity: the 4-bit rungs
    // must not fire and the ladder falls back to Indexed8.
    let (w, h) = (100u16, 80u16);
    let mut pal = Vec::new();
    for i in 0..17u8 {
        pal.push([13 + i * 11, 29 + i * 7, 47 + i * 5]);
    }
    let mut st = 0x17C0_10E5u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 17) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 17 });
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn bit_plane_periodic_content_picks_the_planar_form() {
    // Vertical stripes with period 4 make every bit-plane row a repeat
    // of a single byte — the plane-oriented form RLE-collapses to a few
    // packets per row while the packed-nibble form alternates bytes and
    // cannot. The ladder must notice and pick Indexed1x4.
    let (w, h) = (64u16, 64u16);
    let pal: [[u8; 3]; 3] = [[10, 20, 30], [200, 30, 30], [30, 200, 30]];
    let mut rgb = Vec::new();
    for _y in 0..h as usize {
        for x in 0..w as usize {
            let idx = [0usize, 1, 0, 2][x % 4];
            rgb.extend_from_slice(&pal[idx]);
        }
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed1x4 { colors: 3 });
    assert_lossless(&bytes, w, h, &rgb);
    assert_eq!(bytes[3], 1, "1 bpp");
    assert_eq!(bytes[65], 4, "4 planes");
}

#[test]
fn all_black_palette48_survives_the_hardware_substitution_corner() {
    // Single distinct colour = pure black → the written 48-byte header
    // palette is all zeros, which the 16-colour decode paths replace
    // with the standard EGA hardware palette (spec table §3.1). Entry 0
    // of that palette is also pure black, so the round-trip must stay
    // exact. (Mono1 wins the ladder for all-black content, so pin the
    // corner through the direct writers instead.)
    use oxideav_pcx::{encode_pcx_1bpp_4planes_ega, encode_pcx_4bpp_packed};
    let (w, h) = (16u16, 16u16);
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let indices = vec![0u8; w as usize * h as usize];
    let pal48 = [0u8; 48];
    for bytes in [
        encode_pcx_4bpp_packed(w, h, &indices, &pal48).unwrap(),
        encode_pcx_1bpp_4planes_ega(w, h, &indices, &pal48).unwrap(),
    ] {
        assert_lossless(&bytes, w, h, &rgb);
    }
    // And the ladder itself still round-trips all-black exactly
    // (whatever rung wins).
    let (bytes, _mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn image_auto_threads_dpi_through_indexed4_and_indexed1x4() {
    // 16-colour noise (every index bit-plane busy) → Indexed4; striped
    // low-colour input → Indexed1x4. Both must carry the authoring DPI
    // through the re-emit.
    let (w, h) = (60u16, 40u16);
    let mut pal16: Vec<[u8; 3]> = Vec::new();
    for i in 0..16u8 {
        pal16.push([17 + i * 9, 33 + i * 6, 51 + i * 4]);
    }
    let stripe_pal: [[u8; 3]; 3] = [[10, 20, 30], [200, 30, 30], [30, 200, 30]];
    let mut st = 0xD1D1u32;
    let mut noise = Vec::new();
    let mut stripes = Vec::new();
    for i in 0..(w as usize * h as usize) {
        noise.extend_from_slice(&pal16[(xorshift32(&mut st) % 16) as usize]);
        stripes.extend_from_slice(&stripe_pal[[0usize, 1, 0, 2][i % 4]]);
    }
    for (data, want_planes) in [(noise, 1u8), (stripes, 4u8)] {
        let image = PcxImage {
            width: w as u32,
            height: h as u32,
            pixel_format: PcxPixelFormat::Rgb24,
            data: data.clone(),
            pts: None,
            dpi: Some((300, 600)),
            window_origin: None,
            screen_size: None,
        };
        let (bytes, mode) = encode_pcx_image_auto(&image).unwrap();
        assert!(
            matches!(
                mode,
                PcxAutoMode::Indexed4 { .. } | PcxAutoMode::Indexed1x4 { .. }
            ),
            "unexpected mode {mode:?}"
        );
        assert_eq!(bytes[65], want_planes, "plane geometry");
        let decoded = parse_pcx(&bytes).unwrap();
        assert_eq!(decoded.dpi, Some((300, 600)));
        assert_lossless(&bytes, w, h, &data);
    }
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
