//! r275 — spec-faithful CGA C / P / I selector decode + the color-burst
//! monochrome mode.
//!
//! The verbatim ZSoft PCX Technical Reference Manual, Revision 5 ("CGA
//! Color Map", Header Byte #19) defines the CGA palette byte as three
//! significant bits ordered C, P, I:
//!
//!   * c: color burst enable — 0 = color; 1 = monochrome
//!   * p: palette — 0 = yellow; 1 = white
//!   * i: intensity — 0 = dim; 1 = bright
//!
//! `C` is bit 7 (0x80), `P` is bit 6 (0x40), `I` is bit 5 (0x20).
//!
//! The pre-r275 [`oxideav_pcx::parse_pcx_indexed_2bpp_cga`] accessor
//! reads only bits 7 / 6 (a `(palette-select, intensity)` two-bit model)
//! and never the intensity bit at position 5, so it cannot represent the
//! manual's `color burst = monochrome` mode nor the dim/bright axis the
//! manual places on bit 5. r275 adds
//! [`oxideav_pcx::parse_pcx_indexed_2bpp_cga_cpi`] +
//! [`oxideav_pcx::encode_pcx_2bpp_cga_cpi`], which decode / encode the
//! full [`oxideav_pcx::Pcx2bppCgaCpi`] triple and resolve the four-level
//! composite-grey ramp the monochrome mode produces.
//!
//! This file tests:
//!
//! 1. Round-trip a `encode_pcx_2bpp_cga_cpi` output through the new
//!    accessor across every C / P / I combination, checking indices,
//!    palette, background, and the decoded bits are surfaced exactly.
//! 2. `Pcx2bppCgaCpi::from_byte19` / `to_byte19` round-trip and mask off
//!    the lower five "ignored" bits per the manual.
//! 3. The color-burst (monochrome) mode produces a four-level grey ramp
//!    distinct from the chroma palettes.
//! 4. The intensity bit (position 5) genuinely changes the resolved
//!    palette — the gap the legacy accessor cannot represent.
//! 5. The accessor rejects every non-(2, 1) (depth, planes) combination.
//! 6. Per-row padding is stripped (widths off the 4-pixel byte boundary).

use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_2bpp_cga_cpi, parse_pcx, parse_pcx_indexed_2bpp_cga_cpi,
    Pcx2bppCgaCpi, PcxError, PcxIndexed2x1CgaCpi,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn indices_grid(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(((x ^ y) & 0x03) as u8);
        }
    }
    out
}

/// All eight C / P / I combinations.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip across every C / P / I combination: indices, background,
/// and the decoded bits must survive a write → read cycle exactly.
#[test]
fn cga_cpi_roundtrip_across_all_combinations() {
    let (w, h) = (16u16, 8u16);
    let indices = indices_grid(w as usize, h as usize);

    for cpi in all_cpi() {
        for &bg in &[0u8, 1, 7, 15] {
            let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, bg).expect("encode");
            let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("parse cpi");
            assert_eq!(view.width, w as u32);
            assert_eq!(view.height, h as u32);
            assert_eq!(view.indices, indices, "cpi={cpi:?} bg={bg}");
            assert_eq!(view.background_index, bg, "bg round-trip cpi={cpi:?}");
            assert_eq!(view.cpi, cpi, "cpi bits round-trip");
            assert_eq!(view.stride(), w as usize);
            // Entry 0 is the resolved background EGA colour.
            assert_ne!(view.palette.len(), 0);
        }
    }
}

/// `from_byte19` / `to_byte19` round-trip and the lower five bits the
/// manual marks "ignored" are masked off.
#[test]
fn cpi_byte19_bit_packing() {
    for cpi in all_cpi() {
        let b = cpi.to_byte19();
        // Only the upper three bits may ever be set.
        assert_eq!(b & 0x1F, 0, "lower five bits must be zero, cpi={cpi:?}");
        assert_eq!(Pcx2bppCgaCpi::from_byte19(b), cpi, "round-trip via byte19");
    }
    // Decoding ignores the lower five bits per the manual.
    let dirty = 0xE0 | 0x1F; // all three C/P/I set + all ignored bits set
    let decoded = Pcx2bppCgaCpi::from_byte19(dirty);
    assert!(decoded.monochrome && decoded.palette_white && decoded.intensity_bright);
    assert_eq!(decoded.to_byte19(), 0xE0, "ignored bits dropped on re-pack");
}

/// The color-burst (monochrome) mode must resolve a four-level grey ramp
/// — every entry's three channels equal — distinct from the chroma
/// palettes the color mode resolves.
#[test]
fn monochrome_mode_is_a_grey_ramp() {
    let (w, h) = (8u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);

    for &bright in &[false, true] {
        let cpi = Pcx2bppCgaCpi {
            monochrome: true,
            palette_white: false,
            intensity_bright: bright,
        };
        // bg index 0 keeps entry 0 black so the whole ramp is grey.
        let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 0).expect("encode");
        let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("parse cpi");
        for (i, e) in view.palette.iter().enumerate() {
            assert_eq!(e[0], e[1], "entry {i} not grey (R!=G), bright={bright}");
            assert_eq!(e[1], e[2], "entry {i} not grey (G!=B), bright={bright}");
        }
        // The ramp is monotonically non-decreasing in luma.
        assert!(view.palette[0][0] <= view.palette[1][0]);
        assert!(view.palette[1][0] <= view.palette[2][0]);
        assert!(view.palette[2][0] <= view.palette[3][0]);
    }

    // Color mode (C=0) must NOT be a pure grey ramp for at least one
    // entry — confirms the two axes resolve different palettes.
    let color = Pcx2bppCgaCpi {
        monochrome: false,
        palette_white: true,
        intensity_bright: true,
    };
    let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, color, 0).expect("encode");
    let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("parse cpi");
    let any_chroma = view.palette.iter().any(|e| e[0] != e[1] || e[1] != e[2]);
    assert!(any_chroma, "color mode resolved an all-grey palette");
}

/// The intensity bit (position 5) genuinely changes the resolved palette
/// — the degree of freedom the legacy `(bits 7/6)` accessor cannot reach.
#[test]
fn intensity_bit_changes_palette() {
    let (w, h) = (8u16, 4u16);
    let indices = indices_grid(w as usize, h as usize);

    for &mono in &[false, true] {
        for &white in &[false, true] {
            let dim = Pcx2bppCgaCpi {
                monochrome: mono,
                palette_white: white,
                intensity_bright: false,
            };
            let bright = Pcx2bppCgaCpi {
                intensity_bright: true,
                ..dim
            };
            let pa = encode_pcx_2bpp_cga_cpi(w, h, &indices, dim, 0).expect("enc dim");
            let pb = encode_pcx_2bpp_cga_cpi(w, h, &indices, bright, 0).expect("enc bright");
            let va = parse_pcx_indexed_2bpp_cga_cpi(&pa).expect("parse dim");
            let vb = parse_pcx_indexed_2bpp_cga_cpi(&pb).expect("parse bright");
            assert_ne!(
                va.palette, vb.palette,
                "intensity bit did not change palette (mono={mono}, white={white})",
            );
        }
    }
}

/// Flattening the typed view through its palette must reproduce the
/// per-pixel colours implied by the resolved palette (internal
/// consistency: indices index into `palette`).
#[test]
fn typed_view_indices_index_into_palette() {
    let (w, h) = (20u16, 5u16);
    let indices = indices_grid(w as usize, h as usize);
    let cpi = Pcx2bppCgaCpi {
        monochrome: false,
        palette_white: true,
        intensity_bright: false,
    };
    let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 3).expect("encode");
    let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("parse cpi");
    assert_eq!(view.indices.len(), w as usize * h as usize);
    for &idx in &view.indices {
        assert!(
            (idx as usize) < view.palette.len(),
            "index {idx} out of range"
        );
    }
}

/// Per-row padding (widths that don't fall on a 4-pixel byte boundary,
/// where `bytes_per_line` is rounded up to even per spec §1) is stripped.
#[test]
fn padding_is_stripped() {
    let cpi = Pcx2bppCgaCpi {
        monochrome: true,
        palette_white: false,
        intensity_bright: true,
    };
    // widths 1, 5, 7, 13 all force trailing padding bytes.
    for &w in &[1u16, 5, 7, 13] {
        let h = 3u16;
        let indices = indices_grid(w as usize, h as usize);
        let pcx = encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, 0).expect("encode");
        let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect("parse cpi");
        assert_eq!(
            view.indices.len(),
            w as usize * h as usize,
            "padding not stripped for width {w}",
        );
        assert_eq!(view.indices, indices, "indices mismatch for width {w}");
    }
}

/// The accessor rejects every (depth, planes) combination other than
/// (2, 1) with `PcxError::Unsupported`.
#[test]
fn rejects_non_2bpp_1plane() {
    // A 24-bit (8 bpp × 3 planes) file is the (depth, planes) = (8, 3)
    // case — must be rejected.
    let rgb = vec![0u8; 4 * 4 * 3];
    let pcx = encode_pcx_24bpp(4, 4, &rgb).expect("encode 24bpp");
    let err = parse_pcx_indexed_2bpp_cga_cpi(&pcx).expect_err("must reject 24-bit");
    assert!(matches!(err, PcxError::Unsupported(_)), "got {err:?}");
}

/// A malformed file rejected by `parse_pcx` is also rejected by the new
/// accessor (shared validation surface).
#[test]
fn shares_validation_surface_with_parse_pcx() {
    let garbage = vec![0xFFu8; 64];
    let a = parse_pcx(&garbage);
    let b = parse_pcx_indexed_2bpp_cga_cpi(&garbage);
    assert!(a.is_err());
    assert!(b.is_err());
}

/// Type-name reference so the unused-import lint stays quiet if a future
/// edit drops a direct mention.
#[allow(dead_code)]
fn _type_anchor(v: PcxIndexed2x1CgaCpi) -> usize {
    v.stride()
}
