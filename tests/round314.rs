//! r314 — spec-faithful CGA *flatten*-to-RGBA entry point
//! [`oxideav_pcx::parse_pcx_cga_cpi`].
//!
//! The verbatim ZSoft PCX Technical Reference Manual, Revision 5 ("CGA
//! Color Map", Header Byte #19) defines the CGA palette byte as three
//! significant bits ordered C, P, I (`C` = bit 7 / color burst, `P` = bit
//! 6 / palette family, `I` = bit 5 / intensity). The standard
//! [`oxideav_pcx::parse_pcx`] flatten path resolves the two CGA layouts
//! through the legacy `(palette-select, intensity)` two-bit model of byte
//! 19 (bits 7 / 6), so it cannot represent the manual's
//! `color burst = monochrome` mode (`C = 1`) and places the intensity bit
//! at position 6 instead of 5.
//!
//! r275 added the spec-faithful *typed* accessor
//! [`oxideav_pcx::parse_pcx_indexed_2bpp_cga_cpi`]; r314 adds the matching
//! *flatten*-to-`Rgba` entry point so a display pipeline that wants packed
//! pixels (not indices) also gets the spec-correct C / P / I resolution,
//! including the four-level composite-grey monochrome ramp. It covers BOTH
//! on-disk CGA layouts — `(2, 1)` packed and `(1, 2)` planar.
//!
//! This file tests:
//!
//! 1. The flatten output equals the typed accessor's indices indexed into
//!    the typed accessor's resolved palette (the two spec-faithful paths
//!    agree) across every C / P / I combination.
//! 2. The color-burst monochrome mode flattens to a grey ramp (the mode
//!    the legacy `parse_pcx` flatten path mis-colours).
//! 3. Both CGA on-disk layouts ((2, 1) packed and (1, 2) planar) flatten
//!    to identical pixels for identical indices + header bytes.
//! 4. Non-CGA (depth, planes) combinations are rejected.
//! 5. The authoring-metadata fields (dpi / window / screen) are surfaced
//!    identically to `parse_pcx`.

use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_24bpp, encode_pcx_24bpp_window_dpi_screen,
    encode_pcx_2bpp_cga_cpi, parse_pcx, parse_pcx_cga_cpi, parse_pcx_indexed_2bpp_cga_cpi,
    Pcx2bppCgaCpi, PcxError,
};

fn indices_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x ^ y) & 0x03) as u8);
        }
    }
    out
}

fn all_cpi() -> Vec<Pcx2bppCgaCpi> {
    let mut v = Vec::with_capacity(8);
    for &c in &[false, true] {
        for &p in &[false, true] {
            for &i in &[false, true] {
                v.push(Pcx2bppCgaCpi {
                    monochrome: c,
                    palette_white: p,
                    intensity_bright: i,
                });
            }
        }
    }
    v
}

/// The flatten path must reproduce, pixel for pixel, the typed accessor's
/// `palette[index]` for every C / P / I combination and background — the
/// two spec-faithful CGA paths must agree.
#[test]
fn flatten_agrees_with_typed_accessor_all_cpi() {
    let (w, h) = (16u16, 8u16);
    let indices = indices_grid(w as usize, h as usize);

    for cpi in all_cpi() {
        for &bg in &[0u8, 1, 7, 15] {
            let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, bg).expect("encode");
            let typed = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("typed");
            let flat = parse_pcx_cga_cpi(&pcx).expect("flatten");
            assert_eq!(flat.width, w as u32);
            assert_eq!(flat.height, h as u32);
            assert_eq!(flat.data.len(), w as usize * h as usize * 4);
            for (i, &idx) in typed.indices.iter().enumerate() {
                let p = typed.palette[idx as usize];
                let want = [p[0], p[1], p[2], 0xFF];
                assert_eq!(
                    flat.data[i * 4..i * 4 + 4],
                    want,
                    "pixel {i} mismatch cpi={cpi:?} bg={bg}"
                );
            }
        }
    }
}

/// The color-burst monochrome mode (`C = 1`) — which the legacy
/// `parse_pcx` flatten path cannot represent — flattens to a grey ramp
/// (R == G == B for every non-background pixel).
#[test]
fn monochrome_flatten_is_grey() {
    let (w, h) = (8u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);
    for &bright in &[false, true] {
        let cpi = Pcx2bppCgaCpi {
            monochrome: true,
            palette_white: false,
            intensity_bright: bright,
        };
        // bg index 0 → entry 0 black, so the whole image is grey.
        let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 0).expect("encode");
        let flat = parse_pcx_cga_cpi(&pcx).expect("flatten");
        for px in flat.data.chunks_exact(4) {
            assert_eq!(px[0], px[1], "R != G (bright={bright})");
            assert_eq!(px[1], px[2], "G != B (bright={bright})");
            assert_eq!(px[3], 0xFF, "alpha not opaque");
        }
    }
}

/// The legacy `parse_pcx` flatten path mis-colours a monochrome-CGA file
/// (it lacks the `C` axis); the new spec-faithful path produces grey.
/// This documents the concrete behavioural improvement.
#[test]
fn spec_faithful_path_differs_from_legacy_on_monochrome() {
    let (w, h) = (8u16, 2u16);
    // Use indices that span 1..=3 so the chroma palette would show colour.
    let indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (1 + (i % 3)) as u8)
        .collect();
    let cpi = Pcx2bppCgaCpi {
        monochrome: true,
        palette_white: false,
        intensity_bright: false,
    };
    let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 0).expect("encode");

    let legacy = parse_pcx(&pcx).expect("legacy flatten");
    let faithful = parse_pcx_cga_cpi(&pcx).expect("faithful flatten");

    // The spec-faithful path is a pure grey ramp.
    for px in faithful.data.chunks_exact(4) {
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
    }
    // The legacy path resolves a chroma palette for at least one pixel
    // (R != G or G != B) — i.e. it genuinely mis-colours this file, which
    // is exactly the gap the new path closes.
    let legacy_has_chroma = legacy
        .data
        .chunks_exact(4)
        .any(|px| px[0] != px[1] || px[1] != px[2]);
    assert!(
        legacy_has_chroma,
        "legacy path unexpectedly grey — test premise invalid"
    );
}

/// Both CGA on-disk layouts — `(2, 1)` packed and `(1, 2)` planar — must
/// flatten to identical pixels for identical indices + identical header
/// palette bytes through the spec-faithful path.
#[test]
fn packed_and_planar_layouts_agree() {
    let (w, h) = (12u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);
    let cpi = Pcx2bppCgaCpi {
        monochrome: false,
        palette_white: true,
        intensity_bright: true,
    };
    let bg = 5u8;
    let selector = cpi.to_byte19();

    let packed = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, bg).expect("packed encode");
    // The 1 bpp × 2 planes writer takes a raw selector byte; feeding it the
    // CPI-derived byte 19 makes both files carry identical palette bytes.
    let planar = encode_pcx_1bpp_2planes_cga(w, h, &indices, selector, bg).expect("planar encode");

    let fp = parse_pcx_cga_cpi(&packed).expect("flatten packed");
    let fl = parse_pcx_cga_cpi(&planar).expect("flatten planar");
    assert_eq!(fp.width, fl.width);
    assert_eq!(fp.height, fl.height);
    assert_eq!(fp.data, fl.data, "packed and planar CGA flatten differ");
}

/// Non-CGA (depth, planes) combinations are rejected with
/// `PcxError::Unsupported`.
#[test]
fn rejects_non_cga_modes() {
    // 24-bit (8, 3).
    let rgb = vec![0u8; 4 * 4 * 3];
    let pcx = encode_pcx_24bpp(4, 4, &rgb).expect("encode 24bpp");
    let err = parse_pcx_cga_cpi(&pcx).expect_err("must reject 24-bit");
    assert!(matches!(err, PcxError::Unsupported(_)), "got {err:?}");
}

/// A malformed file rejected by `parse_pcx` is also rejected here (shared
/// validation surface via `decode_planar_scanlines`).
#[test]
fn shares_validation_surface() {
    let garbage = vec![0xFFu8; 64];
    assert!(parse_pcx(&garbage).is_err());
    assert!(parse_pcx_cga_cpi(&garbage).is_err());
}

/// Authoring metadata (dpi / window / screen) is surfaced identically to
/// `parse_pcx`. We can't tag a CGA file directly, but the field-extraction
/// logic is shared; verify a fully-tagged 24-bit file and a plain CGA file
/// both surface the same shape `parse_pcx` does for their respective
/// headers (CGA defaults to all-None).
#[test]
fn metadata_matches_parse_pcx_for_cga() {
    let (w, h) = (8u16, 2u16);
    let indices = indices_grid(w as usize, h as usize);
    let cpi = Pcx2bppCgaCpi {
        monochrome: false,
        palette_white: false,
        intensity_bright: false,
    };
    let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 0).expect("encode");
    let flat = parse_pcx_cga_cpi(&pcx).expect("flatten");
    // The plain CGA writer leaves dpi at the historical 72×72 default and
    // origin/screen at zero; surface shape matches `parse_pcx`.
    let legacy = parse_pcx(&pcx).expect("legacy");
    assert_eq!(flat.dpi, legacy.dpi);
    assert_eq!(flat.window_origin, legacy.window_origin);
    assert_eq!(flat.screen_size, legacy.screen_size);

    // Sanity: the metadata helper does surface non-None when a tagged file
    // carries the fields (24-bit path, exercising the same helper).
    let rgb = vec![0u8; w as usize * h as usize * 3];
    let tagged = encode_pcx_24bpp_window_dpi_screen(2, 3, w, h, &rgb, (300, 300), (640, 480))
        .expect("tagged");
    let tv = parse_pcx(&tagged).expect("tagged decode");
    assert_eq!(tv.dpi, Some((300, 300)));
    assert_eq!(tv.window_origin, Some((2, 3)));
    assert_eq!(tv.screen_size, Some((640, 480)));
}
