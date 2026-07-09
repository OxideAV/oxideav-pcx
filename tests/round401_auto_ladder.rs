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
// Cga2x1 / Cga1x2 candidates (≤ 4 colours, fixed hardware palettes)
// ---------------------------------------------------------------------------

#[test]
fn cga_palette1_high_image_picks_a_cga_form() {
    // Black + the palette-1 bright triple (light cyan / light magenta /
    // white): C/P/I selector 0x60 (white family, bright), background 0.
    // Two bits per pixel must beat the four-bit rungs on a canvas this
    // size.
    let pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let (w, h) = (128u16, 96u16);
    let mut st = 0xC6Au32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 4) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Cga2x1 {
                palette_selector: 0x60,
                background_index: 0,
            } | PcxAutoMode::Cga1x2 {
                palette_selector: 0x60,
                background_index: 0,
            }
        ),
        "unexpected mode {mode:?}"
    );
    assert_lossless(&bytes, w, h, &rgb);
    assert_eq!(bytes[65] as u16 * u16::from(bytes[3]), 2, "2 bits/pixel");
}

#[test]
fn cga_palette0_low_family_is_found() {
    // Green / red / brown are the palette-0 dim triple (C/P/I selector
    // 0x00) — the last CHROMA selector in the scan order, proving the
    // search walks the whole chroma family space. No background colour
    // is needed (all colours sit in the fixed slots) so the first
    // background candidate (0) is kept. Striped content (period 4)
    // keeps every candidate's rows RLE-collapsible so the two-bit CGA
    // geometry's raw-byte advantage decides the contest — 3-colour
    // *noise* would instead drown the advantage in `>= 0xC0` escape
    // bytes and legitimately hand the win to a four-bit planar rung.
    let pal: [[u8; 3]; 3] = [
        [0x00, 0xAA, 0x00], // green
        [0xAA, 0x00, 0x00], // red
        [0xAA, 0x55, 0x00], // brown
    ];
    let (w, h) = (120u16, 90u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[[0usize, 1, 0, 2][i % 4]]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Cga2x1 {
                palette_selector: 0x00,
                background_index: 0,
            } | PcxAutoMode::Cga1x2 {
                palette_selector: 0x00,
                background_index: 0,
            }
        ),
        "unexpected mode {mode:?}"
    );
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn cga_background_search_resolves_off_palette_color() {
    // Blue is in no fixed CGA slot but IS EGA entry 1, so it is only
    // representable as the background colour: the match must land on
    // selector 0x40 (palette 1 low = cyan / magenta / light grey) with
    // background_index 1.
    let pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0xAA], // blue — background only
        [0x00, 0xAA, 0xAA], // cyan
        [0xAA, 0x00, 0xAA], // magenta
        [0xAA, 0xAA, 0xAA], // light grey
    ];
    let (w, h) = (100u16, 60u16);
    let mut st = 0xB16u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 4) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Cga2x1 {
                palette_selector: 0x40,
                background_index: 1,
            } | PcxAutoMode::Cga1x2 {
                palette_selector: 0x40,
                background_index: 1,
            }
        ),
        "unexpected mode {mode:?}"
    );
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn non_cga_four_color_set_skips_the_cga_rungs() {
    // Four colours, one of which no CGA palette + background can
    // produce: the CGA rungs must not fire (the ladder never
    // quantises) and a four-bit header-palette form wins instead.
    let pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0x12, 0x34, 0x56], // representable nowhere in CGA space
    ];
    let (w, h) = (100u16, 60u16);
    let mut st = 0x4C01u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 4) as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Indexed4 { colors: 4 } | PcxAutoMode::Indexed1x4 { colors: 4 }
        ),
        "unexpected mode {mode:?}"
    );
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn five_colors_disqualify_cga() {
    // CGA palettes hold exactly 4 entries; a fifth colour bars the
    // rung even when the first four match a hardware palette.
    let pal: [[u8; 3]; 5] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
        [0x00, 0xAA, 0x00],
    ];
    let (w, h) = (100u16, 60u16);
    let mut st = 0x5C01u32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 5) as usize]);
    }
    let (_bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        !matches!(
            mode,
            PcxAutoMode::Cga2x1 { .. } | PcxAutoMode::Cga1x2 { .. }
        ),
        "5 colours must not fit a CGA palette, got {mode:?}"
    );
}

#[test]
fn cga_beats_the_four_bit_rungs_at_scale() {
    // Same content, direct-writer comparison: the chosen CGA file must
    // be smaller than the 4 bpp packed candidate built from the same
    // first-seen indices (2 bits/pixel vs 4).
    use oxideav_pcx::encode_pcx_4bpp_packed;
    let pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let (w, h) = (160u16, 100u16);
    let mut st = 0xCA5Cu32;
    let mut rgb = Vec::new();
    let mut indices = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        let i = (xorshift32(&mut st) % 4) as u8;
        indices.push(i);
        rgb.extend_from_slice(&pal[i as usize]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(matches!(
        mode,
        PcxAutoMode::Cga2x1 { .. } | PcxAutoMode::Cga1x2 { .. }
    ));
    let mut pal48 = [0u8; 48];
    for (i, c) in pal.iter().enumerate() {
        pal48[i * 3..i * 3 + 3].copy_from_slice(c);
    }
    let packed4 = encode_pcx_4bpp_packed(w, h, &indices, &pal48).unwrap();
    assert!(
        bytes.len() < packed4.len(),
        "CGA ({}) must beat 4 bpp ({}) at 160×100",
        bytes.len(),
        packed4.len()
    );
    assert_lossless(&bytes, w, h, &rgb);
}

#[test]
fn image_auto_threads_dpi_through_cga() {
    let pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let (w, h) = (80u16, 50u16);
    let mut st = 0xD9Au32;
    let mut rgb = Vec::new();
    for _ in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&pal[(xorshift32(&mut st) % 4) as usize]);
    }
    let image = PcxImage {
        width: w as u32,
        height: h as u32,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((96, 96)),
        window_origin: None,
        screen_size: None,
    };
    let (bytes, mode) = encode_pcx_image_auto(&image).unwrap();
    assert!(matches!(
        mode,
        PcxAutoMode::Cga2x1 { .. } | PcxAutoMode::Cga1x2 { .. }
    ));
    let decoded = parse_pcx(&bytes).unwrap();
    assert_eq!(decoded.dpi, Some((96, 96)));
    assert_lossless(&bytes, w, h, &rgb);
}

// ---------------------------------------------------------------------------
// CGA header-offset conformance (manual "CGA Color Map": header bytes 16/19)
// ---------------------------------------------------------------------------

/// Hand-craft a CGA file the way a *foreign, spec-conforming* writer
/// would — background nibble in header byte 16 (the colormap's first
/// byte) and the C / P / I selector in header byte 19 (the colormap's
/// fourth byte) — and assert our decoder resolves the palette from
/// those offsets. Round-trip tests cannot catch an offset slip (a
/// paired encoder+decoder move together); this pins the on-disk
/// contract itself. Regression test for the r401 off-by-16 fix, where
/// both sides read colormap bytes 16 / 19 (header bytes 32 / 35).
#[test]
fn foreign_cga_header_bytes_16_and_19_are_honoured() {
    // 4×1, 2 bpp × 1 plane, indices 0,1,2,3 → one packed byte 0x1B.
    let mut file = vec![0u8; 128];
    file[0] = 0x0A; // manufacturer
    file[1] = 5; // version
    file[2] = 1; // RLE
    file[3] = 2; // bits per pixel
    file[8] = 3; // x_max = 3 → width 4
                 // y_max = 0 → height 1
    file[16] = 0x40; // header byte 16: background = EGA index 4 (red)
    file[19] = 0x60; // header byte 19: C=0, P=1 (white), I=1 (bright)
    file[65] = 1; // n_planes
    file[66] = 2; // bytes_per_line (even)
    file[68] = 1; // palette_info
                  // Pixel payload: 0b00_01_10_11 = 0x1B, then pad byte.
    file.push(0x1B);
    file.push(0x00);
    let img = parse_pcx(&file).unwrap();
    let expected: [[u8; 3]; 4] = [
        [0xAA, 0x00, 0x00], // background = EGA 4 (red)
        [0x55, 0xFF, 0xFF], // light cyan
        [0xFF, 0x55, 0xFF], // light magenta
        [0xFF, 0xFF, 0xFF], // white
    ];
    for (i, want) in expected.iter().enumerate() {
        assert_eq!(
            &img.data[i * 4..i * 4 + 3],
            want,
            "pixel {i}: palette must come from header bytes 16/19"
        );
    }
    // And the same bytes moved 16 deeper (the pre-r401 read positions)
    // must NOT influence decoding: zero 16/19, set 32/35 instead, and
    // the palette must fall back to the all-clear default (yellow-dim,
    // black background) rather than red-background white-bright.
    let mut wrong = file.clone();
    wrong[16] = 0;
    wrong[19] = 0;
    wrong[32] = 0x40;
    wrong[35] = 0x60;
    let img2 = parse_pcx(&wrong).unwrap();
    assert_eq!(
        &img2.data[0..3],
        &[0x00, 0x00, 0x00],
        "colormap bytes 16/19 (header 32/35) must be inert for CGA"
    );
    assert_eq!(&img2.data[4..7], &[0x00, 0xAA, 0x00], "index 1 = green");
}

#[test]
fn cga_mono_ramp_grey_quad_uses_two_bits_per_pixel() {
    // The manual's C bit unlocks two composite-monochrome ramps; the
    // exact dim ramp 0x00/0x55/0xAA/0xFF is therefore CGA-representable
    // and the auto ladder must find it (selector 0x80) — at scale the
    // 2-bit geometry beats every grey alternative (Gray8 is 8 bits,
    // the header-palette forms 4).
    let ramp: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0x55, 0x55],
        [0xAA, 0xAA, 0xAA],
        [0xFF, 0xFF, 0xFF],
    ];
    let (w, h) = (128u16, 64u16);
    let mut rgb = Vec::new();
    for i in 0..(w as usize * h as usize) {
        rgb.extend_from_slice(&ramp[[0usize, 1, 2, 3, 2, 1][i % 6]]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Cga2x1 {
                palette_selector: 0x80,
                ..
            } | PcxAutoMode::Cga1x2 {
                palette_selector: 0x80,
                ..
            }
        ),
        "unexpected mode {mode:?}"
    );
    assert_lossless(&bytes, w, h, &rgb);
}

// ---------------------------------------------------------------------------
// Ladder-wide invariants (extended as candidates land)
// ---------------------------------------------------------------------------

/// Cross-dimensional minimality sweep: for six content flavours whose
/// applicable rungs are known by construction, the ladder's output must
/// (1) round-trip exactly and (2) be no larger than EVERY applicable
/// direct-writer candidate the test rebuilds by hand — mono, EGA RGB,
/// both CGA forms, both 16-colour header-palette forms, grayscale,
/// Indexed8 and planar. Widths cover sub-byte, byte-boundary, odd and
/// even-pad geometries.
#[test]
fn ladder_output_is_minimal_across_dimensions_and_flavors() {
    use oxideav_pcx::{
        encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega,
        encode_pcx_1bpp_mono, encode_pcx_2bpp_cga, encode_pcx_4bpp_packed,
    };
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
    let cga_pal: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let mut st = 0x5EEDu32;
    for &w in &[1u16, 2, 3, 5, 8, 13, 16, 17, 31, 32] {
        for &h in &[1u16, 2, 7, 16] {
            let n = w as usize * h as usize;
            for flavor in 0..6 {
                let mut rgb = Vec::with_capacity(n * 3);
                for i in 0..n {
                    let px: [u8; 3] = match flavor {
                        // bilevel checker
                        0 => {
                            if (i + i / w as usize) % 2 == 0 {
                                [0x00, 0x00, 0x00]
                            } else {
                                [0xFF, 0xFF, 0xFF]
                            }
                        }
                        // all eight EGA primaries, cycling
                        1 => prim[i % 8],
                        // CGA palette-1-high noise
                        2 => cga_pal[(xorshift32(&mut st) % 4) as usize],
                        // grey gradient
                        3 => {
                            let g = ((i * 7) % 0xB0) as u8;
                            [g, g, g]
                        }
                        // 3-colour stripes (plane-periodic)
                        4 => {
                            let pal: [[u8; 3]; 3] = [[10, 20, 30], [200, 30, 30], [30, 200, 30]];
                            pal[[0usize, 1, 0, 2][i % 4]]
                        }
                        // 16-colour cycle
                        _ => {
                            let k = (i % 16) as u8;
                            [13 + k * 11, 29 + k * 7, 47 + k * 5]
                        }
                    };
                    rgb.extend_from_slice(&px);
                }
                let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
                assert_lossless(&bytes, w, h, &rgb);

                // Rebuild first-seen indices + palette the way the scan does.
                let mut pal_seen: Vec<[u8; 3]> = Vec::new();
                let mut indices: Vec<u8> = Vec::new();
                for p in rgb.chunks_exact(3) {
                    let key = [p[0], p[1], p[2]];
                    let idx = match pal_seen.iter().position(|c| *c == key) {
                        Some(i) => i,
                        None => {
                            pal_seen.push(key);
                            pal_seen.len() - 1
                        }
                    };
                    indices.push(idx as u8);
                }
                let colors = pal_seen.len();
                let mut competitors: Vec<(&str, usize)> = Vec::new();
                // Mono.
                if pal_seen
                    .iter()
                    .all(|c| *c == [0, 0, 0] || *c == [255, 255, 255])
                {
                    let mono: Vec<u8> = indices
                        .iter()
                        .map(|&i| u8::from(pal_seen[i as usize] == [255, 255, 255]))
                        .collect();
                    competitors.push(("mono", encode_pcx_1bpp_mono(w, h, &mono).unwrap().len()));
                }
                // EGA RGB primaries.
                if pal_seen
                    .iter()
                    .all(|c| c.iter().all(|&v| v == 0x00 || v == 0xFF))
                {
                    competitors.push((
                        "ega_rgb",
                        encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap().len(),
                    ));
                }
                // CGA (flavor 2 only — palette known by construction).
                if flavor == 2 {
                    let lut: Vec<u8> = pal_seen
                        .iter()
                        .map(|c| cga_pal.iter().position(|p| p == c).unwrap() as u8)
                        .collect();
                    let cga_idx: Vec<u8> = indices.iter().map(|&i| lut[i as usize]).collect();
                    competitors.push((
                        "cga2x1",
                        encode_pcx_2bpp_cga(w, h, &cga_idx, 0x60, 0).unwrap().len(),
                    ));
                    competitors.push((
                        "cga1x2",
                        encode_pcx_1bpp_2planes_cga(w, h, &cga_idx, 0x60, 0)
                            .unwrap()
                            .len(),
                    ));
                }
                // 16-colour header-palette forms.
                if colors <= 16 {
                    let mut pal48 = [0u8; 48];
                    for (i, c) in pal_seen.iter().enumerate() {
                        pal48[i * 3..i * 3 + 3].copy_from_slice(c);
                    }
                    competitors.push((
                        "indexed4",
                        encode_pcx_4bpp_packed(w, h, &indices, &pal48)
                            .unwrap()
                            .len(),
                    ));
                    competitors.push((
                        "indexed1x4",
                        encode_pcx_1bpp_4planes_ega(w, h, &indices, &pal48)
                            .unwrap()
                            .len(),
                    ));
                }
                // Grayscale.
                if pal_seen.iter().all(|c| c[0] == c[1] && c[1] == c[2]) {
                    let gray: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
                    competitors.push((
                        "gray8",
                        encode_pcx_8bpp_grayscale(w, h, &gray).unwrap().len(),
                    ));
                }
                // The two always-applicable baselines.
                let mut pal768 = vec![0u8; 768];
                for (i, c) in pal_seen.iter().enumerate() {
                    pal768[i * 3..i * 3 + 3].copy_from_slice(c);
                }
                competitors.push((
                    "indexed8",
                    encode_pcx_8bpp_indexed(w, h, &indices, &pal768)
                        .unwrap()
                        .len(),
                ));
                competitors.push(("rgb24", encode_pcx_24bpp(w, h, &rgb).unwrap().len()));
                for (name, len) in competitors {
                    assert!(
                        bytes.len() <= len,
                        "{w}×{h} flavor {flavor}: ladder ({} B, {mode:?}) lost to {name} ({len} B)",
                        bytes.len()
                    );
                }
            }
        }
    }
}

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

// ---------------------------------------------------------------------------
// Mono colormap (EGFF 2-colour paletted reading of 1 bpp × 1 plane)
// ---------------------------------------------------------------------------

/// A foreign 1 bpp × 1 plane file may carry a real two-entry colormap
/// (the EGFF canonical mode matrix treats mono as the 2-colour paletted
/// case): the decoder must resolve bit 0 / bit 1 through colormap
/// entries 0 / 1 instead of hard-coding black / white. A zero-filled
/// colormap keeps the classic bit 1 = white convention.
#[test]
fn mono_decoder_honours_a_foreign_two_entry_colormap() {
    use oxideav_pcx::encode_pcx_1bpp_mono;
    let pixels: Vec<u8> = (0..16).map(|i| (i % 2 == 0) as u8).collect();
    let mut bytes = encode_pcx_1bpp_mono(16, 1, &pixels).unwrap();
    // r401 writers store black/white in entries 0/1 — assert that first.
    assert_eq!(&bytes[16..22], &[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..3], &[0xFF, 0xFF, 0xFF], "bit 1 → entry 1");
    assert_eq!(&img.data[4..7], &[0x00, 0x00, 0x00], "bit 0 → entry 0");
    // Foreign palette: white-on-blue.
    bytes[16..19].copy_from_slice(&[0x00, 0x00, 0xAA]); // entry 0 = blue
    bytes[19..22].copy_from_slice(&[0xFF, 0xFF, 0xFF]); // entry 1 = white
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..3], &[0xFF, 0xFF, 0xFF], "bit 1 → white");
    assert_eq!(&img.data[4..7], &[0x00, 0x00, 0xAA], "bit 0 → blue");
    // Zero-filled colormap (the pre-r401 output and the common PCX 3.0+
    // form): classic convention.
    for b in bytes[16..64].iter_mut() {
        *b = 0;
    }
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..3], &[0xFF, 0xFF, 0xFF], "bit 1 = white");
    assert_eq!(&img.data[4..7], &[0x00, 0x00, 0x00], "bit 0 = black");
}
